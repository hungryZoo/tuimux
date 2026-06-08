//! Left-sidebar file explorer model (FR-FILES).
//!
//! Lists a directory's entries with human-readable sizes. Directories sort first,
//! then files, both alphabetically. Errors (permission denied, missing path) are
//! returned as an empty listing rather than panicking, per ERR-6.

use std::fs;
use std::path::{Path, PathBuf};

use crate::util::human_size_narrow;

/// A single row in the explorer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    /// Pre-rendered narrow size (e.g. `12K`), or a marker for directories.
    pub size_display: String,
}

/// A directory's contents, ready to render.
#[derive(Debug, Clone)]
pub struct FileListing {
    pub base_path: PathBuf,
    pub entries: Vec<FileEntry>,
    /// Set when the directory could not be fully read (shown as a hint).
    pub error: Option<String>,
}

impl FileListing {
    /// Read `base` and build a sorted listing. Never fails hard: I/O problems
    /// land in `error` with whatever entries were readable.
    pub fn read(base: &Path) -> FileListing {
        let mut entries = Vec::new();
        let mut error = None;

        match fs::read_dir(base) {
            Ok(rd) => {
                for item in rd {
                    let item = match item {
                        Ok(i) => i,
                        Err(e) => {
                            error = Some(format!("partial read: {e}"));
                            continue;
                        }
                    };
                    let name = item.file_name().to_string_lossy().into_owned();
                    // Use symlink_metadata so we describe the link itself, not its
                    // (possibly broken or out-of-tree) target.
                    let meta = item.metadata().or_else(|_| item.path().symlink_metadata());
                    let (is_dir, size_bytes) = match meta {
                        Ok(m) => (m.is_dir(), m.len()),
                        Err(_) => (false, 0),
                    };
                    let size_display = if is_dir {
                        "dir".to_string()
                    } else {
                        human_size_narrow(size_bytes)
                    };
                    entries.push(FileEntry {
                        name,
                        is_dir,
                        size_bytes,
                        size_display,
                    });
                }
            }
            Err(e) => {
                error = Some(format!("{e}"));
            }
        }

        sort_entries(&mut entries);

        FileListing {
            base_path: base.to_path_buf(),
            entries,
            error,
        }
    }
}

/// Directories first, then files; each group sorted case-insensitively by name.
fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_sort_before_files() {
        let mut v = vec![
            FileEntry {
                name: "zfile".into(),
                is_dir: false,
                size_bytes: 1,
                size_display: "1B".into(),
            },
            FileEntry {
                name: "adir".into(),
                is_dir: true,
                size_bytes: 0,
                size_display: "dir".into(),
            },
            FileEntry {
                name: "bfile".into(),
                is_dir: false,
                size_bytes: 1,
                size_display: "1B".into(),
            },
            FileEntry {
                name: "Cdir".into(),
                is_dir: true,
                size_bytes: 0,
                size_display: "dir".into(),
            },
        ];
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["adir", "Cdir", "bfile", "zfile"]);
    }

    #[test]
    fn reading_a_real_directory_lists_this_crate() {
        // The crate root always has Cargo.toml; use it as a stable fixture.
        let listing = FileListing::read(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(
            listing.error.is_none(),
            "unexpected error: {:?}",
            listing.error
        );
        assert!(
            listing
                .entries
                .iter()
                .any(|e| e.name == "Cargo.toml" && !e.is_dir),
            "Cargo.toml should be listed"
        );
        assert!(
            listing.entries.iter().any(|e| e.name == "src" && e.is_dir),
            "src/ should be listed as a directory"
        );
    }

    #[test]
    fn missing_path_yields_error_not_panic() {
        let listing = FileListing::read(Path::new("/this/path/does/not/exist/tuimux"));
        assert!(listing.entries.is_empty());
        assert!(listing.error.is_some());
    }
}
