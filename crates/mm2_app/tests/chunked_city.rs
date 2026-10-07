//! F29-AC04/AC06: the custom-city `.chunks` manifest through the real
//! `load_city` — a valid two-part city loads, and every way a manifest can
//! point somewhere it must not (another manifest, a PVS file, a missing or
//! corrupt part, the same part twice under another case, the primary file
//! itself) fails the load with a typed error instead of loading a partial
//! or doubled city.
//!
//! Self-authored synthetic data only; no original install is read.

use std::path::Path;

use bevy::asset::Assets;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::Mesh;
use bevy::pbr::StandardMaterial;
use mm2_app::city::{LoadCityError, LoadedCity, load_city};
use mm2_assets::Vfs;
use mm2_game::SessionEntity;

use crate::import_pipeline::{synthetic_inst, synthetic_psdl};
use crate::support::write;

/// A city whose manifest lists `city/test.parts/east.psdl`.
fn chunked(d: &Path, manifest_body: &str) {
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.inst", synthetic_inst());
    write(d, "city/test.parts/east.psdl", synthetic_psdl());
    write(
        d,
        "city/test.chunks",
        format!("MM2_CHUNKS 1\n{manifest_body}\n"),
    );
}

fn load(d: &Path) -> Result<LoadedCity, LoadCityError> {
    let mut vfs = Vfs::new();
    vfs.mount_dir(d, 0).unwrap();
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let mut commands = Commands::new(&mut queue, &world);
    let mut session = mm2_game::Session::new();
    let loaded = load_city(
        &mut commands,
        &vfs,
        "city/test.psdl",
        &mut meshes,
        &mut images,
        &mut materials,
        SessionEntity(1),
        &mut session,
    );
    queue.apply(&mut world);
    // A refused city queued nothing: the load validates before it spawns.
    if loaded.is_err() {
        assert!(meshes.is_empty(), "a refused chunked city built meshes");
    }
    loaded
}

fn refused(d: &Path) -> String {
    match load(d) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("the chunked city loaded"),
    }
}

#[test]
fn a_valid_manifest_loads_every_part_into_one_city() {
    let single = tempfile::tempdir().unwrap();
    write(single.path(), "city/test.psdl", synthetic_psdl());
    write(single.path(), "city/test.inst", synthetic_inst());
    let one = load(single.path()).expect("the single-part city loads");

    let tmp = tempfile::tempdir().unwrap();
    chunked(tmp.path(), "city/test.parts/east.psdl");
    let two = load(tmp.path()).expect("the two-part city loads");
    assert_eq!(
        two.report.rooms,
        one.report.rooms * 2,
        "both parts' rooms are in the one city"
    );
}

#[test]
fn a_part_that_is_itself_a_manifest_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    chunked(tmp.path(), "city/test.parts/east.psdl");
    // east.chunks lists the primary file back: a cycle if it were followed.
    write(
        tmp.path(),
        "city/test.parts/east.chunks",
        "MM2_CHUNKS 1\ncity/test.psdl\n",
    );
    let err = refused(tmp.path());
    assert!(err.contains("nested manifests"), "{err}");
    assert!(err.contains("city/test.parts/east.psdl"), "{err}");
}

#[test]
fn a_part_with_visibility_data_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    chunked(tmp.path(), "city/test.parts/east.psdl");
    write(tmp.path(), "city/test.parts/east.cpvs", b"CPVS");
    assert!(refused(tmp.path()).contains("PVS"));

    // The primary file's own PVS is just as unsupported once chunked.
    let tmp = tempfile::tempdir().unwrap();
    chunked(tmp.path(), "city/test.parts/east.psdl");
    write(tmp.path(), "city/test.cpvs", b"CPVS");
    assert!(refused(tmp.path()).contains("PVS"));
}

#[test]
fn a_missing_or_corrupt_part_fails_the_whole_city() {
    let tmp = tempfile::tempdir().unwrap();
    chunked(
        tmp.path(),
        "city/test.parts/east.psdl\ncity/test.parts/west.psdl",
    );
    let err = refused(tmp.path());
    assert!(err.contains("west.psdl"), "{err}");
    assert!(err.contains("missing"), "{err}");

    write(tmp.path(), "city/test.parts/west.psdl", b"PSD0 truncated");
    let err = refused(tmp.path());
    assert!(err.contains("west.psdl"), "{err}");
    assert!(err.contains("malformed"), "{err}");
}

#[test]
fn a_part_listed_twice_or_as_the_primary_under_another_case_is_refused() {
    for body in [
        "city/test.parts/east.psdl\ncity/test.parts/EAST.psdl",
        "city/TEST.psdl",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        chunked(tmp.path(), body);
        let err = refused(tmp.path());
        assert!(err.contains("invalid or duplicate city chunk"), "{err}");
    }
}
