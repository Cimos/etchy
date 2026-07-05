//! etchy-gui — native desktop viewer (egui).
//!
//! `etchy-gui <old> <new>` diffs two Gerber revision directories and shows the
//! result: a changed-first layer list and a pan/zoom canvas overlaying the diff
//! (added copper green, removed red, base muted). The geometry + diff come from
//! the pure `etchy-core` engine via `compare_detailed`; this crate only does I/O
//! and rendering. M1 scope: flash-only geometry (the engine fails loud otherwise).

mod exportio;
#[cfg(feature = "gpu-transform")]
mod gpu;
mod loader;
mod lod;

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use etchy_core::{BoardDiff, LayerStatus, LayerView, PolygonSet, Pt};

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
        // 0 args → start on the welcome screen; 2 args → load both up front (keeps
        // the `etchy-gui <old> <new>` contract and fails loud on a bad path).
        let seed = match args.len() {
            0 => None,
            2 => match load_seed(&PathBuf::from(&args[0]), &PathBuf::from(&args[1])) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("etchy-gui: error: {e:#}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("usage: etchy-gui [<old-dir> <new-dir>]");
                return ExitCode::from(2);
            }
        };
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
            Box::new(move |cc| Ok(Box::new(build_app(cc, seed)))),
        ) {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("etchy-gui: window error: {e}");
                ExitCode::from(2)
            }
        }
    }

    fn label(p: &Path) -> String {
        derive_label(p)
    }

    /// Load both revisions from CLI paths into source boards + their diff.
    fn load_seed(
        old_dir: &Path,
        new_dir: &Path,
    ) -> anyhow::Result<(LoadedBoard, LoadedBoard, BoardDiff)> {
        let (old_board, of) = loader::load_source(old_dir)?;
        let (new_board, nf) = loader::load_source(new_dir)?;
        let old = LoadedBoard {
            label: label(old_dir),
            board: old_board,
            fmt: of,
        };
        let new = LoadedBoard {
            label: label(new_dir),
            board: new_board,
            fmt: nf,
        };
        let diff = diff_from_sources(&old, &new)?;
        Ok((old, new, diff))
    }

    /// Construct the app, restoring persisted settings. With a seed, both source
    /// boards are set so either can be reopened; without one, the welcome screen
    /// shows (empty diff).
    fn build_app(
        cc: &eframe::CreationContext<'_>,
        seed: Option<(LoadedBoard, LoadedBoard, BoardDiff)>,
    ) -> ViewApp {
        match seed {
            Some((old, new, diff)) => {
                let (ol, nl) = (old.label.clone(), new.label.clone());
                let mut app = ViewApp::from_cc(cc, diff, ol, nl);
                app.src_old = Some(old);
                app.src_new = Some(new);
                app
            }
            None => ViewApp::from_cc(cc, empty_diff(), String::new(), String::new()),
        }
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
        let name = f
            .path()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
        let mut kind = etchy_core::classify(stem, ext);
        // Gerber, Excellon/NC drill (#62), or pick-and-place (#115); anything else
        // is skipped. Best-effort: a layer that fails to parse is dropped rather
        // than crashing the demo.
        let geometry = if etchy_core::looks_like_gerber(bytes) {
            if fmt.is_none() {
                fmt = etchy_core::gerber_format(bytes).ok();
            }
            etchy_core::polygonize_gerber(bytes).ok()
        } else if etchy_core::looks_like_excellon(bytes) {
            kind = etchy_core::LayerKind::Drill;
            etchy_core::resolve_excellon(bytes).ok()
        } else if etchy_core::looks_like_placement(bytes) {
            kind = etchy_core::LayerKind::Placement;
            etchy_core::resolve_placement(bytes).ok()
        } else {
            None
        };
        if let Some(geometry) = geometry {
            layers.push(etchy_core::Layer {
                kind,
                label: name.clone(),
                geometry: std::sync::Arc::new(geometry),
            });
        }
    }
    (etchy_core::Board { layers }, fmt)
}

#[cfg(target_arch = "wasm32")]
fn demo_seed() -> (LoadedBoard, LoadedBoard, BoardDiff) {
    static OLD: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/demo/old");
    static NEW: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/demo/new");
    let (ob, of) = board_from_files(&OLD);
    let (nb, nf) = board_from_files(&NEW);
    // Derive the demo labels through the same helper native uses (#177), from the
    // board's release layout, so web and native present the same old->new text.
    let old = LoadedBoard {
        label: derive_label(std::path::Path::new("Mad_RP2040/v0.0.0")),
        board: ob,
        fmt: of,
    };
    let new = LoadedBoard {
        label: derive_label(std::path::Path::new("Mad_RP2040/v0.0.1")),
        board: nb,
        fmt: nf,
    };
    let diff = diff_from_sources(&old, &new).expect("demo diff");
    (old, new, diff)
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
                    let (old, new, diff) = demo_seed();
                    let (ol, nl) = (old.label.clone(), new.label.clone());
                    let mut app = ViewApp::from_cc(cc, diff, ol, nl);
                    // Seed the sources so "Open A/B" re-diffs against the demo side
                    // the user keeps (#120).
                    app.src_old = Some(old);
                    app.src_new = Some(new);
                    Ok(Box::new(app))
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
    /// The old revision alone (was "Before"; renamed to match the CLI + RevSide, #160).
    Old,
    /// The new revision alone (was "After").
    New,
    /// Side-by-side: old board left, new board right, one shared camera (G4).
    Split,
    /// Curtain wipe: one draggable divider, old board left of it, new board right,
    /// one shared camera (#61). Like Split but the boundary is user-movable.
    Swipe,
}

/// Base-opacity stops matching the retired off/faint/strong control (#12/#6), kept
/// as named constants so the slider's default and the `S`-key cycle reproduce the
/// previous look exactly. The base is the unchanged copper drawn behind the diff
/// (G3) — an always-available faint base keeps it visible so green/red changes read
/// against it instead of floating in black (#8).
const BASE_OPACITY_FAINT: f32 = 0.4;
const BASE_OPACITY_STRONG: f32 = 0.8;

/// Which side panel the activity rail (Feature 1) has expanded. The rail drives
/// one docked panel at a time; `None` collapses it (rail-only, canvas full width).
/// Runtime-only — not persisted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PanelTab {
    Layers,
    Measure,
    Export,
}

impl PanelTab {
    /// The rail's panel tabs, top to bottom. Each has a full body: Layers
    /// (`layers_panel_ui`), Measure (`measure_panel_ui`), Export
    /// (`export_panel_ui`).
    const ALL: [PanelTab; 3] = [PanelTab::Layers, PanelTab::Measure, PanelTab::Export];

    fn label(self) -> &'static str {
        match self {
            PanelTab::Layers => "Layers",
            PanelTab::Measure => "Measure",
            PanelTab::Export => "Export",
        }
    }
}

/// Which edge the activity rail (and the panel it drives) live on (Feature 8).
/// Flippable from Settings; persisted with a stable serde string repr (like
/// `Theme`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum RailSide {
    #[default]
    Left,
    Right,
}

/// Rail click semantics: clicking the active tab collapses the panel; clicking any
/// other tab switches to (and opens) it. Pure → unit-testable off-screen.
fn toggle_panel(current: Option<PanelTab>, clicked: PanelTab) -> Option<PanelTab> {
    if current == Some(clicked) {
        None
    } else {
        Some(clicked)
    }
}

/// Rail-click semantics for the Measure tab — the one icon with a side effect
/// beyond show/hide (#50). Clicking it opens the Measure panel *and* arms measure
/// mode; clicking it again while active collapses the panel *and* disarms. Returns
/// `(next_panel, armed)` where `armed` is the measure-mode flag: it follows the
/// panel, so arming always tracks whether Measure ends up open. Pure →
/// unit-testable off-screen.
fn measure_rail_click(current: Option<PanelTab>) -> (Option<PanelTab>, bool) {
    let next = toggle_panel(current, PanelTab::Measure);
    let armed = next == Some(PanelTab::Measure);
    (next, armed)
}

/// The file names an export writes, in order, for the Export tab's preview.
/// Mirrors [`ViewApp::build_export`]'s naming — an index-prefixed SVG per chosen
/// layer plus a board-wide `areas.csv` — so the panel can show the set without
/// generating the (expensive) SVG content. `layer_names` are the display names of
/// the layers that will be written, in export order. Pure → unit-testable. (#60)
fn export_file_names(layer_names: &[String]) -> Vec<String> {
    let mut names: Vec<String> = layer_names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{i:02}-{n}.svg"))
        .collect();
    names.push("areas.csv".into());
    names
}

/// Input scheme matching the user's ECAD tool (#54). MVP: it only controls which
/// mouse button pans the canvas (the real differentiator between tools) — scroll
/// stays zoom-to-cursor for all three. A full per-key remapper is a follow-up.
/// Persisted via #52 with a stable serde string repr (like `Theme`).
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
    use egui::PointerButton::{Middle, Primary, Secondary};
    // Left-drag pans in every preset (#18). The swipe divider guards its own primary
    // drag (see `draw_canvas`), so primary is free to pan everywhere else.
    match preset {
        InputPreset::KiCad => button == Primary || button == Middle || button == Secondary,
        InputPreset::Altium => button == Primary || button == Secondary,
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
/// the canvas by `t` (0 = pure canvas / base off, 1 = the full layer colour).
/// Opaque (not low-alpha) so unchanged copper reads as dim copper, not near-black
/// over the dark canvas. `t` is the continuous base opacity (#12/#6); the old
/// off/faint/strong stops map to 0.0 / 0.4 / 0.8.
fn base_display_color(layer: Color32, canvas: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
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

/// Step the base opacity through the off/faint/strong stops (the `S` key), so the
/// keyboard keeps the three familiar levels even though the panel slider is now
/// continuous (#12/#6): 0 → faint → strong → 0. Any in-between slider value below
/// strong steps up to strong; strong or above wraps back to off.
fn cycle_base_opacity(t: f32) -> f32 {
    if t <= 0.0 {
        BASE_OPACITY_FAINT
    } else if t < BASE_OPACITY_STRONG {
        BASE_OPACITY_STRONG
    } else {
        0.0
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
    base_opacity: f32,
    outline_visible: bool,
    outline: Option<usize>,
) -> GeomKey {
    GeomKey {
        visible: visible.to_vec(),
        mode,
        base_on: base_opacity > 0.0,
        // The faint outline reference draws whenever the outline layer is visible and
        // exists (#157: its normal `visible_layers` eye now gates it). It's drawn on
        // every layer, so there's no single "selected" layer to suppress it for.
        outline_effective: outline_visible && outline.is_some(),
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

/// On-load visibility (#9/#10 perf): the selected layer plus the board outline are
/// shown; every other layer is opt-in via the per-row/per-group checkboxes.
/// Rendering one layer by default keeps the common case fast on dense boards (the old
/// default showed every changed layer at once). The outline is on by default so the
/// board edge reads as orientation context from the start (#157); it's a normal layer
/// row now, toggled like any other. Out-of-range `selected`/`outline` are ignored.
fn default_visible(n: usize, selected: usize, outline: Option<usize>) -> Vec<bool> {
    let mut v = vec![false; n];
    if let Some(s) = v.get_mut(selected) {
        *s = true;
    }
    if let Some(o) = outline.and_then(|oi| v.get_mut(oi)) {
        *o = true;
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
        // Fab documentation (drill drawing/guide, pad master) — a parchment tan so
        // it reads as a drawing/annotation, distinct from anonymous "other".
        Documentation => (
            Color32::from_rgb(0x9a, 0x8c, 0x6b),
            Color32::from_rgb(0x6b, 0x60, 0x48),
        ),
        // Pick-and-place markers (#115) — a violet so parts read distinct from
        // copper/silk/docs.
        Placement => (
            Color32::from_rgb(0xa2, 0x7c, 0xd8),
            Color32::from_rgb(0x6f, 0x52, 0x9a),
        ),
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

/// Keep the swipe divider within the canvas. The divider travels the FULL width
/// (#14) — right to either edge so you can wipe all the way across; only genuinely
/// out-of-range values are clamped back to an edge.
fn clamp_swipe_frac(frac: f32) -> f32 {
    frac.clamp(0.0, 1.0)
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

/// One loaded revision, retained so either side can be swapped and re-diffed
/// without re-reading the other (#120). Geometry is `Arc`-shared, so keeping the
/// source board alongside the diff is cheap.
#[derive(Clone)]
struct LoadedBoard {
    label: String,
    board: etchy_core::Board,
    fmt: Option<etchy_core::GerberFormat>,
}

/// Which revision a load targets: the old (A) or new (B) side. (Distinct from
/// `Side` above, which is the split-view Left/Right.)
#[derive(Clone, Copy, PartialEq, Eq)]
enum RevSide {
    Old,
    New,
}

/// Result of a web async file pick, delivered back to the UI thread over a
/// channel (the browser file dialog is async; native uses a blocking dialog).
#[cfg(target_arch = "wasm32")]
struct FilePick {
    side: RevSide,
    result: anyhow::Result<LoadedBoard>,
}

/// The Settings window's left-rail sections (#121). The window shows one at a
/// time, so the long per-layer colour list no longer buries the other controls.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SettingsTab {
    #[default]
    Display,
    Diff,
    Grid,
    Input,
    Colours,
    Layers,
}

impl SettingsTab {
    /// (tab, label) in rail order.
    const ALL: [(SettingsTab, &'static str); 6] = [
        (SettingsTab::Display, "Display"),
        (SettingsTab::Diff, "Diff"),
        (SettingsTab::Grid, "Grid"),
        (SettingsTab::Input, "Input"),
        (SettingsTab::Colours, "Colours"),
        (SettingsTab::Layers, "Layers"),
    ];
}

/// How many layers the canvas shows at once (#59). A quick preset over the
/// per-layer visibility checkboxes: Single = only the active layer (the fast
/// default on dense boards); Highlight = every layer, active at full strength and
/// the rest dimmed (the Altium/KiCad way of reading a stack); All = every layer at
/// equal strength.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    #[default]
    Single,
    Highlight,
    All,
}

impl ViewMode {
    const ALL: [(ViewMode, &'static str); 3] = [
        (ViewMode::Single, "single"),
        (ViewMode::Highlight, "highlight"),
        (ViewMode::All, "all"),
    ];
    /// Non-selected layers are dimmed only in Highlight (Single shows one layer;
    /// All shows every layer at equal strength).
    fn dims_others(self) -> bool {
        matches!(self, ViewMode::Highlight)
    }

    /// In All view, non-selected layers draw DIFF-ONLY — their faint base copper
    /// (the bulk of the per-frame vertices, ~70% on a 13-layer board) is dropped so
    /// panning stays smooth on dense boards (#158). The selected layer keeps its
    /// base for context, and Highlight keeps every layer's dimmed base as context;
    /// only All trades the non-selected base away.
    fn hides_unselected_base(self) -> bool {
        matches!(self, ViewMode::All)
    }
}

/// The per-layer visibility a view mode selects (#59): Single shows the active layer
/// (plus the board outline for orientation, #157); Highlight and All show every layer
/// (they differ only in dimming, handled by [`ViewMode::dims_others`]). Pure, so the
/// preset is unit-testable.
fn visibility_for_mode(
    mode: ViewMode,
    n: usize,
    selected: usize,
    outline: Option<usize>,
) -> Vec<bool> {
    match mode {
        ViewMode::Single => default_visible(n, selected, outline),
        ViewMode::Highlight | ViewMode::All => vec![true; n],
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
    /// New-revision area (mm²) per layer, cached at load so the sidebar %-change
    /// (#114) doesn't reshoelace every frame. Denominator for "how much changed".
    new_area_mm2: Vec<f64>,
    mode: Mode,
    /// Opacity of the always-available base copper behind the diff (G3), 0..=1
    /// (#12/#6). 0 hides the base; the old off/faint/strong stops are 0.0/0.4/0.8.
    /// Driven by the slider at the top of the Layers panel.
    base_opacity: f32,
    /// User-configurable diff colors (G3, Altium-compare style). Default to the
    /// brand green/red; a "Settings" popover edits them.
    col_added: Color32,
    col_removed: Color32,
    /// Canvas (board background) colour (#53), kept per-theme so a dark board tuned
    /// in dark mode never leaks into light mode (#31). Resolved via `canvas_color()`;
    /// editable in the Settings window (active theme) and persisted (#52).
    canvas_dark: Color32,
    canvas_light: Color32,
    /// Grid colour (#53), per-theme for the same reason as the canvas (#31).
    grid_dark: Color32,
    grid_light: Color32,
    /// Per-layer base/context colour overrides, keyed by the layer's index in
    /// `diff.layers` (default = layer_type_color for that kind) (#21).
    base_overrides: Vec<(usize, Color32)>,
    /// The Settings editor window is open. A real window (not a menu) so the nested
    /// colour-picker popup works — a menu_button closed on the first inner click.
    show_settings: bool,
    /// Which Settings section the left rail has selected (runtime-only; not
    /// persisted — the window always opens on Display).
    settings_tab: SettingsTab,
    /// How many layers the canvas shows at once (#59; runtime-only). Changing it
    /// resets the per-layer visibility to the mode's preset.
    view_mode: ViewMode,
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
    /// Startup splash (brand logo screen): the time of the first frame, and whether
    /// the splash has finished/been dismissed. `None` until the first frame.
    splash_start: Option<f64>,
    splash_done: bool,
    /// Index of the board-outline layer (Edge.Cuts/GKO), drawn faintly on every layer
    /// for orientation (G10); None if the board has no outline layer. It's a normal
    /// layer row now (#157) — its `visible_layers` bit drives the faint reference, so
    /// there's no separate `show_outline` control.
    outline: Option<usize>,
    /// World-space tessellation cache (G6): rebuilt only when the GeomKey changes,
    /// so pan/zoom/colour edits skip re-triangulation.
    cache: Option<TessCache>,
    /// Measure tool active (#22): canvas clicks drop ruler points instead of
    /// panning; Esc clears and exits.
    measure_mode: bool,
    /// The in-progress ruler buffer (#50): 0 or 1 world-space points. A second
    /// click completes the pair into `measurements` and empties this.
    measure_pts: Vec<[f64; 2]>,
    /// Completed measurements (#50): the running list the Measure tab shows and the
    /// canvas draws. Runtime-only and per-board — cleared on load.
    measurements: Vec<Measurement>,
    /// Unit the measure label is shown in (#50): mm / inch / mil.
    measure_unit: Unit,
    /// Grid overlay on (#51): faint world-spaced lines over the canvas.
    show_grid: bool,
    /// Grid spacing in mm (#51); the DragValue edits this.
    grid_mm: f64,
    /// Snap measure clicks to the nearest grid intersection (#51).
    snap_grid: bool,
    /// Always-on cursor crosshair + coordinate readout (#179), independent of
    /// measure mode. On by default; the toggle lands in the Measure tab later.
    show_crosshair: bool,
    /// Input scheme matching the user's ECAD tool (#54). MVP: controls which mouse
    /// button pans the canvas. Persisted via #52.
    input_preset: InputPreset,
    /// Swipe/curtain divider position, normalized 0..1 across the canvas width
    /// (#61). Clamped to [0.1, 0.9] on use; persisted via #52.
    swipe_frac: f32,
    /// Transient: the swipe divider is being dragged (#61). Not persisted — it only
    /// holds the grab across frames so leaving the handle mid-drag keeps it.
    swipe_drag: bool,

    /// Retained source boards (#120) so either side can be reopened and re-diffed.
    /// `None` before a board is chosen (the welcome screen shows in that state).
    src_old: Option<LoadedBoard>,
    src_new: Option<LoadedBoard>,
    /// The last load/diff error, surfaced in the UI (fail-loud, never silent).
    load_error: Option<String>,
    /// The directory of the last board opened, so the next "Open" dialog starts
    /// there (revisions A and B usually live side by side) (#120 polish). Native
    /// only — the web file picker has no directory concept.
    #[cfg(not(target_arch = "wasm32"))]
    last_dir: Option<std::path::PathBuf>,
    /// Web only: async file-pick results land here and are drained each frame. The
    /// sender is cloned into each pick task; native uses a blocking dialog instead.
    #[cfg(target_arch = "wasm32")]
    file_tx: std::sync::mpsc::Sender<FilePick>,
    #[cfg(target_arch = "wasm32")]
    file_rx: std::sync::mpsc::Receiver<FilePick>,

    /// GPU base-transform (#106): the glow resources, present only when the
    /// `gpu-transform` feature is built and a GL context is available.
    #[cfg(feature = "gpu-transform")]
    gpu: Option<gpu::GpuMesh>,
    /// Runtime toggle for the GPU path; the CPU path is always the fallback.
    #[cfg(feature = "gpu-transform")]
    use_gpu: bool,
    /// Hash of the inputs that determine the uploaded GPU mesh (geometry, visible
    /// set, selection, colours). We re-upload only when it changes — never per pan
    /// frame, which is what makes the GPU path O(1) in triangle count.
    #[cfg(feature = "gpu-transform")]
    gpu_hash: Option<u64>,
    /// Transient status line from the last export (#60), shown by the Export
    /// control. Not persisted.
    export_msg: Option<String>,
    /// Which side panel the activity rail has expanded (Feature 1); `None` =
    /// rail-only (canvas full width). Runtime-only — not persisted.
    active_panel: Option<PanelTab>,
    /// Which edge the activity rail lives on (Feature 8); persisted via #52.
    rail_side: RailSide,
}

impl ViewApp {
    fn new(diff: BoardDiff, old_label: String, new_label: String) -> Self {
        #[cfg(target_arch = "wasm32")]
        let (file_tx_init, file_rx_init) = std::sync::mpsc::channel::<FilePick>();
        let mut order: Vec<usize> = (0..diff.layers.len()).collect();
        order.sort_by_key(|&i| !diff.layers[i].is_changed()); // changed first, stable
        let selected = order.first().copied().unwrap_or(0);
        let outline = pick_outline_index(diff.layers.len(), |i| diff.layers[i].kind);
        // Default visibility (#9/#10 perf): show the selected layer plus the board
        // outline on load; every other layer is opt-in via the checkboxes. Keeps the
        // common case fast on dense boards. `selected` is the most-changed layer
        // (changed-first order); the outline is on for orientation (#157).
        let visible_layers = default_visible(diff.layers.len(), selected, outline);
        let new_area_mm2 = layer_new_areas(&diff);
        Self {
            diff,
            old_label,
            new_label,
            order,
            selected,
            visible_layers,
            new_area_mm2,
            mode: Mode::Overlay,
            base_opacity: BASE_OPACITY_FAINT,
            col_added: C_ADDED,
            col_removed: C_REMOVED,
            canvas_dark: C_CANVAS,
            canvas_light: C_CANVAS_LIGHT,
            grid_dark: C_GRID_DEFAULT,
            grid_light: C_GRID_DEFAULT_LIGHT,
            base_overrides: Vec::new(),
            show_settings: false,
            settings_tab: SettingsTab::default(),
            view_mode: ViewMode::default(),
            min_area_mm2: MIN_AREA_MM2,
            last_hidden: 0,
            cam: Camera::default(),
            theme: Theme::Dark,
            applied_theme: None,
            warning_expanded: false,
            warning_shown_at: None,
            splash_start: None,
            splash_done: false,
            outline,
            cache: None,
            measure_mode: false,
            measure_pts: Vec::new(),
            measurements: Vec::new(),
            measure_unit: Unit::Mm,
            // Grid + snap + crosshair default ON (#179): the snapped-cursor crosshair
            // and coordinate readout are available all the time, not only in measure
            // mode.
            show_grid: true,
            grid_mm: 1.0,
            snap_grid: true,
            show_crosshair: true,
            input_preset: InputPreset::default(),
            swipe_frac: 0.5,
            swipe_drag: false,
            src_old: None,
            src_new: None,
            load_error: None,
            #[cfg(not(target_arch = "wasm32"))]
            last_dir: None,
            #[cfg(target_arch = "wasm32")]
            file_tx: file_tx_init,
            #[cfg(target_arch = "wasm32")]
            file_rx: file_rx_init,
            // The feature is opt-in (off in normal builds), so defaulting the
            // runtime toggle on inside a feature build is safe and lets the GPU
            // path be exercised; the checkbox still turns it off.
            #[cfg(feature = "gpu-transform")]
            gpu: None,
            #[cfg(feature = "gpu-transform")]
            use_gpu: true,
            #[cfg(feature = "gpu-transform")]
            gpu_hash: None,
            export_msg: None,
            // The Layers panel is open by default, matching the old always-visible
            // left panel; the rail can collapse it.
            active_panel: Some(PanelTab::Layers),
            rail_side: RailSide::default(),
        }
    }

    /// Build the export file set (#60): per-layer SVGs (current layer, or all
    /// changed layers) plus the board-wide copper-area CSV.
    fn build_export(&self, all_layers: bool) -> Vec<exportio::ExportFile> {
        let mut files = Vec::new();
        let chosen: Vec<&LayerView> = if all_layers {
            self.diff
                .layers
                .iter()
                .filter(|l| l.status != LayerStatus::Unchanged)
                .collect()
        } else {
            self.diff.layers.get(self.selected).into_iter().collect()
        };
        for (i, l) in chosen.iter().enumerate() {
            // Prefix with an index so two layers sharing a display name (e.g. two
            // "other" layers) don't clobber each other's file.
            files.push(exportio::ExportFile {
                name: format!("{i:02}-{}.svg", l.name()),
                content: etchy_core::layer_svg(l),
            });
        }
        files.push(exportio::ExportFile {
            name: "areas.csv".into(),
            content: etchy_core::board_areas_csv(&self.diff),
        });
        files
    }

    /// Run an export and stash the result message for the toast.
    fn do_export(&mut self, all_layers: bool) {
        let files = self.build_export(all_layers);
        self.export_msg = Some(match exportio::save(&files) {
            Ok(msg) => msg,
            Err(e) => format!("export failed: {e}"),
        });
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
        // visible regardless, so a highlight is never blank there. Selecting a layer
        // does NOT move the camera (#4) — the view stays where the user left it;
        // only Fit reframes.
        self.selected = idx;
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
        Documentation => "docs".to_string(),
        Placement => "placement".to_string(),
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
        Outline | Documentation | Placement => LayerGroup::Mechanical,
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

// Brand palette (assets/brand/README.md): diff accents + board-dark canvas.
const C_ADDED: Color32 = Color32::from_rgb(0x46, 0xd1, 0x8a); // #46d18a
const C_REMOVED: Color32 = Color32::from_rgb(0xff, 0x5d, 0x73); // #ff5d73
const C_BASE: Color32 = Color32::from_rgb(90, 95, 105);

/// Named added/removed colour presets offered in Settings → Colours (#155).
/// Brand is the default green/red; Colour-safe swaps to blue/orange so red-green
/// colour-blind viewers can still tell them apart; High-contrast maxes separation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffPalette {
    Brand,
    ColorSafe,
    HighContrast,
}

impl DiffPalette {
    const ALL: [(DiffPalette, &'static str); 3] = [
        (DiffPalette::Brand, "brand"),
        (DiffPalette::ColorSafe, "colour-safe"),
        (DiffPalette::HighContrast, "high-contrast"),
    ];
    /// The (added, removed) colours for this palette.
    fn colors(self) -> (Color32, Color32) {
        match self {
            DiffPalette::Brand => (C_ADDED, C_REMOVED),
            // Blue / orange — distinguishable under deutan/protan colour blindness.
            DiffPalette::ColorSafe => (
                Color32::from_rgb(0x3b, 0x9c, 0xff),
                Color32::from_rgb(0xff, 0x9e, 0x3d),
            ),
            // Maximum separation on the dark canvas.
            DiffPalette::HighContrast => (
                Color32::from_rgb(0x2b, 0xff, 0x88),
                Color32::from_rgb(0xff, 0x2d, 0x55),
            ),
        }
    }
}
/// Brand "board dark" — the canvas (PCB) background.
const C_CANVAS: Color32 = Color32::from_rgb(0x0b, 0x0f, 0x0e); // #0b0f0e

/// Startup splash (the brand logo screen): hold the wordmark at full opacity for
/// `HOLD`, then fade over `FADE`. Deliberately brief — a launch-time brand moment,
/// and any click/key/scroll dismisses it instantly so `etchy-gui old new` lands on
/// the diff fast.
const SPLASH_HOLD_SECS: f64 = 0.9;
const SPLASH_FADE_SECS: f64 = 0.6;

/// Links for the Help menu.
const URL_REPO: &str = "https://github.com/Cimos/etchy";
const URL_ISSUES: &str = "https://github.com/Cimos/etchy/issues";
const URL_SITE: &str = "https://cimos.github.io";
const URL_SPONSOR: &str = "https://github.com/sponsors/Cimos";

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
/// Render-only contour-simplification tolerance (nm) for the faint base/outline mesh
/// (#94). At ~2µm the fixed 64-gon flashes (pads/vias) collapse to far fewer triangles
/// while large/flat features keep detail; the deviation is invisible at any practical
/// zoom and the diff geometry is never simplified.
const BASE_SIMPLIFY_TOL_NM: f64 = 2000.0;
/// Default min-area threshold (mm²). Diff regions smaller than this are treated
/// as noise (e.g. the sub-µm rims from a units/precision mismatch) and dropped —
/// but the count is always surfaced on the canvas (the top-left hidden-count chip,
/// `hidden_note`), never silently.
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

/// Map a legacy persisted base level (pre-#12 off/faint/strong string) to the
/// equivalent continuous opacity, so old configs keep their look after the slider
/// migration. Unknown values fall back to faint (the old default).
fn legacy_base_opacity(level: &str) -> f32 {
    match level {
        "off" => 0.0,
        "strong" => BASE_OPACITY_STRONG,
        _ => BASE_OPACITY_FAINT,
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

/// Rail side persists as a stable string (same rationale as `Theme`).
impl serde::Serialize for RailSide {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            RailSide::Left => "left",
            RailSide::Right => "right",
        })
    }
}
impl<'de> serde::Deserialize<'de> for RailSide {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Unknown / legacy values fall back to the default (left), never error.
        Ok(match String::deserialize(d)?.as_str() {
            "right" => RailSide::Right,
            _ => RailSide::Left,
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
    /// Base opacity 0..=1 (#12/#6). Replaces the old off/faint/strong `base_level`
    /// segment. An old persisted `base_level` string is migrated in `apply_settings`.
    base_opacity: f32,
    /// Legacy off/faint/strong base level, read only for migration from pre-#12
    /// configs. Never written (new configs persist `base_opacity`), so a fresh
    /// round-trip always sees `None` here.
    #[serde(default, skip_serializing)]
    base_level: Option<String>,
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
    /// Which edge the activity rail lives on (Feature 8).
    rail_side: RailSide,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            base_opacity: BASE_OPACITY_FAINT,
            base_level: None,
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
            rail_side: RailSide::default(),
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
        #[cfg(feature = "gpu-transform")]
        {
            // Build the GL program once from eframe's glow context; None (e.g. no
            // GL) just leaves the CPU path in charge.
            app.gpu = cc
                .gl
                .as_ref()
                .and_then(|gl| gpu::GpuMesh::new(std::sync::Arc::clone(gl)));
        }
        app
    }

    /// Build the colour'd triangle list for the GPU path (#80): every visible item
    /// (base + diff + outline) with its colour baked per-vertex, using the same
    /// colour logic as `transform_cache` minus the per-frame LOD/marker (the GPU
    /// path draws true-scale). Colours are premultiplied (`Color32::to_array`) to
    /// match egui's blend.
    #[cfg(feature = "gpu-transform")]
    fn build_gpu_tris(&self, cache: &TessCache) -> Vec<gpu::ColorTri> {
        let canvas = self.canvas_color();
        let mut out = Vec::new();
        for item in &cache.items {
            let dim = dim_factor(
                item.layer_index,
                self.selected,
                self.view_mode.dims_others(),
            );
            let mut color = match item.role {
                Role::Base => {
                    let base = if item.layer_index == NO_LAYER {
                        C_BASE
                    } else {
                        resolve_base_color(
                            item.layer_index,
                            self.diff.layers[item.layer_index].kind,
                            &self.base_overrides,
                            self.theme,
                        )
                    };
                    base_display_color(base, canvas, self.base_opacity)
                }
                Role::Outline => C_OUTLINE_FAINT,
                Role::Added => self.col_added,
                Role::Removed => self.col_removed,
            };
            if dim < 1.0 {
                color = with_alpha(color, (color.a() as f32 / 255.0) * dim);
            }
            let [r, g, b, a] = color.to_array();
            let inv = 1.0 / 255.0;
            let c = [
                r as f32 * inv,
                g as f32 * inv,
                b as f32 * inv,
                a as f32 * inv,
            ];
            for &tri in &item.tris {
                out.push((tri, c));
            }
        }
        out
    }

    /// Hash of everything that affects the uploaded GPU mesh — geometry/visibility
    /// (the cache key), selection, base opacity, theme, and colours — so we re-upload
    /// only on a real change, never per pan frame.
    #[cfg(feature = "gpu-transform")]
    fn gpu_input_hash(&self, cache: &TessCache) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cache.key.visible.hash(&mut h);
        (cache.key.mode as u8).hash(&mut h);
        cache.key.base_on.hash(&mut h);
        cache.key.outline_effective.hash(&mut h);
        self.selected.hash(&mut h);
        self.base_opacity.to_bits().hash(&mut h);
        (self.theme as u8).hash(&mut h);
        self.canvas_color().to_array().hash(&mut h);
        self.col_added.to_array().hash(&mut h);
        self.col_removed.to_array().hash(&mut h);
        for (i, c) in &self.base_overrides {
            i.hash(&mut h);
            c.to_array().hash(&mut h);
        }
        h.finish()
    }

    /// Snapshot the user-tunable state for persistence (#52).
    fn to_settings(&self) -> Settings {
        Settings {
            theme: self.theme,
            base_opacity: self.base_opacity,
            base_level: None, // legacy field is read-only; new configs store base_opacity
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
            rail_side: self.rail_side,
        }
    }

    /// Apply persisted settings onto a freshly-built app (#52). Diff geometry and
    /// the layer order are NOT touched — only view preferences.
    fn apply_settings(&mut self, s: Settings) {
        self.theme = s.theme;
        self.applied_theme = None; // force re-applying the egui visuals next frame
                                   // Migrate a pre-#12 off/faint/strong string if present; otherwise use the
                                   // stored continuous opacity (new configs never write the legacy field).
        self.base_opacity = match s.base_level.as_deref() {
            Some(level) => legacy_base_opacity(level),
            None => s.base_opacity.clamp(0.0, 1.0),
        };
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
        self.rail_side = s.rail_side;
    }
}

impl ViewApp {
    /// Replace the shown diff with a freshly loaded one, resetting board-derived
    /// state (layer order/selection/visibility, outline, caches, camera) while
    /// keeping user preferences (theme, colours, presets). The camera re-fits next
    /// frame (#120).
    fn adopt_diff(&mut self, diff: BoardDiff, old_label: String, new_label: String) {
        let mut order: Vec<usize> = (0..diff.layers.len()).collect();
        order.sort_by_key(|&i| !diff.layers[i].is_changed());
        let selected = order.first().copied().unwrap_or(0);
        let outline = pick_outline_index(diff.layers.len(), |i| diff.layers[i].kind);
        let visible_layers = default_visible(diff.layers.len(), selected, outline);
        self.diff = diff;
        self.old_label = old_label;
        self.new_label = new_label;
        self.order = order;
        self.selected = selected;
        self.outline = outline;
        self.visible_layers = visible_layers;
        self.new_area_mm2 = layer_new_areas(&self.diff);
        self.base_overrides.clear(); // indices are per-board
        self.cache = None;
        self.cam = Camera::default(); // fitted=false → auto-fit next frame
        self.measure_pts.clear();
        self.measurements.clear(); // world coords are per-board
        self.warning_shown_at = None;
        self.warning_expanded = false;
        self.load_error = None;
        #[cfg(feature = "gpu-transform")]
        {
            self.gpu_hash = None;
        }
    }

    /// Store a freshly loaded board on one side and re-diff if both sides are set.
    fn set_side(&mut self, side: RevSide, loaded: LoadedBoard) {
        match side {
            RevSide::Old => self.src_old = Some(loaded),
            RevSide::New => self.src_new = Some(loaded),
        }
        self.load_error = None;
        self.rebuild_diff();
    }

    /// Recompute the diff from the two source boards, if both are present.
    fn rebuild_diff(&mut self) {
        let (Some(o), Some(n)) = (self.src_old.as_ref(), self.src_new.as_ref()) else {
            return;
        };
        match diff_from_sources(o, n) {
            Ok(diff) => {
                let (ol, nl) = (o.label.clone(), n.label.clone());
                self.adopt_diff(diff, ol, nl);
            }
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// Drain any completed async file picks (web) into the diff.
    #[cfg(target_arch = "wasm32")]
    fn poll_file_picks(&mut self) {
        while let Ok(pick) = self.file_rx.try_recv() {
            match pick.result {
                Ok(loaded) => self.set_side(pick.side, loaded),
                Err(e) => self.load_error = Some(format!("{e:#}")),
            }
        }
    }

    /// Native: open a folder picker for `side`.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_folder(&mut self, side: RevSide) {
        let title = match side {
            RevSide::Old => "Open old revision — folder",
            RevSide::New => "Open new revision — folder",
        };
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(d) = &self.last_dir {
            dialog = dialog.set_directory(d);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.load_side_path(side, &dir);
        }
    }

    /// Native: open a `.zip` fab-pack picker for `side`.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_zip(&mut self, side: RevSide) {
        let title = match side {
            RevSide::Old => "Open old revision — .zip fab pack",
            RevSide::New => "Open new revision — .zip fab pack",
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("fab pack", &["zip"]);
        if let Some(d) = &self.last_dir {
            dialog = dialog.set_directory(d);
        }
        if let Some(file) = dialog.pick_file() {
            self.load_side_path(side, &file);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_side_path(&mut self, side: RevSide, path: &std::path::Path) {
        // Next dialog starts from this board's parent, so opening the other
        // revision lands on its sibling (#120 polish).
        if let Some(parent) = path.parent() {
            self.last_dir = Some(parent.to_path_buf());
        }
        match loader::load_source(path) {
            Ok((board, fmt)) => self.set_side(
                side,
                LoadedBoard {
                    label: path_label(path),
                    board,
                    fmt,
                },
            ),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// The primary toolbar "Open" action for a side: a folder on native, the
    /// browser file picker on web.
    fn open_primary(&mut self, side: RevSide, _ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_folder(side);
        #[cfg(target_arch = "wasm32")]
        self.pick_files_web(side, _ctx);
    }

    /// Handle files dropped onto the window (both surfaces). Fills the first empty
    /// side (A then B); if both are already loaded, a drop replaces A.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let side = if self.src_old.is_none() {
            RevSide::Old
        } else if self.src_new.is_none() {
            RevSide::New
        } else {
            RevSide::Old
        };
        self.load_dropped(side, dropped);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_dropped(&mut self, side: RevSide, files: Vec<egui::DroppedFile>) {
        let paths: Vec<std::path::PathBuf> = files.iter().filter_map(|f| f.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        match load_dropped_paths(&paths) {
            Ok(loaded) => self.set_side(side, loaded),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn load_dropped(&mut self, side: RevSide, files: Vec<egui::DroppedFile>) {
        // Web drops carry bytes. A single `.zip` → load_zip; otherwise every
        // dropped file is treated as a layer.
        let mut byte_files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut zip: Option<Vec<u8>> = None;
        for f in files {
            let Some(bytes) = f.bytes else { continue };
            if f.name.to_ascii_lowercase().ends_with(".zip") {
                zip = Some(bytes.to_vec());
            } else {
                byte_files.push((basename(&f.name), bytes.to_vec()));
            }
        }
        let res = if let Some(zb) = zip {
            loader::load_zip(zb)
        } else {
            loader::board_from_bytes(byte_files)
        };
        match res {
            Ok((board, fmt)) => self.set_side(
                side,
                LoadedBoard {
                    label: "dropped files".into(),
                    board,
                    fmt,
                },
            ),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// Web: open the browser file picker for `side` (multiple Gerbers or one
    /// `.zip`); the result comes back over the channel and is drained next frame.
    #[cfg(target_arch = "wasm32")]
    fn pick_files_web(&mut self, side: RevSide, ctx: &egui::Context) {
        let tx = self.file_tx.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let Some(handles) = rfd::AsyncFileDialog::new()
                .add_filter(
                    "Gerber / fab pack",
                    &[
                        "gbr", "gtl", "gbl", "gts", "gbs", "gto", "gbo", "gtp", "gbp", "gko",
                        "gm1", "zip",
                    ],
                )
                .pick_files()
                .await
            else {
                return;
            };
            let mut byte_files: Vec<(String, Vec<u8>)> = Vec::new();
            let mut zip: Option<Vec<u8>> = None;
            for h in handles {
                let name = h.file_name();
                let bytes = h.read().await;
                if name.to_ascii_lowercase().ends_with(".zip") {
                    zip = Some(bytes);
                } else {
                    byte_files.push((basename(&name), bytes));
                }
            }
            let result = if let Some(zb) = zip {
                loader::load_zip(zb)
            } else {
                loader::board_from_bytes(byte_files)
            }
            .map(|(board, fmt)| LoadedBoard {
                label: "uploaded".into(),
                board,
                fmt,
            });
            let _ = tx.send(FilePick { side, result });
            ctx.request_repaint();
        });
    }

    /// The welcome / empty-state screen shown when no board is loaded (#120).
    fn welcome_ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(56.0);
                ui.label(
                    egui::RichText::new("etchy")
                        .size(40.0)
                        .strong()
                        .color(C_COPPER),
                );
                ui.label(
                    egui::RichText::new("PCB visual + geometric diff")
                        .size(15.0)
                        .color(C_CREAM),
                );
                ui.add_space(22.0);
                ui.label("Open two revisions of a board's fab output to compare them.");
                ui.add_space(18.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(440.0);
                    self.side_open_row(ui, RevSide::Old, "Old revision");
                    ui.add_space(10.0);
                    self.side_open_row(ui, RevSide::New, "New revision");
                });
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(
                        "…or drag a folder, a .zip fab pack, or Gerber files onto the window.",
                    )
                    .weak()
                    .small(),
                );
                if let Some(err) = self.load_error.clone() {
                    ui.add_space(16.0);
                    ui.colored_label(C_REMOVED, format!("[!] {err}"));
                }
            });
        });
    }

    /// One "Revision X: [status] [Open…]" row for the welcome screen.
    fn side_open_row(&mut self, ui: &mut egui::Ui, side: RevSide, label: &str) {
        let status = match side {
            RevSide::Old => &self.src_old,
            RevSide::New => &self.src_new,
        }
        .as_ref()
        .map(|l| l.label.clone());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.open_buttons(ui, side);
                match status {
                    Some(s) => ui.label(egui::RichText::new(s).color(C_ADDED)),
                    None => ui.label(egui::RichText::new("not loaded").weak()),
                };
            });
        });
    }

    /// The Open button(s) for a side, platform-appropriate.
    fn open_buttons(&mut self, ui: &mut egui::Ui, side: RevSide) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if ui.button("Folder…").clicked() {
                self.pick_folder(side);
            }
            if ui.button("Zip…").clicked() {
                self.pick_zip(side);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if ui.button("Open…").clicked() {
                self.pick_files_web(side, ui.ctx());
            }
        }
    }
}

/// The initial empty diff shown before any board is loaded (the welcome screen).
/// Native start-empty path + tests; web always seeds the demo.
#[cfg(not(target_arch = "wasm32"))]
fn empty_diff() -> BoardDiff {
    BoardDiff {
        report: etchy_core::DiffReport::new(Vec::new(), Vec::new()),
        layers: Vec::new(),
    }
}

/// New-revision copper area (mm²) per layer, computed once at load — the
/// denominator for the sidebar's per-layer %-change (#114).
fn layer_new_areas(diff: &BoardDiff) -> Vec<f64> {
    diff.layers.iter().map(|l| l.new.area_mm2()).collect()
}

/// Build a `BoardDiff` from two loaded source boards, carrying the coordinate
/// mismatch warning if the formats disagree (same as the CLI startup path).
fn diff_from_sources(old: &LoadedBoard, new: &LoadedBoard) -> anyhow::Result<BoardDiff> {
    let mut d = etchy_core::compare_detailed(&old.board, &new.board)?;
    if let (Some(o), Some(n)) = (old.fmt.as_ref(), new.fmt.as_ref()) {
        if let Some(w) = etchy_core::coordinate_mismatch_warning(o, n) {
            d.report.warnings.push(w);
        }
    }
    Ok(d)
}

/// Derive a human-meaningful old/new label from an input path (#177), shared by
/// the native (dir args / drops) and web (demo) surfaces so both present the same
/// old->new text. A bare basename is often generic — "old"/"new"/"a" or a plain
/// version like "v0.0.1" — and carries no board identity; when it is, prefix the
/// parent directory name (".../Mad_RP2040/old" -> "Mad_RP2040 old",
/// "Mad_RP2040/v0.0.0" -> "Mad_RP2040 v0.0.0"). Otherwise the basename stands.
fn derive_label(path: &std::path::Path) -> String {
    let base = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim();
    if base.is_empty() {
        return "?".to_string();
    }
    if is_generic_rev_name(base) {
        if let Some(parent) = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
        {
            let parent = parent.trim();
            if !parent.is_empty() && !is_generic_rev_name(parent) {
                return format!("{parent} {base}");
            }
        }
    }
    base.to_string()
}

/// Whether a directory-name segment is a generic revision marker that carries no
/// board identity on its own — a bare rev word or a plain version string — so the
/// label derivation knows to borrow the parent directory name instead (#177).
fn is_generic_rev_name(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    matches!(
        n.as_str(),
        "old" | "new" | "a" | "b" | "before" | "after" | "prev" | "previous" | "current" | "curr"
    ) || is_version_like(&n)
}

/// A plain version string like "v1", "v0.0.1", "1.0", "2" — an optional leading
/// 'v' then only digits and dots. Such a name names a revision, not the board.
fn is_version_like(name: &str) -> bool {
    let s = name.strip_prefix('v').unwrap_or(name);
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Basename of a filename string (handles `/` and `\`). Web-only (native uses
/// `path_label`).
#[cfg(target_arch = "wasm32")]
fn basename(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_string()
}

#[cfg(not(target_arch = "wasm32"))]
fn path_label(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

/// Native: turn dropped paths into a loaded board. A single dropped folder or
/// `.zip` loads directly; multiple dropped files are read as individual layers.
#[cfg(not(target_arch = "wasm32"))]
fn load_dropped_paths(paths: &[std::path::PathBuf]) -> anyhow::Result<LoadedBoard> {
    use anyhow::Context as _;
    if paths.len() == 1 {
        let p = &paths[0];
        let is_zip = p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false);
        if p.is_dir() || is_zip {
            let (board, fmt) = loader::load_source(p)?;
            return Ok(LoadedBoard {
                label: path_label(p),
                board,
                fmt,
            });
        }
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    for p in paths {
        let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        files.push((path_label(p), bytes));
    }
    let (board, fmt) = loader::board_from_bytes(files)?;
    Ok(LoadedBoard {
        label: format!("{} files", paths.len()),
        board,
        fmt,
    })
}

impl ViewApp {
    /// Startup splash — the etchy wordmark over the board-dark, held briefly then
    /// faded out to reveal the app. Painted on the foreground layer so it covers
    /// every panel; any pointer press or key skips it, and it's time-based so it
    /// clears itself even if the window never gets focus.
    fn splash_ui(&mut self, ui: &egui::Ui, now: f64) {
        if self.splash_done {
            return;
        }
        let start = *self.splash_start.get_or_insert(now);
        let elapsed = now - start;
        let dismiss = ui.input(|i| i.pointer.any_pressed() || !i.keys_down.is_empty());
        if dismiss || elapsed >= SPLASH_HOLD_SECS + SPLASH_FADE_SECS {
            self.splash_done = true;
            return;
        }
        let alpha = if elapsed <= SPLASH_HOLD_SECS {
            1.0
        } else {
            1.0 - ((elapsed - SPLASH_HOLD_SECS) / SPLASH_FADE_SECS) as f32
        }
        .clamp(0.0, 1.0);
        let screen = ui.ctx().content_rect();
        let p = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("etchy-splash"),
        ));
        // Board-dark curtain fading to reveal the app underneath.
        p.rect_filled(screen, 0.0, with_alpha(C_CANVAS, alpha));
        let c = screen.center();
        p.text(
            c - egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            "etchy",
            egui::FontId::proportional(64.0),
            with_alpha(C_COPPER, alpha),
        );
        p.text(
            c + egui::vec2(0.0, 34.0),
            egui::Align2::CENTER_CENTER,
            "PCB visual + geometric diff",
            egui::FontId::proportional(17.0),
            with_alpha(C_CREAM, alpha * 0.85),
        );
        ui.ctx().request_repaint(); // keep the fade animating
    }
}

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Loader (#120): drain any async web file picks, then accept drag-and-drop.
        #[cfg(target_arch = "wasm32")]
        self.poll_file_picks();
        self.handle_dropped_files(ui.ctx());

        // Startup splash (foreground overlay; skipped once done). Drawn before the
        // panels so it covers the welcome screen too, but composited on top.
        let now = ui.ctx().input(|i| i.time);
        self.splash_ui(ui, now);

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
            escape,
            cycle_unit,
            toggle_grid,
            toggle_measure,
        ) = ui.input(|i| {
            use egui::Key;
            if typing {
                return (
                    false, false, false, false, false, false, false, false, false, false, false,
                    false, false,
                );
            }
            (
                i.key_pressed(Key::S),
                i.key_pressed(Key::F),
                // Mode hotkeys (#55, #61): 1=Overlay 2=Old 3=New 4=Split 5=Swipe,
                // + aliases O/B/A (B/A kept as legacy Old/New mnemonics).
                i.key_pressed(Key::Num1) || i.key_pressed(Key::O),
                i.key_pressed(Key::Num2) || i.key_pressed(Key::B),
                i.key_pressed(Key::Num3) || i.key_pressed(Key::A),
                i.key_pressed(Key::Num4),
                i.key_pressed(Key::Num5),
                i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J),
                i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::U),
                i.key_pressed(Key::G),
                i.key_pressed(Key::M),
            )
        });
        if toggle_measure {
            // M arms/disarms measure mode (#50), mirroring the rail Measure icon and
            // the top-bar control. Suppressed while typing via the `typing` guard.
            self.measure_mode = !self.measure_mode;
            if !self.measure_mode {
                self.measure_pts.clear();
            }
        }
        if escape {
            if self.show_settings {
                // Esc backs out of the Settings window (#4). An open colour-picker
                // popup consumes the first Esc itself (egui closes it; while its RGB
                // field has focus our `typing` guard suppresses this handler), so the
                // next Esc lands here and closes the window.
                self.show_settings = false;
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
            // S still steps the three familiar off/faint/strong stops (#12/#6); the
            // Layers-panel slider handles fine-grained values.
            self.base_opacity = cycle_base_opacity(self.base_opacity);
        }
        if fit {
            self.cam.fitted = false;
        }
        if mode_overlay {
            self.mode = Mode::Overlay;
        }
        if mode_before {
            self.mode = Mode::Old;
        }
        if mode_after {
            self.mode = Mode::New;
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
            // Title row: the etchy E monogram (Feature 8, replacing the old "etchy"
            // wordmark), the revisions, and the headline totals.
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                etchy_monogram(ui, 26.0);
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
            // Controls row (#57): ONE non-wrapping row. Left = the segmented mode
            // picker (never collapses); right = the action cluster, which folds into a
            // "More" menu when the window is narrow; the flexible middle carries the
            // warnings chip + transient export status. The bar never wraps — it
            // collapses by width tier instead (replaces the old wrapped row, #5/#57).
            // Moved OUT of the bar: base opacity → Layers panel slider (#12/#6), noise
            // filter → Settings > Diff (#154), board edge → Layers panel (#157), Open
            // A/B → the Open menu (#160), GPU checkbox (already in Settings > Display).
            // The top bar spans the full window width (laid out above the left panel),
            // so the window width is the reliable tier measure — available_width inside
            // the nested layout doesn't reflect the true bar width.
            let avail = ui.ctx().content_rect().width();
            let collapse_actions = avail < TIER_MORE_PX;
            let now = ui.ctx().input(|i| i.time);
            // Action intents, set in the (self-borrowing) closures and acted on after.
            let mut fit = false;
            let mut exp_current = false;
            let mut exp_all = false;
            let mut toggle_measure = false;
            let mut toggle_settings = false;
            let mut open_side: Option<RevSide> = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                // Mode picker — the core control, never collapses.
                segmented(
                    ui,
                    &mut self.mode,
                    &[
                        (Mode::Overlay, "Overlay"),
                        (Mode::Old, "Old"),
                        (Mode::New, "New"),
                        (Mode::Split, "Split"),
                        (Mode::Swipe, "Swipe"),
                    ],
                );
                // Right-aligned action cluster. RTL adds in reverse, so the visual
                // order is Open · Fit · Measure · Export · Settings · Help.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if collapse_actions {
                        ui.menu_button("More", |ui| {
                            if ui.button("Open old revision…").clicked() {
                                open_side = Some(RevSide::Old);
                                ui.close();
                            }
                            if ui.button("Open new revision…").clicked() {
                                open_side = Some(RevSide::New);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Fit view").clicked() {
                                fit = true;
                                ui.close();
                            }
                            if ui.selectable_label(self.measure_mode, "Measure").clicked() {
                                toggle_measure = true;
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Export current layer").clicked() {
                                exp_current = true;
                                ui.close();
                            }
                            if ui.button("Export all changed layers").clicked() {
                                exp_all = true;
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Settings").clicked() {
                                toggle_settings = true;
                                ui.close();
                            }
                            ui.menu_button("Help", help_links);
                        });
                    } else {
                        ui.menu_button("Help", help_links);
                        if ui
                            .selectable_label(self.show_settings, "Settings")
                            .clicked()
                        {
                            toggle_settings = true;
                        }
                        ui.menu_button("Export", |ui| {
                            if ui.button("Current layer").clicked() {
                                exp_current = true;
                                ui.close();
                            }
                            if ui.button("All changed layers").clicked() {
                                exp_all = true;
                                ui.close();
                            }
                            ui.separator();
                            ui.label("SVG per layer + areas.csv (copper mm²).");
                        });
                        if ui
                            .selectable_label(self.measure_mode, "Measure")
                            .on_hover_text(
                                "Click two points on the canvas to measure the distance. \
                                 Completed measurements stay in the Measure tab list and on \
                                 the board; toggle off to stop measuring.",
                            )
                            .clicked()
                        {
                            toggle_measure = true;
                        }
                        if ui.button("Fit").clicked() {
                            fit = true;
                        }
                        ui.menu_button("Open", |ui| {
                            if ui.button("Old revision…").clicked() {
                                open_side = Some(RevSide::Old);
                                ui.close();
                            }
                            if ui.button("New revision…").clicked() {
                                open_side = Some(RevSide::New);
                                ui.close();
                            }
                            ui.separator();
                            ui.label(
                                egui::RichText::new(
                                    "…or drag a folder / .zip onto the window\n\
                                     (if the file dialog doesn't open)",
                                )
                                .weak()
                                .small(),
                            );
                        });
                    }
                    // Flexible middle (left of the actions in RTL): transient export
                    // status + the warnings chip (stays in-row, never reflows the
                    // canvas — #49).
                    if let Some(msg) = &self.export_msg {
                        ui.label(egui::RichText::new(msg).weak().small());
                    }
                    self.warnings_ui(ui, now);
                });
            });
            // Act on the collected intents (outside the closures that borrow self).
            if fit {
                self.cam.fitted = false;
            }
            if exp_current {
                self.do_export(false);
            }
            if exp_all {
                self.do_export(true);
            }
            if toggle_measure {
                self.measure_mode = !self.measure_mode;
                if !self.measure_mode {
                    self.measure_pts.clear();
                }
            }
            if toggle_settings {
                self.show_settings = !self.show_settings;
            }
            if let Some(side) = open_side {
                self.open_primary(side, ui.ctx());
            }
        });

        // No board loaded yet → welcome / open screen. Returning here skips the
        // layer list + canvas, which assume at least one layer (#120).
        if self.diff.layers.is_empty() {
            self.welcome_ui(ui);
            return;
        }

        // A reopen that failed while a board is already shown: banner it (the
        // welcome screen shows the same error inline in the empty state).
        if let Some(err) = self.load_error.clone() {
            egui::Panel::top("load_error").show_inside(ui, |ui| {
                ui.colored_label(C_REMOVED, format!("[!] load failed — {err}"));
            });
        }

        // Activity rail (Feature 1): a slim VS Code-style strip on `rail_side`
        // (Feature 8) — the E monogram on top, one icon per side panel, the
        // Settings cog pinned at the bottom. Replaces the always-open left Layers
        // panel; the Layers body moved verbatim into `layers_panel_ui`, so this
        // slice is layout-only.
        let rail = match self.rail_side {
            RailSide::Left => egui::Panel::left("rail"),
            RailSide::Right => egui::Panel::right("rail"),
        };
        rail.exact_size(48.0)
            .resizable(false)
            .show_inside(ui, |ui| {
                // Settings cog pinned to the bottom of the rail.
                egui::Panel::bottom("rail_settings")
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(4.0);
                        ui.vertical_centered(|ui| {
                            if rail_button(ui, self.show_settings, draw_cog_icon)
                                .on_hover_text("Settings")
                                .clicked()
                            {
                                self.show_settings = !self.show_settings;
                            }
                        });
                        ui.add_space(4.0);
                    });
                // Monogram + panel tabs fill the rest, top-down.
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        etchy_monogram(ui, 30.0);
                        ui.add_space(10.0);
                        for tab in PanelTab::ALL {
                            let active = self.active_panel == Some(tab);
                            let hover = if tab == PanelTab::Measure {
                                "Measure — arms the tool and opens the panel (M)"
                            } else {
                                tab.label()
                            };
                            if rail_button(ui, active, |p, r, c| draw_panel_icon(tab, p, r, c))
                                .on_hover_text(hover)
                                .clicked()
                            {
                                if tab == PanelTab::Measure {
                                    // The Measure icon both arms the tool and opens
                                    // the panel; re-clicking disarms + collapses. It
                                    // arms even while the panel is collapsed (#50).
                                    let (next, armed) = measure_rail_click(self.active_panel);
                                    self.active_panel = next;
                                    self.measure_mode = armed;
                                    if !armed {
                                        self.measure_pts.clear();
                                    }
                                } else {
                                    self.active_panel = toggle_panel(self.active_panel, tab);
                                }
                            }
                            ui.add_space(2.0);
                        }
                    });
                });
            });

        // The docked panel the rail drives; shown only when a tab is active
        // (`None` = collapsed, canvas full width), on the same edge as the rail.
        if let Some(tab) = self.active_panel {
            let panel = match self.rail_side {
                RailSide::Left => egui::Panel::left("panel"),
                RailSide::Right => egui::Panel::right("panel"),
            };
            panel
                .resizable(true)
                .default_size(260.0)
                .show_inside(ui, |ui| match tab {
                    PanelTab::Layers => self.layers_panel_ui(ui),
                    PanelTab::Measure => self.measure_panel_ui(ui),
                    PanelTab::Export => self.export_panel_ui(ui),
                });
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.draw_canvas(ui);
        });

        // Settings editor — a real Window (not a menu) so the nested colour-picker
        // popup works; a menu_button closed on the first click inside it.
        if self.show_settings {
            let mut open = true;
            // Open centered on the screen (#56): pin the first-frame position to the
            // viewport centre via a CENTER_CENTER pivot. egui remembers the dragged
            // position afterwards, so it stays movable.
            let center = ui.ctx().content_rect().center();
            egui::Window::new("Settings")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_pos(center)
                .pivot(egui::Align2::CENTER_CENTER)
                .show(ui.ctx(), |ui| {
                    // Copper selection accent for this window (#121): selected rail
                    // entry, theme/units/preset toggles read brand-copper, not the
                    // default blue.
                    ui.visuals_mut().selection.bg_fill = C_COPPER.gamma_multiply(0.30);
                    ui.visuals_mut().selection.stroke = egui::Stroke::new(1.0, C_COPPER);
                    ui.set_min_width(432.0);
                    ui.horizontal_top(|ui| {
                        // Left rail: one section at a time, so the long per-layer list
                        // no longer buries Display/Grid/Input/Colours.
                        ui.vertical(|ui| {
                            ui.set_width(96.0);
                            for (tab, label) in SettingsTab::ALL {
                                let on = self.settings_tab == tab;
                                let text = if on {
                                    egui::RichText::new(label).color(C_COPPER).strong()
                                } else {
                                    egui::RichText::new(label)
                                };
                                if ui.selectable_label(on, text).clicked() {
                                    self.settings_tab = tab;
                                }
                            }
                        });
                        ui.separator();
                        // Content pane.
                        ui.vertical(|ui| {
                            ui.set_min_width(312.0);
                            match self.settings_tab {
                                SettingsTab::Display => self.settings_display(ui),
                                SettingsTab::Diff => self.settings_diff(ui),
                                SettingsTab::Grid => self.settings_grid(ui),
                                SettingsTab::Input => self.settings_input(ui),
                                SettingsTab::Colours => self.settings_colours(ui),
                                SettingsTab::Layers => self.settings_layers(ui),
                            }
                        });
                    });
                });
            self.show_settings = open;
        }

        // Publish "what they're looking at" for the web feedback widget.
        let layer_name = self.diff.layers[self.selected].name().to_string();
        let mode = match self.mode {
            Mode::Overlay => "Overlay",
            Mode::Old => "Old",
            Mode::New => "New",
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
    /// The Layers panel body (Feature 1): moved out of `fn ui` so the rail slice stays
    /// layout-only. Holds all layer behaviour — eye toggles, groups, view modes, Δ%,
    /// per-layer colour. The board outline is a normal layer row here now (#157).
    fn layers_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Layers");
        // Quick visibility actions (#58): show/hide every layer, or only the
        // changed ones. They never move the selection or camera.
        ui.horizontal(|ui| {
            if ui.small_button("Show all").clicked() {
                for v in self.visible_layers.iter_mut() {
                    *v = true;
                }
            }
            if ui.small_button("Hide all").clicked() {
                // Hide-all clears EVERY layer (#2) — including the selected
                // one. (Split/Swipe still force the active layer visible in
                // those modes so their view is never blank.)
                for v in self.visible_layers.iter_mut() {
                    *v = false;
                }
            }
            // "Show changed" button hidden per feedback #8 — the capability
            // stays in `visible_from_changed` (still unit-tested) so it can be
            // re-surfaced later, but the button is removed from the row.
        });
        // View mode (#59): a quick preset over the per-layer checkboxes —
        // single active layer / highlight active over dimmed rest / all equal.
        // Selecting one resets visibility to the preset; per-row checkboxes
        // still fine-tune afterwards.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("view").weak().small());
            ui.visuals_mut().selection.bg_fill = C_COPPER.gamma_multiply(0.30);
            ui.visuals_mut().selection.stroke = Stroke::new(1.0, C_COPPER);
            for (mode, label) in ViewMode::ALL {
                if ui.selectable_label(self.view_mode == mode, label).clicked() {
                    self.view_mode = mode;
                    self.visible_layers = visibility_for_mode(
                        mode,
                        self.diff.layers.len(),
                        self.selected,
                        self.outline,
                    );
                }
            }
        });
        // Base opacity (#12/#6): the unchanged base copper's strength, moved here
        // from the top bar and made continuous. 0 hides the base; the old off/faint/
        // strong stops are 0%/40%/80%. `S` still steps those three stops.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("base").weak().small());
            ui.add(
                egui::Slider::new(&mut self.base_opacity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    // Parse the "%"-formatted text back so click-to-type round-trips
                    // (without a matching parser egui's default numeric parse rejects
                    // the "%" suffix and the typed value is silently dropped).
                    .custom_parser(|s| {
                        s.trim()
                            .trim_end_matches('%')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|p| p / 100.0)
                    }),
            )
            .on_hover_text("Opacity of the unchanged base copper behind the diff (0 hides it).");
        });
        ui.separator();
        // Actions deferred so the per-frame group iteration doesn't borrow
        // self mutably while it's borrowed for the group list.
        let mut select: Option<usize> = None;
        let mut toggle: Option<(usize, bool)> = None; // (layer, show)
        let mut group_set: Option<(Vec<usize>, bool)> = None; // (idxs, show)
        let mut set_color: Option<(usize, Color32)> = None; // (layer, colour) (#3)
        egui::ScrollArea::vertical().show(ui, |ui| {
            // The board outline is a normal layer row now (#157) — it lives under
            // Mechanical > outline with an ordinary eye toggle, no separate "board
            // edge" reference control. Its visibility drives the faint orientation
            // outline drawn on every layer (see draw_canvas / build_cache).
            // Group into sections (copper / mask / silk / …) in fixed order,
            // changed-first within each (G5).
            let groups = group_layers(&self.order, |i| layer_group(self.diff.layers[i].kind));
            for (group, idxs) in groups {
                ui.add_space(4.0);
                // Group header (#36 collapse + #58 show/hide-all): a
                // CollapsingState lets the header carry BOTH the disclosure
                // triangle (rotates down=open / right=collapsed) AND a group
                // show/hide-all checkbox; the body holds the layer rows. egui
                // (with eframe persistence) remembers each group's open state.
                let gid = ui.make_persistent_id(("layer-group", group.title()));
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    gid,
                    true,
                );
                state
                    .show_header(ui, |ui| {
                        // Show/hide every layer in the group (#58), same eye
                        // toggle as the rows (#4). Separate from collapsing,
                        // which only hides the list rows.
                        let all = group_all_visible(&self.visible_layers, &idxs);
                        if eye_toggle(ui, all)
                            .on_hover_text("Show / hide every layer in this group")
                            .clicked()
                        {
                            group_set = Some((idxs.clone(), !all));
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
                            let visible = self.visible_layers.get(idx).copied().unwrap_or(false);
                            let swatch =
                                resolve_base_color(idx, kind, &self.base_overrides, self.theme);
                            let resp = ui
                                .horizontal(|ui| {
                                    // Per-layer visibility: an Altium-style
                                    // eye toggle, separate from the
                                    // click-to-select label (#4/#58).
                                    if eye_toggle(ui, visible)
                                        .on_hover_text("Show / hide this layer")
                                        .clicked()
                                    {
                                        toggle = Some((idx, !visible));
                                    }
                                    // Clickable colour swatch (#3): opens this
                                    // layer's colour picker; a change records a
                                    // per-layer base override (applied below).
                                    let mut sw = swatch;
                                    if square_color_swatch(ui, &mut sw)
                                        .on_hover_text("Layer colour — click to change")
                                        .changed()
                                    {
                                        set_color = Some((idx, sw));
                                    }
                                    // Visible layers read brighter; hidden grey.
                                    let label = match (visible, changed) {
                                        (true, true) => egui::RichText::new(&name).strong(),
                                        (true, false) => egui::RichText::new(&name),
                                        (false, _) => {
                                            egui::RichText::new(&name).weak().color(Color32::GRAY)
                                        }
                                    };
                                    let r = ui.selectable_label(idx == self.selected, label);
                                    // Compact %-change micro-label on changed
                                    // layers (#114): the changed area as a
                                    // share of the layer's new-revision area.
                                    // The mm² deltas move into the hover text so
                                    // the row stays scannable. If the layer is
                                    // gone in the new rev (area 0), fall back to
                                    // the raw deltas.
                                    if changed {
                                        let area =
                                            self.new_area_mm2.get(idx).copied().unwrap_or(0.0);
                                        let txt = if area > 0.0 {
                                            format!("Δ {:.1}%", (added + removed) / area * 100.0)
                                        } else {
                                            format!("+{added:.3} −{removed:.3}")
                                        };
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                // Δ% in copper so the change
                                                // magnitude reads at a glance (#20).
                                                ui.label(
                                                    egui::RichText::new(txt)
                                                        .small()
                                                        .color(C_COPPER),
                                                )
                                                .on_hover_text(format!(
                                                    "+{added:.4} mm² added · \
                                                             −{removed:.4} mm² removed · \
                                                             layer area {area:.3} mm²"
                                                ));
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
    }

    /// Measure tab (stub). Feature 3 fills this with the measurement list, snap
    /// and crosshair toggles, and unit selector; the seam lives here for PR A.
    fn measure_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Measure");
        ui.add_space(6.0);

        // Armed toggle — mirrors the rail Measure icon and the M hotkey. Arming lets
        // canvas clicks drop ruler points; disarming clears the in-progress point
        // but keeps the completed list.
        let mut armed = self.measure_mode;
        if ui
            .checkbox(&mut armed, "Armed — click two points on the canvas")
            .on_hover_text(
                "Arm the measure tool (also the rail Measure icon or the M key). \
                 Click two points on the canvas to add a measurement.",
            )
            .changed()
        {
            self.measure_mode = armed;
            if !armed {
                self.measure_pts.clear();
            }
        }

        ui.add_space(6.0);
        ui.checkbox(&mut self.snap_grid, "Snap clicks to grid")
            .on_hover_text("Snap each placed point to the nearest grid intersection (#51).");
        ui.checkbox(&mut self.show_crosshair, "Cursor crosshair + readout")
            .on_hover_text(
                "Show a crosshair and live coordinates at the cursor, always \
                 (not only while measuring).",
            );

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Units");
            ui.selectable_value(&mut self.measure_unit, Unit::Mm, "mm");
            ui.selectable_value(&mut self.measure_unit, Unit::Mil, "mil");
            ui.selectable_value(&mut self.measure_unit, Unit::Inch, "inch");
        });

        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Measurements ({})", self.measurements.len())).strong(),
            );
            if !self.measurements.is_empty() {
                // Push "Clear all" to the trailing edge.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear all").clicked() {
                        self.measurements.clear();
                    }
                });
            }
        });
        ui.add_space(4.0);

        if self.measurements.is_empty() {
            ui.label(
                egui::RichText::new("No measurements yet. Arm the tool and click two points.")
                    .weak()
                    .small(),
            );
        } else {
            let unit = self.measure_unit;
            let mut remove: Option<usize> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, m) in self.measurements.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove this measurement")
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            ui.label(format_distance(distance_mm(m.a, m.b), unit));
                        });
                    }
                });
            if let Some(i) = remove {
                measurement_remove(&mut self.measurements, i);
            }
        }
    }

    /// Export tab (#60): surfaces the existing export (per-layer SVG + copper-area
    /// CSV) in the panel. Previews *what* each action writes and *which formats*,
    /// then runs the same `do_export` the top-bar Export menu calls. The top-bar
    /// menu stays until PR C shrinks the top bar; both drive one code path.
    fn export_panel_ui(&mut self, ui: &mut egui::Ui) {
        // Preview data — owned up front so the render below never borrows `self`,
        // leaving `do_export(&mut self)` free to run afterwards. Derived from the
        // layer set, not by generating the (expensive) SVG content.
        let current_names: Vec<String> = self
            .diff
            .layers
            .get(self.selected)
            .map(|l| l.name())
            .into_iter()
            .collect();
        let changed_names: Vec<String> = self
            .diff
            .layers
            .iter()
            .filter(|l| l.is_changed())
            .map(|l| l.name())
            .collect();
        let current_files = export_file_names(&current_names);
        let all_files = export_file_names(&changed_names);
        let status = self.export_msg.clone();

        ui.heading("Export");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Write the diff to files you can open outside etchy.")
                .weak()
                .small(),
        );

        ui.add_space(10.0);
        ui.label(egui::RichText::new("Formats").color(C_COPPER).strong());
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("SVG — one vector file per layer (viewBox in mm).")
                .weak()
                .small(),
        );
        ui.label(
            egui::RichText::new(
                "CSV — areas.csv: copper area per layer, mm² (old / new / added / removed).",
            )
            .weak()
            .small(),
        );

        let mut exp_current = false;
        let mut exp_all = false;

        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Current layer")
                .color(C_COPPER)
                .strong(),
        );
        ui.add_space(2.0);
        match current_names.first() {
            Some(name) => {
                ui.label(
                    egui::RichText::new(format!("{name} — {} file(s)", current_files.len()))
                        .weak()
                        .small(),
                );
                Self::export_file_list(ui, "current", &current_files);
            }
            None => {
                ui.label(egui::RichText::new("No layer selected.").weak().small());
            }
        }
        if ui
            .add_enabled(
                !current_names.is_empty(),
                egui::Button::new("Export current layer"),
            )
            .on_hover_text("Write the selected layer's SVG plus areas.csv.")
            .clicked()
        {
            exp_current = true;
        }

        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("All changed layers ({})", changed_names.len()))
                .color(C_COPPER)
                .strong(),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(format!("{} file(s)", all_files.len()))
                .weak()
                .small(),
        );
        Self::export_file_list(ui, "all", &all_files);
        if ui
            .button("Export all changed layers")
            .on_hover_text("Write an SVG for every changed layer plus areas.csv.")
            .clicked()
        {
            exp_all = true;
        }

        if let Some(msg) = status {
            ui.add_space(10.0);
            ui.separator();
            ui.label(egui::RichText::new(msg).weak().small());
        }

        // Run after the render above — `do_export` needs `&mut self`.
        if exp_current {
            self.do_export(false);
        }
        if exp_all {
            self.do_export(true);
        }
    }

    /// Render an export file-name preview list in a bounded, scrollable box.
    /// `salt` distinguishes the two lists (current vs all) so their scroll state
    /// doesn't collide.
    fn export_file_list(ui: &mut egui::Ui, salt: &str, files: &[String]) {
        egui::ScrollArea::vertical()
            .id_salt(("export-files", salt))
            .max_height(120.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for f in files {
                    ui.label(egui::RichText::new(f).monospace().small().weak());
                }
            });
    }

    /// A copper section heading for the Settings panes (#121).
    fn settings_header(ui: &mut egui::Ui, text: &str) {
        ui.add_space(1.0);
        ui.label(egui::RichText::new(text).color(C_COPPER).strong());
        ui.add_space(3.0);
    }

    /// Settings → Display: theme, measure units, and (feature build) the GPU path.
    fn settings_display(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Display");
        ui.horizontal(|ui| {
            ui.label("Theme");
            ui.selectable_value(&mut self.theme, Theme::Dark, "dark");
            ui.selectable_value(&mut self.theme, Theme::Light, "light");
        });
        ui.horizontal(|ui| {
            ui.label("Measure units");
            ui.selectable_value(&mut self.measure_unit, Unit::Mm, "mm");
            ui.selectable_value(&mut self.measure_unit, Unit::Inch, "inch");
            ui.selectable_value(&mut self.measure_unit, Unit::Mil, "mil");
        });
        ui.horizontal(|ui| {
            ui.label("Activity rail");
            ui.selectable_value(&mut self.rail_side, RailSide::Left, "left");
            ui.selectable_value(&mut self.rail_side, RailSide::Right, "right");
        })
        .response
        .on_hover_text("Which edge the activity rail and its panel dock to (Feature 8).");
        #[cfg(feature = "gpu-transform")]
        if self.gpu.is_some() {
            ui.checkbox(&mut self.use_gpu, "GPU base transform (experimental)")
                .on_hover_text(
                    "Transform the base layer on the GPU (#106). \
                     Off falls back to the CPU path.",
                );
        }
    }

    /// Settings → Diff: the noise-filter threshold (moved off the top bar, #154).
    fn settings_diff(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Diff");
        ui.label(
            egui::RichText::new("Noise filter")
                .small()
                .color(Color32::from_gray(150)),
        );
        // Linear range 0..=0.1 mm² (#52/#23).
        ui.add(
            egui::Slider::new(&mut self.min_area_mm2, 0.0..=0.1)
                .text("mm²")
                .fixed_decimals(4),
        )
        .on_hover_text("Drop diff regions smaller than this as noise; 0 = off.");
        // Editable field for any value beyond the slider's max (#23).
        ui.add(
            egui::DragValue::new(&mut self.min_area_mm2)
                .speed(0.001)
                .range(0.0..=f64::INFINITY)
                .fixed_decimals(4),
        )
        .on_hover_text(
            "Type or drag to set the noise filter exactly (mm²), beyond the slider's range.",
        );
    }

    /// Settings → Grid: reference grid overlay + snap.
    fn settings_grid(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Grid");
        ui.checkbox(&mut self.show_grid, "Show reference grid (G)");
        ui.horizontal(|ui| {
            ui.label("Spacing");
            ui.add(
                egui::DragValue::new(&mut self.grid_mm)
                    .speed(0.1)
                    .range(0.01..=100.0)
                    .suffix(" mm"),
            );
        });
        ui.checkbox(
            &mut self.snap_grid,
            "Snap measure clicks to grid intersections",
        );
        ui.checkbox(
            &mut self.show_crosshair,
            "Cursor crosshair + coordinate readout",
        )
        .on_hover_text(
            "Show a crosshair and live coordinates at the cursor, always \
                 (not only in measure mode). Snaps to the grid when snap is on.",
        );
    }

    /// Settings → Input: pan/zoom scheme matching the user's ECAD tool (#54).
    fn settings_input(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Input");
        ui.horizontal(|ui| {
            ui.label("ECAD preset");
            ui.selectable_value(&mut self.input_preset, InputPreset::Altium, "Altium");
            ui.selectable_value(&mut self.input_preset, InputPreset::KiCad, "KiCad");
        })
        .response
        .on_hover_text(
            "Pan mouse button by ECAD tool: \
             Altium = right-drag, KiCad = middle/right-drag.",
        );
    }

    /// Settings → Colours: diff colours + per-theme canvas/grid (#53/#31/#52).
    fn settings_colours(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Diff colours");
        // One-click palette presets (#155); the pickers below still fine-tune.
        ui.horizontal(|ui| {
            ui.label("Preset");
            ui.visuals_mut().selection.bg_fill = C_COPPER.gamma_multiply(0.30);
            ui.visuals_mut().selection.stroke = Stroke::new(1.0, C_COPPER);
            for (pal, label) in DiffPalette::ALL {
                let (a, r) = pal.colors();
                let active = self.col_added == a && self.col_removed == r;
                if ui.selectable_label(active, label).clicked() {
                    self.col_added = a;
                    self.col_removed = r;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("added");
            ui.color_edit_button_srgba(&mut self.col_added);
            ui.label("removed");
            ui.color_edit_button_srgba(&mut self.col_removed);
        });
        ui.add_space(6.0);
        // The pickers edit the ACTIVE theme; switch dark/light to tune the other,
        // so a charcoal canvas never bleeds into light mode.
        let theme_name = match self.theme {
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        Self::settings_header(ui, &format!("Canvas & grid ({theme_name} mode)"));
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
    }

    /// Settings → Layers: one base-colour row per layer (#21), scrollable.
    fn settings_layers(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Layer base colours");
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for idx in 0..self.diff.layers.len() {
                    let kind = self.diff.layers[idx].kind;
                    let label = self.diff.layers[idx].name();
                    ui.horizontal(|ui| {
                        let mut base =
                            resolve_base_color(idx, kind, &self.base_overrides, self.theme);
                        if square_color_swatch(ui, &mut base).changed() {
                            if let Some(e) = self.base_overrides.iter_mut().find(|(i, _)| *i == idx)
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
    }

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
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        // Clicking the board dismisses the Settings window (#19).
        if self.show_settings && response.clicked() {
            self.show_settings = false;
        }
        let rect = response.rect;
        // The board background uses the user-configurable canvas colour (#53),
        // defaulting to the brand board-dark.
        painter.rect_filled(rect, 0.0, self.canvas_color());

        // Fit on first show / explicit Fit only. Fit frames the WHOLE board (#8),
        // not the selected layer, so the view is stable no matter which layer is
        // active — and selecting a layer never moves it (#4).
        if !self.cam.fitted {
            if let Some(bb) = board_bbox(&self.diff.layers) {
                fit(&mut self.cam, bb, rect);
            }
            self.cam.fitted = true;
        }

        // Swipe/curtain divider (#61): a draggable vertical wipe line. Dragging it
        // takes priority over panning, so when the pointer grabs the divider the
        // pan logic below is skipped for this frame. The handle has a few px of
        // grab tolerance and shows a horizontal-resize cursor on hover.
        let mut swipe_dragging = false;
        // Hover-or-drag on the divider (#61) — drives a heavier, highlighted handle
        // in the draw pass so it reads as grabbable.
        let mut swipe_hot = false;
        if self.mode == Mode::Swipe {
            // Generous grab band (#61): the old 6px was very hard to hit on a
            // trackpad. A wide band the full height of the divider makes it easy.
            const GRAB_PX: f32 = 16.0;
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
            swipe_hot = near_div || self.swipe_drag;
            if swipe_hot {
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
                    // The second click completes the pair onto the running list; the
                    // next click starts a fresh one (#50).
                    if let Some(m) = measure_click(&mut self.measure_pts, w) {
                        self.measurements.push(m);
                    }
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
        // The board outline is drawn as the faint orientation reference (Side::Full,
        // on every layer / into both split halves), NOT as a stacked base layer — so
        // its `visible_layers` bit gates that reference (`outline_effective`) and is
        // kept OUT of the stacked `visible` set below (#157: board edge is a normal
        // layer row, but it still renders faint, not as bright copper).
        let outline_visible = self
            .outline
            .is_some_and(|oi| self.visible_layers.get(oi).copied().unwrap_or(false));
        // The visible set: every layer the user has shown (#58/#59), minus the
        // outline (handled above). Split renders the active layer only (a stacked
        // old|new of many layers reads as mud), so it keys off just the selected layer
        // and falls back to it when nothing is on.
        let visible = if self.mode == Mode::Split || self.mode == Mode::Swipe {
            vec![self.selected]
        } else {
            let v: Vec<usize> = visible_indices(&self.visible_layers)
                .into_iter()
                .filter(|&i| Some(i) != self.outline)
                .collect();
            // Anti-blank fallback: if nothing is on, show the selected layer so the
            // canvas isn't empty. But the outline is a normal layer now (PR B) — if
            // the user hid everything and toggled ONLY the outline on, that IS
            // content, so don't force the (hidden) selected layer back on.
            let outline_visible = self
                .outline
                .and_then(|o| self.visible_layers.get(o).copied())
                .unwrap_or(false);
            if v.is_empty() && !outline_visible {
                vec![self.selected]
            } else {
                v
            }
        };
        let key = build_geom_key(
            &visible,
            self.mode,
            self.base_opacity,
            outline_visible,
            self.outline,
        );
        if geom_cache_dirty(self.cache.as_ref().map(|c| &c.key), &key) {
            self.cache = Some(build_cache(&self.diff, &key, self.outline));
        }
        // GPU path (#80/#107): upload ALL visible geometry (base + diff + outline)
        // once, with per-vertex colour, whenever the inputs change — then pan/zoom
        // only updates a uniform, so frame time is O(1) in triangle count (the HDI
        // fix). Overlay/Old/New only; Split/Swipe keep the CPU path. The cache
        // borrow is dropped before we set the hash.
        #[cfg(feature = "gpu-transform")]
        {
            let pending = {
                let cache = self.cache.as_ref().expect("cache built above");
                let eligible = self.use_gpu
                    && self.gpu.is_some()
                    && matches!(self.mode, Mode::Overlay | Mode::Old | Mode::New);
                let hash = self.gpu_input_hash(cache);
                if eligible && self.gpu_hash != Some(hash) {
                    Some((self.build_gpu_tris(cache), hash))
                } else {
                    None
                }
            };
            if let Some((tris, hash)) = pending {
                if let Some(g) = &self.gpu {
                    g.upload(&tris);
                }
                self.gpu_hash = Some(hash);
            }
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
                    self.base_opacity,
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
            // The divider: a copper wipe line. In Swipe it's the draggable handle,
            // heavier and brighter when hovered/dragged (#61) so it reads as movable.
            let line_col = if swipe && swipe_hot {
                C_CREAM
            } else {
                C_COPPER
            };
            painter.line_segment(
                [
                    Pos2::new(div_x, rect.top()),
                    Pos2::new(div_x, rect.bottom()),
                ],
                Stroke::new(if swipe { 2.5 } else { 1.5 }, line_col),
            );
            if swipe {
                // A clear grab handle at mid-height: a rounded copper pill with three
                // grip lines, so the divider is an obvious, easy target (#61). It
                // brightens with a cream outline when hovered/dragged.
                let mid_y = rect.center().y;
                let handle =
                    Rect::from_center_size(Pos2::new(div_x, mid_y), egui::vec2(12.0, 48.0));
                painter.rect_filled(handle, 6.0, C_COPPER);
                if swipe_hot {
                    painter.rect_stroke(
                        handle,
                        6.0,
                        Stroke::new(1.5, C_CREAM),
                        egui::StrokeKind::Outside,
                    );
                }
                // Grip lines.
                for dy in [-8.0, 0.0, 8.0] {
                    painter.line_segment(
                        [
                            Pos2::new(div_x - 3.0, mid_y + dy),
                            Pos2::new(div_x + 3.0, mid_y + dy),
                        ],
                        Stroke::new(1.2, C_CANVAS),
                    );
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
            // GPU path active this frame iff a mesh is uploaded for the current
            // inputs (#80). When active, the GPU draws everything and the CPU path
            // is skipped entirely. No per-feature LOD/markers on the GPU path — it
            // draws true-scale; that's the dense/HDI trade for O(1) frames.
            #[cfg(feature = "gpu-transform")]
            let gpu_active = self.use_gpu
                && self.gpu.is_some()
                && self.gpu_hash.is_some()
                && matches!(self.mode, Mode::Overlay | Mode::Old | Mode::New);
            #[cfg(not(feature = "gpu-transform"))]
            let gpu_active = false;

            #[cfg(feature = "gpu-transform")]
            if gpu_active {
                if let Some(g) = &self.gpu {
                    painter.add(g.callback(rect, self.cam.scale, self.cam.center));
                }
            }

            if gpu_active {
                // GPU drew it; the CPU build is skipped (the O(1) win).
                self.last_hidden = 0;
                n = 1;
            } else {
                let (shapes, hidden) = transform_cache(
                    cache,
                    &self.cam,
                    rect,
                    self.base_opacity,
                    self.selected,
                    base_of,
                    self.canvas_color(),
                    self.col_added,
                    self.col_removed,
                    min_area_nm2,
                    false,
                    self.view_mode.dims_others(),
                    self.view_mode.hides_unselected_base(),
                );
                self.last_hidden = hidden;
                n = shapes.len();
                painter.extend(shapes);
            }
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

        // In Old/New the whole board is drawn in its layer colour (not the
        // green "added") — say so, so it's not mistaken for the diff (#3). Split and
        // Swipe likewise show the RAW boards, not the computed diff, and skip the
        // noise filter — spell that out so a filtered Overlay and a raw Split aren't
        // read as disagreeing about "what changed" (#91).
        let mode_note = match self.mode {
            Mode::Old => Some("showing OLD board"),
            Mode::New => Some("showing NEW board"),
            Mode::Split => {
                Some("raw boards: OLD (left) | NEW (right) — diff + noise filter apply in Overlay")
            }
            Mode::Swipe => {
                Some("raw boards, swipe OLD / NEW — diff + noise filter apply in Overlay")
            }
            Mode::Overlay => None,
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

        // The busy per-layer status caption (layer name, status, region counts) was
        // removed from the board surface (#178) to keep it clean — but the colour
        // legend and the two trust/context signals below stay on-canvas, because
        // those must remain visible. The underlying counts (`self.last_hidden`,
        // `layer.change`) are untouched.
        if self.mode == Mode::Overlay {
            legend(&painter, rect, self.col_added, self.col_removed);
            // TRUST — no silent misses (#178): the noise filter's hidden-region
            // count is the one signal that must never disappear (see MIN_AREA_MM2).
            // Surface it as a small top-left chip, and ONLY when something is
            // actually hidden — nothing hidden, nothing drawn. Overlay-only: Split /
            // Swipe / Old / New show raw boards and clear `last_hidden`.
            if let Some(note) = hidden_note(self.last_hidden, self.min_area_mm2) {
                corner_chip(
                    &painter,
                    rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    &note,
                );
            }
        }

        // #112 context hint: when exactly one layer of several is visible, a lone
        // trace reads as "my traces vanished" rather than "one layer of many". A
        // minimal bottom-right chip restores that context without the old busy
        // caption; it shows only in that single-of-many case. The board outline is
        // orientation context, not one of the layers being compared, so it's left out
        // of the "shown" tally (#157) — one real layer + the outline still reads as
        // "1 / N".
        let shown = self
            .visible_layers
            .iter()
            .enumerate()
            .filter(|&(i, &v)| v && Some(i) != self.outline)
            .count();
        if let Some(hint) = single_layer_hint(shown, self.visible_layers.len()) {
            corner_chip(
                &painter,
                rect.right_bottom() + egui::vec2(-8.0, -8.0),
                egui::Align2::RIGHT_BOTTOM,
                &hint,
            );
        }

        // Always-on crosshair + coordinate readout (#179): a snapped-cursor
        // crosshair and a live world-coordinate readout, drawn regardless of measure
        // mode (default on). Snaps to the grid when snap-to-grid is on, so what the
        // readout shows is exactly where a measure click would land. In measure mode
        // the crosshair is always drawn so the ruler stays aligned even if the
        // standalone crosshair is toggled off.
        if self.show_crosshair || self.measure_mode {
            if let Some(ptr) = response.hover_pos() {
                let w_raw = screen_to_world(&self.cam, ptr, rect);
                let w = if self.snap_grid {
                    snap_world_to_grid(w_raw, self.grid_mm)
                } else {
                    w_raw
                };
                let cross_at = Pos2::new(
                    (rect.center().x as f64 + (w[0] - self.cam.center[0]) * self.cam.scale) as f32,
                    (rect.center().y as f64 - (w[1] - self.cam.center[1]) * self.cam.scale) as f32,
                );
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
                // Coordinate readout on a copper chip, offset from the crosshair
                // centre so it doesn't sit under the lines. When snap is on the
                // value is grid-quantised, so the readout says "· grid" (#178) — the
                // three decimals aren't false precision, they're an on-grid point.
                let mm = etchy_core::NM_PER_MM as f64;
                let txt = format_coord_mm(w[0] / mm, w[1] / mm, self.snap_grid);
                measure_label(&painter, cross_at + egui::vec2(46.0, -14.0), &txt);
            }
        }

        // Measure tool overlay (#22/#50): completed rulers from the running list,
        // plus the in-progress point while the tool is armed. Each ruler is a
        // segment with a distance label offset off the line. The cursor crosshair
        // is drawn above (always-on, #179).
        // World [f64;2] → screen, matching world_to_screen's float transform.
        let w2s = |w: [f64; 2]| -> Pos2 {
            let x = rect.center().x as f64 + (w[0] - self.cam.center[0]) * self.cam.scale;
            let y = rect.center().y as f64 - (w[1] - self.cam.center[1]) * self.cam.scale;
            Pos2::new(x as f32, y as f32)
        };
        // Completed measurements persist on-canvas so they stay visible for
        // reference even when the tool is disarmed; the Measure tab list mirrors
        // them (delete/clear there update the canvas too).
        for m in &self.measurements {
            draw_ruler(
                &painter,
                w2s(m.a),
                w2s(m.b),
                &format_distance(distance_mm(m.a, m.b), self.measure_unit),
            );
        }
        if self.measure_mode {
            // In-progress: the first point of the pair (the second click completes
            // it into the list above).
            for w in &self.measure_pts {
                painter.circle_filled(w2s(*w), 3.0, C_COPPER);
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
        // area/extent the diff path computes — from the EXACT outer, not the simplified
        // mesh, so culling/LOD stay accurate.
        let bb = ring_bbox(outer);
        let extent_nm = (bb[2] - bb[0]).max(bb[3] - bb[1]);
        let area_nm2 = lod::ring_area_nm2(outer);
        // Render-only simplification (#94): coarsen the faint base/outline mesh to
        // ~2µm so the fixed 64-gon flashes (round pads/vias) collapse to far fewer
        // triangles, cutting the per-frame transform on dense boards. The diff geometry
        // is untouched (push_diff_items uses the exact contours).
        let simplified: etchy_core::Shape = shape
            .iter()
            .map(|c| etchy_core::simplify_contour(c, BASE_SIMPLIFY_TOL_NM))
            .collect();
        let tris = etchy_core::triangulate_shape(&simplified);
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
            Mode::Old => push_context_items(&mut items, &layer.old, Role::Base, Side::Full, li),
            Mode::New => push_context_items(&mut items, &layer.new, Role::Base, Side::Full, li),
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

/// A rounded **segmented control** (#57): a pill-group of options, zero gap
/// between them, the selected one filled copper with board-dark text. Used for the
/// top-bar mode and base pickers. egui 0.34 has no built-in segmented widget.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    egui::Frame::default()
        .stroke(Stroke::new(1.0, ui.visuals().widgets.inactive.bg_fill))
        .corner_radius(8.0)
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // Selected segment reads brand copper with board-dark text.
                ui.visuals_mut().selection.bg_fill = C_COPPER;
                ui.visuals_mut().selection.stroke = Stroke::NONE;
                for (opt, label) in options {
                    let on = *value == *opt;
                    let text = if on {
                        egui::RichText::new(*label).color(C_CANVAS).strong()
                    } else {
                        egui::RichText::new(*label)
                    };
                    if ui.selectable_label(on, text).clicked() {
                        *value = *opt;
                    }
                }
            });
        });
}

// Width tiers in egui POINTS (screen_rect width; ~half the CSS px at ppp 2). The
// full inline bar's content needs ~900 pt, so below that the actions collapse.
/// Below this window width (pt) the right action cluster collapses into "More".
const TIER_MORE_PX: f32 = 900.0;

/// Running under WSL? WSL sets `WSL_DISTRO_NAME`, and the kernel release contains
/// "microsoft". On real Windows/macOS/Linux this is always false (the /proc read
/// just fails), so it only affects the WSL dev path.
#[cfg(not(target_arch = "wasm32"))]
fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

/// Open a URL in the user's browser (#159). On WSL the Linux browser handlers
/// don't reach the Windows browser, so route through `explorer.exe`; everywhere
/// else use egui's normal handler (Win32 ShellExecute / macOS `open` / a Linux
/// desktop's opener).
fn open_url(ctx: &egui::Context, url: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if is_wsl() {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
        return;
    }
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// The Help menu's links + version — shared by the top bar's Help button and the
/// narrow "More" menu (#57). Uses [`open_url`] so the links work on WSL too (#159).
fn help_links(ui: &mut egui::Ui) {
    for (label, url) in [
        ("etchy on GitHub", URL_REPO),
        ("Website", URL_SITE),
        ("Report an issue", URL_ISSUES),
    ] {
        if ui.link(label).clicked() {
            open_url(ui.ctx(), url);
        }
    }
    ui.separator();
    if ui.link("Sponsor / fund etchy").clicked() {
        open_url(ui.ctx(), URL_SPONSOR);
    }
    ui.separator();
    ui.label(format!("etchy v{}", env!("CARGO_PKG_VERSION")))
        .on_hover_text("The engine version.");
}

/// Opacity multiplier for an item from `layer_index` given the active `selected`
/// layer: 1.0 for the selected layer (and for layer-less items like the outline),
/// `DIM_ALPHA` for the other visible layers (#59).
fn dim_factor(layer_index: usize, selected: usize, dim_others: bool) -> f32 {
    if !dim_others || layer_index == NO_LAYER || layer_index == selected {
        1.0
    } else {
        DIM_ALPHA
    }
}

/// An Altium-style eye toggle for layer visibility (#4): an open eye when shown, a
/// dimmed eye with a slash when hidden. Painted (egui's default font has no eye
/// glyph — same reason the layer swatch is a painted rect, #16). Returns the click
/// response so the caller flips visibility. Replaces the old per-row checkbox.
fn eye_toggle(ui: &mut egui::Ui, visible: bool) -> egui::Response {
    let size = egui::vec2(18.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    let c = rect.center();
    let col = if visible {
        if hovered {
            C_CREAM
        } else {
            Color32::from_rgb(0xcd, 0xd6, 0xe4)
        }
    } else {
        Color32::from_gray(if hovered { 130 } else { 90 })
    };
    let p = ui.painter();
    // Almond outline (a wide ellipse) reads as an eye; a pupil dot when open.
    let (rx, ry) = (6.5_f32, 3.6_f32);
    let pts: Vec<Pos2> = (0..=18)
        .map(|i| {
            let t = i as f32 / 18.0 * std::f32::consts::TAU;
            egui::pos2(c.x + rx * t.cos(), c.y + ry * t.sin())
        })
        .collect();
    p.add(Shape::closed_line(pts, Stroke::new(1.3, col)));
    if visible {
        p.circle_filled(c, 2.1, col);
    } else {
        // Hidden: a diagonal slash across the eye.
        p.line_segment(
            [
                egui::pos2(c.x - rx - 1.0, c.y + ry + 1.0),
                egui::pos2(c.x + rx + 1.0, c.y - ry - 1.0),
            ],
            Stroke::new(1.3, col),
        );
    }
    resp
}

/// The etchy brand mark (Feature 8): a copper rounded-square "E" monogram. Drawn
/// with the painter (a copper fill + a letter galley — letters render fine; only
/// symbol glyphs are tofu, #16/#30). Replaces the top-bar wordmark and marks the
/// top of the activity rail.
fn etchy_monogram(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, size * 0.22, C_COPPER);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "E",
        egui::FontId::proportional(size * 0.66),
        C_CANVAS,
    );
    resp
}

/// A small SQUARE colour swatch (Feature 7) that opens egui's colour picker on
/// click. `color_edit_button_srgba` is normally a rounded, interact-sized button;
/// scoping the interact size down and zeroing the widget corner radius makes it a
/// compact square while keeping the picker popup intact.
fn square_color_swatch(ui: &mut egui::Ui, color: &mut Color32) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size = egui::vec2(14.0, 14.0);
        let v = ui.visuals_mut();
        v.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
        v.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        v.widgets.active.corner_radius = egui::CornerRadius::ZERO;
        v.widgets.open.corner_radius = egui::CornerRadius::ZERO;
        ui.color_edit_button_srgba(color)
    })
    .inner
}

/// One activity-rail cell (Feature 1): a fixed-size button that paints a
/// hover/active background (copper accent when active, matching the app's selection
/// accent) then draws its glyph via the painter — never a font symbol (the bundled
/// font renders many symbols as tofu, #16/#30). Returns the click response.
fn rail_button(
    ui: &mut egui::Ui,
    active: bool,
    draw: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 34.0), Sense::click());
    let hovered = resp.hovered();
    if active {
        ui.painter()
            .rect_filled(rect, 5.0, C_COPPER.gamma_multiply(0.30));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, 5.0, C_COPPER.gamma_multiply(0.12));
    }
    let col = if active {
        C_COPPER
    } else if hovered {
        C_CREAM
    } else {
        Color32::from_rgb(0xcd, 0xd6, 0xe4)
    };
    let icon = Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
    draw(ui.painter(), icon, col);
    resp
}

/// Draw the glyph for a panel tab's rail icon (painter marks, glyph-free).
fn draw_panel_icon(tab: PanelTab, p: &egui::Painter, r: Rect, col: Color32) {
    match tab {
        PanelTab::Layers => draw_layers_icon(p, r, col),
        PanelTab::Measure => draw_measure_icon(p, r, col),
        PanelTab::Export => draw_export_icon(p, r, col),
    }
}

/// Layers icon: three stacked bars reading as a layer list.
fn draw_layers_icon(p: &egui::Painter, r: Rect, col: Color32) {
    let bar_h = (r.height() * 0.16).max(2.0);
    for i in 0..3 {
        let y = r.min.y + r.height() * (0.10 + i as f32 * 0.33);
        let bar = Rect::from_min_size(egui::pos2(r.min.x, y), egui::vec2(r.width(), bar_h));
        p.rect_filled(bar, 1.0, col);
    }
}

/// Measure icon: a ruler — an outlined bar with tick marks along its top edge.
fn draw_measure_icon(p: &egui::Painter, r: Rect, col: Color32) {
    let bar = Rect::from_min_max(
        egui::pos2(r.min.x, r.center().y - r.height() * 0.20),
        egui::pos2(r.max.x, r.center().y + r.height() * 0.20),
    );
    p.rect_stroke(bar, 1.0, Stroke::new(1.4, col), StrokeKind::Inside);
    for i in 1..4 {
        let x = r.min.x + r.width() * (i as f32 / 4.0);
        p.line_segment(
            [
                egui::pos2(x, bar.min.y),
                egui::pos2(x, bar.min.y + r.height() * 0.16),
            ],
            Stroke::new(1.2, col),
        );
    }
}

/// Export icon: a down arrow above a tray line ("write to disk").
fn draw_export_icon(p: &egui::Painter, r: Rect, col: Color32) {
    let cx = r.center().x;
    let tip_y = r.center().y + r.height() * 0.10;
    p.line_segment(
        [egui::pos2(cx, r.min.y), egui::pos2(cx, tip_y)],
        Stroke::new(1.6, col),
    );
    let aw = r.width() * 0.20;
    let ah = r.height() * 0.16;
    p.line_segment(
        [egui::pos2(cx - aw, tip_y - ah), egui::pos2(cx, tip_y)],
        Stroke::new(1.6, col),
    );
    p.line_segment(
        [egui::pos2(cx + aw, tip_y - ah), egui::pos2(cx, tip_y)],
        Stroke::new(1.6, col),
    );
    p.line_segment(
        [egui::pos2(r.min.x, r.max.y), egui::pos2(r.max.x, r.max.y)],
        Stroke::new(1.6, col),
    );
}

/// Settings cog icon: a toothed ring with a hub.
fn draw_cog_icon(p: &egui::Painter, r: Rect, col: Color32) {
    let c = r.center();
    let ring_r = r.width().min(r.height()) * 0.30;
    let tooth = ring_r * 0.5;
    for i in 0..8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        let (s, cs) = a.sin_cos();
        p.line_segment(
            [
                egui::pos2(c.x + cs * ring_r, c.y + s * ring_r),
                egui::pos2(c.x + cs * (ring_r + tooth), c.y + s * (ring_r + tooth)),
            ],
            Stroke::new(1.5, col),
        );
    }
    let ring: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            egui::pos2(c.x + a.cos() * ring_r, c.y + a.sin() * ring_r)
        })
        .collect();
    p.add(Shape::closed_line(ring, Stroke::new(1.5, col)));
    p.circle_filled(c, ring_r * 0.42, col);
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
    base_opacity: f32,
    selected: usize,
    base_of: impl Fn(usize) -> Color32,
    canvas: Color32,
    col_added: Color32,
    col_removed: Color32,
    min_area_nm2: f64,
    // When true, Role::Base items are skipped here because the GPU path is drawing
    // them this frame (#106). Always false without the `gpu-transform` feature.
    skip_base: bool,
    // Whether non-selected layers are dimmed (#59 Highlight mode).
    dim_others: bool,
    // Whether non-selected layers' base copper is dropped (#158 All view) — the
    // selected layer keeps its base; others draw diff-only, cutting ~70% of verts.
    hide_unselected_base: bool,
) -> (Vec<Shape>, usize) {
    // Merge everything into ONE mesh (per-vertex colour preserves the LOD fade)
    // instead of one Mesh+Shape per region — the FMU top-copper layer was ~5.5k
    // mesh allocations per frame; this makes it one. Off-screen items are culled
    // before their vertices are built (cheaper when zoomed in). Items are pushed
    // base → outline → diff, so draw order within the single mesh stays correct.
    let mut mesh = egui::epaint::Mesh::default();
    // Pre-reserve the vertex/index buffers (#80). Without this they double-and-copy
    // as they grow; on a dense board (e.g. an 8-layer pack with all layers on) the
    // per-frame mesh rebuild hits a realloc cliff — a measured 500k-triangle frame
    // dropped from ~27 ms to ~4.6 ms just from reserving. (3 verts + 3 indices/tri.)
    let cap: usize = cache.items.iter().map(|it| it.tris.len() * 3).sum();
    mesh.vertices.reserve(cap);
    mesh.indices.reserve(cap);
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
            if skip_base {
                continue; // base drawn on the GPU this frame (#106)
            }
            // All view (#158): drop non-selected layers' base copper (diff-only for
            // them) — it's ~70% of the verts and just dimmed context. The selected
            // layer keeps its base.
            if hide_unselected_base && item.layer_index != selected {
                continue;
            }
            let thickness = feature_thickness_nm(item.area_nm2, item.extent_nm);
            if region_screen_px(thickness, cam.scale) < LOD_LO_PX {
                continue;
            }
        }
        // Highlight/dim (#59): the active layer at full opacity, the other visible
        // layers dimmed; layer-less items (outline) never dim.
        let dim = dim_factor(item.layer_index, selected, dim_others);
        let (mut color, is_diff) = match item.role {
            Role::Base => (
                base_display_color(base_of(item.layer_index), canvas, base_opacity),
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

fn legend(painter: &egui::Painter, rect: Rect, added: Color32, removed: Color32) {
    let mut y = rect.right_top() + egui::vec2(-150.0, 8.0);
    let rows = [(added, "added"), (removed, "removed")];
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

/// A completed measurement: the two world-space endpoints of a ruler (#50). The
/// Measure tab keeps a running list of these; `distance_mm(a, b)` gives the length
/// in the chosen unit.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Measurement {
    a: [f64; 2],
    b: [f64; 2],
}

/// Build a completed `Measurement` from two world points (#50) — the pure kernel
/// behind finishing a ruler.
fn finish_measurement(a: [f64; 2], b: [f64; 2]) -> Measurement {
    Measurement { a, b }
}

/// Apply a measure click at world point `p` to the in-progress buffer `pts` (#50).
/// The buffer holds 0 or 1 points: a click on an empty buffer stores the first
/// point and returns `None`; a click when a point is already placed completes the
/// pair — it empties the buffer and returns the finished `Measurement` (the caller
/// pushes it onto the running list). The next click then starts a fresh pair. Pure
/// so the click→state transition is unit-testable off-screen.
fn measure_click(pts: &mut Vec<[f64; 2]>, p: [f64; 2]) -> Option<Measurement> {
    match pts.first().copied() {
        Some(a) => {
            pts.clear();
            Some(finish_measurement(a, p))
        }
        None => {
            pts.push(p);
            None
        }
    }
}

/// Remove the measurement at `idx` from the running list if in range (#50).
/// Bounds-checked so a stale index carried across a frame can never panic.
fn measurement_remove(list: &mut Vec<Measurement>, idx: usize) {
    if idx < list.len() {
        list.remove(idx);
    }
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

/// Format the live cursor coordinate readout (#179), in millimetres. When the
/// value is grid-snapped (#178 fix) it carries a "· grid" suffix so the three
/// decimals on a quantised value don't read as false precision — the readout is
/// honest about being on the grid.
fn format_coord_mm(x_mm: f64, y_mm: f64, snapped: bool) -> String {
    let base = format!("{x_mm:.3}, {y_mm:.3} mm");
    if snapped {
        format!("{base} · grid")
    } else {
        base
    }
}

/// The noise-filter's hidden-region trust line (#178): when the min-area filter
/// dropped `hidden` regions, name the count and the threshold that hid them, so
/// the suppression is never silent (see `MIN_AREA_MM2`). Nothing hidden → no line.
fn hidden_note(hidden: usize, min_area_mm2: f64) -> Option<String> {
    if hidden == 0 {
        None
    } else {
        Some(format!("{hidden} hidden < {min_area_mm2:.4} mm²"))
    }
}

/// The "one layer of many" hint (#112): when exactly one layer of several is
/// visible, a lone trace reads as "my traces vanished" rather than "one layer of
/// many". Only that single-of-many case produces a hint — anything else is None
/// so the corner stays clean.
fn single_layer_hint(shown: usize, total: usize) -> Option<String> {
    if shown == 1 && total > 1 {
        Some(format!("{shown} / {total} layers"))
    } else {
        None
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

/// Draw one complete measure ruler in screen space (#50): both endpoints, the
/// segment, and the distance `label` offset ~14 px perpendicular to the line so it
/// never sits on top of it. Shared by the completed-measurement loop so every
/// ruler looks identical.
fn draw_ruler(painter: &egui::Painter, a: Pos2, b: Pos2, label: &str) {
    painter.circle_filled(a, 3.0, C_COPPER);
    painter.circle_filled(b, 3.0, C_COPPER);
    painter.line_segment([a, b], Stroke::new(1.5, C_COPPER));
    let mid = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let off = egui::vec2(-dy / len, dx / len) * 14.0;
    measure_label(painter, mid + off, label);
}

/// A small, unobtrusive status chip anchored into a canvas corner — copper text
/// on a translucent dark surface so it stays legible over any board colour while
/// reading as chrome, not diff content. `anchor`/`align` place it against a corner
/// (e.g. `LEFT_TOP` for top-left, `RIGHT_BOTTOM` for bottom-right).
fn corner_chip(painter: &egui::Painter, anchor: Pos2, align: egui::Align2, text: &str) {
    let font = egui::FontId::proportional(12.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, C_COPPER);
    let pad = egui::vec2(6.0, 3.0);
    let rect = align.anchor_size(anchor, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 3.0, C_SURFACE.gamma_multiply(0.85));
    painter.galley(rect.min + pad, galley, C_COPPER);
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

/// Union two optional bboxes `[minx, miny, maxx, maxy]`; `None` is the identity.
fn union_bbox(a: Option<[i64; 4]>, b: Option<[i64; 4]>) -> Option<[i64; 4]> {
    match (a, b) {
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

/// Union bbox of a layer's old+new geometry.
fn layer_bbox(layer: &LayerView) -> Option<[i64; 4]> {
    union_bbox(layer.old.bbox_nm(), layer.new.bbox_nm())
}

/// Union bbox of every layer — the WHOLE board's extent, so Fit and the initial
/// view frame the entire board rather than one layer's geometry (#8).
fn board_bbox(layers: &[LayerView]) -> Option<[i64; 4]> {
    layers.iter().map(layer_bbox).fold(None, union_bbox)
}

#[cfg(test)]
mod tests {
    use super::{
        base_display_color, build_geom_key, cycle_base_opacity, derive_label, distance_mm,
        export_file_names, finish_measurement, format_coord_mm, geom_cache_dirty, group_layers,
        hidden_note, is_version_like, layer_group, legacy_base_opacity, measure_click,
        measure_rail_click, measurement_remove, pans_on, pick_outline_index, region_screen_px,
        scroll_to_camera_action, short_layer_name, single_layer_hint, step_in_order, toggle_panel,
        warning_phase, CameraAction, InputPreset, LayerGroup, Measurement, Mode, PanelTab,
        RailSide, Theme, WarningPhase, BASE_OPACITY_FAINT, BASE_OPACITY_STRONG,
    };
    use etchy_core::LayerKind;

    #[test]
    fn hidden_note_only_when_something_is_hidden() {
        // TRUST (#178): the noise-filter's hidden count must never be dropped
        // silently. When nothing is hidden there is no chip; when regions are
        // hidden it names the count and the threshold that hid them.
        assert_eq!(hidden_note(0, 0.0004), None);
        assert_eq!(hidden_note(0, 0.0), None);
        assert_eq!(
            hidden_note(3, 0.0004).as_deref(),
            Some("3 hidden < 0.0004 mm²")
        );
        assert_eq!(
            hidden_note(1, 0.01).as_deref(),
            Some("1 hidden < 0.0100 mm²")
        );
    }

    #[test]
    fn single_layer_hint_only_when_one_of_many_shows() {
        // #112: a lone visible layer of several reads as "traces vanished" without
        // a hint. Show it only when exactly one of two-or-more layers is visible.
        assert_eq!(single_layer_hint(1, 13).as_deref(), Some("1 / 13 layers"));
        assert_eq!(single_layer_hint(1, 2).as_deref(), Some("1 / 2 layers"));
        // Not a single-of-many situation → no hint (no clutter).
        assert_eq!(single_layer_hint(2, 13), None); // more than one shown
        assert_eq!(single_layer_hint(13, 13), None); // all shown
        assert_eq!(single_layer_hint(1, 1), None); // only one layer exists
        assert_eq!(single_layer_hint(0, 5), None); // none shown
    }

    #[test]
    fn coord_readout_flags_grid_snap() {
        // MED (#178): snapped coords quantize to the grid, so the readout must say
        // so — otherwise 3 decimals on a 1 mm-grid value reads as false precision.
        assert_eq!(format_coord_mm(12.5, -3.25, false), "12.500, -3.250 mm");
        assert_eq!(
            format_coord_mm(12.0, -3.0, true),
            "12.000, -3.000 mm · grid"
        );
    }

    #[test]
    fn derive_label_borrows_parent_for_generic_rev_dirs() {
        use std::path::Path;
        // A bare rev-word basename ("old"/"new") is meaningless alone → prefix the
        // parent directory so web and native present the same board name (#177).
        assert_eq!(
            derive_label(Path::new("gerbers/Mad_RP2040/old")),
            "Mad_RP2040 old"
        );
        assert_eq!(
            derive_label(Path::new("gerbers/Mad_RP2040/new")),
            "Mad_RP2040 new"
        );
        // Plain version basenames borrow the parent too — this is how the web demo
        // seeds "Mad_RP2040 v0.0.0"/"v0.0.1", matching the native derivation.
        assert_eq!(
            derive_label(Path::new("Mad_RP2040/v0.0.0")),
            "Mad_RP2040 v0.0.0"
        );
        assert_eq!(
            derive_label(Path::new("Mad_RP2040/v0.0.1")),
            "Mad_RP2040 v0.0.1"
        );
        // Other generic markers.
        assert_eq!(derive_label(Path::new("board/before")), "board before");
        assert_eq!(derive_label(Path::new("board/A")), "board A");
    }

    #[test]
    fn derive_label_keeps_meaningful_basenames() {
        use std::path::Path;
        // A meaningful basename stands on its own — no parent prefix.
        assert_eq!(
            derive_label(Path::new("gerbers/Mad_RP2040_v1")),
            "Mad_RP2040_v1"
        );
        assert_eq!(derive_label(Path::new("some/where/RevB_fab")), "RevB_fab");
        // A generic basename with a generic (or absent) parent stays as-is rather
        // than producing "new new" or borrowing nothing useful.
        assert_eq!(derive_label(Path::new("old/new")), "new");
        assert_eq!(derive_label(Path::new("new")), "new");
        // Empty path → sentinel, never a panic.
        assert_eq!(derive_label(Path::new("")), "?");
    }

    #[test]
    fn version_like_detects_plain_versions_only() {
        assert!(is_version_like("v1"));
        assert!(is_version_like("v0.0.1"));
        assert!(is_version_like("1.0"));
        assert!(is_version_like("2"));
        // Not versions: a leading letter (other than the v prefix), or trailing text.
        assert!(!is_version_like("rev1"));
        assert!(!is_version_like("v1a"));
        assert!(!is_version_like("board"));
        assert!(!is_version_like(""));
        assert!(!is_version_like("v"));
    }

    // Manual perf bench for the per-frame mesh build (#80), worst case = everything
    // on-screen, nothing culled. Ignored in CI (timing is machine-dependent).
    // Run: cargo test --release -p etchy-gui bench_transform_cache -- --ignored --nocapture
    fn dense_cache(total_tris: usize) -> super::TessCache {
        let per_item = 20usize;
        let n_items = (total_tris / per_item).max(1);
        let board = 50_000_000i64; // 50 mm
        let cols = (n_items as f64).sqrt().ceil() as i64;
        let cell = (board / cols).max(1);
        let mut items = Vec::with_capacity(n_items);
        for i in 0..n_items {
            let cx = (i as i64 % cols) * cell;
            let cy = (i as i64 / cols) * cell;
            let tris: Vec<[super::Pt; 3]> = (0..per_item)
                .map(|k| {
                    let o = k as i64 * 800;
                    [
                        super::Pt::new(cx + o, cy),
                        super::Pt::new(cx + o + 4000, cy),
                        super::Pt::new(cx + o, cy + 4000),
                    ]
                })
                .collect();
            let role = match i % 3 {
                0 => super::Role::Base,
                1 => super::Role::Added,
                _ => super::Role::Removed,
            };
            let s = 1_000_000i64; // 1 mm feature → passes LOD (not culled)
            items.push(super::CachedItem {
                role,
                side: super::Side::Full,
                layer_index: 0,
                tris,
                bbox: [cx, cy, cx + s, cy + s],
                extent_nm: s,
                area_nm2: (s as f64) * (s as f64),
            });
        }
        super::TessCache {
            key: super::GeomKey {
                visible: vec![0],
                mode: Mode::Overlay,
                base_on: true,
                outline_effective: false,
            },
            items,
        }
    }

    #[test]
    #[ignore = "manual perf bench (#80); run with --ignored --nocapture"]
    fn bench_transform_cache_scaling() {
        use std::time::Instant;
        let rect = egui::Rect::from_min_size(egui::pos2(260.0, 130.0), egui::vec2(1140.0, 760.0));
        for &target in &[50_000usize, 150_000, 300_000, 500_000] {
            let cache = dense_cache(target);
            let real: usize = cache.items.iter().map(|it| it.tris.len()).sum();
            let mut cam = super::Camera::default();
            super::fit(&mut cam, [0, 0, 50_000_000, 50_000_000], rect);
            let frames = 60u32;
            let mut sink = 0usize;
            let t = Instant::now();
            for f in 0..frames {
                cam.center[0] += f as f64 * 1000.0; // simulate a pan
                let (shapes, _h) = super::transform_cache(
                    &cache,
                    &cam,
                    rect,
                    BASE_OPACITY_FAINT,
                    0,
                    |_| super::C_BASE,
                    egui::Color32::BLACK,
                    super::C_ADDED,
                    super::C_REMOVED,
                    0.0,
                    false, // skip_base: CPU path draws everything in this bench
                    true,  // dim_others
                    false, // hide_unselected_base
                );
                sink += shapes.len();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            println!("BENCH transform_cache: {real} tris -> {ms:.2} ms/frame (sink={sink})");
        }
    }

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
        // Opacity 0 shows the canvas (base off); 1 shows the pure layer colour. The
        // old faint/strong stops (0.4/0.8) still order the same, and both are opaque
        // so unchanged copper isn't black.
        assert_eq!(base_display_color(C_COPPER, C_CANVAS, 0.0), C_CANVAS);
        assert_eq!(base_display_color(C_COPPER, C_CANVAS, 1.0), C_COPPER);
        // Values outside 0..=1 are clamped, never a wrap/overflow.
        assert_eq!(base_display_color(C_COPPER, C_CANVAS, -0.5), C_CANVAS);
        assert_eq!(base_display_color(C_COPPER, C_CANVAS, 1.5), C_COPPER);
        let faint = base_display_color(C_COPPER, C_CANVAS, BASE_OPACITY_FAINT);
        let strong = base_display_color(C_COPPER, C_CANVAS, BASE_OPACITY_STRONG);
        assert!(strong.r() > faint.r());
        assert!(faint.r() > C_CANVAS.r()); // even faint is visibly above the black canvas
        assert_eq!(strong.a(), 255); // opaque
    }

    #[test]
    fn geom_key_tracks_selection_inputs_only() {
        let base = build_geom_key(&[0], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1));
        // base 0.4 vs 0.8 is a colour, not geometry -> same key (no rebuild)
        assert_eq!(
            base,
            build_geom_key(&[0], Mode::Overlay, BASE_OPACITY_STRONG, true, Some(1))
        );
        // base opacity 0 flips base_on -> different key (the base mesh joins/leaves the draw)
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Overlay, 0.0, true, Some(1))
        );
        // visible set / mode changes -> different key
        assert_ne!(
            base,
            build_geom_key(&[2], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Old, BASE_OPACITY_FAINT, true, Some(1))
        );
        // With multiple layers shown, the outline still draws (it's enabled and
        // exists), so a visible-set change is what flips the key.
        assert_ne!(
            base,
            build_geom_key(&[0, 1], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1))
        );
    }

    #[test]
    fn geom_cache_dirty_on_none_or_change() {
        let k = build_geom_key(&[0], Mode::Overlay, BASE_OPACITY_FAINT, false, None);
        assert!(geom_cache_dirty(None, &k));
        assert!(!geom_cache_dirty(Some(&k), &k));
        let k2 = build_geom_key(&[2], Mode::Overlay, BASE_OPACITY_FAINT, false, None);
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
        // frac spans the FULL canvas width (#14): the divider reaches either edge
        // so the user can wipe all the way across; only out-of-range is clamped.
        assert_eq!(clamp_swipe_frac(0.0), 0.0);
        assert_eq!(clamp_swipe_frac(1.0), 1.0);
        assert_eq!(clamp_swipe_frac(0.5), 0.5);
        assert_eq!(clamp_swipe_frac(-0.5), 0.0); // below range → left edge
        assert_eq!(clamp_swipe_frac(1.5), 1.0); // above range → right edge
                                                // A clamped frac drives the divider position too — to the very edges.
        let (_, _, div_lo) = swipe_rects(r, -1.0);
        assert_eq!(div_lo, 0.0);
        let (_, _, div_hi) = swipe_rects(r, 2.0);
        assert_eq!(div_hi, 100.0);
    }

    #[test]
    fn union_bbox_unions_or_passes_through() {
        // board_bbox folds this over every layer so Fit frames the WHOLE board (#8),
        // not just the selected layer's extent.
        use super::union_bbox;
        assert_eq!(
            union_bbox(Some([0, 0, 10, 10]), Some([5, -5, 20, 3])),
            Some([0, -5, 20, 10])
        );
        assert_eq!(union_bbox(Some([1, 2, 3, 4]), None), Some([1, 2, 3, 4]));
        assert_eq!(union_bbox(None, Some([1, 2, 3, 4])), Some([1, 2, 3, 4]));
        assert_eq!(union_bbox(None, None), None);
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
    fn cycle_base_opacity_rotates_off_faint_strong() {
        // The S key steps the three familiar stops: 0 → faint → strong → 0.
        assert_eq!(cycle_base_opacity(0.0), BASE_OPACITY_FAINT);
        assert_eq!(cycle_base_opacity(BASE_OPACITY_FAINT), BASE_OPACITY_STRONG);
        assert_eq!(cycle_base_opacity(BASE_OPACITY_STRONG), 0.0);
        // An in-between slider value below strong steps up to strong; at/above strong wraps to off.
        assert_eq!(cycle_base_opacity(0.2), BASE_OPACITY_STRONG);
        assert_eq!(cycle_base_opacity(1.0), 0.0);
    }

    #[test]
    fn legacy_base_level_migrates_to_opacity() {
        // Pre-#12 off/faint/strong strings map to the equivalent opacity; unknown
        // values fall back to faint (the old default).
        assert_eq!(legacy_base_opacity("off"), 0.0);
        assert_eq!(legacy_base_opacity("faint"), BASE_OPACITY_FAINT);
        assert_eq!(legacy_base_opacity("strong"), BASE_OPACITY_STRONG);
        assert_eq!(legacy_base_opacity("bogus"), BASE_OPACITY_FAINT);
    }

    #[test]
    fn old_settings_json_migrates_base_level_to_opacity() {
        use super::{Settings, ViewApp};
        // An old persisted blob carries `base_level` as a string and no `base_opacity`.
        // Deserializing fills base_opacity from Default, and apply_settings migrates
        // the legacy string over it.
        let old = r#"{"theme":"dark","base_level":"strong"}"#;
        let s: Settings = serde_json::from_str(old).expect("deserialize old blob");
        assert_eq!(s.base_level.as_deref(), Some("strong"));
        let mut app = ViewApp::new(empty_diff(), "x".into(), "y".into());
        app.apply_settings(s);
        assert_eq!(app.base_opacity, BASE_OPACITY_STRONG);
        // A blob with neither field keeps the faint default.
        let bare: Settings = serde_json::from_str("{}").expect("deserialize empty");
        let mut app2 = ViewApp::new(empty_diff(), "x".into(), "y".into());
        app2.apply_settings(bare);
        assert_eq!(app2.base_opacity, BASE_OPACITY_FAINT);
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
        // Left-drag pans in every preset now (#18) — the swipe-divider drag guards
        // itself separately, so primary is free to pan elsewhere on the canvas.
        // KiCad: primary, middle OR right.
        assert!(pans_on(InputPreset::KiCad, Primary));
        assert!(pans_on(InputPreset::KiCad, Middle));
        assert!(pans_on(InputPreset::KiCad, Secondary));
        // Altium: primary OR right.
        assert!(pans_on(InputPreset::Altium, Primary));
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
    fn rail_side_serde_round_trips() {
        for s in [RailSide::Left, RailSide::Right] {
            let json = serde_json::to_string(&s).expect("serialize");
            let back: RailSide = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(s, back);
        }
        // Unknown / legacy values fall back to the default (left), never error.
        assert_eq!(
            serde_json::from_str::<RailSide>("\"bogus\"").expect("deserialize"),
            RailSide::Left,
        );
        assert_eq!(RailSide::default(), RailSide::Left);
    }

    #[test]
    fn toggle_panel_opens_switches_and_collapses() {
        // Clicking an inactive tab opens it; clicking the active tab collapses the
        // panel; clicking a different tab switches to it.
        assert_eq!(toggle_panel(None, PanelTab::Layers), Some(PanelTab::Layers));
        assert_eq!(toggle_panel(Some(PanelTab::Layers), PanelTab::Layers), None);
        assert_eq!(
            toggle_panel(Some(PanelTab::Layers), PanelTab::Measure),
            Some(PanelTab::Measure)
        );
        assert_eq!(
            toggle_panel(Some(PanelTab::Measure), PanelTab::Layers),
            Some(PanelTab::Layers)
        );
    }

    #[test]
    fn export_file_names_mirror_build_export() {
        // The preview must match `build_export`'s naming exactly: an
        // index-prefixed SVG per chosen layer (so two layers with the same
        // display name don't clobber each other) plus a trailing `areas.csv`.
        let names = vec!["top-copper".to_string(), "inner-copper1".to_string()];
        assert_eq!(
            export_file_names(&names),
            vec![
                "00-top-copper.svg".to_string(),
                "01-inner-copper1.svg".to_string(),
                "areas.csv".to_string(),
            ]
        );

        // No layers (e.g. nothing changed, or none selected) still writes the CSV.
        assert_eq!(export_file_names(&[]), vec!["areas.csv".to_string()]);

        // Same display name twice → distinct index-prefixed files.
        let dup = vec!["other".to_string(), "other".to_string()];
        assert_eq!(
            export_file_names(&dup),
            vec![
                "00-other.svg".to_string(),
                "01-other.svg".to_string(),
                "areas.csv".to_string(),
            ]
        );
    }

    #[test]
    fn settings_serde_round_trips() {
        use super::{color_to_rgba, rgba_to_color, Settings};
        use egui::Color32;
        let s = Settings {
            theme: Theme::Light,
            base_opacity: BASE_OPACITY_STRONG,
            base_level: None, // legacy migration field; never written, always None after a round trip
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
            rail_side: RailSide::Right,
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
    fn finish_measurement_carries_both_endpoints() {
        let m = finish_measurement([1.0, 2.0], [3.0, 4.0]);
        assert_eq!(
            m,
            Measurement {
                a: [1.0, 2.0],
                b: [3.0, 4.0]
            }
        );
    }

    #[test]
    fn measure_click_completes_a_pair_then_resets() {
        // Two clicks produce one Measurement and leave the buffer empty, ready for
        // the next pair (#50).
        let mut pts: Vec<[f64; 2]> = Vec::new();
        // First click: stores the point, nothing completed yet.
        assert_eq!(measure_click(&mut pts, [0.0, 0.0]), None);
        assert_eq!(pts, vec![[0.0, 0.0]]);
        // Second click: completes the pair and empties the buffer.
        let done = measure_click(&mut pts, [3_000_000.0, 4_000_000.0]);
        assert_eq!(
            done,
            Some(Measurement {
                a: [0.0, 0.0],
                b: [3_000_000.0, 4_000_000.0],
            })
        );
        assert!(pts.is_empty(), "buffer resets after completing a pair");
        // Third click starts a fresh pair.
        assert_eq!(measure_click(&mut pts, [5.0, 6.0]), None);
        assert_eq!(pts, vec![[5.0, 6.0]]);
    }

    #[test]
    fn measurement_remove_deletes_index_and_is_bounds_safe() {
        let mut list = vec![
            Measurement {
                a: [0.0, 0.0],
                b: [1.0, 0.0],
            },
            Measurement {
                a: [0.0, 0.0],
                b: [2.0, 0.0],
            },
            Measurement {
                a: [0.0, 0.0],
                b: [3.0, 0.0],
            },
        ];
        // Removes the requested index and keeps the rest in order.
        measurement_remove(&mut list, 1);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].b, [1.0, 0.0]);
        assert_eq!(list[1].b, [3.0, 0.0]);
        // Out-of-range index is a no-op, never a panic.
        measurement_remove(&mut list, 9);
        assert_eq!(list.len(), 2);
        // Clearing empties the list.
        list.clear();
        assert!(list.is_empty());
    }

    #[test]
    fn measure_rail_click_arms_on_open_disarms_on_collapse() {
        // Clicking Measure from any non-Measure state opens the panel and arms.
        assert_eq!(
            measure_rail_click(None),
            (Some(PanelTab::Measure), true),
            "opening Measure arms the tool"
        );
        assert_eq!(
            measure_rail_click(Some(PanelTab::Layers)),
            (Some(PanelTab::Measure), true),
            "switching to Measure from another tab arms the tool"
        );
        // Clicking the active Measure tab collapses the panel and disarms.
        assert_eq!(
            measure_rail_click(Some(PanelTab::Measure)),
            (None, false),
            "collapsing Measure disarms the tool"
        );
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
        app.base_opacity = BASE_OPACITY_STRONG;
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
        assert_eq!(fresh.base_opacity, app.base_opacity);
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
    fn diff_palette_presets() {
        use super::{DiffPalette, C_ADDED, C_REMOVED};
        // Brand is exactly the default brand green/red (so it doubles as "reset").
        assert_eq!(DiffPalette::Brand.colors(), (C_ADDED, C_REMOVED));
        // The other presets differ from brand and from each other.
        let brand = DiffPalette::Brand.colors();
        let safe = DiffPalette::ColorSafe.colors();
        let hc = DiffPalette::HighContrast.colors();
        assert_ne!(safe, brand);
        assert_ne!(hc, brand);
        assert_ne!(safe, hc);
        // added != removed within each palette (so a diff is always legible).
        for (a, r) in [brand, safe, hc] {
            assert_ne!(a, r);
        }
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
    fn view_mode_visibility_presets() {
        use super::{visibility_for_mode, ViewMode};
        // Single: the active layer plus the board outline are on (#157).
        assert_eq!(
            visibility_for_mode(ViewMode::Single, 4, 2, Some(0)),
            vec![true, false, true, false]
        );
        // No outline layer: Single is just the active layer.
        assert_eq!(
            visibility_for_mode(ViewMode::Single, 4, 2, None),
            vec![false, false, true, false]
        );
        // Highlight and All: every layer on (they differ only in dimming); the
        // outline is already covered by the all-on set.
        assert_eq!(
            visibility_for_mode(ViewMode::Highlight, 3, 0, Some(2)),
            vec![true, true, true]
        );
        assert_eq!(
            visibility_for_mode(ViewMode::All, 3, 0, None),
            vec![true, true, true]
        );
    }

    #[test]
    fn dim_factor_dims_only_in_highlight() {
        use super::{dim_factor, ViewMode, DIM_ALPHA, NO_LAYER};
        // Highlight (dim_others=true): non-selected dim, selected/outline full.
        assert_eq!(dim_factor(1, 0, true), DIM_ALPHA);
        assert_eq!(dim_factor(0, 0, true), 1.0);
        assert_eq!(dim_factor(NO_LAYER, 0, true), 1.0);
        // All (dim_others=false): every layer at full strength.
        assert_eq!(dim_factor(1, 0, false), 1.0);
        // The mode's own flag: only Highlight dims.
        assert!(ViewMode::Highlight.dims_others());
        assert!(!ViewMode::All.dims_others());
        assert!(!ViewMode::Single.dims_others());
        // Only All drops non-selected base copper (#158); Single/Highlight keep it.
        assert!(ViewMode::All.hides_unselected_base());
        assert!(!ViewMode::Highlight.hides_unselected_base());
        assert!(!ViewMode::Single.hides_unselected_base());
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
    fn default_visible_shows_the_selected_layer_and_outline() {
        use super::default_visible;
        // On load the selected layer plus the board outline are visible; every other
        // layer is opt-in (#9/#10 perf), and the outline reads as orientation from the
        // start (#157).
        assert_eq!(
            default_visible(4, 2, Some(0)),
            vec![true, false, true, false]
        );
        // No outline layer: just the selected one, as before.
        assert_eq!(default_visible(4, 2, None), vec![false, false, true, false]);
        // Outline == selected: a single true, no double-set panic.
        assert_eq!(default_visible(3, 1, Some(1)), vec![false, true, false]);
        // Out-of-range outline is ignored (only the selected turns on).
        assert_eq!(default_visible(2, 0, Some(9)), vec![true, false]);
        assert_eq!(default_visible(1, 0, None), vec![true]);
        // Empty board / out-of-range selected: no panic, nothing forced on.
        assert_eq!(default_visible(0, 0, None), Vec::<bool>::new());
        assert_eq!(default_visible(3, 9, None), vec![false, false, false]);
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
        let base = build_geom_key(&[0, 2], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1));
        // Same visible set + same other inputs -> equal (no rebuild).
        assert_eq!(
            base,
            build_geom_key(&[0, 2], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1))
        );
        // A different visible set -> different key (the merged mesh changes).
        assert_ne!(
            base,
            build_geom_key(&[0], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0, 2, 3], Mode::Overlay, BASE_OPACITY_FAINT, true, Some(1))
        );
        // base-off and mode still flip the key.
        assert_ne!(
            base,
            build_geom_key(&[0, 2], Mode::Overlay, 0.0, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(&[0, 2], Mode::Old, BASE_OPACITY_FAINT, true, Some(1))
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
