//! Pure naming heuristics shared by the CLI and GUI surfaces: filename →
//! [`LayerKind`] and a Gerber content sniff. **No I/O** — the binaries do the
//! filesystem walk and pass strings/bytes here, keeping path/discovery policy in
//! the surfaces while the (rename-tolerant) classification stays in one place.

use crate::model::LayerKind;

/// Content sniff for a Gerber layer. RS-274X requires a format-spec (`%FS`) and a
/// mode (`%MO`) statement, each its own `%…%` block on a line. We look for either
/// as a **line-start** marker, scanning the **whole** file — not a byte-capped
/// substring match (which both skipped real layers with long X2/attribute headers
/// and false-matched prose). Non-Gerber files (drill, job, READMEs) lack these.
pub fn looks_like_gerber(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("%FS") || t.starts_with("%MO")
    })
}

/// Map a Gerber filename (stem + extension) → [`LayerKind`]. Rename-tolerant:
/// keys on KiCad-style suffixes and the standard Gerber extensions.
pub fn classify(stem: &str, ext: &str) -> LayerKind {
    let s = stem.to_ascii_uppercase();
    let e = ext.to_ascii_lowercase();

    if let Some(n) = inner_copper_index(&s) {
        return LayerKind::InnerCopper(n);
    }
    // Altium inner-copper extensions: `.g1` .. `.g99` (but not `.gm1`, `.gtl`, …).
    if let Some(n) = e.strip_prefix('g').and_then(|d| d.parse::<u8>().ok()) {
        if (1..=99).contains(&n) {
            return LayerKind::InnerCopper(n);
        }
    }
    // Altium fabrication-documentation gerbers: drill drawing (`.gd*`), drill
    // guide (`.gg*`), pad master (`.gpt`/`.gpb`). These are drawings/masters, not
    // board copper and not the real Excellon drill file — classify by extension
    // BEFORE the stem heuristics below, so a `DrillDrawing.gd1` isn't grabbed by
    // the `has("DRILL")` branch and mislabelled as the actual drill layer.
    if e.starts_with("gd") || e.starts_with("gg") || e == "gpt" || e == "gpb" {
        return LayerKind::Documentation;
    }
    let has = |needle: &str| s.contains(needle);
    if has("F_CU") || has("F.CU") || e == "gtl" {
        LayerKind::TopCopper
    } else if has("B_CU") || has("B.CU") || e == "gbl" {
        LayerKind::BottomCopper
    } else if has("F_MASK") || has("F.MASK") || e == "gts" {
        LayerKind::TopMask
    } else if has("B_MASK") || has("B.MASK") || e == "gbs" {
        LayerKind::BottomMask
    } else if has("F_SILK") || has("F.SILK") || e == "gto" {
        LayerKind::TopSilk
    } else if has("B_SILK") || has("B.SILK") || e == "gbo" {
        LayerKind::BottomSilk
    } else if has("F_PASTE") || has("F.PASTE") || e == "gtp" {
        LayerKind::TopPaste
    } else if has("B_PASTE") || has("B.PASTE") || e == "gbp" {
        LayerKind::BottomPaste
    } else if has("DRILL") || e == "drl" || e == "xln" {
        LayerKind::Drill
    } else if has("EDGE") || has("OUTLINE") || e == "gko" || e == "gm1" || e == "gm" {
        LayerKind::Outline
    } else {
        LayerKind::Other
    }
}

/// Extract the inner-copper index from an uppercased stem: `IN<digits>` followed
/// by `_CU` / `.CU` (e.g. `In3_Cu` → 3). `None` otherwise (so `OUTLINE` etc. don't
/// false-match).
pub fn inner_copper_index(upper: &str) -> Option<u8> {
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if &bytes[i..i + 2] == b"IN" {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 {
                let rest = &upper[j..];
                if rest.starts_with("_CU") || rest.starts_with(".CU") {
                    if let Ok(n) = upper[i + 2..j].parse::<u8>() {
                        return Some(n);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_kicad_names() {
        assert_eq!(classify("synth-F_Cu", "gbr"), LayerKind::TopCopper);
        assert_eq!(classify("synth-B_Cu", "gbr"), LayerKind::BottomCopper);
        assert_eq!(classify("synth-In1_Cu", "gbr"), LayerKind::InnerCopper(1));
        assert_eq!(classify("synth-In12_Cu", "gbr"), LayerKind::InnerCopper(12));
        assert_eq!(classify("synth-F_Mask", "gbr"), LayerKind::TopMask);
        assert_eq!(classify("synth-F_Silk", "gbr"), LayerKind::TopSilk);
        assert_eq!(classify("synth-B_Paste", "gbr"), LayerKind::BottomPaste);
        assert_eq!(classify("board-Edge_Cuts", "gm1"), LayerKind::Outline);
        assert_eq!(classify("something", "gtl"), LayerKind::TopCopper);
        assert_eq!(classify("random", "txt"), LayerKind::Other);
    }

    #[test]
    fn classify_altium_documentation() {
        // Drill drawing / guide and pad master → Documentation, not Other.
        assert_eq!(classify("board", "gd1"), LayerKind::Documentation);
        assert_eq!(classify("board", "gg1"), LayerKind::Documentation);
        assert_eq!(classify("board", "gpt"), LayerKind::Documentation);
        assert_eq!(classify("board", "gpb"), LayerKind::Documentation);
        // Extension wins over a "DRILL" in the stem: a drill *drawing* is docs,
        // not the actual (Excellon) drill layer.
        assert_eq!(classify("DrillDrawing", "gd1"), LayerKind::Documentation);
        // The real Excellon drill file is still Drill.
        assert_eq!(classify("board-PTH", "drl"), LayerKind::Drill);
        // Altium profile/mechanical outline extensions still map to Outline, and
        // paste (.gtp/.gbp) is not confused with pad master (.gpt/.gpb).
        assert_eq!(classify("board", "gm"), LayerKind::Outline);
        assert_eq!(classify("board", "gtp"), LayerKind::TopPaste);
        assert_eq!(classify("board", "gbp"), LayerKind::BottomPaste);
    }

    #[test]
    fn inner_index_does_not_false_match() {
        assert_eq!(inner_copper_index("OUTLINE"), None);
        assert_eq!(inner_copper_index("F_CU"), None);
        assert_eq!(inner_copper_index("IN3_CU"), Some(3));
        assert_eq!(inner_copper_index("IN10.CU"), Some(10));
    }

    #[test]
    fn sniff_detects_gerber() {
        assert!(looks_like_gerber(b"%FSLAX46Y46*%\n%MOMM*%\n"));
        assert!(!looks_like_gerber(b"M48\nINCH,TZ\nT1C0.5\n")); // excellon
        assert!(!looks_like_gerber(b"{ \"note\": \"see %MO settings\" }")); // not line-start
    }
}
