//! etchy-gui — native desktop viewer (egui). Phase-0 stub.
//!
//! The single-window viewer (layer tree, overlay/before/after/split/swipe/onion
//! + heatmap, pan-zoom) lands in Milestone 1 — see docs/DEVELOPER_GUIDE.md
//! "GUI architecture". Kept a separate binary so headless/CI builds never
//! compile egui/winit/wgpu.

fn main() {
    eprintln!(
        "etchy-gui {} — native viewer not yet implemented (Phase-0 scaffold).",
        etchy_core::version()
    );
}
