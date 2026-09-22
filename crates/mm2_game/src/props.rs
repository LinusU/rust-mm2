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
//!   0.1 ≈ curb-hugging, 0.5 ≈ mid-sidewalk). The kerb chain runs at
//!   the *foot* of the kerb face (road level), so its height is first
//!   lifted by [`SIDEWALK_KERB_LIFT`] — the walkable surface the
//!   renderer emits — before lerping. Stamping the raw lerp buried
//!   every kerbside prop ≈ kerb-height × (1 − lerp) into the
//!   pavement (operator play-test report 4).
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
//! - A stamp's `forward` is the direction the prop's +X axis is yawed
//!   to — the kerb→building-line direction measured from the strip
//!   cross-section at the stamp (inferred; `docs/research/
//!   proprules.md`). Retail kerb props are authored front/arm-first
//!   along local −X — lamp and traffic-light mast arms reach −X
//!   metres off the pole while the `dgBangerData` bound wraps the
//!   pole alone — so +X building-ward puts the prop's face on the
//!   carriageway and, on a curved kerb, rotates each stamp with the
//!   road edge.
//!
//! The module also hosts the two shared placement helpers the audit
//! tooling reuses: [`path_stamp_sites`] expands a `PTH1` path into the
//! stamp positions `mm2_app` then spawns (one source of truth for the
//! expansion policy), and [`carriageways`] extracts each room's
//! authored drivable surfaces — the reference a stamped position is
//! checked against when auditing "prop in the road" reports.

use std::collections::HashMap;

use mm2_formats::{
    pathset::{Path, PathKind},
    pkg::{Pkg, lod_split},
    proprules::{PropDefs, PropRuleSide, PropRules},
    psdl::{AttributeType, Psdl, PsdlRoom, RoomAttribute},
};

/// The height a sidewalk top sits above the kerb *foot* (road-edge)
/// vertex — measured on retail `city/{london,sf}.psdl`:
/// `sw.y − road.y == 0.15` on the overwhelming majority of
/// `RoadWithSidewalks`/`DividedRoad` sections (the few at 0 are
/// authored flush ramps/driveways) and `top.y − ground.y == 0.15` on
/// every `SidewalkStrip` pair (~11.5k, both cities). `mm2_app::city`
/// emits the sidewalk top and its collider as the road-edge chain
/// lifted by this constant, so the walkable surface a prop stands on
/// is `lerp(kerb + SIDEWALK_KERB_LIFT, outer, t)` — see
/// [`walk_prop_rules`] and [`walkable_surfaces`].
pub const SIDEWALK_KERB_LIFT: f32 = 0.15;

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
    /// Direction the prop's local +X axis is yawed to (XZ-normalized):
    /// the kerb→building-line direction at the stamp, so directional
    /// props — authored front/arm-first along local −X on every
    /// measured retail kerb prop (lamps, traffic-light masts, benches,
    /// sign plates) — face the carriageway.
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

/// `v` normalized on the XZ plane, or `None` when it has no
/// horizontal component to normalize.
fn norm_xz(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[2] * v[2]).sqrt();
    (l > 1e-6).then(|| [v[0] / l, 0.0, v[2] / l])
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
    /// Height the kerb chain's vertices sit *below* the walkable
    /// surface they bound: [`SIDEWALK_KERB_LIFT`] when the chain is
    /// the foot of a raised kerb face (road-edge and sidewalk-strip
    /// ground chains), `0` for walkways whose authored edge is already
    /// the surface.
    lift: f32,
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
                for s in refs.as_chunks::<4>().0 {
                    ol.push(s[0]);
                    kl.push(s[1]);
                    kr.push(s[2]);
                    or_.push(s[3]);
                }
                out.push(KerbStrip {
                    kerb: kl,
                    outer: ol,
                    lift: SIDEWALK_KERB_LIFT,
                });
                out.push(KerbStrip {
                    kerb: kr,
                    outer: or_,
                    lift: SIDEWALK_KERB_LIFT,
                });
            }
            AttributeType::DividedRoad => {
                // Six refs per section — layout in `divided_refs`.
                let Some(refs) = divided_refs(attr) else {
                    continue;
                };
                let (mut kl, mut ol, mut kr, mut or_) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for s in refs.as_chunks::<6>().0 {
                    ol.push(s[0]);
                    kl.push(s[1]);
                    kr.push(s[4]);
                    or_.push(s[5]);
                }
                out.push(KerbStrip {
                    kerb: kl,
                    outer: ol,
                    lift: SIDEWALK_KERB_LIFT,
                });
                out.push(KerbStrip {
                    kerb: kr,
                    outer: or_,
                    lift: SIDEWALK_KERB_LIFT,
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
                for s in refs.as_chunks::<2>().0 {
                    kerb.push(s[0]);
                    outer.push(s[1]);
                }
                out.push(KerbStrip {
                    kerb,
                    outer,
                    lift: SIDEWALK_KERB_LIFT,
                });
            }
            AttributeType::RoadNoSidewalks => {
                let Some(refs) = counted_refs(attr, 2) else {
                    continue;
                };
                let (mut l, mut r) = (Vec::new(), Vec::new());
                for s in refs.as_chunks::<2>().0 {
                    l.push(s[0]);
                    r.push(s[1]);
                }
                out.push(KerbStrip {
                    kerb: l.clone(),
                    outer: l,
                    lift: 0.0,
                });
                out.push(KerbStrip {
                    kerb: r.clone(),
                    outer: r,
                    lift: 0.0,
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
    // The kerb chain runs at the *foot* of the kerb face; the walkable
    // surface `mm2_app` renders and collides starts one kerb height
    // above it. Stamps rest on that surface, so evaluate the chain at
    // its lifted height — XZ (and therefore every arc length, cross-
    // section lerp and facing) is untouched. Walkway and fallback
    // chains carry `lift = 0`.
    for p in &mut kerb {
        p[1] += st.lift;
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
                        // `forward` is the direction the prop's +X axis
                        // is yawed to. Every directional kerb prop
                        // measured on retail (lamp/mast arms, bench and
                        // sign faces) is authored facing local −X, so
                        // +X must run kerb→building-line for the face
                        // to end up on the carriageway — walking the
                        // side's travel direction instead leaves every
                        // prop turned a quarter turn (operator
                        // play-test report 3). The strip cross-section
                        // gives that direction per stamp, so stamps
                        // follow a curved kerb. On the building-line
                        // fallback (kerb == outer) the stamp aims away
                        // from the room centre; a degenerate room keeps
                        // the walk's own right (road stays on its left).
                        let forward = norm_xz(sub(outer, curb))
                            .or_else(|| norm_xz(sub(position, centre)))
                            .unwrap_or([side.forward[2], 0.0, -side.forward[0]]);
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
                            forward,
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

// ---------------------------------------------------------------------------
// Pathset stamp expansion (shared with the app) and the drivable-surface
// reference the placement audit classifies stamps against.
// ---------------------------------------------------------------------------

/// Hard bound on the prop instances one `props.pathset` file may stamp.
/// The parser bounds path/point *counts* but not coordinates: a line
/// strip expands to `segment_length / spacing` stamps, so authored
/// positions make the expansion unbounded without this cap — a corrupt
/// or hostile file (a VFS mod can legitimately override
/// `props.pathset`) could stall `t += spacing` below the f32 ulp or
/// push millions of instances and hang/OOM the city load. Retail's
/// densest expansion is London's `props.pathset` at 1188 stamps / 87
/// paths (sf: 925 / 113 + 31 decal-only paths; densest single path
/// 162) — the cap sits ~7x above it. Suppressed stamps are counted in
/// [`PathStampSites::capped`], never dropped silently.
pub const MAX_PATHSET_STAMPS: usize = 8192;

/// One stamp the pathset expansion produces: the authored-space
/// position plus the facing the prop's +X axis is yawed to (`None` =
/// unrotated). The direction is the raw authored intent — consumers
/// normalize on XZ.
#[derive(Debug, Clone)]
pub struct PathStampSite {
    /// Placement position, authored coordinates.
    pub position: [f32; 3],
    /// Direction the stamp faces, or `None` for an unrotated stamp.
    pub forward: Option<[f32; 3]>,
}

/// What [`path_stamp_sites`] produced and suppressed — expansion
/// over the `budget` is counted, never dropped silently.
#[derive(Debug, Default)]
pub struct PathStampSites {
    /// Stamped sites in expansion order.
    pub sites: Vec<PathStampSite>,
    /// Stamps suppressed because `budget` ran out.
    pub capped: usize,
}

/// Expand one pathset path into stamped sites, emitting at most
/// `budget` — the sole implementation of the stamping policy both the
/// city loader and the placement audit consume, so an audit measures
/// exactly what the game stamps. Kind rules per R3 —
/// `docs/research/pathset.md`:
///
/// - `Points`: one unrotated stamp per vertex.
/// - `Directed`: one stamp per point pair at the first point, facing
///   toward the second. A lone trailing point on an odd-count path
///   stamps nothing (`Pathset::validate` reports the anomaly; the
///   expansion does not guess its mate).
/// - `LineStrip`: each segment is filled with stamps at `spacing`
///   intervals measured from the segment's start (t = 0, s, 2s, …
///   strictly below the segment length, so the shared vertex is
///   stamped once by the following segment), and the path's final
///   vertex caps the row. Stamps face along their segment. The
///   per-segment restart is the literal R3 rule — whether the original
///   resets spacing at vertices is unverified (UNK-20).
///
/// A zero `spacing` on a strip stamps one unrotated prop per vertex
/// (designed fallback — spacing 0 means "densest possible" and no
/// documented rule subdivides further). Undocumented kinds stamp
/// nothing; `validate()` names them.
///
/// Non-finite point coordinates stamp nothing either — the parser
/// bounds counts, not magnitudes; callers run [`Pathset::validate`] so
/// corrupt points are reported as issues.
///
/// [`Pathset::validate`]: mm2_formats::pathset::Pathset::validate
pub fn path_stamp_sites(path: &Path, budget: usize) -> PathStampSites {
    match path.kind() {
        Some(PathKind::Points) => {
            let finite: Vec<[f32; 3]> = path
                .points
                .iter()
                .map(|p| p.position)
                .filter(|p| p.iter().all(|c| c.is_finite()))
                .collect();
            let take = finite.len().min(budget);
            PathStampSites {
                sites: finite[..take]
                    .iter()
                    .map(|&position| PathStampSite {
                        position,
                        forward: None,
                    })
                    .collect(),
                capped: finite.len() - take,
            }
        }
        Some(PathKind::Directed) => {
            let pairs: Vec<([f32; 3], [f32; 3])> = path
                .points
                .as_chunks::<2>()
                .0
                .iter()
                .filter(|pair| {
                    pair.iter()
                        .all(|p| p.position.iter().all(|c| c.is_finite()))
                })
                .map(|pair| (pair[0].position, pair[1].position))
                .collect();
            let take = pairs.len().min(budget);
            PathStampSites {
                sites: pairs[..take]
                    .iter()
                    .map(|&(a, b)| PathStampSite {
                        position: a,
                        forward: Some(sub(b, a)),
                    })
                    .collect(),
                capped: pairs.len() - take,
            }
        }
        Some(PathKind::LineStrip) => line_strip_sites(path, budget),
        None => PathStampSites::default(),
    }
}

/// `LineStrip` expansion — see [`path_stamp_sites`] for the rule.
fn line_strip_sites(path: &Path, budget: usize) -> PathStampSites {
    let spacing = path.spacing_metres();
    let pts: Vec<[f32; 3]> = path.points.iter().map(|p| p.position).collect();
    if spacing <= f32::EPSILON {
        let finite: Vec<[f32; 3]> = pts
            .iter()
            .copied()
            .filter(|p| p.iter().all(|c| c.is_finite()))
            .collect();
        let take = finite.len().min(budget);
        return PathStampSites {
            sites: finite[..take]
                .iter()
                .map(|&position| PathStampSite {
                    position,
                    forward: None,
                })
                .collect(),
            capped: finite.len() - take,
        };
    }
    let mut out = Vec::new();
    let mut capped = 0usize;
    let mut left = budget;
    let mut last_dir = [1.0, 0.0, 0.0];
    for w in pts.windows(2) {
        let seg = sub(w[1], w[0]);
        let len = (seg[0] * seg[0] + seg[1] * seg[1] + seg[2] * seg[2]).sqrt();
        // Zero-length segments stamp nothing. Non-finite lengths come
        // from corrupt coordinates (`Pathset::validate` reports them);
        // skipping them also keeps the expansion arithmetic finite.
        if !len.is_finite() || len <= f32::EPSILON {
            continue;
        }
        let dir = [seg[0] / len, seg[1] / len, seg[2] / len];
        last_dir = dir;
        // Stamps sit at t = 0, s, 2s, … strictly below len: ceil(len/s)
        // of them, counted arithmetically so a huge or hostile segment
        // is measured against the budget instead of walked — `t += s`
        // stalls below the f32 ulp long before a multi-thousand-km
        // segment ends. The float→int cast saturates, which `min`
        // turns into the full remaining budget.
        let want = (f64::from(len) / f64::from(spacing)).ceil() as usize;
        let take = want.min(left);
        for i in 0..take {
            let t = i as f32 * spacing;
            out.push(PathStampSite {
                position: [
                    w[0][0] + dir[0] * t,
                    w[0][1] + dir[1] * t,
                    w[0][2] + dir[2] * t,
                ],
                forward: Some(dir),
            });
        }
        left -= take;
        capped = capped.saturating_add(want - take);
    }
    // The walk stops short of the final vertex by construction;
    // the authored row is capped at its end point. A lone vertex
    // (no segments) still stamps once, unrotated. A non-finite final
    // vertex stamps nothing.
    if let Some(&last) = pts.last().filter(|p| p.iter().all(|c| c.is_finite())) {
        if left == 0 {
            capped = capped.saturating_add(1);
        } else {
            out.push(PathStampSite {
                position: last,
                forward: (pts.len() > 1).then_some(last_dir),
            });
        }
    }
    PathStampSites { sites: out, capped }
}

/// The authored-space basis a stamp facing `dir` gets: the prop's
/// local +X axis yawed about Y onto `dir`'s XZ projection — the INST
/// simple-placement convention (`docs/research/inst.md`, verified on
/// `wl_buckpalace_l`'s fence; confirmed for directed pathset stamps by
/// the retail `sp_lightstreet_rt_f` kerb rows, whose +Z lamp arms land
/// over the carriageway only under this reading). +Z lands on
/// `left(dir)`; a degenerate XZ direction leaves the prop unrotated.
/// `mm2_app` wraps the returned (x, y, z) axis images into its
/// placement transform; the placement audit sweeps prop footprints
/// through the same basis.
pub fn yawed_basis(dir: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let x = norm_xz(dir).unwrap_or([1.0, 0.0, 0.0]);
    (x, [0.0, 1.0, 0.0], [-x[2], 0.0, x[0]])
}

/// The content offset a stamped prop carries onto its authored point
/// — measured on retail `dgBangerData` records
/// (`docs/research/banger.md` § "The `Size`/`CG` bound convention"): a
/// bound prop's mesh is authored centred at the bound's `CG`, so
/// content lands `+CG` above the stamp point; an unbound prop is
/// ground-lifted by `−min_y` so its lowest authored vertex rests on
/// the point.
pub fn stamp_content_offset(pkg: &Pkg, bound_cg: Option<[f32; 3]>) -> [f32; 3] {
    if let Some(cg) = bound_cg {
        return cg;
    }
    let min_y = pkg
        .geometries()
        .flat_map(|(_, g)| g.sections.iter())
        .flat_map(|s| s.strips.iter())
        .flat_map(|s| s.vertices.iter())
        .map(|v| v.position[1])
        .fold(f32::MAX, f32::min);
    [0.0, (-min_y).max(0.0), 0.0]
}

/// The prop's rendered-geometry vertices in stamp space — best LOD per
/// stem with `shadow`/`dmg` stand-ins excluded, every vertex offset by
/// [`stamp_content_offset`]. This is the selection `mm2_app` renders
/// and collides; the placement audit sweeps it through each stamp's
/// basis as the prop's world-space footprint.
pub fn stamp_space_verts(pkg: &Pkg, bound_cg: Option<[f32; 3]>) -> Vec<[f32; 3]> {
    let offset = stamp_content_offset(pkg, bound_cg);
    let mut best: HashMap<String, (u8, &str)> = HashMap::new();
    for (name, _geo) in pkg.geometries() {
        let (stem, rank) = lod_split(name);
        let entry = best.entry(stem).or_insert((rank, name));
        if rank > entry.0 {
            *entry = (rank, name);
        }
    }
    let mut out = Vec::new();
    for (name, geo) in pkg.geometries() {
        let (stem, _) = lod_split(name);
        // Shadow/damage stand-ins are not rendered prop surface.
        if stem.contains("shadow") || stem.contains("dmg") {
            continue;
        }
        if best.get(&stem).map(|(_, n)| *n) != Some(name) {
            continue; // a better-LOD chunk owns this stem
        }
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    out.push([
                        v.position[0] + offset[0],
                        v.position[1] + offset[1],
                        v.position[2] + offset[2],
                    ]);
                }
            }
        }
    }
    out
}

/// One drivable region extracted from a room's road attributes: the
/// carriageway surface the renderer and colliders both emit, expressed
/// as a boundary ring plus its triangulation. This is the reference a
/// stamped prop position is checked against when auditing lateral
/// placement — a stamp inside a ring at surface height sits on the
/// road.
///
/// Drivable surfaces are `RoadWithSidewalks` (`road_l`↔`road_r`),
/// `DividedRoad` (`rl_out`↔`rl_in` and `rr_in`↔`rr_out` — the divider
/// strip is not drivable), `RoadNoSidewalks` (the whole walkway strip),
/// `Crosswalk` rectangles and `RoadFan` rings. `SidewalkStrip` and
/// generic `Fan` surfaces are not carriageway. Junction rooms carry no
/// road attributes, so a stamp inside a junction box is not detected —
/// the audit is conservative, it can miss in-road stamps but not
/// invent them.
#[derive(Debug)]
pub struct Carriageway {
    /// 1-based room id.
    pub room: u16,
    /// Which attribute authored the region.
    pub kind: AttributeType,
    /// Which authored surface of the attribute the region covers —
    /// [`SurfaceBand::Driving`] for every region [`carriageways`]
    /// returns; [`walkable_surfaces`] adds the others.
    pub band: SurfaceBand,
    /// Boundary ring in authored coordinates (closed implicitly).
    pub ring: Vec<[f32; 3]>,
    /// Triangles covering the region — the XZ point-in-triangle test
    /// plus a barycentric surface height for the point.
    pub tris: Vec<[[f32; 3]; 3]>,
}

/// Which authored surface of a room attribute a [`Carriageway`]
/// region covers — one attribute authors several (a
/// `RoadWithSidewalks` is a drivable band *and* two raised sidewalk
/// tops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBand {
    /// The drivable band [`carriageways`] extracts — road surface,
    /// walkway strip, crosswalk or road fan.
    Driving,
    /// The raised sidewalk top: the kerb-foot chain lifted by
    /// [`SIDEWALK_KERB_LIFT`] on the inside edge, the authored
    /// outer/building-line chain outside — the surface
    /// `mm2_app::city` renders and collides, and the one prop-rule
    /// stamps rest on.
    SidewalkTop,
    /// A generic floor `Fan` — junction floors and plaza aprons the
    /// drivable audit does not classify. Only fans whose surface
    /// normal is mostly vertical (the renderer's own floor-vs-wall
    /// test) are included.
    FloorFan,
}

/// The `DividedRoad` payload's six-refs-per-section slice, shared by
/// the kerb-strip walk and the carriageway extraction. Inline form is
/// `[packed, value, subtype×6]`; counted form `[count, packed, value,
/// count×6]`. `None` on a malformed record.
fn divided_refs(attr: &RoomAttribute) -> Option<&[u16]> {
    if attr.subtype == 0 {
        if attr.data.len() < 3 || attr.data[3..].len() != attr.data[0] as usize * 6 {
            return None;
        }
        Some(&attr.data[3..])
    } else {
        if attr.data.len() < 2 || attr.data[2..].len() != attr.subtype as usize * 6 {
            return None;
        }
        Some(&attr.data[2..])
    }
}

/// A fan attribute's vertex refs — `[pivot, ring…]` — or `None` on a
/// malformed record. `subtype` is the triangle count; subtype 0 takes
/// a leading count word instead.
fn fan_refs(attr: &RoomAttribute) -> Option<&[u16]> {
    if attr.subtype == 0 {
        let (&n, rest) = attr.data.split_first()?;
        (rest.len() == n as usize + 2).then_some(rest)
    } else {
        (attr.data.len() == attr.subtype as usize + 2).then_some(&attr.data)
    }
}

/// Resolve a chain of vertex ids against the PSDL vertex table.
fn resolve_ids(psdl: &Psdl, ids: &[u16]) -> Option<Vec<[f32; 3]>> {
    ids.iter()
        .map(|&v| psdl.vertices.get(v as usize).copied())
        .collect()
}

/// A resolved strip's two index-paired edge chains → boundary ring +
/// quads split into triangles.
fn push_strip(
    room: u16,
    kind: AttributeType,
    band: SurfaceBand,
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    out: &mut Vec<Carriageway>,
) {
    if a.len() != b.len() || a.len() < 2 {
        return;
    }
    let mut ring = a.to_vec();
    ring.extend(b.iter().rev());
    let mut tris = Vec::with_capacity((a.len() - 1) * 2);
    for i in 0..a.len() - 1 {
        tris.push([a[i], a[i + 1], b[i + 1]]);
        tris.push([a[i], b[i + 1], b[i]]);
    }
    out.push(Carriageway {
        room,
        kind,
        band,
        ring,
        tris,
    });
}

/// A fan record's resolved points → boundary ring + pivot triangles.
fn push_fan(
    room: u16,
    kind: AttributeType,
    band: SurfaceBand,
    pts: &[[f32; 3]],
    out: &mut Vec<Carriageway>,
) {
    if pts.len() < 3 {
        return;
    }
    let ring: Vec<[f32; 3]> = pts[1..].to_vec();
    let tris: Vec<[[f32; 3]; 3]> = ring.windows(2).map(|w| [pts[0], w[0], w[1]]).collect();
    out.push(Carriageway {
        room,
        kind,
        band,
        ring,
        tris,
    });
}

/// Extract every drivable surface region in the city — see
/// [`Carriageway`] for what counts. Malformed attribute data is
/// skipped whole (the city importer's own policy), never guessed.
pub fn carriageways(psdl: &Psdl) -> Vec<Carriageway> {
    let mut out = Vec::new();
    // A strip's two index-paired edge id-chains → resolved chains.
    let strip =
        |room: u16, kind: AttributeType, a: &[u16], b: &[u16], out: &mut Vec<Carriageway>| {
            let (Some(a), Some(b)) = (resolve_ids(psdl, a), resolve_ids(psdl, b)) else {
                return;
            };
            push_strip(room, kind, SurfaceBand::Driving, &a, &b, out);
        };
    for (ri, room) in psdl.rooms.iter().enumerate() {
        let rid = (ri + 1) as u16;
        for attr in &room.attributes {
            match attr.kind {
                AttributeType::RoadWithSidewalks => {
                    // Sections [sw_l, road_l, road_r, sw_r]: the
                    // carriageway is the road_l↔road_r band.
                    let Some(refs) = counted_refs(attr, 4) else {
                        continue;
                    };
                    let a: Vec<u16> = refs.as_chunks::<4>().0.iter().map(|s| s[1]).collect();
                    let b: Vec<u16> = refs.as_chunks::<4>().0.iter().map(|s| s[2]).collect();
                    strip(rid, attr.kind, &a, &b, &mut out);
                }
                AttributeType::DividedRoad => {
                    // [sw_l, rl_out, rl_in, rr_in, rr_out, sw_r]: two
                    // carriageways; the rl_in↔rr_in divider is not
                    // drivable surface.
                    let Some(refs) = divided_refs(attr) else {
                        continue;
                    };
                    let rl_out: Vec<u16> = refs.as_chunks::<6>().0.iter().map(|s| s[1]).collect();
                    let rl_in: Vec<u16> = refs.as_chunks::<6>().0.iter().map(|s| s[2]).collect();
                    let rr_in: Vec<u16> = refs.as_chunks::<6>().0.iter().map(|s| s[3]).collect();
                    let rr_out: Vec<u16> = refs.as_chunks::<6>().0.iter().map(|s| s[4]).collect();
                    strip(rid, attr.kind, &rl_out, &rl_in, &mut out);
                    strip(rid, attr.kind, &rr_in, &rr_out, &mut out);
                }
                AttributeType::RoadNoSidewalks => {
                    let Some(refs) = counted_refs(attr, 2) else {
                        continue;
                    };
                    let a: Vec<u16> = refs.as_chunks::<2>().0.iter().map(|s| s[0]).collect();
                    let b: Vec<u16> = refs.as_chunks::<2>().0.iter().map(|s| s[1]).collect();
                    strip(rid, attr.kind, &a, &b, &mut out);
                }
                AttributeType::Crosswalk => {
                    // Four corner refs in strip order — (0, 1) one
                    // short end, (2, 3) the other.
                    let Some(p) = (attr.data.len() == 4)
                        .then(|| resolve_ids(psdl, &attr.data))
                        .flatten()
                    else {
                        continue;
                    };
                    out.push(Carriageway {
                        room: rid,
                        kind: attr.kind,
                        band: SurfaceBand::Driving,
                        ring: vec![p[0], p[1], p[3], p[2]],
                        tris: vec![[p[0], p[1], p[3]], [p[0], p[3], p[2]]],
                    });
                }
                AttributeType::RoadFan => {
                    // [pivot, ring…] — the fan's ring is the boundary.
                    let Some(pts) = fan_refs(attr).and_then(|ids| resolve_ids(psdl, ids)) else {
                        continue;
                    };
                    push_fan(rid, attr.kind, SurfaceBand::Driving, &pts, &mut out);
                }
                _ => {}
            }
        }
    }
    out
}

/// Extract every surface a stamped prop can legitimately stand on —
/// the drivable regions of [`carriageways`] plus the raised sidewalk
/// tops and the generic floor `Fan`s. This is the reference the
/// placement audit measures *vertical penetration* against: a prop
/// base sitting below the surface covering its XZ is sunk into the
/// geometry, the defect operator play-test report 4 item 4 names.
///
/// The extra bands are built exactly as `mm2_app::city` emits them:
/// a sidewalk top spans the kerb-foot (road-edge) chain lifted by
/// [`SIDEWALK_KERB_LIFT`] to the authored outer chain — on retail
/// data the outer edge sits ≈ the same height, so the top is flat —
/// and a `SidewalkStrip` top spans lifted-ground → authored top.
/// Generic `Fan`s join only when their surface normal is mostly
/// vertical (`|n.y| ≥ 0.3`, the renderer's own floor-vs-wall test in
/// `vertical_facing`), so wall-like fans never produce phantom
/// floors. Divider, wall, curb-face and roof surfaces are not
/// standable and are excluded — the reference can miss a surface,
/// never invent one. Junction rooms still carry no road attributes,
/// so a stamp under a junction floor's `Fan` *is* seen here.
pub fn walkable_surfaces(psdl: &Psdl) -> Vec<Carriageway> {
    let mut out = carriageways(psdl);
    // Resolve a chain of ids, then lift it to the walkable height.
    let lifted = |ids: &[u16]| -> Option<Vec<[f32; 3]>> {
        resolve_ids(psdl, ids).map(|c| {
            c.iter()
                .map(|p| [p[0], p[1] + SIDEWALK_KERB_LIFT, p[2]])
                .collect()
        })
    };
    for (ri, room) in psdl.rooms.iter().enumerate() {
        let rid = (ri + 1) as u16;
        for attr in &room.attributes {
            match attr.kind {
                AttributeType::RoadWithSidewalks => {
                    // [sw_l, road_l, road_r, sw_r]: each sidewalk top
                    // spans lifted-road-edge → authored outer edge.
                    let Some(refs) = counted_refs(attr, 4) else {
                        continue;
                    };
                    let secs = refs.as_chunks::<4>().0;
                    let (sw_l, rl, rr, sw_r) = (
                        secs.iter().map(|s| s[0]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[1]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[2]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[3]).collect::<Vec<_>>(),
                    );
                    let (Some(il), Some(ol), Some(ir), Some(or)) = (
                        lifted(&rl),
                        resolve_ids(psdl, &sw_l),
                        lifted(&rr),
                        resolve_ids(psdl, &sw_r),
                    ) else {
                        continue;
                    };
                    push_strip(rid, attr.kind, SurfaceBand::SidewalkTop, &il, &ol, &mut out);
                    push_strip(rid, attr.kind, SurfaceBand::SidewalkTop, &ir, &or, &mut out);
                }
                AttributeType::DividedRoad => {
                    let Some(refs) = divided_refs(attr) else {
                        continue;
                    };
                    let secs = refs.as_chunks::<6>().0;
                    let (sw_l, rl, rr, sw_r) = (
                        secs.iter().map(|s| s[0]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[1]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[4]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[5]).collect::<Vec<_>>(),
                    );
                    let (Some(il), Some(ol), Some(ir), Some(or)) = (
                        lifted(&rl),
                        resolve_ids(psdl, &sw_l),
                        lifted(&rr),
                        resolve_ids(psdl, &sw_r),
                    ) else {
                        continue;
                    };
                    push_strip(rid, attr.kind, SurfaceBand::SidewalkTop, &il, &ol, &mut out);
                    push_strip(rid, attr.kind, SurfaceBand::SidewalkTop, &ir, &or, &mut out);
                }
                AttributeType::SidewalkStrip => {
                    let Some(refs) = counted_refs(attr, 2) else {
                        continue;
                    };
                    // Same end-cap exclusion `kerb_strips` applies.
                    if refs.len() >= 4 && refs[0] == refs[1] && refs[0] <= 1 {
                        continue;
                    }
                    let secs = refs.as_chunks::<2>().0;
                    let (ground, top) = (
                        secs.iter().map(|s| s[0]).collect::<Vec<_>>(),
                        secs.iter().map(|s| s[1]).collect::<Vec<_>>(),
                    );
                    let (Some(i), Some(o)) = (lifted(&ground), resolve_ids(psdl, &top)) else {
                        continue;
                    };
                    push_strip(rid, attr.kind, SurfaceBand::SidewalkTop, &i, &o, &mut out);
                }
                AttributeType::Fan => {
                    // Generic fan — the junction-floor/plaza surface.
                    // Wall-like fans (mostly-horizontal normal, the
                    // renderer's `vertical_facing` test) are excluded.
                    let Some(pts) = fan_refs(attr).and_then(|ids| resolve_ids(psdl, ids)) else {
                        continue;
                    };
                    let mut n = [0.0f32; 3];
                    for i in 1..pts.len().saturating_sub(1) {
                        let (a, b) = (sub(pts[i], pts[0]), sub(pts[i + 1], pts[0]));
                        let c = [
                            a[1] * b[2] - a[2] * b[1],
                            a[2] * b[0] - a[0] * b[2],
                            a[0] * b[1] - a[1] * b[0],
                        ];
                        if c[0] * c[0] + c[1] * c[1] + c[2] * c[2] > 1e-6 {
                            n = c;
                            break;
                        }
                    }
                    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    if l <= f32::EPSILON || n[1].abs() / l < 0.3 {
                        continue;
                    }
                    push_fan(rid, attr.kind, SurfaceBand::FloorFan, &pts, &mut out);
                }
                _ => {}
            }
        }
    }
    out
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
    /// (x, z) plane like the retail rooms. Heights follow the authored
    /// convention: the building-line chains sit at kerb-top height,
    /// the road-edge (kerb-foot) chains at road level.
    fn quad_verts() -> Vec<[f32; 3]> {
        let sw = SIDEWALK_KERB_LIFT;
        vec![
            [0., sw, 0.],
            [2., 0., 0.],
            [28., 0., 0.],
            [30., sw, 0.], // room 1 entry (z = 0)
            [30., sw, 20.],
            [28., 0., 20.],
            [2., 0., 20.],
            [0., sw, 20.], // shared boundary (z = 20)
            [30., sw, 40.],
            [28., 0., 40.],
            [2., 0., 40.],
            [0., sw, 40.], // room 2 exit (z = 40)
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
        // Stamps stand on the raised sidewalk top (kerb height above
        // the road-edge chain), not on the kerb foot — the burial
        // defect of operator report 4.
        assert!(near(walk.stamps[0].position, [29., SIDEWALK_KERB_LIFT, 2.]));
        assert!(near(walk.stamps[1].position, [29., SIDEWALK_KERB_LIFT, 8.]));
        assert!(walk.stamps[0..2].iter().all(lamp));
        // Stamps face kerb→building-line (+X maps there; the prop's
        // authored −X front then lies on the carriageway): +x on the
        // right-hand x≈30 building line.
        assert!(near(walk.stamps[0].forward, [1., 0., 0.]));
        assert_eq!(walk.stamps[0].pkg, "pb");

        // The left side walks backward from the exit crossing: start=5
        // measures 5 m back along the curb from z = 20.
        assert_eq!(walk.stamps[2].side, PropRuleSide::Left);
        assert!(near(walk.stamps[2].position, [1., SIDEWALK_KERB_LIFT, 15.]));
        assert!(near(walk.stamps[3].position, [1., SIDEWALK_KERB_LIFT, 5.]));
        assert!(near(walk.stamps[2].forward, [-1., 0., 0.]));
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
        assert!(near(r2_right[0].position, [29., SIDEWALK_KERB_LIFT, 22.]));
        assert!(near(r2_right[1].position, [29., SIDEWALK_KERB_LIFT, 28.]));
        assert!(near(r2_right[0].forward, [1., 0., 0.]));
        let r2_left: Vec<&PropStamp> = walk
            .stamps
            .iter()
            .filter(|s| s.room == 2 && s.side == PropRuleSide::Left)
            .collect();
        assert!(near(r2_left[0].position, [1., SIDEWALK_KERB_LIFT, 35.]));
        assert!(near(r2_left[1].position, [1., SIDEWALK_KERB_LIFT, 25.]));
        assert!(near(r2_left[0].forward, [-1., 0., 0.]));
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
        let sw = SIDEWALK_KERB_LIFT;
        vec![
            [0., sw, 0.],
            [2., 0., 0.],
            [28., 0., 0.],
            [30., sw, 0.], // entry (z = 0)
            [30., sw, 40.],
            [28., 0., 40.],
            [2., 0., 40.],
            [0., sw, 40.], // exit (z = 40)
            [0., sw, 20.],
            [2., 0., 20.],
            [14., 0., 20.],
            [30., sw, 20.], // strip mid-section (z = 20)
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
        assert!(near(
            walk.stamps[0].position,
            [22., SIDEWALK_KERB_LIFT, 20.]
        ));
        // s = 1.5·seg → kerb (21,30) → outer (30,30) → (25.5,30).
        assert!(near(
            walk.stamps[1].position,
            [25.5, SIDEWALK_KERB_LIFT, 30.]
        ));
        // The stamp's facing is the strip's kerb→outer direction at
        // its own offset — +x here — not the side's walk direction
        // (+z): a prop authored front-first along −X faces the road.
        assert!(near(walk.stamps[0].forward, [1., 0., 0.]));
        assert!(near(walk.stamps[1].forward, [1., 0., 0.]));
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
        assert!(near(walk.stamps[0].position, [30., SIDEWALK_KERB_LIFT, 2.]));
        assert!(near(walk.stamps[1].position, [30., SIDEWALK_KERB_LIFT, 8.]));
        // No strip means no kerb→outer direction either: the fallback
        // aims the stamp away from the room's crossing midpoint —
        // outward toward the building line, not along the walk.
        let centre = [15., 0., 10.];
        for s in &walk.stamps {
            let want = norm_xz(sub(s.position, centre)).unwrap();
            assert!(near(s.forward, want));
        }
    }

    /// A def like [`def`] with an explicit kerb↔outer lerp factor:
    /// `0` stamps on the kerb chain, `1` on the outer edge.
    fn def_lerp(name: &str, lerp: f32, files: &[&str]) -> PropDef {
        PropDef {
            name: name.into(),
            start: 2.,
            distance: 6.,
            max_use: 1,
            lerp_min: lerp,
            lerp_max: lerp,
            files: files.iter().map(|s| s.to_string()).collect(),
            line: 0,
        }
    }

    #[test]
    fn a_kerb_side_stamp_stands_on_the_sidewalk_top() {
        // Operator report 4's regression: a prop stamped at the kerb
        // (lerp 0) sits on the raised top — before the lift it landed
        // at road level, kerb-height inside the rendered sidewalk.
        let city = psdl(
            quad_verts(),
            vec![room1_solo()],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![def_lerp("kerb", 0.0, &["pa"])],
            vec![rule("n01left", &["kerb"]), rule("n01right", &["kerb"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stamps.len(), 2);
        for s in &walk.stamps {
            // On the kerb chains (x = 2 left, x = 28 right) at top
            // height. The left side matches its strip reversed, so
            // both directions carry the lift.
            let on_edge = (s.position[0] - 2.0).abs() < 1e-3 || (s.position[0] - 28.0).abs() < 1e-3;
            assert!(on_edge, "pos {:?}", s.position);
            assert!(
                (s.position[1] - SIDEWALK_KERB_LIFT).abs() < 1e-3,
                "pos {:?}",
                s.position
            );
        }
    }

    #[test]
    fn a_flush_authored_outer_edge_keeps_its_height() {
        // Ramps and driveways author the outer chain flush with the
        // road: the lift applies to the kerb foot, never to authored
        // heights — a stamp on the building edge lands on the
        // authored vertex, not above it.
        let flush = quad_verts().into_iter().map(|v| [v[0], 0., v[2]]).collect();
        let city = psdl(
            flush,
            vec![room1_solo()],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![
                def_lerp("kerb", 0.0, &["pa"]),
                def_lerp("edge", 1.0, &["pb"]),
            ],
            vec![rule("n01left", &[]), rule("n01right", &["kerb", "edge"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stamps.len(), 2);
        // Kerb foot lifted to the top; authored flush edge untouched.
        assert!(near(walk.stamps[0].position, [28., SIDEWALK_KERB_LIFT, 2.]));
        assert!(near(walk.stamps[1].position, [30., 0., 2.]));
    }

    #[test]
    fn a_no_sidewalk_walkway_stamps_at_its_authored_height() {
        // RoadNoSidewalks (walkways, rails) carries no kerb: the
        // authored edge is already the surface, so authored Y is
        // preserved — no lift, on either side or direction.
        let mut verts = quad_verts();
        for i in [1usize, 2, 5, 6] {
            verts[i][1] = 2.0;
        }
        let walkway = RoomAttribute {
            last: false,
            kind: AttributeType::RoadNoSidewalks,
            subtype: 2,
            data: vec![1, 2, 6, 5],
        };
        let city = psdl(
            verts,
            vec![room(
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
                vec![walkway],
            )],
            &[0, 1],
            vec![path([1, 2, 0, 0], [5, 6, 0, 0], &[1])],
        );
        let (defs, rules) = tables(
            vec![def("lamp", 2., 6., 2, &["pb"])],
            vec![rule("n01left", &["lamp"]), rule("n01right", &["lamp"])],
        );
        let walk = walk_prop_rules(&city, &defs, &rules);
        assert_eq!(walk.stamps.len(), 4);
        for s in &walk.stamps {
            assert!((s.position[1] - 2.0).abs() < 1e-3, "pos {:?}", s.position);
        }
    }

    // ------------------------------------------------------------------
    // Shared pathset expansion and carriageway extraction.
    // ------------------------------------------------------------------

    fn pp(position: [f32; 3]) -> mm2_formats::pathset::PathPoint {
        mm2_formats::pathset::PathPoint {
            attributes: 0,
            position,
        }
    }

    fn ppath(points: &[[f32; 3]], kind: u8, spacing: u8) -> mm2_formats::pathset::Path {
        mm2_formats::pathset::Path {
            name: "sp_test".to_string(),
            selection: 0,
            points: points.iter().map(|&p| pp(p)).collect(),
            kind_code: kind,
            spacing_code: spacing,
        }
    }

    #[test]
    fn points_expansion_marks_unrotated_sites() {
        let p = ppath(&[[1., 0., 0.], [2., 0., 0.]], 0, 0);
        let s = path_stamp_sites(&p, 100);
        assert_eq!(s.capped, 0);
        assert_eq!(s.sites.len(), 2);
        assert!(s.sites.iter().all(|s| s.forward.is_none()));
        assert_eq!(s.sites[1].position, [2., 0., 0.]);
    }

    #[test]
    fn directed_expansion_faces_pair_targets() {
        let p = ppath(&[[0., 0., 0.], [0., 0., 5.], [9., 9., 9.]], 1, 0);
        let s = path_stamp_sites(&p, 100);
        // The lone trailing point stamps nothing.
        assert_eq!(s.sites.len(), 1);
        assert_eq!(s.sites[0].position, [0., 0., 0.]);
        assert_eq!(s.sites[0].forward, Some([0., 0., 5.]));
    }

    #[test]
    fn line_strip_restarts_spacing_per_segment() {
        // Two 4 m segments with a bend, spacing 1 m (code 4): four
        // stamps per segment at t = 0,1,2,3 plus the final vertex = 9.
        let p = ppath(&[[0., 0., 0.], [4., 0., 0.], [4., 0., 4.]], 2, 4);
        let s = path_stamp_sites(&p, 100);
        assert_eq!(s.capped, 0);
        let positions: Vec<[f32; 3]> = s.sites.iter().map(|s| s.position).collect();
        assert_eq!(
            positions,
            vec![
                [0., 0., 0.],
                [1., 0., 0.],
                [2., 0., 0.],
                [3., 0., 0.],
                [4., 0., 0.],
                [4., 0., 1.],
                [4., 0., 2.],
                [4., 0., 3.],
                [4., 0., 4.],
            ]
        );
        // The bend: the shared vertex's stamp faces the second
        // segment, and the cap faces it too.
        assert_eq!(s.sites[4].forward, Some([0., 0., 1.]));
        assert_eq!(s.sites[8].forward, Some([0., 0., 1.]));
    }

    #[test]
    fn the_budget_caps_and_counts() {
        let p = ppath(&[[0., 0., 0.], [4., 0., 0.]], 2, 4);
        let s = path_stamp_sites(&p, 2);
        assert_eq!(s.sites.len(), 2);
        // Four wanted on the segment + the cap vertex; 2 emitted.
        assert_eq!(s.capped, 3);
    }

    #[test]
    fn zero_spacing_and_unknown_kinds_stamp_minimally() {
        // spacing 0 → one unrotated stamp per vertex (designed
        // fallback — no documented subdivision).
        let p = ppath(&[[0., 0., 0.], [4., 0., 0.]], 2, 0);
        let s = path_stamp_sites(&p, 100);
        assert_eq!(s.sites.len(), 2);
        assert!(s.sites.iter().all(|s| s.forward.is_none()));
        // Undocumented kind stamps nothing.
        let p = ppath(&[[0., 0., 0.], [4., 0., 0.]], 9, 4);
        assert!(path_stamp_sites(&p, 100).sites.is_empty());
    }

    #[test]
    fn non_finite_points_stamp_nothing() {
        let p = ppath(&[[0., 0., 0.], [f32::NAN, 0., 1.], [4., 0., 0.]], 0, 0);
        let s = path_stamp_sites(&p, 100);
        assert_eq!(s.sites.len(), 2);
    }

    /// A `RoadWithSidewalks` attribute over `quad_verts` room 1 — the
    /// same one `quad_road` builds, asserting only the road band is
    /// extracted (sidewalk bands are not carriageway).
    #[test]
    fn carriageways_extracts_the_road_band_only() {
        let city = psdl(
            quad_verts(),
            vec![room(
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
            )],
            &[0, 0],
            Vec::new(),
        );
        let cw = carriageways(&city);
        assert_eq!(cw.len(), 1);
        assert_eq!(cw[0].room, 1);
        assert_eq!(cw[0].kind, AttributeType::RoadWithSidewalks);
        // The band is road_l↔road_r: x ∈ [2, 28], z ∈ [0, 20] — the
        // x ∈ [0, 2] and [28, 30] sidewalks are outside.
        let xs: Vec<f32> = cw[0].ring.iter().map(|v| v[0]).collect();
        assert!(xs.iter().all(|&x| (2.0..=28.0).contains(&x)));
        assert_eq!(cw[0].tris.len(), 2);
    }

    #[test]
    fn carriageways_extracts_divided_road_as_two_strips() {
        // Divided room: 8 m roadway, 4 m median, 8 m roadway along z;
        // 2 m sidewalks outboard. Section layout is [sw_l, rl_out,
        // rl_in, rr_in, rr_out, sw_r] per the format doc.
        let verts = vec![
            [0., 0., 0.],   // 0 sw_l near
            [2., 0., 0.],   // 1 rl_out near
            [10., 0., 0.],  // 2 rl_in near
            [14., 0., 0.],  // 3 rr_in near
            [22., 0., 0.],  // 4 rr_out near
            [24., 0., 0.],  // 5 sw_r near
            [0., 0., 20.],  // 6 sw_l far
            [2., 0., 20.],  // 7 rl_out far
            [10., 0., 20.], // 8 rl_in far
            [14., 0., 20.], // 9 rr_in far
            [22., 0., 20.], // 10 rr_out far
            [24., 0., 20.], // 11 sw_r far
        ];
        let attr = RoomAttribute {
            last: false,
            kind: AttributeType::DividedRoad,
            subtype: 2,
            data: vec![0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        };
        let city = psdl(verts, vec![room(&[], vec![attr])], &[0], Vec::new());
        let cw = carriageways(&city);
        assert_eq!(cw.len(), 2);
        // Left carriageway x ∈ [2,10], right x ∈ [14,22] — the median
        // x ∈ [10,14] is not drivable.
        let mut bands: Vec<(f32, f32)> = cw
            .iter()
            .map(|c| {
                let xs: Vec<f32> = c.ring.iter().map(|v| v[0]).collect();
                (
                    xs.iter().cloned().fold(f32::INFINITY, f32::min),
                    xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                )
            })
            .collect();
        bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(bands, vec![(2.0, 10.0), (14.0, 22.0)]);
    }

    #[test]
    fn carriageways_covers_fans_and_crosswalks() {
        let verts = vec![
            [0., 0., 0.],  // 0: fan pivot
            [4., 0., 0.],  // 1
            [4., 0., 4.],  // 2
            [0., 0., 4.],  // 3
            [8., 0., 0.],  // 4: crosswalk quad
            [12., 0., 0.], // 5
            [8., 0., 4.],  // 6
            [12., 0., 4.], // 7
            [16., 0., 0.], // 8: no-sidewalk strip
            [20., 0., 0.], // 9
            [16., 0., 4.], // 10
            [20., 0., 4.], // 11
        ];
        let room = room(
            &[],
            vec![
                RoomAttribute {
                    last: false,
                    kind: AttributeType::RoadFan,
                    subtype: 2, // two triangles: pivot + ring of 3
                    data: vec![0, 1, 2, 3],
                },
                RoomAttribute {
                    last: false,
                    kind: AttributeType::Crosswalk,
                    subtype: 0,
                    data: vec![4, 5, 6, 7],
                },
                RoomAttribute {
                    last: false,
                    kind: AttributeType::RoadNoSidewalks,
                    subtype: 2,
                    data: vec![8, 9, 10, 11],
                },
                // A sidewalk strip in the same room must not become
                // carriageway.
                RoomAttribute {
                    last: false,
                    kind: AttributeType::SidewalkStrip,
                    subtype: 2,
                    data: vec![0, 1, 3, 2],
                },
            ],
        );
        let city = psdl(verts, vec![room], &[0], Vec::new());
        let cw = carriageways(&city);
        let mut kinds: Vec<AttributeType> = cw.iter().map(|c| c.kind).collect();
        kinds.sort_by_key(|k| format!("{k:?}"));
        assert_eq!(
            kinds,
            vec![
                AttributeType::Crosswalk,
                AttributeType::RoadFan,
                AttributeType::RoadNoSidewalks,
            ]
        );
        let fan = cw
            .iter()
            .find(|c| c.kind == AttributeType::RoadFan)
            .unwrap();
        assert_eq!(fan.tris.len(), 2); // pivot + two ring edges
        assert_eq!(fan.ring.len(), 3);
        // Crosswalk ring orders (0,1,3,2) around the quad.
        let x = cw
            .iter()
            .find(|c| c.kind == AttributeType::Crosswalk)
            .unwrap();
        assert_eq!(x.ring.len(), 4);
        assert_eq!(x.tris.len(), 2);
    }

    #[test]
    fn carriageways_skips_malformed_attributes() {
        let mut bad = quad_road(false);
        bad.data.truncate(6); // subtype claims 2 sections, data has 1.5
        let city = psdl(quad_verts(), vec![room(&[], vec![bad])], &[0], Vec::new());
        assert!(carriageways(&city).is_empty());
    }

    // ------------------------------------------------------------------
    // Walkable-surface extraction (the placement audit's vertical leg).
    // ------------------------------------------------------------------

    #[test]
    fn walkable_surfaces_adds_the_raised_sidewalk_tops() {
        let city = psdl(quad_verts(), vec![room1_solo()], &[0, 0], Vec::new());
        let ws = walkable_surfaces(&city);
        let count = |b: SurfaceBand| ws.iter().filter(|c| c.band == b).count();
        assert_eq!(count(SurfaceBand::Driving), 1);
        assert_eq!(count(SurfaceBand::SidewalkTop), 2);
        assert_eq!(count(SurfaceBand::FloorFan), 0);
        // Each top is flat at kerb height and spans the 2 m sidewalk
        // margin outboard of the road band — the surface
        // `emit_sidewalk` renders and collides.
        let mut bands: Vec<(f32, f32)> = ws
            .iter()
            .filter(|c| c.band == SurfaceBand::SidewalkTop)
            .map(|c| {
                assert!(
                    c.ring
                        .iter()
                        .all(|v| (v[1] - SIDEWALK_KERB_LIFT).abs() < 1e-4),
                    "top must be flat at kerb height: {:?}",
                    c.ring
                );
                let xs: Vec<f32> = c.ring.iter().map(|v| v[0]).collect();
                (
                    xs.iter().cloned().fold(f32::INFINITY, f32::min),
                    xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                )
            })
            .collect();
        bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(bands, vec![(0.0, 2.0), (28.0, 30.0)]);
    }

    #[test]
    fn walkable_surfaces_keeps_walkways_at_authored_height() {
        // RoadNoSidewalks is one driving band at authored height —
        // no sidewalk top, no lift.
        let mut verts = quad_verts();
        for i in [1usize, 2, 5, 6] {
            verts[i][1] = 2.0;
        }
        let walkway = RoomAttribute {
            last: false,
            kind: AttributeType::RoadNoSidewalks,
            subtype: 2,
            data: vec![1, 2, 6, 5],
        };
        let city = psdl(verts, vec![room(&[], vec![walkway])], &[0], Vec::new());
        let ws = walkable_surfaces(&city);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].band, SurfaceBand::Driving);
        assert!(ws[0].ring.iter().all(|v| v[1] == 2.0));
    }

    #[test]
    fn walkable_surfaces_lifts_strips_and_reads_fan_normals() {
        // A SidewalkStrip's (ground, top) pairs plus a horizontal
        // plaza Fan and a vertical wall Fan in one room.
        let verts = vec![
            [2., 0., 0.],    // 0: strip ground near
            [0., 0.15, 0.],  // 1: strip top near
            [2., 0., 20.],   // 2: ground far
            [0., 0.15, 20.], // 3: top far
            [8., 0., 0.],    // 4: plaza fan pivot
            [12., 0., 0.],   // 5
            [12., 0., 4.],   // 6
            [8., 0., 4.],    // 7
            [16., 0., 0.],   // 8: wall fan pivot
            [20., 0., 0.],   // 9
            [20., 3., 0.],   // 10
            [16., 3., 0.],   // 11
        ];
        let attr = |kind: AttributeType, subtype: u8, data: &[u16]| RoomAttribute {
            last: false,
            kind,
            subtype,
            data: data.to_vec(),
        };
        let city = psdl(
            verts,
            vec![room(
                &[],
                vec![
                    attr(AttributeType::SidewalkStrip, 2, &[0, 1, 2, 3]),
                    attr(AttributeType::Fan, 2, &[4, 5, 6, 7]),
                    attr(AttributeType::Fan, 2, &[8, 9, 10, 11]),
                ],
            )],
            &[0],
            Vec::new(),
        );
        let ws = walkable_surfaces(&city);
        let tops: Vec<&Carriageway> = ws
            .iter()
            .filter(|c| c.band == SurfaceBand::SidewalkTop)
            .collect();
        assert_eq!(tops.len(), 1);
        assert_eq!(tops[0].kind, AttributeType::SidewalkStrip);
        // Lifted-ground → authored top, flat at kerb height.
        assert!(
            tops[0]
                .ring
                .iter()
                .all(|v| (v[1] - SIDEWALK_KERB_LIFT).abs() < 1e-4),
            "strip top: {:?}",
            tops[0].ring
        );
        let fans: Vec<&Carriageway> = ws
            .iter()
            .filter(|c| c.band == SurfaceBand::FloorFan)
            .collect();
        assert_eq!(fans.len(), 1, "the wall fan never becomes a floor");
        assert_eq!(fans[0].kind, AttributeType::Fan);
    }
}
