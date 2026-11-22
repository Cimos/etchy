#![no_main]
//! Fuzz waiver-file parsing (#425). The TOML step lives in the CLI binary, so
//! this target mirrors `parse_waivers` in etchy-cli: the same `WaiverFile`
//! shape and toml settings, then `Waiver::new` on every entry. The contract:
//! for ANY text, parsing returns entries or an error — it never panics, and a
//! waiver that builds has an ordered box and a non-empty reason.
//!
//! Run: `cargo +nightly fuzz run parse_waivers`

use etchy_core::Waiver;
use libfuzzer_sys::fuzz_target;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WaiverFile {
    #[serde(default)]
    waiver: Vec<WaiverEntry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WaiverEntry {
    layer: String,
    #[serde(rename = "box")]
    bbox: Vec<f64>,
    reason: String,
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(file) = toml::from_str::<WaiverFile>(text) else {
        return;
    };
    for e in &file.waiver {
        let Ok(bbox) = <[f64; 4]>::try_from(e.bbox.as_slice()) else {
            continue;
        };
        if let Ok(w) = Waiver::new(&e.layer, bbox, &e.reason) {
            let [x0, y0, x1, y1] = w.box_nm();
            assert!(x0 <= x1 && y0 <= y1, "built waiver has an inverted box");
            assert!(!w.reason.is_empty());
        }
    }
});
