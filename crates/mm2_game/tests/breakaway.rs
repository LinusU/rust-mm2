//! F05-B.3 contract tests: the authored breakaway rig — the
//! `ImpulseLimit2` detach rule shared with the prop bangers, the
//! one-shot detach bound and repair-side rig restoration.

use bevy::prelude::*;
use mm2_formats::banger::BangerData;
use mm2_game::{BangerDefinition, BreakPartSpec, VehicleBreaks};

fn record(mass: f32, limit: f32) -> BangerData {
    BangerData {
        audio_id: 0,
        size: [0.5, 0.5, 0.1],
        cg: [0.0, 0.0, 0.0],
        num_glows: None,
        glows: Vec::new(),
        mass,
        elasticity: 0.5,
        friction: 0.9,
        impulse_limit2: limit,
        spin_axis: 0,
        flash: 0,
        num_parts: 0,
        birth_rule: None,
        tex_number: 0,
        bill_flags: 0,
        y_radius: 0.0,
        collider_id: None,
        collision_prim: None,
        collision_type: None,
        warnings: Vec::new(),
    }
}

fn spec(name: &str, limit: f32) -> BreakPartSpec {
    BreakPartSpec {
        name: name.to_string(),
        def: BangerDefinition::from_record(format!("vpcar_{name}"), &record(25.0, limit)),
    }
}

/// A two-part rig: mass 25 each, a light trim piece (limit 700 →
/// detach at 28 m/s) and a heavy panel (limit 1.8 M → 72 000 m/s,
/// "never") — the retail pattern, where `limit / mass` is the authored
/// detach speed.
fn rig() -> VehicleBreaks {
    VehicleBreaks::new(vec![spec("break01", 700.0), spec("break0", 1_800_000.0)])
}

#[test]
fn the_rig_carries_the_authored_specs_verbatim() {
    let rig = rig();
    assert_eq!(rig.parts.len(), 2);
    assert_eq!(rig.parts[0].spec.name, "break01");
    assert_eq!(rig.parts[1].spec.name, "break0");
    assert_eq!(rig.parts[0].spec.def.impulse_limit2, 700.0);
    assert_eq!(rig.parts[1].spec.def.impulse_limit2, 1_800_000.0);
    assert!(rig.parts.iter().all(|p| p.attached && p.fragment.is_none()));
    assert_eq!(rig.detached_count(), 0);
}

#[test]
fn detachable_reads_each_parts_own_authored_limit() {
    let rig = rig();
    // Below the 28 m/s trim threshold: nothing.
    assert!(rig.detachable(27.0).is_empty());
    // Past the trim limit only (28 × 25 = 700 exactly — at is not over).
    assert!(rig.detachable(28.0).is_empty());
    assert_eq!(rig.detachable(28.1), vec![0]);
    // Past both (72 000 m/s panel threshold).
    assert_eq!(rig.detachable(80_000.0), vec![0, 1]);
}

#[test]
fn already_detached_parts_are_not_re_reported() {
    let mut rig = rig();
    let frag = Entity::from_raw_u32(7).unwrap();
    assert!(rig.detach(0, Some(frag)));
    assert_eq!(rig.detached_count(), 1);
    // The detach itself is the dedup: a second qualifying impact
    // reports only the still-attached parts.
    assert_eq!(rig.detachable(80_000.0), vec![1]);
    assert!(!rig.detach(0, None), "a part detaches once per rig restore");
}

#[test]
fn garbage_speeds_never_detach() {
    let rig = rig();
    for speed in [f32::NAN, 0.0, -500.0] {
        assert!(rig.detachable(speed).is_empty());
    }
    // +inf is finite-checkable junk too — never a detach verdict.
    assert!(rig.detachable(f32::INFINITY).is_empty());
}

#[test]
fn detach_is_bounded_and_bad_indices_do_nothing() {
    let mut rig = rig();
    assert!(!rig.detach(9, None));
    assert_eq!(rig.detached_count(), 0);
    assert!(rig.detach(1, None));
    assert_eq!(rig.parts[1].fragment, None);
    assert_eq!(rig.detached_count(), 1);
}

#[test]
fn restore_reattaches_and_drains_fragments() {
    let mut rig = rig();
    let f0 = Entity::from_raw_u32(11).unwrap();
    rig.detach(0, Some(f0));
    rig.detach(1, None);
    assert_eq!(rig.detached_count(), 2);

    let drained = rig.restore();
    assert_eq!(drained, vec![f0], "only spawned fragments drain");
    assert_eq!(rig.detached_count(), 0);
    assert!(rig.parts.iter().all(|p| p.attached && p.fragment.is_none()));
    // Idempotent: a second restore touches nothing.
    assert!(rig.restore().is_empty());
}
