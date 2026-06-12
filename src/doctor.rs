//! Non-interactive environment diagnostics (`tuimux --doctor`).
//!
//! Prints a checklist and returns a non-zero exit code if the native tuimux
//! runtime is missing something required.

use std::env;

use crate::tmux;

/// Run the diagnostics and return a process exit code (0 = all good).
pub fn run() -> i32 {
    let mut ok = true;
    println!("tuimux doctor");
    println!("=============");
    println!("tuimux version : {}", env!("CARGO_PKG_VERSION"));
    println!();

    // --- optional tmux fallback --------------------------------------------
    match tmux::probe() {
        Ok(probe) => {
            line("tmux fallback", true, &probe.binary);
            match probe.version {
                Some(v) if v.is_supported() => {
                    line(
                        "tmux version",
                        true,
                        &format!("{v} (>= {} for --native-client)", tmux::MIN_SUPPORTED),
                    );
                }
                Some(v) => {
                    line(
                        "tmux version",
                        true,
                        &format!(
                            "{v} is too old for --native-client; native tuimux is unaffected; recommended >= {}",
                            tmux::MIN_SUPPORTED
                        ),
                    );
                }
                None => {
                    line(
                        "tmux version",
                        true,
                        &format!(
                            "could not parse `{}`; native tuimux is unaffected",
                            probe.raw_version
                        ),
                    );
                }
            }
        }
        Err(e) => {
            line(
                "tmux fallback",
                true,
                &format!("not available ({e}); native tuimux does not require tmux"),
            );
        }
    }

    // --- terminal environment ----------------------------------------------
    let term = env::var("TERM").unwrap_or_default();
    let term_ok = !term.is_empty() && term != "dumb";
    ok &= term_ok;
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
