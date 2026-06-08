//! Non-interactive environment diagnostics (`tuimux --doctor`).
//!
//! Maps to PRD RK-2's "clear diagnostic command" and SRS ENV-1 checks. Prints a
//! checklist and returns a non-zero exit code if anything required is missing, so
//! it is usable in scripts and CI.

use std::env;

use crate::tmux;

/// Run the diagnostics and return a process exit code (0 = all good).
pub fn run() -> i32 {
    let mut ok = true;
    println!("tuimux doctor");
    println!("=============");
    println!("tuimux version : {}", env!("CARGO_PKG_VERSION"));
    println!();

    // --- tmux presence & version -------------------------------------------
    match tmux::probe() {
        Ok(probe) => {
            line("tmux binary", true, &probe.binary);
            match probe.version {
                Some(v) if v.is_supported() => {
                    line(
                        "tmux version",
                        true,
                        &format!("{v} (>= {} required)", tmux::MIN_SUPPORTED),
                    );
                }
                Some(v) => {
                    ok = false;
                    line(
                        "tmux version",
                        false,
                        &format!(
                            "{v} is too old; need >= {}. Try: brew install tmux",
                            tmux::MIN_SUPPORTED
                        ),
                    );
                }
                None => {
                    ok = false;
                    line(
                        "tmux version",
                        false,
                        &format!("could not parse `{}`", probe.raw_version),
                    );
                }
            }
        }
        Err(e) => {
            ok = false;
            line("tmux binary", false, &e);
            println!("                  install with: brew install tmux  (macOS) / apt install tmux (Debian)");
        }
    }

    // --- terminal environment ----------------------------------------------
    let term = env::var("TERM").unwrap_or_default();
    let term_ok = !term.is_empty() && term != "dumb";
    line(
        "TERM",
        term_ok,
        if term.is_empty() { "(unset)" } else { &term },
    );

    let colorterm = env::var("COLORTERM").unwrap_or_default();
    let truecolor = colorterm.contains("truecolor") || colorterm.contains("24bit");
    line(
        "truecolor",
        true, // informational, not required
        if truecolor {
            "yes"
        } else {
            "no (256/mono fallback will be used)"
        },
    );

    // A tmux/SSH session indicator — purely informational.
    let inside_tmux = env::var("TMUX").is_ok();
    line(
        "running inside tmux",
        true,
        if inside_tmux { "yes" } else { "no" },
    );

    println!();
    if ok {
        println!("Result: OK — environment looks ready for tuimux.");
    } else {
        println!("Result: PROBLEMS FOUND — see the ✗ lines above.");
    }

    if ok {
        0
    } else {
        1
    }
}

/// Print one checklist line: `✓/✗ label : detail`.
fn line(label: &str, ok: bool, detail: &str) {
    let mark = if ok { "✓" } else { "✗" };
    println!("{mark} {label:<16}: {detail}");
}
