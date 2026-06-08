//! The live terminal UI (`tuimux` with no subcommand).
//!
//! As of v0.1.4 the right sidebar is wired to a **real tmux server**: it lists
//! real sessions and windows, clicking a window runs `select-window`, clicking
//! `+ new` runs `new-window`, clicking a session row runs `switch-client` (only
//! when tuimux itself is running inside tmux), and Detach runs `detach-client`.
//! The center pane area is still a static mock — the `tmux -CC` control-mode
//! renderer is the next milestone (SRS FR-CONN).
//!
//! If there is no tmux server yet, tuimux creates a detached session named
//! `tuimux` so the UI always has something real to show.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::preview::PreviewData;
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
    Window(usize),
    NewWindow,
    ModalSession(usize),
    ModalDetach,
}

#[derive(Default, Clone, Copy)]
struct Regions {
    session_button: Rect,
    detach_button: Rect,
    new_window: Rect,
    windows: [Rect; 8],
    window_count: usize,
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
    let data = PreviewData::default();
    let result = run_loop(&mut terminal, probe, &tmux, &data, &mut state);
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

fn run_loop(
    terminal: &mut Term,
    probe: &TmuxProbe,
    tmux: &Tmux<RealTmux>,
    data: &PreviewData,
    state: &mut UiState,
) -> io::Result<Exit> {
    loop {
        terminal.draw(|f| ui(f, probe, tmux, data, state))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match (key.code, key.modifiers) {
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
                        Some(Hotspot::Window(idx)) => {
                            select_window(tmux, state, idx);
                        }
                        Some(Hotspot::NewWindow) => {
                            new_window(tmux, state);
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

fn ui(
    f: &mut Frame,
    probe: &TmuxProbe,
    tmux: &Tmux<RealTmux>,
    data: &PreviewData,
    state: &mut UiState,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status
            Constraint::Min(5),    // body
        ])
        .split(f.size());

    let tmux_desc = match &probe.version {
        Some(v) => format!("tmux {v}"),
        None => "tmux: not detected".to_string(),
    };
    let scope = if tmux.inside_tmux() {
        "inside tmux"
    } else {
        "outside tmux"
    };
    let session_label = if state.current_session.is_empty() {
        "(no session)"
    } else {
        &state.current_session
    };
    let mut info = format!("  {session_label} · {tmux_desc} · {scope}");
    if let Some(msg) = &state.status {
        info.push_str(" · ");
        info.push_str(msg);
    }
    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            " tuimux ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(info),
    ]));
    f.render_widget(status, root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(26)])
        .split(root[1]);

    render_main(f, body[0], &data.panes);
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

fn render_main(f: &mut Frame, area: Rect, panes: &[&str]) {
    let lines: Vec<Line> = panes.iter().map(|p| Line::from(*p)).collect();
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" MAIN AREA (tmux panes — mock) "),
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
        regions.windows[row] = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
        regions.window_count += 1;

        let marker = if win.active { "▸" } else { " " };
        let is_hot = hover == Some(Hotspot::Window(row));
        let style = if is_hot {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else if win.active {
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        win_items.push(ListItem::new(Line::from(Span::styled(
            format!("{marker} {}: {}", win.index, win.name),
            style,
        ))));
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

    regions.modal_detach = chunks[1];
    let hot = hover == Some(Hotspot::ModalDetach);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, Color::Red, hot));
    f.render_widget(detach, chunks[1]);
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
        if contains(regions.modal_detach, x, y) {
            return Some(Hotspot::ModalDetach);
        }
    }

    if contains(regions.session_button, x, y) {
        return Some(Hotspot::SessionButton);
    }
    if contains(regions.detach_button, x, y) {
        return Some(Hotspot::DetachButton);
    }
    for idx in 0..regions.window_count {
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
