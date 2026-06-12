//! Simplified layout preview (SRS §5.1).
//!
//! Renders the tuimux screen as a static text preview: a center terminal area,
//! a compact right sidebar with a button-like session name, a red Detach button,
//! vertical window tabs, and a centered session dialog scaffold. The early
//! explorer, bottom menu bar, and PROCS panel were intentionally removed.

use std::path::Path;

/// Preview content. The live client derives equivalent data from NativeMux;
/// this static data keeps --layout-preview reproducible.
pub struct PreviewData {
    pub sessions: Vec<(&'static str, u32, bool)>, // (name, windows, active)
    pub windows: Vec<(u32, &'static str, bool)>,  // (index, name, active)
    pub panes: Vec<&'static str>,
}

impl Default for PreviewData {
    fn default() -> Self {
        PreviewData {
            sessions: vec![("dev", 3, true), ("work", 2, false), ("scratch", 1, false)],
            windows: vec![(1, "build", true), (2, "logs", false), (3, "ssh", false)],
            panes: vec![
                "pane 0 (focus)            pane 1",
                "$ cargo build             $ htop",
                "  Compiling tuimux…         tasks: 142",
                "  Compiling ratatui…        load:  0.4",
                "────────────────(drag border ↔ to resize)────────",
                "pane 2",
                "$ tail -f app.log",
                "  [14:03:11] GET /  200",
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

/// Build a "name … info" row that fits `width`, right-aligning the info.
fn name_info_row(name: &str, info: &str, width: usize) -> String {
    let info_len = info.chars().count();
    if width <= info_len + 1 {
        return fit(name, width);
    }
    let name_room = width - info_len - 1;
    let name_fit = fit(name, name_room);
    format!("{name_fit} {info}")
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

    let main = main_column(&data.panes, main_w, body_h);
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

    overlay_session_dialog(&mut out, data, width);
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

    rows.push(center_button("Session", w));
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
    if w <= 2 {
        return fit("✕", w);
    }
    let close = "✕";
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

fn overlay_session_dialog(out: &mut String, data: &PreviewData, width: usize) {
    let dialog_w = 34usize.min(width.saturating_sub(4)).max(24);
    let pad = (width.saturating_sub(dialog_w)) / 2;
    let indent = " ".repeat(pad);

    out.push('\n');
    out.push_str(&indent);
    out.push('┌');
    out.push_str(&"─".repeat(dialog_w - 2));
    out.push_str("┐\n");
    for (name, windows, active) in &data.sessions {
        out.push_str(&indent);
        out.push('│');
        let mark = if *active { "●" } else { " " };
        out.push_str(&name_info_row(
            &format!(" {mark} {name}"),
            &format!("{windows} win"),
            dialog_w - 2,
        ));
        out.push_str("│\n");
    }
    out.push_str(&indent);
    out.push('│');
    out.push_str(&fit(" ─────────────────────────────", dialog_w - 2));
    out.push_str("│\n");
    out.push_str(&indent);
    out.push('│');
    out.push_str(&fit(" [ New Session ]   [ Detach ]", dialog_w - 2));
    out.push_str("│\n");
    out.push_str(&indent);
    out.push('└');
    out.push_str(&"─".repeat(dialog_w - 2));
    out.push('┘');
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
    fn name_info_row_right_aligns_info() {
        let row = name_info_row("main.rs", "12K", 16);
        assert_eq!(row.chars().count(), 16);
        assert!(row.ends_with("12K"));
        assert!(row.starts_with("main.rs"));
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
            out.contains("[ Session ]"),
            "sidebar session button should be labeled Session, not the session name"
        );
        assert!(
            out.contains("[ Detach ]"),
            "detach should render as a button label only"
        );
        assert!(out.contains("WINDOWS"));
        assert!(
            out.contains('✕'),
            "window rows should expose a right-side close button"
        );
        assert!(
            out.contains("[ New Session ]"),
            "session dialog should expose an explicit New Session button"
        );
        assert!(
            out.contains("● dev"),
            "session list still shows the active session name"
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
        assert!(
            !out.contains("session:"),
            "session label prefix was removed"
        );
        assert!(!out.contains('▾'), "dropdown glyph was removed");
    }
}
