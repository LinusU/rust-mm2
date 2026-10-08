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
use mm2_app::city::{MaterialCache, load_city, trace_city_reads};
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

/// F29 req 5: the per-mod classification agrees with what the consumers
/// and the fingerprint observed — the texture and cue mods are
/// cosmetic-only (record-eligible, join-compatible), the tuning and prop
/// mods are gameplay — and the session verdict follows the worst mod.
#[test]
fn each_mod_is_classified_the_way_its_consumer_and_the_fingerprint_see_it() {
    let f = fixture();
    let base = observe(&f.base, &[]).fingerprint;
    for (m, gameplay) in [
        (&f.texture, false),
        (&f.audio, false),
        (&f.handling, true),
        (&f.prop, true),
    ] {
        let mut vfs = Vfs::new();
        vfs.mount_dir(&f.base, 0).unwrap();
        vfs.mount_mod(m, 300).unwrap();
        let reports = fingerprint::mod_reports(&vfs);
        assert_eq!(reports.len(), 1);
        assert_eq!(!reports[0].is_cosmetic_only(), gameplay, "{reports:?}");
        assert_eq!(fingerprint::mods_cosmetic_only(&vfs), !gameplay);
        assert_eq!(
            fingerprint::gameplay(&vfs).unwrap().hash != base,
            gameplay,
            "the classification and the fingerprint must agree: {reports:?}"
        );
    }
    let mut vfs = Vfs::new();
    vfs.mount_dir(&f.base, 0).unwrap();
    vfs.mount_mod(&f.texture, 300).unwrap();
    vfs.mount_mod(&f.audio, 301).unwrap();
    assert!(fingerprint::mods_cosmetic_only(&vfs), "two cosmetic mods");
    vfs.mount_mod(&f.handling, 302).unwrap();
    assert!(!fingerprint::mods_cosmetic_only(&vfs), "plus a tuning mod");
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

/// F29 req 4: the wave bank owns its stem index, so mounting a mod after the
/// index was built must not leave the old index answering for the new
/// layout. The stale bank refuses (naming both revisions) and a fresh index
/// of the same VFS sees the mod's cue — restart-based replacement is a
/// rebuild, never a silent mix.
#[test]
fn a_wave_index_built_before_a_remount_is_refused_not_answered_stale() {
    let f = fixture();
    let mut vfs = Vfs::new();
    vfs.mount_dir(&f.base, 0).unwrap();
    let mut bank = WaveBank::index(&vfs);
    let mut waves: Assets<PcmAudio> = Assets::default();
    let base_handle = bank
        .load(&vfs, &mut waves, "roadskid1")
        .expect("the index serves the mount set it was built from");
    let base_frames = waves.get(&base_handle).unwrap().samples.len();

    vfs.mount_mod(&f.audio, 300).unwrap();
    for result in [
        bank.load(&vfs, &mut waves, "roadskid1"),
        bank.load_siren(&vfs, &mut waves, "roadskid1"),
    ] {
        let err = result.expect_err("the stale index must not answer, even for a cached cue");
        assert!(err.contains("stale"), "{err}");
    }

    let mut fresh = WaveBank::index(&vfs);
    let handle = fresh.load(&vfs, &mut waves, "roadskid1").unwrap();
    assert_ne!(
        waves.get(&handle).unwrap().samples.len(),
        base_frames,
        "a rebuilt index sees the mod's cue"
    );
}

/// F29-AC03: the explanation of a conflict names the mod the production
/// consumer actually loaded, in either mount order, and lists the mod it
/// shadowed and the base file beneath both.
#[test]
fn a_conflict_explanation_names_the_mod_the_consumer_loaded() {
    let f = fixture();
    let heavier = f._tmp.path().join("heavier");
    std::fs::create_dir_all(&heavier).unwrap();
    manifest(&heavier, "heavier");
    let logical = "tune/vehicle/vpt.vehcarsim";
    write(&heavier, logical, crate::support::vehcarsim(3600.0));
    let base_mass = observe(&f.base, &[]).vpt_mass;
    let tune_mass = observe(&f.base, &[&f.handling]).vpt_mass;
    let heavier_mass = observe(&f.base, &[&heavier]).vpt_mass;
    assert!(base_mass != tune_mass && tune_mass != heavier_mass);

    let cases = [
        ([&f.handling, &heavier], ["heavier", "tune"], heavier_mass),
        ([&heavier, &f.handling], ["tune", "heavier"], tune_mass),
    ];
    for (order, ids, expected_mass) in cases {
        let mut vfs = Vfs::new();
        vfs.mount_dir(&f.base, 0).unwrap();
        for (i, m) in order.iter().enumerate() {
            vfs.mount_mod(m, 300 + i as i32).unwrap();
        }
        let ex = vfs.explain(logical).unwrap();
        let labels: Vec<_> = ex
            .candidates
            .iter()
            .map(|c| c.source.label.as_deref())
            .collect();
        assert_eq!(
            labels,
            [Some(ids[0]), Some(ids[1]), None],
            "winner, the shadowed mod, then the base file"
        );
        assert!(ex.is_mod_conflict());
        let loaded = load_vehicle(&vfs, "vpt", 0).unwrap().config.mass;
        assert_eq!(loaded, expected_mass, "the consumer loaded the winner");
    }
}

/// F29 req 4 (dependency diagnostics): tracing a production load names the
/// files it pulled in and the source that served each, so a mod author can
/// see that a replacement is live for exactly the consumer it targets — and
/// that a bystander never touched it.
#[test]
fn a_traced_load_names_the_mod_files_each_consumer_pulled_in() {
    let f = fixture();
    let mut vfs = Vfs::new();
    vfs.mount_dir(&f.base, 0).unwrap();
    for (i, m) in [&f.texture, &f.handling, &f.prop, &f.audio]
        .iter()
        .enumerate()
    {
        vfs.mount_mod(m, 300 + i as i32).unwrap();
    }

    let (_, vpt) = vfs.trace_reads(|| load_vehicle(&vfs, "vpt", 0).unwrap());
    assert_eq!(vpt.from_mod("tune"), ["tune/vehicle/vpt.vehcarsim"]);
    assert!(vpt.from_mod("prop").is_empty() && vpt.from_mod("cue").is_empty());
    // The base files the car also needs are credited to the install.
    assert!(
        vpt.by_origin().keys().any(|k| k.starts_with("original")),
        "{}",
        vpt.render()
    );

    // The bystander car reads the same loader's files but none of the mod's.
    let (_, vpu) = vfs.trace_reads(|| load_vehicle(&vfs, "vpu", 0).unwrap());
    assert!(vpu.from_mod("tune").is_empty(), "{}", vpu.render());
    assert!(
        vpu.accesses
            .iter()
            .any(|a| a.logical == "tune/vehicle/vpu.vehcarsim"),
        "vpu's own tune was read from the install"
    );

    let (_, skin) = vfs.trace_reads(|| skin_pixels(&vfs, "vpt_skin"));
    assert_eq!(skin.from_mod("skin"), ["texture/vpt_skin.png"]);
    let (_, bystander) = vfs.trace_reads(|| skin_pixels(&vfs, "vpu_skin"));
    assert!(bystander.from_mod("skin").is_empty());

    let (_, city) = vfs.trace_reads(|| city_vertices(&vfs));
    assert_eq!(city.from_mod("prop"), ["geometry/testprop.pkg"]);
    assert!(
        city.accesses.iter().any(|a| a.logical == "city/test.psdl"),
        "the city's own geometry is traced too"
    );

    let (_, wave) = vfs.trace_reads(|| cue(&vfs, "roadskid1"));
    assert_eq!(
        wave.from_mod("cue"),
        ["aud/aud22/surfaces/roadskid1.22k.wav"]
    );
    let (_, grass) = vfs.trace_reads(|| cue(&vfs, "grassskid"));
    assert!(grass.from_mod("cue").is_empty());
    assert!(!grass.accesses.is_empty());
}

/// The city leg of `mm2 --trace-deps`: the production loader runs in a
/// throwaway world, a mod that replaces the city's own geometry is credited
/// with that file, a mod aimed at another city is not, and a load that
/// fails still reports the mod file it choked on.
#[test]
fn a_traced_city_load_credits_the_mod_that_replaced_its_geometry() {
    let f = fixture();
    let root = f._tmp.path();

    let live = broken_mod(root, "reshape", "city/test.psdl", &synthetic_psdl());
    let elsewhere = broken_mod(root, "elsewhere", "city/other.psdl", &synthetic_psdl());
    let mut vfs = mounted(&f.base, &live);
    vfs.mount_mod(&elsewhere, 301).unwrap();
    let (loaded, trace) = trace_city_reads(&vfs, "city/test.psdl");
    let label = loaded.expect("the replaced city still loads");
    assert!(label.starts_with("city/test.psdl (1 room"), "{label}");
    assert_eq!(trace.from_mod("reshape"), ["city/test.psdl"]);
    assert!(trace.from_mod("elsewhere").is_empty(), "{}", trace.render());
    assert_eq!(
        trace.absent_mods(&["reshape".into(), "elsewhere".into()]),
        [&"elsewhere".to_string()]
    );
    assert!(
        trace
            .by_origin()
            .keys()
            .any(|k| k.starts_with("original") && k.contains("base")),
        "the prop and inst the mod left alone come from the install: {}",
        trace.render()
    );

    // The unmodded city credits no mod.
    let mut plain = Vfs::new();
    plain.mount_dir(&f.base, 0).unwrap();
    let (loaded, trace) = trace_city_reads(&plain, "city/test.psdl");
    assert!(loaded.is_ok());
    assert!(trace.from_mod("reshape").is_empty());

    // A mod whose geometry does not parse fails the load, and the trace
    // still names the file that was read from it.
    let bad = broken_mod(root, "brokencity", "city/test.psdl", b"not a psdl");
    let vfs = mounted(&f.base, &bad);
    let (loaded, trace) = trace_city_reads(&vfs, "city/test.psdl");
    assert!(loaded.is_err(), "garbage geometry must not load");
    assert_eq!(trace.from_mod("brokencity"), ["city/test.psdl"]);

    // A city no source provides is an error with a recorded miss.
    let (loaded, trace) = trace_city_reads(&plain, "city/nowhere.psdl");
    assert!(loaded.is_err());
    assert_eq!(trace.missing(), ["city/nowhere.psdl"]);
}

/// `mm2 --trace-deps` as a process: prints the per-source report without a
/// window, `--expect-mod` turns "is my mod live for this city" into an exit
/// status, and a city nothing provides fails with the miss listed.
#[test]
fn the_trace_deps_flag_reports_and_checks_which_mod_serves_a_city() {
    let f = fixture();
    let mods = f._tmp.path().join("deps_mods");
    broken_mod(&mods, "reshape", "city/test.psdl", &synthetic_psdl());
    let run = |city: &str, expect: Option<&str>| {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_mm2"));
        cmd.arg("--mm2-path")
            .arg(&f.base)
            .arg("--mods")
            .arg(&mods)
            .args(["--city", city, "--trace-deps"])
            .env_remove("RUST_LOG");
        if let Some(id) = expect {
            cmd.args(["--expect-mod", id]);
        }
        let out = cmd.output().expect("spawn mm2");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };

    let (code, out) = run("test", Some("reshape"));
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("mod `reshape`: 1 file(s)"), "{out}");
    assert!(out.contains("  city/test.psdl ("), "{out}");
    assert!(out.contains("city/test.psdl (1 room"), "{out}");

    let (code, out) = run("test", Some("not-mounted"));
    assert_eq!(code, Some(2), "a mod that served nothing fails\n{out}");

    let (code, out) = run("nowhere", None);
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("load failed after"), "{out}");
    assert!(out.contains("city/nowhere.psdl (read failed)"), "{out}");
}

/// A mod mounted over `base` that carries one broken file at `logical`.
fn broken_mod(root: &Path, id: &str, logical: &str, bytes: &[u8]) -> std::path::PathBuf {
    let d = root.join(id);
    std::fs::create_dir_all(&d).unwrap();
    manifest(&d, id);
    write(&d, logical, bytes);
    d
}

fn mounted(base: &Path, m: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(base, 0).unwrap();
    vfs.mount_mod(m, 300).unwrap();
    vfs
}

/// AC04: a selected override that does not decode is reported by its
/// consumer — it neither passes for the replacement nor quietly hands back
/// the base content it was meant to replace.
#[test]
fn a_malformed_selected_override_is_reported_not_papered_over() {
    let f = fixture();
    let root = f._tmp.path();

    // Texture: bytes that are not a PNG under a PNG name.
    let m = broken_mod(root, "badskin", "texture/vpt_skin.png", b"not a png at all");
    let vfs = mounted(&f.base, &m);
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let mut cache = MaterialCache::new(&vfs, &mut images, &mut materials);
    let handle = cache.get("vpt_skin");
    assert!(
        cache.missing_textures().contains("vpt_skin"),
        "the broken skin is reported"
    );
    let textured = materials
        .get(&handle)
        .and_then(|m| m.base_color_texture.clone());
    assert!(textured.is_none(), "the base skin must not stand in");

    // Handling: a tune that is not a vehcarsim.
    let m = broken_mod(
        root,
        "badtune",
        "tune/vehicle/vpt.vehcarsim",
        b"\x00\x01 junk",
    );
    let vfs = mounted(&f.base, &m);
    assert!(
        load_vehicle(&vfs, "vpt", 0).is_err(),
        "a garbage tune is an error"
    );
    assert!(
        load_vehicle(&vfs, "vpu", 0).is_ok(),
        "the bystander still loads"
    );

    // Prop: a PKG that is truncated garbage.
    let m = broken_mod(root, "badprop", "geometry/testprop.pkg", b"PKG3 truncated");
    let vfs = mounted(&f.base, &m);
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let loaded = {
        let mut commands = Commands::new(&mut queue, &world);
        let mut session = mm2_game::Session::new();
        load_city(
            &mut commands,
            &vfs,
            "city/test.psdl",
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
    };
    queue.apply(&mut world);
    let city = loaded.expect("the city around a broken prop still loads");
    assert_eq!(
        city.report.props_spawned, 0,
        "the base prop must not stand in"
    );
    assert_eq!(city.report.props_failed, 1, "the broken prop is reported");

    // Audio: a RIFF/WAVE header that promises more than it carries.
    let mut wav = pcm_wav(22050, 220);
    wav.truncate(30);
    let m = broken_mod(root, "badcue", "aud/aud22/surfaces/roadskid1.22k.wav", &wav);
    let vfs = mounted(&f.base, &m);
    let mut bank = WaveBank::index(&vfs);
    let mut waves: Assets<PcmAudio> = Assets::default();
    assert!(
        bank.load(&vfs, &mut waves, "roadskid1").is_err(),
        "a truncated cue is an error"
    );
    assert!(
        bank.load(&vfs, &mut waves, "grassskid").is_ok(),
        "the other cue still decodes"
    );

    // Audio, unsupported encoding: a well-formed WAVE whose format tag
    // (0x0055, MPEG layer 3) the mixer has no decoder for.
    let mut mp3 = pcm_wav(22050, 220);
    mp3[20..22].copy_from_slice(&0x55u16.to_le_bytes());
    let m = broken_mod(root, "mp3cue", "aud/aud22/surfaces/roadskid1.22k.wav", &mp3);
    let vfs = mounted(&f.base, &m);
    let mut bank = WaveBank::index(&vfs);
    assert!(
        bank.load(&vfs, &mut waves, "roadskid1").is_err(),
        "an unsupported encoding is an error, not the base cue"
    );
}
