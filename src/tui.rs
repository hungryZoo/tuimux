//! The live terminal UI (`tuimux` with no subcommand).
//!
//! This is an MVP *scaffold*: it renders the real VS Code-inspired layout with a
//! live file explorer (from the current directory), a mock main area, and the
//! always-visible bottom menu bar. It does **not** yet drive a tmux control-mode
//! session — that is the next milestone (SRS FR-CONN). Quitting/"detaching" here
//! simply restores the terminal; no tmux session is touched.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::files::FileListing;
use crate::preview::PreviewData;
use crate::tmux::TmuxProbe;

/// Why the UI loop ended — affects the farewell message.
enum Exit {
    Quit,
    Detach,
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
    let result = run_loop(&mut terminal, probe, &data, &cwd);
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
    cwd: &std::path::Path,
) -> io::Result<Exit> {
    // The file listing is read once for the scaffold; the live client will
    // refresh it on focus/cwd changes (FR-FILES-3).
    let listing = FileListing::read(cwd);

    loop {
        terminal.draw(|f| ui(f, probe, data, &listing))?;

        // Poll so the loop stays event-driven (idle CPU ~0%, NFR-2).
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return Ok(Exit::Quit),
                    // Alt-d (or plain 'd') = Detach, per the bottom menu bar.
                    (KeyCode::Char('d'), _) => return Ok(Exit::Detach),
                    _ => {}
                }
            }
            // Mouse events are captured so the live client can route clicks to
            // regions; for now they don't change state.
            Event::Mouse(_) | Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn ui(f: &mut Frame, probe: &TmuxProbe, data: &PreviewData, listing: &FileListing) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status
            Constraint::Min(5),    // body
            Constraint::Length(3), // menu bar
        ])
        .split(f.size());

    // --- status line --------------------------------------------------------
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
            "  session: {} · {} · scaffold preview",
            data.session, tmux_desc
        )),
    ]));
    f.render_widget(status, root[0]);

    // --- body: explorer | main | right sidebar ------------------------------
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Min(20),
            Constraint::Length(26),
        ])
        .split(root[1]);

    render_explorer(f, body[0], listing);
    render_main(f, body[1], data);
    render_sidebar(f, body[2], data);

    // --- bottom menu bar ----------------------------------------------------
    let menu = Paragraph::new(Line::from(vec![
        menu_item("Detach", "Alt-d", Color::Yellow),
        menu_item("New", "Alt-n", Color::Green),
        menu_item("Split", "Alt-|", Color::Green),
        menu_item("Close", "Alt-w", Color::Red),
        menu_item("Help", "?", Color::Blue),
        menu_item("Palette", "Alt-p", Color::Magenta),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" menu "));
    f.render_widget(menu, root[2]);
}

fn menu_item(label: &str, key: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!(" [{label} {key}] "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn render_explorer(f: &mut Frame, area: Rect, listing: &FileListing) {
    let title = format!(" EXPLORER — {} ", listing.base_path.display());
    let mut items: Vec<ListItem> = Vec::new();
    for e in &listing.entries {
        let (name, style) = if e.is_dir {
            (
                format!("▸ {}/", e.name),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (format!("  {}", e.name), Style::default())
        };
        let line = Line::from(vec![
            Span::styled(name, style),
            Span::raw(" "),
            Span::styled(e.size_display.clone(), Style::default().fg(Color::DarkGray)),
        ]);
        items.push(ListItem::new(line));
    }
    if let Some(err) = &listing.error {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("! {err}"),
            Style::default().fg(Color::Red),
        ))));
    }
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_style(Style::default().add_modifier(Modifier::BOLD)),
    );
    f.render_widget(list, area);
}

fn render_main(f: &mut Frame, area: Rect, data: &PreviewData) {
    let lines: Vec<Line> = data.panes.iter().map(|p| Line::from(*p)).collect();
    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" MAIN AREA (tmux panes — mock) "),
    );
    f.render_widget(para, area);
}

fn render_sidebar(f: &mut Frame, area: Rect, data: &PreviewData) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // session
            Constraint::Min(4),    // windows
            Constraint::Length(6), // procs
        ])
        .split(area);

    // Session name (clickable in the live UI -> session modal).
    let session = Paragraph::new(Line::from(vec![
        Span::raw("session: "),
        Span::styled(
            data.session.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ▾"),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" SESSION "));
    f.render_widget(session, inner[0]);

    // Vertical window tab bar.
    let mut win_items: Vec<ListItem> = data
        .windows
        .iter()
        .map(|(idx, name, active)| {
            let marker = if *active { "▸" } else { " " };
            let style = if *active {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker} {idx}: {name}"),
                style,
            )))
        })
        .collect();
    win_items.push(ListItem::new(Line::from(Span::styled(
        "  + new",
        Style::default().fg(Color::Green),
    ))));
    let windows =
        List::new(win_items).block(Block::default().borders(Borders::ALL).title(" WINDOWS "));
    f.render_widget(windows, inner[1]);

    // PROCS list.
    let proc_items: Vec<ListItem> = data
        .procs
        .iter()
        .map(|(cmd, info)| {
            let color = if cmd.starts_with('✓') {
                Color::Green
            } else if cmd.starts_with('✗') {
                Color::Red
            } else {
                Color::Yellow
            };
            ListItem::new(Line::from(vec![
                Span::styled((*cmd).to_string(), Style::default().fg(color)),
                Span::raw("  "),
                Span::styled((*info).to_string(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let procs =
        List::new(proc_items).block(Block::default().borders(Borders::ALL).title(" PROCS "));
    f.render_widget(procs, inner[2]);
}
