# Design — GPU-side geometry transform for the viewer

**Status:** proposed (awaiting the owner's review). **Author:** the owner. **Date:** 2026-06-24.

## Problem

On dense boards with many layers visible, the egui viewer is sluggish to pan and
zoom. Field feedback on the real board (rev A→rev B, 24+ layers, ~hundreds of
thousands of triangles):

- "Noticeably slower. Might be the larger layers?"
- "Turning off all the other layers speeds up the app a lot."

An adaptive-LOD pass (cull sub-pixel base copper when zoomed out) shipped in #74 and
**did not help at working zoom** — confirmed by the owner ("no real change"). That is
expected: LOD only removes features that are already sub-pixel, which only happens
when zoomed far out. At normal working zoom every feature is full size, so nothing is
culled and the full triangle set is processed each frame.

### Root cause

`transform_cache` (crates/etchy-gui/src/main.rs) runs **every frame**, on the CPU,
over every visible cached item:

1. world→screen transform of every triangle vertex (`world_to_screen` per vertex),
2. building a fresh `egui::epaint::Mesh` (vertex buffer) each frame,
3. egui then uploads that mesh to the GPU.

Cost scales with `visible layers × triangles per layer`, **independent of zoom**. That
is exactly the "turning off layers speeds it up" observation. The world-space
tessellation cache (`TessCache`) already avoids re-*triangulating* per frame, but the
per-frame **transform + mesh rebuild + re-upload** is the remaining bottleneck.

## Goal

Smooth pan/zoom on dense multi-layer boards (target: 60 fps on the real board with all
layers visible), with **no visual change** versus the current CPU renderer.

### Non-goals

- No change to the diff math, layer model, or any non-rendering behaviour.
- No new external crate (the backend is `glow`, already pulled in by eframe).
- Not a general scene-graph or a switch to wgpu. Keep the egui/glow integration.

## Approaches considered

| Approach | Idea | Win | Effort | Risk |
|---|---|---|---|---|
| **A. GPU-side transform** (chosen) | Upload world-space geometry to a GPU buffer once; each frame the vertex shader applies the camera transform. Per-frame CPU cost → ~0. | High — fixes pan/zoom at any layer count | Large | Med-high (custom GL, web compat, precision) |
| B. Geometry decimation | Permanently simplify base-copper triangle count. | Medium, proportional | Medium | Fidelity loss; lower ceiling — still CPU-transforms what remains |
| C. Mesh cache + repaint gating | Cache the built `Vec<Shape>`; rebuild only on camera/visibility change. | Helps idle only, **not** active pan/zoom | Small | Low; doesn't address the actual complaint |

Decimation and mesh-caching don't fix the interactive case (the camera changes every
frame during pan/zoom, so the transform must rerun). Only moving the transform to the
GPU removes the per-frame CPU cost. Hence **Approach A**.

## Chosen design — hybrid GPU base / CPU diff

The base/context copper is the bulk of the geometry, has a simple per-vertex colour,
and does **not** need zoom-dependent treatment. The diff items (added/removed) are far
fewer but use zoom-dependent LOD (marker dots, thickness fade — #14/#46) that is
awkward to do on the GPU. So:

- **GPU**: the stacked **base copper** for the Overlay / Before / After modes — uploaded
  once per "geometry key" change, transformed in the vertex shader every frame.
- **CPU (unchanged)**: diff items (Added/Removed) with their existing LOD/marker logic,
  the board outline, and the grid/measure/crosshair overlays — drawn with the normal
  egui painter **on top of** the GPU base layer.
- **CPU (unchanged)**: Split and Swipe modes already render a **single** layer's
  old/new (cheap) — they stay on the CPU path. The GPU path is only for the
  multi-layer stacked modes, which is where the cost is.

### Rendering integration (egui `PaintCallback`, glow)

egui lets a widget inject custom GL draw commands via `egui::PaintCallback` /
`egui_glow::CallbackFn`. Plan:

1. In `draw_canvas`, before the existing CPU shapes, add a `PaintCallback` over the
   canvas rect. Its closure receives the `glow::Context`.
2. The callback binds our shader program, sets a `u_transform` uniform (the world→screen
   affine as a 3×3 / mat3 or two vec2s: scale + translate), and draws the base VBO.
3. egui then paints the CPU shapes (diff/outline/overlays) afterward, so they layer on
   top with correct z-order.

### GPU resources (a `BaseRenderer` struct, lazily created)

- **Program**: a tiny shader pair (below). Compiled once.
- **VBO + VAO**: world-space vertices (position + colour). Rebuilt only when the
  *geometry key* changes — i.e. the visible-layer set, base level, per-layer colours,
  theme, or dimming/selection. Same invalidation trigger as today's `TessCache`, so it
  piggybacks on the existing dirty check.
- Stored on the app (or in egui's paint-callback resource map) so it survives frames.

### Coordinate precision (important)

World coordinates are nanometres as `i64`; a ~130 mm board is ~1.3e8 nm, which exceeds
`f32` mantissa precision and would shimmer if uploaded raw as `f32`. Mitigation: at
upload time subtract a **local origin** (board centre, an `i64`) from every vertex, so
uploaded `f32` values are small and precise. The camera uniform then maps
`local-space → screen`, with the origin folded into the camera translate (computed in
`f64` on the CPU, passed as `f32` offsets). Unit-testable as a pure function.

### Shaders (GLSL, glow handles native `#version 330` / web `#version 300 es`)

- **Vertex**: `screen_pos = u_scale * (a_pos) + u_translate;` then to clip space via the
  egui screen-size uniform (matching egui's own vertex shader conventions). Pass through
  `a_color`.
- **Fragment**: `out = v_color;` (straight alpha-blended, premultiplied to match egui).

### Colour, dimming, base level

These change infrequently (on selection / control change, not per frame), so they are
**baked into the per-vertex colour at upload time** — the same values
`base_display_color` / the #59 dim factor compute today. A selection change re-bakes the
VBO (cheap, infrequent). This keeps the shader trivial and the per-frame path pure
transform.

## What changes / files

- `crates/etchy-gui/src/main.rs` — new `BaseRenderer` (glow program/VBO), a
  `PaintCallback` in `draw_canvas` for the stacked modes, and a split of
  `transform_cache` so base items feed the GPU upload while diff/outline/overlays stay
  on the CPU painter. New pure helpers (local-origin rebasing, camera→uniform) with unit
  tests.
- `crates/etchy-gui/Cargo.toml` — add `egui_glow` (part of the egui workspace; provides
  `CallbackFn`). Confirm it is permissive-licensed and already in the lock; run
  `cargo deny check` either way. No other new deps.
- No changes to `etchy-core`.

## Native + web

`glow` is the eframe default renderer on both surfaces; `PaintCallback` +
`egui_glow::CallbackFn` work native (OpenGL) and web (WebGL2). The shader targets
WebGL2 (`#version 300 es`). A board that fails GPU init (no WebGL2) **falls back to the
current CPU path** — see below.

## Fallback & safety

- Keep the CPU transform path intact. If the `BaseRenderer` fails to initialise
  (shader/program/WebGL2 error), log once and render base copper on the CPU as today.
  No board should ever fail to render because of this optimisation. (Trust bar: no
  silent misses — the fallback is loud in logs and visually identical.)

## Testing & verification

- **Visual parity**: headless screenshots (Playwright, `shot.py`) of the GPU path vs the
  CPU path on the real board and the public Mad_RP2040 — must be pixel-equivalent
  (overlay, before, after; dark + light; dimming on/off).
- **Perf**: frame-time / fps measurement on the real board, all layers visible, during a
  scripted pan/zoom — before vs after. Target ≥60 fps (or a clear multiple of current).
- **Unit tests**: the pure coordinate/uniform helpers (local-origin rebasing,
  camera→uniform, precision bound).
- **Fallback**: force-fail GPU init and confirm the CPU path still renders.

## Rollout

1. **Spike** (timeboxed): GPU-transform base copper for Overlay mode only on the real
   board; measure the fps win to confirm the approach pays off before building it out.
2. If the spike wins: complete Before/After, dimming/colour baking, precision handling,
   fallback, parity + perf tests.
3. Ship behind the existing modes (no user-facing flag needed if parity holds);
   document in DEVELOPER_GUIDE.

## Open decisions (for the owner)

1. **Dimming/colour**: bake per-vertex at upload (chosen here — simplest) vs a per-layer
   uniform + per-vertex layer id (avoids re-upload on selection change, more shader
   complexity). Recommendation: bake; re-upload on selection is infrequent.
2. **Keep the CPU path permanently** as a fallback (recommended for the trust bar), or
   remove it once the GPU path is proven?
3. **Spike-first** (recommended) vs build the full path directly.
4. Scope now: base copper only (recommended), or also move diff items to GPU later with
   LOD evaluated in the shader?

## Effort estimate

- Spike: ~0.5–1 day.
- Full implementation + parity/perf tests + fallback: ~2–4 days.
