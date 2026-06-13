//! Simplified layout preview (SRS §5.1).
//!
//! Renders the tuimux screen as a static text preview: a center terminal area,
//! a compact right sidebar with a red Detach button and vertical window tabs.
//! The early split panes, explorer, bottom menu bar, session picker, and PROCS
//! panel were intentionally removed.

use std::path::Path;

/// Preview content. The live client derives equivalent data from NativeMux;
/// this static data keeps --layout-preview reproducible.
pub struct PreviewData {
    pub windows: Vec<(u32, &'static str, bool)>, // (index, name, active)
    pub terminal_lines: Vec<&'static str>,
}

impl Default for PreviewData {
    fn default() -> Self {
        PreviewData {
            windows: vec![(1, "build", true), (2, "logs", false), (3, "ssh", false)],
            terminal_lines: vec![
                "$ cargo test --quiet",
                "running 43 tests",
                "...........................................",
                "test result: ok. 43 passed; 0 failed",
                "",
                "$ btop",
                "  PTY body runs beside the integrated tuimux rail",
                "  click + new, close, Detach, or STATUS in the rail",
            ],
        }
    }
}

/// Truncate `s` to `width` display columns (counted as chars — preview content is
/// single-width) and left-pad/right-pad to exactly `width`.
fn fit(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count == width {
        s.to_string()
    } else if count > width {
        if width == 0 {
            String::new()
        } else if width == 1 {
            "…".to_string()
        } else {
            let kept: String = s.chars().take(width - 1).collect();
            format!("{kept}…")
        }
    } else {
        format!("{s}{}", " ".repeat(width - count))
    }
}

/// Render the simplified layout to a string (newline-separated rows), sized to
/// `width` × `height` columns/rows. Reasonable minimums are enforced.
pub fn render(_base: &Path, data: &PreviewData, width: usize, height: usize) -> String {
    let width = width.max(60);
    let height = height.max(16);

    // Main + right sidebar. Outer border + separator consume 3 columns.
    let right_w = 22usize;
    let main_w = width.saturating_sub(right_w + 3);

    // Body height = total - top border - separator - bottom border.
    let body_h = height.saturating_sub(3).max(8);

    let main = main_column(&data.terminal_lines, main_w, body_h);
    let right = right_column(data, right_w, body_h);

    let mut out = String::new();

    out.push('┌');
    out.push_str(&"─".repeat(width - 2));
    out.push_str("┐\n");

    out.push('├');
    out.push_str(&"─".repeat(main_w));
    out.push('┬');
    out.push_str(&"─".repeat(right_w));
    out.push_str("┤\n");

    for i in 0..body_h {
        out.push('│');
        out.push_str(&main[i]);
        out.push('│');
        out.push_str(&right[i]);
        out.push_str("│\n");
    }

    out.push('└');
    out.push_str(&"─".repeat(main_w));
    out.push('┴');
    out.push_str(&"─".repeat(right_w));
    out.push('┘');

    out
}

fn main_column(panes: &[&str], w: usize, h: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(h);
    for line in panes {
        if rows.len() >= h {
            break;
        }
        rows.push(fit(line, w));
    }
    pad_rows(rows, w, h)
}

fn right_column(data: &PreviewData, w: usize, h: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(h);

    rows.push(center_button("Detach", w));
    rows.push(fit(&"─".repeat(w), w));
    rows.push(fit("WINDOWS", w));
    for (idx, name, active) in &data.windows {
        if rows.len() >= h {
            break;
        }
        let marker = if *active { "▸" } else { " " };
        rows.push(fit(
            &row_with_close(&format!("{marker} {idx}: {name}"), w),
            w,
        ));
    }
    if rows.len() < h {
        rows.push(fit("  + new", w));
    }

    pad_rows(rows, w, h)
}

fn row_with_close(label: &str, w: usize) -> String {
    if w <= 3 {
        return fit(" X ", w);
    }
    let close = " X ";
    let label_w = w.saturating_sub(close.chars().count() + 1);
    format!("{} {}", fit(label, label_w), close)
}

fn center_button(label: &str, w: usize) -> String {
    let button = format!("[ {label} ]");
    let len = button.chars().count();
    if len >= w {
        return fit(&button, w);
    }
    let left = (w - len) / 2;
    let right = w - len - left;
    format!("{}{}{}", " ".repeat(left), button, " ".repeat(right))
}

/// Ensure exactly `h` rows, each exactly `w` columns wide.
fn pad_rows(mut rows: Vec<String>, w: usize, h: usize) -> Vec<String> {
    rows.truncate(h);
    while rows.len() < h {
        rows.push(" ".repeat(w));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcd", 4), "abcd");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("x", 0), "");
    }

    #[test]
    fn render_produces_rectangular_output_with_simplified_sidebar() {
        let data = PreviewData::default();
        let out = render(Path::new(env!("CARGO_MANIFEST_DIR")), &data, 80, 24);
        let lines: Vec<&str> = out.lines().take(24).collect();
        let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.iter().all(|&x| x == widths[0]),
            "rows are not rectangular: {widths:?}"
        );

        assert!(
            out.contains("[ Detach ]"),
            "detach should render as a button label only"
        );
        assert!(out.contains("WINDOWS"));
        assert!(
            out.contains(" X "),
            "window rows should expose a right-side close button"
        );
        assert!(
            !out.contains("Session"),
            "preview should not expose sessions"
        );

        assert!(
            !out.contains("MAIN AREA"),
            "main pane border/header should not expose internal wording"
        );
        assert!(
            !out.contains("mock"),
            "preview should not call the main pane a mock"
        );
        assert!(
            !out.contains(" tuimux ·"),
            "top header/status row was removed"
        );

        assert!(
            !out.contains("[ dev ]"),
            "sidebar button label should not be dev"
        );
        assert!(
            !out.contains("Session picker"),
            "dialog header/title was removed"
        );
        assert!(
            !out.contains("┌──── Sessions"),
            "dialog border title was removed"
        );
        assert!(!out.contains("EXPLORER"), "left file explorer was removed");
        assert!(!out.contains("PROCS"), "right PROCS panel was removed");
        assert!(!out.contains("Detach Alt-d"), "bottom menu bar was removed");
        assert!(!out.contains("pane 0"), "split pane sample was removed");
        assert!(
            !out.contains("drag border"),
            "preview should not advertise split resizing"
        );
        assert!(
            !out.contains("session:"),
            "session label prefix was removed"
        );
        assert!(!out.contains('▾'), "dropdown glyph was removed");
    }
}
