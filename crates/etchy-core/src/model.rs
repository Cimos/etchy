//! Layer + board model, layer pairing, and the same-board guard.
//!
//! Purity: a [`Layer`] carries an opaque `label` string (for diagnostics/report),
//! **not** a `PathBuf`. The CLI classifies filenames → [`LayerKind`] (naming
//! policy) and hands the engine an already-tagged [`Board`]; the engine never
//! touches the filesystem or interprets a path.

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
    pub geometry: PolygonSet,
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

/// How a layer kind pairs across the two revisions.
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

/// Fail loud if a revision contains two layers of the same kind. Pairing is one
/// layer per kind, so a duplicate would be **silently dropped** (a missed change);
/// we refuse it instead. `which` names the revision for the error message.
pub fn ensure_unique_kinds(board: &Board, which: &str) -> Result<()> {
    for (i, a) in board.layers.iter().enumerate() {
        for b in &board.layers[i + 1..] {
            if a.kind == b.kind {
                return Err(EngineError::DuplicateLayerKind {
                    which: which.to_string(),
                    kind: format!("{:?}", a.kind),
                    first: a.label.clone(),
                    second: b.label.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Pair layers by kind (rename-tolerant — pairing is by *kind*, not filename),
/// in a stable stack order. Layers present in only one revision are reported as
/// `OnlyOld`/`OnlyNew` (an added/removed layer is a legitimate revision change,
/// not an error).
pub fn pair_layers<'a>(old: &'a Board, new: &'a Board) -> Vec<LayerPairing<'a>> {
    let find = |b: &'a Board, k: LayerKind| b.layers.iter().find(|l| l.kind == k);

    // Union of kinds present in either revision.
    let mut kinds: Vec<LayerKind> = Vec::new();
    for l in old.layers.iter().chain(new.layers.iter()) {
        if !kinds.contains(&l.kind) {
            kinds.push(l.kind);
        }
    }
    kinds.sort_by_key(|k| k.sort_key());

    kinds
        .into_iter()
        .map(|kind| match (find(old, kind), find(new, kind)) {
            (Some(o), Some(n)) => LayerPairing::Both {
                kind,
                old: o,
                new: n,
            },
            (Some(o), None) => LayerPairing::OnlyOld(o),
            (None, Some(n)) => LayerPairing::OnlyNew(n),
            (None, None) => unreachable!("kind came from one of the boards"),
        })
        .collect()
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
    // Generous: 1 mm or 2% of the larger span, whichever is bigger.
    let tol = (NM_PER_MM).max((max_span as f64 * 0.02) as i64);

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
            geometry: PolygonSet::new(vec![vec![vec![
                Pt::new(x0, y0),
                Pt::new(x1, y0),
                Pt::new(x1, y1),
                Pt::new(x0, y1),
            ]]]),
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
    fn duplicate_kind_is_rejected() {
        // Two layers of the same kind must fail loud (pairing would drop one).
        let b = Board {
            layers: vec![
                layer(LayerKind::Other, [0, 0, 1, 1]),
                layer(LayerKind::Other, [0, 0, 1, 1]),
            ],
        };
        assert!(matches!(
            ensure_unique_kinds(&b, "old"),
            Err(EngineError::DuplicateLayerKind { .. })
        ));
        // A board with distinct kinds passes.
        let ok = Board {
            layers: vec![
                layer(LayerKind::TopCopper, [0, 0, 1, 1]),
                layer(LayerKind::Other, [0, 0, 1, 1]),
            ],
        };
        assert!(ensure_unique_kinds(&ok, "old").is_ok());
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
