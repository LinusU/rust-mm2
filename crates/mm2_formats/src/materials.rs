//! Parsers for the authored surface-material tables: `city/materials.mtl`
//! (material property blocks) and `city/materials.csv` (texture → material
//! name map).
//!
//! The `.mtl` file holds one `mtl <name> { ... }` block per surface type
//! carrying `key: value` fields — elasticity, friction, drag, a sound
//! class, a `width`/`height`/`depth` triple and two `ptx*` particle
//! specifiers. `materials.csv` is a two-column table mapping a texture
//! stem to a material name (`none` marks "no named material"). A surface
//! query in the original presumably walks texture name → `materials.csv`
//! → `materials.mtl`; the texture name itself reaches the table from the
//! PSDL texture table, a PKG shader or a bound material (see
//! `docs/research/materials.md`). Which consumer uses which lookup is
//! unverified (UNK-23).
//!
//! Measured on the retail install (2026-09-21): one global pair
//! `city/materials.{csv,mtl}` — no per-city copies exist. The `.mtl`
//! grammar is line-oriented: `mtl name {` opens a block (the brace may
//! sit on the next line), `key: v1 v2 …` lines carry fields (`:` is
//! optional, whitespace otherwise separates), a lone `}` closes, `//`
//! starts a comment. Every retail block carries the same ten fields in
//! authored order: `elasticity`, `friction`, `effect`, `sound`, `drag`,
//! `width`, `height`, `depth`, `ptxindex` (two integers), `ptxthreshold`
//! (two floats). `materials.csv` rows are `name,material` under a
//! `texture,physics` header with no quoting.

use std::fmt;

use crate::FormatError;
use crate::racedata::TableDiagnostic;

/// Field names every retail `mtl` block carries, in authored order.
/// Requiredness is unverified — [`MaterialSet::validate`] reports a
/// missing one as an issue rather than failing the parse.
pub const KNOWN_FIELDS: [&str; 10] = [
    "elasticity",
    "friction",
    "effect",
    "sound",
    "drag",
    "width",
    "height",
    "depth",
    "ptxindex",
    "ptxthreshold",
];

/// The `materials.csv` value meaning "no named material".
pub const NONE_PHYSICS: &str = "none";

/// The fallback material block the retail table defines.
pub const DEFAULT_MATERIAL: &str = "_default";

/// One `key: v1 v2 …` field inside an `mtl` block. Values stay raw —
/// typed accessors on [`MaterialDef`] interpret them on demand.
#[derive(Debug, Clone)]
pub struct MaterialField {
    /// Field name, e.g. `friction`.
    pub name: String,
    /// Raw value tokens after the name (and optional `:`).
    pub values: Vec<String>,
    /// 1-based source line.
    pub line: u32,
}

/// One `mtl <name> { ... }` block.
#[derive(Debug, Clone)]
pub struct MaterialDef {
    /// Material name referenced by `materials.csv` rows (`_default` is
    /// the retail fallback).
    pub name: String,
    /// Fields in authored order; unknown names are preserved verbatim.
    pub fields: Vec<MaterialField>,
    /// 1-based source line of the `mtl` header.
    pub line: u32,
}

impl MaterialDef {
    /// First field with an exact name match.
    pub fn field(&self, name: &str) -> Option<&MaterialField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// First value of the named field parsed as `f32`.
    pub fn f32(&self, name: &str) -> Option<f32> {
        self.field(name)?.values.first()?.parse().ok()
    }

    /// First `n` values of the named field parsed as `f32`.
    pub fn vec_f32(&self, name: &str, n: usize) -> Option<Vec<f32>> {
        let f = self.field(name)?;
        if f.values.len() < n {
            return None;
        }
        f.values[..n].iter().map(|v| v.parse().ok()).collect()
    }

    /// First `n` values of the named field parsed as `i64`.
    pub fn vec_i64(&self, name: &str, n: usize) -> Option<Vec<i64>> {
        let f = self.field(name)?;
        if f.values.len() < n {
            return None;
        }
        f.values[..n].iter().map(|v| v.parse().ok()).collect()
    }

    /// Raw first value of the named field (for word values like
    /// `effect: none`).
    pub fn text(&self, name: &str) -> Option<&str> {
        self.field(name)?.values.first().map(String::as_str)
    }
}

/// A parsed `.mtl` file: material definitions in authored order.
#[derive(Debug, Clone)]
pub struct MaterialSet {
    /// One entry per `mtl` block, in file order. Position in this list
    /// is the natural authored index space for a runtime surface id.
    pub defs: Vec<MaterialDef>,
}

/// A consistency problem in a parsed material table.
#[derive(Debug, Clone, PartialEq)]
pub enum MaterialIssue {
    /// Two `mtl` blocks share a name.
    DuplicateDef {
        /// Shared name.
        name: String,
        /// Line of the later block.
        line: u32,
    },
    /// No `_default` block — the retail table's fallback.
    MissingDefault,
    /// A known field is absent from a block (all ten are present on
    /// every retail block).
    MissingField {
        /// Material name.
        material: String,
        /// Absent field name.
        field: &'static str,
        /// Line of the `mtl` header.
        line: u32,
    },
    /// A known field carries the wrong shape — non-numeric where a
    /// number is expected, or the wrong value count for `ptxindex` /
    /// `ptxthreshold`.
    BadFieldValue {
        /// Material name.
        material: String,
        /// Field name.
        field: &'static str,
        /// Line of the field.
        line: u32,
    },
    /// A field outside the ten-name retail set.
    UnknownField {
        /// Material name.
        material: String,
        /// Unrecognized field name.
        field: String,
        /// Line of the field.
        line: u32,
    },
    /// A physical quantity is negative.
    NegativeValue {
        /// Material name.
        material: String,
        /// Field name.
        field: &'static str,
        /// Line of the field.
        line: u32,
        /// Offending value.
        value: f32,
    },
    /// Two `materials.csv` rows name the same texture.
    DuplicateMapRow {
        /// Texture name.
        texture: String,
        /// Line of the later row.
        line: u32,
    },
    /// The `materials.csv` header does not start `texture,physics`.
    BadMapHeader {
        /// Recorded header cells.
        header: Vec<String>,
    },
}

impl fmt::Display for MaterialIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaterialIssue::DuplicateDef { name, line } => {
                write!(f, "line {line}: duplicate material {name:?}")
            }
            MaterialIssue::MissingDefault => write!(f, "no {DEFAULT_MATERIAL:?} material defined"),
            MaterialIssue::MissingField {
                material,
                field,
                line,
            } => write!(
                f,
                "line {line}: material {material:?} lacks field {field:?}"
            ),
            MaterialIssue::BadFieldValue {
                material,
                field,
                line,
            } => write!(
                f,
                "line {line}: material {material:?} field {field:?} has a bad value"
            ),
            MaterialIssue::UnknownField {
                material,
                field,
                line,
            } => write!(
                f,
                "line {line}: material {material:?} unknown field {field:?}"
            ),
            MaterialIssue::NegativeValue {
                material,
                field,
                line,
                value,
            } => write!(
                f,
                "line {line}: material {material:?} {field} = {value} < 0"
            ),
            MaterialIssue::DuplicateMapRow { texture, line } => {
                write!(
                    f,
                    "line {line}: duplicate materials.csv row for {texture:?}"
                )
            }
            MaterialIssue::BadMapHeader { header } => write!(
                f,
                "unexpected materials.csv header: expected (texture,physics), have ({})",
                header.join(",")
            ),
        }
    }
}

impl MaterialSet {
    /// Parse a `.mtl` file. The whole file must be a sequence of `mtl
    /// <name> { ... }` blocks; anything else is a `FormatError`.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut defs = Vec::new();
        // (line number, comment-stripped text) for every non-blank line.
        let lines: Vec<(u32, String)> = input
            .lines()
            .enumerate()
            .map(|(i, l)| {
                (
                    (i + 1) as u32,
                    l.split("//").next().unwrap_or("").to_string(),
                )
            })
            .filter(|(_, l)| !l.trim().is_empty())
            .collect();
        let mut i = 0usize;
        while i < lines.len() {
            let (line_no, text) = &lines[i];
            let words: Vec<&str> = text.split_whitespace().collect();
            if words.first() != Some(&"mtl") || words.len() > 3 {
                return Err(FormatError::parse(
                    0,
                    format!("line {line_no}: expected 'mtl <name> {{', found {text:?}"),
                ));
            }
            let (name, mut consumed) = match words.as_slice() {
                ["mtl", name, "{"] => ((*name).to_string(), i + 1),
                ["mtl", "{", ..] => {
                    return Err(FormatError::parse(
                        0,
                        format!("line {line_no}: 'mtl' header needs a name before '{{'"),
                    ));
                }
                ["mtl", name] => ((*name).to_string(), i + 1),
                _ => {
                    return Err(FormatError::parse(
                        0,
                        format!("line {line_no}: malformed 'mtl' header {text:?}"),
                    ));
                }
            };
            // The '{' may sit on its own next line.
            if !text.contains('{') {
                match lines.get(consumed) {
                    Some((_, t)) if t.trim() == "{" => consumed += 1,
                    _ => {
                        return Err(FormatError::parse(
                            0,
                            format!("line {line_no}: expected '{{' after 'mtl {name}'"),
                        ));
                    }
                }
            }
            let mut fields = Vec::new();
            let mut closed = false;
            while consumed < lines.len() {
                let (fl, ftext) = &lines[consumed];
                consumed += 1;
                let ftext = ftext.trim();
                if ftext == "}" {
                    closed = true;
                    break;
                }
                let (fname, values) = match ftext.split_once(':') {
                    Some((k, rest)) => (
                        k.trim().to_string(),
                        rest.split_whitespace().map(str::to_string).collect(),
                    ),
                    None => {
                        let mut it = ftext.split_whitespace();
                        let Some(k) = it.next() else { continue };
                        (
                            k.to_string(),
                            it.map(str::to_string).collect::<Vec<String>>(),
                        )
                    }
                };
                if fname.is_empty() {
                    return Err(FormatError::parse(
                        0,
                        format!("line {fl}: field with no name"),
                    ));
                }
                fields.push(MaterialField {
                    name: fname,
                    values,
                    line: *fl,
                });
            }
            if !closed {
                return Err(FormatError::parse(
                    0,
                    format!("line {line_no}: unterminated 'mtl {name}' block"),
                ));
            }
            defs.push(MaterialDef {
                name,
                fields,
                line: *line_no,
            });
            i = consumed;
        }
        Ok(MaterialSet { defs })
    }

    /// Position of a material by name — the authored index space.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.defs.iter().position(|d| d.name == name)
    }

    /// The `_default` block, when present.
    pub fn default_def(&self) -> Option<&MaterialDef> {
        self.defs.iter().find(|d| d.name == DEFAULT_MATERIAL)
    }

    /// Internal consistency checks (cross-references to `materials.csv`
    /// and the VFS belong to the audit layer).
    pub fn validate(&self) -> Vec<MaterialIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for def in &self.defs {
            if !seen.insert(def.name.as_str()) {
                issues.push(MaterialIssue::DuplicateDef {
                    name: def.name.clone(),
                    line: def.line,
                });
            }
            for &known in &KNOWN_FIELDS {
                match def.field(known) {
                    None => issues.push(MaterialIssue::MissingField {
                        material: def.name.clone(),
                        field: known,
                        line: def.line,
                    }),
                    Some(f) => {
                        let bad = match known {
                            "effect" => f.values.len() != 1,
                            "sound" => def.vec_i64(known, 1).is_none() || f.values.len() != 1,
                            "ptxindex" | "ptxthreshold" => {
                                f.values.len() != 2
                                    || f.values.iter().any(|v| v.parse::<f64>().is_err())
                            }
                            _ => f.values.len() != 1 || def.f32(known).is_none(),
                        };
                        if bad {
                            issues.push(MaterialIssue::BadFieldValue {
                                material: def.name.clone(),
                                field: known,
                                line: f.line,
                            });
                            continue;
                        }
                        if matches!(
                            known,
                            "elasticity" | "friction" | "drag" | "width" | "height" | "depth"
                        ) {
                            let v = def.f32(known).unwrap_or(0.0);
                            if v < 0.0 {
                                issues.push(MaterialIssue::NegativeValue {
                                    material: def.name.clone(),
                                    field: known,
                                    line: f.line,
                                    value: v,
                                });
                            }
                        }
                    }
                }
            }
            for f in &def.fields {
                if !KNOWN_FIELDS.contains(&f.name.as_str()) {
                    issues.push(MaterialIssue::UnknownField {
                        material: def.name.clone(),
                        field: f.name.clone(),
                        line: f.line,
                    });
                }
            }
        }
        if self.default_def().is_none() {
            issues.push(MaterialIssue::MissingDefault);
        }
        issues
    }
}

/// One `materials.csv` row: texture stem → material name.
#[derive(Debug, Clone)]
pub struct MaterialRow {
    /// Texture stem as authored (no extension, e.g. `r1_l`, `s_ocean`).
    pub texture: String,
    /// Material name, or the `none` keyword for "no named material".
    pub physics: String,
    /// 1-based source line.
    pub line: u32,
}

impl MaterialRow {
    /// Whether this row names the `none` keyword rather than a material.
    pub fn is_none(&self) -> bool {
        self.physics == NONE_PHYSICS
    }
}

/// A parsed `materials.csv` table.
#[derive(Debug, Clone)]
pub struct MaterialMap {
    /// The header's first two labels (`texture`,`physics` on retail).
    pub header: [String; 2],
    /// One entry per row, in authored order.
    pub rows: Vec<MaterialRow>,
    /// Recoverable problems (malformed rows).
    pub diagnostics: Vec<TableDiagnostic>,
}

impl MaterialMap {
    /// Parse a `materials.csv` table. The first non-blank line must hold
    /// at least two cells (labels are recorded, not enforced —
    /// [`MaterialMap::validate`] flags a nonstandard header). Rows with
    /// fewer than two cells are skipped into `diagnostics`; extra cells
    /// are dropped with a diagnostic.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let Some(header_line) = input.lines().map(str::trim).find(|l| !l.is_empty()) else {
            return Err(FormatError::parse(0, "empty materials.csv table"));
        };
        let hcells: Vec<&str> = header_line.split(',').map(str::trim).collect();
        if hcells.len() < 2 {
            return Err(FormatError::parse(
                0,
                format!("unexpected materials.csv header: {header_line:?}"),
            ));
        }
        let header = [hcells[0].to_string(), hcells[1].to_string()];

        let mut rows = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen_header = false;
        for (idx, raw) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true;
                continue;
            }
            let cells: Vec<&str> = trimmed.split(',').map(str::trim).collect();
            if cells.len() < 2 || cells[0].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("skipping row: expected 2 cells, have {}", cells.len()),
                });
                continue;
            }
            if cells.len() > 2 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("dropping {} extra cell(s)", cells.len() - 2),
                });
            }
            if cells[1].is_empty() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: "empty physics cell".into(),
                });
            }
            rows.push(MaterialRow {
                texture: cells[0].to_string(),
                physics: cells[1].to_string(),
                line,
            });
        }
        Ok(MaterialMap {
            header,
            rows,
            diagnostics,
        })
    }

    /// The physics name a texture stem maps to, if a row exists.
    pub fn lookup(&self, texture: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|r| r.texture == texture)
            .map(|r| r.physics.as_str())
    }

    /// Rows whose `physics` value is neither `none` nor a name defined
    /// in `set` — the audit reports these as dead references.
    pub fn undefined_refs<'a>(&'a self, set: &'a MaterialSet) -> Vec<&'a MaterialRow> {
        self.rows
            .iter()
            .filter(|r| !r.is_none() && set.index(&r.physics).is_none())
            .collect()
    }

    /// Internal consistency checks.
    pub fn validate(&self) -> Vec<MaterialIssue> {
        let mut issues = Vec::new();
        if self.header != ["texture", "physics"] {
            issues.push(MaterialIssue::BadMapHeader {
                header: self.header.to_vec(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for row in &self.rows {
            if !seen.insert(row.texture.as_str()) {
                issues.push(MaterialIssue::DuplicateMapRow {
                    texture: row.texture.clone(),
                    line: row.line,
                });
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MTL: &str = "\
// a comment line
mtl deepwater {
    elasticity: 0.500000
    friction: 0.650000
    effect: none
    sound: 0
    drag: 0.500000
    width: 0.550000
    height: 0.060000
    depth: 100.0000
    ptxindex: -1 -1
    ptxthreshold: 0.25 0.5
}
mtl _default
{
    elasticity: 0.900000
    friction 0.900000
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 4 -1
    ptxthreshold: 0.25 0.5
}
";

    #[test]
    fn parses_mtl_blocks() {
        let set = MaterialSet::parse(MTL).unwrap();
        assert_eq!(set.defs.len(), 2);
        assert_eq!(set.defs[0].name, "deepwater");
        assert_eq!(set.defs[0].line, 2);
        assert_eq!(set.defs[0].f32("friction"), Some(0.65));
        assert_eq!(set.defs[0].text("effect"), Some("none"));
        assert_eq!(set.defs[0].vec_i64("ptxindex", 2), Some(vec![-1, -1]));
        // second block: brace on next line, colon-less field
        assert_eq!(set.defs[1].name, "_default");
        assert_eq!(set.defs[1].f32("friction"), Some(0.9));
        assert_eq!(set.index("_default"), Some(1));
        assert_eq!(set.index("bogus"), None);
        assert!(set.validate().is_empty());
    }

    #[test]
    fn rejects_garbage_outside_blocks() {
        assert!(MaterialSet::parse("hello world").is_err());
        assert!(MaterialSet::parse("mtl {").is_err());
        assert!(MaterialSet::parse("mtl x {\nfriction: 1\n").is_err()); // unterminated
        assert!(MaterialSet::parse("mtl x\nfriction: 1").is_err()); // no brace
    }

    #[test]
    fn validate_reports_def_problems() {
        let input = "\
mtl foo {
    elasticity: -1.0
    friction: 0.9
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 1
    ptxthreshold: 0.25 0.5
    sparkle: yes
}
mtl foo {
    elasticity: 0.1
    friction: 0.2
    effect: none
    sound: 0
    drag: 0.0
    width: 0.0
    height: 0.0
    depth: 0.0
    ptxindex: 0 0
    ptxthreshold: 0.1 0.2
}
";
        let set = MaterialSet::parse(input).unwrap();
        let issues = set.validate();
        assert!(issues.contains(&MaterialIssue::DuplicateDef {
            name: "foo".into(),
            line: 14,
        }));
        assert!(issues.contains(&MaterialIssue::MissingDefault));
        assert!(issues.iter().any(|i| matches!(
            i,
            MaterialIssue::NegativeValue {
                field: "elasticity",
                ..
            }
        )));
        assert!(issues.iter().any(|i| matches!(
            i,
            MaterialIssue::BadFieldValue {
                field: "ptxindex",
                ..
            }
        )));
        assert!(issues.iter().any(|i| matches!(
            i,
            MaterialIssue::UnknownField { field, .. } if field == "sparkle"
        )));
        // every field present but `sound` missing on block two? no —
        // both blocks carry all known fields here
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i, MaterialIssue::MissingField { .. }))
        );
    }

    #[test]
    fn validate_reports_missing_field() {
        let input = "mtl _default {\n elasticity: 0.9\n}\n";
        let set = MaterialSet::parse(input).unwrap();
        let issues = set.validate();
        // nine of the ten known fields are absent
        assert_eq!(
            issues
                .iter()
                .filter(|i| matches!(i, MaterialIssue::MissingField { .. }))
                .count(),
            9
        );
    }

    const CSV: &str = "\
texture,physics
r1_l,cobblestone
s_ocean,deepwater
vp4x4_mud_sd,none
bad_row
r2_l,cobblestone,junk
";

    #[test]
    fn parses_csv_map() {
        let map = MaterialMap::parse(CSV).unwrap();
        assert_eq!(map.header, ["texture", "physics"]);
        assert_eq!(map.rows.len(), 4);
        assert_eq!(map.lookup("r1_l"), Some("cobblestone"));
        assert_eq!(map.lookup("s_ocean"), Some("deepwater"));
        assert!(map.rows[2].is_none());
        assert_eq!(map.lookup("missing"), None);
        assert_eq!(map.diagnostics.len(), 2); // short row + extra cell
        assert!(map.validate().is_empty());
    }

    #[test]
    fn csv_duplicate_and_header_issues() {
        let map = MaterialMap::parse("tex,phys\na,none\na,grass\n").unwrap();
        let issues = map.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, MaterialIssue::BadMapHeader { .. }))
        );
        assert!(issues.iter().any(|i| matches!(
            i,
            MaterialIssue::DuplicateMapRow { texture, .. } if texture == "a"
        )));
    }

    #[test]
    fn undefined_refs_ignores_none() {
        let set = MaterialSet::parse(MTL).unwrap();
        let map = MaterialMap::parse(CSV).unwrap();
        let undef = map.undefined_refs(&set);
        // `deepwater` is defined; cobblestone is not in the synthetic set
        assert_eq!(undef.len(), 2);
        assert!(undef.iter().all(|r| r.physics == "cobblestone"));
    }
}
