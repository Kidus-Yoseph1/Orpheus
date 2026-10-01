//! GUIDE §8 — directory scanning for books.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FoundBook {
    pub path: PathBuf,
    pub format: String,
}

pub fn scan_directory(dir: &Path, recursive: bool) -> Vec<FoundBook> {
    let mut out = Vec::new();
    scan_inner(dir, recursive, 0, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn scan_inner(dir: &Path, recursive: bool, depth: usize, out: &mut Vec<FoundBook>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if recursive {
                scan_inner(&p, recursive, depth + 1, out);
            }
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let lower = ext.to_lowercase();
            if lower == "epub" || lower == "pdf" {
                out.push(FoundBook {
                    path: p,
                    format: lower,
                });
            }
        }
    }
}
