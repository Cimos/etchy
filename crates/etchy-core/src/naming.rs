//! Pure naming heuristics shared by the CLI and GUI surfaces: filename →
//! [`LayerKind`] and a Gerber content sniff. **No I/O** — the binaries do the
//! filesystem walk and pass strings/bytes here, keeping path/discovery policy in
//! the surfaces while the (rename-tolerant) classification stays in one place.

use crate::model::{DrillKind, FilePolarity, LayerKind};

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

/// Parse the X2 `.FilePolarity` file attribute out of a Gerber's bytes (#317):
/// `%TF.FilePolarity,Negative*%`, or its X1-compatible comment form
/// `G04 #@! TF.FilePolarity,Negative*` (spec §5.1). `None` when the attribute is
/// absent or malformed — callers treat that as positive, the spec default.
///
/// Byte-sniffed like [`file_function`], but not line-based: exporters may pack
/// several `%…%` blocks on one line, and missing a `Negative` here would silently
/// invert every sign on the layer, so the token is searched wherever it follows a
/// `%` or a `#@!` marker. The first declaration wins.
pub fn file_polarity(bytes: &[u8]) -> Option<FilePolarity> {
    const TOKEN: &str = "TF.FilePolarity,";
    let text = String::from_utf8_lossy(bytes);
    text.match_indices(TOKEN).find_map(|(at, _)| {
        // Only an extended-code block or a standard-comment attribute counts —
        // not the token quoted in an ordinary comment.
        let before = text[..at].trim_end();
        let marked = before.ends_with('%') || before.ends_with("#@!");
        if !marked {
            return None;
        }
        let rest = &text[at + TOKEN.len()..];
        let end = rest.find(['*', '%', '\n', '\r']).unwrap_or(rest.len());
        match rest[..end].trim().to_ascii_uppercase().as_str() {
            "POSITIVE" => Some(FilePolarity::Positive),
            "NEGATIVE" => Some(FilePolarity::Negative),
            _ => None,
        }
    })
}

/// Parse the X2 `.FileFunction` file attribute (`%TF.FileFunction,<args>*%`) out
/// of a Gerber's bytes and map it to a [`LayerKind`] (#239). X2 files declare
/// their true layer role machine-readably, so this is authoritative where the
/// filename heuristic is silent and a cross-check where it is not — see
/// [`reconcile_kind`]. Returns `None` when the attribute is absent or names a role
/// we don't model, so callers fall back to the filename (never a guess).
///
/// Byte-sniffed like [`looks_like_gerber`] / [`crate::gerber_format`] — the
/// attribute is a standalone `%…%` block, so we don't need the full parse. The
/// first `.FileFunction` declaration wins.
pub fn file_function(bytes: &[u8]) -> Option<LayerKind> {
    let text = String::from_utf8_lossy(bytes);
    let body = text.lines().find_map(|l| {
        let l = l.trim();
        let l = l.strip_prefix('%').unwrap_or(l);
        // Strip the trailing block terminator (`*%` after the `%` prefix is gone,
        // so `*`), tolerating a missing one.
        l.strip_prefix("TF.FileFunction,")
            .map(|rest| rest.trim_end_matches('%').trim_end_matches('*'))
    })?;
    let mut f = body.split(',').map(str::trim);
    let role = f.next()?;
    // Position keyword (`Top` / `Bot`) → the top/bottom pair for a side-scoped
    // layer (mask/legend/paste). `None` for anything else.
    let side_pair = |top: LayerKind, bot: LayerKind, s: Option<&str>| match s
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("TOP") => Some(top),
        Some("BOT") => Some(bot),
        _ => None,
    };
    match role.to_ascii_uppercase().as_str() {
        "COPPER" => {
            // Copper,L<n>,<Top|Bot|Inr>[,<type>]
            let layer = f.next()?; // e.g. "L2"
            let side = f.next()?.to_ascii_uppercase();
            let n: u8 = layer.trim_start_matches(['L', 'l']).parse().ok()?;
            match side.as_str() {
                "TOP" => Some(LayerKind::TopCopper),
                "BOT" => Some(LayerKind::BottomCopper),
                // Physical stack number `L<n>` → ORDINAL inner index (top = L1), so
                // `L2` is the 1st inner — matching classify()'s `.gl<n>` and
                // `In<n>_Cu` handling so an attribute and a filename agree on the
                // same physical layer. `L1,Inr` is malformed → `None`.
                "INR" => n
                    .checked_sub(1)
                    .filter(|o| *o >= 1)
                    .map(LayerKind::InnerCopper),
                _ => None,
            }
        }
        "SOLDERMASK" => side_pair(LayerKind::TopMask, LayerKind::BottomMask, f.next()),
        "LEGEND" => side_pair(LayerKind::TopSilk, LayerKind::BottomSilk, f.next()),
        "PASTE" => side_pair(LayerKind::TopPaste, LayerKind::BottomPaste, f.next()),
        // Board outline. Plating suffix (`,P` / `,NP`) is irrelevant to the kind.
        "PROFILE" => Some(LayerKind::Outline),
        // Drill/rout files carry their plating in the role itself.
        "PLATED" => Some(LayerKind::Drill(DrillKind::Plated)),
        "NONPLATED" => Some(LayerKind::Drill(DrillKind::NonPlated)),
        _ => None,
    }
}

/// A human label for a [`LayerKind`] that keeps the inner-copper index (unlike the
/// JSON-contract [`LayerKind::kind_str`], where every inner is just
/// `"inner-copper"`), so a conflict warning names the exact layer.
fn kind_label(k: LayerKind) -> String {
    match k.inner_index() {
        Some(n) => format!("inner-copper-{n}"),
        None => k.kind_str().to_string(),
    }
}

/// Cross-check the filename classification against the X2 `.FileFunction`
/// attribute (#239) and return the kind to use plus an optional warning. This is a
/// confirmation / tiebreaker, **never a silent override** (the trust bar):
///
/// * attribute absent → filename kind, no warning (behaviour unchanged);
/// * attribute agrees → filename kind, no warning (confirmed);
/// * filename matched nothing (`Other`) but the file declares a role → adopt the
///   attribute and note it (the recovery this issue is about — a rename that broke
///   the heuristic, e.g. #111/#176);
/// * both are concrete but disagree → keep the (pairing-stable) filename kind and
///   warn loudly, so a genuine mismatch is surfaced, not silently resolved.
pub fn reconcile_kind(
    filename: LayerKind,
    attr: Option<LayerKind>,
    label: &str,
) -> (LayerKind, Option<String>) {
    let Some(attr) = attr else {
        return (filename, None);
    };
    if attr == filename {
        return (filename, None);
    }
    if filename == LayerKind::Other {
        let msg = format!(
            "{label}: filename matched no layer naming pattern; classified as {} from its \
             X2 .FileFunction attribute",
            kind_label(attr)
        );
        return (attr, Some(msg));
    }
    let msg = format!(
        "{label}: filename classifies this as {} but its X2 .FileFunction attribute says {}; \
         keeping the filename classification — check the export or rename the file",
        kind_label(filename),
        kind_label(attr)
    );
    (filename, Some(msg))
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

    // ---- #239: X2 .FileFunction attribute cross-check ----

    /// Wrap a `.FileFunction` value in a minimal valid X2 header.
    fn ff(value: &str) -> Vec<u8> {
        format!("%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,{value}*%\nM02*\n").into_bytes()
    }

    #[test]
    fn file_function_maps_standard_roles() {
        assert_eq!(
            file_function(&ff("Copper,L1,Top")),
            Some(LayerKind::TopCopper)
        );
        assert_eq!(
            file_function(&ff("Copper,L4,Bot")),
            Some(LayerKind::BottomCopper)
        );
        // Physical L2 = 1st inner (ordinal), matching `.gl2` / `In1_Cu`.
        assert_eq!(
            file_function(&ff("Copper,L2,Inr")),
            Some(LayerKind::InnerCopper(1))
        );
        assert_eq!(
            file_function(&ff("Copper,L3,Inr")),
            Some(LayerKind::InnerCopper(2))
        );
        assert_eq!(
            file_function(&ff("Soldermask,Top")),
            Some(LayerKind::TopMask)
        );
        assert_eq!(
            file_function(&ff("Soldermask,Bot")),
            Some(LayerKind::BottomMask)
        );
        assert_eq!(file_function(&ff("Legend,Top")), Some(LayerKind::TopSilk));
        assert_eq!(
            file_function(&ff("Paste,Bot")),
            Some(LayerKind::BottomPaste)
        );
        assert_eq!(file_function(&ff("Profile,NP")), Some(LayerKind::Outline));
        assert_eq!(file_function(&ff("Profile")), Some(LayerKind::Outline));
        assert_eq!(
            file_function(&ff("Plated,1,2,PTH")),
            Some(LayerKind::Drill(DrillKind::Plated))
        );
        assert_eq!(
            file_function(&ff("NonPlated,1,2,NPTH")),
            Some(LayerKind::Drill(DrillKind::NonPlated))
        );
    }

    #[test]
    fn file_polarity_parses_both_values_and_defaults_to_none() {
        // #317: the attribute is a standalone %TF block; case and a missing `*`
        // are tolerated, anything else is None (callers treat None as positive).
        let g = |v: &str| format!("%FSLAX46Y46*%\n%TF.FilePolarity,{v}*%\n%MOMM*%\n");
        assert_eq!(
            file_polarity(g("Negative").as_bytes()),
            Some(FilePolarity::Negative)
        );
        assert_eq!(
            file_polarity(g("Positive").as_bytes()),
            Some(FilePolarity::Positive)
        );
        assert_eq!(
            file_polarity(g("negative").as_bytes()),
            Some(FilePolarity::Negative)
        );
        assert_eq!(
            file_polarity(b"%TF.FilePolarity,Negative%\n"),
            Some(FilePolarity::Negative)
        );
        assert_eq!(file_polarity(g("Sideways").as_bytes()), None);
        assert_eq!(file_polarity(b"%FSLAX46Y46*%\n%MOMM*%\n"), None);
        // A .FileFunction line is not a polarity.
        assert_eq!(file_polarity(b"%TF.FileFunction,Copper,L1,Top*%\n"), None);
        // The X1-compatible comment form (spec §5.1) counts…
        assert_eq!(
            file_polarity(b"G04 #@! TF.FilePolarity,Negative*\n%FSLAX46Y46*%\n"),
            Some(FilePolarity::Negative)
        );
        // …and so does a block that shares its line with other blocks.
        assert_eq!(
            file_polarity(b"%FSLAX46Y46*%%MOMM*%%TF.FilePolarity,Negative*%%LPD*%\n"),
            Some(FilePolarity::Negative)
        );
        // The token quoted in an ordinary comment is not a declaration.
        assert_eq!(
            file_polarity(b"G04 exporter note: TF.FilePolarity,Negative is unsupported*\n"),
            None
        );
    }

    #[test]
    fn file_function_absent_or_unmodelled_is_none() {
        // No attribute at all → None (filename-only path, unchanged behaviour).
        assert_eq!(file_function(b"%FSLAX46Y46*%\n%MOMM*%\nM02*\n"), None);
        // A role we deliberately don't model → None, never a wrong guess.
        assert_eq!(file_function(&ff("Glue,Top")), None);
        assert_eq!(file_function(&ff("Other,mystery")), None);
        // Malformed inner (L1 can't be an inner layer) → None, not InnerCopper(0).
        assert_eq!(file_function(&ff("Copper,L1,Inr")), None);
    }

    #[test]
    fn reconcile_absent_attribute_keeps_filename() {
        // Behaviour unchanged when the file carries no .FileFunction.
        let (k, w) = reconcile_kind(LayerKind::TopCopper, None, "F_Cu.gbr");
        assert_eq!(k, LayerKind::TopCopper);
        assert!(w.is_none());
    }

    #[test]
    fn reconcile_agreement_is_silent() {
        let (k, w) = reconcile_kind(LayerKind::TopCopper, Some(LayerKind::TopCopper), "F_Cu.gbr");
        assert_eq!(k, LayerKind::TopCopper);
        assert!(w.is_none(), "a confirming attribute must not warn");
    }

    #[test]
    fn reconcile_recovers_other_from_attribute() {
        // The #239 win: an unrecognized filename that the file itself classifies.
        // The attribute is adopted, and the promotion is surfaced (not silent).
        let (k, w) = reconcile_kind(
            LayerKind::Other,
            Some(LayerKind::InnerCopper(2)),
            "weird-name.xyz",
        );
        assert_eq!(k, LayerKind::InnerCopper(2));
        let w = w.expect("adopting the attribute must be surfaced");
        assert!(
            w.contains("weird-name.xyz") && w.contains("inner-copper-2"),
            "{w}"
        );
    }

    #[test]
    fn reconcile_conflict_keeps_filename_and_warns() {
        // Trust bar: a genuine disagreement is NOT silently overridden. The
        // pairing-stable filename kind is kept, and the conflict is warned loudly.
        let (k, w) = reconcile_kind(
            LayerKind::TopCopper,
            Some(LayerKind::BottomCopper),
            "F_Cu.gbr",
        );
        assert_eq!(
            k,
            LayerKind::TopCopper,
            "filename classification is retained"
        );
        let w = w.expect("a real conflict must warn");
        assert!(
            w.contains("top-copper") && w.contains("bottom-copper") && w.contains("F_Cu.gbr"),
            "warning names both kinds and the file: {w}"
        );
    }

    #[test]
    fn sniff_detects_gerber() {
        assert!(looks_like_gerber(b"%FSLAX46Y46*%\n%MOMM*%\n"));
        assert!(!looks_like_gerber(b"M48\nINCH,TZ\nT1C0.5\n")); // excellon
        assert!(!looks_like_gerber(b"{ \"note\": \"see %MO settings\" }")); // not line-start
    }
}
