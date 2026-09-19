//! Parser for MM2 ASCII bound files (`bound/<id>_bound.bnd`).
//!
//! ```text
//! version: 1.01
//! verts: 16
//! materials: 1
//! edges: 0
//! polys: 14
//!
//! v <x> <y> <z>
//! ...
//! mtl default {
//!   elasticity: 0.100000
//!   friction: 0.500000
//!   effect: none
//!   sound: 0
//! }
//! quad 0 1 2 3 0
//! tri 3 2 8 0
//! ```
//!
//! Polygon lines carry vertex indices (0-based) followed by a material index.
//! A binary variant of the same data exists; this parser detects and rejects
//! it rather than misreading it as text.

use std::fmt;

/// Parsed bound file.
#[derive(Debug, Clone)]
pub struct BndFile {
    pub version: String,
    pub verts: Vec<[f32; 3]>,
    pub materials: Vec<BndMaterial>,
    /// Edge index pairs (from `e` lines), usually empty for car bounds.
    pub edges: Vec<[u32; 2]>,
    /// Polygon vertex indices (3 or 4 entries) with material index.
    pub polys: Vec<BndPoly>,
}

/// A `mtl name { ... }` block.
#[derive(Debug, Clone)]
pub struct BndMaterial {
    pub name: String,
    pub elasticity: Option<f32>,
    pub friction: Option<f32>,
    /// Other `key: value` lines preserved verbatim.
    pub extra: Vec<(String, String)>,
}

/// One `quad`/`tri` line.
#[derive(Debug, Clone)]
pub struct BndPoly {
    pub indices: Vec<u32>,
    pub material: u32,
}

/// Bound parse error.
#[derive(Debug)]
pub struct BndError {
    pub line: u32,
    pub message: String,
}

impl fmt::Display for BndError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for BndError {}

impl BndFile {
    /// Parse bound text. Returns an error for non-text/binary input.
    pub fn parse(input: &str) -> Result<Self, BndError> {
        if input.contains('\0') {
            return Err(BndError {
                line: 0,
                message: "binary bound data is not supported".into(),
            });
        }

        let mut file = BndFile {
            version: String::new(),
            verts: Vec::new(),
            materials: Vec::new(),
            edges: Vec::new(),
            polys: Vec::new(),
        };

        let mut declared_verts = None;
        let mut declared_polys = None;

        let mut lines = input.lines().enumerate().peekable();
        while let Some((idx, raw)) = lines.next() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut words = trimmed.split_whitespace();
            match words.next().unwrap_or("") {
                "v" => {
                    let p = parse_floats(&mut words, 3, line)?;
                    file.verts.push([p[0], p[1], p[2]]);
                }
                "quad" => {
                    let idxs = parse_poly(&mut words, 4, line, file.verts.len())?;
                    file.polys.push(idxs);
                }
                "tri" => {
                    let idxs = parse_poly(&mut words, 3, line, file.verts.len())?;
                    file.polys.push(idxs);
                }
                "e" => {
                    let a = parse_u32(words.next(), line)?;
                    let b = parse_u32(words.next(), line)?;
                    file.edges.push([a, b]);
                }
                "mtl" => {
                    // `mtl <name> {` possibly with `{` on the same or next line.
                    let name = words.next().unwrap_or("default").to_string();
                    let mut rest: Vec<&str> = words.collect();
                    if rest.first() != Some(&"{") {
                        // Expect `{` on a following line.
                        loop {
                            match lines.next() {
                                Some((_, l)) if l.trim() == "{" => break,
                                Some((n, l)) if l.trim().is_empty() => {
                                    let _ = n;
                                    continue;
                                }
                                Some((n, l)) => {
                                    return Err(BndError {
                                        line: n as u32 + 1,
                                        message: format!(
                                            "expected '{{' after mtl, found {:?}",
                                            l.trim()
                                        ),
                                    });
                                }
                                None => {
                                    return Err(BndError {
                                        line,
                                        message: "unterminated mtl header".into(),
                                    });
                                }
                            }
                        }
                    } else {
                        rest.remove(0);
                    }

                    let mut mtl = BndMaterial {
                        name,
                        elasticity: None,
                        friction: None,
                        extra: Vec::new(),
                    };
                    loop {
                        let Some((midx, mraw)) = lines.next() else {
                            return Err(BndError {
                                line,
                                message: "unterminated mtl block".into(),
                            });
                        };
                        let mt = mraw.trim();
                        if mt == "}" {
                            break;
                        }
                        if mt.is_empty() {
                            continue;
                        }
                        let mline = (midx + 1) as u32;
                        if let Some((k, v)) = mt.split_once(':') {
                            let k = k.trim();
                            let v = v.trim();
                            match k {
                                "elasticity" => {
                                    mtl.elasticity = Some(v.parse().map_err(|_| BndError {
                                        line: mline,
                                        message: format!("bad elasticity value {v:?}"),
                                    })?);
                                }
                                "friction" => {
                                    mtl.friction = Some(v.parse().map_err(|_| BndError {
                                        line: mline,
                                        message: format!("bad friction value {v:?}"),
                                    })?);
                                }
                                _ => mtl.extra.push((k.to_string(), v.to_string())),
                            }
                        } else {
                            mtl.extra.push((mt.to_string(), String::new()));
                        }
                    }
                    file.materials.push(mtl);
                }
                word => {
                    // Header `key: value` lines (the colon may be attached).
                    let key = word.trim_end_matches(':');
                    match key {
                        "version" => {
                            file.version = words.next().unwrap_or("").to_string();
                        }
                        "verts" => {
                            declared_verts = Some(parse_u32(words.next(), line)?);
                        }
                        "polys" => {
                            declared_polys = Some(parse_u32(words.next(), line)?);
                        }
                        "materials" | "edges" => {
                            let _ = parse_u32(words.next(), line)?;
                        }
                        _ => {
                            return Err(BndError {
                                line,
                                message: format!("unrecognised bound line {trimmed:?}"),
                            });
                        }
                    }
                }
            }
        }

        if let Some(n) = declared_verts
            && file.verts.len() != n as usize
        {
            return Err(BndError {
                line: 0,
                message: format!("declared {n} verts but parsed {}", file.verts.len()),
            });
        }
        if let Some(n) = declared_polys
            && file.polys.len() != n as usize
        {
            return Err(BndError {
                line: 0,
                message: format!("declared {n} polys but parsed {}", file.polys.len()),
            });
        }
        Ok(file)
    }

    /// Combined axis-aligned bounds of all vertices.
    pub fn aabb(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        if self.verts.is_empty() {
            return None;
        }
        for v in &self.verts {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
        Some((min, max))
    }
}

fn parse_floats<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    n: usize,
    line: u32,
) -> Result<Vec<f32>, BndError> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let w = words.next().ok_or(BndError {
            line,
            message: "unexpected end of line".into(),
        })?;
        out.push(w.parse().map_err(|_| BndError {
            line,
            message: format!("bad float {w:?}"),
        })?);
    }
    Ok(out)
}

fn parse_u32(w: Option<&str>, line: u32) -> Result<u32, BndError> {
    let w = w.ok_or(BndError {
        line,
        message: "unexpected end of line".into(),
    })?;
    w.parse().map_err(|_| BndError {
        line,
        message: format!("bad integer {w:?}"),
    })
}

fn parse_poly<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    n: usize,
    line: u32,
    nverts: usize,
) -> Result<BndPoly, BndError> {
    let mut indices = Vec::with_capacity(n);
    for _ in 0..n {
        let i = parse_u32(words.next(), line)? as usize;
        if i >= nverts {
            return Err(BndError {
                line,
                message: format!("vertex index {i} out of range ({nverts} verts)"),
            });
        }
        indices.push(i as u32);
    }
    let material = parse_u32(words.next(), line)?;
    Ok(BndPoly { indices, material })
}
