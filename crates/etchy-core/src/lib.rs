//! etchy-core — the format-agnostic PCB diff engine.
//!
//! Pipeline (see `docs/DEVELOPER_GUIDE.md`):
//! `parse → resolve graphics state → polygonize → boolean diff → measure → render`.
//!
//! Phase-0 scaffold: the module skeleton and the core IR types are in place;
//! the algorithms (Gerber/Excellon front-ends, polygonization, `i_overlay`
//! boolean diff, clustering, renderers) land in Milestone 1.

pub mod diff;
pub mod geo;
pub mod model;
pub mod report;

/// The crate version, from Cargo.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::version().is_empty());
    }
}
