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
