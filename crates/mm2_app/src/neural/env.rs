//! The training environment: a city with nothing in it but its
//! collision, its props and a field of cars, stepped as fast as the
//! physics allows.
//!
//! A full game session carries audio, HUD, pedestrians, ambient
//! traffic and render bookkeeping and runs at about twice real time.
//! Training needs the driving and nothing else, so [`DrivingEnv`]
//! builds only what a car can touch: the production city importer's
//! colliders and knockable props ([`crate::city::load_city`]), the
//! production vehicle simulation, and the banger systems that let a car
//! knock a lamp post over instead of stopping against it.
//!
//! Every car is an *agent*. Each follows a [`Guide`] built from a real
//! opponent route of the city's races — the same re-pathed driving line
//! ([`crate::opponents::driving_route`]) the game hands its opponents —
//! from a random place along it, in a random car. An agent that
//! finishes, wrecks or gets stuck is replaced at once by a fresh one
//! somewhere else, so the field never empties and one environment
//! yields `agents` transitions per decision.
//!
//! One decision lasts [`EnvConfig::decision_ticks`] physics steps.
//! The reward is progress along the guide, less the speed lost in
//! impacts, less a little for sawing at the wheel; see [`Rewards`].

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_assets::Vfs;
use mm2_content::EventCatalog;
use mm2_game::nav::NavRng;
use mm2_game::{BangerPool, BangerStateChanged, Difficulty, EventTableKind, Session, SessionPhase};
use mm2_vehicle::{
    DriveDirection, Vehicle, VehicleConfig, VehicleInput, VehiclePlugin, VehicleState,
    vehicle_bundle,
};

use super::guide::Guide;
use super::observe::{
    ACT_DIM, CarProfile, Motion, Neighbour, OBS_DIM, RayHit, action_input, observe,
};
use crate::city;
use crate::opponents;

/// Physics rate (Hz) — the game's.
const HZ: f64 = 60.0;

/// How an environment is set up.
#[derive(Debug, Clone)]
pub struct EnvConfig {
    /// City stem (`sf`, `london`).
    pub city: String,
    /// Cars driving at once.
    pub agents: usize,
    /// Seed for every random choice the environment makes.
    pub seed: u64,
    /// Physics steps per decision (3 = 20 Hz).
    pub decision_ticks: u32,
    /// Worker threads for the physics; 1 keeps a process on one core so
    /// many environments can run side by side.
    pub threads: usize,
    /// Reward weights.
    pub rewards: Rewards,
    /// Where and how fresh agents start.
    pub starts: Starts,
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self {
            city: "sf".into(),
            agents: 16,
            seed: 1,
            decision_ticks: 3,
            threads: 1,
            rewards: Rewards::default(),
            starts: Starts::default(),
        }
    }
}

/// Reward weights.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Rewards {
    /// Per metre of progress along the guide.
    pub progress: f32,
    /// Per m/s of horizontal speed lost in one physics step beyond
    /// [`Rewards::impact_free`] — a wall, a car, a lamp post.
    pub impact: f32,
    /// Horizontal speed change (m/s) one physics step may make before
    /// it counts as an impact — past anything tyres and brakes do.
    pub impact_free: f32,
    /// Per squared unit of steering change between decisions.
    pub steer_change: f32,
    /// For an episode that ends in a wreck, a stall or off the route.
    pub failure: f32,
}

impl Default for Rewards {
    fn default() -> Self {
        Self {
            progress: 0.02,
            impact: 0.03,
            impact_free: 1.0,
            steer_change: 0.01,
            failure: -2.0,
        }
    }
}

/// How fresh agents are placed.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Starts {
    /// Share of starts placed just behind or ahead of another agent on
    /// its line, so cars meet.
    pub join: f32,
    /// Share of starts on a route's own starting stretch.
    pub grid: f32,
    /// Share of starts facing any direction at a standstill — practice
    /// at getting back on the line.
    pub astray: f32,
    /// Largest lateral offset from the line (m).
    pub lateral: f32,
    /// Largest heading error (rad) of an ordinary start.
    pub heading: f32,
    /// Largest starting speed (m/s).
    pub speed: f32,
    /// Share of ordinary starts that begin at a standstill.
    pub standing: f32,
}

impl Default for Starts {
    fn default() -> Self {
        Self {
            join: 0.3,
            grid: 0.1,
            astray: 0.05,
            lateral: 2.5,
            heading: 0.2,
            speed: 20.0,
            standing: 0.25,
        }
    }
}

/// Why an episode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum End {
    /// Reached the end of an open guide.
    Finished = 0,
    /// Ran out the episode clock.
    TimeLimit = 1,
    /// Made no progress for [`STALL_SECONDS`].
    Stalled = 2,
    /// Strayed further from the guide than [`OFF_ROUTE`].
    OffRoute = 3,
    /// Lay on its side or roof for [`UPENDED_SECONDS`].
    Upended = 4,
    /// Drove into deadly water or fell out of the world.
    Lost = 5,
}

impl End {
    /// Whether the episode was cut short rather than failed: the value
    /// of carrying on still counts.
    pub fn is_truncation(self) -> bool {
        matches!(self, Self::Finished | Self::TimeLimit)
    }
}

/// Seconds without a metre of new progress before an agent is stalled.
pub const STALL_SECONDS: f32 = 10.0;
/// Distance (m, XZ) from the guide beyond which an agent is off route.
pub const OFF_ROUTE: f32 = 35.0;
/// Height difference (m) from the guide beyond which an agent is off
/// route — under a bridge it should be on, or over a tunnel.
const OFF_ROUTE_HEIGHT: f32 = 12.0;
/// Seconds on its side or roof before an agent is written off.
const UPENDED_SECONDS: f32 = 3.0;
/// Longest episode (s).
const EPISODE_SECONDS: f32 = 150.0;
/// Metres short of an open guide's end at which an agent has finished.
const FINISH_MARGIN: f32 = 4.0;
/// Clearance (m) a fresh start keeps from every other agent.
const START_CLEAR: f32 = 9.0;
/// Attempts at finding a clear, grounded start before giving up on the
/// preferred kind of start.
const START_TRIES: usize = 24;

/// A finished episode, for the trainer's log.
#[derive(Debug, Clone, Copy)]
pub struct Episode {
    pub end: End,
    /// Decisions it lasted.
    pub steps: u32,
    /// Metres of guide covered.
    pub metres: f32,
    /// Sum of rewards.
    pub ret: f32,
    /// Index of the driving line.
    pub line: u32,
    /// Index of the car.
    pub car: u32,
    /// Speed lost to impacts (m/s, summed).
    pub impacts: f32,
}

/// A car that can be driven.
pub struct CarKind {
    pub id: String,
    pub config: VehicleConfig,
    pub profile: CarProfile,
}

/// A line that can be driven.
pub struct Line {
    /// `race3/p/2`-style label: event stem, difficulty, roster slot.
    pub label: String,
    pub guide: Arc<Guide>,
    /// The car the race's roster puts on this line.
    pub car: usize,
    /// The race field this line belongs to: event stem and difficulty
    /// (`race3/p`).
    pub field: String,
    /// Where the roster stages this line's car: position and heading.
    pub grid: (Vec3, f32),
}

struct Slot {
    entity: Entity,
    car: usize,
    line: usize,
    guide: Arc<Guide>,
    /// Arc position on the guide.
    s: f32,
    /// Net metres of progress this episode, and the most it has been.
    odometer: f32,
    best: f32,
    /// Decisions since `best` last grew by a metre.
    since_best: u32,
    /// Physics steps spent upended.
    upended: u32,
    steps: u32,
    ret: f32,
    impacts: f32,
    last_action: [f32; ACT_DIM],
    /// Horizontal velocity after the previous physics step.
    last_velocity: Vec3,
    motion: Motion,
    /// Out of a staged field: its episode ended and nothing replaced it.
    retired: bool,
}

type Probe<'w, 's> = (
    SpatialQuery<'w, 's>,
    Query<'w, 's, &'static RigidBody>,
    Query<'w, 's, &'static ColliderOf>,
    Query<'w, 's, (), With<Vehicle>>,
);

/// One step's results, `agents` rows each.
#[derive(Debug, Default)]
pub struct StepOut {
    /// Observations to act on next, `agents × OBS_DIM`.
    pub obs: Vec<f32>,
    pub reward: Vec<f32>,
    /// 0 running, 1 the episode failed, 2 it was cut short.
    pub done: Vec<u8>,
    /// For each agent with `done == 2`, in agent order: what it saw at
    /// the cut, before its replacement started.
    pub final_obs: Vec<f32>,
    /// Episodes that ended this step.
    pub episodes: Vec<Episode>,
}

/// The environment.
pub struct DrivingEnv {
    app: App,
    config: EnvConfig,
    probe: SystemState<Probe<'static, 'static>>,
    cars: Vec<CarKind>,
    lines: Vec<Line>,
    water: Option<crate::water::CityWater>,
    floor: f32,
    slots: Vec<Slot>,
    rng: NavRng,
    /// Whether the agents are a staged race field ([`Self::stage_field`])
    /// rather than a self-renewing training field.
    staged: bool,
}

/// Why an environment could not be built.
#[derive(Debug)]
pub struct EnvError(pub String);

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EnvError {}

fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

/// Every drivable opponent line of `city`'s checkpoint and circuit
/// races at both difficulties, with the cars their rosters field.
fn load_lines(vfs: &Vfs, city: &str) -> Result<(Vec<CarKind>, Vec<Line>), EnvError> {
    let nav = mm2_content::load_routing_nav_graph(vfs, city)
        .map_err(|e| EnvError(format!("nav graph: {e}")))?
        .graph;
    let catalog = EventCatalog::scan(vfs, city);
    let mut cars: Vec<CarKind> = Vec::new();
    let mut lines = Vec::new();
    for event in &catalog.events {
        if !matches!(
            event.event_ref.table,
            EventTableKind::Checkpoint | EventTableKind::Circuit
        ) || !event.status.is_ready()
        {
            continue;
        }
        for (difficulty, tag) in [(Difficulty::Amateur, 'a'), (Difficulty::Professional, 'p')] {
            let Ok(definition) = mm2_content::race_definition(event, difficulty) else {
                continue;
            };
            let Ok(roster) = mm2_content::opponent_roster(vfs, event, difficulty) else {
                continue;
            };
            let (player_pos, player_yaw) = definition
                .start_slots
                .first()
                .map(|s| (s.position, s.yaw_deg.unwrap_or(0.0).to_radians()))
                .unwrap_or((Vec3::ZERO, 0.0));
            for (i, spec) in roster.entries.iter().enumerate() {
                let Some(route) = &spec.route else { continue };
                let car = match cars.iter().position(|c| c.id == spec.vehicle) {
                    Some(c) => c,
                    None => {
                        let Ok(def) = mm2_content::load_opponent(vfs, &spec.vehicle, 0) else {
                            continue;
                        };
                        cars.push(CarKind {
                            id: spec.vehicle.clone(),
                            profile: CarProfile::of(&def.config),
                            config: def.config,
                        });
                        cars.len() - 1
                    }
                };
                let (pos, yaw) =
                    opponents::spawn_pose(&definition, i, spec, player_pos, player_yaw);
                let driven =
                    opponents::driving_route(route, Some(&nav), &definition.checkpoints, pos, yaw);
                let Some(guide) = Guide::from_route(&driven) else {
                    continue;
                };
                if guide.length() < 200.0 {
                    continue;
                }
                lines.push(Line {
                    label: format!("{}/{tag}/{i}", event.stem),
                    guide: Arc::new(guide),
                    car,
                    field: format!("{}/{tag}", event.stem),
                    grid: (pos, yaw),
                });
            }
        }
    }
    if lines.is_empty() {
        return Err(EnvError(format!("{city}: no drivable opponent lines")));
    }
    Ok((cars, lines))
}

impl DrivingEnv {
    /// Build the world and seat the first field of agents.
    pub fn new(vfs: &Vfs, config: EnvConfig) -> Result<Self, EnvError> {
        let (cars, lines) = load_lines(vfs, &config.city)?;

        let mut session = Session::new();
        session
            .begin(mm2_game::SessionConfig::default())
            .map_err(|e| EnvError(format!("session: {e}")))?;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.set(TaskPoolPlugin {
            task_pool_options: TaskPoolOptions::with_num_threads(config.threads.max(1)),
        }))
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(HZ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / HZ),
        ))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<BangerStateChanged>()
        .init_resource::<BangerPool>()
        .add_systems(FixedUpdate, mm2_game::advance_session_tick)
        .add_systems(
            FixedLast,
            (
                crate::banger::activate_bangers,
                crate::banger::settle_bangers,
            )
                .chain(),
        );
        app.finish();
        app.cleanup();

        let loaded = {
            let world = app.world_mut();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let loaded = {
                let mut commands = Commands::new(&mut queue, world);
                // The importer fills the render-side stores too; only
                // its colliders matter here, so they are thrown away.
                let (mut meshes, mut images, mut materials) = (
                    Assets::<Mesh>::default(),
                    Assets::<Image>::default(),
                    Assets::<StandardMaterial>::default(),
                );
                city::load_city(
                    &mut commands,
                    vfs,
                    &format!("city/{}.psdl", config.city),
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    mm2_game::SessionEntity(session.generation()),
                    &mut session,
                )
                .map_err(|e| EnvError(format!("city: {e:?}")))?
            };
            queue.apply(world);
            loaded
        };
        for phase in [SessionPhase::Ready, SessionPhase::Playing] {
            session
                .transition(phase)
                .map_err(|e| EnvError(format!("session: {e}")))?;
        }
        let gravity = cars
            .first()
            .and_then(|c| c.config.original.as_ref())
            .map_or(9.81, |o| o.gravity);
        app.insert_resource(Gravity(Vec3::NEG_Y * gravity))
            .insert_resource(session);
        // Let the colliders reach the spatial query structures before
        // anything is seated on them.
        for _ in 0..3 {
            app.update();
        }

        let probe = SystemState::new(app.world_mut());
        let mut env = Self {
            app,
            probe,
            cars,
            lines,
            water: loaded.water,
            floor: loaded.floor.map_or(-1.0e4, |f| f.0),
            slots: Vec::new(),
            rng: NavRng::new(config.seed),
            staged: false,
            config,
        };
        for _ in 0..env.config.agents {
            let slot = env.fresh_slot(None);
            env.slots.push(slot);
        }
        Ok(env)
    }

    /// The cars this environment draws from.
    pub fn cars(&self) -> &[CarKind] {
        &self.cars
    }

    /// The lines this environment draws from.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Replace the reward weights from here on.
    pub fn set_rewards(&mut self, rewards: Rewards) {
        self.config.rewards = rewards;
    }

    /// Replace the start distribution from here on.
    pub fn set_starts(&mut self, starts: Starts) {
        self.config.starts = starts;
    }

    /// Number of agents.
    pub fn agents(&self) -> usize {
        self.slots.len()
    }

    fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rng.next_f32()
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.rng.next_u64() % n.max(1) as u64) as usize
    }

    /// Seat a level car at `pos` on static, dry ground — `None` when
    /// there is none under it or another agent is too close.
    fn seat(
        &mut self,
        config: &VehicleConfig,
        pos: Vec3,
        yaw: f32,
        skip: Option<usize>,
    ) -> Option<Vec3> {
        let near = self.slots.iter().enumerate().any(|(i, s)| {
            Some(i) != skip && !s.retired && s.motion.position.distance(pos) < START_CLEAR
        });
        if near {
            return None;
        }
        self.ground(config, pos, yaw)
    }

    /// A level car at `pos` set down on the static, dry ground under it.
    fn ground(&mut self, config: &VehicleConfig, pos: Vec3, yaw: f32) -> Option<Vec3> {
        let (spatial, bodies, colliders, _) = self.probe.get(self.app.world()).ok()?;
        let is_static = |c: Entity| {
            let body = colliders.get(c).map_or(c, |c| c.body);
            !bodies
                .get(body)
                .is_ok_and(|b| b.is_dynamic() || b.is_kinematic())
        };
        opponents::seat_reanchor(
            &spatial,
            &is_static,
            self.water.as_ref(),
            Entity::PLACEHOLDER,
            config,
            pos,
            yaw,
            opponents::reanchor_lift(config),
        )
    }

    /// Choose a line, a car and a pose for a new agent and spawn it.
    /// `replacing` is the slot being refilled, whose own position does
    /// not block the new start.
    fn fresh_slot(&mut self, replacing: Option<usize>) -> Slot {
        let starts = self.config.starts;
        for attempt in 0.. {
            let roll = self.rng.next_f32();
            // Past the retry budget, fall back to the plainest start.
            let plain = attempt >= START_TRIES;
            let others: Vec<usize> = (0..self.slots.len())
                .filter(|&i| Some(i) != replacing)
                .collect();
            let (line, s, astray) = if !plain && roll < starts.join && !others.is_empty() {
                let other = others[self.pick(others.len())];
                let side = if self.rng.next_f32() < 0.5 { -1.0 } else { 1.0 };
                let gap = self.uniform(START_CLEAR, 45.0) * side;
                (self.slots[other].line, self.slots[other].s + gap, false)
            } else if !plain && roll < starts.join + starts.grid {
                (self.pick(self.lines.len()), self.uniform(0.0, 30.0), false)
            } else {
                let line = self.pick(self.lines.len());
                let len = self.lines[line].guide.length();
                let astray = !plain && roll > 1.0 - starts.astray;
                (line, self.uniform(0.0, (len - 60.0).max(1.0)), astray)
            };
            let guide = self.lines[line].guide.clone();
            if !guide.is_closed() && !(0.0..guide.length() - 50.0).contains(&s) {
                continue;
            }
            let s = guide.normalize(s);
            let car = if self.rng.next_f32() < 0.5 {
                self.lines[line].car
            } else {
                self.pick(self.cars.len())
            };
            let tangent = guide.tangent_at(s);
            let right = Vec3::new(-tangent.z, 0.0, tangent.x).normalize_or_zero();
            let lateral = self.uniform(-starts.lateral, starts.lateral);
            let (yaw_error, speed) = if astray {
                (
                    self.uniform(-std::f32::consts::PI, std::f32::consts::PI),
                    0.0,
                )
            } else if self.rng.next_f32() < starts.standing {
                (self.uniform(-starts.heading, starts.heading), 0.0)
            } else {
                (
                    self.uniform(-starts.heading, starts.heading),
                    self.uniform(0.0, starts.speed),
                )
            };
            let yaw = yaw_of(tangent) + yaw_error;
            let config = self.cars[car].config.clone();
            let Some(pos) = self.seat(&config, guide.point_at(s) + right * lateral, yaw, replacing)
            else {
                continue;
            };
            if pos.distance(guide.point_at(s)) > OFF_ROUTE_HEIGHT {
                continue;
            }
            return self.spawn(line, car, pos, yaw, speed, s);
        }
        unreachable!("the start search only leaves by returning")
    }

    /// Put `car` on `line` at `pos` facing `yaw`, rolling at `speed`,
    /// and start its episode. `s` is roughly where on the line it is.
    fn spawn(&mut self, line: usize, car: usize, pos: Vec3, yaw: f32, speed: f32, s: f32) -> Slot {
        let config = &self.cars[car].config;
        let guide = self.lines[line].guide.clone();
        let rotation = Quat::from_rotation_y(yaw);
        let velocity = rotation * Vec3::NEG_Z * speed;
        let world = self.app.world_mut();
        let entity = world
            .spawn((
                vehicle_bundle(config),
                CollidingEntities::default(),
                Position(pos),
                Rotation(rotation),
                Transform::from_translation(pos).with_rotation(rotation),
            ))
            .id();
        world.entity_mut(entity).insert(LinearVelocity(velocity));
        if let Some(mut state) = world.get_mut::<VehicleState>(entity) {
            state.roll_at(config, speed);
        }
        Slot {
            entity,
            car,
            line,
            s: guide.advance(s, pos),
            guide,
            odometer: 0.0,
            best: 0.0,
            since_best: 0,
            upended: 0,
            steps: 0,
            ret: 0.0,
            impacts: 0.0,
            last_action: [0.0; ACT_DIM],
            last_velocity: Vec3::new(velocity.x, 0.0, velocity.z),
            motion: Motion {
                position: pos,
                rotation,
                linear_velocity: velocity,
                angular_velocity: Vec3::ZERO,
                steer_angle: 0.0,
                grounded: 1.0,
                reversing: false,
            },
            retired: false,
        }
    }

    /// Replace the agents with one race field: each of `lines` gets its
    /// roster's car, standing on its grid slot. Agents whose episodes
    /// end are retired, not replaced — step until [`Self::running`]
    /// reaches zero. For evaluating a policy on whole races.
    pub fn stage_field(&mut self, lines: &[usize]) {
        for slot in std::mem::take(&mut self.slots) {
            if !slot.retired {
                self.app.world_mut().despawn(slot.entity);
            }
        }
        self.staged = true;
        for &line in lines {
            let car = self.lines[line].car;
            let (pos, yaw) = self.lines[line].grid;
            let config = self.cars[car].config.clone();
            let pos = self
                .ground(&config, pos, yaw)
                .unwrap_or(pos + Vec3::Y * opponents::reanchor_lift(&config));
            let s = self.lines[line].guide.locate(pos);
            let slot = self.spawn(line, car, pos, yaw, 0.0, s);
            self.slots.push(slot);
        }
    }

    /// Agents still driving.
    pub fn running(&self) -> usize {
        self.slots.iter().filter(|s| !s.retired).count()
    }

    fn read_motion(world: &World, entity: Entity) -> Option<Motion> {
        let state = world.get::<VehicleState>(entity)?;
        let wheels = state.wheels.len().max(1) as f32;
        Some(Motion {
            position: world.get::<Position>(entity)?.0,
            rotation: world.get::<Rotation>(entity)?.0,
            linear_velocity: world.get::<LinearVelocity>(entity)?.0,
            angular_velocity: world.get::<AngularVelocity>(entity)?.0,
            steer_angle: state.steer_angle,
            grounded: state.wheels.iter().filter(|w| w.grounded).count() as f32 / wheels,
            reversing: state.direction == DriveDirection::Reverse,
        })
    }

    /// Observations for every agent, `agents × OBS_DIM`.
    pub fn observations(&mut self) -> Vec<f32> {
        let mut out = vec![0.0; self.slots.len() * OBS_DIM];
        for i in 0..self.slots.len() {
            self.observe_into(i, &mut out[i * OBS_DIM..(i + 1) * OBS_DIM]);
        }
        out
    }

    fn observe_into(&mut self, i: usize, out: &mut [f32]) {
        let neighbours: Vec<Neighbour> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(j, o)| *j != i && !o.retired)
            .map(|(_, o)| {
                let profile = &self.cars[o.car].profile;
                Neighbour {
                    position: o.motion.position,
                    velocity: o.motion.linear_velocity,
                    forward: o.motion.rotation * Vec3::NEG_Z,
                    half_width: profile.half_width,
                    half_length: profile.half_length,
                }
            })
            .collect();
        let slot = &self.slots[i];
        if slot.retired {
            out.fill(0.0);
            return;
        }
        let (spatial, _, _, vehicles) = self
            .probe
            .get(self.app.world())
            .expect("the probe reads only what the physics plugins provide");
        let filter = SpatialQueryFilter::default();
        let cast = |origin: Vec3, dir: Vec3, reach: f32| -> Option<RayHit> {
            let dir = Dir3::new(dir).ok()?;
            spatial
                .cast_ray_predicate(origin, dir, reach, true, &filter, &|e| {
                    !vehicles.contains(e)
                })
                .map(|hit| (hit.distance, hit.normal))
        };
        observe(
            out,
            &self.cars[slot.car].profile,
            &slot.motion,
            &slot.guide,
            slot.s,
            &slot.last_action,
            &neighbours,
            cast,
        );
    }

    /// Apply one action per agent (`agents × ACT_DIM`), advance the
    /// physics one decision and report what came of it.
    pub fn step(&mut self, actions: &[f32]) -> StepOut {
        assert_eq!(actions.len(), self.slots.len() * ACT_DIM);
        let rewards = self.config.rewards;
        let n = self.slots.len();
        let mut steer_change = vec![0.0f32; n];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.retired {
                continue;
            }
            let mut action = [0.0; ACT_DIM];
            action.copy_from_slice(&actions[i * ACT_DIM..(i + 1) * ACT_DIM]);
            for a in &mut action {
                *a = if a.is_finite() {
                    a.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
            steer_change[i] = action[0] - slot.last_action[0];
            slot.last_action = action;
            if let Some(mut input) = self.app.world_mut().get_mut::<VehicleInput>(slot.entity) {
                *input = action_input(&action, 1.0);
            }
        }

        let mut impact = vec![0.0f32; n];
        for _ in 0..self.config.decision_ticks {
            self.app.update();
            let world = self.app.world();
            for (i, slot) in self.slots.iter_mut().enumerate() {
                let Some(v) = world.get::<LinearVelocity>(slot.entity) else {
                    continue;
                };
                let flat = Vec3::new(v.0.x, 0.0, v.0.z);
                let change = flat.distance(slot.last_velocity);
                if change.is_finite() {
                    impact[i] += (change - rewards.impact_free).max(0.0);
                }
                slot.last_velocity = flat;
                let up = world
                    .get::<Rotation>(slot.entity)
                    .map_or(1.0, |r| (r.0 * Vec3::Y).y);
                slot.upended = if up < 0.2 { slot.upended + 1 } else { 0 };
            }
        }

        let decisions_per_second = HZ as f32 / self.config.decision_ticks as f32;
        let mut out = StepOut {
            obs: vec![0.0; n * OBS_DIM],
            reward: vec![0.0; n],
            done: vec![0; n],
            ..Default::default()
        };
        let mut ended: Vec<(usize, End)> = Vec::new();
        for i in 0..n {
            if self.slots[i].retired {
                continue;
            }
            let motion = Self::read_motion(self.app.world(), self.slots[i].entity);
            let slot = &mut self.slots[i];
            let finite = motion.as_ref().is_some_and(|m| {
                m.position.is_finite() && m.linear_velocity.is_finite() && m.rotation.is_finite()
            });
            let mut end = None;
            let mut progress = 0.0;
            if let (Some(motion), true) = (motion, finite) {
                slot.motion = motion;
                let s = slot.guide.advance(slot.s, motion.position);
                progress = slot.guide.delta(slot.s, s);
                slot.s = s;
                slot.odometer += progress;
                if slot.odometer > slot.best + 1.0 {
                    slot.best = slot.odometer;
                    slot.since_best = 0;
                } else {
                    slot.since_best += 1;
                }
                slot.steps += 1;
                let on_line = slot.guide.point_at(s);
                let off = motion.position - on_line;
                if motion.position.y < self.floor
                    || self
                        .water
                        .as_ref()
                        .is_some_and(|w| w.is_deadly(motion.position))
                {
                    end = Some(End::Lost);
                } else if off.x.hypot(off.z) > OFF_ROUTE || off.y.abs() > OFF_ROUTE_HEIGHT {
                    end = Some(End::OffRoute);
                } else if slot.upended as f32 > UPENDED_SECONDS * HZ as f32 {
                    end = Some(End::Upended);
                } else if slot.since_best as f32 > STALL_SECONDS * decisions_per_second {
                    end = Some(End::Stalled);
                } else if slot
                    .guide
                    .remaining(s)
                    .is_some_and(|left| left < FINISH_MARGIN)
                    // A staged field's circuit is one lap long.
                    || (self.staged
                        && slot.guide.is_closed()
                        && slot.odometer >= slot.guide.length())
                {
                    end = Some(End::Finished);
                } else if !self.staged && slot.steps as f32 > EPISODE_SECONDS * decisions_per_second
                {
                    end = Some(End::TimeLimit);
                }
            } else {
                end = Some(End::Lost);
            }
            let mut reward = rewards.progress * progress
                - rewards.impact * impact[i]
                - rewards.steer_change * steer_change[i] * steer_change[i];
            if end.is_some_and(|e| !e.is_truncation()) {
                reward += rewards.failure;
            }
            slot.ret += reward;
            slot.impacts += impact[i];
            out.reward[i] = reward;
            if let Some(end) = end {
                out.done[i] = if end.is_truncation() { 2 } else { 1 };
                ended.push((i, end));
            }
        }

        for &(i, end) in &ended {
            if end.is_truncation() {
                let mut last = vec![0.0; OBS_DIM];
                self.observe_into(i, &mut last);
                out.final_obs.extend_from_slice(&last);
            }
            let slot = &self.slots[i];
            out.episodes.push(Episode {
                end,
                steps: slot.steps,
                metres: slot.odometer,
                ret: slot.ret,
                line: slot.line as u32,
                car: slot.car as u32,
                impacts: slot.impacts,
            });
            let old = slot.entity;
            self.app.world_mut().despawn(old);
            if self.staged {
                self.slots[i].retired = true;
            } else {
                let fresh = self.fresh_slot(Some(i));
                self.slots[i] = fresh;
            }
        }
        for i in 0..n {
            let (a, b) = (i * OBS_DIM, (i + 1) * OBS_DIM);
            // `out.obs` is detached from `self` while it is filled.
            let mut row = [0.0; OBS_DIM];
            self.observe_into(i, &mut row);
            out.obs[a..b].copy_from_slice(&row);
        }
        out
    }

    /// Rigid bodies in the world as (dynamic, kinematic, static,
    /// sleeping) counts and the number of live contact pairs — what the
    /// physics step is paying for.
    pub fn census(&mut self) -> (usize, usize, usize, usize, usize) {
        let world = self.app.world_mut();
        let mut q = world.query::<(&RigidBody, Has<Sleeping>)>();
        let mut c = (0, 0, 0, 0, 0);
        for (body, sleeping) in q.iter(world) {
            match body {
                RigidBody::Dynamic => c.0 += 1,
                RigidBody::Kinematic => c.1 += 1,
                RigidBody::Static => c.2 += 1,
            }
            c.3 += usize::from(sleeping);
        }
        c.4 = world
            .get_resource::<ContactGraph>()
            .map_or(0, |g| g.active_pairs().len());
        c
    }

    /// Position, heading (rad), speed (m/s) and arc position of every
    /// agent — for traces and diagnostics.
    pub fn poses(&self) -> Vec<(Vec3, f32, f32, f32)> {
        self.slots
            .iter()
            .map(|s| {
                let fwd = s.motion.rotation * Vec3::NEG_Z;
                (
                    s.motion.position,
                    yaw_of(fwd),
                    s.motion.linear_velocity.dot(fwd),
                    s.s,
                )
            })
            .collect()
    }
}
