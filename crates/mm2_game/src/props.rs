//! Roadside-prop stamping (F03-B.3): the PSDL `prop_rule` byte plus the
//! city's `proprules.csv`/`propdefs.csv` tables place the props that
//! line every sidewalk — street lights, trees, parking meters,
//! mailboxes, benches.
//!
//! Geometry, measured on retail `city/{london,sf}.psdl` (see
//! `docs/research/proprules.md`):
//!
//! - A [`mm2_formats::psdl::RoomPath`] is one road run between two
//!   junction crossings. `road_rooms` lists the road rooms it passes
//!   through; consecutive entries share a direct road-to-road
//!   boundary — a junction sits between two paths, never inside one.
//! - `start_crossroads`/`end_crossroads` name the two curb vertices
//!   of the junction crossing at each path end.
//! - A crossing occupies four consecutive perimeter points —
//!   `[outer, curb, curb, outer]` (outer = building-line corner, curb
//!   = road-edge corner). At a path end the curb pair is given; at a
//!   direct road-to-road boundary it is the *widest* consecutive pair
//!   of perimeter points marked with the neighbouring room's id (the
//!   curb gap spans the road; the corner–curb gap only the sidewalk).
//! - The two perimeter arcs between the entry and exit crossings are
//!   the sidewalk building lines; each pairs with the curb segment
//!   joining the crossings' curbs on that side.
//!
//! Placement policy — inferred, not verified (UNK-21):
//!
//! - Each side is walked so the road stays on the walker's left: the
//!   side on the right of travel runs entry→exit, the side on the
//!   left runs exit→entry. `start`/`distance` are metres along the
//!   side's curb segment from its walk-start crossing; `maxUse` caps
//!   placements per def, per side, per room.
//! - A stamp at offset `s` sits at `lerp(curb(u), outer(u))` — `u` the
//!   curb fraction `s / curb_len`, `outer` arc-length-parametrized —
//!   with lerp factor `(minLerp + maxLerp) / 2` (the two are equal on
//!   every retail row; 0.1 ≈ curb-hugging, 0.5 ≈ mid-sidewalk).
//! - `n{NN}left`/`n{NN}right` rows are assigned by which side of
//!   travel each arc lies on, using the authored left-handed
//!   convention `right(d) = (d.z, -d.x)`. If retail comparison shows
//!   the labels swapped the assignment is a one-line flip.
//! - `file1`–`file4` variants are picked by a deterministic hash of
//!   (path, room, side, def, index); the original's `RandomSeed`
//!   selection is unrecovered.
//! - A stamp faces its walk direction (`forward`); the app yaws the
//!   prop's +X axis along it like a directed pathset stamp.

use mm2_formats::{
    proprules::{PropDefs, PropRuleSide, PropRules},
    psdl::Psdl,
};

/// Hard bound on stamps one walk may emit — authored data is bounded,
/// but a hostile table could request `maxUse` placements at near-zero
/// `distance`; overflow is counted, not silently dropped.
pub const MAX_PROP_RULE_STAMPS: usize = 200_000;
/// Bounded diagnostics: a corrupt file can raise an issue per room.
const MAX_ISSUES: usize = 64;

/// One placed prop, in authored city coordinates.
#[derive(Debug, Clone)]
pub struct PropStamp {
    /// Index of the PSDL path the stamp came from.
    pub path: usize,
    /// 1-based room id (the PSDL room index + 1).
    pub room: u16,
    /// The room's `prop_rule` byte.
    pub rule: u8,
    /// Which rule row the stamp's side resolved to.
    pub side: PropRuleSide,
    /// `propdefs.csv` prototype name.
    pub def: String,
    /// Chosen `file1`–`file4` variant (PKG basename).
    pub pkg: String,
    /// Placement ordinal within the def's row on this side.
    pub index: u32,
    /// Placement position, authored coordinates.
    pub position: [f32; 3],
    /// Walk direction the prop faces (XZ-normalized).
    pub forward: [f32; 3],
}

/// What one walk produced and skipped, classified — every room on a
/// path lands in a count, nothing is dropped silently.
#[derive(Debug, Default)]
pub struct PropWalkStats {
    /// Path records inspected.
    pub paths: usize,
    /// `road_rooms` entries visited.
    pub rooms_on_paths: usize,
    /// Rooms that stamped at least one side.
    pub rooms_stamped: usize,
    /// Rooms whose `prop_rule` byte is 0 (no roadside props).
    pub rooms_no_rule: usize,
    /// `road_rooms` entries that are 0 or out of range — an encoded
    /// record kind the walk does not interpret, counted not guessed.
    pub rooms_bad_ref: usize,
    /// Rule-bearing rooms whose entry/exit crossings could not be
    /// resolved from the crossing vertices or boundary marks.
    pub rooms_no_crossing: usize,
    /// Rule-bearing rooms whose `prop_rule` byte names no
    /// `n{NN}left`/`n{NN}right` rows (the retail byte-205 anomaly).
    pub rules_missing: usize,
    /// Rule-row prop names that resolved no `propdefs.csv` entry.
    pub defs_missing: usize,
    /// Stamps suppressed by [`MAX_PROP_RULE_STAMPS`].
    pub stamps_capped: usize,
    /// `prop_rule`-bearing rooms no path reaches (kept visible — they
    /// carry authored rules the path relation does not cover).
    pub rule_rooms_unreached: usize,
    /// Human-readable anomalies, bounded at [`MAX_ISSUES`].
    pub issues: Vec<String>,
}

/// The walk's output: stamped placements plus the classified stats.
#[derive(Debug, Default)]
pub struct PropWalk {
    /// Stamped placements in path order.
    pub stamps: Vec<PropStamp>,
    /// Classified counters.
    pub stats: PropWalkStats,
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Deterministic variant pick — FNV-1a over the stamp's identity.
fn variant_hash(path: usize, room: u16, side: PropRuleSide, def: &str, k: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for b in path.to_le_bytes() {
        mix(b);
    }
    for b in room.to_le_bytes() {
        mix(b);
    }
    mix(match side {
        PropRuleSide::Left => b'L',
        PropRuleSide::Right => b'R',
    });
    for b in def.bytes() {
        mix(b);
    }
    for b in k.to_le_bytes() {
        mix(b);
    }
    h
}

/// The resolved run of four perimeter indices
/// `[outer_a, curb_a, curb_b, outer_b]` in perimeter order.
type Run = [usize; 4];

/// Locate the crossing run whose curb pair is the given vertex pair —
/// the pair occupies the run's two middle positions, adjacent in
/// either order. Returns `None` when the vertices are missing or not
/// adjacent (counted by the caller, not guessed).
fn junction_run(psdl: &Psdl, room: usize, v0: u16, v1: u16) -> Option<Run> {
    let perim = &psdl.rooms[room].perimeter;
    let n = perim.len();
    if n < 4 {
        return None;
    }
    for (p, a) in perim.iter().enumerate() {
        if a.vertex != v0 {
            continue;
        }
        for (q, b) in perim.iter().enumerate() {
            if b.vertex != v1 {
                continue;
            }
            if (p + 1) % n == q {
                return Some([(p + n - 1) % n, p, q, (q + 1) % n]);
            }
            if (q + 1) % n == p {
                return Some([(q + n - 1) % n, q, p, (p + 1) % n]);
            }
        }
    }
    None
}

/// Locate the direct road-to-road boundary toward `rid`: the 4-point
/// run whose middle two points (the curb pair) are marked `rid`. The
/// marked corner points sit one sidewalk-width from their curb, so
/// the curb pair is the *widest* consecutive `rid`-marked pair.
fn boundary_run(psdl: &Psdl, room: usize, rid: u16) -> Option<Run> {
    let perim = &psdl.rooms[room].perimeter;
    let n = perim.len();
    if n < 4 {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    for i in 0..n {
        let j = (i + 1) % n;
        if perim[i].room == rid && perim[j].room == rid {
            let (Some(&a), Some(&b)) = (
                psdl.vertices.get(perim[i].vertex as usize),
                psdl.vertices.get(perim[j].vertex as usize),
            ) else {
                continue;
            };
            let d = dist(a, b);
            if best.is_none_or(|(_, bd)| d > bd) {
                best = Some((i, d));
            }
        }
    }
    let (i, _) = best?;
    Some([(i + n - 1) % n, i, (i + 1) % n, (i + 2) % n])
}

/// Perimeter vertex positions along the cycle from index `a` to `b`
/// inclusive (perimeter order).
fn chain(psdl: &Psdl, room: usize, a: usize, b: usize) -> Option<Vec<[f32; 3]>> {
    let perim = &psdl.rooms[room].perimeter;
    let n = perim.len();
    let mut out = Vec::new();
    let mut i = a;
    loop {
        out.push(*psdl.vertices.get(perim[i].vertex as usize)?);
        if i == b {
            return Some(out);
        }
        i = (i + 1) % n;
    }
}

/// Point at fraction `t` of a polyline's total arc length.
fn polyline_at(poly: &[[f32; 3]], lens: &[f32], total: f32, t: f32) -> [f32; 3] {
    if poly.len() < 2 || total <= f32::EPSILON {
        return poly.first().copied().unwrap_or([0.0; 3]);
    }
    let target = t.clamp(0.0, 1.0) * total;
    for (i, w) in poly.windows(2).enumerate() {
        let (s0, s1) = (lens[i], lens[i + 1]);
        if target <= s1 || i + 2 == poly.len() {
            let u = if s1 > s0 {
                (target - s0) / (s1 - s0)
            } else {
                0.0
            };
            return lerp3(w[0], w[1], u.clamp(0.0, 1.0));
        }
    }
    *poly.last().unwrap()
}

fn arc_lengths(poly: &[[f32; 3]]) -> (Vec<f32>, f32) {
    let mut lens = Vec::with_capacity(poly.len());
    lens.push(0.0);
    for w in poly.windows(2) {
        lens.push(lens.last().unwrap() + dist(w[0], w[1]));
    }
    let total = *lens.last().unwrap();
    (lens, total)
}

/// One side of a room resolved for stamping: the curb segment and the
/// building-line arc, both in walk order (walk-start → walk-end).
struct Side {
    which: PropRuleSide,
    curb_a: [f32; 3],
    curb_b: [f32; 3],
    outer: Vec<[f32; 3]>,
    forward: [f32; 3],
}

/// Stamp every rule-bearing room reached by the PSDL prop paths.
///
/// `defs`/`rules` are the city's `propdefs.csv`/`proprules.csv`
/// tables; their own `diagnostics`/`validate()` findings are the
/// caller's to report — the walk consumes what resolves and counts
/// the rest per room.
pub fn walk_prop_rules(psdl: &Psdl, defs: &PropDefs, rules: &PropRules) -> PropWalk {
    let mut walk = PropWalk {
        stamps: Vec::new(),
        stats: PropWalkStats {
            paths: psdl.paths.len(),
            ..PropWalkStats::default()
        },
    };
    let issue = |stats: &mut PropWalkStats, msg: String| {
        if stats.issues.len() < MAX_ISSUES {
            stats.issues.push(msg);
        }
    };

    let def_of = |name: &str| defs.defs.iter().find(|d| d.name == name);
    let rule_of = |num: u8, side: PropRuleSide| {
        rules
            .rules
            .iter()
            .find(|r| r.rule_key() == Some((num, side)))
    };

    let mut reached = vec![false; psdl.prop_rules.len()];
    let mut stamps_left = MAX_PROP_RULE_STAMPS;
    for (pi, path) in psdl.paths.iter().enumerate() {
        for (i, &rid) in path.road_rooms.iter().enumerate() {
            walk.stats.rooms_on_paths += 1;
            if rid == 0 || rid as usize > psdl.rooms.len() {
                walk.stats.rooms_bad_ref += 1;
                issue(
                    &mut walk.stats,
                    format!("path {pi}: road_rooms[{i}] = {rid} is not a room id"),
                );
                continue;
            }
            let ri = (rid - 1) as usize;
            reached[rid as usize] = true;
            let rule = psdl.prop_rules.get(rid as usize).copied().unwrap_or(0);
            if rule == 0 {
                walk.stats.rooms_no_rule += 1;
                continue;
            }
            if rule_of(rule, PropRuleSide::Left).is_none()
                && rule_of(rule, PropRuleSide::Right).is_none()
            {
                walk.stats.rules_missing += 1;
                issue(
                    &mut walk.stats,
                    format!("path {pi} room {rid}: rule n{rule:02} has no left/right rows"),
                );
                continue;
            }

            // Entry/exit crossings: path ends use the authored curb
            // pair; interior boundaries use the neighbour-room marks.
            // An insane neighbour id resolves no crossing — the marks
            // are never searched for 0 (every unmarked point) or an
            // out-of-range room.
            let sane = |r: u16| r != 0 && (r as usize) <= psdl.rooms.len();
            let boundary = |r: u16| {
                if sane(r) {
                    boundary_run(psdl, ri, r)
                } else {
                    None
                }
            };
            let entry = if i == 0 {
                junction_run(psdl, ri, path.start_crossroads[0], path.start_crossroads[1])
            } else {
                boundary(path.road_rooms[i - 1])
            };
            let exit = if i + 1 == path.road_rooms.len() {
                junction_run(psdl, ri, path.end_crossroads[0], path.end_crossroads[1])
            } else {
                boundary(path.road_rooms[i + 1])
            };
            let (Some(e), Some(x)) = (entry, exit) else {
                walk.stats.rooms_no_crossing += 1;
                issue(
                    &mut walk.stats,
                    format!("path {pi} room {rid}: crossing pair not found on the perimeter"),
                );
                continue;
            };

            let perim = &psdl.rooms[ri].perimeter;
            let v = |idx: usize| psdl.vertices.get(perim[idx].vertex as usize).copied();
            // `e_outer_a`/`x_outer_b` are the arcs' endpoints — the
            // `chain` call resolves them again, but extracting all
            // eight here keeps a bad vertex ref in one place.
            let (Some(_e_outer_a), Some(e_curb_a), Some(e_curb_b), Some(_e_outer_b)) =
                (v(e[0]), v(e[1]), v(e[2]), v(e[3]))
            else {
                walk.stats.rooms_no_crossing += 1;
                continue;
            };
            let (Some(_x_outer_a), Some(x_curb_a), Some(x_curb_b), Some(_x_outer_b)) =
                (v(x[0]), v(x[1]), v(x[2]), v(x[3]))
            else {
                walk.stats.rooms_no_crossing += 1;
                continue;
            };
            let (Some(arc_a), Some(arc_b)) =
                (chain(psdl, ri, e[3], x[0]), chain(psdl, ri, x[3], e[0]))
            else {
                walk.stats.rooms_no_crossing += 1;
                continue;
            };

            // Travel direction, XZ, authored left-handed convention:
            // right(d) = (d.z, -d.x) (facing +z → right = +x).
            let centre = lerp3(
                lerp3(e_curb_a, e_curb_b, 0.5),
                lerp3(x_curb_a, x_curb_b, 0.5),
                0.5,
            );
            let dvec = sub(
                lerp3(x_curb_a, x_curb_b, 0.5),
                lerp3(e_curb_a, e_curb_b, 0.5),
            );
            let dlen = (dvec[0] * dvec[0] + dvec[2] * dvec[2]).sqrt();
            let d = if dlen > f32::EPSILON {
                [dvec[0] / dlen, 0.0, dvec[2] / dlen]
            } else {
                walk.stats.rooms_no_crossing += 1;
                continue;
            };
            let right = [d[2], 0.0, -d[0]];

            // Which arc sits on which side of travel. Arc A pairs
            // curbs (e_curb_b → x_curb_a); arc B pairs
            // (e_curb_a → x_curb_b). The perimeter's winding decides
            // which arc is left of travel, so the label is measured,
            // not assumed.
            let mid_a = lerp3(e_curb_b, x_curb_a, 0.5);
            let off_a = sub(mid_a, centre);
            let a_right = off_a[0] * right[0] + off_a[2] * right[2] >= 0.0;

            // Each side is walked so the road stays on the walker's
            // left: a right-of-travel side runs entry→exit, a
            // left-of-travel side runs exit→entry — and `start`
            // measures from the crossing its walk begins at.
            // `arc` arrives in travel order (entry → exit); the walk
            // keeps it for a right-of-travel side and reverses it for
            // a left-of-travel side.
            let side = |which: PropRuleSide,
                        entry_curb: [f32; 3],
                        exit_curb: [f32; 3],
                        arc: Vec<[f32; 3]>,
                        walk_with_travel: bool| {
                let (curb_a, curb_b, outer, forward) = if walk_with_travel {
                    (entry_curb, exit_curb, arc, d)
                } else {
                    let mut rev = arc;
                    rev.reverse();
                    (exit_curb, entry_curb, rev, [-d[0], 0.0, -d[2]])
                };
                Side {
                    which,
                    curb_a,
                    curb_b,
                    outer,
                    forward,
                }
            };
            // `arc_b` is stored exit→entry (chain x3 → e0); reversed
            // it is the travel-order outer line e_outer_a → x_outer_b.
            let mut arc_b_fwd = arc_b;
            arc_b_fwd.reverse();
            let sides = [
                side(
                    if a_right {
                        PropRuleSide::Right
                    } else {
                        PropRuleSide::Left
                    },
                    e_curb_b,
                    x_curb_a,
                    arc_a,
                    a_right,
                ),
                side(
                    if a_right {
                        PropRuleSide::Left
                    } else {
                        PropRuleSide::Right
                    },
                    e_curb_a,
                    x_curb_b,
                    arc_b_fwd,
                    !a_right,
                ),
            ];

            let mut stamped_room = false;
            for side in &sides {
                let curb_len = dist(side.curb_a, side.curb_b);
                if curb_len <= f32::EPSILON {
                    continue;
                }
                let (lens, _) = arc_lengths(&side.outer);
                let outer_total = *lens.last().unwrap_or(&0.0);
                let Some(row) = rule_of(rule, side.which) else {
                    continue;
                };
                for name in &row.props {
                    let Some(def) = def_of(name) else {
                        walk.stats.defs_missing += 1;
                        continue;
                    };
                    if def.distance <= 0.0 || def.files.is_empty() || def.max_use <= 0 {
                        continue;
                    }
                    // Offsets s = start + k·distance ≤ curb_len,
                    // counted arithmetically so a hostile def is
                    // measured against the budget instead of walked.
                    let want = if def.start <= curb_len {
                        (((curb_len - def.start) / def.distance) as u64) + 1
                    } else {
                        0
                    }
                    .min(def.max_use as u64);
                    let take = want.min(stamps_left as u64);
                    walk.stats.stamps_capped = walk
                        .stats
                        .stamps_capped
                        .saturating_add((want - take) as usize);
                    stamps_left -= take as usize;
                    let lerp = (def.lerp_min + def.lerp_max) * 0.5;
                    for k in 0..take {
                        let s = def.start + k as f32 * def.distance;
                        let u = s / curb_len;
                        let curb = lerp3(side.curb_a, side.curb_b, u);
                        let outer = polyline_at(&side.outer, &lens, outer_total, u);
                        let position = lerp3(curb, outer, lerp);
                        let pkg = &def.files[(variant_hash(pi, rid, side.which, &def.name, k)
                            as usize)
                            % def.files.len()];
                        walk.stamps.push(PropStamp {
                            path: pi,
                            room: rid,
                            rule,
                            side: side.which,
                            def: def.name.clone(),
                            pkg: pkg.clone(),
                            index: k as u32,
                            position,
                            forward: side.forward,
                        });
                        stamped_room = true;
                    }
                }
            }
            if stamped_room {
                walk.stats.rooms_stamped += 1;
            }
        }
    }

    for (r, &rule) in psdl.prop_rules.iter().enumerate() {
        if rule != 0 && !reached[r] {
            walk.stats.rule_rooms_unreached += 1;
        }
    }
    walk
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::{
        proprules::{PropDef, PropRule},
        psdl::{PerimeterPoint, PsdlRoom, RoomPath},
    };

    /// Two quad road rooms sharing the z = 20 boundary: room 1 spans
    /// z 0–20, room 2 spans z 20–40; the road runs x 2–28 with the
    /// building lines at x = 0 and x = 30. Perimeters wind CCW in the
    /// (x, z) plane like the retail rooms.
    fn quad_verts() -> Vec<[f32; 3]> {
        vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [28., 0., 0.],
            [30., 0., 0.], // room 1 entry (z = 0)
            [30., 0., 20.],
            [28., 0., 20.],
            [2., 0., 20.],
            [0., 0., 20.], // shared boundary (z = 20)
            [30., 0., 40.],
            [28., 0., 40.],
            [2., 0., 40.],
            [0., 0., 40.], // room 2 exit (z = 40)
        ]
    }

    fn room(perim: &[(u16, u16)]) -> PsdlRoom {
        PsdlRoom {
            perimeter: perim
                .iter()
                .map(|&(vertex, room)| PerimeterPoint { vertex, room })
                .collect(),
            attributes: Vec::new(),
            unparsed_attributes: Vec::new(),
        }
    }

    fn path(xr_start: [u16; 4], xr_end: [u16; 4], rooms: &[u16]) -> RoomPath {
        RoomPath {
            unknown4: 0,
            unknown5: 0,
            density: Vec::new(),
            unknown6: 0,
            start_crossroads: xr_start,
            end_crossroads: xr_end,
            road_rooms: rooms.to_vec(),
        }
    }

    fn psdl(rooms: Vec<PsdlRoom>, rules: &[u8], paths: Vec<RoomPath>) -> Psdl {
        Psdl {
            target_size: 2,
            vertices: quad_verts(),
            heights: Vec::new(),
            textures: Vec::new(),
            rooms,
            room_flags: vec![0; rules.len()],
            prop_rules: rules.to_vec(),
            junction_count: 0,
            bounds_min: [0.; 3],
            bounds_max: [0.; 3],
            bounds_center: [0.; 3],
            bounds_radius: 0.,
            paths,
        }
    }

    fn def(name: &str, start: f32, distance: f32, max_use: i64, files: &[&str]) -> PropDef {
        PropDef {
            name: name.into(),
            start,
            distance,
            max_use,
            lerp_min: 0.5,
            lerp_max: 0.5,
            files: files.iter().map(|s| s.to_string()).collect(),
            line: 0,
        }
    }

    fn rule(name: &str, props: &[&str]) -> PropRule {
        PropRule {
            name: name.into(),
            props: props.iter().map(|s| s.to_string()).collect(),
            line: 0,
        }
    }

    fn tables(defs: Vec<PropDef>, rules: Vec<PropRule>) -> (PropDefs, PropRules) {
        (
            PropDefs {
                defs,
                diagnostics: Vec::new(),
            },
            PropRules {
                rules,
                diagnostics: Vec::new(),
            },
        )
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        dist(a, b) < 1e-3
    }

    /// Room 1's perimeter, marks cleared (junction crossings at both
    /// ends — the single-room-path case).
    fn room1_solo() -> PsdlRoom {
        room(&[
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
            (7, 0),
        ])
    }

    #[test]
    fn a_single_room_path_stamps_both_sides() {
        // n01right = lamps on the x≈30 side (right of +z travel),
        // n01left = meters on the x≈0 side walked exit→entry.
        let city = psdl(
            vec![room1_solo()],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![
                def("meter", 5., 10., 99, &["pa"]),
                def("lamp", 2., 6., 2, &["pb"]),
            ],
            vec![
                rule("n01left", &["meter"]),
                rule("n01right", &["lamp", "ghost"]),
            ],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stats.rooms_stamped, 1);
        assert_eq!(walk.stats.defs_missing, 1, "ghost has no propdef");
        assert_eq!(walk.stats.rule_rooms_unreached, 0);
        assert_eq!(walk.stamps.len(), 4);

        let lamp = |s: &PropStamp| s.side == PropRuleSide::Right;
        assert!(near(walk.stamps[0].position, [29., 0., 2.]));
        assert!(near(walk.stamps[1].position, [29., 0., 8.]));
        assert!(walk.stamps[0..2].iter().all(lamp));
        assert!(near(walk.stamps[0].forward, [0., 0., 1.]));
        assert_eq!(walk.stamps[0].pkg, "pb");

        // The left side walks backward from the exit crossing: start=5
        // measures 5 m back along the curb from z = 20.
        assert_eq!(walk.stamps[2].side, PropRuleSide::Left);
        assert!(near(walk.stamps[2].position, [1., 0., 15.]));
        assert!(near(walk.stamps[3].position, [1., 0., 5.]));
        assert!(near(walk.stamps[2].forward, [0., 0., -1.]));
    }

    #[test]
    fn start_beyond_the_curb_and_max_use_bound_the_row() {
        let city = psdl(
            vec![room1_solo()],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![
                def("far", 25., 10., 99, &["pa"]), // start past curb_len
                def("one", 0., 5., 1, &["pa"]),    // capped by maxUse
            ],
            vec![rule("n01left", &["far"]), rule("n01right", &["one"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stamps.len(), 1);
        assert_eq!(walk.stamps[0].def, "one");
    }

    #[test]
    fn a_multi_room_path_walks_the_interior_boundary_marks() {
        // Room 1's exit run and room 2's entry run are the shared
        // z = 20 boundary, identified by neighbour marks — the curb
        // pair is the widest consecutive marked pair.
        let r1 = room(&[
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 2),
            (5, 2),
            (6, 2),
            (7, 0),
        ]);
        let r2 = room(&[
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 0),
            (11, 0),
            (10, 0),
            (9, 0),
            (8, 0),
        ]);
        let city = psdl(
            vec![r1, r2],
            &[0, 1, 1],
            vec![path([1, 2, 0, 0], [10, 9, 0, 0], &[1, 2])],
        );
        let (defs, rules) = tables(
            vec![
                def("meter", 5., 10., 99, &["pa"]),
                def("lamp", 2., 6., 2, &["pb"]),
            ],
            vec![rule("n01left", &["meter"]), rule("n01right", &["lamp"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stats.rooms_stamped, 2, "{:?}", walk.stats.issues);
        assert_eq!(walk.stamps.len(), 8);
        // Room 2's perimeter runs the other way at the shared edge, so
        // its left side is arc A — the labels still land on the same
        // physical sides.
        let r2_right: Vec<&PropStamp> = walk
            .stamps
            .iter()
            .filter(|s| s.room == 2 && s.side == PropRuleSide::Right)
            .collect();
        assert!(near(r2_right[0].position, [29., 0., 22.]));
        assert!(near(r2_right[1].position, [29., 0., 28.]));
        assert!(near(r2_right[0].forward, [0., 0., 1.]));
        let r2_left: Vec<&PropStamp> = walk
            .stamps
            .iter()
            .filter(|s| s.room == 2 && s.side == PropRuleSide::Left)
            .collect();
        assert!(near(r2_left[0].position, [1., 0., 35.]));
        assert!(near(r2_left[1].position, [1., 0., 25.]));
        assert!(near(r2_left[0].forward, [0., 0., -1.]));
    }

    #[test]
    fn zero_rules_bad_refs_and_undefined_bytes_are_counted() {
        let city = psdl(
            vec![room1_solo(), room1_solo()],
            // room 1: no rule; room 2: byte 205 (the retail anomaly);
            // room 4: rule-bearing but on no path.
            &[0, 0, 205, 1],
            vec![
                path([1, 2, 0, 0], [5, 6, 0, 0], &[1, 0, 999]),
                path([1, 2, 0, 0], [5, 6, 0, 0], &[2]),
            ],
        );
        let (defs, rules) = tables(
            vec![def("meter", 5., 10., 99, &["pa"])],
            vec![rule("n01left", &["meter"]), rule("n01right", &["meter"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stats.rooms_no_rule, 1);
        assert_eq!(walk.stats.rooms_bad_ref, 2, "0 and 999 are not rooms");
        assert_eq!(walk.stats.rules_missing, 1);
        assert_eq!(walk.stats.rule_rooms_unreached, 1);
        assert!(walk.stamps.is_empty());
    }
}
