//! Small system clipboard bridge.
//!
//! Avoids a GUI clipboard dependency so cross-compilation stays simple. The
//! common platform clipboard commands are used when available.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

pub fn copy_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        return pipe_to("pbcopy", &[], text);
    }

    #[cfg(target_os = "windows")]
    {
        return pipe_to("clip", &[], text);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if command_exists("wl-copy") {
            return pipe_to("wl-copy", &[], text);
        }
        if command_exists("xclip") {
            return pipe_to("xclip", &["-selection", "clipboard"], text);
        }
        if command_exists("xsel") {
            return pipe_to("xsel", &["--clipboard", "--input"], text);
        }
        bail!("no clipboard command found; install wl-copy, xclip, or xsel");
    }

    #[allow(unreachable_code)]
    {
        bail!("system clipboard is not supported on this platform")
    }
}

fn pipe_to(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start clipboard command `{program}`"))?;
    child
        .stdin
        .as_mut()
        .context("clipboard command stdin is unavailable")?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        bail!("clipboard command `{program}` exited with {status}")
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command_exists(program: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {program} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
