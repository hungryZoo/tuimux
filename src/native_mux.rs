//! Rust-native multiplexer core.
//!
//! The daemon owns sessions, windows, panes, and real PTY-backed terminal
//! processes directly. The UI only attaches to this state through the backend
//! boundary.

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
    pub panes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub index: u32,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneAxis {
    Columns,
    Rows,
}

impl Default for PaneAxis {
    fn default() -> Self {
        Self::Columns
    }
}

pub struct PaneRef<'a> {
    pub index: u32,
    pub title: &'a str,
    pub active: bool,
    pub terminal: &'a PtyTerminal,
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
    panes: Vec<NativePane>,
    active_pane: usize,
    pane_axis: PaneAxis,
}

struct NativePane {
    index: u32,
    title: String,
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
                        panes: window.panes.len() as u32,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pane_infos(&self) -> Vec<Pane> {
        self.active_window()
            .map(|window| {
                window
                    .panes
                    .iter()
                    .enumerate()
                    .map(|(idx, pane)| Pane {
                        index: pane.index,
                        title: pane.title.clone(),
                        active: idx == window.active_pane,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn active_pane_refs(&self) -> Vec<PaneRef<'_>> {
        self.active_window()
            .map(|window| {
                window
                    .panes
                    .iter()
                    .enumerate()
                    .map(|(idx, pane)| PaneRef {
                        index: pane.index,
                        title: &pane.title,
                        active: idx == window.active_pane,
                        terminal: &pane.terminal,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn active_pane_axis(&self) -> PaneAxis {
        self.active_window()
            .map(|window| window.pane_axis)
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

    pub fn select_pane_by_row(&mut self, row: usize) -> Result<()> {
        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        if row >= window.panes.len() {
            bail!("pane row {row} does not exist");
        }
        window.active_pane = row;
        Ok(())
    }

    pub fn select_next_pane(&mut self) -> Result<u32> {
        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        if window.panes.is_empty() {
            bail!("no panes in active window");
        }
        window.active_pane = (window.active_pane + 1) % window.panes.len();
        Ok(window.panes[window.active_pane].index)
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

    pub fn split_active_pane_right(&mut self, width: u16, height: u16) -> Result<u32> {
        self.split_active_pane(PaneAxis::Columns, width, height)
    }

    pub fn split_active_pane_down(&mut self, width: u16, height: u16) -> Result<u32> {
        self.split_active_pane(PaneAxis::Rows, width, height)
    }

    pub fn kill_active_pane(&mut self, width: u16, height: u16) -> Result<u32> {
        let (window_index, needs_replacement) = {
            let Some(window) = self.active_window() else {
                bail!("no active window");
            };
            (window.index, window.panes.len() == 1)
        };

        let replacement = if needs_replacement {
            Some(self.spawn_pane(window_index, 1, width, height)?)
        } else {
            None
        };

        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        if window.panes.is_empty() {
            bail!("no panes in active window");
        }
        let removed = window.panes.remove(window.active_pane).index;
        if let Some(pane) = replacement {
            window.panes.push(pane);
            window.active_pane = 0;
            window.pane_axis = PaneAxis::Columns;
        } else {
            window.active_pane = window.active_pane.min(window.panes.len().saturating_sub(1));
        }
        self.resize_active(width, height);
        Ok(removed)
    }

    pub fn active_terminal(&self) -> Option<&PtyTerminal> {
        self.active_window()
            .and_then(|window| window.panes.get(window.active_pane))
            .map(|pane| &pane.terminal)
    }

    pub fn active_terminal_mut(&mut self) -> Option<&mut PtyTerminal> {
        self.active_window_mut()
            .and_then(|window| window.panes.get_mut(window.active_pane))
            .map(|pane| &mut pane.terminal)
    }

    pub fn drain_all(&mut self) -> bool {
        let mut changed = false;
        for session in &mut self.sessions {
            for window in &mut session.windows {
                for pane in &mut window.panes {
                    changed |= pane.terminal.drain();
                }
            }
        }
        changed
    }

    pub fn resize_active(&mut self, width: u16, height: u16) {
        if let Some(window) = self.active_window_mut() {
            let sizes = pane_sizes(width, height, window.panes.len(), window.pane_axis);
            for (pane, (pane_width, pane_height)) in window.panes.iter_mut().zip(sizes) {
                pane.terminal.resize(pane_width, pane_height);
            }
        }
    }

    fn spawn_window(&mut self, index: u32, width: u16, height: u16) -> Result<NativeWindow> {
        let name = if index == 1 {
            "shell".to_string()
        } else {
            format!("shell-{index}")
        };
        let pane = self.spawn_pane(index, 1, width, height)?;
        Ok(NativeWindow {
            index,
            name,
            panes: vec![pane],
            active_pane: 0,
            pane_axis: PaneAxis::Columns,
        })
    }

    fn spawn_pane(
        &mut self,
        window_index: u32,
        pane_index: u32,
        width: u16,
        height: u16,
    ) -> Result<NativePane> {
        let id = self.next_window_id;
        self.next_window_id += 1;
        let title = if pane_index == 1 {
            format!("shell-{window_index}")
        } else {
            format!("shell-{window_index}.{pane_index}")
        };
        let terminal_title = format!("{title}-{id}");
        let terminal =
            PtyTerminal::new_shell(&terminal_title, cwd_or_current(&self.cwd), width, height)?;
        Ok(NativePane {
            index: pane_index,
            title,
            terminal,
        })
    }

    fn split_active_pane(&mut self, axis: PaneAxis, width: u16, height: u16) -> Result<u32> {
        let (window_index, pane_index) = {
            let Some(window) = self.active_window() else {
                bail!("no active window");
            };
            let next = window
                .panes
                .iter()
                .map(|pane| pane.index)
                .max()
                .unwrap_or(0)
                + 1;
            (window.index, next)
        };

        let pane = self.spawn_pane(window_index, pane_index, width, height)?;
        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        window.panes.push(pane);
        window.active_pane = window.panes.len().saturating_sub(1);
        window.pane_axis = axis;
        self.resize_active(width, height);
        Ok(pane_index)
    }

    fn active_session(&self) -> Option<&NativeSession> {
        self.sessions.get(self.active_session)
    }

    fn active_session_mut(&mut self) -> Option<&mut NativeSession> {
        self.sessions.get_mut(self.active_session)
    }

    fn active_window(&self) -> Option<&NativeWindow> {
        self.active_session()
            .and_then(|session| session.windows.get(session.active_window))
    }

    fn active_window_mut(&mut self) -> Option<&mut NativeWindow> {
        self.active_session_mut()
            .and_then(|session| session.windows.get_mut(session.active_window))
    }
}

pub fn pane_sizes(width: u16, height: u16, count: usize, axis: PaneAxis) -> Vec<(u16, u16)> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![(width.max(2), height.max(1))];
    }

    match axis {
        PaneAxis::Columns => split_lengths(width.saturating_sub((count - 1) as u16), count)
            .into_iter()
            .map(|pane_width| (pane_width.max(2), height.max(1)))
            .collect(),
        PaneAxis::Rows => split_lengths(height.saturating_sub((count - 1) as u16), count)
            .into_iter()
            .map(|pane_height| (width.max(2), pane_height.max(1)))
            .collect(),
    }
}

fn split_lengths(total: u16, count: usize) -> Vec<u16> {
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

    #[test]
    fn native_mux_can_split_cycle_and_kill_panes() {
        let mut mux = NativeMux::new("tuimux", PathBuf::from("."), 80, 24).unwrap();

        let split = mux.split_active_pane_right(80, 24).unwrap();
        assert_eq!(split, 2);
        assert_eq!(mux.window_infos()[0].panes, 2);
        assert_eq!(mux.active_pane_axis(), PaneAxis::Columns);
        let panes = mux.pane_infos();
        assert_eq!(panes.len(), 2);
        assert!(!panes[0].active);
        assert!(panes[1].active);

        let selected = mux.select_next_pane().unwrap();
        assert_eq!(selected, 1);
        assert!(mux.pane_infos()[0].active);

        let killed = mux.kill_active_pane(80, 24).unwrap();
        assert_eq!(killed, 1);
        let panes = mux.pane_infos();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].index, 2);
        assert!(panes[0].active);
    }

    #[test]
    fn pane_sizes_reserve_separator_space() {
        assert_eq!(
            pane_sizes(11, 5, 2, PaneAxis::Columns),
            vec![(5, 5), (5, 5)]
        );
        assert_eq!(
            pane_sizes(10, 7, 3, PaneAxis::Rows),
            vec![(10, 2), (10, 2), (10, 1)]
        );
    }
}
