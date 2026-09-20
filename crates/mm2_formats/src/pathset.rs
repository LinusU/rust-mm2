//! Parser for MM2 `PTH1` pathset files (`*.pathset`).
//!
//! Pathsets are the city's second placement mechanism after INST: each
//! file holds named point sequences that stamp a prop PKG or a decal
//! texture along their geometry. Cities ship `props.pathset` /
//! `decals.pathset` sets, per-event files live under `race/<city>/`
//! (course barricades, parked cars, animated objects such as the
//! bridges/ferries/trains), and `audio_pathsets/` holds ambient-sound
//! source paths whose names are labels, not asset references.
//!
//! Provenance: `angel-file-formats/Midtown Madness 2/Pathset.md`
//! (source R3), measured against all 101 retail `.pathset` files — see
//! `docs/research/pathset.md`.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! "PTH1" u32 path_count u32 current_path
//! path: char[32] name (NUL-padded)
//!       u32 point_count u32 selection
//!       point_count * (u32 attributes + f32 x y z)
//!       u8 kind u8 spacing u8 pad[2]
//! ```
//!
//! `current_path`/`selection` are development-tool cursors (the
//! path/point last selected in the editor — inferred; `selection` is
//! always `< point_count` or equal on retail) and are preserved raw.
//! The per-point `attributes` word is undocumented and likewise kept
//! verbatim. `spacing` is authored in units of 1/4 metre
//! ([`Path::spacing_metres`]); R3 documents it as meaningful only for
//! line strips, though retail files carry nonzero values on other
//! kinds.

use crate::{FormatError, Reader};
use std::fmt;

/// File magic.
pub const PATHSET_MAGIC: &[u8; 4] = b"PTH1";

/// Fixed width of the NUL-padded path name field.
pub const NAME_LEN: usize = 32;

// Sanity caps. Retail maxima measured 2026-09-20: 144 paths/file,
// 195 points/path, 2692 paths / 13185 points across 98 parseable files.
const MAX_PATHS: usize = 4096;
const MAX_POINTS: usize = 1 << 16;

/// How a path stamps its object, per R3's documented `type` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Code 0: one object per point; orientation and spacing unused.
    Points,
    /// Code 1: points come in pairs — position then a second point
    /// whose offset sets the object's yaw (rotation about Y only).
    Directed,
    /// Code 2: a polyline; objects are stamped along each segment,
    /// spaced by [`Path::spacing_metres`].
    LineStrip,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathKind::Points => f.write_str("points"),
            PathKind::Directed => f.write_str("directed"),
            PathKind::LineStrip => f.write_str("line-strip"),
        }
    }
}

/// One vertex of a path: a position plus an undocumented attribute
/// word, preserved raw.
#[derive(Debug, Clone)]
pub struct PathPoint {
    /// Undocumented per-point word (values like `0x1b5`, `0xf0`,
    /// `0x3b09` recur on retail; meaning unknown — UNK-20 area).
    pub attributes: u32,
    /// World-space position.
    pub position: [f32; 3],
}

/// A named point sequence and its stamping rule.
#[derive(Debug, Clone)]
pub struct Path {
    /// Path name. On placement files this is the asset basename
    /// (`geometry/<name>.pkg` for props, `texture/<name>.*` for
    /// decals); event-state prefixes (`OPEN:`, `inactive:`) and
    /// `PATHnn` route labels also occur on retail.
    pub name: String,
    /// Development-tool point cursor, preserved raw (inferred — see
    /// module docs).
    pub selection: u32,
    /// The path's vertices.
    pub points: Vec<PathPoint>,
    /// Raw `type` byte; see [`Path::kind`].
    pub kind_code: u8,
    /// Raw `spacing` byte in units of 1/4 metre.
    pub spacing_code: u8,
}

impl Path {
    /// Interpret the raw `type` byte. `None` for undocumented values
    /// (none exist on cleanly parsed retail files).
    pub fn kind(&self) -> Option<PathKind> {
        match self.kind_code {
            0 => Some(PathKind::Points),
            1 => Some(PathKind::Directed),
            2 => Some(PathKind::LineStrip),
            _ => None,
        }
    }

    /// Prop spacing along a line strip, in metres (`spacing_code` is
    /// authored in quarter metres).
    pub fn spacing_metres(&self) -> f32 {
        f32::from(self.spacing_code) / 4.0
    }

    /// The asset basename this path stamps, if the name is one.
    /// `PATHnn` names are internal route labels (ambient-sound paths,
    /// parked-car/ferry/train routes), never asset references — `None`.
    /// `PREFIX:` event-state decorations (`OPEN:`, `inactive:` — see
    /// `docs/research/pathset.md`) are stripped: the returned name is
    /// the last `:`-separated segment. `None` also for an empty tail.
    pub fn asset_name(&self) -> Option<&str> {
        let tail = self.name.rsplit(':').next().unwrap_or(&self.name);
        if tail.is_empty()
            || (tail.len() > 4
                && tail.starts_with("PATH")
                && tail[4..].chars().all(|c| c.is_ascii_digit()))
        {
            return None;
        }
        Some(tail)
    }
}

/// A parsed pathset file.
#[derive(Debug, Clone)]
pub struct Pathset {
    /// Development-tool path cursor, preserved raw (inferred — see
    /// module docs).
    pub current_path: u32,
    /// All paths in file order.
    pub paths: Vec<Path>,
}

/// A consistency problem found by [`Pathset::validate`]. Parsing only
/// checks structural bounds; authored quirks are reported here so good
/// files still load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsetIssue {
    /// A path has an empty name.
    EmptyName {
        /// Path index.
        path: usize,
    },
    /// A path carries a `type` byte outside the documented 0–2.
    UnknownPathKind {
        /// Path index.
        path: usize,
        /// Raw `type` byte.
        kind: u8,
    },
    /// A `Directed` path has an odd point count; the kind stamps one
    /// object per point *pair*.
    DirectedOddPointCount {
        /// Path index.
        path: usize,
        /// Point count.
        points: usize,
    },
    /// A point coordinate is NaN or infinite.
    NonFinitePoint {
        /// Path index.
        path: usize,
        /// Point index.
        point: usize,
    },
    /// The file's `current_path` cursor names a path that does not
    /// exist (only reported when the file has paths at all).
    CurrentPathOutOfRange {
        /// Raw `current_path` value.
        current_path: u32,
        /// Number of paths in the file.
        paths: usize,
    },
}

impl fmt::Display for PathsetIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathsetIssue::EmptyName { path } => write!(f, "path {path}: empty name"),
            PathsetIssue::UnknownPathKind { path, kind } => {
                write!(f, "path {path}: undocumented type {kind}")
            }
            PathsetIssue::DirectedOddPointCount { path, points } => write!(
                f,
                "path {path}: directed path with odd point count {points}"
            ),
            PathsetIssue::NonFinitePoint { path, point } => {
                write!(f, "path {path}: point {point} has a non-finite coordinate")
            }
            PathsetIssue::CurrentPathOutOfRange {
                current_path,
                paths,
            } => write!(f, "current_path {current_path} but only {paths} path(s)"),
        }
    }
}

impl Pathset {
    /// Parse a complete `PTH1` file. Trailing bytes are an error —
    /// every byte of a pathset is accounted for.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(data);
        let magic = r.bytes(4)?;
        if magic != PATHSET_MAGIC {
            return Err(FormatError::BadMagic {
                offset: 0,
                expected: "PTH1",
                found: magic.to_vec(),
            });
        }
        let n_paths = r.u32()? as usize;
        if n_paths > MAX_PATHS {
            return Err(FormatError::InvalidValue {
                offset: 4,
                field: "path_count",
                value: n_paths as u64,
                reason: "path count exceeds sanity cap",
            });
        }
        let current_path = r.u32()?;

        let mut paths = Vec::with_capacity(n_paths);
        for _ in 0..n_paths {
            paths.push(parse_path(&mut r)?);
        }

        if !r.is_empty() {
            return Err(FormatError::parse(
                r.pos(),
                format!("{} trailing byte(s) after last path", r.remaining()),
            ));
        }
        Ok(Pathset {
            current_path,
            paths,
        })
    }

    /// Check authored-data consistency: name/kind/point anomalies and
    /// the `current_path` cursor. See [`PathsetIssue`].
    pub fn validate(&self) -> Vec<PathsetIssue> {
        let mut issues = Vec::new();
        for (pi, path) in self.paths.iter().enumerate() {
            if path.name.is_empty() {
                issues.push(PathsetIssue::EmptyName { path: pi });
            }
            if path.kind().is_none() {
                issues.push(PathsetIssue::UnknownPathKind {
                    path: pi,
                    kind: path.kind_code,
                });
            }
            if path.kind() == Some(PathKind::Directed) && path.points.len() % 2 != 0 {
                issues.push(PathsetIssue::DirectedOddPointCount {
                    path: pi,
                    points: path.points.len(),
                });
            }
            for (vi, point) in path.points.iter().enumerate() {
                if point.position.iter().any(|c| !c.is_finite()) {
                    issues.push(PathsetIssue::NonFinitePoint {
                        path: pi,
                        point: vi,
                    });
                }
            }
        }
        if !self.paths.is_empty() && self.current_path as usize >= self.paths.len() {
            issues.push(PathsetIssue::CurrentPathOutOfRange {
                current_path: self.current_path,
                paths: self.paths.len(),
            });
        }
        issues
    }
}

fn parse_path(r: &mut Reader<'_>) -> Result<Path, FormatError> {
    let name_offset = r.pos();
    let raw_name = r.bytes(NAME_LEN)?;
    let end = raw_name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    let name =
        String::from_utf8(raw_name[..end].to_vec()).map_err(|_| FormatError::InvalidString {
            offset: name_offset,
            reason: "path name is not valid UTF-8/ASCII",
        })?;

    let n_points = r.u32()? as usize;
    if n_points > MAX_POINTS {
        return Err(FormatError::InvalidValue {
            offset: r.pos() - 4,
            field: "point_count",
            value: n_points as u64,
            reason: "point count exceeds sanity cap",
        });
    }
    let selection = r.u32()?;

    let mut points = Vec::with_capacity(n_points);
    for _ in 0..n_points {
        let attributes = r.u32()?;
        let position = r.vec3()?;
        points.push(PathPoint {
            attributes,
            position,
        });
    }

    let kind_code = r.u8()?;
    let spacing_code = r.u8()?;
    r.skip(2)?; // padding

    Ok(Path {
        name,
        selection,
        points,
        kind_code,
        spacing_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_bytes(
        name: &str,
        selection: u32,
        points: &[(u32, [f32; 3])],
        kind: u8,
        spacing: u8,
    ) -> Vec<u8> {
        let mut d = vec![0u8; NAME_LEN];
        d[..name.len()].copy_from_slice(name.as_bytes());
        d.extend_from_slice(&(points.len() as u32).to_le_bytes());
        d.extend_from_slice(&selection.to_le_bytes());
        for (attr, pos) in points {
            d.extend_from_slice(&attr.to_le_bytes());
            for c in pos {
                d.extend_from_slice(&c.to_le_bytes());
            }
        }
        d.push(kind);
        d.push(spacing);
        d.extend_from_slice(&[0, 0]);
        d
    }

    fn file(paths: &[Vec<u8>], current: u32) -> Vec<u8> {
        let mut d = PATHSET_MAGIC.to_vec();
        d.extend_from_slice(&(paths.len() as u32).to_le_bytes());
        d.extend_from_slice(&current.to_le_bytes());
        for p in paths {
            d.extend_from_slice(p);
        }
        d
    }

    #[test]
    fn parses_all_three_kinds() {
        let d = file(
            &[
                path_bytes("sp_tree_f", 0, &[(7, [1.0, 2.0, 3.0])], 0, 0),
                path_bytes(
                    "sp_sign_f",
                    1,
                    &[(0, [0.0, 0.0, 0.0]), (0, [1.0, 0.0, 0.0])],
                    1,
                    20,
                ),
                path_bytes(
                    "decal_l",
                    0,
                    &[
                        (0, [0.0, 0.0, 0.0]),
                        (0, [4.0, 0.0, 0.0]),
                        (0, [8.0, 0.0, 0.0]),
                    ],
                    2,
                    20,
                ),
            ],
            2,
        );
        let ps = Pathset::parse(&d).unwrap();
        assert_eq!(ps.current_path, 2);
        assert_eq!(ps.paths.len(), 3);
        assert_eq!(ps.paths[0].name, "sp_tree_f");
        assert_eq!(ps.paths[0].kind(), Some(PathKind::Points));
        assert_eq!(ps.paths[0].points[0].attributes, 7);
        assert_eq!(ps.paths[0].points[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(ps.paths[1].kind(), Some(PathKind::Directed));
        assert_eq!(ps.paths[1].selection, 1);
        assert_eq!(ps.paths[1].spacing_metres(), 5.0);
        assert_eq!(ps.paths[2].kind(), Some(PathKind::LineStrip));
        assert!(ps.validate().is_empty());
    }

    #[test]
    fn rejects_bad_magic() {
        let d = b"XXXX________".to_vec();
        assert!(matches!(
            Pathset::parse(&d),
            Err(FormatError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_truncated_points() {
        // Declares 2 points but supplies one.
        let mut d = PATHSET_MAGIC.to_vec();
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        let mut p = vec![0u8; NAME_LEN];
        p[..4].copy_from_slice(b"test");
        p.extend_from_slice(&2u32.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&[0u8; 16]);
        d.extend_from_slice(&p);
        assert!(matches!(
            Pathset::parse(&d),
            Err(FormatError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut d = file(&[path_bytes("a", 0, &[], 0, 0)], 0);
        d.push(0xAA);
        assert!(matches!(Pathset::parse(&d), Err(FormatError::Parse { .. })));
    }

    #[test]
    fn rejects_absurd_counts() {
        let mut d = PATHSET_MAGIC.to_vec();
        d.extend_from_slice(&u32::MAX.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            Pathset::parse(&d),
            Err(FormatError::InvalidValue { .. })
        ));
    }

    #[test]
    fn zero_point_paths_are_legal() {
        // Retail files carry 66 empty paths (e.g. `decal_zigzag_l`,
        // `r4i_rails_f`); an empty path stamps nothing and is not an
        // anomaly.
        let d = file(&[path_bytes("decal_zigzag_l", 0, &[], 2, 20)], 0);
        let ps = Pathset::parse(&d).unwrap();
        assert!(ps.paths[0].points.is_empty());
        assert!(ps.validate().is_empty());
    }

    #[test]
    fn validate_reports_authored_anomalies() {
        let d = file(
            &[
                path_bytes("", 0, &[], 0, 0),
                path_bytes("x", 0, &[(0, [0.0, 0.0, 0.0])], 17, 0),
                path_bytes("y", 0, &[(0, [0.0, 0.0, 0.0])], 1, 0),
                path_bytes("z", 0, &[(0, [f32::NAN, 0.0, 0.0])], 0, 0),
            ],
            9,
        );
        let ps = Pathset::parse(&d).unwrap();
        let issues = ps.validate();
        assert!(issues.contains(&PathsetIssue::EmptyName { path: 0 }));
        assert!(issues.contains(&PathsetIssue::UnknownPathKind { path: 1, kind: 17 }));
        assert!(issues.contains(&PathsetIssue::DirectedOddPointCount { path: 2, points: 1 }));
        assert!(issues.contains(&PathsetIssue::NonFinitePoint { path: 3, point: 0 }));
        assert!(issues.contains(&PathsetIssue::CurrentPathOutOfRange {
            current_path: 9,
            paths: 4
        }));
    }

    #[test]
    fn names_use_all_32_bytes_when_unterminated() {
        let name = "ab".repeat(16); // 32 chars, no room for NUL
        let d = file(&[path_bytes(&name, 0, &[], 0, 0)], 0);
        let ps = Pathset::parse(&d).unwrap();
        assert_eq!(ps.paths[0].name, name);
    }
}
