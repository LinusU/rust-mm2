//! World-prop replication (F26-A): the host's knocked, broken and
//! settled props reach every client as authoritative state.
//!
//! The authority alone transitions a banger (`banger::activate_bangers`
//! and `settle_bangers` skip a `Predicted` session), so before this
//! module a client's copy of every prop stayed dormant while the host's
//! flew — the two worlds diverged at the first lamp post. Now the host
//! sends [`Message::Props`] frames and a client folds them into its own
//! stamped world:
//!
//! - **Identity.** A row names a placement by [`BangerSite`], the stamp
//!   ordinal every process that loads the same content agrees on
//!   (`ObjectId` slots interleave with vehicles and fragments in a
//!   process-local order, so they never cross the wire). A break
//!   fragment adds its index among the placement's collidable pieces.
//! - **State, not events.** Each row is the prop's phase and pose.
//!   Every active body rides every frame; a prop that just changed
//!   rides at once; a rolling window of eight settled/broken ones
//!   rides each frame, so a dropped frame, a reordered one and a late
//!   joiner all converge on the host's world within one cycle without
//!   the frame growing with the session.
//! - **Application.** The client never runs the transition itself.
//!   `Active` makes the body kinematic and drives its pose from the
//!   wire (the predicted car still collides with it), `Settled` makes
//!   it a static collider at the row's pose, `Broken` removes the
//!   unified collider and mesh, and a fragment row spawns the piece
//!   from the placement's own authored `BangerPieces`. Phases only move
//!   forward (`Dormant → Active → Settled`, or `→ Broken`), so a
//!   reordered `Active` row cannot un-settle a prop.
//!
//! Not replicated, by decision: vehicle breakaway parts (they have
//! their own wire path, `SnapEntry.breaks`), and a client's own contact
//! with a prop it has not yet heard about (the predicted car meets the
//! dormant collider; the host's truth lands a frame later). Poses are
//! applied as received — no interpolation yet, so an active prop moves
//! at the publish rate.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    Banger, BangerFragment, BangerPhase, BangerSite, Session, SessionEntity, SessionPhase,
};
use mm2_net::{MAX_SNAP_PROPS, Message, SNAP_NO_FRAGMENT, SnapProp};

use crate::banger::{BangerPieces, FragmentSpawn, shatter_placement, spawn_fragment};
use crate::net::HostLink;
use crate::netdrive::{RemoteSnaps, wire_quat};

/// [`SnapProp::phase`]: a struck, live body.
pub const PROP_ACTIVE: u8 = 1;
/// [`SnapProp::phase`]: at rest as a static collider.
pub const PROP_SETTLED: u8 = 2;
/// [`SnapProp::phase`]: a placement that shattered into fragments.
pub const PROP_BROKEN: u8 = 3;

/// Settled/broken rows re-sent per frame on top of the active and the
/// freshly changed — the resend cycle that heals a dropped frame and
/// brings a late joiner current.
pub const RESEND_WINDOW: usize = 8;

/// Staged rows a client holds before refusing new keys: a hostile or
/// runaway host cannot grow the inbox without bound while a session
/// loads (rows hold until the world exists).
pub const MAX_STAGED_PROPS: usize = 4096;

/// Publish every this-many frames: the active set moves continuously,
/// and half the frame rate is plenty for a prop that has no
/// interpolation to hide behind anyway.
const PUBLISH_EVERY: u32 = 2;

/// A prop's wire identity: placement ordinal plus fragment index.
type PropKey = (u32, u8);

fn key_of(row: &SnapProp) -> PropKey {
    (row.site, row.fragment)
}

fn encode_phase(phase: BangerPhase) -> Option<u8> {
    match phase {
        BangerPhase::Dormant => None,
        BangerPhase::Active => Some(PROP_ACTIVE),
        BangerPhase::Settled => Some(PROP_SETTLED),
        BangerPhase::Broken => Some(PROP_BROKEN),
    }
}

fn decode_phase(raw: u8) -> Option<BangerPhase> {
    match raw {
        PROP_ACTIVE => Some(BangerPhase::Active),
        PROP_SETTLED => Some(BangerPhase::Settled),
        PROP_BROKEN => Some(BangerPhase::Broken),
        _ => None,
    }
}

/// The client-side prop inbox, a field of [`RemoteSnaps`] so the
/// stream's two authority boundaries (an accepted `Start`, the link's
/// `Closed`) reset it with everything else.
///
/// Latest-wins *per prop* on the frame's `(generation, tick)`: a
/// reordered older row never displaces a newer staged or applied one,
/// while an equal tick is idempotent state and passes (the session
/// clock is frozen through `Ready`/`Countdown`, so successive resend
/// frames share a tick).
#[derive(Default)]
pub struct PropStage {
    rows: HashMap<PropKey, (u64, u64, SnapProp)>,
    applied: HashMap<PropKey, (u64, u64)>,
    stale: u64,
    refused: u64,
    unresolved: u64,
    landed: u64,
}

impl PropStage {
    /// Queue one frame's rows.
    pub fn push(&mut self, generation: u64, tick: u64, rows: Vec<SnapProp>) {
        for row in rows {
            let key = key_of(&row);
            let stamp = (generation, tick);
            let newest = self
                .rows
                .get(&key)
                .map(|(g, t, _)| (*g, *t))
                .into_iter()
                .chain(self.applied.get(&key).copied())
                .max();
            if newest.is_some_and(|n| stamp < n) {
                self.stale += 1;
                continue;
            }
            if !self.rows.contains_key(&key) && self.rows.len() >= MAX_STAGED_PROPS {
                self.refused += 1;
                continue;
            }
            self.rows.insert(key, (generation, tick, row));
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

    /// Rows that named nothing this process could resolve, carried an
    /// unreadable phase/pose or belonged to another generation —
    /// drained, never applied.
    pub fn unresolved(&self) -> u64 {
        self.unresolved
    }

    /// Rows that resolved against the local world.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Rows staged and not yet drained.
    pub fn staged(&self) -> usize {
        self.rows.len()
    }

    /// Drain the staged rows of the `wire` generation in a
    /// deterministic order — placements before their fragments — and
    /// remember each as applied. Rows stamped for another generation
    /// are dropped counted: never replayed into this session.
    fn drain_for(&mut self, wire: u64) -> Vec<SnapProp> {
        let mut rows: Vec<(PropKey, u64, u64, SnapProp)> = self
            .rows
            .drain()
            .map(|(key, (generation, tick, row))| (key, generation, tick, row))
            .collect();
        // A placement (`SNAP_NO_FRAGMENT`) sorts before its pieces.
        rows.sort_by_key(|(key, ..)| (key.0, key.1 != SNAP_NO_FRAGMENT, key.1));
        let mut current = Vec::with_capacity(rows.len());
        for (key, generation, tick, row) in rows {
            if generation == wire {
                self.applied.insert(key, (generation, tick));
                current.push(row);
            } else {
                self.unresolved += 1;
            }
        }
        current
    }
}

/// The host's per-session memory of which props have left `Dormant`.
#[derive(Default)]
pub struct PropLedger {
    generation: u64,
    live: BTreeMap<PropKey, Entity>,
    /// Changed since the last frame that carried them.
    fresh: BTreeSet<PropKey>,
    cursor: usize,
    frames: u32,
}

/// Host: broadcast the world's prop state as [`Message::Props`].
///
/// Transitions are absorbed into the ledger every run — before any
/// phase gate, so a change landing while the stream is quiet is
/// remembered — and only the send is gated, like
/// `netdrive::publish_snapshots`.
#[allow(clippy::type_complexity)] // Bevy system — the queries are the contract.
pub fn publish_props(
    host: Res<HostLink>,
    session: Res<Session>,
    mut ledger: Local<PropLedger>,
    placements: Query<(Entity, &BangerSite, &Banger), (Changed<Banger>, Without<BangerFragment>)>,
    fragments: Query<(Entity, &BangerFragment, &Banger), Changed<Banger>>,
    sites: Query<&BangerSite>,
    poses: Query<(&Banger, &Position, &Rotation)>,
) {
    if ledger.generation != session.generation() {
        *ledger = PropLedger {
            generation: session.generation(),
            ..default()
        };
    }
    for (entity, site, banger) in &placements {
        if banger.phase != BangerPhase::Dormant {
            let key = (site.0, SNAP_NO_FRAGMENT);
            ledger.live.insert(key, entity);
            ledger.fresh.insert(key);
        }
    }
    for (entity, fragment, banger) in &fragments {
        // A fragment is named by its placement; a parent that is gone
        // names nothing the wire can carry.
        if banger.phase != BangerPhase::Dormant
            && let Ok(site) = sites.get(fragment.parent)
        {
            let key = (site.0, fragment.index);
            ledger.live.insert(key, entity);
            ledger.fresh.insert(key);
        }
    }
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Ready | SessionPhase::Countdown | SessionPhase::Playing
        )
    {
        return;
    }
    ledger.frames = ledger.frames.wrapping_add(1);
    if !ledger.frames.is_multiple_of(PUBLISH_EVERY) {
        return;
    }

    // `Err` — the entity is gone; `Ok(None)` — nothing sendable.
    let row_for = |key: PropKey, entity: Entity| -> Result<Option<SnapProp>, ()> {
        let (banger, pos, rot) = poses.get(entity).map_err(|_| ())?;
        let Some(phase) = encode_phase(banger.phase) else {
            return Ok(None);
        };
        if !pos.0.is_finite() || !rot.0.is_finite() {
            return Ok(None);
        }
        Ok(Some(SnapProp {
            site: key.0,
            fragment: key.1,
            phase,
            pos: pos.0.to_array(),
            rot: rot.0.to_array(),
        }))
    };
    let cap = MAX_SNAP_PROPS as usize;
    let mut gone: Vec<PropKey> = Vec::new();
    let mut rows: Vec<SnapProp> = Vec::new();
    let mut included: BTreeSet<PropKey> = BTreeSet::new();
    // Active bodies and the freshly changed first: they are what moves.
    for (&key, &entity) in &ledger.live {
        if rows.len() >= cap {
            break;
        }
        let active = poses
            .get(entity)
            .is_ok_and(|(b, ..)| b.phase == BangerPhase::Active);
        if !(active || ledger.fresh.contains(&key)) {
            continue;
        }
        match row_for(key, entity) {
            Ok(Some(row)) => {
                included.insert(key);
                rows.push(row);
            }
            Ok(None) => {}
            Err(()) => gone.push(key),
        }
    }
    // Then the rolling resend window over everything not yet carried.
    let keys: Vec<(PropKey, Entity)> = ledger.live.iter().map(|(k, e)| (*k, *e)).collect();
    if !keys.is_empty() {
        let start = ledger.cursor % keys.len();
        let mut taken = 0;
        for step in 0..keys.len() {
            if taken >= RESEND_WINDOW || rows.len() >= cap {
                break;
            }
            let (key, entity) = keys[(start + step) % keys.len()];
            if included.contains(&key) {
                continue;
            }
            match row_for(key, entity) {
                Ok(Some(row)) => {
                    included.insert(key);
                    rows.push(row);
                }
                Ok(None) => {}
                Err(()) => gone.push(key),
            }
            taken += 1;
        }
        ledger.cursor = (start + RESEND_WINDOW) % keys.len();
    }
    for key in gone {
        ledger.live.remove(&key);
        ledger.fresh.remove(&key);
    }
    if rows.is_empty() {
        return;
    }
    let frame = Message::Props {
        generation: session.wire_generation(),
        tick: session.tick(),
        rows,
    };
    if host.ctl().broadcast(&frame).is_ok() {
        // Only a frame that left clears the fresh mark — a failed send
        // leaves the change queued for the next.
        ledger.fresh.retain(|key| !included.contains(key));
    }
}

/// The client index from placement ordinal / fragment key to entity.
/// Maintained from `Added<BangerSite>`; every lookup re-validates the
/// entity's session ownership, so entries from an earlier session
/// (whose ordinals the new stamp reuses and overwrites) can never be
/// mistaken for live ones.
#[derive(Default)]
pub struct PropIndex {
    sites: HashMap<u32, Entity>,
    fragments: HashMap<PropKey, Entity>,
}

/// The mutable pieces a replicated row writes.
type PropBody = (
    &'static mut Banger,
    &'static mut Position,
    &'static mut Rotation,
    &'static mut Transform,
    &'static SessionEntity,
);

/// Client: fold the staged prop rows into the local world. Gated like
/// `apply_snapshots`: an authority never applies, a `Loading` session
/// holds the rows (the world they name is still being stamped), and a
/// session that is gone drops them.
#[allow(clippy::type_complexity)] // Bevy system — the queries are the contract.
pub fn apply_props(
    mut commands: Commands,
    mut snaps: ResMut<RemoteSnaps>,
    mut session: ResMut<Session>,
    mut index: Local<PropIndex>,
    added: Query<(Entity, &BangerSite), Added<BangerSite>>,
    mut bodies: Query<PropBody>,
    pieces: Query<&BangerPieces>,
) {
    for (entity, site) in &added {
        index.sites.insert(site.0, entity);
    }
    if session.authority_role().is_authority() {
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
            snaps.props.reset();
            index.sites.clear();
            index.fragments.clear();
            return;
        }
    }
    let owner = SessionEntity(session.generation());
    let wire = session.wire_generation();
    for row in snaps.props.drain_for(wire) {
        let resolved = apply_row(
            &row,
            &mut commands,
            &mut index,
            owner,
            &mut session,
            &mut bodies,
            &pieces,
        );
        if resolved {
            snaps.props.landed += 1;
        } else {
            snaps.props.unresolved += 1;
        }
    }
}

/// Apply one row; `false` when it names nothing this process can
/// resolve or says something unreadable.
fn apply_row(
    row: &SnapProp,
    commands: &mut Commands,
    index: &mut PropIndex,
    owner: SessionEntity,
    session: &mut Session,
    bodies: &mut Query<PropBody>,
    pieces: &Query<&BangerPieces>,
) -> bool {
    let Some(phase) = decode_phase(row.phase) else {
        return false;
    };
    let pos = Vec3::from_array(row.pos);
    if !pos.is_finite() {
        return false;
    }
    let rot = wire_quat(row.rot);
    let tick = session.tick();
    let live = |bodies: &Query<PropBody>, entity: Entity| {
        bodies
            .get(entity)
            .is_ok_and(|(.., session_owner)| *session_owner == owner)
    };
    let Some(parent) = index
        .sites
        .get(&row.site)
        .copied()
        .filter(|e| live(bodies, *e))
    else {
        return false;
    };
    if row.fragment == SNAP_NO_FRAGMENT {
        return apply_body(parent, phase, pos, rot, tick, commands, bodies, true);
    }
    // A fragment is a body, never a shatter marker; and its row proves
    // the placement shattered even when the placement's own row was in
    // a frame that never arrived.
    if phase == BangerPhase::Broken {
        return false;
    }
    apply_body(
        parent,
        BangerPhase::Broken,
        pos,
        rot,
        tick,
        commands,
        bodies,
        true,
    );
    let key = key_of(row);
    if let Some(entity) = index
        .fragments
        .get(&key)
        .copied()
        .filter(|e| live(bodies, *e))
    {
        return apply_body(entity, phase, pos, rot, tick, commands, bodies, false);
    }
    let Ok(authored) = pieces.get(parent) else {
        return false;
    };
    let Some(piece) = authored.collidable().nth(row.fragment as usize) else {
        return false;
    };
    let Ok((parent_banger, ..)) = bodies.get(parent) else {
        return false;
    };
    let name = format!("{}-break{}", parent_banger.def.name, piece.index);
    let mut spawned = spawn_fragment(
        commands,
        piece,
        BangerFragment {
            parent,
            index: row.fragment,
        },
        FragmentSpawn {
            // A process-local id: nothing off this process names it.
            object: session.mint_object_id(),
            role: session.authority_role(),
            owner,
            transform: Transform::from_translation(pos).with_rotation(rot),
            phase,
            activated: (phase == BangerPhase::Active).then_some(tick),
            name,
        },
    );
    spawned.insert(if phase == BangerPhase::Active {
        RigidBody::Kinematic
    } else {
        RigidBody::Static
    });
    index.fragments.insert(key, spawned.id());
    true
}

/// Move one local body to the replicated `phase`/pose. Phases only move
/// forward: a row older than the body's state resolves (it named a real
/// body) without changing it.
#[allow(clippy::too_many_arguments)]
fn apply_body(
    entity: Entity,
    phase: BangerPhase,
    pos: Vec3,
    rot: Quat,
    tick: u64,
    commands: &mut Commands,
    bodies: &mut Query<PropBody>,
    placement: bool,
) -> bool {
    let Ok((mut banger, mut position, mut rotation, mut transform, _)) = bodies.get_mut(entity)
    else {
        return false;
    };
    let current = banger.phase;
    match (current, phase) {
        // Terminal states do not regress.
        (BangerPhase::Broken, _)
        | (BangerPhase::Settled, BangerPhase::Active | BangerPhase::Broken) => {
            return true;
        }
        (_, BangerPhase::Broken) => {
            if !placement {
                return false;
            }
            banger.phase = BangerPhase::Broken;
            banger.activated = None;
            shatter_placement(commands, entity);
            return true;
        }
        (_, BangerPhase::Dormant) => return false,
        (BangerPhase::Dormant, BangerPhase::Active) => {
            banger.phase = BangerPhase::Active;
            banger.activated = Some(tick);
            commands.entity(entity).insert(RigidBody::Kinematic);
        }
        (BangerPhase::Dormant | BangerPhase::Active, BangerPhase::Settled) => {
            banger.phase = BangerPhase::Settled;
            banger.activated = None;
            commands.entity(entity).insert(RigidBody::Static);
        }
        // Same phase: a pose refresh.
        (BangerPhase::Active, BangerPhase::Active)
        | (BangerPhase::Settled, BangerPhase::Settled) => {}
    }
    // A resend of an unchanged settled pose must not churn change
    // detection (and Avian's transform sync) every cycle.
    if position.0.distance_squared(pos) > 1e-8 || rotation.0.dot(rot).abs() < 1.0 - 1e-7 {
        position.0 = pos;
        rotation.0 = rot;
        transform.translation = pos;
        transform.rotation = rot;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(site: u32, fragment: u8, phase: u8) -> SnapProp {
        SnapProp {
            site,
            fragment,
            phase,
            pos: [0.0; 3],
            rot: [0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn the_newest_row_per_prop_wins_and_an_equal_tick_passes() {
        let mut stage = PropStage::default();
        stage.push(1, 10, vec![row(3, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        // Older: stale, counted, not staged over the newer.
        stage.push(1, 9, vec![row(3, SNAP_NO_FRAGMENT, PROP_SETTLED)]);
        assert_eq!(stage.stale(), 1);
        // Same tick: idempotent state (the clock is frozen through the
        // countdown), so it replaces without counting.
        stage.push(1, 10, vec![row(3, SNAP_NO_FRAGMENT, PROP_SETTLED)]);
        assert_eq!(stage.stale(), 1);
        let rows = stage.drain_for(1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, PROP_SETTLED);
        // An older row after the apply is still stale — the applied
        // watermark outlives the drain.
        stage.push(1, 8, vec![row(3, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        assert_eq!(stage.stale(), 2);
        assert_eq!(stage.staged(), 0);
    }

    #[test]
    fn props_are_independent_of_each_other() {
        // A newer frame for one prop never shadows an older frame's row
        // for another — the watermark is per prop, not per stream.
        let mut stage = PropStage::default();
        stage.push(1, 20, vec![row(1, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        stage.push(1, 12, vec![row(2, SNAP_NO_FRAGMENT, PROP_SETTLED)]);
        assert_eq!(stage.stale(), 0);
        assert_eq!(stage.staged(), 2);
    }

    #[test]
    fn a_placement_drains_before_its_fragments() {
        let mut stage = PropStage::default();
        stage.push(
            1,
            5,
            vec![
                row(4, 2, PROP_ACTIVE),
                row(4, SNAP_NO_FRAGMENT, PROP_BROKEN),
                row(2, SNAP_NO_FRAGMENT, PROP_SETTLED),
                row(4, 0, PROP_SETTLED),
            ],
        );
        let order: Vec<(u32, u8)> = stage.drain_for(1).iter().map(key_of).collect();
        assert_eq!(
            order,
            vec![(2, SNAP_NO_FRAGMENT), (4, SNAP_NO_FRAGMENT), (4, 0), (4, 2)]
        );
    }

    #[test]
    fn the_inbox_refuses_new_props_past_its_bound() {
        let mut stage = PropStage::default();
        let rows: Vec<SnapProp> = (0..MAX_STAGED_PROPS as u32 + 10)
            .map(|site| row(site, SNAP_NO_FRAGMENT, PROP_SETTLED))
            .collect();
        for chunk in rows.chunks(MAX_SNAP_PROPS as usize) {
            stage.push(1, 1, chunk.to_vec());
        }
        assert_eq!(stage.staged(), MAX_STAGED_PROPS);
        assert_eq!(stage.refused(), 10);
        // A prop already staged still updates in place at the bound.
        stage.push(1, 2, vec![row(0, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        assert_eq!(stage.refused(), 10);
    }

    #[test]
    fn a_foreign_generation_is_dropped_counted_at_the_drain() {
        let mut stage = PropStage::default();
        stage.push(7, 1, vec![row(1, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        stage.push(8, 1, vec![row(2, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        let rows = stage.drain_for(8);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].site, 2);
        assert_eq!(stage.unresolved(), 1);
    }

    #[test]
    fn a_reset_forgets_the_stream_but_keeps_the_evidence() {
        let mut stage = PropStage::default();
        stage.push(1, 10, vec![row(1, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        stage.push(1, 5, vec![row(1, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        stage.reset();
        assert_eq!(stage.staged(), 0);
        assert_eq!(stage.stale(), 1, "counters are evidence, not stream state");
        // A fresh authority restarts its ticks: nothing stale-drops
        // under the dead stream's watermark.
        stage.push(2, 1, vec![row(1, SNAP_NO_FRAGMENT, PROP_ACTIVE)]);
        assert_eq!(stage.stale(), 1);
        assert_eq!(stage.staged(), 1);
    }

    #[test]
    fn only_wire_phases_decode() {
        assert_eq!(decode_phase(PROP_ACTIVE), Some(BangerPhase::Active));
        assert_eq!(decode_phase(PROP_SETTLED), Some(BangerPhase::Settled));
        assert_eq!(decode_phase(PROP_BROKEN), Some(BangerPhase::Broken));
        assert_eq!(decode_phase(0), None, "dormant is never sent");
        assert_eq!(decode_phase(4), None);
        assert_eq!(encode_phase(BangerPhase::Dormant), None);
    }
}
