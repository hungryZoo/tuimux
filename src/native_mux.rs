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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl PaneRect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSeparator {
    pub axis: PaneAxis,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub struct PaneRef<'a> {
    pub index: u32,
    pub title: &'a str,
    pub active: bool,
    pub rect: PaneRect,
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
    layout: PaneNode,
}

struct NativePane {
    index: u32,
    title: String,
    terminal: PtyTerminal,
}

// Deprecated product surface: split panes are no longer exposed by the default
// UI or daemon protocol, but the legacy layout code is retained while v0.2
// focuses on window-list-driven navigation.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum PaneNode {
    Leaf(u32),
    Split {
        axis: PaneAxis,
        first_ratio: u16,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
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
                    .pane_order()
                    .into_iter()
                    .filter_map(|pane_index| {
                        let pane = window.pane_by_index(pane_index)?;
                        Some(Pane {
                            index: pane.index,
                            title: pane.title.clone(),
                            active: pane.index == window.active_pane_index(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn active_pane_refs(&self, width: u16, height: u16) -> Vec<PaneRef<'_>> {
        self.active_window()
            .map(|window| {
                let rects = window.pane_rects(width, height);
                window
                    .pane_order()
                    .into_iter()
                    .filter_map(|pane_index| {
                        let pane = window.pane_by_index(pane_index)?;
                        Some(PaneRef {
                            index: pane.index,
                            title: &pane.title,
                            active: pane.index == window.active_pane_index(),
                            rect: rects
                                .iter()
                                .find(|(index, _)| *index == pane.index)
                                .map(|(_, rect)| *rect)
                                .unwrap_or_else(|| PaneRect::new(0, 0, width, height)),
                            terminal: &pane.terminal,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn active_pane_axis(&self) -> PaneAxis {
        self.active_window()
            .and_then(|window| window.layout.primary_axis())
            .unwrap_or_default()
    }

    pub fn active_pane_separators(&self, width: u16, height: u16) -> Vec<PaneSeparator> {
        self.active_window()
            .map(|window| window.pane_separators(width, height))
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
        let order = window.pane_order();
        if row >= order.len() {
            bail!("pane row {row} does not exist");
        }
        window.select_pane_index(order[row])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn select_next_pane(&mut self) -> Result<u32> {
        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        if window.panes.is_empty() {
            bail!("no panes in active window");
        }
        let order = window.pane_order();
        let active = window.active_pane_index();
        let current = order.iter().position(|index| *index == active).unwrap_or(0);
        let next = order[(current + 1) % order.len()];
        window.select_pane_index(next)?;
        Ok(next)
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

    #[allow(dead_code)]
    pub fn split_active_pane_right(&mut self, width: u16, height: u16) -> Result<u32> {
        self.split_active_pane(PaneAxis::Columns, width, height)
    }

    #[allow(dead_code)]
    pub fn split_active_pane_down(&mut self, width: u16, height: u16) -> Result<u32> {
        self.split_active_pane(PaneAxis::Rows, width, height)
    }

    #[allow(dead_code)]
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
            window.layout = PaneNode::Leaf(1);
        } else {
            window.layout = remove_leaf_from_layout(&window.layout, removed)
                .unwrap_or_else(|| PaneNode::Leaf(window.panes[0].index));
            let replacement_active = window
                .pane_order()
                .into_iter()
                .next()
                .unwrap_or(window.panes[0].index);
            window.select_pane_index(replacement_active)?;
            window.active_pane = window.active_pane.min(window.panes.len().saturating_sub(1));
        }
        self.resize_active(width, height);
        Ok(removed)
    }

    #[allow(dead_code)]
    pub fn resize_active_pane(
        &mut self,
        axis: PaneAxis,
        grow: bool,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let Some(window) = self.active_window_mut() else {
            bail!("no active window");
        };
        let active = window.active_pane_index();
        if !window.layout.resize_leaf(active, axis, grow, 50) {
            bail!("active pane has no {:?} split to resize", axis);
        }
        self.resize_active(width, height);
        Ok(())
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
            let rects = window.pane_rects(width, height);
            for pane in &mut window.panes {
                if let Some((_, rect)) = rects.iter().find(|(index, _)| *index == pane.index) {
                    pane.terminal.resize(rect.width, rect.height);
                }
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
            layout: PaneNode::Leaf(1),
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

    #[allow(dead_code)]
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
        let active_index = window.active_pane_index();
        window.layout = split_leaf_in_layout(&window.layout, active_index, pane_index, axis);
        window.panes.push(pane);
        window.active_pane = window.panes.len().saturating_sub(1);
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

impl NativeWindow {
    fn active_pane_index(&self) -> u32 {
        self.panes
            .get(self.active_pane)
            .map(|pane| pane.index)
            .unwrap_or(0)
    }

    fn pane_by_index(&self, pane_index: u32) -> Option<&NativePane> {
        self.panes.iter().find(|pane| pane.index == pane_index)
    }

    fn select_pane_index(&mut self, pane_index: u32) -> Result<()> {
        let Some(row) = self.panes.iter().position(|pane| pane.index == pane_index) else {
            bail!("pane {pane_index} does not exist");
        };
        self.active_pane = row;
        Ok(())
    }

    fn pane_order(&self) -> Vec<u32> {
        let mut order = Vec::new();
        self.layout.collect_leaves(&mut order);
        order.retain(|index| self.pane_by_index(*index).is_some());
        order
    }

    fn pane_rects(&self, width: u16, height: u16) -> Vec<(u32, PaneRect)> {
        let mut rects = Vec::new();
        self.layout
            .collect_rects(PaneRect::new(0, 0, width, height), &mut rects);
        rects.retain(|(index, _)| self.pane_by_index(*index).is_some());
        rects
    }

    fn pane_separators(&self, width: u16, height: u16) -> Vec<PaneSeparator> {
        let mut separators = Vec::new();
        self.layout
            .collect_separators(PaneRect::new(0, 0, width, height), &mut separators);
        separators
    }
}

impl PaneNode {
    fn primary_axis(&self) -> Option<PaneAxis> {
        match self {
            PaneNode::Leaf(_) => None,
            PaneNode::Split { axis, .. } => Some(*axis),
        }
    }

    fn collect_leaves(&self, leaves: &mut Vec<u32>) {
        match self {
            PaneNode::Leaf(index) => leaves.push(*index),
            PaneNode::Split { first, second, .. } => {
                first.collect_leaves(leaves);
                second.collect_leaves(leaves);
            }
        }
    }

    fn collect_rects(&self, rect: PaneRect, out: &mut Vec<(u32, PaneRect)>) {
        match self {
            PaneNode::Leaf(index) => out.push((*index, rect)),
            PaneNode::Split {
                axis,
                first_ratio,
                first,
                second,
            } => {
                let (first_rect, _, second_rect) = split_rect(rect, *axis, *first_ratio);
                first.collect_rects(first_rect, out);
                second.collect_rects(second_rect, out);
            }
        }
    }

    fn collect_separators(&self, rect: PaneRect, out: &mut Vec<PaneSeparator>) {
        match self {
            PaneNode::Leaf(_) => {}
            PaneNode::Split {
                axis,
                first_ratio,
                first,
                second,
            } => {
                let (first_rect, separator, second_rect) = split_rect(rect, *axis, *first_ratio);
                out.push(separator);
                first.collect_separators(first_rect, out);
                second.collect_separators(second_rect, out);
            }
        }
    }

    fn contains_leaf(&self, target: u32) -> bool {
        match self {
            PaneNode::Leaf(index) => *index == target,
            PaneNode::Split { first, second, .. } => {
                first.contains_leaf(target) || second.contains_leaf(target)
            }
        }
    }

    fn resize_leaf(&mut self, target: u32, axis: PaneAxis, grow: bool, step: u16) -> bool {
        match self {
            PaneNode::Leaf(_) => false,
            PaneNode::Split {
                axis: split_axis,
                first_ratio,
                first,
                second,
            } => {
                if first.resize_leaf(target, axis, grow, step)
                    || second.resize_leaf(target, axis, grow, step)
                {
                    return true;
                }

                let in_first = first.contains_leaf(target);
                let in_second = second.contains_leaf(target);
                if *split_axis != axis || (!in_first && !in_second) {
                    return false;
                }

                let grow_first = (in_first && grow) || (in_second && !grow);
                *first_ratio = if grow_first {
                    first_ratio.saturating_add(step)
                } else {
                    first_ratio.saturating_sub(step)
                }
                .clamp(100, 900);
                true
            }
        }
    }
}

#[allow(dead_code)]
fn split_leaf_in_layout(node: &PaneNode, target: u32, new_pane: u32, axis: PaneAxis) -> PaneNode {
    match node {
        PaneNode::Leaf(index) if *index == target => PaneNode::Split {
            axis,
            first_ratio: 500,
            first: Box::new(PaneNode::Leaf(*index)),
            second: Box::new(PaneNode::Leaf(new_pane)),
        },
        PaneNode::Leaf(index) => PaneNode::Leaf(*index),
        PaneNode::Split {
            axis: existing_axis,
            first_ratio,
            first,
            second,
        } => PaneNode::Split {
            axis: *existing_axis,
            first_ratio: *first_ratio,
            first: Box::new(split_leaf_in_layout(first, target, new_pane, axis)),
            second: Box::new(split_leaf_in_layout(second, target, new_pane, axis)),
        },
    }
}

#[allow(dead_code)]
fn remove_leaf_from_layout(node: &PaneNode, target: u32) -> Option<PaneNode> {
    match node {
        PaneNode::Leaf(index) if *index == target => None,
        PaneNode::Leaf(index) => Some(PaneNode::Leaf(*index)),
        PaneNode::Split {
            axis,
            first_ratio,
            first,
            second,
        } => match (
            remove_leaf_from_layout(first, target),
            remove_leaf_from_layout(second, target),
        ) {
            (Some(first), Some(second)) => Some(PaneNode::Split {
                axis: *axis,
                first_ratio: *first_ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        },
    }
}

fn split_rect(
    rect: PaneRect,
    axis: PaneAxis,
    first_ratio: u16,
) -> (PaneRect, PaneSeparator, PaneRect) {
    match axis {
        PaneAxis::Columns => {
            let content = rect.width.saturating_sub(1);
            let first_width = split_first_length(content, first_ratio);
            let second_width = content.saturating_sub(first_width);
            let first = PaneRect::new(rect.x, rect.y, first_width, rect.height);
            let separator = PaneSeparator {
                axis,
                x: first.right(),
                y: rect.y,
                width: u16::from(rect.width > 0),
                height: rect.height,
            };
            let second = PaneRect::new(
                separator.x.saturating_add(separator.width),
                rect.y,
                second_width,
                rect.height,
            );
            (first, separator, second)
        }
        PaneAxis::Rows => {
            let content = rect.height.saturating_sub(1);
            let first_height = split_first_length(content, first_ratio);
            let second_height = content.saturating_sub(first_height);
            let first = PaneRect::new(rect.x, rect.y, rect.width, first_height);
            let separator = PaneSeparator {
                axis,
                x: rect.x,
                y: first.bottom(),
                width: rect.width,
                height: u16::from(rect.height > 0),
            };
            let second = PaneRect::new(
                rect.x,
                separator.y.saturating_add(separator.height),
                rect.width,
                second_height,
            );
            (first, separator, second)
        }
    }
}

fn split_first_length(content: u16, first_ratio: u16) -> u16 {
    if content <= 1 {
        return content;
    }
    let ratio = first_ratio.clamp(100, 900) as u32;
    let first = ((content as u32 * ratio) + 500) / 1000;
    first.clamp(1, content.saturating_sub(1) as u32) as u16
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
    fn nested_layout_preserves_existing_split_when_splitting_active_leaf() {
        let mut mux = NativeMux::new("tuimux", PathBuf::from("."), 80, 24).unwrap();

        mux.split_active_pane_right(80, 24).unwrap();
        mux.select_pane_by_row(0).unwrap();
        mux.split_active_pane_down(80, 24).unwrap();

        let panes = mux.pane_infos();
        let order = panes.iter().map(|pane| pane.index).collect::<Vec<_>>();
        assert_eq!(order, vec![1, 3, 2]);
        assert!(panes[1].active);

        let refs = mux.active_pane_refs(80, 24);
        let rects = refs
            .iter()
            .map(|pane| (pane.index, pane.rect))
            .collect::<Vec<_>>();
        assert_eq!(rects[0], (1, PaneRect::new(0, 0, 40, 12)));
        assert_eq!(rects[1], (3, PaneRect::new(0, 13, 40, 11)));
        assert_eq!(rects[2], (2, PaneRect::new(41, 0, 39, 24)));

        let separators = mux.active_pane_separators(80, 24);
        assert_eq!(separators.len(), 2);
        assert_eq!(
            separators[0],
            PaneSeparator {
                axis: PaneAxis::Columns,
                x: 40,
                y: 0,
                width: 1,
                height: 24,
            }
        );
        assert_eq!(
            separators[1],
            PaneSeparator {
                axis: PaneAxis::Rows,
                x: 0,
                y: 12,
                width: 40,
                height: 1,
            }
        );

        mux.resize_active_pane(PaneAxis::Rows, true, 80, 24)
            .unwrap();
        let refs = mux.active_pane_refs(80, 24);
        assert_eq!(refs[0].rect, PaneRect::new(0, 0, 40, 10));
        assert_eq!(refs[1].rect, PaneRect::new(0, 11, 40, 13));

        mux.resize_active_pane(PaneAxis::Columns, true, 80, 24)
            .unwrap();
        let refs = mux.active_pane_refs(80, 24);
        assert_eq!(refs[0].rect, PaneRect::new(0, 0, 43, 10));
        assert_eq!(refs[1].rect, PaneRect::new(0, 11, 43, 13));
        assert_eq!(refs[2].rect, PaneRect::new(44, 0, 36, 24));
    }
}
