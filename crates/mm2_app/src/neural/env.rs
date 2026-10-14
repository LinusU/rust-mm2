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
//! Around the agents drive *drones*: cars nobody is training, parked
//! in the road or pottering along a line at traffic pace, so there is
//! always something to steer round. And every so often a handful of
//! agents are pulled out and lined up on a race's real starting grid,
//! with a drone where the player would be, because a standing start in
//! a pack is its own skill.
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
    ACT_DIM, CarProfile, Motion, Neighbour, OBS_DIM, RayHit, action_input, guide_bearing, observe,
};
use super::unstick::Unstick;
use crate::city;
use crate::opponents;

/// Physics rate (Hz) — the game's.
const HZ: f64 = 60.0;

/// How an environment is set up.
#[derive(Debug, Clone)]
pub struct EnvConfig {
    /// City stem (`sf`, `london`).
    pub city: String,
    /// Event stem (`race3`, `circuit0`) whose world and lines to load:
    /// its course barricades, parked cars and drawbridges, and the
    /// lines of its two rosters. [`ROAM`] loads the city as it stands
    /// in cruise with lines planned through the road graph between
    /// random places — driving with no authored route, as a Cops &
    /// Robbers car or a pursuer must. `None` loads the cruise city with
    /// every event's lines; props that fence one race's course are then
    /// missing from it.
    pub event: Option<String>,
    /// Cars driving at once.
    pub agents: usize,
    /// Cars in the way: parked or driven at traffic pace by a crude
    /// line follower, never trained.
    pub drones: usize,
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
            event: None,
            agents: 16,
            drones: 4,
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
    /// Per m/s² of cornering beyond [`Rewards::lateral_free`], each
    /// decision — what makes a cautious driver. 0 leaves cornering to
    /// the tyres.
    pub lateral: f32,
    /// Lateral acceleration (m/s²) a driver may use for free.
    pub lateral_free: f32,
    /// Per m/s of speed beyond [`Rewards::speed_free`], each decision.
    pub speed: f32,
    /// Speed (m/s) a driver may carry for free.
    pub speed_free: f32,
}

impl Default for Rewards {
    fn default() -> Self {
        Self {
            progress: 0.02,
            impact: 0.03,
            impact_free: 1.0,
            steer_change: 0.01,
            failure: -2.0,
            lateral: 0.0,
            lateral_free: 0.0,
            speed: 0.0,
            speed_free: 0.0,
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
    /// Decisions between staged grid starts — a group of agents pulled
    /// onto a race's real starting grid together. 0 stages none.
    pub grid_period: u32,
    /// Share of episodes driven at full throttle; the rest draw a
    /// throttle ceiling between `throttle_floor` and 1, as the authored
    /// `maxThrottle` gives the game's opponents.
    pub full_throttle: f32,
    /// Lowest throttle ceiling an episode may draw.
    pub throttle_floor: f32,
    /// Share of starts placed a little way before a spot where an
    /// episode lately came to grief, so the hard places of a city —
    /// one hairpin, one bridge ramp — are practised far more often
    /// than their few metres of a course would be met by chance.
    pub revisit: f32,
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
            grid_period: 500,
            full_throttle: 0.6,
            throttle_floor: 0.55,
            revisit: 0.25,
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
/// How many recent failures' places are kept to start agents before.
const TROUBLE_SPOTS: usize = 96;
/// How far before such a place (m) a revisiting agent starts: room to
/// arrive at speed, as a car that has driven the course would.
const REVISIT_BACK: (f32, f32) = (50.0, 220.0);
/// Seconds on its side or roof before an agent is written off.
const UPENDED_SECONDS: f32 = 3.0;
/// Longest episode (s).
const EPISODE_SECONDS: f32 = 150.0;
/// Metres short of an open guide's end at which an agent has finished
/// — the game's own rule for a route-bound finish, which credits the
/// arc the car has driven and asks nothing of where across the road it
/// arrives. A tighter test here failed a third of the arrivals on a
/// line that swerves to its last row, and taught the driver to fear the
/// finish.
const FINISH_MARGIN: f32 = mm2_game::race::ROUTE_FINISH_RANGE;
/// How far under the city's authored floor (m) a car is out of the
/// world. The floor is the lowest authored geometry and cars drive on
/// it — London's Underground runs along the bottom of the bounds — so
/// the line sits well below.
const FLOOR_SLACK: f32 = 20.0;
/// Most agents one staged grid start takes.
const GRID_MOST: usize = 8;
/// Seconds a drone stays where it is put, at least and at most.
const DRONE_SECONDS: (f32, f32) = (20.0, 60.0);
/// How far ahead of an agent (m) a drone is put, nearest and furthest.
const DRONE_AHEAD: (f32, f32) = (40.0, 160.0);
/// Pace (m/s) a moving drone holds, slowest and fastest.
const DRONE_PACE: (f32, f32) = (6.0, 16.0);
/// Share of drones that stand still.
const DRONE_PARKED: f32 = 0.4;
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
    /// Hardest sustained cornering (m/s², smoothed over about a
    /// quarter second).
    pub lateral: f32,
    /// Highest speed (m/s).
    pub top_speed: f32,
    /// Arc position on the line where it ended.
    pub arc: f32,
    /// World position where it ended.
    pub position: Vec3,
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
    /// The roster's `maxThrottle` for this line's driver.
    pub throttle_cap: f32,
}

/// One race's field: the lines of one event at one difficulty.
pub struct Field {
    /// Event stem and difficulty (`race3/p`).
    pub name: String,
    /// Indices of its lines.
    pub lines: Vec<usize>,
    /// Where the player's car stands on its grid.
    pub player: (Vec3, f32),
}

/// What a drone does.
#[derive(Debug, Clone, Copy)]
struct Drone {
    /// Pace it holds (m/s); 0 stands still on the handbrake.
    pace: f32,
    /// Decisions left before it is put somewhere else.
    left: u32,
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
    /// Smoothed lateral acceleration (m/s²), its peak, and the top speed.
    lateral: f32,
    lateral_peak: f32,
    top_speed: f32,
    last_action: [f32; ACT_DIM],
    /// Horizontal velocity after the previous physics step.
    last_velocity: Vec3,
    motion: Motion,
    /// The most throttle this driver's pedal is given.
    throttle_cap: f32,
    /// The stalled-car watch that backs it out of a dead end.
    unstick: Unstick,
    /// Out of a staged field: its episode ended and nothing replaced it.
    /// For a drone: nowhere to put it just now.
    retired: bool,
    /// Set on a drone.
    drone: Option<Drone>,
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
    fields: Vec<Field>,
    slots: Vec<Slot>,
    drones: Vec<Slot>,
    /// Where the last [`TROUBLE_SPOTS`] failed episodes ended: line
    /// and arc position.
    trouble: std::collections::VecDeque<(usize, f32)>,
    rng: NavRng,
    /// Decisions stepped.
    clock: u64,
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

/// The pseudo-event that trains on road-graph routes in the cruise
/// city (see [`EnvConfig::event`]).
pub const ROAM: &str = "roam";
/// Road-graph lines a roam environment plans.
const ROAM_LINES: usize = 40;
/// Shortest and longest roam line worth driving (m).
const ROAM_LENGTH: (f32, f32) = (500.0, 5000.0);

/// What an environment draws its agents from.
struct Content {
    cars: Vec<CarKind>,
    lines: Vec<Line>,
    fields: Vec<Field>,
    /// Logical paths of the loaded event's `.pathset` overlays.
    pathsets: Vec<String>,
}

/// The stems of `city`'s checkpoint and circuit races that are ready to
/// load — what [`EnvConfig::event`] may name.
pub fn race_events(vfs: &Vfs, city: &str) -> Vec<String> {
    EventCatalog::scan(vfs, city)
        .events
        .iter()
        .filter(|e| {
            matches!(
                e.event_ref.table,
                EventTableKind::Checkpoint | EventTableKind::Circuit
            ) && e.status.is_ready()
        })
        .map(|e| e.stem.clone())
        .collect()
}

/// Every drivable opponent line of `city`'s checkpoint and circuit
/// races (or of `only` one) at both difficulties, with the cars their
/// rosters field.
fn load_lines(vfs: &Vfs, city: &str, only: Option<&str>, seed: u64) -> Result<Content, EnvError> {
    let nav = mm2_content::load_routing_nav_graph(vfs, city)
        .map_err(|e| EnvError(format!("nav graph: {e}")))?
        .graph;
    if only == Some(ROAM) {
        // Every car the city's races field, on lines of our own.
        let cars = load_lines(vfs, city, None, seed)?.cars;
        let lines = roam_lines(&nav, cars.len(), seed);
        if lines.is_empty() {
            return Err(EnvError(format!("{city}: the road graph plans no routes")));
        }
        return Ok(Content {
            cars,
            lines,
            fields: Vec::new(),
            pathsets: Vec::new(),
        });
    }
    let catalog = EventCatalog::scan(vfs, city);
    let mut cars: Vec<CarKind> = Vec::new();
    let mut lines = Vec::new();
    let mut fields = Vec::new();
    let mut pathsets = Vec::new();
    for event in &catalog.events {
        if !matches!(
            event.event_ref.table,
            EventTableKind::Checkpoint | EventTableKind::Circuit
        ) || !event.status.is_ready()
            || only.is_some_and(|stem| stem != event.stem)
        {
            continue;
        }
        if only.is_some() {
            pathsets = event
                .records
                .iter()
                .filter(|r| r.kind == mm2_formats::racefiles::RaceFileKind::Pathset)
                .map(|r| r.logical.clone())
                .collect();
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
            let mut field = Field {
                name: format!("{}/{tag}", event.stem),
                lines: Vec::new(),
                player: (player_pos, player_yaw),
            };
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
                let driven = opponents::driving_route(
                    route,
                    Some(&nav),
                    &[],
                    &definition.checkpoints,
                    pos,
                    yaw,
                );
                let Some(guide) = Guide::from_route_on(&driven, Some(&nav)) else {
                    continue;
                };
                if guide.length() < 200.0 {
                    continue;
                }
                field.lines.push(lines.len());
                lines.push(Line {
                    label: format!("{}/{tag}/{i}", event.stem),
                    guide: Arc::new(guide),
                    car,
                    field: field.name.clone(),
                    grid: (pos, yaw),
                    throttle_cap: spec
                        .drive_params()
                        .max_throttle
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0),
                });
            }
            if !field.lines.is_empty() {
                fields.push(field);
            }
        }
    }
    if lines.is_empty() {
        return Err(EnvError(format!(
            "{city}{}: no drivable opponent lines",
            only.map(|e| format!("/{e}")).unwrap_or_default()
        )));
    }
    Ok(Content {
        cars,
        lines,
        fields,
        pathsets,
    })
}

/// Lines planned through the road graph between random places on it.
fn roam_lines(nav: &mm2_game::NavGraph, cars: usize, seed: u64) -> Vec<Line> {
    let mut rng = NavRng::new(seed ^ 0x726f_616d);
    let places: Vec<Vec3> = nav
        .lanes()
        .iter()
        .filter_map(|lane| lane.vertices().first().copied())
        .map(Vec3::from)
        .collect();
    let mut lines = Vec::new();
    if places.len() < 2 || cars == 0 {
        return lines;
    }
    let mut pick = |n: usize| (rng.next_u64() % n as u64) as usize;
    for _ in 0..ROAM_LINES * 20 {
        if lines.len() >= ROAM_LINES {
            break;
        }
        let (from, to) = (places[pick(places.len())], places[pick(places.len())]);
        let Some(route) = crate::scripted::plan_nav_route(nav, from, to) else {
            continue;
        };
        let Some(guide) = Guide::from_route_on(&route, Some(nav)) else {
            continue;
        };
        if !(ROAM_LENGTH.0..=ROAM_LENGTH.1).contains(&guide.length()) {
            continue;
        }
        let start = guide.point_at(0.0);
        lines.push(Line {
            label: format!("{ROAM}/n/{}", lines.len()),
            car: pick(cars),
            field: format!("{ROAM}/n"),
            grid: (start, yaw_of(guide.tangent_at(0.0))),
            throttle_cap: 1.0,
            guide: Arc::new(guide),
        });
    }
    lines
}

impl DrivingEnv {
    /// Build the world and seat the first field of agents.
    pub fn new(vfs: &Vfs, config: EnvConfig) -> Result<Self, EnvError> {
        let Content {
            cars,
            lines,
            fields,
            pathsets,
        } = load_lines(vfs, &config.city, config.event.as_deref(), config.seed)?;

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
                crate::drawbridge::drive_drawbridges,
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
                let owner = mm2_game::SessionEntity(session.generation());
                let loaded = city::load_city(
                    &mut commands,
                    vfs,
                    &format!("city/{}.psdl", config.city),
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    owner,
                    &mut session,
                )
                .map_err(|e| EnvError(format!("city: {e:?}")))?;
                // What a session adds to the bare city, in its order:
                // the event's course props, then the drawbridge leaves
                // (the road under them is a hole without) and the
                // kerbside parked cars — the event's own files when it
                // ships them, the city's otherwise.
                let stem = config.event.as_deref().filter(|e| *e != ROAM);
                city::spawn_event_pathsets(
                    &mut commands,
                    vfs,
                    &pathsets,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    owner,
                    &mut session,
                );
                crate::drawbridge::spawn_drawbridges(
                    &mut commands,
                    vfs,
                    &config.city,
                    stem,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    owner,
                );
                city::spawn_parked_cars(
                    &mut commands,
                    vfs,
                    &config.city,
                    stem,
                    config.seed,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    owner,
                    &mut session,
                );
                loaded
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
            fields,
            slots: Vec::new(),
            drones: Vec::new(),
            trouble: std::collections::VecDeque::new(),
            rng: NavRng::new(config.seed),
            clock: 0,
            staged: false,
            config,
        };
        env.ground_lines();
        for _ in 0..env.config.agents {
            let slot = env.fresh_slot(None);
            env.slots.push(slot);
        }
        if !env.slots.is_empty() {
            for i in 0..env.config.drones {
                let drone = env.fresh_drone();
                env.drones.push(drone);
                // Spread the first moves out.
                if let Some(d) = &mut env.drones[i].drone {
                    d.left = d.left * (i as u32 + 1) / env.config.drones as u32;
                }
            }
        }
        Ok(env)
    }

    /// Lay every line on the world's ground ([`Guide::regrounded`]),
    /// as the game does for each opponent before its race starts.
    fn ground_lines(&mut self) {
        let Ok((spatial, bodies, colliders, vehicles)) = self.probe.get(self.app.world()) else {
            return;
        };
        let world = self.app.world();
        let filter = SpatialQueryFilter::default();
        let ground = |e: Entity| {
            let body = colliders.get(e).map_or(e, |c| c.body);
            !vehicles.contains(body)
                && bodies.get(body).is_ok_and(|b| !b.is_dynamic())
                && world.get::<mm2_game::Banger>(body).is_none()
        };
        for line in &mut self.lines {
            let grounded = line.guide.regrounded(|p| {
                super::guide::ground_near(p, |from, reach| {
                    spatial
                        .cast_ray_predicate(from, Dir3::NEG_Y, reach, false, &filter, &ground)
                        .map(|hit| hit.distance)
                })
            });
            line.guide = Arc::new(grounded);
        }
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
        }) || self
            .drones
            .iter()
            .any(|d| !d.retired && d.motion.position.distance(pos) < START_CLEAR);
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
            let revisit =
                !plain && !self.trouble.is_empty() && self.rng.next_f32() < starts.revisit;
            let (line, s, astray) = if revisit {
                let spot = self.pick(self.trouble.len());
                let (line, at) = self.trouble[spot];
                let back = self.uniform(REVISIT_BACK.0, REVISIT_BACK.1);
                let s = if self.lines[line].guide.is_closed() {
                    at - back
                } else {
                    (at - back).max(0.0)
                };
                (line, s, false)
            } else if !plain && roll < starts.join && !others.is_empty() {
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
            let mut slot = self.spawn(line, car, pos, yaw, speed, s);
            if self.rng.next_f32() >= starts.full_throttle {
                slot.throttle_cap = self.uniform(starts.throttle_floor, 1.0);
            }
            return slot;
        }
        unreachable!("the start search only leaves by returning")
    }

    /// A drone put down ahead of some agent on that agent's line — or a
    /// retired placeholder when no clear spot turns up, to be tried
    /// again shortly.
    fn fresh_drone(&mut self) -> Slot {
        let per_second = HZ as f32 / self.config.decision_ticks as f32;
        for _ in 0..START_TRIES {
            let agents: Vec<usize> = (0..self.slots.len())
                .filter(|&i| !self.slots[i].retired)
                .collect();
            if agents.is_empty() {
                break;
            }
            let agent = agents[self.pick(agents.len())];
            let line = self.slots[agent].line;
            let guide = self.lines[line].guide.clone();
            let s = self.slots[agent].s + self.uniform(DRONE_AHEAD.0, DRONE_AHEAD.1);
            if !guide.is_closed() && s > guide.length() - 60.0 {
                continue;
            }
            let s = guide.normalize(s);
            let parked = self.rng.next_f32() < DRONE_PARKED;
            let pace = if parked {
                0.0
            } else {
                self.uniform(DRONE_PACE.0, DRONE_PACE.1)
            };
            let tangent = guide.tangent_at(s);
            let right = Vec3::new(-tangent.z, 0.0, tangent.x).normalize_or_zero();
            let lateral = self.uniform(-4.0, 4.0);
            // A parked car may stand at any angle; a moving one points
            // down the road.
            let yaw = yaw_of(tangent)
                + if parked && self.rng.next_f32() < 0.3 {
                    self.uniform(-1.6, 1.6)
                } else {
                    self.uniform(-0.1, 0.1)
                };
            let car = self.pick(self.cars.len());
            let config = self.cars[car].config.clone();
            let Some(pos) = self.seat(&config, guide.point_at(s) + right * lateral, yaw, None)
            else {
                continue;
            };
            let mut slot = self.spawn(line, car, pos, yaw, pace, s);
            slot.drone = Some(Drone {
                pace,
                left: (self.uniform(DRONE_SECONDS.0, DRONE_SECONDS.1) * per_second) as u32,
            });
            return slot;
        }
        // Nowhere just now: a placeholder that retries in a second.
        let mut slot = self.blank_slot();
        slot.drone = Some(Drone {
            pace: 0.0,
            left: per_second as u32,
        });
        slot
    }

    /// A slot with no car in the world behind it.
    fn blank_slot(&self) -> Slot {
        Slot {
            entity: Entity::PLACEHOLDER,
            car: 0,
            line: 0,
            guide: self.lines[0].guide.clone(),
            s: 0.0,
            odometer: 0.0,
            best: 0.0,
            since_best: 0,
            upended: 0,
            steps: 0,
            ret: 0.0,
            impacts: 0.0,
            lateral: 0.0,
            lateral_peak: 0.0,
            top_speed: 0.0,
            last_action: [0.0; ACT_DIM],
            last_velocity: Vec3::ZERO,
            motion: Motion {
                position: Vec3::splat(1.0e9),
                rotation: Quat::IDENTITY,
                linear_velocity: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                steer_angle: 0.0,
                grounded: 0.0,
                reversing: false,
            },
            throttle_cap: 1.0,
            unstick: Unstick::default(),
            retired: true,
            drone: None,
        }
    }

    /// The pedals of a drone: the handbrake for a parked one, else steer
    /// at the line a little way ahead and hold the pace.
    fn drone_input(slot: &Slot, drone: Drone) -> VehicleInput {
        if drone.pace <= 0.0 {
            return VehicleInput {
                handbrake: 1.0,
                ..default()
            };
        }
        let fwd = slot.motion.rotation * Vec3::NEG_Z;
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        let aim = slot.guide.point_at(slot.s + 14.0) - slot.motion.position;
        let bearing = aim.dot(right).atan2(aim.dot(fwd));
        let speed = slot.motion.linear_velocity.dot(fwd);
        VehicleInput {
            steering: (bearing * 2.0).clamp(-1.0, 1.0),
            throttle: if speed < drone.pace { 0.6 } else { 0.0 },
            brake: if speed > drone.pace + 3.0 { 0.5 } else { 0.0 },
            ..default()
        }
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
                // A participant like the game's opponents: the proximity
                // drawbridges open for `Player` cars. Nothing here reads
                // the id, and agents are replaced far more often than
                // the session can mint one.
                mm2_game::Player {
                    id: mm2_game::PlayerId(0),
                    control: mm2_game::PlayerControl::Ai,
                },
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
            lateral: 0.0,
            lateral_peak: 0.0,
            top_speed: 0.0,
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
            throttle_cap: 1.0,
            unstick: Unstick::default(),
            retired: false,
            drone: None,
        }
    }

    /// Replace the agents with one race field: each of `lines` gets its
    /// roster's car, standing on its grid slot. Agents whose episodes
    /// end are retired, not replaced — step until [`Self::running`]
    /// reaches zero. For evaluating a policy on whole races.
    pub fn stage_field(&mut self, lines: &[usize]) {
        let old = std::mem::take(&mut self.slots)
            .into_iter()
            .chain(std::mem::take(&mut self.drones));
        for slot in old {
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
            let mut slot = self.spawn(line, car, pos, yaw, 0.0, s);
            slot.throttle_cap = self.lines[line].throttle_cap;
            self.slots.push(slot);
        }
    }

    /// The race fields this environment can stage.
    pub fn fields(&self) -> &[Field] {
        &self.fields
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
            .map(|(_, o)| o)
            .chain(self.drones.iter().filter(|d| !d.retired))
            .map(|o| {
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
        let water = self.water.as_ref();
        let cast = |origin: Vec3, dir: Vec3, reach: f32| -> Option<RayHit> {
            let unit = Dir3::new(dir).ok()?;
            let hit = spatial.cast_ray_predicate(origin, unit, reach, true, &filter, &|e| {
                !vehicles.contains(e)
            })?;
            // Deadly water is no ground to drive on.
            let point = origin + dir * hit.distance;
            (!water.is_some_and(|w| w.is_deadly(point))).then_some((hit.distance, hit.normal))
        };
        observe(
            out,
            &self.cars[slot.car].profile,
            &slot.motion,
            &slot.guide,
            slot.s,
            &slot.last_action,
            &neighbours,
            slot.throttle_cap,
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
            // A stalled car is backed out, as in the game; what was
            // applied is what the next observation reports.
            let bearing = guide_bearing(&slot.motion, &slot.guide, slot.s);
            let backing = slot.unstick.step(slot.motion.position, &action, bearing);
            let action = backing.unwrap_or(action);
            if backing.is_none() {
                steer_change[i] = action[0] - slot.last_action[0];
            }
            slot.last_action = action;
            if let Some(mut input) = self.app.world_mut().get_mut::<VehicleInput>(slot.entity) {
                *input = action_input(&action, slot.throttle_cap);
            }
        }
        for slot in &self.drones {
            if let (false, Some(drone)) = (slot.retired, slot.drone)
                && let Some(mut input) = self.app.world_mut().get_mut::<VehicleInput>(slot.entity)
            {
                *input = Self::drone_input(slot, drone);
            }
        }
        // A staged grid start falls due: the longest-running agents are
        // cut short this step and lined up on one race's grid.
        self.clock += 1;
        let period = self.config.starts.grid_period as u64;
        let mut grid: Vec<(usize, usize)> = Vec::new();
        if !self.staged
            && period > 0
            && self.clock.is_multiple_of(period)
            && !self.fields.is_empty()
        {
            let field = self.pick(self.fields.len());
            let mut oldest: Vec<usize> = (0..n).collect();
            oldest.sort_by_key(|&i| std::cmp::Reverse(self.slots[i].steps));
            let take = self.fields[field].lines.len().min(GRID_MOST).min(n / 3);
            grid = oldest
                .into_iter()
                .zip(self.fields[field].lines.iter().copied())
                .take(take)
                .collect();
            if !grid.is_empty() && !self.drones.is_empty() {
                self.stage_player(field);
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
            let (mut cornering, mut speed) = (0.0, 0.0);
            if let (Some(motion), true) = (motion, finite) {
                let moved = motion.position.distance(slot.motion.position);
                slot.motion = motion;
                // Cornering as speed × yaw rate, on the ground only — a
                // car spinning through the air is not cornering.
                let fwd = motion.rotation * Vec3::NEG_Z;
                speed = motion.linear_velocity.dot(fwd).abs();
                cornering = if motion.grounded > 0.0 {
                    (speed * motion.angular_velocity.y).abs()
                } else {
                    0.0
                };
                slot.lateral += 0.2 * (cornering - slot.lateral);
                slot.lateral_peak = slot.lateral_peak.max(slot.lateral);
                slot.top_speed = slot.top_speed.max(speed);
                // The car's place moves only as far as the car did, as
                // in the game.
                let s = slot.guide.follow(slot.s, moved, motion.position);
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
                if motion.position.y < self.floor - FLOOR_SLACK
                    || self
                        .water
                        .as_ref()
                        .is_some_and(|w| w.is_deadly(motion.position))
                {
                    end = Some(End::Lost);
                } else if off.x.hypot(off.z) > OFF_ROUTE
                    // A car in the air over its line is jumping, not
                    // lost: a San Francisco crest at speed puts one a
                    // long way above the road.
                    || (off.y.abs() > OFF_ROUTE_HEIGHT && (off.y < 0.0 || motion.grounded > 0.0))
                {
                    end = Some(End::OffRoute);
                } else if slot.upended as f32 > UPENDED_SECONDS * HZ as f32 {
                    end = Some(End::Upended);
                } else if slot.since_best as f32 > STALL_SECONDS * decisions_per_second {
                    end = Some(End::Stalled);
                } else if (slot
                    .guide
                    .remaining(s)
                    .is_some_and(|left| left < FINISH_MARGIN)
                    && off.x.hypot(off.z) < super::guide::ON_LINE)
                    // A staged field's circuit is one lap long.
                    || (self.staged
                        && slot.guide.is_closed()
                        && slot.odometer >= slot.guide.length())
                {
                    end = Some(End::Finished);
                } else if !self.staged
                    && (slot.steps as f32 > EPISODE_SECONDS * decisions_per_second
                        || grid.iter().any(|&(agent, _)| agent == i))
                {
                    end = Some(End::TimeLimit);
                }
            } else {
                end = Some(End::Lost);
            }
            let mut reward = rewards.progress * progress
                - rewards.impact * impact[i]
                - rewards.steer_change * steer_change[i] * steer_change[i]
                - rewards.lateral * (cornering - rewards.lateral_free).max(0.0)
                - rewards.speed * (speed - rewards.speed_free).max(0.0);
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
            if !end.is_truncation() {
                if self.trouble.len() == TROUBLE_SPOTS {
                    self.trouble.pop_front();
                }
                self.trouble
                    .push_back((self.slots[i].line, self.slots[i].s));
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
                lateral: slot.lateral_peak,
                top_speed: slot.top_speed,
                arc: slot.s,
                position: slot.motion.position,
            });
            let old = slot.entity;
            self.app.world_mut().despawn(old);
            if self.staged {
                self.slots[i].retired = true;
            } else if let Some(&(_, line)) = grid.iter().find(|&&(agent, _)| agent == i) {
                // Its place on the staged grid, in the roster's car at
                // the roster's throttle.
                let car = self.lines[line].car;
                let (pos, yaw) = self.lines[line].grid;
                let config = self.cars[car].config.clone();
                let pos = self
                    .ground(&config, pos, yaw)
                    .unwrap_or(pos + Vec3::Y * opponents::reanchor_lift(&config));
                let s = self.lines[line].guide.locate(pos);
                let mut fresh = self.spawn(line, car, pos, yaw, 0.0, s);
                fresh.throttle_cap = self.lines[line].throttle_cap;
                self.slots[i] = fresh;
            } else {
                let fresh = self.fresh_slot(Some(i));
                self.slots[i] = fresh;
            }
        }
        self.step_drones();
        for i in 0..n {
            let (a, b) = (i * OBS_DIM, (i + 1) * OBS_DIM);
            // `out.obs` is detached from `self` while it is filled.
            let mut row = [0.0; OBS_DIM];
            self.observe_into(i, &mut row);
            out.obs[a..b].copy_from_slice(&row);
        }
        out
    }

    /// Follow the drones through the step just taken and move on any
    /// whose time is up, that wrecked, or that ran out of line.
    fn step_drones(&mut self) {
        for i in 0..self.drones.len() {
            let Some(mut drone) = self.drones[i].drone else {
                continue;
            };
            drone.left = drone.left.saturating_sub(1);
            let mut spent = drone.left == 0;
            if !self.drones[i].retired {
                match Self::read_motion(self.app.world(), self.drones[i].entity) {
                    Some(m) if m.position.is_finite() => {
                        let slot = &mut self.drones[i];
                        slot.motion = m;
                        slot.s = slot.guide.advance(slot.s, m.position);
                        let off = m.position - slot.guide.point_at(slot.s);
                        spent |= (m.rotation * Vec3::Y).y < 0.2
                            || m.position.y < self.floor - FLOOR_SLACK
                            || off.x.hypot(off.z) > OFF_ROUTE
                            || slot.guide.remaining(slot.s).is_some_and(|left| left < 30.0)
                            || self.water.as_ref().is_some_and(|w| w.is_deadly(m.position));
                    }
                    _ => spent = true,
                }
            }
            if spent {
                if !self.drones[i].retired {
                    let old = self.drones[i].entity;
                    self.app.world_mut().despawn(old);
                    self.drones[i].retired = true;
                }
                let fresh = self.fresh_drone();
                self.drones[i] = fresh;
            } else {
                self.drones[i].drone = Some(drone);
            }
        }
    }

    /// Put a drone where the player's car stands on `field`'s grid:
    /// parked there, or setting off down one of the field's lines.
    fn stage_player(&mut self, field: usize) {
        let per_second = HZ as f32 / self.config.decision_ticks as f32;
        let (pos, yaw) = self.fields[field].player;
        let lines = self.fields[field].lines.clone();
        let line = lines[self.pick(lines.len())];
        let car = self.pick(self.cars.len());
        let config = self.cars[car].config.clone();
        let Some(pos) = self.ground(&config, pos, yaw) else {
            return;
        };
        if !self.drones[0].retired {
            let old = self.drones[0].entity;
            self.app.world_mut().despawn(old);
        }
        let pace = if self.rng.next_f32() < 0.5 {
            0.0
        } else {
            self.uniform(DRONE_PACE.0, DRONE_PACE.1)
        };
        let s = self.lines[line].guide.locate(pos);
        let mut slot = self.spawn(line, car, pos, yaw, 0.0, s);
        slot.drone = Some(Drone {
            pace,
            left: (DRONE_SECONDS.1 * per_second) as u32,
        });
        self.drones[0] = slot;
    }

    /// Walk `line` in `step`-metre strides and report where it cannot
    /// be driven as drawn: no ground under it (`"void"`), deadly water
    /// (`"water"`), ground more than 3 m off the line's own height
    /// (`"level"`), or a stride that passes through something solid a
    /// metre above the road (`"wall"`). Each entry is the arc position,
    /// the point and what is wrong there. A line can be fine to drive
    /// and still be flagged — a jump has no ground under it — so this
    /// is a list of places to look, not of faults.
    pub fn audit_line(&mut self, line: usize, step: f32) -> Vec<(f32, Vec3, &'static str)> {
        let guide = self.lines[line].guide.clone();
        let mut found = Vec::new();
        let Ok((spatial, bodies, colliders, vehicles)) = self.probe.get(self.app.world()) else {
            return found;
        };
        let filter = SpatialQueryFilter::default();
        let world = self.app.world();
        // Ground is anything that is not a car or a loose prop — a
        // drawbridge leaf is kinematic and is road. A wall is static
        // and not a prop a car knocks over.
        let body_of = |e: Entity| colliders.get(e).map_or(e, |c| c.body);
        let ground = |e: Entity| {
            !vehicles.contains(e) && bodies.get(body_of(e)).is_ok_and(|b| !b.is_dynamic())
        };
        let solid = |e: Entity| {
            !vehicles.contains(e)
                && bodies.get(body_of(e)).is_ok_and(|b| b.is_static())
                && world.get::<mm2_game::Banger>(body_of(e)).is_none()
        };
        let ground_at = |p: Vec3| -> Option<Vec3> {
            spatial
                .cast_ray_predicate(
                    p + Vec3::Y * 4.0,
                    Dir3::NEG_Y,
                    30.0,
                    false,
                    &filter,
                    &ground,
                )
                .map(|hit| p + Vec3::Y * (4.0 - hit.distance))
        };
        let mut s = 0.0;
        let mut last: Option<Vec3> = None;
        while s < guide.length() {
            let p = guide.point_at(s);
            match ground_at(p) {
                None => {
                    found.push((s, p, "void"));
                    last = None;
                }
                Some(g) if self.water.as_ref().is_some_and(|w| w.is_deadly(g)) => {
                    found.push((s, p, "water"));
                    last = None;
                }
                Some(g) => {
                    if (g.y - p.y).abs() > 3.0 {
                        found.push((s, p, "level"));
                    }
                    let here = g + Vec3::Y;
                    if let Some(from) = last
                        && let Ok(dir) = Dir3::new(here - from)
                        && let Some(hit) = spatial.cast_ray_predicate(
                            from,
                            dir,
                            from.distance(here),
                            true,
                            &filter,
                            &solid,
                        )
                        && hit.normal.y < 0.7
                    {
                        found.push((s, p, "wall"));
                    }
                    last = Some(here);
                }
            }
            s += step;
        }
        found
    }

    /// Heights of whatever a car could stand on or run into over a
    /// square of the world: `cells × cells` samples `step` metres apart
    /// centred on `centre`, each the height of the first non-vehicle
    /// surface below `centre.y + above`, or NaN where there is none.
    /// Row-major from the low-X, low-Z corner. For drawing a trouble
    /// spot from above.
    pub fn height_map(&mut self, centre: Vec3, step: f32, cells: usize, above: f32) -> Vec<f32> {
        let mut out = vec![f32::NAN; cells * cells];
        let Ok((spatial, _, _, vehicles)) = self.probe.get(self.app.world()) else {
            return out;
        };
        let filter = SpatialQueryFilter::default();
        let half = (cells as f32 - 1.0) * 0.5 * step;
        let top = centre.y + above;
        for row in 0..cells {
            for col in 0..cells {
                let from = Vec3::new(
                    centre.x - half + col as f32 * step,
                    top,
                    centre.z - half + row as f32 * step,
                );
                if let Some(hit) = spatial.cast_ray_predicate(
                    from,
                    Dir3::NEG_Y,
                    above + 60.0,
                    false,
                    &filter,
                    &|e| !vehicles.contains(e),
                ) {
                    out[row * cells + col] = top - hit.distance;
                }
            }
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
