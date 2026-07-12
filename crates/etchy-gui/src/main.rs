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
// Without the `pdf` feature the view model can't be constructed, so most of the
// module is (deliberately) dead — the pure helpers stay compiled + unit-tested.
#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
mod pdfview;

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use etchy_core::{BoardDiff, LayerStatus, LayerView, PolygonSet, Pt};
use pdfview::PdfView;

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
        // 4x MSAA so sub-pixel slivers (thin track/pad junctions, shared edges)
        // cover at least one sample and don't drop out as "no copper" (#55/#47).
        // BUT under software GL — Mesa llvmpipe on WSLg (no GPU passthrough) —
        // rasterisation runs across every core, so 4x MSAA on a dense board
        // pegs the whole machine per rendered frame (#215). Drop MSAA there and
        // keep it on real GPUs; `ETCHY_MSAA=<n>` overrides either way (e.g. a WSL
        // user with working GPU passthrough who wants the samples back). Diffs
        // stay safe without MSAA — sub-pixel changes are marker-LOD'd, not
        // multisample-dependent.
        let multisampling: u16 = std::env::var("ETCHY_MSAA")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(if is_wsl() { 0 } else { 4 });
        if multisampling == 0 {
            eprintln!(
                "etchy-gui: MSAA off for software rendering (WSL) — set ETCHY_MSAA=4 to force it on"
            );
        }
        let native_options = eframe::NativeOptions {
            viewport,
            multisampling,
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

    /// Load both revisions from CLI paths — Gerber dirs/zips or schematic PDFs
    /// (#63) — into sources + their computed diff.
    fn load_seed(
        old_path: &Path,
        new_path: &Path,
    ) -> anyhow::Result<(LoadedSource, LoadedSource, ComputedDiff, f32)> {
        let old = load_source_any(old_path, label(old_path))?;
        let new = load_source_any(new_path, label(new_path))?;
        // Persisted settings aren't restored yet at this point — seed a PDF pair
        // at the default DPI; `build_app` re-rasterizes if the restored setting
        // differs (#223). The raster caps scale with DPI, so a pair that only
        // fits at a lower DPI falls back to the CLI default rather than
        // refusing to open at all (#223 review) — the DPI actually used is
        // returned so build_app can keep the setting honest.
        let (diff, dpi) = match compute_diff(&old, &new, PDF_DPI_DEFAULT) {
            Ok(d) => (d, PDF_DPI_DEFAULT),
            Err(first) => match compute_diff(&old, &new, PDF_DPI_FALLBACK) {
                Ok(d) => (d, PDF_DPI_FALLBACK),
                Err(_) => return Err(first),
            },
        };
        Ok((old, new, diff, dpi))
    }

    /// Construct the app, restoring persisted settings. With a seed, both sources
    /// are set so either can be reopened; without one, the welcome screen shows
    /// (empty diff).
    fn build_app(
        cc: &eframe::CreationContext<'_>,
        seed: Option<(LoadedSource, LoadedSource, ComputedDiff, f32)>,
    ) -> ViewApp {
        match seed {
            Some((old, new, computed, seed_dpi)) => {
                let (ol, nl) = (old.label().to_string(), new.label().to_string());
                let mut app = match computed {
                    ComputedDiff::Board(diff) => ViewApp::from_cc(cc, diff, ol, nl),
                    ComputedDiff::Pdf(view) => {
                        let mut app = ViewApp::from_cc(cc, empty_diff(), ol, nl);
                        app.pdf = Some(view);
                        app
                    }
                };
                app.src_old = Some(old);
                app.src_new = Some(new);
                // The seed was rasterized before settings restore; honour a
                // different persisted DPI now (#223). If that re-rasterize
                // fails (caps), the setting must fall back to the DPI the
                // on-screen view was actually built at — the chip may never
                // claim a DPI the view isn't (#223 review).
                if app.pdf.is_some() && app.pdf_dpi != seed_dpi {
                    app.rebuild_diff();
                    if app.load_error.is_some() {
                        app.pdf_dpi = seed_dpi;
                    }
                }
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
                    app.src_old = Some(LoadedSource::Board(old));
                    app.src_new = Some(LoadedSource::Board(new));
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
    Export,
    Settings,
}

impl PanelTab {
    /// The rail's top-down panel tabs. Settings is NOT here — its gear is pinned
    /// to the rail's bottom (#199) but drives the same `toggle_panel` flow. Each
    /// tab has a full body: Layers (`layers_panel_ui`), Export
    /// (`export_panel_ui`), Settings (`settings_panel_ui`). Measure is NOT a tab
    /// (#211): its rail ruler icon is a plain tool toggle that opens no panel.
    const ALL: [PanelTab; 2] = [PanelTab::Layers, PanelTab::Export];

    fn label(self) -> &'static str {
        match self {
            PanelTab::Layers => "Layers",
            PanelTab::Export => "Export",
            PanelTab::Settings => "Settings",
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

/// A key press that can clear measure state (#50, #198).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MeasureKey {
    Escape,
    ShiftC,
}

/// What a measure-clearing key press does (#198). Pure → unit-testable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MeasureAction {
    /// Nothing to do.
    None,
    /// Clear the in-progress point; stay in the tool.
    ClearPoints,
    /// Clear the completed measurements list.
    ClearList,
    /// Leave measure mode.
    ExitTool,
}

/// Resolve a measure-clearing key press (#50, #198). Esc cascades: while a
/// measurement is in progress it clears the point but keeps measure mode on;
/// under the KiCad preset the next Esc clears the completed list (matching
/// KiCad's own binding, even when the tool is disarmed); an Esc with nothing
/// left to clear exits measure mode. Shift+C clears the completed list under
/// the Altium preset (its native binding) and is unbound under KiCad.
/// `custom_clear` = the user rebound Clear-measurements in the Hotkeys editor
/// (#201): the preset list-clears stand down so only the explicit binding
/// clears the list (Esc keeps its point-clear and exit steps).
fn measure_key_action(
    preset: InputPreset,
    key: MeasureKey,
    measure_mode: bool,
    has_points: bool,
    has_list: bool,
    custom_clear: bool,
) -> MeasureAction {
    match key {
        MeasureKey::ShiftC => {
            if !custom_clear && preset == InputPreset::Altium && has_list {
                MeasureAction::ClearList
            } else {
                MeasureAction::None
            }
        }
        MeasureKey::Escape => {
            if has_points {
                MeasureAction::ClearPoints
            } else if !custom_clear && preset == InputPreset::KiCad && has_list {
                MeasureAction::ClearList
            } else if measure_mode {
                MeasureAction::ExitTool
            } else {
                MeasureAction::None
            }
        }
    }
}

/// One rebindable hotkey (#201): a key plus its exact modifier set. `ctrl` maps
/// to egui's `command` modifier (Ctrl on Windows/Linux, Cmd on mac), matching
/// the shipped Ctrl+M wiring (#197). Persists as its display string ("Ctrl+M")
/// via [`format_binding`] / [`parse_binding`], so the config stays hand-readable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct KeyBinding {
    ctrl: bool,
    shift: bool,
    alt: bool,
    key: egui::Key,
}

impl KeyBinding {
    /// A bare key with no modifiers.
    const fn plain(key: egui::Key) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }
}

/// Render a binding the way menus show shortcuts: "Ctrl+Shift+Alt+Key".
fn format_binding(b: KeyBinding) -> String {
    let mut s = String::new();
    if b.ctrl {
        s.push_str("Ctrl+");
    }
    if b.shift {
        s.push_str("Shift+");
    }
    if b.alt {
        s.push_str("Alt+");
    }
    s.push_str(b.key.name());
    s
}

/// Parse [`format_binding`]'s output back into a binding. `None` on anything
/// unknown (fail-loud: a bad persisted string falls back to the defaults rather
/// than guessing a key).
fn parse_binding(s: &str) -> Option<KeyBinding> {
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    let mut parts = s.split('+').peekable();
    let mut key = None;
    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            match part {
                "Ctrl" => ctrl = true,
                "Shift" => shift = true,
                "Alt" => alt = true,
                _ => return None,
            }
        } else {
            key = egui::Key::from_name(part);
        }
    }
    Some(KeyBinding {
        ctrl,
        shift,
        alt,
        key: key?,
    })
}

/// A binding persists as its display string (see [`format_binding`]); unknown
/// strings error, which drops the whole `Settings` blob back to defaults —
/// fail-loud over a silently wrong key.
impl serde::Serialize for KeyBinding {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&format_binding(*self))
    }
}
impl<'de> serde::Deserialize<'de> for KeyBinding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_binding(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown key binding {s:?}")))
    }
}

/// The rebindable actions (#201), addressed by the Hotkeys editor and resolved
/// against the [`Keymap`] by the dispatch in `fn ui`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HotkeyAction {
    ToggleMeasure,
    ClearMeasurements,
    FitView,
    CycleBase,
    CycleUnit,
    ToggleGrid,
    ModeOverlay,
    ModeOld,
    ModeNew,
    ModeSplit,
    ModeSwipe,
}

impl HotkeyAction {
    /// (action, editor label), in the order the Hotkeys section lists them.
    const ALL: [(HotkeyAction, &'static str); 11] = [
        (HotkeyAction::ToggleMeasure, "Toggle measure"),
        (HotkeyAction::ClearMeasurements, "Clear measurements"),
        (HotkeyAction::FitView, "Fit view"),
        (HotkeyAction::CycleBase, "Cycle base opacity"),
        (HotkeyAction::CycleUnit, "Cycle measure units"),
        (HotkeyAction::ToggleGrid, "Toggle grid"),
        (HotkeyAction::ModeOverlay, "Mode: Overlay"),
        (HotkeyAction::ModeOld, "Mode: Old"),
        (HotkeyAction::ModeNew, "Mode: New"),
        (HotkeyAction::ModeSplit, "Mode: Split"),
        (HotkeyAction::ModeSwipe, "Mode: Swipe"),
    ];
}

/// The user's hotkey map (#201), persisted inside [`Settings`]. Missing fields
/// deserialize to the defaults (container-level `serde(default)`), so pre-#201
/// configs and partial blobs both come up sane.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
struct Keymap {
    toggle_measure: KeyBinding,
    /// `None` = the #198 preset defaults stand (Altium Shift+C / KiCad Esc).
    /// An explicit rebind overrides them and stands the presets down (see
    /// `measure_key_action`'s `custom_clear`).
    clear_measure: Option<KeyBinding>,
    fit_view: KeyBinding,
    cycle_base: KeyBinding,
    /// `None` = the preset defaults stand: Altium **Q**, KiCad **Ctrl+U** —
    /// each tool's own units toggle (#211). An explicit rebind overrides them,
    /// same pattern as `clear_measure`. Persisted under a new name so a
    /// pre-#211 config's fixed `"cycle_unit":"U"` default is dropped (back to
    /// the preset) instead of read in as an explicit rebind pinning U forever.
    #[serde(rename = "cycle_units")]
    cycle_unit: Option<KeyBinding>,
    toggle_grid: KeyBinding,
    mode_overlay: KeyBinding,
    mode_old: KeyBinding,
    mode_new: KeyBinding,
    mode_split: KeyBinding,
    mode_swipe: KeyBinding,
}

impl Default for Keymap {
    fn default() -> Self {
        use egui::Key;
        Self {
            // Ctrl+M arms/disarms the measure tool (#197).
            toggle_measure: KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::M,
            },
            clear_measure: None,
            fit_view: KeyBinding::plain(Key::F),
            cycle_base: KeyBinding::plain(Key::S),
            cycle_unit: None,
            toggle_grid: KeyBinding::plain(Key::G),
            mode_overlay: KeyBinding::plain(Key::Num1),
            mode_old: KeyBinding::plain(Key::Num2),
            mode_new: KeyBinding::plain(Key::Num3),
            mode_split: KeyBinding::plain(Key::Num4),
            mode_swipe: KeyBinding::plain(Key::Num5),
        }
    }
}

impl Keymap {
    /// The current binding for an action; `None` only for Clear-measurements
    /// and Cycle-measure-units while they still ride their preset defaults
    /// (#198 clear, #211 units).
    fn get(&self, a: HotkeyAction) -> Option<KeyBinding> {
        match a {
            HotkeyAction::ToggleMeasure => Some(self.toggle_measure),
            HotkeyAction::ClearMeasurements => self.clear_measure,
            HotkeyAction::FitView => Some(self.fit_view),
            HotkeyAction::CycleBase => Some(self.cycle_base),
            HotkeyAction::CycleUnit => self.cycle_unit,
            HotkeyAction::ToggleGrid => Some(self.toggle_grid),
            HotkeyAction::ModeOverlay => Some(self.mode_overlay),
            HotkeyAction::ModeOld => Some(self.mode_old),
            HotkeyAction::ModeNew => Some(self.mode_new),
            HotkeyAction::ModeSplit => Some(self.mode_split),
            HotkeyAction::ModeSwipe => Some(self.mode_swipe),
        }
    }

    /// Rebind an action (a successful capture).
    fn set(&mut self, a: HotkeyAction, b: KeyBinding) {
        match a {
            HotkeyAction::ToggleMeasure => self.toggle_measure = b,
            HotkeyAction::ClearMeasurements => self.clear_measure = Some(b),
            HotkeyAction::FitView => self.fit_view = b,
            HotkeyAction::CycleBase => self.cycle_base = b,
            HotkeyAction::CycleUnit => self.cycle_unit = Some(b),
            HotkeyAction::ToggleGrid => self.toggle_grid = b,
            HotkeyAction::ModeOverlay => self.mode_overlay = b,
            HotkeyAction::ModeOld => self.mode_old = b,
            HotkeyAction::ModeNew => self.mode_new = b,
            HotkeyAction::ModeSplit => self.mode_split = b,
            HotkeyAction::ModeSwipe => self.mode_swipe = b,
        }
    }

    /// The effective cycle-units binding (#211): the explicit rebind when one
    /// exists, else the input preset's default (`preset_unit_binding`).
    fn unit_binding(&self, preset: InputPreset) -> KeyBinding {
        self.cycle_unit
            .unwrap_or_else(|| preset_unit_binding(preset))
    }
}

/// The preset default for the cycle-units hotkey (#211): each ECAD tool's own
/// units toggle — Altium **Q**, KiCad **Ctrl+U**. Stands until an explicit
/// rebind (`Keymap::cycle_unit`) overrides it, like the clear-measure preset
/// defaults (#198). Pure → unit-testable.
fn preset_unit_binding(preset: InputPreset) -> KeyBinding {
    match preset {
        InputPreset::Altium => KeyBinding::plain(egui::Key::Q),
        InputPreset::KiCad => KeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            key: egui::Key::U,
        },
    }
}

/// Why a candidate binding can't be used: the label of whatever already owns
/// it — another rebindable action, a fixed plain-key alias (O/B/A modes,
/// J/K/arrow layer-step, Esc), or a preset-driven default while no custom
/// binding stands it down (the Altium Shift+C clear, the preset units-cycle
/// key). Checked at capture time so a rebind can never make one key press
/// dispatch two actions (#201 review).
fn binding_conflict(km: &Keymap, action: HotkeyAction, b: KeyBinding) -> Option<&'static str> {
    use egui::Key;
    for (other, label) in HotkeyAction::ALL {
        if other != action && km.get(other) == Some(b) {
            return Some(label);
        }
    }
    if !b.ctrl && !b.shift && !b.alt {
        match b.key {
            Key::O => return Some("Mode: Overlay (fixed alias)"),
            Key::B => return Some("Mode: Old (fixed alias)"),
            Key::A => return Some("Mode: New (fixed alias)"),
            Key::J | Key::K | Key::ArrowDown | Key::ArrowUp => {
                return Some("layer step (fixed)");
            }
            Key::Escape => return Some("measure escape (fixed)"),
            _ => {}
        }
    }
    // Preset-driven defaults are reserved regardless of which Input preset is
    // ACTIVE — otherwise a key bound while one preset is selected would collide
    // the moment the user switches presets, making one press dispatch two
    // actions (#211 review). Altium's Shift+C clear (KiCad's clear is the fixed
    // Esc, reserved above):
    if action != HotkeyAction::ClearMeasurements
        && km.clear_measure.is_none()
        && b.key == Key::C
        && b.shift
        && !b.ctrl
        && !b.alt
    {
        return Some("Clear measurements (preset Shift+C)");
    }
    // Either preset's units-cycle key (Altium Q, KiCad Ctrl+U), while no custom
    // cycle-units binding stands it down.
    if action != HotkeyAction::CycleUnit
        && km.cycle_unit.is_none()
        && (b == preset_unit_binding(InputPreset::Altium)
            || b == preset_unit_binding(InputPreset::KiCad))
    {
        return Some("Cycle measure units (preset)");
    }
    None
}

/// Outcome of a key press while a rebind capture is armed (#201).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CaptureResult {
    Bind(KeyBinding),
    Cancel,
}

/// Rebind capture (#201): the next key press (with its modifiers) becomes the
/// binding; Esc cancels — even with modifiers held, so Esc itself is never
/// bindable (it stays the universal way out). Pure → unit-testable.
fn capture_key(key: egui::Key, ctrl: bool, shift: bool, alt: bool) -> CaptureResult {
    if key == egui::Key::Escape {
        CaptureResult::Cancel
    } else {
        CaptureResult::Bind(KeyBinding {
            ctrl,
            shift,
            alt,
            key,
        })
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

/// One loaded schematic PDF revision (#63): the raw bytes, rasterized when both
/// sides are present (the diff needs both, and DPI/pairing are pair-level).
struct LoadedPdf {
    label: String,
    bytes: Vec<u8>,
}

/// One loaded input revision: a Gerber board or a schematic PDF (#63). The two
/// kinds never mix — `compute_diff` fails loud on a PDF-vs-Gerber pair.
enum LoadedSource {
    Board(LoadedBoard),
    Pdf(LoadedPdf),
}

impl LoadedSource {
    fn label(&self) -> &str {
        match self {
            LoadedSource::Board(b) => &b.label,
            LoadedSource::Pdf(p) => &p.label,
        }
    }
}

/// The computed comparison of two sources: a geometric board diff, or a per-page
/// PDF pixel diff (#63).
enum ComputedDiff {
    Board(BoardDiff),
    Pdf(PdfView),
}

/// Diff two loaded sources. Both Gerber → the geometry diff; both PDF → the
/// per-page pixel diff at `pdf_dpi` (a loud error when this build lacks the
/// `pdf` feature); mixed → a loud error, never a guess.
fn compute_diff(
    old: &LoadedSource,
    new: &LoadedSource,
    pdf_dpi: f32,
) -> anyhow::Result<ComputedDiff> {
    match (old, new) {
        (LoadedSource::Board(o), LoadedSource::Board(n)) => {
            Ok(ComputedDiff::Board(diff_from_sources(o, n)?))
        }
        (LoadedSource::Pdf(o), LoadedSource::Pdf(n)) => Ok(ComputedDiff::Pdf(
            pdfview::build_pdf_view(&o.bytes, &n.bytes, pdf_dpi)?,
        )),
        _ => anyhow::bail!(
            "cannot compare a PDF with Gerber input — load two schematic PDFs \
             or two board revisions"
        ),
    }
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
    result: anyhow::Result<LoadedSource>,
}

/// Default Focus (#224): a gentle highlight — non-selected layers at 75% so the
/// stack reads as context behind the selected layer without hiding anything.
const FOCUS_DEFAULT: f32 = 0.25;

/// The GUI's default PDF rasterization DPI (#223). Deliberately HIGHER than the
/// CLI's `etchy_pdf::DEFAULT_DPI` (150): 150 reads soft when zooming a schematic
/// on screen, while the CLI's overlay PNGs are usually consumed at 1:1 and keep
/// their `--dpi` flag for anything else.
const PDF_DPI_DEFAULT: f32 = 200.0;

/// Fallback for the CLI-args seed (#223 review): the raster caps scale with
/// DPI, so a pair that fit at the old 150 default must still open — it seeds at
/// 150 when 200 breaches a cap, and the Settings chip reflects the DPI used.
const PDF_DPI_FALLBACK: f32 = 150.0;

/// The Settings > Diff DPI chips (#223). Bounded choices, not a free slider —
/// each step is checked against the raster caps at load, and a chip that
/// breaches them fails loud and reverts.
const PDF_DPI_CHOICES: [f32; 3] = [150.0, 200.0, 300.0];

/// Floor for the Layers-panel row ghosting (#224): rows mirror the canvas focus
/// dim but never fade below this — a control you need to click back must stay
/// legible even at focus 100%.
const ROW_GHOST_FLOOR: f32 = 0.35;

/// The Focus dim (#224, owner-locked): the alpha multiplier for a VISIBLE layer
/// under the Focus slider. The selected layer always draws at full strength;
/// every other visible layer's whole render — base AND diff geometry — scales by
/// `1 - focus`. Focus 0 = all visible layers equal; focus 1 = only the selected
/// layer visible. Pure → unit-testable. Replaces the deleted view segment
/// (single/highlight/all/none): the per-row eyes are the ONLY visibility
/// control, and this one slider is the only emphasis control.
fn focus_alpha(focus: f32, is_selected: bool) -> f32 {
    if is_selected {
        1.0
    } else {
        1.0 - focus.clamp(0.0, 1.0)
    }
}

/// [`focus_alpha`] keyed by layer index: layer-less items (the outline
/// orientation reference, `NO_LAYER`) never dim — they are context, not a layer
/// competing for attention.
fn layer_focus_alpha(layer_index: usize, selected: usize, focus: f32) -> f32 {
    focus_alpha(focus, layer_index == selected || layer_index == NO_LAYER)
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
    /// The user's hotkey map (#201), edited in Settings > Hotkeys and persisted
    /// via #52. The dispatch in `fn ui` resolves its bindings every frame.
    keymap: Keymap,
    /// A Hotkeys rebind capture is armed for this action (#201): the next key
    /// press becomes its binding, Esc cancels. Runtime-only — never persisted.
    capture_action: Option<HotkeyAction>,
    /// Why the last capture was rejected (the binding is already taken) —
    /// shown under the Hotkeys grid; the capture stays armed for another try.
    capture_conflict: Option<String>,
    /// Set each frame the Hotkeys editor draws. A capture armed while the
    /// editor is off-screen (rail tab switched, panel collapsed) cancels
    /// instead of silently eating — and rebinding on — the next key press.
    hotkeys_drawn: bool,
    /// Focus 0..=1 (#224): how strongly the selected layer stands out — every
    /// other VISIBLE layer renders at `1 - focus` (base and diff alike). The
    /// slider at the top of the Layers panel drives it; persisted via #52.
    focus: f32,
    /// Rasterization DPI for schematic-PDF pairs (#223), set in Settings > Diff
    /// (150/200/300 chips). Changing it re-rasterizes a loaded PDF pair from the
    /// retained source bytes; a DPI that breaches the pixel caps fails loud into
    /// `load_error` and the setting reverts. Persisted via #52. The CLI default
    /// stays 150 (its --dpi flag covers it).
    pdf_dpi: f32,
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
    /// Completed measurements (#50): the running list the canvas draws as
    /// persistent rulers (#211 — no list UI; cleared by key, MEAS-4).
    /// Runtime-only and per-board — cleared on load.
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
    /// measure mode. On by default; toggled in Settings › Measure (#211).
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

    /// PDF mode (#63): the per-page raster diff shown instead of the board diff.
    /// `Some` exactly when the two loaded sources are PDFs; loading a Gerber pair
    /// resets it (and vice versa — `adopt_pdf` clears the board state).
    pdf: Option<PdfView>,
    /// Retained sources (#120) so either side can be reopened and re-diffed.
    /// `None` before a board is chosen (the welcome screen shows in that state).
    src_old: Option<LoadedSource>,
    src_new: Option<LoadedSource>,
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
    /// The top-bar brand mark (#191): the real etchy icon, decoded from the
    /// embedded PNG and uploaded to a texture once — cached here, never per frame.
    brand_tex: Option<egui::TextureHandle>,
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
            keymap: Keymap::default(),
            capture_action: None,
            capture_conflict: None,
            hotkeys_drawn: false,
            focus: FOCUS_DEFAULT,
            pdf_dpi: PDF_DPI_DEFAULT,
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
            pdf: None,
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
            brand_tex: None,
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
                content: etchy_core::layer_svg(l).into_bytes(),
            });
        }
        files.push(exportio::ExportFile {
            name: "areas.csv".into(),
            content: etchy_core::board_areas_csv(&self.diff).into_bytes(),
        });
        files
    }

    /// Where the native export folder is rooted (#222): next to the last opened
    /// input when known, so the output lands where the user is already looking.
    /// Web has no directory concept — downloads go wherever the browser puts them.
    fn export_dir_hint(&self) -> Option<std::path::PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.last_dir.clone()
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    /// Run an export and stash the result message for the toast.
    fn do_export(&mut self, all_layers: bool) {
        let files = self.build_export(all_layers);
        self.export_msg = Some(
            match exportio::save(&files, self.export_dir_hint().as_deref()) {
                Ok(msg) => msg,
                Err(e) => format!("export failed: {e}"),
            },
        );
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
    /// order — the keyboard equivalent of clicking the next/previous layer. In
    /// PDF mode the same keys step through the pages instead (#63).
    fn step_layer(&mut self, delta: i32) {
        if self.pdf.is_some() {
            let next = {
                let pv = self.pdf.as_ref().unwrap();
                step_in_order(&pv.order, pv.selected, delta)
            };
            self.select_pdf_page(next);
            return;
        }
        self.select(step_in_order(&self.order, self.selected, delta));
    }

    /// Select a PDF page. Unlike board layers — which share one physical world —
    /// each sheet is its own drawing at origin [0,0], so rulers measured on one
    /// page would render as stale, misleading annotations on another (#63
    /// review). Switching pages clears the measurements.
    fn select_pdf_page(&mut self, idx: usize) {
        if let Some(pv) = &mut self.pdf {
            if pv.selected != idx {
                pv.selected = idx;
                self.measurements.clear();
                self.measure_pts.clear();
            }
        }
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

/// The git short sha this binary was built from (#213), stamped by `build.rs`
/// at compile time — so it works identically on native and wasm, and a browser
/// tab can prove which build it runs (the stale-wasm confusion). Shown in the
/// Help menu and the brand icon's tooltip; "unknown" outside a git checkout.
const BUILD_SHA: &str = env!("ETCHY_BUILD_SHA");

/// Links for the Help menu.
const URL_REPO: &str = "https://github.com/Cimos/etchy";
const URL_ISSUES: &str = "https://github.com/Cimos/etchy/issues";
const URL_SITE: &str = "https://cimos.github.io";
const URL_SPONSOR: &str = "https://github.com/sponsors/Cimos";

/// The etchy brand icon (#191), embedded at compile time. 256 px source drawn at
/// ~20 pt in the top bar, so it stays crisp on any DPI. Decoded once into a
/// texture via [`ViewApp::brand_texture`].
const BRAND_ICON_PNG: &[u8] = include_bytes!("../../../assets/brand/png/etchy-icon-256.png");

/// On-screen size (pt) of the top-bar brand icon (#191).
const BRAND_ICON_PT: f32 = 21.0;

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
/// but the count is always surfaced on the canvas (the bottom-left hidden-count
/// chip, `hidden_note`), never silently.
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
    /// Focus 0..=1 (#224): non-selected visible layers render at `1 - focus`.
    /// Missing in pre-#224 configs → the default (serde(default) on the struct).
    /// A pre-#224 persisted `view_mode` string, if one ever existed, is simply
    /// an unknown field to serde and is ignored.
    focus: f32,
    /// PDF rasterization DPI (#223). Missing in pre-#223 configs → the default.
    pdf_dpi: f32,
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
    /// The user's hotkey map (#201). Missing in pre-#201 configs → defaults.
    keymap: Keymap,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            base_opacity: BASE_OPACITY_FAINT,
            base_level: None,
            focus: FOCUS_DEFAULT,
            pdf_dpi: PDF_DPI_DEFAULT,
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
            keymap: Keymap::default(),
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
            let dim = layer_focus_alpha(item.layer_index, self.selected, self.focus);
            if dim == 0.0 {
                continue; // fully focus-dimmed — same skip as the CPU path (#224)
            }
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
        // The Focus dim is baked into the uploaded vertex colours (#224).
        self.focus.to_bits().hash(&mut h);
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
            focus: self.focus,
            pdf_dpi: self.pdf_dpi,
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
            keymap: self.keymap,
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
        self.focus = s.focus.clamp(0.0, 1.0);
        // Any persisted DPI is honoured within sane raster bounds; the chips
        // only ever write PDF_DPI_CHOICES values.
        self.pdf_dpi = s.pdf_dpi.clamp(72.0, 600.0);
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
            // Selection is separate from visibility (#224): the restored eye
            // state is honoured as-is, even if it hides the selected layer.
            self.visible_layers = restore_visibility(&s.visible_layers, self.visible_layers.len());
        }
        self.swipe_frac = clamp_swipe_frac(s.swipe_frac);
        self.rail_side = s.rail_side;
        self.keymap = s.keymap;
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
        // A board load leaves PDF mode (#63): the page rasters/textures are
        // per-pair state and must never linger behind a board diff.
        self.pdf = None;
        #[cfg(feature = "gpu-transform")]
        {
            self.gpu_hash = None;
        }
    }

    /// Enter PDF mode (#63): the same full reset as `adopt_diff` (camera,
    /// measurements, caches, board state — via an empty board diff), then the
    /// page view takes over the canvas and panels.
    fn adopt_pdf(&mut self, view: PdfView, old_label: String, new_label: String) {
        self.adopt_diff(empty_diff(), old_label, new_label);
        self.pdf = Some(view);
    }

    /// Store a freshly loaded source on one side and re-diff if both sides are set.
    fn set_side(&mut self, side: RevSide, loaded: LoadedSource) {
        match side {
            RevSide::Old => self.src_old = Some(loaded),
            RevSide::New => self.src_new = Some(loaded),
        }
        self.load_error = None;
        self.rebuild_diff();
    }

    /// Recompute the comparison from the two sources, if both are present.
    fn rebuild_diff(&mut self) {
        let (Some(o), Some(n)) = (self.src_old.as_ref(), self.src_new.as_ref()) else {
            return;
        };
        let (ol, nl) = (o.label().to_string(), n.label().to_string());
        match compute_diff(o, n, self.pdf_dpi) {
            Ok(ComputedDiff::Board(diff)) => self.adopt_diff(diff, ol, nl),
            Ok(ComputedDiff::Pdf(view)) => self.adopt_pdf(view, ol, nl),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// Change the PDF rasterization DPI (#223). With a PDF pair loaded, the pair
    /// is re-rasterized from the retained source bytes at the new DPI; a DPI
    /// that breaches the pixel caps fails loud (`load_error`) and the setting
    /// reverts, leaving the previous view — still built at the previous DPI —
    /// on screen. Without a PDF pair it simply takes effect on the next load.
    fn set_pdf_dpi(&mut self, dpi: f32) {
        let prev = self.pdf_dpi;
        if dpi == prev {
            return;
        }
        self.pdf_dpi = dpi;
        let pdf_pair = matches!(
            (&self.src_old, &self.src_new),
            (Some(LoadedSource::Pdf(_)), Some(LoadedSource::Pdf(_)))
        );
        if pdf_pair {
            self.load_error = None;
            self.rebuild_diff();
            if self.load_error.is_some() {
                // Fail loud AND revert: the error stays visible, the setting
                // goes back to the DPI the on-screen view was built at.
                self.pdf_dpi = prev;
            }
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

    /// Native: open a file picker for `side` — a `.zip` fab pack or a schematic
    /// `.pdf` (#63).
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_zip(&mut self, side: RevSide) {
        let title = match side {
            RevSide::Old => "Open old revision — .zip fab pack or schematic PDF",
            RevSide::New => "Open new revision — .zip fab pack or schematic PDF",
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("fab pack / schematic PDF", &["zip", "pdf"]);
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
        match load_source_any(path, path_label(path)) {
            Ok(src) => self.set_side(side, src),
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
        // Web drops carry bytes; the shared classifier routes a `.zip`, a
        // schematic `.pdf` (#63), or loose Gerber layers.
        let byte_files: Vec<(String, Vec<u8>)> = files
            .into_iter()
            .filter_map(|f| f.bytes.map(|b| (basename(&f.name), b.to_vec())))
            .collect();
        if byte_files.is_empty() {
            return;
        }
        match files_to_source(byte_files, "dropped files") {
            Ok(src) => self.set_side(side, src),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// Web: open the browser file picker for `side` (multiple Gerbers, one
    /// `.zip`, or one schematic `.pdf` — #63); the result comes back over the
    /// channel and is drained next frame.
    #[cfg(target_arch = "wasm32")]
    fn pick_files_web(&mut self, side: RevSide, ctx: &egui::Context) {
        let tx = self.file_tx.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let Some(handles) = rfd::AsyncFileDialog::new()
                .add_filter(
                    "Gerber / fab pack / schematic PDF",
                    &[
                        "gbr", "gtl", "gbl", "gts", "gbs", "gto", "gbo", "gtp", "gbp", "gko",
                        "gm1", "zip", "pdf",
                    ],
                )
                .pick_files()
                .await
            else {
                return;
            };
            let mut byte_files: Vec<(String, Vec<u8>)> = Vec::new();
            for h in handles {
                let name = h.file_name();
                let bytes = h.read().await;
                byte_files.push((basename(&name), bytes));
            }
            let result = files_to_source(byte_files, "uploaded");
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
        .map(|l| l.label().to_string());
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

/// The initial empty diff shown before any board is loaded (the welcome screen),
/// and the board-state blank underneath PDF mode (#63, both surfaces).
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

/// Basename of a filename string (handles `/` and `\`). Used by the web loaders
/// (native uses `path_label`) and the shared in-memory classifier below.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn basename(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_string()
}

/// Classify a set of in-memory `(filename, bytes)` files into one loaded source
/// (#63): a single schematic PDF (by `.pdf` name or `%PDF` magic) → PDF mode; a
/// `.zip` → the fab-pack loader; anything else → loose Gerber layers. A PDF mixed
/// with other files is a loud error — one PDF per side, never a guess. Shared by
/// the web drop and web file-pick paths; pure over bytes, so it's unit-tested on
/// native too.
fn files_to_source(files: Vec<(String, Vec<u8>)>, label: &str) -> anyhow::Result<LoadedSource> {
    let pdf_count = files
        .iter()
        .filter(|(name, bytes)| {
            name.to_ascii_lowercase().ends_with(".pdf") || pdfview::looks_like_pdf(bytes)
        })
        .count();
    if pdf_count > 0 {
        if files.len() > 1 {
            anyhow::bail!(
                "load one schematic PDF per side — a PDF cannot mix with other layer files"
            );
        }
        let (name, bytes) = files.into_iter().next().expect("len checked above");
        if !pdfview::looks_like_pdf(&bytes) {
            anyhow::bail!("{name} has a .pdf name but no PDF content (%PDF magic missing)");
        }
        return Ok(LoadedSource::Pdf(LoadedPdf { label: name, bytes }));
    }
    let mut byte_files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut zip: Option<Vec<u8>> = None;
    for (name, bytes) in files {
        if name.to_ascii_lowercase().ends_with(".zip") {
            zip = Some(bytes);
        } else {
            byte_files.push((name, bytes));
        }
    }
    let (board, fmt) = if let Some(zb) = zip {
        loader::load_zip(zb)
    } else {
        loader::board_from_bytes(byte_files)
    }?;
    Ok(LoadedSource::Board(LoadedBoard {
        label: label.to_string(),
        board,
        fmt,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn path_label(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

/// Native: is this path a schematic PDF? Both signals must agree — `.pdf`
/// extension AND the `%PDF` magic — so a stray directory named `x.pdf` or a
/// mislabelled Gerber never silently reroutes into the pixel-diff path (#63;
/// same rule as the CLI's `is_pdf_input`).
#[cfg(not(target_arch = "wasm32"))]
fn is_pdf_input(path: &std::path::Path) -> bool {
    let ext_is_pdf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !ext_is_pdf || !path.is_file() {
        return false;
    }
    let mut magic = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut magic).is_ok() && &magic == b"%PDF"
        }
        Err(_) => false,
    }
}

/// Native: read one schematic PDF, enforcing the shared per-file byte cap before
/// it hits RAM (#63; same guard as the layer loaders).
#[cfg(not(target_arch = "wasm32"))]
fn read_pdf_bytes(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context as _;
    let len = path
        .metadata()
        .with_context(|| format!("reading metadata for {}", path.display()))?
        .len();
    if len > loader::MAX_LAYER_FILE_BYTES {
        anyhow::bail!(
            "{} is {len} bytes, over the {}-byte per-file limit",
            path.display(),
            loader::MAX_LAYER_FILE_BYTES
        );
    }
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Native: load one revision from a filesystem path — a schematic `.pdf` (#63),
/// or a Gerber directory / `.zip` fab pack. Shared by the CLI-args seed and the
/// Open file dialog so both route PDFs identically.
#[cfg(not(target_arch = "wasm32"))]
fn load_source_any(path: &std::path::Path, label: String) -> anyhow::Result<LoadedSource> {
    if is_pdf_input(path) {
        let bytes = read_pdf_bytes(path)?;
        return Ok(LoadedSource::Pdf(LoadedPdf { label, bytes }));
    }
    let (board, fmt) = loader::load_source(path)?;
    Ok(LoadedSource::Board(LoadedBoard { label, board, fmt }))
}

/// Native: turn dropped paths into a loaded source. A single dropped folder,
/// `.zip`, or schematic `.pdf` (#63) loads directly; multiple dropped files are
/// read as individual layers (a PDF among them is a loud error via
/// `files_to_source` — one PDF per side).
#[cfg(not(target_arch = "wasm32"))]
fn load_dropped_paths(paths: &[std::path::PathBuf]) -> anyhow::Result<LoadedSource> {
    use anyhow::Context as _;
    if paths.len() == 1 {
        let p = &paths[0];
        let is_zip = p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false);
        if p.is_dir() || is_zip || is_pdf_input(p) {
            return load_source_any(p, path_label(p));
        }
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    for p in paths {
        let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        files.push((path_label(p), bytes));
    }
    files_to_source(files, &format!("{} files", paths.len()))
}

impl ViewApp {
    /// The top-bar brand mark's texture (#191): decode the embedded brand-icon
    /// PNG and upload it once, on first use; every later frame returns the cached
    /// handle. The expect is safe — the PNG is compiled in, so a decode failure
    /// is a build defect, not a runtime condition.
    fn brand_texture(&mut self, ctx: &egui::Context) -> &egui::TextureHandle {
        self.brand_tex.get_or_insert_with(|| {
            let img = image::load_from_memory(BRAND_ICON_PNG)
                .expect("embedded brand icon PNG decodes")
                .to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            ctx.load_texture("etchy-brand-icon", pixels, egui::TextureOptions::LINEAR)
        })
    }

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
        // or fire a shortcut (#52, fix #4).
        let typing = ui.ctx().egui_wants_keyboard_input();
        // Rebind capture (#201): while armed, the next key press (with its
        // modifiers) becomes the action's binding and Esc cancels. `capturing`
        // is sampled BEFORE the capture resolves so the captured press never
        // also fires as a shortcut on the same frame.
        let capturing = self.capture_action.is_some();
        // A capture only lives while its editor row is on screen: switching rail
        // tabs or collapsing the panel/section cancels it, so a stale capture can
        // never silently eat — and rebind on — the next key press. The flag is set
        // by last frame's Hotkeys draw (one frame of lag, harmless).
        let editor_visible = std::mem::take(&mut self.hotkeys_drawn)
            && self.active_panel == Some(PanelTab::Settings);
        if capturing && !editor_visible {
            self.capture_action = None;
            self.capture_conflict = None;
        } else if let Some(action) = self.capture_action {
            // While a text field has focus a keystroke is text, not a binding —
            // leave the capture armed and let the field keep the input.
            let captured = if typing {
                None
            } else {
                ui.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => Some(capture_key(
                            *key,
                            modifiers.command,
                            modifiers.shift,
                            modifiers.alt,
                        )),
                        _ => None,
                    })
                })
            };
            match captured {
                Some(CaptureResult::Bind(b)) => {
                    // Refuse a binding something else already owns — one key
                    // press must never dispatch two actions.
                    if let Some(owner) = binding_conflict(&self.keymap, action, b) {
                        self.capture_conflict =
                            Some(format!("{} is taken by {owner}", format_binding(b)));
                    } else {
                        self.keymap.set(action, b);
                        self.capture_action = None;
                        self.capture_conflict = None;
                    }
                }
                Some(CaptureResult::Cancel) => {
                    self.capture_action = None;
                    self.capture_conflict = None;
                }
                None => {}
            }
        }
        let suppressed = typing || capturing;
        let km = self.keymap;
        // The effective cycle-units binding (#211): an explicit rebind, else the
        // preset default (Altium Q / KiCad Ctrl+U — the tools' own units keys).
        let unit_kb = km.unit_binding(self.input_preset);
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
            shift_c,
            custom_clear,
        ) = ui.input(|i| {
            use egui::Key;
            if suppressed {
                return (
                    false, false, false, false, false, false, false, false, false, false, false,
                    false, false, false, false,
                );
            }
            // A rebindable action fires on its keymap binding (#201) — the key
            // plus its exact modifier set, `ctrl` matching egui's `command`
            // (Ctrl, or Cmd on mac).
            let b = |kb: KeyBinding| {
                i.key_pressed(kb.key)
                    && i.modifiers.command == kb.ctrl
                    && i.modifiers.shift == kb.shift
                    && i.modifiers.alt == kb.alt
            };
            (
                b(km.cycle_base),
                b(km.fit_view),
                // Mode hotkeys (#55, #61): 1=Overlay 2=Old 3=New 4=Split 5=Swipe
                // via the keymap, + fixed aliases O/B/A (B/A kept as legacy
                // Old/New mnemonics). Aliases require a BARE key press — like
                // the keymap's exact-modifier match — so a rebound combo that
                // happens to contain one of these letters can't double-fire.
                b(km.mode_overlay) || (i.key_pressed(Key::O) && i.modifiers.is_none()),
                b(km.mode_old) || (i.key_pressed(Key::B) && i.modifiers.is_none()),
                b(km.mode_new) || (i.key_pressed(Key::A) && i.modifiers.is_none()),
                b(km.mode_split),
                b(km.mode_swipe),
                (i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::J)) && i.modifiers.is_none(),
                (i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::K)) && i.modifiers.is_none(),
                i.key_pressed(Key::Escape),
                b(unit_kb),
                b(km.toggle_grid),
                // Default Ctrl+M (Cmd on mac) arms/disarms the measure tool
                // (#197); rebindable via Settings > Hotkeys (#201).
                b(km.toggle_measure),
                // Shift+C: clear-measurements under the Altium preset (#198);
                // stands down once a custom clear binding exists (#201). Exact
                // Shift-only match — Ctrl/Alt+Shift+C must NOT clear the list.
                i.key_pressed(Key::C)
                    && i.modifiers.shift
                    && !i.modifiers.command
                    && !i.modifiers.alt,
                // An explicit Clear-measurements rebind (#201).
                km.clear_measure.is_some_and(b),
            )
        });
        if toggle_measure {
            // Ctrl+M arms/disarms measure mode (#50/#197), mirroring the rail
            // ruler icon (#211). Suppressed while typing via the `typing` guard.
            self.set_measure_mode(!self.measure_mode);
        }
        // An Esc aimed at an open menu/popup (egui doesn't consume it) must not
        // also fall through to the measure cascade — under the KiCad preset it
        // would silently wipe the completed measurements list.
        if escape && !ui.ctx().any_popup_open() {
            // Esc cascades (#50/#198): clear the in-progress point first; under
            // the KiCad preset the next Esc clears the completed list (unless a
            // custom clear binding stands it down, #201); an Esc with nothing
            // left to clear turns the measure tool off.
            match measure_key_action(
                self.input_preset,
                MeasureKey::Escape,
                self.measure_mode,
                !self.measure_pts.is_empty(),
                !self.measurements.is_empty(),
                km.clear_measure.is_some(),
            ) {
                MeasureAction::ClearPoints => self.measure_pts.clear(),
                MeasureAction::ClearList => self.measurements.clear(),
                MeasureAction::ExitTool => self.measure_mode = false,
                MeasureAction::None => {}
            }
        }
        if shift_c
            && measure_key_action(
                self.input_preset,
                MeasureKey::ShiftC,
                self.measure_mode,
                !self.measure_pts.is_empty(),
                !self.measurements.is_empty(),
                km.clear_measure.is_some(),
            ) == MeasureAction::ClearList
        {
            self.measurements.clear();
        }
        if custom_clear {
            // The user's own Clear-measurements binding (#201) — it replaces the
            // preset defaults entirely.
            self.measurements.clear();
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
            // Title row: the etchy brand icon (#191, the real mark from
            // assets/brand — replaces the painter-drawn box-E monogram), the
            // revisions, and the headline totals.
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let brand_id = self.brand_texture(ui.ctx()).id();
                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                    brand_id,
                    egui::vec2(BRAND_ICON_PT, BRAND_ICON_PT),
                )))
                // The brand mark doubles as the build stamp (#213): hovering it
                // names the version + git sha, so any running instance — native
                // or a wasm tab — can prove which build it is.
                .on_hover_text(format!(
                    "etchy v{} · build {BUILD_SHA}",
                    env!("CARGO_PKG_VERSION")
                ));
                ui.add_space(8.0);
                ui.label(
                    // ASCII "->" — egui's default font has no arrow glyph (→ renders
                    // as tofu, #30).
                    egui::RichText::new(format!("{}  ->  {}", self.old_label, self.new_label))
                        .size(15.0)
                        .color(C_CREAM),
                );
                // Totals pushed to the right. PDF mode has no layers or mm² —
                // headline the page tallies instead (#63).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(pv) = &self.pdf {
                        if pv.old_pages != pv.new_pages {
                            // Trust: an added/removed sheet must be legible in
                            // the headline, not folded into a count.
                            ui.label(
                                egui::RichText::new(format!(
                                    "old {} / new {} pages",
                                    pv.old_pages, pv.new_pages
                                ))
                                .size(14.0)
                                .color(C_COPPER),
                            );
                            ui.add_space(10.0);
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{}/{} pages changed",
                                pv.changed_count(),
                                pv.rows.len()
                            ))
                            .size(14.0)
                            .strong(),
                        );
                    } else {
                        let t = &self.diff.report.totals;
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
                    }
                });
            });
            ui.add_space(4.0);
            // Controls row (#57): ONE non-wrapping row. Left = the segmented mode
            // picker (never collapses); right = the reduced action cluster — Open ·
            // Fit · Help — which folds into a "More" menu when the window is narrow;
            // the flexible middle carries the warnings chip. The bar never wraps — it
            // collapses by width tier instead (replaces the old wrapped row, #5/#57).
            // Moved OUT of the bar: base opacity → Layers panel slider (#12/#6), noise
            // filter → Settings > Diff (#154), board edge → Layers panel (#157), Open
            // A/B → the Open menu (#160), GPU checkbox (Settings > Display), and — this
            // slice (#57) — Measure → rail ruler toggle, Export → rail Export tab,
            // Settings → the rail cog. The top bar spans the full window width (laid
            // out above the left panel), so the window width is the reliable tier
            // measure — available_width inside the nested layout doesn't reflect the
            // true bar width.
            let avail = ui.ctx().content_rect().width();
            let collapse_actions = avail < TIER_MORE_PX;
            let now = ui.ctx().input(|i| i.time);
            // Action intents, set in the (self-borrowing) closures and acted on after.
            let mut fit = false;
            let mut open_side: Option<RevSide> = None;
            // Native file pick (.zip fab pack or schematic .pdf, #63); web's one
            // picker already takes every kind.
            #[cfg(not(target_arch = "wasm32"))]
            let mut open_file_side: Option<RevSide> = None;
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
                // order is Open · Fit · Help. Measure/Export/Settings moved to the rail
                // (ruler toggle, Export tab, Settings cog — #57/#211).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The cluster shares the mode picker's segmented chrome (#200):
                    // one outlined group, flat segments inside. `segmented_frame`
                    // lays out left-to-right, so the order is written visually.
                    if collapse_actions {
                        segmented_frame(ui, |ui| {
                            ui.menu_button("More", |ui| {
                                if ui.button("Open old revision…").clicked() {
                                    open_side = Some(RevSide::Old);
                                    ui.close();
                                }
                                if ui.button("Open new revision…").clicked() {
                                    open_side = Some(RevSide::New);
                                    ui.close();
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    if ui.button("Open old file (.zip / .pdf)…").clicked() {
                                        open_file_side = Some(RevSide::Old);
                                        ui.close();
                                    }
                                    if ui.button("Open new file (.zip / .pdf)…").clicked() {
                                        open_file_side = Some(RevSide::New);
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                if ui.button("Fit view").clicked() {
                                    fit = true;
                                    ui.close();
                                }
                                ui.menu_button("Help", help_links);
                            });
                        });
                    } else {
                        segmented_frame(ui, |ui| {
                            ui.menu_button("Open", |ui| {
                                if ui.button("Old revision…").clicked() {
                                    open_side = Some(RevSide::Old);
                                    ui.close();
                                }
                                if ui.button("New revision…").clicked() {
                                    open_side = Some(RevSide::New);
                                    ui.close();
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    if ui.button("Old file (.zip / .pdf)…").clicked() {
                                        open_file_side = Some(RevSide::Old);
                                        ui.close();
                                    }
                                    if ui.button("New file (.zip / .pdf)…").clicked() {
                                        open_file_side = Some(RevSide::New);
                                        ui.close();
                                    }
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
                            if ui.button("Fit").clicked() {
                                fit = true;
                            }
                            ui.menu_button("Help", help_links);
                        });
                    }
                    // Flexible middle (left of the actions in RTL): the warnings chip
                    // (stays in-row, never reflows the canvas — #49). Export status now
                    // shows in the rail Export panel.
                    self.warnings_ui(ui, now);
                });
            });
            // Act on the collected intents (outside the closures that borrow self).
            if fit {
                self.cam.fitted = false;
            }
            if let Some(side) = open_side {
                self.open_primary(side, ui.ctx());
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(side) = open_file_side {
                self.pick_zip(side);
            }
        });

        // No board (or PDF pair, #63) loaded yet → welcome / open screen.
        // Returning here skips the layer list + canvas, which assume content.
        if self.diff.layers.is_empty() && self.pdf.is_none() {
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
        // (Feature 8) — one icon per side panel top-down, the Settings cog pinned
        // at the bottom. The brand mark lives in the top bar only (#190/#191).
        // Replaces the always-open left Layers panel; the Layers body moved
        // verbatim into `layers_panel_ui`, so this slice is layout-only.
        let rail = match self.rail_side {
            RailSide::Left => egui::Panel::left("rail"),
            RailSide::Right => egui::Panel::right("rail"),
        };
        rail.exact_size(48.0)
            .resizable(false)
            .show_inside(ui, |ui| {
                // Settings cog pinned to the bottom of the rail. It drives the
                // same docked panel as the tabs above (#199): click opens the
                // Settings panel, click again collapses it.
                egui::Panel::bottom("rail_settings")
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(4.0);
                        ui.vertical_centered(|ui| {
                            if rail_button(
                                ui,
                                self.active_panel == Some(PanelTab::Settings),
                                draw_cog_icon,
                            )
                            .on_hover_text("Settings")
                            .clicked()
                            {
                                self.active_panel =
                                    toggle_panel(self.active_panel, PanelTab::Settings);
                            }
                        });
                        ui.add_space(4.0);
                    });
                // Panel tabs fill the rest, top-down (no monogram — #190). The
                // measure ruler sits between them but is NOT a tab (#211): it's a
                // plain tool toggle — armed = highlighted — and opens no panel.
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        for tab in PanelTab::ALL {
                            let active = self.active_panel == Some(tab);
                            if rail_button(ui, active, |p, r, c| draw_panel_icon(tab, p, r, c))
                                .on_hover_text(tab.label())
                                .clicked()
                            {
                                self.active_panel = toggle_panel(self.active_panel, tab);
                            }
                            ui.add_space(2.0);
                            if tab == PanelTab::Layers {
                                // Measure tool toggle (#211), in the old tab slot
                                // between Layers and Export. Its options live in
                                // Settings › Measure; rulers persist on canvas.
                                let hover = format!(
                                    "Measure — arm/disarm the tool ({})",
                                    format_binding(self.keymap.toggle_measure)
                                );
                                if rail_button(ui, self.measure_mode, draw_measure_icon)
                                    .on_hover_text(hover)
                                    .clicked()
                                {
                                    self.set_measure_mode(!self.measure_mode);
                                }
                                ui.add_space(2.0);
                            }
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
                    // In PDF mode (#63) the Layers tab lists the document's pages
                    // and Export offers the per-page overlay PNGs; the board-only
                    // controls (view presets, base slider, eyes, swatches) don't
                    // apply to raster pages, so they're not shown at all.
                    PanelTab::Layers if self.pdf.is_some() => self.pdf_pages_panel_ui(ui),
                    PanelTab::Export if self.pdf.is_some() => self.pdf_export_panel_ui(ui),
                    PanelTab::Layers => self.layers_panel_ui(ui),
                    PanelTab::Export => self.export_panel_ui(ui),
                    PanelTab::Settings => self.settings_panel_ui(ui),
                });
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.pdf.is_some() {
                self.draw_canvas_pdf(ui);
            } else {
                self.draw_canvas(ui);
            }
        });

        // Publish "what they're looking at" for the web feedback widget.
        let layer_name = match &self.pdf {
            Some(pv) => format!("page {}", pv.rows[pv.selected].page),
            None => self.diff.layers[self.selected].name().to_string(),
        };
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
        // Focus (#224, owner-locked): the ONE emphasis control, replacing the
        // deleted view segment (single/highlight/all/none). Non-selected VISIBLE
        // layers render at 1 - focus — base and diff alike; 0% = all layers
        // equal, 100% = only the selected layer visible. The per-row/group eyes
        // are the ONLY visibility control; selection stays separate from both.
        // The base-opacity slider moved to Settings > Diff (`S` still cycles it).
        // "Show changed" stays hidden per feedback #8 — the capability lives on in
        // `visible_from_changed` (still unit-tested) so it can be re-surfaced later.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("focus").weak().small());
            ui.add(
                egui::Slider::new(&mut self.focus, 0.0..=1.0)
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
            .on_hover_text(
                "How strongly the selected layer stands out: other visible layers \
                 fade by this amount (100% shows only the selected layer).",
            );
        });
        ui.separator();
        // Panel rows ghost to mirror the canvas (#224): a visible non-selected
        // row's name and Δ% fade with the same focus dim (floored so rows stay
        // legible and clickable); the selected row keeps its full highlight.
        let row_ghost = focus_alpha(self.focus, false).max(ROW_GHOST_FLOOR);
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
                                    // Visible non-selected rows GHOST with the
                                    // canvas focus dim (#224) so the panel
                                    // mirrors what's drawn.
                                    let ghosted = visible && idx != self.selected;
                                    let label = match (visible, changed) {
                                        (true, true) if ghosted => {
                                            egui::RichText::new(&name).strong().color(
                                                ui.visuals()
                                                    .strong_text_color()
                                                    .gamma_multiply(row_ghost),
                                            )
                                        }
                                        (true, false) if ghosted => egui::RichText::new(&name)
                                            .color(
                                                ui.visuals().text_color().gamma_multiply(row_ghost),
                                            ),
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
                                                // magnitude reads at a glance
                                                // (#20); ghosted with the row
                                                // under focus (#224).
                                                let delta_col = if ghosted {
                                                    C_COPPER.gamma_multiply(row_ghost)
                                                } else {
                                                    C_COPPER
                                                };
                                                ui.label(
                                                    egui::RichText::new(txt)
                                                        .small()
                                                        .color(delta_col),
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

    /// Arm/disarm the measure tool (#211): the rail ruler button and the
    /// toggle-measure hotkey (default Ctrl+M) both route here. Disarming drops
    /// the in-progress point; COMPLETED measurements stay drawn on the canvas
    /// until cleared by key (MEAS-4) — there is no list UI to manage them.
    fn set_measure_mode(&mut self, on: bool) {
        self.measure_mode = on;
        if !on {
            self.measure_pts.clear();
        }
    }

    /// Export tab (#60): surfaces the existing export (per-layer SVG + copper-area
    /// CSV) in the panel. Previews *what* each action writes, then runs the same
    /// `do_export` the top-bar Export menu calls — both drive one code path.
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

        let mut exp_current = false;
        let mut exp_all = false;

        // One panel-level scroll (#196): the file lists render at full height and
        // the whole tab overflows here, instead of nested per-list scroll boxes.
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Export");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Write the diff to files you can open outside etchy.")
                    .weak()
                    .small(),
            );

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
                    Self::export_file_list(ui, &current_files);
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
            Self::export_file_list(ui, &all_files);
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
        });

        // Run after the render above — `do_export` needs `&mut self`.
        if exp_current {
            self.do_export(false);
        }
        if exp_all {
            self.do_export(true);
        }
    }

    /// Render an export file-name preview list at full height; the tab's
    /// panel-level ScrollArea handles overflow (#196).
    fn export_file_list(ui: &mut egui::Ui, files: &[String]) {
        for f in files {
            ui.label(egui::RichText::new(f).monospace().small().weak());
        }
    }

    /// A copper section heading for the Settings panes (#121).
    fn settings_header(ui: &mut egui::Ui, text: &str) {
        ui.add_space(1.0);
        ui.label(egui::RichText::new(text).color(C_COPPER).strong());
        ui.add_space(3.0);
    }

    /// The Settings panel body (#199): the rail's bottom gear docks this like any
    /// other tab (the old floating Settings window is gone). The six sections plus
    /// Hotkeys (#201) stack as collapsible headers — Display open by default — in
    /// one panel-level scroll.
    fn settings_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(4.0);
        // Copper selection accent (#121): theme/units/preset toggles read
        // brand-copper, not the default blue. Child uis inherit it.
        ui.visuals_mut().selection.bg_fill = C_COPPER.gamma_multiply(0.30);
        ui.visuals_mut().selection.stroke = egui::Stroke::new(1.0, C_COPPER);
        // auto_shrink off (#212): the scroll area always fills the panel width,
        // so the panel keeps its user-dragged size instead of re-fitting itself
        // to the widest visible row every time a section opens or closes (which
        // also fought the resize handle — the drag was undone the next frame).
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::CollapsingHeader::new("Display")
                    .default_open(true)
                    .show(ui, |ui| self.settings_display(ui));
                egui::CollapsingHeader::new("Diff").show(ui, |ui| self.settings_diff(ui));
                egui::CollapsingHeader::new("Grid").show(ui, |ui| self.settings_grid(ui));
                egui::CollapsingHeader::new("Measure").show(ui, |ui| self.settings_measure(ui));
                egui::CollapsingHeader::new("Input").show(ui, |ui| self.settings_input(ui));
                egui::CollapsingHeader::new("Colours").show(ui, |ui| self.settings_colours(ui));
                // Per-layer base colours are a board concept — raster PDF pages
                // have no layers, so the section is absent in PDF mode (#63).
                if self.pdf.is_none() {
                    egui::CollapsingHeader::new("Layers").show(ui, |ui| self.settings_layers(ui));
                }
                egui::CollapsingHeader::new("Hotkeys").show(ui, |ui| self.settings_hotkeys(ui));
            });
    }

    /// Settings → Measure (#211): the measure tool's options, moved here from
    /// the removed Measure tab. ONE home for snap/crosshair/units — the copies
    /// that used to sit in the Grid section and the Display units row are
    /// consolidated here, not duplicated.
    fn settings_measure(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Units");
            segmented(
                ui,
                &mut self.measure_unit,
                &[(Unit::Mm, "mm"), (Unit::Mil, "mil"), (Unit::Inch, "inch")],
            );
        })
        .response
        .on_hover_text(format!(
            "Unit for measurements and the coordinate readout; {} cycles",
            format_binding(self.keymap.unit_binding(self.input_preset))
        ));
        ui.checkbox(
            &mut self.snap_grid,
            "Snap measure clicks to grid intersections",
        )
        .on_hover_text("Snap each placed point to the nearest grid intersection (#51).");
        ui.checkbox(
            &mut self.show_crosshair,
            "Cursor crosshair + coordinate readout",
        )
        .on_hover_text(
            "Show a crosshair at the cursor plus live coordinates in the \
             bottom-left chip, always (not only while measuring). Snaps to \
             the grid when snap is on.",
        );
    }

    /// Settings → Hotkeys (#201): one row per rebindable action — name, current
    /// binding, Rebind. Clicking Rebind arms a capture; the next key press (with
    /// modifiers) becomes the binding and Esc cancels.
    fn settings_hotkeys(&mut self, ui: &mut egui::Ui) {
        // Tell the dispatch the editor is on screen this frame — a rebind
        // capture cancels the moment this stops being drawn.
        self.hotkeys_drawn = true;
        ui.label(
            egui::RichText::new("Click Rebind, then press the new key. Esc cancels.")
                .weak()
                .small(),
        );
        ui.add_space(4.0);
        egui::Grid::new("hotkeys_grid")
            .num_columns(3)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for (action, label) in HotkeyAction::ALL {
                    ui.label(label);
                    if self.capture_action == Some(action) {
                        ui.label(
                            egui::RichText::new("press a key…")
                                .color(C_COPPER)
                                .monospace(),
                        );
                        if ui.small_button("Cancel").clicked() {
                            self.capture_action = None;
                            self.capture_conflict = None;
                        }
                    } else {
                        let binding = match self.keymap.get(action) {
                            Some(b) => format_binding(b),
                            // Preset-driven defaults until explicitly rebound:
                            // clear-measurements (#198) and cycle-units (#211).
                            None if action == HotkeyAction::CycleUnit => format!(
                                "{} (preset)",
                                format_binding(preset_unit_binding(self.input_preset))
                            ),
                            None => match self.input_preset {
                                InputPreset::Altium => "Shift+C (preset)".to_string(),
                                InputPreset::KiCad => "Esc (preset)".to_string(),
                            },
                        };
                        ui.label(egui::RichText::new(binding).monospace());
                        if ui.small_button("Rebind").clicked() {
                            self.capture_action = Some(action);
                        }
                    }
                    ui.end_row();
                }
            });
        // A rejected rebind (the key is already taken) — the capture stays
        // armed so the user can try another key.
        if let Some(msg) = &self.capture_conflict {
            ui.colored_label(C_REMOVED, msg.as_str());
        }
        ui.add_space(4.0);
        if ui
            .button("Reset to defaults")
            .on_hover_text(
                "Restore every binding, including the preset-driven \
                 clear-measurements (#198) and cycle-units (#211) keys.",
            )
            .clicked()
        {
            self.keymap = Keymap::default();
            self.capture_action = None;
            self.capture_conflict = None;
        }
    }

    /// Settings → Display: theme, rail side, and (feature build) the GPU path.
    /// The selectable values render as segmented chips (#205) — the shared
    /// `segmented` chrome, selected = copper fill — so they read as controls,
    /// clearly distinct from the plain row label. Measure units moved to the
    /// Measure section (#211) so the tool's options have one home.
    fn settings_display(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Theme");
            segmented(
                ui,
                &mut self.theme,
                &[(Theme::Dark, "dark"), (Theme::Light, "light")],
            );
        });
        ui.horizontal(|ui| {
            ui.label("Activity rail");
            segmented(
                ui,
                &mut self.rail_side,
                &[(RailSide::Left, "left"), (RailSide::Right, "right")],
            );
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

    /// Settings → Diff: the base-copper opacity (moved here from the Layers
    /// panel, #224 — the Focus slider took its spot), the noise-filter threshold
    /// (moved off the top bar, #154), and the PDF rasterization DPI (#223).
    fn settings_diff(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Base opacity")
                .small()
                .color(Color32::from_gray(150)),
        );
        // Base opacity (#12/#6): the unchanged base copper's strength, continuous
        // 0..=1. 0 hides the base; the old off/faint/strong stops are 0%/40%/80%.
        // `S` still steps those three stops.
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
        .on_hover_text(format!(
            "Opacity of the unchanged base copper behind the diff (0 hides it); \
             {} cycles off/faint/strong.",
            format_binding(self.keymap.cycle_base)
        ));
        ui.add_space(6.0);
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
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("PDF resolution")
                .small()
                .color(Color32::from_gray(150)),
        );
        // Rasterization DPI for schematic-PDF pairs (#223). A click with a PDF
        // pair loaded re-rasterizes from the retained bytes; over-cap fails loud
        // and reverts (see `set_pdf_dpi`). The CLI default stays 150 (--dpi).
        let mut pick: Option<f32> = None;
        ui.horizontal(|ui| {
            ui.label("DPI");
            segmented_frame(ui, |ui| {
                for dpi in PDF_DPI_CHOICES {
                    let on = self.pdf_dpi == dpi;
                    let label = format!("{dpi:.0}");
                    let text = if on {
                        egui::RichText::new(label).color(C_CANVAS).strong()
                    } else {
                        egui::RichText::new(label)
                    };
                    if ui.selectable_label(on, text).clicked() && !on {
                        pick = Some(dpi);
                    }
                }
            });
        })
        .response
        .on_hover_text(
            "Rasterization DPI for schematic-PDF diffs. Changing it re-renders \
             the loaded pair; a DPI over the raster caps fails loud and reverts.",
        );
        if let Some(dpi) = pick {
            self.set_pdf_dpi(dpi);
        }
    }

    /// Settings → Grid: the reference grid overlay. Snap + crosshair moved to
    /// the Measure section (#211) — one home, no duplicate toggles; snapping
    /// still uses the spacing configured here.
    fn settings_grid(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.show_grid,
            format!(
                "Show reference grid ({})",
                format_binding(self.keymap.toggle_grid)
            ),
        );
        ui.horizontal(|ui| {
            ui.label("Spacing");
            ui.add(
                egui::DragValue::new(&mut self.grid_mm)
                    .speed(0.1)
                    .range(0.01..=100.0)
                    .suffix(" mm"),
            );
        })
        .response
        .on_hover_text("Grid pitch; measure snapping (Settings › Measure) uses this spacing.");
    }

    /// Settings → Input: pan/zoom scheme matching the user's ECAD tool (#54).
    /// Values are segmented chips, distinct from the row label (#205).
    fn settings_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("ECAD preset");
            segmented(
                ui,
                &mut self.input_preset,
                &[
                    (InputPreset::Altium, "Altium"),
                    (InputPreset::KiCad, "KiCad"),
                ],
            );
        })
        .response
        .on_hover_text(
            "Pan mouse button by ECAD tool: \
             Altium = right-drag, KiCad = middle/right-drag.",
        );
    }

    /// Settings → Colours: diff colours + per-theme canvas/grid (#53/#31/#52).
    fn settings_colours(&mut self, ui: &mut egui::Ui) {
        // The added/removed diff colours only drive the vector board render —
        // the PDF overlay bakes its colours in the engine, so showing these in
        // PDF mode would be dead controls (#63 review). Canvas & grid below
        // still apply everywhere.
        if self.pdf.is_none() {
            Self::settings_header(ui, "Diff colours");
            // One-click palette presets (#155); the pickers below still fine-tune.
            // Rendered as segmented chips (#205) — "selected" is derived from the
            // current colours, so this uses the frame directly, not `segmented`.
            ui.horizontal(|ui| {
                ui.label("Preset");
                segmented_frame(ui, |ui| {
                    for (pal, label) in DiffPalette::ALL {
                        let (a, r) = pal.colors();
                        let active = self.col_added == a && self.col_removed == r;
                        let text = if active {
                            egui::RichText::new(label).color(C_CANVAS).strong()
                        } else {
                            egui::RichText::new(label)
                        };
                        if ui.selectable_label(active, text).clicked() {
                            self.col_added = a;
                            self.col_removed = r;
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("added");
                ui.color_edit_button_srgba(&mut self.col_added);
                ui.label("removed");
                ui.color_edit_button_srgba(&mut self.col_removed);
            });
            ui.add_space(6.0);
        }
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

    /// Settings → Layers: one base-colour row per layer (#21). Rendered at full
    /// height — the Settings panel's own scroll handles overflow (#199), the same
    /// pattern as the Export tab's file lists (#196).
    fn settings_layers(&mut self, ui: &mut egui::Ui) {
        Self::settings_header(ui, "Layer base colours");
        for idx in 0..self.diff.layers.len() {
            let kind = self.diff.layers[idx].kind;
            let label = self.diff.layers[idx].name();
            ui.horizontal(|ui| {
                let mut base = resolve_base_color(idx, kind, &self.base_overrides, self.theme);
                if square_color_swatch(ui, &mut base).changed() {
                    if let Some(e) = self.base_overrides.iter_mut().find(|(i, _)| *i == idx) {
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

    /// Camera + tool input over the canvas — the swipe divider, measure clicks,
    /// drag panning, and wheel zoom/pan. Extracted from `draw_canvas` verbatim so
    /// the PDF canvas (#63) shares exactly the same feel (same divider latch, same
    /// pan buttons, same zoom anchoring). Returns whether the swipe divider is
    /// hovered or dragged, for the highlighted handle in the draw pass (#61).
    fn canvas_camera_input(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
    ) -> bool {
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
            // Hover highlight only: hover_pos is Some when the pointer is NOT pressed.
            let hovering_div = response
                .hover_pos()
                .is_some_and(|p| (p.x - div_x).abs() <= GRAB_PX);
            // LATCH the divider grab at drag START, then hold it for the whole
            // gesture. Two things have to be right (#171):
            //  - Detect the grab from press_origin (the button-DOWN point), not the
            //    drag-start point egui reports only after the drag threshold — that
            //    threshold movement could push a grab near the band edge outside the
            //    band and miss it.
            //  - Compare against the PRE-drag div_x (swipe_frac hasn't moved yet at
            //    drag-start) ONCE, then latch. div_x follows the pointer as the wipe
            //    moves, so re-checking every frame would drop the grab after GRAB_PX
            //    of travel and — since left-drag now pans — the board would pan with
            //    the wipe. The latch persists while the primary drag is held.
            if response.drag_started_by(egui::PointerButton::Primary)
                && ui
                    .input(|i| i.pointer.press_origin())
                    .is_some_and(|p| (p.x - div_x).abs() <= GRAB_PX)
            {
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
            swipe_hot = hovering_div || self.swipe_drag;
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
        } else if self.dragging_pans(response) {
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
        swipe_hot
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
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

        // Camera + tool input (swipe divider, measure clicks, panning, wheel
        // zoom), shared with the PDF canvas (#63).
        let swipe_hot = self.canvas_camera_input(ui, &response, rect);

        // Grid overlay (#51): faint world-spaced lines, drawn UNDER the geometry.
        // The drawn pitch adapts to zoom (#195) so it never fills solid or vanishes.
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
            // Split/Swipe still force the selected layer visible (a stacked
            // old|new of many layers reads as mud, and a blank half is useless).
            vec![self.selected]
        } else {
            // The eyes rule absolutely (#224): hiding every layer leaves a truly
            // blank canvas (the "no geometry in this view" hint says so) — no
            // forced fallback. Outline-only is content: the faint reference
            // draws via `outline_effective` above.
            visible_indices(&self.visible_layers)
                .into_iter()
                .filter(|&i| Some(i) != self.outline)
                .collect()
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
            // Projection target per side. Swipe is a CURTAIN over one board: both
            // halves project through the SAME full-canvas rect so the divider bisects
            // a single board (left=old, right=new) and it reads as one board with a
            // wipe — only the clip differs. Split compares the two revs whole, so each
            // half projects into its own sub-rect (a full board per side). (#171
            // follow-up: "half and half on the dividing line" at fit.)
            let (ltarget, rtarget) = if swipe { (rect, rect) } else { (lr, rr) };
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
                    Side::Left => append_tris(&mut lmesh, &item.tris, &self.cam, ltarget, base_col),
                    Side::Right => {
                        append_tris(&mut rmesh, &item.tris, &self.cam, rtarget, base_col)
                    }
                    Side::Full => {
                        append_tris(&mut lmesh, &item.tris, &self.cam, ltarget, C_OUTLINE_FAINT);
                        append_tris(&mut rmesh, &item.tris, &self.cam, rtarget, C_OUTLINE_FAINT);
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
            self.draw_split_chrome(&painter, rect, lr, rr, div_x, swipe, swipe_hot);
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
                    self.focus,
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
        // legend and the trust/context signals (now in the bottom-left chip stack
        // below) stay on-canvas, because those must remain visible. The underlying
        // counts (`self.last_hidden`, `layer.change`) are untouched.
        if self.mode == Mode::Overlay {
            legend(&painter, rect, self.col_added, self.col_removed);
        }

        // Board-specific chips for the bottom-left stack: the noise-filter trust
        // count (Overlay only — Split/Swipe/Old/New show raw boards and clear
        // `last_hidden`) and the single-of-many layer hint (#112; the board
        // outline is orientation context, so it's left out of the "shown" tally,
        // #157). The shared trailing pass adds the coordinate readout and the
        // measure chip around them.
        let mut extra_chips: Vec<String> = Vec::new();
        if self.mode == Mode::Overlay {
            if let Some(note) = hidden_note(self.last_hidden, self.min_area_mm2) {
                extra_chips.push(note);
            }
        }
        let shown = self
            .visible_layers
            .iter()
            .enumerate()
            .filter(|&(i, &v)| v && Some(i) != self.outline)
            .count();
        if let Some(hint) = single_layer_hint(shown, self.visible_layers.len()) {
            extra_chips.push(hint);
        }
        // Focus at 100% hides every non-selected layer — that suppression must
        // be accounted for on-canvas, like the old single-mode hint (#224
        // review: no silent misses, TRUST-1).
        if let Some(note) = focus_note(self.focus, shown) {
            extra_chips.push(note);
        }
        self.canvas_trailing(&painter, &response, rect, extra_chips);
    }

    /// The divider + old/new identity chrome for Split and Swipe, shared by the
    /// board and PDF canvases (#61/#63): the copper wipe line, the swipe grab
    /// handle (heavier and brighter when hovered/dragged), and the per-half
    /// bottom-left labels (#48).
    #[allow(clippy::too_many_arguments)]
    fn draw_split_chrome(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        lr: Rect,
        rr: Rect,
        div_x: f32,
        swipe: bool,
        swipe_hot: bool,
    ) {
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
            let handle = Rect::from_center_size(Pos2::new(div_x, mid_y), egui::vec2(12.0, 48.0));
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
    }

    /// The trailing canvas overlays shared by the board and PDF canvases (#63):
    /// the always-on crosshair + coordinate readout (#179), the bottom-left chip
    /// stack (#193/#194 — coordinate readout bottom-most, then the caller's
    /// mode-specific chips, then the measure how-to while the tool is armed), the
    /// persistent measure rulers (#22/#50), and the canvas border.
    fn canvas_trailing(
        &self,
        painter: &egui::Painter,
        response: &egui::Response,
        rect: Rect,
        extra_chips: Vec<String>,
    ) {
        // Always-on crosshair + coordinate readout (#179): a snapped-cursor
        // crosshair drawn regardless of measure mode (default on). Snaps to the
        // grid when snap-to-grid is on, so what the readout shows is exactly where
        // a measure click would land. In measure mode the crosshair is always
        // drawn so the ruler stays aligned even if the standalone crosshair is
        // toggled off. The coordinate readout itself no longer chases the cursor —
        // it lives in the fixed bottom-left chip stack below (#193).
        let mut coord_txt = None;
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
                // When snap is on the value is grid-quantised, so the readout says
                // "· grid" (#178) — the three decimals aren't false precision,
                // they're an on-grid point.
                let mm = etchy_core::NM_PER_MM as f64;
                coord_txt = Some(format_coord_mm(w[0] / mm, w[1] / mm, self.snap_grid));
            }
        }

        // Bottom-left chip stack (#193/#194): every trust/context chip has ONE
        // fixed home, stacked up from the corner, so nothing chases the cursor and
        // nothing is scattered across four corners. Bottom-most is the live cursor
        // readout (absent when the cursor is off-canvas — the rest slide down);
        // then the caller's mode-specific trust/context chips; then the measure
        // tool's how-to hint while the tool is armed (#22).
        let mut chips: Vec<String> = Vec::new();
        if let Some(txt) = coord_txt {
            chips.push(txt);
        }
        chips.extend(extra_chips);
        if self.measure_mode {
            // With the Measure panel gone (#211) this chip is the tool's whole
            // how-to. All three keys resolve from the live keymap/preset: the
            // units cycle (preset Q / Ctrl+U unless rebound), the list clear
            // (#198 preset unless rebound), and the toggle-off key.
            let clear_key = match self.keymap.clear_measure {
                Some(b) => format_binding(b),
                None => match self.input_preset {
                    InputPreset::Altium => "Shift+C".to_string(),
                    InputPreset::KiCad => "Esc".to_string(),
                },
            };
            chips.push(format!(
                "measure: click two points · {} units · {} clears · {} exits",
                format_binding(self.keymap.unit_binding(self.input_preset)),
                clear_key,
                format_binding(self.keymap.toggle_measure),
            ));
        }
        // Split/Swipe draw their old/new identity labels at this same corner
        // (#48) — start the chip stack above them so neither is covered.
        let label_clear = if matches!(self.mode, Mode::Split | Mode::Swipe) {
            24.0
        } else {
            0.0
        };
        let mut anchor = rect.left_bottom() + egui::vec2(8.0, -8.0 - label_clear);
        for text in &chips {
            let painted = corner_chip(painter, anchor, egui::Align2::LEFT_BOTTOM, text);
            anchor.y = painted.top() - 4.0;
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
        // reference even when the tool is disarmed (#211: the canvas is their
        // only home — cleared by the preset key / a custom clear binding).
        for m in &self.measurements {
            let (dx, dy, angle) = measure_components(m.a, m.b);
            draw_ruler(
                painter,
                w2s(m.a),
                w2s(m.b),
                &format_distance(distance_mm(m.a, m.b), self.measure_unit),
                &format_components(dx, dy, angle, self.measure_unit),
            );
        }
        if self.measure_mode {
            // In-progress: the first point of the pair (the second click completes
            // it into the list above). The how-to hint lives in the bottom-left
            // chip stack (#194).
            for w in &self.measure_pts {
                painter.circle_filled(w2s(*w), 3.0, C_COPPER);
            }
        }

        // Keep a border.
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, Color32::from_gray(60)),
            StrokeKind::Inside,
        );
    }

    /// The PDF-mode canvas (#63): one page of the pair, driven by the same mode
    /// segment, camera, measure tool, grid, and chip stack as the board canvas.
    /// Old/New show that side's raster; Overlay shows the engine's diff overlay
    /// (green added / red removed / amber changed); Split shows old|new side by
    /// side; Swipe is the curtain over one page. Pixels map to millimetres via
    /// the rasterization DPI, so Fit, the grid, and MEASURE keep working in mm.
    fn draw_canvas_pdf(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, self.canvas_color());

        // The selected page's world bbox (nm, via DPI). Fit frames the page.
        let (sel, bb, dpi) = {
            let pv = self.pdf.as_ref().expect("pdf mode");
            let row = &pv.rows[pv.selected];
            (
                pv.selected,
                pdfview::page_world_bbox(row.width, row.height, pv.dpi),
                pv.dpi,
            )
        };
        let _ = dpi;
        if !self.cam.fitted {
            fit(&mut self.cam, bb, rect);
            self.cam.fitted = true;
        }

        // Same camera/tool input pass as the board canvas: swipe divider latch,
        // measure clicks, pan buttons per input preset, wheel zoom/pan.
        let swipe_hot = self.canvas_camera_input(ui, &response, rect);

        // Grid under the page chrome (#51). The opaque page raster covers it
        // within the sheet; it still frames the page against the canvas.
        if self.show_grid {
            draw_grid(&painter, &self.cam, rect, self.grid_mm, self.grid_color());
        }

        // Textures for the selected page — uploaded once on first draw, cached
        // in the row (never re-uploaded per frame).
        let ctx = ui.ctx().clone();
        let (old_tex, new_tex, overlay_tex, presence, page_no) = {
            let pv = self.pdf.as_mut().expect("pdf mode");
            let row = &mut pv.rows[sel];
            (
                row.old_texture(&ctx),
                row.new_texture(&ctx),
                row.overlay_texture(&ctx),
                row.presence,
                row.page,
            )
        };

        // A page missing on one side has no raster there — say so, loudly.
        let missing_note = |painter: &egui::Painter, target: Rect, side: &str| {
            painter.text(
                target.center(),
                egui::Align2::CENTER_CENTER,
                format!("page {page_no} does not exist in the {side} revision"),
                egui::FontId::proportional(15.0),
                C_COPPER,
            );
        };
        let draw_side = |painter: &egui::Painter,
                         clip: Rect,
                         target: Rect,
                         tex: Option<egui::TextureId>,
                         side: &str| match tex {
            Some(id) => draw_page_image(painter, &self.cam, clip, target, bb, id),
            None => missing_note(painter, target, side),
        };

        match self.mode {
            Mode::Old => draw_side(&painter, rect, rect, old_tex, "old"),
            Mode::New => draw_side(&painter, rect, rect, new_tex, "new"),
            Mode::Overlay => match overlay_tex {
                Some(id) => draw_page_image(&painter, &self.cam, rect, rect, bb, id),
                None => {
                    // Unpaired page: there is no diff to overlay — show the side
                    // that exists, with the trust note carried by the chips below.
                    let (tex, side) = if presence == pdfview::Presence::OldOnly {
                        (old_tex, "new")
                    } else {
                        (new_tex, "old")
                    };
                    draw_side(&painter, rect, rect, tex, side);
                }
            },
            Mode::Split => {
                // Side-by-side halves, one shared camera, each projected into its
                // own sub-rect (a full page per side) — same as the board split.
                let (lr, rr, div_x) = split_rects(rect, 0.5, 6.0);
                draw_side(&painter, lr, lr, old_tex, "old");
                draw_side(&painter, rr, rr, new_tex, "new");
                self.draw_split_chrome(&painter, rect, lr, rr, div_x, false, false);
            }
            Mode::Swipe => {
                // Curtain over ONE page: both sides project through the same
                // full-canvas rect and only the clip differs, so the divider
                // bisects a single sheet (#61 pattern).
                let (lr, rr, div_x) = swipe_rects(rect, self.swipe_frac);
                draw_side(&painter, lr, rect, old_tex, "old");
                draw_side(&painter, rr, rect, new_tex, "new");
                self.draw_split_chrome(&painter, rect, lr, rr, div_x, true, swipe_hot);
            }
        }

        // Mode note (top-right), matching the board canvas' language: Old/New and
        // the raw Split/Swipe views are NOT the computed diff — say so (#91).
        let mode_note = match self.mode {
            Mode::Old => Some("showing OLD page"),
            Mode::New => Some("showing NEW page"),
            Mode::Split => Some("raw pages: OLD (left) | NEW (right) — diff applies in Overlay"),
            Mode::Swipe => Some("raw pages, swipe OLD / NEW — diff applies in Overlay"),
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
        if self.mode == Mode::Overlay {
            // The overlay raster's colours are baked by the engine (brand green /
            // red / amber) — legend those, not the user's board diff colours.
            pdf_legend(&painter, rect);
        }

        // PDF-specific chips: the page summary (trust — a page-count mismatch is
        // always visible) and the selected page's own presence tag.
        let mut extra_chips: Vec<String> = Vec::new();
        {
            let pv = self.pdf.as_ref().expect("pdf mode");
            extra_chips.push(format!(
                "page {page_no} · {summary}",
                summary = pv.summary()
            ));
        }
        if let Some(tag) = presence.tag() {
            extra_chips.push(format!("page {page_no} is {tag}"));
        }
        self.canvas_trailing(&painter, &response, rect, extra_chips);
    }

    /// The Pages panel (#63): PDF mode's stand-in for the Layers panel. One row
    /// per page, changed-first, selectable like layers; pages that exist on only
    /// one side are tagged explicitly (trust). The board-only controls (view
    /// presets, base slider, eyes, colour swatches) don't apply to raster pages
    /// and are absent, not disabled.
    fn pdf_pages_panel_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Pages");
        let Some(pv) = &self.pdf else { return };
        ui.label(
            egui::RichText::new(format!("{} · rendered at {} DPI", pv.summary(), pv.dpi))
                .weak()
                .small(),
        );
        ui.separator();
        let order = pv.order.clone();
        let mut select: Option<usize> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let pv = self.pdf.as_ref().expect("pdf mode");
            for idx in order {
                let row = &pv.rows[idx];
                let name = format!("page {}", row.page);
                let label = if row.changed {
                    egui::RichText::new(&name).strong()
                } else {
                    egui::RichText::new(&name)
                };
                let resp = ui
                    .horizontal(|ui| {
                        let r = ui.selectable_label(pv.selected == idx, label);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match row.presence.tag() {
                                // Old-only = a removed sheet, new-only = an added
                                // one — colour them like the diff itself.
                                Some(tag) => {
                                    let col = if row.presence == pdfview::Presence::OldOnly {
                                        C_REMOVED
                                    } else {
                                        C_ADDED
                                    };
                                    ui.label(egui::RichText::new(tag).small().color(col));
                                }
                                None if row.changed => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Δ {:.2}%",
                                            row.changed_fraction * 100.0
                                        ))
                                        .small()
                                        .color(C_COPPER),
                                    );
                                }
                                None => {}
                            }
                        });
                        r
                    })
                    .inner;
                if resp.clicked() {
                    select = Some(idx);
                }
            }
        });
        if let Some(idx) = select {
            // Route through select_pdf_page so page switches clear the rulers.
            self.select_pdf_page(idx);
        }
    }

    /// The Export tab in PDF mode (#63): per-page diff-overlay PNGs through the
    /// same exportio download/save path as the board exports. Only paired pages
    /// have an overlay; unpaired pages are named so their absence is explicit.
    fn pdf_export_panel_ui(&mut self, ui: &mut egui::Ui) {
        let (names, unpaired, dpi) = match &self.pdf {
            Some(pv) => (
                pv.rows
                    .iter()
                    .filter(|r| r.overlay_img.is_some() && r.changed)
                    .map(|r| format!("page-{}.png", r.page))
                    .collect::<Vec<_>>(),
                pv.rows
                    .iter()
                    .filter(|r| r.presence != pdfview::Presence::Both)
                    .map(|r| format!("page {} ({})", r.page, r.presence.tag().unwrap_or("")))
                    .collect::<Vec<_>>(),
                pv.dpi,
            ),
            None => return,
        };
        let status = self.export_msg.clone();
        let mut export = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Export");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "Write each changed page's diff overlay as a PNG ({dpi} DPI raster)."
                ))
                .weak()
                .small(),
            );
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(format!("Changed pages ({})", names.len()))
                    .color(C_COPPER)
                    .strong(),
            );
            ui.add_space(2.0);
            if names.is_empty() {
                ui.label(
                    egui::RichText::new("No changed pages — nothing to export.")
                        .weak()
                        .small(),
                );
            }
            Self::export_file_list(ui, &names);
            if !unpaired.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "No overlay for unpaired {}: a sheet that exists on one side \
                         only has nothing to diff against.",
                        unpaired.join(", ")
                    ))
                    .weak()
                    .small(),
                );
            }
            ui.add_space(6.0);
            if ui
                .add_enabled(!names.is_empty(), egui::Button::new("Export overlay PNGs"))
                .on_hover_text("Write a diff-overlay PNG for every changed page.")
                .clicked()
            {
                export = true;
            }
            if let Some(msg) = status {
                ui.add_space(10.0);
                ui.separator();
                ui.label(egui::RichText::new(msg).weak().small());
            }
        });
        if export {
            self.do_export_pdf();
        }
    }

    /// Encode + save the changed pages' overlay PNGs (#63), stashing the result
    /// message for the Export tab's status line (same flow as `do_export`).
    fn do_export_pdf(&mut self) {
        let Some(pv) = &self.pdf else { return };
        let files = match build_pdf_export(pv) {
            Ok(files) => files,
            Err(e) => {
                self.export_msg = Some(format!("export failed: {e}"));
                return;
            }
        };
        self.export_msg = Some(
            match exportio::save(&files, self.export_dir_hint().as_deref()) {
                Ok(msg) => msg,
                Err(e) => format!("export failed: {e}"),
            },
        );
    }
}

/// The PDF-mode export file set (#222 regression seam): one `page-N.png` per
/// CHANGED paired page — the exact list the Export tab previews. Pure over the
/// view, so a changed pair provably yields a non-empty set in tests.
fn build_pdf_export(pv: &PdfView) -> anyhow::Result<Vec<exportio::ExportFile>> {
    let mut files = Vec::new();
    for row in &pv.rows {
        let Some(img) = &row.overlay_img else {
            continue; // unpaired page: nothing to diff against, named in the UI
        };
        if !row.changed {
            continue;
        }
        files.push(exportio::ExportFile {
            name: format!("page-{}.png", row.page),
            content: pdfview::overlay_png(img)?,
        });
    }
    Ok(files)
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

/// The shared segmented-group chrome (#57/#200): the outlined rounded frame with
/// zero gap between the widgets inside, and the widgets restyled flat — no
/// per-button fill or stroke at rest, the usual soft fill on hover, and copper
/// with board-dark text while a menu inside is open (matching the selected
/// segment). Both the mode picker and the top-bar action cluster build on this,
/// so they read as one visual family.
fn segmented_frame<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .stroke(Stroke::new(1.0, ui.visuals().widgets.inactive.bg_fill))
        .corner_radius(8.0)
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let v = ui.visuals_mut();
                // Buttons in the group draw like unselected segments at rest
                // (selectable_label-flat), not like framed stand-alone buttons.
                v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                v.widgets.inactive.bg_stroke = Stroke::NONE;
                v.widgets.hovered.bg_stroke = Stroke::NONE;
                v.widgets.active.bg_stroke = Stroke::NONE;
                v.widgets.open.bg_stroke = Stroke::NONE;
                // An open menu button reads like the selected segment.
                v.widgets.open.weak_bg_fill = C_COPPER;
                v.widgets.open.fg_stroke = Stroke::new(1.0, C_CANVAS);
                // Selected segment (the mode picker's) reads brand copper.
                v.selection.bg_fill = C_COPPER;
                v.selection.stroke = Stroke::NONE;
                add(ui)
            })
            .inner
        })
        .inner
}

/// A rounded **segmented control** (#57): a pill-group of options, zero gap
/// between them, the selected one filled copper with board-dark text. Used for the
/// top-bar mode and base pickers. egui 0.34 has no built-in segmented widget.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    segmented_frame(ui, |ui| {
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
}

// Width tiers in egui POINTS (screen_rect width; ~half the CSS px at ppp 2). With
// the reduced Open/Fit/Help cluster (#57) the inline bar needs ~660 pt (down from
// ~900 when Measure/Export/Settings still lived here), so below that the actions
// fold into "More" and Open/Fit/Help never clip.
/// Below this window width (pt) the right action cluster collapses into "More".
const TIER_MORE_PX: f32 = 660.0;

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
    // The build stamp (#213): a weak one-liner so a user can prove which build
    // is running — e.g. a browser tab still serving stale wasm.
    ui.label(
        egui::RichText::new(format!("build {BUILD_SHA}"))
            .weak()
            .small(),
    )
    .on_hover_text("The git commit this build was made from.");
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

/// Draw the glyph for a panel tab's rail icon (painter marks, glyph-free). The
/// measure ruler is not a tab (#211) — the rail draws `draw_measure_icon`
/// directly for its tool toggle.
fn draw_panel_icon(tab: PanelTab, p: &egui::Painter, r: Rect, col: Color32) {
    match tab {
        PanelTab::Layers => draw_layers_icon(p, r, col),
        PanelTab::Export => draw_export_icon(p, r, col),
        PanelTab::Settings => draw_cog_icon(p, r, col),
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

/// The four corners of one rectangular gear tooth (#192): a radial quad spanning
/// radius `r0`..`r1` with half-width `half_w`, rotated to `angle` around `c`.
/// Pure geometry so it's unit-testable; `draw_cog_icon` maps it to screen points.
fn gear_tooth_quad(c: [f32; 2], angle: f32, r0: f32, r1: f32, half_w: f32) -> [[f32; 2]; 4] {
    let (s, cs) = angle.sin_cos();
    let corner = |radius: f32, side: f32| {
        [
            c[0] + cs * radius - s * side * half_w,
            c[1] + s * radius + cs * side * half_w,
        ]
    };
    [
        corner(r0, -1.0),
        corner(r1, -1.0),
        corner(r1, 1.0),
        corner(r0, 1.0),
    ]
}

/// Settings gear icon (#192): a solid annulus (a circle stroked thick enough to
/// leave the hub hole open) with eight rectangular teeth around the rim — a
/// proper gear silhouette, painter-drawn like every rail icon (font symbol
/// glyphs are tofu, #16/#30).
fn draw_cog_icon(p: &egui::Painter, r: Rect, col: Color32) {
    let c = r.center();
    let half = r.width().min(r.height()) * 0.5;
    // Ring: stroke centred at 0.52 of the radius, 0.42 thick → body 0.31..0.73
    // with an open hub hole inside.
    p.circle_stroke(c, half * 0.52, Stroke::new(half * 0.42, col));
    // Teeth: rectangular, rooted inside the ring body so they merge with it.
    for i in 0..8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        let quad = gear_tooth_quad([c.x, c.y], a, half * 0.60, half, half * 0.17);
        p.add(Shape::convex_polygon(
            quad.iter().map(|&[x, y]| egui::pos2(x, y)).collect(),
            col,
            Stroke::NONE,
        ));
    }
}

/// Per-frame: transform cached world items to screen meshes, applying colour, the
/// Focus dim (#224), the LOD fade (diff only), and the min-area cull (returns the
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
    // The Focus slider (#224): non-selected visible layers render at 1 - focus,
    // base AND diff geometry alike. Replaces the old Highlight dim and the #158
    // diff-only "All view" trick (retired — GPU surfaces carry the cost).
    focus: f32,
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
            let thickness = feature_thickness_nm(item.area_nm2, item.extent_nm);
            if region_screen_px(thickness, cam.scale) < LOD_LO_PX {
                continue;
            }
        }
        // The Focus dim (#224): the selected layer at full opacity, the other
        // visible layers at 1 - focus; layer-less items (outline) never dim.
        let dim = layer_focus_alpha(item.layer_index, selected, focus);
        if dim == 0.0 {
            // Focus at 100%: non-selected layers are fully invisible — skip
            // them outright instead of drawing alpha-0 vertices, so an
            // all-dimmed canvas honestly reports "no geometry in this view"
            // (n == 0) and no invisible work is transformed (#224 review).
            // User-chosen, like an eye off — NOT counted in `hidden`.
            continue;
        }
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

/// Draw one PDF page raster (#63): project its world bbox through the camera
/// into `target` (the same centre-based transform as `world_to_screen`) and blit
/// the texture, clipped to `clip`. Split projects each side into its own half;
/// Swipe projects both through the full canvas and differs only in clip.
fn draw_page_image(
    painter: &egui::Painter,
    cam: &Camera,
    clip: Rect,
    target: Rect,
    bb: [i64; 4],
    tex: egui::TextureId,
) {
    let sx = |wx: f64| (target.center().x as f64 + (wx - cam.center[0]) * cam.scale) as f32;
    let sy = |wy: f64| (target.center().y as f64 - (wy - cam.center[1]) * cam.scale) as f32;
    // World y-up: the page's top edge is max-y, so it maps to the smaller screen y.
    let screen = Rect::from_min_max(
        Pos2::new(sx(bb[0] as f64), sy(bb[3] as f64)),
        Pos2::new(sx(bb[2] as f64), sy(bb[1] as f64)),
    );
    painter.with_clip_rect(clip).image(
        tex,
        screen,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// The Overlay legend in PDF mode (#63): the engine bakes the overlay's colours
/// (brand green/red/amber), so the legend names those three — including "changed"
/// (recoloured ink), which the geometry diff doesn't have.
fn pdf_legend(painter: &egui::Painter, rect: Rect) {
    let mut y = rect.right_top() + egui::vec2(-150.0, 8.0);
    let rows = [
        (C_ADDED, "added"),
        (C_REMOVED, "removed"),
        (C_COPPER, "changed"),
    ];
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

/// ΔX / ΔY components (mm) and angle (degrees) of a measurement a→b (#208) — the
/// pure kernel behind the per-measurement detail line. World coords are nm with
/// y up (board coordinates), so the components convert straight to mm. The angle
/// is measured from the +X axis, counter-clockwise positive, in SIGNED degrees in
/// (-180, 180] — atan2's native range — so "up" is 90, "down" is -90 and "left"
/// is 180. Signed beats 0..180 here because it keeps the measurement's direction
/// (a→b), not just its slope.
fn measure_components(a: [f64; 2], b: [f64; 2]) -> (f64, f64, f64) {
    let mm = etchy_core::NM_PER_MM as f64;
    let dx = (b[0] - a[0]) / mm;
    let dy = (b[1] - a[1]) / mm;
    let angle_deg = dy.atan2(dx).to_degrees();
    (dx, dy, angle_deg)
}

/// The compact "dX · dY · angle" detail line shown under a measurement's distance
/// (#208) on the canvas ruler label. Reuses
/// [`format_distance`] so dX/dY carry the exact same unit formatting as the
/// primary distance.
fn format_components(dx_mm: f64, dy_mm: f64, angle_deg: f64, unit: Unit) -> String {
    format!(
        "dX {} · dY {} · {angle_deg:.1}°",
        format_distance(dx_mm, unit),
        format_distance(dy_mm, unit),
    )
}

/// A completed measurement: the two world-space endpoints of a ruler (#50). The
/// canvas draws the running list of these as persistent rulers (#211);
/// `distance_mm(a, b)` gives the length in the chosen unit.
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

/// The Focus-suppression chip (#224): at focus 100% every non-selected visible
/// layer is fully hidden — that must be accounted for on-canvas (TRUST-1), the
/// way the old single-mode "1 / N" hint accounted for its hiding. Below 100%
/// the layers are still (faintly) visible, so no chip. Pure → unit-testable.
fn focus_note(focus: f32, shown: usize) -> Option<String> {
    if focus >= 1.0 && shown > 1 {
        Some(format!(
            "focus 100% — {} other layer(s) hidden",
            shown.saturating_sub(1)
        ))
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

/// Minimum on-screen spacing (px) between drawn grid lines (#195). Below this
/// the drawn pitch steps up the 1-2-5 sequence so the grid stays readable
/// instead of vanishing or fusing into a solid fill.
const MIN_GRID_PX: f64 = 24.0;

/// Pick the drawn grid pitch for the current zoom (#195), Altium/KiCad style.
/// Starting from the snap pitch `grid_mm`, walk up a 1-2-5 (x10) sequence until
/// adjacent lines land at least `MIN_GRID_PX` apart on screen. Zoomed in the
/// base pitch already clears the bar and draws unchanged; snapping always stays
/// at `grid_mm` — only the DRAWN grid adapts. Returns `(major, minor)`: the
/// pitch to draw plus the previous 1-2-5 step as the fainter minor grid —
/// `None` when the major IS the base pitch (nothing below the snap grid is
/// honest to draw). Degenerate inputs return `(grid_mm, None)` (the caller
/// guards those).
fn display_grid_pitch(grid_mm: f64, px_per_mm: f64) -> (f64, Option<f64>) {
    if !grid_mm.is_finite() || grid_mm <= 0.0 || !px_per_mm.is_finite() || px_per_mm <= 0.0 {
        return (grid_mm, None);
    }
    let mut prev = None;
    let mut pow10 = 1.0f64;
    loop {
        for mult in [1.0, 2.0, 5.0] {
            let pitch = grid_mm * mult * pow10;
            if !pitch.is_finite() {
                // Overflow guard: never spin forever on absurd zoom — settle for
                // the last finite candidate.
                return (grid_mm * pow10, prev);
            }
            if pitch * px_per_mm >= MIN_GRID_PX {
                return (pitch, prev);
            }
            prev = Some(pitch);
        }
        pow10 *= 10.0;
    }
}

/// Draw the reference grid (#51) across the canvas. The drawn pitch adapts to
/// zoom (#195): zoomed in it is the snap grid `grid_mm` itself; zoomed out it
/// steps up a 1-2-5 sequence (see `display_grid_pitch`) so the grid never
/// vanishes, with the previous step as a fainter minor grid. Snapping is
/// untouched — it stays on `grid_mm`.
fn draw_grid(painter: &egui::Painter, cam: &Camera, rect: Rect, grid_mm: f64, grid_color: Color32) {
    if !grid_mm.is_finite() || grid_mm <= 0.0 || !cam.scale.is_finite() || cam.scale <= 0.0 {
        return;
    }
    let px_per_mm = etchy_core::NM_PER_MM as f64 * cam.scale;
    let (major_mm, minor_mm) = display_grid_pitch(grid_mm, px_per_mm);
    if let Some(minor) = minor_mm {
        // The minor sits one 1-2-5 step below the major (>= MIN_GRID_PX / 2.5 px
        // apart, so it always fits) — fainter, painted first so majors read on top.
        draw_grid_lines(painter, cam, rect, minor, grid_color.gamma_multiply(0.4));
    }
    draw_grid_lines(painter, cam, rect, major_mm, grid_color);
}

/// Paint one family of grid lines at `pitch_mm` world spacing across the canvas.
fn draw_grid_lines(
    painter: &egui::Painter,
    cam: &Camera,
    rect: Rect,
    pitch_mm: f64,
    color: Color32,
) {
    let step_nm = pitch_mm * etchy_core::NM_PER_MM as f64;
    let px_per_line = step_nm * cam.scale; // screen px between adjacent grid lines
    if !px_per_line.is_finite() || px_per_line < 6.0 {
        return; // safety net only — the adaptive pitch already cleared MIN_GRID_PX
    }
    let stroke = Stroke::new(1.0, color);
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

/// Draw the measure label as text on a filled copper chip with dark text (#50),
/// centred at `at` — legible instead of bare text over the copper line. The
/// distance is the primary line; the dX/dY/angle `detail` renders beneath it in a
/// smaller size (#208), both centred within one chip.
fn measure_label(painter: &egui::Painter, at: Pos2, text: &str, detail: &str) {
    let main = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), C_CANVAS);
    let sub = painter.layout_no_wrap(
        detail.to_owned(),
        egui::FontId::proportional(10.0),
        C_CANVAS,
    );
    let (main_size, sub_size) = (main.size(), sub.size());
    let gap = 1.0;
    let inner = egui::vec2(main_size.x.max(sub_size.x), main_size.y + gap + sub_size.y);
    let pad = egui::vec2(5.0, 3.0);
    let rect = Rect::from_center_size(at, inner + pad * 2.0);
    painter.rect_filled(rect, 3.0, C_COPPER);
    let top = rect.min.y + pad.y;
    painter.galley(
        Pos2::new(rect.center().x - main_size.x / 2.0, top),
        main,
        C_CANVAS,
    );
    painter.galley(
        Pos2::new(rect.center().x - sub_size.x / 2.0, top + main_size.y + gap),
        sub,
        C_CANVAS,
    );
}

/// Draw one complete measure ruler in screen space (#50): both endpoints, the
/// segment, and the distance `label` (+ its component `detail` line, #208) offset
/// perpendicular to the line so it never sits on top of it. Shared by the
/// completed-measurement loop so every ruler looks identical.
fn draw_ruler(painter: &egui::Painter, a: Pos2, b: Pos2, label: &str, detail: &str) {
    painter.circle_filled(a, 3.0, C_COPPER);
    painter.circle_filled(b, 3.0, C_COPPER);
    painter.line_segment([a, b], Stroke::new(1.5, C_COPPER));
    let mid = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    // The two-line chip is taller than the old single-line one — push it a
    // little further off the segment so the line stays clear.
    let off = egui::vec2(-dy / len, dx / len) * 18.0;
    measure_label(painter, mid + off, label, detail);
}

/// A small, unobtrusive status chip anchored into a canvas corner — copper text
/// on a translucent dark surface so it stays legible over any board colour while
/// reading as chrome, not diff content. `anchor`/`align` place it against a corner
/// (e.g. `LEFT_TOP` for top-left, `RIGHT_BOTTOM` for bottom-right). Returns the
/// painted rect so callers can stack further chips above it (#194).
fn corner_chip(painter: &egui::Painter, anchor: Pos2, align: egui::Align2, text: &str) -> Rect {
    let font = egui::FontId::proportional(12.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, C_COPPER);
    let pad = egui::vec2(6.0, 3.0);
    let rect = align.anchor_size(anchor, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 3.0, C_SURFACE.gamma_multiply(0.85));
    painter.galley(rect.min + pad, galley, C_COPPER);
    rect
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
        base_display_color, build_geom_key, cycle_base_opacity, derive_label, display_grid_pitch,
        distance_mm, export_file_names, finish_measurement, format_coord_mm, geom_cache_dirty,
        group_layers, hidden_note, is_version_like, layer_group, legacy_base_opacity,
        measure_click, pans_on, pick_outline_index, preset_unit_binding, region_screen_px,
        scroll_to_camera_action, short_layer_name, single_layer_hint, step_in_order, toggle_panel,
        warning_phase, CameraAction, InputPreset, LayerGroup, Measurement, Mode, PanelTab,
        RailSide, Theme, WarningPhase, BASE_OPACITY_FAINT, BASE_OPACITY_STRONG,
    };
    use etchy_core::LayerKind;

    // Minimal valid RS-274X: one 1mm circular flash at the origin (same fixture
    // as the loader tests).
    const MIN_GERBER: &[u8] = b"%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1.0*%\nD10*\nX0Y0D03*\nM02*\n";

    #[test]
    fn files_to_source_routes_a_single_pdf_to_pdf_mode() {
        use super::{files_to_source, LoadedSource};
        let src =
            files_to_source(vec![("sch.pdf".into(), b"%PDF-1.7 junk".to_vec())], "up").unwrap();
        match src {
            LoadedSource::Pdf(p) => {
                assert_eq!(p.label, "sch.pdf", "PDF keeps its file name as label");
                assert!(p.bytes.starts_with(b"%PDF"));
            }
            LoadedSource::Board(_) => panic!("a PDF must enter PDF mode, not the board path"),
        }
    }

    #[test]
    fn files_to_source_rejects_pdf_mixed_with_layers() {
        // Trust: a PDF among Gerber layers is a loud error, never a guess.
        use super::files_to_source;
        let err = match files_to_source(
            vec![
                ("sch.pdf".into(), b"%PDF-1.7".to_vec()),
                ("board-F_Cu.gtl".into(), MIN_GERBER.to_vec()),
            ],
            "up",
        ) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a PDF among layers must fail loud"),
        };
        assert!(err.contains("one schematic PDF per side"), "got: {err}");
    }

    #[test]
    fn files_to_source_rejects_a_pdf_named_file_without_pdf_content() {
        use super::files_to_source;
        let err = match files_to_source(vec![("sch.pdf".into(), b"not a pdf".to_vec())], "up") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a mislabelled .pdf must fail loud"),
        };
        assert!(err.contains("no PDF content"), "got: {err}");
    }

    #[test]
    fn files_to_source_still_loads_gerber_layers() {
        use super::{files_to_source, LoadedSource};
        let src = files_to_source(
            vec![("board-F_Cu.gtl".into(), MIN_GERBER.to_vec())],
            "uploaded",
        )
        .unwrap();
        match src {
            LoadedSource::Board(b) => {
                assert_eq!(b.label, "uploaded");
                assert_eq!(b.board.layers.len(), 1);
            }
            LoadedSource::Pdf(_) => panic!("gerber layers must stay on the board path"),
        }
    }

    #[test]
    fn compute_diff_refuses_a_mixed_pair() {
        use super::{compute_diff, LoadedBoard, LoadedPdf, LoadedSource};
        let (board, fmt) = crate::loader::board_from_bytes(vec![(
            "board-F_Cu.gtl".to_string(),
            MIN_GERBER.to_vec(),
        )])
        .unwrap();
        let gerber = LoadedSource::Board(LoadedBoard {
            label: "a".into(),
            board,
            fmt,
        });
        let pdf = LoadedSource::Pdf(LoadedPdf {
            label: "b.pdf".into(),
            bytes: b"%PDF-1.7".to_vec(),
        });
        let err = match compute_diff(&gerber, &pdf, super::PDF_DPI_DEFAULT) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a mixed PDF/Gerber pair must fail loud"),
        };
        assert!(
            err.contains("cannot compare a PDF with Gerber"),
            "got: {err}"
        );
    }

    #[test]
    fn gear_tooth_quad_is_a_radial_rectangle() {
        use super::gear_tooth_quad;
        // Angle 0 points along +x: the tooth is an axis-aligned rectangle spanning
        // x in [r0, r1], y in [-half_w, +half_w] around the centre.
        let q = gear_tooth_quad([0.0, 0.0], 0.0, 2.0, 4.0, 1.0);
        assert_eq!(q, [[2.0, -1.0], [4.0, -1.0], [4.0, 1.0], [2.0, 1.0]]);
        // Angle PI/2 points along +y: the same rectangle rotated a quarter turn.
        let q = gear_tooth_quad([0.0, 0.0], std::f32::consts::FRAC_PI_2, 2.0, 4.0, 1.0);
        for (got, want) in q
            .iter()
            .flatten()
            .zip([1.0, 2.0, 1.0, 4.0, -1.0, 4.0, -1.0, 2.0])
        {
            assert!((got - want).abs() < 1e-5, "{q:?}");
        }
        // A non-zero centre translates every corner.
        let q = gear_tooth_quad([10.0, 20.0], 0.0, 2.0, 4.0, 1.0);
        assert_eq!(q, [[12.0, 19.0], [14.0, 19.0], [14.0, 21.0], [12.0, 21.0]]);
    }

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
        // Focus 100% must announce its hiding (#224 review, TRUST-1)…
        assert_eq!(
            super::focus_note(1.0, 13).as_deref(),
            Some("focus 100% — 12 other layer(s) hidden")
        );
        // …but below 100% the others are still faintly visible (no chip), and a
        // single visible layer has nothing focus-hidden.
        assert_eq!(super::focus_note(0.99, 13), None);
        assert_eq!(super::focus_note(1.0, 1), None);
        assert_eq!(single_layer_hint(1, 2).as_deref(), Some("1 / 2 layers"));
        // Not a single-of-many situation → no hint (no clutter).
        assert_eq!(single_layer_hint(2, 13), None); // more than one shown
        assert_eq!(single_layer_hint(13, 13), None); // all shown
        assert_eq!(single_layer_hint(1, 1), None); // only one layer exists
        assert_eq!(single_layer_hint(0, 5), None); // none shown
    }

    #[test]
    fn display_grid_pitch_keeps_base_when_zoomed_in() {
        // #195: zoomed in, the drawn grid is the snap grid itself — same as today.
        assert_eq!(display_grid_pitch(1.0, 100.0).0, 1.0); // 100 px between lines
        assert_eq!(display_grid_pitch(1.0, 24.0).0, 1.0); // exactly at the threshold
        assert_eq!(display_grid_pitch(0.5, 60.0).0, 0.5);
    }

    #[test]
    fn display_grid_pitch_scales_1_2_5_when_zoomed_out() {
        // #195: zoomed out, the base pitch would be sub-24 px — walk the 1-2-5
        // sequence up from grid_mm until lines are >= 24 px apart, like
        // Altium/KiCad, instead of dropping the grid entirely.
        assert_eq!(display_grid_pitch(1.0, 20.0).0, 2.0); // 1 mm → 20 px; 2 mm → 40 px
        assert_eq!(display_grid_pitch(1.0, 10.0).0, 5.0); // 2 mm → 20 px; 5 mm → 50 px
        assert_eq!(display_grid_pitch(1.0, 1.0).0, 50.0); // ...20 mm → 20 px; 50 → 50 px
        assert_eq!(display_grid_pitch(0.5, 30.0).0, 1.0); // 0.5 mm → 15 px; 1 mm → 30 px
    }

    #[test]
    fn display_grid_pitch_survives_extreme_zoom() {
        // Deep zoom-out crosses several x10 decades and must stay finite — the
        // grid never vanishes and the loop never spins forever.
        assert_eq!(display_grid_pitch(1.0, 0.1).0, 500.0); // 200 mm → 20 px; 500 → 50
        let p = display_grid_pitch(1.0, 1e-12).0;
        assert!(p.is_finite() && p * 1e-12 >= 24.0);
        // Absurdly tiny px-per-mm still terminates with a finite pitch.
        let p = display_grid_pitch(1.0, f64::MIN_POSITIVE).0;
        assert!(p.is_finite());
        // Degenerate inputs fall back to the base pitch (caller guards them).
        assert_eq!(display_grid_pitch(0.0, 100.0), (0.0, None));
        assert_eq!(display_grid_pitch(1.0, 0.0), (1.0, None));
        assert_eq!(display_grid_pitch(1.0, f64::NAN), (1.0, None));
    }

    #[test]
    fn display_grid_pitch_reports_the_minor_step() {
        // #195: when the drawn pitch was scaled up, the previous 1-2-5 step is the
        // fainter minor grid; at the base pitch there is nothing below the snap
        // grid to show.
        assert_eq!(display_grid_pitch(1.0, 100.0), (1.0, None));
        assert_eq!(display_grid_pitch(1.0, 20.0), (2.0, Some(1.0)));
        assert_eq!(display_grid_pitch(1.0, 10.0), (5.0, Some(2.0)));
        assert_eq!(display_grid_pitch(1.0, 1.0), (50.0, Some(20.0)));
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
                    0.25,  // focus (#224)
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
    fn measure_components_axis_aligned_diagonal_and_negative() {
        use super::measure_components;
        let mm = etchy_core::NM_PER_MM as f64;
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Axis-aligned: +X is 0°, +Y (up, world y-up) is 90°.
        let (dx, dy, ang) = measure_components([0.0, 0.0], [10.0 * mm, 0.0]);
        assert!(close(dx, 10.0) && close(dy, 0.0) && close(ang, 0.0));
        let (dx, dy, ang) = measure_components([0.0, 0.0], [0.0, 5.0 * mm]);
        assert!(close(dx, 0.0) && close(dy, 5.0) && close(ang, 90.0));
        // Diagonal: equal legs → 45°.
        let (dx, dy, ang) = measure_components([1.0 * mm, 1.0 * mm], [3.0 * mm, 3.0 * mm]);
        assert!(close(dx, 2.0) && close(dy, 2.0) && close(ang, 45.0));
        // Negative directions: the angle keeps a→b's direction (signed range,
        // (-180, 180]) — left is 180, down is -90, down-left is -135.
        let (dx, dy, ang) = measure_components([10.0 * mm, 0.0], [0.0, 0.0]);
        assert!(close(dx, -10.0) && close(dy, 0.0) && close(ang, 180.0));
        let (dx, dy, ang) = measure_components([0.0, 5.0 * mm], [0.0, 0.0]);
        assert!(close(dx, 0.0) && close(dy, -5.0) && close(ang, -90.0));
        let (_, _, ang) = measure_components([0.0, 0.0], [-mm, -mm]);
        assert!(close(ang, -135.0));
        // Degenerate zero-length measurement: no NaN, angle reads 0.
        let (dx, dy, ang) = measure_components([2.0 * mm, 2.0 * mm], [2.0 * mm, 2.0 * mm]);
        assert!(close(dx, 0.0) && close(dy, 0.0) && close(ang, 0.0));
    }

    #[test]
    fn format_components_matches_distance_formatting() {
        use super::{format_components, Unit};
        // dX/dY ride format_distance, so they carry the unit's precision; the
        // angle is one decimal of signed degrees.
        assert_eq!(
            format_components(2.0, -3.0, -56.3099324, Unit::Mm),
            "dX 2.000 mm · dY -3.000 mm · -56.3°"
        );
        assert_eq!(
            format_components(25.4, 0.0, 0.0, Unit::Inch),
            "dX 1.0000 in · dY 0.0000 in · 0.0°"
        );
        assert_eq!(
            format_components(25.4, 25.4, 45.0, Unit::Mil),
            "dX 1000.0 mil · dY 1000.0 mil · 45.0°"
        );
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
    fn settings_without_focus_or_dpi_take_the_defaults_and_ignore_view_mode() {
        use super::{Settings, ViewApp, FOCUS_DEFAULT, PDF_DPI_DEFAULT};
        // A pre-#223/#224 blob has neither field — and may carry a stale
        // view-segment remnant. Both new fields default; the unknown key is
        // ignored, never an error (missing-field-safe migration).
        let old = r#"{"theme":"dark","view_mode":"highlight"}"#;
        let s: Settings = serde_json::from_str(old).expect("old blob with view_mode");
        let mut app = ViewApp::new(empty_diff(), "x".into(), "y".into());
        app.apply_settings(s);
        assert_eq!(app.focus, FOCUS_DEFAULT);
        assert_eq!(app.pdf_dpi, PDF_DPI_DEFAULT);
        // Persisted values round-trip, clamped to sane bounds.
        let s2: Settings =
            serde_json::from_str(r#"{"focus":0.6,"pdf_dpi":300.0}"#).expect("new blob");
        let mut app2 = ViewApp::new(empty_diff(), "x".into(), "y".into());
        app2.apply_settings(s2);
        assert_eq!(app2.focus, 0.6);
        assert_eq!(app2.pdf_dpi, 300.0);
        let wild: Settings =
            serde_json::from_str(r#"{"focus":7.0,"pdf_dpi":100000.0}"#).expect("wild blob");
        let mut app3 = ViewApp::new(empty_diff(), "x".into(), "y".into());
        app3.apply_settings(wild);
        assert_eq!(app3.focus, 1.0);
        assert_eq!(app3.pdf_dpi, 600.0);
    }

    /// #222 regression: a changed PDF pair yields a non-empty export set — the
    /// page overlay PNGs the Export tab writes. An unchanged pair yields none.
    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_export_file_list_is_the_changed_page_pngs() {
        let old = crate::pdfview::one_square_pdf(10, 10);
        let new = crate::pdfview::one_square_pdf(60, 60);
        let pv = crate::pdfview::build_pdf_view(&old, &new, 150.0).expect("view");
        let files = super::build_pdf_export(&pv).expect("export list");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "page-1.png");
        assert!(files[0].content.starts_with(&[0x89, b'P', b'N', b'G']));
        let same = crate::pdfview::build_pdf_view(&old, &old, 150.0).expect("view");
        assert!(super::build_pdf_export(&same).expect("list").is_empty());
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
            toggle_panel(Some(PanelTab::Layers), PanelTab::Export),
            Some(PanelTab::Export)
        );
        assert_eq!(
            toggle_panel(Some(PanelTab::Export), PanelTab::Layers),
            Some(PanelTab::Layers)
        );
        // Measure is NOT a tab (#211) — the rail's tab set is Layers + Export.
        assert_eq!(PanelTab::ALL, [PanelTab::Layers, PanelTab::Export]);
        // The rail's bottom gear drives Settings through the same semantics (#199).
        assert_eq!(
            toggle_panel(Some(PanelTab::Layers), PanelTab::Settings),
            Some(PanelTab::Settings)
        );
        assert_eq!(
            toggle_panel(Some(PanelTab::Settings), PanelTab::Settings),
            None
        );
    }

    #[test]
    fn binding_conflict_blocks_taken_and_reserved_keys() {
        use super::{binding_conflict, HotkeyAction, KeyBinding, Keymap};
        use egui::Key;
        let km = Keymap::default();
        // A key another rebindable action already owns is refused...
        assert!(binding_conflict(&km, HotkeyAction::FitView, KeyBinding::plain(Key::S)).is_some());
        // ...but re-capturing an action's own current binding is fine.
        assert!(
            binding_conflict(&km, HotkeyAction::CycleBase, KeyBinding::plain(Key::S)).is_none()
        );
        // Fixed plain-key aliases are reserved (O/B/A modes, J/K layer step)...
        for key in [Key::O, Key::B, Key::A, Key::J, Key::K] {
            assert!(
                binding_conflict(&km, HotkeyAction::FitView, KeyBinding::plain(key)).is_some(),
                "plain {key:?} is a fixed alias and must be refused"
            );
        }
        // ...but the same letters WITH a modifier are free (aliases are bare-key).
        assert!(binding_conflict(
            &km,
            HotkeyAction::FitView,
            KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::O
            },
        )
        .is_none());
        // Preset-driven clear/units keys are reserved regardless of the active
        // preset (so switching preset can't create a double-fire, #211 review):
        // Altium Shift+C, Altium Q, and KiCad Ctrl+U are ALL refused for other
        // actions while no custom binding stands them down.
        let shift_c = KeyBinding {
            ctrl: false,
            shift: true,
            alt: false,
            key: Key::C,
        };
        let q = KeyBinding::plain(Key::Q);
        let ctrl_u = KeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            key: Key::U,
        };
        for b in [shift_c, q, ctrl_u] {
            assert!(
                binding_conflict(&km, HotkeyAction::FitView, b).is_some(),
                "{b:?} is a preset default and must be refused for other actions"
            );
        }
        // Recapturing the units key for Cycle-units itself is fine, and an
        // explicit cycle-units rebind stands the preset reservation down.
        assert!(binding_conflict(&km, HotkeyAction::CycleUnit, q).is_none());
        let mut km2 = km;
        km2.set(HotkeyAction::CycleUnit, KeyBinding::plain(Key::U));
        assert!(binding_conflict(&km2, HotkeyAction::FitView, q).is_none());
        // A genuinely free key binds without complaint.
        assert!(binding_conflict(&km, HotkeyAction::FitView, KeyBinding::plain(Key::T)).is_none());
    }

    #[test]
    fn keymap_defaults_match_the_shipped_bindings() {
        use super::{HotkeyAction, KeyBinding, Keymap};
        use egui::Key;
        let km = Keymap::default();
        assert_eq!(
            km.toggle_measure,
            KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::M
            },
            "measure arms on Ctrl+M (#197)"
        );
        // Clear-measurements follows the #198 preset (Shift+C / Esc) until an
        // explicit rebind overrides it; cycle-units likewise rides the #211
        // preset defaults (Altium Q / KiCad Ctrl+U).
        assert_eq!(km.clear_measure, None);
        assert_eq!(km.cycle_unit, None);
        assert_eq!(km.fit_view, KeyBinding::plain(Key::F));
        assert_eq!(km.cycle_base, KeyBinding::plain(Key::S));
        assert_eq!(km.toggle_grid, KeyBinding::plain(Key::G));
        assert_eq!(km.mode_overlay, KeyBinding::plain(Key::Num1));
        assert_eq!(km.mode_old, KeyBinding::plain(Key::Num2));
        assert_eq!(km.mode_new, KeyBinding::plain(Key::Num3));
        assert_eq!(km.mode_split, KeyBinding::plain(Key::Num4));
        assert_eq!(km.mode_swipe, KeyBinding::plain(Key::Num5));
        // get/set round-trip through the action enum, for every action.
        let mut km = km;
        for (action, _) in HotkeyAction::ALL {
            let b = KeyBinding {
                ctrl: false,
                shift: true,
                alt: false,
                key: Key::X,
            };
            km.set(action, b);
            assert_eq!(km.get(action), Some(b));
        }
    }

    #[test]
    fn format_binding_reads_like_a_shortcut() {
        use super::{format_binding, KeyBinding};
        use egui::Key;
        assert_eq!(
            format_binding(KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::M
            }),
            "Ctrl+M"
        );
        assert_eq!(
            format_binding(KeyBinding {
                ctrl: false,
                shift: true,
                alt: false,
                key: Key::C
            }),
            "Shift+C"
        );
        assert_eq!(format_binding(KeyBinding::plain(Key::S)), "S");
        assert_eq!(format_binding(KeyBinding::plain(Key::Num1)), "1");
        assert_eq!(
            format_binding(KeyBinding {
                ctrl: true,
                shift: true,
                alt: true,
                key: Key::X
            }),
            "Ctrl+Shift+Alt+X"
        );
    }

    #[test]
    fn parse_binding_round_trips_and_rejects_unknown() {
        use super::{format_binding, parse_binding, HotkeyAction, KeyBinding, Keymap};
        use egui::Key;
        // Every shipped default plus a fully-modified combo round-trips.
        let km = Keymap::default();
        let mut all: Vec<KeyBinding> = HotkeyAction::ALL
            .iter()
            .filter_map(|(a, _)| km.get(*a))
            .collect();
        all.push(KeyBinding {
            ctrl: true,
            shift: true,
            alt: true,
            key: Key::Home,
        });
        for b in all {
            assert_eq!(parse_binding(&format_binding(b)), Some(b), "{b:?}");
        }
        // Unknown key names and dangling modifiers are rejected, never guessed.
        assert_eq!(parse_binding("Bogus"), None);
        assert_eq!(parse_binding("Ctrl+"), None);
        assert_eq!(parse_binding(""), None);
    }

    #[test]
    fn keymap_serde_round_trips_and_defaults_missing_fields() {
        use super::{HotkeyAction, KeyBinding, Keymap, Settings};
        use egui::Key;
        let mut km = Keymap::default();
        km.set(
            HotkeyAction::FitView,
            KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::Home,
            },
        );
        km.set(
            HotkeyAction::ClearMeasurements,
            KeyBinding {
                ctrl: false,
                shift: true,
                alt: false,
                key: Key::Delete,
            },
        );
        let json = serde_json::to_string(&km).expect("serialize");
        let back: Keymap = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(km, back);
        // A pre-#201 config (no keymap at all) gets the defaults.
        let s: Settings = serde_json::from_str("{}").expect("deserialize empty");
        assert_eq!(s.keymap, Keymap::default());
        // A partial keymap fills the missing fields from the defaults.
        let partial: Keymap =
            serde_json::from_str(r#"{"fit_view":"Ctrl+Home"}"#).expect("deserialize partial");
        assert_eq!(
            partial.fit_view,
            KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::Home
            }
        );
        assert_eq!(partial.toggle_measure, Keymap::default().toggle_measure);
    }

    #[test]
    fn capture_key_binds_next_press_and_esc_cancels() {
        use super::{capture_key, CaptureResult, KeyBinding};
        use egui::Key;
        // Esc cancels the capture — even with modifiers held, it never binds
        // Ctrl+Esc.
        assert_eq!(
            capture_key(Key::Escape, false, false, false),
            CaptureResult::Cancel
        );
        assert_eq!(
            capture_key(Key::Escape, true, true, true),
            CaptureResult::Cancel
        );
        // Any other press becomes the binding, modifiers included.
        assert_eq!(
            capture_key(Key::M, true, false, false),
            CaptureResult::Bind(KeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                key: Key::M
            })
        );
        assert_eq!(
            capture_key(Key::X, false, true, true),
            CaptureResult::Bind(KeyBinding {
                ctrl: false,
                shift: true,
                alt: true,
                key: Key::X
            })
        );
    }

    #[test]
    fn custom_clear_binding_disables_preset_clears() {
        use super::{measure_key_action, MeasureAction, MeasureKey};
        // With an explicit Clear-measurements rebind (#201), the preset defaults
        // stand down: Shift+C under Altium no longer clears the list…
        assert_eq!(
            measure_key_action(
                InputPreset::Altium,
                MeasureKey::ShiftC,
                true,
                false,
                true,
                true
            ),
            MeasureAction::None
        );
        // …and KiCad's Esc skips the list-clear step, going straight to exit.
        assert_eq!(
            measure_key_action(
                InputPreset::KiCad,
                MeasureKey::Escape,
                true,
                false,
                true,
                true
            ),
            MeasureAction::ExitTool
        );
        // The point-clear and exit steps of the Esc cascade are untouched.
        assert_eq!(
            measure_key_action(
                InputPreset::KiCad,
                MeasureKey::Escape,
                true,
                true,
                true,
                true
            ),
            MeasureAction::ClearPoints
        );
        assert_eq!(
            measure_key_action(
                InputPreset::KiCad,
                MeasureKey::Escape,
                false,
                false,
                true,
                true
            ),
            MeasureAction::None
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
        use super::{color_to_rgba, rgba_to_color, HotkeyAction, KeyBinding, Keymap, Settings};
        use egui::{Color32, Key};
        let mut keymap = Keymap::default();
        keymap.set(
            HotkeyAction::ToggleMeasure,
            KeyBinding {
                ctrl: true,
                shift: true,
                alt: false,
                key: Key::M,
            },
        );
        let s = Settings {
            theme: Theme::Light,
            base_opacity: BASE_OPACITY_STRONG,
            base_level: None, // legacy migration field; never written, always None after a round trip
            focus: 0.65,
            pdf_dpi: 300.0,
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
            keymap,
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
        use super::{measure_key_action, MeasureAction, MeasureKey};
        // In measure mode with an in-progress measurement: first Esc clears the
        // points but stays in measure mode (#50) — same in every preset.
        for p in [InputPreset::Altium, InputPreset::KiCad] {
            assert_eq!(
                measure_key_action(p, MeasureKey::Escape, true, true, false, false),
                MeasureAction::ClearPoints
            );
        }
        // Altium, in measure mode with nothing in progress: Esc exits measure
        // mode — even when the completed list is non-empty (#198: list-clearing
        // Esc is KiCad-only; Altium uses Shift+C).
        assert_eq!(
            measure_key_action(
                InputPreset::Altium,
                MeasureKey::Escape,
                true,
                false,
                true,
                false
            ),
            MeasureAction::ExitTool
        );
        assert_eq!(
            measure_key_action(
                InputPreset::Altium,
                MeasureKey::Escape,
                true,
                false,
                false,
                false
            ),
            MeasureAction::ExitTool
        );
        // Not in measure mode, nothing to clear: Esc is a no-op.
        assert_eq!(
            measure_key_action(
                InputPreset::Altium,
                MeasureKey::Escape,
                false,
                false,
                false,
                false
            ),
            MeasureAction::None
        );
    }

    #[test]
    fn kicad_escape_clears_list_before_exiting() {
        use super::{measure_key_action, MeasureAction, MeasureKey};
        let esc = |mode, pts, list| {
            measure_key_action(
                InputPreset::KiCad,
                MeasureKey::Escape,
                mode,
                pts,
                list,
                false,
            )
        };
        // KiCad (#198): the Esc cascade gains a middle step — in-progress point
        // first, then the completed list, then exit the tool.
        assert_eq!(esc(true, true, true), MeasureAction::ClearPoints);
        assert_eq!(esc(true, false, true), MeasureAction::ClearList);
        assert_eq!(esc(true, false, false), MeasureAction::ExitTool);
        // A leftover list still clears even when the tool is disarmed.
        assert_eq!(esc(false, false, true), MeasureAction::ClearList);
        assert_eq!(esc(false, false, false), MeasureAction::None);
    }

    #[test]
    fn shift_c_clears_list_under_altium_only() {
        use super::{measure_key_action, MeasureAction, MeasureKey};
        // Altium (#198): Shift+C clears the completed list whenever it's
        // non-empty, armed or not; it never touches the in-progress point.
        for (mode, pts) in [(true, true), (true, false), (false, false)] {
            assert_eq!(
                measure_key_action(
                    InputPreset::Altium,
                    MeasureKey::ShiftC,
                    mode,
                    pts,
                    true,
                    false
                ),
                MeasureAction::ClearList
            );
        }
        // Nothing listed: no-op.
        assert_eq!(
            measure_key_action(
                InputPreset::Altium,
                MeasureKey::ShiftC,
                true,
                true,
                false,
                false
            ),
            MeasureAction::None
        );
        // KiCad doesn't bind Shift+C (it clears via Esc).
        assert_eq!(
            measure_key_action(
                InputPreset::KiCad,
                MeasureKey::ShiftC,
                true,
                false,
                true,
                false
            ),
            MeasureAction::None
        );
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
    fn preset_unit_binding_matches_each_ecad_tool() {
        use super::{format_binding, KeyBinding};
        use egui::Key;
        // Each preset defaults the units cycle to that tool's own key (#211):
        // Altium's Q, KiCad's Ctrl+U.
        assert_eq!(
            preset_unit_binding(InputPreset::Altium),
            KeyBinding::plain(Key::Q)
        );
        assert_eq!(
            format_binding(preset_unit_binding(InputPreset::KiCad)),
            "Ctrl+U"
        );
    }

    #[test]
    fn unit_binding_prefers_an_explicit_rebind_over_the_preset() {
        use super::{HotkeyAction, KeyBinding, Keymap};
        use egui::Key;
        // Default keymap: the effective binding follows the preset (#211)…
        let km = Keymap::default();
        assert_eq!(
            km.unit_binding(InputPreset::Altium),
            preset_unit_binding(InputPreset::Altium)
        );
        assert_eq!(
            km.unit_binding(InputPreset::KiCad),
            preset_unit_binding(InputPreset::KiCad)
        );
        // …and an explicit rebind overrides it under EVERY preset.
        let mut km = km;
        let custom = KeyBinding {
            ctrl: false,
            shift: true,
            alt: false,
            key: Key::U,
        };
        km.set(HotkeyAction::CycleUnit, custom);
        assert_eq!(km.unit_binding(InputPreset::Altium), custom);
        assert_eq!(km.unit_binding(InputPreset::KiCad), custom);
    }

    #[test]
    fn legacy_cycle_unit_default_yields_the_preset_default() {
        use super::Keymap;
        // A pre-#211 config persisted the fixed default `"cycle_unit":"U"`.
        // The field moved (serde rename), so the stale value is dropped and the
        // preset defaults take over instead of U being read back as an
        // explicit rebind that pins the old key forever.
        let km: Keymap = serde_json::from_str(r#"{"cycle_unit":"U"}"#).expect("deserialize");
        assert_eq!(km.cycle_unit, None);
        // The renamed field round-trips an explicit rebind.
        let mut km = Keymap::default();
        km.set(
            super::HotkeyAction::CycleUnit,
            super::KeyBinding::plain(egui::Key::U),
        );
        let json = serde_json::to_string(&km).expect("serialize");
        assert!(json.contains("cycle_units"), "{json}");
        let back: Keymap = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, km);
    }

    #[test]
    fn set_measure_mode_disarm_drops_the_point_keeps_the_rulers() {
        // The rail ruler toggle / Ctrl+M route through set_measure_mode (#211):
        // disarming clears only the in-progress point — completed rulers stay
        // drawn on the canvas (they're cleared by key, MEAS-4).
        let mut app = super::ViewApp::new(empty_diff(), "old".into(), "new".into());
        app.set_measure_mode(true);
        assert!(app.measure_mode);
        app.measure_pts.push([0.0, 0.0]);
        app.measurements.push(Measurement {
            a: [0.0, 0.0],
            b: [1.0, 0.0],
        });
        app.set_measure_mode(false);
        assert!(!app.measure_mode);
        assert!(app.measure_pts.is_empty(), "in-progress point is dropped");
        assert_eq!(app.measurements.len(), 1, "completed rulers persist");
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
    fn focus_alpha_scales_non_selected_layers_only() {
        use super::{focus_alpha, layer_focus_alpha, NO_LAYER};
        // The selected layer always draws at full strength.
        assert_eq!(focus_alpha(0.0, true), 1.0);
        assert_eq!(focus_alpha(0.7, true), 1.0);
        assert_eq!(focus_alpha(1.0, true), 1.0);
        // Non-selected layers render at 1 - focus: 0 = all equal, 1 = only the
        // selected layer visible (#224).
        assert_eq!(focus_alpha(0.0, false), 1.0);
        assert!((focus_alpha(0.25, false) - 0.75).abs() < 1e-6);
        assert_eq!(focus_alpha(1.0, false), 0.0);
        // Out-of-range focus values clamp instead of inverting the effect.
        assert_eq!(focus_alpha(2.0, false), 0.0);
        assert_eq!(focus_alpha(-1.0, false), 1.0);
        // By layer index: the selected index and layer-less items (the outline
        // reference, NO_LAYER) never dim; every other layer follows the focus.
        assert_eq!(layer_focus_alpha(0, 0, 0.8), 1.0);
        assert_eq!(layer_focus_alpha(NO_LAYER, 0, 0.8), 1.0);
        assert!((layer_focus_alpha(1, 0, 0.8) - 0.2).abs() < 1e-6);
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
