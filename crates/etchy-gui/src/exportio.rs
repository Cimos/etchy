//! File output for the GUI export (#60). The viewer owns I/O (like the CLI); what
//! to export — per-layer SVGs + the copper-area CSV, or per-page PDF-diff overlay
//! PNGs (#63) — is built by the caller from `etchy-core`/`etchy-pdf`. Contents are
//! bytes so binary formats (PNG) ride the same path as text.
//!
//! Where the files land (#222 — "Export does nothing"):
//! - **Native** writes `etchy-export/` next to the last opened input when the
//!   caller passes that directory, falling back to the current directory — and
//!   the toast reports the **absolute** path, so the output is always findable
//!   (a cwd-relative path from a double-clicked exe pointed at nowhere obvious).
//! - **Web** downloads ONE file directly, but bundles a multi-file export into a
//!   single `etchy-export.zip`: browsers block the second and later automatic
//!   downloads from one click, so a per-file loop silently dropped most of the
//!   set. The synthetic `<a download>` is also attached to the DOM for the
//!   click — Firefox ignores clicks on detached anchors entirely.

/// One file to emit: a name and its contents (text or binary).
pub struct ExportFile {
    pub name: String,
    pub content: Vec<u8>,
}

/// Bundle the files into a single in-memory zip (stored per-entry with deflate).
/// Target-independent so the web bundling path is unit-testable natively — the
/// non-test native build doesn't call it (native writes real files instead).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn bundle_zip(files: &[ExportFile]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts: zip::write::SimpleFileOptions = Default::default();
    for f in files {
        w.start_file(&f.name, opts)
            .and_then(|()| w.write_all(&f.content).map_err(zip::result::ZipError::Io))
            .map_err(|e| format!("zip {}: {e}", f.name))?;
    }
    w.finish()
        .map(|c| c.into_inner())
        .map_err(|e| format!("zip finish: {e}"))
}

/// Save the files. Returns a short user-facing message for the toast (Ok) or an
/// error string (Err). `dir_hint` is where the export folder is rooted on native
/// (the last opened input's directory); web ignores it.
#[cfg(not(target_arch = "wasm32"))]
pub fn save(files: &[ExportFile], dir_hint: Option<&std::path::Path>) -> Result<String, String> {
    use std::io::Write;
    // Root next to the opened boards when known (#222): that is where a user
    // looks for output. Fall back to the cwd, made absolute so the toast never
    // shows an unanchored relative path.
    let base = match dir_hint {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir().map_err(|e| format!("resolve cwd: {e}"))?,
    };
    let dir = base.join("etchy-export");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    for f in files {
        let path = dir.join(&f.name);
        std::fs::File::create(&path)
            .and_then(|mut w| w.write_all(&f.content))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(format!(
        "Exported {} file(s) to {}",
        files.len(),
        dir.display()
    ))
}

#[cfg(target_arch = "wasm32")]
pub fn save(files: &[ExportFile], _dir_hint: Option<&std::path::Path>) -> Result<String, String> {
    match files {
        [] => Ok("Nothing to export".to_string()),
        [f] => {
            download(&f.name, &f.content)?;
            Ok(format!("Downloaded {}", f.name))
        }
        many => {
            // One click = one download (#222): browsers block the 2nd+ automatic
            // download, so a multi-file set ships as a single zip.
            let bytes = bundle_zip(many)?;
            download("etchy-export.zip", &bytes)?;
            Ok(format!(
                "Downloaded etchy-export.zip ({} files)",
                many.len()
            ))
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn download(name: &str, content: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let win = web_sys::window().ok_or("no window")?;
    let doc = win.document().ok_or("no document")?;
    // Blob from the bytes (binary-safe — a str blob would mangle PNGs), an object
    // URL, and a synthetic <a download> click.
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(content));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(|_| "blob")?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "url")?;
    let a = doc
        .create_element("a")
        .map_err(|_| "create <a>")?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "cast <a>")?;
    a.set_href(&url);
    a.set_download(name);
    // The anchor must be IN the document for the click to start a download —
    // Firefox does nothing for a detached anchor (#222).
    let body = doc.body().ok_or("no body")?;
    body.append_child(&a).map_err(|_| "append <a>")?;
    a.click();
    let _ = body.remove_child(&a);
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_zip_round_trips_every_file() {
        let files = [
            ExportFile {
                name: "page-1.png".into(),
                content: vec![0x89, b'P', b'N', b'G'],
            },
            ExportFile {
                name: "areas.csv".into(),
                content: b"layer,added\n".to_vec(),
            },
        ];
        let bytes = bundle_zip(&files).expect("bundle");
        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("read back");
        assert_eq!(ar.len(), 2);
        for f in &files {
            use std::io::Read;
            let mut entry = ar.by_name(&f.name).expect("entry present");
            let mut got = Vec::new();
            entry.read_to_end(&mut got).expect("read entry");
            assert_eq!(got, f.content, "{} content intact", f.name);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_save_writes_into_the_hinted_dir_and_reports_the_absolute_path() {
        let tmp = std::env::temp_dir().join(format!("etchy-exportio-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("tmp dir");
        let files = [ExportFile {
            name: "page-1.png".into(),
            content: vec![1, 2, 3],
        }];
        let msg = save(&files, Some(&tmp)).expect("save");
        let out = tmp.join("etchy-export").join("page-1.png");
        assert_eq!(std::fs::read(&out).expect("written"), vec![1, 2, 3]);
        // The toast carries the absolute location (#222: a bare relative
        // "etchy-export/" left the user unable to find the output).
        assert!(msg.contains(&tmp.join("etchy-export").display().to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
