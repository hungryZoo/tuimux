//! Default tuimux ratatui interface.
//!
//! The main pane is backed by tuimux's Rust-native daemon multiplexer:
//! sessions, windows, and panes are owned by the daemon, and each pane runs a
//! real shell in a PTY rendered through a vt100 screen model.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::clipboard;
use crate::mux_backend::{KeyInput, MouseInput, MuxBackend, MuxSnapshot, PaneSnapshot};
use crate::native_mux::{Pane, PaneAxis, PaneRect, PaneSeparator, Session, Window};
use crate::terminal::{SelectionRange, TerminalColor, TerminalSpan, TerminalStyle};

/// Why the UI loop ended — affects the farewell message.
enum Exit {
    Quit,
    Detach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hotspot {
    SessionButton,
    DetachButton,
    MainPane,
    Window(usize),
    WindowClose(usize),
    NewWindow,
    ModalSession(usize),
    ModalNewSession,
    ModalDetach,
}

#[derive(Default, Clone, Copy)]
struct Regions {
    main_pane: Rect,
    terminal_body: Rect,
    session_button: Rect,
    detach_button: Rect,
    new_window: Rect,
    windows: [Rect; 8],
    window_close: [Rect; 8],
    window_count: usize,
    terminal_panes: [Rect; 8],
    terminal_pane_count: usize,
    modal_new_session: Rect,
    modal_detach: Rect,
    modal_sessions: [Rect; 8],
    modal_session_count: usize,
}

struct UiState {
    session_modal_open: bool,
    hover: Option<Hotspot>,
    regions: Regions,
    mux: MuxBackend,
    /// Live native mux state, refreshed after every mutating command.
    sessions: Vec<Session>,
    windows: Vec<Window>,
    panes: Vec<Pane>,
    current_session: String,
    /// Non-fatal, transient message shown in the status bar (e.g. that a
    /// window was created, selected, or closed).
    status: Option<String>,
    terminal_error: Option<String>,
    terminal_mode: bool,
    selection: Option<SelectionState>,
    terminal_axis: PaneAxis,
    terminal_separators: Vec<PaneSeparator>,
    terminal_panes: Vec<PaneSnapshot>,
    terminal_rows: Vec<Vec<TerminalSpan>>,
    terminal_cursor: Option<(u16, u16)>,
    terminal_hide_cursor: bool,
    terminal_mouse_protocol_active: bool,
    terminal_scrollback: usize,
}

#[derive(Debug, Clone, Copy)]
struct SelectionState {
    pane: usize,
    anchor: (u16, u16),
    focus: (u16, u16),
    dragging: bool,
}

impl UiState {
    /// Build initial state from the native multiplexer.
    fn bootstrap(initial_session: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let mux = MuxBackend::new(initial_session, cwd, 80, 24)?;
        let mut state = UiState {
            // Native tuimux starts focused in the shell. The Session button or
            // Alt-S opens the session switcher when needed.
            session_modal_open: false,
            hover: None,
            regions: Regions::default(),
            mux,
            sessions: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
            current_session: String::new(),
            status: None,
            terminal_error: None,
            terminal_mode: true,
            selection: None,
            terminal_axis: PaneAxis::default(),
            terminal_separators: Vec::new(),
            terminal_panes: Vec::new(),
            terminal_rows: Vec::new(),
            terminal_cursor: None,
            terminal_hide_cursor: true,
            terminal_mouse_protocol_active: false,
            terminal_scrollback: 0,
        };
        let snapshot = state.mux.snapshot(80, 24, None)?;
        state.apply_snapshot(snapshot);
        Ok(state)
    }

    fn sync_terminal(&mut self, width: u16, height: u16) {
        let size = (width.max(1), height.max(1));
        match self.mux.snapshot(size.0, size.1, self.selection_range()) {
            Ok(snapshot) => {
                self.terminal_error = None;
                self.apply_snapshot(snapshot);
            }
            Err(e) => {
                self.terminal_error = Some(format!("native mux backend failed: {e}"));
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: MuxSnapshot) {
        self.sessions = snapshot.sessions;
        self.windows = snapshot.windows;
        self.panes = snapshot.panes;
        self.current_session = snapshot.current_session;
        self.terminal_axis = snapshot.terminal.axis;
        self.terminal_separators = snapshot.terminal.separators;
        self.terminal_panes = snapshot.terminal.panes;
        self.terminal_rows = snapshot.terminal.rows;
        self.terminal_cursor = snapshot.terminal.cursor;
        self.terminal_hide_cursor = snapshot.terminal.hide_cursor;
        self.terminal_mouse_protocol_active = snapshot.terminal.mouse_protocol_active;
        self.terminal_scrollback = snapshot.terminal.scrollback;
    }

    fn selection_range(&self) -> Option<SelectionRange> {
        let selection = self.selection?;
        (selection.anchor != selection.focus).then_some(SelectionRange::new(
            selection.anchor.0,
            selection.anchor.1,
            selection.focus.0,
            selection.focus.1,
        ))
    }

    fn begin_selection(&mut self, pane: usize, row: u16, col: u16) {
        self.selection = Some(SelectionState {
            pane,
            anchor: (row, col),
            focus: (row, col),
            dragging: false,
        });
    }

    fn update_selection(&mut self, row: u16, col: u16) {
        if let Some(selection) = &mut self.selection {
            selection.focus = (row, col);
            selection.dragging = true;
        }
    }

    fn finish_selection(&mut self, row: u16, col: u16) {
        self.update_selection(row, col);
        if self.selection_range().is_none() {
            self.selection = None;
        } else if let Some(selection) = &mut self.selection {
            selection.dragging = false;
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
    }

    fn copy_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        let Ok(text) = self.mux.selected_text(range) else {
            self.status = Some("copy failed: terminal is not ready".to_string());
            return false;
        };
        if text.is_empty() {
            return false;
        }
        match clipboard::copy_text(&text) {
            Ok(()) => {
                self.status = Some(format!("copied {} chars", text.chars().count()));
                true
            }
            Err(e) => {
                self.status = Some(format!("copy failed: {e}"));
                false
            }
        }
    }

    fn terminal_mouse_protocol_active(&self) -> bool {
        self.terminal_mouse_protocol_active
    }

    fn pane_mouse_protocol_active(&self, row: usize) -> bool {
        self.terminal_panes
            .get(row)
            .map(|pane| pane.mouse_protocol_active)
            .unwrap_or_else(|| self.terminal_mouse_protocol_active())
    }

    fn send_terminal_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c')
            && key.modifiers == KeyModifiers::CONTROL
            && self.copy_selection()
        {
            return;
        }

        if self.selection.is_some() {
            self.clear_selection();
        }

        if let Some(key) = KeyInput::from_event(key) {
            if let Err(e) = self.mux.send_key(key) {
                self.status = Some(format!("terminal input failed: {e}"));
            }
        }
    }

    fn send_terminal_paste(&mut self, text: &str) {
        self.clear_selection();
        if let Err(e) = self.mux.send_paste(text) {
            self.status = Some(format!("terminal paste failed: {e}"));
        }
    }

    fn send_terminal_mouse(
        &mut self,
        kind: MouseEventKind,
        row: u16,
        col: u16,
        modifiers: KeyModifiers,
    ) {
        if let Some(mouse) = MouseInput::from_parts(kind, row, col, modifiers) {
            if let Err(e) = self.mux.send_mouse(mouse) {
                self.status = Some(format!("terminal mouse failed: {e}"));
            }
        }
    }
}

/// Entry point for the default run. Returns a process exit code.
pub fn run(initial_session: &str, cwd: PathBuf) -> io::Result<i32> {
    if !io::stdout().is_terminal() {
        eprintln!(
            "tuimux: stdout is not a terminal — refusing to start the interactive UI.\n\
             Try one of:\n  tuimux --layout-preview   # render the layout as text\n  \
             tuimux --doctor           # check your environment"
        );
        return Ok(2);
    }

    let mut state = UiState::bootstrap(initial_session, cwd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    let mut terminal = setup()?;
    let result = run_loop(&mut terminal, &mut state);
    restore(&mut terminal)?;

    match result {
        Ok(Exit::Quit) => {
            println!("tuimux: exited.");
            Ok(0)
        }
        Ok(Exit::Detach) => {
            println!("tuimux: closed native multiplexer UI.");
            Ok(0)
        }
        Err(e) => Err(e),
    }
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup() -> io::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore(terminal: &mut Term) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()
}

fn run_loop(terminal: &mut Term, state: &mut UiState) -> io::Result<Exit> {
    loop {
        let body = state.regions.terminal_body;
        if body.width > 0 && body.height > 0 {
            state.sync_terminal(body.width, body.height);
        }

        terminal.draw(|f| ui(f, state))?;

        if !event::poll(Duration::from_millis(33))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match (key.code, key.modifiers) {
                    (KeyCode::F(12), _) => {
                        state.terminal_mode = !state.terminal_mode;
                        state.status = Some(
                            if state.terminal_mode {
                                "terminal mode"
                            } else {
                                "navigation mode"
                            }
                            .to_string(),
                        );
                    }
                    _ if state.terminal_mode => {
                        state.send_terminal_key(key);
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                    (KeyCode::Char('q'), _) => return Ok(Exit::Quit),
                    // `n` creates a fresh detached `tuimux-<n>` session, but only
                    // while the modal is open so it can't fire by accident.
                    (KeyCode::Char('n'), _) if state.session_modal_open => {
                        new_session(state);
                    }
                    (KeyCode::Esc, _) if state.session_modal_open => {
                        state.session_modal_open = false;
                    }
                    (KeyCode::Esc, _) => return Ok(Exit::Quit),
                    (KeyCode::Char('s'), KeyModifiers::ALT) => {
                        state.session_modal_open = !state.session_modal_open;
                    }
                    (KeyCode::Char('n'), _) => {
                        new_window(state);
                    }
                    (KeyCode::Char('|'), _)
                    | (KeyCode::Char('v'), _)
                    | (KeyCode::Char('-'), _)
                    | (KeyCode::Char('h'), _) => {
                        deprecated_split_pane(state);
                    }
                    (KeyCode::Tab, _) => {
                        select_adjacent_window(state, 1);
                    }
                    (KeyCode::PageUp, _) => {
                        scroll_active_pane(state, scroll_page_lines(state));
                    }
                    (KeyCode::PageDown, _) => {
                        scroll_active_pane(state, -scroll_page_lines(state));
                    }
                    (KeyCode::Home, _) => {
                        scroll_active_pane(state, 1_000_000);
                    }
                    (KeyCode::End, _) => {
                        scroll_active_pane(state, 0);
                    }
                    (KeyCode::Left, _) => {
                        select_adjacent_window(state, -1);
                    }
                    (KeyCode::Right, _) => {
                        select_adjacent_window(state, 1);
                    }
                    (KeyCode::Up, _) => {
                        select_adjacent_window(state, -1);
                    }
                    (KeyCode::Down, _) => {
                        select_adjacent_window(state, 1);
                    }
                    (KeyCode::Char('x'), _) => {
                        kill_active_window(state);
                    }
                    (KeyCode::Char('d'), _) => {
                        return Ok(Exit::Detach);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                if let Some(selection) = state.selection {
                    match mouse.kind {
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some((row, col)) = terminal_cell_for_pane(
                                mouse.column,
                                mouse.row,
                                &state.regions,
                                selection.pane,
                            ) {
                                state.update_selection(row, col);
                                continue;
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if let Some((row, col)) = terminal_cell_for_pane(
                                mouse.column,
                                mouse.row,
                                &state.regions,
                                selection.pane,
                            ) {
                                state.finish_selection(row, col);
                                continue;
                            }
                        }
                        _ => {}
                    }
                }

                if let Some((pane_row, row, col)) =
                    terminal_cell_at_pane(mouse.column, mouse.row, &state.regions)
                {
                    let clicked_left =
                        matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
                    if clicked_left
                        && !state
                            .panes
                            .get(pane_row)
                            .map(|pane| pane.active)
                            .unwrap_or(false)
                    {
                        select_pane(state, pane_row);
                    }

                    let child_wants_mouse = state.pane_mouse_protocol_active(pane_row);
                    let selection_gesture =
                        mouse.modifiers.contains(KeyModifiers::SHIFT) || !child_wants_mouse;

                    if !child_wants_mouse {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                if !state
                                    .panes
                                    .get(pane_row)
                                    .map(|pane| pane.active)
                                    .unwrap_or(false)
                                {
                                    select_pane(state, pane_row);
                                }
                                scroll_active_pane(state, 3);
                                continue;
                            }
                            MouseEventKind::ScrollDown => {
                                if !state
                                    .panes
                                    .get(pane_row)
                                    .map(|pane| pane.active)
                                    .unwrap_or(false)
                                {
                                    select_pane(state, pane_row);
                                }
                                scroll_active_pane(state, -3);
                                continue;
                            }
                            _ => {}
                        }
                    }

                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) if selection_gesture => {
                            state.terminal_mode = true;
                            state.begin_selection(pane_row, row, col);
                            continue;
                        }
                        _ if state.terminal_mode => {
                            state.send_terminal_mouse(mouse.kind, row, col, mouse.modifiers);
                            continue;
                        }
                        _ => {}
                    }
                }

                if state.terminal_mode {
                    if let Some((_pane_row, row, col)) =
                        terminal_cell_at_pane(mouse.column, mouse.row, &state.regions)
                    {
                        state.send_terminal_mouse(mouse.kind, row, col, mouse.modifiers);
                        continue;
                    }
                }

                state.hover = hit_test(
                    mouse.column,
                    mouse.row,
                    &state.regions,
                    state.session_modal_open,
                );
                if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                    match state.hover {
                        Some(Hotspot::SessionButton) => {
                            state.session_modal_open = !state.session_modal_open;
                        }
                        Some(Hotspot::DetachButton) | Some(Hotspot::ModalDetach) => {
                            return Ok(Exit::Detach);
                        }
                        Some(Hotspot::MainPane) => {
                            state.terminal_mode = true;
                            state.status =
                                Some("terminal mode (F12 returns to navigation)".to_string());
                        }
                        Some(Hotspot::WindowClose(idx)) => {
                            kill_window(state, idx);
                        }
                        Some(Hotspot::Window(idx)) => {
                            select_window(state, idx);
                        }
                        Some(Hotspot::NewWindow) => {
                            new_window(state);
                        }
                        Some(Hotspot::ModalNewSession) => {
                            new_session(state);
                        }
                        Some(Hotspot::ModalSession(idx)) => {
                            switch_session(state, idx);
                        }
                        _ => {}
                    }
                }
            }
            Event::Paste(text) if state.terminal_mode => {
                state.send_terminal_paste(&text);
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn active_terminal_size(state: &UiState) -> (u16, u16) {
    let body = state.regions.terminal_body;
    (body.width.max(1), body.height.max(1))
}

fn select_window(state: &mut UiState, idx: usize) {
    match state.mux.select_window_by_row(idx) {
        Ok(()) => {
            state.clear_selection();
            let (width, height) = active_terminal_size(state);
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("select-window failed: {e}")),
    }
}

fn select_adjacent_window(state: &mut UiState, delta: i32) {
    let count = state.windows.len();
    if count == 0 {
        state.status = Some("no windows".to_string());
        return;
    }
    if count == 1 {
        state.status = Some("only one window".to_string());
        return;
    }

    let active = state
        .windows
        .iter()
        .position(|window| window.active)
        .unwrap_or(0) as i32;
    let next = (active + delta).rem_euclid(count as i32) as usize;
    select_window(state, next);
}

fn kill_window(state: &mut UiState, idx: usize) {
    let (width, height) = active_terminal_size(state);
    match state.mux.kill_window_by_row(idx, width, height) {
        Ok(index) => {
            state.status = Some(format!("killed window {index}"));
            state.clear_selection();
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("kill-window failed: {e}")),
    }
}

fn kill_active_window(state: &mut UiState) {
    let Some(active) = state.windows.iter().position(|window| window.active) else {
        state.status = Some("no active window".to_string());
        return;
    };
    kill_window(state, active);
}

fn select_pane(state: &mut UiState, idx: usize) {
    match state.mux.select_pane_by_row(idx) {
        Ok(()) => {
            state.clear_selection();
            let (width, height) = active_terminal_size(state);
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("select-pane failed: {e}")),
    }
}

fn deprecated_split_pane(state: &mut UiState) {
    state.status = Some("split panes are deprecated; use windows".to_string());
}

fn scroll_page_lines(state: &UiState) -> i32 {
    state
        .terminal_panes
        .iter()
        .find(|pane| pane.active)
        .map(|pane| pane.rect.height)
        .unwrap_or_else(|| state.regions.terminal_body.height)
        .saturating_sub(1)
        .max(1) as i32
}

fn scroll_active_pane(state: &mut UiState, lines: i32) {
    match state.mux.scroll_active_pane(lines) {
        Ok(scrollback) => {
            state.clear_selection();
            state.status = Some(if scrollback == 0 {
                "scrollback bottom".to_string()
            } else {
                format!("scrollback {scrollback} rows")
            });
            let (width, height) = active_terminal_size(state);
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("scrollback failed: {e}")),
    }
}

fn new_window(state: &mut UiState) {
    let (width, height) = active_terminal_size(state);
    match state.mux.new_window(width, height) {
        Ok(index) => {
            state.status = Some(format!(
                "created window {index} in '{}'",
                state.current_session
            ));
            state.clear_selection();
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("new-window failed: {e}")),
    }
}

fn new_session(state: &mut UiState) {
    let (width, height) = active_terminal_size(state);
    match state.mux.create_next_session(width, height) {
        Ok(name) => {
            state.status = Some(format!("created session '{name}'"));
            state.clear_selection();
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("new-session failed: {e}")),
    }
}

fn switch_session(state: &mut UiState, idx: usize) {
    match state.mux.switch_session_by_row(idx) {
        Ok(()) => {
            state.session_modal_open = false;
            state.clear_selection();
            let (width, height) = active_terminal_size(state);
            state.sync_terminal(width, height);
            state.status = Some(format!("selected session '{}'", state.current_session));
        }
        Err(e) => state.status = Some(format!("select-session failed: {e}")),
    }
}

fn ui(f: &mut Frame, state: &mut UiState) {
    let root = f.size();
    let terminal_axis = state.terminal_axis;
    let terminal_separators = state.terminal_separators.clone();
    let terminal_panes = state.terminal_panes.clone();
    let terminal_rows = state.terminal_rows.clone();
    let terminal_cursor = state.terminal_cursor;
    let terminal_hide_cursor = state.terminal_hide_cursor;

    let session_label = if state.current_session.is_empty() {
        "(no session)"
    } else {
        &state.current_session
    };

    if state.terminal_mode && !state.session_modal_open {
        render_main(
            f,
            root,
            terminal_axis,
            terminal_separators,
            terminal_panes,
            terminal_rows,
            state.terminal_mode,
            !terminal_hide_cursor,
            terminal_cursor,
            state.terminal_error.as_deref(),
            &mut state.regions,
            false,
        );
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(26)])
        .split(root);

    render_main(
        f,
        body[0],
        terminal_axis,
        terminal_separators,
        terminal_panes,
        terminal_rows,
        state.terminal_mode,
        !state.session_modal_open && !terminal_hide_cursor,
        terminal_cursor,
        state.terminal_error.as_deref(),
        &mut state.regions,
        true,
    );
    render_sidebar(
        f,
        body[1],
        session_label,
        &state.windows,
        state.status.as_deref(),
        state.hover,
        &mut state.regions,
    );

    if state.session_modal_open {
        render_session_modal(
            f,
            &state.sessions,
            &state.current_session,
            state.hover,
            &mut state.regions,
        );
    }
}

fn button_block<'a>(title: Option<&'a str>, color: Color, hot: bool) -> Block<'a> {
    let style = if hot {
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    };
    let block = Block::default().borders(Borders::ALL).border_style(style);
    if let Some(title) = title {
        block.title(Span::styled(format!(" {title} "), style))
    } else {
        block
    }
}

fn render_main(
    f: &mut Frame,
    area: Rect,
    terminal_axis: PaneAxis,
    terminal_separators: Vec<PaneSeparator>,
    terminal_panes: Vec<PaneSnapshot>,
    terminal_rows: Vec<Vec<TerminalSpan>>,
    terminal_mode: bool,
    show_cursor: bool,
    terminal_cursor: Option<(u16, u16)>,
    terminal_error: Option<&str>,
    regions: &mut Regions,
    chrome: bool,
) {
    regions.main_pane = area;
    let inner = if chrome {
        let border = if terminal_mode {
            Color::Green
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border));
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    } else {
        area
    };
    regions.terminal_body = inner;
    regions.terminal_pane_count = 0;

    if terminal_panes.len() > 1 {
        render_terminal_panes(
            f,
            inner,
            terminal_axis,
            terminal_separators,
            terminal_panes,
            show_cursor,
            regions,
        );
        return;
    }

    let lines: Vec<Line> = if let Some(error) = terminal_error {
        vec![Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(Color::LightRed),
        ))]
    } else if terminal_rows.is_empty() {
        vec![Line::from(Span::styled(
            "starting native terminal...",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        terminal_rows
            .into_iter()
            .map(|row| Line::from(terminal_row_spans(row)))
            .collect()
    };

    f.render_widget(Paragraph::new(lines).style(Style::default()), inner);

    if show_cursor {
        if let Some((row, col)) = terminal_cursor {
            if inner.width > 0 && inner.height > 0 {
                let x = inner
                    .x
                    .saturating_add(col)
                    .min(inner.right().saturating_sub(1));
                let y = inner
                    .y
                    .saturating_add(row)
                    .min(inner.bottom().saturating_sub(1));
                f.set_cursor(x, y);
            }
        }
    }
}

fn render_terminal_panes(
    f: &mut Frame,
    area: Rect,
    axis: PaneAxis,
    separators: Vec<PaneSeparator>,
    panes: Vec<PaneSnapshot>,
    show_cursor: bool,
    regions: &mut Regions,
) {
    let pane_rects = panes
        .iter()
        .map(|pane| offset_rect(area, pane.rect))
        .collect::<Vec<_>>();
    regions.terminal_pane_count = pane_rects.len().min(regions.terminal_panes.len());

    for (idx, (pane, rect)) in panes
        .into_iter()
        .zip(pane_rects.iter().copied())
        .enumerate()
    {
        if idx < regions.terminal_panes.len() {
            regions.terminal_panes[idx] = rect;
        }
        let lines = if pane.rows.is_empty() {
            vec![Line::from(Span::styled(
                "starting native terminal...",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            pane.rows
                .into_iter()
                .map(|row| Line::from(terminal_row_spans(row)))
                .collect()
        };
        f.render_widget(Paragraph::new(lines).style(Style::default()), rect);

        if show_cursor && pane.active && !pane.hide_cursor {
            if let Some((row, col)) = pane.cursor {
                if rect.width > 0 && rect.height > 0 {
                    let x = rect
                        .x
                        .saturating_add(col)
                        .min(rect.right().saturating_sub(1));
                    let y = rect
                        .y
                        .saturating_add(row)
                        .min(rect.bottom().saturating_sub(1));
                    f.set_cursor(x, y);
                }
            }
        }
    }

    render_pane_separators(f, area, axis, &separators);
}

fn render_pane_separators(
    f: &mut Frame,
    area: Rect,
    _fallback_axis: PaneAxis,
    separators: &[PaneSeparator],
) {
    let style = Style::default().fg(Color::DarkGray);
    for separator in separators {
        let rect = offset_rect(
            area,
            PaneRect::new(separator.x, separator.y, separator.width, separator.height),
        );
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let symbol = match separator.axis {
            PaneAxis::Columns => "│",
            PaneAxis::Rows => "─",
        };
        let lines = match separator.axis {
            PaneAxis::Columns => (0..rect.height)
                .map(|_| Line::from(Span::styled(symbol, style)))
                .collect::<Vec<_>>(),
            PaneAxis::Rows => vec![Line::from(Span::styled(
                symbol.repeat(rect.width as usize),
                style,
            ))],
        };
        f.render_widget(Paragraph::new(lines), rect);
    }
}

fn offset_rect(area: Rect, rect: PaneRect) -> Rect {
    Rect::new(
        area.x.saturating_add(rect.x),
        area.y.saturating_add(rect.y),
        rect.width.min(area.width.saturating_sub(rect.x)),
        rect.height.min(area.height.saturating_sub(rect.y)),
    )
}

#[allow(dead_code)]
fn render_linear_pane_separators(f: &mut Frame, area: Rect, axis: PaneAxis, pane_rects: &[Rect]) {
    let style = Style::default().fg(Color::DarkGray);
    for pair in pane_rects.windows(2) {
        let separator = match axis {
            PaneAxis::Columns => Rect::new(pair[0].right(), area.y, 1, area.height),
            PaneAxis::Rows => Rect::new(area.x, pair[0].bottom(), area.width, 1),
        };
        if separator.width == 0 || separator.height == 0 {
            continue;
        }
        let lines = match axis {
            PaneAxis::Columns => (0..separator.height)
                .map(|_| Line::from(Span::styled("│", style)))
                .collect::<Vec<_>>(),
            PaneAxis::Rows => vec![Line::from(Span::styled(
                "─".repeat(separator.width as usize),
                style,
            ))],
        };
        f.render_widget(Paragraph::new(lines), separator);
    }
}

fn terminal_row_spans(row: Vec<TerminalSpan>) -> Vec<Span<'static>> {
    row.into_iter()
        .map(|span| Span::styled(span.text, terminal_style(span.style)))
        .collect()
}

fn terminal_style(style: TerminalStyle) -> Style {
    let mut rendered = Style::default();
    if let Some(fg) = terminal_color(style.fg) {
        rendered = rendered.fg(fg);
    }
    if let Some(bg) = terminal_color(style.bg) {
        rendered = rendered.bg(bg);
    }

    if style.inverse {
        rendered = rendered.add_modifier(Modifier::REVERSED);
    }
    if style.bold {
        rendered = rendered.add_modifier(Modifier::BOLD);
    }
    if style.dim {
        rendered = rendered.add_modifier(Modifier::DIM);
    }
    if style.italic {
        rendered = rendered.add_modifier(Modifier::ITALIC);
    }
    if style.underline {
        rendered = rendered.add_modifier(Modifier::UNDERLINED);
    }
    rendered
}

fn terminal_color(color: TerminalColor) -> Option<Color> {
    match color {
        TerminalColor::Default => None,
        TerminalColor::Rgb(red, green, blue) => Some(Color::Rgb(red, green, blue)),
        TerminalColor::Indexed(index) => Some(match index {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            index => Color::Indexed(index),
        }),
    }
}

fn render_sidebar(
    f: &mut Frame,
    area: Rect,
    session_label: &str,
    windows: &[Window],
    status: Option<&str>,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // session button
            Constraint::Length(3), // detach button
            Constraint::Length(2), // status
            Constraint::Min(5),    // windows
        ])
        .split(area);

    regions.session_button = chunks[0];
    regions.detach_button = chunks[1];

    let session_hot = hover == Some(Hotspot::SessionButton);
    let session = Paragraph::new(Line::from(Span::styled(
        session_label.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(Some("Session"), Color::Cyan, session_hot));
    f.render_widget(session, chunks[0]);

    let detach_hot = hover == Some(Hotspot::DetachButton);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, Color::Red, detach_hot));
    f.render_widget(detach, chunks[1]);

    let status_line = Paragraph::new(Line::from(Span::styled(
        status.unwrap_or_default().to_string(),
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(status_line, chunks[2]);

    render_windows(f, chunks[3], windows, hover, regions);
}

fn render_windows(
    f: &mut Frame,
    area: Rect,
    windows: &[Window],
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let mut win_items: Vec<ListItem> = Vec::new();
    regions.window_count = 0;

    let inner_top = area.y.saturating_add(1);
    for (row, win) in windows.iter().enumerate() {
        if row >= regions.windows.len() {
            break;
        }
        let y = inner_top.saturating_add(row as u16);
        let row_rect = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
        let close_rect = Rect::new(
            row_rect.x.saturating_add(row_rect.width.saturating_sub(2)),
            y,
            2.min(row_rect.width),
            1,
        );
        regions.windows[row] = row_rect;
        regions.window_close[row] = close_rect;
        regions.window_count += 1;

        let marker = if win.active { "▸" } else { " " };
        win_items.push(ListItem::new(window_row_line(
            marker,
            win,
            area.width.saturating_sub(2),
            hover,
            row,
        )));
    }

    let new_row = win_items.len();
    regions.new_window = Rect::new(
        area.x + 1,
        inner_top.saturating_add(new_row as u16),
        area.width.saturating_sub(2),
        1,
    );
    let new_hot = hover == Some(Hotspot::NewWindow);
    let new_style = if new_hot {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default().fg(Color::Green)
    };
    win_items.push(ListItem::new(Line::from(Span::styled(
        "  + new", new_style,
    ))));

    let windows = List::new(win_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" WINDOWS "),
    );
    f.render_widget(windows, area);
}

fn render_session_modal(
    f: &mut Frame,
    sessions: &[Session],
    current_session: &str,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let area = centered_rect(48, 44, f.size());
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .margin(1)
        .split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(block, area);

    regions.modal_session_count = 0;
    let mut items = Vec::new();
    for (idx, sess) in sessions.iter().enumerate() {
        if idx >= regions.modal_sessions.len() {
            break;
        }
        let row_rect = Rect::new(
            chunks[0].x,
            chunks[0].y.saturating_add(idx as u16),
            chunks[0].width,
            1,
        );
        regions.modal_sessions[idx] = row_rect;
        regions.modal_session_count += 1;

        let active = sess.name == current_session;
        let mark = if active { "●" } else { " " };
        let hot = hover == Some(Hotspot::ModalSession(idx));
        let style = if hot {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!(" {mark} {}", sess.name), style),
            Span::raw(format!("  {} windows", sess.windows)),
        ])));
    }
    f.render_widget(List::new(items), chunks[0]);

    let actions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);
    regions.modal_new_session = actions[0];
    regions.modal_detach = actions[1];

    let new_hot = hover == Some(Hotspot::ModalNewSession);
    let new_button = Paragraph::new(Line::from(Span::styled(
        "New Session",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, Color::Green, new_hot));
    f.render_widget(new_button, actions[0]);

    let hot = hover == Some(Hotspot::ModalDetach);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, Color::Red, hot));
    f.render_widget(detach, actions[1]);
}

fn window_row_line(
    marker: &str,
    win: &Window,
    width: u16,
    hover: Option<Hotspot>,
    row: usize,
) -> Line<'static> {
    let width = width as usize;
    let close_hot = hover == Some(Hotspot::WindowClose(row));
    if width <= 2 {
        return Line::from(Span::styled("✕".to_string(), close_style(close_hot)));
    }

    let label = format!("{marker} {}: {}", win.index, win.name);
    let label_width = width.saturating_sub(2);
    let label_len = label.chars().count();
    let label_text = if label_len >= label_width {
        label.chars().take(label_width).collect::<String>()
    } else {
        format!("{}{}", label, " ".repeat(label_width - label_len))
    };

    let row_hot = hover == Some(Hotspot::Window(row));
    let row_style = if row_hot {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else if win.active {
        Style::default()
            .fg(Color::White)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    Line::from(vec![
        Span::styled(label_text, row_style),
        Span::raw(" "),
        Span::styled("✕", close_style(close_hot)),
    ])
}

fn close_style(hot: bool) -> Style {
    if hot {
        Style::default()
            .fg(Color::Red)
            .bg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn hit_test(x: u16, y: u16, regions: &Regions, modal_open: bool) -> Option<Hotspot> {
    if modal_open {
        for idx in 0..regions.modal_session_count {
            if contains(regions.modal_sessions[idx], x, y) {
                return Some(Hotspot::ModalSession(idx));
            }
        }
        if contains(regions.modal_new_session, x, y) {
            return Some(Hotspot::ModalNewSession);
        }
        if contains(regions.modal_detach, x, y) {
            return Some(Hotspot::ModalDetach);
        }
    }

    if contains(regions.main_pane, x, y) {
        return Some(Hotspot::MainPane);
    }

    if contains(regions.session_button, x, y) {
        return Some(Hotspot::SessionButton);
    }
    if contains(regions.detach_button, x, y) {
        return Some(Hotspot::DetachButton);
    }
    for idx in 0..regions.window_count {
        if contains(regions.window_close[idx], x, y) {
            return Some(Hotspot::WindowClose(idx));
        }
        if contains(regions.windows[idx], x, y) {
            return Some(Hotspot::Window(idx));
        }
    }
    if contains(regions.new_window, x, y) {
        return Some(Hotspot::NewWindow);
    }
    None
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn terminal_cell_at(x: u16, y: u16, body: Rect) -> Option<(u16, u16)> {
    if body.width == 0 || body.height == 0 || !contains(body, x, y) {
        return None;
    }
    Some((y.saturating_sub(body.y), x.saturating_sub(body.x)))
}

fn terminal_cell_at_pane(x: u16, y: u16, regions: &Regions) -> Option<(usize, u16, u16)> {
    if regions.terminal_pane_count == 0 {
        return terminal_cell_at(x, y, regions.terminal_body).map(|(row, col)| (0, row, col));
    }

    for idx in 0..regions.terminal_pane_count {
        let rect = regions.terminal_panes[idx];
        if let Some((row, col)) = terminal_cell_at(x, y, rect) {
            return Some((idx, row, col));
        }
    }
    None
}

fn terminal_cell_for_pane(x: u16, y: u16, regions: &Regions, pane: usize) -> Option<(u16, u16)> {
    let rect = if regions.terminal_pane_count == 0 {
        if pane != 0 {
            return None;
        }
        regions.terminal_body
    } else {
        if pane >= regions.terminal_pane_count {
            return None;
        }
        regions.terminal_panes[pane]
    };

    if rect.width == 0 || rect.height == 0 {
        return None;
    }

    let max_x = rect.right().saturating_sub(1);
    let max_y = rect.bottom().saturating_sub(1);
    let clamped_x = x.clamp(rect.x, max_x);
    let clamped_y = y.clamp(rect.y, max_y);
    Some((
        clamped_y.saturating_sub(rect.y),
        clamped_x.saturating_sub(rect.x),
    ))
}

#[cfg(test)]
fn pane_rects(area: Rect, axis: PaneAxis, count: usize) -> Vec<Rect> {
    if count == 0 || area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }

    match axis {
        PaneAxis::Columns => {
            let available = area.width.saturating_sub((count - 1) as u16);
            let widths = split_lengths(available, count);
            let mut x = area.x;
            widths
                .into_iter()
                .map(|width| {
                    let rect = Rect::new(x, area.y, width, area.height);
                    x = x.saturating_add(width).saturating_add(1);
                    rect
                })
                .collect()
        }
        PaneAxis::Rows => {
            let available = area.height.saturating_sub((count - 1) as u16);
            let heights = split_lengths(available, count);
            let mut y = area.y;
            heights
                .into_iter()
                .map(|height| {
                    let rect = Rect::new(area.x, y, area.width, height);
                    y = y.saturating_add(height).saturating_add(1);
                    rect
                })
                .collect()
        }
    }
}

#[cfg(test)]
fn split_lengths(total: u16, count: usize) -> Vec<u16> {
    if count == 0 {
        return Vec::new();
    }
    let base = total / count as u16;
    let mut remainder = total % count as u16;
    (0..count)
        .map(|_| {
            let extra = u16::from(remainder > 0);
            remainder = remainder.saturating_sub(1);
            base.saturating_add(extra)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> UiState {
        let mux = crate::native_mux::NativeMux::new("ui-test", PathBuf::from("."), 20, 5).unwrap();
        UiState {
            session_modal_open: false,
            hover: None,
            regions: Regions::default(),
            mux: MuxBackend::Local(mux),
            sessions: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
            current_session: String::new(),
            status: None,
            terminal_error: None,
            terminal_mode: true,
            selection: None,
            terminal_axis: PaneAxis::default(),
            terminal_separators: Vec::new(),
            terminal_panes: Vec::new(),
            terminal_rows: Vec::new(),
            terminal_cursor: None,
            terminal_hide_cursor: true,
            terminal_mouse_protocol_active: false,
            terminal_scrollback: 0,
        }
    }

    #[test]
    fn hit_test_prefers_window_close_x_over_window_row() {
        let mut regions = Regions::default();
        regions.windows[0] = Rect::new(10, 5, 20, 1);
        regions.window_close[0] = Rect::new(28, 5, 2, 1);
        regions.window_count = 1;

        assert_eq!(
            hit_test(28, 5, &regions, false),
            Some(Hotspot::WindowClose(0))
        );
        assert_eq!(hit_test(12, 5, &regions, false), Some(Hotspot::Window(0)));
    }

    #[test]
    fn close_x_hover_gets_its_own_red_style() {
        let active = Window {
            index: 1,
            name: "build".to_string(),
            active: true,
            panes: 1,
        };
        let row = window_row_line("▸", &active, 20, Some(Hotspot::WindowClose(0)), 0);
        let last = row.spans.last().expect("close span");
        assert_eq!(last.content.as_ref(), "✕");
        assert_eq!(last.style.fg, Some(Color::Red));
        assert_eq!(last.style.bg, Some(Color::Black));
    }

    #[test]
    fn hit_test_distinguishes_modal_new_session_and_detach_buttons() {
        let mut regions = Regions::default();
        regions.modal_new_session = Rect::new(5, 20, 12, 3);
        regions.modal_detach = Rect::new(18, 20, 10, 3);

        assert_eq!(
            hit_test(6, 21, &regions, true),
            Some(Hotspot::ModalNewSession)
        );
        assert_eq!(hit_test(19, 21, &regions, true), Some(Hotspot::ModalDetach));
    }

    #[test]
    fn terminal_default_style_does_not_force_a_background() {
        let style = terminal_style(TerminalStyle::default());
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
    }

    #[test]
    fn terminal_reverse_style_uses_reverse_video_modifier() {
        let style = terminal_style(TerminalStyle {
            inverse: true,
            ..TerminalStyle::default()
        });
        assert!(style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn pane_rects_reserve_separator_cells_between_columns() {
        let rects = pane_rects(Rect::new(0, 0, 11, 5), PaneAxis::Columns, 2);

        assert_eq!(rects, vec![Rect::new(0, 0, 5, 5), Rect::new(6, 0, 5, 5)]);
    }

    #[test]
    fn terminal_cell_at_pane_maps_to_local_pane_coordinates() {
        let mut regions = Regions {
            terminal_pane_count: 2,
            ..Regions::default()
        };
        regions.terminal_panes[0] = Rect::new(0, 0, 5, 5);
        regions.terminal_panes[1] = Rect::new(6, 0, 5, 5);

        assert_eq!(terminal_cell_at_pane(7, 3, &regions), Some((1, 3, 1)));
        assert_eq!(terminal_cell_at_pane(5, 3, &regions), None);
    }

    #[test]
    fn terminal_cell_for_pane_clamps_drag_to_original_pane() {
        let mut regions = Regions {
            terminal_pane_count: 2,
            ..Regions::default()
        };
        regions.terminal_panes[0] = Rect::new(0, 0, 5, 5);
        regions.terminal_panes[1] = Rect::new(6, 0, 5, 5);

        assert_eq!(terminal_cell_for_pane(9, 6, &regions, 0), Some((4, 4)));
    }

    #[test]
    fn mouse_up_keeps_non_empty_selection_but_ends_dragging() {
        let mut state = test_state();

        state.begin_selection(0, 1, 2);
        state.update_selection(1, 5);
        state.finish_selection(1, 6);

        assert_eq!(
            state.selection_range(),
            Some(SelectionRange::new(1, 2, 1, 6))
        );
        let selection = state.selection.expect("selection persists after mouse-up");
        assert_eq!(selection.pane, 0);
        assert!(!selection.dragging);
    }

    #[test]
    fn mouse_up_clears_zero_width_selection() {
        let mut state = test_state();

        state.begin_selection(0, 2, 3);
        state.finish_selection(2, 3);

        assert!(state.selection.is_none());
    }

    #[test]
    fn terminal_key_input_clears_existing_selection() {
        let mut state = test_state();
        state.begin_selection(0, 0, 0);
        state.finish_selection(0, 4);

        state.send_terminal_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        assert!(state.selection.is_none());
    }

    #[test]
    fn hit_test_only_exposes_windows_not_deprecated_pane_controls() {
        let regions = Regions::default();

        assert_eq!(hit_test(21, 7, &regions, false), None);
        assert_eq!(hit_test(21, 8, &regions, false), None);
    }
}
