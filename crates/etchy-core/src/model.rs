//! Layer + board model, layer pairing, and the same-board guard.
//!
//! Purity: a [`Layer`] carries an opaque `label` string (for diagnostics/report),
//! **not** a `PathBuf`. The CLI classifies filenames → [`LayerKind`] (naming
//! policy) and hands the engine an already-tagged [`Board`]; the engine never
//! touches the filesystem or interprets a path.

use std::sync::Arc;

use crate::error::{EngineError, Result};
use crate::geo::{PolygonSet, NM_PER_MM};

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
    Drill,
    Outline,
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
            LayerKind::Drill => "drill",
            LayerKind::Outline => "outline",
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
            LayerKind::Drill => (9, 0),
            LayerKind::Outline => (10, 0),
            LayerKind::Other => (11, 0),
        }
    }
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
/// positionally by stack order, so distinct same-kind layers are never collapsed
/// onto one. A layer with no counterpart is `OnlyOld`/`OnlyNew` (a legitimately
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
        // leftover on either side as an added/removed layer.
        let mut leftover_new = news
            .iter()
            .enumerate()
            .filter(|&(j, _)| !new_used[j])
            .map(|(_, n)| *n);
        let mut unmatched_old = unmatched_old.into_iter();
        loop {
            match (unmatched_old.next(), leftover_new.next()) {
                (Some(o), Some(n)) => out.push(LayerPairing::Both {
                    kind,
                    old: o,
                    new: n,
                }),
                (Some(o), None) => out.push(LayerPairing::OnlyOld(o)),
                (None, Some(n)) => out.push(LayerPairing::OnlyNew(n)),
                (None, None) => break,
            }
        }
    }
    out
}

/// Coarse same-board plausibility check: the two revisions' whole-board union
/// extents must agree within a generous tolerance. This is **not** registration —
/// it exists to fail loud when handed two genuinely different boards, while
/// tolerating legitimate revision changes (a moved/added feature). Compares the
/// union of *all* layers (a per-layer bbox would trip on a layer that legitimately
/// grew). See `docs/M1_ENGINE_DESIGN.md`.
pub fn same_board_guard(old: &Board, new: &Board) -> Result<()> {
    let (ob, nb) = match (old.bbox_nm(), new.bbox_nm()) {
        (Some(o), Some(n)) => (o, n),
        // One or both sides have no geometry at all — nothing to compare extents on.
        _ => return Ok(()),
    };

    let span = |bb: [i64; 4]| (bb[2] - bb[0]).max(bb[3] - bb[1]).max(0);
    let max_span = span(ob).max(span(nb));
    // Generous on purpose: this catches *grossly* different boards (wrong files),
    // not legitimate revision changes (a moved edge, an added tab/fiducial). 2 mm
    // or 10% of the larger span, whichever is bigger. `--force` bypasses it.
    let tol = (2 * NM_PER_MM).max((max_span as f64 * 0.10) as i64);

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

    #[test]
    fn pairs_report_unmatched_layers() {
        let a = Board {
            layers: vec![
                layer(LayerKind::TopCopper, [0, 0, 1, 1]),
                layer(LayerKind::Drill, [0, 0, 1, 1]),
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
