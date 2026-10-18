//! F29-AC01/AC02/AC05: one synthetic mod replaces a car texture, a
//! vehicle's handling, a world prop and an audio cue, each observed through
//! the production consumer that reads it — `MaterialCache` (the path car
//! bodies and props take), `load_vehicle`, `load_city`'s prop stamping and
//! `WaveBank` — and the gameplay fingerprint classifies the edits.
//!
//! Everything is self-authored synthetic data; no original install is read.
//! This is *synthetic* evidence for the four consumers named: it says
//! nothing about the other families (menu art, race rules, surface tables,
//! localization …), which stay open as F29 tasks in Rally.

use std::path::Path;

use bevy::asset::Assets;
use bevy::ecs::system::Commands;
use bevy::ecs::world::{CommandQueue, World};
use bevy::image::Image;
use bevy::mesh::{Mesh, VertexAttributeValues};
use bevy::pbr::StandardMaterial;
use mm2_app::audio::{PcmAudio, WaveBank, trace_cue_reads};
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

/// F29-AC05 on the *shipped* examples: `examples/mods` is what an author
/// copies from, so the gameplay example must really move the gameplay
/// fingerprint and every cosmetic example must really leave it alone. The
/// `mm2_inspect` guard test checks what their manifests claim; this checks
/// the engine classifier agrees. Each example is found by its manifest
/// rather than by a hard-coded path, so renaming one keeps the check honest.
#[test]
fn the_shipped_example_mods_move_the_fingerprint_as_their_manifests_claim() {
    use mm2_assets::DeclaredEffect;

    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods");
    let mut dirs: Vec<_> = std::fs::read_dir(&examples)
        .expect("the shipped examples/mods exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("mod.toml").is_file())
        .collect();
    dirs.sort();

    let mut gameplay = Vec::new();
    let mut cosmetic = Vec::new();
    for dir in &dirs {
        let manifest = std::fs::read_to_string(dir.join("mod.toml")).unwrap();
        if manifest.contains("effect = \"gameplay\"") {
            gameplay.push(dir.clone());
        } else if manifest.contains("effect = \"cosmetic\"") {
            cosmetic.push(dir.clone());
        }
    }
    assert!(
        !gameplay.is_empty(),
        "there must be a shipped gameplay example under {examples:?}"
    );
    assert!(
        !cosmetic.is_empty(),
        "there must be a shipped cosmetic example under {examples:?}"
    );

    let f = fixture();
    let base = observe(&f.base, &[]).fingerprint;
    let mounted = |m: &Path| {
        let mut vfs = Vfs::new();
        vfs.mount_dir(&f.base, 0).unwrap();
        let manifest = vfs.mount_mod(m, 300).unwrap();
        (manifest.effect, vfs)
    };

    // The cosmetic examples are invisible to the fingerprint, as claimed.
    for m in &cosmetic {
        let (effect, vfs) = mounted(m);
        assert_eq!(effect, Some(DeclaredEffect::Cosmetic), "{m:?}");
        let reports = fingerprint::mod_reports(&vfs);
        assert!(reports[0].is_cosmetic_only(), "{reports:?}");
        assert_eq!(
            fingerprint::gameplay(&vfs).unwrap().hash,
            base,
            "{m:?} claims cosmetic but moves the fingerprint"
        );
    }

    // The gameplay example moves it, as claimed.
    for m in &gameplay {
        let (effect, vfs) = mounted(m);
        assert_eq!(effect, Some(DeclaredEffect::Gameplay), "{m:?}");
        let reports = fingerprint::mod_reports(&vfs);
        assert!(!reports[0].is_cosmetic_only(), "{reports:?}");
        assert_ne!(
            fingerprint::gameplay(&vfs).unwrap().hash,
            base,
            "{m:?} claims gameplay but leaves the fingerprint alone"
        );

        // The shipped tune must be real content an author can copy, not
        // bytes that only satisfy the classifier: every `.vehcarsim` in the
        // example decodes as a `VehCarSim` with no unknown fields.
        for file in files_with_ext(m, "vehcarsim") {
            let text = std::fs::read_to_string(&file).unwrap();
            let tune = mm2_formats::tune::TuneFile::parse(&text)
                .unwrap_or_else(|e| panic!("{file:?} does not parse: {e}"));
            let sim = mm2_formats::veh::VehCarSim::from_tune(&tune)
                .unwrap_or_else(|e| panic!("{file:?} does not decode as vehCarSim: {e}"));
            assert!(sim.warnings.is_empty(), "{file:?}: {:?}", sim.warnings);
        }
    }
}

/// Every file under `dir` (recursively) whose extension is `ext`, sorted.
fn files_with_ext(dir: &Path, ext: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
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

/// A `--mods` directory that fails to mount (or is not a directory) must not
/// let `--trace-deps` print a report with no mod in it and exit 0: that reads
/// as "no mod serves this city" when the mods were never mounted.
#[test]
fn trace_deps_refuses_a_mods_directory_that_did_not_mount() {
    let f = fixture();
    let root = f._tmp.path();
    let dupes = root.join("dupe_mods");
    let a = dupes.join("a");
    let b = dupes.join("b");
    for d in [&a, &b] {
        std::fs::create_dir_all(d).unwrap();
        manifest(d, "same-id");
    }
    let run = |mods: &Path| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mm2"))
            .arg("--mm2-path")
            .arg(&f.base)
            .arg("--mods")
            .arg(mods)
            .args(["--city", "test", "--trace-deps"])
            .env_remove("RUST_LOG")
            .output()
            .expect("spawn mm2");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (code, out, err) = run(&dupes);
    assert_eq!(
        code,
        Some(2),
        "duplicate mod ids fail the mount\n{out}{err}"
    );
    assert!(
        !out.contains("city/test.psdl ("),
        "no report is printed\n{out}"
    );

    let (code, out, err) = run(&root.join("no_such_mods_dir"));
    assert_eq!(code, Some(2), "a missing mods directory fails\n{out}{err}");
    assert!(
        !out.contains("city/test.psdl ("),
        "no report is printed\n{out}"
    );
}

/// The audio leg of `mm2 --trace-deps`: cue stems resolve and decode through
/// the production `WaveBank`, the mod that replaced a cue's wave is credited
/// with it, a cue the mod did not touch is not, and a wave that fails to
/// decode still names the mod file it choked on.
#[test]
fn a_traced_cue_load_credits_the_mod_that_replaced_its_wave() {
    let f = fixture();
    let root = f._tmp.path();
    let mut vfs = mounted(&f.base, &f.audio);

    let (loaded, trace) = trace_cue_reads(&vfs, &["roadskid1".into(), "grassskid".into()], &[]);
    assert_eq!(loaded.expect("both cues load"), "2 cue wave(s)");
    assert_eq!(
        trace.from_mod("cue"),
        ["aud/aud22/surfaces/roadskid1.22k.wav"]
    );
    assert!(
        trace
            .accesses
            .iter()
            .any(|a| a.logical == "aud/aud22/surfaces/grassskid.11k.wav"),
        "the untouched cue is traced too: {}",
        trace.render()
    );
    let (loaded, trace) = trace_cue_reads(&vfs, &["grassskid".into()], &[]);
    assert!(loaded.is_ok());
    assert!(trace.from_mod("cue").is_empty(), "{}", trace.render());

    // A stem no source provides fails, naming the stem.
    let (loaded, _) = trace_cue_reads(&vfs, &["roadskid1".into(), "nosuchcue".into()], &[]);
    let err = loaded.expect_err("an unknown stem must not load");
    assert!(err.contains("nosuchcue"), "{err}");

    // A mod whose wave does not decode fails the load, and the trace still
    // names the file read from it.
    let bad = broken_mod(
        root,
        "brokencue",
        "aud/aud22/surfaces/grassskid.22k.wav",
        b"RIFFjunk",
    );
    vfs.mount_mod(&bad, 310).unwrap();
    let (loaded, trace) = trace_cue_reads(&vfs, &["grassskid".into()], &[]);
    assert!(loaded.is_err(), "a junk wave must not decode");
    assert_eq!(
        trace.from_mod("brokencue"),
        ["aud/aud22/surfaces/grassskid.22k.wav"]
    );
}

/// The siren leg: a siren-program stem resolves through the `sirens/`-scoped
/// map, so a mod replacing the `sirens/` wave is credited to `--siren-cue`
/// and a mod replacing the same-named flat wave (which a plain cue lookup
/// picks, being the higher rate) is credited to `--cue` only.
#[test]
fn a_traced_siren_cue_follows_the_sirens_scoped_map() {
    let f = fixture();
    let root = f._tmp.path();
    let extra = root.join("siren_base");
    std::fs::create_dir_all(&extra).unwrap();
    write(
        &extra,
        "aud/aud22/surfaces/wail.22k.wav",
        pcm_wav(22050, 220),
    );
    write(&extra, "aud/aud22/sirens/wail.11k.wav", pcm_wav(11025, 220));
    let mut vfs = Vfs::new();
    vfs.mount_dir(&f.base, 0).unwrap();
    vfs.mount_dir(&extra, 1).unwrap();

    let scoped = broken_mod(
        root,
        "scoped",
        "aud/aud22/sirens/wail.11k.wav",
        &pcm_wav(11025, 440),
    );
    let flat = broken_mod(
        root,
        "flat",
        "aud/aud22/surfaces/wail.22k.wav",
        &pcm_wav(22050, 440),
    );
    vfs.mount_mod(&scoped, 300).unwrap();
    vfs.mount_mod(&flat, 301).unwrap();

    let (loaded, trace) = trace_cue_reads(&vfs, &[], &["wail".into()]);
    assert_eq!(
        loaded.expect("the siren stem loads"),
        "0 cue wave(s), 1 siren wave(s)"
    );
    assert_eq!(
        trace.from_mod("scoped"),
        ["aud/aud22/sirens/wail.11k.wav"],
        "{}",
        trace.render()
    );
    assert!(trace.from_mod("flat").is_empty(), "{}", trace.render());

    let (loaded, trace) = trace_cue_reads(&vfs, &["wail".into()], &[]);
    assert_eq!(loaded.expect("the cue stem loads"), "1 cue wave(s)");
    assert_eq!(
        trace.from_mod("flat"),
        ["aud/aud22/surfaces/wail.22k.wav"],
        "{}",
        trace.render()
    );
    assert!(trace.from_mod("scoped").is_empty(), "{}", trace.render());

    // A stem no wave ships fails naming it as a siren cue.
    let (loaded, _) = trace_cue_reads(&vfs, &[], &["nosuchsiren".into()]);
    let err = loaded.expect_err("an unknown siren stem must not load");
    assert!(err.contains("siren cue \"nosuchsiren\""), "{err}");

    // As a process: `--siren-cue` is a target on its own, conflicts with
    // `--city`, and `--expect-mod` reads the sirens/ file.
    let mods = root.join("siren_mods");
    std::fs::create_dir_all(&mods).unwrap();
    for (id, logical) in [
        ("scoped", "aud/aud22/sirens/wail.11k.wav"),
        ("flat", "aud/aud22/surfaces/wail.22k.wav"),
    ] {
        let d = mods.join(id);
        std::fs::create_dir_all(&d).unwrap();
        manifest(&d, id);
        let rate = if id == "scoped" { 11025 } else { 22050 };
        write(&d, logical, pcm_wav(rate, 440));
    }
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mm2"))
            .arg("--mm2-path")
            .arg(&f.base)
            .arg("--mods")
            .arg(&mods)
            .arg("--trace-deps")
            .args(args)
            .env_remove("RUST_LOG")
            .output()
            .expect("spawn mm2");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    // The install has no `wail` wave; each mod adds one.
    let (code, out) = run(&["--siren-cue", "wail", "--expect-mod", "scoped"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("1 siren wave(s)"), "{out}");
    let (code, out) = run(&["--siren-cue", "wail", "--expect-mod", "flat"]);
    assert_eq!(code, Some(2), "the flat mod serves no siren sample\n{out}");
    let (code, out) = run(&["--city", "test", "--siren-cue", "wail"]);
    assert_eq!(code, Some(2), "one target at a time\n{out}");
}

/// `mm2 --trace-deps --cue` as a process: the same per-source report as the
/// city target, `--expect-mod` as an exit status, and a usage error (exit 2)
/// when no target is named or both are.
#[test]
fn the_trace_deps_flag_reports_and_checks_which_mod_serves_a_cue() {
    let f = fixture();
    let mods = f._tmp.path().join("cue_mods");
    let m = mods.join("cue");
    std::fs::create_dir_all(&m).unwrap();
    audio_mod(&m);
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mm2"))
            .arg("--mm2-path")
            .arg(&f.base)
            .arg("--mods")
            .arg(&mods)
            .arg("--trace-deps")
            .args(args)
            .env_remove("RUST_LOG")
            .output()
            .expect("spawn mm2");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };

    let (code, out) = run(&[
        "--cue",
        "roadskid1",
        "--cue",
        "grassskid",
        "--expect-mod",
        "cue",
    ]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("mod `cue`: 1 file(s)"), "{out}");
    assert!(
        out.contains("  aud/aud22/surfaces/roadskid1.22k.wav ("),
        "{out}"
    );

    let (code, out) = run(&["--cue", "grassskid", "--expect-mod", "cue"]);
    assert_eq!(code, Some(2), "a mod that served nothing fails\n{out}");

    let (code, out) = run(&["--cue", "nosuchcue"]);
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("load failed after"), "{out}");

    let (code, out) = run(&[]);
    assert_eq!(
        code,
        Some(2),
        "a trace with no target is a usage error\n{out}"
    );
    let (code, out) = run(&["--city", "test", "--cue", "roadskid1"]);
    assert_eq!(code, Some(2), "one target at a time\n{out}");
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

    // Model, unsupported encoding: a file that is not a `PKG2`/`PKG3`
    // container. The car's geometry is required, so a bad container is an
    // error — the install's own model must not stand in.
    let m = broken_mod(root, "badmodel", "geometry/vpt.pkg", b"PKG9 not a model");
    let vfs = mounted(&f.base, &m);
    assert!(
        load_vehicle(&vfs, "vpt", 0).is_err(),
        "a model with a container the parser does not know is an error"
    );
    assert!(
        load_vehicle(&vfs, "vpu", 0).is_ok(),
        "the bystander still loads its own model"
    );

    // Texture, unsupported encoding: a well-formed TEX whose pixel type
    // has no decoder (`TEX.md` type 99; `decode_rgba` returns `None`).
    // The mod's `.tex` outranks the install's `.png` for the same stem —
    // source priority first — so the undecodable file is what the
    // consumer must report, never the shadowed PNG.
    for (id, bytes) in [
        ("badtex", unknown_tex(2, 2)),
        // Malformed encoding: a TEX header that stops before its pixel
        // fields — a structural parse error, reported the same way.
        ("trunctex", unknown_tex(2, 2)[..6].to_vec()),
    ] {
        let m = broken_mod(root, id, "texture/vpt_skin.tex", &bytes);
        let vfs = mounted(&f.base, &m);
        let mut images: Assets<Image> = Assets::default();
        let mut materials: Assets<StandardMaterial> = Assets::default();
        let mut cache = MaterialCache::new(&vfs, &mut images, &mut materials);
        let broken = cache.get("vpt_skin");
        // The broken override is scoped to its stem: the sibling car's
        // untouched `vpu_skin.png` still decodes.
        let bystander = cache.get("vpu_skin");
        assert!(
            cache.missing_textures().contains("vpt_skin"),
            "{id}: the undecodable .tex is reported"
        );
        assert!(
            materials
                .get(&broken)
                .and_then(|m| m.base_color_texture.clone())
                .is_none(),
            "{id}: the install's PNG must not stand in for the undecodable .tex"
        );
        assert!(
            materials
                .get(&bystander)
                .and_then(|m| m.base_color_texture.clone())
                .is_some(),
            "{id}: the bystander texture still resolves"
        );
    }
}

/// A TEX with a pixel type no decoder recognises: the header parses, the
/// pixels are kept raw, and `decode_rgba` yields nothing.
fn unknown_tex(width: u16, height: u16) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&width.to_le_bytes());
    d.extend_from_slice(&height.to_le_bytes());
    d.extend_from_slice(&99u16.to_le_bytes()); // pixel type 99: Unknown
    d.extend_from_slice(&1u16.to_le_bytes()); // mips
    d.extend_from_slice(&0u16.to_le_bytes()); // unknown
    d.extend_from_slice(&0u32.to_le_bytes()); // bits
    d.extend_from_slice(&[0u8; 16]); // pixels, kept raw for the unknown type
    d
}

/// F29-AC03: the same ranked provenance line for one mod-against-mod
/// conflict from the game's mount log and the dedicated server's startup.
/// `mm2-inspect conflicts` prints the identical line (asserted in
/// `tools/mm2_inspect/tests/conflicts.rs`), as does `mount_mods` itself.
/// Synthetic data; the retail run is left to a human with `MM2_RETAIL`.
#[test]
fn a_mod_conflict_reads_the_same_in_the_game_log_and_the_server_startup() {
    use crate::support::{Proc, WAIT};

    const EXPECTED: &str = "CONFLICT beta over alpha: 1 path(s), first texture/shared.png";
    let tmp = tempfile::tempdir().unwrap();
    let (install, mods) = (tmp.path().join("install"), tmp.path().join("mods"));
    std::fs::create_dir_all(&install).unwrap();
    write(&install, "texture/shared.png", b"install");
    for id in ["alpha", "beta"] {
        let d = mods.join(id);
        manifest(&d, id);
        write(&d, "texture/shared.png", id.as_bytes());
    }
    let (install_arg, mods_arg) = (
        install.to_str().unwrap().to_string(),
        mods.to_str().unwrap().to_string(),
    );

    // The game: any mod-mounting run logs at mount; `--trace-deps` is the
    // cheap windowless one.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mm2"))
        .args(["--mm2-path", &install_arg, "--mods", &mods_arg])
        .args(["--trace-deps", "--siren-cue", "wail"])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    // The game's subscriber writes to stdout.
    let game = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        game.contains(EXPECTED),
        "game mount log ({:?}):\n{game}",
        out.status
    );

    // The dedicated server: the log is on stderr at startup, stdout stays
    // the record stream.
    let host = Proc::spawn(
        env!("CARGO_BIN_EXE_mm2-host"),
        &[
            "--mm2-path".into(),
            install_arg,
            "--mods".into(),
            mods_arg,
            "--dev-world".into(),
            "--bind".into(),
            "127.0.0.1:0".into(),
        ],
    );
    let first = host.line();
    assert!(first.starts_with("listening="), "{first}");
    let server = host.stderr_containing(EXPECTED, WAIT);
    assert!(server.contains(EXPECTED), "server startup:\n{server}");
}

/// Menu art (F29-AC01/02/04/05): a mod's `menu/<name>.png` replaces the
/// embedded backdrop, removing it restores the embedded art, a malformed
/// override is an error rather than the embedded art, and the swap is
/// cosmetic to the gameplay fingerprint.
#[test]
fn a_mod_replaces_menu_art_and_a_malformed_one_is_refused() {
    use mm2_app::menu::menu_art_image;

    let f = fixture();
    let root = f._tmp.path();
    let city = include_bytes!("../assets/menu/city-night.png");
    let london = include_bytes!("../assets/menu/london-night.png");
    let mut plain = Vfs::new();
    plain.mount_dir(&f.base, 0).unwrap();
    let stock = menu_art_image(&plain, "city-night", city).expect("embedded art decodes");

    let art = root.join("menuart");
    manifest(&art, "menuart");
    write(&art, "menu/city-night.png", MOD_TEXTURE);
    let replaced = mounted(&f.base, &art);
    let modded = menu_art_image(&replaced, "city-night", city).expect("mod art decodes");
    assert_ne!(
        (modded.width(), modded.height()),
        (stock.width(), stock.height()),
        "the mod's PNG replaces the embedded art"
    );
    let bystander = menu_art_image(&replaced, "london-night", london).unwrap();
    let london_stock = menu_art_image(&plain, "london-night", london).unwrap();
    assert_eq!(bystander.data, london_stock.data, "unrelated art unchanged");

    let reverted = menu_art_image(&plain, "city-night", city).unwrap();
    assert_eq!(reverted.data, stock.data, "no mod: the embedded art");

    let bad = broken_mod(root, "badart", "menu/city-night.png", b"not a png at all");
    let err = menu_art_image(&mounted(&f.base, &bad), "city-night", city)
        .expect_err("a malformed override is refused");
    assert!(err.contains("menu/city-night.png"), "{err}");

    assert_eq!(
        observe(&f.base, &[&art]).fingerprint,
        observe(&f.base, &[]).fingerprint,
        "menu art is cosmetic"
    );
}

// ─── Families added to the proven set by F29-AC01 (#1528) ───────────────

/// A `mount` that is the base alone when `m` is `None`.
fn vfs_of(base: &Path, m: Option<&Path>) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(base, 0).unwrap();
    if let Some(m) = m {
        vfs.mount_mod(m, 300).unwrap();
    }
    vfs
}

/// Body half-extents, chassis size and fingerprint of `id` as loaded.
fn model_of(vfs: &Vfs, id: &str) -> (([f32; 3], [f32; 3]), String) {
    let def = load_vehicle(vfs, id, 0).expect("vehicle loads");
    (
        def.model.body_aabb.expect("body aabb"),
        format!("{:?}", def.config.chassis_size),
    )
}

/// A mod that swaps `vpt`'s PKG for a longer body and its bound for a
/// wider one.
fn model_mod(d: &Path) {
    manifest(d, "body");
    write(
        d,
        "geometry/vpt.pkg",
        crate::support::car_pkg_with_body([1.2, 0.5, 2.4]),
    );
    write(
        d,
        "bound/vpt_bound.bnd",
        crate::support::car_bnd().replace("0.9", "1.3"),
    );
}

#[test]
fn a_mod_replaces_a_vehicles_model_and_bound_through_load_vehicle() {
    let f = fixture();
    let root = f._tmp.path();
    let m = root.join("body");
    std::fs::create_dir_all(&m).unwrap();
    model_mod(&m);

    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    let (base_aabb, base_chassis) = model_of(&base, "vpt");
    let (mod_aabb, mod_chassis) = model_of(&modded, "vpt");
    assert_ne!(mod_aabb, base_aabb, "the mod's PKG reached the model");
    assert!(mod_aabb.1[2] > base_aabb.1[2], "the mod's body is longer");
    assert_ne!(
        mod_chassis, base_chassis,
        "the mod's bound reached the chassis"
    );
    // The bystander car reads the base files either way.
    assert_eq!(model_of(&modded, "vpu"), model_of(&base, "vpu"));
    // Unmounting restores the base.
    assert_eq!(
        model_of(&vfs_of(&f.base, None), "vpt"),
        (base_aabb, base_chassis)
    );

    // The model is gameplay (collision hull, wheel origins).
    let plain = fingerprint::gameplay(&base).unwrap().hash;
    assert_ne!(fingerprint::gameplay(&modded).unwrap().hash, plain);
    let reports = fingerprint::mod_reports(&modded);
    assert_eq!(reports.len(), 1);
    assert_eq!((reports[0].gameplay, reports[0].cosmetic), (2, 0));
}

#[test]
fn a_malformed_vehicle_pkg_override_is_refused_and_the_base_does_not_stand_in() {
    let f = fixture();
    let m = broken_mod(f._tmp.path(), "badpkg", "geometry/vpt.pkg", b"not a pkg");
    let vfs = mounted(&f.base, &m);
    let err = load_vehicle(&vfs, "vpt", 0).expect_err("a broken model must not load");
    assert!(err.to_string().contains("vpt"), "{err}");
    assert!(
        load_vehicle(&vfs, "vpu", 0).is_ok(),
        "the sibling still loads"
    );
}

#[test]
fn a_malformed_vehicle_bound_override_is_reported_and_the_base_does_not_stand_in() {
    let f = fixture();
    let m = broken_mod(f._tmp.path(), "badbnd", "bound/vpt_bound.bnd", b"junk");
    let vfs = mounted(&f.base, &m);
    let def = load_vehicle(&vfs, "vpt", 0).expect("an undecodable bound is reported, not fatal");
    assert!(
        def.report
            .warnings
            .iter()
            .any(|w| w.contains("bound/vpt_bound.bnd") && w.contains("failed to parse")),
        "{:?}",
        def.report.warnings
    );
    let sibling = load_vehicle(&vfs, "vpu", 0).expect("the sibling still loads");
    assert!(
        !sibling
            .report
            .warnings
            .iter()
            .any(|w| w.contains("_bound.bnd")),
        "{:?}",
        sibling.report.warnings
    );
}

fn banger_record(mass: f32) -> String {
    format!(
        "type: a\ndgBangerData {{\n  AudioId 0\n  Size 0.5 1.0 0.5\n  CG 0.0 0.5 0.0\n  Mass {mass}\n  Elasticity 0.5\n  Friction 0.9\n  ImpulseLimit2 0\n  SpinAxis 0\n  Flash 0\n  NumParts 0\n  TexNumber 0\n  BillFlags 0\n  YRadius 0.5\n}}\n"
    )
}

#[test]
fn a_mod_replaces_a_banger_record_through_banger_defs() {
    use mm2_app::banger::BangerDefs;
    let f = fixture();
    let root = f._tmp.path();
    write(
        &f.base,
        "tune/banger/lamp.dgbangerdata",
        banger_record(40.0),
    );
    write(&f.base, "tune/banger/bin.dgbangerdata", banger_record(15.0));
    let m = root.join("heavy");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "heavy");
    write(&m, "tune/banger/lamp.dgbangerdata", banger_record(400.0));

    let mass = |vfs: &Vfs, name: &str| BangerDefs::new(vfs).get(name).map(|d| d.mass);
    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(mass(&base, "lamp"), Some(40.0));
    assert_eq!(mass(&modded, "lamp"), Some(400.0), "the mod's record won");
    assert_eq!(
        mass(&modded, "bin"),
        mass(&base, "bin"),
        "bystander unmoved"
    );
    assert_eq!(mass(&base, "tree"), None, "an unbound name stays unbound");
    assert_eq!(mass(&vfs_of(&f.base, None), "lamp"), Some(40.0), "revert");

    assert_ne!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash,
        "banger tuning is gameplay"
    );

    // A record that does not decode is counted and stamps unbound; the base
    // record it replaced does not stand in.
    let bad = broken_mod(
        root,
        "badbanger",
        "tune/banger/lamp.dgbangerdata",
        b"\x00junk",
    );
    let vfs = mounted(&f.base, &bad);
    let mut defs = BangerDefs::new(&vfs);
    assert!(defs.get("lamp").is_none());
    assert_eq!(defs.failed, 1);
    assert!(defs.get("bin").is_some());
}

#[test]
fn a_mod_replaces_a_vehicles_audio_table_and_it_is_cosmetic() {
    let f = fixture();
    let root = f._tmp.path();
    write(
        &f.base,
        "aud/cardata/player/vpt.csv",
        crate::support::car_cardata(),
    );
    write(
        &f.base,
        "aud/cardata/player/vpu.csv",
        crate::support::car_cardata(),
    );
    let m = root.join("horn");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "horn");
    write(
        &m,
        "aud/cardata/player/vpt.csv",
        crate::support::car_cardata().replace("TESTHORN", "NEWHORN"),
    );

    let horn = |vfs: &Vfs, id: &str| load_vehicle(vfs, id, 0).unwrap().audio.map(|a| a.horn.name);
    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(horn(&base, "vpt").as_deref(), Some("TESTHORN"));
    assert_eq!(horn(&modded, "vpt").as_deref(), Some("NEWHORN"));
    assert_eq!(
        horn(&modded, "vpu"),
        horn(&base, "vpu"),
        "bystander unmoved"
    );
    assert_eq!(
        horn(&vfs_of(&f.base, None), "vpt").as_deref(),
        Some("TESTHORN")
    );
    assert_eq!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash,
        "an audio table is cosmetic"
    );

    // Malformed: the table is dropped (no audio), not replaced by the base's.
    let bad = broken_mod(
        root,
        "badcsv",
        "aud/cardata/player/vpt.csv",
        b"garbage\n1,2\n",
    );
    let vfs = mounted(&f.base, &bad);
    let def = load_vehicle(&vfs, "vpt", 0).expect("the car itself still loads");
    assert!(def.audio.is_none(), "the base table must not stand in");
}

/// The music report `music_state` produces for a cruise in `city` over `vfs`.
fn music_report(vfs: Vfs, city: &str) -> mm2_app::music::MusicReport {
    use mm2_app::audio::AudioReport;
    use mm2_app::music::{MusicAudio, music_state};
    use mm2_game::{Mm2Vfs, SessionMode};

    let mut app = bevy::app::App::new();
    app.insert_resource(Mm2Vfs(vfs))
        .init_resource::<AudioReport>()
        .insert_resource(MusicAudio::bind(Some(city), &SessionMode::Cruise).unwrap())
        .add_systems(bevy::app::Update, music_state);
    app.update();
    app.world()
        .resource::<AudioReport>()
        .music
        .clone()
        .expect("music binding resolved")
}

const ROAM_ONE_ROW: &str = "Start Music,Idle Music\nBaseStart,BaseIdle\n";

#[test]
fn a_mod_replaces_the_music_cue_tables_and_it_is_cosmetic() {
    let f = fixture();
    let root = f._tmp.path();
    write(&f.base, "aud/dmusic/csv_files/singleroam.csv", ROAM_ONE_ROW);
    write(
        &f.base,
        "aud/dmusic/csv_files/testambience.csv",
        ROAM_ONE_ROW,
    );
    let m = root.join("tunes");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "tunes");
    write(
        &m,
        "aud/dmusic/csv_files/singleroam.csv",
        "Start Music,Idle Music\nModStart,ModIdle\nTwoStart,TwoIdle\n",
    );

    let rows = |r: &mm2_app::music::MusicReport| -> Vec<(Option<usize>, usize)> {
        r.tables.iter().map(|t| (t.rows, t.refs)).collect()
    };
    let base = music_report(vfs_of(&f.base, None), "test");
    assert_eq!(rows(&base), [(Some(1), 2), (Some(1), 2)]);
    let modded = music_report(vfs_of(&f.base, Some(&m)), "test");
    assert_eq!(
        rows(&modded),
        [(Some(2), 4), (Some(1), 2)],
        "roam replaced, the city ambience table (bystander) comes from the base"
    );
    assert_eq!(
        rows(&music_report(vfs_of(&f.base, None), "test")),
        rows(&base),
        "unmounting reverts"
    );
    assert_eq!(
        fingerprint::gameplay(&vfs_of(&f.base, Some(&m)))
            .unwrap()
            .hash,
        fingerprint::gameplay(&vfs_of(&f.base, None)).unwrap().hash,
        "a music table is cosmetic"
    );

    // An override with no header at all is reported missing; the base's
    // rows do not stand in for it.
    let bad = broken_mod(
        root,
        "badmusic",
        "aud/dmusic/csv_files/singleroam.csv",
        b"\n",
    );
    let report = music_report(mounted(&f.base, &bad), "test");
    assert_eq!(report.tables[0].rows, None, "{}", report.line());
    assert!(report.line().contains("singleroam.csv missing"));
    assert_eq!(report.tables[1].rows, Some(1), "bystander unaffected");
}

#[test]
fn a_mod_replaces_a_siren_program_through_siren_audio() {
    use mm2_app::audio::SirenAudio;

    let f = fixture();
    let root = f._tmp.path();
    let program = |a: &str, b: &str| {
        format!(
            "Sample name,\n{a},0.9\nplay time,next index\n0.05,1\nSample name,\n{b},0.8\nplay time,next index\n0.05,0\n"
        )
    };
    write(
        &f.base,
        "aud/cardata/player/testcitypolicesiren.csv",
        program("wail_a", "wail_b"),
    );
    write(
        &f.base,
        "aud/cardata/player/othercitypolicesiren.csv",
        program("wail_a", "wail_b"),
    );
    write(
        &f.base,
        "aud/cardata/opponent/policesiren.csv",
        "Sample name,\nyelp,\nplay time,next index\n60,0\n",
    );
    let m = root.join("sirens");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "sirens");
    write(
        &m,
        "aud/cardata/player/testcitypolicesiren.csv",
        program("modwail", "modyelp"),
    );

    let names = |vfs: &Vfs, city: &str| -> (Vec<String>, Vec<String>) {
        let sa = SirenAudio::load(vfs, 1, Some(city)).expect("a program resolves");
        let of = |s: &Option<mm2_game::audio::SirenSpec>| {
            s.as_ref()
                .map(|s| s.samples.iter().map(|x| x.name.clone()).collect())
                .unwrap_or_default()
        };
        (of(&sa.player), of(&sa.opponent))
    };
    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(names(&base, "testcity").0, ["wail_a", "wail_b"]);
    assert_eq!(names(&modded, "testcity").0, ["modwail", "modyelp"]);
    assert_eq!(
        names(&modded, "othercity"),
        names(&base, "othercity"),
        "another city's program is a bystander"
    );
    assert_eq!(
        names(&modded, "testcity").1,
        ["yelp"],
        "the opponent program is a bystander"
    );
    assert_eq!(
        names(&vfs_of(&f.base, None), "testcity"),
        names(&base, "testcity"),
        "unmounting reverts"
    );
    assert_eq!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash,
        "a siren program is cosmetic"
    );

    // Malformed: the city has no player program (the consumer warns); the
    // base's table does not stand in. The opponent side still loads.
    let bad = broken_mod(
        root,
        "badsiren",
        "aud/cardata/player/testcitypolicesiren.csv",
        b"garbage\n1,2\n",
    );
    let (player, opponent) = names(&mounted(&f.base, &bad), "testcity");
    assert!(player.is_empty(), "base program must not stand in");
    assert_eq!(opponent, ["yelp"]);
}

/// The stems the session's environmental commentary plays over `vfs`, and
/// the report's `(commentary, failed)` counters. The production chain:
/// `CommentaryAudio` resolves registry → speaker → cue table → wave on the
/// first live frame and `commentary_voices` queues what it drew.
fn spoken_stems(vfs: Vfs) -> (Vec<String>, (u64, u64)) {
    use bevy::asset::AssetPlugin;
    use bevy::ecs::schedule::IntoScheduleConfigs;
    use bevy::time::TimeUpdateStrategy;
    use mm2_app::audio::{self, AudioReport, CommentaryAudio, CommentaryVoice};
    use mm2_game::{
        Mm2Vfs, Session, SessionConditions, SessionConfig, SessionPhase, TimeOfDay, Weather,
    };

    let bank = WaveBank::index(&vfs);
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    for phase in [
        SessionPhase::Ready,
        SessionPhase::Countdown,
        SessionPhase::Playing,
    ] {
        session.transition(phase).unwrap();
    }
    let conditions = SessionConditions {
        time_of_day: TimeOfDay::new(1).unwrap(),
        weather: Weather::new(3).unwrap(),
    };
    let mut app = bevy::app::App::new();
    app.add_plugins(bevy::MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .init_resource::<mm2_app::speech::SpeechArbiter>()
        .add_systems(
            bevy::app::Update,
            (
                audio::commentary_voices,
                mm2_app::speech::speech_arbiter.after(audio::commentary_voices),
            ),
        )
        .insert_resource(CommentaryAudio::bind(Some("test"), conditions, 7).unwrap());
    app.finish();
    app.cleanup();
    // One-second clips plus the sequencing gap: ~80 updates play the queue.
    for _ in 0..90 {
        app.update();
    }
    let mut stems: Vec<String> = app
        .world_mut()
        .query::<&CommentaryVoice>()
        .iter(app.world())
        .map(|v| v.stem.clone())
        .collect();
    stems.sort();
    let r = app.world().resource::<AudioReport>();
    (stems, (r.commentary, r.failed))
}

/// Speech base: the `test` registry authors one `AS` announcer, whose
/// weather and time-of-day tables each name one wave. The base ships every
/// wave a mod table can name, so only the tables decide what is spoken.
fn speech_base(d: &Path) {
    write(
        d,
        "aud/spchdata/test.csv",
        "\nNum announcers\n1\nprefix\nAS\n",
    );
    let table = |section: &str, prefix: &str| {
        format!(
            "Name prefix/type header,end sufix value,sufix add value\n{section} header,,\n{prefix},1,0\n"
        )
    };
    write(
        d,
        "aud/spchdata/as1/wearain_prerace.csv",
        table("WEATHER", "WEARAIN"),
    );
    write(
        d,
        "aud/spchdata/as1/timenoon_prerace.csv",
        table("TIMEOFDAY", "TIMENOON"),
    );
    for stem in ["as1wearain01", "as1modrain01", "as1timenoon01"] {
        write(
            d,
            &format!("aud/aud22/as1/{stem}.22k.wav"),
            pcm_wav(22050, 22050),
        );
    }
}

#[test]
fn a_mod_replaces_the_speech_tables_through_the_commentary_consumer_and_it_is_cosmetic() {
    let f = fixture();
    let root = f._tmp.path();
    speech_base(&f.base);
    let m = root.join("voices");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "voices");
    write(
        &m,
        "aud/spchdata/as1/wearain_prerace.csv",
        "Name prefix/type header,end sufix value,sufix add value\nWEATHER header,,\nMODRAIN,1,0\n",
    );

    let base = spoken_stems(vfs_of(&f.base, None));
    assert_eq!(
        base,
        (vec!["as1timenoon01".into(), "as1wearain01".into()], (2, 0))
    );
    let modded = spoken_stems(vfs_of(&f.base, Some(&m)));
    assert_eq!(
        modded,
        (vec!["as1modrain01".into(), "as1timenoon01".into()], (2, 0)),
        "the weather table is replaced, the time-of-day table (bystander) is the base's"
    );
    assert_eq!(
        spoken_stems(vfs_of(&f.base, None)),
        base,
        "unmounting reverts"
    );
    assert_eq!(
        fingerprint::gameplay(&vfs_of(&f.base, Some(&m)))
            .unwrap()
            .hash,
        fingerprint::gameplay(&vfs_of(&f.base, None)).unwrap().hash,
        "a speech table is cosmetic"
    );
    let reports = fingerprint::mod_reports(&vfs_of(&f.base, Some(&m)));
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].gameplay, reports[0].cosmetic),
        (0, 1),
        "{reports:?}"
    );
}

#[test]
fn a_malformed_speech_override_is_counted_and_the_base_does_not_stand_in() {
    let f = fixture();
    let root = f._tmp.path();
    speech_base(&f.base);

    // A weather table with no usable row: that cue is counted failed and the
    // base's WEARAIN line is not spoken in its place; the other cue plays.
    let bad = broken_mod(
        root,
        "badrain",
        "aud/spchdata/as1/wearain_prerace.csv",
        b"garbage\n1,2\n",
    );
    let (stems, (commentary, failed)) = spoken_stems(mounted(&f.base, &bad));
    assert_eq!(stems, ["as1timenoon01"]);
    assert_eq!((commentary, failed), (1, 1));

    // A registry that authors no announcer: no speaker is drawn, so nothing
    // is spoken and the failure is counted, not hidden by the base registry.
    let bad = broken_mod(
        root,
        "badregistry",
        "aud/spchdata/test.csv",
        b"\nNum announcers\n0\nprefix\nAS\n",
    );
    let (stems, (commentary, failed)) = spoken_stems(mounted(&f.base, &bad));
    assert!(stems.is_empty(), "{stems:?}");
    assert_eq!((commentary, failed), (0, 1));
}

#[test]
fn a_mod_replaces_the_cnr_commentary_table_and_a_malformed_one_is_refused() {
    use mm2_app::cnrvoice::CnrCommentary;
    use mm2_game::gold::{Call, CallKind, Side};

    let f = fixture();
    let root = f._tmp.path();
    let table = |robget: &str| {
        format!(
            "Name prefix/type header,end sufix value,sufix add value\nROBGETLOOT header,,\n{robget},1,0\nCOPGETLOOT header,,\nAS1\\AS1COPGET,1,0\n"
        )
    };
    write(&f.base, "aud/spchdata/cnrtest.csv", table("AL1\\AL1ROBGET"));
    write(
        &f.base,
        "aud/spchdata/cnrother.csv",
        table("AL1\\AL1ROBGET"),
    );
    let m = root.join("cnrvoices");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "cnrvoices");
    write(&m, "aud/spchdata/cnrtest.csv", table("AL1\\AL1MODGET"));

    let rob = Call {
        kind: CallKind::Get,
        side: Side::Robbers,
    };
    let cop = Call {
        kind: CallKind::Get,
        side: Side::Cops,
    };
    let draw = |vfs: &Vfs, city: &str, call: Call| {
        CnrCommentary::load(vfs, city, 7).map(|mut c| c.draw(call))
    };
    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(draw(&base, "test", rob), Some(Ok("al1robget01".into())));
    assert_eq!(draw(&modded, "test", rob), Some(Ok("al1modget01".into())));
    assert_eq!(
        draw(&modded, "test", cop),
        draw(&base, "test", cop),
        "an unchanged family of the replaced table is carried by the mod's copy"
    );
    assert_eq!(
        draw(&modded, "other", rob),
        draw(&base, "other", rob),
        "another city's table is a bystander"
    );
    assert_eq!(
        draw(&vfs_of(&f.base, None), "test", rob),
        draw(&base, "test", rob),
        "unmounting reverts"
    );
    assert_eq!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash,
        "a commentary table is cosmetic"
    );

    // Malformed: an empty table does not parse, so the city binds no
    // announcer (warned); the base's table does not stand in.
    let bad = broken_mod(root, "badcnr", "aud/spchdata/cnrtest.csv", b"");
    assert!(CnrCommentary::load(&mounted(&f.base, &bad), "test", 7).is_none());
    // A table that parses but authors no ROBGETLOOT cue draws an error,
    // not the base's line.
    let bad = broken_mod(
        root,
        "norob",
        "aud/spchdata/cnrtest.csv",
        b"Name prefix/type header,end sufix value,sufix add value\nCOPGETLOOT header,,\nAS1\\AS1COPGET,1,0\n",
    );
    let r = draw(&mounted(&f.base, &bad), "test", rob).expect("the table binds");
    assert!(r.is_err(), "{r:?}");
}

/// The `(sample_rate, volume)` of the scream each gender's walker voices
/// over `vfs`, `None` when that gender stays silent, and the report's
/// `failed` count. The production chain:
/// `PedVoiceAudio::load` binds the tables, `pedestrian_voices` draws a
/// sample and the `WaveBank` resolves it. Every table authors one sample,
/// so the draw is deterministic and the rate names the wave that played.
fn walker_screams(vfs: Vfs) -> ([Option<(u32, f32)>; 2], u64) {
    use bevy::asset::AssetPlugin;
    use bevy::audio::{AudioPlayer, PlaybackSettings};
    use bevy::math::Vec3;
    use mm2_app::audio::{
        AudioReport, AudioVoice, PedScream, PedVoiceAudio, VoiceKind, pedestrian_voices,
    };
    use mm2_game::{Mm2Vfs, Session, SessionConfig, SessionPhase};

    let bank = WaveBank::index(&vfs);
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    for phase in [
        SessionPhase::Ready,
        SessionPhase::Countdown,
        SessionPhase::Playing,
    ] {
        session.transition(phase).unwrap();
    }
    let table = PedVoiceAudio::load(&vfs, session.generation());
    let mut app = bevy::app::App::new();
    app.add_plugins(bevy::MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_message::<PedScream>()
        .add_systems(bevy::app::Update, pedestrian_voices);
    if let Some(table) = table {
        app.insert_resource(table);
    }
    app.finish();
    app.cleanup();

    let mut out = [None, None];
    for (slot, female) in [(0, true), (1, false)] {
        let walker = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(PedScream {
            walker,
            position: Vec3::ZERO,
            female,
        });
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&AudioVoice, &AudioPlayer<PcmAudio>, &PlaybackSettings)>();
        let heard: Vec<(u32, f32)> = q
            .iter(world)
            .filter(|(v, _, _)| v.kind == VoiceKind::PedVoice)
            .map(|(_, p, s)| {
                let rate = world
                    .resource::<Assets<PcmAudio>>()
                    .get(&p.0)
                    .unwrap()
                    .sample_rate
                    .get();
                (rate, s.volume.to_linear())
            })
            .collect();
        out[slot] = heard.first().copied();
        // Despawn so the second gender's pass sees only its own voice.
        let ids: Vec<_> = world
            .query_filtered::<bevy::ecs::entity::Entity, bevy::ecs::query::With<AudioVoice>>()
            .iter(world)
            .collect();
        for id in ids {
            world.despawn(id);
        }
    }
    (out, app.world().resource::<AudioReport>().failed)
}

fn ped_voice_table(first: &str, volume: &str) -> String {
    format!(
        "Min speed,Max speed,min time in range,max time out of range\r\n0,500,0,0\r\nsample name,volume,,\r\n{first},{volume},,\r\n"
    )
}

fn ped_voice_base(d: &Path) {
    write(d, "aud/aud22/creature3d/fsc1.22k.wav", pcm_wav(22050, 220));
    write(d, "aud/aud22/creature3d/msc1.22k.wav", pcm_wav(44100, 220));
    write(d, "aud/aud22/creature3d/modsc.22k.wav", pcm_wav(11025, 220));
    write(
        d,
        "aud/creaturedata/default_fpedvoice1.csv",
        ped_voice_table("FSC1", "0.8"),
    );
    write(
        d,
        "aud/creaturedata/default_mpedvoice1.csv",
        ped_voice_table("MSC1", "0.5"),
    );
}

#[test]
fn a_mod_replaces_a_pedestrian_voice_table_through_the_walker_scream_consumer() {
    let f = fixture();
    let root = f._tmp.path();
    ped_voice_base(&f.base);
    let m = root.join("screams");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "screams");
    write(
        &m,
        "aud/creaturedata/default_fpedvoice1.csv",
        ped_voice_table("MODSC", "0.6"),
    );

    let base = walker_screams(vfs_of(&f.base, None));
    assert_eq!(base, ([Some((22050, 0.8)), Some((44100, 0.5))], 0));
    let modded = walker_screams(vfs_of(&f.base, Some(&m)));
    let ([female, male], failed) = modded;
    assert_eq!(failed, 0);
    let (rate, volume) = female.expect("the mod's female scream sounds");
    assert_eq!(rate, 11025, "the female table is the mod's");
    assert!((volume - 0.6).abs() < 0.05, "the mod's authored volume");
    assert_eq!(male, base.0[1], "the male table (bystander) is the base's");
    assert_eq!(
        walker_screams(vfs_of(&f.base, None)),
        base,
        "unmounting reverts"
    );
    assert_eq!(
        fingerprint::gameplay(&vfs_of(&f.base, Some(&m)))
            .unwrap()
            .hash,
        fingerprint::gameplay(&vfs_of(&f.base, None)).unwrap().hash,
        "a pedestrian voice table is cosmetic"
    );
    let reports = fingerprint::mod_reports(&vfs_of(&f.base, Some(&m)));
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].gameplay, reports[0].cosmetic),
        (0, 1),
        "{reports:?}"
    );
}

#[test]
fn a_malformed_pedestrian_voice_override_is_silent_and_the_base_does_not_stand_in() {
    let f = fixture();
    let root = f._tmp.path();
    ped_voice_base(&f.base);

    // A female table with no usable row: that gender authors no sample, so
    // it is silent (the table warns); the base's FSC1 does not stand in
    // and the male table is a bystander.
    let bad = broken_mod(
        root,
        "badfemale",
        "aud/creaturedata/default_fpedvoice1.csv",
        b"garbage\n1,2\n",
    );
    let ([female, male], failed) = walker_screams(mounted(&f.base, &bad));
    assert_eq!(female, None, "the base table must not stand in");
    assert_eq!(male, Some((44100, 0.5)));
    assert_eq!(
        failed, 0,
        "an authored-empty table is silence, not a failure"
    );

    // A table naming a wave nothing carries is counted `failed` and silent
    // for that gender, not answered with another gender's or the base's
    // sample.
    let bad = broken_mod(
        root,
        "nowave",
        "aud/creaturedata/default_mpedvoice1.csv",
        ped_voice_table("NOSUCHWAVE", "0.5").as_bytes(),
    );
    let ([female, male], failed) = walker_screams(mounted(&f.base, &bad));
    assert_eq!(female, Some((22050, 0.8)));
    assert_eq!(male, None);
    assert_eq!(failed, 1);
}

/// A clip of `frames` frames for the three-bone synthetic rig (12 floats a
/// frame).
fn ped_clip(frames: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&frames.to_le_bytes());
    b.extend_from_slice(&12u32.to_le_bytes());
    b.extend_from_slice(&0f32.to_le_bytes());
    b.push(1);
    for _ in 0..frames * 12 {
        b.extend_from_slice(&0f32.to_le_bytes());
    }
    b
}

/// A shader table of `paints` paint jobs over the mesh's two material groups.
fn ped_shaders(paints: u32) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&paints.to_le_bytes());
    s.extend_from_slice(&2u32.to_le_bytes());
    for _ in 0..paints * 2 {
        s.push(0);
        for c in [0.5f32; 16] {
            s.extend_from_slice(&c.to_le_bytes());
        }
        s.extend_from_slice(&4f32.to_le_bytes());
    }
    s
}

/// `(walk frames, stand frames, paint jobs)` of the loaded archetype.
fn ped_shape(vfs: &Vfs) -> Result<(u32, u32, usize), mm2_content::ped::PedLoadError> {
    let a = mm2_content::ped::PedArchetype::load(vfs, "pedmodel_man")?;
    Ok((
        a.clip("xwalk").unwrap().frames,
        a.clip("xstand").unwrap().frames,
        a.paint_jobs(),
    ))
}

#[test]
fn a_mod_replaces_pedestrian_animation_through_the_archetype_loader_and_it_is_gameplay() {
    use mm2_content::ped::PedLoadError;

    let f = fixture();
    let root = f._tmp.path();
    let src = crate::pedestrian::install();
    for name in ["skel", "csv", "mod", "shaders"] {
        let rel = format!("anim/pedmodel_man.{name}");
        write(&f.base, &rel, std::fs::read(src.path().join(&rel)).unwrap());
    }
    for clip in ["xstand", "xwalk"] {
        let rel = format!("anim/{clip}.anim");
        write(&f.base, &rel, std::fs::read(src.path().join(&rel)).unwrap());
    }
    let m = root.join("strut");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "strut");
    write(&m, "anim/xwalk.anim", ped_clip(9));
    write(&m, "anim/pedmodel_man.shaders", ped_shaders(3));

    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(ped_shape(&base).unwrap(), (4, 4, 1));
    assert_eq!(
        ped_shape(&modded).unwrap(),
        (9, 4, 3),
        "walk clip and paint jobs are the mod's, the stand clip (bystander) is the base's"
    );
    assert_eq!(
        ped_shape(&vfs_of(&f.base, None)).unwrap(),
        (4, 4, 1),
        "unmounting reverts"
    );
    assert_ne!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash,
        "`anim/` is hashed as gameplay"
    );
    let reports = fingerprint::mod_reports(&modded);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].gameplay, reports[0].cosmetic),
        (2, 0),
        "{reports:?}"
    );

    // Malformed: each broken member is an error naming it, never the base's
    // member standing in.
    let bad = broken_mod(root, "badclip", "anim/xwalk.anim", b"not a clip");
    assert!(matches!(
        ped_shape(&mounted(&f.base, &bad)),
        Err(PedLoadError::MissingClip { .. })
    ));
    let bad = broken_mod(root, "badmesh", "anim/pedmodel_man.mod", b"garbage\n");
    assert!(ped_shape(&mounted(&f.base, &bad)).is_err());
    let bad = broken_mod(root, "badskel", "anim/pedmodel_man.skel", b"garbage\n");
    assert!(ped_shape(&mounted(&f.base, &bad)).is_err());
    let bad = broken_mod(root, "badcsv", "anim/pedmodel_man.csv", b"");
    assert!(ped_shape(&mounted(&f.base, &bad)).is_err());
    let bad = broken_mod(root, "badshaders", "anim/pedmodel_man.shaders", b"xx");
    assert!(ped_shape(&mounted(&f.base, &bad)).is_err());
}

/// A `camTrackCS` chase-lens record authoring `offset_y` and `fov`.
fn track_cam(offset_y: f32, fov: f32) -> String {
    format!(
        "type: a\ncamTrackCS {{\n  Offset 0.000000 {offset_y:.6} -6.000000\n  MinDist 4.000000\n  MaxDist 8.000000\n  CameraFOV {fov:.6}\n  CameraNear 0.100000\n  CameraFar 600.000000\n}}\n"
    )
}

/// A `camPovCS` cockpit-eye record authoring `eye_y`.
fn pov_cam(eye_y: f32) -> String {
    format!(
        "type: a\ncamPovCS {{\n  Offset 0.000000 {eye_y:.6} -0.500000\n  ReverseOffset 0.000000 1.700000 0.750000\n  Pitch 0.020000\n  CameraFOV 70.000000\n  CameraNear 0.100000\n  CameraFar 600.000000\n}}\n"
    )
}

fn camera_base(d: &Path) {
    for car in ["vpt", "vpu"] {
        write(
            d,
            &format!("tune/camera/{car}_near.camtrackcs"),
            track_cam(2.0, 60.0),
        );
        write(
            d,
            &format!("tune/camera/{car}_far.camtrackcs"),
            track_cam(4.0, 50.0),
        );
        write(d, &format!("tune/camera/{car}_dash.campovcs"), pov_cam(1.2));
    }
}

/// `(near offset y, near fov, far offset y, pov eye y)` of `car`, read
/// through the two production loaders; `None` for a record that did not
/// load.
type CameraShape = (Option<(f32, f32)>, Option<(f32, f32)>, Option<f32>);

fn camera_shape(vfs: &Vfs, car: &str) -> CameraShape {
    let cams = mm2_app::camera::load_track_cams(vfs, car);
    let lens = |c: Option<mm2_formats::camtrack::TrackCamSpec>| {
        c.map(|s| (s.offset.unwrap()[1], s.camera_fov.unwrap()))
    };
    (
        lens(cams.near),
        lens(cams.far),
        mm2_app::dash::load_pov_cam(vfs, car).map(|p| p.offset.unwrap()[1]),
    )
}

#[test]
fn a_mod_replaces_camera_tuning_through_the_camera_and_dash_loaders() {
    let f = fixture();
    let root = f._tmp.path();
    camera_base(&f.base);
    let m = root.join("lens");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "lens");
    write(&m, "tune/camera/vpt_near.camtrackcs", track_cam(3.0, 75.0));
    write(&m, "tune/camera/vpt_dash.campovcs", pov_cam(1.5));

    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    assert_eq!(
        camera_shape(&base, "vpt"),
        (Some((2.0, 60.0)), Some((4.0, 50.0)), Some(1.2))
    );
    assert_eq!(
        camera_shape(&modded, "vpt"),
        (Some((3.0, 75.0)), Some((4.0, 50.0)), Some(1.5)),
        "near lens and cockpit eye are the mod's; the far lens (bystander) is the base's"
    );
    assert_eq!(
        camera_shape(&modded, "vpu"),
        camera_shape(&base, "vpu"),
        "another car is a bystander"
    );
    assert_eq!(
        camera_shape(&vfs_of(&f.base, None), "vpt"),
        camera_shape(&base, "vpt"),
        "unmounting reverts"
    );
    // `tune/` is a gameplay prefix wholesale, so camera records move the
    // fingerprint although they only steer the local view (filed as a
    // classification question, not changed here).
    assert_ne!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash
    );
    let reports = fingerprint::mod_reports(&modded);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].gameplay, reports[0].cosmetic),
        (2, 0),
        "{reports:?}"
    );
}

/// A malformed camera override does not let the base's record stand in:
/// the record reads as absent and the consumer takes its designed
/// fallback. The loaders only `warn!` for *validation* issues; an
/// unparseable record is dropped without a diagnostic (filed).
#[test]
fn a_malformed_camera_override_does_not_let_the_base_stand_in() {
    let f = fixture();
    let root = f._tmp.path();
    camera_base(&f.base);
    let bad = broken_mod(
        root,
        "badnear",
        "tune/camera/vpt_near.camtrackcs",
        b"camPovCS {\n}\n",
    );
    assert_eq!(
        camera_shape(&mounted(&f.base, &bad), "vpt"),
        (None, Some((4.0, 50.0)), Some(1.2))
    );
    let bad = broken_mod(
        root,
        "badpov",
        "tune/camera/vpt_dash.campovcs",
        b"camTrackCS {\n}\n",
    );
    assert_eq!(
        camera_shape(&mounted(&f.base, &bad), "vpt"),
        (Some((2.0, 60.0)), Some((4.0, 50.0)), None)
    );
}

/// One self-authored standalone `asBirthRule`.
fn birth_rule(radius: f32, frame0: i64, spew: f32) -> String {
    format!(
        "type: a\nasBirthRule {{\n  PositionVar 0.200000 0.000000 0.200000\n  Velocity 0.000000 1.000000 0.000000\n  VelocityVar 0.500000 0.500000 0.500000\n  Life 0.500000\n  LifeVar 0.000000\n  Mass 0.300000\n  MassVar 0.000000\n  Radius {radius:.6}\n  RadiusVar 0.000000\n  Drag 0.000000\n  DragVar 0.000000\n  Damp 0.000000\n  DampVar 0.000000\n  DRadius 0.100000\n  DRadiusVar 0.000000\n  DAlpha -64.000000\n  DAlphaVar 0.000000\n  DRotation 0.000000\n  DRotationVar 0.000000\n  InitialBlast 4\n  SpewRate {spew:.6}\n  SpewTimeLimit 0.000000\n  Gravity -1.000000\n  TexFrameStart {frame0}\n  TexFrameEnd {}\n  BirthFlags 0\n  Height 0.000000\n  Intensity 1.000000\n  Color -1\n}}\n",
        frame0 + 7
    )
}

fn effects_base(d: &Path) {
    for (i, name) in mm2_game::PTX_RULE_NAMES.iter().enumerate() {
        write(
            d,
            &format!("tune/effects/{name}.asbirthrule"),
            birth_rule(0.3, i as i64 * 8, 10.0),
        );
    }
    write(d, "tune/rain.asbirthrule", birth_rule(0.1, 0, 200.0));
}

/// `(radius, tile start)` of each wheel effect that bound, and the report's
/// `(loaded, failed)`.
type WheelEffects = (Vec<Option<(f32, i64)>>, (usize, usize));

fn wheel_effects(vfs: &Vfs) -> WheelEffects {
    let (report, fx) = mm2_app::wheel_fx::wheel_fx_session(
        vfs,
        &mut Assets::<Mesh>::default(),
        &mut Assets::<Image>::default(),
        &mut Assets::<StandardMaterial>::default(),
    );
    let specs = fx
        .specs
        .iter()
        .map(|s| s.as_ref().map(|s| (s.radius, s.tex_frame_start)))
        .collect();
    (specs, (report.loaded, report.failed))
}

/// `(radius, spew rate)` of the bound rain rule, and the report's
/// `absent` reason.
fn rain(vfs: &Vfs) -> (Option<(f32, f32)>, Option<&'static str>) {
    let (report, _fx, rig) = mm2_app::precip::precip_session(
        vfs,
        mm2_game::Weather::new(3).unwrap(),
        7,
        &mut Assets::<Mesh>::default(),
        &mut Assets::<Image>::default(),
        &mut Assets::<StandardMaterial>::default(),
    );
    (
        rig.map(|r| (r.spec.radius, r.spec.spew_rate)),
        report.absent,
    )
}

#[test]
fn a_mod_replaces_wheel_effect_and_rain_rules_through_their_session_binders() {
    let f = fixture();
    let root = f._tmp.path();
    effects_base(&f.base);
    let m = root.join("fx");
    std::fs::create_dir_all(&m).unwrap();
    manifest(&m, "fx");
    write(
        &m,
        "tune/effects/dust.asbirthrule",
        birth_rule(0.9, 8, 10.0),
    );
    write(&m, "tune/rain.asbirthrule", birth_rule(0.4, 0, 90.0));

    let base = vfs_of(&f.base, None);
    let modded = vfs_of(&f.base, Some(&m));
    let (base_fx, base_counts) = wheel_effects(&base);
    let n = mm2_game::PTX_RULE_NAMES.len();
    assert_eq!((base_counts, base_fx.len()), ((n, 0), n));
    let dust = mm2_game::PTX_RULE_NAMES
        .iter()
        .position(|&r| r == "dust")
        .unwrap();
    assert_eq!(base_fx[dust], Some((0.3, 8)));

    let (mod_fx, mod_counts) = wheel_effects(&modded);
    assert_eq!(mod_counts, (n, 0));
    assert_eq!(mod_fx[dust], Some((0.9, 8)), "the dust rule is the mod's");
    for (i, (a, b)) in base_fx.iter().zip(&mod_fx).enumerate() {
        if i != dust {
            assert_eq!(a, b, "effect {i} (bystander) is the base's");
        }
    }
    assert_eq!(rain(&base), (Some((0.1, 200.0)), None));
    assert_eq!(rain(&modded), (Some((0.4, 90.0)), None));
    assert_eq!(wheel_effects(&vfs_of(&f.base, None)).0, base_fx);
    assert_eq!(rain(&vfs_of(&f.base, None)), rain(&base), "revert");

    // `tune/` is a gameplay prefix wholesale (see the camera test).
    assert_ne!(
        fingerprint::gameplay(&modded).unwrap().hash,
        fingerprint::gameplay(&base).unwrap().hash
    );
    let reports = fingerprint::mod_reports(&modded);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].gameplay, reports[0].cosmetic),
        (2, 0),
        "{reports:?}"
    );
}

#[test]
fn a_malformed_effect_override_is_counted_and_the_base_does_not_stand_in() {
    let f = fixture();
    let root = f._tmp.path();
    effects_base(&f.base);
    let n = mm2_game::PTX_RULE_NAMES.len();

    let bad = broken_mod(
        root,
        "baddirt",
        "tune/effects/dirt.asbirthrule",
        b"garbage\n",
    );
    let (fx, (loaded, failed)) = wheel_effects(&mounted(&f.base, &bad));
    let dirt = mm2_game::PTX_RULE_NAMES
        .iter()
        .position(|&r| r == "dirt")
        .unwrap();
    assert_eq!((loaded, failed), (n - 1, 1));
    assert_eq!(
        fx[dirt], None,
        "the slot stays dark, the base's rule is not used"
    );

    let bad = broken_mod(root, "badrain", "tune/rain.asbirthrule", b"garbage\n");
    assert_eq!(
        rain(&mounted(&f.base, &bad)),
        (None, Some("rule unparseable"))
    );
}

// ─── Traffic, police and parked-car routes (F29-AC01, #1608) ────────────
//
// Four consumers: `mm2_content::ambient_setup` (`city/<city>.aimap`, the
// city's roster and density), `police::cruise_roster` (`race/<city>/roam
// .aimap`), `load_city`'s `city/<city>/props.pathset` stamping and
// `city::spawn_parked_cars` (`race/<city>/<city>_parkedcar.pathset`).

use crate::banger::{pth1, pth1_path};
use crate::support::{ambient_assets, city_aimap};

const ROAM_STOCK: &str = "[Police]\n1\nvpcop 0 0 130 0 0 15 0.5 50\n";
const ROAM_MOD: &str = "[Police]\n2\nvpcop 0 0 130 0 0 15 0.5 50\nvpcop 4 0 90 0 0 15 0.5 50\n";

fn route_base(d: &Path) {
    base_install(d);
    write(d, "city/test.aimap", city_aimap());
    write(d, "city/other.aimap", city_aimap());
    ambient_assets(d, "va_test_a");
    ambient_assets(d, "va_test_b");
    write(d, "race/test/roam.aimap", ROAM_STOCK);
    write(d, "race/other/roam.aimap", ROAM_STOCK);
    write(
        d,
        "city/test/props.pathset",
        pth1(&[pth1_path("testprop", &[[0.0, 0.0, 15.0]], 0, 0)]),
    );
    write(
        d,
        "race/test/test_parkedcar.pathset",
        pth1(&[pth1_path("park", &[[0.0, 0.0, 0.0]], 0, 0)]),
    );
}

/// A base install plus a mod directory carrying `files`.
fn route_mod(
    files: &[(&str, Vec<u8>)],
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let m = tmp.path().join("routes");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::create_dir_all(&m).unwrap();
    route_base(&base);
    manifest(&m, "routes");
    for (logical, bytes) in files {
        write(&m, logical, bytes);
    }
    (tmp, base, m)
}

fn gameplay_hash(vfs: &Vfs) -> u64 {
    fingerprint::gameplay(vfs).unwrap().hash
}

/// (roster class ids, city density) the ambient consumer builds.
fn ambient_of(vfs: &Vfs, city: &str) -> (usize, Option<f32>) {
    let setup = mm2_content::ambient_setup(vfs, city, None)
        .expect("the city aimap loads")
        .expect("the city authors a roster");
    (setup.roster.entries.len(), setup.city_density)
}

#[test]
fn a_mod_replaces_the_city_ambient_aimap_and_it_is_gameplay() {
    let (tmp, base, m) = route_mod(&[(
        "city/test.aimap",
        b"[Ambient Types/Density]\n1\nva_test_a 1.0 0\n[Density]\n0.75\n".to_vec(),
    )]);
    let stock = vfs_of(&base, None);
    let modded = vfs_of(&base, Some(&m));
    assert_eq!(ambient_of(&stock, "test"), (2, Some(0.25)));
    assert_eq!(ambient_of(&modded, "test"), (1, Some(0.75)), "mod won");
    assert_eq!(
        ambient_of(&modded, "other"),
        ambient_of(&stock, "other"),
        "bystander city unmoved"
    );
    assert_eq!(ambient_of(&vfs_of(&base, None), "test"), (2, Some(0.25)));
    assert_ne!(gameplay_hash(&modded), gameplay_hash(&stock), "gameplay");

    // An aimap that declares more rows than it authors is an error from
    // the consumer; the stock aimap does not stand in.
    let bad = broken_mod(
        tmp.path(),
        "badcityaimap",
        "city/test.aimap",
        b"[Ambient Types/Density]\n2\nva_test_a 1.0 0\n",
    );
    let vfs = mounted(&base, &bad);
    let err = mm2_content::ambient_setup(&vfs, "test", None)
        .expect_err("a malformed city aimap is refused");
    assert!(
        matches!(err, mm2_content::TrafficLoadError::Aimap(ref e) if e.contains("city/test.aimap")),
        "{err:?}"
    );
    assert!(mm2_content::ambient_setup(&vfs, "other", None).is_ok());
}

#[test]
fn a_mod_replaces_the_cruise_police_lineup_and_it_is_gameplay() {
    use mm2_app::police::cruise_roster;
    use mm2_game::Difficulty;
    let (tmp, base, m) = route_mod(&[("race/test/roam.aimap", ROAM_MOD.as_bytes().to_vec())]);
    let stock = vfs_of(&base, None);
    let modded = vfs_of(&base, Some(&m));
    let lineup = |v: &Vfs, city: &str| cruise_roster(v, city, Difficulty::Amateur);
    assert_eq!(lineup(&stock, "test").entries.len(), 1);
    let won = lineup(&modded, "test");
    assert_eq!(won.entries.len(), 2, "the mod's lineup won");
    assert_eq!(won.entries[1].position.x, 4.0);
    assert_eq!(
        lineup(&modded, "other").entries,
        lineup(&stock, "other").entries
    );
    assert_eq!(
        lineup(&vfs_of(&base, None), "test").entries.len(),
        1,
        "revert"
    );
    assert_ne!(gameplay_hash(&modded), gameplay_hash(&stock), "gameplay");

    // The consumer's own error: the malformed record is refused by the
    // content loader. `cruise_roster` logs it and fields no cops rather
    // than the stock lineup, so the base's cop does not stand in.
    let bad = broken_mod(
        tmp.path(),
        "badroam",
        "race/test/roam.aimap",
        b"[Police]\n2\nvpcop 0 0 130 0 0 15 0.5 50\n",
    );
    let vfs = mounted(&base, &bad);
    assert!(matches!(
        mm2_content::cruise_police_roster(&vfs, "test", Difficulty::Amateur),
        Err(mm2_content::RosterBuildError::AimapParse { .. })
    ));
    assert!(lineup(&vfs, "test").entries.is_empty());
    assert_eq!(lineup(&vfs, "other").entries.len(), 1);
}

/// `props.pathset` stamping totals `load_city` reports on `vfs`.
fn props_pathset_report(vfs: &Vfs) -> (usize, usize) {
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
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
    queue.apply(&mut world);
    (
        loaded.report.pathset_props_spawned,
        loaded.report.pathset_props_failed,
    )
}

#[test]
fn a_mod_replaces_the_street_dressing_pathset_and_it_is_gameplay() {
    let two = pth1(&[pth1_path(
        "testprop",
        &[[0.0, 0.0, 15.0], [0.0, 0.0, 25.0]],
        0,
        0,
    )]);
    let (tmp, base, m) = route_mod(&[("city/test/props.pathset", two)]);
    let stock = vfs_of(&base, None);
    let modded = vfs_of(&base, Some(&m));
    assert_eq!(props_pathset_report(&stock), (1, 0));
    assert_eq!(props_pathset_report(&modded), (2, 0), "the mod's path won");
    assert_eq!(props_pathset_report(&vfs_of(&base, None)), (1, 0), "revert");
    assert_ne!(gameplay_hash(&modded), gameplay_hash(&stock), "gameplay");

    // An unparseable file stamps nothing (a `warn!`, no pathset counters);
    // the base's stamp does not stand in.
    let bad = broken_mod(tmp.path(), "badprops", "city/test/props.pathset", b"junk");
    assert_eq!(props_pathset_report(&mounted(&base, &bad)), (0, 0));
}

/// What `spawn_parked_cars` made of `vfs`: (file, failed files, stamps).
fn parked_of(vfs: &Vfs) -> (Option<String>, Vec<String>, usize) {
    let mut world = World::new();
    let mut queue = CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let mut commands = Commands::new(&mut queue, &world);
    let mut session = mm2_game::Session::new();
    let r = mm2_app::city::spawn_parked_cars(
        &mut commands,
        vfs,
        "test",
        None,
        7,
        &mut meshes,
        &mut images,
        &mut materials,
        SessionEntity(1),
        &mut session,
    );
    queue.apply(&mut world);
    (
        r.file,
        r.failed_files,
        r.cars + r.empty_bays + r.missing_models,
    )
}

#[test]
fn a_mod_replaces_the_parked_car_pathset_and_it_is_gameplay() {
    const FILE: &str = "race/test/test_parkedcar.pathset";
    let three = pth1(&[pth1_path(
        "park",
        &[[0.0, 0.0, 0.0], [0.0, 0.0, 40.0], [0.0, 0.0, 80.0]],
        0,
        0,
    )]);
    let (tmp, base, m) = route_mod(&[(FILE, three)]);
    let stock = vfs_of(&base, None);
    let modded = vfs_of(&base, Some(&m));
    assert_eq!(parked_of(&stock), (Some(FILE.into()), vec![], 1));
    assert_eq!(
        parked_of(&modded),
        (Some(FILE.into()), vec![], 3),
        "mod won"
    );
    assert_eq!(parked_of(&vfs_of(&base, None)).2, 1, "revert");
    assert_ne!(gameplay_hash(&modded), gameplay_hash(&stock), "gameplay");

    // An unparseable override is reported in `failed_files`; the loader
    // then falls through to the next candidate, and here there is none, so
    // the base's file does not stand in.
    let bad = broken_mod(tmp.path(), "badparked", FILE, b"junk");
    assert_eq!(
        parked_of(&mounted(&base, &bad)),
        (None, vec![FILE.to_string()], 0)
    );
}
