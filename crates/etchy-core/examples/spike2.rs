//! Spike 2 — Gerber + Excellon parse validation on real, multi-vendor fab output.
//!
//! THROWAWAY de-risking code (ROADMAP Phase 0). Answers two go/no-go questions:
//!
//!   A. Does `gerber-parser` 0.5 cover real fab output? → parse the whole corpus,
//!      report parse-success rate, a feature-coverage histogram, per-file errors,
//!      and (crucially) that it never PANICS (per-file `catch_unwind`).
//!   B. What is the scope of the hand-rolled Excellon front-end? → a minimal
//!      prototype parser over the real `.drl`/`.ncd`/… corpus, reporting the
//!      dialect variance it must absorb and validating tool/hit COUNTS against
//!      the `Quantity = N` ground-truth comments Allegro/OrCAD embed (counts are
//!      independent of coordinate format, so this is a clean structural oracle).
//!
//! Validation corpus is the gerbonara test suite (Apache-2.0 code; the board
//! files are third-party, so they are NOT vendored here). Reproduce:
//!
//!   git clone --depth 1 https://github.com/jaseg/gerbonara /tmp/gn
//!   git clone --depth 1 https://github.com/MakerPnP/gerber-parser /tmp/gp
//!   cargo run --release --example spike2 -p etchy-core -- \
//!       /tmp/gn/tests/resources /tmp/gp/assets corpus/synthetic
//!
//! Trust bar: files are routed by CONTENT sniff (not extension); unrecognized
//! files are counted, never silently ignored; ambiguous Excellon coordinate
//! formats are FLAGGED ("never guess"), not papered over.

use std::collections::BTreeMap;
use std::fs;
use std::panic;
use std::path::{Path, PathBuf};

use gerber_parser::gerber_types::{
    Aperture, Command, DCode, ExtendedCode, FunctionCode, GCode, InterpolationMode, Operation,
    Polarity,
};
use gerber_parser::parse;

fn main() {
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let roots = if roots.is_empty() {
        vec![PathBuf::from("corpus/synthetic")]
    } else {
        roots
    };

    let mut files = Vec::new();
    for r in &roots {
        collect_files(r, &mut files);
    }
    files.sort();
    files.dedup();

    let mut gerbers = Vec::new();
    let mut drills = Vec::new();
    let mut unknown = Vec::new();
    for f in &files {
        match sniff(f) {
            Some(Kind::Gerber) => gerbers.push(f.clone()),
            Some(Kind::Excellon) => drills.push(f.clone()),
            None => unknown.push(f.clone()),
        }
    }

    println!("=== etchy Spike 2 — Gerber + Excellon parse validation ===");
    println!("roots: {:?}", roots);
    println!(
        "discovered {} files → {} gerber, {} excellon, {} unrecognized\n",
        files.len(),
        gerbers.len(),
        drills.len(),
        unknown.len()
    );

    analyze_gerbers(&gerbers);
    println!();
    analyze_excellon(&drills);

    if !unknown.is_empty() {
        println!(
            "\n{} unrecognized (non-Gerber/Excellon) files skipped, e.g.:",
            unknown.len()
        );
        for f in unknown.iter().take(6) {
            println!("  {}", short(f));
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery + content sniff
// ---------------------------------------------------------------------------

enum Kind {
    Gerber,
    Excellon,
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

/// Route by content, not extension — fab tools use dozens of extensions.
fn sniff(path: &Path) -> Option<Kind> {
    let bytes = fs::read(path).ok()?;
    // Heuristic binary guard: lots of NULs ⇒ not a text fab file.
    if bytes.iter().take(2048).filter(|&&b| b == 0).count() > 4 {
        return None;
    }
    let head: String = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_string();

    // Excellon markers (check first — some share G-codes with Gerber).
    let excellon = head.contains("M48")
        || head.contains("FMAT,")
        || head.contains(";FILE_FORMAT")
        || (head.contains("\nT")
            && (head.contains("INCH") || head.contains("METRIC"))
            && !head.contains("%FS"));
    // Gerber markers.
    let gerber = head.contains("%FS")
        || head.contains("%MO")
        || head.contains("%AD")
        || head.contains("D03*");

    match (gerber, excellon) {
        (true, false) => Some(Kind::Gerber),
        (false, true) => Some(Kind::Excellon),
        (true, true) => Some(Kind::Gerber), // %FS present ⇒ Gerber wins
        (false, false) => None,
    }
}

fn short(p: &Path) -> String {
    // …/resources/<vendor>/<file>
    let comps: Vec<_> = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let n = comps.len();
    if n >= 2 {
        format!("{}/{}", comps[n - 2], comps[n - 1])
    } else {
        p.display().to_string()
    }
}

// ---------------------------------------------------------------------------
// A. Gerber coverage via gerber-parser
// ---------------------------------------------------------------------------

fn analyze_gerbers(files: &[PathBuf]) {
    println!(
        "--- A. Gerber: gerber-parser 0.5 over {} files ---",
        files.len()
    );
    let mut clean = 0usize; // parsed, zero per-command errors
    let mut with_errs = 0usize; // parsed, but some commands errored
    let mut hard_fail = 0usize; // parse() returned Err
    let mut panicked = 0usize;
    let mut feature_hist: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut feature_files: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut err_kinds: BTreeMap<String, usize> = BTreeMap::new(); // error kind → occurrences
    let mut err_examples: BTreeMap<String, Vec<String>> = BTreeMap::new(); // kind → sample lines
    let mut failures: Vec<(String, String)> = Vec::new();

    for f in files {
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| analyze_one_gerber(f)));
        match result {
            Err(_) => {
                panicked += 1;
                failures.push((short(f), "PANIC".to_string()));
            }
            Ok(Err(msg)) => {
                hard_fail += 1;
                failures.push((short(f), msg));
            }
            Ok(Ok(g)) => {
                if g.err_count == 0 {
                    clean += 1;
                } else {
                    with_errs += 1;
                    failures.push((
                        short(f),
                        format!("{} per-command error(s): {}", g.err_count, g.first_err),
                    ));
                }
                for (k, v) in g.err_kinds {
                    *err_kinds.entry(k).or_default() += v;
                }
                for (k, line) in g.err_samples {
                    let slot = err_examples.entry(k).or_default();
                    if !line.is_empty() && slot.len() < 3 && !slot.contains(&line) {
                        slot.push(line);
                    }
                }
                for (k, v) in g.features {
                    *feature_hist.entry(k).or_default() += v;
                    *feature_files.entry(k).or_default() += 1;
                }
            }
        }
    }

    println!(
        "  parse result: {} clean, {} parsed-with-errors, {} hard-fail, {} PANIC",
        clean, with_errs, hard_fail, panicked
    );
    let ok = clean + with_errs;
    println!(
        "  → {}/{} files parsed without crashing ({:.1}%)",
        ok,
        files.len(),
        100.0 * ok as f64 / files.len().max(1) as f64
    );

    if !err_kinds.is_empty() {
        println!("\n  per-command error kinds (occurrences) — what gerber-parser rejected:");
        for (kind, n) in &err_kinds {
            let ex = err_examples
                .get(kind)
                .map(|v| v.join(" | "))
                .unwrap_or_default();
            println!("    {:<38} {:>6}   e.g. {}", kind, n, ex);
        }
    }

    println!("\n  feature coverage (occurrences / files using):");
    for (feat, count) in &feature_hist {
        println!(
            "    {:<22} {:>9} occ  /  {:>3} files",
            feat,
            count,
            feature_files.get(feat).unwrap_or(&0)
        );
    }

    if !failures.is_empty() {
        println!("\n  files needing attention ({}):", failures.len());
        for (f, msg) in failures.iter().take(25) {
            let m: String = msg.chars().take(100).collect();
            println!("    {:<40} {}", short_pad(f), m);
        }
        if failures.len() > 25 {
            println!("    … {} more", failures.len() - 25);
        }
    }
}

fn short_pad(s: &str) -> String {
    let mut s = s.to_string();
    s.truncate(40);
    s
}

struct GerberStats {
    err_count: usize,
    first_err: String,
    err_kinds: Vec<(String, usize)>,
    err_samples: Vec<(String, String)>,
    features: Vec<(&'static str, usize)>,
}

/// Reduce a `GerberParserErrorWithContext` Debug string to a short, groupable
/// kind, e.g. `NoRegexMatch(%IR)` or `UnknownCommand(%LN)`.
fn err_kind(dbg: &str) -> String {
    // The inner variant follows "error: " — e.g. "NoRegexMatch", "UnknownCommand".
    let kind = dbg
        .split("error: ")
        .nth(1)
        .and_then(|s| s.split([' ', '{', ',', '(']).next())
        .filter(|s| !s.is_empty())
        .unwrap_or("Unknown")
        .to_string();
    // Try to attach the offending command token (e.g. %IR, %LN).
    let cmd = dbg
        .split('"')
        .find(|s| s.starts_with('%'))
        .map(|s| s.chars().take(3).collect::<String>());
    match cmd {
        Some(c) => format!("{kind}({c})"),
        None => kind,
    }
}

/// Pull the offending source line out of the Debug string: `line: Some((N, "…"))`.
fn offending_line(dbg: &str) -> String {
    dbg.split("line: Some((")
        .nth(1)
        .and_then(|s| s.split('"').nth(1))
        .unwrap_or("")
        .chars()
        .take(48)
        .collect()
}

fn analyze_one_gerber(path: &Path) -> Result<GerberStats, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let reader = std::io::BufReader::new(std::io::Cursor::new(bytes));
    let doc =
        parse(reader).map_err(|(_, e)| format!("{e:?}").chars().take(120).collect::<String>())?;

    let errs = doc.errors();
    let err_count = errs.len();
    let first_err = errs
        .first()
        .map(|e| format!("{e:?}").chars().take(120).collect())
        .unwrap_or_default();
    let mut err_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut err_samples: Vec<(String, String)> = Vec::new();
    for e in &errs {
        let dbg = format!("{e:?}");
        let kind = err_kind(&dbg);
        *err_kinds.entry(kind.clone()).or_default() += 1;
        err_samples.push((kind, offending_line(&dbg)));
    }

    // Feature tally: aperture shapes + operation/region/arc/polarity usage.
    let mut feat: BTreeMap<&'static str, usize> = BTreeMap::new();
    for ap in doc.apertures.values() {
        let k = match ap {
            Aperture::Circle(_) => "aperture:circle",
            Aperture::Rectangle(_) => "aperture:rect",
            Aperture::Obround(_) => "aperture:obround",
            Aperture::Polygon(_) => "aperture:polygon",
            Aperture::Macro(_, _) => "aperture:macro",
        };
        *feat.entry(k).or_default() += 1;
    }
    let mut in_region = false;
    for cmd in doc.commands() {
        match cmd {
            Command::FunctionCode(FunctionCode::DCode(DCode::Operation(op))) => {
                let k = match op {
                    Operation::Interpolate(..) if in_region => "op:region-segment",
                    Operation::Interpolate(..) => "op:interpolate(draw)",
                    Operation::Move(_) => "op:move",
                    Operation::Flash(_) => "op:flash",
                };
                *feat.entry(k).or_default() += 1;
            }
            Command::FunctionCode(FunctionCode::GCode(g)) => match g {
                GCode::RegionMode(true) => {
                    in_region = true;
                    *feat.entry("region(G36/G37)").or_default() += 1;
                }
                GCode::RegionMode(false) => in_region = false,
                GCode::InterpolationMode(InterpolationMode::ClockwiseCircular)
                | GCode::InterpolationMode(InterpolationMode::CounterclockwiseCircular) => {
                    *feat.entry("arc(G02/G03)").or_default() += 1;
                }
                _ => {}
            },
            Command::ExtendedCode(ec) => {
                let k = match ec {
                    ExtendedCode::LoadPolarity(Polarity::Clear) => Some("polarity:clear(LPC)"),
                    ExtendedCode::ApertureMacro(_) => Some("def:aperture-macro"),
                    ExtendedCode::StepAndRepeat(_) => Some("step-and-repeat(SR)"),
                    ExtendedCode::ApertureBlock(_) => Some("aperture-block(AB)"),
                    ExtendedCode::LoadMirroring(_) => Some("transform:mirror(LM)"),
                    ExtendedCode::LoadRotation(_) => Some("transform:rotate(LR)"),
                    ExtendedCode::LoadScaling(_) => Some("transform:scale(LS)"),
                    _ => None,
                };
                if let Some(k) = k {
                    *feat.entry(k).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    Ok(GerberStats {
        err_count,
        first_err,
        err_kinds: err_kinds.into_iter().collect(),
        err_samples,
        features: feat.into_iter().collect(),
    })
}

// ---------------------------------------------------------------------------
// B. Hand-rolled Excellon prototype (the scoping deliverable)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ExcellonDoc {
    units: Option<String>,             // INCH | METRIC
    zero_suppression: Option<String>,  // LZ | TZ
    explicit_format: Option<(u8, u8)>, // from ;FILE_FORMAT or unit param
    has_m48: bool,
    coord_style: CoordStyle,
    tools: BTreeMap<u32, f64>,  // tool number → diameter (in `units`)
    hits: BTreeMap<u32, usize>, // tool number → hit count
    slots: usize,               // G85 routed slots
    quantity_oracle: BTreeMap<u32, usize>, // tool → stated Quantity (Allegro/OrCAD)
    warnings: Vec<String>,
}

#[derive(Default, PartialEq, Debug)]
enum CoordStyle {
    #[default]
    Unknown,
    Decimal,          // coords carry a '.', unambiguous
    IntegerExplicit,  // integer coords + a known (int,dec) format
    IntegerAmbiguous, // integer coords, NO declared format → FAIL LOUD territory
}

fn analyze_excellon(files: &[PathBuf]) {
    println!(
        "--- B. Excellon: hand-rolled prototype over {} files ---",
        files.len()
    );
    let mut total_hits = 0usize;
    let mut oracle_checked = 0usize;
    let mut oracle_ok = 0usize;
    let mut ambiguous = 0usize;
    let mut no_header = 0usize;
    let mut variants: BTreeMap<String, usize> = BTreeMap::new();

    println!(
        "\n  {:<34} {:>6} {:>4} {:>7} {:>7} {:<16} oracle",
        "file", "units", "tls", "hits", "slots", "coord-format"
    );
    for f in files {
        let doc = parse_excellon(&fs::read_to_string(f).unwrap_or_default());
        let nhits: usize = doc.hits.values().sum();
        total_hits += nhits;
        if !doc.has_m48 {
            no_header += 1;
        }
        if doc.coord_style == CoordStyle::IntegerAmbiguous {
            ambiguous += 1;
        }
        let variant = format!(
            "{}{}{}",
            doc.units.clone().unwrap_or_else(|| "?units".into()),
            doc.zero_suppression
                .as_ref()
                .map(|z| format!(",{z}"))
                .unwrap_or_default(),
            if doc.has_m48 { "" } else { " (no-M48)" }
        );
        *variants.entry(variant).or_default() += 1;

        // Oracle: compare per-tool hits to stated Quantity, where present.
        let oracle = if doc.quantity_oracle.is_empty() {
            "—".to_string()
        } else {
            oracle_checked += 1;
            let mut all_match = true;
            for (t, q) in &doc.quantity_oracle {
                if doc.hits.get(t).copied().unwrap_or(0) != *q {
                    all_match = false;
                }
            }
            if all_match {
                oracle_ok += 1;
                "MATCH".to_string()
            } else {
                "MISMATCH".to_string()
            }
        };

        let fmt = match doc.coord_style {
            CoordStyle::Decimal => "decimal".to_string(),
            CoordStyle::IntegerExplicit => doc
                .explicit_format
                .map(|(i, d)| format!("int {i}:{d}"))
                .unwrap_or_else(|| "int ?".into()),
            CoordStyle::IntegerAmbiguous => "int AMBIGUOUS".to_string(),
            CoordStyle::Unknown => "unknown".to_string(),
        };
        println!(
            "  {:<34} {:>6} {:>4} {:>7} {:>7} {:<16} {}",
            short_pad34(&short(f)),
            doc.units.clone().unwrap_or_else(|| "?".into()),
            doc.tools.len(),
            nhits,
            doc.slots,
            fmt,
            oracle
        );
    }

    println!(
        "\n  totals: {} drill files, {} hits parsed",
        files.len(),
        total_hits
    );
    println!("  header variants:");
    for (v, n) in &variants {
        println!("    {:>3}×  {}", n, v);
    }
    println!(
        "  oracle (Quantity= comments): {}/{} files matched on tool/hit counts",
        oracle_ok, oracle_checked
    );
    println!(
        "  no-M48-header files: {}   |   ambiguous coordinate format (need policy): {}",
        no_header, ambiguous
    );
}

fn short_pad34(s: &str) -> String {
    let mut s = s.to_string();
    if s.len() > 34 {
        s = format!("…{}", &s[s.len() - 33..]);
    }
    s
}

/// Minimal Excellon parser — the M1 scoping prototype. Handles M48 (optional),
/// INCH/METRIC + LZ/TZ + explicit format, `;FILE_FORMAT=a:b`, tool table
/// `T<n>C<dia>[Fxx][Sxx]`, tool select, `X..Y..` hits, `G85` slots, and the
/// Allegro/OrCAD `Quantity = N` oracle comments. Flags ambiguous integer
/// coordinate formats instead of guessing.
fn parse_excellon(text: &str) -> ExcellonDoc {
    let mut d = ExcellonDoc::default();
    let mut in_header = false;
    let mut current_tool: Option<u32> = None;
    let mut saw_decimal_coord = false;
    let mut saw_integer_coord = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Comments — but mine them for format + quantity ground truth.
        if let Some(c) = line.strip_prefix(';') {
            if let Some(fmt) = c.split("FILE_FORMAT=").nth(1) {
                if let Some((i, dec)) = parse_ab(fmt) {
                    d.explicit_format = Some((i, dec));
                }
            }
            // Allegro/OrCAD: ";T01 Holesize 1. = 12.0 … Quantity = 241"
            if let (Some(t), Some(q)) = (mine_tool(c), mine_quantity(c)) {
                d.quantity_oracle.insert(t, q);
            }
            continue;
        }

        if line == "M48" {
            d.has_m48 = true;
            in_header = true;
            continue;
        }
        if line == "%" {
            in_header = false;
            continue;
        }
        if line.starts_with("FMAT") {
            continue;
        }
        // Units line: "INCH", "INCH,LZ", "METRIC,TZ,000.000"
        if line.starts_with("INCH") || line.starts_with("METRIC") {
            let mut parts = line.split(',');
            d.units = Some(parts.next().unwrap().to_string());
            for p in parts {
                let p = p.trim();
                if p == "LZ" || p == "TZ" {
                    d.zero_suppression = Some(p.to_string());
                } else if p.contains('0') && p.contains('.') {
                    // e.g. "000.000" → 3:3
                    let i = p.split('.').next().unwrap().len() as u8;
                    let dec = p.split('.').nth(1).unwrap().len() as u8;
                    d.explicit_format = Some((i, dec));
                }
            }
            continue;
        }
        // Tool definition: T<n> … C<dia> … . Fields (F feed, S speed) may appear in
        // ANY order around C — e.g. KiCad `T1C0.016`, Target3001 `T1F00S00C0.300`.
        // So: tool number = leading digits after T; diameter = digits after 'C'.
        if let Some(rest) = line.strip_prefix('T') {
            let tn_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(tn) = tn_str.parse::<u32>() {
                if let Some(cpos) = rest.find('C') {
                    let dia_str: String = rest[cpos + 1..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    if let Ok(dia) = dia_str.parse::<f64>() {
                        d.tools.insert(tn, dia);
                        continue;
                    }
                }
                // Tool SELECT in body: "T1" (no C).
                if !in_header {
                    current_tool = Some(tn);
                }
                continue;
            }
        }
        // Coordinate / op lines in the body.
        if !in_header && (line.starts_with('X') || line.starts_with('Y')) {
            if line.contains("G85") {
                d.slots += 1;
            }
            // classify coordinate style from the first coordinate token
            if line.contains('.') {
                saw_decimal_coord = true;
            } else {
                saw_integer_coord = true;
            }
            if let Some(t) = current_tool {
                *d.hits.entry(t).or_default() += 1;
            } else {
                // hit before any tool select — record once
                if d.warnings.is_empty() {
                    d.warnings.push("coordinate before tool-select".into());
                }
            }
            continue;
        }
        // G90/G05/M71/M72/M30/G00/G01/… modes: ignored for the count-level spike.
    }

    d.coord_style = if saw_decimal_coord {
        CoordStyle::Decimal
    } else if saw_integer_coord {
        if d.explicit_format.is_some() {
            CoordStyle::IntegerExplicit
        } else {
            CoordStyle::IntegerAmbiguous
        }
    } else {
        CoordStyle::Unknown
    };
    d
}

fn parse_ab(s: &str) -> Option<(u8, u8)> {
    let s: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let (a, b) = s.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// From ";T01 Holesize …" → 1
fn mine_tool(c: &str) -> Option<u32> {
    let c = c.trim();
    let rest = c.strip_prefix('T')?;
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

/// From "… Quantity = 241" → 241
fn mine_quantity(c: &str) -> Option<usize> {
    let after = c.split("Quantity").nth(1)?;
    let num: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    num.parse().ok()
}
