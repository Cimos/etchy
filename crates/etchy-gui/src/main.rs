//! etchy-gui — native desktop viewer (egui).
//!
//! `etchy-gui <old> <new>` diffs two Gerber revision directories and shows the
//! result: a changed-first layer list and a pan/zoom canvas overlaying the diff
//! (added copper green, removed red, base muted). The geometry + diff come from
//! the pure `etchy-core` engine via `compare_detailed`; this crate only does I/O
//! and rendering. M1 scope: flash-only geometry (the engine fails loud otherwise).

#[cfg(not(target_arch = "wasm32"))]
mod loader;
mod lod;

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use etchy_core::{BoardDiff, LayerView, PolygonSet, Pt};

// ===========================================================================
// Native entry (desktop) — diff two Gerber directories given on the CLI.
// ===========================================================================
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    /// WSLg's GPU OpenGL path (ZINK/Vulkan-on-GL) commonly fails to initialize, and
    /// its Wayland socket can drop ("Broken pipe"). Software rendering (llvmpipe) over
    /// X11 (Xwayland) is the reliable combination, so on WSL we default to it.
    fn configure_display_for_wsl() {
        let is_wsl = std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .map(|s| {
                    let s = s.to_ascii_lowercase();
                    s.contains("microsoft") || s.contains("wsl")
                })
                .unwrap_or(false);
        if !is_wsl {
            return;
        }
        if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none() {
            std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        }
        if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }

    pub fn run() -> ExitCode {
        configure_display_for_wsl();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.len() != 2 {
            eprintln!("usage: etchy-gui <old-dir> <new-dir>");
            return ExitCode::from(2);
        }
        let (old_dir, new_dir) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
        let diff = match build_diff(&old_dir, &new_dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("etchy-gui: error: {e:#}");
                return ExitCode::from(2);
            }
        };
        let (old_lbl, new_lbl) = (label(&old_dir), label(&new_dir));
        // No window icon: the in-app "etchy" wordmark is the single logo on both
        // surfaces (#18). Setting a window icon here gave native a second logo.
        let viewport = egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_title("etchy — PCB diff viewer");
        let native_options = eframe::NativeOptions {
            viewport,
            // 4x MSAA so sub-pixel slivers (thin track/pad junctions, shared edges)
            // cover at least one sample and don't drop out as "no copper" (#55/#47).
            multisampling: 4,
            ..Default::default()
        };
        // Move the labels + diff into the creation closure so persisted settings
        // (#52) restore from `cc.storage` before the first frame.
        match eframe::run_native(
            "etchy",
            native_options,
            Box::new(move |cc| Ok(Box::new(ViewApp::from_cc(cc, diff, old_lbl, new_lbl)))),
        ) {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("etchy-gui: window error: {e}");
                ExitCode::from(2)
            }
        }
    }

    fn label(p: &Path) -> String {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string()
    }

    fn build_diff(old_dir: &Path, new_dir: &Path) -> anyhow::Result<BoardDiff> {
        let (old, of) = loader::load_board(old_dir)?;
        let (new, nf) = loader::load_board(new_dir)?;
        let mut d = etchy_core::compare_detailed(&old, &new)?;
        if let (Some(o), Some(n)) = (of, nf) {
            if let Some(w) = etchy_core::coordinate_mismatch_warning(&o, &n) {
                d.report.warnings.push(w);
            }
        }
        Ok(d)
    }
}

// ===========================================================================
// Entry points
// ===========================================================================
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    native::run()
}

/// The bundled demo board (gitignored, embedded at compile time). A layer that
/// fails to parse is skipped rather than crashing the demo.
#[cfg(target_arch = "wasm32")]
fn board_from_files(
    dir: &include_dir::Dir,
) -> (etchy_core::Board, Option<etchy_core::GerberFormat>) {
    let mut files: Vec<_> = dir.files().collect();
    files.sort_by_key(|f| f.path().to_path_buf());
    let mut layers = Vec::new();
    let mut fmt = None;
    for f in files {
        let bytes = f.contents();
        if !etchy_core::looks_like_gerber(bytes) {
            continue;
        }
        if fmt.is_none() {
            fmt = etchy_core::gerber_format(bytes).ok();
        }
        let name = f
            .path()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
        if let Ok(geometry) = etchy_core::polygonize_gerber(bytes) {
            layers.push(etchy_core::Layer {
                kind: etchy_core::classify(stem, ext),
                label: name.clone(),
                geometry,
            });
        }
    }
    (etchy_core::Board { layers }, fmt)
}

#[cfg(target_arch = "wasm32")]
fn demo_diff() -> BoardDiff {
    static OLD: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/demo/old");
    static NEW: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/demo/new");
    let (old, of) = board_from_files(&OLD);
    let (new, nf) = board_from_files(&NEW);
    let mut d = etchy_core::compare_detailed(&old, &new).expect("demo diff");
    if let (Some(o), Some(n)) = (of, nf) {
        if let Some(w) = etchy_core::coordinate_mismatch_warning(&o, &n) {
            d.report.warnings.push(w);
        }
    }
    d
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    console_error_panic_hook::set_once();
    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async move {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("the_canvas_id"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("canvas element #the_canvas_id");
        // The bundled demo board is the public Mad_RP2040 (v0.0.0 -> v0.0.1), from
        // its GitHub release fab packs — label it accordingly. The creation closure
        // restores persisted settings from localStorage via `cc.storage` (#52).
        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    Ok(Box::new(ViewApp::from_cc(
                        cc,
                        demo_diff(),
                        "Mad_RP2040 v0.0.0".into(),
                        "Mad_RP2040 v0.0.1".into(),
                    )))
                }),
            )
            .await
            .expect("failed to start eframe web runner");
    });
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Overlay,
    Before,
    After,
    /// Side-by-side: old board left, new board right, one shared camera (G4).
    Split,
    /// Curtain wipe: one draggable divider, old board left of it, new board right,
    /// one shared camera (#61). Like Split but the boundary is user-movable.
    Swipe,
}

/// How strongly to draw the unchanged base (the new layer) behind the diff (G3).
/// An always-available faint base keeps unchanged copper visible so green/red
/// changes read against it instead of floating in black (#8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BaseLevel {
    Off,
    Faint,
    Strong,
}

/// Input scheme matching the user's ECAD tool (#54). MVP: it only controls which
/// mouse button pans the canvas (the real differentiator between tools) — scroll
/// stays zoom-to-cursor for all three. A full per-key remapper is a follow-up.
/// Persisted via #52 with a stable serde string repr (like `Theme`/`BaseLevel`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum InputPreset {
    /// KiCad: middle OR right drag pans.
    KiCad,
    /// Altium: right drag pans. The default (alphabetically first of the supported
    /// ECAD tools) (#54/#55).
    #[default]
    Altium,
}

/// Whether `button` drags should pan the canvas under the given preset (#54). Pure
/// → unit-testable; the only per-preset difference in the MVP.
fn pans_on(preset: InputPreset, button: egui::PointerButton) -> bool {
    use egui::PointerButton::{Middle, Secondary};
    match preset {
        InputPreset::KiCad => button == Middle || button == Secondary,
        InputPreset::Altium => button == Secondary,
    }
}

/// What a scroll gesture should do to the camera (G7b #12). Pan amounts are in
/// screen px; Zoom is a multiplicative scale factor.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CameraAction {
    Zoom(f64),
    PanX(f64),
    PanY(f64),
    None,
}

/// Map a scroll delta + modifiers to a camera action: plain wheel zooms (cursor-
/// anchored by the caller), Ctrl pans vertically, Shift pans horizontally; Ctrl wins
/// if both held. Drives off the LARGER-magnitude axis (not "y else x") so a Ctrl/
/// Shift+wheel that lands on either axis still pans — the old guard discarded it when
/// it arrived on the zeroed axis (why Ctrl-scroll did nothing). Pure → unit-testable.
fn scroll_to_camera_action(
    dx: f32,
    dy: f32,
    ctrl: bool,
    shift: bool,
    zoom_rate: f64,
    pan_step: f64,
) -> CameraAction {
    let primary = if dy.abs() >= dx.abs() { dy } else { dx };
    if primary == 0.0 {
        CameraAction::None
    } else if ctrl {
        CameraAction::PanY(primary as f64 * pan_step)
    } else if shift {
        CameraAction::PanX(primary as f64 * pan_step)
    } else {
        CameraAction::Zoom((primary as f64 * zoom_rate).exp())
    }
}

/// Normalise a raw wheel delta to a common "points" scale so a physical notch zooms
/// the same on native (Line units, ~1/notch) and web (Point units, ~100/notch): Line
/// scales up to ≈ a web notch; Page is viewport-relative (#56 scroll-feel parity).
fn wheel_points(unit: egui::MouseWheelUnit, delta: egui::Vec2, viewport_h: f32) -> egui::Vec2 {
    match unit {
        egui::MouseWheelUnit::Point => delta,
        egui::MouseWheelUnit::Line => delta * 100.0,
        egui::MouseWheelUnit::Page => delta * viewport_h,
    }
}

/// Opaque display colour for the unchanged base: the layer colour blended toward
/// the canvas by level (Faint = dim, Strong = near-full). Opaque (not low-alpha)
/// so unchanged copper reads as dim copper, not near-black over the dark canvas.
fn base_display_color(layer: Color32, canvas: Color32, level: BaseLevel) -> Color32 {
    let t = match level {
        BaseLevel::Off => 0.0,
        BaseLevel::Faint => 0.4,
        BaseLevel::Strong => 0.8,
    };
    let mix = |a: u8, b: u8| (b as f32 + (a as f32 - b as f32) * t).round() as u8;
    Color32::from_rgb(
        mix(layer.r(), canvas.r()),
        mix(layer.g(), canvas.g()),
        mix(layer.b(), canvas.b()),
    )
}

/// Resolve an Esc press for the measure tool (#50). Esc cascades: while a measurement
/// is in progress it clears the points but keeps measure mode on; a second Esc (nothing
/// left to clear) exits measure mode. Returns `(next_measure_mode, clear_points)`.
fn measure_escape(measure_mode: bool, has_points: bool) -> (bool, bool) {
    if has_points {
        (measure_mode, true) // clear the in-progress measurement, stay in the tool
    } else {
        (false, false) // nothing to clear -> leave measure mode (no-op if already off)
    }
}

/// Cycle Off → Faint → Strong → Off (the `S` key / base selector).
fn cycle_base(level: BaseLevel) -> BaseLevel {
    match level {
        BaseLevel::Off => BaseLevel::Faint,
        BaseLevel::Faint => BaseLevel::Strong,
        BaseLevel::Strong => BaseLevel::Off,
    }
}

/// The inputs that change WHICH geometry is triangulated (G6 cache key). Colours,
/// the base alpha, the noise threshold, and the camera are deliberately NOT here —
/// they're applied cheaply at transform time, so they never force a re-triangulation.
/// Adding a new geometry-selecting input? It MUST join this key or the cache goes
/// stale and shows the wrong thing (a silent miss the project forbids).
#[derive(Clone, PartialEq, Debug)]
struct GeomKey {
    /// Indices (into `diff.layers`) of every VISIBLE layer (#58/#59). The cache
    /// triangulates each one, so the merged mesh must rebuild whenever this set
    /// changes. Highlight/dim and colours are applied per frame, so they're NOT here.
    visible: Vec<usize>,
    mode: Mode,
    base_on: bool,
    outline_effective: bool,
}

/// Build the cache key from the current view inputs. `visible` is the set of layer
/// indices to draw (one merged mesh over all of them).
fn build_geom_key(
    visible: &[usize],
    mode: Mode,
    base_level: BaseLevel,
    show_outline: bool,
    outline: Option<usize>,
) -> GeomKey {
    GeomKey {
        visible: visible.to_vec(),
        mode,
        base_on: base_level != BaseLevel::Off,
        // The outline reference draws whenever it's enabled and exists; with several
        // layers shown there's no single "selected" layer to suppress it for.
        outline_effective: show_outline && outline.is_some(),
    }
}

/// Set visibility for every index in `idxs` (a layer group) at once (#58). Indices
/// outside `visible` are ignored, so a stale group list can't panic.
fn set_group_visibility(visible: &mut [bool], idxs: &[usize], show: bool) {
    for &i in idxs {
        if let Some(v) = visible.get_mut(i) {
            *v = show;
        }
    }
}

/// Whether every layer in `idxs` is currently visible — drives a group's
/// show/hide-all toggle state (#58). An empty group reads as "all visible".
fn group_all_visible(visible: &[bool], idxs: &[usize]) -> bool {
    idxs.iter()
        .all(|&i| visible.get(i).copied().unwrap_or(false))
}

/// Visibility vector that shows only the changed layers ("Show changed", #58),
/// from a per-index changed-flag slice. The UI button was hidden per feedback #8,
/// but the capability is kept (and tested) so it can be re-surfaced later.
#[allow(dead_code)]
fn visible_from_changed(changed: &[bool]) -> Vec<bool> {
    changed.to_vec()
}

/// On-load visibility (#9/#10 perf): only the selected layer is shown; multiple
/// layers are opt-in via the per-row/per-group checkboxes. Rendering one layer by
/// default keeps the common case fast on dense boards (the old default showed every
/// changed layer at once). An out-of-range `selected` just yields nothing forced on.
fn default_visible(n: usize, selected: usize) -> Vec<bool> {
    let mut v = vec![false; n];
    if let Some(s) = v.get_mut(selected) {
        *s = true;
    }
    v
}

/// Rebuild a per-layer visibility vector of length `n` from saved visible indices
/// (#52). Out-of-range indices (the layer count shrank between sessions) are
/// dropped rather than panicking.
fn restore_visibility(saved: &[usize], n: usize) -> Vec<bool> {
    let mut vis = vec![false; n];
    for &i in saved {
        if let Some(v) = vis.get_mut(i) {
            *v = true;
        }
    }
    vis
}

/// The indices of the set bits in a visibility vector, in order (for the GeomKey
/// and for persistence).
fn visible_indices(visible: &[bool]) -> Vec<usize> {
    visible
        .iter()
        .enumerate()
        .filter_map(|(i, &v)| v.then_some(i))
        .collect()
}

/// Rebuild the tessellation cache when there's none yet or the key changed.
fn geom_cache_dirty(prev: Option<&GeomKey>, now: &GeomKey) -> bool {
    prev != Some(now)
}

/// On-screen px width of a region from its cached world extent (nm) and the scale.
fn region_screen_px(extent_nm: i64, scale: f64) -> f32 {
    (extent_nm as f64 * scale) as f32
}

/// A region's THICKNESS (nm) ≈ area / longest-dimension. Used for the LOD fade so a
/// long-but-sub-pixel-thin diff crescent (large extent, tiny thickness) fades like
/// the thin feature it is, instead of staying opaque and flickering when panned (#46).
fn feature_thickness_nm(area_nm2: f64, extent_nm: i64) -> i64 {
    if extent_nm <= 0 {
        0
    } else {
        (area_nm2 / extent_nm as f64) as i64
    }
}

/// Default base/context colour for a layer by type, so flipping layers reads by
/// colour (copper→copper-gold, silk→cream, mask→green, paste→grey…). Added/removed
/// stay green/red — these only colour the unchanged base. Per-review re-scope.
fn layer_type_color(kind: etchy_core::LayerKind, theme: Theme) -> Color32 {
    use etchy_core::LayerKind::*;
    // (dark, light) per family — light variants are darker so they read on a
    // cream board (e.g. silk's cream would vanish on a light canvas) (#57).
    let (dark, light) = match kind {
        // Per-layer copper defaults in the spirit of KiCad/Altium so flipping
        // layers reads by colour, not all one gold (#20). Top = F.Cu red,
        // bottom = B.Cu blue, inners cycle through distinct muted tones.
        TopCopper => (
            Color32::from_rgb(0xc8, 0x34, 0x34),
            Color32::from_rgb(0xa3, 0x2a, 0x2a),
        ),
        BottomCopper => (
            Color32::from_rgb(0x4d, 0x7f, 0xc4),
            Color32::from_rgb(0x36, 0x5c, 0x94),
        ),
        InnerCopper(n) => {
            let inner = [
                (0x4f_u8, 0x9c_u8, 0x4f_u8), // green
                (0x9c, 0x8a, 0x3a),          // olive
                (0x7a, 0x5c, 0xa8),          // violet
                (0x3a, 0x9c, 0x94),          // teal
                (0xb0, 0x6a, 0x3a),          // burnt orange
                (0x8a, 0x4f, 0x6a),          // mauve
            ];
            let (r, g, b) = inner[(n as usize).saturating_sub(1) % inner.len()];
            let darker = |v: u8| (v as f32 * 0.78) as u8;
            (
                Color32::from_rgb(r, g, b),
                Color32::from_rgb(darker(r), darker(g), darker(b)),
            )
        }
        TopSilk | BottomSilk => (C_CREAM, Color32::from_rgb(0x6b, 0x64, 0x56)),
        TopMask | BottomMask => (
            Color32::from_rgb(0x2e, 0x7d, 0x4f),
            Color32::from_rgb(0x1f, 0x5c, 0x3a),
        ),
        TopPaste | BottomPaste => (
            Color32::from_rgb(0xb4, 0xb4, 0xbe),
            Color32::from_rgb(0x6b, 0x6f, 0x78),
        ),
        Drill => (
            Color32::from_rgb(0x7a, 0x8a, 0xa0),
            Color32::from_rgb(0x4a, 0x55, 0x68),
        ),
        Outline => (C_COPPER, C_COPPER),
        Other => (C_BASE, Color32::from_rgb(0x6b, 0x72, 0x80)),
    };
    match theme {
        Theme::Dark => dark,
        Theme::Light => light,
    }
}

/// The base colour for the layer at `index` (with kind `kind`): a per-layer user
/// override if set, else the per-kind type default. Overrides are keyed by the
/// layer's index in `diff.layers` so two layers of the same kind colour apart (#21).
fn resolve_base_color(
    index: usize,
    kind: etchy_core::LayerKind,
    overrides: &[(usize, Color32)],
    theme: Theme,
) -> Color32 {
    overrides
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, c)| *c)
        .unwrap_or_else(|| layer_type_color(kind, theme))
}

/// Which viewport a cached item draws into (G4 split). Full = the whole canvas
/// (every non-split mode); Left/Right = the old/new halves of the split view.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Full,
    Left,
    Right,
}

/// Keep the split fraction within [0.1, 0.9] so neither half ever vanishes.
fn clamp_split_frac(frac: f32) -> f32 {
    frac.clamp(0.1, 0.9)
}

/// Partition `rect` into left/right sub-rects about a divider at `frac` (clamped),
/// separated by `gutter` px; returns `(left, right, divider_x)` (G4).
fn split_rects(rect: Rect, frac: f32, gutter: f32) -> (Rect, Rect, f32) {
    let div = rect.left() + rect.width() * clamp_split_frac(frac);
    let left = Rect::from_min_max(rect.min, Pos2::new(div - gutter * 0.5, rect.max.y));
    let right = Rect::from_min_max(Pos2::new(div + gutter * 0.5, rect.min.y), rect.max);
    (left, right, div)
}

/// Keep the swipe divider within [0.1, 0.9] so neither side ever vanishes (#61).
fn clamp_swipe_frac(frac: f32) -> f32 {
    frac.clamp(0.1, 0.9)
}

/// Partition `rect` into left/right clip-rects meeting at a single draggable
/// divider at `frac` (clamped); returns `(left, right, divider_x)` (#61). Unlike
/// [`split_rects`] there is no gutter — the sides touch so the wipe is seamless.
fn swipe_rects(rect: Rect, frac: f32) -> (Rect, Rect, f32) {
    let div = rect.left() + rect.width() * clamp_swipe_frac(frac);
    let left = Rect::from_min_max(rect.min, Pos2::new(div, rect.max.y));
    let right = Rect::from_min_max(Pos2::new(div, rect.min.y), rect.max);
    (left, right, div)
}

/// Pan/zoom camera in world (nm) space.
struct Camera {
    center: [f64; 2], // world nm
    scale: f64,       // pixels per nm
    fit_scale: f64,   // scale at last fit — the "100%" reference for zoom %
    fitted: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            scale: 1.0,
            fit_scale: 1.0,
            fitted: false,
        }
    }
}

struct ViewApp {
    diff: BoardDiff,
    old_label: String,
    new_label: String,
    order: Vec<usize>, // indices into diff.layers, changed-first
    selected: usize,   // active/highlighted layer; index into diff.layers
    /// Per-layer visibility, indexed the same as `diff.layers` (#58/#59). Several
    /// layers can be drawn at once; the `selected` one is highlighted and the rest
    /// are dimmed at draw time. Persisted via #52.
    visible_layers: Vec<bool>,
    mode: Mode,
    /// Always-available faint base behind the diff (G3): Off / Faint / Strong.
    base_level: BaseLevel,
    /// User-configurable diff colors (G3, Altium-compare style). Default to the
    /// brand green/red; a "Colors" popover edits them.
    col_added: Color32,
    col_removed: Color32,
    /// Canvas (board background) colour (#53), kept per-theme so a dark board tuned
    /// in dark mode never leaks into light mode (#31). Resolved via `canvas_color()`;
    /// editable in the Colors window (active theme) and persisted (#52).
    canvas_dark: Color32,
    canvas_light: Color32,
    /// Grid colour (#53), per-theme for the same reason as the canvas (#31).
    grid_dark: Color32,
    grid_light: Color32,
    /// Per-layer base/context colour overrides, keyed by the layer's index in
    /// `diff.layers` (default = layer_type_color for that kind) (#21).
    base_overrides: Vec<(usize, Color32)>,
    /// The Colors editor window is open. A real window (not a menu) so the nested
    /// colour-picker popup works — a menu_button closed on the first inner click.
    show_colors: bool,
    /// Min-area noise threshold in mm² (G9): diff regions smaller than this are
    /// dropped. 0 disables it. Always surfaced — the caption reports how many were
    /// hidden. Driven by a slider in the top bar.
    min_area_mm2: f64,
    /// Regions hidden by the threshold last frame, for the caption.
    last_hidden: usize,
    cam: Camera,
    /// Current theme (dark/light) and the one last applied to egui, so a change
    /// re-applies the visuals (the old code applied once and never updated).
    theme: Theme,
    applied_theme: Option<Theme>,
    /// Trust warning is expanded (G1b). Auto-hide is time-derived; this records
    /// only explicit user intent (chip click expands, overlay dismiss collapses).
    warning_expanded: bool,
    /// `ctx.input().time` when the warning was first shown / last re-expanded.
    warning_shown_at: Option<f64>,
    /// Index of the board-outline layer (Edge.Cuts/GKO), drawn faintly on every
    /// layer for orientation (G10); None if the board has no outline layer.
    outline: Option<usize>,
    show_outline: bool,
    /// World-space tessellation cache (G6): rebuilt only when the GeomKey changes,
    /// so pan/zoom/colour edits skip re-triangulation.
    cache: Option<TessCache>,
    /// Measure tool active (#22): canvas clicks drop ruler points instead of
    /// panning; Esc clears and exits.
    measure_mode: bool,
    /// The last (up to) two world-space points of the ruler.
    measure_pts: Vec<[f64; 2]>,
    /// Unit the measure label is shown in (#50): mm / inch / mil.
    measure_unit: Unit,
    /// Grid overlay on (#51): faint world-spaced lines over the canvas.
    show_grid: bool,
    /// Grid spacing in mm (#51); the DragValue edits this.
    grid_mm: f64,
    /// Snap measure clicks to the nearest grid intersection (#51).
    snap_grid: bool,
    /// Input scheme matching the user's ECAD tool (#54). MVP: controls which mouse
    /// button pans the canvas. Persisted via #52.
    input_preset: InputPreset,
    /// Swipe/curtain divider position, normalized 0..1 across the canvas width
    /// (#61). Clamped to [0.1, 0.9] on use; persisted via #52.
    swipe_frac: f32,
    /// Transient: the swipe divider is being dragged (#61). Not persisted — it only
    /// holds the grab across frames so leaving the handle mid-drag keeps it.
    swipe_drag: bool,
}

impl ViewApp {
    fn new(diff: BoardDiff, old_label: String, new_label: String) -> Self {
        let mut order: Vec<usize> = (0..diff.layers.len()).collect();
        order.sort_by_key(|&i| !diff.layers[i].is_changed()); // changed first, stable
        let selected = order.first().copied().unwrap_or(0);
        let outline = pick_outline_index(diff.layers.len(), |i| diff.layers[i].kind);
        // Default visibility (#9/#10 perf): show only the selected layer on load;
        // multi-layer is opt-in via the checkboxes. Keeps the common case fast on
        // dense boards. `selected` is the most-changed layer (changed-first order).
        let visible_layers = default_visible(diff.layers.len(), selected);
        Self {
            diff,
            old_label,
            new_label,
            order,
            selected,
            visible_layers,
            mode: Mode::Overlay,
            base_level: BaseLevel::Faint,
            col_added: C_ADDED,
            col_removed: C_REMOVED,
            canvas_dark: C_CANVAS,
            canvas_light: C_CANVAS_LIGHT,
            grid_dark: C_GRID_DEFAULT,
            grid_light: C_GRID_DEFAULT_LIGHT,
            base_overrides: Vec::new(),
            show_colors: false,
            min_area_mm2: MIN_AREA_MM2,
            last_hidden: 0,
            cam: Camera::default(),
            theme: Theme::Dark,
            applied_theme: None,
            warning_expanded: false,
            warning_shown_at: None,
            outline,
            show_outline: true,
            cache: None,
            measure_mode: false,
            measure_pts: Vec::new(),
            measure_unit: Unit::Mm,
            show_grid: false,
            grid_mm: 1.0,
            snap_grid: false,
            input_preset: InputPreset::default(),
            swipe_frac: 0.5,
            swipe_drag: false,
        }
    }

    /// Trust-warning affordance (G1b): a fixed-height copper chip. While expanded
    /// it drops the full warning text as a floating overlay (so toggling never
    /// reflows the canvas); it auto-collapses to the chip after AUTO_HIDE_SECS, and
    /// clicking the chip re-expands. Always present when there are warnings — never
    /// silently gone.
    fn warnings_ui(&mut self, ui: &mut egui::Ui, now: f64) {
        if self.diff.report.warnings.is_empty() {
            return;
        }
        if self.warning_shown_at.is_none() {
            self.warning_shown_at = Some(now);
        }
        let phase = warning_phase(
            now,
            self.warning_shown_at,
            self.warning_expanded,
            AUTO_HIDE_SECS,
        );
        let n = self.diff.report.warnings.len();
        // No hover/active size change, so hovering the chip can't nudge the row.
        ui.visuals_mut().widgets.hovered.expansion = 0.0;
        ui.visuals_mut().widgets.active.expansion = 0.0;
        // Inline chip in the controls row. ASCII "[!]" marker — egui's default font
        // has no warning glyph (⚠ rendered as tofu). Click expands the overlay.
        let chip = ui
            .small_button(
                egui::RichText::new(format!("[!] {}", warning_label(n)))
                    .strong()
                    .color(C_COPPER),
            )
            .on_hover_text(self.diff.report.warnings[0].as_str());
        if chip.clicked() {
            self.warning_expanded = true;
            self.warning_shown_at = Some(now);
        }
        if matches!(phase, WarningPhase::Expanded | WarningPhase::Counting) {
            let pos = chip.rect.left_bottom() + egui::vec2(0.0, 4.0);
            egui::Area::new(egui::Id::new("etchy-warning-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::default()
                        .fill(chrome(self.theme).canvas)
                        .stroke(Stroke::new(1.0, C_COPPER))
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.set_max_width(560.0);
                            for w in &self.diff.report.warnings {
                                ui.label(egui::RichText::new("warning:").strong().color(C_COPPER));
                                ui.label(egui::RichText::new(w).color(C_CREAM));
                            }
                            if ui.small_button("dismiss").clicked() {
                                self.warning_expanded = false;
                            }
                        });
                });
            if phase == WarningPhase::Counting {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }

    fn select(&mut self, idx: usize) {
        // Selection (highlight) is independent of visibility (#2): clicking a layer
        // name highlights it but does NOT tick it on — the per-row checkbox is the
        // only thing that toggles visibility. Split/Swipe force the selected layer
        // visible regardless, so a highlight is never blank there. Only a genuine
        // selection change refits the camera.
        if idx != self.selected {
            self.selected = idx;
            self.cam.fitted = false; // refit on layer change
        }
    }

    /// Move the selection `delta` steps through the displayed (changed-first)
    /// order — the keyboard equivalent of clicking the next/previous layer.
    fn step_layer(&mut self, delta: i32) {
        self.select(step_in_order(&self.order, self.selected, delta));
    }
}

/// A section in the layer list (G5). Sections render in [`LayerGroup::ALL`] order;
/// classification works for both KiCad and Altium because it keys off the engine's
/// already-normalized [`etchy_core::LayerKind`], not raw filenames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LayerGroup {
    Copper,
    Mask,
    Silk,
    Paste,
    Drill,
    Mechanical,
    Other,
}

impl LayerGroup {
    /// Fixed top-to-bottom section order.
    const ALL: [LayerGroup; 7] = [
        LayerGroup::Copper,
        LayerGroup::Mask,
        LayerGroup::Silk,
        LayerGroup::Paste,
        LayerGroup::Drill,
        LayerGroup::Mechanical,
        LayerGroup::Other,
    ];

    fn title(self) -> &'static str {
        match self {
            LayerGroup::Copper => "Copper",
            LayerGroup::Mask => "Soldermask",
            LayerGroup::Silk => "Silkscreen",
            LayerGroup::Paste => "Paste",
            LayerGroup::Drill => "Drill",
            LayerGroup::Mechanical => "Mechanical",
            LayerGroup::Other => "Other",
        }
    }
}

/// Short layer label for the grouped list: just the position within its section
/// (the section header already names the category), e.g. `top`, `bottom`,
/// `inner 2`. The CLI keeps the full `top-copper` form via `LayerView::name`.
fn short_layer_name(kind: etchy_core::LayerKind) -> String {
    use etchy_core::LayerKind::*;
    match kind {
        TopCopper | TopMask | TopSilk | TopPaste => "top".to_string(),
        BottomCopper | BottomMask | BottomSilk | BottomPaste => "bottom".to_string(),
        InnerCopper(n) => format!("inner {n}"),
        Drill => "drill".to_string(),
        Outline => "outline".to_string(),
        Other => "other".to_string(),
    }
}

/// Which section a layer kind belongs to.
fn layer_group(kind: etchy_core::LayerKind) -> LayerGroup {
    use etchy_core::LayerKind::*;
    match kind {
        TopCopper | BottomCopper | InnerCopper(_) => LayerGroup::Copper,
        TopMask | BottomMask => LayerGroup::Mask,
        TopSilk | BottomSilk => LayerGroup::Silk,
        TopPaste | BottomPaste => LayerGroup::Paste,
        Drill => LayerGroup::Drill,
        Outline => LayerGroup::Mechanical,
        Other => LayerGroup::Other,
    }
}

/// Bucket layer indices into sections in [`LayerGroup::ALL`] order, preserving the
/// incoming (changed-first) order within each section. Empty sections are omitted.
/// `group_of` maps an index to its section, so this is testable without a `Board`.
fn group_layers(
    order: &[usize],
    group_of: impl Fn(usize) -> LayerGroup,
) -> Vec<(LayerGroup, Vec<usize>)> {
    LayerGroup::ALL
        .iter()
        .filter_map(|&g| {
            let idxs: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&i| group_of(i) == g)
                .collect();
            (!idxs.is_empty()).then_some((g, idxs))
        })
        .collect()
}

/// Step `delta` positions through `order` from whichever entry equals `selected`,
/// wrapping at both ends; returns the new `selected` (an index into `diff.layers`).
/// A `selected` absent from `order` starts from the front; an empty `order` is a
/// no-op. Pure index math, kept separate from egui so it can be unit-tested.
fn step_in_order(order: &[usize], selected: usize, delta: i32) -> usize {
    if order.is_empty() {
        return selected;
    }
    let n = order.len() as i32;
    let pos = order.iter().position(|&i| i == selected).unwrap_or(0) as i32;
    let next = (pos + delta).rem_euclid(n);
    order[next as usize]
}

/// Seconds the trust warning stays expanded before auto-collapsing to a chip (G1b).
const AUTO_HIDE_SECS: f64 = 6.0;

/// Count-aware warning label (singular/plural); empty for zero.
fn warning_label(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "1 warning".to_string(),
        _ => format!("{n} warnings"),
    }
}

/// Display state of the trust-warning affordance (G1b).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WarningPhase {
    /// Collapsed to a clickable chip (user dismissed it, or it auto-hid).
    Chip,
    /// Expanded and counting down to auto-hide (egui must keep repainting).
    Counting,
    /// Expanded; not yet stamped with a shown-at time.
    Expanded,
}

/// Which phase the warning is in, from the current time, when it was first shown,
/// and whether the user has it expanded. Pure so the timing is unit-testable; the
/// egui shell just reads `i.time` and renders chip-or-overlay. Auto-hide is derived
/// purely from elapsed time, so `expanded` only records explicit user intent.
fn warning_phase(
    now: f64,
    shown_at: Option<f64>,
    expanded: bool,
    auto_hide_secs: f64,
) -> WarningPhase {
    if !expanded {
        return WarningPhase::Chip;
    }
    match shown_at {
        None => WarningPhase::Expanded,
        Some(t) => {
            if now - t >= auto_hide_secs {
                WarningPhase::Chip
            } else {
                WarningPhase::Counting
            }
        }
    }
}

/// Index of the board-outline layer (first `LayerKind::Outline`), if any (G10).
/// Takes a count + kind accessor so it's testable without building `LayerView`s.
fn pick_outline_index(n: usize, kind_of: impl Fn(usize) -> etchy_core::LayerKind) -> Option<usize> {
    (0..n).find(|&i| kind_of(i) == etchy_core::LayerKind::Outline)
}

/// Whether to show the "board edge" legend row: only when the outline is enabled,
/// exists, and isn't the layer currently being viewed (G10).
fn outline_legend_visible(show_outline: bool, outline: Option<usize>, selected: usize) -> bool {
    show_outline && outline.is_some_and(|i| i != selected)
}

// Brand palette (assets/brand/README.md): diff accents + board-dark canvas.
const C_ADDED: Color32 = Color32::from_rgb(0x46, 0xd1, 0x8a); // #46d18a
const C_REMOVED: Color32 = Color32::from_rgb(0xff, 0x5d, 0x73); // #ff5d73
const C_BASE: Color32 = Color32::from_rgb(90, 95, 105);
/// Brand "board dark" — the canvas (PCB) background.
const C_CANVAS: Color32 = Color32::from_rgb(0x0b, 0x0f, 0x0e); // #0b0f0e
/// Brand "surface" charcoal — panels/chrome, one step up from the board so the
/// UI doesn't read as one flat near-black mass.
const C_SURFACE: Color32 = Color32::from_rgb(0x14, 0x1a, 0x18); // #141a18
/// Brand copper-gold (ENIG) accent.
const C_COPPER: Color32 = Color32::from_rgb(0xe8, 0xa3, 0x3d); // #e8a33d
/// Brand paper-cream text.
const C_CREAM: Color32 = Color32::from_rgb(0xf4, 0xf1, 0xe8); // #f4f1e8
/// Faint copper for the board-outline reference on every layer (G10) — reads as
/// chrome, not diff content. Premultiply-safe via from_rgba_unmultiplied.
const C_OUTLINE_FAINT: Color32 = Color32::from_rgba_premultiplied(0x38, 0x27, 0x0e, 0x3c);
/// Default grid colour (#53): a faint copper, readable on the board-dark canvas
/// without competing with diff content. User-overridable + persisted (#52); the
/// grid (#51) draws with the live `grid_color`, this is just its default.
const C_GRID_DEFAULT: Color32 = Color32::from_rgba_premultiplied(0x1f, 0x16, 0x08, 0x22);
/// Light-theme default canvas: the paper-cream board (matches `chrome(Light)`), so a
/// dark canvas tuned in dark mode never carries into light mode and vice versa (#31).
const C_CANVAS_LIGHT: Color32 = Color32::from_rgb(0xf4, 0xf1, 0xe8);
/// Light-theme default grid: a faint copper-brown that reads on the cream board (the
/// dark default is far too light to see there) (#31).
const C_GRID_DEFAULT_LIGHT: Color32 = Color32::from_rgba_premultiplied(0x26, 0x1c, 0x09, 0x46);

/// Default canvas colour for a theme (#31) — the per-theme reset target.
fn default_canvas(theme: Theme) -> Color32 {
    match theme {
        Theme::Dark => C_CANVAS,
        Theme::Light => C_CANVAS_LIGHT,
    }
}

/// Default grid colour for a theme (#31) — the per-theme reset target.
fn default_grid(theme: Theme) -> Color32 {
    match theme {
        Theme::Dark => C_GRID_DEFAULT,
        Theme::Light => C_GRID_DEFAULT_LIGHT,
    }
}
/// Faint copper for the measure crosshairs (#50) — visible against the board but
/// clearly chrome, not diff content.
const C_CROSSHAIR: Color32 = Color32::from_rgba_premultiplied(0x70, 0x55, 0x22, 0x80);

/// Light or dark theme (G — dark/light mode).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Theme {
    Dark,
    Light,
}

/// Theme-dependent brand colours. Copper/added/removed stay constant across themes.
struct Chrome {
    canvas: Color32,
    surface: Color32,
    text: Color32,
}

/// The board (canvas) / chrome (panels) / text colours for a theme.
fn chrome(theme: Theme) -> Chrome {
    match theme {
        Theme::Dark => Chrome {
            canvas: C_CANVAS,
            surface: C_SURFACE,
            text: C_CREAM,
        },
        Theme::Light => Chrome {
            canvas: Color32::from_rgb(0xf4, 0xf1, 0xe8), // paper-cream board
            surface: Color32::from_rgb(0xe6, 0xe2, 0xd5), // slightly darker chrome
            text: Color32::from_rgb(0x1c, 0x22, 0x20),   // near-black ink
        },
    }
}

/// The etchy egui theme for the given mode: branded panels, copper accents.
fn brand_visuals(theme: Theme) -> egui::Visuals {
    let c = chrome(theme);
    let mut v = if theme == Theme::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    v.panel_fill = c.surface; // chrome, distinct from the canvas board colour
    v.window_fill = c.surface;
    v.override_text_color = Some(c.text);
    v.hyperlink_color = C_COPPER;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xe8, 0xa3, 0x3d, 70);
    v.selection.stroke = Stroke::new(1.0, C_COPPER);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, C_COPPER);
    v.widgets.active.bg_fill = Color32::from_rgb(0x6b, 0x4c, 0x1d);
    v.widgets.active.bg_stroke = Stroke::new(1.0, C_COPPER);
    v
}

// Level-of-detail (G9). Diff features are drawn true-to-scale: at/above
// LOD_HI_PX they're fully opaque geometry, at/below LOD_LO_PX they vanish into
// the heatmap, linear between. So tiny changes fade out instead of clamping to a
// fixed dot (the old blob/all-green-when-zoomed-out bug).
const LOD_LO_PX: f32 = 1.5;
const LOD_HI_PX: f32 = 5.0;
/// Side length (screen px) of the fixed marker dot drawn for a real-but-sub-pixel
/// diff region (below `LOD_LO_PX`), so it stays visible at every zoom instead of
/// phantoming (#14). Approximate — needs later visual tuning against real boards.
const MARKER_PX: f32 = 3.0;
/// Default min-area threshold (mm²). Diff regions smaller than this are treated
/// as noise (e.g. the sub-µm rims from a units/precision mismatch) and dropped —
/// but the count is always surfaced in the caption, never silently.
const MIN_AREA_MM2: f64 = 0.0004;

// ===========================================================================
// Persisted user settings (#52). eframe's built-in storage saves these to a RON
// config file on native and to localStorage on web — no new external crate. The
// struct mirrors the user-tunable view state on `ViewApp`; `Color32` isn't
// `Serialize`, so colours are stored as `[u8; 4]` (unmultiplied sRGBA).
// ===========================================================================

/// Theme persists as a stable string so the on-disk form survives enum reordering.
impl serde::Serialize for Theme {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        })
    }
}
impl<'de> serde::Deserialize<'de> for Theme {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(match String::deserialize(d)?.as_str() {
            "light" => Theme::Light,
            _ => Theme::Dark,
        })
    }
}

/// Base level persists as a stable string (same rationale as `Theme`).
impl serde::Serialize for BaseLevel {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            BaseLevel::Off => "off",
            BaseLevel::Faint => "faint",
            BaseLevel::Strong => "strong",
        })
    }
}
impl<'de> serde::Deserialize<'de> for BaseLevel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(match String::deserialize(d)?.as_str() {
            "off" => BaseLevel::Off,
            "strong" => BaseLevel::Strong,
            _ => BaseLevel::Faint,
        })
    }
}

/// Input preset persists as a stable string (same rationale as `Theme`).
impl serde::Serialize for InputPreset {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            InputPreset::KiCad => "kicad",
            InputPreset::Altium => "altium",
        })
    }
}
impl<'de> serde::Deserialize<'de> for InputPreset {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Unknown / legacy values (incl. the removed "etchy") fall back to the default.
        Ok(match String::deserialize(d)?.as_str() {
            "kicad" => InputPreset::KiCad,
            _ => InputPreset::Altium,
        })
    }
}

/// `Color32` -> unmultiplied sRGBA bytes, for storage (Color32 isn't Serialize).
fn color_to_rgba(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}
/// The inverse of [`color_to_rgba`].
fn rgba_to_color([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// The user-tunable view state, persisted via eframe storage (#52). Colours are
/// `[u8; 4]` because `Color32` isn't `Serialize`; everything round-trips through
/// [`ViewApp::to_settings`] / [`ViewApp::apply_settings`].
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
struct Settings {
    theme: Theme,
    base_level: BaseLevel,
    /// Per-layer base-colour overrides, keyed by layer index (#21).
    base_overrides: Vec<(usize, [u8; 4])>,
    min_area_mm2: f64,
    col_added: [u8; 4],
    col_removed: [u8; 4],
    /// Canvas + grid colours (#53), per-theme so light/dark keep separate boards (#31).
    canvas_dark: [u8; 4],
    canvas_light: [u8; 4],
    grid_dark: [u8; 4],
    grid_light: [u8; 4],
    /// Input scheme matching the user's ECAD tool (#54).
    input_preset: InputPreset,
    /// Indices of the layers that were visible (#58/#59), restored on next open
    /// (#52). Empty = fall back to the on-load default (changed layers + selected).
    visible_layers: Vec<usize>,
    /// Swipe/curtain divider position, normalized 0..1 (#61).
    swipe_frac: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            base_level: BaseLevel::Faint,
            base_overrides: Vec::new(),
            min_area_mm2: MIN_AREA_MM2,
            col_added: color_to_rgba(C_ADDED),
            col_removed: color_to_rgba(C_REMOVED),
            canvas_dark: color_to_rgba(C_CANVAS),
            canvas_light: color_to_rgba(C_CANVAS_LIGHT),
            grid_dark: color_to_rgba(C_GRID_DEFAULT),
            grid_light: color_to_rgba(C_GRID_DEFAULT_LIGHT),
            input_preset: InputPreset::default(),
            visible_layers: Vec::new(),
            swipe_frac: 0.5,
        }
    }
}

impl ViewApp {
    /// Build the app and restore any persisted settings from eframe storage (#52).
    /// The creation closure for both `run_native` and the web `WebRunner` routes
    /// through here so saved preferences apply identically on both surfaces.
    fn from_cc(
        cc: &eframe::CreationContext<'_>,
        diff: BoardDiff,
        old_label: String,
        new_label: String,
    ) -> Self {
        let mut app = Self::new(diff, old_label, new_label);
        if let Some(storage) = cc.storage {
            if let Some(settings) = eframe::get_value::<Settings>(storage, eframe::APP_KEY) {
                app.apply_settings(settings);
            }
        }
        app
    }

    /// Snapshot the user-tunable state for persistence (#52).
    fn to_settings(&self) -> Settings {
        Settings {
            theme: self.theme,
            base_level: self.base_level,
            base_overrides: self
                .base_overrides
                .iter()
                .map(|(i, c)| (*i, color_to_rgba(*c)))
                .collect(),
            min_area_mm2: self.min_area_mm2,
            col_added: color_to_rgba(self.col_added),
            col_removed: color_to_rgba(self.col_removed),
            canvas_dark: color_to_rgba(self.canvas_dark),
            canvas_light: color_to_rgba(self.canvas_light),
            grid_dark: color_to_rgba(self.grid_dark),
            grid_light: color_to_rgba(self.grid_light),
            input_preset: self.input_preset,
            visible_layers: visible_indices(&self.visible_layers),
            swipe_frac: self.swipe_frac,
        }
    }

    /// Apply persisted settings onto a freshly-built app (#52). Diff geometry and
    /// the layer order are NOT touched — only view preferences.
    fn apply_settings(&mut self, s: Settings) {
        self.theme = s.theme;
        self.applied_theme = None; // force re-applying the egui visuals next frame
        self.base_level = s.base_level;
        self.base_overrides = s
            .base_overrides
            .into_iter()
            .map(|(i, c)| (i, rgba_to_color(c)))
            .collect();
        self.min_area_mm2 = s.min_area_mm2;
        self.col_added = rgba_to_color(s.col_added);
        self.col_removed = rgba_to_color(s.col_removed);
        self.canvas_dark = rgba_to_color(s.canvas_dark);
        self.canvas_light = rgba_to_color(s.canvas_light);
        self.grid_dark = rgba_to_color(s.grid_dark);
        self.grid_light = rgba_to_color(s.grid_light);
        self.input_preset = s.input_preset;
        // Restore the visible set (#58/#59) over the current layer count, dropping
        // stale indices. An empty saved set keeps the on-load default.
        if !s.visible_layers.is_empty() {
            self.visible_layers = restore_visibility(&s.visible_layers, self.visible_layers.len());
            // The selected layer must stay visible so its highlight has something
            // to draw.
            if let Some(v) = self.visible_layers.get_mut(self.selected) {
                *v = true;
            }
        }
        self.swipe_frac = clamp_swipe_frac(s.swipe_frac);
    }
}

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keyboard shortcuts. Suppressed while a text field has focus (the numeric
        // noise-filter / grid-spacing DragValue) so typing digits doesn't switch mode
        // or fire a shortcut. Ctrl+M is modifier-aware (#52, fix #4).
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (
            toggle_base,
            fit,
            mode_overlay,
            mode_before,
            mode_after,
            mode_split,
            mode_swipe,
            next,
            prev,
            toggle_outline,
            escape,
            cycle_unit,
            toggle_grid,
            toggle_measure,
        ) = ui.input(|i| {
            use egui::Key;
            if typing {
                return (
                    false, false, false, false, false, false, false, false, false, false, false,
                    false, false, false,
                );
            }
            (
                i.key_pressed(Key::S),
                i.key_pressed(Key::F),
                // Mode hotkeys (#55, #61): 1=Overlay 2=Before 3=After 4=Split 5=Swipe,
                // + aliases O/B/A.
                i.key_pressed(Key::Num1) || i.key_pressed(Key::O),
                i.key_pressed(Key::Num2) || i.key_pressed(Key::B),
                i.key_pressed(Key::Num3) || i.key_pressed(Key::A),
                i.key_pressed(Key::Num4),
                i.key_pressed(Key::Num5),
                i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J),
                i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K),
                i.key_pressed(Key::E),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::U),
                i.key_pressed(Key::G),
                i.modifiers.ctrl && i.key_pressed(Key::M),
            )
        });
        if toggle_measure {
            // Ctrl+M toggles measure mode (#52, fix #4), mirroring the button.
            self.measure_mode = !self.measure_mode;
            if !self.measure_mode {
                self.measure_pts.clear();
            }
        }
        if escape {
            if self.show_colors {
                // Esc backs out of the Colours window (#4). An open colour-picker
                // popup consumes the first Esc itself (egui closes it; while its RGB
                // field has focus our `typing` guard suppresses this handler), so the
                // next Esc lands here and closes the window.
                self.show_colors = false;
            } else {
                // Esc cascades (#50): first clear the in-progress measurement, then a
                // second Esc (nothing to clear) turns the measure tool off.
                let (next_mode, clear) =
                    measure_escape(self.measure_mode, !self.measure_pts.is_empty());
                if clear {
                    self.measure_pts.clear();
                }
                self.measure_mode = next_mode;
            }
        }
        if cycle_unit {
            self.measure_unit = self.measure_unit.next();
        }
        if toggle_grid {
            self.show_grid = !self.show_grid;
        }
        if toggle_base {
            self.base_level = cycle_base(self.base_level);
        }
        if toggle_outline {
            self.show_outline = !self.show_outline;
        }
        if fit {
            self.cam.fitted = false;
        }
        if mode_overlay {
            self.mode = Mode::Overlay;
        }
        if mode_before {
            self.mode = Mode::Before;
        }
        if mode_after {
            self.mode = Mode::After;
        }
        if mode_split {
            self.mode = Mode::Split;
        }
        if mode_swipe {
            self.mode = Mode::Swipe;
        }
        if next {
            self.step_layer(1);
        }
        if prev {
            self.step_layer(-1);
        }

        // Brand theme, applied once (G7c).
        if self.applied_theme != Some(self.theme) {
            ui.ctx().set_visuals(brand_visuals(self.theme));
            self.applied_theme = Some(self.theme);
        }
        egui::Panel::top("top").show_inside(ui, |ui| {
            // Title row: a single "etchy" wordmark (one lockup, matching the web),
            // the revisions, and the headline totals.
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("etchy")
                        .size(24.0)
                        .strong()
                        .color(C_COPPER),
                );
                ui.add_space(8.0);
                ui.label(
                    // ASCII "->" — egui's default font has no arrow glyph (→ renders
                    // as tofu, #30).
                    egui::RichText::new(format!("{}  ->  {}", self.old_label, self.new_label))
                        .size(15.0)
                        .color(C_CREAM),
                );
                // Totals pushed to the right.
                let t = &self.diff.report.totals;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "+{:.4}  −{:.4} mm²",
                            t.added_area_mm2, t.removed_area_mm2
                        ))
                        .size(14.0)
                        .color(Color32::from_gray(170)),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}/{} layers changed",
                            t.layers_changed, t.layers_total
                        ))
                        .size(14.0)
                        .strong(),
                    );
                });
            });
            ui.add_space(4.0);
            // Controls row: larger hit targets than the egui default. Wrapped so a
            // narrow window flows controls onto a second line instead of running them
            // off the right edge (#5).
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(14.0, 10.0);
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.selectable_value(&mut self.mode, Mode::Overlay, "Overlay");
                ui.selectable_value(&mut self.mode, Mode::Before, "Before");
                ui.selectable_value(&mut self.mode, Mode::After, "After");
                ui.selectable_value(&mut self.mode, Mode::Split, "Split");
                ui.selectable_value(&mut self.mode, Mode::Swipe, "Swipe");
                ui.separator();
                ui.label("base:");
                ui.selectable_value(&mut self.base_level, BaseLevel::Off, "off");
                ui.selectable_value(&mut self.base_level, BaseLevel::Faint, "faint");
                ui.selectable_value(&mut self.base_level, BaseLevel::Strong, "strong");
                if ui.selectable_label(self.show_colors, "Colors").clicked() {
                    self.show_colors = !self.show_colors;
                }
                if self.outline.is_some() {
                    ui.checkbox(&mut self.show_outline, "board edge").on_hover_text(
                        "Show the board outline (Edge.Cuts/GKO) as a faint reference on every layer.",
                    );
                }
                if ui.button("Fit").clicked() {
                    self.cam.fitted = false;
                }
                if ui
                    .selectable_label(self.measure_mode, "Measure")
                    .on_hover_text(
                        "Click two points on the canvas to measure the distance. The result \
                         stays drawn (Esc clears it but keeps measuring); the next click after \
                         two points starts a fresh measurement. Toggle off to exit + clear.",
                    )
                    .clicked()
                {
                    self.measure_mode = !self.measure_mode;
                    if !self.measure_mode {
                        self.measure_pts.clear();
                    }
                }
                // Units toggle (#50): click to cycle mm → inch → mil (hotkey: U).
                if ui
                    .selectable_label(true, format!("units: {}", self.measure_unit.label()))
                    .on_hover_text("Distance unit for the measure label — click or press U to cycle mm / inch / mil.")
                    .clicked()
                {
                    self.measure_unit = self.measure_unit.next();
                }
                ui.separator();
                // Grid overlay (#51). Colour is a fixed faint default for now —
                // configurable grid colour is deferred to #53.
                if ui
                    .selectable_label(self.show_grid, "Grid")
                    .on_hover_text("Overlay a faint reference grid (hotkey: G). Grid colour is configurable later (#53).")
                    .clicked()
                {
                    self.show_grid = !self.show_grid;
                }
                ui.add(
                    egui::DragValue::new(&mut self.grid_mm)
                        .speed(0.1)
                        .range(0.01..=100.0)
                        .suffix(" mm"),
                )
                .on_hover_text("Grid spacing in mm.");
                ui.checkbox(&mut self.snap_grid, "snap")
                    .on_hover_text("Snap measure clicks to the nearest grid intersection.");
                ui.separator();
                // Input scheme matching the user's ECAD tool (#54). MVP: picks
                // which mouse button pans the canvas; persisted via #52.
                let preset_label = |p: InputPreset| match p {
                    InputPreset::KiCad => "KiCad",
                    InputPreset::Altium => "Altium",
                };
                egui::ComboBox::from_id_salt("input_preset")
                    .selected_text(preset_label(self.input_preset))
                    .show_ui(ui, |ui| {
                        for p in [InputPreset::Altium, InputPreset::KiCad] {
                            ui.selectable_value(&mut self.input_preset, p, preset_label(p));
                        }
                    })
                    .response
                    .on_hover_text(
                        "Pan mouse button by ECAD tool: \
                         Altium = right-drag, KiCad = middle/right-drag.",
                    );
                ui.separator();
                // Dark/light toggle. ASCII label — egui's default font has no
                // sun/moon glyph (it rendered as tofu). Re-applied live in ui().
                let label = if self.theme == Theme::Dark {
                    "theme: dark"
                } else {
                    "theme: light"
                };
                if ui.button(label).clicked() {
                    self.theme = if self.theme == Theme::Dark {
                        Theme::Light
                    } else {
                        Theme::Dark
                    };
                }
                ui.separator();
                ui.add(
                    // Linear range (user found the log feel odd — #52). Widened to
                    // 0.1 mm² so coarser noise can be filtered (#23).
                    egui::Slider::new(&mut self.min_area_mm2, 0.0..=0.1)
                        .text("noise filter (mm²)")
                        .fixed_decimals(4),
                )
                .on_hover_text(
                    "Drop diff regions smaller than this as noise; 0 = off. \
                     The hidden count is shown in the canvas caption.",
                );
                // Editable numeric field so the user can set any value, including
                // beyond the slider's max (#23).
                ui.add(
                    egui::DragValue::new(&mut self.min_area_mm2)
                        .speed(0.001)
                        .range(0.0..=f64::INFINITY)
                        .fixed_decimals(4),
                )
                .on_hover_text("Type or drag to set the noise filter exactly (mm²), beyond the slider's range.");
                ui.separator();
                // Warning chip lives IN the controls row (no separate row that can
                // reflow the canvas — #49). Overlay floats; ASCII glyph (no tofu).
                let now = ui.ctx().input(|i| i.time);
                self.warnings_ui(ui, now);
            });
        });

        egui::Panel::left("layers")
            .resizable(true)
            .default_size(260.0)
            .show_inside(ui, |ui| {
                ui.heading("Layers");
                ui.label(egui::RichText::new("changed first").weak().small());
                // Quick visibility actions (#58): show/hide every layer, or only the
                // changed ones. They never move the selection or camera.
                ui.horizontal(|ui| {
                    if ui.small_button("Show all").clicked() {
                        for v in self.visible_layers.iter_mut() {
                            *v = true;
                        }
                    }
                    if ui.small_button("Hide all").clicked() {
                        for v in self.visible_layers.iter_mut() {
                            *v = false;
                        }
                        // Keep the active layer drawn so its highlight isn't blank.
                        if let Some(v) = self.visible_layers.get_mut(self.selected) {
                            *v = true;
                        }
                    }
                    // "Show changed" button hidden per feedback #8 — the capability
                    // stays in `visible_from_changed` (still unit-tested) so it can be
                    // re-surfaced later, but the button is removed from the row.
                });
                ui.separator();
                // Actions deferred so the per-frame group iteration doesn't borrow
                // self mutably while it's borrowed for the group list.
                let mut select: Option<usize> = None;
                let mut toggle: Option<(usize, bool)> = None; // (layer, show)
                let mut group_set: Option<(Vec<usize>, bool)> = None; // (idxs, show)
                let mut set_color: Option<(usize, Color32)> = None; // (layer, colour) (#3)
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Group into sections (copper / mask / silk / …) in fixed order,
                    // changed-first within each (G5).
                    let groups =
                        group_layers(&self.order, |i| layer_group(self.diff.layers[i].kind));
                    for (group, idxs) in groups {
                        ui.add_space(4.0);
                        // Group header (#36 collapse + #58 show/hide-all): a
                        // CollapsingState lets the header carry BOTH the disclosure
                        // triangle (rotates down=open / right=collapsed) AND a group
                        // show/hide-all checkbox; the body holds the layer rows. egui
                        // (with eframe persistence) remembers each group's open state.
                        let gid = ui.make_persistent_id(("layer-group", group.title()));
                        let state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                gid,
                                true,
                            );
                        state
                            .show_header(ui, |ui| {
                                // Show/hide every layer in the group (#58). Separate
                                // from collapsing, which only hides the list rows.
                                let mut all = group_all_visible(&self.visible_layers, &idxs);
                                if ui
                                    .checkbox(&mut all, "")
                                    .on_hover_text("Show / hide every layer in this group")
                                    .changed()
                                {
                                    group_set = Some((idxs.clone(), all));
                                }
                                ui.label(
                                    egui::RichText::new(group.title())
                                        .small()
                                        .color(Color32::from_rgb(0xe8, 0xa3, 0x3d)),
                                );
                            })
                            // Indent the rows under the header (#1) so the group name
                            // reads as the parent, left of its layers.
                            .body(|ui| {
                                for idx in &idxs {
                                    let idx = *idx;
                                    let l = &self.diff.layers[idx];
                                    // Row: a per-layer visibility checkbox, a small
                                    // colour swatch (painted rect, not a font glyph —
                                    // the default font lacks ● and renders tofu,
                                    // #16), the layer name, then a compact change
                                    // micro-label (#25).
                                    let kind = l.kind;
                                    let changed = l.is_changed();
                                    let added = l.change.added_area_mm2();
                                    let removed = l.change.removed_area_mm2();
                                    let name = short_layer_name(kind);
                                    let visible =
                                        self.visible_layers.get(idx).copied().unwrap_or(false);
                                    let swatch = resolve_base_color(
                                        idx,
                                        kind,
                                        &self.base_overrides,
                                        self.theme,
                                    );
                                    let resp = ui
                                        .horizontal(|ui| {
                                            // Per-layer visibility toggle, separate from
                                            // the click-to-select label (#58).
                                            let mut vis = visible;
                                            if ui
                                                .checkbox(&mut vis, "")
                                                .on_hover_text("Show / hide this layer")
                                                .changed()
                                            {
                                                toggle = Some((idx, vis));
                                            }
                                            // Clickable colour swatch (#3): opens this
                                            // layer's colour picker; a change records a
                                            // per-layer base override (applied below).
                                            let mut sw = swatch;
                                            if ui
                                                .color_edit_button_srgba(&mut sw)
                                                .on_hover_text("Layer colour — click to change")
                                                .changed()
                                            {
                                                set_color = Some((idx, sw));
                                            }
                                            // Visible layers read brighter; hidden grey.
                                            let label = match (visible, changed) {
                                                (true, true) => egui::RichText::new(&name).strong(),
                                                (true, false) => egui::RichText::new(&name),
                                                (false, _) => egui::RichText::new(&name)
                                                    .weak()
                                                    .color(Color32::GRAY),
                                            };
                                            let r =
                                                ui.selectable_label(idx == self.selected, label);
                                            // Compact +A/-B mm² micro-label on changed
                                            // layers. No old/base area is exposed by
                                            // etchy-core, so a percent isn't available —
                                            // show the deltas instead.
                                            if changed {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "+{added:.3} −{removed:.3}"
                                                            ))
                                                            .small()
                                                            .weak(),
                                                        );
                                                    },
                                                );
                                            }
                                            r
                                        })
                                        .inner;
                                    if resp.clicked() {
                                        select = Some(idx);
                                    }
                                }
                            });
                    }
                });
                // Apply deferred actions.
                if let Some((idxs, show)) = group_set {
                    set_group_visibility(&mut self.visible_layers, &idxs, show);
                }
                if let Some((idx, show)) = toggle {
                    if let Some(v) = self.visible_layers.get_mut(idx) {
                        *v = show;
                    }
                }
                if let Some(idx) = select {
                    self.select(idx);
                }
                // Per-layer colour override from the inline swatch picker (#3).
                if let Some((idx, c)) = set_color {
                    if let Some(e) = self.base_overrides.iter_mut().find(|(i, _)| *i == idx) {
                        e.1 = c;
                    } else {
                        self.base_overrides.push((idx, c));
                    }
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.draw_canvas(ui);
        });

        // Colors editor — a real Window (not a menu) so the nested colour-picker
        // popup works; a menu_button closed on the first click inside it.
        if self.show_colors {
            let mut open = true;
            // Open centered on the screen (#56): pin the first-frame position to the
            // viewport centre via a CENTER_CENTER pivot. egui remembers the dragged
            // position afterwards, so it stays movable.
            let center = ui.ctx().content_rect().center();
            egui::Window::new("Colors")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_pos(center)
                .pivot(egui::Align2::CENTER_CENTER)
                .show(ui.ctx(), |ui| {
                    ui.label(egui::RichText::new("Diff colours").strong());
                    ui.horizontal(|ui| {
                        ui.label("added");
                        ui.color_edit_button_srgba(&mut self.col_added);
                        ui.label("removed");
                        ui.color_edit_button_srgba(&mut self.col_removed);
                    });
                    if ui.button("reset diff to brand").clicked() {
                        self.col_added = C_ADDED;
                        self.col_removed = C_REMOVED;
                    }
                    ui.separator();
                    // Canvas + grid colours (#53) — per-theme (#31), persisted (#52).
                    // The pickers edit the ACTIVE theme; switch dark/light to tune the
                    // other, so a charcoal canvas never bleeds into light mode.
                    let theme_name = match self.theme {
                        Theme::Dark => "dark",
                        Theme::Light => "light",
                    };
                    ui.label(
                        egui::RichText::new(format!("Canvas & grid ({theme_name} mode)")).strong(),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Canvas");
                        ui.color_edit_button_srgba(self.canvas_color_mut());
                        ui.label("Grid");
                        ui.color_edit_button_srgba(self.grid_color_mut());
                    });
                    if ui.button("reset canvas & grid to default").clicked() {
                        self.canvas_dark = default_canvas(Theme::Dark);
                        self.canvas_light = default_canvas(Theme::Light);
                        self.grid_dark = default_grid(Theme::Dark);
                        self.grid_light = default_grid(Theme::Light);
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("Layer base colours").strong());
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        // Fill the window width so the scrollbar sits at the far right
                        // instead of hugging the (narrow) content (#6).
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            // One row per LAYER (by index), so two layers of the same
                            // kind can be coloured apart (#21).
                            for idx in 0..self.diff.layers.len() {
                                let kind = self.diff.layers[idx].kind;
                                let label = self.diff.layers[idx].name();
                                ui.horizontal(|ui| {
                                    let mut base = resolve_base_color(
                                        idx,
                                        kind,
                                        &self.base_overrides,
                                        self.theme,
                                    );
                                    if ui.color_edit_button_srgba(&mut base).changed() {
                                        if let Some(e) =
                                            self.base_overrides.iter_mut().find(|(i, _)| *i == idx)
                                        {
                                            e.1 = base;
                                        } else {
                                            self.base_overrides.push((idx, base));
                                        }
                                    }
                                    ui.label(label);
                                    if self.base_overrides.iter().any(|(i, _)| *i == idx)
                                        && ui.small_button("reset").clicked()
                                    {
                                        self.base_overrides.retain(|(i, _)| *i != idx);
                                    }
                                });
                            }
                        });
                });
            self.show_colors = open;
        }

        // Publish "what they're looking at" for the web feedback widget.
        let layer_name = self.diff.layers[self.selected].name().to_string();
        let mode = match self.mode {
            Mode::Overlay => "Overlay",
            Mode::Before => "Before",
            Mode::After => "After",
            Mode::Split => "Split",
            Mode::Swipe => "Swipe",
        };
        let zoom_pct = if self.cam.fit_scale > 0.0 {
            (self.cam.scale / self.cam.fit_scale * 100.0).round() as i32
        } else {
            100
        };
        publish_state(&layer_name, mode, zoom_pct);
    }

    /// Clear the native framebuffer to the brand board-dark, so the window reads
    /// as #0b0f0e (not the default near-black) and matches the web page.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.canvas_color().to_normalized_gamma_f32()
    }

    /// Persist the user-tunable view state (#52). eframe calls this periodically
    /// and on shutdown; it writes to a RON config file on native and localStorage
    /// on web. Restored in the constructor via `eframe::get_value`.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.to_settings());
    }
}

impl ViewApp {
    /// Active-theme canvas colour (#31). The board paints with this, so light mode
    /// keeps its own background independent of any dark-mode tuning.
    fn canvas_color(&self) -> Color32 {
        match self.theme {
            Theme::Dark => self.canvas_dark,
            Theme::Light => self.canvas_light,
        }
    }

    /// Mutable handle to the active-theme canvas colour, for the colour picker (#31).
    fn canvas_color_mut(&mut self) -> &mut Color32 {
        match self.theme {
            Theme::Dark => &mut self.canvas_dark,
            Theme::Light => &mut self.canvas_light,
        }
    }

    /// Active-theme grid colour (#31).
    fn grid_color(&self) -> Color32 {
        match self.theme {
            Theme::Dark => self.grid_dark,
            Theme::Light => self.grid_light,
        }
    }

    /// Mutable handle to the active-theme grid colour, for the colour picker (#31).
    fn grid_color_mut(&mut self) -> &mut Color32 {
        match self.theme {
            Theme::Dark => &mut self.grid_dark,
            Theme::Light => &mut self.grid_light,
        }
    }

    /// Is the canvas being dragged with a button the current input preset assigns
    /// to panning (#54)? Maps egui's per-button drag state through `pans_on`.
    fn dragging_pans(&self, response: &egui::Response) -> bool {
        use egui::PointerButton::{Middle, Primary, Secondary};
        [Primary, Middle, Secondary]
            .into_iter()
            .any(|b| pans_on(self.input_preset, b) && response.dragged_by(b))
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui) {
        let layer = &self.diff.layers[self.selected];
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        // Clicking the board dismisses the Colors window (#19).
        if self.show_colors && response.clicked() {
            self.show_colors = false;
        }
        let rect = response.rect;
        // The board background uses the user-configurable canvas colour (#53),
        // defaulting to the brand board-dark.
        painter.rect_filled(rect, 0.0, self.canvas_color());

        // Fit on first show / layer change.
        if !self.cam.fitted {
            if let Some(bb) = layer_bbox(layer) {
                fit(&mut self.cam, bb, rect);
            }
            self.cam.fitted = true;
        }

        // Swipe/curtain divider (#61): a draggable vertical wipe line. Dragging it
        // takes priority over panning, so when the pointer grabs the divider the
        // pan logic below is skipped for this frame. The handle has a few px of
        // grab tolerance and shows a horizontal-resize cursor on hover.
        let mut swipe_dragging = false;
        if self.mode == Mode::Swipe {
            const GRAB_PX: f32 = 6.0;
            let (_, _, div_x) = swipe_rects(rect, self.swipe_frac);
            let near_div = response
                .hover_pos()
                .is_some_and(|p| (p.x - div_x).abs() <= GRAB_PX);
            // Track an in-progress drag that started on the divider so leaving the
            // grab band mid-drag doesn't drop it.
            if response.drag_started_by(egui::PointerButton::Primary) && near_div {
                self.swipe_drag = true;
            }
            if !response.dragged_by(egui::PointerButton::Primary) {
                self.swipe_drag = false;
            }
            if self.swipe_drag {
                if let Some(p) = response.interact_pointer_pos() {
                    let frac = (p.x - rect.left()) / rect.width().max(1.0);
                    self.swipe_frac = clamp_swipe_frac(frac);
                }
                swipe_dragging = true;
            }
            if near_div || self.swipe_drag {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        } else {
            self.swipe_drag = false;
        }

        // Measure tool (#22): record clicks as world points (keep the last 2).
        // In measure mode only a PRIMARY click places a point; secondary
        // (right) and middle drags still pan the board (#52, fix #2) so the user
        // can reposition mid-measure. Outside measure mode, any drag pans.
        if swipe_dragging {
            // Divider drag owns the pointer this frame — no panning or measuring.
        } else if self.measure_mode {
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let mut w = screen_to_world(&self.cam, pos, rect);
                    // Snap the placed point to the nearest grid intersection when
                    // enabled (#51) — the click follows the already-snapped cursor.
                    if self.snap_grid {
                        w = snap_world_to_grid(w, self.grid_mm);
                    }
                    // A completed pair persists; the next click starts a fresh one (#50).
                    if self.measure_pts.len() >= 2 {
                        self.measure_pts.clear();
                    }
                    self.measure_pts.push(w);
                }
            }
            // Pan with secondary/middle drag while measuring (#52, fix #2):
            // primary-drag stays reserved for point placement.
            if response.dragged_by(egui::PointerButton::Secondary)
                || response.dragged_by(egui::PointerButton::Middle)
            {
                let d = response.drag_delta();
                self.cam.center[0] -= d.x as f64 / self.cam.scale;
                self.cam.center[1] += d.y as f64 / self.cam.scale; // y flipped
            }
        } else if self.dragging_pans(&response) {
            // Pan (only when not measuring), on the button(s) the input preset
            // assigns to panning (#54).
            let d = response.drag_delta();
            self.cam.center[0] -= d.x as f64 / self.cam.scale;
            self.cam.center[1] += d.y as f64 / self.cam.scale; // y flipped
        }
        // Plain wheel = zoom (cursor-anchored); Ctrl+wheel = pan Y; Shift+wheel = pan X (G7b #12).
        // Read the RAW MouseWheel events (not smooth_scroll_delta): egui consumes
        // Ctrl+wheel into its own zoom and routes Shift+wheel to the X axis, so the
        // smoothed delta misses both modifiers. Sum the raw deltas, normalised to
        // points by unit, with the modifiers carried on each event.
        let vp_h = rect.height();
        let (raw, ctrl, shift) = ui.input(|i| {
            let mut d = egui::Vec2::ZERO;
            let (mut ctrl, mut shift) = (false, false);
            for ev in &i.events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = ev
                {
                    d += wheel_points(*unit, *delta, vp_h);
                    ctrl |= modifiers.ctrl;
                    shift |= modifiers.shift;
                }
            }
            // Clamp per-frame so a fast trackpad burst can't fling the view.
            d.x = d.x.clamp(-200.0, 200.0);
            d.y = d.y.clamp(-200.0, 200.0);
            (d, ctrl, shift)
        });
        match scroll_to_camera_action(raw.x, raw.y, ctrl, shift, 0.0015, 1.0) {
            CameraAction::Zoom(f) => {
                if let Some(ptr) = response.hover_pos() {
                    let before = screen_to_world(&self.cam, ptr, rect);
                    self.cam.scale *= f;
                    let after = screen_to_world(&self.cam, ptr, rect);
                    self.cam.center[0] += before[0] - after[0];
                    self.cam.center[1] += before[1] - after[1];
                }
            }
            // Screen px → world; y is flipped (matches drag panning).
            CameraAction::PanX(dx) => self.cam.center[0] -= dx / self.cam.scale,
            CameraAction::PanY(dy) => self.cam.center[1] += dy / self.cam.scale,
            CameraAction::None => {}
        }

        // Grid overlay (#51): faint world-spaced lines, drawn UNDER the geometry.
        // Skip if the on-screen spacing is too dense (< 6 px) so it never fills solid.
        if self.show_grid {
            draw_grid(&painter, &self.cam, rect, self.grid_mm, self.grid_color());
        }

        // Build the shapes to draw, per mode.
        // Geometry is triangulated ONCE and cached in world space (G6); only the
        // cheap world→screen transform + colour/alpha/min-area cull run per frame,
        // so pan/zoom and colour edits never re-triangulate. The cache rebuilds only
        // when the GeomKey (selection inputs) changes.
        // The visible set: every layer the user has shown (#58/#59). Split renders
        // the active layer only (a stacked old|new of many layers reads as mud), so
        // it keys off just the selected layer and falls back to it when nothing is on.
        let visible = if self.mode == Mode::Split || self.mode == Mode::Swipe {
            vec![self.selected]
        } else {
            let v = visible_indices(&self.visible_layers);
            if v.is_empty() {
                vec![self.selected]
            } else {
                v
            }
        };
        let key = build_geom_key(
            &visible,
            self.mode,
            self.base_level,
            self.show_outline,
            self.outline,
        );
        if geom_cache_dirty(self.cache.as_ref().map(|c| &c.key), &key) {
            self.cache = Some(build_cache(&self.diff, &key, self.outline));
        }
        let min_area_nm2 =
            self.min_area_mm2 * etchy_core::NM_PER_MM as f64 * etchy_core::NM_PER_MM as f64;
        // Per-layer base/context colour resolver (#21) — used for both the stacked
        // overlay and the split halves.
        let theme = self.theme;
        let base_overrides = &self.base_overrides;
        let layers = &self.diff.layers;
        // Guard the outline sentinel (NO_LAYER): the Side::Full outline item isn't
        // tied to a real layer, so never index `layers` with it. The Split/Swipe
        // render path runs base_of over every item (outline included) and would panic
        // on `layers[usize::MAX]` (#61 swipe froze the app here). The outline is drawn
        // C_OUTLINE_FAINT regardless, so the returned colour is unused for it.
        let base_of = |li: usize| {
            if li == NO_LAYER {
                C_BASE
            } else {
                resolve_base_color(li, layers[li].kind, base_overrides, theme)
            }
        };
        let cache = self.cache.as_ref().expect("cache built above");
        let n;
        if self.mode == Mode::Split || self.mode == Mode::Swipe {
            // Side-by-side / curtain: old (left) and new (right), one shared camera,
            // each clipped to its side so geometry can't bleed past the divider (G4,
            // #61). Split fixes the boundary at 50/50 with a gutter; Swipe puts it at
            // the draggable `swipe_frac` with no gutter (seamless wipe).
            let swipe = self.mode == Mode::Swipe;
            let (lr, rr, div_x) = if swipe {
                swipe_rects(rect, self.swipe_frac)
            } else {
                split_rects(rect, 0.5, 6.0)
            };
            // One mesh per side (not per item) → a single clipped draw per half,
            // matching the smooth non-split path instead of a painter per item.
            let mut lmesh = egui::epaint::Mesh::default();
            let mut rmesh = egui::epaint::Mesh::default();
            // Base boards honour the base-level (faint/strong dimming) — #44; the
            // board outline (Side::Full) is drawn into BOTH halves for orientation — #45.
            // Split shows the active layer only, so its base colour is per-item.
            for item in &cache.items {
                let base_col = base_display_color(
                    base_of(item.layer_index),
                    self.canvas_color(),
                    self.base_level,
                );
                match item.side {
                    Side::Left => append_tris(&mut lmesh, &item.tris, &self.cam, lr, base_col),
                    Side::Right => append_tris(&mut rmesh, &item.tris, &self.cam, rr, base_col),
                    Side::Full => {
                        append_tris(&mut lmesh, &item.tris, &self.cam, lr, C_OUTLINE_FAINT);
                        append_tris(&mut rmesh, &item.tris, &self.cam, rr, C_OUTLINE_FAINT);
                    }
                }
            }
            let (ln, rn) = (lmesh.vertices.len(), rmesh.vertices.len());
            if !lmesh.is_empty() {
                painter.with_clip_rect(lr).add(Shape::from(lmesh));
            }
            if !rmesh.is_empty() {
                painter.with_clip_rect(rr).add(Shape::from(rmesh));
            }
            // The divider: a copper wipe line. In Swipe it's the draggable handle —
            // drawn a touch heavier, with grab pips, so it reads as movable.
            painter.line_segment(
                [
                    Pos2::new(div_x, rect.top()),
                    Pos2::new(div_x, rect.bottom()),
                ],
                Stroke::new(if swipe { 2.5 } else { 1.5 }, C_COPPER),
            );
            if swipe {
                // A small grab handle at mid-height so the divider reads as draggable.
                let mid_y = rect.center().y;
                for dy in [-14.0, 0.0, 14.0] {
                    painter.circle_filled(Pos2::new(div_x, mid_y + dy), 2.5, C_COPPER);
                }
            }
            // Labels at each half's BOTTOM-left so they don't collide with the
            // top-left per-layer caption — #48.
            for (r, txt) in [
                (lr, format!("{} (old)", self.old_label)),
                (rr, format!("{} (new)", self.new_label)),
            ] {
                painter.text(
                    r.left_bottom() + egui::vec2(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    txt,
                    egui::FontId::proportional(13.0),
                    C_COPPER,
                );
            }
            self.last_hidden = 0;
            n = ln + rn;
        } else {
            let (shapes, hidden) = transform_cache(
                cache,
                &self.cam,
                rect,
                self.base_level,
                self.selected,
                base_of,
                self.canvas_color(),
                self.col_added,
                self.col_removed,
                min_area_nm2,
            );
            self.last_hidden = hidden;
            n = shapes.len();
            painter.extend(shapes);
        }

        // Empty-state hint.
        if n == 0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "no geometry in this view",
                egui::FontId::proportional(16.0),
                Color32::GRAY,
            );
        }

        // In Before/After the whole board is drawn in its layer colour (not the
        // green "added") — say so, so it's not mistaken for the diff (#3).
        let mode_note = match self.mode {
            Mode::Before => Some("showing OLD board (before)"),
            Mode::After => Some("showing NEW board (after)"),
            _ => None,
        };
        if let Some(note) = mode_note {
            painter.text(
                rect.right_top() + egui::vec2(-8.0, 8.0),
                egui::Align2::RIGHT_TOP,
                note,
                egui::FontId::proportional(13.0),
                C_COPPER,
            );
        }

        // Per-layer caption + a tiny legend.
        let mut cap = format!(
            "{}  —  {}   (+{} / −{} regions)",
            layer.name(),
            status_str(layer.status),
            layer.change.added_region_count,
            layer.change.removed_region_count,
        );
        if self.mode == Mode::Overlay && self.min_area_mm2 > 0.0 && self.last_hidden > 0 {
            cap.push_str(&format!(
                "   ·   {} hidden < {:.4} mm²",
                self.last_hidden, self.min_area_mm2
            ));
        }
        painter.text(
            rect.left_top() + egui::vec2(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            cap,
            egui::FontId::proportional(14.0),
            Color32::from_gray(200),
        );
        if self.mode == Mode::Overlay {
            let outline_row =
                outline_legend_visible(self.show_outline, self.outline, self.selected);
            legend(
                &painter,
                rect,
                self.col_added,
                self.col_removed,
                outline_row,
            );
        }

        // Measure tool overlay (#22/#50): crosshairs at the cursor, the ruler points,
        // the segment, and a sticky distance label offset off the line.
        if self.measure_mode {
            // World [f64;2] → screen, matching world_to_screen's float transform.
            let w2s = |w: [f64; 2]| -> Pos2 {
                let x = rect.center().x as f64 + (w[0] - self.cam.center[0]) * self.cam.scale;
                let y = rect.center().y as f64 - (w[1] - self.cam.center[1]) * self.cam.scale;
                Pos2::new(x as f32, y as f32)
            };
            // Crosshairs at the hover position to aid alignment (#50). Faint, clipped
            // to the canvas rect (full width + full height through the cursor).
            // With snap on (#52, fix #3) the crosshair locks LIVE to the nearest
            // grid intersection as the mouse moves, so the user sees where the next
            // click will land; the placed point just follows this snapped cursor.
            if let Some(ptr) = response.hover_pos() {
                let cross_at = if self.snap_grid {
                    let w = screen_to_world(&self.cam, ptr, rect);
                    w2s(snap_world_to_grid(w, self.grid_mm))
                } else {
                    ptr
                };
                let cross = Stroke::new(1.0, C_CROSSHAIR);
                painter.line_segment(
                    [
                        Pos2::new(rect.left(), cross_at.y),
                        Pos2::new(rect.right(), cross_at.y),
                    ],
                    cross,
                );
                painter.line_segment(
                    [
                        Pos2::new(cross_at.x, rect.top()),
                        Pos2::new(cross_at.x, rect.bottom()),
                    ],
                    cross,
                );
            }
            for w in &self.measure_pts {
                painter.circle_filled(w2s(*w), 3.0, C_COPPER);
            }
            if self.measure_pts.len() == 2 {
                let (a, b) = (self.measure_pts[0], self.measure_pts[1]);
                let (sa, sb) = (w2s(a), w2s(b));
                painter.line_segment([sa, sb], Stroke::new(1.5, C_COPPER));
                // Label OFF the line (#50): offset ~14 px perpendicular to the
                // segment, on a filled copper chip with dark text for legibility.
                let mid = Pos2::new((sa.x + sb.x) / 2.0, (sa.y + sb.y) / 2.0);
                let (dx, dy) = (sb.x - sa.x, sb.y - sa.y);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let off = egui::vec2(-dy / len, dx / len) * 14.0;
                let dist_mm = distance_mm(a, b);
                measure_label(
                    &painter,
                    mid + off,
                    &format_distance(dist_mm, self.measure_unit),
                );
            }
            // Hint at the bottom-left.
            painter.text(
                rect.left_bottom() + egui::vec2(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                "measure: click two points · U units · Esc clears · toggle off to exit",
                egui::FontId::proportional(12.0),
                C_COPPER,
            );
        }

        // Keep a border.
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, Color32::from_gray(60)),
            StrokeKind::Inside,
        );
    }
}

/// What a cached item is, so the per-frame pass knows how to colour it (G6).
#[derive(Clone, Copy, PartialEq)]
enum Role {
    Base,
    Outline,
    Added,
    Removed,
}

/// One triangulated draw item, in WORLD space (camera-independent). For diff items
/// `extent_nm`/`area_nm2` drive the per-frame LOD fade + min-area cull without
/// re-triangulating; base/outline leave them 0.
struct CachedItem {
    role: Role,
    side: Side,
    /// Source layer index in `diff.layers` (#59) — drives the highlight/dim: items
    /// from a layer other than the selected one draw at reduced alpha. The outline
    /// reference isn't tied to one layer, so it uses `usize::MAX` (never dimmed).
    layer_index: usize,
    tris: Vec<[Pt; 3]>,
    /// World bbox [minx, miny, maxx, maxy] — for off-screen culling per frame.
    bbox: [i64; 4],
    extent_nm: i64,
    area_nm2: f64,
}

/// Sentinel `layer_index` for items not tied to a single layer (the board outline
/// reference); they're never dimmed by the highlight/dim pass.
const NO_LAYER: usize = usize::MAX;

/// World bbox of a ring as [minx, miny, maxx, maxy].
fn ring_bbox(ring: &[Pt]) -> [i64; 4] {
    let mut bb = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for p in ring {
        bb[0] = bb[0].min(p.x);
        bb[1] = bb[1].min(p.y);
        bb[2] = bb[2].max(p.x);
        bb[3] = bb[3].max(p.y);
    }
    bb
}

/// The tessellation cache: world-space items valid for one [`GeomKey`].
struct TessCache {
    key: GeomKey,
    items: Vec<CachedItem>,
}

fn push_context_items(
    items: &mut Vec<CachedItem>,
    set: &PolygonSet,
    role: Role,
    side: Side,
    layer_index: usize,
) {
    for shape in &set.shapes {
        let Some(outer) = shape.first() else { continue };
        if outer.len() < 3 {
            continue;
        }
        // Thickness data (#9/#10): lets transform_cache LOD-cull sub-pixel base
        // features when zoomed out, so a dense multi-layer view stays fast. Same
        // area/extent the diff path computes.
        let bb = ring_bbox(outer);
        let extent_nm = (bb[2] - bb[0]).max(bb[3] - bb[1]);
        let area_nm2 = lod::ring_area_nm2(outer);
        let tris = etchy_core::triangulate_shape(shape);
        if !tris.is_empty() {
            items.push(CachedItem {
                role,
                side,
                layer_index,
                tris,
                bbox: bb,
                extent_nm,
                area_nm2,
            });
        }
    }
}

fn push_diff_items(items: &mut Vec<CachedItem>, set: &PolygonSet, role: Role, layer_index: usize) {
    for shape in &set.shapes {
        let Some(outer) = shape.first() else { continue };
        if outer.len() < 3 {
            continue;
        }
        let area_nm2 = lod::ring_area_nm2(outer);
        let bb = ring_bbox(outer);
        let extent_nm = (bb[2] - bb[0]).max(bb[3] - bb[1]);
        let tris = etchy_core::triangulate_shape(shape);
        if !tris.is_empty() {
            items.push(CachedItem {
                role,
                side: Side::Full,
                layer_index,
                tris,
                bbox: bb,
                extent_nm,
                area_nm2,
            });
        }
    }
}

/// Triangulate the geometry selected by `key` into world-space items, once.
///
/// Multi-layer (#58/#59): every VISIBLE layer is triangulated and pushed, each item
/// tagged with its source layer so the per-frame pass can highlight the selected
/// layer and dim the rest. Split shows the active (selected) layer only — a
/// stacked old|new of many layers reads as mud — so it never grows past one layer.
///
/// Perf note (#59 MVP): a board with many large visible layers merges into one big
/// mesh. The per-frame off-screen cull keeps draw cheap, but there's no adaptive
/// LOD across the merged set yet — a known follow-up if very dense packs stutter.
fn build_cache(diff: &BoardDiff, key: &GeomKey, outline: Option<usize>) -> TessCache {
    let mut items = Vec::new();
    // Outline first (drawn underneath), in all modes (G10). Not tied to a layer, so
    // it never dims.
    if key.outline_effective {
        if let Some(oi) = outline {
            let lo = &diff.layers[oi];
            let set = if !lo.new.shapes.is_empty() {
                &lo.new
            } else {
                &lo.old
            };
            push_context_items(&mut items, set, Role::Outline, Side::Full, NO_LAYER);
        }
    }
    if key.mode == Mode::Split || key.mode == Mode::Swipe {
        // Split & Swipe: active layer only (the selected layer is the first visible
        // one the shell records; see draw_canvas). Single-layer old|new view; only
        // the on-screen clip boundary differs (fixed vs draggable divider) — #61.
        if let Some(&li) = key.visible.first() {
            let layer = &diff.layers[li];
            if key.base_on {
                push_context_items(&mut items, &layer.old, Role::Base, Side::Left, li);
                push_context_items(&mut items, &layer.new, Role::Base, Side::Right, li);
            }
        }
        return TessCache {
            key: key.clone(),
            items,
        };
    }
    // Overlay / Before / After: stack every visible layer.
    for &li in &key.visible {
        let layer = &diff.layers[li];
        match key.mode {
            Mode::Before => push_context_items(&mut items, &layer.old, Role::Base, Side::Full, li),
            Mode::After => push_context_items(&mut items, &layer.new, Role::Base, Side::Full, li),
            Mode::Overlay => {
                if key.base_on {
                    push_context_items(&mut items, &layer.new, Role::Base, Side::Full, li);
                }
                push_diff_items(&mut items, &layer.removed, Role::Removed, li);
                push_diff_items(&mut items, &layer.added, Role::Added, li);
            }
            Mode::Split | Mode::Swipe => unreachable!("split/swipe handled above"),
        }
    }
    TessCache {
        key: key.clone(),
        items,
    }
}

/// Alpha multiplier for visible-but-not-selected layers (#59, Altium "dim"
/// default). The selected (active) layer stays full opacity so it reads on top.
const DIM_ALPHA: f32 = 0.4;

/// Opacity multiplier for an item from `layer_index` given the active `selected`
/// layer: 1.0 for the selected layer (and for layer-less items like the outline),
/// `DIM_ALPHA` for the other visible layers (#59).
fn dim_factor(layer_index: usize, selected: usize) -> f32 {
    if layer_index == NO_LAYER || layer_index == selected {
        1.0
    } else {
        DIM_ALPHA
    }
}

/// Per-frame: transform cached world items to screen meshes, applying colour, the
/// highlight/dim (#59), the LOD fade (diff only), and the min-area cull (returns the
/// hidden count). No triangulation here — this is the cheap part that runs every
/// frame. `base_of` resolves each layer's base/context colour by its index, so
/// stacked layers read by their own colour.
#[allow(clippy::too_many_arguments)]
fn transform_cache(
    cache: &TessCache,
    cam: &Camera,
    rect: Rect,
    base_level: BaseLevel,
    selected: usize,
    base_of: impl Fn(usize) -> Color32,
    canvas: Color32,
    col_added: Color32,
    col_removed: Color32,
    min_area_nm2: f64,
) -> (Vec<Shape>, usize) {
    // Merge everything into ONE mesh (per-vertex colour preserves the LOD fade)
    // instead of one Mesh+Shape per region — the FMU top-copper layer was ~5.5k
    // mesh allocations per frame; this makes it one. Off-screen items are culled
    // before their vertices are built (cheaper when zoomed in). Items are pushed
    // base → outline → diff, so draw order within the single mesh stays correct.
    let mut mesh = egui::epaint::Mesh::default();
    let mut hidden = 0usize;
    for item in &cache.items {
        if !bbox_visible(item.bbox, cam, rect) {
            continue; // off-screen: not a threshold "hidden", just nothing to draw
        }
        // Adaptive LOD for base copper (#9/#10): a base feature thinner than a pixel
        // on screen is invisible, so don't transform it. Large planes/pads (big
        // area/extent) stay; thin sub-pixel traces drop out when zoomed out, which is
        // exactly where dense multi-layer views were slow. The outline reference
        // (Role::Outline) is deliberately never culled, and diff items keep their own
        // marker-dot LOD below.
        if item.role == Role::Base {
            let thickness = feature_thickness_nm(item.area_nm2, item.extent_nm);
            if region_screen_px(thickness, cam.scale) < LOD_LO_PX {
                continue;
            }
        }
        // Highlight/dim (#59): the active layer at full opacity, the other visible
        // layers dimmed; layer-less items (outline) never dim.
        let dim = dim_factor(item.layer_index, selected);
        let (mut color, is_diff) = match item.role {
            Role::Base => (
                base_display_color(base_of(item.layer_index), canvas, base_level),
                false,
            ),
            Role::Outline => (C_OUTLINE_FAINT, false),
            Role::Added => (col_added, true),
            Role::Removed => (col_removed, true),
        };
        if !is_diff && dim < 1.0 {
            // Dim the base/context layers by scaling their existing alpha.
            color = with_alpha(color, (color.a() as f32 / 255.0) * dim);
        }
        if is_diff {
            // Fade by on-screen THICKNESS, not extent: a long thin crescent has a
            // large extent but is sub-pixel thick — fading by thickness stops it
            // flickering by position when panned (#46).
            let thickness = feature_thickness_nm(item.area_nm2, item.extent_nm);
            let px = region_screen_px(thickness, cam.scale);
            // The kernel keeps the absolute-min cull (genuine noise) separate from
            // the screen-px LOD: a real diff below LOD_LO_PX draws as a fixed marker
            // dot instead of vanishing (the #14 phantom/all-green-when-zoomed-out fix).
            match lod::lod_render(px, LOD_LO_PX, LOD_HI_PX, item.area_nm2 < min_area_nm2) {
                lod::Lod::Cull => {
                    hidden += 1;
                    continue;
                }
                lod::Lod::Marker => {
                    // A fixed-size dot at the region centre so the diff stays visible
                    // at every zoom. Centre comes from the cached world bbox.
                    let cx = (item.bbox[0] + item.bbox[2]) / 2;
                    let cy = (item.bbox[1] + item.bbox[3]) / 2;
                    let at = world_to_screen(cam, Pt::new(cx, cy), rect);
                    push_screen_quad(&mut mesh, at, MARKER_PX, with_alpha(color, dim));
                    continue;
                }
                // Combine the LOD fade with the per-layer dim so a non-selected
                // layer's diffs sit behind the active layer's.
                lod::Lod::Fade(alpha) => color = with_alpha(color, alpha * dim),
            }
        }
        for tri in &item.tris {
            let base = mesh.vertices.len() as u32;
            for &p in tri {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: world_to_screen(cam, p, rect),
                    uv: egui::epaint::WHITE_UV,
                    color,
                });
            }
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    }
    let shapes = if mesh.is_empty() {
        Vec::new()
    } else {
        vec![Shape::from(mesh)]
    };
    (shapes, hidden)
}

/// Append a cached item's world triangles to `mesh`, transformed into `target` and
/// tinted `color`. Shared by the split path (which draws into two half-rects).
fn append_tris(
    mesh: &mut egui::epaint::Mesh,
    tris: &[[Pt; 3]],
    cam: &Camera,
    target: Rect,
    color: Color32,
) {
    for tri in tris {
        let b = mesh.vertices.len() as u32;
        for &p in tri {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: world_to_screen(cam, p, target),
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.indices.extend_from_slice(&[b, b + 1, b + 2]);
    }
}

/// Is a world bbox at all on-screen? Transforms its corners and tests the screen
/// AABB against the canvas rect — used to cull fully-off-screen items per frame.
fn bbox_visible(bbox: [i64; 4], cam: &Camera, rect: Rect) -> bool {
    let corners = [
        Pt::new(bbox[0], bbox[1]),
        Pt::new(bbox[2], bbox[1]),
        Pt::new(bbox[0], bbox[3]),
        Pt::new(bbox[2], bbox[3]),
    ];
    let mut lo = Pos2::new(f32::INFINITY, f32::INFINITY);
    let mut hi = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in corners {
        let s = world_to_screen(cam, c, rect);
        lo.x = lo.x.min(s.x);
        lo.y = lo.y.min(s.y);
        hi.x = hi.x.max(s.x);
        hi.y = hi.y.max(s.y);
    }
    Rect::from_min_max(lo, hi).intersects(rect)
}

/// Scale a colour's opacity by `a` (clamped to [0,1]).
fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Push a fixed-size, axis-aligned square (two triangles) centred at `at` in screen
/// space into `mesh` — the LOD marker dot for a sub-pixel diff region (#14).
fn push_screen_quad(mesh: &mut egui::epaint::Mesh, at: Pos2, px: f32, color: Color32) {
    let h = px * 0.5;
    let corners = [
        Pos2::new(at.x - h, at.y - h),
        Pos2::new(at.x + h, at.y - h),
        Pos2::new(at.x + h, at.y + h),
        Pos2::new(at.x - h, at.y + h),
    ];
    let base = mesh.vertices.len() as u32;
    for pos in corners {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn legend(
    painter: &egui::Painter,
    rect: Rect,
    added: Color32,
    removed: Color32,
    outline_row: bool,
) {
    let mut y = rect.right_top() + egui::vec2(-150.0, 8.0);
    let mut rows = vec![(added, "added"), (removed, "removed")];
    if outline_row {
        rows.push((C_OUTLINE_FAINT, "board edge"));
    }
    for (c, txt) in rows {
        painter.rect_filled(Rect::from_min_size(y, egui::vec2(12.0, 12.0)), 2.0, c);
        painter.text(
            y + egui::vec2(18.0, 6.0),
            egui::Align2::LEFT_CENTER,
            txt,
            egui::FontId::proportional(13.0),
            Color32::from_gray(200),
        );
        y.y += 18.0;
    }
}

fn status_str(s: etchy_core::LayerStatus) -> &'static str {
    use etchy_core::LayerStatus::*;
    match s {
        Unchanged => "unchanged",
        Changed => "changed",
        AddedLayer => "added layer",
        RemovedLayer => "removed layer",
    }
}

// ---- coordinate transforms (world nm <-> screen px) ----

fn world_to_screen(cam: &Camera, p: Pt, rect: Rect) -> Pos2 {
    let x = rect.center().x as f64 + (p.x as f64 - cam.center[0]) * cam.scale;
    let y = rect.center().y as f64 - (p.y as f64 - cam.center[1]) * cam.scale; // flip y
    Pos2::new(x as f32, y as f32)
}

/// Euclidean distance between two world-space points (nm), returned in mm — the
/// pure kernel behind the measure tool (#22). World coords are nm, so the raw
/// distance is nm; divide by `NM_PER_MM` for the mm the label shows.
fn distance_mm(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    (dx * dx + dy * dy).sqrt() / etchy_core::NM_PER_MM as f64
}

/// Unit the measure tool reports distances in (#50). Cycles mm → inch → mil.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Mm,
    Inch,
    Mil,
}

impl Unit {
    /// Next unit in the cycle (mm → inch → mil → mm), for the hotkey/toggle.
    fn next(self) -> Unit {
        match self {
            Unit::Mm => Unit::Inch,
            Unit::Inch => Unit::Mil,
            Unit::Mil => Unit::Mm,
        }
    }

    /// Short label for the controls-row toggle.
    fn label(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Inch => "in",
            Unit::Mil => "mil",
        }
    }
}

/// Format a millimetre distance for display in the chosen unit (#50) — the pure
/// kernel behind the measure label. 25.4 mm == 1 in == 1000 mil. Decimals are
/// tuned per unit so the precision is sensible at each scale.
fn format_distance(mm: f64, unit: Unit) -> String {
    match unit {
        Unit::Mm => format!("{mm:.3} mm"),
        Unit::Inch => format!("{:.4} in", mm / 25.4),
        Unit::Mil => format!("{:.1} mil", mm / 25.4 * 1000.0),
    }
}

/// Snap a millimetre coordinate to the nearest grid multiple (#51) — the pure
/// kernel behind snap-to-grid. `grid_mm <= 0` (or non-finite) leaves it unchanged.
/// Rounds half away from zero so the behaviour is symmetric across the origin.
fn snap_to_grid_mm(coord_mm: f64, grid_mm: f64) -> f64 {
    if !grid_mm.is_finite() || grid_mm <= 0.0 {
        return coord_mm;
    }
    (coord_mm / grid_mm).round() * grid_mm
}

/// Snap a world point (nm) to the nearest grid intersection (#51/#52). World
/// coords are nm; snap in mm then convert back. `grid_mm <= 0` leaves it as-is.
fn snap_world_to_grid(w: [f64; 2], grid_mm: f64) -> [f64; 2] {
    let mm = etchy_core::NM_PER_MM as f64;
    [
        snap_to_grid_mm(w[0] / mm, grid_mm) * mm,
        snap_to_grid_mm(w[1] / mm, grid_mm) * mm,
    ]
}

fn screen_to_world(cam: &Camera, s: Pos2, rect: Rect) -> [f64; 2] {
    let wx = cam.center[0] + (s.x - rect.center().x) as f64 / cam.scale;
    let wy = cam.center[1] - (s.y - rect.center().y) as f64 / cam.scale;
    [wx, wy]
}

/// Draw the reference grid (#51) at `grid_mm` world spacing across the canvas.
/// Skips drawing if the on-screen spacing would be < 6 px (too dense → solid fill).
fn draw_grid(painter: &egui::Painter, cam: &Camera, rect: Rect, grid_mm: f64, grid_color: Color32) {
    if !grid_mm.is_finite() || grid_mm <= 0.0 {
        return;
    }
    let step_nm = grid_mm * etchy_core::NM_PER_MM as f64;
    let px_per_line = step_nm * cam.scale; // screen px between adjacent grid lines
    if !px_per_line.is_finite() || px_per_line < 6.0 {
        return;
    }
    let stroke = Stroke::new(1.0, grid_color);
    // World coords visible at the rect edges (y is flipped on screen).
    let left = screen_to_world(cam, Pos2::new(rect.left(), rect.center().y), rect)[0];
    let right = screen_to_world(cam, Pos2::new(rect.right(), rect.center().y), rect)[0];
    let bottom = screen_to_world(cam, Pos2::new(rect.center().x, rect.bottom()), rect)[1];
    let top = screen_to_world(cam, Pos2::new(rect.center().x, rect.top()), rect)[1];
    // Vertical lines at each grid X within view.
    let mut i = (left / step_nm).ceil() as i64;
    while (i as f64 * step_nm) <= right {
        let sx = rect.center().x + ((i as f64 * step_nm - cam.center[0]) * cam.scale) as f32;
        painter.line_segment(
            [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
            stroke,
        );
        i += 1;
    }
    // Horizontal lines at each grid Y within view.
    let mut j = (bottom / step_nm).ceil() as i64;
    while (j as f64 * step_nm) <= top {
        let sy = rect.center().y - ((j as f64 * step_nm - cam.center[1]) * cam.scale) as f32;
        painter.line_segment(
            [Pos2::new(rect.left(), sy), Pos2::new(rect.right(), sy)],
            stroke,
        );
        j += 1;
    }
}

/// Draw the measure distance label as text on a filled copper chip with dark text
/// (#50), centred at `at` — legible instead of bare text over the copper line.
fn measure_label(painter: &egui::Painter, at: Pos2, text: &str) {
    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, C_CANVAS);
    let pad = egui::vec2(5.0, 3.0);
    let rect = Rect::from_center_size(at, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 3.0, C_COPPER);
    painter.galley(rect.min + pad, galley, C_CANVAS);
}

fn fit(cam: &mut Camera, bb: [i64; 4], rect: Rect) {
    let (w, h) = ((bb[2] - bb[0]) as f64, (bb[3] - bb[1]) as f64);
    cam.center = [
        (bb[0] as f64 + bb[2] as f64) / 2.0,
        (bb[1] as f64 + bb[3] as f64) / 2.0,
    ];
    let sx = if w > 0.0 {
        rect.width() as f64 / w
    } else {
        1.0
    };
    let sy = if h > 0.0 {
        rect.height() as f64 / h
    } else {
        1.0
    };
    cam.scale = 0.9 * sx.min(sy);
    if !cam.scale.is_finite() || cam.scale <= 0.0 {
        cam.scale = 1e-6;
    }
    cam.fit_scale = cam.scale; // "100%" reference for the zoom readout
}

/// Publish the current view (selected layer, mode, zoom %) to JS so the web
/// feedback widget can attach "what the user was looking at" to a submission.
/// `window.__etchyState = { layer, mode, zoomPct }`. No-op off the web.
#[cfg(target_arch = "wasm32")]
fn publish_state(layer: &str, mode: &str, zoom_pct: i32) {
    use eframe::wasm_bindgen::JsValue;
    if let Some(win) = web_sys::window() {
        let obj = js_sys::Object::new();
        let set = |k: &str, v: &JsValue| {
            let _ = js_sys::Reflect::set(&obj, &JsValue::from_str(k), v);
        };
        set("layer", &JsValue::from_str(layer));
        set("mode", &JsValue::from_str(mode));
        set("zoomPct", &JsValue::from_f64(zoom_pct as f64));
        let _ = js_sys::Reflect::set(&win, &JsValue::from_str("__etchyState"), &obj);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_state(_layer: &str, _mode: &str, _zoom_pct: i32) {}

/// Union bbox of a layer's old+new geometry (so the view frames the whole board).
fn layer_bbox(layer: &LayerView) -> Option<[i64; 4]> {
    match (layer.old.bbox_nm(), layer.new.bbox_nm()) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        base_display_color, build_geom_key, cycle_base, distance_mm, geom_cache_dirty,
        group_all_visible, group_layers, layer_group, outline_legend_visible, pans_on,
        pick_outline_index, region_screen_px, restore_visibility, scroll_to_camera_action,
        set_group_visibility, short_layer_name, step_in_order, visible_from_changed,
        visible_indices, warning_phase, BaseLevel, CameraAction, InputPreset, LayerGroup, Mode,
        Theme, WarningPhase,
    };
    use etchy_core::LayerKind;

    #[test]
    fn distance_mm_is_a_3_4_5_triangle() {
        // World coords are nm. A 3 mm / 4 mm leg pair → 5 mm hypotenuse.
        let mm = etchy_core::NM_PER_MM as f64;
        let a = [0.0, 0.0];
        let b = [3.0 * mm, 4.0 * mm];
        assert!((distance_mm(a, b) - 5.0).abs() < 1e-9, "3-4-5 → 5 mm");
        // Symmetric, and zero for a point on itself.
        assert!((distance_mm(b, a) - 5.0).abs() < 1e-9);
        assert_eq!(distance_mm(a, a), 0.0);
    }

    #[test]
    fn chrome_dark_and_light_differ() {
        use super::{chrome, Theme, C_CANVAS, C_SURFACE};
        let d = chrome(Theme::Dark);
        assert_eq!(d.canvas, C_CANVAS);
        assert_eq!(d.surface, C_SURFACE);
        assert_ne!(d.canvas, d.surface); // chrome reads distinct from the board
        let l = chrome(Theme::Light);
        let lum = |c: egui::Color32| c.r() as u16 + c.g() as u16 + c.b() as u16;
        assert!(lum(l.canvas) > lum(d.canvas)); // light canvas is lighter
        assert!(lum(l.text) < lum(l.canvas)); // dark ink on a light board
    }

    #[test]
    fn base_display_color_dims_toward_canvas() {
        use super::{C_CANVAS, C_COPPER};
        // Off shows the canvas (base not drawn); Strong reads closer to the real
        // layer colour than Faint — both opaque so unchanged copper isn't black.
        assert_eq!(
            base_display_color(C_COPPER, C_CANVAS, BaseLevel::Off),
            C_CANVAS
        );
        let faint = base_display_color(C_COPPER, C_CANVAS, BaseLevel::Faint);
        let strong = base_display_color(C_COPPER, C_CANVAS, BaseLevel::Strong);
        assert!(strong.r() > faint.r());
        assert!(faint.r() > C_CANVAS.r()); // even faint is visibly above the black canvas
        assert_eq!(strong.a(), 255); // opaque
    }

    #[test]
    fn geom_key_tracks_selection_inputs_only() {
        let base = build_geom_key(&[0], Mode::Overlay, BaseLevel::Faint, true, Some(1));
        // base Faint vs Strong is a colour, not geometry -> same key (no rebuild)
        assert_eq!(
            base,
            build_geom_key(&[0], Mode::Overlay, BaseLevel::Strong, true, Some(1))
        );
        // base Off flips base_on -> different key (the base mesh joins/leaves the draw)
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Overlay, BaseLevel::Off, true, Some(1))
        );
        // visible set / mode changes -> different key
        assert_ne!(
            base,
            build_geom_key(&[2], Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Before, BaseLevel::Faint, true, Some(1))
        );
        // With multiple layers shown, the outline still draws (it's enabled and
        // exists), so a visible-set change is what flips the key.
        assert_ne!(
            base,
            build_geom_key(&[0, 1], Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
    }

    #[test]
    fn geom_cache_dirty_on_none_or_change() {
        let k = build_geom_key(&[0], Mode::Overlay, BaseLevel::Faint, false, None);
        assert!(geom_cache_dirty(None, &k));
        assert!(!geom_cache_dirty(Some(&k), &k));
        let k2 = build_geom_key(&[2], Mode::Overlay, BaseLevel::Faint, false, None);
        assert!(geom_cache_dirty(Some(&k), &k2));
    }

    #[test]
    fn region_screen_px_scales_extent() {
        assert_eq!(region_screen_px(1000, 0.5), 500.0);
        assert_eq!(region_screen_px(0, 2.0), 0.0);
    }

    #[test]
    fn marker_quad_is_a_fixed_size_square() {
        use super::push_screen_quad;
        use egui::{pos2, Color32};
        let mut mesh = egui::epaint::Mesh::default();
        push_screen_quad(&mut mesh, pos2(10.0, 20.0), 4.0, Color32::RED);
        // Two triangles, four shared corners centred on `at`, side = px.
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.vertices.len(), 4);
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.x).collect();
        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.y).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 8.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 12.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 18.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 22.0);
    }

    #[test]
    fn feature_thickness_is_area_over_extent() {
        use super::feature_thickness_nm;
        // 1000 x 100 rect: area 100000, extent 1000 -> thickness ~100
        assert_eq!(feature_thickness_nm(100_000.0, 1000), 100);
        // a long thin crescent (10000 x 10): same area, extent 10000 -> thickness ~10,
        // so LOD treats it as THIN (it fades) rather than as a big feature by extent.
        assert_eq!(feature_thickness_nm(100_000.0, 10_000), 10);
        assert_eq!(feature_thickness_nm(5.0, 0), 0); // guard
    }

    #[test]
    fn split_rects_halves_with_a_divider() {
        use super::{clamp_split_frac, split_rects};
        use egui::{pos2, Rect};
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        let (l, rr, div) = split_rects(r, 0.5, 0.0);
        assert_eq!(div, 50.0);
        assert_eq!(l.left(), 0.0);
        assert_eq!(l.right(), 50.0);
        assert_eq!(rr.left(), 50.0);
        assert_eq!(rr.right(), 100.0);
        // frac is clamped so a side never vanishes
        assert_eq!(clamp_split_frac(0.0), 0.1);
        assert_eq!(clamp_split_frac(1.0), 0.9);
        assert_eq!(clamp_split_frac(0.5), 0.5);
    }

    #[test]
    fn swipe_rects_split_at_the_divider_with_no_gutter() {
        use super::{clamp_swipe_frac, swipe_rects};
        use egui::{pos2, Rect};
        let r = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
        // The two sides meet exactly at the divider — no gutter, one wipe line.
        let (l, rr, div) = swipe_rects(r, 0.25);
        assert_eq!(div, 25.0);
        assert_eq!(l.left(), 0.0);
        assert_eq!(l.right(), 25.0);
        assert_eq!(rr.left(), 25.0);
        assert_eq!(rr.right(), 100.0);
        // The clip rects keep the full canvas height.
        assert_eq!(l.top(), 0.0);
        assert_eq!(l.bottom(), 50.0);
        assert_eq!(rr.top(), 0.0);
        assert_eq!(rr.bottom(), 50.0);
        // frac is clamped to [0.1, 0.9] so neither side ever vanishes.
        assert_eq!(clamp_swipe_frac(0.0), 0.1);
        assert_eq!(clamp_swipe_frac(1.0), 0.9);
        assert_eq!(clamp_swipe_frac(0.5), 0.5);
        // A clamped frac drives the divider position too.
        let (_, _, div_lo) = swipe_rects(r, -1.0);
        assert_eq!(div_lo, 10.0);
        let (_, _, div_hi) = swipe_rects(r, 2.0);
        assert_eq!(div_hi, 90.0);
    }

    #[test]
    fn layer_type_color_is_distinct_per_family() {
        use super::{layer_type_color, Theme, C_CREAM};
        let d = Theme::Dark;
        assert_eq!(layer_type_color(LayerKind::BottomSilk, d), C_CREAM);
        // Copper kinds now default to distinct per-layer colours (#20): top vs
        // bottom differ, and inner coppers are distinct from both and each other.
        assert_ne!(
            layer_type_color(LayerKind::TopCopper, d),
            layer_type_color(LayerKind::BottomCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::InnerCopper(1), d),
            layer_type_color(LayerKind::TopCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::InnerCopper(1), d),
            layer_type_color(LayerKind::BottomCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::InnerCopper(1), d),
            layer_type_color(LayerKind::InnerCopper(2), d)
        );
        // copper kinds are distinct from mask and paste
        assert_ne!(
            layer_type_color(LayerKind::TopMask, d),
            layer_type_color(LayerKind::TopCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::TopMask, d),
            layer_type_color(LayerKind::BottomCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::TopPaste, d),
            layer_type_color(LayerKind::TopMask, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::TopPaste, d),
            layer_type_color(LayerKind::TopCopper, d)
        );
        // light-mode silk is darker than dark-mode silk so it reads on a cream board
        let lum = |c: egui::Color32| c.r() as u16 + c.g() as u16 + c.b() as u16;
        assert!(
            lum(layer_type_color(LayerKind::TopSilk, Theme::Light))
                < lum(layer_type_color(LayerKind::TopSilk, Theme::Dark))
        );
    }

    #[test]
    fn resolve_base_color_prefers_override() {
        use super::{layer_type_color, resolve_base_color, Theme};
        use egui::Color32;
        let d = Theme::Dark;
        // Overrides are keyed by the layer's index, not its kind (#21).
        let ovr = [(0usize, Color32::from_rgb(1, 2, 3))];
        assert_eq!(
            resolve_base_color(0, LayerKind::TopCopper, &ovr, d),
            Color32::from_rgb(1, 2, 3)
        );
        // a different index with the same kind falls back to the per-kind default
        assert_eq!(
            resolve_base_color(1, LayerKind::TopCopper, &ovr, d),
            layer_type_color(LayerKind::TopCopper, d)
        );
        // no override for this index -> the type default
        assert_eq!(
            resolve_base_color(2, LayerKind::TopSilk, &ovr, d),
            layer_type_color(LayerKind::TopSilk, d)
        );
    }

    #[test]
    fn scroll_maps_to_zoom_or_pan_by_modifier() {
        // plain wheel (y) = zoom
        assert!(matches!(
            scroll_to_camera_action(0.0, 10.0, false, false, 0.0015, 1.0),
            CameraAction::Zoom(_)
        ));
        // ctrl = vertical pan, shift = horizontal pan
        assert_eq!(
            scroll_to_camera_action(0.0, 10.0, true, false, 0.0015, 2.0),
            CameraAction::PanY(20.0)
        );
        assert_eq!(
            scroll_to_camera_action(0.0, 10.0, false, true, 0.0015, 2.0),
            CameraAction::PanX(20.0)
        );
        // Shift+wheel arriving on the X axis still pans (the bug we fixed)
        assert_eq!(
            scroll_to_camera_action(8.0, 0.0, false, true, 0.0015, 1.0),
            CameraAction::PanX(8.0)
        );
        // ctrl wins if both held
        assert!(matches!(
            scroll_to_camera_action(0.0, 5.0, true, true, 0.0015, 1.0),
            CameraAction::PanY(_)
        ));
        // no scroll = nothing
        assert_eq!(
            scroll_to_camera_action(0.0, 0.0, false, false, 0.0015, 1.0),
            CameraAction::None
        );
        // Ctrl+wheel that arrives on the X axis (axis-swapped) STILL pans — the bug
        // was the old code discarding it as None because Y was zero.
        assert_eq!(
            scroll_to_camera_action(8.0, 0.0, true, false, 0.0015, 1.0),
            CameraAction::PanY(8.0)
        );
    }

    #[test]
    fn wheel_points_normalises_native_and_web_to_a_close_notch() {
        use super::wheel_points;
        use egui::{vec2, MouseWheelUnit};
        // one native notch (1 line) and one web notch (~100 points) end up close,
        // so a physical notch zooms similarly on both surfaces.
        let native = wheel_points(MouseWheelUnit::Line, vec2(0.0, 1.0), 800.0).y;
        let web = wheel_points(MouseWheelUnit::Point, vec2(0.0, 100.0), 800.0).y;
        let ratio = native / web;
        assert!((0.8..=1.25).contains(&ratio), "ratio {ratio}");
        assert_eq!(
            wheel_points(MouseWheelUnit::Page, vec2(0.0, 1.0), 800.0).y,
            800.0
        );
    }

    #[test]
    fn cycle_base_rotates_off_faint_strong() {
        assert_eq!(cycle_base(BaseLevel::Off), BaseLevel::Faint);
        assert_eq!(cycle_base(BaseLevel::Faint), BaseLevel::Strong);
        assert_eq!(cycle_base(BaseLevel::Strong), BaseLevel::Off);
    }

    #[test]
    fn pick_outline_index_finds_the_first_outline_layer() {
        let kinds = [
            LayerKind::TopCopper,
            LayerKind::Outline,
            LayerKind::BottomCopper,
            LayerKind::Outline,
        ];
        assert_eq!(pick_outline_index(kinds.len(), |i| kinds[i]), Some(1));
        let none = [LayerKind::TopCopper, LayerKind::BottomCopper];
        assert_eq!(pick_outline_index(none.len(), |i| none[i]), None);
        assert_eq!(pick_outline_index(0, |_| LayerKind::Outline), None);
    }

    #[test]
    fn outline_legend_visible_only_when_shown_and_not_selected() {
        assert!(outline_legend_visible(true, Some(2), 1)); // shown, different layer
        assert!(!outline_legend_visible(true, Some(2), 2)); // viewing the outline itself
        assert!(!outline_legend_visible(false, Some(2), 1)); // hidden
        assert!(!outline_legend_visible(true, None, 1)); // no outline layer
    }

    #[test]
    fn warning_label_is_count_aware() {
        use super::warning_label;
        assert_eq!(warning_label(0), "");
        assert_eq!(warning_label(1), "1 warning");
        assert_eq!(warning_label(3), "3 warnings");
    }

    #[test]
    fn warning_phase_auto_hides_after_the_window() {
        let hide = 6.0;
        // collapsed by the user -> chip, regardless of time
        assert_eq!(
            warning_phase(100.0, Some(10.0), false, hide),
            WarningPhase::Chip
        );
        // expanded, not yet stamped -> expanded (caller stamps this frame)
        assert_eq!(
            warning_phase(100.0, None, true, hide),
            WarningPhase::Expanded
        );
        // expanded, just shown -> counting down
        assert_eq!(
            warning_phase(10.0, Some(10.0), true, hide),
            WarningPhase::Counting
        );
        // still within the window -> counting
        assert_eq!(
            warning_phase(15.9, Some(10.0), true, hide),
            WarningPhase::Counting
        );
        // window elapsed -> auto-hidden to chip
        assert_eq!(
            warning_phase(16.0, Some(10.0), true, hide),
            WarningPhase::Chip
        );
        assert_eq!(
            warning_phase(99.0, Some(10.0), true, hide),
            WarningPhase::Chip
        );
        // clock skew (negative elapsed) -> counting, never auto-hide early
        assert_eq!(
            warning_phase(9.0, Some(10.0), true, hide),
            WarningPhase::Counting
        );
    }

    #[test]
    fn short_layer_name_drops_the_group_suffix() {
        // With section headers, the row only needs the position within the group.
        assert_eq!(short_layer_name(LayerKind::TopCopper), "top");
        assert_eq!(short_layer_name(LayerKind::BottomMask), "bottom");
        assert_eq!(short_layer_name(LayerKind::InnerCopper(2)), "inner 2");
        assert_eq!(short_layer_name(LayerKind::Drill), "drill");
        assert_eq!(short_layer_name(LayerKind::Other), "other");
    }

    #[test]
    fn layer_group_maps_kinds_to_sections() {
        assert_eq!(layer_group(LayerKind::TopCopper), LayerGroup::Copper);
        assert_eq!(layer_group(LayerKind::InnerCopper(2)), LayerGroup::Copper);
        assert_eq!(layer_group(LayerKind::BottomMask), LayerGroup::Mask);
        assert_eq!(layer_group(LayerKind::TopSilk), LayerGroup::Silk);
        assert_eq!(layer_group(LayerKind::BottomPaste), LayerGroup::Paste);
        assert_eq!(layer_group(LayerKind::Drill), LayerGroup::Drill);
        assert_eq!(layer_group(LayerKind::Outline), LayerGroup::Mechanical);
        assert_eq!(layer_group(LayerKind::Other), LayerGroup::Other);
    }

    #[test]
    fn group_layers_sections_in_order_preserving_input_order() {
        // index -> group; `order` is changed-first.
        let groups_by_idx = [
            LayerGroup::Copper,
            LayerGroup::Silk,
            LayerGroup::Copper,
            LayerGroup::Drill,
        ];
        let order = [2usize, 0, 1, 3]; // changed-first
        let out = group_layers(&order, |i| groups_by_idx[i]);
        // Fixed section order (Copper before Silk before Drill); within Copper the
        // incoming order [2, 0] is preserved; empty sections are omitted.
        assert_eq!(
            out,
            vec![
                (LayerGroup::Copper, vec![2, 0]),
                (LayerGroup::Silk, vec![1]),
                (LayerGroup::Drill, vec![3]),
            ]
        );
    }

    #[test]
    fn step_in_order_wraps_both_ways() {
        // `order` is changed-first (indices into diff.layers), e.g. [3, 0, 2, 1].
        let order = [3usize, 0, 2, 1];
        // forward from the selected value 3 (pos 0) -> 0 (pos 1)
        assert_eq!(step_in_order(&order, 3, 1), 0);
        // forward past the end wraps to the front
        assert_eq!(step_in_order(&order, 1, 1), 3);
        // backward past the front wraps to the back
        assert_eq!(step_in_order(&order, 3, -1), 1);
        // a selected value not present in order starts from the front
        assert_eq!(step_in_order(&order, 99, 1), 0);
        // empty order is a no-op (returns the input)
        assert_eq!(step_in_order(&[], 5, 1), 5);
    }

    #[test]
    fn format_distance_per_unit() {
        use super::{format_distance, Unit};
        // 25.4 mm == 1 inch == 1000 mil — a known conversion.
        assert_eq!(format_distance(25.4, Unit::Mm), "25.400 mm");
        assert_eq!(format_distance(25.4, Unit::Inch), "1.0000 in");
        assert_eq!(format_distance(25.4, Unit::Mil), "1000.0 mil");
        // Zero in every unit.
        assert_eq!(format_distance(0.0, Unit::Mm), "0.000 mm");
        assert_eq!(format_distance(0.0, Unit::Inch), "0.0000 in");
        assert_eq!(format_distance(0.0, Unit::Mil), "0.0 mil");
    }

    #[test]
    fn snap_to_grid_rounds_to_nearest_multiple() {
        use super::snap_to_grid_mm;
        // Nearest multiple of the grid spacing.
        assert_eq!(snap_to_grid_mm(0.6, 0.5), 0.5);
        assert_eq!(snap_to_grid_mm(0.8, 0.5), 1.0);
        assert_eq!(snap_to_grid_mm(2.4, 1.0), 2.0);
        assert_eq!(snap_to_grid_mm(2.5, 1.0), 3.0); // round half up (away from zero)
                                                    // Negative coords snap symmetrically.
        assert_eq!(snap_to_grid_mm(-0.6, 0.5), -0.5);
        assert_eq!(snap_to_grid_mm(-0.8, 0.5), -1.0);
        // grid <= 0 leaves the coord unchanged (guard).
        assert_eq!(snap_to_grid_mm(1.234, 0.0), 1.234);
        assert_eq!(snap_to_grid_mm(1.234, -1.0), 1.234);
    }

    #[test]
    fn snap_world_to_grid_snaps_both_axes_in_nm() {
        use super::snap_world_to_grid;
        let mm = etchy_core::NM_PER_MM as f64;
        // 0.6 mm, 2.4 mm in nm; 1 mm grid → 1 mm, 2 mm.
        let snapped = snap_world_to_grid([0.6 * mm, 2.4 * mm], 1.0);
        assert_eq!(snapped, [1.0 * mm, 2.0 * mm]);
        // Half-mm grid, negative axis snaps symmetrically.
        let snapped = snap_world_to_grid([0.8 * mm, -0.6 * mm], 0.5);
        assert_eq!(snapped, [1.0 * mm, -0.5 * mm]);
        // grid <= 0 leaves the point unchanged (passes the guard through).
        let raw = [1.234 * mm, 5.678 * mm];
        assert_eq!(snap_world_to_grid(raw, 0.0), raw);
    }

    #[test]
    fn pans_on_matches_each_preset() {
        use egui::PointerButton::{Middle, Primary, Secondary};
        // KiCad: middle OR right, not primary.
        assert!(!pans_on(InputPreset::KiCad, Primary));
        assert!(pans_on(InputPreset::KiCad, Middle));
        assert!(pans_on(InputPreset::KiCad, Secondary));
        // Altium: right only.
        assert!(!pans_on(InputPreset::Altium, Primary));
        assert!(!pans_on(InputPreset::Altium, Middle));
        assert!(pans_on(InputPreset::Altium, Secondary));
        // Default preset is Altium (alphabetically first of the supported tools, #55).
        assert_eq!(InputPreset::default(), InputPreset::Altium);
    }

    #[test]
    fn input_preset_serde_round_trips() {
        for p in [InputPreset::KiCad, InputPreset::Altium] {
            let json = serde_json::to_string(&p).expect("serialize");
            let back: InputPreset = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(p, back);
        }
        // Unknown / legacy strings (incl. the removed "etchy") fall back to the
        // default, never error.
        assert_eq!(
            serde_json::from_str::<InputPreset>("\"etchy\"").expect("deserialize"),
            InputPreset::Altium,
        );
        assert_eq!(
            serde_json::from_str::<InputPreset>("\"bogus\"").expect("deserialize"),
            InputPreset::Altium,
        );
    }

    #[test]
    fn settings_serde_round_trips() {
        use super::{color_to_rgba, rgba_to_color, Settings};
        use egui::Color32;
        let s = Settings {
            theme: Theme::Light,
            base_level: BaseLevel::Strong,
            base_overrides: vec![(0, [1, 2, 3, 4]), (3, [255, 0, 128, 255])],
            min_area_mm2: 0.0123,
            col_added: [10, 20, 30, 255],
            col_removed: [200, 50, 60, 255],
            canvas_dark: [11, 15, 14, 255],
            canvas_light: [244, 241, 232, 255],
            grid_dark: [56, 39, 14, 60],
            grid_light: [138, 102, 34, 70],
            input_preset: InputPreset::KiCad,
            visible_layers: vec![0, 2, 5],
            swipe_frac: 0.42,
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
        // Color32 <-> [u8;4] is a faithful round trip.
        let c = Color32::from_rgba_unmultiplied(56, 39, 14, 60);
        assert_eq!(rgba_to_color(color_to_rgba(c)), c);
    }

    #[test]
    fn measure_escape_clears_then_exits() {
        use super::measure_escape;
        // In measure mode with an in-progress measurement: first Esc clears the
        // points but stays in measure mode (#50).
        assert_eq!(measure_escape(true, true), (true, true));
        // In measure mode with nothing to clear: a second Esc exits measure mode.
        assert_eq!(measure_escape(true, false), (false, false));
        // Not in measure mode: Esc is a no-op.
        assert_eq!(measure_escape(false, false), (false, false));
    }

    #[test]
    fn canvas_and_grid_colours_are_per_theme() {
        use super::ViewApp;
        use egui::Color32;
        let mut app = ViewApp::new(empty_diff(), "old".into(), "new".into());

        // Defaults differ by theme, so light mode is never a dark canvas (#31).
        app.theme = Theme::Dark;
        let dark_default = app.canvas_color();
        app.theme = Theme::Light;
        let light_default = app.canvas_color();
        assert_ne!(
            dark_default, light_default,
            "light canvas default must differ from dark"
        );

        // Editing the canvas in one theme must not bleed into the other (#31).
        app.theme = Theme::Dark;
        *app.canvas_color_mut() = Color32::from_rgb(1, 2, 3);
        app.theme = Theme::Light;
        assert_eq!(
            app.canvas_color(),
            light_default,
            "light canvas unchanged by a dark-mode edit"
        );
        *app.canvas_color_mut() = Color32::from_rgb(4, 5, 6);
        app.theme = Theme::Dark;
        assert_eq!(
            app.canvas_color(),
            Color32::from_rgb(1, 2, 3),
            "dark canvas kept its own edit"
        );

        // Grid colour is per-theme the same way.
        app.theme = Theme::Dark;
        *app.grid_color_mut() = Color32::from_rgb(7, 8, 9);
        app.theme = Theme::Light;
        assert_ne!(app.grid_color(), Color32::from_rgb(7, 8, 9));
    }

    #[test]
    fn per_theme_colours_round_trip_through_settings() {
        use super::ViewApp;
        use egui::Color32;
        let mut app = ViewApp::new(empty_diff(), "old".into(), "new".into());
        app.canvas_dark = Color32::from_rgb(1, 1, 1);
        app.canvas_light = Color32::from_rgb(250, 250, 250);
        app.grid_dark = Color32::from_rgba_unmultiplied(2, 2, 2, 30);
        app.grid_light = Color32::from_rgba_unmultiplied(200, 200, 200, 30);

        let mut fresh = ViewApp::new(empty_diff(), "x".into(), "y".into());
        fresh.apply_settings(app.to_settings());
        assert_eq!(fresh.canvas_dark, app.canvas_dark);
        assert_eq!(fresh.canvas_light, app.canvas_light);
        assert_eq!(fresh.grid_dark, app.grid_dark);
        assert_eq!(fresh.grid_light, app.grid_light);
    }

    /// A minimal empty diff for constructing a `ViewApp` in tests (no layers).
    fn empty_diff() -> super::BoardDiff {
        super::BoardDiff {
            report: etchy_core::DiffReport::new(Vec::new(), Vec::new()),
            layers: Vec::new(),
        }
    }

    #[test]
    fn viewapp_to_settings_and_back_preserves_fields() {
        use super::ViewApp;
        use egui::Color32;
        let mut app = ViewApp::new(empty_diff(), "old".into(), "new".into());
        app.theme = Theme::Light;
        app.base_level = BaseLevel::Strong;
        app.min_area_mm2 = 0.05;
        app.col_added = Color32::from_rgb(1, 2, 3);
        app.col_removed = Color32::from_rgb(4, 5, 6);
        app.canvas_dark = Color32::from_rgb(7, 8, 9);
        app.grid_dark = Color32::from_rgba_unmultiplied(10, 11, 12, 40);
        app.base_overrides = vec![(0, Color32::from_rgb(20, 21, 22))];
        app.input_preset = InputPreset::Altium;
        app.swipe_frac = 0.37;

        let settings = app.to_settings();
        // A fresh app gets the saved settings applied; every tunable field matches.
        let mut fresh = ViewApp::new(empty_diff(), "x".into(), "y".into());
        fresh.apply_settings(settings);
        assert_eq!(fresh.theme, app.theme);
        assert_eq!(fresh.base_level, app.base_level);
        assert_eq!(fresh.min_area_mm2, app.min_area_mm2);
        assert_eq!(fresh.col_added, app.col_added);
        assert_eq!(fresh.col_removed, app.col_removed);
        assert_eq!(fresh.canvas_dark, app.canvas_dark);
        assert_eq!(fresh.grid_dark, app.grid_dark);
        assert_eq!(fresh.base_overrides, app.base_overrides);
        assert_eq!(fresh.input_preset, app.input_preset);
        assert_eq!(fresh.swipe_frac, app.swipe_frac);
    }

    #[test]
    fn set_group_visibility_flips_only_the_group() {
        use super::set_group_visibility;
        let mut vis = vec![true, true, true, true];
        // Hide just indices 1 and 3.
        set_group_visibility(&mut vis, &[1, 3], false);
        assert_eq!(vis, vec![true, false, true, false]);
        // Show them again.
        set_group_visibility(&mut vis, &[1, 3], true);
        assert_eq!(vis, vec![true, true, true, true]);
        // Out-of-range indices are ignored (guard against stale group lists).
        set_group_visibility(&mut vis, &[99], false);
        assert_eq!(vis, vec![true, true, true, true]);
    }

    #[test]
    fn group_all_visible_reports_whole_group_state() {
        use super::group_all_visible;
        let vis = vec![true, false, true, true];
        // Group {0,2,3} all visible.
        assert!(group_all_visible(&vis, &[0, 2, 3]));
        // Group {0,1} not all visible (1 is hidden).
        assert!(!group_all_visible(&vis, &[0, 1]));
        // Empty group counts as "all visible" (nothing hidden).
        assert!(group_all_visible(&vis, &[]));
        // Out-of-range index doesn't crash and reads as not-visible.
        assert!(!group_all_visible(&vis, &[99]));
    }

    #[test]
    fn visible_from_changed_shows_only_changed_layers() {
        use super::visible_from_changed;
        // changed flags per layer index.
        let changed = [false, true, false, true];
        assert_eq!(
            visible_from_changed(&changed),
            vec![false, true, false, true]
        );
        // No changed layers -> nothing visible (caller decides whether to keep the
        // selection visible separately).
        assert_eq!(visible_from_changed(&[false, false]), vec![false, false]);
    }

    #[test]
    fn default_visible_shows_only_the_selected_layer() {
        use super::default_visible;
        // On load only the selected layer is visible; multi-layer is opt-in (#9/#10
        // perf — fewer layers transformed by default).
        assert_eq!(default_visible(4, 2), vec![false, false, true, false]);
        assert_eq!(default_visible(1, 0), vec![true]);
        // Empty board / out-of-range selected: no panic, nothing forced on.
        assert_eq!(default_visible(0, 0), Vec::<bool>::new());
        assert_eq!(default_visible(3, 9), vec![false, false, false]);
    }

    #[test]
    fn restore_visibility_rebuilds_from_indices_guarding_count() {
        use super::restore_visibility;
        // Saved indices {0, 2} over 4 layers.
        assert_eq!(
            restore_visibility(&[0, 2], 4),
            vec![true, false, true, false]
        );
        // Out-of-range saved indices (layer count shrank) are dropped, not panicking.
        assert_eq!(restore_visibility(&[0, 9], 2), vec![true, false]);
        // No saved indices -> all hidden vector of the right length.
        assert_eq!(restore_visibility(&[], 3), vec![false, false, false]);
        // Zero layers -> empty.
        assert_eq!(restore_visibility(&[0, 1], 0), Vec::<bool>::new());
    }

    #[test]
    fn visible_indices_lists_set_bits_in_order() {
        use super::visible_indices;
        assert_eq!(visible_indices(&[true, false, true, true]), vec![0, 2, 3]);
        assert_eq!(visible_indices(&[false, false]), Vec::<usize>::new());
        assert_eq!(visible_indices(&[]), Vec::<usize>::new());
    }

    #[test]
    fn geom_key_tracks_the_visible_set() {
        let base = build_geom_key(&[0, 2], Mode::Overlay, BaseLevel::Faint, true, Some(1));
        // Same visible set + same other inputs -> equal (no rebuild).
        assert_eq!(
            base,
            build_geom_key(&[0, 2], Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
        // A different visible set -> different key (the merged mesh changes).
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0, 2, 3], Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
        // base-off and mode still flip the key.
        assert_ne!(
            base,
            build_geom_key(&[0, 2], Mode::Overlay, BaseLevel::Off, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0, 2], Mode::Before, BaseLevel::Faint, true, Some(1))
        );
    }

    #[test]
    fn viewapp_visibility_round_trips_through_settings() {
        use super::ViewApp;
        let mut app = ViewApp::new(empty_diff(), "old".into(), "new".into());
        // Pretend a 4-layer board: drive the visible set directly.
        app.visible_layers = vec![true, false, true, false];
        let settings = app.to_settings();
        assert_eq!(settings.visible_layers, vec![0, 2]);
        let mut fresh = ViewApp::new(empty_diff(), "x".into(), "y".into());
        fresh.visible_layers = vec![false, false, false, false];
        fresh.apply_settings(settings);
        assert_eq!(fresh.visible_layers, vec![true, false, true, false]);
    }
}
