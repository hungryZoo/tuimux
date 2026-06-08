//! The live terminal UI (`tuimux` with no subcommand).
//!
//! This is an MVP *scaffold*: it renders the revised compact layout with a mock
//! tmux pane area and a right sidebar. The right sidebar has a button-like
//! session name, a red Detach button, and vertical window tabs. Clicking the
//! session button opens a centered session dialog. It does **not** yet drive a
//! tmux control-mode session — that is the next milestone (SRS FR-CONN).
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
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
use crate::tmux::TmuxProbe;

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
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            // Open by default in the scaffold so users immediately see the dialog
            // direction; click the session button or Esc to toggle it.
            session_modal_open: true,
            hover: None,
            regions: Regions::default(),
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

    let mut terminal = setup()?;
    let data = PreviewData::default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut state = UiState::default();
    let result = run_loop(&mut terminal, probe, &data, &cwd, &mut state);
    restore(&mut terminal)?;

    match result {
        Ok(Exit::Quit) => {
            println!("tuimux: exited.");
            Ok(0)
        }
        Ok(Exit::Detach) => {
            println!("tuimux: detached. (MVP scaffold — no tmux session was attached yet.)");
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
    data: &PreviewData,
    _cwd: &std::path::Path,
    state: &mut UiState,
) -> io::Result<Exit> {
    loop {
        terminal.draw(|f| ui(f, probe, data, state))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                    (KeyCode::Char('q'), _) => return Ok(Exit::Quit),
                    (KeyCode::Esc, _) if state.session_modal_open => {
                        state.session_modal_open = false;
                    }
                    (KeyCode::Esc, _) => return Ok(Exit::Quit),
                    (KeyCode::Char('s'), KeyModifiers::ALT) => {
                        state.session_modal_open = !state.session_modal_open;
                    }
                    (KeyCode::Char('d'), _) => return Ok(Exit::Detach),
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
                            return Ok(Exit::Detach);
                        }
                        Some(Hotspot::ModalSession(_)) => {
                            // Scaffold only: pretend the selected session switched.
                            state.session_modal_open = false;
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

fn ui(f: &mut Frame, probe: &TmuxProbe, data: &PreviewData, state: &mut UiState) {
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
    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            " tuimux ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(
            "  {} · {} · scaffold preview",
            data.session, tmux_desc
        )),
    ]));
    f.render_widget(status, root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(26)])
        .split(root[1]);

    render_main(f, body[0], data);
    render_sidebar(f, body[1], data, state);

    if state.session_modal_open {
        render_session_modal(f, data, state);
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

fn render_main(f: &mut Frame, area: Rect, data: &PreviewData) {
    let lines: Vec<Line> = data.panes.iter().map(|p| Line::from(*p)).collect();
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" MAIN AREA (tmux panes — mock) "),
    );
    f.render_widget(para, area);
}

fn render_sidebar(f: &mut Frame, area: Rect, data: &PreviewData, state: &mut UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // session button
            Constraint::Length(3), // detach button
            Constraint::Min(5),    // windows
        ])
        .split(area);

    state.regions.session_button = chunks[0];
    state.regions.detach_button = chunks[1];

    let session_hot = state.hover == Some(Hotspot::SessionButton);
    let session = Paragraph::new(Line::from(Span::styled(
        data.session.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(Some("Session"), Color::Cyan, session_hot));
    f.render_widget(session, chunks[0]);

    let detach_hot = state.hover == Some(Hotspot::DetachButton);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, Color::Red, detach_hot));
    f.render_widget(detach, chunks[1]);

    render_windows(f, chunks[2], data, state);
}

fn render_windows(f: &mut Frame, area: Rect, data: &PreviewData, state: &mut UiState) {
    let mut win_items: Vec<ListItem> = Vec::new();
    state.regions.window_count = 0;

    let inner_top = area.y.saturating_add(1);
    for (row, (idx, name, active)) in data.windows.iter().enumerate() {
        if row >= state.regions.windows.len() {
            break;
        }
        let y = inner_top.saturating_add(row as u16);
        state.regions.windows[row] = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
        state.regions.window_count += 1;

        let marker = if *active { "▸" } else { " " };
        let is_hot = state.hover == Some(Hotspot::Window(row));
        let style = if is_hot {
            Style::default().fg(Color::Black).bg(Color::Green)
        } else if *active {
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        win_items.push(ListItem::new(Line::from(Span::styled(
            format!("{marker} {idx}: {name}"),
            style,
        ))));
    }

    let new_row = win_items.len();
    state.regions.new_window = Rect::new(
        area.x + 1,
        inner_top.saturating_add(new_row as u16),
        area.width.saturating_sub(2),
        1,
    );
    let new_hot = state.hover == Some(Hotspot::NewWindow);
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

fn render_session_modal(f: &mut Frame, data: &PreviewData, state: &mut UiState) {
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

    state.regions.modal_session_count = 0;
    let mut items = Vec::new();
    for (idx, (name, windows, active)) in data.sessions.iter().enumerate() {
        if idx >= state.regions.modal_sessions.len() {
            break;
        }
        let row_rect = Rect::new(
            chunks[0].x,
            chunks[0].y.saturating_add(idx as u16),
            chunks[0].width,
            1,
        );
        state.regions.modal_sessions[idx] = row_rect;
        state.regions.modal_session_count += 1;

        let mark = if *active { "●" } else { " " };
        let hot = state.hover == Some(Hotspot::ModalSession(idx));
        let style = if hot {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else if *active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!(" {mark} {name}"), style),
            Span::raw(format!("  {windows} windows")),
        ])));
    }
    f.render_widget(List::new(items), chunks[0]);

    state.regions.modal_detach = chunks[1];
    let hot = state.hover == Some(Hotspot::ModalDetach);
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
