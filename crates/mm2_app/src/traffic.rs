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
//! `signals_dropped`) and draws the retail `sp_traflit*` unit there —
//! the aimap's `[Traffic Lights]` models or the executable defaults,
//! single or dual by approach-lane count (designed), standing on the
//! origin with its lamps facing along the road away from the junction
//! (axis sign unverified, so the facing is designed); a model that does
//! not resolve is reported, not replaced. `drive_signals` then sets each indicator's
//! aspect off the authoritative [`Junctions`] controller every tick —
//! green while the approach's road holds the signal phase, red out of
//! phase and in the all-red clearance, constant green on free-flow
//! ends, the stop aspect on stop-signed ends; `sync_signal_lamps`
//! shows the matching authored glow chunk. The mapping is a
//! designed presentation over authored positions and rules; the
//! original's signal visuals and exact state semantics are unverified
//! (UNK-12).
//!
//! [`drape_ambient`] rests every lane follower on the road surface.
//! BAI lane curves are linear between authored road sections, and on
//! SF's hills they leave the PSDL road by up to ±1.9 m — cars floated
//! over dips and sank into crests. The drape probes the city's ground
//! colliders under each car's corners every tick and takes height,
//! pitch and camber from the road, heading and travel from the lane.
//! The probe footprint and search window are designed values.

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
    AmbientRoster, AmbientSpec, ArcEnd, AuthorityRole, FollowPolicy, JunctionGate, JunctionPolicy,
    Junctions, KnockPolicy, LaneAdvance, LaneCursor, LaneId, MAX_BANGER_ANGULAR_SPEED,
    MAX_BANGER_LINEAR_SPEED, NavGraph, NavIssue, NavOverrides, NavRng, ObjectIdentity, Player,
    Session, SessionAuthority, SessionConfig, SessionEntity, SessionPhase, SignalAspect,
    SpawnDirective, SpawnDraw, SpawnPolicy, StuckPolicy, StuckWindow, SurfaceMaterial, WorldMode,
    advance_lane_cursor, corridor_gap, draw_spawn, eligible_lanes, follow_speed,
    inside_junction_zone, junction_speed, junction_zone, plan_ambient, within_interest,
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
pub(crate) struct AmbientClass {
    pub(crate) model: mm2_content::VehicleModel,
    pub(crate) collider: Collider,
    /// `aud/cardata/ambient/<id>_engine.csv` (or the authored default),
    /// resolved to mix parameters. `None` on absent/malformed tables or
    /// a sentinel sample — authored silence, never a spawn blocker.
    pub(crate) audio: Option<mm2_game::AmbientEngineSpec>,
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
    /// Cars ever spawned per roster class (F10-AC01 type census); sums
    /// to `spawned`.
    pub spawned_by_class: std::collections::BTreeMap<usize, usize>,
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
    /// Authored signal-unit model names that did not resolve — the
    /// heads at those models draw no geometry; reported, never
    /// replaced by a placeholder.
    pub signal_models_missing: Vec<String>,
    /// Planner/setup problems, reported honestly.
    pub issues: Vec<String>,
}

/// How far from its junction's occupancy-zone centre an authored
/// signal origin may sit before it is treated as junk data and
/// dropped (m) — retail's farthest legitimate origin is ~40 m, and a
/// handful of authored outliers reach hundreds of metres (counted in
/// `signals_dropped`). Designed bound, UNK-12.
const SIGNAL_MAX_DISTANCE: f32 = 60.0;

/// The retail signal-unit models `Midtown2.exe` carries as the
/// `[Traffic Lights]` defaults (WLD-35) — a map's aimap line overrides
/// them (`city/london.aimap` names `sp_traflitsingle_ped_l` for both).
const DEFAULT_SINGLE_MODEL: &str = "sp_traflitsingle_f";
const DEFAULT_DUAL_MODEL: &str = "sp_traflitdual_f";

/// Lateral width (m) a vehicle lane is taken to occupy when choosing
/// between the single and dual signal unit — designed, UNK-12.
const SIGNAL_LANE_PITCH: f32 = 3.0;

/// One lamp state the signal-unit PKGs author as glow geometry
/// (`{RED,YELLOW,GREEN}GLOW{DAY,NIGHT}_H`, `{WALK,NOWALK}_{DAY,NIGHT}_H`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LampState {
    Red,
    Yellow,
    Green,
    Walk,
    NoWalk,
}

impl LampState {
    /// The lamp state a PKG geometry stem names, for the day variant —
    /// the night chunks are larger halo billboards of the same lamps
    /// and are not drawn (the world has no night lighting to pair
    /// them with; designed).
    fn from_day_stem(stem: &str) -> Option<Self> {
        match stem {
            "redglowday" => Some(Self::Red),
            "yellowglowday" => Some(Self::Yellow),
            "greenglowday" => Some(Self::Green),
            "walk_day" => Some(Self::Walk),
            "nowalk_day" => Some(Self::NoWalk),
            _ => None,
        }
    }

    /// Whether the chunk stem is any lamp state, day or night.
    fn is_lamp_stem(stem: &str) -> bool {
        Self::from_day_stem(stem).is_some()
            || matches!(
                stem,
                "redglownight"
                    | "yellowglownight"
                    | "greenglownight"
                    | "walk_night"
                    | "nowalk_night"
            )
    }

    /// Whether this lamp is lit while a head shows `aspect`. The
    /// pedestrian lamps follow the vehicle aspect (green ↔ walk, anything
    /// else ↔ don't walk): a designed presentation, the original's
    /// pedestrian phasing is unrecovered (UNK-12).
    fn lit(self, aspect: SignalAspect) -> bool {
        matches!(
            (self, aspect),
            (Self::Green | Self::Walk, SignalAspect::Green)
                | (Self::Red | Self::NoWalk, SignalAspect::Red)
                | (Self::Yellow | Self::NoWalk, SignalAspect::Stop)
        )
    }
}

type ModelPart = (Handle<Mesh>, Handle<StandardMaterial>);

/// A lamp mesh before its material is built: state, mesh, base texture
/// material and the authored glow colour.
type RawLampPart = (LampState, Handle<Mesh>, Handle<StandardMaterial>, [f32; 4]);

/// One authored signal-unit PKG prepared for drawing: the pole/arm/housing
/// body and each lamp state's glow geometry as its own part list.
pub(crate) struct SignalModel {
    body: Vec<ModelPart>,
    lamps: Vec<(LampState, Vec<ModelPart>)>,
    /// Lift (m) that stands the body's lowest vertex on the anchor —
    /// the unbound-prop ground convention (`stamp_content_offset`).
    lift: f32,
    /// How far the body reaches along its local −X (the mast arm), m.
    reach: f32,
}

/// The signal-unit models a session draws with, resolved once.
#[derive(Default)]
pub(crate) struct SignalAssets {
    single: Option<Arc<SignalModel>>,
    dual: Option<Arc<SignalModel>>,
    /// Authored model names that did not resolve to drawable geometry —
    /// reported, never replaced by a placeholder.
    pub(crate) missing: Vec<String>,
}

impl SignalAssets {
    /// Resolve the single/dual signal-unit models: the city aimap's
    /// `[Traffic Lights]` line when it names two models, else the
    /// executable's defaults (WLD-35).
    pub(crate) fn load(
        vfs: &Vfs,
        stem: &str,
        images: &mut Assets<Image>,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Self {
        let (single, dual) = signal_model_names(vfs, stem);
        let mut out = Self::default();
        let mut cache: HashMap<String, Option<Arc<SignalModel>>> = HashMap::new();
        for (slot, name) in [(0, single), (1, dual)] {
            let model = cache
                .entry(name.clone())
                .or_insert_with(|| {
                    let built = build_signal_model(vfs, &name, images, meshes, materials);
                    if built.is_none() {
                        warn!(model = %name, "signal unit model missing or empty — heads draw no geometry");
                    }
                    built.map(Arc::new)
                })
                .clone();
            if model.is_none() {
                out.missing.push(name);
            }
            if slot == 0 {
                out.single = model;
            } else {
                out.dual = model;
            }
        }
        out
    }

    /// The model a head with `lanes` approach lanes draws: the dual
    /// unit when those lanes ([`SIGNAL_LANE_PITCH`] each) are wider than
    /// the single unit's mast arm reaches, else the single. The
    /// original's selection rule is unrecovered; this one is designed
    /// from the measured arm reach (WLD-35).
    fn model_for(&self, lanes: usize) -> Option<&Arc<SignalModel>> {
        match (&self.single, &self.dual) {
            (Some(single), Some(dual)) if lanes as f32 * SIGNAL_LANE_PITCH > single.reach => {
                Some(dual)
            }
            (Some(single), _) => Some(single),
            (None, dual) => dual.as_ref(),
        }
    }
}

/// The map's `[Traffic Lights]` single/dual model names: the city
/// aimap's line when it carries two names, else the executable defaults.
fn signal_model_names(vfs: &Vfs, stem: &str) -> (String, String) {
    let authored = mm2_content::load_city_aimap(vfs, stem)
        .ok()
        .flatten()
        .and_then(|aimap| {
            let line = aimap.traffic_lights.first()?.clone();
            let mut names = line.split_whitespace();
            Some((names.next()?.to_string(), names.next()?.to_string()))
        });
    authored.unwrap_or_else(|| (DEFAULT_SINGLE_MODEL.into(), DEFAULT_DUAL_MODEL.into()))
}

/// Build one signal unit from its PKG: the non-lamp chunks as the body,
/// each day lamp chunk as a per-state part list drawn unlit.
fn build_signal_model(
    vfs: &Vfs,
    name: &str,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<SignalModel> {
    use mm2_formats::pkg::{Pkg, PkgChunk, lod_split};
    let resolved = vfs.resolve_preferred(&format!("geometry/{name}"), &["pkg"])?;
    let pkg = Pkg::parse(&vfs.read(&resolved).ok()?).ok()?;
    let stem_of = |file: &mm2_formats::pkg::PkgFile| lod_split(&file.name).0;
    let is_geo = |file: &mm2_formats::pkg::PkgFile| matches!(file.data, PkgChunk::Geometry(_));

    let mut body_pkg = pkg.clone();
    body_pkg
        .files
        .retain(|f| !is_geo(f) || !LampState::is_lamp_stem(&stem_of(f)));
    let verts = mm2_game::local_geometry_verts(&body_pkg);
    if verts.is_empty() {
        return None;
    }
    let lift = -verts.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
    let reach = -verts.iter().map(|v| v[0]).fold(f32::MAX, f32::min);

    // Phase 1 — meshes and base textures through the shared PKG path.
    let mut mats = crate::city::MaterialCache::new(vfs, images, materials);
    let mut missing_prims = 0usize;
    let body = crate::city::pkg_paint_parts(&body_pkg, &mut mats, meshes, &mut missing_prims, 0);
    let mut lamp_parts: Vec<RawLampPart> = Vec::new();
    for file in &pkg.files {
        let PkgChunk::Geometry(geo) = &file.data else {
            continue;
        };
        let Some(state) = LampState::from_day_stem(&stem_of(file)) else {
            continue;
        };
        for section in &geo.sections {
            let Some(shader) = pkg.shaders().and_then(|s| {
                usize::try_from(section.shader_offset)
                    .ok()
                    .and_then(|i| s.shaders.get(i))
            }) else {
                continue;
            };
            let mut one = pkg.clone();
            one.files.retain(|f| !is_geo(f) || f.name == file.name);
            if let Some(PkgChunk::Geometry(g)) = one
                .files
                .iter_mut()
                .find(|f| is_geo(f))
                .map(|f| &mut f.data)
            {
                g.sections = vec![section.clone()];
            }
            let base = mats.get(&shader.texture);
            let emissive = shader.emissive;
            let color = if emissive[..3].iter().any(|c| *c > 0.0) {
                emissive
            } else {
                shader.diffuse
            };
            for (mesh, _) in
                crate::city::pkg_paint_parts(&one, &mut mats, meshes, &mut missing_prims, 0)
            {
                lamp_parts.push((state, mesh, base.clone(), color));
            }
        }
    }
    if missing_prims > 0 {
        warn!(model = %name, missing_prims, "signal unit: non-triangle primitives skipped");
    }
    drop(mats);

    // Phase 2 — lamp materials: unlit and tinted with the authored
    // glow colour, blended over the housing.
    let mut lamps: Vec<(LampState, Vec<ModelPart>)> = Vec::new();
    for (state, mesh, base, color) in lamp_parts {
        let mut material = materials.get(&base).cloned().unwrap_or_default();
        material.base_color = Color::srgb(color[0], color[1], color[2]);
        material.unlit = true;
        material.cull_mode = None;
        material.alpha_mode = AlphaMode::Blend;
        let part = (mesh, materials.add(material));
        match lamps.iter_mut().find(|(s, _)| *s == state) {
            Some((_, parts)) => parts.push(part),
            None => lamps.push((state, vec![part])),
        }
    }
    Some(SignalModel {
        body,
        lamps,
        lift,
        reach,
    })
}

/// A lamp-state mesh under a signal head: shown while its state is lit.
#[derive(Component)]
pub struct SignalLamp(LampState);

/// One authored signal head a session renders (F10-B.7): its junction,
/// the member road it governs, the authored rule that selects its
/// aspect, whether the road is in the junction's light-controlled
/// member set, and the authored world position.
pub(crate) struct SignalHead {
    pub junction: u16,
    pub road: u16,
    pub rule: Option<VehicleRule>,
    /// Whether `road` is in `Junctions::signal_members` for `junction`
    /// — a `TrafficLight` head whose road is not shows the green
    /// fallback, not a phase-dependent aspect.
    pub member: bool,
    pub origin: Vec3,
    /// Horizontal unit vector the unit's lamps face: along the road,
    /// away from the junction, toward the traffic the head governs.
    pub facing: Vec3,
    /// Vehicle lanes the head's road feeds into its junction — selects
    /// the single or dual unit.
    pub approach_lanes: usize,
}

/// The horizontal direction a head faces. The authored axis is a unit
/// vector across the carriageway stored as `origin + axis`
/// (docs/research/bai.md), so the lamps face along the road, at right
/// angles to it; its sign is mixed and tracks nothing measurable, so
/// the facing is signed to point away from the junction (designed —
/// heads face the approaching traffic). A head with no usable axis (4
/// SF ends author `axis == origin`) faces straight away from the
/// junction centre.
fn signal_facing(origin: Vec3, axis_point: Vec3, center: Vec3) -> Vec3 {
    let outward = Vec3::new(origin.x - center.x, 0.0, origin.z - center.z);
    let across = Vec3::new(axis_point.x - origin.x, 0.0, axis_point.z - origin.z);
    let along = across
        .try_normalize()
        .map(|a| Vec3::new(-a.z, 0.0, a.x))
        .filter(|f| f.is_finite());
    match (along, outward.try_normalize()) {
        (Some(f), Some(o)) if f.dot(o) < 0.0 => -f,
        (Some(f), _) => f,
        (None, Some(o)) => o,
        (None, None) => Vec3::Z,
    }
}

/// Enumerate the authored signal heads a session renders: every
/// [`NavGraph::signals`] entry in authored order that passes the
/// [`SIGNAL_MAX_DISTANCE`] sanity bound against its junction zone (a
/// wild outlier is counted in the returned drop count rather than drawn
/// kilometres off the road). Both the authority that spawns the heads
/// and a networked client that renders them share this so the two
/// derive the same scene from the same `.bai`.
pub(crate) fn authored_signal_heads(
    graph: &NavGraph,
    policy: &JunctionPolicy,
) -> (Vec<SignalHead>, usize) {
    let mut heads = Vec::new();
    let mut dropped = 0usize;
    let mut members: HashMap<u16, Vec<u16>> = HashMap::new();
    for head in graph.signals() {
        let ix = head.junction;
        let origin = Vec3::from(head.signal.origin);
        let Some(center) = junction_zone(graph, ix, policy)
            .map(|(center, _)| Vec3::from(center))
            .filter(|center| center.is_finite() && origin.distance(*center) <= SIGNAL_MAX_DISTANCE)
        else {
            dropped += 1;
            continue;
        };
        let member_set = members
            .entry(ix)
            .or_insert_with(|| Junctions::signal_members(graph, ix));
        heads.push(SignalHead {
            junction: ix,
            road: head.road,
            rule: head.rule(),
            member: member_set.contains(&head.road),
            origin,
            facing: signal_facing(
                origin,
                Some(Vec3::from(head.signal.axis))
                    .filter(|a| *a != Vec3::ZERO)
                    .unwrap_or(origin),
                center,
            ),
            approach_lanes: graph.road(head.road).map_or(0, |road| {
                road.arcs
                    .iter()
                    .flatten()
                    .map(|a| graph.arc(*a))
                    .filter(|arc| arc.exit == ArcEnd::Intersection(ix))
                    .map(|arc| arc.lanes.len())
                    .max()
                    .unwrap_or(0)
            }),
        });
    }
    (heads, dropped)
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
    /// Whether the road is in the junction's light-controlled member
    /// set — precomputed at spawn so a client applying the host's phase
    /// ([`crate::signalnet`]) needs no graph of its own.
    pub member: bool,
    /// The aspect currently applied — the update skips entities whose
    /// state did not change.
    pub aspect: SignalAspect,
}

impl AmbientTraffic {
    /// The aimap overrides this session's plan runs under (city aimap
    /// layered with the event's) — exposed so teardown tests can see
    /// that an event's exceptions do not outlive it.
    pub fn overrides(&self) -> &NavOverrides {
        &self.overrides
    }

    /// The merged roster the class draws index — what a frame's roster
    /// digest vouches for.
    pub fn roster(&self) -> &AmbientRoster {
        &self.roster
    }

    /// The navigation graph the cars follow — exposed for diagnostics
    /// and tests that resolve a [`LaneCursor`] to a world pose.
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }

    /// Ever-spawned cars per authored vehicle id, sorted by id — the
    /// F10-AC01 type census. Roster rows that repeat an id (london
    /// authors `va_compact_s` twice) are merged.
    pub fn type_census(&self) -> Vec<(String, usize)> {
        let mut by_id = std::collections::BTreeMap::new();
        for (class, n) in &self.spawned_by_class {
            if let Some(spec) = self.roster.entries.get(*class) {
                *by_id.entry(spec.id.clone()).or_insert(0) += n;
            }
        }
        by_id.into_iter().collect()
    }

    /// Lane followers whose pose and velocity agree with the authored
    /// travel direction at their cursor: `(agreeing, audited)`
    /// (F10-AC01). A car agrees when its nose lies within ~8 degrees of
    /// the lane (or crossing) tangent and it is not moving backwards
    /// along it. Knocked wrecks and cars whose cursor no longer
    /// samples are not audited.
    pub fn heading_audit<'a>(
        &self,
        cars: impl Iterator<Item = (&'a AmbientCar, &'a Transform, &'a LinearVelocity)>,
    ) -> (usize, usize) {
        let (mut ok, mut total) = (0, 0);
        for (car, transform, velocity) in cars {
            if car.drive != AmbientDrive::Lane {
                continue;
            }
            let Some(sample) = car.cursor.sample(&self.graph) else {
                continue;
            };
            let tangent = Vec3::from(sample.tangent).normalize_or_zero();
            total += 1;
            let aligned = transform.forward().dot(tangent) > 0.99;
            let forward_speed = velocity.0.dot(tangent) >= 0.0;
            if aligned && forward_speed {
                ok += 1;
            }
        }
        (ok, total)
    }

    /// The junction controller beside the graph it reads, so a caller
    /// outside the lane drivers (the cable car) can gate its own route
    /// through [`Junctions::gate_approach`].
    pub fn junctions_and_graph(&mut self) -> (&mut Junctions, &NavGraph) {
        (&mut self.junctions, &self.graph)
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

/// A body that is neither a participant nor an ambient car but sits on
/// the road network — a cable car — which ambient cars must queue
/// behind rather than drive into. [`drive_ambient`] senses it along the
/// body's local +Z axis (nose first) at a car's own sensing grain.
#[derive(Component, Debug, Clone, Copy)]
pub struct RoadObstacle {
    /// Distance from the body's origin to its front, m.
    pub nose: f32,
    /// Distance from the body's origin back to its tail, m.
    pub tail: f32,
}

impl RoadObstacle {
    /// Points standing for the body in the corridor sense, which reads
    /// centres: the origin plus a nose and a tail point one car's
    /// half-length (2 m) inside each end, so a car stops short of the
    /// body's edge and not of its middle.
    pub fn sense_points(&self, at: Vec3, rotation: Quat) -> impl Iterator<Item = Vec3> {
        let reach = |end: f32| (end.min(12.0) - OBSTACLE_END_INSET).max(0.0);
        let fwd = rotation * Vec3::Z;
        [at, at + fwd * reach(self.nose), at - fwd * reach(self.tail)]
            .into_iter()
            .filter(|p| p.is_finite())
    }
}

/// How far inside a [`RoadObstacle`]'s ends its sensing points sit (m):
/// about half a car, so an ambient car queues as it would behind one.
const OBSTACLE_END_INSET: f32 = 2.0;

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

/// Whether a session fields ambient traffic at all (F26-A). Offline
/// sessions always do. A networked one does only in free-roam Cruise:
/// MP-4 (documented) says multiplayer *races* carry no ambient traffic,
/// cops or AI opponents, and the help text names no Cruise exception —
/// so traffic in networked Cruise is an enhanced policy (the F26 spec
/// asks Cruise clients to share traffic), not a verified original rule.
/// On a `Host` the authority simulates the cars and `worldtraffic`
/// replicates them; a `Remote` client only receives them.
pub fn fields_ambient_traffic(config: &SessionConfig) -> bool {
    config.authority == SessionAuthority::Local || config.mode == mm2_game::SessionMode::Cruise
}

/// Load the ambient setup for this session and spawn the initial plan.
/// Returns the resource the caller inserts — `None` when the session
/// does not field traffic ([`fields_ambient_traffic`]: networked races,
/// and a `Remote` client, whose cars are the host's copies), the world
/// is not a city, or no layer authors a roster. All failures log and degrade to *no*
/// ambient traffic; ambient cars never sink an otherwise loadable
/// session.
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
    // Only the process that runs the rules simulates lane followers:
    // a `Remote` client's cars are copies of the host's, spawned by
    // `worldtraffic` from its frames, never a second local world.
    if !fields_ambient_traffic(config) || config.authority == SessionAuthority::Remote {
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
    let mut policy = SpawnPolicy::default();
    if let Some(separation) = config.dev.ambient_separation {
        policy.min_ambient_separation = separation;
    }
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

    let closed_roads = setup.overrides.closed_roads.len();
    let exceptions = setup.overrides.exceptions.len();
    let default_speed_limit = setup.overrides.default_speed_limit;
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
        spawned_by_class: std::collections::BTreeMap::new(),
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
        signal_models_missing: Vec::new(),
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
            *traffic.spawned_by_class.entry(directive.class).or_insert(0) += 1;
        }
    }
    let signal_assets = SignalAssets::load(vfs, stem, images, meshes, materials);
    traffic.signal_models_missing = signal_assets.missing.clone();
    traffic.issues.extend(
        signal_assets
            .missing
            .iter()
            .map(|m| format!("signal unit model {m} missing")),
    );
    spawn_traffic_signals(commands, &mut traffic, owner, &signal_assets);
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
        closed_roads,
        exceptions,
        default_speed_limit,
        signals = traffic.signals,
        signals_dropped = traffic.signals_dropped,
        issues = traffic.issues.len(),
        "ambient traffic loaded"
    );
    Some(traffic)
}

/// Spawn the authored traffic-signal units (F10-B.7): one session-owned
/// head at every `NavGraph::signals` entry that passes the
/// `SIGNAL_MAX_DISTANCE` sanity bound, drawn as the retail `sp_traflit*`
/// prop (WLD-35). Each head shows the aspect its end's authored rule
/// admits under the junction controller's initial state — a head whose
/// road never joins the member set resolves the same green fallback
/// `gate` does; `drive_signals` keeps them current. The unit stands on
/// the authored origin, its lamps facing along the road away from the
/// junction (see [`signal_facing`]).
fn spawn_traffic_signals(
    commands: &mut Commands,
    traffic: &mut AmbientTraffic,
    owner: SessionEntity,
    assets: &SignalAssets,
) {
    let (heads, dropped) = authored_signal_heads(&traffic.graph, &JunctionPolicy::default());
    // A wild authored outlier — drop it and count the loss rather than
    // draw a stray lamp kilometres off the road.
    traffic.signals_dropped += dropped;
    let mut greens: HashMap<u16, Option<u16>> = HashMap::new();
    for head in heads {
        let green = *greens
            .entry(head.junction)
            .or_insert_with(|| traffic.junctions.green_road(&traffic.graph, head.junction));
        let aspect = SignalAspect::of_phase(head.member, green, head.road, head.rule);
        spawn_signal_head(commands, owner, assets, &head, aspect);
        traffic.signals += 1;
    }
}

/// Spawn one signal head: the logic entity at the authored origin,
/// rotated so the unit's local +Z (the lamp face) points along
/// `head.facing`, with the body and every lamp state as children — only
/// the lamps lit by `aspect` visible.
fn spawn_signal_head(
    commands: &mut Commands,
    owner: SessionEntity,
    assets: &SignalAssets,
    head: &SignalHead,
    aspect: SignalAspect,
) {
    let face = head.facing;
    let rotation = Quat::from_mat3(&Mat3::from_cols(Vec3::Y.cross(face), Vec3::Y, face));
    let mut entity = commands.spawn((
        owner,
        TrafficSignal {
            junction: head.junction,
            road: head.road,
            rule: head.rule,
            member: head.member,
            aspect,
        },
        Transform::from_translation(head.origin).with_rotation(rotation),
        Visibility::Visible,
    ));
    let Some(model) = assets.model_for(head.approach_lanes) else {
        return;
    };
    entity.with_children(|children| {
        let lift = Transform::from_xyz(0.0, model.lift, 0.0);
        for (mesh, material) in &model.body {
            children.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), lift));
        }
        for (state, parts) in &model.lamps {
            let visibility = if state.lit(aspect) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            for (mesh, material) in parts {
                children.spawn((
                    SignalLamp(*state),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    lift,
                    visibility,
                ));
            }
        }
    });
}

/// Spawn the client's traffic-signal heads for a `Remote` city session
/// (F26-A.4b): the same authored heads the authority draws, at their
/// authored positions, from the same `.bai` both peers share. Each
/// head starts at its phase-independent aspect (a light member starts
/// red until the host's [`Message::Signals`](mm2_net::Message::Signals)
/// frame names its phase); [`crate::signalnet::apply_signals`] then
/// keeps the light members on the host's phase. Returns `false` when
/// the graph cannot load (the session simply renders no signals, like a
/// failed ambient load).
pub(crate) fn spawn_client_signals(
    commands: &mut Commands,
    vfs: &Vfs,
    stem: &str,
    owner: SessionEntity,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> bool {
    let build = match mm2_content::load_nav_graph(vfs, stem) {
        Ok(build) => build,
        Err(e) => {
            warn!(error = %e, "signal replica: nav graph failed — no signal heads");
            return false;
        }
    };
    let assets = SignalAssets::load(vfs, stem, images, meshes, materials);
    let (heads, _dropped) = authored_signal_heads(&build.graph, &JunctionPolicy::default());
    for head in heads {
        let aspect = SignalAspect::of_phase(head.member, None, head.road, head.rule);
        spawn_signal_head(commands, owner, &assets, &head, aspect);
    }
    true
}

/// Keep every authored signal head's aspect in step with the
/// authoritative [`Junctions`] controller — runs after
/// `drive_ambient` so the just-advanced phase is what the lamps show
/// (F10-B.7). Rule admission only: a light member is green exactly
/// while it holds the phase (red through the all-red clearance),
/// `NeverStop`/unruled ends stay green, `StopSign` ends show the stop
/// aspect, `AlwaysStop` ends stay red. The box-yield that holds cars
/// behind a green is an obstruction rule, not a lamp state. The lamp
/// meshes follow in [`sync_signal_lamps`].
pub fn drive_signals(
    session: Res<Session>,
    traffic: Option<Res<AmbientTraffic>>,
    mut signals: Query<&mut TrafficSignal>,
) {
    let Some(traffic) = traffic else { return };
    if session.authority_role() != AuthorityRole::Authority {
        return;
    }
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    ) {
        return;
    }
    // The junction's green is per-intersection — cache it rather than
    // rescanning the member set for every head at the junction.
    let mut greens: HashMap<u16, Option<u16>> = HashMap::new();
    for mut sig in &mut signals {
        let green = *greens
            .entry(sig.junction)
            .or_insert_with(|| traffic.junctions.green_road(&traffic.graph, sig.junction));
        let aspect = SignalAspect::of_phase(sig.member, green, sig.road, sig.rule);
        if aspect != sig.aspect {
            sig.aspect = aspect;
        }
    }
}

/// Show exactly the lamp geometry the head's aspect lights: the
/// original toggles authored glow meshes rather than recolouring one
/// (WLD-35). Runs for heads whose aspect changed, on the authority and
/// on a client applying the host's phase alike.
pub fn sync_signal_lamps(
    heads: Query<(&TrafficSignal, &Children), Changed<TrafficSignal>>,
    mut lamps: Query<(&SignalLamp, &mut Visibility)>,
) {
    for (head, children) in &heads {
        for child in children.iter() {
            if let Ok((lamp, mut visibility)) = lamps.get_mut(child) {
                let want = if lamp.0.lit(head.aspect) {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                if *visibility != want {
                    *visibility = want;
                }
            }
        }
    }
}

/// Resolve a class's assets once and cache the outcome. A class whose
/// model or collider cannot be produced caches `None` — it counts
/// unspawnable like a tuning-less row rather than retrying every tick.
/// The ambient engine table resolves alongside (F07-B.6): a malformed
/// resolved file warns once per class, an absent/sentinel one is
/// authored silence — neither blocks the spawn.
pub(crate) fn class_assets(vfs: &Vfs, spec: &AmbientSpec) -> Option<AmbientClass> {
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
    box_collider(size, cg)
}

/// The convex hull of the box `size` centred on `cg`; a cuboid of
/// `size` when the corners are degenerate.
fn box_collider(size: Vec3, cg: Vec3) -> Collider {
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
/// other ambient car and [`RoadObstacle`] (a cable car), then the `follow_speed` law sets its kinematic
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
    obstacles: Query<
        (Entity, &Position, &Rotation, &RoadObstacle),
        (Without<AmbientCar>, Without<Player>),
    >,
    mut commands: Commands,
) {
    let Some(mut traffic) = traffic else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
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
    // Cable cars: a body the sense reads at its nose and tail as well.
    for (e, p, r, body) in &obstacles {
        blockers.extend(body.sense_points(p.0, r.0).map(|at| (e, at)));
    }
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
        let was = previous
            .sample(&traffic.graph)
            .map_or(position.0, |s| Vec3::from(s.position));
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
        // Measured lane-to-lane: the body sits where `drape_ambient`
        // rested it, up to `DRAPE_RISE` off the lane, and that offset
        // is not a step the car drove.
        let step_limit = car.speed.max(0.0) * dt * 2.0 + 1.0;
        if pos.distance(was) > step_limit {
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

/// How far above the lane curve the ground probe starts. BAI lane
/// heights are interpolated between road sections, and on SF's hills
/// they leave the PSDL road surface by up to ±1.9 m (measured on
/// retail `city/sf`, 2026-10-03) — the window covers that with room
/// to spare while staying under any deck a car could fit beneath.
const DRAPE_RISE: f32 = 2.5;
/// How far below the lane curve the ground probe still looks.
const DRAPE_DROP: f32 = 2.5;
/// Fraction of the class's authored half-length/half-width the four
/// ground probes sit at — roughly where the tyres meet the road.
/// Designed, not recovered.
const DRAPE_FOOTPRINT: f32 = 0.8;

/// Rest a lane pose on the ground under its four corners.
///
/// `half` is the probe footprint's (half-width, half-length) in the
/// car's frame; `ground` answers the ground height under a world
/// point, or `None` where there is none in reach. Returns the pose
/// whose origin sits on the plane through the four ground points and
/// whose up axis is that plane's normal — pitch and camber follow the
/// road rather than the lane curve. `None` when any corner misses the
/// ground or the fit tips past 60°: the caller keeps the lane pose,
/// which is never worse than before draping existed.
fn drape_pose(
    pos: Vec3,
    rot: Quat,
    half: Vec2,
    ground: impl Fn(Vec3) -> Option<f32>,
) -> Option<(Vec3, Quat)> {
    let corner = |x: f32, z: f32| {
        let p = pos + rot * Vec3::new(x * half.x, 0.0, z * half.y);
        ground(p).map(|y| Vec3::new(p.x, y, p.z))
    };
    // Bevy convention: -Z is forward, +X is right.
    let fl = corner(-1.0, -1.0)?;
    let fr = corner(1.0, -1.0)?;
    let rl = corner(-1.0, 1.0)?;
    let rr = corner(1.0, 1.0)?;
    let forward = ((fl + fr) - (rl + rr)).normalize_or_zero();
    let right = ((fr + rr) - (fl + rl)).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    if up.y < 0.5 || forward == Vec3::ZERO {
        return None;
    }
    let right = forward.cross(up);
    let rot = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    Some(((fl + fr + rl + rr) * 0.25, rot))
}

/// Rest every lane-following ambient car on the road surface.
///
/// `drive_ambient` poses a car on its BAI lane curve, which is sampled
/// linearly between authored road sections — over a crest or a dip
/// the curve cuts above or below the PSDL road the player drives on,
/// and the car floats or sinks by up to a couple of metres. This
/// probes the city's ground colliders (only room colliders carry a
/// [`SurfaceMaterial`], so other cars, props and bangers never catch
/// the ray) under the car's four corners and rewrites the pose onto
/// them: height, pitch and camber from the road, travel position and
/// heading from the lane. The lane cursor is untouched, so the drape
/// is recomputed from the lane every tick and never accumulates.
///
/// Runs right after `drive_ambient`, under the same phase/authority
/// gate, so the solver and the renderer only ever see the draped
/// pose. A car whose corners do not all find ground keeps its lane
/// pose.
#[allow(clippy::type_complexity)] // Bevy system: the ParamSet is the system's actual signature
pub fn drape_ambient(
    session: Res<Session>,
    traffic: Option<Res<AmbientTraffic>>,
    ground: Query<(), With<SurfaceMaterial>>,
    mut world: ParamSet<(
        SpatialQuery,
        Query<(
            Entity,
            &AmbientCar,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut Transform,
        )>,
    )>,
) {
    let Some(traffic) = traffic else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
        )
    {
        return;
    }
    // The spatial query reads every collider's pose, ambient cars
    // included, so the lane poses are read out first and the draped
    // ones written back after.
    let lane_poses: Vec<(Entity, Vec3, Quat, Vec2)> = world
        .p1()
        .iter()
        .filter(|(_, car, ..)| car.drive == AmbientDrive::Lane)
        .map(|(entity, car, position, rotation, ..)| {
            let size = traffic
                .roster
                .entries
                .get(car.class)
                .and_then(|spec| spec.tuning.as_ref())
                .map_or([2.0, 1.5, 5.0], |t| t.size);
            let half = Vec2::new(size[0], size[2]) * 0.5 * DRAPE_FOOTPRINT;
            (entity, position.0, rotation.0, half)
        })
        .collect();
    let draped: Vec<(Entity, Vec3, Quat)> = {
        let spatial = world.p0();
        let filter = SpatialQueryFilter::default();
        let ground_at = |p: Vec3| {
            let origin = p + Vec3::Y * DRAPE_RISE;
            // Avian's BVH asserts a finite ray origin.
            if !origin.is_finite() {
                return None;
            }
            spatial
                .cast_ray_predicate(
                    origin,
                    Dir3::NEG_Y,
                    DRAPE_RISE + DRAPE_DROP,
                    true,
                    &filter,
                    &|e| ground.contains(e),
                )
                .map(|hit| origin.y - hit.distance)
        };
        lane_poses
            .into_iter()
            .filter_map(|(entity, pos, rot, half)| {
                drape_pose(pos, rot, half, ground_at).map(|(pos, rot)| (entity, pos, rot))
            })
            .collect()
    };
    let mut cars = world.p1();
    for (entity, pos, rot) in draped {
        let Ok((_, car, mut position, mut rotation, mut velocity, mut transform)) =
            cars.get_mut(entity)
        else {
            continue;
        };
        position.0 = pos;
        rotation.0 = rot;
        velocity.0 = rot * Vec3::NEG_Z * car.speed.max(0.0);
        *transform = Transform::from_translation(pos).with_rotation(rot);
    }
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
    /// The car's velocity along `dir` the instant before its launch,
    /// captured in the flip pass — the striker correction's velocity
    /// target needs it and the correction pass runs after every flip,
    /// so it cannot be re-read there. `None` until this knock's car
    /// actually hands over; an edge the apply pass's `Lane` re-check
    /// drops never sets it and owes no striker debit.
    struck_pre: Option<f32>,
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
/// its velocity. When a mass cannot be resolved the
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
/// F10-B.15: flips and striker corrections run as two passes — a
/// striker that is itself a lane car is `Knocked` before its
/// striker-side debts are charged, whatever the edge order — and the
/// correction compounds per striker instead of re-targeting per edge:
/// the striker's first committed edge writes the velocity target
/// (returns the wall and pays the exchange in one step), every later
/// edge is a pure `-dir · J` debit so one striker striking two cars
/// pays both transfers instead of having its second write erase the
/// first's payment. A striker this pass itself flipped starts from
/// its post-flip launch — a kinematic–kinematic edge charged it no
/// wall — so its striker edges owe only the pure debit: a same-tick
/// daisy chain carries the momentum debt through rather than the last
/// car launching free.
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
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
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
                struck_pre: None,
            });
        }
    }

    // Flip pass first, corrections second (F10-B.15): a striker that
    // is itself a lane car this pass flips must be `Knocked` by the
    // time its striker-side debts are charged, regardless of edge
    // order, so the two passes cannot interleave.
    //
    // `handed_over` — cars this pass flipped, recorded as
    // `(car, its edge's striker)` — and `corrected` — strikers that
    // already took a correction — shape the second pass: a striker
    // owes each committed transfer independently (one striker plowing
    // through two cars pays both), except on the follower-follower
    // edge where the striker is the same pair's other side — its own
    // launch there already *is* its share.
    let mut handed_over: Vec<(Entity, Entity)> = Vec::new();
    let mut corrected: Vec<Entity> = Vec::new();
    for knock in &mut knocks {
        // The car's velocity along the push direction before its
        // launch — the striker correction's velocity target needs it.
        // A later edge can name a car an earlier edge already flipped
        // — the `Lane` re-check is the dedup.
        let Ok((mut car, mut linvel, mut angvel, inertia, rotation)) = cars.get_mut(knock.entity)
        else {
            continue;
        };
        if car.drive != AmbientDrive::Lane {
            continue;
        }
        car.drive = AmbientDrive::Knocked;
        knock.struck_pre = Some(linvel.0.dot(knock.dir));
        linvel.0 += knock.dir * knock.launch;
        angvel.0 += angular_share(knock.lever, knock.dir * knock.impulse, inertia, rotation);
        // Write-side spin bound (the solver-side `MaxAngularSpeed`
        // clamps the integration too): an unbounded lever share
        // would leave the wreck spinning fast enough to inflate a
        // later contact's approach-speed reading — the same
        // cascade amplifier the banger bound closes.
        angvel.0 = angvel.0.clamp_length_max(MAX_BANGER_ANGULAR_SPEED);
        handed_over.push((knock.entity, knock.striker));
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
    }

    for knock in &knocks {
        let (Some(struck_pre), Some(transfer)) = (knock.struck_pre, knock.transfer.as_ref()) else {
            continue;
        };
        // The striker keeps its share of the exchange: its velocity
        // along the push direction becomes its approach component
        // (`struck_pre` + `severity` reconstructs the pair's
        // pre-solver closing) minus the transferred impulse over its
        // mass. Write the velocity target rather than returning the
        // manifold's recorded impulse —
        // `total_impulse` accumulates penetration-recovery and
        // restitution passes, so on a kinematic pair it over-reports
        // the wall response the striker actually took and charging it
        // back would inject energy. A lane-follower striker takes no
        // correction: `drive_ambient` owns its velocity — which also
        // keeps a follower-follower edge from charging the exchange
        // twice (the other side's own launch is its share).
        //
        // F10-B.15: the target write is only the striker's *first*
        // debit this drain — it returns the wall response the solver
        // charged along `dir` and pays the exchange in one step.
        // Every later edge of the same striker is a pure impulse
        // debit (`-dir · J`): a second target write along a shared
        // direction would erase the first edge's payment — the
        // striker would pay one share while both struck cars launch,
        // injecting the other transfer outright. The same pure debit
        // applies when the striker is a car this pass itself flipped
        // on a *different* pair: its post-flip velocity is already
        // its free post-exchange state (a kinematic–kinematic edge
        // charged it no wall), so a same-tick daisy chain carries
        // the debt through instead of the last car launching free.
        // The one case still owed nothing is the follower-follower
        // edge where the striker's own flip came off this same pair:
        // its struck-side launch already is its share of the exchange.
        let target = struck_pre + knock.severity - transfer.impulse / transfer.striker_mass;
        let debit = knock.dir * -transfer.impulse;
        if let Ok((linvel, angvel, inertia, rotation)) = strikers.get_mut(knock.striker) {
            let delta = if corrected.contains(&knock.striker) {
                debit
            } else {
                corrected.push(knock.striker);
                knock.dir * (target - linvel.0.dot(knock.dir)) * transfer.striker_mass
            };
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
            // lane follower's velocity is owned by `drive_ambient`.
            // A wreck not handed over this pass is a free dynamic
            // body whose first debit is the wall-returning target
            // write like any striker. A car this pass itself flipped
            // owes the pure debit when its flip came off a *different*
            // pair, and nothing at all on the same pair — its
            // struck-side launch there already was its share.
            if striker_car.drive == AmbientDrive::Knocked
                && !handed_over.contains(&(knock.striker, knock.entity))
            {
                let delta = if corrected.contains(&knock.striker)
                    || handed_over.iter().any(|&(car, _)| car == knock.striker)
                {
                    debit
                } else {
                    corrected.push(knock.striker);
                    knock.dir * (target - linvel.0.dot(knock.dir)) * transfer.striker_mass
                };
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
    // Same live-phase gate `drive_ambient` runs under — a paused
    // session freezes its population instead of churning respawns
    // behind the overlay. `Results` stays live: the field races on
    // behind the results screen (DSN-11), through live traffic.
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
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
                    *traffic.spawned_by_class.entry(directive.class).or_insert(0) += 1;
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

#[cfg(test)]
mod drape_tests {
    //! `drape_pose` rests a lane pose on the ground under its corners:
    //! height, pitch and camber come from the ground, heading from the
    //! lane.

    use bevy::prelude::*;

    use super::drape_pose;

    const HALF: Vec2 = Vec2::new(0.8, 2.0);

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn a_lane_above_flat_ground_drops_onto_it_keeping_its_heading() {
        let rot = Quat::from_rotation_y(0.7);
        let (pos, draped) = drape_pose(Vec3::new(3.0, 1.2, -4.0), rot, HALF, |_| Some(0.0))
            .expect("flat ground under every corner");
        assert!(close(pos, Vec3::new(3.0, 0.0, -4.0)));
        assert!(close(draped * Vec3::NEG_Z, rot * Vec3::NEG_Z));
        assert!(close(draped * Vec3::Y, Vec3::Y));
    }

    #[test]
    fn a_level_lane_on_a_hill_takes_its_pitch() {
        // Ground rising 0.3 m per metre of travel toward -Z.
        let (pos, rot) = drape_pose(Vec3::ZERO, Quat::IDENTITY, HALF, |p| Some(-0.3 * p.z))
            .expect("the slope is under every corner");
        assert!(close(pos, Vec3::ZERO));
        let forward = rot * Vec3::NEG_Z;
        assert!(close(forward, Vec3::new(0.0, 0.3, -1.0).normalize()));
        assert!(close(rot * Vec3::X, Vec3::X));
    }

    #[test]
    fn a_cambered_road_rolls_the_car_and_leaves_travel_level() {
        // Ground rising 0.1 m per metre to the car's right.
        let (_, rot) = drape_pose(Vec3::ZERO, Quat::IDENTITY, HALF, |p| Some(0.1 * p.x))
            .expect("the camber is under every corner");
        assert!(close(rot * Vec3::NEG_Z, Vec3::NEG_Z));
        assert!(close(rot * Vec3::X, Vec3::new(1.0, 0.1, 0.0).normalize()));
    }

    #[test]
    fn a_corner_without_ground_keeps_the_lane_pose() {
        let ground = |p: Vec3| (p.x < 0.0 || p.z < 0.0).then_some(0.0);
        assert!(drape_pose(Vec3::ZERO, Quat::IDENTITY, HALF, ground).is_none());
    }

    #[test]
    fn ground_too_steep_to_stand_on_keeps_the_lane_pose() {
        // 70° side slope: the fitted up axis leans past 60°.
        let tan = 70f32.to_radians().tan();
        assert!(drape_pose(Vec3::ZERO, Quat::IDENTITY, HALF, |p| Some(tan * p.x)).is_none());
    }
}

#[cfg(test)]
mod size_collider_tests {
    //! Authored `Size`/`CG` reach `Collider::convex_hull` verbatim
    //! (`authored-numbers.md` S4). The vehicle decoders reject
    //! non-finite values; these pin that a finite-but-overflowing box
    //! and the degenerate boxes do not panic either.

    use bevy::prelude::*;

    use super::box_collider;

    #[test]
    fn degenerate_and_extreme_boxes_build_without_panicking() {
        for (size, cg) in [
            (Vec3::ZERO, Vec3::ZERO),
            (Vec3::new(2.0, 0.0, 5.0), Vec3::ZERO),
            (Vec3::splat(-2.0), Vec3::ZERO),
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MAX)),
            (Vec3::splat(1e30), Vec3::splat(1e30)),
        ] {
            let _ = box_collider(size, cg);
        }
    }

    #[test]
    fn an_ordinary_box_is_a_hull() {
        assert!(
            box_collider(Vec3::new(2.0, 1.5, 5.0), Vec3::new(0.0, 0.75, 0.0))
                .shape()
                .as_convex_polyhedron()
                .is_some()
        );
    }
}

#[cfg(test)]
mod signal_unit_tests {
    //! F10 signal-unit geometry: facing, lamp states and the retail
    //! `sp_traflit*` models.

    use super::*;

    #[test]
    fn a_head_faces_along_the_road_away_from_the_junction_whatever_the_axis_sign() {
        let origin = Vec3::new(10.0, 5.0, 0.0);
        let center = Vec3::ZERO;
        // The axis runs across the road (±Z); the face is along ±X.
        for axis in [Vec3::Z, -Vec3::Z] {
            let face = signal_facing(origin, origin + axis, center);
            assert!((face - Vec3::X).length() < 1e-5, "{face:?}");
        }
    }

    #[test]
    fn a_head_with_no_axis_faces_straight_out_of_the_junction() {
        let origin = Vec3::new(0.0, 3.0, -8.0);
        assert!((signal_facing(origin, origin, Vec3::ZERO) + Vec3::Z).length() < 1e-5);
    }

    #[test]
    fn each_aspect_lights_its_own_lamps() {
        assert!(LampState::Green.lit(SignalAspect::Green));
        assert!(LampState::Walk.lit(SignalAspect::Green));
        assert!(!LampState::Red.lit(SignalAspect::Green));
        assert!(LampState::Red.lit(SignalAspect::Red));
        assert!(LampState::NoWalk.lit(SignalAspect::Red));
        assert!(LampState::Yellow.lit(SignalAspect::Stop));
        assert!(!LampState::Green.lit(SignalAspect::Stop));
    }

    #[test]
    fn lamp_stems_are_recognised_by_state_and_time_of_day() {
        assert_eq!(LampState::from_day_stem("redglowday"), Some(LampState::Red));
        assert_eq!(
            LampState::from_day_stem("nowalk_day"),
            Some(LampState::NoWalk)
        );
        assert_eq!(LampState::from_day_stem("greenglownight"), None);
        assert!(LampState::is_lamp_stem("greenglownight"));
        assert!(!LampState::is_lamp_stem(""));
    }

    #[test]
    fn a_missing_model_is_reported_not_replaced() {
        let vfs = Vfs::new();
        let mut images = Assets::<Image>::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let assets = SignalAssets::load(&vfs, "none", &mut images, &mut meshes, &mut materials);
        assert_eq!(
            assets.missing,
            vec![
                DEFAULT_SINGLE_MODEL.to_string(),
                DEFAULT_DUAL_MODEL.to_string()
            ]
        );
        assert!(assets.model_for(4).is_none());
    }

    /// Original-content level: both retail defaults and the London
    /// override resolve with body and all five/three lamp states.
    #[test]
    fn retail_signal_units_load_with_their_lamp_geometry() {
        let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
            eprintln!("skipped: MM2_RETAIL is not set; retail signal models NOT checked");
            return;
        };
        let mut vfs = Vfs::new();
        mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default())
            .expect("the install mounts");
        let mut images = Assets::<Image>::default();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        for (city, single, dual, lamps) in [
            ("sf", "sp_traflitsingle_f", "sp_traflitdual_f", 5),
            (
                "london",
                "sp_traflitsingle_ped_l",
                "sp_traflitsingle_ped_l",
                5,
            ),
        ] {
            assert_eq!(
                signal_model_names(&vfs, city),
                (single.to_string(), dual.to_string())
            );
            let assets = SignalAssets::load(&vfs, city, &mut images, &mut meshes, &mut materials);
            assert!(assets.missing.is_empty(), "{city}: {:?}", assets.missing);
            for model in [&assets.single, &assets.dual] {
                let model = model.as_ref().expect("model resolves");
                assert!(!model.body.is_empty());
                assert_eq!(model.lamps.len(), lamps, "{city}: lamp states");
            }
        }
    }
}
