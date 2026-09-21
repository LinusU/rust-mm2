//! F16-A profile store tests: create/load/save/delete, atomic-write
//! recovery, isolation between profiles and the documented original
//! rules the store enforces (DRV-7 last driver, DRV-2 rank).

use std::path::PathBuf;

use mm2_game::*;

fn store() -> (tempfile::TempDir, ProfileStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path().join("profiles")).unwrap();
    (dir, store)
}

fn event_key() -> EventKey {
    EventKey {
        city: "sf".to_string(),
        table: EventTableKind::Checkpoint,
        stem: "race3".to_string(),
    }
}

#[test]
fn create_save_load_round_trip() {
    let (_dir, store) = store();
    let mut profile = store
        .create("DriverX", Difficulty::Professional, ProfileKind::Standard)
        .unwrap();
    assert_eq!(profile.id.as_str(), "driver-0");
    assert_eq!(profile.version, PROFILE_SCHEMA_VERSION);

    profile.event_mut(event_key()).record_finish(14_400);
    profile.event_mut(event_key()).record_finish(13_200);
    profile.progress.unlocks.insert("vehicle:vpbus".to_string());
    profile.selections.vehicle = Some(VehicleChoice {
        id: "vpbug".to_string(),
        paint: 2,
    });
    profile.selections.last_event = Some(event_key());
    store.save(&mut profile).unwrap();

    let loaded = store.load(&profile.id).unwrap();
    assert!(!loaded.recovered_from_backup);
    let loaded = loaded.profile;
    assert_eq!(loaded.id, profile.id);
    assert_eq!(loaded.name, "DriverX");
    assert_eq!(loaded.rank, Difficulty::Professional);
    assert_eq!(loaded.kind, ProfileKind::Standard);
    let record = loaded.event(&event_key()).unwrap();
    assert_eq!(record.finishes, 2);
    assert_eq!(record.best_race_ticks, Some(13_200));
    assert!(loaded.progress.unlocks.contains("vehicle:vpbus"));
    assert_eq!(
        loaded.selections.vehicle,
        Some(VehicleChoice {
            id: "vpbug".to_string(),
            paint: 2
        })
    );
    assert_eq!(loaded.selections.last_event, Some(event_key()));
}

#[test]
fn unknown_fields_survive_a_round_trip() {
    let (_dir, store) = store();
    let profile = store
        .create("Future", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // Simulate a file written by a newer schema-1 build with fields this
    // build does not model.
    let path = store.root().join("driver-0.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    doc["future_field"] = serde_json::json!({"nested": [1, 2, 3]});
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

    let mut loaded = store.load(&profile.id).unwrap().profile;
    assert_eq!(
        loaded.extra["future_field"],
        serde_json::json!({"nested": [1, 2, 3]})
    );
    store.save(&mut loaded).unwrap();
    let rewritten = std::fs::read_to_string(&path).unwrap();
    assert!(rewritten.contains("future_field"));
    assert!(rewritten.contains("nested"));
}

#[test]
fn profiles_are_isolated() {
    let (_dir, store) = store();
    let mut a = store
        .create("Alice", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("Bob", Difficulty::Professional, ProfileKind::Standard)
        .unwrap();
    assert_ne!(a.id, b.id);

    a.event_mut(event_key()).record_finish(9_000);
    a.progress.unlocks.insert("vehicle:vpcoop".to_string());
    store.save(&mut a).unwrap();

    let b_loaded = store.load(&b.id).unwrap().profile;
    assert!(b_loaded.progress.events.is_empty());
    assert!(b_loaded.progress.unlocks.is_empty());
    assert_eq!(b_loaded.name, "Bob");

    let summaries = store.list().unwrap();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].meta.as_ref().unwrap().name, "Alice");
    assert_eq!(summaries[1].meta.as_ref().unwrap().name, "Bob");
}

#[test]
fn duplicate_display_names_get_distinct_ids() {
    let (_dir, store) = store();
    let a = store
        .create("DriverX", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("DriverX", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert_eq!(a.name, b.name);
    assert_ne!(a.id, b.id);
}

#[test]
fn invalid_names_are_rejected() {
    let (_dir, store) = store();
    for bad in ["", "   ", &"x".repeat(MAX_NAME_CHARS + 1), "bad\nname"] {
        assert!(
            matches!(
                store.create(bad, Difficulty::Amateur, ProfileKind::Standard),
                Err(ProfileError::Invalid(_))
            ),
            "name {bad:?} should be rejected"
        );
    }
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn deleted_ids_are_never_reused() {
    let (_dir, store) = store();
    let a = store
        .create("A", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("B", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.delete(&a.id).unwrap();
    let c = store
        .create("C", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert_eq!(c.id.as_str(), "driver-2");
    assert!(store.load(&b.id).is_ok());
}

#[test]
fn corrupt_main_recovers_from_backup() {
    let (_dir, store) = store();
    let mut profile = store
        .create("Recover", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    profile.name = "RecoverV2".to_string();
    store.save(&mut profile).unwrap();
    // Now driver-0.json.bak holds v1, driver-0.json holds v2.
    let main = store.root().join("driver-0.json");
    std::fs::write(&main, b"{\"version\":1,\"id\":\"driver-0\",\"na").unwrap();

    let loaded = store.load(&profile.id).unwrap();
    assert!(loaded.recovered_from_backup);
    assert_eq!(loaded.profile.name, "Recover");
    // The corrupt main is still on disk — nothing destroyed it.
    assert!(main.exists());
    assert!(store.root().join("driver-0.json.bak").exists());
}

#[test]
fn interrupted_write_recovers_the_backup() {
    let (_dir, store) = store();
    let mut profile = store
        .create("Crash", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.save(&mut profile).unwrap();
    // Simulate a crash between the main→bak rotation and the tmp→main
    // rename: main gone, a stale tmp left behind, bak intact.
    let root = store.root();
    std::fs::remove_file(root.join("driver-0.json")).unwrap();
    std::fs::write(root.join("driver-0.json.tmp"), b"partial").unwrap();

    let loaded = store.load(&profile.id).unwrap();
    assert!(loaded.recovered_from_backup);
    assert_eq!(loaded.profile.name, "Crash");
    // The orphaned backup still lists and still owns its id — a new
    // profile must not inherit driver-0's surviving data.
    let summaries = store.list().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].meta.as_ref().unwrap().name, "Crash");
    assert!(summaries[0].error.is_some());
    let next = store
        .create("New", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert_eq!(next.id.as_str(), "driver-1");
    assert_eq!(store.load(&profile.id).unwrap().profile.name, "Crash");
}

#[test]
fn a_complete_tmp_is_newer_than_the_main_it_never_replaced() {
    let (_dir, store) = store();
    let mut profile = store
        .create("V1", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.save(&mut profile).unwrap();
    // Simulate a crash after the tmp write was flushed: the complete
    // newer document sits in tmp while main still holds the previous
    // revision.
    profile.name = "V2".to_string();
    profile.revision += 1;
    std::fs::write(
        store.root().join("driver-0.json.tmp"),
        serde_json::to_vec_pretty(&profile).unwrap(),
    )
    .unwrap();

    let loaded = store.load(&profile.id).unwrap();
    assert!(loaded.recovered_from_backup);
    assert_eq!(loaded.profile.name, "V2");
    assert_eq!(loaded.profile.revision, profile.revision);
}

#[test]
fn an_orphaned_backup_still_owns_its_id() {
    let (_dir, store) = store();
    let mut a = store
        .create("A", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let _b = store
        .create("B", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.save(&mut a).unwrap(); // driver-0 now has a main and a .bak
    // A delete or save interrupted after the main file vanished leaves
    // only the .bak — the id is still owned and the data recoverable.
    std::fs::remove_file(store.root().join("driver-0.json")).unwrap();

    let summaries = store.list().unwrap();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id.as_str(), "driver-0");
    assert_eq!(summaries[0].meta.as_ref().unwrap().name, "A");
    assert!(summaries[0].error.is_some());

    let loaded = store.load(&a.id).unwrap();
    assert!(loaded.recovered_from_backup);
    assert_eq!(loaded.profile.name, "A");

    // The orphan's id is not reallocated...
    let c = store
        .create("C", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert_eq!(c.id.as_str(), "driver-2");
    // ...and the orphan can still be deleted deliberately.
    store.delete(&a.id).unwrap();
    assert!(!store.root().join("driver-0.json.bak").exists());
}

#[test]
fn malformed_ids_are_rejected_before_any_path_probe() {
    let (_dir, store) = store();
    store
        .create("Real", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let escape = ProfileId::from("../escape".to_string());
    assert!(matches!(store.load(&escape), Err(ProfileError::Invalid(_))));
    assert!(matches!(
        store.set_active(&escape),
        Err(ProfileError::Invalid(_))
    ));
    assert!(matches!(
        store.delete(&escape),
        Err(ProfileError::Invalid(_))
    ));
    // Marker content is data, not caller input — a garbage marker
    // reads as no selection rather than an error.
    std::fs::write(store.root().join("active"), "../escape\n").unwrap();
    assert_eq!(store.active().unwrap(), None);
}

#[test]
fn unsorted_or_duplicate_event_records_are_corrupt() {
    let (_dir, store) = store();
    let profile = store
        .create("Sloppy", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // Hand-edit the file so `events` is out of key order — the own
    // writers never produce this, and `event_mut` relies on the order.
    let path = store.root().join("driver-0.json");
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["progress"]["events"] = serde_json::json!([
        {"key": {"city": "sf", "table": "checkpoint", "stem": "race9"},
         "finishes": 1, "best_race_ticks": null},
        {"key": {"city": "sf", "table": "checkpoint", "stem": "race3"},
         "finishes": 1, "best_race_ticks": null},
    ]);
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

    match store.load(&profile.id) {
        Err(ProfileError::Corrupt { main, .. }) => {
            assert!(main.unwrap().contains("unsorted"));
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn fully_corrupt_profile_reports_and_preserves_files() {
    let (_dir, store) = store();
    let profile = store
        .create("Gone", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let root = store.root();
    std::fs::write(root.join("driver-0.json"), b"not json").unwrap();
    std::fs::write(root.join("driver-0.json.bak"), b"also not json").unwrap();

    match store.load(&profile.id) {
        Err(ProfileError::Corrupt {
            id, main, backup, ..
        }) => {
            assert_eq!(id, profile.id);
            assert!(main.unwrap().contains("invalid JSON"));
            assert!(backup.unwrap().contains("invalid JSON"));
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
    // Both files remain for manual recovery.
    assert!(root.join("driver-0.json").exists());
    assert!(root.join("driver-0.json.bak").exists());
    // And the profile still lists — corrupt does not mean invisible.
    let summaries = store.list().unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].meta.is_none());
    assert!(summaries[0].error.is_some());
}

#[test]
fn unsupported_schema_version_is_rejected() {
    let (_dir, store) = store();
    let profile = store
        .create("Old", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let path = store.root().join("driver-0.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("\"version\": 1", "\"version\": 99")).unwrap();

    match store.load(&profile.id) {
        Err(ProfileError::Corrupt { main, .. }) => {
            assert!(main.unwrap().contains("unsupported schema version 99"));
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn file_naming_a_different_id_is_corrupt() {
    let (_dir, store) = store();
    let mut a = store
        .create("A", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let _b = store
        .create("B", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    // Overwrite driver-1.json with driver-0's contents.
    let root = store.root();
    let bytes = std::fs::read(root.join("driver-0.json")).unwrap();
    std::fs::write(root.join("driver-1.json"), bytes).unwrap();

    match store.load(&ProfileId::from("driver-1".to_string())) {
        Err(ProfileError::Corrupt { main, .. }) => {
            assert!(main.unwrap().contains("different profile id"));
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
    a.name = "still fine".to_string();
    store.save(&mut a).unwrap();
}

#[test]
fn the_last_profile_cannot_be_deleted() {
    let (_dir, store) = store();
    let only = store
        .create("Last", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert!(matches!(
        store.delete(&only.id),
        Err(ProfileError::LastProfile(_))
    ));
    // DRV-7 — and the profile is still loadable.
    assert_eq!(store.load(&only.id).unwrap().profile.name, "Last");
}

#[test]
fn delete_removes_only_that_profile() {
    let (_dir, store) = store();
    let a = store
        .create("A", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("B", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    store.delete(&a.id).unwrap();

    assert!(matches!(
        store.load(&a.id),
        Err(ProfileError::UnknownProfile(_))
    ));
    assert!(matches!(
        store.delete(&a.id),
        Err(ProfileError::UnknownProfile(_))
    ));
    let root: PathBuf = store.root().to_path_buf();
    assert!(!root.join("driver-0.json").exists());
    assert!(!root.join("driver-0.json.bak").exists());
    assert_eq!(store.load(&b.id).unwrap().profile.name, "B");
}

#[test]
fn active_marker_tracks_the_selection() {
    let (_dir, store) = store();
    let a = store
        .create("A", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    let b = store
        .create("B", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert_eq!(store.active().unwrap(), None);

    store.set_active(&b.id).unwrap();
    assert_eq!(store.active().unwrap(), Some(b.id.clone()));

    // A marker naming a deleted profile reads as no selection.
    store.delete(&a.id).unwrap();
    store.set_active(&b.id).unwrap();
    std::fs::write(store.root().join("active"), "driver-9\n").unwrap();
    assert_eq!(store.active().unwrap(), None);
}

#[test]
fn listing_orders_ids_numerically() {
    let (_dir, store) = store();
    for i in 0..11 {
        store
            .create(format!("P{i}"), Difficulty::Amateur, ProfileKind::Standard)
            .unwrap();
    }
    let ids: Vec<String> = store
        .list()
        .unwrap()
        .iter()
        .map(|s| s.id.as_str().to_string())
        .collect();
    let want: Vec<String> = (0..11).map(|i| format!("driver-{i}")).collect();
    assert_eq!(ids, want);
}

#[test]
fn sandbox_profiles_do_not_record_progress() {
    let (_dir, store) = store();
    let sandbox = store
        .create("Dev", Difficulty::Amateur, ProfileKind::Sandbox)
        .unwrap();
    let standard = store
        .create("Real", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    assert!(!sandbox.records_progress());
    assert!(standard.records_progress());
}
