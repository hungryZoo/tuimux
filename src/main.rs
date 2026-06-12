//! tuimux — a prefix-free, mouse-first terminal multiplexer.
//!
//! See `docs/prd.md` and `docs/srs.md` for the full design. This binary is an
//! early MVP. The default path uses tuimux's Rust-native multiplexer; a plain
//! native tmux client remains available only as an explicit fallback.

mod clipboard;
mod doctor;
mod mux_backend;
mod native_mux;
mod preview;
mod terminal;
mod tmux;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use crossterm::terminal as crossterm_terminal;

use preview::PreviewData;

/// Command-line interface. clap provides `--help` and `--version` for free.
#[derive(Parser, Debug)]
#[command(
    name = "tuimux",
    version,
    about = "Prefix-free, mouse-first Rust-native terminal multiplexer",
    long_about = "tuimux is a Rust-native terminal multiplexer with a ratatui interface, \n\
                  PTY-backed windows, mouse-first sidebar controls, and native-feeling selection.\n\n\
                  This is an early 0.x MVP. Run with no flags to start the TUI, or use \n\
                  --layout-preview / --doctor for non-interactive output."
)]
struct Cli {
    /// Run environment diagnostics and exit (non-interactive).
    #[arg(long)]
    doctor: bool,

    /// Render the VS Code-inspired layout as text and exit (non-interactive).
    #[arg(long)]
    layout_preview: bool,

    /// Run the ratatui dashboard. This is the default and kept for compatibility.
    #[arg(long, hide = true)]
    dashboard: bool,

    /// Fallback: open a plain native tmux client instead of the Rust-native tuimux TUI.
    #[arg(long, hide = true)]
    native_client: bool,

    /// Internal: run the Rust-native multiplexer daemon.
    #[arg(long, hide = true)]
    daemon: bool,

    /// Internal: socket path for daemon mode.
    #[arg(long, hide = true, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Internal: stop the Rust-native multiplexer daemon for the selected session.
    #[arg(long, hide = true)]
    stop_server: bool,

    /// tuimux session to create. Defaults to `tuimux`.
    #[arg(long, value_name = "NAME", default_value = "tuimux")]
    session: String,

    /// Directory to use for future workspace features (default: current directory).
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let base = cli
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    if cli.daemon {
        let Some(socket) = cli.socket.clone() else {
            eprintln!("tuimux: --daemon requires --socket <PATH>");
            return ExitCode::FAILURE;
        };
        return code(mux_backend::run_daemon(socket, &cli.session, base));
    }

    if cli.stop_server {
        return match mux_backend::stop_daemon(&cli.session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("tuimux: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if cli.doctor {
        return code(doctor::run());
    }

    if cli.layout_preview {
        let (cols, rows) = crossterm_terminal::size().unwrap_or((80, 24));
        let data = PreviewData::default();
        println!(
            "{}",
            preview::render(&base, &data, cols as usize, rows as usize)
        );
        return ExitCode::SUCCESS;
    }

    if cli.native_client {
        match tmux::probe() {
            Ok(probe) => {
                if let Some(v) = probe.version {
                    if !v.is_supported() {
                        eprintln!(
                        "tuimux: warning — {} {} is older than the supported {}. Some features may not work.",
                        probe.binary,
                        v,
                        tmux::MIN_SUPPORTED
                    );
                    }
                } else {
                    eprintln!(
                        "tuimux: warning — could not parse tmux version from `{}`.",
                        probe.raw_version
                    );
                }

                match run_native_tmux(&probe, &cli.session) {
                    Ok(rc) => code(rc),
                    Err(e) => {
                        eprintln!("tuimux: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            Err(e) => {
                eprintln!("tuimux: {e}");
                eprintln!(
                    "\n`--native-client` requires tmux. The default tuimux TUI no longer depends on tmux."
                );
                code(1)
            }
        }
    } else {
        match tui::run(&cli.session, base) {
            Ok(rc) => code(rc),
            Err(e) => {
                eprintln!("tuimux: {e}");
                ExitCode::FAILURE
            }
        }
    }
}

fn run_native_tmux(probe: &tmux::TmuxProbe, session: &str) -> std::io::Result<i32> {
    let client = tmux::Tmux::new(tmux::RealTmux::new(probe.binary.clone()));
    client
        .ensure_session(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    client
        .enable_mouse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    client
        .open_native_client(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    Ok(0)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Dashboard,
    NativeClient,
}

#[cfg(test)]
fn choose_run_mode(cli: &Cli) -> RunMode {
    if cli.native_client {
        RunMode::NativeClient
    } else {
        RunMode::Dashboard
    }
}

/// Convert a small integer exit code into a `process::ExitCode`.
fn code(rc: i32) -> ExitCode {
    ExitCode::from(rc.clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_without_flags() -> Cli {
        Cli {
            doctor: false,
            layout_preview: false,
            dashboard: false,
            native_client: false,
            daemon: false,
            socket: None,
            stop_server: false,
            session: "tuimux".to_string(),
            cwd: None,
        }
    }

    #[test]
    fn default_run_mode_is_ratatui_dashboard_not_plain_tmux_client() {
        let cli = cli_without_flags();

        assert_eq!(RunMode::Dashboard, choose_run_mode(&cli));
    }

    #[test]
    fn explicit_native_client_mode_is_opt_in_only() {
        let mut cli = cli_without_flags();
        cli.native_client = true;

        assert_eq!(RunMode::NativeClient, choose_run_mode(&cli));
    }
}
