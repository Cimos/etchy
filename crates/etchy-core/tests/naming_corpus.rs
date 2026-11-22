//! Filename-classification corpus (IN-6, #426): every `corpus/naming/*.txt`
//! line `<filename> => <kind>` must classify as listed.

use etchy_core::{classify, LayerKind};
use std::path::PathBuf;

fn label(kind: LayerKind) -> String {
    match kind {
        LayerKind::InnerCopper(n) => format!("inner-copper-{n}"),
        k => k.kind_str().to_string(),
    }
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/naming")
}

fn check_scheme(file: &str) {
    let path = corpus_dir().join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut failures = Vec::new();
    let mut checked = 0;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, want) = line
            .split_once(" => ")
            .unwrap_or_else(|| panic!("{file}:{}: expected `<name> => <kind>`", i + 1));
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let got = label(classify(stem, ext));
        if got != want.trim() {
            failures.push(format!("{file}:{}: {name} => {got}, want {want}", i + 1));
        }
        checked += 1;
    }
    assert!(checked > 0, "{file} has no entries");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn altium_baseline() {
    check_scheme("altium.txt");
}

#[test]
fn kicad_baseline() {
    check_scheme("kicad.txt");
}

#[test]
fn protel_scheme() {
    check_scheme("protel.txt");
}

#[test]
fn eagle_scheme() {
    check_scheme("eagle.txt");
}

#[test]
fn orcad_allegro_scheme() {
    check_scheme("orcad-allegro.txt");
}

#[test]
fn easyeda_scheme() {
    check_scheme("easyeda.txt");
}

#[test]
fn every_corpus_file_is_checked() {
    let mut files: Vec<String> = std::fs::read_dir(corpus_dir())
        .expect("corpus/naming")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".txt"))
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "altium.txt",
            "eagle.txt",
            "easyeda.txt",
            "kicad.txt",
            "orcad-allegro.txt",
            "protel.txt"
        ]
    );
}
