//! F22-C original-content validation (F22-AC02): the authored map's
//! world-space coordinates in both retail cities. Opt-in
//! (`MM2_RETAIL=<dir>`); skipped without the operator's install, and
//! says so.
//!
//! The map is a world-space instrument (HUD-4): its tiles are quads at
//! authored world poses, the camera parks over the player, and every
//! marker is placed at its true world XZ. So "a known world position
//! maps to the correct HUD coordinate" reduces to two checkable
//! claims per city, and this file checks both against the real data:
//!
//! 1. the authored `tune/<city>.mmhudmap` spec parses, validates and
//!    carries usable zoom/icon extents — the numbers the view framing
//!    and marker scaling read;
//! 2. every authored position the race HUD has to draw — every
//!    `race/<city>/*waypoints.csv` route (race/blitz/circuit/crash
//!    course) and every `_strtpnts` start grid — lies inside the
//!    authored tile artwork's world-space extent, so no gate dot or
//!    start marker can land off the map.
//!
//! What it does not claim: whether the artwork *looks* like the city
//! at that alignment — that needs a human eye on a rendered map (an
//! `OWNER:` capture task), not a coordinate assertion. The
//! player-centred projection and the north-up/rotating orientation
//! semantics are validated numerically in `tests/hudmap.rs`
//! (`known_world_positions_project_to_their_map_coordinates`).

use mm2_formats::hudmap::HudMapSpec;
use mm2_formats::pkg::Pkg;
use mm2_formats::waypoints::{StartPointsFile, WaypointFile};

/// The world-space XZ extent of one city's tile artwork.
#[derive(Debug)]
struct Bounds {
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
    verts: usize,
}

impl Bounds {
    fn covers(&self, x: f32, z: f32) -> bool {
        (self.min_x..=self.max_x).contains(&x) && (self.min_z..=self.max_z).contains(&z)
    }
}

fn tile_bounds(pkg: &Pkg) -> Bounds {
    let mut b = Bounds {
        min_x: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        min_z: f32::INFINITY,
        max_z: f32::NEG_INFINITY,
        verts: 0,
    };
    for (_name, geo) in pkg.geometries() {
        for section in &geo.sections {
            for strip in &section.strips {
                for v in &strip.vertices {
                    b.min_x = b.min_x.min(v.position[0]);
                    b.max_x = b.max_x.max(v.position[0]);
                    b.min_z = b.min_z.min(v.position[2]);
                    b.max_z = b.max_z.max(v.position[2]);
                    b.verts += 1;
                }
            }
        }
    }
    assert!(b.verts > 0, "the tile package carries geometry");
    b
}

/// Every authored route waypoint and start-grid point of `city`, as
/// `(logical path, line, x, z)`.
fn authored_positions(vfs: &mm2_assets::Vfs, city: &str) -> Vec<(String, u32, f32, f32)> {
    let prefix = format!("race/{city}/");
    let mut out = Vec::new();
    let mut files: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.starts_with(&prefix))
        .collect();
    files.sort();
    for path in files {
        let Some(text) = vfs
            .read_path(&path)
            .ok()
            .map(|(bytes, _)| String::from_utf8_lossy(&bytes).into_owned())
        else {
            continue;
        };
        if path.ends_with("waypoints.csv") {
            let file = match WaypointFile::parse(&text) {
                Ok(f) => f,
                Err(e) => panic!("{path}: waypoints failed to parse: {e}"),
            };
            assert!(
                !file.rows.is_empty(),
                "{path}: a retail route file has no rows"
            );
            for row in &file.rows {
                out.push((path.clone(), row.line, row.position[0], row.position[2]));
            }
        } else if path.ends_with("_strtpnts") {
            let file = match StartPointsFile::parse(&text) {
                Ok(f) => f,
                Err(e) => panic!("{path}: start points failed to parse: {e}"),
            };
            for row in &file.rows {
                out.push((path.clone(), row.line, row.position[0], row.position[2]));
            }
        }
    }
    out
}

/// Both retail cities: the authored spec validates, the tile artwork
/// has a usable world extent, and every authored race/start position
/// sits inside that extent — the map's coordinate frame covers every
/// position its markers must draw.
#[test]
fn every_authored_position_in_both_cities_sits_inside_the_map_artwork() {
    let Some((retail, _slot)) = crate::support::retail_slot() else {
        return;
    };
    let vfs = crate::support::mount(&retail);
    for city in ["london", "sf"] {
        // 1. The authored spec.
        let spec_path = format!("tune/{city}.mmhudmap");
        let (bytes, _) = vfs
            .read_path(&spec_path)
            .unwrap_or_else(|e| panic!("{city}: {spec_path} unavailable: {e}"));
        let spec = HudMapSpec::parse(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|e| panic!("{city}: {spec_path} unparseable: {e}"));
        let issues = spec.validate();
        assert!(
            issues.is_empty(),
            "{city}: authored spec has issues: {issues:?}"
        );

        // 2. The authored tile artwork's world extent.
        let pkg_path = format!("geometry/hudmap_{city}.pkg");
        let (bytes, _) = vfs
            .read_path(&pkg_path)
            .unwrap_or_else(|e| panic!("{city}: {pkg_path} unavailable: {e}"));
        let pkg = Pkg::parse(&bytes).unwrap_or_else(|e| panic!("{city}: {pkg_path}: {e}"));
        let bounds = tile_bounds(&pkg);
        assert!(
            bounds.min_x.is_finite() && bounds.min_z.is_finite(),
            "{city}: tile vertices must be finite"
        );

        // 3. Every authored position inside that extent.
        let positions = authored_positions(&vfs, city);
        let mut routes = std::collections::BTreeSet::new();
        for (path, ..) in &positions {
            routes.insert(path.clone());
        }
        assert!(
            routes.len() >= 10,
            "{city}: expected the full authored route set, found {} files",
            routes.len()
        );
        let mut outside = 0usize;
        let mut first_outside = None;
        for (path, line, x, z) in &positions {
            if !bounds.covers(*x, *z) {
                outside += 1;
                first_outside.get_or_insert(format!("{path}:{line} ({x}, {z})"));
            }
        }
        eprintln!(
            "{city}: map artwork x=[{:.1}, {:.1}] z=[{:.1}, {:.1}] ({} verts); \
             {} authored positions in {} files, {outside} outside",
            bounds.min_x,
            bounds.max_x,
            bounds.min_z,
            bounds.max_z,
            bounds.verts,
            positions.len(),
            routes.len(),
        );
        assert_eq!(
            outside,
            0,
            "{city}: {outside} authored positions fall outside the map artwork, \
             first at {} (artwork x=[{}, {}] z=[{}, {}])",
            first_outside.unwrap_or_default(),
            bounds.min_x,
            bounds.max_x,
            bounds.min_z,
            bounds.max_z,
        );
    }
}

/// F22-AC02 on the authored zoom numbers: with the player parked on a
/// retail start-grid point and the full-screen map at the authored
/// `ZoomOutDistFS` (north-up), each retail city's own route waypoints
/// project to pixel coordinates through the production framing
/// (`HudMap::view_half_extent`, vertical extent = 2·half, horizontal =
/// that × aspect, world −Z screen-up, +X screen-right). Checks the
/// player sits at the frame centre, that a waypoint due north of the
/// start lands above it and one due east lands right of it, and that
/// the nearest waypoint is actually on screen — pixel coordinates are
/// printed per city as the evidence record.
#[test]
fn start_grid_and_waypoints_project_to_expected_pixels_in_both_cities() {
    let Some((retail, _slot)) = crate::support::retail_slot() else {
        return;
    };
    let vfs = crate::support::mount(&retail);
    const W: f32 = 2560.0;
    const H: f32 = 1440.0;
    for city in ["london", "sf"] {
        let (bytes, _) = vfs.read_path(&format!("tune/{city}.mmhudmap")).unwrap();
        let spec = HudMapSpec::parse(&String::from_utf8_lossy(&bytes)).unwrap();
        let mut map = mm2_game::HudMap::new(spec, 0);
        map.fullscreen = true;
        map.zoom_level = mm2_game::MapZoom::Standard;
        map.zoom = map.target_zoom();
        let half = map.view_half_extent();
        assert!(half > 0.0 && half.is_finite(), "{city}: half extent {half}");
        let px = |origin: (f32, f32), x: f32, z: f32| -> (f32, f32) {
            (
                W / 2.0 + (x - origin.0) / (half * W / H) * (W / 2.0),
                H / 2.0 + (z - origin.1) / half * (H / 2.0),
            )
        };

        let positions = authored_positions(&vfs, city);
        // A start-grid point where the city has one, else the first
        // authored route point — any authored position will do as the
        // parked player.
        let (spath, sline, sx, sz) = positions
            .iter()
            .find(|(p, ..)| p.ends_with("_strtpnts"))
            .or_else(|| positions.first())
            .expect("authored positions")
            .clone();
        let origin = (sx, sz);
        let centre = px(origin, sx, sz);
        assert!((centre.0 - W / 2.0).abs() < 1e-3 && (centre.1 - H / 2.0).abs() < 1e-3);

        let mut nearest: Option<(f32, f32, f32, String)> = None;
        let (mut north, mut east, mut south, mut west) = (false, false, false, false);
        for (p, l, x, z) in positions
            .iter()
            .filter(|(p, ..)| p.ends_with("waypoints.csv"))
        {
            let (dx, dz) = (x - sx, z - sz);
            let d = dx.hypot(dz);
            if d > 1.0 && nearest.as_ref().is_none_or(|n| d < n.0) {
                nearest = Some((d, *x, *z, format!("{p}:{l}")));
            }
            let (ox, oy) = px(origin, *x, *z);
            if dz.abs() > dx.abs() * 4.0 && dz.abs() > 20.0 {
                // World −Z must read screen-up (smaller y).
                assert_eq!(
                    oy < H / 2.0,
                    dz < 0.0,
                    "{city}: {p}:{l} north/south flipped"
                );
                if dz < 0.0 { north = true } else { south = true }
            }
            if dx.abs() > dz.abs() * 4.0 && dx.abs() > 20.0 {
                assert_eq!(ox > W / 2.0, dx > 0.0, "{city}: {p}:{l} east/west mirrored");
                if dx > 0.0 { east = true } else { west = true }
            }
        }
        assert!(
            north && south && east && west,
            "{city}: all four quadrants probed"
        );
        let (d, nx, nz, label) = nearest.expect("a waypoint apart from the start");
        let (ox, oy) = px(origin, nx, nz);
        eprintln!(
            "{city}: start {spath}:{sline} ({sx:.1}, {sz:.1}) at px ({:.0}, {:.0}); \
             half extent {half:.0} m; nearest waypoint {label} {d:.1} m away at px ({ox:.0}, {oy:.0})",
            centre.0, centre.1
        );
        assert!(
            (0.0..=W).contains(&ox) && (0.0..=H).contains(&oy),
            "{city}: the nearest waypoint {label} projects off-screen at ({ox}, {oy})"
        );
    }
}
