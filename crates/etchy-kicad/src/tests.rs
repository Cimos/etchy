use super::*;

fn board(body: &str) -> String {
    format!("(kicad_pcb (version 20240108) {body})")
}
fn tiny_limits() -> Limits {
    Limits {
        max_input_bytes: 1000,
        max_nesting_depth: 8,
        max_node_count: 100,
        max_atom_length: 20,
        max_string_length: 20,
    }
}

#[test]
fn parses_syntax_whitespace_comments_and_escapes() {
    let parsed = parse(board("# note\n(foo bare \"a\\\"b\\\\c\\n\")").as_bytes()).unwrap();
    assert_eq!(parsed.header.version, "20240108");
    let foo = &parsed.root.as_list().unwrap()[2];
    assert_eq!(foo.as_list().unwrap()[2].text(), Some("a\"b\\c\n"));
    assert_eq!(foo.span.start.line, 2);
}

#[test]
fn preserves_all_nodes_and_accounts_for_consumption() {
    let parsed = parse(board("(footprint (pad 1) (foo bar))").as_bytes()).unwrap();
    assert_eq!(parsed.node_count(), 13);
    let footprint = &parsed.root.as_list().unwrap()[2];
    let mut accounting = parsed.consumption();
    accounting.mark_child(footprint, 1);
    let paths = accounting.unconsumed_paths(&parsed.root);
    assert!(paths.contains(&"kicad_pcb/footprint/foo".to_owned()));
    assert!(!accounting.is_consumed(&footprint.as_list().unwrap()[2]));
}

#[test]
fn rejects_invalid_utf8() {
    assert_eq!(parse(&[b'(', 0xff]), Err(Error::InvalidUtf8 { offset: 1 }));
}

#[test]
fn rejects_malformed_inputs() {
    assert!(matches!(parse(b""), Err(Error::EmptyInput)));
    assert!(matches!(
        parse(b"(kicad_pcb (version 1)"),
        Err(Error::UnbalancedParens { .. })
    ));
    assert!(matches!(parse(b")"), Err(Error::UnbalancedParens { .. })));
    assert!(matches!(
        parse(b"(kicad_pcb (version 1)) x"),
        Err(Error::TrailingData { .. })
    ));
    assert!(matches!(
        parse(b"(wrong (version 1))"),
        Err(Error::InvalidRoot { .. })
    ));
    assert!(matches!(
        parse(b"(kicad_pcb)"),
        Err(Error::MissingVersion { .. })
    ));
    assert!(matches!(
        parse(b"(kicad_pcb (version 1) \"x)"),
        Err(Error::UnterminatedString { .. })
    ));
    assert!(matches!(
        parse(b"(kicad_pcb (version 1) \"\\q\")"),
        Err(Error::BadEscape { .. })
    ));
}

#[test]
fn enforces_each_limit() {
    let mut limits = tiny_limits();
    limits.max_input_bytes = 5;
    assert!(matches!(
        parse_with_limits(board("").as_bytes(), limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::InputBytes,
            ..
        })
    ));
    let mut limits = tiny_limits();
    limits.max_nesting_depth = 2;
    assert!(matches!(
        parse_with_limits(board("((x))").as_bytes(), limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::NestingDepth,
            ..
        })
    ));
    let mut limits = tiny_limits();
    limits.max_node_count = 3;
    assert!(matches!(
        parse_with_limits(board("").as_bytes(), limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::NodeCount,
            ..
        })
    ));
    let mut limits = tiny_limits();
    limits.max_atom_length = 3;
    assert!(matches!(
        parse_with_limits(board("(abcdef)").as_bytes(), limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::AtomLength,
            ..
        })
    ));
    let mut limits = tiny_limits();
    limits.max_string_length = 3;
    assert!(matches!(
        parse_with_limits(board("\"abcdef\"").as_bytes(), limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::StringLength,
            ..
        })
    ));
}

#[test]
fn checked_numeric_helpers_cover_traps() {
    let parsed =
        parse(board("(n 1.234567) (e 1234567e-6) (bad nan) (precise .0000001)").as_bytes())
            .unwrap();
    let children = parsed.root.as_list().unwrap();
    assert_eq!(
        parse_mm_nm(&children[2].as_list().unwrap()[1]),
        Ok(1_234_567)
    );
    assert_eq!(
        parse_mm_nm(&children[3].as_list().unwrap()[1]),
        Ok(1_234_567)
    );
    assert!(matches!(
        parse_f64(&children[4].as_list().unwrap()[1]),
        Err(Error::Numeric {
            problem: NumericProblem::NonFinite,
            ..
        })
    ));
    assert!(matches!(
        parse_mm_nm(&children[5].as_list().unwrap()[1]),
        Err(Error::Numeric {
            problem: NumericProblem::ExcessPrecision,
            ..
        })
    ));
}

#[test]
fn numeric_overlong_and_integer_overflow_are_typed() {
    let long = "9".repeat(129);
    let parsed = parse(
        board(&format!(
            "(a {long}) (b 9223372036854775808) (c 9223372036854.775808)"
        ))
        .as_bytes(),
    )
    .unwrap();
    let children = parsed.root.as_list().unwrap();
    assert!(matches!(
        parse_i64(&children[2].as_list().unwrap()[1]),
        Err(Error::Numeric {
            problem: NumericProblem::Overlong,
            ..
        })
    ));
    assert!(matches!(
        parse_i64(&children[3].as_list().unwrap()[1]),
        Err(Error::IntegerOverflow { .. })
    ));
    assert!(matches!(
        parse_mm_nm(&children[4].as_list().unwrap()[1]),
        Err(Error::IntegerOverflow { .. })
    ));
}

#[test]
fn extracts_header() {
    let parsed =
        parse(b"(kicad_pcb (version 20231120) (generator pcbnew) (generator_version \"8.0.1\"))")
            .unwrap();
    assert_eq!(
        parsed.header,
        Header {
            version: "20231120".into(),
            generator: Some("pcbnew".into()),
            generator_version: Some("8.0.1".into())
        }
    );
}

#[test]
fn preserves_unicode_strings_and_byte_spans() {
    let parsed = parse(board("(label \"μΩ\")").as_bytes()).unwrap();
    let label = &parsed.root.as_list().unwrap()[2].as_list().unwrap()[1];
    assert_eq!(label.text(), Some("μΩ"));
    assert_eq!(label.span.end.offset - label.span.start.offset, 6);
}

#[test]
fn decodes_every_utf8_width_in_strings() {
    let parsed = parse(board("(label \"aé中😀\")").as_bytes()).unwrap();
    let label = &parsed.root.as_list().unwrap()[2].as_list().unwrap()[1];
    assert_eq!(label.text(), Some("aé中😀"));
}

#[test]
fn long_strings_parse_in_linear_time() {
    let long = "x".repeat(200_000);
    let input = board(&format!("(label \"{long}\")"));
    let started = std::time::Instant::now();
    parse(input.as_bytes()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn diagnostics_are_deterministic() {
    let input = b"(kicad_pcb (version 1) \"\\z\")";
    assert_eq!(
        parse(input).unwrap_err().to_string(),
        parse(input).unwrap_err().to_string()
    );
}

#[test]
fn mutations_and_truncations_never_panic() {
    let sample = board("(footprint \"U1\" (at 1.25 -3) (pad \"1\" thru_hole circle))").into_bytes();
    for end in 0..=sample.len() {
        let _ = parse(&sample[..end]);
    }
    for index in 0..sample.len() {
        for replacement in [0, b'(', b')', b'"', b'\\', 0xff] {
            let mut mutated = sample.clone();
            mutated[index] = replacement;
            let _ = parse(&mutated);
        }
    }
}

#[test]
fn optional_public_board_smoke() {
    let Some(path) = option_env!("ETCHY_KICAD_SMOKE_FILE") else {
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let started = std::time::Instant::now();
    let parsed = parse(&bytes).unwrap();
    eprintln!(
        "Mad_RP2040: {} nodes in {:?}",
        parsed.node_count(),
        started.elapsed()
    );
}

fn native_fixture(relative: &str) -> ParsedBoard {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/native/kicad")
        .join(relative);
    parse(&std::fs::read(path).unwrap()).unwrap()
}

fn manifest_inventory(relative: &str) -> std::collections::BTreeMap<String, usize> {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/native/kicad/manifest.toml"),
    )
    .unwrap();
    let marker = format!("path = \"{relative}\"");
    let tail = manifest.split(&marker).nth(1).unwrap();
    let line = tail
        .lines()
        .find(|line| line.starts_with("expected_inventory = "))
        .unwrap();
    line.split('{')
        .nth(1)
        .unwrap()
        .trim_end_matches('}')
        .split(',')
        .map(|entry| {
            let (name, value) = entry.trim().split_once(" = ").unwrap();
            (name.to_owned(), value.parse().unwrap())
        })
        .collect()
}

fn check_fixture(relative: &str, major: u8, layer_count: usize, net_count: usize) {
    let parsed = native_fixture(relative);
    let tables = decode_tables(&parsed).unwrap();
    assert_eq!(tables.profile.producer_major, major);
    assert_eq!(tables.layers.len(), layer_count);
    assert_eq!(tables.nets.len(), net_count);
    let report = inventory(&parsed, Strictness::Strict).unwrap();
    for (kind, expected) in manifest_inventory(relative) {
        assert_eq!(
            report.kind_counts.get(&kind).copied().unwrap_or(0),
            expected,
            "{relative}: {kind}"
        );
    }
    assert!(report.consumption.unconsumed_paths(&parsed.root).is_empty());
    assert!(report.records.windows(2).all(|pair| pair[0] <= pair[1]));
}

macro_rules! fixture_test {
    ($name:ident, $path:literal, $major:literal, $layers:literal, $nets:literal) => {
        #[test]
        fn $name() {
            check_fixture($path, $major, $layers, $nets);
        }
    };
}

fixture_test!(
    fixture_track_arc,
    "fixtures/kicad7/track_arc.kicad_pcb",
    7,
    5,
    2
);
fixture_test!(
    fixture_pad_shapes,
    "fixtures/kicad7/pad_shapes.kicad_pcb",
    7,
    7,
    1
);
fixture_test!(
    fixture_via_spans,
    "fixtures/kicad7/via_spans.kicad_pcb",
    7,
    5,
    2
);
fixture_test!(
    fixture_filled_zone,
    "fixtures/kicad7/filled_zone.kicad_pcb",
    7,
    3,
    2
);
fixture_test!(
    fixture_unfilled_zone,
    "fixtures/kicad7/unfilled_zone.kicad_pcb",
    7,
    3,
    2
);
fixture_test!(
    fixture_stroke_text,
    "fixtures/kicad7/stroke_text.kicad_pcb",
    7,
    4,
    1
);
fixture_test!(
    fixture_renamed_layer,
    "fixtures/kicad7/renamed_layer.kicad_pcb",
    7,
    3,
    1
);
fixture_test!(
    fixture_bottom_footprint,
    "fixtures/kicad7/bottom_footprint.kicad_pcb",
    7,
    6,
    1
);
fixture_test!(
    fixture_mad_old,
    "mad_rp2040/old/Mad_RP2040.kicad_pcb",
    9,
    31,
    106
);
fixture_test!(
    fixture_mad_new,
    "mad_rp2040/new/Mad_RP2040.kicad_pcb",
    9,
    31,
    106
);

#[test]
fn renamed_layers_keep_internal_kinds() {
    let tables = decode_tables(&native_fixture("fixtures/kicad7/renamed_layer.kicad_pcb")).unwrap();
    assert_eq!(tables.layers[0].kind, LayerKind::TopCopper);
    assert_eq!(tables.layers[0].user_name.as_deref(), Some("Top Signal"));
    assert_eq!(tables.layers[2].kind, LayerKind::Outline);
}

fn decodable(body: &str) -> String {
    format!("(kicad_pcb (version 20221018) (general (thickness 1.6)) (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal)) (setup (pad_to_mask_clearance 0)) (net 0 \"\") {body})")
}

#[test]
fn unknown_top_level_is_counted_warning() {
    let parsed = parse(decodable("(mystery 1)").as_bytes()).unwrap();
    let report = inventory(&parsed, Strictness::Strict).unwrap();
    assert_eq!(report.unknown_counts["kicad_pcb/mystery [mystery]"], 1);
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn unknown_material_child_is_warning_or_strict_error() {
    let parsed = parse(decodable("(segment (start 0 0) (mystery 1))").as_bytes()).unwrap();
    assert_eq!(
        inventory(&parsed, Strictness::Report)
            .unwrap()
            .warnings
            .len(),
        1
    );
    assert!(matches!(
        inventory(&parsed, Strictness::Strict),
        Err(Error::UnknownMaterialRecord { .. })
    ));
}

#[test]
fn duplicate_required_field_is_typed() {
    let input = decodable("(general (thickness 2))");
    let parsed = parse(input.as_bytes()).unwrap();
    assert!(matches!(
        decode_tables(&parsed),
        Err(Error::DuplicateField { .. })
    ));
}

#[test]
fn future_version_is_rejected() {
    let parsed = parse(b"(kicad_pcb (version 20991231))").unwrap();
    assert!(
        matches!(version_profile(&parsed), Err(Error::UnsupportedBoardVersion { date, .. }) if date == "20991231")
    );
}

#[test]
fn accepted_aliases_decode() {
    let input = "(kicad_pcb (version 20221018) (general (thickness 1.6)) (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal) (37 \"F.Silkscreen\" user)) (setup (pad_to_mask_clearance 0)) (net 0 \"\") (gr_line (width 0.2) (tstamp abc)))";
    let parsed = parse(input.as_bytes()).unwrap();
    let tables = decode_tables(&parsed).unwrap();
    assert_eq!(tables.layers[2].kind, LayerKind::TopSilk);
    assert_eq!(
        inventory(&parsed, Strictness::Strict).unwrap().kind_counts["gr_line"],
        1
    );
}

#[test]
fn inventory_aggregation_is_stable_and_sorted() {
    let parsed =
        parse(decodable("(segment (start 0 0)) (segment (start 1 1))").as_bytes()).unwrap();
    let first = inventory(&parsed, Strictness::Strict).unwrap();
    let second = inventory(&parsed, Strictness::Strict).unwrap();
    assert_eq!(first.records, second.records);
    assert_eq!(first.kind_counts["segments"], 2);
    assert!(first.records.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn footprint_properties_are_kept_for_the_text_decoder() {
    let parsed = native_fixture("mad_rp2040/old/Mad_RP2040.kicad_pcb");
    let report = inventory(&parsed, Strictness::Strict).unwrap();
    let property = report
        .records
        .iter()
        .find(|record| record.path.ends_with("footprint/property"))
        .expect("footprint property record");
    assert!(matches!(
        property.disposition,
        Disposition::KnownForLater { .. }
    ));
}
