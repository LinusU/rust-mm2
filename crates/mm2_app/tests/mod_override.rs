//! F29-AC01/AC02/AC05: one synthetic mod replaces a car texture, a
//! vehicle's handling, a world prop and an audio cue, each observed through
//! the production consumer that reads it — `MaterialCache` (the path car
//! bodies and props take), `load_vehicle`, `load_city`'s prop stamping and
//! `WaveBank` — and the gameplay fingerprint classifies the edits.
//!
//! Everything is self-authored synthetic data; no original install is read.
//! This is *synthetic* evidence for the four consumers named: it says
//! nothing about the other families (menu art, race rules, surface tables,
//! localization …), which stay open in `docs/ralph/PLAN.md`.

use std::path::Path;

use bevy::asset::Assets;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::{Mesh, VertexAttributeValues};
use bevy::pbr::StandardMaterial;
use mm2_app::audio::{PcmAudio, WaveBank};
use mm2_app::city::{MaterialCache, load_city};
use mm2_assets::Vfs;
use mm2_content::{fingerprint, load_vehicle};
use mm2_game::SessionEntity;

use crate::import_pipeline::{synthetic_inst, synthetic_pkg, synthetic_pkg_scaled, synthetic_psdl};
use crate::support::{pcm_wav, tuned_car, write};

const BASE_TEXTURE: &[u8] = include_bytes!("../../../assets/texture/dev_road.png");
const MOD_TEXTURE: &[u8] =
    include_bytes!("../../../examples/mods/checker-override/texture/dev_road.png");

/// The base install: two cars (`vpt` is the one a mod retunes, `vpu` the
/// bystander), a one-prop city, two cue waves and the two textures.
fn base_install(d: &Path) {
    tuned_car(d, "vpt", 1200.0);
    tuned_car(d, "vpu", 1500.0);
    write(d, "texture/vpt_skin.png", BASE_TEXTURE);
    write(d, "texture/vpu_skin.png", BASE_TEXTURE);
    write(d, "texture/test_road.png", BASE_TEXTURE);
    write(d, "city/test.psdl", synthetic_psdl());
    write(d, "city/test.inst", synthetic_inst());
    write(d, "geometry/testprop.pkg", synthetic_pkg());
    write(
        d,
        "aud/aud22/surfaces/roadskid1.22k.wav",
        pcm_wav(22050, 220),
    );
    write(
        d,
        "aud/aud22/surfaces/grassskid.11k.wav",
        pcm_wav(11025, 220),
    );
}

fn manifest(d: &Path, id: &str) {
    write(d, "mod.toml", format!("[mod]\nid = \"{id}\"\n"));
}

/// One mod per consumer, so each replacement can be mounted alone.
fn texture_mod(d: &Path) {
    manifest(d, "skin");
    write(d, "texture/vpt_skin.png", MOD_TEXTURE);
}

fn handling_mod(d: &Path) {
    manifest(d, "tune");
    write(
        d,
        "tune/vehicle/vpt.vehcarsim",
        crate::support::vehcarsim(2400.0),
    );
}

fn prop_mod(d: &Path) {
    manifest(d, "prop");
    write(d, "geometry/testprop.pkg", synthetic_pkg_scaled(2.0));
}

fn audio_mod(d: &Path) {
    manifest(d, "cue");
    write(
        d,
        "aud/aud22/surfaces/roadskid1.22k.wav",
        pcm_wav(32000, 440),
    );
}

/// What each consumer produced for one mount set.
#[derive(Debug, PartialEq)]
struct Observed {
    /// Decoded pixels behind the `vpt` and `vpu` skin materials.
    vpt_skin: Vec<u8>,
    vpu_skin: Vec<u8>,
    /// Converted handling mass of the two cars.
    vpt_mass: f32,
    vpu_mass: f32,
    /// Every city mesh vertex, sorted — world geometry plus the prop.
    city_vertices: Vec<[u32; 3]>,
    /// `(frames, sample rate)` of the two cue waves.
    skid1: (usize, u32),
    grass: (usize, u32),
    /// The gameplay fingerprint hash.
    fingerprint: u64,
}

fn skin_pixels(vfs: &Vfs, stem: &str) -> Vec<u8> {
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let handle = MaterialCache::new(vfs, &mut images, &mut materials).get(stem);
    let tex = materials
        .get(&handle)
        .and_then(|m| m.base_color_texture.clone())
        .unwrap_or_else(|| panic!("{stem} resolved to the untextured fallback"));
    images
        .get(&tex)
        .and_then(|i| i.data.clone())
        .expect("the skin decoded to pixels")
}

fn city_vertices(vfs: &Vfs) -> Vec<[u32; 3]> {
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        let loaded = load_city(
            &mut commands,
            vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("city loads");
        assert_eq!(loaded.report.props_spawned, 1, "the prop stamped");
        assert_eq!(loaded.report.props_failed, 0);
    }
    queue.apply(&mut world);
    let mut out = Vec::new();
    for (_, mesh) in meshes.iter() {
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        out.extend(pos.iter().map(|p| p.map(f32::to_bits)));
    }
    out.sort_unstable();
    out
}

fn cue(vfs: &Vfs, stem: &str) -> (usize, u32) {
    let mut bank = WaveBank::index(vfs);
    let mut waves: Assets<PcmAudio> = Assets::default();
    let handle = bank.load(vfs, &mut waves, stem).expect("cue resolves");
    let audio = waves.get(&handle).expect("cue decoded");
    (
        audio.samples.len() / usize::from(audio.channels.get()),
        audio.sample_rate.get(),
    )
}

fn observe(base: &Path, mods: &[&Path]) -> Observed {
    let mut vfs = Vfs::new();
    vfs.mount_dir(base, 0).unwrap();
    for (i, m) in mods.iter().enumerate() {
        vfs.mount_mod(m, 300 + i as i32).unwrap();
    }
    Observed {
        vpt_skin: skin_pixels(&vfs, "vpt_skin"),
        vpu_skin: skin_pixels(&vfs, "vpu_skin"),
        vpt_mass: load_vehicle(&vfs, "vpt", 0).unwrap().config.mass,
        vpu_mass: load_vehicle(&vfs, "vpu", 0).unwrap().config.mass,
        city_vertices: city_vertices(&vfs),
        skid1: cue(&vfs, "roadskid1"),
        grass: cue(&vfs, "grassskid"),
        fingerprint: fingerprint::gameplay(&vfs).unwrap().hash,
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: std::path::PathBuf,
    texture: std::path::PathBuf,
    handling: std::path::PathBuf,
    prop: std::path::PathBuf,
    audio: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = |n: &str| {
        let p = root.join(n);
        std::fs::create_dir_all(&p).unwrap();
        p
    };
    let f = Fixture {
        base: dir("base"),
        texture: dir("skin"),
        handling: dir("tune"),
        prop: dir("prop"),
        audio: dir("cue"),
        _tmp: tmp,
    };
    base_install(&f.base);
    texture_mod(&f.texture);
    handling_mod(&f.handling);
    prop_mod(&f.prop);
    audio_mod(&f.audio);
    f
}

/// AC01 + AC02: each mod changes exactly its own consumer, all four
/// together change all four, and unmounting restores the base.
#[test]
fn a_mod_replaces_texture_handling_prop_and_cue_through_their_consumers() {
    let f = fixture();
    let base = observe(&f.base, &[]);

    // Texture: only the retextured car's skin, not its sibling's.
    let skin = observe(&f.base, &[&f.texture]);
    assert_ne!(skin.vpt_skin, base.vpt_skin);
    assert_eq!(skin.vpt_skin, {
        let mut vfs = Vfs::new();
        vfs.mount_dir(&f.texture, 0).unwrap();
        skin_pixels(&vfs, "vpt_skin")
    });
    assert_eq!(skin.vpu_skin, base.vpu_skin);
    assert_eq!(
        Observed {
            vpt_skin: base.vpt_skin.clone(),
            ..skin
        },
        base,
        "a texture mod moves nothing but the skin"
    );

    // Handling: the retuned car only.
    let tune = observe(&f.base, &[&f.handling]);
    assert!(tune.vpt_mass > base.vpt_mass, "{tune:?} vs {base:?}");
    assert_eq!(tune.vpu_mass, base.vpu_mass);
    assert_eq!(tune.vpt_skin, base.vpt_skin);
    assert_eq!(tune.city_vertices, base.city_vertices);
    assert_eq!((tune.skid1, tune.grass), (base.skid1, base.grass));

    // Prop: the city's vertex set changes, the world around it keeps its
    // own vertices (the intersection with the base is not empty).
    let prop = observe(&f.base, &[&f.prop]);
    assert_ne!(prop.city_vertices, base.city_vertices);
    assert!(
        prop.city_vertices
            .iter()
            .any(|v| base.city_vertices.contains(v)),
        "the road/room meshes are untouched by a prop replacement"
    );
    assert_eq!(
        (prop.vpt_mass, prop.vpu_mass),
        (base.vpt_mass, base.vpu_mass)
    );
    assert_eq!(prop.vpt_skin, base.vpt_skin);
    assert_eq!((prop.skid1, prop.grass), (base.skid1, base.grass));

    // Audio cue: the replaced wave only.
    let audio = observe(&f.base, &[&f.audio]);
    assert_eq!(base.skid1, (220, 22050));
    assert_eq!(audio.skid1, (440, 32000));
    assert_eq!(audio.grass, base.grass);
    assert_eq!(audio.city_vertices, base.city_vertices);
    assert_eq!(audio.vpt_mass, base.vpt_mass);

    // All four at once: every consumer sees its replacement, and the
    // bystanders (`vpu`'s tune and skin, the grass cue) are still base.
    let all = observe(&f.base, &[&f.texture, &f.handling, &f.prop, &f.audio]);
    assert_eq!(all.vpt_skin, skin.vpt_skin);
    assert_eq!(all.vpt_mass, tune.vpt_mass);
    assert_eq!(all.city_vertices, prop.city_vertices);
    assert_eq!(all.skid1, audio.skid1);
    assert_eq!(all.vpu_skin, base.vpu_skin);
    assert_eq!(all.vpu_mass, base.vpu_mass);
    assert_eq!(all.grass, base.grass);

    // Removing the mods (a fresh mount of the base alone) restores base
    // behaviour exactly, fingerprint included.
    assert_eq!(observe(&f.base, &[]), base);
}

/// AC05: the gameplay fingerprint moves for the physics and geometry
/// edits and stays put for the texture and audio replacements.
#[test]
fn the_gameplay_fingerprint_separates_physics_edits_from_cosmetic_ones() {
    let f = fixture();
    let base = observe(&f.base, &[]).fingerprint;
    let skin = observe(&f.base, &[&f.texture]).fingerprint;
    let cue = observe(&f.base, &[&f.audio]).fingerprint;
    let tune = observe(&f.base, &[&f.handling]).fingerprint;
    let prop = observe(&f.base, &[&f.prop]).fingerprint;
    assert_eq!(skin, base, "a texture swap is cosmetic");
    assert_eq!(cue, base, "an audio cue swap is cosmetic");
    assert_ne!(tune, base, "a tuning edit is gameplay");
    assert_ne!(prop, base, "a prop mesh edit is gameplay (collision hull)");
    assert_ne!(tune, prop);
}

/// Mount order: when two mods replace the same file the later one wins,
/// and swapping the order swaps the winner — the documented last-wins
/// precedence, observed through the handling consumer.
#[test]
fn the_later_of_two_conflicting_mods_wins() {
    let f = fixture();
    let heavier = f._tmp.path().join("heavier");
    std::fs::create_dir_all(&heavier).unwrap();
    manifest(&heavier, "heavier");
    write(
        &heavier,
        "tune/vehicle/vpt.vehcarsim",
        crate::support::vehcarsim(3600.0),
    );
    let a = observe(&f.base, &[&f.handling, &heavier]);
    let b = observe(&f.base, &[&heavier, &f.handling]);
    assert!(a.vpt_mass > b.vpt_mass, "{} vs {}", a.vpt_mass, b.vpt_mass);
    assert_eq!(
        a.vpt_mass,
        observe(&f.base, &[&heavier]).vpt_mass,
        "the later mount is the whole effect"
    );
}
