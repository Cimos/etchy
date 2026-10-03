#![forbid(unsafe_code)]

use std::fmt;

const MAX_NUMERIC_TEXT: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    id: usize,
    pub span: Span,
    pub kind: NodeKind,
}

impl Node {
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn as_atom(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Atom(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Node]> {
        match &self.kind {
            NodeKind::List(children) => Some(children),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Atom(value) | NodeKind::String(value) => Some(value),
            NodeKind::List(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeKind {
    List(Vec<Node>),
    Atom(String),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub version: String,
    pub generator: Option<String>,
    pub generator_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedBoard {
    pub root: Node,
    pub header: Header,
    node_count: usize,
}

impl ParsedBoard {
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn consumption(&self) -> Consumption {
        Consumption {
            consumed: vec![false; self.node_count],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_nesting_depth: usize,
    pub max_node_count: usize,
    pub max_atom_length: usize,
    pub max_string_length: usize,
}

impl Limits {
    pub const fn native() -> Self {
        Self {
            max_input_bytes: 256 * 1024 * 1024,
            max_nesting_depth: 256,
            max_node_count: 8_000_000,
            max_atom_length: 1024 * 1024,
            max_string_length: 16 * 1024 * 1024,
        }
    }

    pub const fn wasm() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_nesting_depth: 128,
            max_node_count: 2_000_000,
            max_atom_length: 256 * 1024,
            max_string_length: 4 * 1024 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        if cfg!(target_arch = "wasm32") {
            Self::wasm()
        } else {
            Self::native()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitKind {
    InputBytes,
    NestingDepth,
    NodeCount,
    AtomLength,
    StringLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericProblem {
    NonFinite,
    Overlong,
    Invalid,
    ExcessPrecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidUtf8 {
        offset: usize,
    },
    UnterminatedString {
        span: Span,
    },
    BadEscape {
        escape: u8,
        span: Span,
    },
    UnbalancedParens {
        span: Span,
    },
    EmptyInput,
    TrailingData {
        span: Span,
    },
    LimitExceeded {
        limit: LimitKind,
        span: Span,
    },
    InvalidRoot {
        span: Span,
    },
    MissingVersion {
        span: Span,
    },
    Numeric {
        problem: NumericProblem,
        span: Span,
    },
    IntegerOverflow {
        span: Span,
    },
    UnsupportedBoardVersion {
        date: String,
        span: Span,
    },
    DuplicateField {
        field: String,
        span: Span,
    },
    MissingField {
        field: String,
        span: Span,
    },
    InvalidRecord {
        record: String,
        span: Span,
    },
    UnknownMaterialRecord {
        path: String,
        token: String,
        span: Span,
    },
    UnknownMaterialLayer {
        name: String,
        ordinal: i64,
        span: Span,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 { offset } => write!(f, "invalid UTF-8 at byte {offset}"),
            Self::UnterminatedString { span } => at(f, "unterminated string", *span),
            Self::BadEscape { escape, span } => {
                at(f, &format!("bad string escape \\x{escape:02x}"), *span)
            }
            Self::UnbalancedParens { span } => at(f, "unbalanced parentheses", *span),
            Self::EmptyInput => f.write_str("empty input"),
            Self::TrailingData { span } => at(f, "trailing data after root", *span),
            Self::LimitExceeded { limit, span } => {
                at(f, &format!("resource limit exceeded: {limit:?}"), *span)
            }
            Self::InvalidRoot { span } => at(f, "root list head is not kicad_pcb", *span),
            Self::MissingVersion { span } => at(f, "missing board version", *span),
            Self::Numeric { problem, span } => {
                at(f, &format!("numeric text is {problem:?}"), *span)
            }
            Self::IntegerOverflow { span } => at(f, "integer overflow", *span),
            Self::UnsupportedBoardVersion { date, span } => {
                at(f, &format!("unsupported board version {date}"), *span)
            }
            Self::DuplicateField { field, span } => {
                at(f, &format!("duplicate required field {field}"), *span)
            }
            Self::MissingField { field, span } => at(f, &format!("missing field {field}"), *span),
            Self::InvalidRecord { record, span } => {
                at(f, &format!("invalid {record} record"), *span)
            }
            Self::UnknownMaterialRecord { path, token, span } => at(
                f,
                &format!("unknown material-bearing record {token} at {path}"),
                *span,
            ),
            Self::UnknownMaterialLayer {
                name,
                ordinal,
                span,
            } => at(
                f,
                &format!("unknown material layer {name} at ordinal {ordinal}"),
                *span,
            ),
        }
    }
}

impl std::error::Error for Error {}

fn at(f: &mut fmt::Formatter<'_>, message: &str, span: Span) -> fmt::Result {
    write!(
        f,
        "{message} at byte {} ({}:{})",
        span.start.offset, span.start.line, span.start.column
    )
}

pub fn parse(input: &[u8]) -> Result<ParsedBoard, Error> {
    parse_with_limits(input, Limits::default())
}

pub fn parse_with_limits(input: &[u8], limits: Limits) -> Result<ParsedBoard, Error> {
    if input.len() > limits.max_input_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::InputBytes,
            span: point_span(input, limits.max_input_bytes),
        });
    }
    let text = std::str::from_utf8(input).map_err(|error| Error::InvalidUtf8 {
        offset: error.valid_up_to(),
    })?;
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        line: 1,
        column: 1,
        next_id: 0,
        limits,
    };
    parser.skip_space();
    if parser.eof() {
        return Err(Error::EmptyInput);
    }
    let root = parser.node(1)?;
    parser.skip_space();
    if !parser.eof() {
        return Err(Error::TrailingData {
            span: parser.here_span(),
        });
    }
    let header = extract_header(&root)?;
    Ok(ParsedBoard {
        root,
        header,
        node_count: parser.next_id,
    })
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    column: usize,
    next_id: usize,
    limits: Limits,
}

impl Parser<'_> {
    fn node(&mut self, depth: usize) -> Result<Node, Error> {
        if self.next_id >= self.limits.max_node_count {
            return Err(Error::LimitExceeded {
                limit: LimitKind::NodeCount,
                span: self.here_span(),
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        let start = self.position();
        let kind = match self.peek() {
            Some(b'(') => {
                if depth > self.limits.max_nesting_depth {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::NestingDepth,
                        span: self.here_span(),
                    });
                }
                self.bump();
                let mut children = Vec::new();
                loop {
                    self.skip_space();
                    match self.peek() {
                        Some(b')') => {
                            self.bump();
                            break;
                        }
                        None => {
                            return Err(Error::UnbalancedParens {
                                span: Span {
                                    start,
                                    end: self.position(),
                                },
                            })
                        }
                        _ => children.push(self.node(depth + 1)?),
                    }
                }
                NodeKind::List(children)
            }
            Some(b')') => {
                return Err(Error::UnbalancedParens {
                    span: self.here_span(),
                })
            }
            Some(b'"') => NodeKind::String(self.string()?),
            Some(_) => NodeKind::Atom(self.atom()?),
            None => {
                return Err(Error::UnbalancedParens {
                    span: self.here_span(),
                })
            }
        };
        Ok(Node {
            id,
            span: Span {
                start,
                end: self.position(),
            },
            kind,
        })
    }

    fn string(&mut self) -> Result<String, Error> {
        let start = self.position();
        self.bump();
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or(Error::UnterminatedString {
                span: Span {
                    start,
                    end: self.position(),
                },
            })?;
            if byte == b'"' {
                self.bump();
                return Ok(out);
            }
            if byte == b'\\' {
                let escape_start = self.position();
                self.bump();
                let escaped = self.peek().ok_or(Error::UnterminatedString {
                    span: Span {
                        start,
                        end: self.position(),
                    },
                })?;
                self.bump();
                match escaped {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    other => {
                        return Err(Error::BadEscape {
                            escape: other,
                            span: Span {
                                start: escape_start,
                                end: self.position(),
                            },
                        })
                    }
                }
            } else {
                let width = match byte {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                let character = std::str::from_utf8(&self.bytes[self.pos..self.pos + width])
                    .expect("validated input")
                    .chars()
                    .next()
                    .expect("input remains");
                for _ in 0..character.len_utf8() {
                    self.bump();
                }
                out.push(character);
            }
            if out.len() > self.limits.max_string_length {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::StringLength,
                    span: Span {
                        start,
                        end: self.position(),
                    },
                });
            }
        }
    }

    fn atom(&mut self) -> Result<String, Error> {
        let start = self.position();
        let offset = self.pos;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'"' | b'#') {
                break;
            }
            self.bump();
            if self.pos - offset > self.limits.max_atom_length {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::AtomLength,
                    span: Span {
                        start,
                        end: self.position(),
                    },
                });
            }
        }
        Ok(std::str::from_utf8(&self.bytes[offset..self.pos])
            .expect("validated input")
            .to_owned())
    }

    fn skip_space(&mut self) {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.bump();
            }
            if self.peek() != Some(b'#') {
                break;
            }
            while self.peek().is_some_and(|byte| byte != b'\n') {
                self.bump();
            }
        }
    }

    fn bump(&mut self) {
        let byte = self.bytes[self.pos];
        self.pos += 1;
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else if byte & 0b1100_0000 != 0b1000_0000 {
            self.column += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn eof(&self) -> bool {
        self.pos == self.bytes.len()
    }
    fn position(&self) -> Position {
        Position {
            offset: self.pos,
            line: self.line,
            column: self.column,
        }
    }
    fn here_span(&self) -> Span {
        let position = self.position();
        Span {
            start: position,
            end: position,
        }
    }
}

fn point_span(input: &[u8], offset: usize) -> Span {
    let mut line = 1;
    let mut column = 1;
    for &byte in &input[..offset.min(input.len())] {
        if byte == b'\n' {
            line += 1;
            column = 1;
        } else if byte & 0b1100_0000 != 0b1000_0000 {
            column += 1;
        }
    }
    let position = Position {
        offset,
        line,
        column,
    };
    Span {
        start: position,
        end: position,
    }
}

fn extract_header(root: &Node) -> Result<Header, Error> {
    let children = root
        .as_list()
        .ok_or(Error::InvalidRoot { span: root.span })?;
    if children.first().and_then(Node::as_atom) != Some("kicad_pcb") {
        return Err(Error::InvalidRoot { span: root.span });
    }
    let mut version = None;
    let mut generator = None;
    let mut generator_version = None;
    for child in &children[1..] {
        let Some(items) = child.as_list() else {
            continue;
        };
        let Some(name) = items.first().and_then(Node::as_atom) else {
            continue;
        };
        let value = items.get(1).and_then(Node::text).map(str::to_owned);
        match name {
            "version" => version = value,
            "generator" => generator = value,
            "generator_version" => generator_version = value,
            _ => {}
        }
    }
    Ok(Header {
        version: version.ok_or(Error::MissingVersion { span: root.span })?,
        generator,
        generator_version,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Consumption {
    consumed: Vec<bool>,
}

impl Consumption {
    pub fn mark(&mut self, node: &Node) {
        self.consumed[node.id] = true;
    }

    pub fn mark_child(&mut self, list: &Node, child_index: usize) -> bool {
        let Some(child) = list
            .as_list()
            .and_then(|children| children.get(child_index))
        else {
            return false;
        };
        self.mark(child);
        true
    }

    pub fn is_consumed(&self, node: &Node) -> bool {
        self.consumed[node.id]
    }

    pub fn unconsumed_paths(&self, root: &Node) -> Vec<String> {
        let mut paths = Vec::new();
        let root_name = list_name(root).unwrap_or("<root>");
        collect_unconsumed(root, root_name, self, &mut paths, true);
        paths
    }
}

fn collect_unconsumed(
    node: &Node,
    path: &str,
    state: &Consumption,
    out: &mut Vec<String>,
    is_root: bool,
) {
    if !is_root && !state.is_consumed(node) {
        out.push(path.to_owned());
    }
    if let Some(children) = node.as_list() {
        for child in children.iter().skip(1) {
            let segment = list_name(child)
                .or_else(|| child.text())
                .unwrap_or("<list>");
            let child_path = format!("{path}/{segment}");
            collect_unconsumed(child, &child_path, state, out, false);
        }
    }
}

fn list_name(node: &Node) -> Option<&str> {
    node.as_list()?.first()?.as_atom()
}

pub fn parse_i64(node: &Node) -> Result<i64, Error> {
    let text = numeric_text(node)?;
    text.parse().map_err(|_| {
        if valid_integer(text) {
            Error::IntegerOverflow { span: node.span }
        } else {
            Error::Numeric {
                problem: NumericProblem::Invalid,
                span: node.span,
            }
        }
    })
}

pub fn parse_f64(node: &Node) -> Result<f64, Error> {
    let text = numeric_text(node)?;
    let value: f64 = text.parse().map_err(|_| Error::Numeric {
        problem: NumericProblem::Invalid,
        span: node.span,
    })?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::Numeric {
            problem: NumericProblem::NonFinite,
            span: node.span,
        })
    }
}

pub fn parse_mm_nm(node: &Node) -> Result<i64, Error> {
    let text = numeric_text(node)?;
    decimal_scaled(text, 6)
        .map_err(|problem| match problem {
            NumericProblem::NonFinite
            | NumericProblem::Invalid
            | NumericProblem::ExcessPrecision => Error::Numeric {
                problem,
                span: node.span,
            },
            NumericProblem::Overlong => Error::Numeric {
                problem,
                span: node.span,
            },
        })
        .and_then(|value| {
            i64::try_from(value).map_err(|_| Error::IntegerOverflow { span: node.span })
        })
}

fn numeric_text(node: &Node) -> Result<&str, Error> {
    let text = node.as_atom().ok_or(Error::Numeric {
        problem: NumericProblem::Invalid,
        span: node.span,
    })?;
    if text.len() > MAX_NUMERIC_TEXT {
        return Err(Error::Numeric {
            problem: NumericProblem::Overlong,
            span: node.span,
        });
    }
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "nan" | "+nan" | "-nan" | "inf" | "+inf" | "-inf" | "infinity" | "+infinity" | "-infinity"
    ) {
        return Err(Error::Numeric {
            problem: NumericProblem::NonFinite,
            span: node.span,
        });
    }
    Ok(text)
}

fn valid_integer(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn decimal_scaled(text: &str, scale: usize) -> Result<i128, NumericProblem> {
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (
            &unsigned[..index],
            unsigned[index + 1..]
                .parse::<i32>()
                .map_err(|_| NumericProblem::Invalid)?,
        ),
        None => (unsigned, 0),
    };
    let mut digits = String::new();
    let mut fraction = 0usize;
    let mut dot = false;
    for byte in mantissa.bytes() {
        match byte {
            b'.' if !dot => dot = true,
            b'0'..=b'9' => {
                digits.push(byte as char);
                if dot {
                    fraction += 1;
                }
            }
            _ => return Err(NumericProblem::Invalid),
        }
    }
    if digits.is_empty() {
        return Err(NumericProblem::Invalid);
    }
    let shift = scale as i32 + exponent - fraction as i32;
    let mut value = digits
        .parse::<i128>()
        .map_err(|_| NumericProblem::Overlong)?;
    if shift < 0 {
        let divisor = checked_pow10((-shift) as u32).ok_or(NumericProblem::Overlong)?;
        if value % divisor != 0 {
            return Err(NumericProblem::ExcessPrecision);
        }
        value /= divisor;
    } else {
        value = value
            .checked_mul(checked_pow10(shift as u32).ok_or(NumericProblem::Overlong)?)
            .ok_or(NumericProblem::Overlong)?;
    }
    Ok(if negative { -value } else { value })
}

fn checked_pow10(power: u32) -> Option<i128> {
    (0..power).try_fold(1i128, |value, _| value.checked_mul(10))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileEvidence {
    Corpus,
    NotYetVerified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenAliases {
    pub silk: &'static [&'static str],
    pub stable_id: &'static [&'static str],
    pub stroke_width: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionProfile {
    pub producer_major: u8,
    pub header_dates: &'static [&'static str],
    pub aliases: TokenAliases,
    pub evidence: ProfileEvidence,
}

const MODERN_ALIASES: TokenAliases = TokenAliases {
    silk: &["F.SilkS", "B.SilkS", "F.Silkscreen", "B.Silkscreen"],
    stable_id: &["tstamp", "uuid"],
    stroke_width: &["width", "stroke"],
};

pub const VERSION_PROFILES: &[VersionProfile] = &[
    VersionProfile {
        producer_major: 6,
        header_dates: &[],
        aliases: MODERN_ALIASES,
        evidence: ProfileEvidence::NotYetVerified,
    },
    VersionProfile {
        producer_major: 7,
        header_dates: &["20221018"],
        aliases: MODERN_ALIASES,
        evidence: ProfileEvidence::Corpus,
    },
    VersionProfile {
        producer_major: 8,
        header_dates: &[],
        aliases: MODERN_ALIASES,
        evidence: ProfileEvidence::NotYetVerified,
    },
    VersionProfile {
        producer_major: 9,
        header_dates: &["20241229"],
        aliases: MODERN_ALIASES,
        evidence: ProfileEvidence::Corpus,
    },
    VersionProfile {
        producer_major: 10,
        header_dates: &[],
        aliases: MODERN_ALIASES,
        evidence: ProfileEvidence::NotYetVerified,
    },
];

pub fn version_profile(board: &ParsedBoard) -> Result<&'static VersionProfile, Error> {
    let mut versions = board
        .root
        .as_list()
        .into_iter()
        .flatten()
        .filter(|node| list_name(node) == Some("version"));
    let first = versions.next().ok_or(Error::MissingVersion {
        span: board.root.span,
    })?;
    if versions.next().is_some() {
        return Err(Error::DuplicateField {
            field: "version".into(),
            span: first.span,
        });
    }
    VERSION_PROFILES
        .iter()
        .find(|profile| {
            profile
                .header_dates
                .contains(&board.header.version.as_str())
        })
        .ok_or_else(|| Error::UnsupportedBoardVersion {
            date: board.header.version.clone(),
            span: first.span,
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Side {
    Front,
    Back,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum LayerKind {
    TopCopper,
    InnerCopper(u8),
    BottomCopper,
    TopMask,
    BottomMask,
    TopPaste,
    BottomPaste,
    TopSilk,
    BottomSilk,
    Outline,
    Fab(Side),
    Courtyard(Side),
    Adhesive(Side),
    User,
    Eco,
    Comments,
    Drawings,
    Margin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layer {
    pub ordinal: i64,
    pub internal_name: String,
    pub layer_type: String,
    pub user_name: Option<String>,
    pub kind: LayerKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointNm {
    pub x: i64,
    pub y: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub raw: Node,
    pub pad_to_mask_clearance_nm: Option<i64>,
    pub solder_mask_min_width_nm: Option<i64>,
    pub pad_to_paste_clearance_nm: Option<i64>,
    pub pad_to_paste_clearance_ratio: Option<f64>,
    pub aux_axis_origin_nm: Option<PointNm>,
    pub grid_origin_nm: Option<PointNm>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct General {
    pub thickness_nm: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoardTables {
    pub profile: &'static VersionProfile,
    pub layers: Vec<Layer>,
    pub setup: Setup,
    pub nets: std::collections::BTreeMap<i64, String>,
    pub general: General,
}

pub fn decode_tables(board: &ParsedBoard) -> Result<BoardTables, Error> {
    let profile = version_profile(board)?;
    let root = board.root.as_list().ok_or(Error::InvalidRoot {
        span: board.root.span,
    })?;
    let layers_node = exactly_one(root, "layers", board.root.span)?;
    let setup_node = exactly_one(root, "setup", board.root.span)?;
    let general_node = exactly_one(root, "general", board.root.span)?;
    let layers = decode_layers(layers_node, profile)?;
    let setup = decode_setup(setup_node)?;
    let general_items = general_node.as_list().unwrap_or_default();
    let thickness = exactly_one(general_items, "thickness", general_node.span)?;
    let thickness_nm = parse_mm_nm(value(thickness, 1)?)?;
    let mut nets = std::collections::BTreeMap::new();
    for node in root.iter().filter(|node| list_name(node) == Some("net")) {
        let items = node.as_list().unwrap_or_default();
        let number = parse_i64(value(node, 1)?)?;
        let name = value(node, 2)?
            .text()
            .ok_or_else(|| invalid("net", node.span))?
            .to_owned();
        if nets.insert(number, name).is_some() {
            return Err(Error::DuplicateField {
                field: format!("net {number}"),
                span: node.span,
            });
        }
        if items.len() != 3 {
            return Err(invalid("net", node.span));
        }
    }
    Ok(BoardTables {
        profile,
        layers,
        setup,
        nets,
        general: General { thickness_nm },
    })
}

fn decode_layers(node: &Node, profile: &VersionProfile) -> Result<Vec<Layer>, Error> {
    let mut raw = Vec::new();
    for item in node.as_list().unwrap_or_default().iter().skip(1) {
        let fields = item.as_list().ok_or_else(|| invalid("layer", item.span))?;
        if fields.len() < 3 || fields.len() > 4 {
            return Err(invalid("layer", item.span));
        }
        raw.push((
            item,
            parse_i64(&fields[0])?,
            fields[1]
                .text()
                .ok_or_else(|| invalid("layer", item.span))?
                .to_owned(),
            fields[2]
                .text()
                .ok_or_else(|| invalid("layer", item.span))?
                .to_owned(),
            fields.get(3).and_then(Node::text).map(str::to_owned),
        ));
    }
    let mut inner = 0u8;
    let mut out = Vec::with_capacity(raw.len());
    for (source, ordinal, name, layer_type, user_name) in raw {
        let kind = match name.as_str() {
            "F.Cu" => LayerKind::TopCopper,
            "B.Cu" => LayerKind::BottomCopper,
            n if n.starts_with("In")
                && n.ends_with(".Cu")
                && matches!(layer_type.as_str(), "signal" | "power" | "mixed" | "jumper") =>
            {
                inner = inner
                    .checked_add(1)
                    .ok_or(Error::IntegerOverflow { span: source.span })?;
                LayerKind::InnerCopper(inner)
            }
            "F.Mask" => LayerKind::TopMask,
            "B.Mask" => LayerKind::BottomMask,
            "F.Paste" => LayerKind::TopPaste,
            "B.Paste" => LayerKind::BottomPaste,
            "F.SilkS" | "F.Silkscreen" if profile.aliases.silk.contains(&name.as_str()) => {
                LayerKind::TopSilk
            }
            "B.SilkS" | "B.Silkscreen" if profile.aliases.silk.contains(&name.as_str()) => {
                LayerKind::BottomSilk
            }
            "Edge.Cuts" => LayerKind::Outline,
            "F.Fab" => LayerKind::Fab(Side::Front),
            "B.Fab" => LayerKind::Fab(Side::Back),
            "F.CrtYd" => LayerKind::Courtyard(Side::Front),
            "B.CrtYd" => LayerKind::Courtyard(Side::Back),
            "F.Adhes" => LayerKind::Adhesive(Side::Front),
            "B.Adhes" => LayerKind::Adhesive(Side::Back),
            "Dwgs.User" => LayerKind::Drawings,
            "Cmts.User" => LayerKind::Comments,
            "Eco1.User" | "Eco2.User" => LayerKind::Eco,
            "Margin" => LayerKind::Margin,
            n if n.starts_with("User.") => LayerKind::User,
            _ => {
                return Err(Error::UnknownMaterialLayer {
                    name,
                    ordinal,
                    span: source.span,
                })
            }
        };
        out.push(Layer {
            ordinal,
            internal_name: name,
            layer_type,
            user_name,
            kind,
        });
    }
    Ok(out)
}

fn decode_setup(node: &Node) -> Result<Setup, Error> {
    let items = node.as_list().unwrap_or_default();
    Ok(Setup {
        raw: node.clone(),
        pad_to_mask_clearance_nm: optional_mm(items, "pad_to_mask_clearance")?,
        solder_mask_min_width_nm: optional_mm(items, "solder_mask_min_width")?,
        pad_to_paste_clearance_nm: optional_mm(items, "pad_to_paste_clearance")?,
        pad_to_paste_clearance_ratio: optional_number(items, "pad_to_paste_clearance_ratio")?,
        aux_axis_origin_nm: optional_point(items, "aux_axis_origin")?,
        grid_origin_nm: optional_point(items, "grid_origin")?,
    })
}

fn optional_mm(items: &[Node], name: &str) -> Result<Option<i64>, Error> {
    optional_node(items, name)?
        .map(|n| parse_mm_nm(value(n, 1)?))
        .transpose()
}
fn optional_number(items: &[Node], name: &str) -> Result<Option<f64>, Error> {
    optional_node(items, name)?
        .map(|n| parse_f64(value(n, 1)?))
        .transpose()
}
fn optional_point(items: &[Node], name: &str) -> Result<Option<PointNm>, Error> {
    optional_node(items, name)?
        .map(|n| {
            Ok(PointNm {
                x: parse_mm_nm(value(n, 1)?)?,
                y: parse_mm_nm(value(n, 2)?)?,
            })
        })
        .transpose()
}
fn optional_node<'a>(items: &'a [Node], name: &str) -> Result<Option<&'a Node>, Error> {
    let mut found = items.iter().filter(|n| list_name(n) == Some(name));
    let first = found.next();
    if found.next().is_some() {
        return Err(Error::DuplicateField {
            field: name.into(),
            span: first.unwrap().span,
        });
    }
    Ok(first)
}
fn exactly_one<'a>(items: &'a [Node], name: &str, span: Span) -> Result<&'a Node, Error> {
    optional_node(items, name)?.ok_or_else(|| Error::MissingField {
        field: name.into(),
        span,
    })
}
fn value(node: &Node, index: usize) -> Result<&Node, Error> {
    node.as_list()
        .and_then(|v| v.get(index))
        .ok_or_else(|| invalid(list_name(node).unwrap_or("record"), node.span))
}
fn invalid(record: &str, span: Span) -> Error {
    Error::InvalidRecord {
        record: record.into(),
        span,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Strictness {
    Report,
    Strict,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Disposition {
    Decoded,
    KnownForLater { kind: String },
    IgnoredByRule { rule: String },
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct InventoryRecord {
    pub path: String,
    pub token: String,
    pub disposition: Disposition,
    pub count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inventory {
    pub kind_counts: std::collections::BTreeMap<String, usize>,
    pub unknown_counts: std::collections::BTreeMap<String, usize>,
    pub warnings: Vec<String>,
    pub records: Vec<InventoryRecord>,
    pub consumption: Consumption,
}

const MATERIAL_RECORDS: &[&str] = &[
    "footprint",
    "pad",
    "segment",
    "arc",
    "via",
    "zone",
    "gr_line",
    "gr_arc",
    "gr_circle",
    "gr_rect",
    "gr_poly",
    "gr_curve",
    "gr_text",
    "gr_text_box",
    "dimension",
    "group",
    "image",
    "fp_line",
    "fp_arc",
    "fp_circle",
    "fp_rect",
    "fp_poly",
    "fp_curve",
    "fp_text",
    "fp_text_box",
    "filled_polygon",
];
const DECODED_RECORDS: &[&str] = &[
    "version",
    "generator",
    "generator_version",
    "general",
    "thickness",
    "layers",
    "setup",
    "pad_to_mask_clearance",
    "solder_mask_min_width",
    "pad_to_paste_clearance",
    "pad_to_paste_clearance_ratio",
    "aux_axis_origin",
    "grid_origin",
    "net",
];
const IGNORED_RULES: &[(&str, &str)] = &[
    (
        "paper",
        "worksheet paper selection does not describe board material",
    ),
    (
        "title_block",
        "drawing title metadata does not describe board material",
    ),
    (
        "property",
        "board-level property (text variable) carries no geometry; footprint properties are kept for the text decoder",
    ),
    (
        "embedded_fonts",
        "font embedding flag has no geometry without embedded font data",
    ),
    (
        "pcbplotparams",
        "last-used plot settings are outside native projection policy",
    ),
    (
        "teardrop",
        "generated teardrop parameters are retained for the later pad and track decoder",
    ),
    (
        "teardrops",
        "generated teardrop settings are retained for the later pad and track decoder",
    ),
    (
        "generated",
        "producer-generated marker is metadata retained with its owning object",
    ),
];
const KNOWN_CHILDREN: &[&str] = &[
    "at",
    "start",
    "mid",
    "end",
    "center",
    "size",
    "width",
    "stroke",
    "type",
    "fill",
    "layer",
    "layers",
    "tstamp",
    "uuid",
    "net",
    "net_name",
    "net_tie_pad_groups",
    "attr",
    "effects",
    "font",
    "thickness",
    "justify",
    "hide",
    "locked",
    "unlocked",
    "drill",
    "remove_unused_layers",
    "keep_end_layers",
    "roundrect_rratio",
    "rect_delta",
    "primitives",
    "gr_line",
    "gr_arc",
    "gr_circle",
    "gr_rect",
    "gr_poly",
    "gr_curve",
    "fp_line",
    "fp_arc",
    "fp_circle",
    "fp_rect",
    "fp_poly",
    "fp_curve",
    "fp_text",
    "fp_text_box",
    "pad",
    "zone_connect",
    "solder_mask_margin",
    "solder_paste_margin_ratio",
    "clearance",
    "hatch",
    "connect_pads",
    "min_thickness",
    "filled_areas_thickness",
    "polygon",
    "pts",
    "xy",
    "filled_polygon",
    "thermal_gap",
    "thermal_bridge_width",
    "thermal_bridge_angle",
    "priority",
    "island_removal_mode",
    "island_area_min",
    "smoothing",
    "radius",
    "name",
    "members",
    "model",
    "offset",
    "scale",
    "rotate",
    "xyz",
    "descr",
    "tags",
    "path",
    "sheetname",
    "sheetfile",
    "pinfunction",
    "pintype",
    "free",
    "group",
    "image",
    "dimension",
    "format",
    "style",
    "height",
    "orientation",
    "arrow_length",
    "extension_offset",
    "keep_text_aligned",
    "text_position_mode",
    "exclude_from_pos_files",
    "exclude_from_bom",
    "dnp",
    "fields_autoplaced",
    "layer",
    "property",
    "embedded_fonts",
    "teardrop",
    "teardrops",
    "generated",
    "target_length",
    "target_length_min",
    "target_length_max",
    "target_skew",
    "target_skew_min",
    "target_skew_max",
    "tuning_mode",
    "initial_side",
    "last_tuning",
    "last_track_width",
    "last_diff_pair_gap",
    "last_netname",
    "last_status",
    "tuned",
    "single_sided",
    "rounded",
    "chamfer_ratio",
    "chamfer",
    "options",
    "anchor",
    "bold",
    "face",
    "line_spacing",
    "mirror",
    "knockout",
    "render_cache",
    "group_ids",
    "allow_two_segments",
    "best_length_ratio",
    "best_width_ratio",
    "curved_edges",
    "enabled",
    "filter_ratio",
    "max_length",
    "max_width",
    "prefer_zone_connections",
    "width",
    "length",
    "mode",
    "min_amplitude",
    "max_amplitude",
    "min_spacing",
    "base_line",
    "corner_radius_percent",
];

pub fn inventory(board: &ParsedBoard, strictness: Strictness) -> Result<Inventory, Error> {
    let tables = decode_tables(board)?;
    let mut state = Inventory {
        kind_counts: Default::default(),
        unknown_counts: Default::default(),
        warnings: Vec::new(),
        records: Vec::new(),
        consumption: board.consumption(),
    };
    state.consumption.mark(&board.root);
    let root = board.root.as_list().unwrap_or_default();
    for child in root.iter().skip(1) {
        walk_record(
            child,
            "kicad_pcb",
            false,
            strictness,
            tables.profile.producer_major,
            &mut state,
        )?;
    }
    state.records.sort();
    state.warnings.sort();
    Ok(state)
}

fn walk_record(
    node: &Node,
    parent_path: &str,
    parent_material: bool,
    strictness: Strictness,
    producer_major: u8,
    out: &mut Inventory,
) -> Result<(), Error> {
    mark_tree(&mut out.consumption, node);
    let Some(token) = list_name(node) else {
        return Ok(());
    };
    let path = format!("{parent_path}/{token}");
    let material = MATERIAL_RECORDS.contains(&token);
    let disposition = if DECODED_RECORDS.contains(&token)
        || (parent_path == "kicad_pcb/layers"
            && node
                .as_list()
                .and_then(|v| v.first())
                .and_then(Node::as_atom)
                .is_some_and(|v| v.parse::<i64>().is_ok()))
    {
        Disposition::Decoded
    } else if material || (parent_material && token == "property") {
        Disposition::KnownForLater { kind: token.into() }
    } else if let Some((_, rule)) = IGNORED_RULES.iter().find(|(name, _)| *name == token) {
        Disposition::IgnoredByRule {
            rule: (*rule).into(),
        }
    } else if parent_material && KNOWN_CHILDREN.contains(&token) {
        Disposition::KnownForLater { kind: token.into() }
    } else if parent_path.starts_with("kicad_pcb/setup") {
        Disposition::IgnoredByRule {
            rule: "board setup field retained in the raw setup record for later policy decoding"
                .into(),
        }
    } else {
        *out.unknown_counts
            .entry(format!("{path} [{token}]"))
            .or_insert(0) += 1;
        let warning = format!("unknown record {token} at {path}");
        if parent_material && strictness == Strictness::Strict {
            return Err(Error::UnknownMaterialRecord {
                path,
                token: token.into(),
                span: node.span,
            });
        }
        out.warnings.push(warning);
        Disposition::Unknown
    };
    let count_key = count_key(token);
    if material && !(token == "fp_text" && producer_major >= 9) {
        *out.kind_counts.entry(count_key.into()).or_insert(0) += 1;
    }
    add_record(out, path.clone(), token, disposition);
    if let Some(children) = node.as_list() {
        for child in children.iter().skip(1).filter(|n| n.as_list().is_some()) {
            walk_record(
                child,
                &path,
                parent_material || material,
                strictness,
                producer_major,
                out,
            )?;
        }
    }
    Ok(())
}

fn count_key(token: &str) -> &str {
    match token {
        "segment" => "segments",
        "arc" => "arcs",
        "footprint" => "footprints",
        "pad" => "pads",
        "via" => "vias",
        "zone" => "zones",
        "gr_text" | "fp_text" | "gr_text_box" | "fp_text_box" => "texts",
        "filled_polygon" => "saved_filled_polygons",
        "gr_rect" => "graphic_rectangles",
        "fp_line" => "footprint_lines",
        other => other,
    }
}
fn add_record(out: &mut Inventory, path: String, token: &str, disposition: Disposition) {
    if let Some(row) = out
        .records
        .iter_mut()
        .find(|r| r.path == path && r.token == token && r.disposition == disposition)
    {
        row.count += 1;
    } else {
        out.records.push(InventoryRecord {
            path,
            token: token.into(),
            disposition,
            count: 1,
        });
    }
}
fn mark_tree(consumption: &mut Consumption, node: &Node) {
    consumption.mark(node);
    if let Some(children) = node.as_list() {
        for child in children {
            mark_tree(consumption, child);
        }
    }
}

#[cfg(test)]
mod tests;
