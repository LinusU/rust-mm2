//! NETPLAY-03E: pin the historical lifetimes of the things a captured
//! tick refers to.
//!
//! A [`crate::physics_world::WorldCapture`] names `Entity` ids of one
//! world and is replayed up to [`crate::replay::MAX_REPLAY_STEPS`] ticks
//! later (the [`crate::replay::HistoryRing`] holds at most
//! [`crate::replay::MAX_HISTORY`] captures). Over that window an id can
//! stop meaning what it meant, in three ways:
//!
//! - **Despawned and the slot reused.** Bevy entity ids carry a
//!   generation, so a stale id never resolves to the new occupant: the
//!   restore reports it as `missing` and writes nothing. This needs no
//!   extra state, and the tests pin it for traffic recycling.
//! - **Repurposed in place.** The entity survives but is a different
//!   actor: a banger that broke and left fragments, a fragment pool slot
//!   reused, a recycled traffic car teleported to a new lane, a collider
//!   whose body was swapped. Bevy cannot see this, so the code that
//!   repurposes an entity bumps its [`ActorIncarnation`]; a capture
//!   records it and a restore treats a mismatch as a different actor
//!   ([`LifetimeIssue::Retired`]).
//! - **The world around it changed epoch.** A new session generation
//!   ([`SessionEpoch`]) or an authority reset of a vehicle
//!   ([`ResetEpoch`]) makes older history unusable, and a trailer
//!   attach or detach changes which bodies the solver ties together
//!   ([`LifetimeIssue::HitchChanged`]).
//!
//! Instead of keeping despawned entities alive for the length of the
//! history ("pinning"), the lifetime is made *reversible by detection*: a
//! capture is a self-describing record, a restore refuses to write through
//! an id whose lifetime moved on, and [`crate::replay::replay_from`]
//! fails with [`crate::replay::ReplayError::StaleHistory`] before writing
//! anything, so the caller adopts the authority's capture instead
//! ([`crate::replay::recover_missing_history`]).
//!
//! Bounds: a counter that wraps needs 2^32 bumps of one entity inside the
//! 121-tick history to alias, which a 60 Hz step cannot produce. Only
//! [`SphericalJoint`] hitches are tracked (the trailer rig); other joint
//! types do not exist in the game. A body that is merely despawned is not
//! an issue: it is the existing `RestoreReport::missing`.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::physics_world::WorldCapture;

/// Which life of an entity this is. Whoever repurposes an entity in place
/// calls [`ActorIncarnation::bump`]. Absent means `0`.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActorIncarnation(pub u32);

impl ActorIncarnation {
    /// Start a new life.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// A vehicle's reset epoch: every authority teleport bumps it. A vehicle
/// capture with a lower `EpochRef::reset_epoch` predates a reset and is
/// stale; a higher one (an authority snapshot ahead of this peer) is not.
/// When absent the vehicle is not checked.
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResetEpoch(pub u32);

/// The current session generation. Compared with `EpochRef::generation`.
/// When the resource is absent nothing is checked.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionEpoch(pub u32);

/// A joint between two bodies at capture time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hitch {
    pub joint: Entity,
    pub body1: Entity,
    pub body2: Entity,
}

/// Why a captured tick no longer describes the live world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifetimeIssue {
    /// The capture is from another session generation.
    StaleSession { captured: u32, current: u32 },
    /// The entity is now a different actor than the one captured.
    Retired {
        entity: Entity,
        captured: u32,
        current: u32,
    },
    /// The vehicle was reset since the capture.
    StaleReset {
        entity: Entity,
        captured: u32,
        current: u32,
    },
    /// A joint was attached (`attached`) or detached since the capture.
    HitchChanged { joint: Entity, attached: bool },
}

pub(crate) fn incarnation(world: &World, entity: Entity) -> u32 {
    world.get::<ActorIncarnation>(entity).map_or(0, |i| i.0)
}

pub(crate) fn capture_hitches(world: &mut World) -> Vec<Hitch> {
    let mut q = world.query::<(Entity, &SphericalJoint)>();
    let mut out: Vec<Hitch> = q
        .iter(world)
        .map(|(joint, j)| Hitch {
            joint,
            body1: j.body1,
            body2: j.body2,
        })
        .collect();
    out.sort();
    out
}

/// Compare a capture against the live world. Read-only; empty means every
/// captured id still refers to the actor it did.
pub fn check_lifetimes(world: &mut World, capture: &WorldCapture) -> Vec<LifetimeIssue> {
    let mut issues = Vec::new();
    if let Some(current) = world.get_resource::<SessionEpoch>()
        && current.0 != capture.generation
    {
        issues.push(LifetimeIssue::StaleSession {
            captured: capture.generation,
            current: current.0,
        });
    }
    for (entity, captured) in &capture.incarnations {
        // A despawned entity is `missing`, not retired.
        if world.get_entity(*entity).is_err() {
            continue;
        }
        let current = incarnation(world, *entity);
        if current != *captured {
            issues.push(LifetimeIssue::Retired {
                entity: *entity,
                captured: *captured,
                current,
            });
        }
    }
    for (entity, v) in &capture.vehicles {
        if let Some(r) = world.get::<ResetEpoch>(*entity)
            && v.epoch.reset_epoch < r.0
        {
            issues.push(LifetimeIssue::StaleReset {
                entity: *entity,
                captured: v.epoch.reset_epoch,
                current: r.0,
            });
        }
    }
    // A wire snapshot carries no joint topology: nothing to compare.
    let Some(hitches) = &capture.hitches else {
        return issues;
    };
    let now = capture_hitches(world);
    for h in hitches {
        if !now.contains(h) {
            issues.push(LifetimeIssue::HitchChanged {
                joint: h.joint,
                attached: false,
            });
        }
    }
    for h in &now {
        if !hitches.contains(h) {
            issues.push(LifetimeIssue::HitchChanged {
                joint: h.joint,
                attached: true,
            });
        }
    }
    issues
}
