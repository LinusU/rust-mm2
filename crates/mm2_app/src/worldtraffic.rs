//! Ambient-traffic replication (F26-A): the host's lane followers reach
//! every client as authoritative state.
//!
//! Before this module a networked session fielded no ambient traffic at
//! all (the old MP-4 gate): a lane follower the host simulated was a
//! car no client could see, and a client spawning its own set from local
//! state would have diverged from the first junction. Now, in free-roam
//! Cruise ([`fields_ambient_traffic`]), the authority runs the ordinary
//! `drive_ambient`/`maintain_ambient` population and a client holds
//! *copies*:
//!
//! - **Identity.** The host mints a per-spawn car id the first time it
//!   publishes a car (`ObjectId` slots are process-local and never cross
//!   the wire); the class is the car's index into the ambient roster,
//!   which both peers derive from the same city content. Every frame
//!   carries a digest of the host's roster, and a client whose own
//!   roster differs refuses the rows counted as `mismatched` rather than
//!   pose a different car than the host drives.
//! - **State, not events.** A row is the car's pose, velocity and drive
//!   state (lane follower or knocked wreck). Every live car rides every
//!   frame (bounded by [`MAX_SNAP_CARS`]); a lost or reordered frame
//!   self-corrects on the next, and per-car latest-wins on the frame's
//!   `(generation, tick)` drops a reordered older row.
//! - **Application.** The client never drives a copy. It is a kinematic
//!   body — the predicted car still collides with it — whose pose is
//!   written from the row and carried between frames by the row's
//!   velocity. A knocked wreck stays a kinematic copy posed from the
//!   wire (the host's solver owns it). Copies carry the class's ambient
//!   engine table, so they sound like the host's.
//! - **Retirement.** A copy no frame has carried for [`COPY_TTL_TICKS`]
//!   is despawned: absence from one frame is not a despawn order, so a
//!   lost frame cannot erase the population, and a car the host recycled
//!   leaves on its own. A late joiner needs no snapshot message — the
//!   next frame is already complete.
//!
//! - **Cable cars.** The host's San Francisco cable cars
//!   (`cablecar`; their stops follow the host's signal clock and the
//!   cars on their rails, so a client cannot simulate them) ride the
//!   same frame as [`CAR_CABLE`] rows — id, pose, velocity — and a
//!   client spawns the retail model with its collider on a kinematic
//!   copy. A late joiner sees them wherever the next frame says.
//!
//! Not replicated, by decision: traffic signals (aspect is a pure
//! function of the host's clock that nothing client-side reads), the
//! stuck/queue accounting, and per-client relevancy — the host's own
//! population already lives inside the union of every participant's
//! bubble, and the frame is broadcast. Poses apply as received; the only
//! smoothing is the velocity carry and the car's transform
//! interpolation.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::cablecar::CABLE_CAR_MODEL;
use mm2_game::{
    AmbientAudio, AmbientRoster, Mm2Vfs, ObjectIdentity, Session, SessionAuthority, SessionConfig,
    SessionEntity, SessionPhase, WorldMode,
};
use mm2_net::{MAX_SNAP_CARS, Message, SnapCar};
use tracing::warn;

use crate::cablecar::CableCar;
use crate::car_visual::spawn_vehicle_model;
use crate::city::{MovableModel, MovableModels};
use crate::movers::spawn_body;
use crate::net::{HostLink, LobbyState};
use crate::netdrive::{NetDriveReport, NetPlayer, RemotePick, RemoteSnaps, wire_quat};
use crate::relevancy::InterestSet;
use crate::traffic::fields_ambient_traffic;
use crate::traffic::{AmbientCar, AmbientClass, AmbientDrive, AmbientTraffic, class_assets};
use crate::worldprops::fnv;

/// [`SnapCar::state`]: a lane follower the host's `drive_ambient` poses.
pub const CAR_LANE: u8 = 0;
/// [`SnapCar::state`]: a wreck the host's solver owns.
pub const CAR_KNOCKED: u8 = 1;

/// [`SnapCar::state`]: one of the host's cable cars. `class` is the
/// car's circuit index (informational — the client needs only the
/// pose), and the car rides the same id ledger as the ambient cars.
pub const CAR_CABLE: u8 = 2;

/// Publish every this-many frames: lane followers move smoothly under
/// the velocity carry, so a third of the frame rate holds them.
const PUBLISH_EVERY: u32 = 3;

/// Staged rows a client holds before refusing new keys: a hostile or
/// runaway host cannot grow the inbox without bound.
pub const MAX_STAGED_CARS: usize = 256;

/// Applied-row watermarks a client remembers — the hard backstop on the
/// ledger. At the bound a new car simply goes unwatermarked (a reordered
/// older row for it may re-apply, harmless: the next frame corrects it).
pub const MAX_APPLIED_CARS: usize = 16384;

/// Copies a client keeps alive: the host's own population bound
/// ([`MAX_SNAP_CARS`] rows a frame) with headroom for a car whose
/// retirement is still pending. A row for a new car past it is refused.
pub const MAX_COPIES: usize = 2 * MAX_SNAP_CARS as usize;

/// Session ticks (fixed 60 Hz) a copy survives without a frame
/// carrying it — two seconds: far longer than a frame's gap, short
/// enough that a car the host recycled does not linger.
pub const COPY_TTL_TICKS: u64 = 2 * mm2_game::RACE_TICK_HZ as u64;

/// A copy this far (m) from its row's pose snaps instead of easing:
/// a recycle that reused no id cannot teleport a copy, but a host-side
/// reset or wreck launch can.
const SNAP_DISTANCE: f32 = 3.0;

/// A digest of the roster a frame's class indices are relative to: the
/// row count and every row's authored vehicle id in order. Both peers
/// derive the roster from the same city content, so it agrees unless
/// content differs.
pub fn roster_digest(roster: &AmbientRoster) -> u64 {
    let mut hash = fnv(
        0xcbf2_9ce4_8422_2325,
        &(roster.entries.len() as u64).to_le_bytes(),
    );
    for entry in &roster.entries {
        hash = fnv(hash, &(entry.id.len() as u64).to_le_bytes());
        hash = fnv(hash, entry.id.as_bytes());
    }
    hash
}

/// The wire state of an ambient car's drive.
pub fn drive_state(drive: AmbientDrive) -> u8 {
    match drive {
        AmbientDrive::Lane => CAR_LANE,
        AmbientDrive::Knocked => CAR_KNOCKED,
    }
}

/// One staged row with the frame stamp and roster it arrived under.
type StagedCar = (u64, u64, u64, SnapCar);

/// The client-side traffic inbox, a field of [`RemoteSnaps`] so the
/// stream's two authority boundaries (an accepted `Start`, the link's
/// `Closed`) reset it with everything else.
///
/// Latest-wins *per car* on the frame's `(generation, tick)`: a
/// reordered older row never displaces a newer staged or applied one,
/// while an equal tick is idempotent state and passes.
#[derive(Default)]
pub struct TrafficStage {
    rows: HashMap<u32, StagedCar>,
    applied: HashMap<u32, (u64, u64)>,
    stale: u64,
    refused: u64,
    unresolved: u64,
    mismatched: u64,
    landed: u64,
}

impl TrafficStage {
    /// Queue one frame's rows, noting the host's `roster` digest they
    /// are relative to.
    pub fn push(&mut self, generation: u64, tick: u64, roster: u64, rows: Vec<SnapCar>) {
        for row in rows {
            let stamp = (generation, tick);
            let newest = self
                .rows
                .get(&row.id)
                .map(|(g, t, ..)| (*g, *t))
                .into_iter()
                .chain(self.applied.get(&row.id).copied())
                .max();
            if newest.is_some_and(|n| stamp < n) {
                self.stale += 1;
                continue;
            }
            if !self.rows.contains_key(&row.id) && self.rows.len() >= MAX_STAGED_CARS {
                self.refused += 1;
                continue;
            }
            self.rows.insert(row.id, (generation, tick, roster, row));
        }
    }

    /// Drop everything staged or remembered — the authority's stream
    /// ended. Counters are evidence, not stream state, and survive.
    pub fn reset(&mut self) {
        self.rows.clear();
        self.applied.clear();
    }

    /// Rows dropped as older than what was staged or applied.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Rows refused because the inbox was full.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Rows that named nothing this process could spawn, or belonged to
    /// another generation — drained, never applied.
    pub fn unresolved(&self) -> u64 {
        self.unresolved
    }

    /// Rows dropped unapplied because the host's roster differs from
    /// this process's own.
    pub fn mismatched(&self) -> u64 {
        self.mismatched
    }

    /// Rows that landed on a copy.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Rows staged and not yet drained.
    pub fn staged(&self) -> usize {
        self.rows.len()
    }

    /// Applied-row watermarks currently remembered.
    pub fn remembered(&self) -> usize {
        self.applied.len()
    }

    fn remember(&mut self, id: u32, stamp: (u64, u64)) {
        if self.applied.contains_key(&id) || self.applied.len() < MAX_APPLIED_CARS {
            self.applied.insert(id, stamp);
        }
    }

    /// Drain the staged rows in car-id order, each with the stamp to
    /// [`remember`](Self::remember) once it resolves. Rows of another
    /// generation are dropped counted: never replayed into this session.
    fn drain_for(&mut self, wire: u64) -> Vec<(SnapCar, (u64, u64), u64)> {
        let mut rows: Vec<(u32, StagedCar)> = self.rows.drain().collect();
        rows.sort_by_key(|(id, _)| *id);
        let mut current = Vec::with_capacity(rows.len());
        for (_, (generation, tick, roster, row)) in rows {
            if generation == wire {
                current.push((row, (generation, tick), roster));
            } else {
                self.unresolved += 1;
            }
        }
        current
    }
}

/// A client's local copy of one host car.
#[derive(Component)]
pub struct TrafficCopy {
    /// The host's car id.
    pub id: u32,
    /// The roster class the copy was spawned from.
    pub class: u16,
    /// The host's drive state as of the last row.
    pub state: u8,
    /// This process's session tick when a row last carried the copy.
    seen: u64,
}

/// A client's traffic world: the roster its rows index, the class assets
/// loaded for it and the live copies. Inserted by the session load on a
/// `Remote` city Cruise session; removed with the session.
#[derive(Resource)]
pub struct TrafficReplica {
    roster: AmbientRoster,
    digest: u64,
    classes: HashMap<usize, Option<Arc<AmbientClass>>>,
    /// The cable car's model, loaded on the first cable row: `None`
    /// until tried, `Some(None)` when the install cannot supply it.
    cable: Option<Option<MovableModel>>,
    copies: HashMap<u32, Entity>,
}

impl TrafficReplica {
    /// Copies currently alive.
    pub fn live(&self) -> usize {
        self.copies.len()
    }

    /// The digest of this process's own roster.
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// The ambient roster the wire's class indices resolve against.
    pub fn roster(&self) -> &AmbientRoster {
        &self.roster
    }
}

/// Build the client's traffic world for `config`: `None` unless the
/// session is a `Remote` city session that fields traffic and the
/// city authors a roster. The roster is the same `ambient_setup` merge
/// the host runs, so the digests agree on the same content.
pub fn load_traffic_replica(vfs: &Vfs, config: &SessionConfig) -> Option<TrafficReplica> {
    if config.authority != SessionAuthority::Remote || !fields_ambient_traffic(config) {
        return None;
    }
    let WorldMode::City { psdl } = &config.world else {
        return None;
    };
    let stem = std::path::Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());
    let setup = match mm2_content::ambient_setup(vfs, stem, None) {
        Ok(setup) => setup?,
        Err(e) => {
            warn!(error = %e, "ambient replica: city aimap failed — no traffic copies");
            return None;
        }
    };
    for d in &setup.diagnostics {
        warn!(diagnostic = %d, "ambient replica: roster diagnostic");
    }
    Some(TrafficReplica {
        digest: roster_digest(&setup.roster),
        roster: setup.roster,
        classes: HashMap::new(),
        cable: None,
        copies: HashMap::new(),
    })
}

/// The host's per-session memory of the ids it minted.
#[derive(Default)]
pub struct TrafficLedger {
    generation: u64,
    ids: HashMap<Entity, u32>,
    next: u32,
    frames: u32,
    /// Per remote player: the cars near its vehicle (F26-A.1).
    interest: BTreeMap<u16, InterestSet<u32>>,
}

impl TrafficLedger {
    /// The frame's rows for the cars of session `generation` — ids
    /// minted on first sight, unreadable poses skipped, rows in id order
    /// — and how many live cars the [`MAX_SNAP_CARS`] bound held back.
    /// Ids of despawned cars are forgotten (they are never reused).
    pub fn collect(
        &mut self,
        generation: u64,
        cars: impl Iterator<Item = (Entity, usize, u8, Vec3, Quat, Vec3)>,
    ) -> (Vec<SnapCar>, usize) {
        let mut live = self.collect_all(generation, cars);
        let omitted = live.len().saturating_sub(MAX_SNAP_CARS as usize);
        live.truncate(MAX_SNAP_CARS as usize);
        (live, omitted)
    }

    /// [`collect`](Self::collect) without the frame bound: every live
    /// car's row, in id order. The per-client relevancy pass applies the
    /// bound after it has narrowed the population to what is near.
    pub fn collect_all(
        &mut self,
        generation: u64,
        cars: impl Iterator<Item = (Entity, usize, u8, Vec3, Quat, Vec3)>,
    ) -> Vec<SnapCar> {
        if self.generation != generation {
            *self = Self {
                generation,
                ..default()
            };
        }
        let mut live: Vec<SnapCar> = Vec::new();
        let mut alive: Vec<Entity> = Vec::new();
        for (entity, class, state, pos, rot, vel) in cars {
            alive.push(entity);
            if !pos.is_finite() || !rot.is_finite() || !vel.is_finite() {
                continue;
            }
            let Ok(class) = u16::try_from(class) else {
                continue;
            };
            let id = match self.ids.get(&entity) {
                Some(id) => *id,
                None => {
                    let id = self.next;
                    self.next = self.next.wrapping_add(1);
                    self.ids.insert(entity, id);
                    id
                }
            };
            live.push(SnapCar {
                id,
                class,
                state,
                pos: pos.to_array(),
                rot: rot.to_array(),
                vel: vel.to_array(),
            });
        }
        self.ids.retain(|entity, _| alive.contains(entity));
        live.sort_by_key(|row| row.id);
        live
    }
}

/// The rows one client is owed this frame: the cars near `centre` (its
/// own vehicle), at most [`MAX_SNAP_CARS`], in id order. A client whose
/// vehicle the host has not spawned yet has no centre; it is sent the
/// lowest ids unfiltered rather than nothing, so a joiner is never
/// starved of the population while its seat loads. Rows are whole state
/// every frame, so a car entering relevance needs nothing extra.
/// Returns the rows and how many near cars the bound held back.
pub fn rows_for_client(
    interest: &mut InterestSet<u32>,
    centre: Option<Vec3>,
    all: &[SnapCar],
) -> (Vec<SnapCar>, usize) {
    let cap = MAX_SNAP_CARS as usize;
    let Some(centre) = centre else {
        interest.clear();
        let rows: Vec<SnapCar> = all.iter().take(cap).copied().collect();
        return (rows, all.len().saturating_sub(cap));
    };
    let by_id: HashMap<u32, &SnapCar> = all.iter().map(|row| (row.id, row)).collect();
    let near = interest.update(
        centre,
        all.iter().map(|row| (row.id, Vec3::from_array(row.pos))),
        usize::MAX,
    );
    let omitted = near.relevant.len().saturating_sub(cap);
    // `relevant` is nearest-first; the bound keeps the nearest.
    let mut keep: Vec<u32> = near.relevant.into_iter().take(cap).collect();
    keep.sort_unstable();
    let rows = keep
        .into_iter()
        .filter_map(|id| by_id.get(&id).map(|row| **row))
        .collect();
    (rows, omitted)
}

/// Host: broadcast the ambient population as [`Message::Traffic`].
///
/// Sends only while a traffic resource exists (a session that fields no
/// traffic sends nothing), gated like `worldprops::publish_props`.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // Bevy system — the queries are the contract.
pub fn publish_traffic(
    host: Res<HostLink>,
    session: Res<Session>,
    lobby: Res<LobbyState>,
    traffic: Option<Res<AmbientTraffic>>,
    remotes: Query<(&NetPlayer, &Position), With<RemotePick>>,
    mut ledger: Local<TrafficLedger>,
    cars: Query<(
        Entity,
        &AmbientCar,
        &Position,
        &Rotation,
        &LinearVelocity,
        &SessionEntity,
    )>,
    cable: Query<(
        Entity,
        &CableCar,
        &Position,
        &Rotation,
        &LinearVelocity,
        &SessionEntity,
    )>,
    report: Option<ResMut<NetDriveReport>>,
) {
    if ledger.generation != session.generation() {
        *ledger = TrafficLedger {
            generation: session.generation(),
            ..default()
        };
    }
    let Some(traffic) = traffic else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Ready
                | SessionPhase::Countdown
                | SessionPhase::Playing
                | SessionPhase::Results
        )
    {
        return;
    }
    ledger.frames = ledger.frames.wrapping_add(1);
    if !ledger.frames.is_multiple_of(PUBLISH_EVERY) {
        return;
    }

    let all = ledger.collect_all(
        session.generation(),
        // The cable cars first: they are few, so they take the lowest
        // ids and the frame's bound can never be what drops one.
        cable
            .iter()
            .filter(|(.., owner)| owner.0 == session.generation())
            .map(|(entity, car, pos, rot, vel, _)| {
                (entity, car.circuit, CAR_CABLE, pos.0, rot.0, vel.0)
            })
            .chain(
                cars.iter()
                    .filter(|(.., owner)| owner.0 == session.generation())
                    .map(|(entity, car, pos, rot, vel, _)| {
                        (
                            entity,
                            car.class,
                            drive_state(car.drive),
                            pos.0,
                            rot.0,
                            vel.0,
                        )
                    }),
            ),
    );
    if all.is_empty() {
        return;
    }
    let roster = roster_digest(traffic.roster());
    // Each remote player gets the cars near its own vehicle (F26-A.1).
    let players: BTreeSet<u16> = lobby.roster.iter().map(|e| e.player_id).collect();
    ledger.interest.retain(|id, _| players.contains(id));
    let mut report = report;
    for id in players {
        let centre = remotes
            .iter()
            .find(|(wire, _)| wire.0 == id)
            .map(|(_, pos)| pos.0);
        let (rows, omitted) = rows_for_client(ledger.interest.entry(id).or_default(), centre, &all);
        if rows.is_empty() {
            continue;
        }
        let sent = rows.len() as u64;
        let cable = rows.iter().filter(|r| r.state == CAR_CABLE).count() as u64;
        let frame = Message::Traffic {
            generation: session.wire_generation(),
            tick: session.tick(),
            roster,
            rows,
        };
        if host.ctl().send_to(id, &frame).is_ok()
            && let Some(report) = report.as_mut()
        {
            report.cars_sent += sent;
            report.cars_omitted += omitted as u64;
            report.cable_sent += cable;
        }
    }
}

/// The mutable pieces a replicated row writes.
type CopyBody = (
    &'static mut TrafficCopy,
    &'static mut Position,
    &'static mut Rotation,
    &'static mut LinearVelocity,
    &'static mut Transform,
    &'static SessionEntity,
);

/// Client: fold the staged traffic rows into the local world. Gated like
/// `worldprops::apply_props`: an authority never applies, a `Loading`
/// session holds the rows, and a session that is gone drops them.
#[allow(clippy::type_complexity, clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn apply_traffic(
    mut commands: Commands,
    mut snaps: ResMut<RemoteSnaps>,
    mut session: ResMut<Session>,
    replica: Option<ResMut<TrafficReplica>>,
    vfs: Option<Res<Mm2Vfs>>,
    mut bodies: Query<CopyBody>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    report: Option<ResMut<NetDriveReport>>,
) {
    let mut report = report;
    let mut publish = |snaps: &RemoteSnaps, live: usize, cable: usize| {
        if let Some(report) = report.as_mut() {
            report.cars_landed = snaps.traffic.landed();
            report.cars_mismatched = snaps.traffic.mismatched();
            report.cars_live = live;
            report.cable_live = cable;
        }
    };
    let live = replica.as_ref().map_or(0, |r| r.live());
    if session.authority_role().is_authority() {
        publish(&snaps, live, 0);
        return;
    }
    match session.phase() {
        SessionPhase::Loading => return,
        SessionPhase::Ready
        | SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Paused
        | SessionPhase::Results => {}
        _ => {
            snaps.traffic.reset();
            if let Some(mut replica) = replica {
                replica.copies.clear();
            }
            publish(&snaps, 0, 0);
            return;
        }
    }
    let wire = session.wire_generation();
    let rows = snaps.traffic.drain_for(wire);
    // A session that fields no traffic (a race, or a city with no
    // roster) has nothing for the rows to land on.
    let (Some(mut replica), Some(vfs)) = (replica, vfs) else {
        snaps.traffic.unresolved += rows.len() as u64;
        publish(&snaps, 0, 0);
        return;
    };
    let owner = SessionEntity(session.generation());
    let now = session.tick();
    let known: HashSet<u32> = replica.copies.keys().copied().collect();
    for (row, stamp, roster) in rows {
        if roster != replica.digest {
            snaps.traffic.mismatched += 1;
            continue;
        }
        let landed = apply_row(
            &row,
            now,
            &mut commands,
            &vfs.0,
            &mut replica,
            owner,
            &mut session,
            &mut bodies,
            &mut meshes,
            &mut images,
            &mut materials,
        );
        if landed {
            snaps.traffic.landed += 1;
            snaps.traffic.remember(row.id, stamp);
        } else {
            snaps.traffic.unresolved += 1;
        }
    }
    // Retire the copies no frame has carried lately — and any whose
    // entity is already gone (a teardown swept it). A copy spawned this
    // run is still queued in `commands`, so the query cannot see it yet.
    replica
        .copies
        .retain(|id, entity| match bodies.get(*entity) {
            Ok((copy, ..)) if now.saturating_sub(copy.seen) <= COPY_TTL_TICKS => true,
            Ok(_) => {
                commands.entity(*entity).despawn();
                false
            }
            Err(_) => !known.contains(id),
        });
    let live = replica.live();
    let cable = replica
        .copies
        .values()
        .filter(|e| bodies.get(**e).is_ok_and(|b| b.0.state == CAR_CABLE))
        .count();
    publish(&snaps, live, cable);
}

/// Apply one row; `false` when it names nothing this process can spawn
/// or says something unreadable.
#[allow(clippy::too_many_arguments)] // threads the same stores the session load does
fn apply_row(
    row: &SnapCar,
    now: u64,
    commands: &mut Commands,
    vfs: &Vfs,
    replica: &mut TrafficReplica,
    owner: SessionEntity,
    session: &mut Session,
    bodies: &mut Query<CopyBody>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> bool {
    let pos = Vec3::from_array(row.pos);
    let vel = Vec3::from_array(row.vel);
    if !pos.is_finite()
        || !vel.is_finite()
        || !matches!(row.state, CAR_LANE | CAR_KNOCKED | CAR_CABLE)
    {
        return false;
    }
    let rot = wire_quat(row.rot);
    if let Some(entity) = replica.copies.get(&row.id).copied()
        && let Ok((mut copy, mut position, mut rotation, mut velocity, mut transform, owned)) =
            bodies.get_mut(entity)
        && *owned == owner
    {
        copy.seen = now;
        copy.state = row.state;
        if position.0.distance(pos) > SNAP_DISTANCE {
            // A reset or a launch: show the jump, do not glide across it.
            transform.translation = pos;
            transform.rotation = rot;
        }
        position.0 = pos;
        rotation.0 = rot;
        velocity.0 = vel;
        return true;
    }
    // A new car (or one whose copy was swept): spawn it.
    if replica.copies.len() >= MAX_COPIES {
        return false;
    }
    if row.state == CAR_CABLE {
        return spawn_cable_copy(
            row,
            (pos, rot, vel),
            now,
            commands,
            vfs,
            replica,
            owner,
            session,
            (meshes, images, materials),
        );
    }
    let class = usize::from(row.class);
    let Some(spec) = replica.roster.entries.get(class) else {
        return false;
    };
    if spec.tuning.is_none() {
        return false;
    }
    let Some(assets) = replica
        .classes
        .entry(class)
        .or_insert_with(|| class_assets(vfs, spec).map(Arc::new))
        .clone()
    else {
        return false;
    };
    let entity = commands
        .spawn((
            owner,
            ObjectIdentity(session.mint_object_id()),
            session.authority_role(),
            TrafficCopy {
                id: row.id,
                class: row.class,
                state: row.state,
                seen: now,
            },
            RigidBody::Kinematic,
            assets.collider.clone(),
            Position(pos),
            Rotation(rot),
            LinearVelocity(vel),
            AngularVelocity::ZERO,
            Transform::from_translation(pos).with_rotation(rot),
            TransformInterpolation,
            Visibility::Visible,
        ))
        .id();
    if let Some(audio) = &assets.audio {
        commands.entity(entity).insert(AmbientAudio {
            spec: audio.clone(),
        });
    }
    let missing = spawn_vehicle_model(
        commands,
        vfs,
        &assets.model,
        0,
        meshes,
        images,
        materials,
        entity,
        None,
    );
    if !missing.is_empty() {
        warn!(class = %spec.id, "ambient replica: missing textures: {}", missing.join(", "));
    }
    replica.copies.insert(row.id, entity);
    true
}

/// Spawn a client's copy of one host cable car: the retail model and
/// its collider on a kinematic body, posed from the row. The copy is a
/// [`TrafficCopy`] like any other, so it rides the same retirement and
/// pose application; it carries no [`CableCar`] — the client never
/// drives one.
#[allow(clippy::too_many_arguments)] // threads the same stores the session load does
fn spawn_cable_copy(
    row: &SnapCar,
    (pos, rot, vel): (Vec3, Quat, Vec3),
    now: u64,
    commands: &mut Commands,
    vfs: &Vfs,
    replica: &mut TrafficReplica,
    owner: SessionEntity,
    session: &mut Session,
    (meshes, images, materials): (
        &mut Assets<Mesh>,
        &mut Assets<Image>,
        &mut Assets<StandardMaterial>,
    ),
) -> bool {
    if replica.cable.is_none() {
        let mut models = MovableModels::new(vfs, meshes, images, materials);
        let model = models.load(CABLE_CAR_MODEL, Vec3::ZERO);
        let missing = models.finish(commands, owner);
        if model.is_none() {
            warn!(
                model = CABLE_CAR_MODEL,
                "cable car replica: model unresolved — no copies"
            );
        } else if !missing.is_empty() {
            warn!(
                "cable car replica: missing textures: {}",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        replica.cable = Some(model);
    }
    let Some(Some(model)) = &replica.cable else {
        return false;
    };
    let entity = spawn_body(
        commands,
        model,
        Transform::from_translation(pos).with_rotation(rot),
        owner,
        format!("cablecar-copy-{}", row.id),
    );
    commands.entity(entity).insert((
        ObjectIdentity(session.mint_object_id()),
        session.authority_role(),
        TrafficCopy {
            id: row.id,
            class: row.class,
            state: row.state,
            seen: now,
        },
        Position(pos),
        Rotation(rot),
        LinearVelocity(vel),
        AngularVelocity::ZERO,
        TransformInterpolation,
    ));
    replica.copies.insert(row.id, entity);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car_at(id: u32, x: f32) -> SnapCar {
        SnapCar {
            pos: [x, 0.0, 0.0],
            ..row(id)
        }
    }

    #[test]
    fn a_far_client_is_sent_no_car_a_near_client_is_and_entry_delivers_the_current_pose() {
        let mut near = InterestSet::default();
        let mut far = InterestSet::default();
        let cars = [car_at(1, 0.0), car_at(2, 40.0)];
        let (near_rows, _) = rows_for_client(&mut near, Some(Vec3::ZERO), &cars);
        let far_centre = Vec3::new(5000.0, 0.0, 0.0);
        let (far_rows, _) = rows_for_client(&mut far, Some(far_centre), &cars);
        assert_eq!(near_rows.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert!(far_rows.is_empty(), "nothing near the far client");
        // The far client drives to the cars; the car has moved meanwhile.
        let moved = [car_at(1, 12.0), car_at(2, 40.0)];
        let (entered, _) = rows_for_client(&mut far, Some(Vec3::new(20.0, 0.0, 0.0)), &moved);
        assert_eq!(entered.len(), 2);
        assert_eq!(
            entered[0].pos,
            [12.0, 0.0, 0.0],
            "the pose is the current one"
        );
    }

    #[test]
    fn the_per_client_bound_keeps_the_nearest_cars_in_id_order() {
        let mut set = InterestSet::default();
        let cars: Vec<SnapCar> = (0..80).map(|i| car_at(i, i as f32)).collect();
        let (rows, omitted) = rows_for_client(&mut set, Some(Vec3::ZERO), &cars);
        assert_eq!(rows.len(), MAX_SNAP_CARS as usize);
        assert_eq!(omitted, 80 - MAX_SNAP_CARS as usize);
        assert!(rows.windows(2).all(|w| w[0].id < w[1].id));
        assert_eq!(rows.last().unwrap().id, MAX_SNAP_CARS as u32 - 1);
    }

    #[test]
    fn a_client_with_no_vehicle_yet_gets_the_population_unfiltered() {
        let mut set = InterestSet::default();
        let cars = [car_at(1, 9000.0)];
        assert_eq!(rows_for_client(&mut set, None, &cars).0.len(), 1);
    }

    fn row(id: u32) -> SnapCar {
        SnapCar {
            id,
            class: 0,
            state: CAR_LANE,
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
            vel: [0.0; 3],
        }
    }

    #[test]
    fn the_newest_row_per_car_wins_and_an_equal_tick_passes() {
        let mut stage = TrafficStage::default();
        stage.push(1, 10, 7, vec![row(3)]);
        stage.push(1, 9, 7, vec![row(3)]);
        assert_eq!((stage.staged(), stage.stale()), (1, 1));
        // Equal tick: idempotent state, accepted.
        stage.push(1, 10, 7, vec![row(3)]);
        assert_eq!(stage.stale(), 1);
        // A newer generation outranks any tick of an older one.
        stage.push(2, 1, 7, vec![row(3)]);
        let drained = stage.drain_for(2);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].1, (2, 1));
    }

    #[test]
    fn an_applied_watermark_stales_a_reordered_older_row() {
        let mut stage = TrafficStage::default();
        stage.remember(5, (1, 20));
        stage.push(1, 19, 7, vec![row(5)]);
        assert_eq!((stage.staged(), stage.stale()), (0, 1));
        stage.push(1, 21, 7, vec![row(5)]);
        assert_eq!(stage.staged(), 1);
    }

    #[test]
    fn rows_of_another_generation_drain_counted_never_applied() {
        let mut stage = TrafficStage::default();
        stage.push(1, 5, 7, vec![row(0), row(1)]);
        assert!(stage.drain_for(2).is_empty());
        assert_eq!(stage.unresolved(), 2);
        assert_eq!(stage.staged(), 0);
    }

    #[test]
    fn the_inbox_refuses_new_cars_past_its_bound_but_refreshes_known_ones() {
        let mut stage = TrafficStage::default();
        for chunk in (0..MAX_STAGED_CARS as u32 + 8)
            .collect::<Vec<_>>()
            .chunks(60)
        {
            stage.push(1, 1, 7, chunk.iter().map(|i| row(*i)).collect());
        }
        assert_eq!(stage.staged(), MAX_STAGED_CARS);
        assert_eq!(stage.refused(), 8);
        stage.push(1, 2, 7, vec![row(0)]);
        assert_eq!(stage.staged(), MAX_STAGED_CARS);
        assert_eq!(stage.refused(), 8);
    }

    #[test]
    fn the_applied_ledger_has_a_hard_cap() {
        let mut stage = TrafficStage::default();
        for id in 0..MAX_APPLIED_CARS as u32 + 100 {
            stage.remember(id, (1, 1));
        }
        assert_eq!(stage.remembered(), MAX_APPLIED_CARS);
        // A known car still advances at the cap.
        stage.remember(0, (1, 9));
        assert_eq!(stage.remembered(), MAX_APPLIED_CARS);
    }

    #[test]
    fn reset_clears_the_stream_but_not_the_evidence() {
        let mut stage = TrafficStage::default();
        stage.push(1, 5, 7, vec![row(0)]);
        stage.push(1, 4, 7, vec![row(0)]);
        stage.remember(0, (1, 5));
        stage.reset();
        assert_eq!((stage.staged(), stage.remembered()), (0, 0));
        assert_eq!(stage.stale(), 1);
    }

    #[test]
    fn the_roster_digest_tracks_order_and_identity() {
        use mm2_game::AmbientSpec;
        let spec = |id: &str| AmbientSpec {
            id: id.into(),
            cumulative_weight: 1.0,
            flag: 0,
            tuning: None,
        };
        let a = AmbientRoster::new(vec![spec("va_a"), spec("va_b")]);
        let same = AmbientRoster::new(vec![spec("va_a"), spec("va_b")]);
        let swapped = AmbientRoster::new(vec![spec("va_b"), spec("va_a")]);
        let shorter = AmbientRoster::new(vec![spec("va_a")]);
        let renamed = AmbientRoster::new(vec![spec("va_a"), spec("va_c")]);
        assert_eq!(roster_digest(&a), roster_digest(&same));
        for other in [&swapped, &shorter, &renamed] {
            assert_ne!(roster_digest(&a), roster_digest(other));
        }
        // The joined-string ambiguity a plain concatenation would have.
        let ab = AmbientRoster::new(vec![spec("ab"), spec("c")]);
        let abc = AmbientRoster::new(vec![spec("a"), spec("bc")]);
        assert_ne!(roster_digest(&ab), roster_digest(&abc));
    }
}
