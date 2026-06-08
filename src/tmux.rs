//! tmux discovery, command execution, and state parsing.
//!
//! tuimux is a front-end for a tmux server. v0.1.6 still uses tmux commands,
//! but avoids pretending to be a terminal emulator: visible-pane capture does
//! not read scrollback history, key forwarding ignores repeat / release events,
//! and tmux keeps owning shell semantics.

use std::fmt;
use std::process::Command;

/// Minimum tmux version tuimux targets. Control mode exists earlier, but 3.0 is
/// the floor we document and test against.
pub const MIN_SUPPORTED: TmuxVersion = TmuxVersion {
    major: 3,
    minor: 0,
    suffix_ascii: 0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxVersion {
    pub major: u32,
    pub minor: u32,
    pub suffix_ascii: u8,
}

impl TmuxVersion {
    pub fn parse(raw: &str) -> Option<TmuxVersion> {
        let s = raw.trim();
        let s = s.strip_prefix("tmux").map(str::trim_start).unwrap_or(s);
        let start = s.find(|c: char| c.is_ascii_digit())?;
        let s = &s[start..];
        let token: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || c.is_ascii_alphabetic())
            .collect();
        if token.is_empty() {
            return None;
        }
        let digits_end = token
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(token.len());
        let (numeric, suffix) = token.split_at(digits_end);
        let mut parts = numeric.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let minor: u32 = match parts.next() {
            Some(m) if !m.is_empty() => m.parse().ok()?,
            _ => 0,
        };
        let suffix_ascii = suffix.bytes().next().unwrap_or(0);
        Some(TmuxVersion {
            major,
            minor,
            suffix_ascii,
        })
    }

    pub fn is_supported(&self) -> bool {
        *self >= MIN_SUPPORTED
    }
}

impl fmt::Display for TmuxVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.suffix_ascii != 0 {
            write!(f, "{}", self.suffix_ascii as char)?;
        }
        Ok(())
    }
}

impl PartialOrd for TmuxVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TmuxVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.suffix_ascii).cmp(&(
            other.major,
            other.minor,
            other.suffix_ascii,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct TmuxProbe {
    pub binary: String,
    pub raw_version: String,
    pub version: Option<TmuxVersion>,
}

impl TmuxProbe {
    #[allow(dead_code)]
    pub fn is_usable(&self) -> bool {
        self.version.map(|v| v.is_supported()).unwrap_or(false)
    }
}

pub fn probe() -> Result<TmuxProbe, String> {
    let binary = "tmux";
    let output = Command::new(binary).arg("-V").output().map_err(|e| {
        format!("could not run `{binary} -V`: {e} (is tmux installed and on PATH?)")
    })?;

    if !output.status.success() {
        return Err(format!(
            "`{binary} -V` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let raw_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let version = TmuxVersion::parse(&raw_version);

    Ok(TmuxProbe {
        binary: binary.to_string(),
        raw_version,
        version,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub index: u32,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxError {
    Spawn(String),
    Failed { status: Option<i32>, stderr: String },
}

impl fmt::Display for TmuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TmuxError::Spawn(msg) => write!(f, "{msg}"),
            TmuxError::Failed { status, stderr } => {
                write!(f, "tmux command failed")?;
                if let Some(code) = status {
                    write!(f, " with exit code {code}")?;
                }
                if !stderr.trim().is_empty() {
                    write!(f, ": {}", stderr.trim())?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for TmuxError {}

pub trait TmuxRunner {
    fn run(&self, args: &[String]) -> Result<String, TmuxError>;
    fn inside_tmux(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct RealTmux {
    pub binary: String,
}

impl RealTmux {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl TmuxRunner for RealTmux {
    fn run(&self, args: &[String]) -> Result<String, TmuxError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|e| TmuxError::Spawn(format!("could not run `{}`: {e}", self.binary)))?;
        if !output.status.success() {
            return Err(TmuxError::Failed {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn inside_tmux(&self) -> bool {
        std::env::var_os("TMUX").is_some()
    }
}

pub struct Tmux<R: TmuxRunner> {
    runner: R,
}

impl<R: TmuxRunner> Tmux<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    /// Whether tuimux itself is running inside a tmux client (the `TMUX` env var
    /// is set). The UI uses this to decide whether switching a session is safe:
    /// inside tmux we `switch-client`; outside we must not `attach-session`
    /// (that would spawn an interactive tmux on top of us), so we just report it.
    pub fn inside_tmux(&self) -> bool {
        self.runner.inside_tmux()
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, TmuxError> {
        match self.runner.run(&list_sessions_args()) {
            Ok(stdout) => Ok(parse_sessions(&stdout)),
            Err(err) if is_no_server_error(&err) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    pub fn list_windows(&self, session: &str) -> Result<Vec<Window>, TmuxError> {
        match self.runner.run(&list_windows_args(session)) {
            Ok(stdout) => Ok(parse_windows(&stdout)),
            Err(err) if is_no_server_error(&err) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    pub fn switch_session(&self, name: &str) -> Result<(), TmuxError> {
        self.runner
            .run(&switch_session_args(name, self.runner.inside_tmux()))
            .map(|_| ())
    }

    pub fn new_session(&self, name: &str) -> Result<(), TmuxError> {
        self.runner.run(&new_session_args(name)).map(|_| ())
    }

    pub fn select_window(&self, session: &str, index: u32) -> Result<(), TmuxError> {
        self.runner
            .run(&select_window_args(session, index))
            .map(|_| ())
    }

    pub fn new_window(&self, session: &str) -> Result<(), TmuxError> {
        self.runner.run(&new_window_args(session)).map(|_| ())
    }

    pub fn kill_window(&self, session: &str, index: u32) -> Result<(), TmuxError> {
        self.runner
            .run(&kill_window_args(session, index))
            .map(|_| ())
    }

    pub fn capture_pane(
        &self,
        session: &str,
        width: u16,
        height: u16,
    ) -> Result<Vec<String>, TmuxError> {
        let _ = (width, height);
        match self.runner.run(&capture_pane_args(session)) {
            Ok(stdout) => Ok(parse_capture(&stdout)),
            Err(err) if is_no_server_error(&err) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    pub fn send_keys(&self, session: &str, keys: &[String]) -> Result<(), TmuxError> {
        self.runner.run(&send_keys_args(session, keys)).map(|_| ())
    }

    pub fn detach(&self) -> Result<(), TmuxError> {
        if self.runner.inside_tmux() {
            self.runner.run(&detach_args()).map(|_| ())
        } else {
            Ok(())
        }
    }
}

fn is_no_server_error(err: &TmuxError) -> bool {
    match err {
        TmuxError::Failed { stderr, .. } => stderr.contains("no server running"),
        TmuxError::Spawn(_) => false,
    }
}

pub(crate) fn parse_sessions(raw: &str) -> Vec<Session> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?.trim();
            let windows = parts.next()?.trim().parse().ok()?;
            let attached = parts.next()?.trim() == "1";
            if name.is_empty() {
                return None;
            }
            Some(Session {
                name: name.to_string(),
                windows,
                attached,
            })
        })
        .collect()
}

pub(crate) fn parse_windows(raw: &str) -> Vec<Window> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let index = parts.next()?.trim().parse().ok()?;
            let name = parts.next()?.trim();
            let active = parts.next()?.trim() == "1";
            Some(Window {
                index,
                name: name.to_string(),
                active,
            })
        })
        .collect()
}

pub(crate) fn parse_capture(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

pub(crate) fn list_sessions_args() -> Vec<String> {
    vec![
        "list-sessions".to_string(),
        "-F".to_string(),
        "#{session_name}\t#{session_windows}\t#{session_attached}".to_string(),
    ]
}

pub(crate) fn list_windows_args(session: &str) -> Vec<String> {
    vec![
        "list-windows".to_string(),
        "-t".to_string(),
        session.to_string(),
        "-F".to_string(),
        "#{window_index}\t#{window_name}\t#{window_active}".to_string(),
    ]
}

pub(crate) fn switch_session_args(session: &str, inside_tmux: bool) -> Vec<String> {
    if inside_tmux {
        vec![
            "switch-client".to_string(),
            "-t".to_string(),
            session.to_string(),
        ]
    } else {
        vec![
            "attach-session".to_string(),
            "-t".to_string(),
            session.to_string(),
        ]
    }
}

pub(crate) fn new_session_args(session: &str) -> Vec<String> {
    vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session.to_string(),
    ]
}

pub(crate) fn select_window_args(session: &str, index: u32) -> Vec<String> {
    vec![
        "select-window".to_string(),
        "-t".to_string(),
        format!("{session}:{index}"),
    ]
}

pub(crate) fn new_window_args(session: &str) -> Vec<String> {
    vec![
        "new-window".to_string(),
        "-t".to_string(),
        session.to_string(),
    ]
}

pub(crate) fn kill_window_args(session: &str, index: u32) -> Vec<String> {
    vec![
        "kill-window".to_string(),
        "-t".to_string(),
        format!("{session}:{index}"),
    ]
}

pub(crate) fn capture_pane_args(session: &str) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        session.to_string(),
    ]
}

pub(crate) fn send_keys_args(session: &str, keys: &[String]) -> Vec<String> {
    let mut args = vec![
        "send-keys".to_string(),
        "-t".to_string(),
        session.to_string(),
    ];
    args.extend(keys.iter().cloned());
    args
}

pub(crate) fn detach_args() -> Vec<String> {
    vec!["detach-client".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_version() {
        let v = TmuxVersion::parse("tmux 3.4").unwrap();
        assert_eq!(v.major, 3);
        assert_eq!(v.minor, 4);
        assert_eq!(v.suffix_ascii, 0);
        assert_eq!(v.to_string(), "3.4");
    }

    #[test]
    fn parses_patch_letter_suffix() {
        let v = TmuxVersion::parse("tmux 3.0a").unwrap();
        assert_eq!(v.major, 3);
        assert_eq!(v.minor, 0);
        assert_eq!(v.suffix_ascii, b'a');
        assert_eq!(v.to_string(), "3.0a");
    }

    #[test]
    fn parses_bare_string_without_program_name() {
        assert_eq!(
            TmuxVersion::parse("2.9"),
            Some(TmuxVersion {
                major: 2,
                minor: 9,
                suffix_ascii: 0
            })
        );
    }

    #[test]
    fn parses_development_prefix() {
        let v = TmuxVersion::parse("tmux next-3.4").unwrap();
        assert_eq!((v.major, v.minor), (3, 4));
    }

    #[test]
    fn parses_major_only_as_dot_zero() {
        let v = TmuxVersion::parse("tmux 3").unwrap();
        assert_eq!((v.major, v.minor, v.suffix_ascii), (3, 0, 0));
    }

    #[test]
    fn ignores_trailing_noise() {
        let v = TmuxVersion::parse("tmux 3.2a (some-distro-build)").unwrap();
        assert_eq!(v.to_string(), "3.2a");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(TmuxVersion::parse(""), None);
        assert_eq!(TmuxVersion::parse("tmux"), None);
        assert_eq!(TmuxVersion::parse("not a version"), None);
    }

    #[test]
    fn ordering_is_sensible() {
        let v30 = TmuxVersion::parse("3.0").unwrap();
        let v30a = TmuxVersion::parse("3.0a").unwrap();
        let v31 = TmuxVersion::parse("3.1").unwrap();
        let v210 = TmuxVersion::parse("2.10").unwrap();
        let v29 = TmuxVersion::parse("2.9").unwrap();

        assert!(v30a > v30);
        assert!(v31 > v30a);
        assert!(v210 > v29, "2.10 should sort after 2.9 numerically");
        assert!(v30 > v210);
    }

    #[test]
    fn support_floor_is_three_zero() {
        assert!(!TmuxVersion::parse("2.9").unwrap().is_supported());
        assert!(TmuxVersion::parse("3.0").unwrap().is_supported());
        assert!(TmuxVersion::parse("3.0a").unwrap().is_supported());
        assert!(TmuxVersion::parse("3.4").unwrap().is_supported());
    }

    #[test]
    fn parses_sessions_from_tmux_tab_format() {
        let sessions = parse_sessions("dev\t3\t1\nmy work\t2\t0\n\n");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev");
        assert_eq!(sessions[0].windows, 3);
        assert!(sessions[0].attached);
        assert_eq!(sessions[1].name, "my work");
        assert_eq!(sessions[1].windows, 2);
        assert!(!sessions[1].attached);
    }

    #[test]
    fn parses_windows_from_tmux_tab_format_and_skips_bad_lines() {
        let windows = parse_windows("1\tbuild\t1\nnot-enough\n2\tlogs\t0\n");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].index, 1);
        assert_eq!(windows[0].name, "build");
        assert!(windows[0].active);
        assert_eq!(windows[1].index, 2);
        assert_eq!(windows[1].name, "logs");
        assert!(!windows[1].active);
    }

    #[test]
    fn tmux_command_args_use_safe_machine_readable_formats() {
        assert_eq!(
            list_sessions_args(),
            vec![
                "list-sessions",
                "-F",
                "#{session_name}\t#{session_windows}\t#{session_attached}"
            ]
        );
        assert_eq!(
            list_windows_args("dev"),
            vec![
                "list-windows",
                "-t",
                "dev",
                "-F",
                "#{window_index}\t#{window_name}\t#{window_active}"
            ]
        );
        assert_eq!(
            switch_session_args("work", true),
            vec!["switch-client", "-t", "work"]
        );
        assert_eq!(
            switch_session_args("work", false),
            vec!["attach-session", "-t", "work"]
        );
        assert_eq!(
            new_session_args("scratch"),
            vec!["new-session", "-d", "-s", "scratch"]
        );
        assert_eq!(
            select_window_args("dev", 2),
            vec!["select-window", "-t", "dev:2"]
        );
        assert_eq!(new_window_args("dev"), vec!["new-window", "-t", "dev"]);
        assert_eq!(detach_args(), vec!["detach-client"]);
    }

    #[test]
    fn tmux_command_args_cover_real_pane_and_window_actions() {
        assert_eq!(
            capture_pane_args("dev"),
            vec!["capture-pane", "-p", "-t", "dev"]
        );
        assert_eq!(
            send_keys_args("dev", &["-l".to_string(), "a".to_string()]),
            vec!["send-keys", "-t", "dev", "-l", "a"]
        );
        assert_eq!(
            kill_window_args("dev", 2),
            vec!["kill-window", "-t", "dev:2"]
        );
    }

    #[test]
    fn parses_capture_output_and_trims_trailing_blank_lines() {
        assert_eq!(parse_capture("alpha\nbeta\n\n"), vec!["alpha", "beta"]);
        assert_eq!(parse_capture(""), Vec::<String>::new());
    }
}
