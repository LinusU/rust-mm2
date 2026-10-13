//! F05-B.3 contract tests: the authored breakaway rig — the recovered
//! manager-wide impulse / nearest-part detach rule (REC-2), the
//! one-shot detach bound and repair-side rig restoration.

use bevy::prelude::*;
use mm2_formats::banger::BangerData;
use mm2_game::{BangerDefinition, BreakGate, BreakPartSpec, NavRng, VehicleBreaks};

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
        gate: BreakGate::Impact,
    }
}

/// A damage-driven wheel spec (`whl0..3`).
fn wheel(name: &str) -> BreakPartSpec {
    BreakPartSpec {
        name: name.to_string(),
        def: BangerDefinition::from_record(format!("vpcar_{name}"), &record(25.0, 700.0)),
        gate: BreakGate::Damage,
    }
}

/// A two-part rig: mass 25 each, a light trim piece (limit 700) and a
/// heavy panel (limit 1.8 M) — the retail `Mass × {31.25, 2500}`
/// pattern. The authored limits are fragment data now: detachment is
/// gated by the manager-wide impulse, not by these.
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
fn the_manager_gate_is_ten_thousand_kilogram_metres_per_second() {
    let rig = rig();
    assert_eq!(VehicleBreaks::DETACH_IMPULSE, 10_000.0);
    // Just below the gate: nothing. At it: the rig is eligible.
    assert!(!rig.detaches_on(9_999.0));
    assert!(rig.detaches_on(VehicleBreaks::DETACH_IMPULSE));
    assert!(rig.detaches_on(1.0e9));
}

#[test]
fn the_authored_part_limit_no_longer_gates_detachment() {
    // A part authoring a 1-unit limit against its 25 kg mass would have
    // detached at ~0.04 m/s under the old per-part reading (DSN-21). The
    // recovered gate ignores the limit entirely: only the manager
    // impulse decides, so nothing detaches below 10 000 kg·m/s.
    let eager = VehicleBreaks::new(vec![spec("break0", 1.0)]);
    assert!(!eager.detaches_on(9_999.0));
    assert!(eager.detaches_on(10_000.0));
}

#[test]
fn garbage_impulses_never_detach() {
    let rig = rig();
    for impulse in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -500.0] {
        assert!(!rig.detaches_on(impulse), "{impulse}");
    }
}

#[test]
fn nearest_attached_picks_the_closest_anchor() {
    let rig = rig();
    let anchors = [
        (0, Vec3::new(0.0, 0.0, 0.0)),
        (1, Vec3::new(10.0, 0.0, 0.0)),
    ];
    assert_eq!(
        rig.nearest_attached(Vec3::new(1.0, 0.0, 0.0), anchors),
        Some(0)
    );
    assert_eq!(
        rig.nearest_attached(Vec3::new(9.0, 0.0, 0.0), anchors),
        Some(1)
    );
    // An exact tie keeps the lowest index — a deterministic choice.
    assert_eq!(
        rig.nearest_attached(Vec3::new(5.0, 0.0, 0.0), anchors),
        Some(0)
    );
}

#[test]
fn nearest_attached_skips_detached_and_anchorless_parts() {
    let mut rig = rig();
    rig.detach(0, None);
    // Part 0 is off the rig even though it anchors on the contact: the
    // scan only considers attached parts.
    assert_eq!(
        rig.nearest_attached(
            Vec3::ZERO,
            [(0, Vec3::ZERO), (1, Vec3::new(10.0, 0.0, 0.0))]
        ),
        Some(1)
    );
    // A part with no anchor (no render node) is never chosen.
    assert_eq!(
        rig.nearest_attached(Vec3::ZERO, [(1, Vec3::new(4.0, 0.0, 0.0))]),
        Some(1)
    );
    rig.detach(1, None);
    assert!(!rig.detaches_on(1.0e9), "nothing attached, nothing to take");
    assert_eq!(rig.nearest_attached(Vec3::ZERO, Vec::new()), None);
}

#[test]
fn nearest_attached_rejects_junk_contacts_and_anchors() {
    let rig = rig();
    assert_eq!(
        rig.nearest_attached(Vec3::NAN, [(0, Vec3::ZERO)]),
        None,
        "a non-finite contact is not a detach event"
    );
    // A non-finite anchor is skipped; a valid one still wins.
    assert_eq!(
        rig.nearest_attached(Vec3::ZERO, [(0, Vec3::NAN), (1, Vec3::new(3.0, 0.0, 0.0))]),
        Some(1)
    );
    // An out-of-range part index is ignored.
    assert_eq!(rig.nearest_attached(Vec3::ZERO, [(9, Vec3::ZERO)]), None);
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

#[test]
fn impact_gate_never_takes_a_wheel() {
    // The two triggers are separate recovered rules: an impact reaching
    // the manager gate detaches panels, never a damage-gated wheel, and
    // the wheel is not a candidate for the nearest-part scan.
    let rig = VehicleBreaks::new(vec![spec("break0", 700.0), wheel("whl0"), wheel("whl1")]);
    assert!(rig.detaches_on(1.0e9));
    assert_eq!(
        rig.nearest_attached(
            Vec3::ZERO,
            [
                (0, Vec3::new(10.0, 0.0, 0.0)),
                (1, Vec3::ZERO),
                (2, Vec3::ZERO),
            ]
        ),
        Some(0),
        "the nearest attached *panel* wins over closer wheels"
    );
    // With the only panel gone, an impact has nothing left to take.
    let mut rig = VehicleBreaks::new(vec![spec("break0", 700.0), wheel("whl0")]);
    rig.detach(0, None);
    assert!(!rig.detaches_on(1.0e9));
    assert_eq!(rig.nearest_attached(Vec3::ZERO, [(1, Vec3::ZERO)]), None);
}

fn wheel_rig(n: usize) -> VehicleBreaks {
    VehicleBreaks::new((0..n).map(|i| wheel(&format!("whl{i}"))).collect())
}

#[test]
fn eject_bands_follow_speed_mph() {
    let mut rng = NavRng::new(7);
    assert_eq!(VehicleBreaks::EJECT_ONE_MPH, 50.0);
    assert_eq!(VehicleBreaks::EJECT_TWO_MPH, 75.0);
    assert_eq!(VehicleBreaks::EJECT_ALL_MPH, 100.0);

    // At or below 50 mph: nothing, but the episode still latches.
    let mut slow = wheel_rig(4);
    assert!(slow.eject_at(50.0, &mut rng).is_empty());
    assert!(slow.ejected);

    // Above 50: one corner.
    let mut one = wheel_rig(4);
    let picked = one.eject_at(50.1, &mut rng);
    assert_eq!(picked.len(), 1);
    assert!(picked[0] < 4);

    // Above 75: two corners — unless the second draw repeats the first,
    // which the leaf `0x4d8930` refuses (already detached), so the
    // episode takes one wheel. Sweep seeds to observe both outcomes and
    // pin that a repeat never yields a duplicate index.
    let mut saw_one = false;
    let mut saw_two = false;
    for seed in 0..64 {
        let mut two = wheel_rig(4);
        let mut seed_rng = NavRng::new(seed);
        let picked = two.eject_at(75.1, &mut seed_rng);
        assert!(picked.len() <= 2, "at most one wheel per drawn corner");
        assert!(picked.iter().all(|i| *i < 4));
        let mut unique = picked.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique,
            {
                let mut sorted = picked.clone();
                sorted.sort_unstable();
                sorted
            },
            "a repeated corner is refused, never ejected twice"
        );
        match picked.len() {
            1 => saw_one = true,
            2 => saw_two = true,
            other => panic!("two-pair band drew {other} corners"),
        }
    }
    assert!(
        saw_one && saw_two,
        "the two-pair band can draw the same corner (one wheel) or two"
    );

    // Above 100: every wheel.
    let mut all = wheel_rig(4);
    assert_eq!(all.eject_at(100.1, &mut rng), vec![0, 1, 2, 3]);
    assert_eq!(
        all.eject_at(200.0, &mut rng),
        Vec::<usize>::new(),
        "one-shot"
    );
}

#[test]
fn eject_is_one_shot_until_restore() {
    let mut rig = wheel_rig(4);
    let mut rng = NavRng::new(3);
    assert_eq!(rig.eject_at(150.0, &mut rng).len(), 4);
    // The episode latched: a second wreck before a repair draws nothing.
    assert!(rig.eject_at(150.0, &mut rng).is_empty());
    // The model reset re-arms it (the original's `0x4cdfe0`).
    rig.restore();
    assert!(!rig.ejected);
    assert_eq!(rig.eject_at(150.0, &mut rng).len(), 4);
}

#[test]
fn eject_ignores_non_finite_speed_and_panel_only_rigs() {
    let mut rig = wheel_rig(4);
    let mut rng = NavRng::new(5);
    assert!(rig.eject_at(f32::NAN, &mut rng).is_empty());
    assert!(rig.ejected, "a junk reading still closes the episode");

    // A rig with no damage-gated parts ejects nothing at any speed.
    let mut panels = VehicleBreaks::new(vec![spec("break0", 700.0)]);
    assert!(panels.eject_at(200.0, &mut rng).is_empty());
}
