#![no_main]
//! Fuzz the Gerber front-end over its public entry point. The contract: for ANY
//! input bytes, `resolve_layer` returns `Ok` or a typed `EngineError` — it never
//! panics and never amplifies unboundedly (the catch_unwind boundary, #85, plus
//! the per-layer object cap, #83, enforce this).
//!
//! Run: `cargo +nightly fuzz run resolve_layer`

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = etchy_core::resolve_layer(data);
});
