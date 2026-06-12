//! Default tuimux ratatui interface.
//!
//! v0.1.9 keeps this UI as the default after v0.1.7 accidentally launched a
//! plain tmux client with no tuimux chrome. The main pane is now backed by a
//! real tmux client running inside a PTY and rendered through a vt100 screen
//! model, so it behaves like a terminal surface instead of a text snapshot.
//!
//! If there is no tmux server yet, tuimux creates a detached session named
//! `tuimux` so the UI always has something real to show.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
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

use crate::terminal::{TerminalColor, TerminalSpan, TerminalStyle, TmuxTerminal};
use crate::tmux::{RealTmux, Session, Tmux, TmuxProbe, Window};

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
    /// Live tmux state, refreshed after every mutating command.
    sessions: Vec<Session>,
    windows: Vec<Window>,
    current_session: String,
    /// Non-fatal, transient message shown in the status bar (e.g. why a switch
    /// was refused outside tmux, or that a window was created).
    status: Option<String>,
    tmux_binary: String,
    terminal: Option<TmuxTerminal>,
    terminal_error: Option<String>,
    terminal_mode: bool,
    /// Monotonic counter behind the `tuimux-<n>` names created by the `n` key.
    new_session_counter: u32,
}

impl UiState {
    /// Build initial state from the live server, creating a detached `tuimux`
    /// session if no server is running yet.
    fn bootstrap(tmux: &Tmux<RealTmux>, tmux_binary: String) -> Self {
        let mut state = UiState {
            // Open by default so users immediately see the session dialog; click
            // the session button or Esc to toggle it.
            session_modal_open: true,
            hover: None,
            regions: Regions::default(),
            sessions: Vec::new(),
            windows: Vec::new(),
            current_session: String::new(),
            status: None,
            tmux_binary,
            terminal: None,
            terminal_error: None,
            terminal_mode: false,
            new_session_counter: 0,
        };

        let mut sessions = tmux.list_sessions().unwrap_or_default();
        if sessions.is_empty() {
            if let Err(e) = tmux.new_session("tuimux") {
                state.status = Some(format!("could not create session 'tuimux': {e}"));
            }
            sessions = tmux.list_sessions().unwrap_or_default();
        }
        state.sessions = sessions;
        state.current_session = state.pick_current();
        state.reload_windows(tmux);
        state.sync_terminal(80, 24);
        state
    }

    /// Choose the session to focus: the attached one if any, else the first.
    fn pick_current(&self) -> String {
        self.sessions
            .iter()
            .find(|s| s.attached)
            .or_else(|| self.sessions.first())
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    fn reload_windows(&mut self, tmux: &Tmux<RealTmux>) {
        self.windows = if self.current_session.is_empty() {
            Vec::new()
        } else {
            tmux.list_windows(&self.current_session).unwrap_or_default()
        };
    }

    fn sync_terminal(&mut self, width: u16, height: u16) {
        if self.current_session.is_empty() {
            self.terminal = None;
            return;
        }
        let size = (width.max(1), height.max(1));
        let needs_spawn = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.session() != self.current_session)
            .unwrap_or(true);

        if needs_spawn {
            self.terminal = None;
            match TmuxTerminal::new(&self.tmux_binary, &self.current_session, size.0, size.1) {
                Ok(mut terminal) => {
                    terminal.drain();
                    self.terminal = Some(terminal);
                    self.terminal_error = None;
                }
                Err(e) => {
                    self.terminal_error = Some(format!("tmux PTY terminal failed to start: {e}"));
                    self.status = self.terminal_error.clone();
                    return;
                }
            }
        }

        if let Some(terminal) = &mut self.terminal {
            terminal.resize(size.0, size.1);
            terminal.drain();
        }
    }

    /// Re-read sessions and windows after a mutating command.
    fn refresh(&mut self, tmux: &Tmux<RealTmux>) {
        let previous_session = self.current_session.clone();
        self.sessions = tmux.list_sessions().unwrap_or_default();
        if !self.sessions.iter().any(|s| s.name == self.current_session) {
            self.current_session = self.pick_current();
        }
        self.reload_windows(tmux);
        if self.current_session != previous_session {
            self.terminal = None;
            self.terminal_error = None;
        }
    }

    /// Next free `tuimux-<n>` session name, skipping any that already exist.
    fn next_session_name(&self) -> (String, u32) {
        let mut n = self.new_session_counter + 1;
        loop {
            let name = format!("tuimux-{n}");
            if !self.sessions.iter().any(|s| s.name == name) {
                return (name, n);
            }
            n += 1;
        }
    }

    fn refresh_tmux_metadata(&mut self, tmux: &Tmux<RealTmux>) {
        match tmux.list_sessions() {
            Ok(sessions) => self.sessions = sessions,
            Err(e) => {
                self.status = Some(format!("list-sessions failed: {e}"));
                return;
            }
        }

        if !self.sessions.iter().any(|s| s.name == self.current_session) {
            self.current_session = self.pick_current();
            self.terminal = None;
            self.terminal_error = None;
        }
        self.reload_windows(tmux);
    }

    fn send_terminal_key(&mut self, key: KeyEvent) {
        if let Some(terminal) = &mut self.terminal {
            if let Err(e) = terminal.send_key(key) {
                self.status = Some(format!("terminal input failed: {e}"));
            }
        } else {
            self.status = Some("terminal is not ready".to_string());
        }
    }

    fn send_terminal_paste(&mut self, text: &str) {
        if let Some(terminal) = &mut self.terminal {
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
        if let Some(terminal) = &mut self.terminal {
            if let Err(e) = terminal.send_mouse_event(kind, row, col, modifiers) {
                self.status = Some(format!("terminal mouse failed: {e}"));
            }
        }
    }
}

/// Entry point for the default run. Returns a process exit code.
pub fn run(probe: &TmuxProbe) -> io::Result<i32> {
    if !io::stdout().is_terminal() {
        eprintln!(
            "tuimux: stdout is not a terminal — refusing to start the interactive UI.\n\
             Try one of:\n  tuimux --layout-preview   # render the layout as text\n  \
             tuimux --doctor           # check your environment"
        );
        return Ok(2);
    }

    let tmux = Tmux::new(RealTmux::new(probe.binary.clone()));
    let mut state = UiState::bootstrap(&tmux, probe.binary.clone());

    let mut terminal = setup()?;
    let result = run_loop(&mut terminal, &tmux, &mut state);
    restore(&mut terminal)?;

    match result {
        Ok(Exit::Quit) => {
            println!("tuimux: exited.");
            Ok(0)
        }
        Ok(Exit::Detach) => {
            println!("tuimux: detached embedded tmux client.");
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

fn run_loop(terminal: &mut Term, tmux: &Tmux<RealTmux>, state: &mut UiState) -> io::Result<Exit> {
    let mut last_metadata_refresh = Instant::now();
    loop {
        let body = state.regions.terminal_body;
        if body.width > 0 && body.height > 0 {
            state.sync_terminal(body.width, body.height);
        }
        if last_metadata_refresh.elapsed() >= Duration::from_millis(750) {
            state.refresh_tmux_metadata(tmux);
            last_metadata_refresh = Instant::now();
        }

        terminal.draw(|f| ui(f, state))?;

        if !event::poll(Duration::from_millis(33))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match (key.code, key.modifiers) {
                    (KeyCode::F(12), _) if state.terminal_mode => {
                        state.terminal_mode = false;
                        state.status = Some("navigation mode".to_string());
                    }
                    _ if state.terminal_mode => {
                        state.send_terminal_key(key);
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                    (KeyCode::Char('q'), _) => return Ok(Exit::Quit),
                    // `n` creates a fresh detached `tuimux-<n>` session, but only
                    // while the modal is open so it can't fire by accident.
                    (KeyCode::Char('n'), _) if state.session_modal_open => {
                        new_session(tmux, state);
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
                            kill_window(tmux, state, idx);
                        }
                        Some(Hotspot::Window(idx)) => {
                            select_window(tmux, state, idx);
                        }
                        Some(Hotspot::NewWindow) => {
                            new_window(tmux, state);
                        }
                        Some(Hotspot::ModalNewSession) => {
                            new_session(tmux, state);
                        }
                        Some(Hotspot::ModalSession(idx)) => {
                            switch_session(tmux, state, idx);
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

fn select_window(tmux: &Tmux<RealTmux>, state: &mut UiState, idx: usize) {
    let Some(index) = state.windows.get(idx).map(|w| w.index) else {
        return;
    };
    match tmux.select_window(&state.current_session, index) {
        Ok(()) => state.refresh(tmux),
        Err(e) => state.status = Some(format!("select-window failed: {e}")),
    }
}

fn kill_window(tmux: &Tmux<RealTmux>, state: &mut UiState, idx: usize) {
    let Some(index) = state.windows.get(idx).map(|w| w.index) else {
        return;
    };
    match tmux.kill_window(&state.current_session, index) {
        Ok(()) => {
            state.status = Some(format!("killed window {index}"));
            state.refresh(tmux);
        }
        Err(e) => state.status = Some(format!("kill-window failed: {e}")),
    }
}

fn new_window(tmux: &Tmux<RealTmux>, state: &mut UiState) {
    match tmux.new_window(&state.current_session) {
        Ok(()) => {
            state.status = Some(format!("created a window in '{}'", state.current_session));
            state.refresh(tmux);
        }
        Err(e) => state.status = Some(format!("new-window failed: {e}")),
    }
}

fn new_session(tmux: &Tmux<RealTmux>, state: &mut UiState) {
    let (name, counter) = state.next_session_name();
    match tmux.new_session(&name) {
        Ok(()) => {
            state.new_session_counter = counter;
            state.status = Some(format!("created session '{name}'"));
            state.refresh(tmux);
        }
        Err(e) => state.status = Some(format!("new-session failed: {e}")),
    }
}

fn switch_session(tmux: &Tmux<RealTmux>, state: &mut UiState, idx: usize) {
    let Some(name) = state.sessions.get(idx).map(|s| s.name.clone()) else {
        return;
    };
    state.current_session = name.clone();
    state.session_modal_open = false;
    state.status = Some(format!("selected session '{name}'"));
    state.terminal = None;
    state.terminal_error = None;
    state.reload_windows(tmux);
}

fn ui(f: &mut Frame, state: &mut UiState) {
    let root = f.size();
    let terminal_rows = state
        .terminal
        .as_ref()
        .map(TmuxTerminal::styled_rows)
        .unwrap_or_default();
    let terminal_cursor = state.terminal.as_ref().map(TmuxTerminal::cursor);
    let terminal_hide_cursor = state
        .terminal
        .as_ref()
        .map(TmuxTerminal::hide_cursor)
        .unwrap_or(true);

    let session_label = if state.current_session.is_empty() {
        "(no session)"
    } else {
        &state.current_session
    };
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
) {
    regions.main_pane = area;
    let border = if terminal_mode {
        Color::Green
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    regions.terminal_body = inner;
    f.render_widget(block, area);

    let lines: Vec<Line> = if let Some(error) = terminal_error {
        vec![Line::from(Span::styled(
            error.to_string(),
            Style::default().fg(Color::LightRed),
        ))]
    } else if terminal_rows.is_empty() {
        vec![Line::from(Span::styled(
            "starting tmux terminal...",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        terminal_rows
            .into_iter()
            .map(|row| Line::from(terminal_row_spans(row)))
            .collect()
    };

    f.render_widget(
        Paragraph::new(lines).style(Style::default().fg(Color::Gray).bg(Color::Black)),
        inner,
    );

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
    let mut fg = terminal_color(style.fg).unwrap_or(Color::Gray);
    let mut bg = terminal_color(style.bg).unwrap_or(Color::Black);
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut rendered = Style::default().fg(fg).bg(bg);
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
}
