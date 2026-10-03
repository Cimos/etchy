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
    InvalidUtf8 { offset: usize },
    UnterminatedString { span: Span },
    BadEscape { escape: u8, span: Span },
    UnbalancedParens { span: Span },
    EmptyInput,
    TrailingData { span: Span },
    LimitExceeded { limit: LimitKind, span: Span },
    InvalidRoot { span: Span },
    MissingVersion { span: Span },
    Numeric { problem: NumericProblem, span: Span },
    IntegerOverflow { span: Span },
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

#[cfg(test)]
mod tests;
