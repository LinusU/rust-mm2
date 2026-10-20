//! NETPLAY-03B: restore or invalidate the Avian physics world around the
//! vehicles that [`crate::capture`] already restores.
//!
//! [`capture_world`] runs between fixed steps and records, for the bodies
//! it is given or finds, everything a later step reads that is **not** in
//! a [`VehicleCapture`]: dynamic non-vehicle bodies (pose, velocities,
//! sleep state and timer) and the contact graph's touching pairs (manifold
//! points with their warm-start impulses, and the contact recycling cache).
//! [`restore_world`] puts it back and invalidates what it cannot trust.
//!
//! Everything here is *local history* (class H in
//! `docs/netcode-redesign.md`): it names `Entity` ids of one ECS world and
//! is never put on the wire. The authority sends vehicle state; each peer
//! rewinds its own world with its own history.
//!
//! What is deliberately **not** touched: `LastPhysicsTick` (a Bevy
//! change-detection tick, not a simulation tick, and what makes Avian see
//! the restored `Position`/`Rotation` writes as moved bodies), the
//! `Time<Physics>`/`Time<Substeps>` clocks (no step reads their elapsed
//! value; the replay layer owns the simulation tick), and the joint
//! graph, whose constraint is stateless in 0.8 (compliance and anchors
//! are the component's own fields, re-solved from the restored poses).

use std::collections::HashMap;

use avian3d::dynamics::solver::islands::{BodyIslandNode, PhysicsIslands, SleepBody, WakeBody};
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::capture::{EpochRef, RestoreError, VehicleCapture, capture_vehicle, restore_vehicle};
use crate::vehicle::Vehicle;

/// A dynamic body that is not a vehicle (a prop).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyState {
    pub position: Vec3,
    pub rotation: Quat,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    pub sleeping: bool,
    pub sleep_timer: f32,
}

/// The contact data of one collider pair that held manifolds at capture.
#[derive(Clone, Debug, PartialEq)]
pub struct PairState {
    manifolds: Vec<ContactManifold>,
    recycling_cache: Option<avian3d::collision::contact_types::ContactRecyclingCache>,
}

impl PairState {
    /// Whether any captured manifold point is in contact.
    pub fn touching(&self) -> bool {
        self.manifolds.iter().any(|m| !m.points.is_empty())
    }
}

/// Local-history capture of the physics world; see the module docs.
#[derive(Clone, Debug, Default)]
pub struct WorldCapture {
    pub vehicles: Vec<(Entity, VehicleCapture)>,
    pub bodies: Vec<(Entity, BodyState)>,
    pub pairs: Vec<((Entity, Entity), PairState)>,
}

/// What a [`restore_world`] did, for tests and the replay layer's log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestoreReport {
    /// Captured bodies that no longer exist (despawned since the capture).
    pub missing: Vec<Entity>,
    /// Dynamic bodies that exist now but were not captured (spawned since).
    /// Left as they are; their contacts are invalidated.
    pub uncaptured: Vec<Entity>,
    /// Contact pairs whose captured manifolds were put back.
    pub pairs_restored: usize,
    /// Contact pairs invalidated (cold start): not captured, or the
    /// manifold layout changed since.
    pub pairs_invalidated: usize,
    /// Bodies captured asleep that stay awake for one step because a stale
    /// contact still links them to an awake island; see [`PendingSleeps`].
    pub sleep_deferred: Vec<Entity>,
    /// Captured vehicles whose restore was refused.
    pub refused: Vec<(Entity, RestoreError)>,
}

/// Capture every vehicle, every non-vehicle dynamic body and the touching
/// contact pairs. Call between fixed steps.
pub fn capture_world(world: &mut World, epoch: EpochRef) -> WorldCapture {
    capture_world_with(world, epoch, true)
}

/// [`capture_world`] without the contact graph: bodies and vehicles only.
/// What a wire snapshot is built from (`crate::wire`); the receiver
/// cold-starts every contact pair.
pub(crate) fn capture_bodies(world: &mut World, epoch: EpochRef) -> WorldCapture {
    capture_world_with(world, epoch, false)
}

fn capture_world_with(world: &mut World, epoch: EpochRef, contacts: bool) -> WorldCapture {
    let mut out = WorldCapture::default();
    let mut q = world.query::<(
        Entity,
        &RigidBody,
        Has<Vehicle>,
        Option<&Position>,
        Option<&Rotation>,
        Option<&LinearVelocity>,
        Option<&AngularVelocity>,
        Has<Sleeping>,
        Option<&SleepTimer>,
    )>();
    let mut vehicles = Vec::new();
    for (e, body, is_vehicle, p, r, lv, av, sleeping, timer) in q.iter(world) {
        if is_vehicle {
            vehicles.push(e);
        } else if matches!(body, RigidBody::Dynamic) {
            out.bodies.push((
                e,
                BodyState {
                    position: p.map_or(Vec3::ZERO, |p| p.0),
                    rotation: r.map_or(Quat::IDENTITY, |r| r.0),
                    linear_velocity: lv.map_or(Vec3::ZERO, |v| v.0),
                    angular_velocity: av.map_or(Vec3::ZERO, |v| v.0),
                    sleeping,
                    sleep_timer: timer.map_or(0.0, |t| t.0),
                },
            ));
        }
    }
    for e in vehicles {
        if let Some(c) = capture_vehicle(world, e, epoch) {
            out.vehicles.push((e, c));
        }
    }
    // Query order is not stable across worlds; entity order is.
    out.vehicles.sort_by_key(|(e, _)| *e);
    out.bodies.sort_by_key(|(e, _)| *e);
    if !contacts {
        return out;
    }
    if let Some(graph) = world.get_resource::<ContactGraph>() {
        for pair in graph.iter_active().chain(graph.iter_sleeping()) {
            if pair.manifolds.is_empty() {
                continue;
            }
            out.pairs.push((
                (pair.collider1, pair.collider2),
                PairState {
                    manifolds: pair.manifolds.clone(),
                    recycling_cache: pair.recycling_cache.clone(),
                },
            ));
        }
    }
    out.pairs.sort_by_key(|(k, _)| *k);
    out
}

fn write_pose(ent: &mut EntityWorldMut, s: &BodyState) {
    if let Some(mut p) = ent.get_mut::<Position>() {
        p.set_if_neq(Position(s.position));
    }
    if let Some(mut r) = ent.get_mut::<Rotation>() {
        r.set_if_neq(Rotation(s.rotation));
    }
    if let Some(mut t) = ent.get_mut::<Transform>()
        && (t.translation != s.position || t.rotation != s.rotation)
    {
        t.translation = s.position;
        t.rotation = s.rotation;
    }
    if let Some(mut v) = ent.get_mut::<LinearVelocity>() {
        v.set_if_neq(LinearVelocity(s.linear_velocity));
    }
    if let Some(mut v) = ent.get_mut::<AngularVelocity>() {
        v.set_if_neq(AngularVelocity(s.angular_velocity));
    }
    if let Some(mut t) = ent.get_mut::<SleepTimer>() {
        t.set_if_neq(SleepTimer(s.sleep_timer));
    }
}

/// Whether `body` is the only body of its island, so sleeping the island
/// cannot put anything else to sleep.
fn island_is_alone(world: &World, body: Entity) -> bool {
    let Some(node) = world.get::<BodyIslandNode>(body) else {
        return false;
    };
    world
        .resource::<PhysicsIslands>()
        .get(node.island_id())
        .is_some_and(|island| island.body_count() == 1)
}

/// Bodies [`restore_world`] could not put to sleep yet, with the state to
/// put them to sleep in.
#[derive(Resource, Default)]
pub struct PendingSleeps(Vec<(Entity, BodyState)>);

/// `PhysicsSchedule`, `PhysicsStepSystems::Last` (before Avian advances
/// `LastPhysicsTick`): the step after a restore has removed the stale
/// contacts, so a body that no longer touches a dynamic body goes back to
/// sleep in its captured state, bit-exact (`SleepBody` splits the island
/// the stale contact had merged). A body that touches a live dynamic body
/// stays awake: it was woken by a real contact.
pub(crate) fn finish_sleep_restore(
    mut commands: Commands,
    mut pending: ResMut<PendingSleeps>,
    graph: Res<ContactGraph>,
    bodies: Query<(&RigidBody, Has<Sleeping>)>,
    mut poses: Query<(
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
        &mut SleepTimer,
    )>,
) {
    for (e, s) in std::mem::take(&mut pending.0) {
        let Ok((_, asleep)) = bodies.get(e) else {
            continue;
        };
        let touching_dynamic = graph.iter_active_touching().any(|p| {
            let other = if p.body1 == Some(e) {
                p.body2
            } else if p.body2 == Some(e) {
                p.body1
            } else {
                return false;
            };
            other.is_some_and(|o| bodies.get(o).is_ok_and(|(b, _)| !b.is_static()))
        });
        if asleep || touching_dynamic {
            continue;
        }
        if let Ok((mut p, mut r, mut lv, mut av, mut t)) = poses.get_mut(e) {
            p.0 = s.position;
            r.0 = s.rotation;
            lv.0 = s.linear_velocity;
            av.0 = s.angular_velocity;
            t.0 = s.sleep_timer;
        }
        commands.queue(SleepBody(e));
    }
}

/// Zero everything a warm start would carry into the next solve.
fn cold_start(pair: &mut ContactPair) {
    pair.recycling_cache = None;
    for m in &mut pair.manifolds {
        for p in &mut m.points {
            p.normal_impulse = 0.0;
            p.warm_start_normal_impulse = 0.0;
            p.warm_start_tangent_impulse = Default::default();
        }
    }
}

/// Put a [`WorldCapture`] back. Vehicles are validated by
/// [`restore_vehicle`] (a refused one is reported, not half-written).
/// Contacts: a pair that was captured and still has the same manifold
/// layout gets its captured manifolds (impulses, features, recycling
/// cache) back, so the next solve warm-starts exactly as the original
/// run did; every other pair is cold-started, because carrying impulses
/// from the abandoned future into the replay would be wrong.
pub fn restore_world(world: &mut World, capture: &WorldCapture) -> RestoreReport {
    let mut report = RestoreReport::default();

    for (e, c) in &capture.vehicles {
        match restore_vehicle(world, *e, c) {
            Ok(()) => {}
            Err(RestoreError::NotAVehicle) if world.get_entity(*e).is_err() => {
                report.missing.push(*e);
            }
            Err(err) => report.refused.push((*e, err)),
        }
    }

    let mut wake = Vec::new();
    let mut sleep = Vec::new();
    for (e, s) in &capture.bodies {
        let Ok(mut ent) = world.get_entity_mut(*e) else {
            report.missing.push(*e);
            continue;
        };
        // Write only what differs: Avian wakes a sleeping body on any
        // `Position`/`Rotation`/velocity/`SleepTimer` change detection,
        // even to the same value, and a body captured asleep must stay
        // asleep.
        write_pose(&mut ent, s);
        let now_sleeping = ent.contains::<Sleeping>();
        if s.sleeping && !now_sleeping {
            sleep.push((*e, s));
        } else if !s.sleeping && now_sleeping {
            wake.push(*e);
        }
    }
    // Wake before sleeping so a shared island ends up as captured.
    for e in wake {
        let _ = WakeBody(e).apply(world);
    }
    for (e, s) in sleep {
        if island_is_alone(world, e) {
            let _ = SleepBody(e).apply(world);
            write_pose(&mut world.entity_mut(e), s);
        } else {
            // A stale contact still links the body to a live island;
            // sleeping it now would sleep its neighbours. The next step
            // removes the stale contact, and `finish_sleep_restore` puts
            // it to sleep at the end of that step.
            world.resource_mut::<PendingSleeps>().0.push((e, s.clone()));
            report.sleep_deferred.push(e);
        }
    }
    let known: std::collections::HashSet<Entity> = capture
        .vehicles
        .iter()
        .map(|(e, _)| *e)
        .chain(capture.bodies.iter().map(|(e, _)| *e))
        .collect();
    let mut q = world.query_filtered::<Entity, With<RigidBody>>();
    for e in q.iter(world).collect::<Vec<_>>() {
        if !known.contains(&e) && matches!(world.get::<RigidBody>(e), Some(RigidBody::Dynamic)) {
            report.uncaptured.push(e);
        }
    }

    let by_key: HashMap<(Entity, Entity), &PairState> =
        capture.pairs.iter().map(|(k, v)| (*k, v)).collect();
    if let Some(mut graph) = world.get_resource_mut::<ContactGraph>() {
        let mut visit = |pair: &mut ContactPair| match by_key.get(&(pair.collider1, pair.collider2))
        {
            Some(state)
                if state.manifolds.len() == pair.manifolds.len()
                    && state
                        .manifolds
                        .iter()
                        .zip(&pair.manifolds)
                        .all(|(a, b)| a.points.len() == b.points.len()) =>
            {
                pair.manifolds.clone_from(&state.manifolds);
                pair.recycling_cache.clone_from(&state.recycling_cache);
                report.pairs_restored += 1;
            }
            _ => {
                cold_start(pair);
                report.pairs_invalidated += 1;
            }
        };
        graph.iter_active_mut().for_each(&mut visit);
        graph.iter_sleeping_mut().for_each(&mut visit);
    }
    report
}
