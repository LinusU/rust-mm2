//! Parsers for the `anim/` pedestrian-rig corpus.
//!
//! Each `anim/pedmodel_*` archetype assembles from several formats:
//!
//! - `.skel` — ASCII skeleton: a `NumBones <n>` header then a recursive
//!   `bone <name> { offset <x> <y> <z> … }` hierarchy. Retail ships one
//!   19-bone humanoid rig per `pedmodel_{man,manw,woman,womanw}` plus a
//!   26-bone `pedmodel_wolf` rig with no companion files.
//! - `.csv` — ASCII state model: `#`-comment header, then rows of
//!   `state,clip,first_frame,last_frame,y_offset,y_distance,x_offset,
//!   x_distance,next` (9 cells). Column names come from the authored
//!   header comment (`anim name,mma name,first frame,…`); the offset /
//!   distance semantics are unrecovered and the values are preserved
//!   verbatim.
//! - `.remap` — ASCII bone remap: a count line then that many integer
//!   indices. Retail ships exactly one (`pedmodel_woman.remap`, 17
//!   entries); its purpose is unknown — preserved verbatim.
//! - `.rays` — ASCII: a count line, `count` rows of `<f32 f32 f32 i32
//!   i32>`, then a grid of integer rows (48 on man/woman, 24 on
//!   manw/womanw, `count` columns each). Purpose unknown — preserved
//!   verbatim.
//! - `.anim` — binary clip: `u32` reserved (0), `u32` frame count,
//!   `u32` floats per frame, `f32` motion hint, `u8` kind (1), then
//!   `frames * floats_per_frame` little-endian `f32` samples. Every
//!   retail clip carries 60 floats per frame — 20 XYZ triples against
//!   the 19-bone rigs (channel order and the extra triple are
//!   unrecovered). The header float tracks the state table's authored
//!   Y-axis travel on several clips (man walk ≈ 1.55, run ≈ 2.97;
//!   woman walk matches `1.087` exactly) — a plausible locomotion-speed
//!   hint, but its exact semantics are unknown.
//!
//! `pedmodel_*.shaders` is a standalone copy of the PKG shader-chunk
//! grammar — parse it with [`crate::pkg::PkgShaders::parse`]. The large
//! ASCII `.mod` meshes are not decoded yet (F19-A.2).

use crate::racedata::TableDiagnostic;
use crate::{FormatError, Reader};

/// Hard bound on `frames * floats_per_frame` so a hostile header cannot
/// force a giant allocation. The largest retail clip stores 3,180 floats.
const MAX_ANIM_FLOATS: usize = 1 << 20;

/// Bound on `.skel` nesting depth — retail rigs nest at most 5 deep.
const MAX_BONE_DEPTH: usize = 64;

/// A parsed `.skel` skeleton.
#[derive(Debug, Clone)]
pub struct PedSkel {
    /// `NumBones` header value, verbatim.
    pub declared_bones: i64,
    /// Top-level bones — a single `root` on every retail file.
    pub roots: Vec<PedBone>,
    /// Non-fatal parse problems (unknown directives, unclosed blocks).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One bone in a [`PedSkel`] hierarchy.
#[derive(Debug, Clone)]
pub struct PedBone {
    /// Bone name (`root`, `spine`, `wrist_l`, …).
    pub name: String,
    /// `offset` line — authored local translation relative to the
    /// parent bone (bind-pose semantics inferred from the tree shape).
    pub offset: [f32; 3],
    /// Nested bones.
    pub children: Vec<PedBone>,
    /// 1-based line where this `bone` record opened.
    pub line: u32,
}

impl PedSkel {
    /// Parse a `.skel` file. Malformed records degrade to diagnostics;
    /// only a missing/garbled `NumBones` header is fatal.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut diagnostics = Vec::new();
        let mut lines = input
            .lines()
            .enumerate()
            .map(|(i, l)| (i as u32 + 1, l.trim()))
            .peekable();
        let declared_bones = loop {
            let Some((ln, text)) = lines.next() else {
                return Err(FormatError::parse(0, "empty .skel file"));
            };
            if text.is_empty() {
                continue;
            }
            let mut t = text.split_whitespace();
            match t.next() {
                Some("NumBones") => {
                    let n = t
                        .next()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or_else(|| {
                            FormatError::parse(0, "NumBones header has no integer value")
                        })?;
                    if t.next().is_some() {
                        diagnostics.push(TableDiagnostic {
                            line: ln,
                            message: "trailing tokens after NumBones value".into(),
                        });
                    }
                    break n;
                }
                _ => {
                    return Err(FormatError::parse(
                        0,
                        format!("expected `NumBones <n>` header, found {text:?}"),
                    ));
                }
            }
        };
        let mut roots = Vec::new();
        while let Some((ln, text)) = lines.next() {
            if text.is_empty() {
                continue;
            }
            let mut t = text.split_whitespace();
            match t.next() {
                Some("bone") => roots.push(parse_bone(&mut lines, &mut t, ln, &mut diagnostics, 0)),
                Some("}") => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: "unbalanced `}` at top level".into(),
                }),
                Some(other) => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("unexpected top-level directive {other:?}"),
                }),
                None => {}
            }
        }
        Ok(PedSkel {
            declared_bones,
            roots,
            diagnostics,
        })
    }

    /// Total bones in the hierarchy.
    pub fn bone_count(&self) -> usize {
        fn count(b: &PedBone) -> usize {
            1 + b.children.iter().map(count).sum::<usize>()
        }
        self.roots.iter().map(count).sum()
    }

    /// Pre-order traversal of `(depth, bone)` pairs — the order the
    /// hierarchy is written in.
    pub fn flatten(&self) -> Vec<(usize, &PedBone)> {
        fn walk<'b>(b: &'b PedBone, depth: usize, out: &mut Vec<(usize, &'b PedBone)>) {
            out.push((depth, b));
            for c in &b.children {
                walk(c, depth + 1, out);
            }
        }
        let mut out = Vec::with_capacity(self.bone_count());
        for r in &self.roots {
            walk(r, 0, &mut out);
        }
        out
    }

    /// Consistency checks beyond parse diagnostics: declared-vs-actual
    /// bone count, duplicate names, non-finite offsets.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        let flat = self.flatten();
        if self.declared_bones != flat.len() as i64 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "NumBones {} does not match {} parsed bone(s)",
                    self.declared_bones,
                    flat.len()
                ),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for (_, b) in &flat {
            if !seen.insert(&b.name) {
                out.push(TableDiagnostic {
                    line: b.line,
                    message: format!("duplicate bone name {:?}", b.name),
                });
            }
            if b.offset.iter().any(|v| !v.is_finite()) {
                out.push(TableDiagnostic {
                    line: b.line,
                    message: format!("bone {:?} has a non-finite offset", b.name),
                });
            }
        }
        if self.roots.is_empty() {
            out.push(TableDiagnostic {
                line: 1,
                message: "no bone records".into(),
            });
        }
        out
    }
}

fn parse_bone<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = (u32, &'a str)>>,
    header: &mut std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
    depth: usize,
) -> PedBone {
    let name = header.next().unwrap_or("").to_string();
    if name.is_empty() {
        diagnostics.push(TableDiagnostic {
            line,
            message: "bone record has no name".into(),
        });
    }
    if header.next() != Some("{") {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("bone {name:?}: expected `{{` after name"),
        });
    }
    let mut bone = PedBone {
        name,
        offset: [0.0; 3],
        children: Vec::new(),
        line,
    };
    while let Some((ln, text)) = lines.next() {
        if text.is_empty() {
            continue;
        }
        let mut t = text.split_whitespace();
        match t.next() {
            Some("offset") => {
                let mut ok = true;
                for (i, v) in bone.offset.iter_mut().enumerate() {
                    match t.next().and_then(|s| s.parse::<f32>().ok()) {
                        Some(f) => *v = f,
                        None => {
                            ok = false;
                            diagnostics.push(TableDiagnostic {
                                line: ln,
                                message: format!(
                                    "bone {:?}: offset component {i} is missing or non-numeric",
                                    bone.name
                                ),
                            });
                        }
                    }
                }
                if !ok {
                    bone.offset = [0.0; 3];
                }
            }
            Some("bone") if depth < MAX_BONE_DEPTH => {
                bone.children
                    .push(parse_bone(lines, &mut t, ln, diagnostics, depth + 1))
            }
            Some("bone") => diagnostics.push(TableDiagnostic {
                line: ln,
                message: format!("bone nesting exceeds {MAX_BONE_DEPTH} levels"),
            }),
            Some("}") => return bone,
            Some(other) => diagnostics.push(TableDiagnostic {
                line: ln,
                message: format!("bone {:?}: unknown directive {other:?}", bone.name),
            }),
            None => {}
        }
    }
    diagnostics.push(TableDiagnostic {
        line,
        message: format!("bone {:?}: unclosed at end of file", bone.name),
    });
    bone
}

/// A parsed `pedmodel_*.csv` animation state model.
#[derive(Debug, Clone)]
pub struct PedStates {
    /// State rows in file order.
    pub states: Vec<PedState>,
    /// Non-fatal parse problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One authored animation state (`STAND`, `WALK`, `ANTIC_LDIVE`, …).
#[derive(Debug, Clone)]
pub struct PedState {
    /// State name (column 1, `anim name` in the authored header).
    pub name: String,
    /// Clip stem under `anim/` — resolves to `anim/<stem>.anim`
    /// (column 2, `mma name` in the authored header).
    pub anim: String,
    /// First clip frame, 1-based on retail data.
    pub first_frame: i64,
    /// Last clip frame — equals `frames` or `frames + 1` on retail.
    pub last_frame: i64,
    /// `Y AXIS Offset` column — semantics unrecovered.
    pub y_offset: f32,
    /// `Y AXIS DISTANCE` column — semantics unrecovered.
    pub y_distance: f32,
    /// `X AXIS Offset` column — semantics unrecovered.
    pub x_offset: f32,
    /// `X AXIS DISTANCE` column — semantics unrecovered.
    pub x_distance: f32,
    /// `default next` column — the state chained after this one.
    pub next: String,
    /// 1-based source line.
    pub line: u32,
}

impl PedStates {
    /// Parse a `pedmodel_*.csv` state model. `#` lines and blank lines
    /// are comments; malformed rows degrade to diagnostics.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut diagnostics = Vec::new();
        let mut states = Vec::new();
        for (i, raw) in input.lines().enumerate() {
            let line = i as u32 + 1;
            let text = raw.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            let cells: Vec<&str> = text.split(',').map(str::trim).collect();
            if cells.len() != 9 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("expected 9 cells, found {}", cells.len()),
                });
                continue;
            }
            let mut ok = true;
            let ints: Vec<i64> = cells[2..4]
                .iter()
                .map(|c| {
                    c.parse::<i64>().unwrap_or_else(|_| {
                        ok = false;
                        0
                    })
                })
                .collect();
            let floats: Vec<f32> = cells[4..8]
                .iter()
                .map(|c| {
                    c.parse::<f32>().unwrap_or_else(|_| {
                        ok = false;
                        0.0
                    })
                })
                .collect();
            if !ok {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("state {:?}: non-numeric cell(s)", cells[0]),
                });
                continue;
            }
            states.push(PedState {
                name: cells[0].to_string(),
                anim: cells[1].to_string(),
                first_frame: ints[0],
                last_frame: ints[1],
                y_offset: floats[0],
                y_distance: floats[1],
                x_offset: floats[2],
                x_distance: floats[3],
                next: cells[8].to_string(),
                line,
            });
        }
        Ok(PedStates {
            states,
            diagnostics,
        })
    }

    /// Look up a state by name (case-sensitive, as authored).
    pub fn state(&self, name: &str) -> Option<&PedState> {
        self.states.iter().find(|s| s.name == name)
    }

    /// Consistency checks: duplicate names, reversed or negative frame
    /// windows, dangling `next` links, non-finite offsets.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        let names: std::collections::BTreeSet<&str> =
            self.states.iter().map(|s| s.name.as_str()).collect();
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.states {
            if !seen.insert(s.name.as_str()) {
                out.push(TableDiagnostic {
                    line: s.line,
                    message: format!("duplicate state name {:?}", s.name),
                });
            }
            if s.first_frame < 0 || s.last_frame < s.first_frame {
                out.push(TableDiagnostic {
                    line: s.line,
                    message: format!(
                        "state {:?}: invalid frame window {}..{}",
                        s.name, s.first_frame, s.last_frame
                    ),
                });
            }
            if s.anim.is_empty() {
                out.push(TableDiagnostic {
                    line: s.line,
                    message: format!("state {:?} has no clip name", s.name),
                });
            }
            if !s.next.is_empty() && !names.contains(s.next.as_str()) {
                out.push(TableDiagnostic {
                    line: s.line,
                    message: format!("state {:?}: next state {:?} is not defined", s.name, s.next),
                });
            }
            for (field, v) in [
                ("y_offset", s.y_offset),
                ("y_distance", s.y_distance),
                ("x_offset", s.x_offset),
                ("x_distance", s.x_distance),
            ] {
                if !v.is_finite() {
                    out.push(TableDiagnostic {
                        line: s.line,
                        message: format!("state {:?}: {field} is not finite", s.name),
                    });
                }
            }
        }
        out
    }
}

/// A parsed `.remap` bone remap (retail ships one:
/// `pedmodel_woman.remap`). Semantics unknown — likely a bone-order
/// remap for sharing clips across differently ordered rigs.
#[derive(Debug, Clone)]
pub struct PedRemap {
    /// Count line value, verbatim.
    pub declared: i64,
    /// Index entries in file order.
    pub indices: Vec<i64>,
    /// Non-fatal parse problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

impl PedRemap {
    /// Parse a `.remap` file: first token is the declared count, the
    /// rest are integer indices (whitespace-separated, any line split).
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut diagnostics = Vec::new();
        let mut declared: Option<i64> = None;
        let mut indices = Vec::new();
        for (i, raw) in input.lines().enumerate() {
            let line = i as u32 + 1;
            for tok in raw.split_whitespace() {
                if declared.is_none() {
                    match tok.parse::<i64>() {
                        Ok(v) => declared = Some(v),
                        Err(_) => {
                            return Err(FormatError::parse(
                                0,
                                format!("expected a count on the first line, found {tok:?}"),
                            ));
                        }
                    }
                    continue;
                }
                match tok.parse::<i64>() {
                    Ok(v) => indices.push(v),
                    Err(_) => diagnostics.push(TableDiagnostic {
                        line,
                        message: format!("non-integer index {tok:?}"),
                    }),
                }
            }
        }
        let Some(declared) = declared else {
            return Err(FormatError::parse(0, "empty .remap file"));
        };
        Ok(PedRemap {
            declared,
            indices,
            diagnostics,
        })
    }

    /// Consistency checks: declared-vs-actual count, negative indices.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        if self.declared != self.indices.len() as i64 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "declared {} indices, found {}",
                    self.declared,
                    self.indices.len()
                ),
            });
        }
        for (i, v) in self.indices.iter().enumerate() {
            if *v < 0 {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("index {i} is negative ({v})"),
                });
            }
        }
        out
    }
}

/// A parsed `.rays` file. Semantics unknown — the record shape is
/// recovered, what the rows and the integer grid mean is not.
#[derive(Debug, Clone)]
pub struct PedRays {
    /// Count line value, verbatim (19 on every retail file — the bone
    /// count).
    pub declared: i64,
    /// `declared` rows of vector + two integers.
    pub rays: Vec<PedRay>,
    /// Integer grid block after the ray rows — `declared` columns per
    /// row on retail (48 rows on man/woman, 24 on manw/womanw).
    pub grid: Vec<Vec<i64>>,
    /// Non-fatal parse problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One `.rays` record: a vector plus two integer indices.
#[derive(Debug, Clone)]
pub struct PedRay {
    /// Leading vector — unknown semantics (offset? direction?).
    pub vector: [f32; 3],
    /// First integer field — unknown semantics.
    pub index_a: i64,
    /// Second integer field — unknown semantics.
    pub index_b: i64,
}

impl PedRays {
    /// Parse a `.rays` file. Line-aligned: line 1 is the count, the
    /// next `count` non-empty lines are ray rows, the rest are the
    /// integer grid.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut diagnostics = Vec::new();
        let mut lines = input
            .lines()
            .enumerate()
            .map(|(i, l)| (i as u32 + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty());
        let Some((_, first)) = lines.next() else {
            return Err(FormatError::parse(0, "empty .rays file"));
        };
        let declared = first.parse::<i64>().map_err(|_| {
            FormatError::parse(
                0,
                format!("expected a count on the first line, found {first:?}"),
            )
        })?;
        let mut rays = Vec::new();
        for _ in 0..declared.max(0) {
            let Some((ln, text)) = lines.next() else {
                diagnostics.push(TableDiagnostic {
                    line: 1,
                    message: format!(
                        "declared {declared} ray rows but the file ends after {}",
                        rays.len()
                    ),
                });
                break;
            };
            let t: Vec<&str> = text.split_whitespace().collect();
            if t.len() != 5 {
                diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("ray row expects 5 fields, found {}", t.len()),
                });
                continue;
            }
            let mut vector = [0.0f32; 3];
            let mut ok = true;
            for (i, v) in vector.iter_mut().enumerate() {
                match t[i].parse::<f32>() {
                    Ok(f) => *v = f,
                    Err(_) => {
                        ok = false;
                        diagnostics.push(TableDiagnostic {
                            line: ln,
                            message: format!("ray row component {i} is non-numeric"),
                        });
                    }
                }
            }
            let a = t[3].parse::<i64>();
            let b = t[4].parse::<i64>();
            match (a, b) {
                (Ok(a), Ok(b)) if ok => rays.push(PedRay {
                    vector,
                    index_a: a,
                    index_b: b,
                }),
                (Err(_), _) | (_, Err(_)) => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: "ray row index is non-integer".into(),
                }),
                _ => {}
            }
        }
        let mut grid = Vec::new();
        for (ln, text) in lines {
            let row: Result<Vec<i64>, _> = text.split_whitespace().map(str::parse).collect();
            match row {
                Ok(row) => {
                    if declared > 0 && row.len() != declared as usize {
                        diagnostics.push(TableDiagnostic {
                            line: ln,
                            message: format!(
                                "grid row has {} cells, expected {declared}",
                                row.len()
                            ),
                        });
                    }
                    grid.push(row);
                }
                Err(_) => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: "grid row contains a non-integer cell".into(),
                }),
            }
        }
        Ok(PedRays {
            declared,
            rays,
            grid,
            diagnostics,
        })
    }

    /// Consistency checks: declared-vs-actual row counts.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        if self.declared != self.rays.len() as i64 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "declared {} ray rows, parsed {}",
                    self.declared,
                    self.rays.len()
                ),
            });
        }
        out
    }
}

/// A parsed binary `.anim` clip.
#[derive(Debug, Clone)]
pub struct PedAnim {
    /// Leading header word — always 0 on retail.
    pub reserved: u32,
    /// Frame count.
    pub frames: u32,
    /// Floats stored per frame (60 on retail — 20 XYZ channel triples
    /// against the 19-bone rigs; channel order is unrecovered).
    pub floats_per_frame: u32,
    /// Header float — tracks authored travel speed on locomotion clips
    /// (walk ≈ 1.55, run ≈ 2.97 on man); exact semantics unknown.
    pub motion_hint: f32,
    /// Trailing header byte — always 1 on retail.
    pub kind: u8,
    /// `frames * floats_per_frame` little-endian samples.
    pub samples: Vec<f32>,
}

impl PedAnim {
    /// Parse a `.anim` clip. The grammar is strict: any byte outside
    /// the measured `17 + frames*fpf*4` shape is an error.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(bytes);
        let reserved = r.u32()?;
        let frames = r.u32()?;
        let floats_per_frame = r.u32()?;
        let motion_hint = r.f32()?;
        let kind = r.u8()?;
        let total = (frames as usize)
            .checked_mul(floats_per_frame as usize)
            .filter(|&t| t <= MAX_ANIM_FLOATS)
            .ok_or(FormatError::InvalidValue {
                offset: 4,
                field: "frames*floats_per_frame",
                value: frames as u64 * floats_per_frame as u64,
                reason: "implausible clip size",
            })?;
        let mut samples = Vec::with_capacity(total);
        for _ in 0..total {
            samples.push(r.f32()?);
        }
        if !r.rest().is_empty() {
            return Err(FormatError::parse(
                r.pos(),
                format!("{} trailing byte(s) after clip samples", r.rest().len()),
            ));
        }
        Ok(PedAnim {
            reserved,
            frames,
            floats_per_frame,
            motion_hint,
            kind,
            samples,
        })
    }

    /// Frame `i` as a slice of `floats_per_frame` samples.
    pub fn frame(&self, i: u32) -> Option<&[f32]> {
        if i >= self.frames {
            return None;
        }
        let n = self.floats_per_frame as usize;
        self.samples.get(i as usize * n..(i as usize + 1) * n)
    }

    /// `floats_per_frame` read as XYZ triples per channel — 20 on
    /// retail (19 bones + one extra channel). `None` when the frame
    /// size is not a multiple of 3.
    pub fn channels(&self) -> Option<u32> {
        self.floats_per_frame
            .is_multiple_of(3)
            .then_some(self.floats_per_frame / 3)
    }

    /// Checks beyond the strict grammar: unexpected header values,
    /// empty clips, non-finite samples.
    pub fn validate(&self) -> Vec<PedAnimIssue> {
        let mut out = Vec::new();
        if self.reserved != 0 {
            out.push(PedAnimIssue::NonZeroReserved(self.reserved));
        }
        if self.kind != 1 {
            out.push(PedAnimIssue::UnknownKind(self.kind));
        }
        if self.frames == 0 {
            out.push(PedAnimIssue::EmptyClip);
        }
        if self.channels().is_none() {
            out.push(PedAnimIssue::RaggedFrameSize(self.floats_per_frame));
        }
        if let Some(i) = self.samples.iter().position(|v| !v.is_finite()) {
            out.push(PedAnimIssue::NonFiniteSample(i));
        }
        out
    }
}

/// Validation findings for a [`PedAnim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PedAnimIssue {
    /// The reserved header word is not 0.
    NonZeroReserved(u32),
    /// The kind byte is not 1.
    UnknownKind(u8),
    /// The clip declares zero frames.
    EmptyClip,
    /// `floats_per_frame` is not a multiple of 3.
    RaggedFrameSize(u32),
    /// First non-finite sample index.
    NonFiniteSample(usize),
}

impl std::fmt::Display for PedAnimIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonZeroReserved(v) => write!(f, "reserved header word is {v:#x}"),
            Self::UnknownKind(v) => write!(f, "kind byte is {v}, expected 1"),
            Self::EmptyClip => write!(f, "clip declares zero frames"),
            Self::RaggedFrameSize(n) => {
                write!(f, "floats_per_frame {n} is not a multiple of 3")
            }
            Self::NonFiniteSample(i) => write!(f, "sample {i} is not finite"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKEL: &str = "NumBones 4\r\nbone root {\r\n\toffset 0 1.1 0\r\n\tbone spine {\r\n\t\toffset 0 0.05 0.02\r\n\t\tbone neck {\r\n\t\t\toffset 0 0.3 0\r\n\t\t\tbone head {\r\n\t\t\t\toffset 0 0.15 0\r\n\t\t\t}\r\n\t\t}\r\n\t}\r\n}\r\n";

    #[test]
    fn skel_parses_retail_shape() {
        let s = PedSkel::parse(SKEL).unwrap();
        assert_eq!(s.declared_bones, 4);
        assert_eq!(s.bone_count(), 4);
        assert!(s.diagnostics.is_empty());
        assert!(s.validate().is_empty());
        let flat = s.flatten();
        let names: Vec<&str> = flat.iter().map(|(_, b)| b.name.as_str()).collect();
        assert_eq!(names, ["root", "spine", "neck", "head"]);
        assert_eq!(flat[1].0, 1);
        assert_eq!(flat[3].1.offset, [0.0, 0.15, 0.0]);
    }

    #[test]
    fn skel_rejects_missing_header() {
        assert!(PedSkel::parse("bone root {").is_err());
        assert!(PedSkel::parse("").is_err());
        assert!(PedSkel::parse("NumBones x").is_err());
    }

    #[test]
    fn skel_diagnoses_bad_records() {
        let s = PedSkel::parse(
            "NumBones 3\nwobble\nbone root {\noffset 0 x 0\nbone kid {\noffset 0 0 0\n}\n",
        )
        .unwrap();
        // wobble directive, non-numeric offset, unclosed root.
        assert_eq!(s.diagnostics.len(), 3);
        // Declared 3 bones but only 2 parsed.
        let issues = s.validate();
        assert!(issues.iter().any(|i| i.message.contains("does not match")));
    }

    #[test]
    fn skel_flags_duplicate_names() {
        let s =
            PedSkel::parse("NumBones 2\nbone a {\noffset 0 0 0\nbone a {\noffset 0 0 0\n}\n}\n")
                .unwrap();
        assert!(s.validate().iter().any(|i| i.message.contains("duplicate")));
    }

    const CSV: &str = "\
# anim name,mma name,first frame,last frame,Y AXIS Offset,Y AXIS DISTANCE,X AXIS Offset,X AXIS DISTANCE,default next[
STAND,pedanim_tstand,1,30,0,0,0,0,STAND
WALK,pedanim_twalk,1,20,0.281,1.409,0,0,WALK
STAND_WALK,pedanim_ts2w,1,4,0,0.281,0,0,WALK
";

    #[test]
    fn states_parse_retail_shape() {
        let s = PedStates::parse(CSV).unwrap();
        assert_eq!(s.states.len(), 3);
        assert!(s.diagnostics.is_empty());
        assert!(s.validate().is_empty());
        let w = s.state("WALK").unwrap();
        assert_eq!(w.anim, "pedanim_twalk");
        assert_eq!((w.first_frame, w.last_frame), (1, 20));
        assert_eq!(w.y_distance, 1.409);
        assert_eq!(w.next, "WALK");
    }

    #[test]
    fn states_diagnose_short_rows_and_bad_numbers() {
        let s = PedStates::parse("A,b,1\nC,d,1,2,3,4,5,6,C\nD,e,x,2,0,0,0,0,D\n").unwrap();
        // The well-formed middle row still parses.
        assert_eq!(s.states.len(), 1);
        assert_eq!(s.diagnostics.len(), 2);
    }

    #[test]
    fn states_flag_dangling_next_and_duplicates() {
        let s = PedStates::parse("A,a,1,2,0,0,0,0,MISSING\nA,a,1,2,0,0,0,0,A\nB,b,5,2,0,0,0,0,B\n")
            .unwrap();
        let v = s.validate();
        assert!(v.iter().any(|i| i.message.contains("not defined")));
        assert!(v.iter().any(|i| i.message.contains("duplicate")));
        assert!(v.iter().any(|i| i.message.contains("frame window")));
    }

    #[test]
    fn remap_parses_retail_shape() {
        let r = PedRemap::parse("17\r\n1 3 2 5 4 7 6 9 8 11 10 13 12 15 14 17 16\r\n").unwrap();
        assert_eq!(r.declared, 17);
        assert_eq!(r.indices.len(), 17);
        assert_eq!(r.indices[0], 1);
        assert_eq!(r.indices[1], 3);
        assert!(r.validate().is_empty());
    }

    #[test]
    fn remap_flags_count_mismatch() {
        let r = PedRemap::parse("3\n1 2\n").unwrap();
        assert_eq!(r.indices.len(), 2);
        assert!(r.validate().iter().any(|i| i.message.contains("declared")));
        assert!(PedRemap::parse("").is_err());
        assert!(PedRemap::parse("x\n1 2\n").is_err());
    }

    #[test]
    fn rays_parse_retail_shape() {
        let mut text = String::from("3\n");
        for i in 0..3 {
            text.push_str(&format!("0.1 0.2 0.3 {i} {}\n", i + 1));
        }
        text.push_str("0 1 2\n3 4 5\n");
        let r = PedRays::parse(&text).unwrap();
        assert_eq!(r.declared, 3);
        assert_eq!(r.rays.len(), 3);
        assert_eq!(r.rays[1].vector, [0.1, 0.2, 0.3]);
        assert_eq!(r.rays[1].index_b, 2);
        assert_eq!(r.grid, vec![vec![0, 1, 2], vec![3, 4, 5]]);
        assert!(r.diagnostics.is_empty());
        assert!(r.validate().is_empty());
    }

    #[test]
    fn rays_diagnose_short_files_and_ragged_grid() {
        let r = PedRays::parse("5\n0 0 0 1 2\n").unwrap();
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("file ends"))
        );
        let r = PedRays::parse("3\n0 0 0 0 0\n0 0 0 0 0\n0 0 0 0 0\n1 2\n").unwrap();
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("grid row has 2 cells"))
        );
        assert!(PedRays::parse("").is_err());
        assert!(PedRays::parse("x\n").is_err());
    }

    fn clip(frames: u32, fpf: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&frames.to_le_bytes());
        b.extend_from_slice(&fpf.to_le_bytes());
        b.extend_from_slice(&1.5f32.to_le_bytes());
        b.push(1);
        for i in 0..frames * fpf {
            b.extend_from_slice(&(i as f32).to_le_bytes());
        }
        b
    }

    #[test]
    fn anim_parses_retail_shape() {
        let a = PedAnim::parse(&clip(20, 60)).unwrap();
        assert_eq!(a.frames, 20);
        assert_eq!(a.floats_per_frame, 60);
        assert_eq!(a.motion_hint, 1.5);
        assert_eq!(a.kind, 1);
        assert_eq!(a.samples.len(), 1200);
        assert_eq!(a.channels(), Some(20));
        assert_eq!(a.frame(19).unwrap()[59], 1199.0);
        assert!(a.frame(20).is_none());
        assert!(a.validate().is_empty());
    }

    #[test]
    fn anim_rejects_truncated_and_oversized() {
        assert!(PedAnim::parse(&clip(20, 60)[..100]).is_err());
        assert!(PedAnim::parse(&[]).is_err());
        let mut b = clip(0, 60);
        // frames = u32::MAX with fpf 60 must not try to allocate.
        b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            PedAnim::parse(&b),
            Err(FormatError::InvalidValue { .. })
        ));
    }

    #[test]
    fn anim_rejects_trailing_bytes() {
        let mut b = clip(1, 3);
        b.push(0);
        assert!(PedAnim::parse(&b).is_err());
    }

    #[test]
    fn anim_validate_flags_odd_headers() {
        let mut b = clip(2, 5);
        b[0] = 1; // reserved
        b[16] = 7; // kind
        let a = PedAnim::parse(&b).unwrap();
        let v = a.validate();
        assert!(v.contains(&PedAnimIssue::NonZeroReserved(1)));
        assert!(v.contains(&PedAnimIssue::UnknownKind(7)));
        assert!(v.contains(&PedAnimIssue::RaggedFrameSize(5)));
        // NaN sample
        let mut b = clip(1, 3);
        b[17..21].copy_from_slice(&f32::NAN.to_le_bytes());
        let a = PedAnim::parse(&b).unwrap();
        assert!(a.validate().contains(&PedAnimIssue::NonFiniteSample(0)));
    }
}
