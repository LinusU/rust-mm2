//! Pedestrian reactions to approaching cars (F19-B.3): when a walker
//! senses a car bearing down on it, how it decides to stop, look and
//! dive clear, and how far each authored dive state carries it.
//!
//! What the retail data authors is the *vocabulary*: every stock
//! `pedmodel_*.csv` has the `ANTIC` (anticipation) loop with its
//! `WALK_ANTIC` / `ANTIC_WALK` transitions, `ANTIC_LDIVE` /
//! `ANTIC_RDIVE` and the mid-stride `WALK_LDIVE` / `WALK_RDIVE` dives
//! chaining through `*_GROUNDL/R` and `GROUND_STAND*` back to `STAND`,
//! with the lateral travel of each window in the `X AXIS` columns
//! (±2.2 m per leg, ≈4.4 m across the chain). *When* the original
//! switches between them, and how it senses a car, is **unrecovered**
//! (UNK-43): the thresholds in [`ReactPolicy`] are designed bounds, a
//! constant-velocity time-to-contact test against the car's footprint,
//! not original constants.
//!
//! The decision is a four-phase machine ([`Phase`]):
//! `Walking → Wary → Diving → Rejoining → Walking`.
//! A walker is `Wary` (stopped, facing the car in `ANTIC`) when a car
//! is heading for it inside [`ReactPolicy::alert_time`]; it dives away
//! from the car's line when contact is inside
//! [`ReactPolicy::dive_time`]; after the authored chain ends in `STAND`
//! it walks back to its sidewalk curve and carries on. Every phase has
//! a time bound ([`ReactPolicy::max_wary`], [`ReactPolicy::max_dive`])
//! so a missing state or a threat that never clears cannot wedge a
//! walker (F19-AC04 "terminate recovery").
//!
//! A walker is never *hit* by this model: there is no collider, so the
//! reaction is the whole response to a car (F19 req 4: "Do not turn the
//! game into pedestrian run-over simulation"). A car that gives a walker
//! less time than the dive needs simply drives through the figure.
//!
//! Everything here is pure: the app's system feeds it positions and the
//! animator's state and applies the [`Order`] it returns.

use bevy::prelude::Vec3;

use crate::ped::PedAnimState;

/// Designed sensing and timing bounds (UNK-43: none is an original
/// constant).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReactPolicy {
    /// A car slower than this (m/s) is not a threat: stopped, parked
    /// and crawling traffic does not scare anyone.
    pub min_speed: f32,
    /// Time to contact (s) inside which a car on a collision course
    /// makes a walker wary.
    pub alert_time: f32,
    /// Time to contact (s) inside which a walker dives.
    pub dive_time: f32,
    /// Clearance (m) beyond the car's half-width inside which the car's
    /// line counts as heading for the walker (alert).
    pub alert_margin: f32,
    /// Clearance (m) beyond the car's half-width inside which the car
    /// would actually strike the walker (dive).
    pub dive_margin: f32,
    /// Seconds without a threat before a wary walker walks on.
    pub clear_hold: f32,
    /// A wary walker walks on after this long whatever it sees (s).
    pub max_wary: f32,
    /// A diving walker whose chain has not reached `STAND` after this
    /// long (s) walks back anyway.
    pub max_dive: f32,
    /// Distance (m) from its curve at which a rejoining walker counts
    /// as back on the sidewalk.
    pub rejoin_radius: f32,
}

impl Default for ReactPolicy {
    fn default() -> Self {
        Self {
            min_speed: 2.0,
            alert_time: 3.0,
            dive_time: 1.2,
            alert_margin: 1.5,
            dive_margin: 0.5,
            clear_hold: 1.0,
            max_wary: 8.0,
            max_dive: 6.0,
            rejoin_radius: 0.25,
        }
    }
}

/// A moving car as a walker perceives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Approacher {
    /// Car centre, world space.
    pub position: Vec3,
    /// Velocity, world space (only the horizontal part is read).
    pub velocity: Vec3,
    /// Half the footprint's length along travel (m).
    pub half_length: f32,
    /// Half the footprint's width (m).
    pub half_width: f32,
}

/// How urgent a threat is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Alarm {
    /// A car is heading for the walker: stop and look.
    Alert,
    /// Contact is imminent: dive.
    Imminent,
}

/// One car's threat to one walker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threat {
    /// How urgent.
    pub alarm: Alarm,
    /// Seconds until the car's nose reaches the walker's position at
    /// constant velocity (0 once it is alongside).
    pub time_to_contact: f32,
    /// Horizontal unit vector from the walker toward the car.
    pub toward: Vec3,
    /// Horizontal unit vector across the car's line, on the walker's
    /// side — the way to dive.
    pub away: Vec3,
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// The threat `car` poses a walker standing at `ped`, if any.
///
/// The car is taken to keep its velocity: the walker is threatened
/// when it is ahead of the car's centre, within the car's width plus a
/// clearance of the line it is travelling on, and the time to reach it
/// is inside the policy. Non-finite inputs and cars below
/// [`ReactPolicy::min_speed`] are not threats.
pub fn assess(ped: Vec3, car: &Approacher, policy: &ReactPolicy) -> Option<Threat> {
    let v = flat(car.velocity);
    let speed = v.length();
    let inputs_finite = ped.is_finite()
        && car.position.is_finite()
        && v.is_finite()
        && car.half_length.is_finite()
        && car.half_width.is_finite();
    if !inputs_finite || speed < policy.min_speed {
        return None;
    }
    let dir = v / speed;
    let perp = Vec3::new(-dir.z, 0.0, dir.x);
    let rel = flat(ped - car.position);
    let along = rel.dot(dir);
    let side = rel.dot(perp);
    // The car's centre has already passed the walker.
    if along < 0.0 {
        return None;
    }
    let reach = (along - car.half_length).max(0.0);
    let time_to_contact = reach / speed;
    if time_to_contact > policy.alert_time {
        return None;
    }
    let lateral = side.abs();
    let alarm =
        if lateral <= car.half_width + policy.dive_margin && time_to_contact <= policy.dive_time {
            Alarm::Imminent
        } else if lateral <= car.half_width + policy.alert_margin {
            Alarm::Alert
        } else {
            return None;
        };
    let toward = if rel.length_squared() > 1e-6 {
        -rel.normalize()
    } else {
        dir
    };
    // A walker dead on the line has no side: the same fixed side every
    // time, so the choice is deterministic.
    let away = if side >= 0.0 { perp } else { -perp };
    Some(Threat {
        alarm,
        time_to_contact,
        toward,
        away,
    })
}

/// The most pressing threat among `cars`: an imminent one over an
/// alert, then the soonest contact.
pub fn most_urgent(
    ped: Vec3,
    cars: impl IntoIterator<Item = Approacher>,
    policy: &ReactPolicy,
) -> Option<Threat> {
    cars.into_iter()
        .filter_map(|c| assess(ped, &c, policy))
        .min_by(|a, b| {
            b.alarm
                .cmp(&a.alarm)
                .then(a.time_to_contact.total_cmp(&b.time_to_contact))
        })
}

/// Which way a walker dives, in its own frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiveSide {
    /// `*_LDIVE` states.
    Left,
    /// `*_RDIVE` states.
    Right,
}

impl DiveSide {
    /// The side whose direction best matches `away`, given the walker's
    /// right-hand direction (`rotation * +X` when it faces `-Z`). A
    /// tie dives right.
    pub fn toward(right: Vec3, away: Vec3) -> Self {
        if flat(right).dot(flat(away)) >= 0.0 {
            Self::Right
        } else {
            Self::Left
        }
    }

    /// The dive state entered from the `ANTIC` loop.
    pub fn from_antic(self) -> &'static str {
        match self {
            Self::Left => "ANTIC_LDIVE",
            Self::Right => "ANTIC_RDIVE",
        }
    }

    /// The dive state entered mid-stride from `WALK`.
    pub fn from_walk(self) -> &'static str {
        match self {
            Self::Left => "WALK_LDIVE",
            Self::Right => "WALK_RDIVE",
        }
    }
}

/// The states an archetype must author to react at all. An archetype
/// missing any of them walks on obliviously (and is reported by its
/// owner) rather than react half-way.
pub const REACTION_STATES: [&str; 6] = [
    "ANTIC",
    "ANTIC_LDIVE",
    "ANTIC_RDIVE",
    "WALK_LDIVE",
    "WALK_RDIVE",
    "STAND",
];

/// Lateral travel of the dive chain at `cursor` frames into `state`,
/// in the csv's sign (`+` = the walker's left; `ANTIC_LDIVE` authors
/// `+2.2`, `ANTIC_RDIVE` `-2.2`). The `X AXIS Offset`/`DISTANCE`
/// columns chain — `ANTIC_LDIVE` 0 → 2.2, `LDIVE_GROUNDL` 2.2 → 4.38 —
/// so the value is the state's offset plus its distance scaled by how
/// far through the window the cursor is (linear: the clip's own
/// root-channel drift is not sampled). `None` for a state that authors
/// no lateral travel (the ground-recovery rows), where the walker
/// holds where the chain left it. `clip_frames` clamps the window the
/// way `PedAnimator::tick` does.
pub fn dive_lateral(state: &PedAnimState, cursor: f32, clip_frames: u32) -> Option<f32> {
    if clip_frames == 0
        || !state.x_distance.is_finite()
        || !state.x_offset.is_finite()
        || state.x_distance == 0.0
    {
        return None;
    }
    let first = state.first_frame as f32;
    let end = state.last_frame.min(clip_frames - 1) as f32 + 1.0;
    let span = (end - first).max(1.0);
    let progress = ((cursor - first) / span).clamp(0.0, 1.0);
    Some(state.x_offset + state.x_distance * progress)
}

/// Where a walker is in the reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Following its sidewalk curve.
    Walking,
    /// Stopped, facing the car (`ANTIC`).
    Wary,
    /// Mid-dive, carried by the authored chain.
    Diving,
    /// Back on its feet, walking back to its curve.
    Rejoining,
}

/// What the owner must do with the animator after
/// [`Reaction::decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Nothing.
    None,
    /// Request `ANTIC`; stop following the curve.
    Alert,
    /// Request the dive state for this side, from `ANTIC` when the
    /// walker is in it (else the mid-stride one).
    Dive(DiveSide),
    /// Request `WALK`.
    Resume,
}

/// One walker's reaction state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reaction {
    /// Current phase.
    pub phase: Phase,
    wary_for: f32,
    clear_for: f32,
    dive_for: f32,
}

impl Default for Reaction {
    fn default() -> Self {
        Self::new()
    }
}

impl Reaction {
    /// A walker following its curve.
    pub const fn new() -> Self {
        Self {
            phase: Phase::Walking,
            wary_for: 0.0,
            clear_for: 0.0,
            dive_for: 0.0,
        }
    }

    /// Whether the walker is under the curve-following walk.
    pub fn is_walking(&self) -> bool {
        self.phase == Phase::Walking
    }

    /// Step the decision. `anim` is the animator's current state name,
    /// `threat` the most urgent threat this frame, `side` the dive
    /// side the walker's frame gives for it. Non-finite or negative
    /// `dt` advances nothing.
    pub fn decide(
        &mut self,
        anim: &str,
        threat: Option<&Threat>,
        side: DiveSide,
        dt: f32,
        policy: &ReactPolicy,
    ) -> Order {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        match self.phase {
            Phase::Walking => match threat.map(|t| t.alarm) {
                Some(Alarm::Imminent) => self.start_dive(side),
                Some(Alarm::Alert) => {
                    self.phase = Phase::Wary;
                    self.wary_for = 0.0;
                    self.clear_for = 0.0;
                    Order::Alert
                }
                None => Order::None,
            },
            Phase::Wary => {
                self.wary_for += dt;
                if threat.is_some_and(|t| t.alarm == Alarm::Imminent) {
                    return self.start_dive(side);
                }
                if threat.is_some() {
                    self.clear_for = 0.0;
                } else {
                    self.clear_for += dt;
                }
                let settled = matches!(anim, "ANTIC" | "ANTIC2");
                if settled
                    && (self.clear_for >= policy.clear_hold || self.wary_for >= policy.max_wary)
                {
                    self.phase = Phase::Walking;
                    return Order::Resume;
                }
                Order::None
            }
            Phase::Diving => {
                self.dive_for += dt;
                if anim == "STAND" || self.dive_for >= policy.max_dive {
                    self.phase = Phase::Rejoining;
                    return Order::Resume;
                }
                Order::None
            }
            Phase::Rejoining => Order::None,
        }
    }

    fn start_dive(&mut self, side: DiveSide) -> Order {
        self.phase = Phase::Diving;
        self.dive_for = 0.0;
        Order::Dive(side)
    }

    /// The walker is back on its curve.
    pub fn rejoined(&mut self) {
        *self = Self::new();
    }
}

/// Move `from` toward `to` by at most `step` metres. Returns the new
/// position and whether it is within `radius` of `to`. Non-finite
/// input holds still and reports not arrived.
pub fn rejoin_step(from: Vec3, to: Vec3, step: f32, radius: f32) -> (Vec3, bool) {
    if !from.is_finite() || !to.is_finite() || !step.is_finite() || step < 0.0 {
        return (from, false);
    }
    let gap = to - from;
    let dist = gap.length();
    if dist <= radius.max(0.0) || dist <= step {
        return (to, true);
    }
    (from + gap / dist * step, false)
}
