//! `placement` audit (F03-C): where each placement channel stamps
//! props relative to the city's authored drivable surface.
//!
//! Three channels are audited independently per city — the operator's
//! "prop in the road" report named them separately, and each has its
//! own stamping authority:
//!
//! - `city/<c>.inst` — INST placements (verbatim authored transforms)
//! - `city/<c>/props.pathset` — stamped prop rows, expanded through
//!   the shared [`path_stamp_sites`] with the runtime's own
//!   classification (`PATHnn` labels, `giz_*` animated objects and
//!   texture-named decal paths are counted, not stamped) and the same
//!   [`MAX_PATHSET_STAMPS`] budget, so the audit measures exactly what
//!   the game stamps. Event-scoped `race/<c>/*.pathset` overlays use
//!   the same expansion at runtime but are excluded here — they are
//!   covered by the `event` audit's dependency closure.
//! - `propdefs.csv` + `proprules.csv` + the PSDL `prop_rule` bytes —
//!   stamped by [`walk_prop_rules`], the same walk the game runs.
//!
//! The reference is the authored PSDL carriageway —
//! [`carriageways`]: `RoadWithSidewalks` road bands, `DividedRoad`
//! carriageways (median excluded), `RoadNoSidewalks`, `Crosswalk` and
//! `RoadFan` regions. A stamp counts *in-road* when its XZ lands
//! inside a region and its height sits within the band below. This is
//! a measured geometric check, not an authority on intent: retail
//! authors legitimately stamp onto these surfaces — street banners
//! pivot on the carriageway and hang overhead, bollards and trees are
//! planted on pedestrianised `RoadNoSidewalks` streets and plaza
//! `RoadFan`s, and INST facades sit under elevated walkways. Hits are
//! therefore reported with positions and a channel×kind histogram for
//! review; the channel×kind split is the signal — a `RoadWithSidewalks`
//! band hit is hard evidence, a `RoadFan`/`RoadNoSidewalks` hit may be
//! authored plaza dressing.
//!
//! The stamped channels additionally sweep each prop's rendered
//! geometry ([`stamp_space_verts`]) through the stamp's basis —
//! [`yawed_basis`], the same transform the runtime builds — and test
//! every vertex against the carriageway. A vertex inside a region
//! within the street-level band is footprint evidence ("the bench
//! sticks into the road" — the check that sees orientation, which the
//! origin test cannot); vertices in the overhead band record a
//! legitimate overhang (lamp arms, banners) as a separate count. INST
//! placements are verbatim authored transforms — their orientation is
//! not synthesized — so they keep the origin check alone.
//!
//! `--strict` exits nonzero on missing/failed expected sources,
//! unresolved names and channel issues. In-road and footprint counts
//! are measured findings, not failures — strict stays meaningful on
//! retail data.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use mm2_assets::Vfs;
use mm2_formats::banger::BangerData;
use mm2_formats::inst::{self, InstPlacement};
use mm2_formats::pathset::Pathset;
use mm2_formats::pkg::Pkg;
use mm2_formats::proprules::{PropDefs, PropRules};
use mm2_formats::psdl::{AttributeType, Psdl};
use mm2_game::{
    Carriageway, MAX_PATHSET_STAMPS, carriageways, path_stamp_sites, stamp_space_verts, yawed_basis,
};

use crate::TEXTURE_EXTS;

/// How far below the carriageway surface a stamp may sit and still
/// count as on it — covers authored positions planted at kerb height
/// against a cambered or dipped surface.
const BAND_BELOW: f32 = 1.0;
/// How far above the surface a stamp may sit and still count — the
/// stamp pivot is authored at the prop's base, so this only needs to
/// cover surface-sampling error plus kerb-height lifts.
const BAND_ABOVE: f32 = 0.6;
/// In-road hits listed per city; the total is always printed.
const MAX_HITS_SHOWN: usize = 24;
/// Footprint band ceiling above the surface: verts below this are
/// street-level prop body — inside a region, they are the "bench in
/// the road" evidence (a car is ~2 m tall; 2.5 m clears one).
const BODY_TOP: f32 = 2.5;
/// Overhang band ceiling: verts between [`BODY_TOP`] and this inside a
/// region are overhead content (lamp arms, banners) — a separate
/// count, not a footprint hit.
const OVERHANG_TOP: f32 = 8.0;
/// A vert less than this deep inside a region is kerb-edge contact,
/// not an incursion — bounds and ring vertices share float error at
/// the shared edge.
const FOOTPRINT_EPS: f32 = 0.15;

/// A carriageway region plus its XZ bounds for cheap rejection.
struct Region {
    room: u16,
    kind: AttributeType,
    min: [f32; 2],
    max: [f32; 2],
    ring: Vec<[f32; 3]>,
    tris: Vec<[[f32; 3]; 3]>,
}

impl Region {
    fn from_carriageway(c: &Carriageway) -> Self {
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for t in &c.tris {
            for v in t {
                min[0] = min[0].min(v[0]);
                min[1] = min[1].min(v[2]);
                max[0] = max[0].max(v[0]);
                max[1] = max[1].max(v[2]);
            }
        }
        Region {
            room: c.room,
            kind: c.kind,
            min,
            max,
            ring: c.ring.clone(),
            tris: c.tris.clone(),
        }
    }

    /// Distance on XZ from `p` to the nearest boundary edge — how far
    /// *inside* the region a stamp sits. A stamp planted on the kerb
    /// line reports ~0; one in the lane's middle reports metres.
    fn depth(&self, p: [f32; 3]) -> f32 {
        let (px, pz) = (p[0], p[2]);
        let mut best = f32::INFINITY;
        for w in self.ring.windows(2) {
            let (ax, az) = (w[0][0], w[0][2]);
            let (bx, bz) = (w[1][0], w[1][2]);
            let (dx, dz) = (bx - ax, bz - az);
            let len2 = dx * dx + dz * dz;
            let t = if len2 > f32::EPSILON {
                (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ez) = (px - (ax + dx * t), pz - (az + dz * t));
            best = best.min((ex * ex + ez * ez).sqrt());
        }
        // Close the ring: last vertex back to first.
        if let (Some(&a), Some(&b)) = (self.ring.last(), self.ring.first()) {
            let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
            let len2 = dx * dx + dz * dz;
            let t = if len2 > f32::EPSILON {
                (((px - a[0]) * dx + (pz - a[2]) * dz) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ez) = (px - (a[0] + dx * t), pz - (a[2] + dz * t));
            best = best.min((ex * ex + ez * ez).sqrt());
        }
        best
    }
}

/// Barycentric test on XZ: `Some(surface_y)` when `p`'s XZ projection
/// lies inside triangle `t`, `None` otherwise.
fn tri_surface_y(p: [f32; 3], t: &[[f32; 3]; 3]) -> Option<f32> {
    let (px, pz) = (p[0], p[2]);
    let (ax, az) = (t[0][0], t[0][2]);
    let (bx, bz) = (t[1][0], t[1][2]);
    let (cx, cz) = (t[2][0], t[2][2]);
    let d = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if d.abs() < 1e-9 {
        return None; // degenerate XZ triangle
    }
    let w0 = ((bz - cz) * (px - cx) + (cx - bx) * (pz - cz)) / d;
    let w1 = ((cz - az) * (px - cx) + (ax - cx) * (pz - cz)) / d;
    let w2 = 1.0 - w0 - w1;
    // A small epsilon absorbs float noise on shared edges — a stamp
    // exactly on a kerb edge counts as on whichever side it lands.
    const EDGE_EPS: f32 = 1e-4;
    if w0 < -EDGE_EPS || w1 < -EDGE_EPS || w2 < -EDGE_EPS {
        return None;
    }
    Some(w0 * t[0][1] + w1 * t[1][1] + w2 * t[2][1])
}

/// One in-road finding.
struct Hit {
    channel: &'static str,
    detail: String,
    pos: [f32; 3],
    room: u16,
    kind: AttributeType,
    /// Stamp height minus the carriageway surface height.
    dy: f32,
    /// XZ distance from the region's boundary edge — kerb-edge stamps
    /// report ~0, mid-lane stamps report metres.
    depth: f32,
}

/// Per-channel tallies — every discovered/expected item lands in a
/// count; nothing filters out of a denominator.
#[derive(Default)]
struct Channel {
    /// Expected source files for this channel.
    expected: usize,
    /// Expected sources that resolved.
    found: usize,
    /// Expected sources missing or failing to parse.
    failed: usize,
    /// Placement records/paths/rule-rooms considered.
    items: usize,
    /// Stamped positions measured against the carriageway.
    stamps: usize,
    /// Stamps inside a carriageway region at surface height.
    in_road: usize,
    /// Stamps whose rendered geometry was swept through the stamp
    /// basis (stamped channels only).
    swept: usize,
    /// Swept stamps whose street-level verts reach a carriageway
    /// region past [`FOOTPRINT_EPS`] — the prop's body sits on the
    /// drivable surface.
    body_in_road: usize,
    /// Swept stamps with verts over a region in the overhead band —
    /// legitimate overhangs (lamp arms, banners), not hits.
    overhang: usize,
    /// Items classified but never stamped (labels, decals, animated,
    /// unresolved names, cap-overflow).
    skipped: usize,
    /// Stamps suppressed by the shared stamp budget.
    capped: usize,
    /// Channel anomalies (dead refs, walk issues, …).
    issues: Vec<String>,
}

/// Find the carriageway region `p` stands on: the first region whose
/// XZ footprint contains `p` and whose surface sits within
/// `[p.y - BAND_BELOW, p.y + BAND_ABOVE]`. Regions do not overlap on
/// the drivable surface (sidewalks, medians and junctions are
/// separate), so first-hit is unambiguous.
fn on_road(p: [f32; 3], regions: &[Region]) -> Option<(&Region, f32)> {
    for r in regions {
        if p[0] < r.min[0] || p[0] > r.max[0] || p[2] < r.min[1] || p[2] > r.max[1] {
            continue;
        }
        for t in &r.tris {
            if let Some(y) = tri_surface_y(p, t) {
                let dy = p[1] - y;
                if (-BAND_BELOW..=BAND_ABOVE).contains(&dy) {
                    return Some((r, dy));
                }
            }
        }
    }
    None
}

/// Cache of prop name → stamp-space rendered verts ([`stamp_space_verts`]
/// — best-LOD, bound `CG`/ground-lift offset applied), resolved exactly
/// like the runtime's `PropCache` plus the `tune/banger` record that
/// chooses the offset convention.
struct GeomCache<'a> {
    vfs: &'a Vfs,
    cache: HashMap<String, Option<Vec<[f32; 3]>>>,
}

impl GeomCache<'_> {
    fn get(&mut self, name: &str) -> Option<&Vec<[f32; 3]>> {
        // The runtime's PropCache resolves the lowercased name; match.
        let key = name.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let built = self.build(&key);
            self.cache.insert(key.clone(), built);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    fn build(&self, name: &str) -> Option<Vec<[f32; 3]>> {
        let res = self
            .vfs
            .resolve_preferred(&format!("geometry/{name}"), &["pkg"])
            .or_else(|| self.vfs.resolve_preferred(name, &["pkg"]))?;
        let pkg = Pkg::parse(&self.vfs.read(&res).ok()?).ok()?;
        let cg = self
            .vfs
            .read_path(&format!("tune/banger/{name}.dgbangerdata"))
            .ok()
            .and_then(|(b, _)| BangerData::parse(&String::from_utf8_lossy(&b)).ok())
            // Same clean `BangerDefinition::from_record` applies —
            // `MIRROR_Z` is off, so `mirrored_cg` is the identity.
            .map(|d| d.cg.map(|c| if c.is_finite() { c } else { 0.0 }));
        let verts = stamp_space_verts(&pkg, cg);
        (!verts.is_empty()).then_some(verts)
    }
}

/// One stamp's placement for the footprint sweep: `axes` is the
/// stamp's basis images ([`yawed_basis`] for a directed stamp,
/// identity for an unrotated one), `verts` its stamp-space rendered
/// geometry.
struct FootprintStamp<'a> {
    channel: &'static str,
    detail: String,
    position: [f32; 3],
    axes: ([f32; 3], [f32; 3], [f32; 3]),
    verts: &'a [[f32; 3]],
}

/// Sweep one stamp's rendered geometry through its basis and test the
/// verts against the carriageway — the orientation check the origin
/// test cannot see. A vert inside a region at street level past
/// [`FOOTPRINT_EPS`] deep counts the stamp `body_in_road`; one in the
/// overhead band counts `overhang` instead.
fn measure_footprint(
    stamp: FootprintStamp<'_>,
    regions: &[Region],
    out: &mut Channel,
    hits: &mut Vec<Hit>,
) {
    out.swept += 1;
    let (x, y, z) = stamp.axes;
    let position = stamp.position;
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let world = |v: &[f32; 3]| -> [f32; 3] {
        [
            position[0] + v[0] * x[0] + v[1] * y[0] + v[2] * z[0],
            position[1] + v[0] * x[1] + v[1] * y[1] + v[2] * z[1],
            position[2] + v[0] * x[2] + v[1] * y[2] + v[2] * z[2],
        ]
    };
    for v in stamp.verts {
        let w = world(v);
        min[0] = min[0].min(w[0]);
        min[1] = min[1].min(w[2]);
        max[0] = max[0].max(w[0]);
        max[1] = max[1].max(w[2]);
    }
    let candidates: Vec<&Region> = regions
        .iter()
        .filter(|r| {
            min[0] <= r.max[0] && max[0] >= r.min[0] && min[1] <= r.max[1] && max[1] >= r.min[1]
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    // Deepest street-level vert carries the finding; overhead verts
    // only flip the overhang flag.
    let mut deepest: Option<(f32, f32, &Region)> = None;
    let mut overhang = false;
    for v in stamp.verts {
        let w = world(v);
        for r in &candidates {
            if w[0] < r.min[0] || w[0] > r.max[0] || w[2] < r.min[1] || w[2] > r.max[1] {
                continue;
            }
            let mut inside = false;
            for t in &r.tris {
                if let Some(sy) = tri_surface_y(w, t) {
                    inside = true;
                    let dy = w[1] - sy;
                    if (-BAND_BELOW..=BODY_TOP).contains(&dy) {
                        let dep = r.depth(w);
                        if deepest.is_none_or(|(d, _, _)| dep > d) {
                            deepest = Some((dep, dy, r));
                        }
                    } else if (BODY_TOP..=OVERHANG_TOP).contains(&dy) {
                        overhang = true;
                    }
                    break;
                }
            }
            if inside {
                break; // regions do not overlap on the drivable surface
            }
        }
    }
    if let Some((dep, dy, r)) = deepest.filter(|(d, _, _)| *d > FOOTPRINT_EPS) {
        out.body_in_road += 1;
        hits.push(Hit {
            channel: stamp.channel,
            detail: stamp.detail,
            pos: stamp.position,
            room: r.room,
            kind: r.kind,
            dy,
            depth: dep,
        });
    }
    if overhang {
        out.overhang += 1;
    }
}

/// Measure one batch of stamped positions for `channel`, recording
/// hits into `hits`.
fn measure(
    channel: &'static str,
    positions: impl Iterator<Item = ([f32; 3], String)>,
    regions: &[Region],
    out: &mut Channel,
    hits: &mut Vec<Hit>,
) {
    for (pos, detail) in positions {
        if !pos.iter().all(|c| c.is_finite()) {
            out.issues
                .push(format!("{channel}: {detail} has a non-finite position"));
            continue;
        }
        out.stamps += 1;
        if let Some((r, dy)) = on_road(pos, regions) {
            out.in_road += 1;
            hits.push(Hit {
                channel,
                detail,
                pos,
                room: r.room,
                kind: r.kind,
                dy,
                depth: r.depth(pos),
            });
        }
    }
}

/// The basis an unrotated stamp gets — `yawed_basis` on a degenerate
/// direction returns the same identity axes.
const IDENTITY_BASIS: ([f32; 3], [f32; 3], [f32; 3]) =
    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);

/// Run the audit for one city against a parsed PSDL and its
/// extracted carriageway regions. `hits` collects in-road origins,
/// `fp_hits` swept-footprint body hits.
fn audit_city(
    vfs: &Vfs,
    city: &str,
    psdl: &Psdl,
    regions: &[Region],
    hits: &mut Vec<Hit>,
    fp_hits: &mut Vec<Hit>,
) -> (BTreeMap<&'static str, Channel>, Vec<String>) {
    let mut channels: BTreeMap<&'static str, Channel> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut geoms = GeomCache {
        vfs,
        cache: HashMap::new(),
    };
    let inst = channels.entry("inst").or_default();
    inst.expected = 1;
    let inst_path = format!("city/{city}.inst");
    match vfs.read_path(&inst_path) {
        Ok((bytes, res)) => {
            inst.found += 1;
            match inst::parse(&bytes) {
                Ok(comps) => {
                    inst.items += comps.len();
                    let positions = comps.iter().enumerate().map(|(i, c)| {
                        let pos = match &c.placement {
                            InstPlacement::Coordinate(c) => c.origin,
                            InstPlacement::Simple(s) => s.location,
                        };
                        (
                            pos,
                            format!("{}#{i} {} room {}", res.logical, c.package_name, c.room),
                        )
                    });
                    measure("inst", positions, regions, inst, hits);
                }
                Err(e) => {
                    inst.failed += 1;
                    failures.push(format!("{inst_path}: {e}"));
                }
            }
        }
        Err(e) => {
            inst.failed += 1;
            failures.push(format!("{inst_path}: {e}"));
        }
    }

    let pathset = channels.entry("pathset").or_default();
    pathset.expected = 1;
    let pathset_path = format!("city/{city}/props.pathset");
    match vfs.read_path(&pathset_path) {
        Ok((bytes, res)) => {
            pathset.found += 1;
            match Pathset::parse(&bytes) {
                Ok(ps) => {
                    let mut pkg_class: HashMap<String, bool> = HashMap::new();
                    let mut prop_pkg = |vfs: &Vfs, name: &str| -> bool {
                        *pkg_class.entry(name.to_string()).or_insert_with(|| {
                            let res = vfs
                                .resolve_preferred(&format!("geometry/{name}"), &["pkg"])
                                .or_else(|| vfs.resolve_preferred(name, &["pkg"]));
                            // The runtime's PropCache fails a name whose
                            // PKG cannot decode; match that here so an
                            // unparseable pkg lands in `skipped`, not
                            // `stamps`.
                            res.and_then(|r| vfs.read(&r).ok())
                                .and_then(|b| Pkg::parse(&b).ok())
                                .is_some()
                        })
                    };
                    // Thread the file's stamp budget exactly like
                    // `stamp_pathset` does.
                    let mut left = MAX_PATHSET_STAMPS;
                    for (pi, path) in ps.paths.iter().enumerate() {
                        pathset.items += 1;
                        let Some(name) = path.asset_name() else {
                            pathset.skipped += 1; // PATHnn route label
                            continue;
                        };
                        let name = name.to_ascii_lowercase();
                        if name.starts_with("giz_") {
                            pathset.skipped += 1; // animated object
                            continue;
                        }
                        if !prop_pkg(vfs, &name) {
                            pathset.skipped += 1;
                            if TEXTURE_EXTS
                                .iter()
                                .any(|ext| vfs.resolve(&format!("texture/{name}.{ext}")).is_some())
                            {
                                // decal leftover — classified, not stamped
                            } else {
                                pathset.issues.push(format!(
                                    "pathset: {name:?} (path {pi}) resolves to no geometry or texture"
                                ));
                            }
                            continue;
                        }
                        let sites = path_stamp_sites(path, left);
                        left -= sites.sites.len();
                        pathset.capped += sites.capped;
                        measure(
                            "pathset",
                            sites.sites.iter().enumerate().map(|(si, s)| {
                                (s.position, format!("{}:{pi}:{si} {name}", res.logical))
                            }),
                            regions,
                            pathset,
                            hits,
                        );
                        // Swept footprint — same yawed basis the
                        // runtime builds for each site.
                        if let Some(verts) = geoms.get(&name) {
                            for (si, s) in sites.sites.iter().enumerate() {
                                measure_footprint(
                                    FootprintStamp {
                                        channel: "pathset",
                                        detail: format!("{}:{pi}:{si} {name}", res.logical),
                                        position: s.position,
                                        axes: s.forward.map(yawed_basis).unwrap_or(IDENTITY_BASIS),
                                        verts,
                                    },
                                    regions,
                                    pathset,
                                    fp_hits,
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    pathset.failed += 1;
                    failures.push(format!("{pathset_path}: {e}"));
                }
            }
        }
        Err(e) => {
            pathset.failed += 1;
            failures.push(format!("{pathset_path}: {e}"));
        }
    }

    let rules = channels.entry("prop-rule").or_default();
    rules.expected = 2;
    let defs_path = format!("city/{city}/propdefs.csv");
    let rules_path = format!("city/{city}/proprules.csv");
    let defs = vfs
        .read_path(&defs_path)
        .map_err(|e| format!("{defs_path}: {e}"))
        .and_then(|(b, _)| {
            PropDefs::parse(&String::from_utf8_lossy(&b)).map_err(|e| format!("{defs_path}: {e}"))
        });
    let rule_rows = vfs
        .read_path(&rules_path)
        .map_err(|e| format!("{rules_path}: {e}"))
        .and_then(|(b, _)| {
            PropRules::parse(&String::from_utf8_lossy(&b)).map_err(|e| format!("{rules_path}: {e}"))
        });
    rules.found = usize::from(defs.is_ok()) + usize::from(rule_rows.is_ok());
    match (defs, rule_rows) {
        (Ok(defs), Ok(rule_rows)) => {
            rules.items += psdl.prop_rules.iter().filter(|&&b| b != 0).count();
            let walk = mm2_game::walk_prop_rules(psdl, &defs, &rule_rows);
            rules.issues.extend(walk.stats.issues.iter().cloned());
            if walk.stats.rule_rooms_unreached > 0 {
                rules.issues.push(format!(
                    "{} rule-bearing rooms unreached by any path",
                    walk.stats.rule_rooms_unreached
                ));
            }
            if walk.stats.rooms_bad_ref > 0 {
                rules
                    .issues
                    .push(format!("{} bad road_rooms refs", walk.stats.rooms_bad_ref));
            }
            if walk.stats.rules_missing > 0 {
                rules.issues.push(format!(
                    "{} rooms name undefined rule numbers",
                    walk.stats.rules_missing
                ));
            }
            if walk.stats.defs_missing > 0 {
                rules.issues.push(format!(
                    "{} rule props resolve no propdef",
                    walk.stats.defs_missing
                ));
            }
            rules.capped += walk.stats.stamps_capped;
            measure(
                "prop-rule",
                walk.stamps.iter().map(|s| {
                    (
                        s.position,
                        format!("path {} room {} {:?} {}", s.path, s.room, s.side, s.pkg),
                    )
                }),
                regions,
                rules,
                hits,
            );
            // Swept footprint through each stamp's measured facing —
            // the prop's −X front toward the carriageway.
            for s in &walk.stamps {
                let Some(verts) = geoms.get(&s.pkg) else {
                    continue;
                };
                measure_footprint(
                    FootprintStamp {
                        channel: "prop-rule",
                        detail: format!("path {} room {} {:?} {}", s.path, s.room, s.side, s.pkg),
                        position: s.position,
                        axes: yawed_basis(s.forward),
                        verts,
                    },
                    regions,
                    rules,
                    fp_hits,
                );
            }
        }
        (d, r) => {
            for res in [d.err(), r.err()].into_iter().flatten() {
                rules.failed += 1;
                failures.push(res);
            }
        }
    }

    (channels, failures)
}

pub fn placement(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = crate::build_vfs(dir, mods)?;
    let cities: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };

    println!("== prop placement audit ==");
    println!("  reference: authored PSDL carriageway regions");
    println!(
        "  in-road band: surface height {:.2} m below .. {:.2} m above the stamp",
        BAND_BELOW, BAND_ABOVE
    );

    let mut failures: Vec<String> = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    let mut any_stamps = false;
    let mut any_in_road = 0usize;
    let mut any_body = 0usize;

    for c in &cities {
        println!("city {c}");
        let psdl_path = format!("city/{c}.psdl");
        let Some(res) = vfs.resolve(&psdl_path) else {
            println!("  {psdl_path}: missing — channels cannot be audited");
            failures.push(format!("{psdl_path}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let psdl = match Psdl::parse(&bytes) {
            Ok(p) => p,
            Err(e) => {
                println!("  {psdl_path}: parse failed — channels cannot be audited");
                failures.push(format!("{psdl_path}: {e}"));
                continue;
            }
        };
        let cw = carriageways(&psdl);
        let regions: Vec<Region> = cw.iter().map(Region::from_carriageway).collect();
        let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
        for r in &regions {
            *by_kind.entry(format!("{:?}", r.kind)).or_default() += 1;
        }
        println!(
            "  carriageway regions: {} ({})",
            regions.len(),
            by_kind
                .iter()
                .map(|(k, n)| format!("{k}={n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );

        let mut hits = Vec::new();
        let mut fp_hits = Vec::new();
        let (channels, ch_failures) = audit_city(&vfs, c, &psdl, &regions, &mut hits, &mut fp_hits);
        failures.extend(ch_failures);
        for (name, ch) in &channels {
            println!(
                "  {name}: {}/{} sources ok ({} failed), {} items, {} stamps, {} in-road, {} swept, {} body-in-road, {} overhang, {} skipped, {} capped, {} issues",
                ch.found,
                ch.expected,
                ch.failed,
                ch.items,
                ch.stamps,
                ch.in_road,
                ch.swept,
                ch.body_in_road,
                ch.overhang,
                ch.skipped,
                ch.capped,
                ch.issues.len(),
            );
            issues.extend(ch.issues.iter().cloned());
            any_stamps |= ch.stamps > 0;
            any_in_road += ch.in_road;
            any_body += ch.body_in_road;
        }
        // In-road totals by channel × region kind: `RoadFan` regions
        // cover junctions but also authored plazas where dressing is
        // legitimately planted, so the kind split is the meaningful
        // signal — a hit inside a `RoadWithSidewalks` band is hard
        // evidence, a fan hit needs a human look.
        let mut by_ck: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        for h in &hits {
            *by_ck
                .entry((h.channel, format!("{:?}", h.kind)))
                .or_default() += 1;
        }
        if !by_ck.is_empty() {
            let detail: Vec<String> = by_ck
                .iter()
                .map(|((ch, k), n)| format!("{ch}/{k}={n}"))
                .collect();
            println!("  in-road by channel/kind: {}", detail.join(" "));
        }
        // List the most damning first: hard carriageway kinds before
        // the ambiguous `RoadFan`/`Crosswalk` regions, deepest
        // penetration first within a kind.
        hits.sort_by(|a, b| {
            let rank = |k: AttributeType| match k {
                AttributeType::RoadWithSidewalks => 0,
                AttributeType::DividedRoad => 1,
                AttributeType::RoadNoSidewalks => 2,
                AttributeType::Crosswalk => 3,
                _ => 4,
            };
            rank(a.kind)
                .cmp(&rank(b.kind))
                .then(b.depth.total_cmp(&a.depth))
        });
        for h in hits.iter().take(MAX_HITS_SHOWN) {
            println!(
                "    in-road [{ch}] {det} pos=({x:.2},{y:.2},{z:.2}) room {room} {kind:?} dy={dy:+.2} depth={depth:.2}",
                ch = h.channel,
                det = h.detail,
                x = h.pos[0],
                y = h.pos[1],
                z = h.pos[2],
                room = h.room,
                kind = h.kind,
                dy = h.dy,
                depth = h.depth,
            );
        }
        if hits.len() > MAX_HITS_SHOWN {
            println!("    … +{} more in-road stamps", hits.len() - MAX_HITS_SHOWN);
        }
        // Swept-footprint body hits — the prop's street-level mesh on
        // the carriageway, deepest penetration first. These are what
        // an orientation defect looks like to the driver.
        let mut fp_by_ck: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        for h in &fp_hits {
            *fp_by_ck
                .entry((h.channel, format!("{:?}", h.kind)))
                .or_default() += 1;
        }
        if !fp_by_ck.is_empty() {
            let detail: Vec<String> = fp_by_ck
                .iter()
                .map(|((ch, k), n)| format!("{ch}/{k}={n}"))
                .collect();
            println!("  body-in-road by channel/kind: {}", detail.join(" "));
        }
        // …and by channel×prop — the hit list's last token is the
        // PKG — so a systematic defect shows as a family, not a
        // position.
        let mut fp_by_prop: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        for h in &fp_hits {
            if let Some(name) = h.detail.rsplit(' ').next() {
                *fp_by_prop.entry((h.channel, name.to_string())).or_default() += 1;
            }
        }
        if !fp_by_prop.is_empty() {
            let mut by_prop: Vec<(&(&'static str, String), &usize)> = fp_by_prop.iter().collect();
            by_prop.sort_by(|a, b| b.1.cmp(a.1));
            let detail: Vec<String> = by_prop
                .iter()
                .take(16)
                .map(|((ch, k), n)| format!("{ch}/{k}={n}"))
                .collect();
            println!("  body-in-road by prop (top): {}", detail.join(" "));
        }
        fp_hits.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        if !fp_hits.is_empty() {
            println!("  body-in-road footprint hits ({})", fp_hits.len());
        }
        for h in fp_hits.iter().take(MAX_HITS_SHOWN) {
            println!(
                "    body-in-road [{ch}] {det} pos=({x:.2},{y:.2},{z:.2}) room {room} {kind:?} dy={dy:+.2} depth={depth:.2}",
                ch = h.channel,
                det = h.detail,
                x = h.pos[0],
                y = h.pos[1],
                z = h.pos[2],
                room = h.room,
                kind = h.kind,
                dy = h.dy,
                depth = h.depth,
            );
        }
        if fp_hits.len() > MAX_HITS_SHOWN {
            println!(
                "    … +{} more footprint hits",
                fp_hits.len() - MAX_HITS_SHOWN
            );
        }
    }

    println!(
        "  {} in-road stamps, {} body-in-road footprint hits across audited channels; {} issue(s), {} failure(s)",
        any_in_road,
        any_body,
        issues.len(),
        failures.len(),
    );
    for i in &issues {
        println!("    issue: {i}");
    }
    if !any_stamps && !cities.is_empty() {
        failures.push("no placements were measured in any channel".to_string());
    }
    if strict && (!failures.is_empty() || !issues.is_empty()) {
        return Err(format!(
            "strict placement audit: {} failures, {} issues",
            failures.len(),
            issues.len()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 m × 4 m flat road strip along +X at height `y`, built the
    /// way `carriageways` builds a two-section strip.
    fn flat_region(y: f32) -> Region {
        Region {
            room: 1,
            kind: AttributeType::RoadWithSidewalks,
            min: [0.0, -2.0],
            max: [10.0, 2.0],
            ring: vec![
                [0.0, y, -2.0],
                [10.0, y, -2.0],
                [10.0, y, 2.0],
                [0.0, y, 2.0],
            ],
            tris: vec![
                [[0.0, y, -2.0], [10.0, y, -2.0], [10.0, y, 2.0]],
                [[0.0, y, -2.0], [10.0, y, 2.0], [0.0, y, 2.0]],
            ],
        }
    }

    #[test]
    fn stamp_inside_at_surface_height_is_on_road() {
        let r = [flat_region(0.0)];
        let (region, dy) = on_road([5.0, 0.0, 0.0], &r).expect("on the strip");
        assert_eq!(region.room, 1);
        assert_eq!(dy, 0.0);
    }

    #[test]
    fn depth_measures_distance_from_the_boundary() {
        let r = [flat_region(0.0)];
        // Kerb-edge stamp: on the strip edge, depth ~0.
        let (_, _) = on_road([5.0, 0.0, 1.95], &r).expect("inside");
        assert!(r[0].depth([5.0, 0.0, 1.95]) < 0.1);
        // Mid-strip: depth = half the 4 m width.
        assert!((r[0].depth([5.0, 0.0, 0.0]) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn stamps_outside_the_footprint_are_clear() {
        let r = [flat_region(0.0)];
        // Beside the strip (sidewalk side).
        assert!(on_road([5.0, 0.15, 3.0], &r).is_none());
        // Past the end.
        assert!(on_road([11.0, 0.0, 0.0], &r).is_none());
    }

    #[test]
    fn the_height_band_bounds_the_verdict() {
        let r = [flat_region(0.0)];
        // Kerb-height props above the road still count; a lamp on a
        // tall pole's painted base is still its base.
        assert!(on_road([5.0, BAND_ABOVE, 0.0], &r).is_some());
        assert!(on_road([5.0, -BAND_BELOW, 0.0], &r).is_some());
        // Outside the band — overhead or sunk — is not on the road.
        assert!(on_road([5.0, BAND_ABOVE + 0.01, 0.0], &r).is_none());
        assert!(on_road([5.0, -BAND_BELOW - 0.01, 0.0], &r).is_none());
    }

    #[test]
    fn sloped_surface_reports_the_local_height() {
        // A strip sloping from y=0 at x=0 to y=2 at x=10.
        let r = [Region {
            room: 2,
            kind: AttributeType::RoadNoSidewalks,
            min: [0.0, -2.0],
            max: [10.0, 2.0],
            ring: vec![
                [0.0, 0.0, -2.0],
                [10.0, 2.0, -2.0],
                [10.0, 2.0, 2.0],
                [0.0, 0.0, 2.0],
            ],
            tris: vec![
                [[0.0, 0.0, -2.0], [10.0, 2.0, -2.0], [10.0, 2.0, 2.0]],
                [[0.0, 0.0, -2.0], [10.0, 2.0, 2.0], [0.0, 0.0, 2.0]],
            ],
        }];
        // At x=5 the surface is at y=1: a stamp planted on it counts.
        let (_, dy) = on_road([5.0, 1.0, 0.0], &r).expect("on the slope");
        assert!(dy.abs() < 1e-4, "dy {dy}");
        // At x=5 a stamp at y=0 is a metre under the surface — still
        // inside the band, still in-road.
        let (_, dy) = on_road([5.0, 0.0, 0.0], &r).expect("under the slope");
        assert!((dy - -1.0).abs() < 1e-4, "dy {dy}");
    }

    #[test]
    fn degenerate_triangles_never_report() {
        let r = [Region {
            room: 3,
            kind: AttributeType::RoadFan,
            min: [0.0, 0.0],
            max: [1.0, 1.0],
            ring: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            tris: vec![[[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 1.0]]],
        }];
        // XZ-degenerate triangle has no footprint; the second edge's
        // bbox still lets the point reach the tri test.
        assert!(on_road([0.4, 0.0, 0.4], &r).is_none());
    }

    #[test]
    fn measure_counts_stamps_and_hits() {
        let r = [flat_region(0.0)];
        let mut ch = Channel::default();
        let mut hits = Vec::new();
        measure(
            "inst",
            [
                ([5.0, 0.0, 0.0], "road".to_string()),
                ([5.0, 0.15, 4.0], "sidewalk".to_string()),
                ([f32::NAN, 0.0, 0.0], "corrupt".to_string()),
            ]
            .into_iter(),
            &r,
            &mut ch,
            &mut hits,
        );
        assert_eq!(ch.stamps, 2); // the NaN position is an issue, not a stamp
        assert_eq!(ch.in_road, 1);
        assert_eq!(ch.issues.len(), 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].channel, "inst");
    }

    #[test]
    fn footprint_catches_the_rotation_the_origin_misses() {
        let r = [flat_region(0.0)];
        // A bench-shaped prop: 0.6 m deep along local ±X, 2 m along
        // ±Z, waist-high — plus one vertex up a 5 m pole.
        let verts: Vec<[f32; 3]> = vec![
            [-0.3, 0.0, -1.0],
            [0.3, 0.0, -1.0],
            [-0.3, 0.6, -1.0],
            [0.3, 0.6, -1.0],
            [-0.3, 0.0, 1.0],
            [0.3, 0.0, 1.0],
            [-0.3, 0.6, 1.0],
            [0.3, 0.6, 1.0],
            [0.0, 5.0, -1.6],
        ];
        let mut ch = Channel::default();
        let mut hits = Vec::new();
        // Stamped just off the road's z = 2 edge. With the prop's long
        // axis yawed across the kerb (local +Z → world +Z) the body
        // reaches half a metre into the carriageway — the quarter-turn
        // defect the operator reported — while the pole-top vertex
        // overhangs in the overhead band.
        measure_footprint(
            FootprintStamp {
                channel: "prop-rule",
                detail: "bench".to_string(),
                position: [5.0, 0.0, 2.5],
                axes: yawed_basis([1.0, 0.0, 0.0]),
                verts: &verts,
            },
            &r,
            &mut ch,
            &mut hits,
        );
        assert_eq!(ch.swept, 1);
        assert_eq!(ch.body_in_road, 1);
        assert_eq!(ch.overhang, 1);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].depth - 0.5).abs() < 1e-4, "{}", hits[0].depth);
        // Turned the authored way — long axis along the kerb — nothing
        // but the pole's overhead vertex touches the region XZ.
        measure_footprint(
            FootprintStamp {
                channel: "prop-rule",
                detail: "bench".to_string(),
                position: [5.0, 0.0, 2.5],
                axes: yawed_basis([0.0, 0.0, 1.0]),
                verts: &verts,
            },
            &r,
            &mut ch,
            &mut hits,
        );
        assert_eq!(ch.swept, 2);
        assert_eq!(ch.body_in_road, 1, "no new body hit");
        assert_eq!(
            ch.overhang, 1,
            "pole top lands at x≈6.6, z=2.5 — off the road"
        );
        assert_eq!(hits.len(), 1);
    }
}
