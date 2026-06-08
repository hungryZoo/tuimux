//! VS Code-inspired layout preview (SRS §5.1).
//!
//! Renders the tuimux screen as a static text mock: a left file explorer with
//! sizes, a center "main area" standing in for tmux panes, a right sidebar with
//! the session name + vertical window tabs + a PROCS list, and an always-visible
//! bottom menu bar containing **Detach**.
//!
//! This is deliberately data-driven so the same composition can back both the
//! non-interactive `--layout-preview` output and (later) the live ratatui UI.

use std::path::Path;

use crate::files::FileListing;

/// Mock content for the regions that aren't wired to a real tmux server yet.
/// In the live client these come from control-mode events; here they are static
/// so the preview is reproducible.
pub struct PreviewData {
    pub session: String,
    pub windows: Vec<(u32, &'static str, bool)>, // (index, name, active)
    pub panes: Vec<&'static str>,
    pub procs: Vec<(&'static str, &'static str)>, // (status glyph + cmd, pid/info)
}

impl Default for PreviewData {
    fn default() -> Self {
        PreviewData {
            session: "dev".to_string(),
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
            procs: vec![
                ("● cargo build", "pid 4211"),
                ("● htop", "pid 4250"),
                ("✓ cargo test", "ok"),
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
        // Reserve one column for an ellipsis when we have to cut.
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

/// Build a "name … size" row that fits `width`, right-aligning the size.
fn name_size_row(name: &str, size: &str, width: usize) -> String {
    let size_len = size.chars().count();
    if width <= size_len + 1 {
        return fit(name, width);
    }
    let name_room = width - size_len - 1;
    let name_fit = fit(name, name_room);
    format!("{name_fit} {size}")
}

/// Render the full layout to a string (newline-separated rows), sized to
/// `width` × `height` columns/rows. Reasonable minimums are enforced.
pub fn render(base: &Path, data: &PreviewData, width: usize, height: usize) -> String {
    let width = width.max(60);
    let height = height.max(16);

    // Column inner widths. Outer border + 2 separators consume 4 columns.
    let left_w = 16usize;
    let right_w = 20usize;
    let main_w = width.saturating_sub(left_w + right_w + 4);

    // Body height = total - top border - status - bottom border - menu - menu border.
    let body_h = height.saturating_sub(5).max(8);

    let listing = FileListing::read(base);
    let left = left_column(&listing, left_w, body_h);
    let main = main_column(&data.panes, main_w, body_h);
    let right = right_column(data, right_w, body_h);

    let mut out = String::new();

    // Top border.
    out.push('┌');
    out.push_str(&"─".repeat(width - 2));
    out.push_str("┐\n");

    // Status line (spans full inner width).
    let status = format!(
        " tuimux · session: {} · 1 client · layout preview ",
        data.session
    );
    out.push('│');
    out.push_str(&fit(&status, width - 2));
    out.push_str("│\n");

    // Separator between status and body, with column tees.
    out.push('├');
    out.push_str(&"─".repeat(left_w));
    out.push('┬');
    out.push_str(&"─".repeat(main_w));
    out.push('┬');
    out.push_str(&"─".repeat(right_w));
    out.push_str("┤\n");

    // Body rows.
    for i in 0..body_h {
        out.push('│');
        out.push_str(&left[i]);
        out.push('│');
        out.push_str(&main[i]);
        out.push('│');
        out.push_str(&right[i]);
        out.push_str("│\n");
    }

    // Separator between body and menu bar.
    out.push('├');
    out.push_str(&"─".repeat(left_w));
    out.push('┴');
    out.push_str(&"─".repeat(main_w));
    out.push('┴');
    out.push_str(&"─".repeat(right_w));
    out.push_str("┤\n");

    // Bottom menu bar — always visible, Detach first (FR-BAR-3/4).
    let menu =
        " [Detach Alt-d]  [New Alt-n]  [Split Alt-|]  [Close Alt-w]  [? Help]  [Palette Alt-p] ";
    out.push('│');
    out.push_str(&fit(menu, width - 2));
    out.push_str("│\n");

    // Bottom border.
    out.push('└');
    out.push_str(&"─".repeat(width - 2));
    out.push('┘');

    out
}

fn left_column(listing: &FileListing, w: usize, h: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(h);
    rows.push(fit("EXPLORER", w));
    let base = listing.base_path.display().to_string();
    rows.push(fit(&base, w));
    rows.push(fit(&"─".repeat(w), w));

    for entry in &listing.entries {
        if rows.len() >= h {
            break;
        }
        if entry.is_dir {
            rows.push(name_size_row(&format!("▸ {}/", entry.name), "dir", w));
        } else {
            rows.push(name_size_row(
                &format!("  {}", entry.name),
                &entry.size_display,
                w,
            ));
        }
    }

    if let Some(err) = &listing.error {
        if rows.len() < h {
            rows.push(fit(&format!("! {err}"), w));
        }
    }

    pad_rows(rows, w, h)
}

fn main_column(panes: &[&str], w: usize, h: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(h);
    rows.push(fit("MAIN AREA (tmux panes — mock)", w));
    rows.push(fit(&"─".repeat(w), w));
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
    // Session name line — clickable in the live UI (opens the session modal).
    rows.push(fit(&format!("session: {} ▾", data.session), w));
    rows.push(fit(&"─".repeat(w), w));
    rows.push(fit("WINDOWS", w));
    for (idx, name, active) in &data.windows {
        if rows.len() >= h {
            break;
        }
        let marker = if *active { "▸" } else { " " };
        rows.push(fit(&format!("{marker} {idx}: {name}"), w));
    }
    if rows.len() < h {
        rows.push(fit("  + new", w));
    }
    if rows.len() < h {
        rows.push(fit(&"─".repeat(w), w));
    }
    if rows.len() < h {
        rows.push(fit("PROCS", w));
    }
    for (cmd, info) in &data.procs {
        if rows.len() >= h {
            break;
        }
        rows.push(name_size_row(cmd, info, w));
    }
    pad_rows(rows, w, h)
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
    fn name_size_row_right_aligns_size() {
        let row = name_size_row("main.rs", "12K", 16);
        assert_eq!(row.chars().count(), 16);
        assert!(row.ends_with("12K"));
        assert!(row.starts_with("main.rs"));
    }

    #[test]
    fn render_produces_rectangular_output_with_menu() {
        let data = PreviewData::default();
        let out = render(Path::new(env!("CARGO_MANIFEST_DIR")), &data, 80, 24);
        let lines: Vec<&str> = out.lines().collect();
        // Every visual row should be the same display width.
        let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.iter().all(|&x| x == widths[0]),
            "rows are not rectangular: {widths:?}"
        );
        assert!(out.contains("Detach Alt-d"), "menu bar must contain Detach");
        assert!(out.contains("WINDOWS"));
        assert!(out.contains("EXPLORER"));
        assert!(out.contains("session: dev"));
    }
}
