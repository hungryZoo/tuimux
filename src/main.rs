//! tuimux — a prefix-free, mouse-first TUI front-end for tmux.
//!
//! See `docs/prd.md` and `docs/srs.md` for the full design. This binary is an
//! early MVP scaffold: the CLI plumbing, environment checks, layout preview, and
//! a ratatui UI shell are real; the tmux control-mode client is the next step.

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
    about = "Prefix-free, mouse-first TUI front-end for tmux (VS Code-inspired layout)",
    long_about = "tuimux is a TUI front-end for tmux. It renders a VS Code-inspired layout \n\
                  (left file explorer, center panes, right session/window tabs, bottom menu) \n\
                  and drives a tmux server over control mode.\n\n\
                  This is an early MVP scaffold. Run with no flags to open the UI, or use \n\
                  --layout-preview / --doctor for non-interactive output."
)]
struct Cli {
    /// Run environment diagnostics and exit (non-interactive).
    #[arg(long)]
    doctor: bool,

    /// Render the VS Code-inspired layout as text and exit (non-interactive).
    #[arg(long)]
    layout_preview: bool,

    /// Directory to use for the file explorer (default: current directory).
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // --doctor: full environment checklist, scriptable exit code.
    if cli.doctor {
        return code(doctor::run());
    }

    let base = cli
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // --layout-preview: static text mock of the UI.
    if cli.layout_preview {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let data = PreviewData::default();
        println!(
            "{}",
            preview::render(&base, &data, cols as usize, rows as usize)
        );
        return ExitCode::SUCCESS;
    }

    // Default: probe tmux, report it, then enter the UI.
    match tmux::probe() {
        Ok(probe) => {
            match &probe.version {
                Some(v) if v.is_supported() => {
                    eprintln!("tuimux: found {} {}", probe.binary, v);
                }
                Some(v) => {
                    eprintln!(
                        "tuimux: warning — {} {} is older than the supported {}. \
                         Some features may not work. Try: brew install tmux",
                        probe.binary,
                        v,
                        tmux::MIN_SUPPORTED
                    );
                }
                None => {
                    eprintln!(
                        "tuimux: warning — could not parse tmux version from `{}`.",
                        probe.raw_version
                    );
                }
            }
            match tui::run(&probe) {
                Ok(rc) => code(rc),
                Err(e) => {
                    eprintln!("tuimux: UI error: {e}");
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

/// Convert a small integer exit code into a `process::ExitCode`.
fn code(rc: i32) -> ExitCode {
    ExitCode::from(rc.clamp(0, 255) as u8)
}
