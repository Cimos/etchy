//! Layer + board model, layer pairing, and the same-board guard.
//!
//! Purity: a [`Layer`] carries an opaque `label` string (for diagnostics/report),
//! **not** a `PathBuf`. The CLI classifies filenames → [`LayerKind`] (naming
//! policy) and hands the engine an already-tagged [`Board`]; the engine never
//! touches the filesystem or interprets a path.

use std::sync::Arc;

use crate::error::{EngineError, Result};
use crate::geo::{PolygonSet, NM_PER_MM};

/// Plating of an Excellon drill file. Real fab packs routinely ship plated and
/// non-plated holes as separate files (`*-PTH.drl` / `*-NPTH.drl`, Altium split
/// drills). Carrying the plating in [`LayerKind::Drill`] keeps them distinct
/// identities so pairing can only match like with like (#237) — a PTH file can
/// never pair against an NPTH file across two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrillKind {
    /// Plated through-holes (PTH).
    Plated,
    /// Non-plated through-holes (NPTH).
    NonPlated,
    /// Plating not indicated by the filename (a combined/generic drill file).
    Unspecified,
}

/// Rename-tolerant layer identity used to pair the two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerKind {
    TopCopper,
    BottomCopper,
    InnerCopper(u8),
    TopMask,
    BottomMask,
    TopSilk,
    BottomSilk,
    TopPaste,
    BottomPaste,
    Drill(DrillKind),
    Outline,
    /// Fabrication documentation (drill drawing/guide, pad master, assembly).
    /// Not a physical board layer — never diffed as copper; kept distinct from
    /// `Other` so Altium doc exports don't show up as anonymous "other".
    Documentation,
    /// Pick-and-place / component centroids, rendered as position+rotation markers
    /// (#115) — a placement diff, not copper geometry.
    Placement,
    Other,
}

impl LayerKind {
    pub fn is_copper(&self) -> bool {
        matches!(
            self,
            LayerKind::TopCopper | LayerKind::BottomCopper | LayerKind::InnerCopper(_)
        )
    }

    /// Stable kebab-case tag for the JSON contract (independent of Rust repr).
    pub fn kind_str(&self) -> &'static str {
        match self {
            LayerKind::TopCopper => "top-copper",
            LayerKind::BottomCopper => "bottom-copper",
            LayerKind::InnerCopper(_) => "inner-copper",
            LayerKind::TopMask => "top-mask",
            LayerKind::BottomMask => "bottom-mask",
            LayerKind::TopSilk => "top-silk",
            LayerKind::BottomSilk => "bottom-silk",
            LayerKind::TopPaste => "top-paste",
            LayerKind::BottomPaste => "bottom-paste",
            LayerKind::Drill(DrillKind::Plated) => "drill-pth",
            LayerKind::Drill(DrillKind::NonPlated) => "drill-npth",
            LayerKind::Drill(DrillKind::Unspecified) => "drill",
            LayerKind::Outline => "outline",
            LayerKind::Documentation => "documentation",
            LayerKind::Placement => "placement",
            LayerKind::Other => "other",
        }
    }

    /// Inner-copper index (1-based), if applicable.
    pub fn inner_index(&self) -> Option<u8> {
        match self {
            LayerKind::InnerCopper(n) => Some(*n),
            _ => None,
        }
    }

    /// A total order for stable, human-sensible report ordering (stack top→bottom).
    fn sort_key(&self) -> (u8, u8) {
        match self {
            LayerKind::TopCopper => (0, 0),
            LayerKind::InnerCopper(n) => (1, *n),
            LayerKind::BottomCopper => (2, 0),
            LayerKind::TopMask => (3, 0),
            LayerKind::BottomMask => (4, 0),
            LayerKind::TopSilk => (5, 0),
            LayerKind::BottomSilk => (6, 0),
            LayerKind::TopPaste => (7, 0),
            LayerKind::BottomPaste => (8, 0),
            // Group all drill files together in the report, plated before
            // non-plated before generic, so a pack with several drill files reads
            // in a stable order.
            LayerKind::Drill(d) => (
                9,
                match d {
                    DrillKind::Plated => 0,
                    DrillKind::NonPlated => 1,
                    DrillKind::Unspecified => 2,
                },
            ),
            LayerKind::Outline => (10, 0),
            LayerKind::Documentation => (11, 0),
            LayerKind::Placement => (12, 0),
            LayerKind::Other => (13, 0),
        }
    }
}

/// The X2 `%TF.FilePolarity` file attribute: whether the layer's objects draw
/// material (`Positive`, the default) or its absence (`Negative`, e.g. a plane
/// layer exported as clearances). etchy resolves the objects as drawn either way;
/// the polarity decides what a geometric difference *means* (#317).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilePolarity {
    Positive,
    Negative,
}

/// One resolved layer: its kind, an opaque label (e.g. the filename, used only in
/// diagnostics — never as a path), and its filled geometry.
#[derive(Debug, Clone)]
pub struct Layer {
    pub kind: LayerKind,
    pub label: String,
    /// Shared so the diff can hand the same geometry to the report and the
    /// viewer without deep-copying it per layer (was the parallel-diff memory
    /// spike, #81). Clones are refcount bumps.
    pub geometry: Arc<PolygonSet>,
    /// `true` when the file declares `%TF.FilePolarity,Negative*%`: its geometry
    /// is the *absence* of material, so `added`/`removed` are swapped when the
    /// layer is diffed and a negative layer never pairs against a positive one
    /// (#317). Always `false` for Excellon and placement layers.
    pub negative: bool,
}

/// A whole fab pack: the set of resolved layers for one revision.
#[derive(Debug, Clone, Default)]
pub struct Board {
    pub layers: Vec<Layer>,
}

impl Board {
    /// Union bounding box over every layer's geometry, in nm.
    pub fn bbox_nm(&self) -> Option<[i64; 4]> {
        self.layers
            .iter()
            .filter_map(|l| l.geometry.bbox_nm())
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
    }
}

/// How a layer pairs across the two revisions.
#[derive(Debug)]
pub enum LayerPairing<'a> {
    Both {
        kind: LayerKind,
        old: &'a Layer,
        new: &'a Layer,
    },
    OnlyOld(&'a Layer),
    OnlyNew(&'a Layer),
}

/// Pair layers by **`LayerKind`** (rename-tolerant), in stable stack order. etchy
/// diffs same-board revisions, but real fab packs embed the board name in every
/// filename, so the *same* layer usually has different names across two revisions
/// (`revA-F_Cu.gbr` vs `revB-F_Cu.gbr`) — pairing by name would spuriously report
/// every layer as removed + added. Within a kind that has several layers (e.g.
/// mechanical "other" layers), exact-filename matches pair first and the rest pair
/// positionally by stack order — but a positional guess is only accepted when the
/// two layers' extents plausibly overlap (see [`positional_pair_score`]), so
/// two unrelated same-kind files are never silently collapsed onto one "changed"
/// layer (#238). A layer with no counterpart is `OnlyOld`/`OnlyNew` (a legitimately
/// added/removed layer, not an error).
pub fn pair_layers<'a>(old: &'a Board, new: &'a Board) -> Vec<LayerPairing<'a>> {
    // Distinct kinds present in either revision, ordered by stack position.
    let mut kinds: Vec<LayerKind> = Vec::new();
    for l in old.layers.iter().chain(new.layers.iter()) {
        if !kinds.contains(&l.kind) {
            kinds.push(l.kind);
        }
    }
    kinds.sort_by_key(|k| k.sort_key());

    let mut out = Vec::new();
    for kind in kinds {
        let olds: Vec<&Layer> = old.layers.iter().filter(|l| l.kind == kind).collect();
        let news: Vec<&Layer> = new.layers.iter().filter(|l| l.kind == kind).collect();
        let mut new_used = vec![false; news.len()];

        // Pass 1: exact-filename matches within the kind — keeps distinct same-kind
        // layers apart, and is simply a no-op when the names differ across revisions.
        let mut unmatched_old: Vec<&Layer> = Vec::new();
        for o in olds.iter().copied() {
            match news
                .iter()
                .enumerate()
                .find(|&(j, n)| !new_used[j] && n.label == o.label)
                .map(|(j, _)| j)
            {
                Some(j) => {
                    new_used[j] = true;
                    out.push(LayerPairing::Both {
                        kind,
                        old: o,
                        new: news[j],
                    });
                }
                None => unmatched_old.push(o),
            }
        }

        // Pass 2: pair the remainder positionally (rename-tolerant); report any
        // leftover on either side as an added/removed layer. A positional pair is
        // only a *guess* (it matched by stack order, not by name), so it must pass
        // a generous plausibility gate — two same-kind files that sit in disjoint
        // regions of the board can't be one physical layer's two revisions, so we
        // report them as an honest removed + added rather than one misleading
        // "changed" layer (#238).
        //
        // The two sides may list same-kind layers in a different order, so pairing
        // strictly by encounter position would fail the gate on every pair even
        // when a cross pairing passes (#318). Instead, score every plausible
        // (old, new) combination in the bucket and take the best-overlap pair
        // first, then the next best among what is left, and so on. Ties break by
        // index so the result is deterministic. Buckets are small (a handful of
        // mechanical layers at most), so the quadratic scan is negligible.
        let leftover_new: Vec<&Layer> = news
            .iter()
            .enumerate()
            .filter(|&(j, _)| !new_used[j])
            .map(|(_, n)| *n)
            .collect();
        let mut scored: Vec<(u32, usize, usize)> = Vec::new();
        for (i, o) in unmatched_old.iter().enumerate() {
            for (j, n) in leftover_new.iter().enumerate() {
                if let Some(score) = positional_pair_score(o, n) {
                    scored.push((score, i, j));
                }
            }
        }
        // Highest score first; equal scores fall back to old index, then new index.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut old_partner: Vec<Option<usize>> = vec![None; unmatched_old.len()];
        let mut new_taken = vec![false; leftover_new.len()];
        for (_, i, j) in scored {
            if old_partner[i].is_none() && !new_taken[j] {
                old_partner[i] = Some(j);
                new_taken[j] = true;
            }
        }
        // Emit in old-side order (a pair sits where its old layer does), then any
        // new layer nothing claimed.
        for (o, partner) in unmatched_old.iter().zip(old_partner) {
            match partner {
                Some(j) => out.push(LayerPairing::Both {
                    kind,
                    old: o,
                    new: leftover_new[j],
                }),
                None => out.push(LayerPairing::OnlyOld(o)),
            }
        }
        for (n, taken) in leftover_new.iter().zip(new_taken) {
            if !taken {
                out.push(LayerPairing::OnlyNew(n));
            }
        }
    }
    out
}

/// Generous plausibility gate for a **positional** (rename-tolerant) pairing:
/// two same-kind layers that matched only by stack order — not by filename —
/// must still plausibly be the same physical layer on the same board. Same-board
/// revisions keep a layer in roughly the same place, so a genuine pair's filled
/// extents overlap heavily; two unrelated files sharing a kind bucket (several
/// mechanical/`Other` layers, or split drill files per #237) tend to sit in
/// disjoint regions. When the extents barely overlap we refuse the pair, so the
/// diff reports an honest removed + added instead of one misleading "changed"
/// layer (#238).
///
/// Returns `None` when the pair is implausible, otherwise `Some(score)` where the
/// score is the overlap as a fraction of the smaller layer's extent, in parts per
/// million — higher means a better match. [`pair_layers`] uses the score to pick
/// the best-overlap pairs within a kind bucket (#318); the accept/refuse line is
/// unchanged from the original gate.
///
/// This gates *only* positional guesses — exact-filename matches are intentional
/// and never checked. Empty or degenerate geometry can't be judged geometrically,
/// so it passes with the lowest score (an empty diff is harmless and never a
/// silent miss, but a real geometric match should win over it).
fn positional_pair_score(old: &Layer, new: &Layer) -> Option<u32> {
    let (Some(o), Some(n)) = (old.geometry.bbox_nm(), new.geometry.bbox_nm()) else {
        return Some(0); // one side has no geometry — nothing to compare on
    };
    // Intersection of the two axis-aligned bounding boxes.
    let ix0 = o[0].max(n[0]);
    let iy0 = o[1].max(n[1]);
    let ix1 = o[2].min(n[2]);
    let iy1 = o[3].min(n[3]);
    if ix1 <= ix0 || iy1 <= iy0 {
        return None; // disjoint (or edge-touching) extents — not one layer
    }
    let inter = (ix1 - ix0) as i128 * (iy1 - iy0) as i128;
    let area = |bb: [i64; 4]| (bb[2] - bb[0]) as i128 * (bb[3] - bb[1]) as i128;
    let smaller = area(o).min(area(n));
    if smaller <= 0 {
        return Some(0); // a zero-width bbox can't be judged by overlap
    }
    // Generous: the overlap need only reach 10% of the smaller layer's extent.
    // Tuned to catch clearly-unrelated (near-disjoint) files while never splitting
    // a heavily-edited but co-located revision of the same layer.
    if inter * 10 < smaller {
        return None;
    }
    // `inter <= smaller`, so the ppm fraction is at most 1_000_000.
    Some((inter * 1_000_000 / smaller) as u32)
}

/// Coarse same-board plausibility check: the two revisions' whole-board union
/// extents must agree within a generous tolerance. This is **not** registration —
/// it exists to fail loud when handed two genuinely different boards, while
/// tolerating legitimate revision changes (a moved/added feature). Compares the
/// union of *all* layers (a per-layer bbox would trip on a layer that legitimately
/// grew). See `docs/M1_ENGINE_DESIGN.md`.
pub fn same_board_guard(old: &Board, new: &Board) -> Result<()> {
    // Physical board layers only. Documentation (drill drawings with legend
    // tables) and placement markers are annotations whose extents legitimately
    // change wildly between revisions — a regenerated drawing template must not
    // read as "different board" (validated against a real Altium pack).
    let physical = |b: &Board| {
        b.layers
            .iter()
            .filter(|l| !matches!(l.kind, LayerKind::Documentation | LayerKind::Placement))
            .filter_map(|l| l.geometry.bbox_nm())
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
    };
    let (ob, nb) = match (physical(old), physical(new)) {
        (Some(o), Some(n)) => (o, n),
        // One or both sides have no physical geometry — nothing to compare on.
        _ => return Ok(()),
    };

    let span = |bb: [i64; 4]| (bb[2] - bb[0]).max(bb[3] - bb[1]).max(0);
    let max_span = span(ob).max(span(nb));
    // Catches grossly different boards (wrong files) while tolerating legitimate
    // revision changes (a moved edge, an added tab/fiducial): 1 mm or 2% of the
    // larger span, whichever is bigger — matching the M1_ENGINE_DESIGN.md spec.
    // Kept tight on purpose (was 2 mm / 10%, #92): a "no silent misses" guard must
    // err toward failing loud on a possibly-wrong pair, not passing it and emitting
    // a garbage diff. `--force` bypasses it for the rare legit outlier.
    let tol = NM_PER_MM.max((max_span as f64 * 0.02) as i64);

    for i in 0..4 {
        if (ob[i] - nb[i]).abs() > tol {
            return Err(EngineError::BoardMismatch {
                detail: format!(
                    "union extents differ beyond {tol} nm tolerance: old {ob:?} vs new {nb:?} (nm)"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::{PolygonSet, Pt};

    fn layer(kind: LayerKind, sq: [i64; 4]) -> Layer {
        let [x0, y0, x1, y1] = sq;
        Layer {
            kind,
            label: format!("{kind:?}"),
            geometry: Arc::new(PolygonSet::new(vec![vec![vec![
                Pt::new(x0, y0),
                Pt::new(x1, y0),
                Pt::new(x1, y1),
                Pt::new(x0, y1),
            ]]])),
            negative: false,
        }
    }

    #[test]
    fn guard_passes_same_extent() {
        let a = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_000_000, 100_000_000],
            )],
        };
        let b = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_000_000, 100_000_000],
            )],
        };
        assert!(same_board_guard(&a, &b).is_ok());
    }

    #[test]
    fn guard_fails_different_board() {
        let a = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_000_000, 100_000_000],
            )],
        };
        let b = Board {
            layers: vec![layer(LayerKind::TopCopper, [0, 0, 10_000_000, 10_000_000])],
        };
        assert!(matches!(
            same_board_guard(&a, &b),
            Err(EngineError::BoardMismatch { .. })
        ));
    }

    #[test]
    fn guard_rejects_a_five_percent_extent_difference() {
        // Trust bar (#92): the guard must fail loud on boards that differ by more
        // than ~2% of span — a 5%-of-span extent difference is not "the same board
        // with a moved feature", it's likely the wrong file pair. (Under the old
        // 10% tolerance this silently passed and produced a garbage diff.)
        let a = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_000_000, 100_000_000],
            )],
        };
        // max_x grown by 5 mm on a 100 mm board = 5% of span.
        let b = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 105_000_000, 100_000_000],
            )],
        };
        assert!(matches!(
            same_board_guard(&a, &b),
            Err(EngineError::BoardMismatch { .. })
        ));
    }

    #[test]
    fn guard_ignores_documentation_layer_extents() {
        // Validated against a real Altium pack: drill DRAWINGS (Documentation)
        // grew a legend table beside the board in one revision — same board,
        // wildly different documentation extents. The guard must compare
        // physical board layers only, or it false-positives on every pack whose
        // drawing template changed.
        let mm = 1_000_000;
        let a = Board {
            layers: vec![
                layer(LayerKind::TopCopper, [0, 0, 34 * mm, 34 * mm]),
                layer(LayerKind::Documentation, [0, 0, 35 * mm, 35 * mm]),
            ],
        };
        let b = Board {
            layers: vec![
                layer(LayerKind::TopCopper, [0, 0, 34 * mm, 34 * mm]),
                // The regenerated drawing: legend table way off-board.
                layer(
                    LayerKind::Documentation,
                    [-10 * mm, -10 * mm, 130 * mm, 60 * mm],
                ),
            ],
        };
        assert!(
            same_board_guard(&a, &b).is_ok(),
            "same board; only the drawing template changed"
        );
        // …but mismatched COPPER still fails loud.
        let c = Board {
            layers: vec![layer(LayerKind::TopCopper, [0, 0, 90 * mm, 34 * mm])],
        };
        assert!(matches!(
            same_board_guard(&a, &c),
            Err(EngineError::BoardMismatch { .. })
        ));
    }

    #[test]
    fn guard_tolerates_a_small_revision_change() {
        // A legitimate revision that nudges an edge by well under the tolerance
        // (0.5 mm here) must still pass — the guard is a wrong-board catch, not
        // registration.
        let a = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_000_000, 100_000_000],
            )],
        };
        let b = Board {
            layers: vec![layer(
                LayerKind::TopCopper,
                [0, 0, 100_500_000, 100_000_000],
            )],
        };
        assert!(same_board_guard(&a, &b).is_ok());
    }

    #[test]
    fn same_kind_layers_pair_by_label() {
        // Two layers of the same kind (e.g. several mechanical layers) must each
        // pair by filename, never collapse onto one (which would drop a layer).
        let mk = |label: &str| Layer {
            kind: LayerKind::Other,
            label: label.into(),
            geometry: Arc::new(PolygonSet::new(vec![vec![vec![
                Pt::new(0, 0),
                Pt::new(1, 0),
                Pt::new(1, 1),
                Pt::new(0, 1),
            ]]])),
            negative: false,
        };
        let a = Board {
            layers: vec![mk("M.GM1"), mk("M.GM2")],
        };
        let b = Board {
            layers: vec![mk("M.GM1"), mk("M.GM2")],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|p| matches!(p, LayerPairing::Both { .. })));
    }

    #[test]
    fn pairs_by_kind_across_renamed_files() {
        // Real fab packs embed the board name, so the *same* layer has different
        // filenames between revisions (revA-F_Cu.gbr vs revB-F_Cu.gbr). It must
        // still pair by LayerKind — not report a spurious removed+added layer.
        let mk = |label: &str| Layer {
            kind: LayerKind::TopCopper,
            label: label.into(),
            geometry: Arc::new(PolygonSet::new(vec![vec![vec![
                Pt::new(0, 0),
                Pt::new(1, 0),
                Pt::new(1, 1),
                Pt::new(0, 1),
            ]]])),
            negative: false,
        };
        let a = Board {
            layers: vec![mk("revA-F_Cu.gbr")],
        };
        let b = Board {
            layers: vec![mk("revB-F_Cu.gbr")],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(
            pairs.len(),
            1,
            "same kind, different names → one paired layer"
        );
        assert!(matches!(pairs[0], LayerPairing::Both { .. }));
    }

    fn labeled(kind: LayerKind, label: &str, sq: [i64; 4]) -> Layer {
        let mut l = layer(kind, sq);
        l.label = label.into();
        l
    }

    #[test]
    fn positional_pairing_rejects_disjoint_layers() {
        // Two same-kind "other" layers whose filenames differ (so the exact-name
        // pass misses) and whose geometry sits in disjoint regions of the board.
        // Positional order alone would pair them into one misleading "changed"
        // layer; the guard must instead report an honest removed + added (#238).
        let mm = 1_000_000;
        let a = Board {
            layers: vec![labeled(
                LayerKind::Other,
                "rev1-notes.gbr",
                [0, 0, 40 * mm, 40 * mm],
            )],
        };
        let b = Board {
            layers: vec![labeled(
                LayerKind::Other,
                "rev2-keepout.gbr",
                [60 * mm, 60 * mm, 100 * mm, 100 * mm],
            )],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(
            pairs.len(),
            2,
            "disjoint unrelated same-kind layers must not collapse onto one"
        );
        assert!(
            pairs.iter().any(|p| matches!(p, LayerPairing::OnlyOld(_))),
            "the old-only layer must be reported as removed"
        );
        assert!(
            pairs.iter().any(|p| matches!(p, LayerPairing::OnlyNew(_))),
            "the new-only layer must be reported as added"
        );
    }

    #[test]
    fn positional_pairing_keeps_co_located_rename() {
        // Guard against false splits: a genuinely-revised same-kind layer keeps
        // its filename different across revisions but stays in roughly the same
        // place. Overlapping extents must still pair (rename tolerance), even when
        // the geometry changed a lot.
        let mm = 1_000_000;
        let a = Board {
            layers: vec![labeled(
                LayerKind::Other,
                "rev1-mech.gbr",
                [0, 0, 50 * mm, 50 * mm],
            )],
        };
        let b = Board {
            layers: vec![labeled(
                LayerKind::Other,
                "rev2-mech.gbr",
                [2 * mm, 2 * mm, 48 * mm, 55 * mm],
            )],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(pairs.len(), 1, "co-located rename must still pair");
        assert!(matches!(pairs[0], LayerPairing::Both { .. }));
    }

    /// The `(old, new)` labels of every `Both` pairing, in emitted order.
    fn both_labels<'a>(pairs: &[LayerPairing<'a>]) -> Vec<(&'a str, &'a str)> {
        pairs
            .iter()
            .filter_map(|p| match p {
                LayerPairing::Both { old, new, .. } => {
                    Some((old.label.as_str(), new.label.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn positional_pairing_matches_best_overlap_across_order() {
        // #318: two renamed same-kind layers listed in opposite order across the
        // revisions. Pairing strictly by encounter position (notes↔keepout',
        // keepout↔notes') fails the plausibility gate on both pairs and reports
        // four one-sided layers, even though the cross pairing passes. The bucket
        // must be assigned by best overlap instead: two changed pairs.
        let mm = 1_000_000;
        let a = Board {
            layers: vec![
                labeled(LayerKind::Other, "rev1-notes.gbr", [0, 0, 40 * mm, 40 * mm]),
                labeled(
                    LayerKind::Other,
                    "rev1-keepout.gbr",
                    [60 * mm, 60 * mm, 100 * mm, 100 * mm],
                ),
            ],
        };
        let b = Board {
            layers: vec![
                labeled(
                    LayerKind::Other,
                    "rev2-keepout.gbr",
                    [61 * mm, 60 * mm, 100 * mm, 100 * mm],
                ),
                labeled(LayerKind::Other, "rev2-notes.gbr", [0, 0, 40 * mm, 41 * mm]),
            ],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(
            pairs.len(),
            2,
            "two renamed layers must give two changed pairs"
        );
        assert_eq!(
            both_labels(&pairs),
            vec![
                ("rev1-notes.gbr", "rev2-notes.gbr"),
                ("rev1-keepout.gbr", "rev2-keepout.gbr"),
            ],
            "each layer pairs with its co-located counterpart, emitted in old order"
        );
    }

    #[test]
    fn positional_pairing_same_order_is_unchanged() {
        // The same two layers listed in the same order on both sides pair exactly
        // as they did before #318: like-with-like, in stack order.
        let mm = 1_000_000;
        let a = Board {
            layers: vec![
                labeled(LayerKind::Other, "rev1-notes.gbr", [0, 0, 40 * mm, 40 * mm]),
                labeled(
                    LayerKind::Other,
                    "rev1-keepout.gbr",
                    [60 * mm, 60 * mm, 100 * mm, 100 * mm],
                ),
            ],
        };
        let b = Board {
            layers: vec![
                labeled(LayerKind::Other, "rev2-notes.gbr", [0, 0, 40 * mm, 41 * mm]),
                labeled(
                    LayerKind::Other,
                    "rev2-keepout.gbr",
                    [61 * mm, 60 * mm, 100 * mm, 100 * mm],
                ),
            ],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            both_labels(&pairs),
            vec![
                ("rev1-notes.gbr", "rev2-notes.gbr"),
                ("rev1-keepout.gbr", "rev2-keepout.gbr"),
            ]
        );
    }

    #[test]
    fn positional_pairing_best_overlap_leaves_unrelated_layer_one_sided() {
        // Best-overlap assignment must not loosen the gate: with an extra new
        // layer that overlaps nothing on the old side, the co-located pair still
        // pairs and the stranger is still reported as added.
        let mm = 1_000_000;
        let a = Board {
            layers: vec![labeled(
                LayerKind::Other,
                "rev1-notes.gbr",
                [0, 0, 40 * mm, 40 * mm],
            )],
        };
        let b = Board {
            layers: vec![
                labeled(
                    LayerKind::Other,
                    "rev2-keepout.gbr",
                    [60 * mm, 60 * mm, 100 * mm, 100 * mm],
                ),
                labeled(LayerKind::Other, "rev2-notes.gbr", [0, 0, 40 * mm, 41 * mm]),
            ],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            both_labels(&pairs),
            vec![("rev1-notes.gbr", "rev2-notes.gbr")]
        );
        assert!(pairs
            .iter()
            .any(|p| matches!(p, LayerPairing::OnlyNew(l) if l.label == "rev2-keepout.gbr")));
    }

    #[test]
    fn pth_and_npth_drills_never_cross_pair() {
        // #237: a plated (PTH) and a non-plated (NPTH) drill file, with the
        // board/rev name embedded (so exact-name pairing can't help) and listed
        // in opposite encounter order across the two revisions. They must pair
        // like-with-like — PTH↔PTH, NPTH↔NPTH — never a plated↔non-plated pair
        // reporting a large meaningless "changed" drill layer.
        use crate::naming::classify;
        let mk = |name: &str| {
            let (stem, ext) = name.rsplit_once('.').unwrap();
            Layer {
                kind: classify(stem, ext),
                label: name.into(),
                geometry: Arc::new(PolygonSet::new(vec![vec![vec![
                    Pt::new(0, 0),
                    Pt::new(1_000_000, 0),
                    Pt::new(1_000_000, 1_000_000),
                    Pt::new(0, 1_000_000),
                ]]])),
                negative: false,
            }
        };
        let old = Board {
            layers: vec![mk("revA-NPTH.drl"), mk("revA-PTH.drl")],
        };
        let new = Board {
            layers: vec![mk("revB-PTH.drl"), mk("revB-NPTH.drl")],
        };
        let pairs = pair_layers(&old, &new);
        let is_npth = |l: &str| l.contains("NPTH");
        let both: Vec<_> = pairs
            .iter()
            .filter_map(|p| match p {
                LayerPairing::Both { old, new, .. } => Some((old, new)),
                _ => None,
            })
            .collect();
        assert_eq!(both.len(), 2, "both drill files should pair, none dropped");
        for (o, n) in both {
            assert_eq!(
                is_npth(&o.label),
                is_npth(&n.label),
                "plating must match across the pair: {} vs {}",
                o.label,
                n.label
            );
        }
    }

    #[test]
    fn pairs_report_unmatched_layers() {
        let a = Board {
            layers: vec![
                layer(LayerKind::TopCopper, [0, 0, 1, 1]),
                layer(LayerKind::Drill(DrillKind::Unspecified), [0, 0, 1, 1]),
            ],
        };
        let b = Board {
            layers: vec![layer(LayerKind::TopCopper, [0, 0, 1, 1])],
        };
        let pairs = pair_layers(&a, &b);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().any(|p| matches!(p, LayerPairing::OnlyOld(_))));
    }
}
