//! F05-B.8: impact sparks — the `asLineSparks` leg of F05-B's
//! remaining work.
//!
//! MM2Hook's recovered `vehCarDamage` owns a per-vehicle
//! `asLineSparks` renderer fired from the car's impact callback —
//! `RadialBlast(count, position, velocity)` is the emission
//! primitive, a radial burst of streaks at the impact point.
//! `mm2_game::effects` owns the contract:
//! [`mm2_game::VehicleSparks`] is the per-vehicle rig (deterministic
//! `NavRng` + [`mm2_game::SparkPolicy`], DSN-26) and
//! [`mm2_game::Spark`] the bounded live streak. This module feeds the
//! rig the deduplicated [`ImpactEvent`] stream — the authored contact
//! point and normal are the blast's `position`/`velocity` —
//! builds the sprite assets through the VFS (`texture/spark.tga`, an
//! authored 8×8 spark fleck) and renders each streak as a
//! velocity-aligned crossed quad under additive blending.
//!
//! Burst sizing, exit speed, spread, life and the live bound are
//! designed (the struct's `SparkMultiplier` is runtime state no retail
//! tune record authors; `vehCarDamage::Update()` is a thunk — UNK-13
//! stands for the original's shape).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use tracing::warn;

use mm2_assets::Vfs;
use mm2_game::{
    ImpactEvent, ObjectId, ObjectIdentity, Player, PlayerControl, Session, SessionEntity, Spark,
    VehicleSparks,
};

use crate::city;

/// `texture/spark.tga` — the authored spark fleck the recovered
/// renderer binds (designed binding: the original's `Init` texture
/// name is unrecovered, but this is the only spark-named texture the
/// install ships).
const SPARK_TEXTURE: &str = "spark";
/// Streak width in metres — designed presentation.
const STREAK_WIDTH: f32 = 0.03;
/// Warm spark tint, sRGB — designed (the recovered renderer's colour
/// is unrecovered; `spark.tga` decodes as a pale fleck).
const SPARK_TINT: [f32; 3] = [1.0, 0.72, 0.32];

/// Render assets for the spark streaks — built once per session.
pub struct SparkAssets {
    /// The crossed-quad streak mesh — unit extents on every axis so
    /// the transform's scale reads as (width, length, width) metres.
    pub mesh: Handle<Mesh>,
    /// Base material — cloned per spark so [`Spark::alpha`] animates
    /// one streak, not the pool.
    pub material: Handle<StandardMaterial>,
}

/// Session-scoped spark state — inserted by `load_session_world`,
/// removed on teardown. Absent in tests that never load a world; the
/// systems simply no-op.
#[derive(Resource)]
pub struct SparkFx {
    pub assets: SparkAssets,
}

/// Session-scoped counters the headless record reports (`spk=`).
#[derive(Resource, Default, Debug)]
pub struct SparkFxReport {
    /// Impact bursts drawn this session.
    pub bursts: u64,
    /// Streaks spawned this session.
    pub emitted: u64,
    /// Streaks expired and despawned this session.
    pub expired: u64,
}

impl SparkFxReport {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Build the sprite assets for one session — `spark.tga` resolves
/// through `city::load_image` so mods can override it like any
/// texture. A missing/undecodable texture warns and falls back to an
/// untextured material — the same missing-texture policy the rest of
/// the loader applies; it never sinks the session.
pub fn spark_assets(
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> SparkAssets {
    let texture = city::load_image(vfs, SPARK_TEXTURE).map(|(img, _)| images.add(img));
    if texture.is_none() {
        warn!(
            texture = SPARK_TEXTURE,
            "spark texture unresolved — untextured streaks"
        );
    }
    let material = materials.add(StandardMaterial {
        base_color_texture: texture,
        base_color: Color::srgba(SPARK_TINT[0], SPARK_TINT[1], SPARK_TINT[2], 1.0),
        // Additive blending: the authored fleck has no alpha channel —
        // black adds nothing, so the streak reads as light, and the
        // per-spark alpha scales the contribution.
        alpha_mode: AlphaMode::Add,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    SparkAssets {
        mesh: meshes.add(streak_mesh()),
        material,
    }
}

/// Two quads crossed along +Y — the "line" of a line spark: the
/// shared axis is the streak direction, and the cross section keeps
/// the streak visible from any viewpoint without per-frame camera
/// work (implementation choice). Unit extents on every axis; the
/// transform scale carries the real dimensions.
fn streak_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            // Quad A — XY plane.
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
            // Quad B — ZY plane.
            [0.0, -0.5, -0.5],
            [0.0, -0.5, 0.5],
            [0.0, 0.5, 0.5],
            [0.0, 0.5, -0.5],
        ],
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![[0.0, 0.0, 1.0]; 4]
            .into_iter()
            .chain(vec![[1.0, 0.0, 0.0]; 4])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![
            [0.0, 1.0],
            [1.0, 1.0],
            [1.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [1.0, 0.0],
            [0.0, 0.0],
        ],
    )
    .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]))
}

/// One streak's transform: +Y aligned to the velocity, scale
/// (width, length, width) in metres.
fn streak_transform(spark: &Spark) -> Transform {
    let dir = spark.velocity.try_normalize().unwrap_or(Vec3::Y);
    Transform::from_translation(spark.position)
        .with_rotation(Quat::from_rotation_arc(Vec3::Y, dir))
        .with_scale(Vec3::new(STREAK_WIDTH, spark.streak(), STREAK_WIDTH))
}

/// Emit a burst per reportable impact involving a spark-rigged
/// participant — the recovered `ImpactCB`→`RadialBlast` chain read
/// through the shared deduplicated [`ImpactEvent`] stream, so resting
/// contact and sub-threshold taps spark nothing. Each participant's
/// burst leaves the contact point along the normal *toward* it —
/// sprayed away from the surface it struck. Remote participants are
/// skipped like every F05 system (their own client renders them);
/// the reader drains while not `Playing` so a buffered stale impact
/// never flushes sparks into a pause or the next session.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn emit_sparks(
    mut commands: Commands,
    mut reader: MessageReader<ImpactEvent>,
    session: Res<Session>,
    fx: Option<Res<SparkFx>>,
    mut report: ResMut<SparkFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    mut rigs: Query<&mut VehicleSparks>,
    sparks: Query<&Spark>,
) {
    if !session.is_playing() {
        reader.read().for_each(drop);
        return;
    }
    let Some(fx) = fx else { return };
    let Some(base) = materials.get(&fx.assets.material).cloned() else {
        return;
    };
    let generation = session.generation();
    let owner = SessionEntity(generation);
    let index: HashMap<ObjectId, (Entity, Option<PlayerControl>)> = identities
        .iter()
        .map(|(entity, id, player)| (id.0, (entity, player.map(|p| p.control))))
        .collect();
    // Live streaks per emitter — the query cannot see this frame's
    // deferred spawns, so the count accrues locally or a burst-heavy
    // tick could overrun the designed pool bound.
    let mut live: HashMap<Entity, usize> = HashMap::new();
    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        for side in 0..2 {
            let object = if side == 0 {
                event.participants.0
            } else {
                event.participants.1
            };
            let Some(&(entity, control)) = index.get(&object) else {
                continue;
            };
            if control == Some(PlayerControl::Remote) {
                continue;
            }
            let Ok(mut rig) = rigs.get_mut(entity) else {
                continue;
            };
            // `normal` points from participant 0 toward participant 1 —
            // each side's burst rebounds the other way.
            let outward = if side == 0 {
                -event.normal
            } else {
                event.normal
            };
            let live_now = *live
                .entry(entity)
                .or_insert_with(|| sparks.iter().filter(|s| s.emitter == entity).count());
            let burst = rig.burst(event.point, outward, event.severity, live_now, entity);
            if !burst.is_empty() {
                report.bursts += 1;
                live.insert(entity, live_now + burst.len());
            }
            for spark in burst {
                let mut material = base.clone();
                material.base_color =
                    Color::srgba(SPARK_TINT[0], SPARK_TINT[1], SPARK_TINT[2], spark.alpha());
                let transform = streak_transform(&spark);
                commands.spawn((
                    owner,
                    spark,
                    Mesh3d(fx.assets.mesh.clone()),
                    MeshMaterial3d(materials.add(material)),
                    transform,
                ));
                report.emitted += 1;
            }
        }
    }
}

/// Integrate live streaks and sync their transforms/materials —
/// position, velocity alignment and the designed alpha fade. Expired
/// streaks despawn on their `life` (the bound is the policy, not a
/// leak). Session-stamped, so teardown takes any stragglers.
pub fn advance_sparks(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    mut report: ResMut<SparkFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sparks: Query<(
        Entity,
        &mut Spark,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    if !session.is_playing() {
        return;
    }
    let dt = time.delta_secs();
    for (entity, mut spark, mut xf, material) in &mut sparks {
        if !spark.advance(dt) {
            commands.entity(entity).despawn();
            report.expired += 1;
            continue;
        }
        *xf = streak_transform(&spark);
        if let Some(mut mat) = materials.get_mut(&material.0) {
            mat.base_color =
                Color::srgba(SPARK_TINT[0], SPARK_TINT[1], SPARK_TINT[2], spark.alpha());
        }
    }
}
