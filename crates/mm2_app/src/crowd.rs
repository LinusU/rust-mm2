//! The sidewalk crowd (F19-B.2): pedestrians that populate a city
//! session, walk the authored sidewalks and are recycled with the
//! players' interest areas.
//!
//! [`mm2_game::pedwalk`] owns the rules (which curves walk, the seeded
//! placement draw, the bounded stepper); [`crate::pedestrian`] owns the
//! figures. This module is the glue that makes them a population:
//!
//! - **Load.** The first `Playing` frame of a city session builds the
//!   [`PedCrowd`]: the city's BAI nav graph and aimap overrides through
//!   the VFS, the walkable [`SidewalkNet`], and every stock archetype
//!   that loads and authors forward travel on `WALK`. An archetype that
//!   fails is logged and left out — never replaced by a stand-in — and a
//!   city without sidewalks or archetypes simply fields nobody.
//! - **Density and seed.** The population target is the resolved
//!   [`PedDensity`] (the player's pick, else the event's authored
//!   `Peds`, else the session default) scaled by the walk policy's cap;
//!   the draw stream is seeded from the session seed, so a restart with
//!   the same seed and spawn point fields the same crowd (F19-AC06).
//! - **Walking.** [`walk_pedestrians`] advances each walker along its
//!   curve at the speed its `WALK` clip plants the feet at, hopping
//!   kerb corners and turning at dead ends, and poses the figure on the
//!   curve facing its travel.
//! - **Interest.** [`maintain_pedestrians`] recycles a walker that
//!   leaves every player's bubble and refills toward the target through
//!   the planner's own placement draw — inside the spawn annulus,
//!   clear of every walker and every player — a couple per tick after
//!   the initial fill.
//! - **Pause and reset.** Both systems run only while the session is
//!   `Playing`, so a pause freezes the crowd. Figures are
//!   `SessionEntity`-stamped and the [`PedCrowd`] resource is removed on
//!   unload, so a restart starts clean and accumulates nothing.
//!
//! - **Reacting.** [`react_pedestrians`] (F19-B.3) lets a walker sense
//!   a car heading for it ([`mm2_game::pedreact`]): it stops and faces
//!   the car in the authored `ANTIC` state, dives clear through the
//!   authored `*_DIVE` chain when contact is imminent, then walks back
//!   to its curve. There is still no collider — a car that gives a
//!   walker too little time drives through it.
//!
//! - **Networked sessions (F19-B.5).** The crowd is cosmetic — no
//!   collider, no score, no rule reads it — so no process is its
//!   authority: the host and every `Remote` client field their own from
//!   the same density and seed, each around the players that process
//!   holds (a client's copies of remote cars included), and nothing
//!   about it travels on the wire. Two processes therefore see
//!   different walkers; that is a designed trade (DSN-90) for keeping
//!   the snapshot stream free of a population nobody can collide with.
//!
//! Not done here, deliberately: crossings (UNK-42), the original's
//! sensing rule (UNK-43), and pedestrian audio.

use std::sync::Arc;

use avian3d::prelude::{LinearVelocity, Position};
use bevy::prelude::*;
use mm2_content::PedArchetype;
use mm2_game::pedreact::{
    Approacher, DiveSide, Order, Phase, ReactPolicy, Reaction, most_urgent, rejoin_step,
};
use mm2_game::pedwalk::{
    SidewalkNet, WalkPolicy, Walker, candidate_curves, draw_pedestrian, target_population,
};
use mm2_game::{
    Mm2Vfs, NavGraph, NavOverrides, NavRng, Player, Session, SessionConfig, SessionEntity,
    WorldMode,
};
use mm2_vehicle::vehicle::Vehicle;

use crate::pedestrian::{LAB_ARCHETYPES, MAX_PED_ACTORS, PedActor, PedShape, spawn_pedestrian};

/// Walkers spawned per tick after the initial fill. The refill is a
/// drip so a bubble crossing a district does not pay a burst of mesh
/// builds in one frame.
const REFILL_PER_TICK: usize = 2;

/// Rate at which a walker's facing eases toward its travel direction
/// (1/s) — a corner turn reads as a turn, not a snap.
const TURN_RATE: f32 = 8.0;

/// The resolved pedestrian density for the running session
/// (`0.0..=1.0`). Inserted at session load; a session without it fields
/// no crowd.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PedDensity(pub f32);

/// The density chain, most specific first: the player's pick
/// (`SessionCustomization`), the event's authored `Peds` dial
/// (`RaceParams::peds` — Circuit rows author 0, CIR-3), then the
/// session default. Non-finite values read as 0.
pub fn resolve_density(config: &SessionConfig, authored: Option<f32>) -> PedDensity {
    let d = config
        .customization
        .map(|c| c.densities.pedestrians)
        .or(authored)
        .unwrap_or(config.densities.pedestrians);
    PedDensity(if d.is_finite() {
        d.clamp(0.0, 1.0)
    } else {
        0.0
    })
}

/// A pedestrian's walk state, on the figure's root entity beside its
/// [`PedActor`].
#[derive(Component, Debug, Clone, Copy)]
pub struct PedWalk {
    /// Where it is on the sidewalk net.
    pub walker: Walker,
    /// Ground speed (m/s), from the archetype's `WALK` clip.
    pub speed: f32,
}

/// A pedestrian's reaction to cars, on the figure's root beside its
/// [`PedWalk`] (F19-B.3).
#[derive(Component, Debug, Clone, Copy)]
pub struct PedReact {
    /// Where the walker is in the reaction.
    pub reaction: Reaction,
    /// Where the current dive started.
    anchor: Vec3,
    /// The walker's right-hand direction when the dive started.
    right: Vec3,
    /// Lateral dive travel reached so far (csv sign: `+` = left).
    lateral: f32,
    /// Where the dive row last played ends.
    row_end: f32,
}

impl PedReact {
    fn new() -> Self {
        Self {
            reaction: Reaction::new(),
            anchor: Vec3::ZERO,
            right: Vec3::X,
            lateral: 0.0,
            row_end: 0.0,
        }
    }
}

/// What a built crowd walks on.
struct CrowdMap {
    graph: NavGraph,
    net: SidewalkNet,
    shapes: Vec<Arc<PedShape>>,
}

/// Session-scoped crowd state. Inserted by [`maintain_pedestrians`] on
/// the first `Playing` frame of a city session, removed by session
/// teardown.
#[derive(Resource)]
pub struct PedCrowd {
    generation: u64,
    map: Option<CrowdMap>,
    rng: NavRng,
    /// The placement and recycle bounds this session runs under.
    pub policy: WalkPolicy,
    /// Population target for this session's density.
    pub target: usize,
    /// Whether the initial fill has run.
    populated: bool,
    /// Walkers ever spawned (initial fill + refills).
    pub spawned: usize,
    /// Walkers recycled out of every player's bubble.
    pub recycled: usize,
    /// Placement draws that found no spot (never forced).
    pub dropped: usize,
    /// Spawns refused because the figure could not be built.
    pub unspawnable: usize,
    /// Corner hops walkers have taken.
    pub hops: u64,
    /// Dead-end turn-arounds walkers have made.
    pub turned_around: u64,
    /// Walkers that stopped to watch an approaching car.
    pub alerts: u64,
    /// Dives walkers have made.
    pub dives: u64,
    /// Divers that walked back to their curve.
    pub rejoined: u64,
    /// The sensing and timing bounds walkers react under.
    pub react: ReactPolicy,
    /// Loaded archetypes that author every reaction state, of those
    /// loaded; set when the map is built.
    pub reacting: (usize, usize),
    /// Why the crowd is empty, when it is for a structural reason.
    pub issues: Vec<String>,
}

impl PedCrowd {
    /// Whether the session has anything to walk on.
    pub fn is_active(&self) -> bool {
        self.map.is_some()
    }

    /// The smoke-record field: `peds=<live>/<target> psp=<spawned>
    /// prec=<recycled> pdrop=<dropped> puns=<unspawnable> phop=<hops>
    /// pturn=<turn-arounds> pwary=<alerts> pdive=<dives>
    /// prej=<rejoined> prx=<reacting>/<archetypes>`, given the live walker count.
    pub fn smoke_detail(&self, live: usize) -> String {
        format!(
            " peds={live}/{} psp={} prec={} pdrop={} puns={} phop={} pturn={} pwary={} pdive={} prej={} prx={}/{}",
            self.target,
            self.spawned,
            self.recycled,
            self.dropped,
            self.unspawnable,
            self.hops,
            self.turned_around,
            self.alerts,
            self.dives,
            self.rejoined,
            self.reacting.0,
            self.reacting.1
        )
    }

    /// The session generation the crowd was built for.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Build the crowd for a city session. `None` map (with the reason in
/// `issues`) for a non-city world, a missing nav graph, no sidewalks or
/// no usable archetype.
fn build_crowd(
    vfs: &mm2_assets::Vfs,
    config: &SessionConfig,
    generation: u64,
    density: f32,
) -> PedCrowd {
    let policy = WalkPolicy::default();
    debug_assert!(policy.max_active <= MAX_PED_ACTORS);
    let mut crowd = PedCrowd {
        generation,
        map: None,
        rng: NavRng::new(config.seed.wrapping_add(0x5045_4443)),
        target: target_population(density, &policy),
        policy,
        populated: false,
        spawned: 0,
        recycled: 0,
        dropped: 0,
        unspawnable: 0,
        hops: 0,
        turned_around: 0,
        alerts: 0,
        dives: 0,
        rejoined: 0,
        react: ReactPolicy::default(),
        reacting: (0, 0),
        issues: Vec::new(),
    };
    if crowd.target == 0 {
        crowd.issues.push("pedestrian density is 0".to_string());
        return crowd;
    }
    let WorldMode::City { psdl } = &config.world else {
        crowd.issues.push("not a city world".to_string());
        return crowd;
    };
    let stem = std::path::Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());
    let build = match mm2_content::load_nav_graph(vfs, stem) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "pedestrians: nav graph failed — no crowd");
            crowd.issues.push(format!("nav graph: {e}"));
            return crowd;
        }
    };
    let overrides = match mm2_content::load_nav_overrides(vfs, &format!("city/{stem}.aimap")) {
        Ok(o) => o.unwrap_or_default(),
        Err(e) => {
            warn!(error = %e, "pedestrians: aimap failed — walking without its closures");
            crowd.issues.push(format!("aimap: {e}"));
            NavOverrides::default()
        }
    };
    let net = SidewalkNet::build(&build.graph, &overrides, &crowd.policy);
    if net.stats().walkable == 0 {
        crowd.issues.push("no walkable sidewalks".to_string());
        return crowd;
    }
    let mut shapes = Vec::new();
    for stem in LAB_ARCHETYPES {
        match PedArchetype::load(vfs, stem) {
            Ok(a) => {
                let shape = PedShape::new(Arc::new(a));
                if shape.walk_speed.is_some() {
                    shapes.push(Arc::new(shape));
                } else {
                    warn!(
                        archetype = stem,
                        "pedestrians: no WALK travel — archetype skipped"
                    );
                }
            }
            Err(e) => warn!(archetype = stem, error = %e, "pedestrians: archetype skipped"),
        }
    }
    if shapes.is_empty() {
        crowd
            .issues
            .push("no usable pedestrian archetype".to_string());
        return crowd;
    }
    crowd.reacting = (shapes.iter().filter(|s| s.reacts).count(), shapes.len());
    for shape in shapes.iter().filter(|s| !s.reacts) {
        warn!(
            archetype = shape.archetype.stem,
            "pedestrians: reaction states missing — it walks on obliviously"
        );
    }
    let st = net.stats();
    info!(
        density,
        target = crowd.target,
        walkable = st.walkable,
        joined_ends = st.joined_ends,
        archetypes = shapes.len(),
        reacting = crowd.reacting.0,
        "pedestrian crowd loaded"
    );
    crowd.map = Some(CrowdMap {
        graph: build.graph,
        net,
        shapes,
    });
    crowd
}

/// Heading change that is worth turning for: ignores a vertical curve
/// whose tangent has no horizontal part.
fn heading(tangent: [f32; 3]) -> Option<Quat> {
    facing(Vec3::from(tangent))
}

/// The rotation that turns a figure (facing `-Z`) toward the horizontal
/// part of `dir`; `None` when there is none.
fn facing(dir: Vec3) -> Option<Quat> {
    let h = Vec3::new(dir.x, 0.0, dir.z);
    (h.length_squared() > 1e-6).then(|| Quat::from_rotation_arc(Vec3::NEG_Z, h.normalize()))
}

/// Advance every walker along its sidewalk and pose its figure. Runs
/// only while the session is `Playing`.
pub fn walk_pedestrians(
    time: Res<Time>,
    session: Res<Session>,
    crowd: Option<ResMut<PedCrowd>>,
    mut walkers: Query<(&mut PedWalk, &mut Transform, Option<&PedReact>)>,
) {
    if !session.is_playing() {
        return;
    }
    let Some(mut crowd) = crowd else {
        return;
    };
    let crowd = &mut *crowd;
    let Some(map) = &crowd.map else {
        return;
    };
    let dt = time.delta_secs();
    let ease = (dt * TURN_RATE).min(1.0);
    for (mut walk, mut at, react) in &mut walkers {
        // A walker watching, diving or walking back is not on its curve.
        if react.is_some_and(|r| !r.reaction.is_walking()) {
            continue;
        }
        let ds = walk.speed * dt;
        let step = map.net.advance(&mut walk.walker, ds, &mut crowd.rng);
        crowd.hops += u64::from(step.hops);
        crowd.turned_around += u64::from(step.turned_around);
        let Some(sample) = map.net.sample(&map.graph, &walk.walker) else {
            continue;
        };
        at.translation = Vec3::from(sample.position);
        if let Some(face) = heading(sample.tangent) {
            at.rotation = at.rotation.slerp(face, ease);
        }
    }
}

/// Let walkers sense approaching cars and react (F19-B.3): stop and
/// face a car that is heading for them, dive clear when contact is
/// imminent, then walk back to their curve. Runs only while `Playing`,
/// before [`walk_pedestrians`]; a figure whose archetype lacks the
/// reaction states is left to walk on.
#[allow(clippy::type_complexity)] // Bevy query tuple: one system, one query.
pub fn react_pedestrians(
    time: Res<Time>,
    session: Res<Session>,
    crowd: Option<ResMut<PedCrowd>>,
    cars: Query<(&Position, &LinearVelocity, &Vehicle)>,
    mut walkers: Query<(&PedWalk, &mut PedReact, &mut PedActor, &mut Transform)>,
) {
    if !session.is_playing() {
        return;
    }
    let Some(mut crowd) = crowd else {
        return;
    };
    let crowd = &mut *crowd;
    let Some(map) = &crowd.map else {
        return;
    };
    let dt = time.delta_secs();
    let ease = (dt * TURN_RATE).min(1.0);
    let policy = crowd.react;
    let approachers: Vec<Approacher> = cars
        .iter()
        .map(|(pos, vel, vehicle)| Approacher {
            position: pos.0,
            velocity: vel.0,
            half_length: vehicle.config.chassis_size[2] * 0.5,
            half_width: vehicle.config.chassis_size[0] * 0.5,
        })
        .collect();
    for (walk, mut react, mut actor, mut at) in &mut walkers {
        if !actor.reacts() {
            continue;
        }
        let react = &mut *react;
        let right = Vec3::new((at.rotation * Vec3::X).x, 0.0, (at.rotation * Vec3::X).z)
            .normalize_or_zero();
        let threat = match react.reaction.phase {
            Phase::Walking | Phase::Wary => {
                most_urgent(at.translation, approachers.iter().copied(), &policy)
            }
            Phase::Diving | Phase::Rejoining => None,
        };
        let side = threat.map_or(DiveSide::Right, |t| DiveSide::toward(right, t.away));
        match react
            .reaction
            .decide(actor.state(), threat.as_ref(), side, dt, &policy)
        {
            Order::None => {}
            Order::Alert => {
                actor.request("ANTIC");
                crowd.alerts += 1;
            }
            Order::Dive(side) => {
                let state = if actor.state().contains("ANTIC") {
                    side.from_antic()
                } else {
                    side.from_walk()
                };
                actor.request(state);
                react.anchor = at.translation;
                react.right = right;
                react.lateral = 0.0;
                react.row_end = 0.0;
                crowd.dives += 1;
            }
            Order::Resume => {
                actor.request("WALK");
            }
        }
        match react.reaction.phase {
            Phase::Walking => {}
            Phase::Wary => {
                if let Some(face) = threat.and_then(|t| facing(t.toward)) {
                    at.rotation = at.rotation.slerp(face, ease);
                }
            }
            Phase::Diving => {
                // A row that authors no lateral travel (the ground
                // recovery) follows the dive row that finished: it
                // holds that row's full distance, not the last frame
                // the cursor happened to sample.
                match actor.lateral_travel() {
                    Some((now, end)) => {
                        react.lateral = now;
                        react.row_end = end;
                    }
                    None => react.lateral = react.row_end,
                }
                // The csv authors the left dive positive; `right` is the
                // walker's right-hand side.
                at.translation = react.anchor + react.right * -react.lateral;
            }
            Phase::Rejoining => {
                let Some(sample) = map.net.sample(&map.graph, &walk.walker) else {
                    react.reaction.rejoined();
                    continue;
                };
                let home = Vec3::from(sample.position);
                let (next, arrived) =
                    rejoin_step(at.translation, home, walk.speed * dt, policy.rejoin_radius);
                if let Some(face) = facing(next - at.translation) {
                    at.rotation = at.rotation.slerp(face, ease);
                }
                at.translation = next;
                if arrived {
                    react.reaction.rejoined();
                    crowd.rejoined += 1;
                }
            }
        }
    }
}

/// Recycle walkers that left every player's bubble, then refill toward
/// the density target. Builds the [`PedCrowd`] on the session's first
/// `Playing` frame. Runs only while `Playing` and never beside the
/// `--ped-lab` line-up.
#[allow(clippy::too_many_arguments)] // Bevy system: the borrows are the contract.
pub fn maintain_pedestrians(
    mut commands: Commands,
    session: Res<Session>,
    density: Option<Res<PedDensity>>,
    vfs: Option<Res<Mm2Vfs>>,
    crowd: Option<ResMut<PedCrowd>>,
    players: Query<&Position, With<Player>>,
    walkers: Query<(Entity, &PedWalk, &Transform)>,
    actors: Query<(), With<PedActor>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !session.is_playing() {
        return;
    }
    let (Some(density), Some(vfs), Some(config)) = (density, vfs, session.config()) else {
        return;
    };
    if config.dev.ped_lab {
        return;
    }
    let generation = session.generation();
    let Some(mut crowd) = crowd.filter(|c| c.generation == generation) else {
        commands.insert_resource(build_crowd(&vfs.0, config, generation, density.0));
        return;
    };
    let crowd = &mut *crowd;
    let Some(map) = &crowd.map else {
        return;
    };
    // No players means no bubble to populate or collect against: hold
    // the crowd rather than despawning the city.
    let interest: Vec<[f32; 3]> = players.iter().map(|p| p.0.to_array()).collect();
    if interest.is_empty() {
        return;
    }

    let mut occupied: Vec<[f32; 3]> = Vec::new();
    for (entity, _, at) in &walkers {
        if mm2_game::pedwalk::within_bubble(at.translation.to_array(), &interest, &crowd.policy) {
            occupied.push(at.translation.to_array());
        } else {
            crowd.recycled += 1;
            commands.entity(entity).despawn();
        }
    }
    let active = occupied.len();
    if active >= crowd.target {
        crowd.populated = true;
        return;
    }
    occupied.extend(interest.iter().copied());

    let budget = if crowd.populated {
        REFILL_PER_TICK
    } else {
        crowd.target
    };
    let candidates = candidate_curves(&map.net, &map.graph, &interest, &crowd.policy);
    let owner = SessionEntity(generation);
    let mut live = actors.iter().count();
    for _ in 0..budget.min(crowd.target - active) {
        let Some(spawn) = draw_pedestrian(
            &map.net,
            &map.graph,
            &candidates,
            &mut crowd.rng,
            &interest,
            &occupied,
            &crowd.policy,
        ) else {
            crowd.dropped += 1;
            continue;
        };
        let shape = &map.shapes[(spawn.variant % map.shapes.len() as u64) as usize];
        let Some(speed) = shape.walk_speed else {
            continue;
        };
        let paint = (spawn.variant >> 24) as usize;
        let at = Transform::from_translation(Vec3::from(spawn.sample.position))
            .with_rotation(heading(spawn.sample.tangent).unwrap_or(Quat::IDENTITY));
        let Some(figure) = spawn_pedestrian(
            &mut commands,
            &mut meshes,
            &mut materials,
            shape,
            paint,
            "WALK",
            at,
            owner,
            live,
        ) else {
            crowd.unspawnable += 1;
            continue;
        };
        commands.entity(figure).insert((
            PedWalk {
                walker: spawn.walker,
                speed,
            },
            PedReact::new(),
        ));
        occupied.push(spawn.sample.position);
        crowd.spawned += 1;
        live += 1;
    }
    crowd.populated = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::{Densities, SessionConditions, SessionCustomization};

    fn config() -> SessionConfig {
        SessionConfig::default()
    }

    #[test]
    fn density_prefers_the_pick_then_the_authored_dial_then_the_default() {
        let mut c = config();
        assert_eq!(resolve_density(&c, None).0, c.densities.pedestrians);
        assert_eq!(resolve_density(&c, Some(0.0)).0, 0.0);
        c.customization = Some(SessionCustomization {
            conditions: SessionConditions::default(),
            densities: Densities {
                traffic: 0.5,
                pedestrians: 0.75,
                cops: 1.0,
            },
            race: None,
        });
        assert_eq!(resolve_density(&c, Some(0.0)).0, 0.75);
    }

    #[test]
    fn hostile_densities_read_as_a_fraction() {
        let c = config();
        assert_eq!(resolve_density(&c, Some(f32::NAN)).0, 0.0);
        assert_eq!(resolve_density(&c, Some(9.0)).0, 1.0);
        assert_eq!(resolve_density(&c, Some(-1.0)).0, 0.0);
    }

    #[test]
    fn a_vertical_tangent_keeps_the_heading() {
        assert!(heading([0.0, 1.0, 0.0]).is_none());
        let face = heading([1.0, 0.0, 0.0]).unwrap();
        assert!((face * Vec3::NEG_Z).abs_diff_eq(Vec3::X, 1e-5));
    }
}
