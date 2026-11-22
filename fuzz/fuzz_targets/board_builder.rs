#![no_main]
//! Fuzz board loading (#436): a few named files go through `BoardBuilder`,
//! which classifies each by name, sniffs its content and routes it to the
//! Gerber, Excellon or placement front-end; the board is then compared with
//! itself. The contract: for ANY names and bytes, each push returns `Ok` or a
//! typed `EngineError`, and the compare never panics.
//!
//! Run: `cargo +nightly fuzz run board_builder`

use etchy_core::BoardBuilder;
use libfuzzer_sys::fuzz_target;

/// Enough files to mix Gerber, Excellon and placement layers on one board.
const MAX_FILES: usize = 4;

fuzz_target!(|files: Vec<(String, Vec<u8>)>| {
    let mut builder = BoardBuilder::new();
    for (name, bytes) in files.into_iter().take(MAX_FILES) {
        let _ = builder.push(name, bytes);
    }
    let (board, _, _) = builder.finish();
    let _ = etchy_core::compare(&board, &board);
});
