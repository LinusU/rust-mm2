//! NETPLAY-03D: the authoritative state a host sends and a peer applies to
//! a *different* ECS world.
//!
//! [`AuthoritativeState`] is a plain serde DTO. It names nothing local: a
//! vehicle is a [`VehicleSlot`] (seat or trailer), a prop a [`PropSlot`],
//! and every vehicle carries the [`EpochRef`] (session generation and reset
//! epoch) it was captured under. No `Entity`, no proxy id, no Avian handle.
//! The encoding (bytes on the wire) is the transport's; this crate only
//! guarantees the DTO survives any lossless serde format, f32 included.
//!
//! One snapshot is **one tick**: the complete [`VehicleCapture`] of every
//! slotted vehicle plus the pose, velocities and sleep state of every
//! slotted dynamic prop. The local history ring
//! ([`crate::replay::HistoryRing`], up to 121 ticks of contact graph) never
//! goes on the wire, and neither does the contact graph at the tick: a
//! receiver's manifolds, warm-start impulses and recycling cache describe
//! its own timeline, so [`apply_authoritative`] cold-starts every contact
//! pair (see `docs/netcode-redesign.md`, "Wire round trip").
//!
//! The snapshot is validated on both sides: counts are bounded, slots are
//! unique, floats are finite. Anything else is a [`WireError`], never a
//! partial write.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::capture::{EpochRef, RestoreError, VehicleCapture};
use crate::physics_world::{BodyState, RestoreReport, WorldCapture, capture_bodies, restore_world};
use crate::vehicle::Vehicle;

/// Most vehicles one snapshot may carry (cars and trailers).
pub const MAX_VEHICLES: usize = 64;
/// Most props one snapshot may carry.
pub const MAX_PROPS: usize = 512;

/// Stable network identity of a vehicle: a seat or a trailer slot. Chosen
/// by the session, identical on every peer.
#[derive(
    Component, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct VehicleSlot(pub u16);

/// Stable network identity of a dynamic prop.
#[derive(
    Component, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PropSlot(pub u16);

/// A dynamic prop on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PropWire {
    pub position: [f32; 3],
    /// Quaternion `x, y, z, w`.
    pub rotation: [f32; 4],
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub sleeping: bool,
    pub sleep_timer: f32,
}

impl PropWire {
    fn from_state(s: &BodyState) -> Self {
        Self {
            position: s.position.to_array(),
            rotation: s.rotation.to_array(),
            linear_velocity: s.linear_velocity.to_array(),
            angular_velocity: s.angular_velocity.to_array(),
            sleeping: s.sleeping,
            sleep_timer: s.sleep_timer,
        }
    }

    fn to_state(self) -> BodyState {
        BodyState {
            position: Vec3::from(self.position),
            rotation: Quat::from_array(self.rotation),
            linear_velocity: Vec3::from(self.linear_velocity),
            angular_velocity: Vec3::from(self.angular_velocity),
            sleeping: self.sleeping,
            sleep_timer: self.sleep_timer,
        }
    }

    fn all_finite(&self) -> bool {
        self.position
            .iter()
            .chain(&self.rotation)
            .chain(&self.linear_velocity)
            .chain(&self.angular_velocity)
            .chain(&[self.sleep_timer])
            .all(|v| v.is_finite())
    }
}

/// The authoritative world state at the end of host tick `tick`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthoritativeState {
    /// Host simulation tick this is the post-step state of.
    pub tick: u64,
    /// Session generation; a peer in another generation rejects it.
    pub generation: u32,
    /// Sorted by slot.
    pub vehicles: Vec<(VehicleSlot, VehicleCapture)>,
    /// Sorted by slot.
    pub props: Vec<(PropSlot, PropWire)>,
}

/// Why a snapshot was not captured or not applied. Nothing was written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WireError {
    #[error("vehicle {0:?} has no `VehicleSlot`")]
    UnslottedVehicle(Entity),
    #[error("dynamic body {0:?} has no `PropSlot`")]
    UnslottedProp(Entity),
    #[error("{0} appears twice")]
    DuplicateSlot(String),
    #[error("{what}: {count} exceeds the bound of {max}")]
    TooMany {
        what: &'static str,
        count: usize,
        max: usize,
    },
    #[error("non-finite value in {0}")]
    NonFinite(String),
    #[error("snapshot is from generation {got}, this session is {expected}")]
    StaleGeneration { got: u32, expected: u32 },
    #[error("vehicle slot {slot:?} is at reset epoch {got}, the peer expects {expected}")]
    StaleResetEpoch {
        slot: VehicleSlot,
        got: u32,
        expected: u32,
    },
    #[error("vehicle {slot:?} generation {got} differs from the snapshot's {expected}")]
    MixedGeneration {
        slot: VehicleSlot,
        got: u32,
        expected: u32,
    },
}

impl AuthoritativeState {
    /// Bounds, uniqueness and finiteness. Run on decode; a hostile or
    /// corrupt packet must not reach the world.
    pub fn validate(&self) -> Result<(), WireError> {
        if self.vehicles.len() > MAX_VEHICLES {
            return Err(WireError::TooMany {
                what: "vehicles",
                count: self.vehicles.len(),
                max: MAX_VEHICLES,
            });
        }
        if self.props.len() > MAX_PROPS {
            return Err(WireError::TooMany {
                what: "props",
                count: self.props.len(),
                max: MAX_PROPS,
            });
        }
        let mut seen = HashSet::new();
        for (slot, v) in &self.vehicles {
            if !seen.insert(slot.0) {
                return Err(WireError::DuplicateSlot(format!("{slot:?}")));
            }
            if v.epoch.generation != self.generation {
                return Err(WireError::MixedGeneration {
                    slot: *slot,
                    got: v.epoch.generation,
                    expected: self.generation,
                });
            }
        }
        let mut seen = HashSet::new();
        for (slot, p) in &self.props {
            if !seen.insert(slot.0) {
                return Err(WireError::DuplicateSlot(format!("{slot:?}")));
            }
            if !p.all_finite() {
                return Err(WireError::NonFinite(format!("{slot:?}")));
            }
        }
        // Vehicle floats are checked by `restore_vehicle` (which writes
        // nothing on a refusal) and by `deviation`; check here too so a
        // bad snapshot is refused before any vehicle is written.
        for (slot, v) in &self.vehicles {
            if let Some(name) = v.first_non_finite() {
                return Err(WireError::NonFinite(format!("{slot:?} {name}")));
            }
        }
        Ok(())
    }
}

/// Capture the authoritative state at a tick boundary. Every vehicle must
/// carry a [`VehicleSlot`] and every dynamic non-vehicle body a
/// [`PropSlot`]: a body the peers cannot name would silently drift, so it
/// is an error rather than skipped.
pub fn capture_authoritative(
    world: &mut World,
    generation: u32,
    reset_epochs: &dyn Fn(VehicleSlot) -> u32,
    tick: u64,
) -> Result<AuthoritativeState, WireError> {
    let vehicle_slots: HashMap<Entity, VehicleSlot> = world
        .query::<(Entity, &VehicleSlot)>()
        .iter(world)
        .map(|(e, s)| (e, *s))
        .collect();
    let prop_slots: HashMap<Entity, PropSlot> = world
        .query::<(Entity, &PropSlot)>()
        .iter(world)
        .map(|(e, s)| (e, *s))
        .collect();
    // Each vehicle is stamped with its own reset epoch.
    let base = capture_bodies(world, EpochRef::default());
    let mut vehicles = Vec::with_capacity(base.vehicles.len());
    for (e, c) in &base.vehicles {
        let slot = *vehicle_slots
            .get(e)
            .ok_or(WireError::UnslottedVehicle(*e))?;
        let mut c = c.clone();
        c.epoch = EpochRef {
            generation,
            reset_epoch: reset_epochs(slot),
        };
        vehicles.push((slot, c));
    }
    let mut props = Vec::with_capacity(base.bodies.len());
    for (e, b) in &base.bodies {
        let slot = *prop_slots.get(e).ok_or(WireError::UnslottedProp(*e))?;
        props.push((slot, PropWire::from_state(b)));
    }
    vehicles.sort_by_key(|(s, _)| *s);
    props.sort_by_key(|(s, _)| *s);
    let state = AuthoritativeState {
        tick,
        generation,
        vehicles,
        props,
    };
    state.validate()?;
    Ok(state)
}

/// What [`apply_authoritative`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    /// Snapshot vehicle slots with no local vehicle.
    pub unknown_vehicles: Vec<VehicleSlot>,
    /// Snapshot prop slots with no local prop.
    pub unknown_props: Vec<PropSlot>,
    /// Local vehicles the snapshot did not mention; left as they are.
    pub unmentioned_vehicles: Vec<VehicleSlot>,
    /// Vehicles whose restore was refused (wheel count, model).
    pub refused: Vec<(VehicleSlot, RestoreError)>,
    /// The Avian side: contact pairs invalidated, sleeps deferred, bodies
    /// spawned locally since (`uncaptured`).
    pub world: RestoreReport,
}

/// Put an authoritative snapshot into `world`, whatever its entity layout.
///
/// `session_generation` is the peer's current session generation; a
/// snapshot from another one is refused. `expected_reset_epoch` is the peer's current reset epoch for a slot
/// (`None` = the peer does not know the slot yet): a snapshot older than a
/// reset the peer already applied is refused whole, so it cannot resurrect
/// the pre-reset car. All validation happens before the first write.
///
/// Solver caches: the capture carries no contact graph, so
/// [`restore_world`] cold-starts every contact pair (manifold impulses and
/// the recycling cache zeroed). Avian rebuilds pairs, islands and the
/// broad phase from the restored poses through its own change detection.
pub fn apply_authoritative(
    world: &mut World,
    state: &AuthoritativeState,
    session_generation: u32,
    expected_reset_epoch: &dyn Fn(VehicleSlot) -> Option<u32>,
) -> Result<ApplyReport, WireError> {
    state.validate()?;
    if state.generation != session_generation {
        return Err(WireError::StaleGeneration {
            got: state.generation,
            expected: session_generation,
        });
    }
    for (slot, v) in &state.vehicles {
        if let Some(expected) = expected_reset_epoch(*slot)
            && v.epoch.reset_epoch < expected
        {
            return Err(WireError::StaleResetEpoch {
                slot: *slot,
                got: v.epoch.reset_epoch,
                expected,
            });
        }
    }
    let by_slot: HashMap<VehicleSlot, Entity> = world
        .query_filtered::<(Entity, &VehicleSlot), With<Vehicle>>()
        .iter(world)
        .map(|(e, s)| (*s, e))
        .collect();
    let props: HashMap<PropSlot, Entity> = world
        .query::<(Entity, &PropSlot)>()
        .iter(world)
        .map(|(e, s)| (*s, e))
        .collect();

    let mut report = ApplyReport::default();
    // Validated equal to the peer's generation above; no incarnations or
    // joints are recorded, so none are compared.
    let mut capture = WorldCapture {
        generation: state.generation,
        ..Default::default()
    };
    let mut vehicle_of = HashMap::new();
    for (slot, v) in &state.vehicles {
        match by_slot.get(slot) {
            Some(e) => {
                vehicle_of.insert(*e, *slot);
                capture.vehicles.push((*e, v.clone()));
            }
            None => report.unknown_vehicles.push(*slot),
        }
    }
    for (slot, p) in &state.props {
        match props.get(slot) {
            Some(e) => capture.bodies.push((*e, p.to_state())),
            None => report.unknown_props.push(*slot),
        }
    }
    let mentioned: HashSet<VehicleSlot> = state.vehicles.iter().map(|(s, _)| *s).collect();
    report.unmentioned_vehicles = by_slot
        .keys()
        .filter(|s| !mentioned.contains(s))
        .copied()
        .collect();
    report.unmentioned_vehicles.sort();
    capture.vehicles.sort_by_key(|(e, _)| *e);
    capture.bodies.sort_by_key(|(e, _)| *e);

    report.world = restore_world(world, &capture);
    report.refused = report
        .world
        .refused
        .iter()
        .filter_map(|(e, err)| vehicle_of.get(e).map(|s| (*s, err.clone())))
        .collect();
    report.refused.sort_by_key(|(s, _)| *s);
    Ok(report)
}
