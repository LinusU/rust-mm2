//! The documented opponent indicator (F22-A.2; HUD-3/CTL-1 `I
//! opponent indicator`): a session-owned pool of markers hovering over
//! each race opponent, toggled by `I` while driving.
//!
//! The original's indicator presentation is unrecovered — the
//! documentation records only the toggle — so the instrument's look is
//! a designed reading (DSN-51): the authored `hudmap_tri` marker mesh,
//! stood upright over the opponent's collider roof, apex down and
//! yaw-facing the active world camera, painted with the same authored
//! `TRI_PAINT_OPPONENTS` slots the minimap's opponent tris use. Both
//! instruments bind their pools in entity order, so an opponent's
//! arrow and its map tri share a colour. What is original: the `I`
//! toggle and the instrument's scope — non-local `Player`
//! participants (AI opponents today, remote drivers when F25 exists),
//! never ambient traffic, never the local car.
//!
//! - [`spawn_opponent_indicators`] runs in `load_session_world` for
//!   event sessions: it sizes the pool to the authored roster and
//!   loads `geometry/hudmap_tri.pkg` through the VFS like every
//!   authored resource — a missing or unparseable package records
//!   `absent` on the [`OppIndReport`], never a substitute mesh.
//!   Every marker is `SessionEntity`-stamped so teardown owns it.
//! - [`indicator_input`] owns the `I` toggle in `Playing`/`Countdown`
//!   so overlays keep the key — the same contract `mirror_input`
//!   holds for BACKSPACE.
//! - [`drive_opponent_indicators`] rebinds the pool every frame to the
//!   live non-local participants in entity order: a despawned or
//!   vehicle-less participant frees its slot the same update, so the
//!   indicator can never hover over a stale or invalid one (AC03).
//!   Runs ungated by `capturing` like `drive_mirror` — a
//!   `--frames`/`--screenshot` run must render the markers with live
//!   input frozen.

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{Player, PlayerControl, Session, SessionEntity, SessionPhase};
use mm2_vehicle::Vehicle;

use crate::city::MaterialCache;

/// Local rotation standing the flat authored `hudmap_tri` up as a
/// down-pointing arrow: the tri's apex is authored at local −Z, and
/// `from_rotation_x(-π/2)` carries it to −Y (down) with the flat face
/// vertical. `drive_opponent_indicators` yaws this upright marker
/// toward the camera.
const APEX_DOWN: Quat = Quat::from_xyzw(
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f32::consts::FRAC_1_SQRT_2,
);
/// Target world extent of the arrow (metres) — designed; the authored
/// ~16×28 m tri is scaled down the way the minimap's `IconScale`
/// rescales it, just at an in-world size instead.
const INDICATOR_EXTENT: f32 = 1.5;
/// Gap between the opponent's collider roof and the arrow (metres) —
/// designed; per-car from the authored collider/`chassis_size` so tall
/// vehicles (vpbus, vpsemi) clear the marker.
const INDICATOR_CLEARANCE: f32 = 0.6;

/// Whether opponent indicators are switched on (HUD-3/CTL-1: `I`
/// toggles). Session-agnostic like [`crate::camera::RearView`]: a
/// restart respawns the pool and the drive system re-applies the
/// driver's choice. Defaults on — a designed default: the original's
/// start state is unrecovered, and the indicator is the instrument the
/// rostered field is meant to be visible through.
#[derive(Resource, Debug, Clone, Copy)]
pub struct OpponentIndicators(pub bool);

impl Default for OpponentIndicators {
    fn default() -> Self {
        Self(true)
    }
}

/// Marker on one session-owned indicator pool entity.
#[derive(Component)]
pub struct OppIndicator;

/// Session-scoped load report for the indicator pool — the `ind=`
/// record field's source. Inserted for every event session (pool
/// bound or not — `absent` says why not); cruise and dev-world
/// sessions get no report, so their records stay bit-identical.
#[derive(Resource, Default)]
pub struct OppIndReport {
    /// Pool slots spawned — the authored roster size.
    pub markers: usize,
    /// Live opponents bound on the last
    /// [`drive_opponent_indicators`] pass — recorded even while the
    /// toggle hides the markers, so the field shows demand vs pool.
    pub bound: usize,
    /// Why nothing bound: `missing-pkg`, `unparseable-pkg`,
    /// `empty-pkg`.
    pub absent: Option<&'static str>,
}

impl OppIndReport {
    /// The `ind=` record field body: `<on|off>/<pool>m/<bound>b`, or
    /// `absent:<why>` when the marker package never bound.
    pub fn smoke_detail(&self, indicators: &OpponentIndicators) -> String {
        match self.absent {
            Some(why) => format!("absent:{why}"),
            None => format!(
                "{}/{}m/{}b",
                if indicators.0 { "on" } else { "off" },
                self.markers,
                self.bound
            ),
        }
    }
}

/// Spawn the session's indicator pool sized to `opponent_count`
/// (the authored roster — slots whose vehicle failed to load simply
/// stay hidden). Marker geometry and paint ride the authored
/// `hudmap_tri.pkg` through the VFS; its authored extent is measured
/// and scaled to [`INDICATOR_EXTENT`].
pub fn spawn_opponent_indicators(
    commands: &mut Commands,
    vfs: &Vfs,
    opponent_count: usize,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> OppIndReport {
    let mut report = OppIndReport::default();
    let tri_pkg = match crate::hudmap::read_pkg(vfs, "geometry/hudmap_tri.pkg") {
        Some(p) => p,
        None => {
            report.absent = Some(if vfs.resolve("geometry/hudmap_tri.pkg").is_some() {
                "unparseable-pkg"
            } else {
                "missing-pkg"
            });
            return report;
        }
    };
    let mut mats = MaterialCache::new(vfs, images, materials);
    let extent = crate::hudmap::authored_extent(&tri_pkg).max(1.0);
    let mut missing_prims = 0usize;
    let Some((mesh, _)) =
        crate::city::pkg_paint_parts(&tri_pkg, &mut mats, meshes, &mut missing_prims, 0)
            .into_iter()
            .next()
    else {
        report.absent = Some("empty-pkg");
        return report;
    };
    let scale = INDICATOR_EXTENT / extent;
    for slot in 0..opponent_count {
        // The same authored opponent palette the minimap cycles — the
        // entity-order binding on both instruments means an opponent's
        // arrow matches its map tri.
        let paint =
            crate::hudmap::TRI_PAINT_OPPONENTS[slot % crate::hudmap::TRI_PAINT_OPPONENTS.len()];
        let Some(mat) = crate::hudmap::paint_material(&tri_pkg, paint, &mut mats) else {
            continue;
        };
        commands.spawn((
            owner,
            OppIndicator,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_scale(Vec3::splat(scale)),
            Visibility::Hidden,
        ));
        report.markers += 1;
    }
    report
}

/// `I` toggles the opponent indicators (HUD-3/CTL-1) in the live
/// phases. `Paused`/`Results`/menu contexts keep the key for their own
/// owners — the toggle would be invisible there either way — the same
/// contract `mirror_input` holds for BACKSPACE.
pub fn indicator_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    mut indicators: ResMut<OpponentIndicators>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Playing | SessionPhase::Countdown
    ) {
        return;
    }
    if keys.just_pressed(KeyCode::KeyI) {
        indicators.0 = !indicators.0;
    }
}

/// Rebind the pool to the live non-local participants. Each marker
/// rides its opponent's collider roof (the authored
/// `collider_points`/`chassis_size` bound plus a designed gap), stands
/// upright apex-down, and yaw-faces the active world camera — a
/// billboard in heading only, so the view's pitch never tips the
/// arrow. The camera pick goes through [`crate::hudmap::WorldCamera3d`]
/// like every other "the world view" consumer: the map camera and the
/// mirror strip are never the facing source.
pub fn drive_opponent_indicators(
    indicators: Res<OpponentIndicators>,
    hud: Res<crate::hud::HudVisible>,
    mut report: Option<ResMut<OppIndReport>>,
    participants: Query<(Entity, &Player, &GlobalTransform, &Vehicle)>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
    mut markers: Query<(&mut Transform, &mut Visibility), With<OppIndicator>>,
) {
    let Some(report) = report.as_deref_mut() else {
        return;
    };
    // Entity-order binding — the same ordering the minimap's opponent
    // tri pool uses, so slot paint agrees between the instruments.
    let mut opponents: Vec<(Entity, Vec3, f32)> = participants
        .iter()
        .filter(|(_, p, ..)| p.control != PlayerControl::Local)
        .map(|(e, _, gt, v)| {
            let roof = v
                .config
                .collider_points
                .as_ref()
                .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::max))
                .unwrap_or(v.config.chassis_size[1] * 0.5);
            (e, gt.translation(), roof + INDICATOR_CLEARANCE)
        })
        .collect();
    opponents.sort_by_key(|(e, ..)| *e);
    report.bound = opponents.len().min(report.markers);

    let cam_pos = cameras
        .iter()
        .find(|(c, _)| c.is_active)
        .map(|(_, xf)| xf.translation());
    let mut iter = opponents.iter();
    for (mut xf, mut vis) in &mut markers {
        match iter.next() {
            // The `H` gate suppresses the markers with the rest of the
            // layer (F22-A.3 — the original draws its indicators from
            // inside `mmHUD`'s `mmHudMap`); `bound` above still counts
            // live demand while either toggle is off.
            Some((_, pos, lift)) if indicators.0 && hud.0 => {
                xf.translation = *pos + Vec3::Y * *lift;
                // Yaw toward the view on the ground plane only; a
                // degenerate same-point camera keeps the last heading.
                if let Some(c) = cam_pos {
                    let d = c - xf.translation;
                    let ground = (d.x * d.x + d.z * d.z).sqrt();
                    if ground > 0.01 {
                        xf.rotation = Quat::from_rotation_y(d.x.atan2(d.z)) * APEX_DOWN;
                    }
                }
                *vis = Visibility::Visible;
            }
            _ => *vis = Visibility::Hidden,
        }
    }
}
