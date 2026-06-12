//! PTY-backed tmux terminal surface.
//!
//! The old main pane used `tmux capture-pane` snapshots plus `send-keys`.
//! That looked like a terminal but did not maintain a real screen model. This
//! module follows the terminal pane architecture used by hungryZoo/tscode:
//! run a real process in a PTY, feed its byte stream into `vt100::Parser`, and
//! render the resulting cells with ratatui.

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use anyhow::{Context, Result};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use vt100::{Color as VtColor, MouseProtocolEncoding, MouseProtocolMode};

const SCROLLBACK: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpan {
    pub text: String,
    pub style: TerminalStyle,
}

pub struct TmuxTerminal {
    parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: Receiver<Vec<u8>>,
    rows: u16,
    cols: u16,
    session: String,
}

impl TmuxTerminal {
    pub fn new(tmux_binary: &str, session: &str, width: u16, height: u16) -> Result<Self> {
        let rows = height.max(1);
        let cols = width.max(2);
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut command = tmux_attach_command(tmux_binary, session);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "tuimux");

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("failed to spawn tmux client for session '{session}'"))?;
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
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            master,
            writer,
            child,
            rx,
            rows,
            cols,
            session: session.to_string(),
        })
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(bytes) = self.rx.try_recv() {
            self.parser.process(&bytes);
            changed = true;
        }
        changed
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

    pub fn styled_rows(&self) -> Vec<Vec<TerminalSpan>> {
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

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn send_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }

        if let Some(bytes) = key_to_bytes(key, self.parser.screen().application_cursor()) {
            self.writer.write_all(&bytes)?;
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn send_paste(&mut self, text: &str) -> Result<()> {
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

        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        Ok(true)
    }
}

impl Drop for TmuxTerminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(unix)]
fn tmux_attach_command(tmux_binary: &str, session: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("env");
    command.arg("-u");
    command.arg("TMUX");
    command.arg(tmux_binary);
    command.arg("-u");
    command.arg("attach-session");
    command.arg("-t");
    command.arg(session);
    command
}

#[cfg(not(unix))]
fn tmux_attach_command(tmux_binary: &str, session: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new(tmux_binary);
    command.arg("-u");
    command.arg("attach-session");
    command.arg("-t");
    command.arg(session);
    command
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
        KeyCode::Home => cursor_key('H', key.modifiers, application_cursor),
        KeyCode::End => cursor_key('F', key.modifiers, application_cursor),
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
    }
}
