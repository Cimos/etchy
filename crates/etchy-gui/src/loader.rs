//! Filesystem → engine input. The GUI owns I/O + path policy (like the CLI); the
//! pure naming/classify lives in `etchy_core::naming`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use etchy_core::{Board, Layer};

/// Walk a directory (one level), read each Gerber file, classify it, and
/// polygonize it into a [`Layer`]. Non-Gerber files are skipped (Excellon later).
pub fn load_board(dir: &Path) -> Result<(Board, Option<etchy_core::GerberFormat>)> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    let mut layers = Vec::new();
    let mut fmt = None;
    for path in entries {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        if !etchy_core::looks_like_gerber(&bytes) {
            continue;
        }
        if fmt.is_none() {
            fmt = etchy_core::gerber_format(&bytes).ok();
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let kind = etchy_core::classify(stem, ext);
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_string();
        let geometry = std::sync::Arc::new(
            etchy_core::polygonize_gerber(&bytes)
                .with_context(|| format!("processing layer {label}"))?,
        );
        layers.push(Layer {
            kind,
            label,
            geometry,
        });
    }
    Ok((Board { layers }, fmt))
}
