//! F05-B.6 unit coverage — the authored particle spec, the designed
//! emission policy/gate and the puff integrator. The gate/cadence and
//! field readings are designed policy (DSN-24); these tests pin the
//! contract, not original behavior.

use mm2_formats::tune::TuneFile;
use mm2_formats::veh::VehCarDamage;
use mm2_game::{DamageSpec, SmokePolicy, SmokePuff, VehicleSmoke};

/// A `vehCardamage` fixture shaped like the retail records — two
/// authored pivots, the designed-ramp spec fields and a black
/// high-alpha `Color`.
const CARDAMAGE: &str = "type: a\r\n\
vehCarDamage {\r\n\
  MaxDamage 321300.000000\r\n\
  MedDamage 150000.000000\r\n\
  ImpactThreshold 1500.000000\r\n\
  RegenerateRate 0.000000\r\n\
  SmokeOffset 0.100000 0.500000 -1.000000\r\n\
  TextelDamageRadius 0.500000\r\n\
  Position 0.000000 0.000000 0.000000\r\n\
  PositionVar 0.100000 0.000000 0.100000\r\n\
  Velocity 0.000000 1.000000 0.000000\r\n\
  VelocityVar 0.500000 0.000000 0.500000\r\n\
  Life 1.000000\r\n\
  LifeVar 0.500000\r\n\
  Mass 1.000000\r\n\
  MassVar 0.000000\r\n\
  Radius 0.500000\r\n\
  RadiusVar 0.250000\r\n\
  Drag 0.500000\r\n\
  DragVar 0.000000\r\n\
  Damp 0.000000\r\n\
  DampVar 0.000000\r\n\
  DRadius 0.500000\r\n\
  DRadiusVar 0.000000\r\n\
  DAlpha -80.000000\r\n\
  DAlphaVar 0.000000\r\n\
  DRotation 0.000000\r\n\
  DRotationVar 0.000000\r\n\
  InitialBlast 0\r\n\
  SpewRate 0.000000\r\n\
  SpewTimeLimit 0.000000\r\n\
  Gravity 8.700000\r\n\
  TexFrameStart 0\r\n\
  TexFrameEnd 3\r\n\
  BirthFlags 0\r\n\
  Height 0.000000\r\n\
  Intensity 1.000000\r\n\
  Color -167772160\r\n\
  SmokeOffset2 -0.100000 0.500000 -1.000000\r\n\
  DoublePivot 0\r\n\
}\r\n";

const SPEC: DamageSpec = DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};

fn damage(src: &str) -> VehCarDamage {
    VehCarDamage::from_tune(&TuneFile::parse(src).unwrap()).unwrap()
}

fn rig(src: &str) -> VehicleSmoke {
    VehicleSmoke::new(&damage(src), SmokePolicy::default(), 7)
}

#[test]
fn spec_carries_the_authored_fields_verbatim() {
    let d = damage(CARDAMAGE);
    let rig = rig(CARDAMAGE);
    let s = &rig.spec;
    assert_eq!(s.velocity, d.effect.velocity.into());
    assert_eq!(s.life, d.effect.life);
    assert_eq!(s.radius, d.effect.radius);
    assert_eq!(s.gravity, d.effect.gravity);
    assert_eq!(s.tex_frame_start, d.effect.tex_frame_start);
    assert_eq!(s.tex_frame_end, d.effect.tex_frame_end);
    assert_eq!(s.color, d.effect.color);
}

#[test]
fn pivots_resolve_per_the_gate_fields() {
    // Two authored pivots, single-pivot alternation.
    let r = rig(CARDAMAGE);
    assert_eq!(r.emitters.len(), 2);
    assert_eq!(r.emitters[0].pivot.to_array(), [0.1, 0.5, -1.0]);
    assert_eq!(r.emitters[1].pivot.to_array(), [-0.1, 0.5, -1.0]);
    assert!(!r.double);

    // `DoublePivot` emits every pivot per burst — the authored gate.
    let double = rig(&CARDAMAGE.replace("DoublePivot 0", "DoublePivot 1"));
    assert!(double.double);

    // A zero `SmokeOffset2` contributes no pivot (retail authors
    // several zero second pivots).
    let one = rig(&CARDAMAGE.replace(
        "SmokeOffset2 -0.100000 0.500000 -1.000000",
        "SmokeOffset2 0.000000 0.000000 0.000000",
    ));
    assert_eq!(one.emitters.len(), 1);

    // `MirrorPivot != 0` derives the second pivot by mirroring the
    // first about x = 0 — and wins over `SmokeOffset2`.
    let mirror = rig(&CARDAMAGE.replace(
        "DoublePivot 0\r\n}",
        "DoublePivot 0\r\n  MirrorPivot 1\r\n}",
    ));
    assert_eq!(mirror.emitters.len(), 2);
    assert_eq!(mirror.emitters[1].pivot.to_array(), [-0.1, 0.5, -1.0]);
}

#[test]
fn rate_gates_on_the_med_tier_and_ramps_to_max() {
    let policy = SmokePolicy::default();
    // Below `MedDamage` — intact tier emits nothing (the damaged-tier
    // signal, DMG-2's yellow band).
    assert_eq!(policy.rate(0.0, &SPEC), 0.0);
    assert_eq!(policy.rate(SPEC.med_damage - 1.0, &SPEC), 0.0);
    // At the mid tier the designed floor rate applies.
    assert_eq!(policy.rate(SPEC.med_damage, &SPEC), policy.rate_at_med);
    // Half-way between mid and max.
    let mid = (SPEC.med_damage + SPEC.max_damage) * 0.5;
    let expected = (policy.rate_at_med + policy.rate_at_max) * 0.5;
    assert!((policy.rate(mid, &SPEC) - expected).abs() < 1e-4);
    // At/above MaxDamage the rate saturates.
    assert_eq!(policy.rate(SPEC.max_damage, &SPEC), policy.rate_at_max);
    assert_eq!(
        policy.rate(SPEC.max_damage * 2.0, &SPEC),
        policy.rate_at_max
    );

    // A degenerate MedDamage >= MaxDamage spec still emits once the
    // mid tier is reached (never silently off).
    let degenerate = DamageSpec {
        med_damage: 400_000.0,
        max_damage: 321_300.0,
        ..SPEC
    };
    assert_eq!(policy.rate(399_000.0, &degenerate), 0.0);
    assert_eq!(policy.rate(400_000.0, &degenerate), policy.rate_at_max);
}

#[test]
fn emission_accumulates_and_alternates_the_pivots() {
    let mut r = rig(CARDAMAGE);
    // rate 10/s at 60 Hz → a burst every 6 frames.
    let mut counts = [0usize; 2];
    let mut emitted = 0usize;
    for _ in 0..60 {
        for idx in r.draw(1.0 / 60.0, 10.0, 0) {
            counts[idx] += 1;
            emitted += 1;
        }
    }
    // ~10 bursts in a second — accumulated fractionally, not
    // dropped; single-pivot mode alternates the authored pivots.
    assert_eq!(emitted, 10);
    assert_eq!((counts[0] as i64 - counts[1] as i64).abs(), 0);
}

#[test]
fn double_pivot_emits_every_pivot_per_burst() {
    let mut r = rig(&CARDAMAGE.replace("DoublePivot 0", "DoublePivot 1"));
    let plan = r.draw(1.0, 1.0, 0);
    assert_eq!(plan, vec![0, 1]);
}

#[test]
fn emission_respects_the_live_bound() {
    let mut r = rig(CARDAMAGE);
    // One puff of room left: the burst truncates instead of
    // overflowing the designed pool bound (F05-AC03).
    let plan = r.draw(1.0, 5.0, SmokePolicy::default().max_live - 1);
    assert_eq!(plan.len(), 1);
    // A full pool emits nothing.
    let plan = r.draw(1.0, 5.0, SmokePolicy::default().max_live);
    assert!(plan.is_empty());
}

#[test]
fn puffs_draw_authored_fields_deterministically() {
    let origin = bevy::prelude::Vec3::new(10.0, 2.0, -30.0);
    let e = bevy::prelude::Entity::PLACEHOLDER;
    let mut a = rig(CARDAMAGE);
    let mut b = rig(CARDAMAGE);
    // Same seed → identical streams (replicable by construction).
    let pa = a.puff(0, origin, e);
    let pb = b.puff(0, origin, e);
    assert_eq!(pa.position, pb.position);
    assert_eq!(pa.velocity, pb.velocity);
    assert_eq!(pa.life, pb.life);
    assert_eq!(pa.frame, pb.frame);

    // Every draw lands inside the authored ±var envelope.
    let s = a.spec.clone();
    for _ in 0..64 {
        let p = a.puff(0, origin, e);
        assert!(
            (p.position.x - (origin.x + s.position.x)).abs() <= s.position_var.x + 1e-6,
            "{p:?}"
        );
        assert!(
            (p.velocity.y - s.velocity.y).abs() <= s.velocity_var.y + 1e-6,
            "{p:?}"
        );
        assert!(
            p.life >= 0.01 && p.life <= s.life + s.life_var + 1e-6,
            "{p:?}"
        );
        assert!(
            p.radius >= 0.0 && p.radius <= s.radius + s.radius_var + 1e-6,
            "{p:?}"
        );
        assert!(
            p.frame >= s.tex_frame_start && p.frame <= s.tex_frame_end,
            "{p:?}"
        );
        assert_eq!(p.color, s.color);
        assert_eq!(p.emitter, e);
    }
}

#[test]
fn frames_clamp_into_the_atlas_tile_space() {
    // `TexFrameEnd` beyond the 2×2 tile space clamps at emission.
    let mut r = rig(&CARDAMAGE.replace("TexFrameEnd 3", "TexFrameEnd 9"));
    let e = bevy::prelude::Entity::PLACEHOLDER;
    for _ in 0..32 {
        let p = r.puff(0, bevy::prelude::Vec3::ZERO, e);
        assert!((0i64..4).contains(&p.frame), "{p:?}");
    }
}

#[test]
fn advance_integrates_the_authored_fields() {
    let mut puff = SmokePuff {
        emitter: bevy::prelude::Entity::PLACEHOLDER,
        position: bevy::prelude::Vec3::ZERO,
        velocity: bevy::prelude::Vec3::new(0.0, 1.0, 0.0),
        age: 0.0,
        life: 1.0,
        radius: 0.5,
        d_radius: 0.5,
        drag: 0.5,
        gravity: 8.7,
        d_alpha: -80.0,
        frame: 2,
        color: -167772160,
    };
    let dt = 0.1;
    let alive = puff.advance(dt);
    // Gravity adds signed upward velocity; drag decays it.
    let expected_vy = (1.0 + 8.7 * dt) * (-0.5f32 * dt).exp();
    assert!((puff.velocity.y - expected_vy).abs() < 1e-5, "{puff:?}");
    assert!(puff.position.y > 0.0, "{puff:?}");
    assert!((puff.radius - 0.55).abs() < 1e-6, "{puff:?}");
    assert!((puff.age - dt).abs() < 1e-6);
    assert!(alive);

    // Age past `life` → expired.
    while puff.advance(dt) {}
    assert!(puff.age >= puff.life);
}

#[test]
fn alpha_fades_in_byte_space() {
    let mut puff = SmokePuff {
        emitter: bevy::prelude::Entity::PLACEHOLDER,
        position: bevy::prelude::Vec3::ZERO,
        velocity: bevy::prelude::Vec3::ZERO,
        age: 0.0,
        life: 10.0,
        radius: 0.5,
        d_radius: 0.0,
        drag: 0.0,
        gravity: 0.0,
        d_alpha: -80.0,
        frame: 0,
        // 0xF6000000 — near-opaque black, the retail smoke tint.
        color: 0xF6000000u32 as i64,
    };
    assert!((puff.alpha() - 246.0 / 255.0).abs() < 1e-4);
    puff.age = 3.0;
    // 246 - 80*3 = 6 → nearly gone; clamps at 0 below that.
    assert!(puff.alpha() < 0.03);
    puff.age = 4.0;
    assert_eq!(puff.alpha(), 0.0);
    assert_eq!(puff.rgb(), [0.0, 0.0, 0.0]);
}

#[test]
fn garbage_dt_emits_and_ages_nothing() {
    let mut r = rig(CARDAMAGE);
    // NaN rate never emits; a real rate against garbage dt accrues
    // nothing usable — draw treats non-positive rates as idle.
    assert!(r.draw(1.0 / 60.0, 0.0, 0).is_empty());
    assert!(r.draw(1.0 / 60.0, -5.0, 0).is_empty());

    let mut puff = SmokePuff {
        emitter: bevy::prelude::Entity::PLACEHOLDER,
        position: bevy::prelude::Vec3::ZERO,
        velocity: bevy::prelude::Vec3::new(0.0, 1.0, 0.0),
        age: 0.0,
        life: 1.0,
        radius: 0.5,
        d_radius: 0.0,
        drag: 0.0,
        gravity: 8.7,
        d_alpha: 0.0,
        frame: 0,
        color: -1,
    };
    let before = puff.clone();
    assert!(puff.advance(0.0));
    assert!(puff.advance(-1.0));
    assert!(puff.advance(f32::NAN));
    assert_eq!(puff.position, before.position);
    assert_eq!(puff.age, 0.0);
}
