//! PTY-backed terminal surface.
//!
//! tuimux owns real child processes directly: a shell, editor, monitor, or any
//! other terminal program runs in a PTY, its byte stream is fed into
//! `vt100::Parser`, and the resulting screen cells are rendered by ratatui.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use vt100::{Color as VtColor, MouseProtocolEncoding, MouseProtocolMode};

const SCROLLBACK: usize = 10_000;
const MAX_WINDOW_TITLE_CHARS: usize = 120;
const MAX_OSC52_BASE64_BYTES: usize = 1_048_576;
const MAX_OSC52_RESPONSE_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<VtColor> for TerminalColor {
    fn from(color: VtColor) -> Self {
        match color {
            VtColor::Default => TerminalColor::Default,
            VtColor::Idx(index) => TerminalColor::Indexed(index),
            VtColor::Rgb(red, green, blue) => TerminalColor::Rgb(red, green, blue),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStyle {
    pub fg: TerminalColor,
    pub bg: TerminalColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl TerminalStyle {
    fn from_cell(cell: &vt100::Cell) -> Self {
        Self {
            fg: TerminalColor::from(cell.fgcolor()),
            bg: TerminalColor::from(cell.bgcolor()),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    fn is_default(self) -> bool {
        self == Self::default()
    }
}

impl Default for TerminalStyle {
    fn default() -> Self {
        Self {
            fg: TerminalColor::Default,
            bg: TerminalColor::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpan {
    pub text: String,
    pub style: TerminalStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

impl SelectionRange {
    pub fn new(start_row: u16, start_col: u16, end_row: u16, end_col: u16) -> Self {
        Self {
            start_row,
            start_col,
            end_row,
            end_col,
        }
    }

    pub fn normalized(self) -> Self {
        if (self.start_row, self.start_col) <= (self.end_row, self.end_col) {
            self
        } else {
            Self {
                start_row: self.end_row,
                start_col: self.end_col,
                end_row: self.start_row,
                end_col: self.start_col,
            }
        }
    }

    fn contains_cell(self, row: u16, col: u16) -> bool {
        let range = self.normalized();
        (range.start_row, range.start_col) <= (row, col)
            && (row, col) <= (range.end_row, range.end_col)
    }
}

#[derive(Debug, Default)]
struct TerminalCallbacks {
    window_icon_name: Option<String>,
    window_title: Option<String>,
    pending_clipboard_copy: Option<String>,
    pending_clipboard_paste: Option<Vec<u8>>,
}

impl TerminalCallbacks {
    fn title(&self) -> Option<&str> {
        self.window_title
            .as_deref()
            .or(self.window_icon_name.as_deref())
    }

    fn take_clipboard_copy(&mut self) -> Option<String> {
        self.pending_clipboard_copy.take()
    }

    fn take_clipboard_paste(&mut self) -> Option<Vec<u8>> {
        self.pending_clipboard_paste.take()
    }
}

impl vt100::Callbacks for TerminalCallbacks {
    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, icon_name: &[u8]) {
        self.window_icon_name = sanitize_window_title(icon_name);
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.window_title = sanitize_window_title(title);
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, selector: &[u8], data: &[u8]) {
        if !selector_targets_system_clipboard(selector) || data.len() > MAX_OSC52_BASE64_BYTES {
            return;
        }

        let Ok(decoded) = BASE64_STANDARD.decode(data) else {
            return;
        };
        let text = String::from_utf8_lossy(&decoded).to_string();
        if !text.is_empty() {
            self.pending_clipboard_copy = Some(text);
        }
    }

    fn paste_from_clipboard(&mut self, _: &mut vt100::Screen, selector: &[u8]) {
        if selector_targets_system_clipboard(selector) {
            self.pending_clipboard_paste = Some(response_selector(selector));
        }
    }
}

fn selected_text_from_screen(screen: &vt100::Screen, selection: SelectionRange) -> String {
    let (rows, cols) = screen.size();
    if rows == 0 || cols == 0 {
        return String::new();
    }

    let range = selection.normalized();
    let start_row = range.start_row.min(rows.saturating_sub(1));
    let end_row = range.end_row.min(rows.saturating_sub(1));
    if start_row > end_row {
        return String::new();
    }

    let start_col = range.start_col.min(cols);
    let end_col_exclusive = range.end_col.saturating_add(1).min(cols);
    if start_row == end_row && start_col >= end_col_exclusive {
        return String::new();
    }

    screen.contents_between(start_row, start_col, end_row, end_col_exclusive)
}

pub struct PtyTerminal {
    parser: vt100::Parser<TerminalCallbacks>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    pending_parser_bytes: Vec<u8>,
    rows: u16,
    cols: u16,
    finished: bool,
}

impl PtyTerminal {
    pub fn new_shell(title: &str, cwd: &Path, width: u16, height: u16) -> Result<Self> {
        Self::spawn(shell_command(cwd), title.to_string(), width, height)
    }

    fn spawn(mut command: CommandBuilder, title: String, width: u16, height: u16) -> Result<Self> {
        let rows = height.max(1);
        let cols = width.max(2);
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "tuimux");

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("failed to spawn terminal process '{title}'"))?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = pair.master;
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                SCROLLBACK,
                TerminalCallbacks::default(),
            ),
            master,
            writer,
            child,
            rx,
            pending_parser_bytes: Vec::new(),
            rows,
            cols,
            finished: false,
        })
    }

    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    let bytes = normalize_terminal_input(&bytes, &mut self.pending_parser_bytes);
                    self.parser.process(&bytes);
                    self.copy_pending_clipboard();
                    self.respond_pending_clipboard_paste();
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        self.poll_finished();
        changed
    }

    pub fn title(&self) -> Option<&str> {
        self.parser.callbacks().title()
    }

    pub fn is_finished(&mut self) -> bool {
        self.poll_finished()
    }

    fn poll_finished(&mut self) -> bool {
        if self.finished {
            return true;
        }

        match self.child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                self.finished = true;
                true
            }
            Ok(None) => false,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        let rows = height.max(1);
        let cols = width.max(2);
        if self.rows == rows && self.cols == cols {
            return;
        }

        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    #[allow(dead_code)]
    pub fn styled_rows(&self) -> Vec<Vec<TerminalSpan>> {
        self.styled_rows_with_selection(None)
    }

    pub fn styled_rows_with_selection(
        &self,
        selection: Option<SelectionRange>,
    ) -> Vec<Vec<TerminalSpan>> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut rendered_rows = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut last_used_col = None;
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let style = TerminalStyle::from_cell(cell);
                if cell.has_contents() || !style.is_default() {
                    last_used_col = Some(if cell.is_wide() {
                        col.saturating_add(1)
                    } else {
                        col
                    });
                }
            }

            let Some(last_used_col) = last_used_col else {
                rendered_rows.push(Vec::new());
                continue;
            };

            let mut spans = Vec::new();
            let mut current_style = None::<TerminalStyle>;
            let mut current_text = String::new();

            for col in 0..=last_used_col.min(cols.saturating_sub(1)) {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }

                let style = TerminalStyle::from_cell(cell);
                let style = if selection
                    .map(|range| range.contains_cell(row, col))
                    .unwrap_or(false)
                {
                    TerminalStyle {
                        inverse: !style.inverse,
                        ..style
                    }
                } else {
                    style
                };
                let text = if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                };

                if current_style == Some(style) {
                    current_text.push_str(text);
                } else {
                    if let Some(style) = current_style {
                        spans.push(TerminalSpan {
                            text: std::mem::take(&mut current_text),
                            style,
                        });
                    }
                    current_style = Some(style);
                    current_text.push_str(text);
                }
            }

            if let Some(style) = current_style {
                spans.push(TerminalSpan {
                    text: current_text,
                    style,
                });
            }
            rendered_rows.push(spans);
        }

        rendered_rows
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    pub fn hide_cursor(&self) -> bool {
        self.parser.screen().hide_cursor()
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scrollback_up(&mut self, rows: usize) -> usize {
        let next = self.scrollback().saturating_add(rows);
        self.parser.screen_mut().set_scrollback(next);
        self.scrollback()
    }

    pub fn scrollback_down(&mut self, rows: usize) -> usize {
        let next = self.scrollback().saturating_sub(rows);
        self.parser.screen_mut().set_scrollback(next);
        self.scrollback()
    }

    pub fn scrollback_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn mouse_protocol_active(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != MouseProtocolMode::None
    }

    pub fn selected_text(&self, selection: SelectionRange) -> String {
        selected_text_from_screen(self.parser.screen(), selection)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn send_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }

        self.scrollback_bottom();
        if let Some(bytes) = key_to_bytes(key, self.parser.screen().application_cursor()) {
            self.writer.write_all(&bytes)?;
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn send_paste(&mut self, text: &str) -> Result<()> {
        self.scrollback_bottom();
        if self.bracketed_paste() {
            self.writer.write_all(b"\x1b[200~")?;
            self.writer.write_all(text.as_bytes())?;
            self.writer.write_all(b"\x1b[201~")?;
        } else {
            self.writer.write_all(text.as_bytes())?;
        }
        self.writer.flush()?;
        Ok(())
    }

    pub fn send_raw_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.scrollback_bottom();
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn send_mouse_event(
        &mut self,
        kind: MouseEventKind,
        row: u16,
        col: u16,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let mode = self.parser.screen().mouse_protocol_mode();
        if mode == MouseProtocolMode::None {
            return Ok(false);
        }

        let Some(bytes) = mouse_event_to_bytes(
            kind,
            row,
            col,
            modifiers,
            mode,
            self.parser.screen().mouse_protocol_encoding(),
        ) else {
            return Ok(true);
        };

        self.scrollback_bottom();
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        Ok(true)
    }

    fn copy_pending_clipboard(&mut self) {
        let Some(text) = self.parser.callbacks_mut().take_clipboard_copy() else {
            return;
        };
        let _ = crate::clipboard::copy_text(&text);
    }

    fn respond_pending_clipboard_paste(&mut self) {
        let Some(selector) = self.parser.callbacks_mut().take_clipboard_paste() else {
            return;
        };
        let Ok(text) = crate::clipboard::read_text() else {
            return;
        };
        let response = osc52_clipboard_response(&selector, &text);
        let _ = self.writer.write_all(&response);
        let _ = self.writer.flush();
    }
}

fn normalize_terminal_input(chunk: &[u8], pending: &mut Vec<u8>) -> Vec<u8> {
    let mut data = Vec::with_capacity(pending.len().saturating_add(chunk.len()));
    if !pending.is_empty() {
        data.extend_from_slice(pending);
        pending.clear();
    }
    data.extend_from_slice(chunk);

    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0x1b {
            out.push(data[i]);
            i += 1;
            continue;
        }

        if i + 1 >= data.len() {
            pending.extend_from_slice(&data[i..]);
            break;
        }

        if data[i + 1] != b'[' {
            out.extend_from_slice(&data[i..=i + 1]);
            i += 2;
            continue;
        }

        let start = i;
        let mut j = i + 2;
        let mut invalid = false;
        while j < data.len() {
            let byte = data[j];
            if (0x40..=0x7e).contains(&byte) {
                break;
            }
            if !(0x20..=0x3f).contains(&byte) {
                invalid = true;
                break;
            }
            j += 1;
        }

        if invalid {
            out.push(data[i]);
            i += 1;
            continue;
        }

        if j >= data.len() {
            pending.extend_from_slice(&data[start..]);
            break;
        }

        out.extend_from_slice(&data[start..j]);
        out.push(if data[j] == b'f' { b'H' } else { data[j] });
        i = j + 1;
    }

    out
}

impl Drop for PtyTerminal {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
        }
    }
}

#[cfg(unix)]
fn shell_command(cwd: &Path) -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = CommandBuilder::new(shell);
    command.cwd(cwd.as_os_str());
    command
}

#[cfg(not(unix))]
fn shell_command(cwd: &Path) -> CommandBuilder {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    let mut command = CommandBuilder::new(shell);
    command.cwd(cwd.as_os_str());
    command
}

fn sanitize_window_title(raw: &[u8]) -> Option<String> {
    let title = String::from_utf8_lossy(raw)
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_WINDOW_TITLE_CHARS)
        .collect::<String>();
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn selector_targets_system_clipboard(selector: &[u8]) -> bool {
    selector.is_empty() || selector.contains(&b'c')
}

fn response_selector(selector: &[u8]) -> Vec<u8> {
    if selector.is_empty() {
        b"c".to_vec()
    } else {
        selector.to_vec()
    }
}

fn osc52_clipboard_response(selector: &[u8], text: &str) -> Vec<u8> {
    let bytes = truncate_utf8_bytes(text, MAX_OSC52_RESPONSE_BYTES).as_bytes();
    let encoded = BASE64_STANDARD.encode(bytes);
    let selector = String::from_utf8_lossy(selector);
    format!("\x1b]52;{selector};{encoded}\x07").into_bytes()
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    &text[..end]
}

fn key_to_bytes(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return ctrl_byte(c).map(|byte| with_alt(vec![byte], alt));
        }
    }

    let bytes = match key.code {
        KeyCode::Backspace => backspace_key(key.modifiers),
        KeyCode::Enter => with_alt(b"\r".to_vec(), alt),
        KeyCode::Tab => with_alt(b"\t".to_vec(), alt),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Left => cursor_key('D', key.modifiers, application_cursor),
        KeyCode::Right => cursor_key('C', key.modifiers, application_cursor),
        KeyCode::Up => cursor_key('A', key.modifiers, application_cursor),
        KeyCode::Down => cursor_key('B', key.modifiers, application_cursor),
        KeyCode::Home => home_end_key('H', key.modifiers),
        KeyCode::End => home_end_key('F', key.modifiers),
        KeyCode::PageUp => csi_tilde(5, key.modifiers),
        KeyCode::PageDown => csi_tilde(6, key.modifiers),
        KeyCode::Delete => csi_tilde(3, key.modifiers),
        KeyCode::Insert => csi_tilde(2, key.modifiers),
        KeyCode::F(number) => function_key(number, key.modifiers)?,
        KeyCode::Null => b"\0".to_vec(),
        KeyCode::Char(c) => with_alt(c.to_string().into_bytes(), alt),
        _ => return None,
    };

    Some(bytes)
}

fn ctrl_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    if c.is_ascii_lowercase() {
        Some(c as u8 - b'a' + 1)
    } else {
        match c {
            ' ' | '@' => Some(0x00),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            '/' | '-' => Some(0x1f),
            '?' => Some(0x7f),
            '2' => Some(0x00),
            '3' => Some(0x1b),
            '4' => Some(0x1c),
            '5' => Some(0x1d),
            '6' => Some(0x1e),
            '7' => Some(0x1f),
            '8' => Some(0x7f),
            _ => None,
        }
    }
}

fn backspace_key(modifiers: KeyModifiers) -> Vec<u8> {
    let mut bytes = if modifiers.contains(KeyModifiers::CONTROL) {
        vec![0x17]
    } else {
        b"\x7f".to_vec()
    };
    if modifiers.contains(KeyModifiers::ALT) {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn with_alt(mut bytes: Vec<u8>, alt: bool) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn csi_arrow(final_byte: char, modifiers: KeyModifiers) -> Vec<u8> {
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[1;{code}{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn cursor_key(final_byte: char, modifiers: KeyModifiers, application_cursor: bool) -> Vec<u8> {
    if modifier_code(modifiers).is_some() {
        return csi_arrow(final_byte, modifiers);
    }

    if application_cursor {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn home_end_key(final_byte: char, modifiers: KeyModifiers) -> Vec<u8> {
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[1;{code}{final_byte}").into_bytes()
    } else {
        format!("\x1bO{final_byte}").into_bytes()
    }
}

fn csi_tilde(number: u8, modifiers: KeyModifiers) -> Vec<u8> {
    if let Some(code) = modifier_code(modifiers) {
        format!("\x1b[{number};{code}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn function_key(number: u8, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    let final_byte = match number {
        1 => Some('P'),
        2 => Some('Q'),
        3 => Some('R'),
        4 => Some('S'),
        _ => None,
    };

    if let Some(final_byte) = final_byte {
        return Some(if let Some(code) = modifier_code(modifiers) {
            format!("\x1b[1;{code}{final_byte}").into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        });
    }

    let number = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return None,
    };
    Some(csi_tilde(number, modifiers))
}

fn modifier_code(modifiers: KeyModifiers) -> Option<u8> {
    let mut code = 1_u8;
    if modifiers.contains(KeyModifiers::SHIFT) {
        code += 1;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        code += 2;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        code += 4;
    }
    (code != 1).then_some(code)
}

fn mouse_event_to_bytes(
    kind: MouseEventKind,
    row: u16,
    col: u16,
    modifiers: KeyModifiers,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    let mut code = match kind {
        MouseEventKind::Down(button) => button_code(button)?,
        MouseEventKind::Up(button) if reports_release(mode) => match encoding {
            MouseProtocolEncoding::Sgr => button_code(button)?,
            MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => 3,
        },
        MouseEventKind::Drag(button) if reports_drag(mode) => button_code(button)? + 32,
        MouseEventKind::Moved if mode == MouseProtocolMode::AnyMotion => 35,
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
        _ => return None,
    };

    if modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }

    let x = col.saturating_add(1);
    let y = row.saturating_add(1);
    let release = matches!(kind, MouseEventKind::Up(_));

    match encoding {
        MouseProtocolEncoding::Sgr => {
            let final_byte = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{final_byte}").into_bytes())
        }
        MouseProtocolEncoding::Default => default_mouse_sequence(code, x, y),
        MouseProtocolEncoding::Utf8 => utf8_mouse_sequence(code, x, y),
    }
}

fn button_code(button: MouseButton) -> Option<u16> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
    }
}

fn reports_release(mode: MouseProtocolMode) -> bool {
    matches!(
        mode,
        MouseProtocolMode::PressRelease
            | MouseProtocolMode::ButtonMotion
            | MouseProtocolMode::AnyMotion
    )
}

fn reports_drag(mode: MouseProtocolMode) -> bool {
    matches!(
        mode,
        MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
    )
}

fn default_mouse_sequence(code: u16, x: u16, y: u16) -> Option<Vec<u8>> {
    let cb = u8::try_from(code.checked_add(32)?).ok()?;
    let cx = u8::try_from(x.checked_add(32)?).ok()?;
    let cy = u8::try_from(y.checked_add(32)?).ok()?;
    Some(vec![0x1b, b'[', b'M', cb, cx, cy])
}

fn utf8_mouse_sequence(code: u16, x: u16, y: u16) -> Option<Vec<u8>> {
    let mut bytes = vec![0x1b, b'[', b'M'];
    push_utf8_mouse_value(&mut bytes, code)?;
    push_utf8_mouse_value(&mut bytes, x)?;
    push_utf8_mouse_value(&mut bytes, y)?;
    Some(bytes)
}

fn push_utf8_mouse_value(bytes: &mut Vec<u8>, value: u16) -> Option<()> {
    let codepoint = u32::from(value.checked_add(32)?);
    let ch = char::from_u32(codepoint)?;
    let mut buffer = [0_u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn terminal_key_encoding_supports_application_cursor_and_function_keys() {
        assert_eq!(
            key_to_bytes(key(KeyCode::Up, KeyModifiers::NONE), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Up, KeyModifiers::NONE), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Left, KeyModifiers::CONTROL), true),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Home, KeyModifiers::NONE), false),
            Some(b"\x1bOH".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::End, KeyModifiers::NONE), false),
            Some(b"\x1bOF".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Home, KeyModifiers::CONTROL), false),
            Some(b"\x1b[1;5H".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::F(1), KeyModifiers::NONE), false),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::F(5), KeyModifiers::SHIFT), false),
            Some(b"\x1b[15;2~".to_vec())
        );
    }

    #[test]
    fn terminal_key_encoding_supports_shell_editing_shortcuts() {
        assert_eq!(
            key_to_bytes(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Backspace, KeyModifiers::ALT), false),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Backspace, KeyModifiers::CONTROL), false),
            Some(vec![0x17])
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Enter, KeyModifiers::ALT), false),
            Some(b"\x1b\r".to_vec())
        );
    }

    #[test]
    fn terminal_key_encoding_supports_control_punctuation() {
        assert_eq!(
            key_to_bytes(key(KeyCode::Char('/'), KeyModifiers::CONTROL), false),
            Some(vec![0x1f])
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Char('6'), KeyModifiers::CONTROL), false),
            Some(vec![0x1e])
        );
        assert_eq!(
            key_to_bytes(key(KeyCode::Char('8'), KeyModifiers::CONTROL), false),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn terminal_mouse_encoding_reports_sgr_press_release_drag_and_wheel() {
        assert_eq!(
            mouse_event_to_bytes(
                MouseEventKind::Down(MouseButton::Left),
                4,
                9,
                KeyModifiers::NONE,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            mouse_event_to_bytes(
                MouseEventKind::Up(MouseButton::Left),
                4,
                9,
                KeyModifiers::NONE,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<0;10;5m".to_vec())
        );
        assert_eq!(
            mouse_event_to_bytes(
                MouseEventKind::Drag(MouseButton::Right),
                0,
                0,
                KeyModifiers::CONTROL,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<50;1;1M".to_vec())
        );
        assert_eq!(
            mouse_event_to_bytes(
                MouseEventKind::ScrollDown,
                2,
                3,
                KeyModifiers::SHIFT,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<69;4;3M".to_vec())
        );
        assert_eq!(
            mouse_event_to_bytes(
                MouseEventKind::Drag(MouseButton::Left),
                1,
                2,
                KeyModifiers::SHIFT,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<36;3;2M".to_vec())
        );
    }

    #[test]
    fn terminal_parser_preserves_truecolor_cell_styles() {
        let mut parser = vt100::Parser::new(4, 40, 0);
        parser.process(
            b"\x1b[38;2;12;34;56mFG_TRUECOLOR\x1b[0m\r\n\
              \x1b[48;2;78;90;123mBG_TRUECOLOR\x1b[0m",
        );
        let screen = parser.screen();

        let fg = screen.cell(0, 0).expect("fg cell");
        assert_eq!(
            TerminalStyle::from_cell(fg).fg,
            TerminalColor::Rgb(12, 34, 56)
        );

        let bg = screen.cell(1, 0).expect("bg cell");
        assert_eq!(
            TerminalStyle::from_cell(bg).bg,
            TerminalColor::Rgb(78, 90, 123)
        );
    }

    #[test]
    fn selected_text_follows_soft_wrap_without_padding() {
        let mut parser = vt100::Parser::new(4, 10, 0);
        parser.process(b"1234567890ABC");

        assert!(parser.screen().row_wrapped(0));
        assert_eq!(
            selected_text_from_screen(parser.screen(), SelectionRange::new(0, 8, 1, 8)),
            "90ABC"
        );
    }

    #[test]
    fn selected_text_keeps_explicit_line_breaks_but_not_blank_drag_tail() {
        let mut parser = vt100::Parser::new(4, 10, 0);
        parser.process(b"foo\r\nbar");

        assert!(!parser.screen().row_wrapped(0));
        assert_eq!(
            selected_text_from_screen(parser.screen(), SelectionRange::new(0, 0, 1, 8)),
            "foo\nbar"
        );
        assert_eq!(
            selected_text_from_screen(parser.screen(), SelectionRange::new(1, 1, 1, 9)),
            "ar"
        );
        assert_eq!(
            selected_text_from_screen(parser.screen(), SelectionRange::new(1, 5, 1, 9)),
            ""
        );
    }

    #[test]
    fn terminal_input_normalizes_hvp_cursor_position() {
        let mut pending = Vec::new();
        let bytes = normalize_terminal_input(b"\x1b[2;3fX", &mut pending);
        assert_eq!(bytes, b"\x1b[2;3HX");
        assert!(pending.is_empty());

        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process(&bytes);

        assert_eq!(parser.screen().cell(1, 2).unwrap().contents(), "X");
        assert!(!parser.screen().cell(0, 0).unwrap().has_contents());
    }

    #[test]
    fn terminal_input_keeps_incomplete_csi_between_chunks() {
        let mut pending = Vec::new();
        let first = normalize_terminal_input(b"\x1b[2;3", &mut pending);
        assert!(first.is_empty());
        assert_eq!(pending, b"\x1b[2;3");

        let second = normalize_terminal_input(b"fX", &mut pending);
        assert_eq!(second, b"\x1b[2;3HX");
        assert!(pending.is_empty());

        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process(&second);
        assert_eq!(parser.screen().cell(1, 2).unwrap().contents(), "X");
    }

    #[test]
    fn terminal_input_leaves_other_csi_sequences_unchanged() {
        let mut pending = Vec::new();
        assert_eq!(
            normalize_terminal_input(b"\x1b[?25l\x1b[31mred", &mut pending),
            b"\x1b[?25l\x1b[31mred"
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn terminal_callbacks_track_osc_window_titles() {
        let mut parser = vt100::Parser::new_with_callbacks(4, 40, 0, TerminalCallbacks::default());
        parser.process(b"\x1b]2;BUILD WATCH\x07");
        assert_eq!(parser.callbacks().title(), Some("BUILD WATCH"));

        let mut icon_parser =
            vt100::Parser::new_with_callbacks(4, 40, 0, TerminalCallbacks::default());
        icon_parser.process(b"\x1b]1;ICON NAME\x07");
        assert_eq!(icon_parser.callbacks().title(), Some("ICON NAME"));
    }

    #[test]
    fn terminal_callbacks_accept_osc52_clipboard_copy() {
        let mut parser = vt100::Parser::new_with_callbacks(4, 40, 0, TerminalCallbacks::default());
        parser.process(b"\x1b]52;c;Q09QWSBUQVJHRVQ=\x07");

        assert_eq!(
            parser.callbacks_mut().take_clipboard_copy(),
            Some("COPY TARGET".to_string())
        );
    }

    #[test]
    fn terminal_callbacks_accept_osc52_clipboard_paste_query() {
        let mut parser = vt100::Parser::new_with_callbacks(4, 40, 0, TerminalCallbacks::default());
        parser.process(b"\x1b]52;c;?\x07");

        assert_eq!(
            parser.callbacks_mut().take_clipboard_paste(),
            Some(b"c".to_vec())
        );
    }

    #[test]
    fn osc52_clipboard_response_encodes_clipboard_text() {
        assert_eq!(
            osc52_clipboard_response(b"c", "PASTE TARGET"),
            b"\x1b]52;c;UEFTVEUgVEFSR0VU\x07".to_vec()
        );
    }
}
