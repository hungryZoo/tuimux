//! Default tuimux ratatui interface.
//!
//! The main pane is backed by tuimux's Rust-native daemon multiplexer:
//! windows and panes are owned by the daemon, and each pane runs a
//! real shell in a PTY rendered through a vt100 screen model.
//!
//! If stdout is not a TTY we refuse to enter raw mode and instead print guidance,
//! so piping or running under CI stays safe (PRD "keep safe").

use std::io::{self, IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::style::force_color_output;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::clipboard;
use crate::mux_backend::{KeyInput, MouseInput, MuxBackend, MuxSnapshot, PaneSnapshot};
use crate::native_mux::{Pane, PaneAxis, PaneRect, PaneSeparator, Window};
use crate::terminal::{SelectionRange, TerminalColor, TerminalSpan, TerminalStyle};

/// Why the UI loop ended — affects the farewell message.
enum Exit {
    Quit,
    Detach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hotspot {
    DetachButton,
    StatusPanel,
    MainPane,
    Window(usize),
    WindowClose(usize),
    NewWindow,
    ContextMenu(ContextMenuAction),
}

#[derive(Default, Clone, Copy)]
struct Regions {
    main_pane: Rect,
    terminal_body: Rect,
    detach_button: Rect,
    status_panel: Rect,
    new_window: Rect,
    windows: [Rect; 8],
    window_close: [Rect; 8],
    window_count: usize,
    terminal_panes: [Rect; 8],
    terminal_pane_count: usize,
    context_menu: Rect,
    context_menu_items: [Rect; 3],
    context_menu_count: usize,
}

struct UiState {
    hover: Option<Hotspot>,
    regions: Regions,
    mux: MuxBackend,
    /// Live native mux state, refreshed after every mutating command.
    windows: Vec<Window>,
    panes: Vec<Pane>,
    /// Non-fatal, transient message shown in the status bar (e.g. that a
    /// window was created, selected, or closed).
    status: Option<String>,
    terminal_error: Option<String>,
    terminal_mode: bool,
    selection: Option<SelectionState>,
    pending_left_down: Option<PendingMouseDown>,
    paste_highlight_pending: bool,
    raw_key_tail: String,
    context_menu: Option<ContextMenuState>,
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

#[derive(Debug, Clone, Copy)]
struct PendingMouseDown {
    pane: usize,
    row: u16,
    col: u16,
    modifiers: KeyModifiers,
    child_wants_mouse: bool,
}

#[derive(Debug, Clone, Copy)]
struct ContextMenuState {
    anchor: (u16, u16),
    selected: ContextMenuAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuAction {
    Copy,
    Paste,
    Cancel,
}

#[derive(Debug, Clone, Copy)]
struct TerminalModeLayout {
    terminal_body: Rect,
    side_rail: Option<Rect>,
}

const WINDOW_CLOSE_WIDTH: u16 = 3;
const CONTEXT_MENU_WIDTH: u16 = 14;
const CONTEXT_MENU_HEIGHT: u16 = 5;
const CONTEXT_MENU_ACTIONS: [ContextMenuAction; 3] = [
    ContextMenuAction::Copy,
    ContextMenuAction::Paste,
    ContextMenuAction::Cancel,
];
const RAW_BRACKETED_PASTE_END: &str = "\x1b[201~";

fn andromeda_starlight() -> Color {
    Color::Rgb(0xF3, 0xD5, 0x6E)
}

fn andromeda_nova() -> Color {
    Color::Rgb(0xFF, 0x00, 0x7A)
}

fn andromeda_comet() -> Color {
    Color::Rgb(0x00, 0xB8, 0xD4)
}

fn andromeda_aurora() -> Color {
    Color::Rgb(0x96, 0xE0, 0x72)
}

fn andromeda_cosmic() -> Color {
    Color::Rgb(0xC7, 0x4D, 0xED)
}

impl UiState {
    /// Build initial state from the native multiplexer.
    fn bootstrap(socket_scope: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let mux = MuxBackend::new(socket_scope, cwd, 80, 24)?;
        let mut state = UiState {
            hover: None,
            regions: Regions::default(),
            mux,
            windows: Vec::new(),
            panes: Vec::new(),
            status: None,
            terminal_error: None,
            terminal_mode: true,
            selection: None,
            pending_left_down: None,
            paste_highlight_pending: false,
            raw_key_tail: String::new(),
            context_menu: None,
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
        self.windows = snapshot.windows;
        self.panes = snapshot.panes;
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
        self.pending_left_down = None;
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
        self.pending_left_down = None;
        self.paste_highlight_pending = false;
    }

    fn open_context_menu(&mut self, x: u16, y: u16) {
        self.pending_left_down = None;
        self.context_menu = Some(ContextMenuState {
            anchor: (x, y),
            selected: ContextMenuAction::Copy,
        });
        self.hover = Some(Hotspot::ContextMenu(ContextMenuAction::Copy));
    }

    fn close_context_menu(&mut self) {
        self.context_menu = None;
        if matches!(self.hover, Some(Hotspot::ContextMenu(_))) {
            self.hover = None;
        }
    }

    fn select_context_menu_action(&mut self, action: ContextMenuAction) {
        if let Some(menu) = &mut self.context_menu {
            menu.selected = action;
        }
        self.hover = Some(Hotspot::ContextMenu(action));
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
        if is_copy_shortcut(key) {
            if self.selection_range().is_some() {
                self.copy_selection();
                return;
            }
            if !is_plain_control_c(key) {
                self.status = Some("nothing selected".to_string());
                return;
            }
        }

        if is_paste_shortcut(key) {
            self.paste_clipboard();
            return;
        }

        let key = terminal_line_boundary_key(key).unwrap_or(key);

        if key.modifiers.contains(KeyModifiers::SUPER) {
            self.status = Some("shortcut ignored".to_string());
            return;
        }

        if self.selection.is_some() {
            self.clear_selection();
        }

        let completed_raw_paste = self.observe_raw_bracketed_paste_key(key);
        self.paste_highlight_pending = false;
        self.send_terminal_key_event(key);
        if completed_raw_paste {
            self.paste_highlight_pending = true;
        }
    }

    fn observe_raw_bracketed_paste_key(&mut self, key: KeyEvent) -> bool {
        let Some(ch) = raw_key_sequence_char(key) else {
            self.raw_key_tail.clear();
            return false;
        };

        if ch == '\x1b' {
            self.raw_key_tail.clear();
        }
        self.raw_key_tail.push(ch);
        if self.raw_key_tail.len() > RAW_BRACKETED_PASTE_END.len() {
            let excess = self
                .raw_key_tail
                .len()
                .saturating_sub(RAW_BRACKETED_PASTE_END.len());
            self.raw_key_tail.drain(..excess);
        }
        self.raw_key_tail.ends_with(RAW_BRACKETED_PASTE_END)
    }

    fn send_terminal_key_event(&mut self, key: KeyEvent) {
        if let Some(key) = KeyInput::from_event(key) {
            if let Err(e) = self.mux.send_key(key) {
                self.status = Some(format!("terminal input failed: {e}"));
            }
        }
    }

    fn send_terminal_paste(&mut self, text: &str) {
        self.clear_selection();
        match self.mux.send_paste(text) {
            Ok(()) => {
                self.paste_highlight_pending = !text.is_empty();
            }
            Err(e) => {
                self.paste_highlight_pending = false;
                self.status = Some(format!("terminal paste failed: {e}"));
            }
        }
    }

    fn paste_clipboard(&mut self) -> bool {
        self.clear_selection();
        self.paste_highlight_pending = false;
        match clipboard::read_text() {
            Ok(text) if text.is_empty() => {
                self.status = Some("clipboard is empty".to_string());
                false
            }
            Ok(text) => {
                let chars = text.chars().count();
                match self.mux.send_paste(&text) {
                    Ok(()) => {
                        self.paste_highlight_pending = true;
                        self.status = Some(format!("pasted {chars} chars"));
                        true
                    }
                    Err(e) => {
                        self.status = Some(format!("terminal paste failed: {e}"));
                        false
                    }
                }
            }
            Err(e) => {
                self.status = Some(format!("paste failed: {e}"));
                false
            }
        }
    }

    fn clear_paste_highlight_on_click(&mut self) {
        if !self.paste_highlight_pending {
            return;
        }

        self.paste_highlight_pending = false;
        self.send_terminal_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        self.send_terminal_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    }

    fn send_terminal_mouse(
        &mut self,
        kind: MouseEventKind,
        row: u16,
        col: u16,
        modifiers: KeyModifiers,
    ) {
        self.paste_highlight_pending = false;
        if let Some(mouse) = MouseInput::from_parts(kind, row, col, modifiers) {
            if let Err(e) = self.mux.send_mouse(mouse) {
                self.status = Some(format!("terminal mouse failed: {e}"));
            }
        }
    }
}

fn is_copy_shortcut(key: KeyEvent) -> bool {
    shortcut_char(key, 'c')
        && (is_plain_control_c(key)
            || is_control_shift_shortcut(key.modifiers)
            || is_macos_shift_command(key.modifiers))
}

fn is_plain_control_c(key: KeyEvent) -> bool {
    shortcut_char(key, 'c') && key.modifiers == KeyModifiers::CONTROL
}

fn is_paste_shortcut(key: KeyEvent) -> bool {
    shortcut_char(key, 'v')
        && (key.modifiers == KeyModifiers::CONTROL
            || is_control_shift_shortcut(key.modifiers)
            || is_macos_shift_command(key.modifiers))
}

fn shortcut_char(key: KeyEvent, expected: char) -> bool {
    matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&expected))
}

fn terminal_line_boundary_key(key: KeyEvent) -> Option<KeyEvent> {
    match (key.code, key.modifiers) {
        (KeyCode::Left, modifiers) if is_macos_shift_command(modifiers) => {
            Some(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
        }
        (KeyCode::Right, modifiers) if is_macos_shift_command(modifiers) => {
            Some(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
        }
        _ => None,
    }
}

fn raw_key_sequence_char(key: KeyEvent) -> Option<char> {
    if key.modifiers != KeyModifiers::NONE {
        return None;
    }

    match key.code {
        KeyCode::Esc => Some('\x1b'),
        KeyCode::Char(ch) if ch.is_ascii() => Some(ch),
        _ => None,
    }
}

fn is_macos_shift_command(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SUPER)
        && modifiers.contains(KeyModifiers::SHIFT)
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::ALT)
}

fn is_control_shift_shortcut(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && modifiers.contains(KeyModifiers::SHIFT)
        && !modifiers.contains(KeyModifiers::SUPER)
        && !modifiers.contains(KeyModifiers::ALT)
}

/// Entry point for the default run. Returns a process exit code.
pub fn run(socket_scope: &str, cwd: PathBuf) -> io::Result<i32> {
    if !io::stdout().is_terminal() {
        eprintln!(
            "tuimux: stdout is not a terminal — refusing to start the interactive UI.\n\
             Try one of:\n  tuimux --layout-preview   # render the layout as text\n  \
             tuimux --doctor           # check your environment"
        );
        return Ok(2);
    }

    let mut state = UiState::bootstrap(socket_scope, cwd)
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
    preserve_child_terminal_colors();
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

fn preserve_child_terminal_colors() {
    // tuimux is rendering a child terminal, so parent-side NO_COLOR must not
    // strip ANSI colors that the child process explicitly emitted.
    force_color_output(true);
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
                if handle_context_menu_key(state, key) {
                    continue;
                }

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
                    (KeyCode::Char('n'), KeyModifiers::ALT) => {
                        new_window(state);
                    }
                    (KeyCode::Left, KeyModifiers::ALT) => {
                        select_adjacent_window(state, -1);
                    }
                    (KeyCode::Right, KeyModifiers::ALT) => {
                        select_adjacent_window(state, 1);
                    }
                    _ if state.terminal_mode => {
                        state.send_terminal_key(key);
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Exit::Quit),
                    (KeyCode::Char('q'), _) => return Ok(Exit::Quit),
                    (KeyCode::Esc, _) => return Ok(Exit::Quit),
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
                if handle_context_menu_mouse(state, &mouse) {
                    continue;
                }

                if handle_context_menu_request(state, mouse.kind, mouse.column, mouse.row) {
                    continue;
                }

                if should_clear_paste_highlight_for_click(state, &mouse) {
                    state.clear_paste_highlight_on_click();
                }

                if handle_pending_left_down(state, &mouse) {
                    continue;
                }

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
                        MouseEventKind::Down(MouseButton::Left) => {
                            state.terminal_mode = true;
                            if selection_gesture {
                                state.begin_selection(pane_row, row, col);
                            } else {
                                state.pending_left_down = Some(PendingMouseDown {
                                    pane: pane_row,
                                    row,
                                    col,
                                    modifiers: mouse.modifiers,
                                    child_wants_mouse,
                                });
                            }
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

                state.hover = hit_test(mouse.column, mouse.row, &state.regions);
                if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                    match state.hover {
                        Some(Hotspot::DetachButton) => {
                            return Ok(Exit::Detach);
                        }
                        Some(Hotspot::StatusPanel) => {
                            scroll_active_pane(state, 0);
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

fn handle_pending_left_down(state: &mut UiState, mouse: &MouseEvent) -> bool {
    let Some(pending) = state.pending_left_down else {
        return false;
    };

    match mouse.kind {
        MouseEventKind::Drag(MouseButton::Left) => {
            let Some((row, col)) =
                terminal_cell_for_pane(mouse.column, mouse.row, &state.regions, pending.pane)
            else {
                state.pending_left_down = None;
                return true;
            };
            state.terminal_mode = true;
            state.begin_selection(pending.pane, pending.row, pending.col);
            state.update_selection(row, col);
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            state.pending_left_down = None;
            if pending.child_wants_mouse && state.terminal_mode {
                state.send_terminal_mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    pending.row,
                    pending.col,
                    pending.modifiers,
                );
                let (row, col) =
                    terminal_cell_for_pane(mouse.column, mouse.row, &state.regions, pending.pane)
                        .unwrap_or((pending.row, pending.col));
                state.send_terminal_mouse(
                    MouseEventKind::Up(MouseButton::Left),
                    row,
                    col,
                    mouse.modifiers,
                );
            }
            true
        }
        _ => {
            state.pending_left_down = None;
            false
        }
    }
}

fn should_clear_paste_highlight_for_click(state: &UiState, mouse: &MouseEvent) -> bool {
    if !state.paste_highlight_pending
        || !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
    {
        return false;
    }

    terminal_cell_at_pane(mouse.column, mouse.row, &state.regions)
        .map(|(pane_row, _, _)| !state.pane_mouse_protocol_active(pane_row))
        .unwrap_or(true)
}

fn handle_context_menu_key(state: &mut UiState, key: KeyEvent) -> bool {
    let Some(menu) = state.context_menu else {
        return false;
    };

    match key.code {
        KeyCode::Esc => {
            state.close_context_menu();
            true
        }
        KeyCode::Up => {
            let index = context_menu_action_index(menu.selected);
            let next = (index + CONTEXT_MENU_ACTIONS.len() - 1) % CONTEXT_MENU_ACTIONS.len();
            state.select_context_menu_action(CONTEXT_MENU_ACTIONS[next]);
            true
        }
        KeyCode::Down => {
            let index = context_menu_action_index(menu.selected);
            let next = (index + 1) % CONTEXT_MENU_ACTIONS.len();
            state.select_context_menu_action(CONTEXT_MENU_ACTIONS[next]);
            true
        }
        KeyCode::Enter => {
            perform_context_menu_action(state, menu.selected);
            true
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            perform_context_menu_action(state, ContextMenuAction::Copy);
            true
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            perform_context_menu_action(state, ContextMenuAction::Paste);
            true
        }
        _ => true,
    }
}

fn handle_context_menu_mouse(state: &mut UiState, mouse: &MouseEvent) -> bool {
    if state.context_menu.is_none() {
        return false;
    }

    match mouse.kind {
        MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(action) = context_menu_action_at(mouse.column, mouse.row, &state.regions) {
                state.select_context_menu_action(action);
            }
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(action) = context_menu_action_at(mouse.column, mouse.row, &state.regions) {
                state.select_context_menu_action(action);
            } else {
                state.close_context_menu();
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(action) = context_menu_action_at(mouse.column, mouse.row, &state.regions) {
                perform_context_menu_action(state, action);
            } else {
                state.close_context_menu();
            }
            true
        }
        MouseEventKind::Down(MouseButton::Right) => {
            state.open_context_menu(mouse.column, mouse.row);
            true
        }
        MouseEventKind::Up(MouseButton::Right) | MouseEventKind::Drag(MouseButton::Right) => true,
        _ => true,
    }
}

fn perform_context_menu_action(state: &mut UiState, action: ContextMenuAction) {
    state.close_context_menu();
    match action {
        ContextMenuAction::Copy => {
            if !state.copy_selection() {
                state.status = Some("nothing selected".to_string());
            }
        }
        ContextMenuAction::Paste => {
            state.paste_clipboard();
        }
        ContextMenuAction::Cancel => {
            state.status = Some("context menu cancelled".to_string());
        }
    }
}

fn handle_context_menu_request(state: &mut UiState, kind: MouseEventKind, x: u16, y: u16) -> bool {
    match kind {
        MouseEventKind::Down(MouseButton::Right) => {
            state.open_context_menu(x, y);
            true
        }
        MouseEventKind::Up(MouseButton::Right) | MouseEventKind::Drag(MouseButton::Right) => true,
        _ => false,
    }
}

fn context_menu_action_index(action: ContextMenuAction) -> usize {
    CONTEXT_MENU_ACTIONS
        .iter()
        .position(|candidate| *candidate == action)
        .unwrap_or(0)
}

fn context_menu_action_at(x: u16, y: u16, regions: &Regions) -> Option<ContextMenuAction> {
    for idx in 0..regions.context_menu_count {
        if contains(regions.context_menu_items[idx], x, y) {
            return Some(CONTEXT_MENU_ACTIONS[idx]);
        }
    }
    None
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
        Ok(_) => {
            state.clear_selection();
            state.sync_terminal(width, height);
        }
        Err(e) => state.status = Some(format!("new-window failed: {e}")),
    }
}

fn ui(f: &mut Frame, state: &mut UiState) {
    let root = f.size();
    state.regions = Regions::default();
    let terminal_axis = state.terminal_axis;
    let terminal_separators = state.terminal_separators.clone();
    let terminal_panes = state.terminal_panes.clone();
    let terminal_rows = state.terminal_rows.clone();
    let terminal_cursor = state.terminal_cursor;
    let terminal_hide_cursor = state.terminal_hide_cursor;

    if state.terminal_mode {
        let layout = terminal_mode_layout(root);
        render_main(
            f,
            layout.terminal_body,
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
        if let Some(side_rail) = layout.side_rail {
            render_terminal_rail(
                f,
                side_rail,
                &state.windows,
                state.terminal_scrollback,
                state.hover,
                &mut state.regions,
            );
        }
        render_context_menu(f, root, state.context_menu, state.hover, &mut state.regions);
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
        !terminal_hide_cursor,
        terminal_cursor,
        state.terminal_error.as_deref(),
        &mut state.regions,
        true,
    );
    render_sidebar(
        f,
        body[1],
        &state.windows,
        state.status.as_deref(),
        state.hover,
        &mut state.regions,
    );
    render_context_menu(f, root, state.context_menu, state.hover, &mut state.regions);
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

fn terminal_mode_layout(area: Rect) -> TerminalModeLayout {
    if area.width < 100 || area.height < 8 {
        return TerminalModeLayout {
            terminal_body: area,
            side_rail: None,
        };
    }

    let rail_width = 20.min(area.width.saturating_sub(80));
    if rail_width < 16 {
        return TerminalModeLayout {
            terminal_body: area,
            side_rail: None,
        };
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(80), Constraint::Length(rail_width)])
        .split(area);

    TerminalModeLayout {
        terminal_body: chunks[0],
        side_rail: Some(chunks[1]),
    }
}

fn render_terminal_rail(
    f: &mut Frame,
    area: Rect,
    windows: &[Window],
    scrollback: usize,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    regions.window_count = 0;
    regions.new_window = Rect::default();
    regions.detach_button = Rect::default();
    regions.status_panel = Rect::default();
    if area.width == 0 || area.height == 0 {
        return;
    }

    f.render_widget(Clear, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);

    regions.detach_button = chunks[0];

    let detach_hot = hover == Some(Hotspot::DetachButton);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default()
            .fg(andromeda_nova())
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, andromeda_nova(), detach_hot));
    f.render_widget(detach, chunks[0]);

    render_terminal_windows(f, chunks[1], windows, hover, regions);
    render_terminal_status(f, chunks[2], scrollback, hover, regions);
}

fn render_terminal_windows(
    f: &mut Frame,
    area: Rect,
    windows: &[Window],
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let windows_style = Style::default()
        .fg(andromeda_starlight())
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(windows_style)
        .title(Span::styled(" WINDOWS ", windows_style));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let capacity = inner.height as usize;
    let max_windows = capacity.saturating_sub(1).min(regions.windows.len());
    let mut items = Vec::new();

    for (row, win) in windows.iter().take(max_windows).enumerate() {
        let y = inner.y.saturating_add(row as u16);
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        let close_rect = Rect::new(
            row_rect
                .x
                .saturating_add(row_rect.width.saturating_sub(WINDOW_CLOSE_WIDTH)),
            y,
            WINDOW_CLOSE_WIDTH.min(row_rect.width),
            1,
        );
        regions.windows[row] = row_rect;
        regions.window_close[row] = close_rect;
        regions.window_count += 1;

        let marker = if win.active { "▸" } else { " " };
        items.push(ListItem::new(window_row_line(
            marker,
            win,
            area.width.saturating_sub(2),
            hover,
            row,
        )));
    }

    if items.len() < capacity {
        let row = items.len() as u16;
        regions.new_window = Rect::new(inner.x, inner.y.saturating_add(row), inner.width, 1);
        let new_hot = hover == Some(Hotspot::NewWindow);
        let new_style = if new_hot {
            Style::default().fg(Color::Black).bg(andromeda_aurora())
        } else {
            Style::default()
                .fg(andromeda_aurora())
                .add_modifier(Modifier::BOLD)
        };
        items.push(ListItem::new(Line::from(Span::styled(
            "  + new", new_style,
        ))));
    }

    f.render_widget(List::new(items), inner);
}

fn render_terminal_status(
    f: &mut Frame,
    area: Rect,
    scrollback: usize,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    regions.status_panel = area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let hot = hover == Some(Hotspot::StatusPanel);
    let status_style = if hot {
        Style::default()
            .fg(Color::Black)
            .bg(andromeda_comet())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(andromeda_comet())
            .add_modifier(Modifier::BOLD)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(status_style)
        .title(Span::styled(" STATUS ", status_style));
    let status = Paragraph::new(Line::from(Span::styled(
        fit_bar_text(
            &format!("scroll:{scrollback}"),
            area.width.saturating_sub(2) as usize,
        ),
        status_style,
    )))
    .alignment(Alignment::Center)
    .block(block);
    f.render_widget(status, area);
}

fn render_context_menu(
    f: &mut Frame,
    root: Rect,
    menu: Option<ContextMenuState>,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    regions.context_menu = Rect::default();
    regions.context_menu_items = [Rect::default(); 3];
    regions.context_menu_count = 0;

    let Some(menu) = menu else {
        return;
    };
    if root.width == 0 || root.height == 0 {
        return;
    }

    let area = context_menu_rect(root, menu.anchor);
    regions.context_menu = area;
    f.render_widget(Clear, area);

    let menu_style = Style::default()
        .fg(andromeda_starlight())
        .add_modifier(Modifier::BOLD);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(menu_style)
        .title(Span::styled(" MENU ", menu_style));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = CONTEXT_MENU_ACTIONS.len().min(inner.height as usize);
    regions.context_menu_count = rows;
    let items = CONTEXT_MENU_ACTIONS
        .iter()
        .take(rows)
        .enumerate()
        .map(|(idx, action)| {
            let rect = Rect::new(inner.x, inner.y.saturating_add(idx as u16), inner.width, 1);
            regions.context_menu_items[idx] = rect;
            ListItem::new(context_menu_line(
                *action,
                inner.width,
                menu.selected == *action || hover == Some(Hotspot::ContextMenu(*action)),
            ))
        })
        .collect::<Vec<_>>();

    f.render_widget(List::new(items), inner);
}

fn context_menu_rect(root: Rect, anchor: (u16, u16)) -> Rect {
    let width = CONTEXT_MENU_WIDTH.min(root.width);
    let height = CONTEXT_MENU_HEIGHT.min(root.height);
    let max_x = root.right().saturating_sub(width);
    let max_y = root.bottom().saturating_sub(height);
    Rect::new(
        anchor.0.clamp(root.x, max_x),
        anchor.1.clamp(root.y, max_y),
        width,
        height,
    )
}

fn context_menu_line(action: ContextMenuAction, width: u16, hot: bool) -> Line<'static> {
    let label = match action {
        ContextMenuAction::Copy => "Copy",
        ContextMenuAction::Paste => "Paste",
        ContextMenuAction::Cancel => "Cancel",
    };
    let prefix = match action {
        ContextMenuAction::Copy => "C",
        ContextMenuAction::Paste => "P",
        ContextMenuAction::Cancel => "Esc",
    };
    let text = fit_and_pad_text(&format!(" {prefix}  {label}"), width as usize);
    let color = match action {
        ContextMenuAction::Copy => andromeda_starlight(),
        ContextMenuAction::Paste => andromeda_aurora(),
        ContextMenuAction::Cancel => andromeda_comet(),
    };
    let style = if hot {
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    Line::from(Span::styled(text, style))
}

fn fit_bar_text(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        text.to_string()
    } else if width <= 1 {
        " ".repeat(width)
    } else {
        let mut fitted = text
            .chars()
            .take(width.saturating_sub(1))
            .collect::<String>();
        fitted.push('…');
        fitted
    }
}

fn fit_and_pad_text(text: &str, width: usize) -> String {
    let mut fitted = fit_bar_text(text, width);
    let used = fitted.chars().count();
    if used < width {
        fitted.push_str(&" ".repeat(width - used));
    }
    fitted
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
            .map(|row| Line::from(terminal_row_spans_for_width(row, inner.width)))
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
                .map(|row| Line::from(terminal_row_spans_for_width(row, rect.width)))
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

fn terminal_row_spans_for_width(row: Vec<TerminalSpan>, width: u16) -> Vec<Span<'static>> {
    let mut spans = terminal_row_spans(row);
    let used = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let width = width as usize;
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    spans
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
    windows: &[Window],
    status: Option<&str>,
    hover: Option<Hotspot>,
    regions: &mut Regions,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // detach button
            Constraint::Length(2), // status
            Constraint::Min(5),    // windows
        ])
        .split(area);

    regions.detach_button = chunks[0];

    let detach_hot = hover == Some(Hotspot::DetachButton);
    let detach = Paragraph::new(Line::from(Span::styled(
        "Detach",
        Style::default()
            .fg(andromeda_nova())
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(button_block(None, andromeda_nova(), detach_hot));
    f.render_widget(detach, chunks[0]);

    let status_text = fit_and_pad_text(status.unwrap_or_default(), chunks[1].width as usize);
    let status_line = Paragraph::new(Line::from(Span::styled(
        status_text,
        Style::default().fg(andromeda_comet()),
    )));
    f.render_widget(status_line, chunks[1]);

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
            row_rect
                .x
                .saturating_add(row_rect.width.saturating_sub(WINDOW_CLOSE_WIDTH)),
            y,
            WINDOW_CLOSE_WIDTH.min(row_rect.width),
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
        Style::default().fg(Color::Black).bg(andromeda_aurora())
    } else {
        Style::default()
            .fg(andromeda_aurora())
            .add_modifier(Modifier::BOLD)
    };
    win_items.push(ListItem::new(Line::from(Span::styled(
        "  + new", new_style,
    ))));

    let windows = List::new(win_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(andromeda_starlight()))
            .title(Span::styled(
                " WINDOWS ",
                Style::default()
                    .fg(andromeda_starlight())
                    .add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(windows, area);
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
    let close_width = WINDOW_CLOSE_WIDTH as usize;
    if width <= close_width {
        return Line::from(Span::styled(" X ".to_string(), close_style(close_hot)));
    }

    let label = format!("{marker} {}: {}", win.index, win.name);
    let label_width = width.saturating_sub(close_width);
    let label_len = label.chars().count();
    let label_text = if label_len >= label_width {
        label.chars().take(label_width).collect::<String>()
    } else {
        format!("{}{}", label, " ".repeat(label_width - label_len))
    };

    let row_hot = hover == Some(Hotspot::Window(row));
    let row_style = if row_hot {
        Style::default().fg(Color::Black).bg(andromeda_starlight())
    } else if win.active {
        Style::default()
            .fg(Color::Black)
            .bg(andromeda_cosmic())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(andromeda_starlight())
    };

    Line::from(vec![
        Span::styled(label_text, row_style),
        Span::styled(" X ", close_style(close_hot)),
    ])
}

fn close_style(hot: bool) -> Style {
    if hot {
        Style::default()
            .fg(Color::Black)
            .bg(andromeda_nova())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(andromeda_nova())
            .add_modifier(Modifier::BOLD)
    }
}

fn hit_test(x: u16, y: u16, regions: &Regions) -> Option<Hotspot> {
    if let Some(action) = context_menu_action_at(x, y, regions) {
        return Some(Hotspot::ContextMenu(action));
    }

    if contains(regions.main_pane, x, y) {
        return Some(Hotspot::MainPane);
    }

    if contains(regions.detach_button, x, y) {
        return Some(Hotspot::DetachButton);
    }
    if contains(regions.status_panel, x, y) {
        return Some(Hotspot::StatusPanel);
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
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::buffer::Cell;

    fn test_state() -> UiState {
        let mux = crate::native_mux::NativeMux::new(PathBuf::from("."), 20, 5).unwrap();
        UiState {
            hover: None,
            regions: Regions::default(),
            mux: MuxBackend::Local(mux),
            windows: Vec::new(),
            panes: Vec::new(),
            status: None,
            terminal_error: None,
            terminal_mode: true,
            selection: None,
            pending_left_down: None,
            paste_highlight_pending: false,
            raw_key_tail: String::new(),
            context_menu: None,
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

    fn rendered_line(terminal: &Terminal<TestBackend>, y: u16, width: u16) -> String {
        let buffer = terminal.backend().buffer();
        (0..width).map(|x| buffer.get(x, y).symbol()).collect()
    }

    fn first_cell_style_for_text(
        terminal: &Terminal<TestBackend>,
        y: u16,
        width: u16,
        text: &str,
    ) -> Style {
        let line = rendered_line(terminal, y, width);
        let x = line.find(text).expect("text is rendered") as u16;
        terminal.backend().buffer().get(x, y).style()
    }

    #[test]
    fn terminal_mode_narrow_layout_gives_full_area_to_pty() {
        let mut state = test_state();
        state.windows = vec![
            Window {
                index: 1,
                name: "shell".to_string(),
                active: true,
                panes: 1,
            },
            Window {
                index: 2,
                name: "logs".to_string(),
                active: false,
                panes: 1,
            },
        ];
        state.terminal_rows = vec![vec![TerminalSpan {
            text: "BODY_LINE".to_string(),
            style: TerminalStyle::default(),
        }]];

        let backend = TestBackend::new(72, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| ui(frame, &mut state)).unwrap();

        let body = rendered_line(&terminal, 0, 72);

        assert!(body.contains("BODY_LINE"), "{body:?}");
        assert_eq!(state.regions.terminal_body, Rect::new(0, 0, 72, 10));
        assert_eq!(terminal_cell_at_pane(0, 0, &state.regions), Some((0, 0, 0)));
        assert_eq!(
            terminal_cell_at_pane(71, 9, &state.regions),
            Some((0, 9, 71))
        );
        assert_eq!(hit_test(1, 0, &state.regions), Some(Hotspot::MainPane));
    }

    #[test]
    fn terminal_mode_no_longer_uses_compact_top_tabs() {
        let mut state = test_state();
        state.windows = vec![Window {
            index: 1,
            name: "shell".to_string(),
            active: true,
            panes: 1,
        }];

        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| ui(frame, &mut state)).unwrap();

        assert_eq!(state.regions.terminal_body, Rect::new(0, 0, 60, 8));
        assert_eq!(state.regions.window_count, 0);
        assert_eq!(state.regions.new_window, Rect::default());
        assert_eq!(hit_test(1, 0, &state.regions), Some(Hotspot::MainPane));
    }

    #[test]
    fn terminal_mode_wide_layout_integrates_boxed_rail_controls() {
        let mut state = test_state();
        state.terminal_scrollback = 7;
        state.windows = vec![
            Window {
                index: 1,
                name: "shell".to_string(),
                active: true,
                panes: 1,
            },
            Window {
                index: 2,
                name: "logs".to_string(),
                active: false,
                panes: 1,
            },
        ];
        state.terminal_rows = vec![vec![TerminalSpan {
            text: "BODY_LINE".to_string(),
            style: TerminalStyle::default(),
        }]];

        let backend = TestBackend::new(110, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| ui(frame, &mut state)).unwrap();

        let body = rendered_line(&terminal, 0, 90);
        let detach_label = rendered_line(&terminal, 1, 110);
        let sidebar_title = rendered_line(&terminal, 3, 110);
        let status_title = rendered_line(&terminal, 11, 110);
        let scrollback = rendered_line(&terminal, 12, 110);
        let full_screen = (0..14)
            .map(|y| rendered_line(&terminal, y, 110))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(body.contains("BODY_LINE"), "{body:?}");
        assert!(
            !full_screen.contains("Session"),
            "session panel should not render:\n{full_screen}"
        );
        assert!(detach_label.contains("Detach"), "{detach_label:?}");
        assert!(sidebar_title.contains("WINDOWS"), "{sidebar_title:?}");
        assert!(status_title.contains("STATUS"), "{status_title:?}");
        assert!(scrollback.contains("scroll:7"), "{scrollback:?}");
        assert_eq!(
            first_cell_style_for_text(&terminal, 1, 110, "Detach").fg,
            Some(andromeda_nova())
        );
        assert_eq!(
            first_cell_style_for_text(&terminal, 3, 110, "WINDOWS").fg,
            Some(andromeda_starlight())
        );
        assert_eq!(
            first_cell_style_for_text(&terminal, 11, 110, "STATUS").fg,
            Some(andromeda_comet())
        );
        assert_eq!(
            first_cell_style_for_text(&terminal, 12, 110, "scroll:7").fg,
            Some(andromeda_comet())
        );
        assert_eq!(state.regions.terminal_body, Rect::new(0, 0, 90, 14));
        assert_eq!(terminal_cell_at_pane(0, 0, &state.regions), Some((0, 0, 0)));
        assert_eq!(terminal_cell_at_pane(90, 0, &state.regions), None);

        assert_eq!(
            hit_test(
                state.regions.detach_button.x,
                state.regions.detach_button.y,
                &state.regions
            ),
            Some(Hotspot::DetachButton)
        );
        assert_eq!(
            hit_test(
                state.regions.status_panel.x,
                state.regions.status_panel.y,
                &state.regions
            ),
            Some(Hotspot::StatusPanel)
        );
        assert_eq!(
            hit_test(
                state.regions.windows[1].x,
                state.regions.windows[1].y,
                &state.regions
            ),
            Some(Hotspot::Window(1))
        );
        assert_eq!(
            hit_test(
                state.regions.new_window.x,
                state.regions.new_window.y,
                &state.regions
            ),
            Some(Hotspot::NewWindow)
        );
    }

    #[test]
    fn hit_test_prefers_window_close_x_over_window_row() {
        let mut regions = Regions::default();
        regions.windows[0] = Rect::new(10, 5, 20, 1);
        regions.window_close[0] = Rect::new(27, 5, 3, 1);
        regions.window_count = 1;

        assert_eq!(hit_test(27, 5, &regions), Some(Hotspot::WindowClose(0)));
        assert_eq!(hit_test(12, 5, &regions), Some(Hotspot::Window(0)));
    }

    #[test]
    fn right_click_context_menu_renders_copy_paste_cancel() {
        let mut state = test_state();
        state.context_menu = Some(ContextMenuState {
            anchor: (2, 2),
            selected: ContextMenuAction::Copy,
        });
        state.terminal_rows = vec![vec![TerminalSpan {
            text: "BODY_LINE".to_string(),
            style: TerminalStyle::default(),
        }]];

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| ui(frame, &mut state)).unwrap();

        let full_screen = (0..10)
            .map(|y| rendered_line(&terminal, y, 40))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(full_screen.contains("MENU"), "{full_screen}");
        assert!(full_screen.contains("Copy"), "{full_screen}");
        assert!(full_screen.contains("Paste"), "{full_screen}");
        assert!(full_screen.contains("Cancel"), "{full_screen}");
        assert_eq!(state.regions.context_menu_count, 3);
        assert_eq!(
            hit_test(
                state.regions.context_menu_items[0].x,
                state.regions.context_menu_items[0].y,
                &state.regions
            ),
            Some(Hotspot::ContextMenu(ContextMenuAction::Copy))
        );
    }

    #[test]
    fn context_menu_rect_clamps_to_root() {
        let rect = context_menu_rect(Rect::new(0, 0, 20, 8), (19, 7));

        assert_eq!(
            rect,
            Rect::new(6, 3, CONTEXT_MENU_WIDTH, CONTEXT_MENU_HEIGHT)
        );
    }

    #[test]
    fn right_click_request_opens_context_menu() {
        let mut state = test_state();

        let handled =
            handle_context_menu_request(&mut state, MouseEventKind::Down(MouseButton::Right), 7, 3);

        assert!(handled);
        assert_eq!(state.context_menu.map(|menu| menu.anchor), Some((7, 3)));
    }

    #[test]
    fn close_x_hover_gets_its_own_nova_style() {
        let active = Window {
            index: 1,
            name: "build".to_string(),
            active: true,
            panes: 1,
        };
        let row = window_row_line("▸", &active, 20, Some(Hotspot::WindowClose(0)), 0);
        let last = row.spans.last().expect("close span");
        assert_eq!(last.content.as_ref(), " X ");
        assert_eq!(last.style.fg, Some(Color::Black));
        assert_eq!(last.style.bg, Some(andromeda_nova()));
    }

    #[test]
    fn window_row_line_renders_osc_title_name() {
        let active = Window {
            index: 1,
            name: "OSC_TITLE".to_string(),
            active: true,
            panes: 1,
        };
        let row = window_row_line("▸", &active, 26, None, 0);
        let text = row
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("1: OSC_TITLE"), "{text:?}");
        assert_eq!(text.chars().count(), 26);
    }

    #[test]
    fn terminal_default_style_does_not_force_a_background() {
        let style = terminal_style(TerminalStyle::default());
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
    }

    #[test]
    fn terminal_shortcuts_support_host_copy_paste_variants() {
        assert!(is_copy_shortcut(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(is_copy_shortcut(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT
        )));
        assert!(is_copy_shortcut(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(is_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL
        )));
        assert!(is_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT
        )));
        assert!(is_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(!is_copy_shortcut(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
        assert!(!is_copy_shortcut(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER
        )));
        assert!(!is_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::SUPER
        )));
    }

    #[test]
    fn only_plain_control_c_falls_back_to_child_interrupt() {
        assert!(is_plain_control_c(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_plain_control_c(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(!is_plain_control_c(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER
        )));
    }

    #[test]
    fn macos_command_arrows_map_to_terminal_home_end() {
        assert_eq!(
            terminal_line_boundary_key(KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::SUPER | KeyModifiers::SHIFT
            ))
            .map(|key| (key.code, key.modifiers)),
            Some((KeyCode::Home, KeyModifiers::NONE))
        );
        assert_eq!(
            terminal_line_boundary_key(KeyEvent::new(
                KeyCode::Right,
                KeyModifiers::SUPER | KeyModifiers::SHIFT
            ))
            .map(|key| (key.code, key.modifiers)),
            Some((KeyCode::End, KeyModifiers::NONE))
        );
        assert!(
            terminal_line_boundary_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL))
                .is_none()
        );
        assert!(
            terminal_line_boundary_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER)).is_none()
        );
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
    fn terminal_truecolor_style_maps_to_ratatui_rgb() {
        let style = terminal_style(TerminalStyle {
            fg: TerminalColor::Rgb(12, 34, 56),
            bg: TerminalColor::Rgb(78, 90, 123),
            ..TerminalStyle::default()
        });

        assert_eq!(style.fg, Some(Color::Rgb(12, 34, 56)));
        assert_eq!(style.bg, Some(Color::Rgb(78, 90, 123)));
    }

    #[test]
    fn terminal_paragraph_rendering_preserves_truecolor_cells() {
        let rows = vec![
            Line::from(terminal_row_spans(vec![TerminalSpan {
                text: "FG_TRUECOLOR".to_string(),
                style: TerminalStyle {
                    fg: TerminalColor::Rgb(12, 34, 56),
                    ..TerminalStyle::default()
                },
            }])),
            Line::from(terminal_row_spans(vec![TerminalSpan {
                text: "BG_TRUECOLOR".to_string(),
                style: TerminalStyle {
                    bg: TerminalColor::Rgb(78, 90, 123),
                    ..TerminalStyle::default()
                },
            }])),
            Line::from(terminal_row_spans(vec![TerminalSpan {
                text: "DEFAULT_COLOR".to_string(),
                style: TerminalStyle::default(),
            }])),
        ];
        let backend = TestBackend::new(32, 3);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(rows.clone()).style(Style::default()),
                    frame.size(),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.get(0, 0).fg, Color::Rgb(12, 34, 56));
        assert_eq!(buffer.get(0, 0).bg, Color::Reset);
        assert_eq!(buffer.get(0, 1).fg, Color::Reset);
        assert_eq!(buffer.get(0, 1).bg, Color::Rgb(78, 90, 123));
        assert_eq!(buffer.get(0, 2).fg, Color::Reset);
        assert_eq!(buffer.get(0, 2).bg, Color::Reset);
    }

    #[test]
    fn terminal_row_padding_clears_stale_glyphs() {
        let backend = TestBackend::new(20, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(Line::from("PRIMARY_BEFORE_067")),
                    frame.size(),
                );
            })
            .unwrap();
        terminal
            .draw(|frame| {
                let line = Line::from(terminal_row_spans_for_width(
                    vec![TerminalSpan {
                        text: "PRIMARY_AFTER_067".to_string(),
                        style: TerminalStyle::default(),
                    }],
                    20,
                ));
                frame.render_widget(Paragraph::new(line), frame.size());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..20)
            .map(|x| buffer.get(x, 0).symbol())
            .collect::<String>();
        assert_eq!(rendered, "PRIMARY_AFTER_067   ");
    }

    #[test]
    fn terminal_row_padding_uses_display_width() {
        let spans = terminal_row_spans_for_width(
            vec![TerminalSpan {
                text: "한".to_string(),
                style: TerminalStyle::default(),
            }],
            4,
        );

        assert_eq!(spans.last().expect("padding span").content.as_ref(), "  ");
    }

    #[test]
    fn sidebar_status_padding_clears_previous_long_status() {
        let windows = vec![Window {
            index: 1,
            name: "shell".to_string(),
            active: true,
            panes: 1,
        }];
        let backend = TestBackend::new(40, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut regions = Regions::default();

        terminal
            .draw(|frame| {
                render_sidebar(
                    frame,
                    frame.size(),
                    &windows,
                    Some("scrollback 24 rows"),
                    None,
                    &mut regions,
                );
            })
            .unwrap();
        terminal
            .draw(|frame| {
                render_sidebar(
                    frame,
                    frame.size(),
                    &windows,
                    Some("navigation mode"),
                    None,
                    &mut regions,
                );
            })
            .unwrap();

        let status = rendered_line(&terminal, 3, 40);
        assert!(status.contains("navigation mode"), "{status:?}");
        assert!(!status.contains("rows"), "{status:?}");
    }

    #[test]
    fn crossterm_backend_emits_truecolor_sgr() {
        preserve_child_terminal_colors();

        let mut output = Vec::new();
        let mut cell = Cell::default();
        cell.set_symbol("F").set_style(
            Style::default()
                .fg(Color::Rgb(12, 34, 56))
                .bg(Color::Rgb(78, 90, 123)),
        );

        {
            let mut backend = CrosstermBackend::new(&mut output);
            backend.draw(vec![(0, 0, &cell)].into_iter()).unwrap();
        }

        let rendered = String::from_utf8_lossy(&output);
        assert!(rendered.contains("38;2;12;34;56"), "{rendered:?}");
        assert!(rendered.contains("48;2;78;90;123"), "{rendered:?}");
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
    fn pending_mouse_tracking_click_becomes_selection_when_dragged() {
        let mut state = test_state();
        state.regions.terminal_body = Rect::new(0, 0, 20, 5);
        state.pending_left_down = Some(PendingMouseDown {
            pane: 0,
            row: 1,
            col: 2,
            modifiers: KeyModifiers::NONE,
            child_wants_mouse: true,
        });

        let handled = handle_pending_left_down(
            &mut state,
            &MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 8,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert!(handled);
        assert!(state.pending_left_down.is_none());
        assert_eq!(
            state.selection_range(),
            Some(SelectionRange::new(1, 2, 1, 8))
        );
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
    fn successful_paste_marks_highlight_pending_until_click_clear() {
        let mut state = test_state();

        state.send_terminal_paste("pasted text");

        assert!(state.paste_highlight_pending);

        state.clear_paste_highlight_on_click();

        assert!(!state.paste_highlight_pending);
    }

    #[test]
    fn paste_highlight_click_clear_includes_ui_chrome_but_skips_mouse_apps() {
        let mut state = test_state();
        state.paste_highlight_pending = true;
        state.regions.terminal_pane_count = 1;
        state.regions.terminal_panes[0] = Rect::new(0, 0, 10, 5);

        let chrome_click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert!(should_clear_paste_highlight_for_click(
            &state,
            &chrome_click
        ));

        state.terminal_mouse_protocol_active = true;
        let terminal_click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert!(!should_clear_paste_highlight_for_click(
            &state,
            &terminal_click
        ));
    }

    #[test]
    fn raw_bracketed_paste_end_marks_highlight_pending() {
        let mut state = test_state();

        for ch in "\x1b[200~raw text\x1b[201~".chars() {
            let code = if ch == '\x1b' {
                KeyCode::Esc
            } else {
                KeyCode::Char(ch)
            };
            state.send_terminal_key(KeyEvent::new(code, KeyModifiers::NONE));
        }

        assert!(state.paste_highlight_pending);
    }

    #[test]
    fn hit_test_only_exposes_windows_not_deprecated_pane_controls() {
        let regions = Regions::default();

        assert_eq!(hit_test(21, 7, &regions), None);
        assert_eq!(hit_test(21, 8, &regions), None);
    }
}
