//! Filesystem → engine input. The GUI owns I/O + path policy (like the CLI); the
//! pure naming/classify lives in `etchy_core::naming`.

use std::io::{Cursor, Read};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use etchy_core::{Board, GerberFormat, Layer};

/// Reject any single layer file larger than this before reading it into RAM. The
/// loaders copy the bytes a few times (read → utf8 → normalized), so an oversized
/// or junk file is a quick OOM (#82). 100 MiB is far above any real fab layer.
pub(crate) const MAX_LAYER_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Reject a `.zip` fab pack larger than this before reading it into RAM. A pack of
/// Gerber text is small even zipped; 500 MiB is a generous ceiling that still
/// guards against a junk/huge file (#82). Native only (the whole-file read guard);
/// web hands us the bytes already, guarded per-entry in [`load_zip`].
#[cfg(not(target_arch = "wasm32"))]
const MAX_ZIP_BYTES: u64 = 500 * 1024 * 1024;

/// Build a [`Board`] from an in-memory set of `(filename, bytes)` layer files.
/// Shared by every input path — the folder loader, the `.zip` loader, and GUI
/// drag-and-drop — so the classify/sniff/polygonize logic lives in exactly one
/// place (#93). Files are processed in filename order for a deterministic layer
/// order. Non-Gerber files are skipped; a Gerber that fails to polygonize is a
/// hard error (fail-loud over wrong-but-quiet).
pub fn board_from_bytes(
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(Board, Option<etchy_core::GerberFormat>)> {
    let mut files: Vec<(String, Vec<u8>)> = files.into_iter().collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut layers = Vec::new();
    let mut fmt = None;
    for (name, bytes) in files {
        if bytes.len() as u64 > MAX_LAYER_FILE_BYTES {
            bail!(
                "{name} is {} bytes, over the {MAX_LAYER_FILE_BYTES}-byte per-file limit",
                bytes.len()
            );
        }
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (name.as_str(), ""),
        };
        let mut kind = etchy_core::classify(stem, ext);
        // Gerber layer, Excellon/NC drill, pick-and-place, or neither (skip). Drill
        // (#62) and P&P (#115) route through their own front-ends so those changes
        // diff instead of being silently dropped.
        let geometry = if etchy_core::looks_like_gerber(&bytes) {
            if fmt.is_none() {
                fmt = etchy_core::gerber_format(&bytes).ok();
            }
            std::sync::Arc::new(
                etchy_core::polygonize_gerber(&bytes)
                    .with_context(|| format!("processing layer {name}"))?,
            )
        } else if etchy_core::looks_like_excellon(&bytes) {
            // Content wins over the filename: an Excellon file named e.g.
            // Board.TXT must land on the Drill layer, not "other". Plating
            // (PTH/NPTH) still comes from the filename (#237).
            kind = etchy_core::LayerKind::Drill(etchy_core::drill_kind(stem));
            std::sync::Arc::new(
                etchy_core::resolve_excellon(&bytes)
                    .with_context(|| format!("processing drill layer {name}"))?,
            )
        } else if etchy_core::looks_like_placement(&bytes) {
            kind = etchy_core::LayerKind::Placement;
            std::sync::Arc::new(
                etchy_core::resolve_placement(&bytes)
                    .with_context(|| format!("processing placement file {name}"))?,
            )
        } else {
            continue;
        };
        layers.push(Layer {
            kind,
            label: name,
            geometry,
        });
    }
    Ok((Board { layers }, fmt))
}

/// Walk a directory (one level), read each Gerber file, classify it, and
/// polygonize it into a [`Layer`]. Non-Gerber files are skipped (Excellon later).
/// Native only — the web build has no filesystem (it loads via bytes/zip).
#[cfg(not(target_arch = "wasm32"))]
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

    // Read the bytes here (with a pre-read size guard so a huge file can't OOM
    // us before we even look at it, #82) and hand the shared builder the set.
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(entries.len());
    for path in entries {
        let len = path
            .metadata()
            .with_context(|| format!("reading metadata for {}", path.display()))?
            .len();
        if len > MAX_LAYER_FILE_BYTES {
            bail!(
                "{} is {len} bytes, over the {MAX_LAYER_FILE_BYTES}-byte per-file limit",
                path.display()
            );
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        files.push((name, bytes));
    }
    board_from_bytes(files)
}

/// Read a `.zip` fab pack (its raw bytes) into a [`Board`]. Entries are flattened
/// to their basename (a pack zipped with a top folder still classifies correctly),
/// and each entry's size is enforced on the *actual bytes read* so a zip bomb
/// can't OOM us. Cross-platform (in-memory) so the same path serves native file
/// picks and web uploads.
pub fn load_zip(bytes: Vec<u8>) -> Result<(Board, Option<GerberFormat>)> {
    load_zip_capped(bytes, MAX_LAYER_FILE_BYTES)
}

/// [`load_zip`] with an explicit per-entry byte cap so the guard is testable with
/// a small limit. The cap is enforced on the bytes actually decompressed, never on
/// the header-declared uncompressed size: that size is attacker-controlled
/// metadata, so a forged header that under-declares while its deflate stream
/// expands past the limit would otherwise sail straight through (#246). We read
/// through a reader capped at `limit + 1` and bail the moment more than `limit`
/// bytes come out — the stream is abandoned before it can inflate to gigabytes.
fn load_zip_capped(bytes: Vec<u8>, limit: u64) -> Result<(Board, Option<GerberFormat>)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("reading zip archive")?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .with_context(|| format!("reading zip entry {i}"))?;
        if !entry.is_file() {
            continue;
        }
        // Flatten any in-zip directory to the basename so classification (which
        // keys on filename) works regardless of how the pack was zipped.
        let name = entry
            .name()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let mut buf = Vec::new();
        let read = (&mut entry)
            .take(limit + 1)
            .read_to_end(&mut buf)
            .with_context(|| format!("extracting {name}"))?;
        if read as u64 > limit {
            bail!(
                "zip entry {name} expands past the {limit}-byte per-file limit \
                 (header declared {} bytes)",
                entry.size()
            );
        }
        files.push((name, buf));
    }
    board_from_bytes(files)
}

/// Load one revision from a filesystem path: a directory of Gerbers, or a `.zip`
/// fab pack. The native GUI's "Open…" and drag-and-drop both go through here.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_source(path: &Path) -> Result<(Board, Option<GerberFormat>)> {
    if path.is_dir() {
        return load_board(path);
    }
    let is_zip = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if is_zip {
        let len = path
            .metadata()
            .with_context(|| format!("reading metadata for {}", path.display()))?
            .len();
        if len > MAX_ZIP_BYTES {
            bail!(
                "{} is {len} bytes, over the {MAX_ZIP_BYTES}-byte zip limit",
                path.display()
            );
        }
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return load_zip(bytes);
    }
    bail!("{} is not a folder or a .zip fab pack", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use etchy_core::LayerKind;

    // Minimal valid RS-274X: one 1mm circular flash at the origin.
    const MIN_GERBER: &[u8] = b"%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1.0*%\nD10*\nX0Y0D03*\nM02*\n";

    #[test]
    fn board_from_bytes_classifies_and_skips_non_gerber() {
        let files = vec![
            ("readme.txt".to_string(), b"not a gerber file".to_vec()),
            ("board-F_Cu.gtl".to_string(), MIN_GERBER.to_vec()),
        ];
        let (board, fmt) = board_from_bytes(files).unwrap();
        assert_eq!(board.layers.len(), 1, "the non-Gerber file is skipped");
        assert_eq!(board.layers[0].kind, LayerKind::TopCopper);
        assert_eq!(board.layers[0].label, "board-F_Cu.gtl");
        assert!(fmt.is_some(), "format sniffed from the one Gerber");
    }

    #[test]
    fn board_from_bytes_rejects_oversized() {
        let big = vec![0u8; (MAX_LAYER_FILE_BYTES + 1) as usize];
        let err = board_from_bytes(vec![("huge.gtl".to_string(), big)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("per-file limit"), "got: {err}");
    }

    #[test]
    fn load_zip_flattens_entries_and_skips_non_gerber() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            // Zipped under a top folder — load_zip should flatten to the basename.
            w.start_file("fab/board-F_Cu.gtl", opts).unwrap();
            w.write_all(MIN_GERBER).unwrap();
            w.start_file("fab/readme.txt", opts).unwrap();
            w.write_all(b"just notes").unwrap();
            w.finish().unwrap();
        }
        let (board, fmt) = load_zip(buf).unwrap();
        assert_eq!(board.layers.len(), 1, "non-Gerber entry skipped");
        assert_eq!(board.layers[0].kind, LayerKind::TopCopper);
        assert_eq!(
            board.layers[0].label, "board-F_Cu.gtl",
            "flattened basename"
        );
        assert!(fmt.is_some());
    }

    /// Overwrite the little-endian u32 at `field_off` inside the record that
    /// begins with `sig`, but only when it currently holds `expect` — so we patch
    /// the intended header field and nothing that merely happens to match.
    fn forge_u32(buf: &mut [u8], sig: &[u8; 4], field_off: usize, expect: u32, forged: u32) {
        let pos = buf
            .windows(4)
            .position(|w| w == sig)
            .expect("record signature present");
        let at = pos + field_off;
        assert_eq!(
            u32::from_le_bytes(buf[at..at + 4].try_into().unwrap()),
            expect,
            "field holds the expected real size before forging"
        );
        buf[at..at + 4].copy_from_slice(&forged.to_le_bytes());
    }

    // #246: the cap must bite on the bytes actually decompressed, not the size the
    // header claims. We build a real entry, then forge its declared uncompressed
    // size down to 10 in both the local file header and the central directory —
    // the old `entry.size()` check would wave it through.
    #[test]
    fn load_zip_enforces_actual_bytes_over_a_forged_declared_size() {
        use std::io::Write;
        // Real content ~2 KiB (well over our test cap of 100), but highly
        // compressible so the archive stays tiny.
        let payload = MIN_GERBER.repeat(40);
        let real = payload.len() as u32;
        assert!(real > 100, "payload must exceed the test cap");
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("bomb-F_Cu.gtl", opts).unwrap();
            w.write_all(&payload).unwrap();
            w.finish().unwrap();
        }
        // Local file header (PK\x03\x04): uncompressed size at +22.
        forge_u32(&mut buf, b"PK\x03\x04", 22, real, 10);
        // Central directory header (PK\x01\x02): uncompressed size at +24 —
        // this is the field `entry.size()` reports.
        forge_u32(&mut buf, b"PK\x01\x02", 24, real, 10);

        // Cap of 100: above the forged 10 (so a declared-size check passes it) but
        // below the real payload (so an actual-bytes check must reject it).
        let err = load_zip_capped(buf, 100)
            .expect_err("a forged small declaration must not bypass the cap")
            .to_string();
        assert!(err.contains("per-file limit"), "got: {err}");
    }

    // A small honest zip still loads through the capped path.
    #[test]
    fn load_zip_capped_accepts_within_limit() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            w.start_file("board-F_Cu.gtl", opts).unwrap();
            w.write_all(MIN_GERBER).unwrap();
            w.finish().unwrap();
        }
        let (board, _) = load_zip_capped(buf, MAX_LAYER_FILE_BYTES).unwrap();
        assert_eq!(board.layers.len(), 1);
    }
}
