//! tuimux — a prefix-free, mouse-first TUI front-end for tmux.
//!
//! See `docs/prd.md` and `docs/srs.md` for the full design. This binary is an
//! early MVP. As of v0.1.7 the default path is intentionally tmux-native: it
//! opens a real tmux client instead of trying to emulate a shell in ratatui.

mod doctor;
mod preview;
mod tmux;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use crossterm::terminal;

use preview::PreviewData;

/// Command-line interface. clap provides `--help` and `--version` for free.
#[derive(Parser, Debug)]
#[command(
    name = "tuimux",
    version,
    about = "Prefix-free, mouse-first TUI front-end for tmux (tmux-native)",
    long_about = "tuimux is a TUI front-end for tmux. Its default mode opens a real tmux client \n\
                  instead of scraping and replaying a shell. That keeps ls, nano/vim/less, mouse \n\
                  wheel, UTF-8/CJK text, alternate screen, and copy-mode behavior owned by tmux.\n\n\
                  This is an early 0.x MVP. Run with no flags to attach a tmux session, or use \n\
                  --layout-preview / --doctor for non-interactive output."
)]
struct Cli {
    /// Run environment diagnostics and exit (non-interactive).
    #[arg(long)]
    doctor: bool,

    /// Render the VS Code-inspired layout as text and exit (non-interactive).
    #[arg(long)]
    layout_preview: bool,

    /// Run the experimental ratatui dashboard prototype instead of a native tmux client.
    #[arg(long, hide = true)]
    dashboard: bool,

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
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
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

            let result = if cli.dashboard {
                tui::run(&probe)
            } else {
                run_native_tmux(&probe, &cli.session)
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

/// Convert a small integer exit code into a `process::ExitCode`.
fn code(rc: i32) -> ExitCode {
    ExitCode::from(rc.clamp(0, 255) as u8)
}
