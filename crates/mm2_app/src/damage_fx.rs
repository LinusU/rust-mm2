//! F05-B.6: authored engine smoke — the damaged-tier visual leg of
//! F05-B's remaining work.
//!
//! `mm2_game::effects` owns the contract: the authored particle spec
//! (`vehCarDamage`'s `EngineSmokeRule`), the pivot gate
//! (`SmokeOffset`/`SmokeOffset2`/`DoublePivot`/`MirrorPivot`) and the
//! bounded [`SmokePuff`] state. This module builds the sprite assets
//! through the VFS, emits one billboard quad per puff and integrates
//! them. The emission gate and cadence are designed policy (DSN-24 —
//! the original `vehCarDamage::Update()` is a binary thunk); the
//! pivots, particle values and atlas/tile vocabulary are authored.
//! `TextelDamageRadius` and the recovered `ImpactsTable` texel-damage
//! leg stay unconsumed — still UNK-13.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use tracing::warn;

use mm2_assets::Vfs;
use mm2_game::{
    Player, PlayerControl, Session, SessionEntity, SmokePuff, VehicleDamage, VehicleSmoke,
};

use crate::city;

/// `texture/fxpt2` — the engine-smoke atlas: a measured 2×2 grid of
/// puff tiles; `TexFrameStart`/`TexFrameEnd` index its tiles like
/// mm2hook's `asSparkPos::TexCoordOffset` (designed binding — the
/// original's texture choice is unrecovered, but every retail damage
/// record authors frames inside a 2×2 tile space).
const SMOKE_TEXTURE: &str = "fxpt2";

/// Render assets for the smoke sprites — built once per session.
pub struct SmokeAssets {
    /// One 1×1 quad mesh per atlas tile, UVs baked to that tile.
    pub quads: Vec<Handle<Mesh>>,
    /// Base material — cloned per puff so [`SmokePuff::alpha`]
    /// animates one puff, not the pool.
    pub material: Handle<StandardMaterial>,
}

/// Session-scoped smoke state — inserted by `load_session_world`,
/// removed on teardown. Absent in tests that never load a world; the
/// systems simply no-op.
#[derive(Resource)]
pub struct SmokeFx {
    pub assets: SmokeAssets,
}

/// Session-scoped counters the headless record reports (`ptx=`).
#[derive(Resource, Default, Debug)]
pub struct SmokeFxReport {
    /// Puffs spawned this session.
    pub emitted: u64,
    /// Puffs expired and despawned this session.
    pub expired: u64,
}

impl SmokeFxReport {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Build the sprite assets for one session — the atlas resolves
/// through `city::load_image` so mods can override it like any
/// texture. A missing/undecodable `fxpt2` warns and falls back to an
/// untextured material — the same missing-texture policy the rest of
/// the loader applies; it never sinks the session.
pub fn smoke_assets(
    vfs: &Vfs,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    atlas_tiles: u32,
) -> SmokeAssets {
    let texture = city::load_image(vfs, SMOKE_TEXTURE).map(|(img, _)| images.add(img));
    if texture.is_none() {
        warn!(
            texture = SMOKE_TEXTURE,
            "smoke atlas unresolved — untextured puffs"
        );
    }
    let material = materials.add(StandardMaterial {
        base_color_texture: texture,
        // Sprites are unlit blended quads — lit smoke would pick up
        // scene lighting the authored tint never accounted for.
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let tiles = atlas_tiles.max(1);
    let quads = (0..tiles * tiles)
        .map(|t| meshes.add(tile_quad(t, tiles)))
        .collect();
    SmokeAssets { quads, material }
}

/// A 1×1 quad in the XY plane facing +Z, UVs baked to `tile` of an
/// `n×n` atlas (tile 0 = top-left, row-major — the measured `fxpt`
/// layout; v=0 is the image's top row, so the quad's +Y edge binds
/// the tile's top).
fn tile_quad(tile: u32, n: u32) -> Mesh {
    let col = (tile % n) as f32;
    let row = (tile / n) as f32;
    let n = n as f32;
    let (u0, u1) = (col / n, (col + 1.0) / n);
    let (v0, v1) = (row / n, (row + 1.0) / n);
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ],
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4])
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
    )
    .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

/// Emit puffs from every rigged participant. Presentation of whatever
/// `VehicleDamage` state the entity carries — remote participants are
/// skipped like every F05 system because their damage is the remote
/// authority's state (and its own client renders it). Gated on
/// `Playing` so a pause freezes emission with the rest of the sim.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn drive_smoke(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    fx: Option<Res<SmokeFx>>,
    mut report: ResMut<SmokeFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut rigs: Query<(
        Entity,
        &mut VehicleSmoke,
        &VehicleDamage,
        &Transform,
        Option<&Player>,
    )>,
    puffs: Query<&SmokePuff>,
) {
    if !session.is_playing() {
        return;
    }
    let Some(fx) = fx else { return };
    let Some(base) = materials.get(&fx.assets.material).cloned() else {
        return;
    };
    let dt = time.delta_secs();
    let owner = SessionEntity(session.generation());
    for (entity, mut rig, damage, xf, player) in &mut rigs {
        if player.is_some_and(|p| p.control == PlayerControl::Remote) {
            continue;
        }
        let rate = rig.policy.rate(damage.total(), &damage.spec);
        if rate <= 0.0 {
            continue;
        }
        let live = puffs.iter().filter(|p| p.emitter == entity).count();
        for idx in rig.draw(dt, rate, live) {
            let origin = xf.transform_point(rig.emitters[idx].pivot);
            let puff = rig.puff(idx, origin, entity);
            let quad =
                fx.assets.quads[(puff.frame as usize).min(fx.assets.quads.len() - 1)].clone();
            let mut material = base.clone();
            let [r, g, b] = puff.rgb();
            material.base_color = Color::srgba(r, g, b, puff.alpha());
            let transform = Transform::from_translation(puff.position)
                .with_scale(Vec3::splat(puff.radius * 2.0));
            commands.spawn((
                owner,
                puff,
                Mesh3d(quad),
                MeshMaterial3d(materials.add(material)),
                transform,
            ));
            report.emitted += 1;
        }
    }
}

/// Integrate live puffs and sync their sprites — position, growth,
/// camera-facing rotation (billboarding is render policy: the
/// authored spec's `DRotation` is 0 on every retail record and the
/// original's sprite orientation is unrecovered) and per-puff alpha.
/// Expired puffs despawn — the bound is the authored `Life`, not a
/// leak. Session-stamped, so teardown takes any stragglers.
pub fn advance_smoke(
    mut commands: Commands,
    time: Res<Time>,
    session: Res<Session>,
    mut report: ResMut<SmokeFxReport>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut puffs: Query<(
        Entity,
        &mut SmokePuff,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    if !session.is_playing() {
        return;
    }
    let dt = time.delta_secs();
    let cam_rot = cameras
        .iter()
        .find(|(cam, _)| cam.is_active)
        .map(|(_, xf)| xf.compute_transform().rotation);
    for (entity, mut puff, mut xf, material) in &mut puffs {
        if !puff.advance(dt) {
            commands.entity(entity).despawn();
            report.expired += 1;
            continue;
        }
        xf.translation = puff.position;
        xf.scale = Vec3::splat((puff.radius * 2.0).max(0.0));
        if let Some(rot) = cam_rot {
            xf.rotation = rot;
        }
        if let Some(mut mat) = materials.get_mut(&material.0) {
            let [r, g, b] = puff.rgb();
            mat.base_color = Color::srgba(r, g, b, puff.alpha());
        }
    }
}
