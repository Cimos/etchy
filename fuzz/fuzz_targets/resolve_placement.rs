#![no_main]
//! Fuzz the pick-and-place front-end over its public entry point. The contract:
//! for ANY input bytes, `resolve_placement` returns `Ok` or a typed
//! `EngineError` — it never panics (#308).
//!
//! Run: `cargo +nightly fuzz run resolve_placement`

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = etchy_core::resolve_placement(data);
});
