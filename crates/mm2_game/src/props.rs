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
//!   the sidewalk building lines. The *kerb* between a side's two curb
//!   corners is not on the perimeter — it lives in the room's road
//!   attributes (`RoadWithSidewalks`/`DividedRoad`/`SidewalkStrip`/
//!   `RoadNoSidewalks`) as the road-edge vertex chain, which bends with
//!   the authored surface. Walking a straight chord between the curb
//!   corners cuts into the carriageway on every curved block.
//!
//! Placement policy — inferred, not verified (UNK-21):
//!
//! - Each side is walked so the road stays on the walker's left: the
//!   side on the right of travel runs entry→exit, the side on the
//!   left runs exit→entry. `start`/`distance` are metres along the
//!   side's kerb chain from its walk-start crossing; `maxUse` caps
//!   placements per def, per side, per room.
//! - A stamp at offset `s` sits at `lerp(kerb, outer)` evaluated on
//!   the authored strip's cross-section containing `s` — the kerb and
//!   outer chains are index-paired, so the stamp keeps the authored
//!   kerb↔building-line correspondence — with lerp factor
//!   `(minLerp + maxLerp) / 2` (the two are equal on every retail row;
//!   0.1 ≈ curb-hugging, 0.5 ≈ mid-sidewalk).
//! - A side whose kerb chain cannot be resolved (junction rooms whose
//!   strips are fragments, rooms with no road attribute) falls back to
//!   stamping on the building-line arc — never inside the road —
//!   and is counted in `sides_no_kerb`.
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
    psdl::{AttributeType, Psdl, PsdlRoom, RoomAttribute},
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
    /// Sides whose authored kerb chain could not be resolved from the
    /// room's road attributes — stamped on the building-line arc so
    /// they never land in the carriageway.
    pub sides_no_kerb: usize,
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

fn arc_lengths(poly: &[[f32; 3]]) -> (Vec<f32>, f32) {
    let mut lens = Vec::with_capacity(poly.len());
    lens.push(0.0);
    for w in poly.windows(2) {
        lens.push(lens.last().unwrap() + dist(w[0], w[1]));
    }
    let total = *lens.last().unwrap();
    (lens, total)
}

/// Point pair at arc-length `s` along `kerb`, evaluated with the same
/// section index and fraction on `outer` — the two chains are
/// index-paired (section `i` joins `kerb[i]` to `outer[i]`), so the
/// authored kerb↔sidewalk correspondence is preserved.
fn strip_at(kerb: &[[f32; 3]], outer: &[[f32; 3]], lens: &[f32], s: f32) -> ([f32; 3], [f32; 3]) {
    let n = kerb.len();
    if n < 2 {
        let p = kerb.first().copied().unwrap_or([0.0; 3]);
        let q = outer.first().copied().unwrap_or(p);
        return (p, q);
    }
    for (i, w) in lens.windows(2).enumerate().take(n - 1) {
        let (s0, s1) = (w[0], w[1]);
        if s <= s1 || i + 2 == n {
            let u = if s1 > s0 {
                ((s - s0) / (s1 - s0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return (
                lerp3(kerb[i], kerb[i + 1], u),
                lerp3(outer[i], outer[i + 1], u),
            );
        }
    }
    (*kerb.last().unwrap(), *outer.last().unwrap())
}

/// The `counted` layout shared by road, sidewalk and walkway strips:
/// `subtype == 0` → `data = [count, count*per_item refs]`, otherwise
/// `data = subtype*per_item refs`. `None` on a malformed record — the
/// room's city import reports the same attribute as malformed.
fn counted_refs(attr: &RoomAttribute, per_item: usize) -> Option<&[u16]> {
    if attr.subtype == 0 {
        let (&n, rest) = attr.data.split_first()?;
        (rest.len() == n as usize * per_item).then_some(rest)
    } else {
        (attr.data.len() == attr.subtype as usize * per_item).then_some(&attr.data)
    }
}

/// A kerb→outer pair of vertex-id chains extracted from a room's road
/// attribute. `kerb` runs along the road edge, `outer` along the far
/// edge of the sidewalk (the building line); chains are index-paired
/// per cross-section. For a `RoadNoSidewalks` walkway the room edge is
/// both kerb and outer, so a stamp lands on the edge itself.
struct KerbStrip {
    kerb: Vec<u16>,
    outer: Vec<u16>,
}

/// The room's authored kerb strips — the road-edge chains the
/// perimeter does not carry. `RoadWithSidewalks` sections are
/// `[sw_l, road_l, road_r, sw_r]`, `DividedRoad` adds the divider's
/// inner edges `[sw_l, rl_out, rl_in, rr_in, rr_out, sw_r]`, and a
/// `SidewalkStrip` is `(ground, top)` pairs. Layouts mirror the city
/// importer's decoders; malformed attributes are skipped, not guessed.
fn kerb_strips(room: &PsdlRoom) -> Vec<KerbStrip> {
    let mut out = Vec::new();
    for attr in &room.attributes {
        match attr.kind {
            AttributeType::RoadWithSidewalks => {
                let Some(refs) = counted_refs(attr, 4) else {
                    continue;
                };
                let (mut kl, mut ol, mut kr, mut or_) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for s in refs.chunks_exact(4) {
                    ol.push(s[0]);
                    kl.push(s[1]);
                    kr.push(s[2]);
                    or_.push(s[3]);
                }
                out.push(KerbStrip {
                    kerb: kl,
                    outer: ol,
                });
                out.push(KerbStrip {
                    kerb: kr,
                    outer: or_,
                });
            }
            AttributeType::DividedRoad => {
                // [packed, value, subtype×6] inline; [count, packed,
                // value, count×6] counted — same layout as the city
                // importer's DividedRoad arm.
                let refs: &[u16] = if attr.subtype == 0 {
                    if attr.data.len() < 3 || attr.data[3..].len() != attr.data[0] as usize * 6 {
                        continue;
                    }
                    &attr.data[3..]
                } else {
                    if attr.data.len() < 2 || attr.data[2..].len() != attr.subtype as usize * 6 {
                        continue;
                    }
                    &attr.data[2..]
                };
                let (mut kl, mut ol, mut kr, mut or_) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for s in refs.chunks_exact(6) {
                    ol.push(s[0]);
                    kl.push(s[1]);
                    kr.push(s[4]);
                    or_.push(s[5]);
                }
                out.push(KerbStrip {
                    kerb: kl,
                    outer: ol,
                });
                out.push(KerbStrip {
                    kerb: kr,
                    outer: or_,
                });
            }
            AttributeType::SidewalkStrip => {
                let Some(refs) = counted_refs(attr, 2) else {
                    continue;
                };
                // Triangular end-cap piece, not a strip (refs 0–1 are
                // the repeated marker, 2–3 the cap's bottom verts).
                if refs.len() >= 4 && refs[0] == refs[1] && refs[0] <= 1 {
                    continue;
                }
                let (mut kerb, mut outer) = (Vec::new(), Vec::new());
                for s in refs.chunks_exact(2) {
                    kerb.push(s[0]);
                    outer.push(s[1]);
                }
                out.push(KerbStrip { kerb, outer });
            }
            AttributeType::RoadNoSidewalks => {
                let Some(refs) = counted_refs(attr, 2) else {
                    continue;
                };
                let (mut l, mut r) = (Vec::new(), Vec::new());
                for s in refs.chunks_exact(2) {
                    l.push(s[0]);
                    r.push(s[1]);
                }
                out.push(KerbStrip {
                    kerb: l.clone(),
                    outer: l,
                });
                out.push(KerbStrip {
                    kerb: r.clone(),
                    outer: r,
                });
            }
            _ => {}
        }
    }
    out
}

/// A resolved (kerb, outer) position-chain pair.
type Chains = (Vec<[f32; 3]>, Vec<[f32; 3]>);

/// The authored kerb/outer chain pair spanning the side between curb
/// corners `start`→`end` (vertex ids, travel order) — or the same
/// chains reversed when the strip runs the other way. The strip with
/// the longest kerb arc wins when several match; `None` when no
/// authored strip joins the side's two corners.
fn match_strip(psdl: &Psdl, strips: &[KerbStrip], start: u16, end: u16) -> Option<Chains> {
    let arc = |ids: &[u16]| -> Option<f32> {
        let mut len = 0.0;
        for w in ids.windows(2) {
            let (Some(&a), Some(&b)) = (
                psdl.vertices.get(w[0] as usize),
                psdl.vertices.get(w[1] as usize),
            ) else {
                return None;
            };
            len += dist(a, b);
        }
        Some(len)
    };
    let mut best: Option<(usize, bool, f32)> = None;
    for (i, st) in strips.iter().enumerate() {
        if st.kerb.len() != st.outer.len() || st.kerb.len() < 2 {
            continue;
        }
        let (first, last) = (*st.kerb.first().unwrap(), *st.kerb.last().unwrap());
        let reversed = if first == start && last == end {
            false
        } else if first == end && last == start {
            true
        } else {
            continue;
        };
        let Some(len) = arc(&st.kerb) else {
            continue;
        };
        if best.is_none_or(|(_, _, bl)| len > bl) {
            best = Some((i, reversed, len));
        }
    }
    let (i, reversed, _) = best?;
    let st = &strips[i];
    let resolve = |ids: &[u16]| {
        ids.iter()
            .map(|&v| psdl.vertices.get(v as usize).copied())
            .collect::<Option<Vec<_>>>()
    };
    let (mut kerb, mut outer) = (resolve(&st.kerb)?, resolve(&st.outer)?);
    if reversed {
        kerb.reverse();
        outer.reverse();
    }
    Some((kerb, outer))
}

/// One side of a room resolved for stamping: the kerb chain and the
/// sidewalk-outer chain, index-paired, both in walk order
/// (walk-start → walk-end). On the building-line fallback the two
/// chains are the same arc.
struct Side {
    which: PropRuleSide,
    kerb: Vec<[f32; 3]>,
    outer: Vec<[f32; 3]>,
    authored_kerb: bool,
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
            let strips = kerb_strips(&psdl.rooms[ri]);
            let side = |which: PropRuleSide,
                        entry_curb: u16,
                        exit_curb: u16,
                        arc: Vec<[f32; 3]>,
                        walk_with_travel: bool| {
                // The kerb between the side's curb corners: the
                // authored road-edge chain when a strip spans them,
                // otherwise the building-line arc itself — stamps on
                // the arc never land inside the carriageway.
                let (mut kerb, mut outer, authored_kerb) =
                    match match_strip(psdl, &strips, entry_curb, exit_curb) {
                        Some((k, o)) => (k, o, true),
                        None => (arc.clone(), arc, false),
                    };
                let forward = if walk_with_travel {
                    d
                } else {
                    kerb.reverse();
                    outer.reverse();
                    [-d[0], 0.0, -d[2]]
                };
                Side {
                    which,
                    kerb,
                    outer,
                    authored_kerb,
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
                    perim[e[2]].vertex,
                    perim[x[1]].vertex,
                    arc_a,
                    a_right,
                ),
                side(
                    if a_right {
                        PropRuleSide::Left
                    } else {
                        PropRuleSide::Right
                    },
                    perim[e[1]].vertex,
                    perim[x[2]].vertex,
                    arc_b_fwd,
                    !a_right,
                ),
            ];

            let mut stamped_room = false;
            for side in &sides {
                let (lens, curb_len) = arc_lengths(&side.kerb);
                if curb_len <= f32::EPSILON {
                    continue;
                }
                let Some(row) = rule_of(rule, side.which) else {
                    continue;
                };
                if !side.authored_kerb {
                    walk.stats.sides_no_kerb += 1;
                    issue(
                        &mut walk.stats,
                        format!(
                            "path {pi} room {rid}: no authored kerb for {:?}; stamping the building line",
                            side.which
                        ),
                    );
                }
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
                        let (curb, outer) = strip_at(&side.kerb, &side.outer, &lens, s);
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
        psdl::{PerimeterPoint, PsdlRoom, RoomAttribute, RoomPath},
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

    fn room(perim: &[(u16, u16)], attributes: Vec<RoomAttribute>) -> PsdlRoom {
        PsdlRoom {
            perimeter: perim
                .iter()
                .map(|&(vertex, room)| PerimeterPoint { vertex, room })
                .collect(),
            attributes,
            unparsed_attributes: Vec::new(),
        }
    }

    /// An inline `RoadWithSidewalks` attribute: `sections` are
    /// `[sw_l, road_l, road_r, sw_r]` vertex ids per cross-section.
    fn road_attr(sections: &[[u16; 4]]) -> RoomAttribute {
        RoomAttribute {
            last: false,
            kind: AttributeType::RoadWithSidewalks,
            subtype: sections.len() as u8,
            data: sections.iter().flatten().copied().collect(),
        }
    }

    /// The quad room's straight road strip: kerbs on the x = 2 and
    /// x = 28 lines, building lines on x = 0 and x = 30.
    fn quad_road(room2: bool) -> RoomAttribute {
        if room2 {
            road_attr(&[[7, 6, 5, 4], [11, 10, 9, 8]])
        } else {
            road_attr(&[[0, 1, 2, 3], [7, 6, 5, 4]])
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

    fn psdl(
        vertices: Vec<[f32; 3]>,
        rooms: Vec<PsdlRoom>,
        rules: &[u8],
        paths: Vec<RoomPath>,
    ) -> Psdl {
        Psdl {
            target_size: 2,
            vertices,
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
    /// ends — the single-room-path case), with its straight road strip.
    fn room1_solo() -> PsdlRoom {
        room(
            &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 0),
                (7, 0),
            ],
            vec![quad_road(false)],
        )
    }

    #[test]
    fn a_single_room_path_stamps_both_sides() {
        // n01right = lamps on the x≈30 side (right of +z travel),
        // n01left = meters on the x≈0 side walked exit→entry.
        let city = psdl(
            quad_verts(),
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
            quad_verts(),
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
        let r1 = room(
            &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 2),
                (5, 2),
                (6, 2),
                (7, 0),
            ],
            vec![quad_road(false)],
        );
        let r2 = room(
            &[
                (4, 1),
                (5, 1),
                (6, 1),
                (7, 0),
                (11, 0),
                (10, 0),
                (9, 0),
                (8, 0),
            ],
            vec![quad_road(true)],
        );
        let city = psdl(
            quad_verts(),
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
            quad_verts(),
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

    /// A curved room: the right kerb bulges to x = 14 at z = 20 while
    /// the building line stays straight — the layout that put retail
    /// stamps inside the carriageway when the kerb was a chord.
    fn curved_verts() -> Vec<[f32; 3]> {
        vec![
            [0., 0., 0.],
            [2., 0., 0.],
            [28., 0., 0.],
            [30., 0., 0.], // entry (z = 0)
            [30., 0., 40.],
            [28., 0., 40.],
            [2., 0., 40.],
            [0., 0., 40.], // exit (z = 40)
            [0., 0., 20.],
            [2., 0., 20.],
            [14., 0., 20.],
            [30., 0., 20.], // strip mid-section (z = 20)
        ]
    }

    #[test]
    fn a_curved_kerb_follows_the_authored_strip() {
        // The right side's kerb chain (28,0)→(14,20)→(28,40) makes a
        // chord corner-to-corner run down x = 28 — 14 m inside the
        // road at mid-block. The stamp must sit on the chain instead.
        let room = room(
            &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 0),
                (7, 0),
            ],
            vec![road_attr(&[[0, 1, 2, 3], [8, 9, 10, 11], [7, 6, 5, 4]])],
        );
        let city = psdl(
            curved_verts(),
            vec![room],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        // Kerb segments: (28,0)→(14,20) and (14,20)→(28,40), each
        // √(14²+20²) ≈ 24.413 m. s = seg lands on the mid vertex, s =
        // 1.5·seg halfway down the second segment.
        let seg = (14f32 * 14. + 20. * 20.).sqrt();
        let (defs, rules) = tables(
            vec![def("lamp", seg, seg * 0.5, 2, &["pb"])],
            vec![rule("n01right", &["lamp"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stats.sides_no_kerb, 0);
        assert_eq!(walk.stamps.len(), 2);
        // s = seg → strip section 1: kerb (14,20) → outer (30,20),
        // lerp 0.5 → (22,20). A chord kerb would have put it at
        // (29,20), 7 m inside the carriageway.
        assert!(near(walk.stamps[0].position, [22., 0., 20.]));
        // s = 1.5·seg → kerb (21,30) → outer (30,30) → (25.5,30).
        assert!(near(walk.stamps[1].position, [25.5, 0., 30.]));
    }

    #[test]
    fn a_room_without_a_kerb_strip_stamps_the_building_line() {
        // Rule-bearing room with no road attribute: the side has no
        // authored kerb, so the walk falls back to the perimeter arc —
        // on the building line, never inside the road — and counts it.
        let room = room(
            &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 0),
                (7, 0),
            ],
            Vec::new(),
        );
        let city = psdl(
            quad_verts(),
            vec![room],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![def("lamp", 2., 6., 2, &["pb"])],
            vec![rule("n01right", &["lamp"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stats.sides_no_kerb, 1);
        assert_eq!(walk.stamps.len(), 2);
        assert!(near(walk.stamps[0].position, [30., 0., 2.]));
        assert!(near(walk.stamps[1].position, [30., 0., 8.]));
    }
}
