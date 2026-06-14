//! Layer + board model.

use std::path::PathBuf;

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

/// One resolved layer: its kind and where it came from. The filled polygon set
/// is attached during polygonization (Milestone 1).
#[derive(Debug, Clone)]
pub struct Layer {
    pub kind: LayerKind,
    pub source: PathBuf,
}
