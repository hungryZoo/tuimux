//! tuimux — a prefix-free, mouse-first TUI front-end for tmux.
//!
//! See `docs/prd.md` and `docs/srs.md` for the full design. This binary is an
//! early MVP. The default path must show the tuimux ratatui interface; a plain
//! native tmux client is available only as an explicit fallback.

mod doctor;
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
    about = "Prefix-free, mouse-first TUI front-end for tmux",
    long_about = "tuimux is a TUI front-end for tmux. Its default mode opens the tuimux ratatui \n\
                  interface with a mouse-first session/window sidebar over live tmux state.\n\n\
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

    /// Fallback: open a plain native tmux client instead of the tuimux TUI.
    #[arg(long, hide = true)]
    native_client: bool,

    /// tmux session to create/attach. Defaults to `tuimux`.
    #[arg(long, value_name = "NAME", default_value = "tuimux")]
    session: String,

    /// Directory to use for future workspace features (default: current directory).
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.doctor {
        return code(doctor::run());
    }

    let base = cli
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    if cli.layout_preview {
        let (cols, rows) = crossterm_terminal::size().unwrap_or((80, 24));
        let data = PreviewData::default();
        println!(
            "{}",
            preview::render(&base, &data, cols as usize, rows as usize)
        );
        return ExitCode::SUCCESS;
    }

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

            let result = match choose_run_mode(&cli) {
                RunMode::Dashboard => tui::run(&probe),
                RunMode::NativeClient => run_native_tmux(&probe, &cli.session),
            };

            match result {
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
                "\ntmux is required. Install it and try again:\n  \
                 macOS:  brew install tmux\n  \
                 Debian: sudo apt install tmux\n\n\
                 Then run `tuimux --doctor` to verify your environment."
            );
            code(1)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Dashboard,
    NativeClient,
}

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
