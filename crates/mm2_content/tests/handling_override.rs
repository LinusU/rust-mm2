//! The per-car handling-override matrix (F02-AC04): the
//! `--vehicle-config` override channel must reach *every* ready roster
//! car, not just the one car the earlier pass checked. For each ready
//! car this walks the same edits the published sweep in
//! `docs/vehicle-coverage.md` authored — engine output halved, mass
//! doubled, tire grip halved, plus the TOML round-trip an edited dump
//! file takes — through the production
//! [`mm2_content::assemble::apply_handling_override`] the app's
//! `--vehicle-config` and `drive_probe --config` use, asserting each
//! one moves the handling numbers while the imported rig (wheel
//! positions/radii, collision geometry) stays pinned and a wheel-count
//! mismatch is rejected.
//!
//! This is the structural half of the matrix; the measurement half —
//! the overrides actually moving a car's acceleration and braking — is
//! the `drive_probe --override-matrix` leg, whose fresh table lives in
//! `docs/vehicle-coverage.md`.
//!
//! Retail-gated (`MM2_RETAIL=<dir>`); skipped without the operator's
//! install. CI has no retail data, so the implementer and the reviewer
//! both run this locally.

use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_content::assemble::apply_handling_override;
use mm2_content::{EXPECTED_STOCK_ROSTER, VehicleCatalog, load_vehicle};
use mm2_vehicle::VehicleConfig;

/// Mount the operator's install and scan the catalog, or `None` when
/// `MM2_RETAIL` is unset (the skip CI expects).
fn retail_vfs() -> Option<Vfs> {
    let retail = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from)?;
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, &retail, &InstallMount::default()).unwrap();
    Some(vfs)
}

/// The rig is geometry, not tuning: an override that moves a physics
/// wheel off its visual part or reshapes the collider is a misapplied
/// file, whatever it does to the handling numbers.
fn assert_rig_pinned(imported: &VehicleConfig, out: &VehicleConfig, id: &str) {
    assert_eq!(out.wheels.len(), imported.wheels.len(), "{id}: wheel count");
    for (i, (w, src)) in out.wheels.iter().zip(&imported.wheels).enumerate() {
        assert_eq!(w.position, src.position, "{id}: w{i} position moved");
        assert_eq!(w.radius, src.radius, "{id}: w{i} radius moved");
    }
    assert_eq!(
        out.collider_points, imported.collider_points,
        "{id}: collider"
    );
    assert_eq!(out.striker_points, imported.striker_points, "{id}: striker");
    assert_eq!(
        out.chassis_size, imported.chassis_size,
        "{id}: chassis_size"
    );
}

/// One override applied to one car: the whole config must survive the
/// application (the rig re-pin is a no-op on identical values), so the
/// result is byte-identical TOML to the source.
fn apply(imported: &VehicleConfig, over: VehicleConfig, id: &str, what: &str) -> VehicleConfig {
    let out = apply_handling_override(imported, over)
        .unwrap_or_else(|e| panic!("{id}: {what} override rejected: {e}"));
    assert_rig_pinned(imported, &out, id);
    out
}

/// Retail (F02-AC04): the override channel reaches every ready car.
#[test]
fn every_ready_car_takes_the_handling_override_channel() {
    let Some(vfs) = retail_vfs() else {
        eprintln!("skipped: MM2_RETAIL is not set; retail override matrix NOT run");
        return;
    };
    let catalog = VehicleCatalog::scan(&vfs);
    let failures = catalog.stock_audit_failures();
    assert!(failures.is_empty(), "stock roster incomplete: {failures:?}");
    let ready: Vec<_> = catalog.entries.iter().filter(|e| e.is_ready()).collect();
    assert!(
        ready.len() >= EXPECTED_STOCK_ROSTER.len(),
        "only {} ready cars",
        ready.len()
    );

    let tmp = tempfile::tempdir().unwrap();
    for entry in &ready {
        let id = &entry.id;
        let def = load_vehicle(&vfs, id, 0).unwrap_or_else(|e| panic!("{id}: load failed: {e}"));
        let imported = &def.config;

        // The authored-file channel: `--dump-config` writes this exact
        // TOML, the operator edits it, `--vehicle-config` loads it back.
        // Every handling number must survive the round trip.
        let path = tmp.path().join(format!("{id}.toml"));
        std::fs::write(&path, imported.to_toml()).unwrap();
        let reloaded = VehicleConfig::load(&path)
            .unwrap_or_else(|e| panic!("{id}: dumped config reload failed: {e}"));
        let out = apply(imported, reloaded, id, "round-trip");
        assert_eq!(
            out.to_toml(),
            imported.to_toml(),
            "{id}: the TOML round trip must not change the handling"
        );

        // Engine leg (the sweep's `eng` column): both anchors halved
        // together so the cap reaches the whole rev band.
        let mut engine = imported.clone();
        engine.engine.peak_torque_nm *= 0.5;
        engine.engine.max_power_w = engine.engine.max_power_w.map(|w| w * 0.5);
        let out = apply(imported, engine, id, "engine");
        assert_eq!(
            out.engine.peak_torque_nm,
            imported.engine.peak_torque_nm * 0.5,
            "{id}: peak torque"
        );
        assert_eq!(
            out.engine.max_power_w,
            imported.engine.max_power_w.map(|w| w * 0.5),
            "{id}: max power"
        );
        assert_eq!(
            out.mass, imported.mass,
            "{id}: engine leg must not touch mass"
        );

        // Mass leg (AC04 names power *or* mass — check both).
        let mut mass = imported.clone();
        mass.mass *= 2.0;
        let out = apply(imported, mass, id, "mass");
        assert_eq!(out.mass, imported.mass * 2.0, "{id}: mass");

        // Grip leg (the sweep's `grip` column): the global value plus
        // every wheel's own entry — a wheel that authors its own tires
        // config keeps it otherwise.
        let mut grip = imported.clone();
        grip.tires.longitudinal_grip *= 0.5;
        for w in &mut grip.wheels {
            if let Some(t) = &mut w.tires {
                t.longitudinal_grip *= 0.5;
            }
        }
        let out = apply(imported, grip, id, "grip");
        assert_eq!(
            out.tires.longitudinal_grip,
            imported.tires.longitudinal_grip * 0.5,
            "{id}: global longitudinal grip"
        );
        for (i, (w, src)) in out.wheels.iter().zip(&imported.wheels).enumerate() {
            match (w.tires, src.tires) {
                (Some(t), Some(s)) => assert_eq!(
                    t.longitudinal_grip,
                    s.longitudinal_grip * 0.5,
                    "{id}: w{i} longitudinal grip"
                ),
                (None, None) => {}
                _ => panic!("{id}: w{i}: the override invented or dropped a tires config"),
            }
        }

        // A wheel-count change needs a matching model — rejected for
        // every car, not silently misplacing wheels.
        let mut bad = imported.clone();
        bad.wheels.pop();
        let err = match apply_handling_override(imported, bad) {
            Ok(_) => panic!("{id}: a wheel-count mismatch was accepted"),
            Err(e) => e,
        };
        assert!(err.contains("wheels"), "{id}: {err}");

        println!(
            "{id}: wheels={} mass={:.0} torque {:.0}→{:.0} grip {:.2}→{:.2} ok",
            imported.wheels.len(),
            imported.mass,
            imported.engine.peak_torque_nm,
            imported.engine.peak_torque_nm * 0.5,
            imported.tires.longitudinal_grip,
            imported.tires.longitudinal_grip * 0.5,
        );
    }
}

/// Retail (F02-AC03/AC04): a paint variant is a shader set — it must
/// not change the car's handling, so the override channel's baseline
/// is identical across every declared paint of every ready car.
#[test]
fn every_declared_paint_carries_the_same_handling() {
    let Some(vfs) = retail_vfs() else {
        eprintln!("skipped: MM2_RETAIL is not set; retail paint matrix NOT run");
        return;
    };
    let catalog = VehicleCatalog::scan(&vfs);
    let mut checked = 0usize;
    for entry in catalog.entries.iter().filter(|e| e.is_ready()) {
        let base = load_vehicle(&vfs, &entry.id, 0)
            .unwrap_or_else(|e| panic!("{}: paint 0 load failed: {e}", entry.id));
        for paint in 0..base.paints.len() {
            let def = load_vehicle(&vfs, &entry.id, paint)
                .unwrap_or_else(|e| panic!("{}: paint {paint} load failed: {e}", entry.id));
            assert_eq!(
                def.config.to_toml(),
                base.config.to_toml(),
                "{}: paint {paint} changed the handling",
                entry.id
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no paints were checked");
    println!("paint matrix: {checked} declared paints, handling identical to paint 0 on each");
}
