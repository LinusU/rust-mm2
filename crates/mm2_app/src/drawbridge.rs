//! Drawbridges (the retail `gizBridgeMgr`): London's Tower Bridge,
//! Waterloo and dock-bridge leaves and SF's Chinatown gate.
//!
//! The PSDL rooms under these decks suppress their own road (a null
//! texture reference drops render *and* collision), so without the
//! leaves there is a hole where the race route crosses the Thames.
//! Each session loads one bridge file — the event's own
//! `race/<city>/<city>_bridge_<stem>.pathset` when it ships one,
//! `race/<city>/<city>_bridge.pathset` otherwise (cruise always takes
//! the default) — and every path becomes one or two kinematic leaves
//! hinged at their authored points, driven by
//! [`mm2_game::drawbridge::LeafMotion`]. The behaviour and every
//! constant are recovered from the retail executable; see
//! `docs/research/drawbridge.md`.

use std::collections::BTreeSet;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::pathset::Pathset;
use mm2_game::drawbridge::{
    DrawbridgeMode, LEAF_DROP, LeafMotion, LeafPhase, PROXIMITY_RADIUS, RAISE_RATE, leaf_hinges,
};
use mm2_game::{
    CityEntity, Player, Session, SessionEntity, SessionPhase, object_pathset_candidates,
};
use tracing::{info, warn};

use crate::banger::BangerDefs;
use crate::city::{MIRROR_Z, MovableModels, v3};
use crate::layers::GameLayer;
use crate::object_sound::ObjectSound;

use mm2_game::movers::DRAWBRIDGE_LEAF_MODEL as DEFAULT_LEAF_MODEL;

/// One drawbridge leaf: a kinematic body whose origin is the hinge.
#[derive(Component, Debug, Clone)]
pub struct DrawbridgeLeaf {
    /// The leaf's animation state.
    pub motion: LeafMotion,
    /// The other leaf of the same path, which a proximity trigger
    /// opens too.
    pub partner: Option<Entity>,
    /// World-space hinge (the body's fixed position).
    pub hinge: Vec3,
    /// World rotation of the closed leaf.
    pub base: Quat,
}

impl DrawbridgeLeaf {
    /// The leaf's world rotation at `angle` radians of lift. The
    /// original rotates about the leaf's local X; the Z mirror (when
    /// enabled) reverses that rotation's sense.
    pub fn rotation_at(&self, angle: f32) -> Quat {
        self.base * Quat::from_rotation_x(mirror_sign() * angle)
    }
}

fn mirror_sign() -> f32 {
    if MIRROR_Z { -1.0 } else { 1.0 }
}

/// What the session's bridge file produced.
#[derive(Resource, Debug, Default, Clone)]
pub struct DrawbridgeReport {
    /// The file the leaves came from; `None` when the city ships none.
    pub file: Option<String>,
    /// Candidate files that resolved but failed to parse (fallen back
    /// from).
    pub failed_files: Vec<String>,
    /// Leaves spawned, by mode.
    pub timed: usize,
    pub open: usize,
    pub inactive: usize,
    pub proximity: usize,
    /// Leaf models that failed to load (neither the named PKG nor the
    /// default resolved) — those leaves are missing.
    pub missing_models: Vec<String>,
    /// Texture stems the leaf models could not resolve.
    pub missing_textures: BTreeSet<String>,
}

impl DrawbridgeReport {
    /// Total leaves spawned.
    pub fn leaves(&self) -> usize {
        self.timed + self.open + self.inactive + self.proximity
    }
}

/// Load the session's bridge file and spawn its leaves, session-owned
/// so teardown removes them. A file that fails to parse falls back to
/// the next candidate (retail's truncated `london_bridge_blitz10` is
/// unreachable — London's blitz table stops at `blitz9` — so this
/// guards mod content).
#[allow(clippy::too_many_arguments)] // Bevy asset stores have to be threaded separately
pub fn spawn_drawbridges(
    commands: &mut Commands,
    vfs: &Vfs,
    city: &str,
    event_stem: Option<&str>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    owner: SessionEntity,
) -> DrawbridgeReport {
    let mut report = DrawbridgeReport::default();
    let mut chosen = None;
    for logical in object_pathset_candidates(city, "bridge", event_stem) {
        let Ok((bytes, resolved)) = vfs.read_path(&logical) else {
            continue;
        };
        match Pathset::parse(&bytes) {
            Ok(pathset) => {
                chosen = Some((resolved.logical.clone(), pathset));
                break;
            }
            Err(e) => {
                warn!(path = %resolved.logical, error = %e, "bridge pathset parse failed; falling back");
                report.failed_files.push(resolved.logical.clone());
            }
        }
    }
    let Some((file, pathset)) = chosen else {
        return report;
    };
    report.file = Some(file);

    let mut models = MovableModels::new(vfs, meshes, images, materials);
    let mut bangers = BangerDefs::new(vfs);
    // Every leaf carries the `drawbridge` table: motor loop and bell,
    // both off until the leaf moves.
    let sound = crate::object_sound::load_object_audio(vfs, "drawbridge");
    for (pi, path) in pathset.paths.iter().enumerate() {
        let points: Vec<[f32; 3]> = path.points.iter().map(|p| p.position).collect();
        if points.iter().flatten().any(|c| !c.is_finite()) {
            warn!(path = %path.name, "bridge path has non-finite points; skipped");
            continue;
        }
        let hinges = leaf_hinges(&points);
        let Some(&(from, to)) = hinges.first() else {
            continue;
        };
        let mode = DrawbridgeMode::from_path_name(&path.name);
        let named = path.asset_name().unwrap_or(DEFAULT_LEAF_MODEL);
        // The leaf is drawn shifted half its authored length along its
        // local axis, so the hinge sits at one end: the original reads
        // `Size.z` off the leaf's banger record. A record-less (mod)
        // leaf falls back to the geometry the path implies.
        let span = Vec3::from(from).distance(Vec3::from(to));
        let reach = if hinges.len() > 1 { span / 2.0 } else { span };
        let mut load = |name: &str| {
            let half = bangers
                .get(name)
                .map(|d| d.size[2] / 2.0)
                .filter(|h| h.is_finite() && *h > 0.0)
                .unwrap_or(reach / 2.0);
            let offset = Vec3::new(0.0, 0.0, -mirror_sign() * half);
            models.load(name, offset)
        };
        let Some(model) = load(named).or_else(|| load(DEFAULT_LEAF_MODEL)) else {
            warn!(path = %path.name, "bridge leaf model unresolved; leaf missing");
            report.missing_models.push(named.to_string());
            continue;
        };
        let leaves: Vec<(Vec3, Quat)> = hinges
            .into_iter()
            .filter_map(|(from, to)| {
                let base = leaf_base(from, to);
                if base.is_none() {
                    warn!(path = %path.name, "bridge leaf has coincident hinge points; skipped");
                }
                Some((v3(from) - Vec3::Y * LEAF_DROP, base?))
            })
            .collect();
        // Partners link both ways, so both ids exist before either
        // leaf is filled in.
        let ids: Vec<Entity> = leaves.iter().map(|_| commands.spawn_empty().id()).collect();
        for (li, &(hinge, base)) in leaves.iter().enumerate() {
            let partner = match ids[..] {
                [a, b] => Some(if li == 0 { b } else { a }),
                _ => None,
            };
            let leaf = DrawbridgeLeaf {
                motion: LeafMotion::new(mode),
                partner,
                hinge,
                base,
            };
            let transform = Transform::from_translation(hinge)
                .with_rotation(leaf.rotation_at(leaf.motion.angle));
            let root = ids[li];
            commands.entity(root).insert((
                CityEntity,
                owner,
                Name::new(format!("drawbridge-{named}-{pi}-{li}")),
                transform,
                Visibility::default(),
                RigidBody::Kinematic,
                // The body's origin is the hinge, and kinematic
                // rotation integrates about the centre of mass.
                CenterOfMass(Vec3::ZERO),
                NoAutoCenterOfMass,
                leaf,
            ));
            if let Some(collider) = &model.collider {
                // Kinematic scenery never touches the static city
                // geometry — see `layers` for why that pair is skipped.
                commands
                    .entity(root)
                    .insert((collider.clone(), GameLayer::scenery()));
            }
            if let Some(spec) = &sound {
                commands
                    .entity(root)
                    .insert(ObjectSound::new(spec.clone(), (pi * 2 + li) as u32 + 1));
            }
            for (mesh, material) in &model.parts {
                let part = commands
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::IDENTITY,
                    ))
                    .id();
                commands.entity(root).add_child(part);
            }
            match mode {
                DrawbridgeMode::Timed => report.timed += 1,
                DrawbridgeMode::Open => report.open += 1,
                DrawbridgeMode::Inactive => report.inactive += 1,
                DrawbridgeMode::Proximity => report.proximity += 1,
            }
        }
    }
    report.missing_textures = models.finish(commands, owner);
    info!(
        file = report.file.as_deref().unwrap_or("-"),
        leaves = report.leaves(),
        timed = report.timed,
        open = report.open,
        inactive = report.inactive,
        proximity = report.proximity,
        missing_models = report.missing_models.len(),
        "drawbridges spawned"
    );
    report
}

/// The closed leaf's world rotation: hinged at `from`, its free end
/// toward `to`. The original builds the frame in authored space as
/// `z = normalize(from - to)`, `x = up × z`, `y = z × x`, and the leaf
/// extends along its local −Z; the Z mirror (when enabled) maps that
/// onto local +Z. `None` for a vertical or zero-length span.
fn leaf_base(from: [f32; 3], to: [f32; 3]) -> Option<Quat> {
    let z = (v3(from) - v3(to)) * mirror_sign();
    let z = z.try_normalize()?;
    let x = Vec3::Y.cross(z).try_normalize()?;
    let y = z.cross(x);
    Some(Quat::from_mat3(&Mat3::from_cols(x, y, z)))
}

/// What [`drive_drawbridges`] reads and writes on each leaf.
type LeafQuery<'a> = (
    Entity,
    &'a mut DrawbridgeLeaf,
    &'a mut Position,
    &'a mut Rotation,
    &'a mut LinearVelocity,
    &'a mut AngularVelocity,
    Option<&'a mut ObjectSound>,
);

/// Advance every leaf one fixed step and pose its body:
/// proximity leaves open when any participant comes within
/// [`PROXIMITY_RADIUS`] of the hinge (and open their partner), timed
/// leaves run their cycle. The pose is written for the state just
/// reached, and the angular velocity carries the body to the next
/// step's state, so a car on a moving leaf rides its surface rather
/// than being shoved by a teleport. Runs on every peer: the cycle is
/// deterministic from session start.
pub fn drive_drawbridges(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    mut leaves: Query<LeafQuery>,
    cars: Query<&Position, (With<Player>, Without<DrawbridgeLeaf>)>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    ) {
        return;
    }
    let dt = time.delta_secs();
    let radius2 = PROXIMITY_RADIUS * PROXIMITY_RADIUS;
    let mut triggered = Vec::new();
    for (entity, leaf, ..) in &leaves {
        if leaf.motion.mode == DrawbridgeMode::Proximity
            && leaf.motion.phase == LeafPhase::Resting
            && cars
                .iter()
                .any(|p| p.0.distance_squared(leaf.hinge) < radius2)
        {
            triggered.push(entity);
            triggered.extend(leaf.partner);
        }
    }
    for entity in triggered {
        if let Ok((_, mut leaf, ..)) = leaves.get_mut(entity) {
            leaf.motion.trigger();
        }
    }
    for (_, mut leaf, mut pos, mut rot, mut lin, mut ang, sound) in &mut leaves {
        leaf.motion.step(dt);
        // Motor and bell sound exactly while the leaf moves — the
        // original switches both rows on as a leaf starts opening or
        // closing and off as it comes to rest.
        if let Some(mut sound) = sound {
            let moving = matches!(leaf.motion.phase, LeafPhase::Opening | LeafPhase::Closing);
            sound.state.set_active(None, moving);
        }
        let rate = match leaf.motion.phase {
            LeafPhase::Opening => RAISE_RATE,
            LeafPhase::Closing => -RAISE_RATE,
            LeafPhase::Resting | LeafPhase::Raised => 0.0,
        };
        pos.0 = leaf.hinge;
        rot.0 = leaf.rotation_at(leaf.motion.angle);
        lin.0 = Vec3::ZERO;
        ang.0 = leaf.base * Vec3::X * (mirror_sign() * rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_leaf_reaches_toward_its_partner() {
        // A 30 m leaf hinged at the origin facing +X: its content sits
        // half a length along local −Z (unmirrored), so the far end of
        // the deck — local (0, 0, −30) — must land on the target side.
        let base = leaf_base([0.0, 0.0, 0.0], [60.0, 0.0, 0.0]).unwrap();
        let leaf = DrawbridgeLeaf {
            motion: LeafMotion::new(DrawbridgeMode::Inactive),
            partner: None,
            hinge: Vec3::ZERO,
            base,
        };
        let tip_local = Vec3::new(0.0, 0.0, -30.0 * mirror_sign());
        let closed = leaf.rotation_at(0.0) * tip_local;
        assert!(
            (closed - Vec3::new(30.0, 0.0, 0.0)).length() < 1e-4,
            "{closed}"
        );
        // Raising lifts the tip, it never digs it under the hinge.
        let raised = leaf.rotation_at(0.4) * tip_local;
        assert!(raised.y > 10.0, "{raised}");
        assert!(raised.x > 0.0);
    }

    #[test]
    fn angular_velocity_matches_the_pose_change() {
        let base = leaf_base([5.0, 1.0, -3.0], [-20.0, 1.0, 40.0]).unwrap();
        let leaf = DrawbridgeLeaf {
            motion: LeafMotion::new(DrawbridgeMode::Timed),
            partner: None,
            hinge: Vec3::ZERO,
            base,
        };
        let dt = 0.1;
        let omega = leaf.base * Vec3::X * (mirror_sign() * RAISE_RATE);
        let integrated = Quat::from_scaled_axis(omega * dt) * leaf.rotation_at(0.2);
        let expected = leaf.rotation_at(0.2 + RAISE_RATE * dt);
        // `angle_between` is `2·acos(dot)` in f32, which cannot resolve
        // angles below ~7e-4 rad: the test passed only where the dot
        // happened to round to exactly 1. The relative rotation's vector
        // part, sin(θ/2), stays precise for tiny angles.
        let error = (expected.inverse() * integrated).xyz().length();
        assert!(error < 1e-5, "{error}");
    }
}
