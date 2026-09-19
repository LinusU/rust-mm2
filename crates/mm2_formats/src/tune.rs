//! Tokenizer and parser for Angel Studios' block-structured tuning text
//! (`.vehcarsim`, `.vehtrailer`, `.vehgyro`, `.vehstuck`, `.vehcardamage`,
//! `.asnode`, `.dgtrailerjoint`, ...).
//!
//! These files look like:
//!
//! ```text
//! type: a
//! vehCarSim {
//!   Mass 1000.000000
//!   Engine {
//!     MaxHorsePower 260.000000
//!   }
//! }
//! ```
//!
//! The parser is intentionally generic: it produces a name/value tree that
//! preserves repeated keys, nested blocks, unknown fields, and source
//! positions so higher layers can report precise diagnostics. Fields occupy
//! a single line; a name followed by `{` (same or next line) opens a block.

use std::fmt;

/// Maximum supported block nesting depth.
const MAX_DEPTH: usize = 32;

/// A parsed tuning file.
#[derive(Debug, Clone)]
pub struct TuneFile {
    /// Value of the leading `type:` header, when present (e.g. `a`).
    pub type_tag: Option<String>,
    /// Root block, e.g. `vehCarSim { ... }`.
    pub root: TuneBlock,
}

/// A named `{ ... }` block.
#[derive(Debug, Clone)]
pub struct TuneBlock {
    pub name: String,
    pub entries: Vec<TuneEntry>,
    pub line: u32,
    pub col: u32,
}

/// One entry inside a block: either a field line or a nested block.
#[derive(Debug, Clone)]
pub enum TuneEntry {
    Field(TuneField),
    Block(TuneBlock),
}

/// `Name value1 value2 ...` on one line.
#[derive(Debug, Clone)]
pub struct TuneField {
    pub name: String,
    pub values: Vec<TuneValue>,
    pub line: u32,
    pub col: u32,
}

/// A single field value with its raw text preserved.
#[derive(Debug, Clone)]
pub struct TuneValue {
    pub raw: String,
    /// `Some` when the token parses as an `f64` number.
    pub number: Option<f64>,
}

/// Parse error with source position.
#[derive(Debug)]
pub struct TuneError {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl fmt::Display for TuneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for TuneError {}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word,
    OpenBrace,
    CloseBrace,
    Colon,
}

#[derive(Debug, Clone)]
struct Token {
    kind: Tok,
    text: String,
    line: u32,
    col: u32,
}

fn tokenize(input: &str) -> Result<Vec<Token>, TuneError> {
    let mut toks = Vec::new();
    let mut line = 1u32;
    let mut col = 1u32;
    let mut chars = input.chars().peekable();

    macro_rules! advance {
        () => {{
            let c = chars.next();
            if let Some(c) = c {
                if c == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
            }
            c
        }};
    }

    loop {
        // Skip whitespace.
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            advance!();
        }
        let Some(&c) = chars.peek() else { break };
        let (tok_line, tok_col) = (line, col);
        match c {
            '{' => {
                advance!();
                toks.push(Token {
                    kind: Tok::OpenBrace,
                    text: "{".into(),
                    line: tok_line,
                    col: tok_col,
                });
            }
            '}' => {
                advance!();
                toks.push(Token {
                    kind: Tok::CloseBrace,
                    text: "}".into(),
                    line: tok_line,
                    col: tok_col,
                });
            }
            ':' => {
                advance!();
                toks.push(Token {
                    kind: Tok::Colon,
                    text: ":".into(),
                    line: tok_line,
                    col: tok_col,
                });
            }
            '/' if matches!(chars.clone().nth(1), Some('/')) => {
                // `//` comment to end of line.
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    advance!();
                }
            }
            '"' => {
                // Quoted string.
                advance!();
                let mut text = String::new();
                loop {
                    match advance!() {
                        Some('"') | None => break,
                        Some('\\') => {
                            if let Some(esc) = advance!() {
                                text.push(esc);
                            }
                        }
                        Some(ch) => text.push(ch),
                    }
                }
                toks.push(Token {
                    kind: Tok::Word,
                    text,
                    line: tok_line,
                    col: tok_col,
                });
            }
            _ => {
                let mut text = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | ':' | '"') {
                        break;
                    }
                    if c == '/' && matches!(chars.clone().nth(1), Some('/')) {
                        break;
                    }
                    text.push(c);
                    advance!();
                }
                toks.push(Token {
                    kind: Tok::Word,
                    text,
                    line: tok_line,
                    col: tok_col,
                });
            }
        }
    }
    Ok(toks)
}

impl TuneFile {
    /// Parse a tuning file. `input` should already be decoded to UTF-8
    /// (callers typically use `String::from_utf8_lossy`).
    pub fn parse(input: &str) -> Result<Self, TuneError> {
        let toks = tokenize(input)?;
        let mut pos = 0usize;

        let mut type_tag = None;
        if toks.len() >= 3
            && toks[0].kind == Tok::Word
            && toks[0].text.eq_ignore_ascii_case("type")
            && toks[1].kind == Tok::Colon
            && toks[2].kind == Tok::Word
        {
            type_tag = Some(toks[2].text.clone());
            pos = 3;
        }

        let mut depth = 0usize;
        let root = parse_block(&toks, &mut pos, &mut depth)?;

        // Trailing tokens after the root block are only tolerated if empty.
        if let Some(t) = toks.get(pos) {
            return Err(TuneError {
                line: t.line,
                col: t.col,
                message: format!("unexpected token {:?} after root block", t.text),
            });
        }

        Ok(TuneFile { type_tag, root })
    }
}

fn parse_block(toks: &[Token], pos: &mut usize, depth: &mut usize) -> Result<TuneBlock, TuneError> {
    if *depth >= MAX_DEPTH {
        let (line, col) = toks.get(*pos).map(|t| (t.line, t.col)).unwrap_or((0, 0));
        return Err(TuneError {
            line,
            col,
            message: format!("nesting deeper than {MAX_DEPTH} blocks"),
        });
    }

    // Expect: Name '{'
    let name_tok = toks.get(*pos).ok_or(TuneError {
        line: 0,
        col: 0,
        message: "expected block name, found end of file".into(),
    })?;
    if name_tok.kind != Tok::Word {
        return Err(TuneError {
            line: name_tok.line,
            col: name_tok.col,
            message: format!("expected block name, found {:?}", name_tok.text),
        });
    }
    *pos += 1;
    match toks.get(*pos) {
        Some(t) if t.kind == Tok::OpenBrace => {
            *pos += 1;
        }
        Some(t) => {
            return Err(TuneError {
                line: t.line,
                col: t.col,
                message: format!(
                    "expected '{{' after block name {:?}, found {:?}",
                    name_tok.text, t.text
                ),
            });
        }
        None => {
            return Err(TuneError {
                line: name_tok.line,
                col: name_tok.col,
                message: format!("expected '{{' after block name {:?}", name_tok.text),
            });
        }
    }
    parse_block_body(toks, pos, depth, name_tok)
}

/// Parse a block whose `{` token is at `brace_idx` (used when the header
/// carries extra qualifier tokens between the name and the brace).
fn parse_block_ex(
    toks: &[Token],
    pos: &mut usize,
    depth: &mut usize,
    brace_idx: usize,
) -> Result<TuneBlock, TuneError> {
    if *depth >= MAX_DEPTH {
        let (line, col) = toks.get(*pos).map(|t| (t.line, t.col)).unwrap_or((0, 0));
        return Err(TuneError {
            line,
            col,
            message: format!("nesting deeper than {MAX_DEPTH} blocks"),
        });
    }
    let name_tok = toks[*pos].clone();
    *pos = brace_idx + 1;
    parse_block_body(toks, pos, depth, &name_tok)
}

fn parse_block_body(
    toks: &[Token],
    pos: &mut usize,
    depth: &mut usize,
    name_tok: &Token,
) -> Result<TuneBlock, TuneError> {
    let mut block = TuneBlock {
        name: name_tok.text.clone(),
        entries: Vec::new(),
        line: name_tok.line,
        col: name_tok.col,
    };

    *depth += 1;
    loop {
        let Some(tok) = toks.get(*pos) else {
            return Err(TuneError {
                line: name_tok.line,
                col: name_tok.col,
                message: format!("unterminated block {:?} opened here", name_tok.text),
            });
        };
        match tok.kind {
            Tok::CloseBrace => {
                *pos += 1;
                break;
            }
            Tok::Word => {
                // Gather the rest of the tokens on this line. Two shapes:
                //   * field:  `Name v1 v2 ...`         (no trailing '{')
                //   * block:  `Name [qualifiers] {`    ('{' last on the line)
                // A bare `Name` line followed by `{` on the next line is also
                // a block. Qualifier words (e.g. `Aero asAero :075abc8c {`
                // in the opponent-variant dialect) are preserved as the
                // block's trailing field values for diagnostics.
                let start_line = tok.line;
                let mut line_end = *pos + 1;
                while let Some(t) = toks.get(line_end) {
                    if t.line != start_line {
                        break;
                    }
                    line_end += 1;
                }
                let same_line = &toks[*pos..line_end];
                let brace_same_line = same_line
                    .last()
                    .map(|t| t.kind == Tok::OpenBrace)
                    .unwrap_or(false);
                let brace_next_line = same_line.len() == 1
                    && toks
                        .get(line_end)
                        .map(|t| t.kind == Tok::OpenBrace)
                        .unwrap_or(false);

                if brace_same_line || brace_next_line {
                    // Rewind to the block-name position and parse the block
                    // (header words between the name and '{' are skipped by
                    // parse_block since it only reads name + '{').
                    let inner = parse_block_ex(
                        toks,
                        pos,
                        depth,
                        if brace_same_line {
                            line_end - 1
                        } else {
                            line_end
                        },
                    )?;
                    block.entries.push(TuneEntry::Block(inner));
                } else {
                    let name_tok = tok.clone();
                    *pos += 1;
                    let mut values = Vec::new();
                    while let Some(v) = toks.get(*pos) {
                        if v.line != name_tok.line || v.kind == Tok::CloseBrace {
                            break;
                        }
                        values.push(TuneValue {
                            number: if v.kind == Tok::Word {
                                v.text.parse::<f64>().ok()
                            } else {
                                None
                            },
                            raw: v.text.clone(),
                        });
                        *pos += 1;
                    }
                    block.entries.push(TuneEntry::Field(TuneField {
                        name: name_tok.text,
                        values,
                        line: name_tok.line,
                        col: name_tok.col,
                    }));
                }
            }
            _ => {
                return Err(TuneError {
                    line: tok.line,
                    col: tok.col,
                    message: format!("unexpected token {:?} in block", tok.text),
                });
            }
        }
    }
    *depth -= 1;
    Ok(block)
}

impl TuneBlock {
    /// First field with an exact name match.
    pub fn field(&self, name: &str) -> Option<&TuneField> {
        self.entries.iter().find_map(|e| match e {
            TuneEntry::Field(f) if f.name == name => Some(f),
            _ => None,
        })
    }

    /// First field with a case-insensitive name match.
    pub fn field_ci(&self, name: &str) -> Option<&TuneField> {
        self.entries.iter().find_map(|e| match e {
            TuneEntry::Field(f) if f.name.eq_ignore_ascii_case(name) => Some(f),
            _ => None,
        })
    }

    /// First nested block with an exact name match.
    pub fn block(&self, name: &str) -> Option<&TuneBlock> {
        self.entries.iter().find_map(|e| match e {
            TuneEntry::Block(b) if b.name == name => Some(b),
            _ => None,
        })
    }

    /// All nested blocks with an exact name match.
    pub fn blocks(&self, name: &str) -> impl Iterator<Item = &TuneBlock> {
        self.entries.iter().filter_map(move |e| match e {
            TuneEntry::Block(b) if b.name == name => Some(b),
            _ => None,
        })
    }

    /// All field names present (for unknown-field diagnostics).
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|e| match e {
            TuneEntry::Field(f) => Some(f.name.as_str()),
            _ => None,
        })
    }

    /// All nested block names present.
    pub fn block_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|e| match e {
            TuneEntry::Block(b) => Some(b.name.as_str()),
            _ => None,
        })
    }

    /// First value of the named field parsed as `f32`.
    pub fn f32(&self, name: &str) -> Option<f32> {
        self.field(name)?.values.first()?.number.map(|n| n as f32)
    }

    /// First `n` values of the named field parsed as `f32`.
    pub fn vec_f32(&self, name: &str, n: usize) -> Option<Vec<f32>> {
        let f = self.field(name)?;
        if f.values.len() < n {
            return None;
        }
        f.values[..n]
            .iter()
            .map(|v| v.number.map(|x| x as f32))
            .collect()
    }

    /// Three-component float vector field.
    pub fn vec3(&self, name: &str) -> Option<[f32; 3]> {
        let v = self.vec_f32(name, 3)?;
        Some([v[0], v[1], v[2]])
    }
}
