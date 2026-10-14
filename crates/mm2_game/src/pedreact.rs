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
//! switches between them, and how it senses a car, is recovered from
//! `Midtown2.exe` (UNK-43): a walker senses only player-driven cars
//! within 35 m moving faster than 1 m/s whose own driven axis meets one
//! of the two corridors — the near one (`0x54e660`: from 0.25 of the
//! car's length out to 20 m ahead, within half-width + 2 m) can stop or
//! dive, the far one (`0x54e8a0`: out to 35 m, within half-width + 4 m)
//! can only stop. The value compared to the timing bounds is
//! `(L − 2)/speed` seconds. The dive side follows the sensed car's own
//! steering when it is past full lock (`> 0.85` right, `< −0.85` left),
//! else the walker's side of the car's line. The timing bounds stay
//! tunable in [`ReactPolicy`];
//! the corridor geometry is the recovered constants [`NEAR_RANGE`],
//! [`FAR_RANGE`], [`NEAR_MARGIN`], [`FAR_MARGIN`], [`NOSE_FRACTION`] and
//! [`TIME_OFFSET`] below.
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
//! This model has no notion of a body: a car that gives a walker less
//! time than the dive needs does not change the reaction. The app does
//! give each walker a kinematic body (F19-B.8b, `mm2_app::pedestrian`,
//! `GameLayer::Pedestrian`), so a car that catches one is blocked at the
//! body instead of driving through the figure — but the walker is never
//! moved or "hit" by it, and the reaction above stays the whole
//! walker-side response (F19 req 4: "Do not turn the game into
//! pedestrian run-over simulation").
//!
//! Everything here is pure: the app's system feeds it positions and the
//! animator's state and applies the [`Order`] it returns.

use bevy::prelude::Vec3;

use crate::ped::PedAnimState;

/// The near corridor's along-axis end (m): a walker farther ahead than
/// this is not in the stop-or-dive corridor. Retail `0x5d716c`.
pub const NEAR_RANGE: f32 = 20.0;
/// The far corridor's along-axis end (m): it reaches as far as the
/// radial sense range. Retail `0x5d7170` (= 35, the same value as the
/// 1225 m² distance gate).
pub const FAR_RANGE: f32 = 35.0;
/// Lateral clearance (m) beyond the car's half-width for the near
/// corridor (`0x54e850`: `0.5·width + 2`).
pub const NEAR_MARGIN: f32 = 2.0;
/// Lateral clearance (m) beyond the car's half-width for the far
/// corridor (`0x54e850`-pair `+ 4`).
pub const FAR_MARGIN: f32 = 4.0;
/// The along-axis offset (m) the time value is measured short by
/// (`0x5af578` = 2): value = (L − 2)/speed.
pub const TIME_OFFSET: f32 = 2.0;
/// The near corridor starts at this fraction of the car's **full**
/// length ahead of its centre (`0x5af584` = 0.25), i.e. half a
/// half-length.
pub const NOSE_FRACTION: f32 = 0.25;
/// Steering input (`carsim+0x1554`, `+1` right) past which (right), or
/// below whose negative (left), a walker dives *with* the sensed car's
/// steer instead of to its own side of the car's line (retail `0x5b1e34`
/// = 0.85 compared strictly, the negative mirror at `0x5b4880`).
pub const DIVE_STEER: f32 = 0.85;

/// Sensing and timing bounds. `sense_range`, `min_speed`, `alert_time`
/// and `dive_time` are the retail constants recovered from
/// `Midtown2.exe` (UNK-43, `docs/research/pedanim.md` "Car sensing");
/// the wary/dive/rejoin timers are designed bounds. The corridor
/// geometry is the recovered constants above, not policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReactPolicy {
    /// Horizontal distance (m) from the car beyond which a walker does
    /// not notice it (retail: squared distance < 1225).
    pub sense_range: f32,
    /// A car slower than this (m/s) is not a threat: stopped, parked
    /// and crawling traffic does not scare anyone.
    pub min_speed: f32,
    /// Time value (s) inside which a car in a corridor makes a walker
    /// wary (retail: 2.3).
    pub alert_time: f32,
    /// Time value (s) inside which the near corridor makes a walker
    /// dive (retail: 0.75; the far corridor never dives).
    pub dive_time: f32,
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
            sense_range: 35.0,
            min_speed: 1.0,
            alert_time: 2.3,
            dive_time: 0.75,
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
    /// The driver's steering input, `−1..1` (`+` right) — the car's own
    /// `carsim+0x1554`. A hard-steering car picks the walker's dive
    /// side; `0` when the driver's input is unknown.
    pub steering: f32,
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
    /// The retail time value: seconds `(L − 2)/speed` to the walker
    /// along the car's axis (0 once alongside). Read from the near
    /// corridor (`0x54e630`), shared by both corridors.
    pub time_to_contact: f32,
    /// Horizontal unit vector from the walker toward the car.
    pub toward: Vec3,
    /// Horizontal unit vector across the car's line, on the walker's
    /// side — the way to dive.
    pub away: Vec3,
    /// The sensed car's steering input, `−1..1` (`+` right), carried out
    /// of [`assess`] for [`Threat::dive_side`].
    pub steering: f32,
}

impl Threat {
    /// Which of its own sides the walker dives for this threat.
    ///
    /// The original reads the *sensed car's* steering (`carsim+0x1554`,
    /// `+1` right): past full lock — `> 0.85` right or `< −0.85` left
    /// (`0x54dc3f`, both strict, so exactly 0.85 falls through) — the
    /// walker dives with the steering; otherwise it dives to its own
    /// side of the car's line (`0x54dccf`, the sign of the stored side
    /// value, which the port already renders as `away`). A walker dead
    /// on the line is measure-zero and keeps the fixed right tie of
    /// [`DiveSide::toward`].
    pub fn dive_side(&self, right: Vec3) -> DiveSide {
        if self.steering > DIVE_STEER {
            DiveSide::Right
        } else if self.steering < -DIVE_STEER {
            DiveSide::Left
        } else {
            DiveSide::toward(right, self.away)
        }
    }
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// The threat `car` poses a walker standing at `ped`, if any.
///
/// The car is taken to keep its velocity. The walker must be ahead of
/// the car's nose (past `NOSE_FRACTION` of the car's length) and inside
/// one of the original's two corridors along the car's axis: the near
/// corridor (to [`NEAR_RANGE`], within half-width + [`NEAR_MARGIN`])
/// stops the walker, or dives it when the time value is inside
/// [`ReactPolicy::dive_time`]; the far corridor (to [`FAR_RANGE`],
/// within half-width + [`FAR_MARGIN`]) only stops it. The time value is
/// `(L − `[`TIME_OFFSET`]`)/speed` seconds, and the walker must be
/// inside [`ReactPolicy::alert_time`] of contact. Non-finite inputs and
/// cars below [`ReactPolicy::min_speed`] are not threats.
pub fn assess(ped: Vec3, car: &Approacher, policy: &ReactPolicy) -> Option<Threat> {
    let v = flat(car.velocity);
    let speed = v.length();
    let inputs_finite = ped.is_finite()
        && car.position.is_finite()
        && v.is_finite()
        && car.half_length.is_finite()
        && car.half_width.is_finite()
        && car.steering.is_finite();
    if !inputs_finite || speed < policy.min_speed {
        return None;
    }
    if flat(ped - car.position).length() >= policy.sense_range {
        return None;
    }
    let dir = v / speed;
    let perp = Vec3::new(-dir.z, 0.0, dir.x);
    let rel = flat(ped - car.position);
    let along = rel.dot(dir);
    let side = rel.dot(perp);
    // Ahead of the car's nose: past `0.25·length` — half a half-length
    // (`0x54e7f3`). Behind it, or alongside, is no threat.
    let nose = NOSE_FRACTION * car.half_length * 2.0;
    if along <= nose {
        return None;
    }
    let lateral = side.abs();
    // The near corridor (`0x54e660`) may dive; the far one (`0x54e8a0`)
    // only stops. Both share the time value (`0x54e630`).
    let near = along < NEAR_RANGE && lateral < car.half_width + NEAR_MARGIN;
    let far = along < FAR_RANGE && lateral < car.half_width + FAR_MARGIN;
    let time_to_contact = (along - TIME_OFFSET) / speed;
    if time_to_contact >= policy.alert_time || !(near || far) {
        return None;
    }
    let alarm = if near && time_to_contact < policy.dive_time {
        Alarm::Imminent
    } else {
        Alarm::Alert
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
        steering: car.steering,
    })
}

/// The car the walker senses: the nearest of `cars` by horizontal
/// squared distance to its centre, with no speed or corridor
/// pre-filter. The first of equals wins (retail replaces the pick only
/// when strictly nearer, `Midtown2.exe` `0x54ba4a`, UNK-43). A
/// non-finite position never wins.
pub fn nearest_car(ped: Vec3, cars: impl IntoIterator<Item = Approacher>) -> Option<Approacher> {
    let mut best: Option<(f32, Approacher)> = None;
    for car in cars {
        let d = flat(ped - car.position).length_squared();
        if d.is_finite() && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, car));
        }
    }
    best.map(|(_, car)| car)
}

/// The threat the walker's one sensed car poses: the nearest car is
/// picked first ([`nearest_car`]) and only it is assessed, so a nearer
/// harmless car hides a farther dangerous one, as in retail.
pub fn sensed_threat(
    ped: Vec3,
    cars: impl IntoIterator<Item = Approacher>,
    policy: &ReactPolicy,
) -> Option<Threat> {
    assess(ped, &nearest_car(ped, cars)?, policy)
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
    /// `threat` the threat from the sensed car this frame, `side` the dive
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
