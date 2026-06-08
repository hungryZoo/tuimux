//! Experimental dashboard prototype (`tuimux --dashboard`).
//!
//! The default v0.1.7 command path does **not** use this renderer; it opens a
//! real tmux client. This module remains only as a hidden prototype for future
//! sidebar/control-mode experiments.
//!
//! If there is no tmux server yet, tuimux creates a detached session named
//! `tuimux` so the UI always has something real to show.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

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
    pane_lines: Vec<String>,
    terminal_mode: bool,
    /// Monotonic counter behind the `tuimux-<n>` names created by the `n` key.
    new_session_counter: u32,
}

impl UiState {
    /// Build initial state from the live server, creating a detached `tuimux`
    /// session if no server is running yet.
    fn bootstrap(tmux: &Tmux<RealTmux>) -> Self {
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
            pane_lines: Vec::new(),
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
        state.refresh_pane(tmux, 80, 24);
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

    fn refresh_pane(&mut self, tmux: &Tmux<RealTmux>, width: u16, height: u16) {
        if self.current_session.is_empty() {
            self.pane_lines.clear();
            return;
        }
        match tmux.capture_pane(&self.current_session, width, height) {
            Ok(lines) => self.pane_lines = lines,
            Err(e) => self.status = Some(format!("capture-pane failed: {e}")),
        }
    }

    /// Re-read sessions and windows after a mutating command.
    fn refresh(&mut self, tmux: &Tmux<RealTmux>) {
        self.sessions = tmux.list_sessions().unwrap_or_default();
        if !self.sessions.iter().any(|s| s.name == self.current_session) {
            self.current_session = self.pick_current();
        }
        self.reload_windows(tmux);
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
    let mut state = UiState::bootstrap(&tmux);

    let mut terminal = setup()?;
    let result = run_loop(&mut terminal, &tmux, &mut state);
    restore(&mut terminal)?;

    match result {
        Ok(Exit::Quit) => {
            println!("tuimux: exited.");
            Ok(0)
        }
        Ok(Exit::Detach) => {
            if tmux.inside_tmux() {
                println!("tuimux: detached the tmux client.");
            } else {
                println!("tuimux: exited (not running inside tmux — nothing to detach).");
            }
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
    loop {
        terminal.draw(|f| ui(f, state))?;

        if !event::poll(Duration::from_millis(120))? {
            let area = state.regions.main_pane;
            state.refresh_pane(
                tmux,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
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
                        if let Some(keys) = key_event_to_send_keys(key) {
                            if let Err(e) = tmux.send_keys(&state.current_session, &keys) {
                                state.status = Some(format!("send-keys failed: {e}"));
                            }
                            let area = state.regions.main_pane;
                            state.refresh_pane(
                                tmux,
                                area.width.saturating_sub(2),
                                area.height.saturating_sub(2),
                            );
                        }
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
                        let _ = tmux.detach();
                        return Ok(Exit::Detach);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
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
                            let _ = tmux.detach();
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
    if tmux.inside_tmux() {
        match tmux.switch_session(&name) {
            Ok(()) => {
                state.current_session = name;
                state.session_modal_open = false;
                state.refresh(tmux);
            }
            Err(e) => state.status = Some(format!("switch-client failed: {e}")),
        }
    } else {
        // Outside tmux, do not run `attach-session` from inside the TUI. Instead,
        // select the session as the sidebar target so window inspection/creation
        // still works until the control-mode attach renderer lands.
        state.current_session = name.clone();
        state.session_modal_open = false;
        state.status = Some(format!("selected session '{name}'"));
        state.reload_windows(tmux);
    }
}

fn ui(f: &mut Frame, state: &mut UiState) {
    let root = f.size();

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
        &state.pane_lines,
        state.terminal_mode,
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
    pane_lines: &[String],
    terminal_mode: bool,
    regions: &mut Regions,
) {
    regions.main_pane = area;
    let mut lines: Vec<Line> = if pane_lines.is_empty() {
        vec![Line::from(
            "tmux pane is empty; click here and type, F12 returns to navigation",
        )]
    } else {
        pane_lines.iter().map(|p| Line::from(p.as_str())).collect()
    };
    if let Some(last) = lines.last_mut() {
        last.spans.push(Span::raw(" "));
    }
    let border = if terminal_mode {
        Color::Green
    } else {
        Color::DarkGray
    };
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border)),
    );
    f.render_widget(para, area);
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

fn key_event_to_send_keys(key: KeyEvent) -> Option<Vec<String>> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return Some(vec![format!("C-{}", c.to_ascii_lowercase())]);
        }
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            return Some(vec![format!("M-{c}")]);
        }
    }
    match key.code {
        KeyCode::Char(c) => Some(vec!["-l".to_string(), c.to_string()]),
        KeyCode::Enter => Some(vec!["Enter".to_string()]),
        KeyCode::Backspace => Some(vec!["BSpace".to_string()]),
        KeyCode::Tab => Some(vec!["Tab".to_string()]),
        KeyCode::Esc => Some(vec!["Escape".to_string()]),
        KeyCode::Left => Some(vec!["Left".to_string()]),
        KeyCode::Right => Some(vec!["Right".to_string()]),
        KeyCode::Up => Some(vec!["Up".to_string()]),
        KeyCode::Down => Some(vec!["Down".to_string()]),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn key_mapping_sends_literal_text_and_named_keys_to_tmux() {
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(vec!["-l".to_string(), "a".to_string()])
        );
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(vec!["Enter".to_string()])
        );
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Backspace, KeyModifiers::NONE)),
            Some(vec!["BSpace".to_string()])
        );
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Left, KeyModifiers::NONE)),
            Some(vec!["Left".to_string()])
        );
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(vec!["C-c".to_string()])
        );
        assert_eq!(
            key_event_to_send_keys(key(KeyCode::Char('x'), KeyModifiers::ALT)),
            Some(vec!["M-x".to_string()])
        );
    }

    #[test]
    fn only_press_key_events_are_forwarded_to_tmux() {
        let press = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let repeat = KeyEvent {
            kind: KeyEventKind::Repeat,
            ..press
        };
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press
        };

        assert_eq!(
            key_event_to_send_keys(press),
            Some(vec!["Enter".to_string()])
        );
        assert_eq!(key_event_to_send_keys(repeat), None);
        assert_eq!(key_event_to_send_keys(release), None);
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
