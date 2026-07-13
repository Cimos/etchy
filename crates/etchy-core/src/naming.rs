//! Pure naming heuristics shared by the CLI and GUI surfaces: filename →
//! [`LayerKind`] and a Gerber content sniff. **No I/O** — the binaries do the
//! filesystem walk and pass strings/bytes here, keeping path/discovery policy in
//! the surfaces while the (rename-tolerant) classification stays in one place.

use crate::model::{DrillKind, LayerKind};

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
    // KiCad inner-copper extensions: `.gl2` .. `.gl99` ("Copper,L<n>,Inr").
    // KiCad names these by PHYSICAL stack position (top is L1), so on a 4-layer
    // board `.gl2`/`.gl3` are the two inners. But `InnerCopper` is indexed
    // ORDINALLY everywhere else (1st inner = 1, matching `In1_Cu` and Altium
    // `.g1`), so convert physical → ordinal by dropping the top layer: `.gl2` →
    // InnerCopper(1), `.gl3` → InnerCopper(2). Without this a `.gl2` rev wouldn't
    // pair against an `In1_Cu`/`.g1` rev of the same board (#176). `.gl1` (physical
    // top) isn't inner and falls through — top copper is `.gtl`. Does not clash with
    // top/bottom copper (`.gtl`/`.gbl`) which start with `gt`/`gb`, not `gl`.
    if let Some(n) = e.strip_prefix("gl").and_then(|d| d.parse::<u8>().ok()) {
        if (2..=99).contains(&n) {
            return LayerKind::InnerCopper(n - 1);
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
        LayerKind::Drill(drill_kind(stem))
    } else if has("EDGE") || has("OUTLINE") || e == "gko" || e == "gm1" || e == "gm" {
        LayerKind::Outline
    } else if e == "pos" || has("PICK") || has("PLACE") || has("CENTROID") || has("PNP") {
        // Pick-and-place / centroid files (#115): KiCad `.pos`, or a CSV named
        // pick-place / centroid / pnp.
        LayerKind::Placement
    } else {
        LayerKind::Other
    }
}

/// Detect drill plating from a filename stem (#237). Fab tools mark plated vs
/// non-plated holes with `PTH` / `NPTH` (KiCad `*-PTH.drl` / `*-NPTH.drl`, Altium
/// split drills) or the spelled-out `PLATED` / `NON-PLATED`. NPTH is checked
/// first because `NPTH` and `NON-PLATED` both contain the plated marker. A stem
/// with no marker (a combined `.drl`, or Altium's `Board.TXT`) is `Unspecified`.
pub fn drill_kind(stem: &str) -> DrillKind {
    // Uppercase and fold `_`/space to `-` so `NON_PLATED`/`NON PLATED` read the
    // same as `NON-PLATED`.
    let s: String = stem
        .to_ascii_uppercase()
        .chars()
        .map(|c| if c == '_' || c == ' ' { '-' } else { c })
        .collect();
    if s.contains("NPTH") || s.contains("NON-PLATED") || s.contains("NONPLATED") {
        DrillKind::NonPlated
    } else if s.contains("PTH") || s.contains("PLATED") {
        DrillKind::Plated
    } else {
        DrillKind::Unspecified
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
        // The real Excellon drill file is still Drill (plated here).
        assert_eq!(
            classify("board-PTH", "drl"),
            LayerKind::Drill(DrillKind::Plated)
        );
        // Altium profile/mechanical outline extensions still map to Outline, and
        // paste (.gtp/.gbp) is not confused with pad master (.gpt/.gpb).
        assert_eq!(classify("board", "gm"), LayerKind::Outline);
        assert_eq!(classify("board", "gtp"), LayerKind::TopPaste);
        assert_eq!(classify("board", "gbp"), LayerKind::BottomPaste);
    }

    #[test]
    fn classify_kicad_gl_inner_copper() {
        // KiCad emits inner copper as `.gl<n>` (e.g. Mad_RP2040.gl2 =
        // "Copper,L2,Inr"). These must be Copper, not Other (#176).
        //
        // `.gl<n>` uses the PHYSICAL stack number (top is L1), but the rest of the
        // engine indexes inner copper ORDINALLY — 1st inner layer = 1 — matching
        // KiCad `In1_Cu` and Altium `.g1` (MidLayer 1). So a 4-layer board's first
        // inner (`.gl2`, physical L2) is InnerCopper(1), and the second (`.gl3`) is
        // InnerCopper(2). Using the physical number here would mislabel the layer
        // ("inner 2" for the first inner) and — worse — fail to pair a `.gl2` rev
        // against an `In1_Cu`/`.g1` rev of the same board (#176).
        assert_eq!(classify("Mad_RP2040", "gl2"), LayerKind::InnerCopper(1));
        assert_eq!(classify("Mad_RP2040", "gl3"), LayerKind::InnerCopper(2));
        assert!(classify("Mad_RP2040", "gl2").is_copper());
        // Cross-scheme consistency: the same physical inner layer gets the same
        // ordinal index whether it arrives as KiCad protel `.gl2`, KiCad
        // `In1_Cu`, or Altium `.g1` — so pairing old-vs-new never spuriously
        // reports a removed+added inner layer just because the export changed.
        assert_eq!(
            classify("board", "gl2"),
            classify("board-In1_Cu", "gbr"),
            ".gl2 (physical L2) must match In1_Cu (1st inner)"
        );
        assert_eq!(
            classify("board", "gl2"),
            classify("board", "g1"),
            ".gl2 (physical L2) must match Altium .g1 (MidLayer 1)"
        );
        // Top/bottom copper (`.gtl`/`.gbl`) are unaffected by the `gl` branch.
        assert_eq!(classify("board", "gtl"), LayerKind::TopCopper);
        assert_eq!(classify("board", "gbl"), LayerKind::BottomCopper);
    }

    #[test]
    fn inner_index_does_not_false_match() {
        assert_eq!(inner_copper_index("OUTLINE"), None);
        assert_eq!(inner_copper_index("F_CU"), None);
        assert_eq!(inner_copper_index("IN3_CU"), Some(3));
        assert_eq!(inner_copper_index("IN10.CU"), Some(10));
    }

    #[test]
    fn classify_distinguishes_pth_npth_drills() {
        // #237: plated / non-plated / generic drill files get distinct kinds so
        // pairing can only match like with like. NPTH must not be read as PTH
        // (the `NPTH` string contains `PTH`).
        assert_eq!(
            classify("board-PTH", "drl"),
            LayerKind::Drill(DrillKind::Plated)
        );
        assert_eq!(
            classify("board-NPTH", "drl"),
            LayerKind::Drill(DrillKind::NonPlated)
        );
        assert_eq!(
            classify("board-NPTH", "drl").kind_str(),
            "drill-npth",
            "gate token `drill` still matches (contains check), but the tag is specific"
        );
        // A combined drill file with no plating marker stays generic.
        assert_eq!(
            classify("board", "drl"),
            LayerKind::Drill(DrillKind::Unspecified)
        );
        // Spelled-out and underscore/space variants.
        assert_eq!(drill_kind("board-NON_PLATED"), DrillKind::NonPlated);
        assert_eq!(drill_kind("board Non Plated"), DrillKind::NonPlated);
        assert_eq!(drill_kind("board-plated"), DrillKind::Plated);
        assert_eq!(drill_kind("Board.TXT-ish"), DrillKind::Unspecified);
    }

    #[test]
    fn sniff_detects_gerber() {
        assert!(looks_like_gerber(b"%FSLAX46Y46*%\n%MOMM*%\n"));
        assert!(!looks_like_gerber(b"M48\nINCH,TZ\nT1C0.5\n")); // excellon
        assert!(!looks_like_gerber(b"{ \"note\": \"see %MO settings\" }")); // not line-start
    }
}
