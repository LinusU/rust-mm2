//! Particle-effect contracts (F05-B.6) — the authored emission specs
//! and the deterministic puff state the app renders.
//!
//! The `vehCarDamage` record embeds a flat particle spec — mm2hook's
//! recovered `vehCarDamage` struct names it `EngineSmokeRule`: engine
//! smoke emitted from `SmokeOffset`/`SmokeOffset2` while the vehicle
//! is damaged, with `DoublePivot` gating whether both pivots emit and
//! `m_CurrentPivot` an alternation cursor between them. The original
//! `vehCarDamage::Update()` is a binary thunk, so the emission gate,
//! cadence and per-field interpretation are designed policy (DSN-24);
//! the pivots, the particle field values and the atlas/tile vocabulary
//! are authored. `TextelDamageRadius` and the recovered `ImpactsTable`
//! texel-damage leg stay unconsumed here (still UNK-13).

use bevy::prelude::*;

use mm2_formats::veh::{DamageEffect, VehCarDamage};

use crate::damage::DamageSpec;
use crate::nav::NavRng;

/// Distilled authored particle spec — the `vehCarDamage` effect fields
/// verbatim, so consumers bind authored values rather than a subset.
///
/// Consumed by the smoke sim: `position_var`, `velocity`/`_var`,
/// `life`/`_var`, `radius`/`_var`, `drag`/`_var`, `d_radius`/`_var`,
/// `d_alpha`/`_var`, `gravity`, `tex_frame_start`/`tex_frame_end`,
/// `color`.
///
/// Carried but not yet consumed: `position` (the authored emitter
/// pivots own placement; retail `Position` is near-zero anyway),
/// `mass`/`mass_var`, `damp`/`damp_var`, `d_rotation`/`d_rotation_var`
/// (0 on retail), `initial_blast`, `spew_rate`/`spew_time_limit`
/// (driver fields — 0 on every retail damage record; the designed
/// [`SmokePolicy`] owns cadence), `birth_flags`, `height`, `intensity`.
/// Their original semantics are unrecovered (UNK-13).
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleSpec {
    pub position: Vec3,
    pub position_var: Vec3,
    pub velocity: Vec3,
    pub velocity_var: Vec3,
    pub life: f32,
    pub life_var: f32,
    pub mass: f32,
    pub mass_var: f32,
    pub radius: f32,
    pub radius_var: f32,
    pub drag: f32,
    pub drag_var: f32,
    pub damp: f32,
    pub damp_var: f32,
    pub d_radius: f32,
    pub d_radius_var: f32,
    pub d_alpha: f32,
    pub d_alpha_var: f32,
    pub d_rotation: f32,
    pub d_rotation_var: f32,
    pub initial_blast: i64,
    pub spew_rate: f32,
    pub spew_time_limit: f32,
    pub gravity: f32,
    pub tex_frame_start: i64,
    pub tex_frame_end: i64,
    pub birth_flags: i64,
    pub height: f32,
    pub intensity: f32,
    /// Packed authored `Color` word — retail decodes as high-alpha
    /// ARGB (e.g. `0xF6000000`, near-opaque black smoke).
    pub color: i64,
}

impl From<&DamageEffect> for ParticleSpec {
    fn from(e: &DamageEffect) -> Self {
        Self {
            position: e.position.into(),
            position_var: e.position_var.into(),
            velocity: e.velocity.into(),
            velocity_var: e.velocity_var.into(),
            life: e.life,
            life_var: e.life_var,
            mass: e.mass,
            mass_var: e.mass_var,
            radius: e.radius,
            radius_var: e.radius_var,
            drag: e.drag,
            drag_var: e.drag_var,
            damp: e.damp,
            damp_var: e.damp_var,
            d_radius: e.d_radius,
            d_radius_var: e.d_radius_var,
            d_alpha: e.d_alpha,
            d_alpha_var: e.d_alpha_var,
            d_rotation: e.d_rotation,
            d_rotation_var: e.d_rotation_var,
            initial_blast: e.initial_blast,
            spew_rate: e.spew_rate,
            spew_time_limit: e.spew_time_limit,
            gravity: e.gravity,
            tex_frame_start: e.tex_frame_start,
            tex_frame_end: e.tex_frame_end,
            birth_flags: e.birth_flags,
            height: e.height,
            intensity: e.intensity,
            color: e.color,
        }
    }
}

/// Designed emission policy for engine smoke (DSN-24). The recovered
/// struct proves the original emits smoke from the authored pivots
/// while damaged; its `Update()` is a thunk, so the gate and rate
/// shape here are ours, not recovered original behavior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokePolicy {
    /// Puffs/second emitted once damage reaches `MedDamage` —
    /// designed. Smoke is the damaged-tier signal, matching the DMG-2
    /// "yellow band" reading of `MedDamage`.
    pub rate_at_med: f32,
    /// Puffs/second at `MaxDamage` — designed.
    pub rate_at_max: f32,
    /// Live-puff bound per vehicle — designed (F05-AC03 bounded
    /// cleanup: a damaged vehicle can never flood the world).
    pub max_live: usize,
    /// Tiles across the smoke atlas — `texture/fxpt2` is a measured
    /// 2×2 puff atlas and `TexFrameStart`/`TexFrameEnd` index its
    /// tiles (mm2hook `asSparkPos::TexCoordOffset` reads frames the
    /// same way).
    pub atlas_tiles: u32,
}

impl Default for SmokePolicy {
    fn default() -> Self {
        Self {
            rate_at_med: 4.0,
            rate_at_max: 24.0,
            max_live: 48,
            atlas_tiles: 2,
        }
    }
}

impl SmokePolicy {
    /// Emission rate for a damage `total` under `spec`: zero below
    /// `MedDamage`, then a linear ramp to `rate_at_max` at
    /// `MaxDamage` (designed — the original cadence is unrecovered).
    /// A degenerate `MedDamage >= MaxDamage` spec emits at the max
    /// rate as soon as the mid tier is reached.
    pub fn rate(&self, total: f32, spec: &DamageSpec) -> f32 {
        if !(total.is_finite() && total >= spec.med_damage) {
            return 0.0;
        }
        let span = spec.max_damage - spec.med_damage;
        if span <= 0.0 {
            return self.rate_at_max;
        }
        let t = ((total - spec.med_damage) / span).clamp(0.0, 1.0);
        self.rate_at_med + (self.rate_at_max - self.rate_at_med) * t
    }

    /// Clamp an authored frame index into the atlas's tile space.
    pub fn tile(&self, frame: i64) -> usize {
        frame.clamp(0, (self.atlas_tiles * self.atlas_tiles) as i64 - 1) as usize
    }
}

/// One authored emission anchor — a car-space pivot plus the
/// deterministic RNG that keeps per-puff jitter replicable.
#[derive(Debug, Clone)]
pub struct SmokeEmitter {
    /// Car-space pivot — an authored `SmokeOffset`/`SmokeOffset2`, or
    /// the mirror-derived second pivot when `MirrorPivot != 0` (that
    /// reading is designed; all 7 retail `MirrorPivot` fields are 0).
    pub pivot: Vec3,
    rng: NavRng,
}

impl SmokeEmitter {
    /// Uniform `v ± var` draw on this emitter's stream.
    fn jitter(&mut self, v: f32, var: f32) -> f32 {
        v + (self.rng.next_f32() * 2.0 - 1.0) * var
    }

    fn jitter3(&mut self, v: Vec3, var: Vec3) -> Vec3 {
        Vec3::new(
            self.jitter(v.x, var.x),
            self.jitter(v.y, var.y),
            self.jitter(v.z, var.z),
        )
    }
}

/// Component: the authored smoke rig for one vehicle — which pivots
/// emit and how (the recovered `m_CurrentPivot`/`DoublePivot` gate),
/// the authored particle spec, and the designed emission policy.
#[derive(Component)]
pub struct VehicleSmoke {
    /// The distilled authored spec (`EngineSmokeRule`).
    pub spec: ParticleSpec,
    /// Resolved emission pivots — always at least `SmokeOffset`.
    pub emitters: Vec<SmokeEmitter>,
    /// Emission policy — designed (DSN-24).
    pub policy: SmokePolicy,
    /// Emit every pivot per burst (`DoublePivot != 0`) — authored gate.
    pub double: bool,
    /// Fractional burst carry — accumulates `rate*dt` to whole puffs.
    acc: f32,
    /// Alternation cursor — the recovered struct's `m_CurrentPivot`.
    current: usize,
}

impl VehicleSmoke {
    /// Resolve the authored pivot set (gate readings are designed —
    /// see module docs and `docs/research/damage.md`):
    ///
    /// - `SmokeOffset` always emits.
    /// - `MirrorPivot != 0` derives the second pivot by mirroring
    ///   `SmokeOffset` about the car's sagittal (x = 0) plane.
    /// - Otherwise a non-zero `SmokeOffset2` is the second pivot.
    /// - `DoublePivot != 0` emits every pivot each burst; with one
    ///   pivot it is a no-op. Single-pivot rigs alternate one pivot
    ///   per burst via `m_CurrentPivot`.
    ///
    /// `seed` must be session-stable (the vehicle's object id) so the
    /// emission stream replays identically — replicable by
    /// construction (F05 req 6).
    pub fn new(damage: &VehCarDamage, policy: SmokePolicy, seed: u64) -> Self {
        let mut pivots = vec![Vec3::from(damage.smoke_offset)];
        if damage.mirror_pivot.is_some_and(|m| m != 0) {
            let p = damage.smoke_offset;
            pivots.push(Vec3::new(-p[0], p[1], p[2]));
        } else if damage.smoke_offset2.iter().any(|v| *v != 0.0) {
            pivots.push(damage.smoke_offset2.into());
        }
        let emitters = pivots
            .into_iter()
            .enumerate()
            .map(|(i, pivot)| SmokeEmitter {
                pivot,
                rng: NavRng::new(seed.wrapping_add(i as u64)),
            })
            .collect();
        Self {
            spec: ParticleSpec::from(&damage.effect),
            emitters,
            policy,
            double: damage.double_pivot != 0,
            acc: 0.0,
            current: 0,
        }
    }

    /// Advance emission one frame and return one emitter index per
    /// puff to spawn — `rate` puffs/second in bursts, alternating the
    /// resolved pivots (or all of them under `DoublePivot`). `live`
    /// is the vehicle's live puff count; `max_live` bounds the total
    /// and may cut a burst short. The alternation cursor advances only
    /// on emitted puffs, so a dry stretch never desynchronizes it.
    pub fn draw(&mut self, dt: f32, rate: f32, live: usize) -> Vec<usize> {
        if !(rate > 0.0 && dt.is_finite() && dt > 0.0) || self.emitters.is_empty() {
            return Vec::new();
        }
        self.acc += rate * dt;
        let bursts = self.acc.floor() as usize;
        if bursts == 0 {
            return Vec::new();
        }
        self.acc -= bursts as f32;
        let mut room = self.policy.max_live.saturating_sub(live);
        let mut out = Vec::with_capacity(bursts);
        for _ in 0..bursts {
            if room == 0 {
                break;
            }
            if self.double {
                for e in 0..self.emitters.len() {
                    if room == 0 {
                        break;
                    }
                    out.push(e);
                    room -= 1;
                }
            } else {
                out.push(self.current % self.emitters.len());
                self.current = (self.current + 1) % self.emitters.len();
                room -= 1;
            }
        }
        out
    }

    /// Build one puff for emitter `i` at world-space `origin` — the
    /// authored spec drawn through this emitter's deterministic
    /// stream. `origin` already includes the vehicle transform; the
    /// authored `Position`/`PositionVar` jitter applies around it.
    pub fn puff(&mut self, i: usize, origin: Vec3, emitter: Entity) -> SmokePuff {
        let spec = &self.spec;
        let e = &mut self.emitters[i];
        let frame = if spec.tex_frame_end >= spec.tex_frame_start {
            spec.tex_frame_start
                + (e.rng.next_u64() % (spec.tex_frame_end - spec.tex_frame_start + 1) as u64) as i64
        } else {
            spec.tex_frame_start
        };
        SmokePuff {
            emitter,
            position: origin + spec.position + e.jitter3(Vec3::ZERO, spec.position_var),
            velocity: e.jitter3(spec.velocity, spec.velocity_var),
            age: 0.0,
            life: e.jitter(spec.life, spec.life_var).max(0.01),
            radius: e.jitter(spec.radius, spec.radius_var).max(0.0),
            d_radius: e.jitter(spec.d_radius, spec.d_radius_var),
            drag: e.jitter(spec.drag, spec.drag_var),
            gravity: spec.gravity,
            d_alpha: e.jitter(spec.d_alpha, spec.d_alpha_var),
            frame: self.policy.tile(frame) as i64,
            color: spec.color,
        }
    }
}

/// Component: one live smoke puff — the entity itself is the
/// particle. [`SmokePuff::advance`] integrates the authored fields;
/// the render side owns transform, atlas tile and material alpha.
#[derive(Component, Debug, Clone)]
pub struct SmokePuff {
    /// The vehicle entity that emitted this puff — pool accounting
    /// (the `max_live` bound counts puffs per emitter).
    pub emitter: Entity,
    /// World-space position — integrated each step.
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub life: f32,
    /// Sprite half-extent in metres — `Radius + DRadius·age` drawn.
    pub radius: f32,
    /// `DRadius` — authored per-second growth.
    pub d_radius: f32,
    /// `Drag` — authored velocity decay coefficient.
    pub drag: f32,
    /// `Gravity` — authored signed rise rate (+Y designed).
    pub gravity: f32,
    /// `DAlpha` — authored alpha-byte drift per second (retail ≈ −83).
    pub d_alpha: f32,
    /// Atlas tile the sprite binds — `TexFrameStart..=TexFrameEnd`
    /// clamped to the policy's tile space at emission.
    pub frame: i64,
    /// Packed authored `Color` word — alpha byte is the initial alpha.
    pub color: i64,
}

impl SmokePuff {
    /// Integrate one step — the field readings are designed
    /// (mm2hook recovers no per-particle integrator for this spec):
    /// `Gravity` adds signed upward velocity (+Y so smoke rises),
    /// `Drag` decays velocity exponentially, `DRadius` grows the
    /// sprite, age accrues. Returns `false` past `life` — the caller
    /// despawns. A non-positive/non-finite `dt` accrues nothing.
    pub fn advance(&mut self, dt: f32) -> bool {
        if dt.is_finite() && dt > 0.0 {
            self.velocity.y += self.gravity * dt;
            self.velocity *= (-self.drag * dt).exp();
            self.position += self.velocity * dt;
            self.radius += self.d_radius * dt;
            self.age += dt;
        }
        self.age < self.life
    }

    /// Sprite alpha in 0..1 — byte-space reading: `Color`'s alpha
    /// byte plus `DAlpha`/second, clamped (designed; retail `DAlpha`
    /// ≈ −83 ⇒ a ~1 s fade inside the ~1.5 s authored life).
    pub fn alpha(&self) -> f32 {
        let a0 = ((self.color as u32 >> 24) & 0xff) as f32;
        (a0 + self.d_alpha * self.age).clamp(0.0, 255.0) / 255.0
    }

    /// RGB tint in 0..1 — `Color`'s low three bytes.
    pub fn rgb(&self) -> [f32; 3] {
        let c = self.color as u32;
        [
            ((c >> 16) & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
            (c & 0xff) as f32 / 255.0,
        ]
    }
}
