//! The typed error surface for the engine. `etchy-core` is **pure**: errors carry
//! no `PathBuf` and the library never does I/O — the CLI wraps these with context
//! at the boundary and maps any `Err` to exit code 2.
//!
//! Trust bar: every feature the engine cannot render faithfully becomes a loud,
//! typed [`EngineError`] — never a silently dropped piece of geometry.

/// Geometry / quantization errors, surfaced when mapping real coordinates onto
/// the fixed-point grid.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum GeoError {
    /// A coordinate was NaN or infinite.
    #[error("non-finite coordinate from input")]
    NonFiniteCoord,
    /// A coordinate is outside the representable ±i64-nanometre grid.
    #[error("coordinate {mm} mm is outside the representable grid")]
    CoordOutOfRange { mm: f64 },
}

/// Everything the engine can fail on. Constructed across `gerber`, `polygonize`,
/// `model`, and `lib`.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum EngineError {
    /// `gerber_parser::parse` returned its hard-error arm.
    #[error("gerber parse failed: {0}")]
    Parse(String),

    /// `GerberDoc` carried per-command parse errors (we fail loud rather than
    /// proceed with partial geometry — see `docs/SPIKE_2.md`).
    #[error("{count} per-command parse error(s); first: {first}")]
    CommandErrors { count: usize, first: String },

    /// No `%MO` (and no resolvable deprecated `G70`/`G71`): we refuse to assume mm.
    #[error("units unresolved (no %MO / G70 / G71) — refusing to assume a unit")]
    UnitsUnresolved,

    /// A geometry feature this build does not yet render. **Fail loud, never
    /// silently dropped.** The next increment widens what is supported.
    #[error("unsupported geometry (not silently dropped): {feature}")]
    Unsupported { feature: String },

    /// A malformed aperture (non-positive dimension) that would otherwise produce
    /// phantom or mis-wound geometry — rejected loud rather than diffed wrong.
    #[error("invalid aperture: {detail}")]
    InvalidAperture { detail: String },

    /// One revision of a layer declares `%TF.FilePolarity,Negative` and the other
    /// does not: their images are complements, so a diff between them would
    /// report the whole layer changed with the signs inverted (#317).
    #[error(
        "layer polarity differs: {label_old} vs {label_new} — one is a negative image \
         (%TF.FilePolarity,Negative); re-export both revisions with the same polarity"
    )]
    PolarityMismatch {
        label_old: String,
        label_new: String,
    },

    /// A flash/draw referenced an aperture code that was never defined.
    #[error("operation references undefined aperture D{code}")]
    UndefinedAperture { code: i32 },

    /// A flash/draw occurred before any aperture was selected.
    #[error("operation with no aperture selected")]
    NoApertureSelected,

    /// Quantization / geometry error.
    #[error(transparent)]
    Geometry(#[from] GeoError),

    /// The two revisions do not share the same set of layer kinds is allowed
    /// (added/removed layers are reported), but a genuinely empty comparison is not.
    #[error("no layers to compare")]
    NoLayers,

    /// The same-board guard: the two inputs are not plausibly the same board.
    /// etchy diffs same-board revisions only and will not auto-align.
    #[error("not the same board (etchy will not auto-align): {detail}")]
    BoardMismatch { detail: String },

    /// Two raster pages (e.g. schematic-PDF pages) that must line up pixel-for-pixel
    /// differ in size. etchy diffs same-document revisions and will not rescale or
    /// realign — a size change means the export changed, so fail loud (mirrors the
    /// same-board guard for geometry).
    #[error("page sizes differ ({ow}x{oh} vs {nw}x{nh}) — etchy will not rescale raster pages")]
    ImageSizeMismatch { ow: u32, oh: u32, nw: u32, nh: u32 },

    /// A raster buffer's length doesn't match its declared width×height×4 (RGBA).
    #[error("malformed image: {len} bytes for {w}x{h} RGBA (expected {expected})")]
    MalformedImage {
        w: u32,
        h: u32,
        len: usize,
        expected: usize,
    },

    /// Too many pages to align. The page-alignment DP matrix is quadratic in the
    /// page count, so a pair of huge documents (small files, under every other
    /// cap) would ask for gigabytes and abort the process. Fail loud *before*
    /// allocating, mirroring the per-file and per-page ceilings (#249).
    #[error(
        "too many pages to align: old {old} page(s), new {new} \
         — etchy aligns at most {limit} pages per side"
    )]
    TooManyPages {
        old: usize,
        new: usize,
        limit: usize,
    },

    /// A single layer emitted more objects than the engine will process. Guards
    /// against a tiny file amplifying (many arcs / region loops / polarity spans)
    /// into millions of contours — a CPU/RAM exhaustion DoS. Fail loud rather
    /// than grind or OOM (#83).
    #[error("layer object limit exceeded: {count} {what} (limit {limit})")]
    ObjectLimit {
        what: &'static str,
        count: usize,
        limit: usize,
    },
}

/// Convenience result alias for the engine.
pub type Result<T> = std::result::Result<T, EngineError>;
