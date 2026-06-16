//! etchy-gui — native desktop viewer (egui).
//!
//! `etchy-gui <old> <new>` diffs two Gerber revision directories and shows the
//! result: a changed-first layer list and a pan/zoom canvas overlaying the diff
//! (added copper green, removed red, base muted). The geometry + diff come from
//! the pure `etchy-core` engine via `compare_detailed`; this crate only does I/O
//! and rendering. M1 scope: flash-only geometry (the engine fails loud otherwise).

mod loader;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use etchy_core::{BoardDiff, LayerView, PolygonSet, Pt};

/// WSLg's GPU OpenGL path (ZINK/Vulkan-on-GL) commonly fails to initialize, and
/// its Wayland socket can drop ("Broken pipe"). Software rendering (llvmpipe) over
/// X11 (Xwayland) is the reliable combination, so on WSL we default to it — unless
/// the user has already chosen otherwise. No effect off WSL or on a real desktop.
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
    // Prefer X11 over the flaky WSLg Wayland path when both are offered.
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
}

fn main() -> ExitCode {
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
    // Brand app-icon (assets/brand) on the window/taskbar; ignore if it can't decode.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/brand/png/etchy-app-icon-512.png"
    )) {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
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

fn label(p: &std::path::Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}

fn build_diff(old_dir: &std::path::Path, new_dir: &std::path::Path) -> anyhow::Result<BoardDiff> {
    let old = loader::load_board(old_dir)?;
    let new = loader::load_board(new_dir)?;
    Ok(etchy_core::compare_detailed(&old, &new)?)
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Overlay,
    Before,
    After,
}

/// Pan/zoom camera in world (nm) space.
struct Camera {
    center: [f64; 2], // world nm
    scale: f64,       // pixels per nm
    fitted: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            scale: 1.0,
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
    show_base: bool,
    cam: Camera,
}

impl ViewApp {
    fn new(diff: BoardDiff, old_label: String, new_label: String) -> Self {
        let mut order: Vec<usize> = (0..diff.layers.len()).collect();
        order.sort_by_key(|&i| !diff.layers[i].is_changed()); // changed first, stable
        let selected = order.first().copied().unwrap_or(0);
        Self {
            diff,
            old_label,
            new_label,
            order,
            selected,
            mode: Mode::Overlay,
            show_base: false,
            cam: Camera::default(),
        }
    }

    fn select(&mut self, idx: usize) {
        if idx != self.selected {
            self.selected = idx;
            self.cam.fitted = false; // refit on layer change
        }
    }
}

// Brand palette (assets/brand/README.md): diff accents + board-dark canvas.
const C_ADDED: Color32 = Color32::from_rgb(0x46, 0xd1, 0x8a); // #46d18a
const C_REMOVED: Color32 = Color32::from_rgb(0xff, 0x5d, 0x73); // #ff5d73
const C_BASE: Color32 = Color32::from_rgb(90, 95, 105);
/// Brand "board dark" — the canvas background.
const C_CANVAS: Color32 = Color32::from_rgb(0x0b, 0x0f, 0x0e); // #0b0f0e

/// A changed region smaller than this many screen pixels is drawn as one crisp
/// marker dot instead of its (sub-pixel, aliasing) real geometry.
const MIN_FEATURE_PX: f32 = 3.0;
/// Radius (px) of that marker dot.
const MARKER_R: f32 = 3.0;

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("etchy");
                ui.separator();
                ui.label(format!("{}  →  {}", self.old_label, self.new_label));
                ui.separator();
                let t = &self.diff.report.totals;
                ui.label(format!(
                    "{}/{} layers changed   +{:.4} mm²  −{:.4} mm²",
                    t.layers_changed, t.layers_total, t.added_area_mm2, t.removed_area_mm2
                ));
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.mode, Mode::Overlay, "Overlay");
                ui.selectable_value(&mut self.mode, Mode::Before, "Before");
                ui.selectable_value(&mut self.mode, Mode::After, "After");
                ui.separator();
                ui.checkbox(&mut self.show_base, "show base");
                if ui.button("Fit").clicked() {
                    self.cam.fitted = false;
                }
                ui.separator();
                ui.label("drag = pan · scroll = zoom");
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
                    let order = self.order.clone();
                    for idx in order {
                        let l = &self.diff.layers[idx];
                        let dot = if l.is_changed() { "●" } else { "○" };
                        let text = format!(
                            "{dot} {}\n   +{:.4} −{:.4} mm²  ({}+/{}−)",
                            l.name(),
                            l.change.added_area_mm2(),
                            l.change.removed_area_mm2(),
                            l.change.added_region_count,
                            l.change.removed_region_count,
                        );
                        if ui.selectable_label(idx == self.selected, text).clicked() {
                            self.select(idx);
                        }
                    }
                });
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.draw_canvas(ui);
        });
    }
}

impl ViewApp {
    fn draw_canvas(&mut self, ui: &mut egui::Ui) {
        let layer = &self.diff.layers[self.selected];
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, 0.0, C_CANVAS);

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
        // Zoom around the cursor.
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(ptr) = response.hover_pos() {
                let before = screen_to_world(&self.cam, ptr, rect);
                self.cam.scale *= (scroll as f64 * 0.0015).exp();
                let after = screen_to_world(&self.cam, ptr, rect);
                self.cam.center[0] += before[0] - after[0];
                self.cam.center[1] += before[1] - after[1];
            }
        }

        // Build the shapes to draw, per mode.
        let mut shapes: Vec<Shape> = Vec::new();
        let cam = &self.cam;
        // Render each shape as a filled mesh from our own concave-correct
        // triangulation. We deliberately do NOT use egui's path stroke for an
        // outline: its miter join extrudes each vertex by `normal / length_sq`,
        // which explodes at the ~0° tips that boolean-diff crescents always have,
        // flinging stroke vertices clear across the board (the "green lines"). The
        // filled mesh alone shows the shape correctly and can't spike.
        //
        // `marker` shapes (the diff) additionally collapse to a single fixed dot
        // when they'd be sub-pixel on screen, so changes stay visible (and don't
        // alias into red/green speckle) when zoomed out.
        let mut push = |set: &PolygonSet, fill: Color32, marker: bool| {
            for shape in &set.shapes {
                let Some(outer) = shape.first() else { continue };
                if outer.len() < 3 {
                    continue;
                }
                if marker {
                    let mut bb = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
                    for p in outer {
                        bb[0] = bb[0].min(p.x);
                        bb[1] = bb[1].min(p.y);
                        bb[2] = bb[2].max(p.x);
                        bb[3] = bb[3].max(p.y);
                    }
                    let px = ((bb[2] - bb[0]).max(bb[3] - bb[1]) as f64 * cam.scale) as f32;
                    if px < MIN_FEATURE_PX {
                        let center = Pt::new((bb[0] + bb[2]) / 2, (bb[1] + bb[3]) / 2);
                        let c = world_to_screen(cam, center, rect);
                        shapes.push(Shape::circle_filled(c, MARKER_R, fill));
                        continue;
                    }
                }
                let mut mesh = egui::epaint::Mesh::default();
                for tri in etchy_core::triangulate_ring(outer) {
                    let base = mesh.vertices.len() as u32;
                    for p in tri {
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos: world_to_screen(cam, p, rect),
                            uv: egui::epaint::WHITE_UV,
                            color: fill,
                        });
                    }
                    mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
                if !mesh.is_empty() {
                    shapes.push(Shape::from(mesh));
                }
            }
        };

        match self.mode {
            Mode::Before => push(&layer.old, C_BASE, false),
            Mode::After => push(&layer.new, C_BASE, false),
            Mode::Overlay => {
                if self.show_base {
                    // Draw the unchanged base (the new layer) faintly behind the diff.
                    let faint = Color32::from_rgba_unmultiplied(90, 95, 105, 90);
                    push(&layer.new, faint, false);
                }
                push(&layer.removed, C_REMOVED, true);
                push(&layer.added, C_ADDED, true);
            }
        }
        let n = shapes.len();
        painter.extend(shapes);

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

        // Per-layer caption + a tiny legend.
        let cap = format!(
            "{}  —  {}   (+{} / −{} regions)",
            layer.name(),
            status_str(layer.status),
            layer.change.added_region_count,
            layer.change.removed_region_count,
        );
        painter.text(
            rect.left_top() + egui::vec2(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            cap,
            egui::FontId::proportional(14.0),
            Color32::from_gray(200),
        );
        if self.mode == Mode::Overlay {
            legend(&painter, rect);
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

fn legend(painter: &egui::Painter, rect: Rect) {
    let mut y = rect.right_top() + egui::vec2(-150.0, 8.0);
    for (c, txt) in [(C_ADDED, "added"), (C_REMOVED, "removed")] {
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
}

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
