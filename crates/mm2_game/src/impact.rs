//! Bounded, meaningful impact reporting (F01-B contract).
//!
//! Sound, damage and network consumers must never see raw solver
//! contacts: a resting car holds a contact pair every step, a grazing
//! hit flaps between touching and not, and a busy scene can produce
//! hundreds of edges per tick. [`ImpactEvent`] is what they get instead —
//! one event per physical impact, with participants, point, surface,
//! severity and the session tick it happened on, produced under an
//! explicit [`ImpactPolicy`] bound.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::ids::ObjectId;
use crate::surface::SurfaceState;

/// Monotonic impact identity within a session — lets consumers
/// deduplicate deliveries (AC04-style, same pattern as result ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImpactId(pub u64);

/// One meaningful collision between two simulation objects.
#[derive(Message, Debug, Clone, Copy)]
pub struct ImpactEvent {
    /// Session-unique identity of this impact.
    pub id: ImpactId,
    /// Session generation the impact belongs to.
    pub generation: u64,
    /// Fixed-step session tick the impact was recorded on (post-solver).
    pub tick: u64,
    /// The two participants, in contact-graph order. Unmarked static
    /// geometry reports as [`ObjectId::WORLD`].
    pub participants: (ObjectId, ObjectId),
    /// World-space point of the deepest contact.
    pub point: Vec3,
    /// Contact normal pointing from `participants.0` toward
    /// `participants.1`.
    pub normal: Vec3,
    /// Approach speed along the contact normal, m/s — measured before
    /// the solver, so it is mass-independent and comparable across
    /// vehicle classes.
    pub severity: f32,
    /// Surface state of the passive participant at the contact (the
    /// non-vehicle side; [`SurfaceMaterial::Unspecified`] when nothing is
    /// authored).
    pub surface: SurfaceState,
}

/// Reporting policy for [`ImpactEvent`] production. The numbers are
/// designed values, not original-game rules — they exist so every
/// consumer sees the same filtered stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactPolicy {
    /// Approach speed below which a contact is a touch, not an impact
    /// (m/s). Filters out resting-contact edges and parking taps.
    pub min_severity: f32,
    /// Ticks the same collider pair may not re-emit after reporting —
    /// collapses the flap of a contact edge that starts/stops over
    /// consecutive solver steps into one impact. 24 ticks = 200 ms at
    /// the 120 Hz fixed step.
    pub pair_cooldown_ticks: u64,
    /// Most impacts one tick may emit; beyond it the lowest-severity
    /// candidates are dropped (and counted, so suppression is visible).
    pub max_per_tick: usize,
}

impl Default for ImpactPolicy {
    fn default() -> Self {
        Self {
            min_severity: 0.5,
            pair_cooldown_ticks: 24,
            max_per_tick: 16,
        }
    }
}

/// Per-pair emission window backing [`ImpactPolicy::pair_cooldown_ticks`].
/// Keeps the flap of one physical contact — or a car settling onto a
/// surface over several steps — from delivering a stream of events.
#[derive(Debug)]
pub struct ImpactDedup {
    cooldown_ticks: u64,
    last_emitted: HashMap<(Entity, Entity), u64>,
}

impl ImpactDedup {
    pub fn new(cooldown_ticks: u64) -> Self {
        Self {
            cooldown_ticks,
            last_emitted: HashMap::new(),
        }
    }

    /// Whether a contact edge between `a` and `b` may be reported at
    /// `tick`. The pair is unordered — (a,b) and (b,a) are the same
    /// physical contact.
    pub fn allow(&mut self, a: Entity, b: Entity, tick: u64) -> bool {
        let key = if a.index() <= b.index() {
            (a, b)
        } else {
            (b, a)
        };
        match self.last_emitted.get(&key) {
            Some(&t) if tick.saturating_sub(t) < self.cooldown_ticks => false,
            _ => {
                self.last_emitted.insert(key, tick);
                true
            }
        }
    }

    /// Forget all pairs — session teardown, so a recycled `Entity` in the
    /// next session cannot inherit a cooldown.
    pub fn clear(&mut self) {
        self.last_emitted.clear();
    }
}
