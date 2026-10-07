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
//!   header comment (`anim name,mma name,first frame,…`). The
//!   `* OFFSET`/`* DISTANCE` columns are authored per-window travel
//!   bookkeeping along the forward/lateral axes — chained transition
//!   rows accumulate the prior `DISTANCE` into the next `OFFSET` on
//!   retail (man `STAND_WALK` 0.281 + `WALK` 1.409 → `WALK_STAND`
//!   1.69; the dive chain carries ±2.2 m laterally). mm2hook names the
//!   pair `FSpeed`/`LSpeed` on `pedAnimationSequence`; they feed the
//!   movement controller, not pose sampling.
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
//!   retail clip carries 60 floats per frame — one XYZ root
//!   translation plus 19 Euler rotation triples, one per bone in
//!   `.skel` pre-order (measured on retail: the root channel's Z
//!   travel equals the state table's authored `Y AXIS DISTANCE`, and
//!   the standing pose's mirrored L/R arm rotations land on the
//!   clavicle/elbow pairs; the runtime composition lives in
//!   `mm2_game::ped`). The header float tracks the state table's
//!   authored Y-axis travel on several clips (man walk ≈ 1.55, run
//!   ≈ 2.97; woman walk matches `1.087` exactly) — a plausible
//!   locomotion-speed hint, but its exact semantics are unknown.
//!
//! `pedmodel_*.shaders` is a standalone copy of the PKG shader-chunk
//! grammar — parse it with [`crate::pkg::PkgShaders::parse`].
//! - `.mod` — ASCII skinned mesh ([`PedMod`]): a `version:` line and
//!   declared counts (`verts`/`normals`/`colors`/`tex1s`/`tex2s`/
//!   `tangents`/`materials`/`adjuncts`/`primitives`/`matrices`), the
//!   `v`/`n`/`c`/`t1`/`t2`/`ts`/`tt` resource lists, `materials`
//!   `mtl { … }` shader groups (which must match the `.shaders`
//!   per-paint-job order), then the geometry — either one global
//!   adjunct/primitive list partitioned to materials by declared count
//!   (`pedmodel_woman*`/`womanw`), or `packet { adj … tri … mtx … }`
//!   blocks carrying per-adjunct bone references (`pedmodel_man*`) —
//!   and the trailing `mtxv`/`mtxn` per-bone contiguous vertex/normal
//!   counts. Grammar documented in R3 (`Pedestrian_model.md`) and
//!   measured byte-consistent on all four retail files.

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
    /// `Y AXIS Offset` column — accumulated forward travel at window
    /// entry: chained rows carry the prior `Y AXIS DISTANCE` (man
    /// `WALK` `0.281` → `WALK_STAND` `1.69` = 0.281 + 1.409).
    pub y_offset: f32,
    /// `Y AXIS DISTANCE` column — authored forward travel across the
    /// window in metres: measured equal to the clip's root-channel Z
    /// travel on retail (man walk `1.409` vs a measured −1.410 drift,
    /// man run `2.854` vs −2.8535). mm2hook names the corresponding
    /// `pedAnimationSequence` field `FSpeed`.
    pub y_distance: f32,
    /// `X AXIS Offset` column — accumulated lateral position at window
    /// entry (dive chains carry `±2.2`).
    pub x_offset: f32,
    /// `X AXIS DISTANCE` column — authored lateral travel across the
    /// window, metres (±2.2 on the dive rows; mm2hook's `LSpeed`).
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
    /// Floats stored per frame (60 on retail — one XYZ root translation
    /// plus one Euler rotation triple per bone, in `.skel` pre-order).
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
        let mut samples = r.vec_for(total, 4);
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
    /// retail (one root translation + 19 bone rotations). `None` when
    /// the frame
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

/// Declared `.mod` header counts — each `name: <int>` is optional in
/// the grammar; `None` means the file omitted it (a validation issue,
/// not a parse error).
#[derive(Debug, Clone, Default)]
pub struct PedModCounts {
    /// `verts:` — vertex count.
    pub verts: Option<i64>,
    /// `normals:` — normal count.
    pub normals: Option<i64>,
    /// `colors:` — vertex-colour count (1 on retail).
    pub colors: Option<i64>,
    /// `tex1s:` — first texture-coordinate set size.
    pub tex1s: Option<i64>,
    /// `tex2s:` — second texture-coordinate set size (0 on retail).
    pub tex2s: Option<i64>,
    /// `tangents:` — tangent count, split across the `ts`/`tt` lists
    /// (0 on retail; the per-list split is inferred, R3 does not pin it).
    pub tangents: Option<i64>,
    /// `materials:` — `mtl` group count (equals the `.shaders`
    /// per-paint-job count on retail).
    pub materials: Option<i64>,
    /// `adjuncts:` — total adjunct count. On the flat dialect this is
    /// the global `adj` list length; on the packet dialect it measures
    /// the *distinct* `(vert, normal)` pairs across packets (packet
    /// adjuncts repeat at group boundaries — measured on retail).
    pub adjuncts: Option<i64>,
    /// `primitives:` — total `tri`/`stp`/`str` count.
    pub primitives: Option<i64>,
    /// `matrices:` — skin matrix count (== skeleton bone count on
    /// retail).
    pub matrices: Option<i64>,
}

/// One `.mod` adjunct — a (vertex, normal, colour, tex1, tex2) index
/// tuple; in the packet dialect a sixth field picks the owning bone
/// from the packet's `mtx` list.
#[derive(Debug, Clone)]
pub struct PedModAdj {
    /// Index into [`PedMod::verts`].
    pub vert: i64,
    /// Index into [`PedMod::normals`].
    pub normal: i64,
    /// Index into [`PedMod::colors`].
    pub color: i64,
    /// Index into [`PedMod::tex1s`] (0 with an empty set = unset).
    pub tex1: i64,
    /// Index into [`PedMod::tex2s`].
    pub tex2: i64,
    /// Packet dialect only: index into the owning packet's
    /// [`PedModPacket::matrices`] list.
    pub matrix: Option<i64>,
    /// 1-based source line.
    pub line: u32,
}

/// One `.mod` primitive.
#[derive(Debug, Clone)]
pub enum PedModPrim {
    /// `tri a b c` — indices into the owning adjunct list.
    Tri([i64; 3]),
    /// `stp <count> i…` (`reversed` false) or `str` (true) triangle
    /// strip — documented in R3 for other AGE games; none on retail.
    Strip {
        /// `str` records wind in reverse.
        reversed: bool,
        /// Adjunct indices, authored order.
        indices: Vec<i64>,
    },
}

impl PedModPrim {
    /// The adjunct indices this primitive references.
    pub fn indices(&self) -> &[i64] {
        match self {
            Self::Tri(t) => t,
            Self::Strip { indices, .. } => indices,
        }
    }
}

/// A `packet { … }` block — the packet dialect's per-group geometry:
/// adjuncts, packet-local primitives and the bone matrix list.
#[derive(Debug, Clone)]
pub struct PedModPacket {
    /// Declared adjunct count (header int 1).
    pub declared_adjuncts: i64,
    /// Declared primitive count (header int 2).
    pub declared_primitives: i64,
    /// Declared matrix count (header int 3).
    pub declared_matrices: i64,
    /// Optional fourth header int (reskin count, R3) — unused on
    /// retail, preserved verbatim.
    pub reskins: Option<i64>,
    /// `adj` rows — six fields each in this dialect.
    pub adjuncts: Vec<PedModAdj>,
    /// `tri`/`stp`/`str` rows — indices are local to `adjuncts`.
    pub primitives: Vec<PedModPrim>,
    /// `mtx` row — skeleton bone (matrix) indices the packet's
    /// adjuncts reference by position.
    pub matrices: Vec<i64>,
    /// 1-based line of the `packet` record.
    pub line: u32,
}

/// A `mtl <name> { … }` shader group. The group order must match the
/// `.shaders` per-paint-job order (R3).
#[derive(Debug, Clone)]
pub struct PedModMtl {
    /// `mtl` name, e.g. `Businessman1:SKIN`.
    pub name: String,
    /// Flat dialect: declared adjunct count — this group's slice of
    /// the global adjunct list.
    pub adjuncts: Option<i64>,
    /// Packet dialect: declared packet count — this group's slice of
    /// the packet list.
    pub packets: Option<i64>,
    /// Declared primitive count for the group.
    pub primitives: Option<i64>,
    /// `textures:` count.
    pub textures: Option<i64>,
    /// `texture: <index> <name>` rows.
    pub texture_names: Vec<(i64, String)>,
    /// `illum:` value (`diffuse` on every retail group; `emit` is the
    /// other documented value).
    pub illum: Option<String>,
    /// `ambient:` fallback colour (the `.shaders` job overrides it).
    pub ambient: Option<[f32; 3]>,
    /// `diffuse:` fallback colour.
    pub diffuse: Option<[f32; 3]>,
    /// `specular:` fallback colour.
    pub specular: Option<[f32; 3]>,
    /// Flat dialect: this group's slice of `PedMod::adjuncts`, carved
    /// from the declared counts in material order (clamped to the
    /// actual list; `validate` reports when the declared partition
    /// overruns the data).
    pub adjunct_range: std::ops::Range<usize>,
    /// Flat dialect: this group's slice of `PedMod::primitives`.
    pub primitive_range: std::ops::Range<usize>,
    /// Packet dialect: this group's slice of `PedMod::packets`.
    pub packet_range: std::ops::Range<usize>,
    /// 1-based line of the `mtl` record.
    pub line: u32,
}

/// A parsed `.mod` pedestrian mesh (the ASCII `version: 1.09` export
/// grammar documented in R3 `Pedestrian_model.md`).
#[derive(Debug, Clone)]
pub struct PedMod {
    /// `version:` line, verbatim (`1.09` on retail).
    pub version: String,
    /// Declared header counts.
    pub declared: PedModCounts,
    /// `v` vertex positions.
    pub verts: Vec<[f32; 3]>,
    /// `n` normal vectors.
    pub normals: Vec<[f32; 3]>,
    /// `c` RGBA colours.
    pub colors: Vec<[f32; 4]>,
    /// `t1` texture coordinates, first set.
    pub tex1s: Vec<[f32; 2]>,
    /// `t2` texture coordinates, second set.
    pub tex2s: Vec<[f32; 2]>,
    /// `ts` tangent rows (first tangent list; unused on retail).
    pub tangent_s: Vec<[f32; 3]>,
    /// `tt` tangent rows (second tangent list; unused on retail).
    pub tangent_t: Vec<[f32; 3]>,
    /// `mtl` shader groups in authored order.
    pub materials: Vec<PedModMtl>,
    /// Flat dialect: the shared `adj` list.
    pub adjuncts: Vec<PedModAdj>,
    /// Flat dialect: the shared `tri`/`stp`/`str` list.
    pub primitives: Vec<PedModPrim>,
    /// Packet dialect: `packet` blocks in authored order.
    pub packets: Vec<PedModPacket>,
    /// `mtxv` row — per-matrix counts of consecutive vertices, length
    /// `matrices`, summing to `verts` on retail.
    pub matrix_verts: Vec<i64>,
    /// `mtxn` row — per-matrix counts of consecutive normals.
    pub matrix_normals: Vec<i64>,
    /// Non-fatal parse problems.
    pub diagnostics: Vec<TableDiagnostic>,
}

/// Which adjunct layout a [`PedMod`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PedModDialect {
    /// `packet { … }` blocks with per-adjunct matrix references
    /// (`pedmodel_man`, `pedmodel_manw` on retail).
    Packets,
    /// Global `adj`/`tri` lists partitioned by material counts
    /// (`pedmodel_woman`, `pedmodel_womanw` on retail).
    Flat,
    /// Both or neither layout present — a malformed/mixed file.
    Mixed,
}

impl PedMod {
    /// Parse a `.mod` file. A missing `version:` header is fatal;
    /// malformed records degrade to diagnostics.
    pub fn parse(input: &str) -> Result<Self, FormatError> {
        let mut diagnostics = Vec::new();
        let mut lines = input
            .lines()
            .enumerate()
            .map(|(i, l)| (i as u32 + 1, l.trim()))
            .filter(|(_, l)| !l.is_empty())
            .peekable();

        // Header: `version: <v>` then `name: <int>` count fields in
        // any order, ending at the first non-`key:` line.
        let Some((_, text)) = lines.next() else {
            return Err(FormatError::parse(0, "empty .mod file"));
        };
        let version = match text.strip_prefix("version:") {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => {
                return Err(FormatError::parse(
                    0,
                    format!("expected a `version:` header, found {text:?}"),
                ));
            }
        };
        let mut declared = PedModCounts::default();
        while let Some((ln, text)) = lines.peek().copied() {
            let Some((key, value)) = text.split_once(':') else {
                break;
            };
            // `mtl`/record keywords never carry a colon first; a
            // material name does (`Businessman1:SKIN`) but `mtl`
            // starts with a keyword, not `name:` syntax.
            if key.contains(char::is_whitespace) || key.is_empty() {
                break;
            }
            let parsed = value.trim().parse::<i64>();
            let slot = match key {
                "verts" => Some(&mut declared.verts),
                "normals" => Some(&mut declared.normals),
                "colors" => Some(&mut declared.colors),
                "tex1s" => Some(&mut declared.tex1s),
                "tex2s" => Some(&mut declared.tex2s),
                "tangents" => Some(&mut declared.tangents),
                "materials" => Some(&mut declared.materials),
                "adjuncts" => Some(&mut declared.adjuncts),
                "primitives" => Some(&mut declared.primitives),
                "matrices" => Some(&mut declared.matrices),
                _ => None,
            };
            match (slot, parsed) {
                (Some(slot), Ok(v)) => *slot = Some(v),
                (Some(_), Err(_)) => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("header field {key:?} is not an integer"),
                }),
                (None, _) => diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("unknown header field {key:?}"),
                }),
            }
            lines.next();
        }

        let mut m = PedMod {
            version,
            declared,
            verts: Vec::new(),
            normals: Vec::new(),
            colors: Vec::new(),
            tex1s: Vec::new(),
            tex2s: Vec::new(),
            tangent_s: Vec::new(),
            tangent_t: Vec::new(),
            materials: Vec::new(),
            adjuncts: Vec::new(),
            primitives: Vec::new(),
            packets: Vec::new(),
            matrix_verts: Vec::new(),
            matrix_normals: Vec::new(),
            diagnostics,
        };

        // Resource lists run until the first `mtl` block.
        while let Some(&(ln, text)) = lines.peek() {
            if text.starts_with("mtl ") {
                break;
            }
            lines.next();
            let mut t = text.split_whitespace();
            let Some(tag) = t.next() else { continue };
            match tag {
                "v" => parse_vec3(&mut m.verts, t, ln, &mut m.diagnostics),
                "n" => parse_vec3(&mut m.normals, t, ln, &mut m.diagnostics),
                "c" => parse_vec4(&mut m.colors, t, ln, &mut m.diagnostics),
                "t1" => parse_vec2(&mut m.tex1s, t, ln, &mut m.diagnostics),
                "t2" => parse_vec2(&mut m.tex2s, t, ln, &mut m.diagnostics),
                "ts" => parse_vec3(&mut m.tangent_s, t, ln, &mut m.diagnostics),
                "tt" => parse_vec3(&mut m.tangent_t, t, ln, &mut m.diagnostics),
                _ => m.diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("unexpected record {tag:?} in the resource lists"),
                }),
            }
        }

        // Material groups.
        while let Some(&(ln, text)) = lines.peek() {
            if !text.starts_with("mtl ") {
                break;
            }
            lines.next();
            m.materials
                .push(parse_mtl(&mut lines, ln, text, &mut m.diagnostics));
        }

        // Geometry: packet blocks, the flat adjunct/primitive lists,
        // and the mtxv/mtxn trailers.
        while let Some((ln, text)) = lines.next() {
            let mut t = text.split_whitespace();
            let Some(tag) = t.next() else { continue };
            match tag {
                "packet" => {
                    let p = parse_packet(&mut lines, ln, t.collect(), &mut m.diagnostics);
                    m.packets.push(p);
                }
                "adj" => {
                    if let Some(a) = parse_adj(t, ln, false, &mut m.diagnostics) {
                        m.adjuncts.push(a);
                    }
                }
                "tri" | "stp" | "str" => {
                    if let Some(p) = parse_prim(tag, t, ln, &mut m.diagnostics) {
                        m.primitives.push(p);
                    }
                }
                "mtxv" => m.matrix_verts = parse_int_list(t, ln, &mut m.diagnostics),
                "mtxn" => m.matrix_normals = parse_int_list(t, ln, &mut m.diagnostics),
                "mtl" => m.diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: "`mtl` block inside the geometry section".into(),
                }),
                _ => m.diagnostics.push(TableDiagnostic {
                    line: ln,
                    message: format!("unexpected record {tag:?} in the geometry section"),
                }),
            }
        }

        // Carve per-material slices. Flat groups consume the shared
        // lists in authored order; packet groups consume packet blocks.
        // Both range ends clamp to the data (a declared-count overrun
        // must not leave an inverted range that later indexing panics
        // on); validate() reports overruns.
        let mut adj_at = 0usize;
        let mut prim_at = 0usize;
        let mut pkt_at = 0usize;
        for mtl in &mut m.materials {
            if let Some(n) = mtl.adjuncts {
                let end = adj_at.saturating_add(n.max(0) as usize);
                mtl.adjunct_range = adj_at.min(m.adjuncts.len())..end.min(m.adjuncts.len());
                adj_at = end;
            }
            if let Some(n) = mtl.primitives
                && mtl.adjuncts.is_some()
            {
                let end = prim_at.saturating_add(n.max(0) as usize);
                mtl.primitive_range = prim_at.min(m.primitives.len())..end.min(m.primitives.len());
                prim_at = end;
            }
            if let Some(n) = mtl.packets {
                let end = pkt_at.saturating_add(n.max(0) as usize);
                mtl.packet_range = pkt_at.min(m.packets.len())..end.min(m.packets.len());
                pkt_at = end;
            }
        }
        Ok(m)
    }

    /// Which adjunct layout the file uses.
    pub fn dialect(&self) -> PedModDialect {
        match (
            self.packets.is_empty(),
            self.adjuncts.is_empty() && self.primitives.is_empty(),
        ) {
            (false, true) => PedModDialect::Packets,
            (true, false) => PedModDialect::Flat,
            _ => PedModDialect::Mixed,
        }
    }

    /// Every adjunct across both dialects (packet adjuncts in packet
    /// order after the global list — which is empty in practice).
    pub fn all_adjuncts(&self) -> impl Iterator<Item = &PedModAdj> {
        self.adjuncts
            .iter()
            .chain(self.packets.iter().flat_map(|p| p.adjuncts.iter()))
    }

    /// Total primitive count across both dialects.
    pub fn primitive_count(&self) -> usize {
        self.primitives.len()
            + self
                .packets
                .iter()
                .map(|p| p.primitives.len())
                .sum::<usize>()
    }

    /// Consistency checks beyond parse diagnostics: declared-vs-actual
    /// counts, index ranges, dialect mixing, packet bookkeeping, the
    /// material partition, and the `mtxv`/`mtxn` trailers.
    pub fn validate(&self) -> Vec<TableDiagnostic> {
        let mut out = Vec::new();
        let count_check =
            |field: &'static str, declared: Option<i64>, actual: usize, out: &mut Vec<_>| {
                match declared {
                    Some(d) if d != actual as i64 => out.push(TableDiagnostic {
                        line: 1,
                        message: format!("declared {field} {d} but {actual} record(s) parsed"),
                    }),
                    None => out.push(TableDiagnostic {
                        line: 1,
                        message: format!("missing `{field}:` header field"),
                    }),
                    _ => {}
                }
            };
        count_check("verts", self.declared.verts, self.verts.len(), &mut out);
        count_check(
            "normals",
            self.declared.normals,
            self.normals.len(),
            &mut out,
        );
        count_check("colors", self.declared.colors, self.colors.len(), &mut out);
        count_check("tex1s", self.declared.tex1s, self.tex1s.len(), &mut out);
        count_check("tex2s", self.declared.tex2s, self.tex2s.len(), &mut out);
        count_check(
            "tangents",
            self.declared.tangents,
            self.tangent_s.len().max(self.tangent_t.len()),
            &mut out,
        );
        count_check(
            "materials",
            self.declared.materials,
            self.materials.len(),
            &mut out,
        );
        count_check(
            "primitives",
            self.declared.primitives,
            self.primitive_count(),
            &mut out,
        );
        if let Some(mtx) = self.declared.matrices {
            if !self.matrix_verts.is_empty() && self.matrix_verts.len() != mtx as usize {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!(
                        "mtxv lists {} entries against {} declared matrices",
                        self.matrix_verts.len(),
                        mtx
                    ),
                });
            }
            if !self.matrix_normals.is_empty() && self.matrix_normals.len() != mtx as usize {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!(
                        "mtxn lists {} entries against {} declared matrices",
                        self.matrix_normals.len(),
                        mtx
                    ),
                });
            }
        }
        // `adjuncts:` counts the flat list verbatim; on packet files it
        // counts distinct (vert, normal) pairs — packet adjuncts repeat
        // at material boundaries (measured on manw: 279 rows, 252
        // distinct, header 252).
        if let Some(d) = self.declared.adjuncts {
            let actual = match self.dialect() {
                PedModDialect::Flat => self.adjuncts.len(),
                PedModDialect::Packets => self
                    .all_adjuncts()
                    .map(|a| (a.vert, a.normal))
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                PedModDialect::Mixed => self.all_adjuncts().count(),
            };
            if d != actual as i64 {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("declared adjuncts {d} but {actual} parsed"),
                });
            }
        }
        // Retail invariant: the normals array is sized one-per-distinct
        // adjunct tuple (all four files: normals == distinct (v,n)
        // pairs == the `adjuncts:` header).
        let distinct = self
            .all_adjuncts()
            .map(|a| (a.vert, a.normal))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if distinct != 0 && distinct != self.normals.len() {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "{} distinct adjunct tuples against {} normals",
                    distinct,
                    self.normals.len()
                ),
            });
        }
        if self.dialect() == PedModDialect::Mixed {
            out.push(TableDiagnostic {
                line: 1,
                message: "mixed geometry dialects: packet blocks and flat lists both present"
                    .into(),
            });
        }
        if self.materials.is_empty() {
            out.push(TableDiagnostic {
                line: 1,
                message: "no `mtl` material groups".into(),
            });
        }

        // Adjunct index ranges. Texture/colour sets accept index 0 as
        // "unset" when the list is empty (retail `tex2s: 0` rows all
        // carry 0).
        let in_range = |i: i64, len: usize, empty_ok: bool| {
            i >= 0 && (i as usize) < len.max(empty_ok as usize)
        };
        for a in self.all_adjuncts() {
            if !(0..self.verts.len() as i64).contains(&a.vert) {
                out.push(TableDiagnostic {
                    line: a.line,
                    message: format!("adjunct vert {} outside 0..{}", a.vert, self.verts.len()),
                });
            }
            if !(0..self.normals.len() as i64).contains(&a.normal) {
                out.push(TableDiagnostic {
                    line: a.line,
                    message: format!(
                        "adjunct normal {} outside 0..{}",
                        a.normal,
                        self.normals.len()
                    ),
                });
            }
            if !in_range(a.color, self.colors.len(), true) {
                out.push(TableDiagnostic {
                    line: a.line,
                    message: format!("adjunct colour {} out of range", a.color),
                });
            }
            if !in_range(a.tex1, self.tex1s.len(), true) {
                out.push(TableDiagnostic {
                    line: a.line,
                    message: format!("adjunct tex1 {} out of range", a.tex1),
                });
            }
            if !in_range(a.tex2, self.tex2s.len(), true) {
                out.push(TableDiagnostic {
                    line: a.line,
                    message: format!("adjunct tex2 {} out of range", a.tex2),
                });
            }
        }

        // Primitive index ranges: packet prims index their own packet,
        // flat prims the global adjunct list.
        let prim_check = |prims: &[PedModPrim], bound: usize, out: &mut Vec<_>| {
            for p in prims {
                for &i in p.indices() {
                    if !(0..bound as i64).contains(&i) {
                        out.push(TableDiagnostic {
                            line: 0,
                            message: format!("primitive index {i} outside 0..{bound}"),
                        });
                    }
                }
            }
        };
        prim_check(&self.primitives, self.adjuncts.len(), &mut out);

        // Packet bookkeeping. The per-adjunct matrix reference is a
        // second encoding of the `mtxv` vertex→bone partition — both
        // describe the same skinning, and on retail they agree exactly,
        // so a disagreement means corrupt data.
        let mtxv_partition: Option<Vec<i64>> = {
            // Counts are full-range authored i64s — sum in i128 so a
            // hostile row cannot overflow the check itself.
            let sum: i128 = self.matrix_verts.iter().map(|&v| v as i128).sum();
            (!self.matrix_verts.is_empty()
                && self.matrix_verts.iter().all(|&v| v >= 0)
                && sum == self.verts.len() as i128)
                .then(|| {
                    self.matrix_verts
                        .iter()
                        .enumerate()
                        .flat_map(|(b, &n)| std::iter::repeat_n(b as i64, n as usize))
                        .collect()
                })
        };
        for p in &self.packets {
            if p.declared_adjuncts != p.adjuncts.len() as i64 {
                out.push(TableDiagnostic {
                    line: p.line,
                    message: format!(
                        "packet declared {} adjuncts, parsed {}",
                        p.declared_adjuncts,
                        p.adjuncts.len()
                    ),
                });
            }
            if p.declared_primitives != p.primitives.len() as i64 {
                out.push(TableDiagnostic {
                    line: p.line,
                    message: format!(
                        "packet declared {} primitives, parsed {}",
                        p.declared_primitives,
                        p.primitives.len()
                    ),
                });
            }
            if p.declared_matrices != p.matrices.len() as i64 {
                out.push(TableDiagnostic {
                    line: p.line,
                    message: format!(
                        "packet declared {} matrices, parsed {}",
                        p.declared_matrices,
                        p.matrices.len()
                    ),
                });
            }
            if let Some(mtx) = self.declared.matrices {
                for &b in &p.matrices {
                    if !(0..mtx).contains(&b) {
                        out.push(TableDiagnostic {
                            line: p.line,
                            message: format!("packet matrix {b} outside 0..{mtx}"),
                        });
                    }
                }
            }
            for a in &p.adjuncts {
                if let Some(mi) = a.matrix {
                    if !(0..p.matrices.len() as i64).contains(&mi) {
                        out.push(TableDiagnostic {
                            line: a.line,
                            message: format!(
                                "adjunct matrix slot {mi} outside the packet's {} matrices",
                                p.matrices.len()
                            ),
                        });
                    } else if let Some(part) = &mtxv_partition
                        && let Some(&expected) =
                            usize::try_from(a.vert).ok().and_then(|v| part.get(v))
                        && p.matrices[mi as usize] != expected
                    {
                        out.push(TableDiagnostic {
                            line: a.line,
                            message: format!(
                                "adjunct vert {} bound to matrix {} but mtxv assigns {}",
                                a.vert, p.matrices[mi as usize], expected
                            ),
                        });
                    }
                }
            }
            prim_check(&p.primitives, p.adjuncts.len(), &mut out);
        }

        // `mtxn` partitions the normals array by the same per-matrix
        // rule — every adjunct's normal should land in the bucket of
        // the bone its corner rides (the packet `mtx` slot, else the
        // vert's `mtxv` bucket). Measured: 100% agreement across all
        // 1946 retail adjuncts.
        let mtxn_partition: Option<Vec<i64>> = {
            let sum: i128 = self.matrix_normals.iter().map(|&v| v as i128).sum();
            (!self.matrix_normals.is_empty()
                && self.matrix_normals.iter().all(|&v| v >= 0)
                && sum == self.normals.len() as i128)
                .then(|| {
                    self.matrix_normals
                        .iter()
                        .enumerate()
                        .flat_map(|(b, &n)| std::iter::repeat_n(b as i64, n as usize))
                        .collect()
                })
        };
        if let Some(np) = &mtxn_partition {
            let normal_bone = |a: &PedModAdj| {
                usize::try_from(a.normal)
                    .ok()
                    .and_then(|n| np.get(n).copied())
            };
            let check = |a: &PedModAdj, corner: Option<i64>, out: &mut Vec<TableDiagnostic>| {
                if let (Some(b), Some(nb)) = (corner, normal_bone(a))
                    && b != nb
                {
                    out.push(TableDiagnostic {
                        line: a.line,
                        message: format!(
                            "adjunct normal {} in `mtxn` bone {nb} but its corner rides bone {b}",
                            a.normal
                        ),
                    });
                }
            };
            for a in &self.adjuncts {
                let corner = usize::try_from(a.vert)
                    .ok()
                    .and_then(|v| mtxv_partition.as_ref().and_then(|p| p.get(v).copied()));
                check(a, corner, &mut out);
            }
            for p in &self.packets {
                for a in &p.adjuncts {
                    let corner = a
                        .matrix
                        .and_then(|s| usize::try_from(s).ok())
                        .and_then(|s| p.matrices.get(s).copied());
                    check(a, corner, &mut out);
                }
            }
        }

        // Material bookkeeping: dialect fields, texture rows, illum,
        // declared primitives vs the owned data.
        for mtl in &self.materials {
            match (mtl.adjuncts, mtl.packets) {
                (Some(_), Some(_)) => out.push(TableDiagnostic {
                    line: mtl.line,
                    message: format!("material {:?} declares both adjuncts and packets", mtl.name),
                }),
                (None, None) => out.push(TableDiagnostic {
                    line: mtl.line,
                    message: format!(
                        "material {:?} declares neither adjuncts nor packets",
                        mtl.name
                    ),
                }),
                _ => {}
            }
            if let Some(t) = mtl.textures
                && t >= 0
                && t as usize != mtl.texture_names.len()
            {
                out.push(TableDiagnostic {
                    line: mtl.line,
                    message: format!(
                        "material {:?} declares {t} textures, found {}",
                        mtl.name,
                        mtl.texture_names.len()
                    ),
                });
            }
            if let Some(illum) = &mtl.illum
                && illum != "diffuse"
                && illum != "emit"
            {
                out.push(TableDiagnostic {
                    line: mtl.line,
                    message: format!("material {:?} has unknown illum {illum:?}", mtl.name),
                });
            }
            if let Some(d) = mtl.primitives {
                let actual = if mtl.packets.is_some() {
                    self.packets[mtl.packet_range.clone()]
                        .iter()
                        .map(|p| p.primitives.len() as i64)
                        .sum::<i64>()
                } else {
                    mtl.primitive_range.len() as i64
                };
                if d != actual {
                    out.push(TableDiagnostic {
                        line: mtl.line,
                        message: format!(
                            "material {:?} declares {d} primitives, owns {actual}",
                            mtl.name
                        ),
                    });
                }
            }
        }

        // Ownership coverage: the declared per-material counts must
        // exactly partition the shared lists — no orphans, no overrun.
        let claimed_packets: i128 = self
            .materials
            .iter()
            .filter_map(|m| m.packets.map(i128::from))
            .sum();
        if claimed_packets != self.packets.len() as i128 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "materials claim {claimed_packets} packets but {} exist",
                    self.packets.len()
                ),
            });
        }
        let claimed_adj: i128 = self
            .materials
            .iter()
            .filter_map(|m| m.adjuncts.map(i128::from))
            .sum();
        if claimed_adj != self.adjuncts.len() as i128 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "materials claim {claimed_adj} adjuncts but {} exist",
                    self.adjuncts.len()
                ),
            });
        }
        let claimed_prims: i128 = self
            .materials
            .iter()
            .filter(|m| m.adjuncts.is_some())
            .filter_map(|m| m.primitives.map(i128::from))
            .sum();
        if claimed_prims != self.primitives.len() as i128 {
            out.push(TableDiagnostic {
                line: 1,
                message: format!(
                    "materials claim {claimed_prims} primitives but {} exist",
                    self.primitives.len()
                ),
            });
        }

        // mtxv/mtxn trailers: counts are per-matrix, non-negative, and
        // partition the vertex/normal arrays contiguously.
        for (name, list, total) in [
            ("mtxv", &self.matrix_verts, self.verts.len()),
            ("mtxn", &self.matrix_normals, self.normals.len()),
        ] {
            if list.is_empty() {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("no {name} row — the vertex→bone partition is missing"),
                });
                continue;
            }
            if list.iter().any(|&v| v < 0) {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("{name} contains a negative count"),
                });
            }
            let sum: i128 = list.iter().map(|&v| v as i128).sum();
            if sum != total as i128 {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("{name} sums to {sum}, expected {total}"),
                });
            }
        }

        // Non-finite floats.
        for (name, nonfinite) in [
            (
                "v",
                self.verts.iter().any(|v| v.iter().any(|c| !c.is_finite())),
            ),
            (
                "n",
                self.normals
                    .iter()
                    .any(|v| v.iter().any(|c| !c.is_finite())),
            ),
            (
                "c",
                self.colors.iter().any(|v| v.iter().any(|c| !c.is_finite())),
            ),
            (
                "t1",
                self.tex1s.iter().any(|v| v.iter().any(|c| !c.is_finite())),
            ),
            (
                "t2",
                self.tex2s.iter().any(|v| v.iter().any(|c| !c.is_finite())),
            ),
        ] {
            if nonfinite {
                out.push(TableDiagnostic {
                    line: 1,
                    message: format!("{name} list contains a non-finite value"),
                });
            }
        }
        out
    }
}

fn parse_vec3(
    out: &mut Vec<[f32; 3]>,
    mut t: std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) {
    let v: Vec<&str> = t.by_ref().collect();
    if v.len() != 3 {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("vector record expects 3 components, found {}", v.len()),
        });
        return;
    }
    match v
        .iter()
        .map(|s| s.parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(f) => out.push([f[0], f[1], f[2]]),
        Err(_) => diagnostics.push(TableDiagnostic {
            line,
            message: "vector record has a non-numeric component".into(),
        }),
    }
}

fn parse_vec4(
    out: &mut Vec<[f32; 4]>,
    t: std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) {
    let v: Vec<&str> = t.collect();
    if v.len() != 4 {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("colour record expects 4 components, found {}", v.len()),
        });
        return;
    }
    match v
        .iter()
        .map(|s| s.parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(f) => out.push([f[0], f[1], f[2], f[3]]),
        Err(_) => diagnostics.push(TableDiagnostic {
            line,
            message: "colour record has a non-numeric component".into(),
        }),
    }
}

fn parse_vec2(
    out: &mut Vec<[f32; 2]>,
    t: std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) {
    let v: Vec<&str> = t.collect();
    if v.len() != 2 {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("texcoord record expects 2 components, found {}", v.len()),
        });
        return;
    }
    match v
        .iter()
        .map(|s| s.parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(f) => out.push([f[0], f[1]]),
        Err(_) => diagnostics.push(TableDiagnostic {
            line,
            message: "texcoord record has a non-numeric component".into(),
        }),
    }
}

fn parse_int_list(
    t: std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> Vec<i64> {
    let mut out = Vec::new();
    for tok in t {
        match tok.parse::<i64>() {
            Ok(v) => out.push(v),
            Err(_) => diagnostics.push(TableDiagnostic {
                line,
                message: format!("non-integer index {tok:?}"),
            }),
        }
    }
    out
}

fn parse_adj(
    t: std::str::SplitWhitespace<'_>,
    line: u32,
    packet: bool,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> Option<PedModAdj> {
    let toks: Vec<i64> = t
        .map(|s| {
            s.parse::<i64>().unwrap_or_else(|_| {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("adjunct field {s:?} is not an integer"),
                });
                i64::MIN
            })
        })
        .collect();
    if toks.contains(&i64::MIN) {
        return None;
    }
    let want = if packet { 6 } else { 5 };
    if toks.len() != want {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!(
                "adjunct expects {want} fields in this dialect, found {}",
                toks.len()
            ),
        });
        return None;
    }
    Some(PedModAdj {
        vert: toks[0],
        normal: toks[1],
        color: toks[2],
        tex1: toks[3],
        tex2: toks[4],
        matrix: packet.then(|| toks[5]),
        line,
    })
}

fn parse_prim(
    tag: &str,
    t: std::str::SplitWhitespace<'_>,
    line: u32,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> Option<PedModPrim> {
    let toks: Vec<i64> = t
        .map(|s| {
            s.parse::<i64>().unwrap_or_else(|_| {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("primitive field {s:?} is not an integer"),
                });
                i64::MIN
            })
        })
        .collect();
    if toks.contains(&i64::MIN) {
        return None;
    }
    match tag {
        "tri" => {
            if toks.len() != 3 {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("tri expects 3 indices, found {}", toks.len()),
                });
                return None;
            }
            Some(PedModPrim::Tri([toks[0], toks[1], toks[2]]))
        }
        _ => {
            // `stp`/`str`: first int is the index count (R3; neither
            // occurs on retail).
            let (count, indices) = match toks.split_first() {
                Some((&n, rest)) => (n, rest.to_vec()),
                None => {
                    diagnostics.push(TableDiagnostic {
                        line,
                        message: format!("{tag} record is empty"),
                    });
                    return None;
                }
            };
            if count < 0 || count as usize != indices.len() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!("{tag} declares {count} indices, found {}", indices.len()),
                });
            }
            Some(PedModPrim::Strip {
                reversed: tag == "str",
                indices,
            })
        }
    }
}

fn parse_mtl<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = (u32, &'a str)>>,
    line: u32,
    header: &'a str,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> PedModMtl {
    let mut t = header["mtl".len()..].split_whitespace();
    let name = t.next().unwrap_or("").to_string();
    if name.is_empty() {
        diagnostics.push(TableDiagnostic {
            line,
            message: "`mtl` record has no name".into(),
        });
    }
    if t.next() != Some("{") || t.next().is_some() {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("material {name:?}: expected `{{` after the name"),
        });
    }
    let mut mtl = PedModMtl {
        name,
        adjuncts: None,
        packets: None,
        primitives: None,
        textures: None,
        texture_names: Vec::new(),
        illum: None,
        ambient: None,
        diffuse: None,
        specular: None,
        adjunct_range: 0..0,
        primitive_range: 0..0,
        packet_range: 0..0,
        line,
    };
    for (ln, text) in lines.by_ref() {
        let mut t = text.split_whitespace();
        let Some(tag) = t.next() else { continue };
        let int =
            |t: &mut std::str::SplitWhitespace<'_>| t.next().and_then(|s| s.parse::<i64>().ok());
        match tag {
            "}" => return mtl,
            "adjuncts:" => mtl.adjuncts = int(&mut t),
            "packets:" => mtl.packets = int(&mut t),
            "primitives:" => mtl.primitives = int(&mut t),
            "textures:" => mtl.textures = int(&mut t),
            "texture:" => {
                let idx = int(&mut t).unwrap_or(0);
                let name = t.next().unwrap_or("").to_string();
                mtl.texture_names.push((idx, name));
            }
            "illum:" => mtl.illum = t.next().map(str::to_string),
            key @ ("ambient:" | "diffuse:" | "specular:") => {
                let f: Vec<f32> = t.filter_map(|s| s.parse().ok()).collect();
                let slot = match key {
                    "ambient:" => &mut mtl.ambient,
                    "diffuse:" => &mut mtl.diffuse,
                    _ => &mut mtl.specular,
                };
                if f.len() == 3 {
                    *slot = Some([f[0], f[1], f[2]]);
                } else {
                    diagnostics.push(TableDiagnostic {
                        line: ln,
                        message: format!("material {:?}: {key} expects 3 floats", mtl.name),
                    });
                }
            }
            _ => diagnostics.push(TableDiagnostic {
                line: ln,
                message: format!("material {:?}: unknown field {tag:?}", mtl.name),
            }),
        }
    }
    diagnostics.push(TableDiagnostic {
        line,
        message: format!("material {:?}: unclosed at end of file", mtl.name),
    });
    mtl
}

fn parse_packet<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = (u32, &'a str)>>,
    line: u32,
    header: Vec<&'a str>,
    diagnostics: &mut Vec<TableDiagnostic>,
) -> PedModPacket {
    // `packet <adjuncts> <primitives> <matrices> [reskins] {`
    let ints: Vec<Option<i64>> = header
        .iter()
        .filter(|s| **s != "{")
        .map(|s| s.parse::<i64>().ok())
        .collect();
    let mut p = PedModPacket {
        declared_adjuncts: ints.first().copied().flatten().unwrap_or(0),
        declared_primitives: ints.get(1).copied().flatten().unwrap_or(0),
        declared_matrices: ints.get(2).copied().flatten().unwrap_or(0),
        reskins: ints.get(3).copied().flatten(),
        adjuncts: Vec::new(),
        primitives: Vec::new(),
        matrices: Vec::new(),
        line,
    };
    if ints.len() < 3 || ints.len() > 4 || ints.iter().take(3).any(|i| i.is_none()) {
        diagnostics.push(TableDiagnostic {
            line,
            message: format!("packet header expects 3–4 integers, found {}", ints.len()),
        });
    }
    if !header.last().is_some_and(|s| *s == "{") {
        diagnostics.push(TableDiagnostic {
            line,
            message: "packet header is missing the opening `{`".into(),
        });
    }
    for (ln, text) in lines.by_ref() {
        let mut t = text.split_whitespace();
        let Some(tag) = t.next() else { continue };
        match tag {
            "}" => return p,
            "adj" => {
                if let Some(a) = parse_adj(t, ln, true, diagnostics) {
                    p.adjuncts.push(a);
                }
            }
            "tri" | "stp" | "str" => {
                if let Some(pr) = parse_prim(tag, t, ln, diagnostics) {
                    p.primitives.push(pr);
                }
            }
            "mtx" => p.matrices = parse_int_list(t, ln, diagnostics),
            _ => diagnostics.push(TableDiagnostic {
                line: ln,
                message: format!("unexpected record {tag:?} inside a packet"),
            }),
        }
    }
    diagnostics.push(TableDiagnostic {
        line,
        message: "packet unclosed at end of file".into(),
    });
    p
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

    /// Minimal packet-dialect `.mod` (the `pedmodel_man*` shape).
    const MOD_PACKETS: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 3
primitives: 2
matrices: 3

v	0.0	0.0	0.0
v	1.0	0.0	0.0
v	0.0	1.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl Test1:SKIN {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl Test1:HAIR {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

packet 2 1 2 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	1
	tri	0	1	0
	mtx 0 1
}

packet 1 1 1 {
	adj	2	2	0	0	0	0
	tri	0	0	0
	mtx 2
}

mtxv 1 1 1
mtxn 1 1 1
";

    /// Minimal flat-dialect `.mod` (the `pedmodel_woman*` shape): one
    /// global adjunct list partitioned by the materials' declared
    /// counts.
    const MOD_FLAT: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 4
primitives: 2
matrices: 2

v	0.0	0.0	0.0
v	1.0	0.0	0.0
v	0.0	1.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl A:SKIN {
	adjuncts:	3
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl A:HAIR {
	adjuncts:	1
	primitives:	1
	textures:	0
	illum: emit
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

adj	0	0	0	0	0
adj	1	1	0	0	0
adj	2	2	0	0	0
adj	0	0	0	0	0
tri	0	1	2
tri	3	3	3

mtxv 2 1
mtxn 2 1
";

    #[test]
    fn mod_parses_packet_dialect() {
        let m = PedMod::parse(MOD_PACKETS).unwrap();
        assert_eq!(m.version, "1.09");
        assert_eq!(m.verts.len(), 3);
        assert_eq!(m.normals.len(), 3);
        assert_eq!(m.colors.len(), 1);
        assert_eq!(m.tex1s.len(), 1);
        assert_eq!(m.materials.len(), 2);
        assert_eq!(m.materials[0].name, "Test1:SKIN");
        assert_eq!(m.materials[0].packets, Some(1));
        assert_eq!(m.materials[0].packet_range, 0..1);
        assert_eq!(m.materials[1].packet_range, 1..2);
        assert_eq!(m.packets.len(), 2);
        assert_eq!(m.packets[0].matrices, vec![0, 1]);
        assert_eq!(m.packets[0].adjuncts[1].matrix, Some(1));
        assert_eq!(m.packets[0].adjuncts[1].vert, 1);
        assert_eq!(m.dialect(), PedModDialect::Packets);
        assert_eq!(m.primitive_count(), 2);
        assert_eq!(m.matrix_verts, vec![1, 1, 1]);
        assert!(m.diagnostics.is_empty());
        assert!(m.validate().is_empty(), "{:?}", m.validate());
    }

    #[test]
    fn mod_parses_flat_dialect() {
        let m = PedMod::parse(MOD_FLAT).unwrap();
        assert_eq!(m.dialect(), PedModDialect::Flat);
        assert_eq!(m.adjuncts.len(), 4);
        assert_eq!(m.primitives.len(), 2);
        // The materials partition the shared lists by declared count.
        assert_eq!(m.materials[0].adjunct_range, 0..3);
        assert_eq!(m.materials[1].adjunct_range, 3..4);
        assert_eq!(m.materials[0].primitive_range, 0..1);
        assert_eq!(m.materials[1].primitive_range, 1..2);
        assert_eq!(m.materials[1].illum.as_deref(), Some("emit"));
        assert_eq!(m.adjuncts[0].matrix, None);
        assert!(m.diagnostics.is_empty());
        assert!(m.validate().is_empty(), "{:?}", m.validate());
    }

    #[test]
    fn mod_rejects_missing_version() {
        assert!(PedMod::parse("").is_err());
        assert!(PedMod::parse("verts: 3\n").is_err());
        assert!(PedMod::parse("version:\nv 0 0 0\n").is_err());
    }

    #[test]
    fn mod_diagnoses_bad_records() {
        let m = PedMod::parse(
            "\
version: 1.09
verts: 2
bogusfield: 9
normals: x

v	0.0	0.0
v	nope	0	0
mtl A {
	textures:	0
	wobble: 1
}
mtl C
packet 1 0 1 {
	adj	1	x	0	0	0	0
	nonsense 1 2
}
packet 0 0 0 {
	adj	0	0	0	0	0	0
",
        )
        .unwrap();
        let d = &m.diagnostics;
        assert!(d.iter().any(|d| d.message.contains("bogusfield")));
        assert!(d.iter().any(|d| d.message.contains("not an integer")));
        assert!(d.iter().any(|d| d.message.contains("3 components")));
        assert!(d.iter().any(|d| d.message.contains("non-numeric")));
        assert!(d.iter().any(|d| d.message.contains("expected `{`")));
        assert!(d.iter().any(|d| d.message.contains("wobble")));
        // `mtl C` has no `{`; the truncated last packet is unclosed.
        assert!(d.iter().any(|d| d.message.contains("unclosed")));
        assert!(d.iter().any(|d| d.message.contains("nonsense")));
        assert!(d.iter().any(|d| d.message.contains("not an integer")));
        // validate() flags the declared-vs-actual count gaps too.
        let v = m.validate();
        assert!(v.iter().any(|i| i.message.contains("declared verts")));
        assert!(v.iter().any(|i| i.message.contains("missing")));
    }

    #[test]
    fn mod_validate_flags_index_and_partition_errors() {
        // Flat dialect: adjunct vert/normal/tex out of range, tri
        // index out of range, declared counts off, mtxv sum off.
        let m = PedMod::parse(
            &MOD_FLAT
                .replace("verts: 3", "verts: 4")
                .replace("adj\t2\t2\t0\t0\t0", "adj\t9\t9\t0\t9\t0")
                .replace("tri\t0\t1\t2", "tri\t0\t1\t9")
                .replace("mtxv 2 1", "mtxv 2 2"),
        )
        .unwrap();
        let v = m.validate();
        assert!(v.iter().any(|i| i.message.contains("declared verts 4")));
        assert!(v.iter().any(|i| i.message.contains("vert 9")));
        assert!(v.iter().any(|i| i.message.contains("normal 9")));
        assert!(v.iter().any(|i| i.message.contains("tex1 9")));
        assert!(v.iter().any(|i| i.message.contains("index 9")));
        assert!(v.iter().any(|i| i.message.contains("mtxv sums to 4")));
    }

    #[test]
    fn mod_validate_flags_packet_errors() {
        // Packet dialect: bad declared counts, matrix slot out of
        // range, mtx entry beyond `matrices`, and an adjunct whose
        // packet binding disagrees with the mtxv partition.
        let m = PedMod::parse(
            &MOD_PACKETS
                .replace("packet 2 1 2", "packet 3 1 2")
                .replace("adj\t1\t1\t0\t0\t0\t1", "adj\t1\t1\t0\t0\t0\t7")
                .replace("mtx 0 1", "mtx 0 9")
                .replace("mtx 2", "mtx 1"),
        )
        .unwrap();
        let v = m.validate();
        assert!(v.iter().any(|i| i.message.contains("declared 3 adjuncts")));
        assert!(v.iter().any(|i| i.message.contains("matrix slot 7")));
        assert!(v.iter().any(|i| i.message.contains("matrix 9 outside")));
        // Packet 2's `mtx 2` became `mtx 1`: vert 2 now binds bone 1
        // where the mtxv partition assigns bone 2.
        assert!(v.iter().any(|i| i.message.contains("mtxv assigns")));
    }

    #[test]
    fn mod_flags_mixed_dialect_and_missing_fields() {
        // A packet block plus flat records is a mixed file; a material
        // declaring neither adjuncts nor packets is flagged.
        let m = PedMod::parse(
            "\
version: 1.09
verts: 1
normals: 1
colors: 1
tex1s: 0
tex2s: 0
tangents: 0
materials: 1
adjuncts: 1
primitives: 0
matrices: 1

v	0	0	0
n	0	0	1
c	1	1	1	1

mtl A {
	primitives:	0
}

adj	0	0	0	0	0
packet 0 0 1 {
	mtx 0
}
",
        )
        .unwrap();
        let v = m.validate();
        assert!(
            v.iter()
                .any(|i| i.message.contains("mixed geometry dialects"))
        );
        assert!(v.iter().any(|i| i.message.contains("neither")));
        // The flat adjunct does not carry a matrix field.
        assert_eq!(m.adjuncts[0].matrix, None);
    }

    #[test]
    fn mod_validate_reports_overdeclared_material_counts() {
        // A material declaring more packets than the file holds once
        // left later materials an inverted carve range that validate()
        // panicked on. Now the carve clamps both ends and the overrun
        // is a reported issue.
        let m = PedMod::parse(&MOD_PACKETS.replacen("packets:\t1", "packets:\t5", 1)).unwrap();
        assert_eq!(m.materials[0].packet_range, 0..2);
        assert_eq!(m.materials[1].packet_range, 2..2);
        let v = m.validate();
        assert!(
            v.iter()
                .any(|i| i.message.contains("claim 6 packets but 2 exist")),
            "{v:?}"
        );

        // The shared-list carves clamp the same way.
        let m = PedMod::parse(
            &MOD_FLAT
                .replacen("adjuncts:\t3", "adjuncts:\t9", 1)
                .replacen("primitives:\t1", "primitives:\t9", 1),
        )
        .unwrap();
        assert_eq!(m.materials[0].adjunct_range, 0..4);
        assert_eq!(m.materials[1].adjunct_range, 4..4);
        assert_eq!(m.materials[1].primitive_range, 2..2);
        let v = m.validate();
        assert!(
            v.iter()
                .any(|i| i.message.contains("claim 10 adjuncts but 4 exist")),
            "{v:?}"
        );
        assert!(
            v.iter()
                .any(|i| i.message.contains("claim 10 primitives but 2 exist")),
            "{v:?}"
        );
    }

    #[test]
    fn mod_validate_survives_unbounded_authored_counts() {
        // `mtxv`/`mtxn` counts and `mtl` claim fields are full-range
        // authored i64s (`parse_int_list` accepts the lot): hostile
        // values must produce diagnostics, not an overflow panic in a
        // summation — overflow-checks are on in dev/test builds, so
        // reaching the asserts at all is the regression.
        let m = PedMod::parse(
            &MOD_FLAT
                .replace("mtxv 2 1", "mtxv 1 9223372036854775807 1")
                .replace("mtxn 2 1", "mtxn 9223372036854775807 9223372036854775807")
                .replacen("adjuncts:\t3", "adjuncts:\t9223372036854775807", 1)
                .replacen("primitives:\t1", "primitives:\t9223372036854775807", 1),
        )
        .unwrap();
        let v = m.validate();
        assert!(
            v.iter().any(|i| i.message.contains("mtxv sums to")),
            "{v:?}"
        );
        assert!(
            v.iter().any(|i| i.message.contains("mtxn sums to")),
            "{v:?}"
        );
        // i64::MAX + the second material's 1 — the sum must not wrap.
        assert!(
            v.iter().any(|i| i.message.contains("adjuncts but 4 exist")),
            "{v:?}"
        );
        assert!(
            v.iter()
                .any(|i| i.message.contains("primitives but 2 exist")),
            "{v:?}"
        );

        // The packet dialect's `packets:` claim sum overflows the same
        // way.
        let m =
            PedMod::parse(&MOD_PACKETS.replacen("packets:\t1", "packets:\t9223372036854775807", 1))
                .unwrap();
        let v = m.validate();
        assert!(
            v.iter().any(|i| i.message.contains("packets but 2 exist")),
            "{v:?}"
        );
    }
}
