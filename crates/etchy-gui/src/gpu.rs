//! GPU world→screen transform for the diff mesh (#106/#107; extended to all
//! visible layers for HDI-scale boards, #80).
//!
//! **Pullable by design.** Feature-gated (`gpu-transform`) and runtime-toggled
//! (default OFF); the CPU `transform_cache` path is always the default + fallback.
//! To remove: delete this file, its `mod gpu;`, the `gpu`/`use_gpu` fields +
//! toggle in `main.rs`, and the `gpu-transform` feature in Cargo.toml.
//!
//! All visible geometry (base + diff + outline) is uploaded **once** to a GL
//! buffer with **per-vertex colour** (baked from the same colour logic the CPU
//! path uses, minus the per-frame LOD). Each frame only updates the camera
//! uniform and redraws, so frame time is O(1) in triangle count — the fix for
//! massive/HDI boards where the CPU per-frame mesh rebuild cliffs. Re-upload
//! happens only when geometry/colour/visibility changes, never per pan frame.
//!
//! World→clip is a per-axis affine `clip = u_a * v_pos + u_b`; coefficients are
//! computed on the CPU in f64 each frame, vertices stored relative to a board
//! origin so f32 holds them without visible drift.

use std::sync::{Arc, Mutex};

use eframe::glow::{self, HasContext};
use egui::Rect;

use etchy_core::Pt;

/// One triangle plus its (already-normalized 0..1) RGBA colour.
pub type ColorTri = ([Pt; 3], [f32; 4]);

const FLOATS_PER_VERT: usize = 6; // x, y, r, g, b, a
const STRIDE: i32 = (FLOATS_PER_VERT * std::mem::size_of::<f32>()) as i32;

struct Resources {
    program: glow::Program,
    vbo: glow::Buffer,
    vao: glow::VertexArray,
    /// Vertices currently uploaded (3 per triangle).
    count: i32,
    /// World-space origin the uploaded vertices are relative to (nm).
    origin: [f64; 2],
    u_a: Option<glow::UniformLocation>,
    u_b: Option<glow::UniformLocation>,
}

/// Owns the GL program + the uploaded mesh buffer. Lives on `ViewApp` while the
/// GPU path is active; dropping it frees the GL objects.
pub struct GpuMesh {
    gl: Arc<glow::Context>,
    res: Arc<Mutex<Resources>>,
}

fn shader_header() -> &'static str {
    // GLSL ES 3.00 on the web (WebGL2), desktop GLSL 3.30 natively. `precision` is
    // required on ES and a harmless no-op on desktop; `in`/`out` exist in both.
    #[cfg(target_arch = "wasm32")]
    {
        "#version 300 es\nprecision highp float;\n"
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "#version 330\nprecision highp float;\n"
    }
}

const VERT_BODY: &str = r#"
in vec2 v_pos;            // (world - origin) in nm
in vec4 v_col;            // per-vertex RGBA (premultiplied-alpha friendly)
uniform vec2 u_a;         // clip = u_a * v_pos + u_b  (per-axis affine)
uniform vec2 u_b;
out vec4 col;
void main() {
    col = v_col;
    gl_Position = vec4(u_a * v_pos + u_b, 0.0, 1.0);
}
"#;

const FRAG_BODY: &str = r#"
in vec4 col;
out vec4 frag;
void main() { frag = col; }
"#;

impl GpuMesh {
    /// Compile the program once. Returns `None` on any failure (caller falls back
    /// to the CPU path — never a hard error).
    pub fn new(gl: Arc<glow::Context>) -> Option<Self> {
        let res = unsafe { build_program(&gl)? };
        Some(Self {
            gl,
            res: Arc::new(Mutex::new(res)),
        })
    }

    /// Upload all visible triangles + their per-vertex colour. Called when the
    /// geometry/colour/visibility changes — NOT per pan frame. Vertices are stored
    /// relative to the mesh-bbox centre so f32 holds them without drift.
    pub fn upload(&self, tris: &[ColorTri]) {
        let mut mn = [i64::MAX; 2];
        let mut mx = [i64::MIN; 2];
        for (t, _) in tris {
            for p in t {
                mn[0] = mn[0].min(p.x);
                mn[1] = mn[1].min(p.y);
                mx[0] = mx[0].max(p.x);
                mx[1] = mx[1].max(p.y);
            }
        }
        let origin = if tris.is_empty() {
            [0.0, 0.0]
        } else {
            [
                (mn[0] as f64 + mx[0] as f64) / 2.0,
                (mn[1] as f64 + mx[1] as f64) / 2.0,
            ]
        };
        let mut buf: Vec<f32> = Vec::with_capacity(tris.len() * 3 * FLOATS_PER_VERT);
        for (t, c) in tris {
            for p in t {
                buf.push((p.x as f64 - origin[0]) as f32);
                buf.push((p.y as f64 - origin[1]) as f32);
                buf.extend_from_slice(c);
            }
        }
        let mut res = self.res.lock().unwrap();
        unsafe {
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(res.vbo));
            self.gl
                .buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&buf), glow::DYNAMIC_DRAW);
            self.gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        res.count = (tris.len() * 3) as i32;
        res.origin = origin;
    }

    /// Per-frame paint callback: just sets the camera uniform and redraws the
    /// uploaded mesh. O(1) in triangle count — no CPU transform, no re-upload.
    pub fn callback(
        &self,
        rect: Rect,
        cam_scale: f64,
        cam_center: [f64; 2],
    ) -> egui::PaintCallback {
        let res = Arc::clone(&self.res);
        let cb = eframe::egui_glow::CallbackFn::new(move |_info, painter| {
            let gl = painter.gl();
            let res = res.lock().unwrap();
            if res.count == 0 {
                return;
            }
            // egui_glow sets the GL viewport to THIS callback's rect, so clip [-1,1]
            // spans the canvas rect. world→clip collapses to clip = (2*scale/size) *
            // (world - center), y flipped; origin folded into b so f32 verts stay small.
            let w = rect.width() as f64;
            let h = rect.height() as f64;
            let ax = 2.0 * cam_scale / w;
            let ay = 2.0 * cam_scale / h;
            let bx = ax * (res.origin[0] - cam_center[0]);
            let by = ay * (res.origin[1] - cam_center[1]);
            unsafe {
                gl.use_program(Some(res.program));
                gl.uniform_2_f32(res.u_a.as_ref(), ax as f32, ay as f32);
                gl.uniform_2_f32(res.u_b.as_ref(), bx as f32, by as f32);
                gl.bind_vertex_array(Some(res.vao));
                gl.enable(glow::BLEND);
                // Premultiplied alpha, matching egui's own renderer — colours are
                // uploaded premultiplied (Color32::to_array), so dim/fade composite
                // the same as the CPU path.
                gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                gl.draw_arrays(glow::TRIANGLES, 0, res.count);
                gl.bind_vertex_array(None);
            }
        });
        egui::PaintCallback {
            rect,
            callback: Arc::new(cb),
        }
    }
}

impl Drop for GpuMesh {
    fn drop(&mut self) {
        if let Ok(res) = self.res.lock() {
            unsafe {
                self.gl.delete_program(res.program);
                self.gl.delete_buffer(res.vbo);
                self.gl.delete_vertex_array(res.vao);
            }
        }
    }
}

/// Reinterpret an `f32` slice as bytes for `buffer_data` (no external bytemuck).
fn bytes_of(v: &[f32]) -> &[u8] {
    // Safe: f32 has no padding/invalid bit patterns and we only read it.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

unsafe fn build_program(gl: &glow::Context) -> Option<Resources> {
    let header = shader_header();
    let program = gl.create_program().ok()?;
    let stages = [
        (glow::VERTEX_SHADER, format!("{header}{VERT_BODY}")),
        (glow::FRAGMENT_SHADER, format!("{header}{FRAG_BODY}")),
    ];
    let mut shaders = Vec::new();
    for (kind, src) in stages {
        let sh = gl.create_shader(kind).ok()?;
        gl.shader_source(sh, &src);
        gl.compile_shader(sh);
        if !gl.get_shader_compile_status(sh) {
            eprintln!(
                "etchy gpu: shader compile failed: {}",
                gl.get_shader_info_log(sh)
            );
            return None;
        }
        gl.attach_shader(program, sh);
        shaders.push(sh);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        eprintln!(
            "etchy gpu: link failed: {}",
            gl.get_program_info_log(program)
        );
        return None;
    }
    for sh in shaders {
        gl.detach_shader(program, sh);
        gl.delete_shader(sh);
    }

    let vbo = gl.create_buffer().ok()?;
    let vao = gl.create_vertex_array().ok()?;
    gl.bind_vertex_array(Some(vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
    // Interleaved [x, y, r, g, b, a] per vertex.
    let pos = gl.get_attrib_location(program, "v_pos")?;
    gl.enable_vertex_attrib_array(pos);
    gl.vertex_attrib_pointer_f32(pos, 2, glow::FLOAT, false, STRIDE, 0);
    let col = gl.get_attrib_location(program, "v_col")?;
    gl.enable_vertex_attrib_array(col);
    gl.vertex_attrib_pointer_f32(
        col,
        4,
        glow::FLOAT,
        false,
        STRIDE,
        2 * std::mem::size_of::<f32>() as i32,
    );
    gl.bind_vertex_array(None);
    gl.bind_buffer(glow::ARRAY_BUFFER, None);

    Some(Resources {
        program,
        vbo,
        vao,
        count: 0,
        origin: [0.0, 0.0],
        u_a: gl.get_uniform_location(program, "u_a"),
        u_b: gl.get_uniform_location(program, "u_b"),
    })
}
