#![no_main]
//! Fuzz the Excellon front-end over its public entry point. The contract: for
//! ANY input bytes, `resolve_excellon` returns `Ok` or a typed `EngineError` —
//! it never panics and never amplifies unboundedly (the catch_unwind boundary,
//! the shared per-layer contour/point caps, the declared-digit bound and the
//! coordinate range guard, #308, enforce this).
//!
//! Run: `cargo +nightly fuzz run resolve_excellon`

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = etchy_core::resolve_excellon(data);
});
