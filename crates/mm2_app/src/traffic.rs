//! F10-A.2 ambient traffic runtime — the consumer of the seeded spawn
//! plan F10-A.1 landed.
//!
//! `load_session_world` calls [`load_ambient_traffic`] once per city
//! session: it merges the city's `city/<stem>.aimap` ambient config
//! with the event aimap's overrides (`mm2_content::ambient_setup`),
//! builds the navigation graph, runs `plan_ambient` under the session
//! seed and spawns the planned cars as session-owned entities on real
//! `va_*` assets resolved through the VFS. `drive_ambient` then walks
//! each car's [`LaneCursor`] along the authored network at its target
//! speed — seeded legal-exit choices at intersections, rank-preserving
//! lane transfers — and `maintain_ambient` recycles cars that run out
//! of road or leave every player's bubble, respawning to the density
//! target inside the union of player interest areas (F10 spec req 2 —
//! designed composition, see `maintain_ambient`).
//!
//! Deliberately partial scope, per the F10 spec: ambient cars are
//! **kinematic** lane followers — `AiVehicleData` authors no drivetrain
//! (its own doc says it is not `vehCarSim` input), so the car is a
//! rigid hull moved by lane sampling, which also keeps every pose
//! finite and on-road by construction. The kinematic collider blocks
//! the player coherently, and F10-B.1 adds the obstruction response: a
//! forward corridor senses every `Player` participant and other
//! ambient cars, and the follow law brakes to a bounded stop behind
//! the nearest blocker — queueing, never shoving — and resumes when
//! it clears. Lane-change passing stays open (F10-B/F10-C remainder),
//! and dynamic car-vs-player crash fidelity (AC03) is not claimed —
//! the follower stops short, nothing more.
//!
//! F10-B.2 adds the junction controller: the authored `vehicleRule`
//! on each road end (BAI) gates the lane transfer — `NeverStop`
//! approaches flow, `TrafficLight` approaches wait for their road's
//! phase in a deterministic per-junction signal cycle, `StopSign`
//! approaches queue first-come-first-served through a registered
//! dwell, and `AlwaysStop` never opens (the two last are unused on
//! retail data). "At the stop line" is a small designed tolerance —
//! the brake ramp decays the remaining distance geometrically, so an
//! f32 cursor asymptotes a hair short of an exact zero — keying the
//! FCFS registration, the held diagnostic and the last-step clamp.
//! A transfer that would land inside a live car or
//! participant reverts to the lane end and retries — cars never
//! materialise inside a junction queue. Signal timing, dwells,
//! stop-line inset and entry clearance are designed values (the
//! original's are unverified, UNK-12).
//!
//! F10-B.3 adds the bounded stuck recovery the spec's "obstruction
//! response and stuck recovery" requirement asks for: each car runs a
//! displacement window (`StuckWindow`/`StuckPolicy`, designed — the
//! original's handling is unverified) and a car that cannot make
//! `min_displacement` of progress for `window_ticks` despawns into
//! the pool `maintain_ambient` refills from. That is a bounded
//! recovery — the window sits far beyond the worst legitimate wait a
//! signal or draining queue can impose, and the car leaves the world
//! rather than teleporting through whatever pens it.
//!
//! F10-B.4 adds occupied-space rejection at spawn (F10-AC04's spawn
//! leg): `draw_spawn` rejects a sample whose tangent-aligned
//! exclusion box (`SpawnPolicy::spawn_clearance` longitudinal,
//! `spawn_half_width` lateral, `spawn_max_rise` vertical — designed
//! values, UNK-12) touches a live ambient car, a `Player`
//! participant, or a spot an earlier draw claimed this tick/plan.
//! The planner and the maintainer share the check, so neither the
//! initial plan nor a refill can stack a car on occupied road — while
//! an adjacent lane or an overpass stays legitimately spawnable.
//!
//! F10-B.5 adds the junction-box yield (F10-AC02's right-of-way
//! leg): the authored rule decides whose *turn* it is, not whether
//! the box is passable, so an admitted approach — a green member or
//! an FCFS head — still holds while the junction zone
//! (`junction_zone`, authored centre + member-end radius) contains a
//! vehicle not bound for that junction: a car that turned in and has
//! not cleared, a crossing flow's occupant, or a participant parked
//! inside. Cars bound for the junction are excluded — a car waiting
//! at its own stop line is not "inside the box", or two competing
//! approaches would hold each other forever. `NeverStop`/unruled
//! approaches keep their documented free flow; their overlaps remain
//! covered by the corridor sense and the landing clearance.
//!
//! F10-B.11 closes the same-tick hole in that yield: the blocker and
//! bound-for sets are frame-start snapshots, so a second eligible car
//! evaluated after another car's commit in the same tick used to read
//! the box empty and take the interior alongside it. A commit now
//! claims its junction in the controller ([`Junctions::commit`]) for
//! the rest of the tick — gated approaches read the claim as an
//! occupied box — and `advance_tick` sheds it once the car is
//! physically inside and the snapshot sees it. A rolled-back commit
//! (a rejected landing) releases its own claim immediately — never
//! another car's: on a mixed-rule junction a free-flow car can roll
//! back over a gated car's live claim in the same tick. Unruled
//! approaches never consult the record — same authored free flow.
//!
//! F10-B.6 adds the kinematic→dynamic handover (F10-AC03's collision
//! leg — the spec's "transition to dynamic behaviour without
//! duplicating bodies or injecting extreme energy"). [`knock_ambient`]
//! is a third consumer of the solver's `CollisionStart` stream: a
//! lane-following car struck by a contact whose impulse estimate
//! (approach speed × striker mass — banger activation weighs kinetic
//! energy instead; this gate is a designed N·s threshold) reaches
//! `KnockPolicy::min_impulse` flips to
//! `RigidBody::Dynamic` on the same entity — same hull, same velocity
//! — and leaves the lane system: `drive_ambient` never re-poses it,
//! the FCFS queue releases it, and as a `Knocked` car it no longer
//! counts as "bound for" its approach junction, so a wreck resting
//! inside the box holds the yielded approaches like any other
//! occupant. The recovery is the ordinary distance recycler — the
//! wreck stays a physical obstacle until the player's bubble collects
//! it (documented approximation; the original's crash behaviour is
//! unverified, UNK-12).
//!
//! F10-B.12 makes the handover itself momentum-conserving (same
//! implementation choice F04-C.4 took for bangers): the solver
//! already answered the contact against the car as an infinite-mass
//! body, so the committed flip replays it as a two-body transfer —
//! the wreck leaves at the mass-correct `(1+e)·v·μ` launch through
//! its contact lever, and the striker's share rewrites the wall
//! impulse it was charged this step. One exchange, one charge: no
//! flat approach-speed kick compounding on top of the wall response.
//!
//! F10-B.13 bounds the wreck the flip leaves behind and names its
//! striker: the spawned body now carries the solver-side
//! `MaxLinearSpeed`/`MaxAngularSpeed` every banger body gets
//! (non-binding on a lane follower, binding once it is dynamic) and
//! the contact-lever spin write clamps at `MAX_BANGER_ANGULAR_SPEED`,
//! so a transient spike cannot leave a wreck whose spin inflates a
//! later contact's `normal_speed` reading. Each handover also counts
//! its striker's class — `Player` participant, ambient car, or
//! anything else — into `kns=` on the smoke record, so a soak can say
//! whether a *participant* ever struck a car (AC03's damage leg).
//! F10-B.14 puts the same write-side spin bound on the striker's
//! correction inside the shared `write_striker_correction` — the
//! banger path's strikers included — and covers the same-tick pileup
//! edge: two strikers reaching one lane car in a single drain produce
//! one flip, not two.
//!
//! F10-B.9 drives the junction interior (operator report 4 item 1):
//! BAI lane curves stop at each road's junction boundary, so a lane
//! transfer that re-posed the car on the exit lane's start read on
//! screen as a teleport across the box. [`advance_lane_cursor`] now
//! commits a generated [`mm2_game::Crossing`] — a cubic Hermite
//! between the lane-end pose and the landing lane's start pose,
//! resampled to a dense polyline — and the car traverses it under the
//! corner cap and the corridor sense like any other driven stretch.
//! Committing releases the approach's FCFS slot; the box-yield then
//! holds the next car until this one physically clears the junction
//! (a mid-crossing car is an occupant, not an approach). A
//! `traffic.jumps` watchdog counts pose discontinuities a lane
//! follower could not have driven — the measure the report asked for,
//! where spawn/recycle/stuck counts structurally cannot see a
//! teleport. The crossing geometry is generated, not authored — an
//! implementation choice under UNK-12.
//!
//! F10-B.7 renders the authored traffic signals: every BAI road end
//! carries a `trafficLightOrigin`/`trafficLightAxis` pair (R3: lights
//! render only when the origin is nonzero), surfaced on the nav graph
//! both per-approach (`NavArc::exit_light`) and as the full authored
//! head list (`NavGraph::signals` — many heads govern ends no vehicle
//! arc uses, and the original draws them anyway). At load, one
//! session-owned [`TrafficSignal`] indicator spawns at every authored
//! origin within `SIGNAL_MAX_DISTANCE` of its junction (designed
//! sanity bound — retail authors a few wild outliers, counted in
//! `signals_dropped`); `drive_signals` then sets each indicator's
//! aspect off the authoritative [`Junctions`] controller every tick —
//! green while the approach's road holds the signal phase, red out of
//! phase and in the all-red clearance, constant green on free-flow
//! ends, the stop aspect on stop-signed ends. The mapping is a
//! designed presentation over authored positions and rules; the
//! original's signal visuals and exact state semantics are unverified
//! (UNK-12).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::bai::VehicleRule;
use mm2_formats::veh::AiVehicleData;
use mm2_game::{
    AmbientRoster, AmbientSpec, AuthorityRole, FollowPolicy, JunctionGate, JunctionPolicy,
    Junctions, KnockPolicy, LaneAdvance, LaneCursor, LaneId, MAX_BANGER_ANGULAR_SPEED,
    MAX_BANGER_LINEAR_SPEED, NavGraph, NavIssue, NavOverrides, NavRng, ObjectIdentity, Player,
    Session, SessionConfig, SessionEntity, SessionPhase, SignalAspect, SpawnDirective, SpawnDraw,
    SpawnPolicy, StuckPolicy, StuckWindow, WorldMode, advance_lane_cursor, corridor_gap,
    draw_spawn, eligible_lanes, follow_speed, inside_junction_zone, junction_speed, junction_zone,
    plan_ambient, within_interest,
};
use tracing::{debug, info, warn};

use crate::car_visual::spawn_vehicle_model;
use crate::contracts::{
    StruckMut, Transfer, angular_share, deepest_contact, impulse_estimate, resolve_transfer,
    write_striker_correction,
};

/// A `va_*` class's runtime assets: render model plus the collider the
/// bound (or, failing that, the tuning's authored `Size`) describes,
/// and the resolved ambient engine table (F07-B.6) when the class has
/// one — `None` keeps the class's cars noteless like an absent record.
struct AmbientClass {
    model: mm2_content::VehicleModel,
    collider: Collider,
    /// `aud/cardata/ambient/<id>_engine.csv` (or the authored default),
    /// resolved to mix parameters. `None` on absent/malformed tables or
    /// a sentinel sample — authored silence, never a spawn blocker.
    audio: Option<mm2_game::AmbientEngineSpec>,
}

/// Session-scoped ambient-traffic state: the merged roster, the nav
/// graph the cars follow, the seeded draw stream the initial plan and
/// the respawner share, and the accounting the smoke record reports.
/// Inserted by `load_session_world` for city sessions that resolve a
/// roster; removed by session teardown so a restart replans cleanly.
#[derive(Resource)]
pub struct AmbientTraffic {
    graph: NavGraph,
    overrides: NavOverrides,
    roster: AmbientRoster,
    eligible: Vec<LaneId>,
    rng: NavRng,
    /// The spawn/recycle bounds this session runs under — `pub` so
    /// tests and evidence runs can bind different distances.
    pub policy: SpawnPolicy,
    /// Simultaneous population bound for this session's density
    /// (`density × policy.max_active`, the plan's `target`).
    pub target: usize,
    /// Per-class assets, loaded lazily on first successful draw —
    /// `None` caches a failed load so it is not retried every tick.
    classes: HashMap<usize, Option<Arc<AmbientClass>>>,
    /// Cars ever spawned (initial plan + respawns).
    pub spawned: usize,
    /// Cars despawned past `policy.recycle_distance`.
    pub recycled: usize,
    /// Cars that ran out of road (no legal open exit).
    pub dead_ends: usize,
    /// Draws that selected an unspawnable class or fell off an open
    /// weight table — the authored band stood.
    pub unspawnable: usize,
    /// Initial-plan directives dropped outside the spawn annulus.
    pub dropped: usize,
    /// Cars currently held at a stop behind a corridor blocker —
    /// refreshed every `drive_ambient` tick.
    pub queued: usize,
    /// Cars standing at a closed junction gate's stop line (signal
    /// red, stop-sign dwell/queue, `AlwaysStop`) — refreshed every
    /// `drive_ambient` tick.
    pub junction_held: usize,
    /// Cars despawned by the bounded stuck recovery (F10-B.3) — the
    /// "stuck-car outcomes" F10-AC05 asks the record to report.
    pub stuck: usize,
    /// The displacement-window policy the recovery runs under —
    /// `pub` so evidence runs and tests can bind a shorter window.
    pub stuck_policy: StuckPolicy,
    /// Cars handed to dynamic bodies by a qualifying impact
    /// (F10-B.6) — cumulative; the wrecks themselves stay active
    /// population until the distance recycler collects them.
    pub knocked: usize,
    /// Handovers whose striker was a `Player` participant — the local
    /// driver or an AI opponent (F10-B.13). The record names the
    /// class because AC03's collision leg is about *participant*
    /// hits specifically; `knocked` alone cannot say who struck.
    pub knocked_by_participant: usize,
    /// Handovers whose striker was another ambient car — a lane
    /// follower or an already-knocked wreck sliding into a queue.
    pub knocked_by_ambient: usize,
    /// Handovers whose striker was anything else — a banger body, a
    /// break fragment, a world-side body.
    pub knocked_by_other: usize,
    /// The impulse threshold the handover runs under — `pub` so
    /// evidence runs and tests can bind a different gate.
    pub knock_policy: KnockPolicy,
    /// Junction crossings committed this session (F10-B.9) — the
    /// generated interior path is exercised iff this is nonzero.
    pub crossings: usize,
    /// Per-tick pose discontinuities a lane follower could not have
    /// driven — the teleport the aggregate counters cannot see
    /// (operator report 4 item 1). Must stay zero: every move a
    /// follower makes is `speed × dt` along its lane or crossing.
    pub jumps: usize,
    /// The per-junction right-of-way/signal controller (F10-B.2) —
    /// session-scoped like the plan it polices.
    pub junctions: Junctions,
    /// Authored signal indicators spawned at load (F10-B.7).
    pub signals: usize,
    /// Authored signal origins skipped by the `SIGNAL_MAX_DISTANCE`
    /// sanity bound — retail authors a few wild outliers; counted,
    /// not hidden.
    pub signals_dropped: usize,
    /// Shared lamp mesh and aspect materials for the signal
    /// indicators — one allocation serves every head.
    signal_assets: SignalAssets,
    /// Planner/setup problems, reported honestly.
    pub issues: Vec<String>,
}

/// How far from its junction's occupancy-zone centre an authored
/// signal origin may sit before it is treated as junk data and
/// dropped (m) — retail's farthest legitimate origin is ~40 m, and a
/// handful of authored outliers reach hundreds of metres (counted in
/// `signals_dropped`). Designed bound, UNK-12.
const SIGNAL_MAX_DISTANCE: f32 = 60.0;

/// Indicator lamp radius (m) — a designed presentation size; the
/// authored head's dimensions are not part of the BAI record.
const SIGNAL_LAMP_RADIUS: f32 = 0.45;

/// The shared lamp mesh and per-aspect materials every signal
/// indicator draws with.
struct SignalAssets {
    lamp: Handle<Mesh>,
    green: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    amber: Handle<StandardMaterial>,
}

impl SignalAssets {
    fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        // Unlit vivid colours — readable in daylight without lighting
        // support, the same convention the checkpoint markers use.
        let mut mat = |r: f32, g: f32, b: f32| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(r, g, b),
                unlit: true,
                ..default()
            })
        };
        Self {
            lamp: meshes.add(Sphere::new(SIGNAL_LAMP_RADIUS)),
            green: mat(0.15, 0.95, 0.25),
            red: mat(0.95, 0.12, 0.08),
            amber: mat(0.95, 0.65, 0.10),
        }
    }

    fn material(&self, aspect: SignalAspect) -> &Handle<StandardMaterial> {
        match aspect {
            SignalAspect::Green => &self.green,
            SignalAspect::Red => &self.red,
            SignalAspect::Stop => &self.amber,
        }
    }
}

/// One authored traffic signal (F10-B.7): a session-owned indicator
/// at a BAI `trafficLightOrigin`, its aspect driven by the junction
/// controller — green while the approach's road holds the phase, red
/// out of phase, constant green on free-flow ends, the stop aspect on
/// stop-signed ends. Render-only: no collider, no physics — the same
/// convention the checkpoint markers use.
#[derive(Component)]
pub struct TrafficSignal {
    /// Junction the signal's approach enters.
    pub junction: u16,
    /// Member road the signal governs.
    pub road: u16,
    /// The approach's authored rule — selects the aspect.
    pub rule: Option<VehicleRule>,
    /// The aspect currently applied — the update skips entities whose
    /// state did not change.
    pub aspect: SignalAspect,
}

impl AmbientTraffic {
    /// The navigation graph the cars follow — exposed for diagnostics
    /// and tests that resolve a [`LaneCursor`] to a world pose.
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }
}

/// How an ambient car is moving: a kinematic lane follower, or a
/// dynamic body after a qualifying impact (F10-B.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientDrive {
    /// Lane-following — `drive_ambient` owns the pose every tick.
    #[default]
    Lane,
    /// Knocked — Avian owns the pose. The car is an obstacle, not a
    /// driver: no cursor advance, no gate, no stuck window; the
    /// distance recycler collects it like any other car.
    Knocked,
}

/// One ambient car on the network.
#[derive(Component)]
pub struct AmbientCar {
    /// Roster index the class draw selected.
    pub class: usize,
    /// Whether the car lane-follows or lies where a collision left it.
    pub drive: AmbientDrive,
    /// Position on the authored network (travel-direction distance).
    pub cursor: LaneCursor,
    /// The current road's effective speed — refreshed on every turn so
    /// per-road exception limits apply.
    pub target_speed: f32,
    /// The kinematic speed the car actually travels this tick —
    /// `target_speed` on a clear corridor, braked down to a bounded
    /// stop by the obstruction sense (F10-B.1).
    pub speed: f32,
    /// Displacement window for the bounded stuck recovery (F10-B.3):
    /// accumulates drive ticks without `stuck_policy.min_displacement`
    /// of progress; on expiry the car despawns into the pool the
    /// maintainer refills from.
    pub stuck: StuckWindow,
}

/// Split nav-build issues for logging (operator report 4 item 5):
/// `NoVehicleLanes` — expected authored data, one per pedestrian/
/// special/disabled road — goes to the quiet road list the DEBUG
/// summary counts; every rarer anomaly stays in the WARN list.
fn partition_nav_issues(issues: &[NavIssue]) -> (Vec<usize>, Vec<&NavIssue>) {
    let mut quiet = Vec::new();
    let mut notable = Vec::new();
    for i in issues {
        match i {
            NavIssue::NoVehicleLanes { road } => quiet.push(*road),
            other => notable.push(other),
        }
    }
    (quiet, notable)
}

/// Load the ambient setup for this session and spawn the initial plan.
/// Returns the resource the caller inserts — `None` when the session
/// is not authoritative, the world is not a city, or no layer authors
/// a roster. All failures log and degrade to *no* ambient traffic;
/// ambient cars never sink an otherwise loadable session.
///
/// `authored_density` is the event-table `Ambient` dial for event
/// sessions (`RaceDefinition::params.densities.traffic`); cruise
/// passes `None`. The density chain puts the player's
/// `SessionCustomization` pick first (RACE-3/RACE-4 menu options),
/// then most-specific-authored: the event aimap's `[Density]`, the
/// authored table dial, the city aimap's `[Density]`, then
/// `SessionConfig::densities` (implementation choice — the original
/// layering is unverified, UNK-12).
///
/// `interest` is the load-time player interest area set — the spawn
/// poses every `Player` participant starts from (all staged on the
/// same grid, so the session caller passes the local spawn alone);
/// the initial plan draws inside their union band. The runtime
/// maintainer rebuilds the set live from every `Player` participant
/// each tick.
#[allow(clippy::too_many_arguments)] // session-load call site: assets + vfs + session all live here
pub fn load_ambient_traffic(
    commands: &mut Commands,
    vfs: &Vfs,
    config: &SessionConfig,
    event_aimap: Option<&Aimap>,
    authored_density: Option<f32>,
    owner: SessionEntity,
    session: &mut Session,
    interest: &[Vec3],
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<AmbientTraffic> {
    // Ambient traffic is authority state — a predicted remote session
    // mirrors it; it never simulates its own.
    if !session.authority_role().is_authority() {
        return None;
    }
    let WorldMode::City { psdl } = &config.world else {
        return None;
    };
    let stem = Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());

    let setup = match mm2_content::ambient_setup(vfs, stem, event_aimap) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "ambient: city aimap failed — no ambient traffic");
            return None;
        }
    };
    let setup = setup?;
    for d in &setup.diagnostics {
        warn!(diagnostic = %d, "ambient: roster diagnostic");
    }
    let build = match mm2_content::load_nav_graph(vfs, stem) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "ambient: nav graph failed — no ambient traffic");
            return None;
        }
    };

    let density = config
        .customization
        .map(|c| c.densities.traffic)
        .or(setup.event_density)
        .or(authored_density)
        .or(setup.city_density)
        .unwrap_or(config.densities.traffic);
    let policy = SpawnPolicy::default();
    let interest_set: Vec<[f32; 3]> = interest.iter().map(|p| p.to_array()).collect();
    let plan = plan_ambient(
        &build.graph,
        &setup.overrides,
        &setup.roster,
        config.seed,
        density,
        &interest_set,
        &policy,
    );

    let role = session.authority_role();
    let mut traffic = AmbientTraffic {
        eligible: eligible_lanes(&build.graph, &setup.overrides),
        graph: build.graph,
        // A second seeded stream for runtime draws (turn choices and
        // respawn placements) — same seed, never interleaved with the
        // plan's stream (implementation choice).
        rng: NavRng::new(config.seed.wrapping_add(0x4d4d_325f)),
        overrides: setup.overrides,
        roster: setup.roster,
        policy,
        target: plan.target,
        classes: HashMap::new(),
        spawned: 0,
        recycled: 0,
        dead_ends: 0,
        unspawnable: plan.unspawnable,
        dropped: plan.dropped,
        queued: 0,
        junction_held: 0,
        stuck: 0,
        stuck_policy: StuckPolicy::default(),
        knocked: 0,
        knocked_by_participant: 0,
        knocked_by_ambient: 0,
        knocked_by_other: 0,
        knock_policy: KnockPolicy::default(),
        junctions: Junctions::default(),
        crossings: 0,
        jumps: 0,
        signals: 0,
        signals_dropped: 0,
        signal_assets: SignalAssets::new(meshes, materials),
        issues: plan
            .issues
            .iter()
            .map(|i| i.to_string())
            .chain(build.issues.iter().map(|i| i.to_string()))
            .collect(),
    };
    for directive in &plan.spawns {
        if spawn_ambient_car(
            commands,
            vfs,
            &mut traffic,
            directive,
            owner,
            role,
            session,
            meshes,
            images,
            materials,
        )
        .is_some()
        {
            traffic.spawned += 1;
        }
    }
    spawn_traffic_signals(commands, &mut traffic, owner);
    // Operator report 4 item 5: `NoVehicleLanes` fires once per
    // authored pedestrian/special/disabled road — expected data
    // (WLD-11; retail london/sf carry ~150 of them), so per-road WARN
    // lines bury genuine warnings. The class collapses into one DEBUG
    // summary (every issue still lands in `traffic.issues`, counted
    // by `issues=` below, and `mm2-inspect nav` lists them all);
    // every other plan/nav issue kind still warns individually.
    let (no_lane_roads, notable_nav) = partition_nav_issues(&build.issues);
    for i in &plan.issues {
        warn!(issue = %i, "ambient issue");
    }
    for i in notable_nav {
        warn!(issue = %i, "ambient issue");
    }
    if !no_lane_roads.is_empty() {
        debug!(
            count = no_lane_roads.len(),
            roads = ?no_lane_roads,
            "ambient: roads without routable vehicle lanes (authored pedestrian/special/disabled)"
        );
    }
    info!(
        density,
        target = traffic.target,
        spawned = traffic.spawned,
        eligible = traffic.eligible.len(),
        signals = traffic.signals,
        signals_dropped = traffic.signals_dropped,
        issues = traffic.issues.len(),
        "ambient traffic loaded"
    );
    Some(traffic)
}

/// Spawn the authored traffic-signal indicators (F10-B.7): one
/// session-owned lamp at every `NavGraph::signals` head that passes
/// the `SIGNAL_MAX_DISTANCE` sanity bound. Each head shows the aspect
/// its end's authored rule admits under the junction controller's
/// initial state — a head whose road never joins the member set
/// resolves the same green fallback `gate` does; `drive_signals`
/// keeps them current. The authored `trafficLightAxis` is preserved
/// on the nav graph but unused — its convention is unverified (R3
/// notes it only as a flag that the light exists).
fn spawn_traffic_signals(
    commands: &mut Commands,
    traffic: &mut AmbientTraffic,
    owner: SessionEntity,
) {
    // The junction's member list is per-intersection — cache it
    // rather than rescanning every end's rule per signal.
    let jpolicy = JunctionPolicy::default();
    let mut members: HashMap<u16, Vec<u16>> = HashMap::new();
    for head in traffic.graph.signals() {
        let ix = head.junction;
        let origin = Vec3::from(head.signal.origin);
        if junction_zone(&traffic.graph, ix, &jpolicy).is_none_or(|(center, _)| {
            !Vec3::from(center).is_finite()
                || origin.distance(Vec3::from(center)) > SIGNAL_MAX_DISTANCE
        }) {
            // No sane zone or a wild outlier — drop it and count
            // the loss rather than draw a stray lamp kilometres
            // off the road.
            traffic.signals_dropped += 1;
            continue;
        }
        let rule = head.rule();
        let member = members
            .entry(ix)
            .or_insert_with(|| Junctions::signal_members(&traffic.graph, ix));
        let aspect = traffic.junctions.signal_aspect(ix, member, head.road, rule);
        commands.spawn((
            owner,
            TrafficSignal {
                junction: ix,
                road: head.road,
                rule,
                aspect,
            },
            Mesh3d(traffic.signal_assets.lamp.clone()),
            MeshMaterial3d(traffic.signal_assets.material(aspect).clone()),
            Transform::from_translation(origin),
            Visibility::Visible,
        ));
        traffic.signals += 1;
    }
}

/// Keep every authored signal indicator's aspect in step with the
/// authoritative [`Junctions`] controller — runs after
/// `drive_ambient` so the just-advanced phase is what the lamps show
/// (F10-B.7). Rule admission only: a light member is green exactly
/// while it holds the phase (red through the all-red clearance),
/// `NeverStop`/unruled ends stay green, `StopSign` ends show the stop
/// aspect, `AlwaysStop` ends stay red. The box-yield that holds cars
/// behind a green is an obstruction rule, not a lamp state.
pub fn drive_signals(
    session: Res<Session>,
    traffic: Option<Res<AmbientTraffic>>,
    mut signals: Query<(&mut TrafficSignal, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(traffic) = traffic else { return };
    if session.authority_role() != AuthorityRole::Authority {
        return;
    }
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    ) {
        return;
    }
    let mut members: HashMap<u16, Vec<u16>> = HashMap::new();
    for (mut sig, mut material) in &mut signals {
        let member = members
            .entry(sig.junction)
            .or_insert_with(|| Junctions::signal_members(&traffic.graph, sig.junction));
        let aspect = traffic
            .junctions
            .signal_aspect(sig.junction, member, sig.road, sig.rule);
        if aspect != sig.aspect {
            sig.aspect = aspect;
            material.0 = traffic.signal_assets.material(aspect).clone();
        }
    }
}

/// Resolve a class's assets once and cache the outcome. A class whose
/// model or collider cannot be produced caches `None` — it counts
/// unspawnable like a tuning-less row rather than retrying every tick.
/// The ambient engine table resolves alongside (F07-B.6): a malformed
/// resolved file warns once per class, an absent/sentinel one is
/// authored silence — neither blocks the spawn.
fn class_assets(vfs: &Vfs, spec: &AmbientSpec) -> Option<AmbientClass> {
    let loaded = mm2_content::ambient_vehicle(vfs, &spec.id).ok()?;
    let collider = loaded
        .bound
        .as_ref()
        .and_then(|b| {
            Collider::convex_hull(b.verts.iter().map(|v| Vec3::from(*v)).collect::<Vec<_>>())
        })
        .unwrap_or_else(|| size_collider(spec.tuning.as_ref()));
    let audio = match mm2_content::ambient_engine_audio(vfs, &spec.id) {
        Ok(Some(table)) => {
            for d in &table.diagnostics {
                warn!(class = %spec.id, diagnostic = %d, "ambient: engine table diagnostic");
            }
            for issue in table.validate() {
                warn!(class = %spec.id, issue = %issue, "ambient: engine table validation issue");
            }
            mm2_game::AmbientEngineSpec::from_table(&table)
        }
        Ok(None) => None,
        Err(e) => {
            warn!(class = %spec.id, "ambient: engine table malformed — {e}");
            None
        }
    };
    Some(AmbientClass {
        model: loaded.model,
        collider,
        audio,
    })
}

/// Fallback hull over the tuning's authored `Size` centred on its `CG`
/// (the bound-centre offset) — used when `<id>_bound.bnd` never
/// resolves or does not parse. The corners bake the offset into the
/// hull, matching how the authored bound's verts carry it.
fn size_collider(tuning: Option<&AiVehicleData>) -> Collider {
    let (size, cg) = tuning.map_or_else(
        || (Vec3::new(2.0, 1.5, 5.0), Vec3::new(0.0, 0.75, 0.0)),
        |t| {
            (
                Vec3::from(t.size),
                Vec3::from(t.cg.unwrap_or([0.0, t.size[1] * 0.5, 0.0])),
            )
        },
    );
    let h = size * 0.5;
    let corners: Vec<Vec3> = [-1.0f32, 1.0]
        .iter()
        .flat_map(|&x| {
            [-1.0f32, 1.0].iter().flat_map(move |&y| {
                [-1.0f32, 1.0]
                    .iter()
                    .map(move |&z| cg + Vec3::new(h.x * x, h.y * y, h.z * z))
            })
        })
        .collect();
    Collider::convex_hull(corners).unwrap_or_else(|| Collider::cuboid(size.x, size.y, size.z))
}

/// Spawn one planned car as a session-owned kinematic rigid body —
/// real collider, real render model — on its authored lane pose. The
/// face direction comes from the lane tangent (travel direction), so
/// backward-arc lanes orient correctly.
#[allow(clippy::too_many_arguments)] // spawn call site threads the same stores the session load does
fn spawn_ambient_car(
    commands: &mut Commands,
    vfs: &Vfs,
    traffic: &mut AmbientTraffic,
    directive: &SpawnDirective,
    owner: SessionEntity,
    role: AuthorityRole,
    session: &mut Session,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<Entity> {
    let spec = traffic.roster.entries.get(directive.class)?;
    let tuning = spec.tuning.as_ref()?;
    let class = traffic
        .classes
        .entry(directive.class)
        .or_insert_with(|| class_assets(vfs, spec).map(Arc::new))
        .clone()?;
    let pos = Vec3::from(directive.sample.position);
    if !pos.is_finite() {
        warn!(class = %spec.id, "ambient: non-finite spawn pose — skipped");
        return None;
    }
    let tangent = Vec3::from(directive.sample.tangent).normalize_or(Vec3::NEG_Z);
    let yaw = (-tangent.x).atan2(-tangent.z);
    let pitch = tangent.y.clamp(-1.0, 1.0).asin();
    let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    // Authored physicals ride the kinematic body already: they shape
    // resting contact the same way, and the F10-B.6 handover only has
    // to flip the body kind — the same convention `banger_bundle`
    // uses for dormant props. The authored CG sits in the same
    // vehicle-local space the bound verts and `size_collider`'s hull
    // use; a degenerate/absent mass or CG falls back to sane defaults.
    let mass = if tuning.mass.is_finite() && tuning.mass > 0.0 {
        tuning.mass
    } else {
        1000.0
    };
    let cg = tuning
        .cg
        .filter(|c| c.iter().all(|v| v.is_finite()))
        .unwrap_or([0.0, tuning.size[1] * 0.5, 0.0]);
    let entity = commands
        .spawn((
            owner,
            ObjectIdentity(session.mint_object_id()),
            role,
            AmbientCar {
                class: directive.class,
                drive: AmbientDrive::Lane,
                cursor: LaneCursor::new(directive.lane, directive.along),
                target_speed: directive.target_speed,
                speed: directive.target_speed.max(0.0),
                stuck: StuckWindow::new(pos.to_array()),
            },
            RigidBody::Kinematic,
            class.collider.clone(),
            (
                Mass(mass),
                CenterOfMass(Vec3::from(cg)),
                // The striker usually enables the pair's events (the
                // vehicle does), but wreck-vs-car and prop-fragment
                // strikers need the flag on this side too.
                CollisionEventsEnabled,
                Friction::new(tuning.friction),
                Restitution::new(tuning.elasticity),
                // Solver-level speed bounds, the same bound
                // `banger_bundle` stamps (the records carry none):
                // non-binding while the car is kinematic — Avian's
                // `clamp_velocities` covers kinematic solver bodies
                // too, but `drive_ambient` prescribes lane speeds far
                // under the cap — and once an impact flips the body
                // dynamic they keep a fast-spinning wreck from feeding
                // an inflated `normal_speed` back into a later contact
                // (the sf-8 cascade class F13-C.3 bounded).
                MaxLinearSpeed(MAX_BANGER_LINEAR_SPEED),
                MaxAngularSpeed(MAX_BANGER_ANGULAR_SPEED),
            ),
            Position(pos),
            Rotation(rot),
            LinearVelocity(tangent * directive.target_speed.max(0.0)),
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
            TransformInterpolation,
            Visibility::Visible,
        ))
        .id();
    // F07-B.6: the class's resolved ambient engine table rides the
    // car — `ambient_engine_rigs` turns it into a bounded looping
    // voice; a noteless class (absent/malformed table or sentinel
    // sample) stamps nothing, matching "no authored engine note".
    if let Some(spec) = &class.audio {
        commands
            .entity(entity)
            .insert(mm2_game::AmbientAudio { spec: spec.clone() });
    }
    let missing = spawn_vehicle_model(
        commands,
        vfs,
        &class.model,
        0,
        meshes,
        images,
        materials,
        entity,
        // Ambient cars carry no `vehcardamage` — no texel rig.
        None,
    );
    if !missing.is_empty() {
        warn!(class = %spec.id, "ambient: missing textures: {}", missing.join(", "));
    }
    Some(entity)
}

/// Advance every ambient car along its lane by `speed × dt`,
/// re-posing it from the sampled lane each fixed tick. Runs in
/// `FixedLast` — after the physics step consumed the previous pose —
/// so the written `Position`/`Rotation` is what the next solver step
/// and the renderer both see, and `LinearVelocity` reports the surface
/// velocity contacts resolve against.
///
/// F10-B.1 obstruction response: before advancing, each car senses a
/// forward corridor (`corridor_gap`) against every `Player`
/// participant — the local driver and AI opponents alike — and every
/// other ambient car, then the `follow_speed` law sets its kinematic
/// `speed`: road limit on a clear corridor, a bounded brake to
/// `follow_gap` behind a blocker, an outright stop inside
/// `panic_gap`, and a `turn_speed` cap across intersections. A queued
/// car waits — a kinematic body that kept moving would shove whatever
/// blocks it — and resumes when the corridor clears. `traffic.queued`
/// reports the held count each tick.
///
/// A car that runs out of road despawns, as does one whose
/// displacement window expires (F10-B.3 stuck recovery);
/// `maintain_ambient` decides whether a replacement spawns.
#[allow(clippy::type_complexity)] // Bevy system: the two queries are the system's actual signature
pub fn drive_ambient(
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    traffic: Option<ResMut<AmbientTraffic>>,
    mut cars: Query<
        (
            Entity,
            &mut AmbientCar,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut Transform,
        ),
        Without<Player>,
    >,
    players: Query<(Entity, &Position), (With<Player>, Without<AmbientCar>)>,
    mut commands: Commands,
) {
    let Some(mut traffic) = traffic else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        return;
    }
    let traffic = &mut *traffic;
    let dt = time.delta_secs();
    // Corridor blockers: every participant plus every ambient car —
    // the follower queues behind whatever sits on its lane without
    // distinguishing who it is.
    let mut blockers: Vec<(Entity, Vec3)> = players.iter().map(|(e, p)| (e, p.0)).collect();
    blockers.extend(cars.iter().map(|(e, _, p, _, _, _)| (e, p.0)));
    // The junction each lane-following ambient car's current lane is
    // bound for (the downstream end of its arc). The box-yield must
    // not count a car as occupying the junction it is still
    // approaching — a car waiting at or behind its own stop line is
    // not inside the box, or two competing approaches would hold each
    // other forever. A `Knocked` car is not bound for anything: it is
    // an obstacle wherever the collision left it, so a wreck inside
    // the box legitimately occupies it. Neither is a car mid-crossing
    // (F10-B.9): committed to the box, it is an occupant of it — not
    // an approach that may still hold.
    let bound_for: HashMap<Entity, u16> = cars
        .iter()
        .filter_map(|(e, c, ..)| {
            (c.drive == AmbientDrive::Lane && c.cursor.crossing.is_none())
                .then(|| Junctions::approach(&traffic.graph, c.cursor.lane))
                .flatten()
                .map(|(ix, _, _)| (e, ix))
        })
        .collect();
    let follow = FollowPolicy::default();
    let jpolicy = traffic.junctions.policy;
    let stuck_policy = traffic.stuck_policy;
    // The controller's clock ticks with the driver, then sheds queue
    // entries whose cars the recycler collected since last tick.
    traffic.junctions.advance_tick();
    {
        let live: BTreeSet<Entity> = cars.iter().map(|(e, ..)| e).collect();
        traffic.junctions.retain(&live);
    }
    let mut queued = 0usize;
    let mut junction_held = 0usize;
    for (entity, mut car, mut position, mut rotation, mut velocity, mut transform) in
        cars.iter_mut()
    {
        // A knocked car is solver-owned: no cursor advance, no gate,
        // no pose rewrite, no stuck window — the distance recycler
        // collects it like any other car.
        if car.drive == AmbientDrive::Knocked {
            continue;
        }
        let fwd = rotation.0 * Vec3::NEG_Z;
        let reach = follow.near + follow.lead * car.speed.max(0.0);
        let gap = corridor_gap(
            position.0.to_array(),
            fwd.to_array(),
            follow.half_width,
            follow.max_rise,
            reach,
            blockers
                .iter()
                .filter(|(e, _)| *e != entity)
                .map(|(_, p)| p.to_array()),
        );
        car.speed = follow_speed(car.speed, car.target_speed, gap, dt, &follow);
        if gap.is_some() && car.speed <= follow.held_speed {
            queued += 1;
        }
        // Junction gate (F10-B.2): the authored `vehicleRule` at the
        // arc's downstream end decides whether the car may pass the
        // lane end this tick — signal red, stop-sign queue/dwell and
        // `AlwaysStop` close it, everything else opens it. A closed
        // gate brakes to the stop line and never lets the cursor past
        // it; a stopped stop-sign car registers in the FCFS queue.
        //
        // A car mid-crossing (F10-B.9) is committed: no gate, no stop
        // line — it traverses the junction interior under the corner
        // cap and the corridor sense alone.
        let mut ds;
        if car.cursor.crossing.is_some() {
            car.speed = car.speed.min(follow.turn_speed);
            ds = car.speed.max(0.0) * dt;
        } else {
            let dist_to_stop = traffic
                .graph
                .lane(car.cursor.lane)
                .map(|l| l.length - jpolicy.stop_inset - car.cursor.along)
                .unwrap_or(f32::MAX);
            // "At the line" is the policy tolerance, never an exact
            // zero: the brake ramp decays `dist_to_stop`
            // geometrically, so the f32 cursor asymptotes a hair
            // short of the line and would otherwise never register,
            // never open a stop-sign queue, and never report held.
            let at_line = dist_to_stop <= jpolicy.stop_line_tolerance;
            // Junction-box yield (F10-B.5): an otherwise-admitted
            // gated approach still holds while the box contains a
            // vehicle not bound for it — the green/FCFS turn does not
            // make an occupied box passable. Only the rules whose
            // gate can open consult it; `NeverStop`/unruled ends keep
            // their free flow.
            let box_occupied = match Junctions::approach(&traffic.graph, car.cursor.lane) {
                Some((ix, _, Some(VehicleRule::TrafficLight | VehicleRule::StopSign))) => {
                    junction_zone(&traffic.graph, ix, &jpolicy).is_some_and(|zone| {
                        blockers.iter().any(|(e, b)| {
                            *e != entity
                                && bound_for.get(e).copied() != Some(ix)
                                && inside_junction_zone(b.to_array(), zone, &jpolicy)
                        })
                    })
                }
                _ => false,
            };
            let gate = traffic.junctions.gate(
                &traffic.graph,
                car.cursor.lane,
                entity,
                at_line,
                car.speed <= follow.held_speed,
                box_occupied,
            );
            car.speed = junction_speed(car.speed, dist_to_stop, gate, dt, &jpolicy);
            if gate == JunctionGate::Closed && at_line && car.speed <= follow.held_speed {
                junction_held += 1;
            }
            ds = car.speed.max(0.0) * dt;
            if gate == JunctionGate::Closed {
                // Inside the tolerance the residual is below the
                // ramp's f32 resolution — close it outright so the
                // car stands on the line; outside it, never step past
                // the line.
                ds = if at_line {
                    dist_to_stop.max(0.0)
                } else {
                    ds.min(dist_to_stop)
                };
            }
        }
        let previous = car.cursor.clone();
        let step = if ds > 0.0 {
            advance_lane_cursor(
                &traffic.graph,
                &traffic.overrides,
                &mut car.cursor,
                ds,
                &mut traffic.rng,
            )
        } else if car.cursor.crossing.is_some() {
            LaneAdvance::Crossing
        } else {
            LaneAdvance::Along
        };
        match step {
            LaneAdvance::DeadEnd => {
                traffic.dead_ends += 1;
                traffic.junctions.depart(entity);
                commands.entity(entity).despawn();
                continue;
            }
            LaneAdvance::Entered => {
                traffic.crossings += 1;
                // The junction this commit entered (F10-B.11): claimed
                // for the rest of the tick so a second eligible car
                // evaluated later this tick cannot take the box
                // alongside it — `blockers`/`bound_for` are frame-start
                // snapshots and cannot see a same-tick commit.
                let entered_ix =
                    Junctions::approach(&traffic.graph, previous.lane).map(|(ix, _, _)| ix);
                if let Some(ix) = entered_ix {
                    traffic.junctions.commit(ix, entity);
                }
                // Occupied-transfer check (F10-AC04's junction leg):
                // a landing inside `enter_clearance` of a live
                // blocker would materialise the car inside a junction
                // queue — hold at the lane end and retry next tick.
                // When one step covered the whole interior the car is
                // already on the landing; the same check runs on the
                // reached point.
                let check = match &car.cursor.crossing {
                    Some(c) => traffic
                        .graph
                        .sample_lane(c.landing, 0.0)
                        .map(|s| s.position),
                    None => car.cursor.sample(&traffic.graph).map(|s| s.position),
                };
                let landing_occupied = check.is_some_and(|p| {
                    let p = Vec3::from(p);
                    blockers
                        .iter()
                        .any(|(e, b)| *e != entity && b.distance(p) < jpolicy.enter_clearance)
                });
                if landing_occupied {
                    car.cursor = previous;
                    car.speed = 0.0;
                    traffic.crossings -= 1;
                    // Shed only *this* car's claim: on a mixed-rule
                    // junction a free-flow car never consults the
                    // record, so it can roll back over another car's
                    // live commit — a junction-keyed release would
                    // strip that claim and re-open the box this tick.
                    if let Some(ix) = entered_ix {
                        traffic.junctions.release(ix, entity);
                    }
                } else {
                    // Committed to the box: the approach releases its
                    // FCFS slot — the box-yield holds the next car
                    // until this one physically clears the junction.
                    traffic.junctions.depart(entity);
                    // Corner braking stand-in: an intersection turn
                    // is never taken at full road speed.
                    car.speed = car.speed.min(follow.turn_speed);
                    if car.cursor.crossing.is_none() {
                        // Committed and landed inside one step — the
                        // post-landing bookkeeping still applies.
                        if let Some(road) = traffic.graph.road(car.cursor.lane.road) {
                            car.target_speed = traffic.overrides.effective_speed(road);
                        }
                    }
                }
            }
            LaneAdvance::Landed => {
                traffic.junctions.depart(entity);
                if let Some(road) = traffic.graph.road(car.cursor.lane.road) {
                    car.target_speed = traffic.overrides.effective_speed(road);
                }
                car.speed = car.speed.min(follow.turn_speed);
            }
            LaneAdvance::Crossing | LaneAdvance::Along => {}
        }
        let Some(sample) = car.cursor.sample(&traffic.graph) else {
            traffic.dead_ends += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        };
        let pos = Vec3::from(sample.position);
        if !pos.is_finite() {
            traffic.dead_ends += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        }
        let tangent = Vec3::from(sample.tangent).normalize_or(Vec3::NEG_Z);
        let yaw = (-tangent.x).atan2(-tangent.z);
        let pitch = tangent.y.clamp(-1.0, 1.0).asin();
        let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        // The *intended* surface velocity, never the measured delta:
        // Avian integrates kinematic bodies from `LinearVelocity`, so
        // a delta measured across the physics step feeds back on
        // itself and diverges — and a contact resolving against it
        // would see a phantom ~km/s impactor.
        velocity.0 = tangent * car.speed.max(0.0);
        // Position-continuity watchdog (operator report 4 item 1): a
        // lane follower only ever moves its own driven step — a
        // displacement wider than that is a teleport the aggregate
        // counters cannot see. Junction crossings must read zero.
        let step_limit = car.speed.max(0.0) * dt * 2.0 + 1.0;
        if pos.distance(position.0) > step_limit {
            traffic.jumps += 1;
        }
        position.0 = pos;
        rotation.0 = rot;
        *transform = Transform::from_translation(pos).with_rotation(rot);
        // Bounded stuck recovery (F10-B.3): a car that has made no
        // `min_displacement` of progress for `window_ticks` — penned
        // behind a parked blocker, trapped in a queue that never
        // drains, standing at an `AlwaysStop` end or a permanently
        // occupied exit — leaves the world; the maintainer's refill
        // is the recovery. The window sits far beyond the worst
        // legitimate wait (a multi-member signal's full red cycle),
        // so an ordinary hold never trips it, and the despawn is
        // bounded rather than a teleport through whatever pens it.
        if car.stuck.tick(pos.to_array(), &stuck_policy) {
            traffic.stuck += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
            continue;
        }
    }
    traffic.queued = queued;
    traffic.junction_held = junction_held;
}

/// One pending handover decided off a contact edge, before any
/// mutation: who flips, what velocity it leaves with, and what the
/// same exchange owes the striking body.
struct Knock {
    /// The lane follower handing over.
    entity: Entity,
    /// Push direction on the car (the manifold normal in its order).
    dir: Vec3,
    /// The Δv the hit writes along `dir` — the transfer's mass-correct
    /// launch, or the pre-transfer approach-speed fallback when either
    /// mass cannot be resolved.
    launch: f32,
    /// The Δp the car received (`dir` × this) — feeds the
    /// contact-lever spin share.
    impulse: f32,
    /// The measured pre-solver closing speed — with the car's own
    /// `dir` speed it reconstructs the striker's approach component,
    /// which the striker correction's velocity target needs.
    severity: f32,
    /// World lever from the car's centre of mass to the contact.
    lever: Vec3,
    /// The striking body — the other contact body.
    striker: Entity,
    /// The striker's world lever to the same contact.
    striker_lever: Vec3,
    /// The committed two-body transfer — `None` when either mass
    /// cannot be resolved: the launch stays at approach speed and no
    /// correction is written.
    transfer: Option<Transfer>,
}

/// The struck car's mutable pieces in `knock_ambient` — `StruckMut`'s
/// shape plus the lane state the handover flips.
type KnockedCarMut = (
    &'static mut AmbientCar,
    &'static mut LinearVelocity,
    &'static mut AngularVelocity,
    Option<&'static ComputedAngularInertia>,
    Option<&'static Rotation>,
);

/// Kinematic→dynamic handover (F10-B.6): a third consumer of the
/// solver's `CollisionStart` stream, alongside `collect_impacts` and
/// `activate_bangers`. A lane-following car whose contact's impulse
/// estimate — approach speed × striker mass, measured on the same
/// deepest-contact severity banger activation measures — reaches
/// `KnockPolicy::min_impulse`
/// becomes a dynamic body on the same entity: the hull, pose and lane
/// velocity carry over unchanged (no duplicate body, no teleport). The
/// FCFS queue releases the car and `drive_ambient` never re-poses it:
/// from the flip on, it is a wreck the other cars' corridor sense and
/// the box yield treat as an obstacle, until the ordinary distance
/// recycler collects it. Below-threshold touches leave the follower
/// alone — a scrape or a light tap does not convert the car.
///
/// F10-B.12: the flip itself replays the hit as a two-body transfer —
/// the solver just answered the contact against the car as an
/// infinite-mass body, so the striker's component along the push is
/// rewritten to the share the real `(1+e)·v·μ` exchange leaves it
/// (the shared `contracts` transfer banger activation runs): the
/// wreck leaves at the mass-correct launch speed through its contact
/// lever, the striker keeps its share, and the contact impulse is
/// charged exactly once. The striker correction is a velocity target,
/// not a returned impulse: Avian's recorded `total_impulse`
/// accumulates penetration-recovery and restitution passes and
/// over-reports the striker's actual Δv, so the code reconstructs the
/// approach component from the measured severity instead. A
/// lane-follower striker takes no correction — the lane driver owns
/// its velocity — which also keeps a follower-follower edge from
/// charging the exchange twice. When a mass cannot be resolved the
/// car keeps the pre-transfer approach-speed launch and no correction
/// is written.
///
/// F10-B.13: the handover counts its striker's class
/// (`knocked_by_participant`/`_ambient`/`_other` — a `Player`
/// participant, another ambient car, or anything else) so the record
/// can say *who* struck each wreck, and bounds the wreck's own spin:
/// the contact-lever `angular_share` write is clamped at
/// `MAX_BANGER_ANGULAR_SPEED` and the flipped body carries the
/// solver-side `MaxLinearSpeed`/`MaxAngularSpeed` stamped at spawn —
/// the same bounds banger bodies get, so a transient spike cannot
/// leave a wreck whose inflated spin re-enters a later contact's
/// `normal_speed` reading. F10-B.14 extends the same write-side
/// bound to the striker: the shared `write_striker_correction` clamps
/// the correction's post-write spin too (a `Player` striker carries
/// no solver bound — the write clamp is its only one).
///
/// Drains under the same phase/authority gate as `drive_ambient`:
/// edges buffered while paused never flush as a stale burst on
/// resume, and a `Predicted` session never hands over locally.
#[allow(clippy::too_many_arguments)] // Bevy system: the decision threads cars, strikers and masses
pub fn knock_ambient(
    mut reader: MessageReader<CollisionStart>,
    collisions: Collisions,
    session: Res<Session>,
    traffic: Option<ResMut<AmbientTraffic>>,
    mut cars: Query<KnockedCarMut>,
    mut strikers: Query<StruckMut, Without<AmbientCar>>,
    players: Query<(), With<Player>>,
    masses: Query<&ComputedMass>,
    mut commands: Commands,
) {
    let Some(mut traffic) = traffic else {
        reader.read().for_each(drop);
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        reader.read().for_each(drop);
        return;
    }
    let policy = traffic.knock_policy;

    // Decide first, mutate second — the decision pass only reads.
    let mut knocks: Vec<Knock> = Vec::new();
    for event in reader.read() {
        let (c1, c2) = (event.collider1, event.collider2);
        let Some(deepest) = deepest_contact(&collisions, c1, c2) else {
            continue;
        };
        // Same bound banger activation applies: the manifold's
        // approach speed can read a transient solver spike — clamp it
        // so the gate, the transfer impulse and the launch all see a
        // physical approach.
        let severity = deepest.severity.min(MAX_BANGER_LINEAR_SPEED);
        if severity <= 0.0 {
            continue;
        }
        // Either side may be the ambient car; the other body is the
        // striker. The manifold normal points from collider1 toward
        // collider2, so `sign` turns it into the push direction on
        // the car (same convention `activate_bangers` uses). The
        // anchors swap with the side: `lever` is the car's contact
        // arm, `striker_lever` the other body's.
        for (collider, striker, sign, lever, striker_lever) in [
            (
                c1,
                event.body2.unwrap_or(c2),
                -1.0f32,
                deepest.anchor1,
                deepest.anchor2,
            ),
            (
                c2,
                event.body1.unwrap_or(c1),
                1.0f32,
                deepest.anchor2,
                deepest.anchor1,
            ),
        ] {
            let Ok((car, ..)) = cars.get(collider) else {
                continue;
            };
            if car.drive != AmbientDrive::Lane {
                continue;
            }
            if impulse_estimate(striker, severity, &masses) < policy.min_impulse {
                continue;
            }
            let dir = deepest.normal * sign;
            let struck_mass = masses
                .get(collider)
                .ok()
                .map(|m| m.value())
                .filter(|m| m.is_finite() && *m > 0.0);
            let transfer = struck_mass.and_then(|m| {
                resolve_transfer(Some(striker), severity, deepest.restitution, m, &masses)
            });
            // Bound the written velocity itself: `launch` covers the
            // transfer path and `severity` the mass-less fallback.
            let launch = transfer
                .as_ref()
                .map(|t| t.launch)
                .unwrap_or(severity)
                .min(MAX_BANGER_LINEAR_SPEED);
            knocks.push(Knock {
                entity: collider,
                dir,
                launch,
                impulse: transfer
                    .as_ref()
                    .map(|t| t.impulse)
                    .unwrap_or(severity * struck_mass.unwrap_or(1.0)),
                severity,
                lever,
                striker,
                striker_lever,
                transfer,
            });
        }
    }

    // Cars this pass already flipped: on a follower-follower edge the
    // other side's own launch *is* its share of the exchange, so the
    // striker correction must not charge it again.
    let mut handed_over: Vec<Entity> = Vec::new();
    for knock in knocks {
        // The car's velocity along the push direction before its
        // launch — the striker correction's velocity target needs it.
        // A later edge can name a car an earlier edge already flipped
        // — the `Lane` re-check is the dedup.
        let struck_pre = {
            let Ok((mut car, mut linvel, mut angvel, inertia, rotation)) =
                cars.get_mut(knock.entity)
            else {
                continue;
            };
            if car.drive != AmbientDrive::Lane {
                continue;
            }
            car.drive = AmbientDrive::Knocked;
            let struck_pre = linvel.0.dot(knock.dir);
            linvel.0 += knock.dir * knock.launch;
            angvel.0 += angular_share(knock.lever, knock.dir * knock.impulse, inertia, rotation);
            // Write-side spin bound (the solver-side `MaxAngularSpeed`
            // clamps the integration too): an unbounded lever share
            // would leave the wreck spinning fast enough to inflate a
            // later contact's approach-speed reading — the same
            // cascade amplifier the banger bound closes.
            angvel.0 = angvel.0.clamp_length_max(MAX_BANGER_ANGULAR_SPEED);
            struck_pre
        };
        handed_over.push(knock.entity);
        traffic.knocked += 1;
        // Name the striker's class for the record (F10-B.13): a
        // `Player` participant, another ambient car (lane follower or
        // already-knocked wreck), or anything else — a banger body, a
        // break fragment, a world-side body.
        if players.contains(knock.striker) {
            traffic.knocked_by_participant += 1;
        } else if cars.get(knock.striker).is_ok() {
            traffic.knocked_by_ambient += 1;
        } else {
            traffic.knocked_by_other += 1;
        }
        traffic.junctions.depart(knock.entity);
        commands.entity(knock.entity).insert(RigidBody::Dynamic);
        debug!(entity = ?knock.entity, "ambient car knocked to dynamics");

        let Some(transfer) = knock.transfer.as_ref() else {
            continue;
        };
        // The striker keeps its share of the exchange: its velocity
        // along the push direction becomes its approach component
        // (`severity` = the pair's pre-solver closing speed) minus the
        // transferred impulse over its mass. Write the velocity target
        // rather than returning the manifold's recorded impulse —
        // `total_impulse` accumulates penetration-recovery and
        // restitution passes, so on a kinematic pair it over-reports
        // the wall response the striker actually took and charging it
        // back would inject energy. A lane-follower striker takes no
        // correction: `drive_ambient` owns its velocity — which also
        // keeps a follower-follower edge from charging the exchange
        // twice (the other side's own launch is its share).
        let target = struck_pre + knock.severity - transfer.impulse / transfer.striker_mass;
        if let Ok((linvel, angvel, inertia, rotation)) = strikers.get_mut(knock.striker) {
            let delta = knock.dir * (target - linvel.0.dot(knock.dir)) * transfer.striker_mass;
            write_striker_correction(
                delta,
                transfer.striker_mass,
                knock.striker_lever,
                linvel,
                angvel,
                inertia,
                rotation,
            );
        } else if let Ok((striker_car, linvel, angvel, inertia, rotation)) =
            cars.get_mut(knock.striker)
        {
            // Only an already-dynamic wreck takes the correction — a
            // lane follower's velocity is owned by `drive_ambient`,
            // and a car this pass already flipped already took its
            // share as its launch.
            if striker_car.drive == AmbientDrive::Knocked && !handed_over.contains(&knock.striker) {
                let delta = knock.dir * (target - linvel.0.dot(knock.dir)) * transfer.striker_mass;
                write_striker_correction(
                    delta,
                    transfer.striker_mass,
                    knock.striker_lever,
                    linvel,
                    Some(angvel),
                    inertia,
                    rotation,
                );
            }
        }
    }
}

/// Keep the population at the plan's target: despawn cars that left
/// the union of player interest areas — beyond `recycle_distance` of
/// *every* `Player` participant — then draw fresh placements through
/// the same [`draw_spawn`] the planner used — bounded per tick so a
/// drained city cannot stall the frame.
///
/// The interest set is every `Player` participant's position — the
/// local driver, remote drivers and AI opponents alike (designed
/// composition, F10 spec req 2): each participant can hold live
/// interactions with ambient cars — corridor sensing, junction-box
/// occupancy, collisions — so a car parked beside a far-away
/// participant survives the local bubble leaving it (F10-AC04's
/// "preserve active interactions near any player"). A respawn must
/// land inside at least one area's band and outside every area's
/// `min_player_distance` — it can never materialise next to anybody.
///
/// F10-B.4 occupied-space rejection (F10-AC04): the draw sees the
/// positions of every surviving ambient car and every `Player`
/// participant — the local driver and AI opponents alike — and a
/// sample whose exclusion box touches one is retried, never spawned.
/// Each placement joins the set for the rest of the tick, so two
/// deferred spawns in one refill pass cannot stack either.
#[allow(clippy::too_many_arguments)] // Bevy system: respawning threads the same asset stores the session load does
pub fn maintain_ambient(
    mut commands: Commands,
    mut session: ResMut<Session>,
    traffic: Option<ResMut<AmbientTraffic>>,
    vfs: Option<Res<mm2_game::Mm2Vfs>>,
    mut cars: Query<(Entity, &AmbientCar, &Position)>,
    participants: Query<&Position, (With<Player>, Without<AmbientCar>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (Some(mut traffic), Some(vfs)) = (traffic, vfs) else {
        return;
    };
    // Same live-phase gate `drive_ambient` runs under — a paused or
    // resolved session freezes its population instead of churning
    // respawns behind the overlay.
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        return;
    }
    // The union of player interest areas — every `Player` participant
    // carries one (the local vehicle included). No players means no
    // bubble to populate or collect against: freeze the population
    // rather than despawning the city.
    let interest: Vec<[f32; 3]> = participants.iter().map(|p| p.0.to_array()).collect();
    if interest.is_empty() {
        return;
    }
    let traffic = &mut *traffic;

    let mut active = 0usize;
    // Space the respawn draw must not materialise inside: every car
    // that survives the bubble test, plus every participant.
    let mut occupied: Vec<[f32; 3]> = Vec::new();
    for (entity, _, pos) in &mut cars {
        if !within_interest(pos.0.to_array(), &interest, &traffic.policy) {
            traffic.recycled += 1;
            traffic.junctions.depart(entity);
            commands.entity(entity).despawn();
        } else {
            occupied.push(pos.0.to_array());
            active += 1;
        }
    }
    occupied.extend(interest.iter().copied());
    if active >= traffic.target || traffic.eligible.is_empty() {
        return;
    }
    // Recycled slots refill through the same draw — the respawner
    // never out-attempts the planner's own bound.
    let role = session.authority_role();
    let owner = SessionEntity(session.generation());
    for _ in 0..traffic.policy.placement_attempts {
        if active >= traffic.target {
            break;
        }
        match draw_spawn(
            &traffic.graph,
            &traffic.overrides,
            &traffic.eligible,
            &traffic.roster,
            &mut traffic.rng,
            &interest,
            &occupied,
            &traffic.policy,
        ) {
            SpawnDraw::OutOfBand | SpawnDraw::Occupied => continue,
            SpawnDraw::Unspawnable(_) => {
                traffic.unspawnable += 1;
                break;
            }
            SpawnDraw::Placed(directive) => {
                if spawn_ambient_car(
                    &mut commands,
                    &vfs.0,
                    traffic,
                    &directive,
                    owner,
                    role,
                    session.as_mut(),
                    &mut meshes,
                    &mut images,
                    &mut materials,
                )
                .is_some()
                {
                    traffic.spawned += 1;
                    active += 1;
                    // Commands-deferred — later draws this tick cannot
                    // see the new car through the query, so claim its
                    // spot in the occupied set directly.
                    occupied.push(directive.sample.position);
                } else {
                    traffic.unspawnable += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! F10-B.10 (operator report 4 item 5): the bulk `NoVehicleLanes`
    //! class partitions out of the per-issue WARN list into one DEBUG
    //! summary; rarer anomalies keep their individual warnings.

    use mm2_formats::bai::{End, Side};
    use mm2_game::{LaneKind, NavIssue};

    use super::partition_nav_issues;

    #[test]
    fn no_vehicle_lanes_partition_out_of_the_warn_list() {
        let issues = vec![
            NavIssue::NoVehicleLanes { road: 7 },
            NavIssue::UnresolvedEnd {
                road: 1,
                end: End::End,
            },
            NavIssue::NoVehicleLanes { road: 12 },
            NavIssue::DegenerateLane {
                road: 4,
                side: Side::Left,
                kind: LaneKind::Sidewalk,
                index: 0,
            },
            NavIssue::NonFiniteLane {
                road: 9,
                side: Side::Right,
                kind: LaneKind::Vehicle,
                index: 2,
            },
        ];
        let (quiet, notable) = partition_nav_issues(&issues);
        assert_eq!(quiet, [7, 12], "only the bulk class is summarised");
        assert_eq!(
            notable,
            [&issues[1], &issues[3], &issues[4]],
            "every other kind keeps its own WARN, in order"
        );
    }

    #[test]
    fn an_empty_issue_list_partitions_empty() {
        let (quiet, notable) = partition_nav_issues(&[]);
        assert!(quiet.is_empty() && notable.is_empty());
    }
}
