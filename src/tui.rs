//! Default tuimux ratatui interface.
//!
//! The main pane is backed by tuimux's own Rust-native in-process multiplexer:
//! sessions and windows are owned by tuimux, and each window runs a real shell
//! in a PTY rendered through a vt100 screen model.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::clipboard;
use crate::native_mux::{NativeMux, Session, Window};
use crate::terminal::{PtyTerminal, SelectionRange, TerminalColor, TerminalSpan, TerminalStyle};

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
    modal_new_session: Rect,
    modal_detach: Rect,
    modal_sessions: [Rect; 8],
    modal_session_count: usize,
}

struct UiState {
    session_modal_open: bool,
    hover: Option<Hotspot>,
    regions: Regions,
    mux: NativeMux,
    /// Live native mux state, refreshed after every mutating command.
    sessions: Vec<Session>,
    windows: Vec<Window>,
    current_session: String,
    /// Non-fatal, transient message shown in the status bar (e.g. that a
    /// window was created, selected, or closed).
    status: Option<String>,
    terminal_error: Option<String>,
    terminal_mode: bool,
    selection: Option<SelectionState>,
}

#[derive(Debug, Clone, Copy)]
struct SelectionState {
    anchor: (u16, u16),
    focus: (u16, u16),
    dragging: bool,
}

impl UiState {
    /// Build initial state from the native multiplexer.
    fn bootstrap(initial_session: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let mux = NativeMux::new(initial_session, cwd, 80, 24)?;
        let mut state = UiState {
            // Native tuimux starts focused in the shell. The Session button or
            // Alt-S opens the session switcher when needed.
            session_modal_open: false,
            hover: None,
            regions: Regions::default(),
            mux,
            sessions: Vec::new(),
            windows: Vec::new(),
            current_session: String::new(),
            status: None,
            terminal_error: None,
            terminal_mode: true,
            selection: None,
        };
        state.refresh_metadata();
        Ok(state)
    }

    fn sync_terminal(&mut self, width: u16, height: u16) {
        let size = (width.max(1), height.max(1));
        self.mux.resize_active(size.0, size.1);
        self.mux.drain_all();
        self.refresh_metadata();
    }

    fn refresh_metadata(&mut self) {
        self.sessions = self.mux.session_infos();
        self.windows = self.mux.window_infos();
        self.current_session = self.mux.current_session_name().to_string();
    }

    fn active_terminal(&self) -> Option<&PtyTerminal> {
        self.mux.active_terminal()
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

    fn begin_selection(&mut self, row: u16, col: u16) {
        self.selection = Some(SelectionState {
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
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
    }

    fn copy_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        let Some(terminal) = self.mux.active_terminal() else {
            return false;
        };
        let text = terminal.selected_text(range);
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
        self.mux
            .active_terminal()
            .map(PtyTerminal::mouse_protocol_active)
            .unwrap_or(false)
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

        if let Some(terminal) = self.mux.active_terminal_mut() {
            if let Err(e) = terminal.send_key(key) {
                self.status = Some(format!("terminal input failed: {e}"));
            }
        } else {
            self.status = Some("terminal is not ready".to_string());
        }
    }

    fn send_terminal_paste(&mut self, text: &str) {
        self.clear_selection();
        if let Some(terminal) = self.mux.active_terminal_mut() {
            if let Err(e) = terminal.send_paste(text) {
                self.status = Some(format!("terminal paste failed: {e}"));
            }
        } else {
            self.status = Some("terminal is not ready".to_string());
        }
    }

    fn send_terminal_mouse(
        &mut self,
        kind: MouseEventKind,
        row: u16,
        col: u16,
        modifiers: KeyModifiers,
    ) {
        if let Some(terminal) = self.mux.active_terminal_mut() {
            if let Err(e) = terminal.send_mouse_event(kind, row, col, modifiers) {
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore(terminal: &mut Term) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()
}

fn run_loop(terminal: &mut Term, state: &mut UiState) -> io::Result<Exit> {
    let mut last_metadata_refresh = Instant::now();
    loop {
        let body = state.regions.terminal_body;
        if body.width > 0 && body.height > 0 {
            state.sync_terminal(body.width, body.height);
        }
        if last_metadata_refresh.elapsed() >= Duration::from_millis(750) {
            state.refresh_metadata();
            last_metadata_refresh = Instant::now();
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
                    (KeyCode::Char('d'), _) => {
                        return Ok(Exit::Detach);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                if let Some((row, col)) =
                    terminal_cell_at(mouse.column, mouse.row, state.regions.terminal_body)
                {
                    let child_wants_mouse = state.terminal_mouse_protocol_active();
                    let selection_gesture =
                        mouse.modifiers.contains(KeyModifiers::SHIFT) || !child_wants_mouse;

                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) if selection_gesture => {
                            state.terminal_mode = true;
                            state.begin_selection(row, col);
                            continue;
                        }
                        MouseEventKind::Drag(MouseButton::Left) if state.selection.is_some() => {
                            state.update_selection(row, col);
                            continue;
                        }
                        MouseEventKind::Up(MouseButton::Left) if state.selection.is_some() => {
                            state.finish_selection(row, col);
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
                    if let Some((row, col)) =
                        terminal_cell_at(mouse.column, mouse.row, state.regions.terminal_body)
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
            state.refresh_metadata();
        }
        Err(e) => state.status = Some(format!("select-window failed: {e}")),
    }
}

fn kill_window(state: &mut UiState, idx: usize) {
    let (width, height) = active_terminal_size(state);
    match state.mux.kill_window_by_row(idx, width, height) {
        Ok(index) => {
            state.status = Some(format!("killed window {index}"));
            state.clear_selection();
            state.refresh_metadata();
        }
        Err(e) => state.status = Some(format!("kill-window failed: {e}")),
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
            state.refresh_metadata();
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
            state.refresh_metadata();
        }
        Err(e) => state.status = Some(format!("new-session failed: {e}")),
    }
}

fn switch_session(state: &mut UiState, idx: usize) {
    match state.mux.switch_session_by_row(idx) {
        Ok(()) => {
            state.session_modal_open = false;
            state.clear_selection();
            state.refresh_metadata();
            state.status = Some(format!("selected session '{}'", state.current_session));
        }
        Err(e) => state.status = Some(format!("select-session failed: {e}")),
    }
}

fn ui(f: &mut Frame, state: &mut UiState) {
    let root = f.size();
    let selection = state.selection_range();
    let terminal_rows = state
        .active_terminal()
        .map(|terminal| terminal.styled_rows_with_selection(selection))
        .unwrap_or_default();
    let terminal_cursor = state.active_terminal().map(PtyTerminal::cursor);
    let terminal_hide_cursor = state
        .active_terminal()
        .map(PtyTerminal::hide_cursor)
        .unwrap_or(true);

    let session_label = if state.current_session.is_empty() {
        "(no session)"
    } else {
        &state.current_session
    };

    if state.terminal_mode && !state.session_modal_open {
        render_main(
            f,
            root,
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
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // session button
            Constraint::Length(3), // detach button
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

    render_windows(f, chunks[2], windows, hover, regions);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
