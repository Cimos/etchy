#![no_main]
//! Fuzz the Gerber drill-table path (#457): resolve a Gerber layer, turn its
//! circle flashes into a drill table, and compare that table. The contract: for
//! ANY input bytes this returns a table, a reason there is none, or a typed
//! `EngineError` — it never panics, and a table never differs from itself.
//!
//! Run: `cargo +nightly fuzz run gerber_drill_table`

use etchy_core::{resolve_layer_detailed, DrillTableReport};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(resolved) = resolve_layer_detailed(data) else {
        return;
    };
    if let Ok(tools) = resolved.flashes.into_drill_tools() {
        let _ = DrillTableReport::compare(Some(&tools), None);
        if let Some(report) = DrillTableReport::compare(Some(&tools), Some(&tools)) {
            assert!(!report.has_changes(), "a drill table differs from itself");
        }
    }
});
