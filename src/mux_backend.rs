//! Multiplexer backend boundary.
//!
//! The UI talks to this module instead of owning `NativeMux` directly. On Unix
//! platforms we prefer a small daemon process over a local in-process mux so
//! detach/re-attach can preserve PTY children.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use serde::{Deserialize, Serialize};

use crate::native_mux::{NativeMux, Session, Window};
use crate::terminal::{SelectionRange, TerminalSpan};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub rows: Vec<Vec<TerminalSpan>>,
    pub cursor: Option<(u16, u16)>,
    pub hide_cursor: bool,
    pub mouse_protocol_active: bool,
}

impl Default for TerminalSnapshot {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            cursor: None,
            hide_cursor: true,
            mouse_protocol_active: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MuxSnapshot {
    pub sessions: Vec<Session>,
    pub windows: Vec<Window>,
    pub current_session: String,
    pub terminal: TerminalSnapshot,
}

pub enum MuxBackend {
    #[cfg(unix)]
    Remote(RemoteMuxClient),
    #[allow(dead_code)]
    Local(NativeMux),
}

impl MuxBackend {
    pub fn new(initial_session: &str, cwd: PathBuf, width: u16, height: u16) -> Result<Self> {
        #[cfg(unix)]
        {
            let _ = (width, height);
            return RemoteMuxClient::connect_or_spawn(initial_session, cwd).map(MuxBackend::Remote);
        }

        #[cfg(not(unix))]
        {
            NativeMux::new(initial_session, cwd, width, height).map(MuxBackend::Local)
        }
    }

    pub fn snapshot(
        &mut self,
        width: u16,
        height: u16,
        selection: Option<SelectionRange>,
    ) -> Result<MuxSnapshot> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.snapshot(width, height, selection),
            MuxBackend::Local(mux) => {
                mux.resize_active(width, height);
                mux.drain_all();
                Ok(local_snapshot(mux, selection))
            }
        }
    }

    pub fn create_next_session(&mut self, width: u16, height: u16) -> Result<String> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.create_next_session(width, height),
            MuxBackend::Local(mux) => mux.create_next_session(width, height),
        }
    }

    pub fn switch_session_by_row(&mut self, row: usize) -> Result<()> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.switch_session_by_row(row),
            MuxBackend::Local(mux) => mux.switch_session_by_row(row),
        }
    }

    pub fn select_window_by_row(&mut self, row: usize) -> Result<()> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.select_window_by_row(row),
            MuxBackend::Local(mux) => mux.select_window_by_row(row),
        }
    }

    pub fn new_window(&mut self, width: u16, height: u16) -> Result<u32> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.new_window(width, height),
            MuxBackend::Local(mux) => mux.new_window(width, height),
        }
    }

    pub fn kill_window_by_row(&mut self, row: usize, width: u16, height: u16) -> Result<u32> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.kill_window_by_row(row, width, height),
            MuxBackend::Local(mux) => mux.kill_window_by_row(row, width, height),
        }
    }

    pub fn selected_text(&mut self, selection: SelectionRange) -> Result<String> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.selected_text(selection),
            MuxBackend::Local(mux) => mux
                .active_terminal()
                .map(|terminal| terminal.selected_text(selection))
                .ok_or_else(|| anyhow!("terminal is not ready")),
        }
    }

    pub fn send_key(&mut self, key: KeyInput) -> Result<()> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.send_key(key),
            MuxBackend::Local(mux) => {
                let Some(terminal) = mux.active_terminal_mut() else {
                    bail!("terminal is not ready");
                };
                terminal.send_key(key.to_event()?)?;
                Ok(())
            }
        }
    }

    pub fn send_paste(&mut self, text: &str) -> Result<()> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.send_paste(text),
            MuxBackend::Local(mux) => {
                let Some(terminal) = mux.active_terminal_mut() else {
                    bail!("terminal is not ready");
                };
                terminal.send_paste(text)?;
                Ok(())
            }
        }
    }

    pub fn send_mouse(&mut self, mouse: MouseInput) -> Result<()> {
        match self {
            #[cfg(unix)]
            MuxBackend::Remote(client) => client.send_mouse(mouse),
            MuxBackend::Local(mux) => {
                let Some(terminal) = mux.active_terminal_mut() else {
                    bail!("terminal is not ready");
                };
                terminal.send_mouse_event(
                    mouse.kind.to_event_kind(),
                    mouse.row,
                    mouse.col,
                    bits_to_modifiers(mouse.modifiers),
                )?;
                Ok(())
            }
        }
    }
}

fn local_snapshot(mux: &NativeMux, selection: Option<SelectionRange>) -> MuxSnapshot {
    let terminal = mux
        .active_terminal()
        .map(|terminal| TerminalSnapshot {
            rows: terminal.styled_rows_with_selection(selection),
            cursor: Some(terminal.cursor()),
            hide_cursor: terminal.hide_cursor(),
            mouse_protocol_active: terminal.mouse_protocol_active(),
        })
        .unwrap_or_default();

    MuxSnapshot {
        sessions: mux.session_infos(),
        windows: mux.window_infos(),
        current_session: mux.current_session_name().to_string(),
        terminal,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct KeyInput {
    code: KeyInputCode,
    modifiers: u8,
}

impl KeyInput {
    pub fn from_event(key: KeyEvent) -> Option<Self> {
        let code = match key.code {
            KeyCode::Backspace => KeyInputCode::Backspace,
            KeyCode::Enter => KeyInputCode::Enter,
            KeyCode::Left => KeyInputCode::Left,
            KeyCode::Right => KeyInputCode::Right,
            KeyCode::Up => KeyInputCode::Up,
            KeyCode::Down => KeyInputCode::Down,
            KeyCode::Home => KeyInputCode::Home,
            KeyCode::End => KeyInputCode::End,
            KeyCode::PageUp => KeyInputCode::PageUp,
            KeyCode::PageDown => KeyInputCode::PageDown,
            KeyCode::Tab => KeyInputCode::Tab,
            KeyCode::BackTab => KeyInputCode::BackTab,
            KeyCode::Delete => KeyInputCode::Delete,
            KeyCode::Insert => KeyInputCode::Insert,
            KeyCode::F(number) => KeyInputCode::F(number),
            KeyCode::Char(ch) => KeyInputCode::Char(ch),
            KeyCode::Null => KeyInputCode::Null,
            KeyCode::Esc => KeyInputCode::Esc,
            _ => return None,
        };
        Some(Self {
            code,
            modifiers: modifiers_to_bits(key.modifiers),
        })
    }

    fn to_event(self) -> Result<KeyEvent> {
        let code = match self.code {
            KeyInputCode::Backspace => KeyCode::Backspace,
            KeyInputCode::Enter => KeyCode::Enter,
            KeyInputCode::Left => KeyCode::Left,
            KeyInputCode::Right => KeyCode::Right,
            KeyInputCode::Up => KeyCode::Up,
            KeyInputCode::Down => KeyCode::Down,
            KeyInputCode::Home => KeyCode::Home,
            KeyInputCode::End => KeyCode::End,
            KeyInputCode::PageUp => KeyCode::PageUp,
            KeyInputCode::PageDown => KeyCode::PageDown,
            KeyInputCode::Tab => KeyCode::Tab,
            KeyInputCode::BackTab => KeyCode::BackTab,
            KeyInputCode::Delete => KeyCode::Delete,
            KeyInputCode::Insert => KeyCode::Insert,
            KeyInputCode::F(number) => KeyCode::F(number),
            KeyInputCode::Char(ch) => KeyCode::Char(ch),
            KeyInputCode::Null => KeyCode::Null,
            KeyInputCode::Esc => KeyCode::Esc,
        };
        Ok(KeyEvent::new(code, bits_to_modifiers(self.modifiers)))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum KeyInputCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Null,
    Esc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MouseInput {
    pub kind: MouseInputKind,
    pub row: u16,
    pub col: u16,
    pub modifiers: u8,
}

impl MouseInput {
    pub fn from_parts(
        kind: MouseEventKind,
        row: u16,
        col: u16,
        modifiers: KeyModifiers,
    ) -> Option<Self> {
        Some(Self {
            kind: MouseInputKind::from_event_kind(kind)?,
            row,
            col,
            modifiers: modifiers_to_bits(modifiers),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MouseInputKind {
    Down(MouseInputButton),
    Up(MouseInputButton),
    Drag(MouseInputButton),
    Moved,
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

impl MouseInputKind {
    fn from_event_kind(kind: MouseEventKind) -> Option<Self> {
        match kind {
            MouseEventKind::Down(button) => {
                Some(Self::Down(MouseInputButton::from_button(button)?))
            }
            MouseEventKind::Up(button) => Some(Self::Up(MouseInputButton::from_button(button)?)),
            MouseEventKind::Drag(button) => {
                Some(Self::Drag(MouseInputButton::from_button(button)?))
            }
            MouseEventKind::Moved => Some(Self::Moved),
            MouseEventKind::ScrollDown => Some(Self::ScrollDown),
            MouseEventKind::ScrollUp => Some(Self::ScrollUp),
            MouseEventKind::ScrollLeft => Some(Self::ScrollLeft),
            MouseEventKind::ScrollRight => Some(Self::ScrollRight),
        }
    }

    fn to_event_kind(self) -> MouseEventKind {
        match self {
            MouseInputKind::Down(button) => MouseEventKind::Down(button.to_button()),
            MouseInputKind::Up(button) => MouseEventKind::Up(button.to_button()),
            MouseInputKind::Drag(button) => MouseEventKind::Drag(button.to_button()),
            MouseInputKind::Moved => MouseEventKind::Moved,
            MouseInputKind::ScrollDown => MouseEventKind::ScrollDown,
            MouseInputKind::ScrollUp => MouseEventKind::ScrollUp,
            MouseInputKind::ScrollLeft => MouseEventKind::ScrollLeft,
            MouseInputKind::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MouseInputButton {
    Left,
    Right,
    Middle,
}

impl MouseInputButton {
    fn from_button(button: MouseButton) -> Option<Self> {
        match button {
            MouseButton::Left => Some(Self::Left),
            MouseButton::Right => Some(Self::Right),
            MouseButton::Middle => Some(Self::Middle),
        }
    }

    fn to_button(self) -> MouseButton {
        match self {
            MouseInputButton::Left => MouseButton::Left,
            MouseInputButton::Right => MouseButton::Right,
            MouseInputButton::Middle => MouseButton::Middle,
        }
    }
}

fn modifiers_to_bits(modifiers: KeyModifiers) -> u8 {
    let mut bits = 0_u8;
    if modifiers.contains(KeyModifiers::SHIFT) {
        bits |= 1;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 1 << 1;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        bits |= 1 << 2;
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        bits |= 1 << 3;
    }
    bits
}

fn bits_to_modifiers(bits: u8) -> KeyModifiers {
    let mut modifiers = KeyModifiers::empty();
    if bits & 1 != 0 {
        modifiers.insert(KeyModifiers::SHIFT);
    }
    if bits & (1 << 1) != 0 {
        modifiers.insert(KeyModifiers::CONTROL);
    }
    if bits & (1 << 2) != 0 {
        modifiers.insert(KeyModifiers::ALT);
    }
    if bits & (1 << 3) != 0 {
        modifiers.insert(KeyModifiers::SUPER);
    }
    modifiers
}

#[derive(Debug, Serialize, Deserialize)]
enum Request {
    Snapshot {
        width: u16,
        height: u16,
        selection: Option<SelectionRange>,
    },
    CreateNextSession {
        width: u16,
        height: u16,
    },
    SwitchSessionByRow {
        row: usize,
    },
    SelectWindowByRow {
        row: usize,
    },
    NewWindow {
        width: u16,
        height: u16,
    },
    KillWindowByRow {
        row: usize,
        width: u16,
        height: u16,
    },
    SelectedText {
        selection: SelectionRange,
    },
    SendKey {
        key: KeyInput,
    },
    SendPaste {
        text: String,
    },
    SendMouse {
        mouse: MouseInput,
    },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
enum Response {
    Ok,
    Snapshot(MuxSnapshot),
    Name(String),
    Index(u32),
    Text(String),
    Error(String),
}

#[cfg(unix)]
mod unix_remote {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};
    use std::io::{self, BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    pub struct RemoteMuxClient {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }

    impl RemoteMuxClient {
        pub fn connect_or_spawn(initial_session: &str, cwd: PathBuf) -> Result<Self> {
            let socket = socket_path(initial_session);
            if let Ok(client) = Self::connect(&socket) {
                return Ok(client);
            }

            if socket.exists() {
                let _ = fs::remove_file(&socket);
            }
            if let Some(parent) = socket.parent() {
                fs::create_dir_all(parent)?;
            }

            let exe = std::env::current_exe().context("could not resolve current tuimux binary")?;
            let mut command = Command::new(exe);
            command
                .arg("--daemon")
                .arg("--socket")
                .arg(&socket)
                .arg("--session")
                .arg(initial_session)
                .arg("--cwd")
                .arg(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command
                .spawn()
                .context("failed to spawn native mux daemon")?;

            let deadline = Instant::now() + Duration::from_secs(3);
            let mut last_error = None;
            while Instant::now() < deadline {
                match Self::connect(&socket) {
                    Ok(client) => return Ok(client),
                    Err(err) => {
                        last_error = Some(err);
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| anyhow!("native mux daemon did not start")))
        }

        fn connect(socket: &Path) -> Result<Self> {
            let stream = UnixStream::connect(socket)
                .with_context(|| format!("could not connect to {}", socket.display()))?;
            let reader = BufReader::new(stream.try_clone()?);
            Ok(Self {
                reader,
                writer: stream,
            })
        }

        fn request(&mut self, request: Request) -> Result<Response> {
            let encoded = serde_json::to_string(&request)?;
            self.writer.write_all(encoded.as_bytes())?;
            self.writer.write_all(b"\n")?;
            self.writer.flush()?;

            let mut line = String::new();
            let read = self.reader.read_line(&mut line)?;
            if read == 0 {
                bail!("native mux daemon closed the connection");
            }
            let response: Response = serde_json::from_str(line.trim_end())?;
            if let Response::Error(message) = &response {
                bail!("{message}");
            }
            Ok(response)
        }

        pub fn snapshot(
            &mut self,
            width: u16,
            height: u16,
            selection: Option<SelectionRange>,
        ) -> Result<MuxSnapshot> {
            match self.request(Request::Snapshot {
                width,
                height,
                selection,
            })? {
                Response::Snapshot(snapshot) => Ok(snapshot),
                other => bail!("unexpected daemon response: {other:?}"),
            }
        }

        pub fn create_next_session(&mut self, width: u16, height: u16) -> Result<String> {
            match self.request(Request::CreateNextSession { width, height })? {
                Response::Name(name) => Ok(name),
                other => bail!("unexpected daemon response: {other:?}"),
            }
        }

        pub fn switch_session_by_row(&mut self, row: usize) -> Result<()> {
            expect_ok(self.request(Request::SwitchSessionByRow { row })?)
        }

        pub fn select_window_by_row(&mut self, row: usize) -> Result<()> {
            expect_ok(self.request(Request::SelectWindowByRow { row })?)
        }

        pub fn new_window(&mut self, width: u16, height: u16) -> Result<u32> {
            match self.request(Request::NewWindow { width, height })? {
                Response::Index(index) => Ok(index),
                other => bail!("unexpected daemon response: {other:?}"),
            }
        }

        pub fn kill_window_by_row(&mut self, row: usize, width: u16, height: u16) -> Result<u32> {
            match self.request(Request::KillWindowByRow { row, width, height })? {
                Response::Index(index) => Ok(index),
                other => bail!("unexpected daemon response: {other:?}"),
            }
        }

        pub fn selected_text(&mut self, selection: SelectionRange) -> Result<String> {
            match self.request(Request::SelectedText { selection })? {
                Response::Text(text) => Ok(text),
                other => bail!("unexpected daemon response: {other:?}"),
            }
        }

        pub fn send_key(&mut self, key: KeyInput) -> Result<()> {
            expect_ok(self.request(Request::SendKey { key })?)
        }

        pub fn send_paste(&mut self, text: &str) -> Result<()> {
            expect_ok(self.request(Request::SendPaste {
                text: text.to_string(),
            })?)
        }

        pub fn send_mouse(&mut self, mouse: MouseInput) -> Result<()> {
            expect_ok(self.request(Request::SendMouse { mouse })?)
        }
    }

    pub fn run_daemon(socket: PathBuf, initial_session: &str, cwd: PathBuf) -> i32 {
        match run_daemon_inner(socket, initial_session, cwd) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("tuimux daemon: {err:#}");
                1
            }
        }
    }

    pub fn stop_daemon(initial_session: &str) -> Result<()> {
        let socket = socket_path(initial_session);
        let mut client = RemoteMuxClient::connect(&socket)?;
        expect_ok(client.request(Request::Shutdown)?)
    }

    fn run_daemon_inner(socket: PathBuf, initial_session: &str, cwd: PathBuf) -> Result<()> {
        if let Some(parent) = socket.parent() {
            fs::create_dir_all(parent)?;
        }
        if socket.exists() {
            fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket)
            .with_context(|| format!("could not bind {}", socket.display()))?;
        let mut mux = NativeMux::new(initial_session, cwd, 80, 24)?;
        let mut shutdown = false;

        while !shutdown {
            let (stream, _) = listener.accept()?;
            shutdown = handle_client(stream, &mut mux)?;
        }

        let _ = fs::remove_file(socket);
        Ok(())
    }

    fn handle_client(stream: UnixStream, mux: &mut NativeMux) -> Result<bool> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;
        let mut line = String::new();

        loop {
            line.clear();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(false);
            }
            let (response, shutdown) = match serde_json::from_str::<Request>(line.trim_end()) {
                Ok(request) => {
                    let shutdown = matches!(request, Request::Shutdown);
                    let response = handle_request(request, mux);
                    let shutdown = shutdown && matches!(response, Response::Ok);
                    (response, shutdown)
                }
                Err(err) => (Response::Error(err.to_string()), false),
            };
            let encoded = serde_json::to_string(&response)?;
            writer.write_all(encoded.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            if shutdown {
                return Ok(true);
            }
        }
    }

    fn handle_request(request: Request, mux: &mut NativeMux) -> Response {
        match request {
            Request::Snapshot {
                width,
                height,
                selection,
            } => {
                mux.resize_active(width, height);
                mux.drain_all();
                Response::Snapshot(local_snapshot(mux, selection))
            }
            Request::CreateNextSession { width, height } => {
                into_response(mux.create_next_session(width, height), Response::Name)
            }
            Request::SwitchSessionByRow { row } => {
                into_response(mux.switch_session_by_row(row), |_| Response::Ok)
            }
            Request::SelectWindowByRow { row } => {
                into_response(mux.select_window_by_row(row), |_| Response::Ok)
            }
            Request::NewWindow { width, height } => {
                into_response(mux.new_window(width, height), Response::Index)
            }
            Request::KillWindowByRow { row, width, height } => {
                into_response(mux.kill_window_by_row(row, width, height), Response::Index)
            }
            Request::SelectedText { selection } => {
                let result = mux
                    .active_terminal()
                    .map(|terminal| terminal.selected_text(selection))
                    .ok_or_else(|| anyhow!("terminal is not ready"));
                into_response(result, Response::Text)
            }
            Request::SendKey { key } => {
                let result = mux
                    .active_terminal_mut()
                    .ok_or_else(|| anyhow!("terminal is not ready"))
                    .and_then(|terminal| terminal.send_key(key.to_event()?).map_err(Into::into));
                into_response(result, |_| Response::Ok)
            }
            Request::SendPaste { text } => {
                let result = mux
                    .active_terminal_mut()
                    .ok_or_else(|| anyhow!("terminal is not ready"))
                    .and_then(|terminal| terminal.send_paste(&text).map_err(Into::into));
                into_response(result, |_| Response::Ok)
            }
            Request::SendMouse { mouse } => {
                let result = mux
                    .active_terminal_mut()
                    .ok_or_else(|| anyhow!("terminal is not ready"))
                    .and_then(|terminal| {
                        terminal
                            .send_mouse_event(
                                mouse.kind.to_event_kind(),
                                mouse.row,
                                mouse.col,
                                bits_to_modifiers(mouse.modifiers),
                            )
                            .map(|_| ())
                            .map_err(Into::into)
                    });
                into_response(result, |_| Response::Ok)
            }
            Request::Shutdown => Response::Ok,
        }
    }

    fn into_response<T>(result: Result<T>, convert: impl FnOnce(T) -> Response) -> Response {
        match result {
            Ok(value) => convert(value),
            Err(err) => Response::Error(err.to_string()),
        }
    }

    fn expect_ok(response: Response) -> Result<()> {
        match response {
            Response::Ok => Ok(()),
            other => bail!("unexpected daemon response: {other:?}"),
        }
    }

    fn socket_path(initial_session: &str) -> PathBuf {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        let mut hasher = DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut hasher);
        initial_session.hash(&mut hasher);
        let hash = hasher.finish();
        let safe = sanitize(initial_session);
        PathBuf::from("/tmp")
            .join(format!("tuimux-{user}"))
            .join(format!("{safe}-{hash:016x}.sock"))
    }

    fn sanitize(value: &str) -> String {
        let mut out = value
            .chars()
            .filter_map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    Some(ch)
                } else {
                    None
                }
            })
            .take(24)
            .collect::<String>();
        if out.is_empty() {
            out.push_str("session");
        }
        out
    }
}

#[cfg(unix)]
pub use unix_remote::{run_daemon, stop_daemon, RemoteMuxClient};

#[cfg(not(unix))]
pub fn run_daemon(_socket: PathBuf, _initial_session: &str, _cwd: PathBuf) -> i32 {
    eprintln!("tuimux daemon mode is not available on this platform yet");
    1
}

#[cfg(not(unix))]
pub fn stop_daemon(_initial_session: &str) -> Result<()> {
    bail!("tuimux daemon mode is not available on this platform yet")
}
