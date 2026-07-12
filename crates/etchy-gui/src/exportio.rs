//! File output for the GUI export (#60). The viewer owns I/O (like the CLI); what
//! to export — per-layer SVGs + the copper-area CSV, or per-page PDF-diff overlay
//! PNGs (#63) — is built by the caller from `etchy-core`/`etchy-pdf`. Native
//! writes a folder next to the cwd; web triggers downloads. Contents are bytes so
//! binary formats (PNG) ride the same path as text.

/// One file to emit: a name and its contents (text or binary).
pub struct ExportFile {
    pub name: String,
    pub content: Vec<u8>,
}

/// Save the files. Returns a short user-facing message for the toast (Ok) or an
/// error string (Err). Native: writes them into `./etchy-export/`. Web: downloads
/// each via the browser.
#[cfg(not(target_arch = "wasm32"))]
pub fn save(files: &[ExportFile]) -> Result<String, String> {
    use std::io::Write;
    let dir = std::path::PathBuf::from("etchy-export");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    for f in files {
        let path = dir.join(&f.name);
        std::fs::File::create(&path)
            .and_then(|mut w| w.write_all(&f.content))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(format!(
        "Exported {} file(s) to {}/",
        files.len(),
        dir.display()
    ))
}

#[cfg(target_arch = "wasm32")]
pub fn save(files: &[ExportFile]) -> Result<String, String> {
    for f in files {
        download(&f.name, &f.content)?;
    }
    Ok(format!("Downloaded {} file(s)", files.len()))
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
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}
