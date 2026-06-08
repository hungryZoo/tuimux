//! Small, dependency-free helpers shared across tuimux.

/// Render a byte count as a compact, human-readable string.
///
/// Uses binary (1024-based) units, which is what users expect when comparing
/// against `ls -lh` on Linux/macOS. Values below 1 KiB are shown as whole bytes
/// (`512 B`); larger values use one decimal place (`1.5 KB`, `3.0 MB`).
///
/// The unit labels are intentionally the familiar `KB/MB/GB` rather than the
/// pedantic `KiB/MiB/GiB`, matching the look of the SRS layout mock.
///
/// The narrow variant ([`human_size_narrow`]) is what the explorer renders; this
/// spaced form is kept for places that have room (modals, tooltips, logs).
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    const STEP: f64 = 1024.0;

    if bytes < STEP as u64 {
        return format!("{bytes} B");
    }

    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= STEP && unit < UNITS.len() - 1 {
        size /= STEP;
        unit += 1;
    }

    format!("{size:.1} {}", UNITS[unit])
}

/// Render a byte count in a fixed, narrow width suitable for a sidebar column,
/// e.g. `12K`, `4.0K` collapses to `4K`, `1.5M`. No space, single-letter unit.
///
/// This is the form used in the left-hand file explorer where horizontal space
/// is scarce. Directories should not be passed here (callers show a marker).
pub fn human_size_narrow(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    const STEP: f64 = 1024.0;

    if bytes < STEP as u64 {
        return format!("{bytes}B");
    }

    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= STEP && unit < UNITS.len() - 1 {
        size /= STEP;
        unit += 1;
    }

    // Drop the decimal when it rounds to a whole number (12.0K -> 12K).
    if (size.fract() * 10.0).round() == 0.0 {
        format!("{}{}", size.round() as u64, UNITS[unit])
    } else {
        format!("{size:.1}{}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_below_one_kib_are_whole() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1), "1 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn kilobytes_use_one_decimal() {
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(10 * 1024), "10.0 KB");
    }

    #[test]
    fn scales_into_larger_units() {
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(human_size(1024u64.pow(4)), "1.0 TB");
        assert_eq!(human_size(1024u64.pow(5)), "1.0 PB");
    }

    #[test]
    fn very_large_values_clamp_to_top_unit() {
        // 2048 PB should still be expressed in PB, not overflow the table.
        assert_eq!(human_size(2 * 1024u64.pow(5)), "2.0 PB");
        assert_eq!(human_size(u64::MAX).split(' ').nth(1).unwrap(), "PB");
    }

    #[test]
    fn narrow_form_is_compact() {
        assert_eq!(human_size_narrow(0), "0B");
        assert_eq!(human_size_narrow(900), "900B");
        assert_eq!(human_size_narrow(12 * 1024), "12K");
        assert_eq!(human_size_narrow(1536), "1.5K");
        assert_eq!(human_size_narrow(4 * 1024), "4K");
        assert_eq!(human_size_narrow(1024 * 1024), "1M");
    }
}
