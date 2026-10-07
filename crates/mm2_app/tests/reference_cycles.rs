//! F29-AC06: a model that references another model — `xref` chunks naming
//! the PKG itself, or two PKGs naming each other — loads through the real
//! `load_city` prop path without recursion. The importer keeps an `xref`
//! name for diagnostics (`VehicleModel::xrefs`) and never opens the file it
//! names, so a reference cycle cannot expand; this pins that so a future
//! importer that follows xrefs has to bring its own cycle check and fail
//! these tests first.
//!
//! Self-authored synthetic data only; no original install is read.

use std::path::Path;

use bevy::asset::Assets;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::Mesh;
use bevy::pbr::StandardMaterial;
use mm2_app::city::{CityReport, load_city};
use mm2_assets::Vfs;
use mm2_content::build_model;
use mm2_formats::pkg::Pkg;
use mm2_game::SessionEntity;

use crate::import_pipeline::{synthetic_inst, synthetic_pkg, synthetic_psdl};
use crate::support::write;

/// [`synthetic_pkg`] followed by an `xref` chunk naming `targets`.
fn pkg_referencing(targets: &[&str]) -> Vec<u8> {
    let mut payload = (targets.len() as u32).to_le_bytes().to_vec();
    for target in targets {
        for f in [1.0f32, 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.] {
            payload.extend_from_slice(&f.to_le_bytes());
        }
        let mut name = [0u8; 32];
        name[..target.len()].copy_from_slice(target.as_bytes());
        payload.extend_from_slice(&name);
    }
    let mut d = synthetic_pkg();
    d.extend_from_slice(b"FILE");
    d.push(5);
    d.extend_from_slice(b"xref\0");
    d.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    d.extend_from_slice(&payload);
    d
}

fn load(d: &Path) -> CityReport {
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
    )
    .expect("the city loads");
    queue.apply(&mut world);
    loaded.report
}

fn city_with_prop(d: &Path, pkg: Vec<u8>) {
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.inst", synthetic_inst());
    write(d, "geometry/testprop.pkg", pkg);
}

#[test]
fn a_prop_that_references_itself_loads_once() {
    let tmp = tempfile::tempdir().unwrap();
    city_with_prop(tmp.path(), pkg_referencing(&["testprop"]));
    let report = load(tmp.path());
    assert_eq!(report.props_spawned, 1, "the self-referencing prop stamped");
    assert_eq!(report.props_failed, 0);
}

#[test]
fn two_props_that_reference_each_other_load_without_following_either() {
    let tmp = tempfile::tempdir().unwrap();
    city_with_prop(tmp.path(), pkg_referencing(&["other"]));
    write(
        tmp.path(),
        "geometry/other.pkg",
        pkg_referencing(&["testprop"]),
    );
    let report = load(tmp.path());
    assert_eq!(report.props_spawned, 1);
    assert_eq!(report.props_failed, 0);
}

/// The reference is kept as data and nothing resolves it: a name with no
/// file behind it neither fails the prop nor is searched for.
#[test]
fn a_reference_to_a_missing_model_is_kept_not_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    city_with_prop(tmp.path(), pkg_referencing(&["nowhere", "testprop"]));
    let report = load(tmp.path());
    assert_eq!(report.props_spawned, 1);
    assert_eq!(report.props_failed, 0);

    let pkg = Pkg::parse(&pkg_referencing(&["nowhere", "testprop"])).expect("the PKG parses");
    let model = build_model(&pkg, |_| None);
    assert_eq!(model.xrefs, ["nowhere", "testprop"]);
}
