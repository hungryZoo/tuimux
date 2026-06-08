//! tmux discovery and version handling.
//!
//! tuimux (per the SRS) is a *front-end* for a tmux server reached over control
//! mode (`tmux -CC`). The control-mode client itself is future work; for the MVP
//! this module covers what the `--doctor` and default-run paths need today:
//! locating the `tmux` binary and parsing `tmux -V` into something comparable.

use std::fmt;
use std::process::Command;

/// Minimum tmux version tuimux targets. Control mode exists earlier, but 3.0 is
/// the floor we document and test against (it is what ships on current macOS via
/// Homebrew and recent Linux distros).
pub const MIN_SUPPORTED: TmuxVersion = TmuxVersion {
    major: 3,
    minor: 0,
    suffix_ascii: 0,
};

/// A parsed tmux version such as `3.0a` or `next-3.4`.
///
/// tmux versions are `MAJOR.MINOR` optionally followed by a single lowercase
/// letter (`a`, `b`, …) denoting a patch release. We capture the letter as its
/// ASCII byte (0 = no suffix) so ordering is trivial and total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxVersion {
    pub major: u32,
    pub minor: u32,
    /// ASCII value of the patch letter, or 0 when absent. `a` == 97.
    pub suffix_ascii: u8,
}

impl TmuxVersion {
    /// Parse the output of `tmux -V` (e.g. `"tmux 3.0a"`), or a bare version
    /// string (`"3.4"`, `"next-3.4"`). Returns `None` if no `MAJOR.MINOR` core
    /// can be found.
    pub fn parse(raw: &str) -> Option<TmuxVersion> {
        // Strip an optional leading "tmux " program name.
        let s = raw.trim();
        let s = s.strip_prefix("tmux").map(str::trim_start).unwrap_or(s);

        // Skip any non-numeric prefix (e.g. the "next-" of a development build,
        // or "openbsd-") to land on the first digit of the version core. This
        // also drops trailing noise that contains dashes, like
        // "3.2a (some-distro-build)", because we start the token at the digit.
        let start = s.find(|c: char| c.is_ascii_digit())?;
        let s = &s[start..];

        // Collect the leading number.number[letter] token, ignoring any trailing
        // commentary some builds append.
        let token: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || c.is_ascii_alphabetic())
            .collect();
        if token.is_empty() {
            return None;
        }

        // Split off a trailing alphabetic suffix (the patch letter).
        let digits_end = token
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(token.len());
        let (numeric, suffix) = token.split_at(digits_end);

        let mut parts = numeric.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        // A missing minor (e.g. just "3") is treated as ".0".
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

    /// Whether this version meets tuimux's minimum requirement.
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

/// Result of probing the environment for a usable tmux.
#[derive(Debug, Clone)]
pub struct TmuxProbe {
    /// Path/name used to invoke tmux (currently always `"tmux"` from `PATH`).
    pub binary: String,
    /// Raw `tmux -V` output, trimmed (e.g. `"tmux 3.0a"`).
    pub raw_version: String,
    /// Parsed version, if `tmux -V` was understood.
    pub version: Option<TmuxVersion>,
}

impl TmuxProbe {
    /// True when tmux is present and meets the minimum version. Used by callers
    /// that want a single yes/no gate instead of inspecting `version`.
    #[allow(dead_code)]
    pub fn is_usable(&self) -> bool {
        self.version.map(|v| v.is_supported()).unwrap_or(false)
    }
}

/// Run `tmux -V` and parse the result.
///
/// Returns `Err` with a human-friendly message if tmux is missing or could not
/// be executed. A present-but-unparsable version is *not* an error here — it is
/// surfaced via `TmuxProbe { version: None, .. }` so callers can decide.
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
}
