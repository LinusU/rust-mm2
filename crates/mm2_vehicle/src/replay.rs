//! NETPLAY-03C: a controlled fixed-step replay entry point.
//!
//! A correction that rewinds a car to an earlier tick must re-run the
//! ticks since, with the *same* work a live tick does, and without doing
//! anything a live tick only gets to do once. [`replay_from`] therefore
//! restores a [`WorldCapture`] ([`crate::physics_world`]) and then runs the
//! `FixedPostUpdate` schedule, where Avian lives, once per replayed tick,
//! with the generic `Time` set to one fixed step exactly as `FixedMain`
//! does. That runs Avian's own outer sets (transform propagation, mass
//! and collider preparation, transform write-back) around the
//! [`PhysicsSchedule`], which holds in order the retail vehicle step, the
//! world queries, the cached-force integration, collision and substeps,
//! the original static-contact processing and the post-step push-out. A
//! replayed tick is the live tick, not an approximation of one (a tyre
//! function, a repeated `Update`); running only the inner schedule leaves
//! state behind that the next live step then reads differently.
//!
//! What a replay is *not*: it never runs `Update`, `Last`, rendering,
//! `FixedPreUpdate`, `FixedUpdate`, `FixedLast` or any other schedule, so no second render frame, no
//! audio, no network send and no session/race/world system runs. Two
//! things still need explicit care, and are handled here:
//!
//! - **Clocks.** `Time<Physics>`, `Time<Substeps>` and the generic `Time`
//!   are saved and written back, so the live session clock is exactly
//!   where it was. (`Time<Virtual>`/`Time<Fixed>` are never touched.)
//! - **Messages written inside the physics schedule**, notably Avian's
//!   `CollisionStart`/`CollisionEnd` that impact, damage, pickup and
//!   traffic systems consume. While a replay runs, every message type in
//!   [`ReplayQuarantine`] is swapped for a scratch queue and the live one
//!   is put back untouched afterwards; the scratch is dropped and only
//!   counted. Live readers' cursors never see the replay.
//!
//! Systems that live in `FixedPostUpdate` or the `PhysicsSchedule` but
//! have an irreversible or presentation effect gate themselves with
//! [`not_replaying`] (the
//! [`ReplayContext`] flag is set for exactly the duration of a replay).
//!
//! Work is bounded: [`MAX_REPLAY_STEPS`] ticks per replay and a
//! [`HistoryRing`] of at most [`MAX_HISTORY`] captures.
//!
//! **Missing history is not a reset.** When the ring has no capture for
//! the requested tick, [`replay_from`] fails with
//! [`ReplayError::MissingHistory`] and writes nothing. The caller then
//! takes the distinct [`recover_missing_history`] path, which adopts the
//! authority's complete [`VehicleCapture`] (never a pose-only snap) and
//! says so in its [`Recovery`]. `VehicleState::new` is only ever built by
//! a genuine reset (`ResetVehicle` → `vehicle_reset`) or a spawn; nothing
//! in this module constructs it.

use std::any::Any;
use std::collections::VecDeque;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::capture::{RestoreError, VehicleCapture, restore_vehicle};
use crate::physics_world::{RestoreReport, WorldCapture, restore_world};
use crate::vehicle::VehicleInput;

/// Most ticks one replay may run: 2 s at the production 60 Hz step. A
/// correction needing more is stale; it recovers from history instead.
pub const MAX_REPLAY_STEPS: usize = 120;

/// Captures a [`HistoryRing`] keeps: a full replay window plus the tick
/// the replay starts from.
pub const MAX_HISTORY: usize = MAX_REPLAY_STEPS + 1;

/// Set for exactly the duration of a replay. Gate presentation or
/// irreversible work that sits inside the `PhysicsSchedule` with
/// [`not_replaying`].
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayContext {
    active: bool,
    /// 1-based index of the tick being replayed while active.
    step: usize,
    /// Replays completed, for evidence and tests.
    pub replays: u64,
    /// Ticks run by replays in total.
    pub replayed_steps: u64,
}

impl ReplayContext {
    /// Whether a replay is running right now.
    pub fn active(&self) -> bool {
        self.active
    }

    /// 1-based index of the tick being replayed, `0` outside a replay.
    pub fn step(&self) -> usize {
        if self.active { self.step } else { 0 }
    }
}

/// Run condition: `true` outside a replay.
pub fn not_replaying(context: Option<Res<ReplayContext>>) -> bool {
    context.is_none_or(|c| !c.active)
}

type Stash = Box<dyn Any + Send>;

struct Quarantine {
    name: &'static str,
    /// Swap an empty scratch queue in; the live queue is the stash.
    begin: fn(&mut World) -> Option<Stash>,
    /// Count and clear what the last tick wrote to the scratch queue.
    clear: fn(&mut World) -> usize,
    /// Put the live queue back; returns what the scratch still held.
    end: fn(&mut World, Stash) -> usize,
}

fn begin_quarantine<M: Message>(world: &mut World) -> Option<Stash> {
    let live = world.get_resource_mut::<Messages<M>>()?;
    let live = std::mem::take(live.into_inner());
    Some(Box::new(live))
}

fn clear_quarantine<M: Message>(world: &mut World) -> usize {
    let mut scratch = world.resource_mut::<Messages<M>>();
    let n = scratch.len();
    scratch.clear();
    n
}

fn end_quarantine<M: Message>(world: &mut World, stash: Stash) -> usize {
    let live = *stash
        .downcast::<Messages<M>>()
        .expect("quarantine stash holds the type it was begun with");
    let scratch = std::mem::replace(&mut *world.resource_mut::<Messages<M>>(), live);
    scratch.len()
}

/// The message types a replay must not let escape. Avian's collision
/// start/stop edges are registered by [`VehiclePlugin`](crate::VehiclePlugin);
/// an app registers every other message written from inside the
/// `PhysicsSchedule` that a gameplay, audio or network system reads.
#[derive(Resource, Default)]
pub struct ReplayQuarantine {
    entries: Vec<Quarantine>,
}

impl ReplayQuarantine {
    /// Quarantine message type `M` during replays. Idempotent.
    pub fn register<M: Message>(&mut self) {
        let name = std::any::type_name::<M>();
        if self.entries.iter().any(|e| e.name == name) {
            return;
        }
        self.entries.push(Quarantine {
            name,
            begin: begin_quarantine::<M>,
            clear: clear_quarantine::<M>,
            end: end_quarantine::<M>,
        });
    }

    /// Number of quarantined message types.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no message type is quarantined.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Extension to register a quarantined message type from a plugin.
pub trait ReplayAppExt {
    /// Quarantine `M` during replays; see [`ReplayQuarantine`].
    fn quarantine_message_in_replay<M: Message>(&mut self) -> &mut Self;
}

impl ReplayAppExt for App {
    fn quarantine_message_in_replay<M: Message>(&mut self) -> &mut Self {
        self.world_mut()
            .get_resource_or_insert_with(ReplayQuarantine::default)
            .register::<M>();
        self
    }
}

/// The commands for one replayed tick `S[n-1] -> S[n]`. A vehicle with no
/// entry keeps the command it has (the restore put back the captured
/// one); the caller supplies every car it wants driven.
#[derive(Clone, Debug, Default)]
pub struct ReplayStep {
    pub inputs: Vec<(Entity, VehicleInput)>,
}

/// Why a replay did not run. Nothing is written for any of these except
/// [`ReplayError::Restore`], which names what the restore refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReplayError {
    #[error("no steps to replay")]
    NoSteps,
    #[error("{requested} steps exceeds the replay bound of {max}")]
    TooManySteps { requested: usize, max: usize },
    /// The history ring holds no capture for the tick: use
    /// [`recover_missing_history`], never a reset.
    #[error("no history for tick {tick} (ring holds {oldest:?}..={newest:?})")]
    MissingHistory {
        tick: u64,
        oldest: Option<u64>,
        newest: Option<u64>,
    },
    #[error("a replay is already running")]
    Reentrant,
    #[error("the physics clock is paused")]
    PhysicsPaused,
    #[error("the physics step has no time (zero timestep)")]
    ZeroTimestep,
    #[error("the restore refused {} vehicle(s)", .0.len())]
    Restore(Vec<(Entity, RestoreError)>),
}

/// What a replay did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayReport {
    pub from_tick: u64,
    pub steps: usize,
    pub restore: RestoreReport,
    /// Quarantined messages the replayed ticks wrote and that were
    /// discarded, summed over every message type and every tick.
    pub suppressed_messages: usize,
}

/// A bounded ring of local-history [`WorldCapture`]s keyed by server
/// tick: the state `S[tick]` at the post-step boundary.
#[derive(Debug, Clone)]
pub struct HistoryRing {
    cap: usize,
    entries: VecDeque<(u64, WorldCapture)>,
    /// Captures dropped because the ring was full, or discarded as stale.
    pub evicted: u64,
}

impl Default for HistoryRing {
    fn default() -> Self {
        Self::new(MAX_HISTORY)
    }
}

impl HistoryRing {
    /// A ring of at most `cap` captures, clamped to `1..=MAX_HISTORY`.
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.clamp(1, MAX_HISTORY),
            entries: VecDeque::new(),
            evicted: 0,
        }
    }

    /// Record `S[tick]`. A tick not newer than the newest recorded one
    /// replaces everything from that tick on (a rewind rewrote the
    /// future); the oldest entry is evicted at the cap.
    pub fn record(&mut self, tick: u64, capture: WorldCapture) {
        while self.entries.back().is_some_and(|(t, _)| *t >= tick) {
            self.entries.pop_back();
            self.evicted += 1;
        }
        if self.entries.len() == self.cap {
            self.entries.pop_front();
            self.evicted += 1;
        }
        self.entries.push_back((tick, capture));
    }

    /// The capture of exactly `tick`.
    pub fn get(&self, tick: u64) -> Option<&WorldCapture> {
        self.entries
            .iter()
            .find_map(|(t, c)| (*t == tick).then_some(c))
    }

    /// Drop every capture (a new session generation or reset epoch makes
    /// older history unusable).
    pub fn clear(&mut self) {
        self.evicted += self.entries.len() as u64;
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn oldest(&self) -> Option<u64> {
        self.entries.front().map(|(t, _)| *t)
    }

    pub fn newest(&self) -> Option<u64> {
        self.entries.back().map(|(t, _)| *t)
    }
}

/// Restore `S[from_tick]` from `history` and replay `steps` ticks through
/// the physics schedule. Call between fixed steps. On `Ok` the world is at
/// `S[from_tick + steps.len()]`.
///
/// Every precondition is checked before anything is written, so an `Err`
/// other than [`ReplayError::Restore`] leaves the world untouched; in
/// particular [`ReplayError::MissingHistory`] never resets a car.
pub fn replay_from(
    world: &mut World,
    history: &HistoryRing,
    from_tick: u64,
    steps: &[ReplayStep],
) -> Result<ReplayReport, ReplayError> {
    if steps.is_empty() {
        return Err(ReplayError::NoSteps);
    }
    if steps.len() > MAX_REPLAY_STEPS {
        return Err(ReplayError::TooManySteps {
            requested: steps.len(),
            max: MAX_REPLAY_STEPS,
        });
    }
    if world
        .get_resource::<ReplayContext>()
        .is_some_and(|c| c.active)
    {
        return Err(ReplayError::Reentrant);
    }
    let Some(capture) = history.get(from_tick) else {
        return Err(ReplayError::MissingHistory {
            tick: from_tick,
            oldest: history.oldest(),
            newest: history.newest(),
        });
    };
    let physics = world.resource::<Time<Physics>>();
    if physics.is_paused() {
        return Err(ReplayError::PhysicsPaused);
    }
    let step_dt = world.resource::<Time<Fixed>>().timestep();
    if step_dt.is_zero() {
        return Err(ReplayError::ZeroTimestep);
    }

    let restore = restore_world(world, capture);
    if !restore.refused.is_empty() {
        return Err(ReplayError::Restore(restore.refused));
    }

    let saved_physics = *world.resource::<Time<Physics>>();
    let saved_substeps = *world.resource::<Time<Substeps>>();
    let saved_time = world.resource::<Time>().as_generic();

    world
        .get_resource_or_insert_with(ReplayContext::default)
        .active = true;
    let mut stashes = Vec::new();
    if let Some(q) = world.remove_resource::<ReplayQuarantine>() {
        for (i, e) in q.entries.iter().enumerate() {
            if let Some(stash) = (e.begin)(world) {
                stashes.push((i, stash));
            }
        }
        world.insert_resource(q);
    }

    let mut suppressed = 0;
    for (i, step) in steps.iter().enumerate() {
        world.resource_mut::<ReplayContext>().step = i + 1;
        for (entity, input) in &step.inputs {
            if let Some(mut slot) = world.get_mut::<VehicleInput>(*entity) {
                *slot = *input;
            }
        }
        set_fixed_step_time(world, step_dt);
        world.run_schedule(FixedPostUpdate);
        // The next tick sees only its own writes.
        suppressed += clear_scratch(world, &stashes);
    }

    // Put the live messages back, dropping the scratch.
    suppressed += finish_quarantine(world, stashes);
    *world.resource_mut::<Time<Physics>>() = saved_physics;
    *world.resource_mut::<Time<Substeps>>() = saved_substeps;
    *world.resource_mut::<Time>() = saved_time;
    let mut ctx = world.resource_mut::<ReplayContext>();
    ctx.active = false;
    ctx.step = 0;
    ctx.replays += 1;
    ctx.replayed_steps += steps.len() as u64;

    Ok(ReplayReport {
        from_tick,
        steps: steps.len(),
        restore,
        suppressed_messages: suppressed,
    })
}

/// The generic `Time` `FixedMain` hands `FixedPostUpdate`: the fixed
/// clock advanced by one step. Built on a copy so the live `Time<Fixed>`
/// keeps its elapsed time.
fn set_fixed_step_time(world: &mut World, step: Duration) {
    let mut fixed = *world.resource::<Time<Fixed>>();
    fixed.advance_by(step);
    *world.resource_mut::<Time>() = fixed.as_generic();
}

/// Count and clear what the tick just run wrote to each scratch queue, so
/// the scratch never grows past one tick's worth.
fn clear_scratch(world: &mut World, stashes: &[(usize, Stash)]) -> usize {
    let Some(q) = world.remove_resource::<ReplayQuarantine>() else {
        return 0;
    };
    let dropped = stashes
        .iter()
        .map(|(i, _)| (q.entries[*i].clear)(world))
        .sum();
    world.insert_resource(q);
    dropped
}

fn finish_quarantine(world: &mut World, stashes: Vec<(usize, Stash)>) -> usize {
    let mut dropped = 0;
    let Some(q) = world.remove_resource::<ReplayQuarantine>() else {
        return 0;
    };
    for (i, stash) in stashes {
        dropped += (q.entries[i].end)(world, stash);
    }
    world.insert_resource(q);
    dropped
}

/// How a car was brought back after its history was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// The authority's complete capture was adopted: body, tyres, engine,
    /// suspension and force history all come from it. No replay ran and
    /// no state was reset; the physics world around the car is untouched.
    AdoptedAuthoritative,
}

/// The missing-history path, distinct from a normal correction (which
/// replays from history) and from a reset (which is `ResetVehicle`).
/// Adopts `authority`'s complete state for `entity`; fails with the
/// restore's refusal, writing nothing, rather than falling back to a
/// fresh `VehicleState`.
pub fn recover_missing_history(
    world: &mut World,
    entity: Entity,
    authority: &VehicleCapture,
) -> Result<Recovery, RestoreError> {
    restore_vehicle(world, entity, authority)?;
    Ok(Recovery::AdoptedAuthoritative)
}
