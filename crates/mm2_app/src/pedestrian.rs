//! Pedestrian actors on screen (F19-A.5): the assembled `.mod` skin
//! deformed over the authored animation state model, drawn through
//! Bevy meshes.
//!
//! [`PedArchetype`] (mm2_content) owns the parsed rig, skin, states,
//! clips and paint-job shader table; this module turns it into
//! renderable actors. Each actor owns one Bevy mesh per `.mod`
//! material group, and [`animate_pedestrians`] re-deforms those meshes
//! every frame the session is `Playing` — a rigid CPU skin (the original
//! `.mod` verts are bone-local, so a posed bone transform applies
//! directly), which at ~120 vertices per figure is cheaper than the
//! per-actor bind groups a GPU skin would need.
//!
//! The only spawner so far is the `--ped-lab` evidence line-up
//! ([`spawn_ped_lab`]): one figure per stock archetype in front of the
//! player, each stepping through the authored states in place. It is an
//! inspection tool for the "animate without broken limbs" acceptance
//! (F19-AC02) — not a crowd. Sidewalk movement, density and reactions
//! are later slices (F19-B); nothing here makes a pedestrian a
//! gameplay object.
//!
//! Materials come from the archetype's `.shaders` paint-job table
//! (colour only — retail pedestrian meshes author no textures).

use std::collections::HashMap;
use std::sync::Arc;

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use mm2_content::PedArchetype;
use mm2_game::ped::{PedAnimator, PedDeform, PedSkin};
use mm2_game::{Mm2Vfs, PlayerVehicle, Session, SessionEntity};

use crate::layers::GameLayer;

/// Upper bound on live pedestrian actors — the lab spawns a handful,
/// and later spawners must stay under this until a measured budget
/// replaces it (F19-AC05).
pub const MAX_PED_ACTORS: usize = 64;

/// Authored archetypes the lab lines up, in display order.
pub const LAB_ARCHETYPES: [&str; 4] = [
    "pedmodel_man",
    "pedmodel_woman",
    "pedmodel_manw",
    "pedmodel_womanw",
];

/// Authored states the lab steps each figure through, in order. Dive
/// requests chain through their ground states back to `STAND` on their
/// own, so [`LAB_HOLD_SECS`] is long enough for the whole dive.
pub const LAB_STATES: [&str; 9] = [
    "STAND",
    "WALK",
    "RUN",
    "BACKUP",
    "STAND2",
    "ANTIC",
    "ANTIC2",
    "WALK_LDIVE",
    "WALK_RDIVE",
];

/// Seconds the lab holds each requested state.
pub const LAB_HOLD_SECS: f32 = 3.0;
/// Lateral spacing between lab figures, metres.
const LAB_SPACING: f32 = 1.8;
/// How far in front of the player the lab line stands, metres.
const LAB_AHEAD: f32 = 9.0;

/// One mesh's worth of the skin: the corners (and triangles over them)
/// that share a `.mod` material group.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupPlan {
    /// Material group index (== shader column in a paint job).
    pub material: usize,
    /// Skin corner behind each mesh vertex.
    pub corners: Vec<u32>,
    /// Triangle list over mesh vertices.
    pub indices: Vec<u32>,
    /// Per-vertex first UV set.
    pub uvs: Vec<[f32; 2]>,
}

/// Split `skin` into one [`GroupPlan`] per material group that owns
/// triangles. Orphan triangles (owned by no group) are not drawn —
/// `PedSkin::issues` already records them. `.mod` triangles are kept in
/// their authored winding; `PedSkin::winding_agreement` checks that
/// this is the front face.
pub fn plan_groups(skin: &PedSkin) -> Vec<GroupPlan> {
    let corners = skin.corners();
    let tris = skin.triangles();
    let mut out = Vec::new();
    for (material, mtl) in skin.materials().iter().enumerate() {
        if mtl.tris.is_empty() {
            continue;
        }
        let mut local: HashMap<u32, u32> = HashMap::new();
        let mut plan = GroupPlan {
            material,
            corners: Vec::new(),
            indices: Vec::with_capacity(mtl.tris.len() * 3),
            uvs: Vec::new(),
        };
        for tri in &tris[mtl.tris.clone()] {
            for &c in tri {
                let next = plan.corners.len() as u32;
                let idx = *local.entry(c).or_insert_with(|| {
                    plan.corners.push(c);
                    plan.uvs.push(corners[c as usize].uv);
                    next
                });
                plan.indices.push(idx);
            }
        }
        out.push(plan);
    }
    out
}

/// A fresh Bevy mesh for one group over `deform`.
fn group_mesh(plan: &GroupPlan, deform: &PedDeform) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    write_group(&mut mesh, plan, deform);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, plan.uvs.clone());
    mesh.insert_indices(Indices::U32(plan.indices.clone()));
    mesh
}

/// Overwrite a group mesh's positions and normals from `deform`.
fn write_group(mesh: &mut Mesh, plan: &GroupPlan, deform: &PedDeform) {
    let positions: Vec<[f32; 3]> = plan
        .corners
        .iter()
        .map(|&c| deform.positions[c as usize].to_array())
        .collect();
    let normals: Vec<[f32; 3]> = plan
        .corners
        .iter()
        .map(|&c| deform.normals[c as usize].to_array())
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
}

/// What an actor shares with its archetype siblings.
#[derive(Debug)]
pub struct PedShape {
    /// The loaded archetype.
    pub archetype: Arc<PedArchetype>,
    /// The mesh split, computed once per archetype.
    pub groups: Vec<GroupPlan>,
}

impl PedShape {
    /// Plan the mesh split for `archetype`.
    pub fn new(archetype: Arc<PedArchetype>) -> Self {
        let groups = plan_groups(&archetype.skin);
        Self { archetype, groups }
    }
}

/// A pedestrian figure: its archetype, its place in the authored state
/// machine and the mesh handles it deforms.
#[derive(Component)]
pub struct PedActor {
    shape: Arc<PedShape>,
    animator: PedAnimator,
    meshes: Vec<Handle<Mesh>>,
    /// Hold the root's horizontal clip drift at zero so the figure
    /// animates in place (the lab); locomotion would carry it instead.
    in_place: bool,
}

impl PedActor {
    /// The state the figure is in.
    pub fn state(&self) -> &str {
        &self.animator.current().name
    }

    /// The archetype stem, e.g. `pedmodel_man`.
    pub fn archetype(&self) -> &str {
        &self.shape.archetype.stem
    }

    /// Ask for an authored state; `false` (nothing changes) when the
    /// archetype has no such state.
    pub fn request(&mut self, state: &str) -> bool {
        self.animator.request(state)
    }

    /// The deformed skin at the actor's current animation frame.
    pub fn deform(&self) -> Result<PedDeform, String> {
        deform_now(&self.shape.archetype, &self.animator, self.in_place)
    }
}

/// The lab's state tour for one figure.
#[derive(Component)]
pub struct PedLabTour {
    next: usize,
    held: f32,
}

fn deform_now(
    archetype: &PedArchetype,
    animator: &PedAnimator,
    in_place: bool,
) -> Result<PedDeform, String> {
    let clip = archetype
        .clip(&animator.current().clip)
        .ok_or_else(|| format!("clip {:?} is not loaded", animator.current().clip))?;
    let mut pose = animator
        .pose(&archetype.rig, clip)
        .map_err(|e| e.to_string())?;
    if in_place {
        // Channel 0 is the root's world translation; keep its height,
        // drop the horizontal drift the clip walks along.
        pose.bones[0].translation.x = 0.0;
        pose.bones[0].translation.z = 0.0;
    }
    let world = archetype.rig.world_transforms(&pose);
    archetype.skin.deform(&world).map_err(|e| e.to_string())
}

/// Colour material for one group under paint job `paint`.
fn group_material(
    archetype: &PedArchetype,
    paint: usize,
    material: usize,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let colour = archetype
        .shader(paint, material)
        .map_or([0.6, 0.6, 0.6, 1.0], |s| s.diffuse);
    materials.add(StandardMaterial {
        base_color: Color::srgba(colour[0], colour[1], colour[2], 1.0),
        perceptual_roughness: 0.9,
        ..default()
    })
}

/// Spawn one figure of `shape` at `transform` under paint job `paint`
/// (wrapped into the archetype's table), standing in `start`.
/// `None` when `start` is not an authored state, the pose cannot be
/// sampled, or the live-actor cap is reached.
#[allow(clippy::too_many_arguments)] // Bevy spawn helper: Commands plus the asset stores it fills.
pub fn spawn_pedestrian(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    shape: &Arc<PedShape>,
    paint: usize,
    start: &str,
    transform: Transform,
    owner: SessionEntity,
    live: usize,
) -> Option<Entity> {
    if live >= MAX_PED_ACTORS {
        return None;
    }
    let archetype = &shape.archetype;
    let animator = archetype.animator(start).ok()?;
    let bind = deform_now(archetype, &animator, true).ok()?;
    let paint = paint % archetype.paint_jobs().max(1);
    let mut handles = Vec::with_capacity(shape.groups.len());
    let root = commands
        .spawn((transform, Visibility::default(), owner))
        .id();
    for plan in &shape.groups {
        let handle = meshes.add(group_mesh(plan, &bind));
        handles.push(handle.clone());
        let material = group_material(archetype, paint, plan.material, materials);
        // The mesh is re-posed every frame and a dive leaves the bind
        // pose's box, so the spawn-time bounds would cull it wrongly.
        let child = commands
            .spawn((Mesh3d(handle), MeshMaterial3d(material), NoFrustumCulling))
            .id();
        commands.entity(root).add_child(child);
    }
    commands.entity(root).insert(PedActor {
        shape: shape.clone(),
        animator,
        meshes: handles,
        in_place: true,
    });
    Some(root)
}

/// Step every actor's state machine and re-pose its meshes. Runs only
/// while the session is `Playing`, so a pause freezes the figures.
pub fn animate_pedestrians(
    time: Res<Time>,
    session: Res<Session>,
    mut actors: Query<&mut PedActor>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if !session.is_playing() {
        return;
    }
    let dt = time.delta_secs();
    for mut actor in &mut actors {
        let actor = &mut *actor;
        let archetype = &actor.shape.archetype;
        let Some(frames) = archetype
            .clip(&actor.animator.current().clip)
            .map(|c| c.frames)
        else {
            continue;
        };
        actor.animator.tick(dt, frames);
        let Ok(deform) = deform_now(archetype, &actor.animator, actor.in_place) else {
            continue;
        };
        for (handle, plan) in actor.meshes.iter().zip(&actor.shape.groups) {
            if let Some(mut mesh) = meshes.get_mut(handle) {
                write_group(&mut mesh, plan, &deform);
            }
        }
    }
}

/// Walk each lab figure through [`LAB_STATES`], one request per
/// [`LAB_HOLD_SECS`] of playing time.
pub fn advance_ped_lab(
    time: Res<Time>,
    session: Res<Session>,
    mut figures: Query<(&mut PedActor, &mut PedLabTour)>,
) {
    if !session.is_playing() {
        return;
    }
    for (mut actor, mut tour) in &mut figures {
        tour.held += time.delta_secs();
        if tour.held < LAB_HOLD_SECS {
            continue;
        }
        tour.held = 0.0;
        let state = LAB_STATES[tour.next % LAB_STATES.len()];
        tour.next += 1;
        // An archetype without the state is skipped, not faked.
        actor.request(state);
    }
}

/// Whether the running session asked for the lab.
pub fn lab_enabled(session: Res<Session>) -> bool {
    session.config().is_some_and(|c| c.dev.ped_lab)
}

/// Marks the session generation the lab already spawned for.
#[derive(Resource, Default)]
pub struct PedLabSpawned(Option<u64>);

/// Spawn the `--ped-lab` line-up once per session, in front of the
/// local player, on the ground a ray finds there. Archetypes that fail
/// to load are logged and skipped — never replaced by a stand-in.
#[allow(clippy::too_many_arguments)] // Bevy system: the borrows are the contract.
pub fn spawn_ped_lab(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    mut spawned: ResMut<PedLabSpawned>,
    player: Query<&GlobalTransform, With<PlayerVehicle>>,
    live: Query<(), With<PedActor>>,
    spatial: SpatialQuery,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !session.is_playing() || spawned.0 == Some(session.generation()) {
        return;
    }
    let (Some(vfs), Ok(player)) = (vfs, player.single()) else {
        return;
    };
    let origin = player.translation() + player.forward() * LAB_AHEAD;
    let filter = SpatialQueryFilter::from_mask(GameLayer::World);
    let Some(ground) = spatial.cast_ray(origin + Vec3::Y * 20.0, Dir3::NEG_Y, 60.0, false, &filter)
    else {
        // No world under the line-up yet; try again next frame.
        return;
    };
    spawned.0 = Some(session.generation());
    let floor = origin.with_y(origin.y + 20.0 - ground.distance);
    // Figures face the car; they stand along the car's lateral axis.
    let facing = Quat::from_rotation_arc(
        Vec3::NEG_Z,
        Vec3::new(-player.forward().x, 0.0, -player.forward().z).normalize_or(Vec3::Z),
    );
    let lateral = facing * Vec3::X;
    let owner = SessionEntity(session.generation());
    let mut count = live.iter().count();
    let n = LAB_ARCHETYPES.len() as f32;
    for (i, stem) in LAB_ARCHETYPES.iter().enumerate() {
        let archetype = match PedArchetype::load(&vfs.0, stem) {
            Ok(a) => Arc::new(PedShape::new(Arc::new(a))),
            Err(e) => {
                warn!(archetype = *stem, error = %e, "--ped-lab: archetype skipped");
                continue;
            }
        };
        let offset = (i as f32 - (n - 1.0) / 2.0) * LAB_SPACING;
        let at = Transform::from_translation(floor + lateral * offset).with_rotation(facing);
        // A different paint job per figure, so the clothing table shows.
        if let Some(e) = spawn_pedestrian(
            &mut commands,
            &mut meshes,
            &mut materials,
            &archetype,
            i * 5,
            "STAND",
            at,
            owner,
            count,
        ) {
            commands.entity(e).insert(PedLabTour {
                next: 1 + i,
                held: 0.0,
            });
            count += 1;
        }
    }
    info!(figures = count, "--ped-lab: line-up spawned");
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::ped::{PedMod, PedSkel};
    use mm2_game::ped::PedRig;

    const SKEL: &str =
        "NumBones 2\nbone root {\n\toffset 0 1 0\n\tbone a {\n\t\toffset 0 0.5 0\n\t}\n}\n";

    /// Two material groups over three corners; group 1 shares corner 0
    /// with group 0, so each plan keeps its own local numbering.
    const MOD: &str = "\
version: 1.09
verts: 4
normals: 4
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 6
primitives: 2
matrices: 2

v	0.0	0.0	0.0
v	1.0	0.0	0.0
v	0.0	1.0	0.0
v	0.0	2.0	0.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
n	0.0	0.0	1.0
c	1.0	1.0	1.0	1.0
t1	0.5	0.5

mtl Test1:SKIN {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.4 0.3 0.2
	diffuse:	0.7 0.6 0.5
	specular:	0.8 0.7 0.6
}

mtl Test1:HAIR {
	packets:	1
	primitives:	1
	textures:	0
	illum: diffuse
	ambient:	0.1 0.0 0.0
	diffuse:	0.4 0.1 0.1
	specular:	0.6 0.4 0.4
}

packet 3 1 1 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	0
	adj	2	2	0	0	0	0
	tri	0	1	2
	mtx 0
}

packet 3 1 1 {
	adj	0	0	0	0	0	0
	adj	1	1	0	0	0	0
	adj	3	3	0	0	0	0
	tri	0	1	2
	mtx 0
}

mtxv 4 0
mtxn 4 0
";

    fn skin() -> (PedSkin, PedRig) {
        let rig = PedRig::from_skel(&PedSkel::parse(SKEL).unwrap()).unwrap();
        let m = PedMod::parse(MOD).unwrap();
        (PedSkin::from_mod(&m, &rig).unwrap(), rig)
    }

    #[test]
    fn each_material_group_becomes_one_mesh_with_local_numbering() {
        let (skin, _) = skin();
        let plans = plan_groups(&skin);
        assert_eq!(plans.len(), 2);
        for (i, p) in plans.iter().enumerate() {
            assert_eq!(p.material, i);
            assert_eq!(p.indices, vec![0, 1, 2], "indices renumber from zero");
            assert_eq!(p.corners.len(), 3);
            assert_eq!(p.uvs.len(), 3);
        }
        // The two groups reach disjoint skin corners — no shared vertex
        // across meshes (each owns its copy).
        let mut all: Vec<u32> = plans.iter().flat_map(|p| p.corners.clone()).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn a_group_mesh_follows_the_deformed_corners() {
        let (skin, rig) = skin();
        let plans = plan_groups(&skin);
        let bind = rig.world_transforms(&rig.bind_pose());
        let at_bind = skin.deform(&bind).unwrap();
        let mut mesh = group_mesh(&plans[0], &at_bind);
        let read = |m: &Mesh| -> Vec<[f32; 3]> {
            m.attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .unwrap()
                .to_vec()
        };
        let before = read(&mesh);
        // Lift the root a metre: every corner rides it.
        let mut pose = rig.bind_pose();
        pose.bones[0].translation.y += 1.0;
        let lifted = skin.deform(&rig.world_transforms(&pose)).unwrap();
        write_group(&mut mesh, &plans[0], &lifted);
        let after = read(&mesh);
        for (b, a) in before.iter().zip(&after) {
            assert!((a[1] - b[1] - 1.0).abs() < 1e-5, "{b:?} -> {a:?}");
            assert_eq!((a[0], a[2]), (b[0], b[2]));
        }
        assert_eq!(
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().len(),
            before.len()
        );
    }

    #[test]
    fn winding_agreement_counts_only_non_degenerate_triangles() {
        let (skin, rig) = skin();
        let d = skin
            .deform(&rig.world_transforms(&rig.bind_pose()))
            .unwrap();
        // (0,0)-(1,0)-(0,1) in the XY plane is counter-clockwise seen
        // from +Z, the authored normal's side.
        assert_eq!(skin.winding_agreement(&d), (2, 2));
    }
}
