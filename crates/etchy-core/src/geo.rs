//! Geometry primitives shared by every front-end (Gerber, Excellon).
//!
//! Coordinates are fixed-point integers on a defined grid (default 1 nm) so
//! diffs are deterministic and reproducible — see DEVELOPER_GUIDE "Coordinate
//! model". Keeping the grid constant here means every pipeline stage agrees.

/// Nanometres per fixed-point unit — the grid Gerber/Excellon floats snap to.
pub const GRID_NM: i64 = 1;

/// A point in fixed-point nanometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pt {
    pub x: i64,
    pub y: i64,
}

impl Pt {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

/// Stroke / flash shape. Minimal Phase-0 set; macros + more shapes arrive in M1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aperture {
    Circle { diameter_nm: i64 },
    Rect { w_nm: i64, h_nm: i64 },
}

/// A drawn graphic object. The Gerber and Excellon front-ends emit the SAME
/// `Primitive`s (drill hits → `Flash`, routed slots → `Line`/`Arc`), so the
/// diff engine never needs to know which format produced a layer.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Line {
        from: Pt,
        to: Pt,
        aperture: Aperture,
    },
    Arc {
        from: Pt,
        to: Pt,
        center: Pt,
        cw: bool,
        aperture: Aperture,
    },
    Flash {
        at: Pt,
        aperture: Aperture,
    },
    Region {
        outline: Vec<Pt>,
        holes: Vec<Vec<Pt>>,
    },
}
