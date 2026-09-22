//! F05-A damage-contract tests: authored bounds → `DamageSpec`, the
//! threshold-gated accumulator, tier boundaries, regeneration and the
//! per-mode disabled outcome.

use mm2_formats::tune::TuneFile;
use mm2_formats::veh::VehCarDamage;
use mm2_game::*;

const CARDAMAGE: &str = "type: a\n\
vehCarDamage {\n\
  MaxDamage 321300.0\n\
  MedDamage 150000.0\n\
  ImpactThreshold 1500.0\n\
  RegenerateRate 0.0\n\
  SmokeOffset 0.0 0.5 -1.0\n\
  TextelDamageRadius 0.5\n\
  Position 0.0 0.0 0.0\n\
  PositionVar 0.1 0.1 0.1\n\
  Velocity 0.0 1.0 0.0\n\
  VelocityVar 0.5 0.5 0.5\n\
  Life 1.0\n\
  LifeVar 0.5\n\
  Mass 1.0\n\
  MassVar 0.0\n\
  Radius 0.5\n\
  RadiusVar 0.25\n\
  Drag 0.0\n\
  DragVar 0.0\n\
  Damp 0.0\n\
  DampVar 0.0\n\
  DRadius 0.5\n\
  DRadiusVar 0.0\n\
  DAlpha -0.5\n\
  DAlphaVar 0.0\n\
  DRotation 0.0\n\
  DRotationVar 0.0\n\
  InitialBlast 0\n\
  SpewRate 10.0\n\
  SpewTimeLimit 0.0\n\
  Gravity -9.8\n\
  TexFrameStart 0\n\
  TexFrameEnd 0\n\
  BirthFlags 0\n\
  Height 0.0\n\
  Intensity 1.0\n\
  Color -167772161\n\
  SmokeOffset2 0.0 0.5 1.0\n\
  DoublePivot 0\n\
}\n";

fn spec() -> DamageSpec {
    let tune = TuneFile::parse(CARDAMAGE).unwrap();
    let d = VehCarDamage::from_tune(&tune).unwrap();
    DamageSpec::from(&d)
}

#[test]
fn the_spec_reads_the_authored_bounds() {
    let spec = spec();
    assert_eq!(spec.max_damage, 321300.0);
    assert_eq!(spec.med_damage, 150000.0);
    assert_eq!(spec.impact_threshold, 1500.0);
    assert_eq!(spec.regenerate_rate, 0.0);
}

#[test]
fn sub_threshold_and_garbage_impacts_never_accumulate() {
    let spec = spec();
    let mut state = DamageState::default();
    // F05-AC01: resting contact, curb taps and suspension loads sit
    // below the authored threshold and cannot damage.
    for severity in [0.0, -5.0, 1.0, 1499.9, 1500.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            state.apply(severity, &spec),
            DamageVerdict::Rejected,
            "severity {severity}"
        );
    }
    assert_eq!(state.total(), 0.0);
    assert_eq!(state.condition(&spec), DamageTier::Intact);
    assert_eq!(state.health_fraction(&spec), 1.0);
}

#[test]
fn damage_accumulates_through_the_tiers_and_saturates() {
    let spec = spec();
    let mut state = DamageState::default();

    assert_eq!(state.apply(2000.0, &spec), DamageVerdict::Intact);
    assert_eq!(state.total(), 2000.0);

    // Crossing MedDamage moves into the damaged band.
    assert_eq!(state.apply(149000.0, &spec), DamageVerdict::Damaged);
    assert_eq!(state.condition(&spec), DamageTier::Damaged);
    assert!((state.health_fraction(&spec) - 0.530).abs() < 0.001);

    // An overkill hit saturates at MaxDamage rather than growing past.
    assert_eq!(state.apply(1e9, &spec), DamageVerdict::Disabled);
    assert_eq!(state.total(), spec.max_damage);
    assert_eq!(state.remaining(&spec), 0.0);
    assert_eq!(state.health_fraction(&spec), 0.0);

    // Once disabled, further impacts stay rejected-or-saturated: the
    // total never exceeds the authored bound.
    state.apply(1e9, &spec);
    assert_eq!(state.total(), spec.max_damage);
}

#[test]
fn regeneration_heals_but_never_below_zero() {
    let mut spec = spec();
    spec.regenerate_rate = 1000.0;
    let mut state = DamageState::default();
    state.apply(50_000.0, &spec);
    state.tick(10.0, &spec);
    assert_eq!(state.total(), 40_000.0);
    state.tick(1000.0, &spec);
    assert_eq!(state.total(), 0.0);
    // No authored regeneration — every retail record — is a no-op.
    state.apply(2000.0, &spec);
    spec.regenerate_rate = 0.0;
    state.tick(10.0, &spec);
    assert_eq!(state.total(), 2000.0);
}

#[test]
fn repair_and_reset_restore_full_health() {
    let spec = spec();
    let mut state = DamageState::default();
    state.apply(400_000.0, &spec);
    assert_eq!(state.condition(&spec), DamageTier::Disabled);
    state.repair();
    assert_eq!(state.condition(&spec), DamageTier::Intact);
    state.apply(400_000.0, &spec);
    state.reset();
    assert_eq!(state.total(), 0.0);
}

#[test]
fn vehicle_damage_rejects_duplicate_and_stale_impact_ids() {
    // F05-AC06: the per-vehicle watermark means a re-delivered or
    // out-of-order impact can never double-apply, on top of the
    // upstream pair dedup.
    let mut damage = VehicleDamage::new(spec());
    assert_eq!(damage.apply(ImpactId(1), 2000.0), DamageVerdict::Intact);
    // Re-delivery of the same id never accumulates twice.
    assert_eq!(damage.apply(ImpactId(1), 2000.0), DamageVerdict::Duplicate);
    assert_eq!(damage.total(), 2000.0);
    // A later id still lands; an older one arriving after it is stale.
    assert_eq!(damage.apply(ImpactId(3), 5000.0), DamageVerdict::Intact);
    assert_eq!(damage.apply(ImpactId(2), 5000.0), DamageVerdict::Duplicate);
    assert_eq!(damage.total(), 7000.0);
    // The watermark survives a repair — a late duplicate of a
    // pre-repair impact still cannot land.
    damage.repair();
    assert_eq!(damage.total(), 0.0);
    assert_eq!(damage.apply(ImpactId(3), 9000.0), DamageVerdict::Duplicate);
    assert_eq!(damage.total(), 0.0);
}

#[test]
fn vehicle_damage_wraps_the_state_against_its_authored_spec() {
    let mut damage = VehicleDamage::new(spec());
    assert_eq!(damage.condition(), DamageTier::Intact);
    assert_eq!(damage.health_fraction(), 1.0);
    // Sub-threshold deliveries advance the watermark without
    // accumulating — a rejected impact is still consumed.
    assert_eq!(damage.apply(ImpactId(1), 100.0), DamageVerdict::Rejected);
    assert_eq!(damage.apply(ImpactId(1), 100.0), DamageVerdict::Duplicate);
    damage.apply(ImpactId(2), 400_000.0);
    assert_eq!(damage.condition(), DamageTier::Disabled);
    assert_eq!(damage.health_fraction(), 0.0);
    damage.reset();
    assert_eq!(damage.condition(), DamageTier::Intact);
}

#[test]
fn impairment_steps_down_at_the_smoke_gate_and_ramps_to_max() {
    // DSN-25: the documented "when the engine spews smoke" coupling —
    // the factor drops below 1 at exactly the bound the DSN-24 smoke
    // gate emits on (`MedDamage`), then ramps to the designed floor.
    let spec = spec(); // med 150 000, max 321 300
    let policy = ImpairmentPolicy::default();
    assert_eq!(policy.factor(0.0, &spec), 1.0);
    assert_eq!(policy.factor(spec.med_damage - 1.0, &spec), 1.0);
    assert_eq!(
        policy.factor(spec.med_damage, &spec),
        policy.power_at_med,
        "impaired exactly when smoke starts"
    );
    let mid = policy.factor((spec.med_damage + spec.max_damage) * 0.5, &spec);
    let expected = policy.power_at_med + (policy.power_at_max - policy.power_at_med) * 0.5;
    assert!((mid - expected).abs() < 1e-6);
    assert_eq!(policy.factor(spec.max_damage, &spec), policy.power_at_max);
    // Past the bound the factor clamps at the floor (the accumulator
    // saturates there anyway).
    assert_eq!(
        policy.factor(spec.max_damage * 2.0, &spec),
        policy.power_at_max
    );
}

#[test]
fn impairment_degenerates_to_full_output_never_a_stall() {
    let policy = ImpairmentPolicy::default();
    let spec = spec();
    // Non-finite totals and spec fields can never stall the car.
    for total in [f32::NAN, f32::NEG_INFINITY] {
        assert_eq!(policy.factor(total, &spec), 1.0);
    }
    let mut bad = spec;
    bad.med_damage = f32::NAN;
    assert_eq!(policy.factor(spec.max_damage, &bad), 1.0);
    // Degenerate band (med >= max): the mid tier drops to the floor
    // the moment it is reached, mirroring SmokePolicy's max-rate leg.
    let mut flat = spec;
    flat.med_damage = flat.max_damage;
    assert_eq!(policy.factor(flat.med_damage - 1.0, &flat), 1.0);
    assert_eq!(policy.factor(flat.med_damage, &flat), policy.power_at_max);
    // Garbage floors sanitise instead of poisoning the ramp.
    let nan_policy = ImpairmentPolicy {
        power_at_med: f32::NAN,
        power_at_max: -5.0,
    };
    assert_eq!(nan_policy.factor(spec.med_damage, &spec), 1.0);
    assert_eq!(nan_policy.factor(spec.max_damage, &spec), 0.0);
}

#[test]
fn disabled_outcome_is_per_mode() {
    let ev = |table| {
        SessionMode::Event(EventRef {
            city: "sf".into(),
            table,
            index: 0,
        })
    };
    assert_eq!(
        disabled_outcome(&SessionMode::Cruise),
        DisabledOutcome::FreeReset
    );
    assert_eq!(
        disabled_outcome(&ev(EventTableKind::Blitz)),
        DisabledOutcome::RestartEvent
    );
    assert_eq!(
        disabled_outcome(&ev(EventTableKind::Checkpoint)),
        DisabledOutcome::RestartEvent
    );
    assert_eq!(
        disabled_outcome(&ev(EventTableKind::Circuit)),
        DisabledOutcome::PenaltyReset
    );
    assert_eq!(
        disabled_outcome(&ev(EventTableKind::CrashCourse)),
        DisabledOutcome::RestartEvent
    );
}
