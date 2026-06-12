//! Rust-native in-process multiplexer core.
//!
//! This is the first step away from depending on an external tmux server. Each
//! window owns a real PTY-backed terminal process, while sessions and active
//! window state are managed directly by tuimux.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::terminal::PtyTerminal;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub index: u32,
    pub name: String,
    pub active: bool,
}

pub struct NativeMux {
    sessions: Vec<NativeSession>,
    active_session: usize,
    cwd: PathBuf,
    next_session: u32,
    next_window_id: u32,
}

struct NativeSession {
    name: String,
    windows: Vec<NativeWindow>,
    active_window: usize,
}

struct NativeWindow {
    index: u32,
    name: String,
    terminal: PtyTerminal,
}

impl NativeMux {
    pub fn new(initial_session: &str, cwd: PathBuf, width: u16, height: u16) -> Result<Self> {
        let mut mux = Self {
            sessions: Vec::new(),
            active_session: 0,
            cwd,
            next_session: 1,
            next_window_id: 1,
        };
        mux.create_session(initial_session, width, height)?;
        Ok(mux)
    }

    pub fn session_infos(&self) -> Vec<Session> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(idx, session)| Session {
                name: session.name.clone(),
                windows: session.windows.len() as u32,
                attached: idx == self.active_session,
            })
            .collect()
    }

    pub fn window_infos(&self) -> Vec<Window> {
        self.active_session()
            .map(|session| {
                session
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(idx, window)| Window {
                        index: window.index,
                        name: window.name.clone(),
                        active: idx == session.active_window,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn current_session_name(&self) -> &str {
        self.active_session()
            .map(|session| session.name.as_str())
            .unwrap_or("")
    }

    pub fn create_next_session(&mut self, width: u16, height: u16) -> Result<String> {
        loop {
            let name = format!("tuimux-{}", self.next_session);
            self.next_session += 1;
            if !self.sessions.iter().any(|session| session.name == name) {
                self.create_session(&name, width, height)?;
                return Ok(name);
            }
        }
    }

    pub fn create_session(&mut self, name: &str, width: u16, height: u16) -> Result<()> {
        if self.sessions.iter().any(|session| session.name == name) {
            bail!("session '{name}' already exists");
        }
        let window = self.spawn_window(1, width, height)?;
        self.sessions.push(NativeSession {
            name: name.to_string(),
            windows: vec![window],
            active_window: 0,
        });
        self.active_session = self.sessions.len().saturating_sub(1);
        Ok(())
    }

    pub fn switch_session_by_row(&mut self, row: usize) -> Result<()> {
        if row >= self.sessions.len() {
            bail!("session row {row} does not exist");
        }
        self.active_session = row;
        Ok(())
    }

    pub fn select_window_by_row(&mut self, row: usize) -> Result<()> {
        let Some(session) = self.active_session_mut() else {
            bail!("no active session");
        };
        if row >= session.windows.len() {
            bail!("window row {row} does not exist");
        }
        session.active_window = row;
        Ok(())
    }

    pub fn new_window(&mut self, width: u16, height: u16) -> Result<u32> {
        let Some(session) = self.active_session() else {
            bail!("no active session");
        };
        let index = session
            .windows
            .iter()
            .map(|window| window.index)
            .max()
            .unwrap_or(0)
            + 1;
        let window = self.spawn_window(index, width, height)?;
        let Some(session) = self.active_session_mut() else {
            bail!("no active session");
        };
        session.windows.push(window);
        session.active_window = session.windows.len().saturating_sub(1);
        Ok(index)
    }

    pub fn kill_window_by_row(&mut self, row: usize, width: u16, height: u16) -> Result<u32> {
        let Some(session) = self.active_session() else {
            bail!("no active session");
        };
        if row >= session.windows.len() {
            bail!("window row {row} does not exist");
        }
        let needs_replacement = session.windows.len() == 1;
        let replacement = if needs_replacement {
            Some(self.spawn_window(1, width, height)?)
        } else {
            None
        };

        let Some(session) = self.active_session_mut() else {
            bail!("no active session");
        };
        let removed = session.windows.remove(row).index;
        if let Some(window) = replacement {
            session.windows.push(window);
            session.active_window = 0;
        } else {
            session.active_window = session
                .active_window
                .min(session.windows.len().saturating_sub(1));
        }
        Ok(removed)
    }

    pub fn active_terminal(&self) -> Option<&PtyTerminal> {
        self.active_session()
            .and_then(|session| session.windows.get(session.active_window))
            .map(|window| &window.terminal)
    }

    pub fn active_terminal_mut(&mut self) -> Option<&mut PtyTerminal> {
        self.active_session_mut()
            .and_then(|session| session.windows.get_mut(session.active_window))
            .map(|window| &mut window.terminal)
    }

    pub fn drain_all(&mut self) -> bool {
        let mut changed = false;
        for session in &mut self.sessions {
            for window in &mut session.windows {
                changed |= window.terminal.drain();
            }
        }
        changed
    }

    pub fn resize_active(&mut self, width: u16, height: u16) {
        if let Some(terminal) = self.active_terminal_mut() {
            terminal.resize(width, height);
        }
    }

    fn spawn_window(&mut self, index: u32, width: u16, height: u16) -> Result<NativeWindow> {
        let id = self.next_window_id;
        self.next_window_id += 1;
        let name = if index == 1 {
            "shell".to_string()
        } else {
            format!("shell-{index}")
        };
        let title = format!("{name}-{id}");
        let terminal = PtyTerminal::new_shell(&title, cwd_or_current(&self.cwd), width, height)?;
        Ok(NativeWindow {
            index,
            name,
            terminal,
        })
    }

    fn active_session(&self) -> Option<&NativeSession> {
        self.sessions.get(self.active_session)
    }

    fn active_session_mut(&mut self) -> Option<&mut NativeSession> {
        self.sessions.get_mut(self.active_session)
    }
}

fn cwd_or_current(cwd: &Path) -> &Path {
    if cwd.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cwd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mux_starts_with_one_attached_session_and_window() {
        let mux = NativeMux::new("tuimux", PathBuf::from("."), 80, 24).unwrap();
        let sessions = mux.session_infos();
        let windows = mux.window_infos();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "tuimux");
        assert!(sessions[0].attached);
        assert_eq!(sessions[0].windows, 1);
        assert_eq!(windows.len(), 1);
        assert!(windows[0].active);
    }

    #[test]
    fn native_mux_can_create_and_switch_sessions() {
        let mut mux = NativeMux::new("tuimux", PathBuf::from("."), 80, 24).unwrap();
        let name = mux.create_next_session(80, 24).unwrap();

        assert_eq!(name, "tuimux-1");
        assert_eq!(mux.current_session_name(), "tuimux-1");
        mux.switch_session_by_row(0).unwrap();
        assert_eq!(mux.current_session_name(), "tuimux");
    }
}
