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
        let app = ViewApp::new(diff, label(&old_dir), label(&new_dir));
        let mut viewport = egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_title("etchy — PCB diff viewer");
        if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!(
            "../../../assets/brand/png/etchy-app-icon-512.png"
        )) {
            viewport = viewport.with_icon(icon);
        }
        let native_options = eframe::NativeOptions {
            viewport,
            // 4x MSAA so sub-pixel slivers (thin track/pad junctions, shared edges)
            // cover at least one sample and don't drop out as "no copper" (#55/#47).
            multisampling: 4,
            ..Default::default()
        };
        match eframe::run_native("etchy", native_options, Box::new(|_cc| Ok(Box::new(app)))) {
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
        // Generic, non-confidential labels (the bundled demo board is gitignored;
        // real board names come with the public demo board, not from these filenames).
        let app = ViewApp::new(demo_diff(), "old revision".into(), "new revision".into());
        eframe::WebRunner::new()
            .start(canvas, web_options, Box::new(|_cc| Ok(Box::new(app))))
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
    selected: usize,
    mode: Mode,
    base_on: bool,
    outline_effective: bool,
}

/// Build the cache key from the current selection inputs.
fn build_geom_key(
    selected: usize,
    mode: Mode,
    base_level: BaseLevel,
    show_outline: bool,
    outline: Option<usize>,
) -> GeomKey {
    GeomKey {
        selected,
        mode,
        base_on: base_level != BaseLevel::Off,
        outline_effective: outline_legend_visible(show_outline, outline, selected),
    }
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
        TopCopper | BottomCopper | InnerCopper(_) => (C_COPPER, C_COPPER),
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

/// Distinct layer kinds in first-seen order — drives the Colors window's
/// per-layer rows so every kind on the board can be coloured.
fn present_layer_kinds(
    kinds: impl Iterator<Item = etchy_core::LayerKind>,
) -> Vec<etchy_core::LayerKind> {
    let mut seen = Vec::new();
    for k in kinds {
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    seen
}

/// The base colour for `kind`: a user override if set, else the type default.
fn resolve_base_color(
    kind: etchy_core::LayerKind,
    overrides: &[(etchy_core::LayerKind, Color32)],
    theme: Theme,
) -> Color32 {
    overrides
        .iter()
        .find(|(k, _)| *k == kind)
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
    selected: usize,   // index into diff.layers
    mode: Mode,
    /// Always-available faint base behind the diff (G3): Off / Faint / Strong.
    base_level: BaseLevel,
    /// User-configurable diff colors (G3, Altium-compare style). Default to the
    /// brand green/red; a "Colors" popover edits them.
    col_added: Color32,
    col_removed: Color32,
    /// Per-layer-kind base/context colour overrides (default = layer_type_color).
    base_overrides: Vec<(etchy_core::LayerKind, Color32)>,
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
}

impl ViewApp {
    fn new(diff: BoardDiff, old_label: String, new_label: String) -> Self {
        let mut order: Vec<usize> = (0..diff.layers.len()).collect();
        order.sort_by_key(|&i| !diff.layers[i].is_changed()); // changed first, stable
        let selected = order.first().copied().unwrap_or(0);
        let outline = pick_outline_index(diff.layers.len(), |i| diff.layers[i].kind);
        Self {
            diff,
            old_label,
            new_label,
            order,
            selected,
            mode: Mode::Overlay,
            base_level: BaseLevel::Faint,
            col_added: C_ADDED,
            col_removed: C_REMOVED,
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
/// Default min-area threshold (mm²). Diff regions smaller than this are treated
/// as noise (e.g. the sub-µm rims from a units/precision mismatch) and dropped —
/// but the count is always surfaced in the caption, never silently.
const MIN_AREA_MM2: f64 = 0.0004;

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Keyboard shortcuts. The egui UI has no text inputs (the feedback widget
        // lives in the host HTML), so these are always safe to read.
        let (toggle_base, fit, overlay, before, after, next, prev, toggle_outline) =
            ui.input(|i| {
                use egui::Key;
                (
                    i.key_pressed(Key::S),
                    i.key_pressed(Key::F),
                    i.key_pressed(Key::O),
                    i.key_pressed(Key::B),
                    i.key_pressed(Key::A),
                    i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J),
                    i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K),
                    i.key_pressed(Key::E),
                )
            });
        if toggle_base {
            self.base_level = cycle_base(self.base_level);
        }
        if toggle_outline {
            self.show_outline = !self.show_outline;
        }
        if fit {
            self.cam.fitted = false;
        }
        if overlay {
            self.mode = Mode::Overlay;
        }
        if before {
            self.mode = Mode::Before;
        }
        if after {
            self.mode = Mode::After;
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
                    egui::RichText::new(format!("{}  →  {}", self.old_label, self.new_label))
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
            // Controls row: larger hit targets than the egui default.
            ui.horizontal(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.selectable_value(&mut self.mode, Mode::Overlay, "Overlay");
                ui.selectable_value(&mut self.mode, Mode::Before, "Before");
                ui.selectable_value(&mut self.mode, Mode::After, "After");
                ui.selectable_value(&mut self.mode, Mode::Split, "Split");
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
                    // Linear range (user found the log feel odd — #52).
                    egui::Slider::new(&mut self.min_area_mm2, 0.0..=0.02)
                        .text("noise filter (mm²)")
                        .fixed_decimals(4),
                )
                .on_hover_text(
                    "Drop diff regions smaller than this as noise; 0 = off. \
                     The hidden count is shown in the canvas caption.",
                );
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
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Group into sections (copper / mask / silk / …) in fixed order,
                    // changed-first within each (G5).
                    let groups =
                        group_layers(&self.order, |i| layer_group(self.diff.layers[i].kind));
                    for (group, idxs) in groups {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(group.title())
                                .small()
                                .color(Color32::from_rgb(0xe8, 0xa3, 0x3d)),
                        );
                        for idx in idxs {
                            let l = &self.diff.layers[idx];
                            // Just the layer name — no per-layer figures (#7) and no
                            // status glyph (#11; ●/○ render as tofu in the web font).
                            // Changed layers read strong, unchanged are dimmed.
                            let name = short_layer_name(l.kind);
                            let label = if l.is_changed() {
                                egui::RichText::new(name).strong()
                            } else {
                                egui::RichText::new(name).weak()
                            };
                            if ui.selectable_label(idx == self.selected, label).clicked() {
                                self.select(idx);
                            }
                        }
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.draw_canvas(ui);
        });

        // Colors editor — a real Window (not a menu) so the nested colour-picker
        // popup works; a menu_button closed on the first click inside it.
        if self.show_colors {
            let kinds = present_layer_kinds(self.diff.layers.iter().map(|l| l.kind));
            let mut open = true;
            egui::Window::new("Colors")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
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
                    ui.label(egui::RichText::new("Layer base colours").strong());
                    // One row per layer kind present on the board (#5/#10).
                    for kind in kinds {
                        ui.horizontal(|ui| {
                            let mut base =
                                resolve_base_color(kind, &self.base_overrides, self.theme);
                            if ui.color_edit_button_srgba(&mut base).changed() {
                                if let Some(e) =
                                    self.base_overrides.iter_mut().find(|(k, _)| *k == kind)
                                {
                                    e.1 = base;
                                } else {
                                    self.base_overrides.push((kind, base));
                                }
                            }
                            ui.label(short_layer_name(kind));
                            if self.base_overrides.iter().any(|(k, _)| *k == kind)
                                && ui.small_button("reset").clicked()
                            {
                                self.base_overrides.retain(|(k, _)| *k != kind);
                            }
                        });
                    }
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
        chrome(self.theme).canvas.to_normalized_gamma_f32()
    }
}

impl ViewApp {
    fn draw_canvas(&mut self, ui: &mut egui::Ui) {
        let layer = &self.diff.layers[self.selected];
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, chrome(self.theme).canvas);

        // Fit on first show / layer change.
        if !self.cam.fitted {
            if let Some(bb) = layer_bbox(layer) {
                fit(&mut self.cam, bb, rect);
            }
            self.cam.fitted = true;
        }

        // Pan.
        if response.dragged() {
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

        // Build the shapes to draw, per mode.
        // Geometry is triangulated ONCE and cached in world space (G6); only the
        // cheap world→screen transform + colour/alpha/min-area cull run per frame,
        // so pan/zoom and colour edits never re-triangulate. The cache rebuilds only
        // when the GeomKey (selection inputs) changes.
        let key = build_geom_key(
            self.selected,
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
        let base_color = resolve_base_color(
            self.diff.layers[self.selected].kind,
            &self.base_overrides,
            self.theme,
        );
        let cache = self.cache.as_ref().expect("cache built above");
        let n;
        if self.mode == Mode::Split {
            // Side-by-side: old (left) and new (right) halves, one shared camera,
            // each clipped to its half so geometry can't bleed past the divider (G4).
            let (lr, rr, div_x) = split_rects(rect, 0.5, 6.0);
            // One mesh per side (not per item) → a single clipped draw per half,
            // matching the smooth non-split path instead of a painter per item.
            let mut lmesh = egui::epaint::Mesh::default();
            let mut rmesh = egui::epaint::Mesh::default();
            // Base boards honour the base-level (faint/strong dimming) — #44; the
            // board outline (Side::Full) is drawn into BOTH halves for orientation — #45.
            let base_col =
                base_display_color(base_color, chrome(self.theme).canvas, self.base_level);
            for item in &cache.items {
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
            painter.line_segment(
                [
                    Pos2::new(div_x, rect.top()),
                    Pos2::new(div_x, rect.bottom()),
                ],
                Stroke::new(1.5, C_COPPER),
            );
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
                base_color,
                chrome(self.theme).canvas,
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
    tris: Vec<[Pt; 3]>,
    /// World bbox [minx, miny, maxx, maxy] — for off-screen culling per frame.
    bbox: [i64; 4],
    extent_nm: i64,
    area_nm2: f64,
}

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

fn push_context_items(items: &mut Vec<CachedItem>, set: &PolygonSet, role: Role, side: Side) {
    for shape in &set.shapes {
        let Some(outer) = shape.first() else { continue };
        if outer.len() < 3 {
            continue;
        }
        let tris = etchy_core::triangulate_shape(shape);
        if !tris.is_empty() {
            items.push(CachedItem {
                role,
                side,
                tris,
                bbox: ring_bbox(outer),
                extent_nm: 0,
                area_nm2: 0.0,
            });
        }
    }
}

fn push_diff_items(items: &mut Vec<CachedItem>, set: &PolygonSet, role: Role) {
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
                tris,
                bbox: bb,
                extent_nm,
                area_nm2,
            });
        }
    }
}

/// Triangulate the geometry selected by `key` into world-space items, once.
fn build_cache(diff: &BoardDiff, key: &GeomKey, outline: Option<usize>) -> TessCache {
    let mut items = Vec::new();
    // Outline first (drawn underneath), in all modes (G10).
    if key.outline_effective {
        if let Some(oi) = outline {
            let lo = &diff.layers[oi];
            let set = if !lo.new.shapes.is_empty() {
                &lo.new
            } else {
                &lo.old
            };
            push_context_items(&mut items, set, Role::Outline, Side::Full);
        }
    }
    let layer = &diff.layers[key.selected];
    match key.mode {
        Mode::Before => push_context_items(&mut items, &layer.old, Role::Base, Side::Full),
        Mode::After => push_context_items(&mut items, &layer.new, Role::Base, Side::Full),
        Mode::Overlay => {
            if key.base_on {
                push_context_items(&mut items, &layer.new, Role::Base, Side::Full);
            }
            push_diff_items(&mut items, &layer.removed, Role::Removed);
            push_diff_items(&mut items, &layer.added, Role::Added);
        }
        Mode::Split => {
            // Base honours the base-level control (Off omits the boards) — #44.
            if key.base_on {
                push_context_items(&mut items, &layer.old, Role::Base, Side::Left);
                push_context_items(&mut items, &layer.new, Role::Base, Side::Right);
            }
        }
    }
    TessCache {
        key: key.clone(),
        items,
    }
}

/// Per-frame: transform cached world items to screen meshes, applying colour, the
/// LOD fade (diff only), and the min-area cull (returns the hidden count). No
/// triangulation here — this is the cheap part that runs every frame.
#[allow(clippy::too_many_arguments)]
fn transform_cache(
    cache: &TessCache,
    cam: &Camera,
    rect: Rect,
    base_level: BaseLevel,
    base_color: Color32,
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
        let (mut color, is_diff) = match item.role {
            Role::Base => (base_display_color(base_color, canvas, base_level), false),
            Role::Outline => (C_OUTLINE_FAINT, false),
            Role::Added => (col_added, true),
            Role::Removed => (col_removed, true),
        };
        if is_diff {
            if item.area_nm2 < min_area_nm2 {
                hidden += 1;
                continue;
            }
            // Fade by on-screen THICKNESS, not extent: a long thin crescent has a
            // large extent but is sub-pixel thick — fading by thickness stops it
            // flickering by position when panned (#46).
            let thickness = feature_thickness_nm(item.area_nm2, item.extent_nm);
            let alpha =
                lod::geometry_alpha(region_screen_px(thickness, cam.scale), LOD_LO_PX, LOD_HI_PX);
            if alpha <= 0.0 {
                continue;
            }
            color = with_alpha(color, alpha);
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

fn screen_to_world(cam: &Camera, s: Pos2, rect: Rect) -> [f64; 2] {
    let wx = cam.center[0] + (s.x - rect.center().x) as f64 / cam.scale;
    let wy = cam.center[1] - (s.y - rect.center().y) as f64 / cam.scale;
    [wx, wy]
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
        base_display_color, build_geom_key, cycle_base, geom_cache_dirty, group_layers,
        layer_group, outline_legend_visible, pick_outline_index, region_screen_px,
        scroll_to_camera_action, short_layer_name, step_in_order, warning_phase, BaseLevel,
        CameraAction, LayerGroup, Mode, WarningPhase,
    };
    use etchy_core::LayerKind;

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
        let base = build_geom_key(0, Mode::Overlay, BaseLevel::Faint, true, Some(1));
        // base Faint vs Strong is a colour, not geometry -> same key (no rebuild)
        assert_eq!(
            base,
            build_geom_key(0, Mode::Overlay, BaseLevel::Strong, true, Some(1))
        );
        // base Off flips base_on -> different key (the base mesh joins/leaves the draw)
        assert_ne!(
            base,
            build_geom_key(0, Mode::Overlay, BaseLevel::Off, true, Some(1))
        );
        // selected / mode changes -> different key
        assert_ne!(
            base,
            build_geom_key(2, Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
        assert_ne!(
            base,
            build_geom_key(0, Mode::Before, BaseLevel::Faint, true, Some(1))
        );
        // viewing the outline layer itself -> outline not drawn -> different key
        assert_ne!(
            base,
            build_geom_key(1, Mode::Overlay, BaseLevel::Faint, true, Some(1))
        );
    }

    #[test]
    fn geom_cache_dirty_on_none_or_change() {
        let k = build_geom_key(0, Mode::Overlay, BaseLevel::Faint, false, None);
        assert!(geom_cache_dirty(None, &k));
        assert!(!geom_cache_dirty(Some(&k), &k));
        let k2 = build_geom_key(2, Mode::Overlay, BaseLevel::Faint, false, None);
        assert!(geom_cache_dirty(Some(&k), &k2));
    }

    #[test]
    fn region_screen_px_scales_extent() {
        assert_eq!(region_screen_px(1000, 0.5), 500.0);
        assert_eq!(region_screen_px(0, 2.0), 0.0);
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
    fn present_layer_kinds_dedupes_in_order() {
        use super::present_layer_kinds;
        let kinds = [
            LayerKind::TopCopper,
            LayerKind::InnerCopper(1),
            LayerKind::TopCopper,
            LayerKind::BottomMask,
            LayerKind::InnerCopper(1),
        ];
        assert_eq!(
            present_layer_kinds(kinds.iter().copied()),
            vec![
                LayerKind::TopCopper,
                LayerKind::InnerCopper(1),
                LayerKind::BottomMask
            ]
        );
    }

    #[test]
    fn layer_type_color_is_distinct_per_family() {
        use super::{layer_type_color, Theme, C_COPPER, C_CREAM};
        let d = Theme::Dark;
        assert_eq!(layer_type_color(LayerKind::TopCopper, d), C_COPPER);
        assert_eq!(layer_type_color(LayerKind::InnerCopper(2), d), C_COPPER);
        assert_eq!(layer_type_color(LayerKind::BottomSilk, d), C_CREAM);
        // mask, paste, copper read as different families
        assert_ne!(
            layer_type_color(LayerKind::TopMask, d),
            layer_type_color(LayerKind::TopCopper, d)
        );
        assert_ne!(
            layer_type_color(LayerKind::TopPaste, d),
            layer_type_color(LayerKind::TopMask, d)
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
        let ovr = [(LayerKind::TopCopper, Color32::from_rgb(1, 2, 3))];
        assert_eq!(
            resolve_base_color(LayerKind::TopCopper, &ovr, d),
            Color32::from_rgb(1, 2, 3)
        );
        // no override for this kind -> the type default
        assert_eq!(
            resolve_base_color(LayerKind::TopSilk, &ovr, d),
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
}
