//! F18-A.2/.3 environment binding integration: authored `.ltNN` presets
//! and `city/<stem>_fog.csv` rows through the real `load_session_world`
//! path — session-legal condition selection, authored-event precedence
//! and the missing/degenerate-content diagnostics — over a synthetic
//! one-room city.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::environment::{ConditionsSource, EnvironmentReport, SkyDome};
use mm2_app::session::{self, SessionControl};
use mm2_app::{camera, contracts};
use mm2_assets::Vfs;
use mm2_game::{
    Densities, EventRef, EventTableKind, ImpactEvent, Mm2Vfs, RaceStarted, ResultLedger, Session,
    SessionConditions, SessionConfig, SessionCustomization, SessionEntity, SessionMode,
    SessionPhase, TimeOfDay, Weather, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A synthetic `.ltNN` record — the retail grammar with self-authored
/// values distinct enough to identify in assertions.
fn lt_record(name: &str, key_color: [f32; 3], ambient: i32) -> String {
    format!(
        "type: a\n{name} {{\n\
         \x20\x20KeyHeading 0.500000\n\
         \x20\x20KeyPitch -0.200000\n\
         \x20\x20KeyColor {} {} {}\n\
         \x20\x20Fill1Heading 2.500000\n\
         \x20\x20Fill1Pitch -1.000000\n\
         \x20\x20Fill1Color 0.100000 0.200000 0.300000\n\
         \x20\x20Fill2Heading -2.000000\n\
         \x20\x20Fill2Pitch -0.800000\n\
         \x20\x20Fill2Color 0.050000 0.050000 0.050000\n\
         \x20\x20Ambient {ambient}\n\
         }}\n",
        key_color[0], key_color[1], key_color[2],
    )
}

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// The same one-room synthetic PSDL `import_pipeline`/`banger` stamp —
/// a road (x −5..5, z 0..20) plus a ground fan — copied so this test
/// stays self-contained (each `tests/` file is a crate).
fn city_psdl() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"PSD0");
    d.extend_from_slice(&2u32.to_le_bytes()); // target_size
    let verts: &[[f32; 3]] = &[
        [-5., 0., 0.],
        [-3., 0., 0.],
        [3., 0., 0.],
        [5., 0., 0.], // road section 0: sw_l, rl, rr, sw_r
        [-5., 0., 20.],
        [-3., 0., 20.],
        [3., 0., 20.],
        [5., 0., 20.], // road section 1
        [10., 0., 0.],
        [10., 0., 10.],
        [20., 0., 10.],
        [20., 0., 0.], // fan (clockwise in x,z)
        [30., 0., 0.],
        [30., 0., 10.], // wall edge
        [35., 5., 0.],
        [45., 5., 0.],
        [45., 5., 10.],
        [35., 5., 10.], // roof
    ];
    d.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut d, v);
    }
    let heights = [0.15f32, 2.0, 6.0];
    d.extend_from_slice(&(heights.len() as u32).to_le_bytes());
    push_f32s(&mut d, &heights);
    d.extend_from_slice(&2u32.to_le_bytes());
    push_lp(&mut d, "test_road");
    d.extend_from_slice(&2u32.to_le_bytes()); // nRooms
    d.extend_from_slice(&0u32.to_le_bytes()); // junctions
    let mut attr_words: Vec<u16> = Vec::new();
    let attr = |words: &mut Vec<u16>, word: u16, data: &[u16]| {
        words.push(word);
        words.extend_from_slice(data);
    };
    attr(&mut attr_words, 0x0a << 3, &[1]); // texture ref → textures[0]
    attr(&mut attr_words, 0x00, &[2, 0, 1, 2, 3, 4, 5, 6, 7]); // counted road
    attr(&mut attr_words, 0x06 << 3, &[2, 8, 9, 10, 11]); // counted fan
    attr(&mut attr_words, (0x0b << 3) | 6, &[1, 2, 3, 2, 12, 13]); // facade
    attr(&mut attr_words, (0x07 << 3) | 4, &[0, 2, 12, 13]); // facade bound
    attr(&mut attr_words, (0x0c << 3) | 3, &[2, 14, 15, 16, 17]); // roof fan
    attr(&mut attr_words, (0x03 << 3) | 4, &[2, 0, 12, 13]); // sliver
    attr(&mut attr_words, (0x09 << 3) | 3 | 0x80, &[9, 9, 9]); // tunnel (last)
    let mut room = Vec::new();
    room.extend_from_slice(&4u32.to_le_bytes()); // nPerimeter
    room.extend_from_slice(&(attr_words.len() as u32).to_le_bytes());
    for v in [0u16, 1, 2, 3] {
        room.extend_from_slice(&v.to_le_bytes());
        room.extend_from_slice(&0u16.to_le_bytes()); // neighbour room
    }
    for w in &attr_words {
        room.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&room);
    d.extend_from_slice(&[0u8; 2]); // room flags (nRooms entries)
    d.extend_from_slice(&[0u8; 2]); // prop rules
    push_f32s(&mut d, &[-5., 0., 0.]); // bounds min
    push_f32s(&mut d, &[45., 6., 20.]); // bounds max
    push_f32s(&mut d, &[20., 3., 10.]); // bounds centre
    push_f32s(&mut d, &[30.]); // radius
    d.extend_from_slice(&0u32.to_le_bytes()); // nPaths
    d
}

/// A minimal city install: `city/test.psdl` + its road texture, plus
/// whatever `.ltNN` presets the caller writes.
fn city_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    write(d, "city/test.psdl", city_psdl());
    std::fs::create_dir_all(d.join("texture")).unwrap();
    std::fs::write(
        d.join("texture/test_road.png"),
        include_bytes!("../../../assets/texture/dev_road.png"),
    )
    .unwrap();
    tmp
}

/// A synthetic `city/<stem>_fog.csv` — the retail 16-row grammar with
/// self-authored values distinct per slot (`row i` = rgb 10+i/20+i/30+i,
/// start 100+i, end 900+i, label `slot-i`).
fn fog_table(rows: &[(u8, u8, u8, i32, i32); 16]) -> String {
    let mut s =
        String::from("fog red,fog green,fog blue,fog start,fog end,description (ignored)\n");
    for (i, (r, g, b, start, end)) in rows.iter().enumerate() {
        s.push_str(&format!("{r},{g},{b},{start},{end},slot-{i}\n"));
    }
    s
}

fn fog_rows() -> [(u8, u8, u8, i32, i32); 16] {
    let mut rows = [(0u8, 0u8, 0u8, 0i32, 0i32); 16];
    for (i, r) in rows.iter_mut().enumerate() {
        *r = (
            10 + i as u8,
            20 + i as u8,
            30 + i as u8,
            100 + i as i32,
            900 + i as i32,
        );
    }
    rows
}

/// The `DistanceFog` components on the session's 3D cameras.
fn camera_fogs(app: &mut App) -> Vec<DistanceFog> {
    app.world_mut()
        .query_filtered::<&DistanceFog, With<Camera3d>>()
        .iter(app.world())
        .cloned()
        .collect()
}

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";

/// An event overlay for the city: one checkpoint row authoring
/// `TimeofDay=1`/`Weather=2` (slot 6) at amateur difficulty.
fn write_event(d: &Path) {
    write(
        d,
        "race/test/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,1,2,0,0,0.1,0.0,1,50,1,0,0,0,0,0,0.2,0.0,1,40,1\n"),
    );
    write(d, "race/test/race0.aimap", "#\n");
    write(
        d,
        "race/test/race0waypoints.csv",
        format!("{WAYPOINTS}0,0,5,0,15,0,0,0,\n0,0,10,0,15,0,0,0,\n0,0,15,0,15,0,0,0,\n"),
    );
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

fn city_config(conditions: mm2_game::SessionConditions) -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/test.psdl".into(),
        },
        conditions,
        ..SessionConfig::default()
    }
}

/// The same session wiring `headless_smoke` runs — the real
/// `load_session_world` + lifecycle driver on a minimal headless app.
fn city_app(config: SessionConfig, vfs: Vfs) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<ResultLedger>()
        .init_resource::<SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SelectedCar {
            def: None,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                mm2_app::environment::drive_sky_dome,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

fn lights(app: &mut App) -> Vec<(DirectionalLight, Transform, SessionEntity)> {
    app.world_mut()
        .query::<(&DirectionalLight, &Transform, &SessionEntity)>()
        .iter(app.world())
        .map(|(l, t, o)| (*l, *t, *o))
        .collect()
}

/// The configured cruise conditions select `city/test.lt08` and bind
/// its authored lights: three directional lights with the authored
/// colours and the recovered direction convention, the packed ambient
/// into `GlobalAmbientLight`, and a report naming the bound preset.
#[test]
fn configured_conditions_bind_the_authored_preset() {
    let tmp = city_install();
    // 0xFF1E3C5A = r30 g60 b90 packed BGRA → i32 −14795686.
    write(
        tmp.path(),
        "city/test.lt08",
        lt_record("clear-evening", [1.0, 0.75, 0.6], -14795686),
    );
    let config = city_config(mm2_game::SessionConditions {
        time_of_day: TimeOfDay::new(2).unwrap(),
        weather: Weather::new(0).unwrap(),
    });
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 8);
    assert_eq!(report.path, "city/test.lt08");
    assert_eq!(report.name.as_deref(), Some("clear-evening"));
    assert_eq!(report.source, ConditionsSource::Configured);
    assert!(!report.fallback);
    assert_eq!(
        report.smoke_detail(),
        "lt08(clear-evening) fog=none sky=none"
    );

    // Three authored lights: the key casts shadows, the fills do not.
    let all = lights(&mut app);
    assert_eq!(all.len(), 3);
    let key = all
        .iter()
        .find(|(l, ..)| l.shadow_maps_enabled)
        .expect("the key light casts shadows");
    let key_rgb = key.0.color.to_srgba();
    assert!((key_rgb.red - 1.0).abs() < 1e-3);
    assert!((key_rgb.green - 0.75).abs() < 1e-3);
    assert!((key_rgb.blue - 0.6).abs() < 1e-3);
    // Key direction: heading 0.5 / pitch −0.2 → travel dir =
    // (cos h·cos p, sin p, sin h·cos p) ≈ (0.860, −0.199, 0.470).
    let forward = key.1.forward().as_vec3();
    let expected = Vec3::new(0.8601, -0.1987, 0.4699).normalize();
    assert!(
        forward.distance(expected) < 1e-3,
        "key forward {forward:?} ≈ {expected:?}"
    );
    let fills = all.iter().filter(|(l, ..)| !l.shadow_maps_enabled).count();
    assert_eq!(fills, 2, "both fills are shadowless");

    // The packed ambient binds verbatim: r30 g60 b90.
    let ambient = app.world().resource::<GlobalAmbientLight>();
    let a = ambient.color.to_srgba();
    assert!((a.red - 30.0 / 255.0).abs() < 1e-3);
    assert!((a.green - 60.0 / 255.0).abs() < 1e-3);
    assert!((a.blue - 90.0 / 255.0).abs() < 1e-3);
}

/// A preset the VFS cannot provide is an explicit fallback — the
/// previous fixed rig plus a report that says so — never a silent
/// default that reads as a bound preset (F18-AC06).
#[test]
fn missing_preset_reports_the_fallback() {
    let tmp = city_install(); // no .ltNN files at all
    let config = city_config(mm2_game::SessionConditions {
        time_of_day: TimeOfDay::new(1).unwrap(),
        weather: Weather::new(3).unwrap(),
    });
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 7, "tod 1 ×4 + weather 3");
    assert_eq!(report.path, "city/test.lt07");
    assert!(report.fallback);
    assert_eq!(report.name, None);
    assert_eq!(report.smoke_detail(), "lt07(fallback) fog=none sky=none");
    // No `city/test_fog.csv` either — the absence is explicit, not a
    // silent default.
    assert!(report.fog.bound.is_none());
    assert_eq!(report.fog.absent, Some("missing"));

    // The fallback rig is the pre-preset single sun + fixed ambient.
    let all = lights(&mut app);
    assert_eq!(all.len(), 1);
    let ambient = app.world().resource::<GlobalAmbientLight>();
    assert_eq!(ambient.brightness, 400.0);
    assert!(camera_fogs(&mut app).is_empty());
}

/// An authored event's conditions win over the session's configured
/// ones (RACE-2): the row authors `TimeofDay=1`/`Weather=2` → slot 6,
/// and the configured (3,3) → `lt15` — deliberately absent, so the
/// configured choice would report a fallback if it were used.
#[test]
fn authored_event_conditions_take_precedence() {
    let tmp = city_install();
    write_event(tmp.path());
    write(
        tmp.path(),
        "city/test.lt06",
        lt_record("foggy-noon", [0.5, 0.5, 0.5], -16777216),
    );
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "test".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        conditions: mm2_game::SessionConditions {
            time_of_day: TimeOfDay::new(3).unwrap(),
            weather: Weather::new(3).unwrap(),
        },
        ..city_config(mm2_game::SessionConditions::default())
    };
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    let phase = app.world().resource::<Session>().phase().clone();
    assert!(
        matches!(phase, SessionPhase::Countdown | SessionPhase::Playing),
        "the event session loads: {phase:?}"
    );

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 6, "the authored row's tod 1 ×4 + weather 2");
    assert_eq!(report.name.as_deref(), Some("foggy-noon"));
    assert_eq!(report.source, ConditionsSource::Authored);
    assert!(!report.fallback);
}

/// The player's `SessionCustomization` picks beat the authored event
/// conditions (RACE-3 — F17-A.6): the row authors `TimeofDay=1`/
/// `Weather=2` (slot 6) but the customization picks (3,3) → slot 15,
/// and the report names the customized source.
#[test]
fn customized_event_conditions_take_precedence() {
    let tmp = city_install();
    write_event(tmp.path());
    write(
        tmp.path(),
        "city/test.lt15",
        lt_record("rainy-night", [0.2, 0.2, 0.4], -1000000),
    );
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "test".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        customization: Some(SessionCustomization {
            conditions: SessionConditions {
                time_of_day: TimeOfDay::new(3).unwrap(),
                weather: Weather::new(3).unwrap(),
            },
            densities: Densities::DEFAULT,
        }),
        ..city_config(SessionConditions::default())
    };
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    let phase = app.world().resource::<Session>().phase().clone();
    assert!(
        matches!(phase, SessionPhase::Countdown | SessionPhase::Playing),
        "the event session loads: {phase:?}"
    );

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 15, "the player's tod 3 ×4 + weather 3");
    assert_eq!(report.name.as_deref(), Some("rainy-night"));
    assert_eq!(report.source, ConditionsSource::Customized);
    assert!(!report.fallback);
}

/// A preset that parses but carries off-schema fields still binds —
/// authored anomalies are validation findings, not load failures.
#[test]
fn off_schema_fields_count_as_issues_not_fallback() {
    let tmp = city_install();
    let record = lt_record("clear-morning", [0.9, 0.9, 0.8], -14803406).replace(
        "  Ambient -14803406\n",
        "  SepiaTone 1\n  Ambient -14803406\n",
    );
    write(tmp.path(), "city/test.lt00", record);
    let config = city_config(mm2_game::SessionConditions::default());
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 0);
    assert!(!report.fallback);
    assert_eq!(report.issues, 1, "the unknown SepiaTone field");
    assert_eq!(lights(&mut app).len(), 3);
}

/// The authored `city/<stem>_fog.csv` row for the session's slot binds
/// a linear `DistanceFog` on both 3D cameras — colour and clip
/// distances verbatim — and the report records the binding (F18-A.3).
#[test]
fn authored_fog_binds_onto_the_cameras() {
    let tmp = city_install();
    write(tmp.path(), "city/test_fog.csv", fog_table(&fog_rows()));
    // Configured (tod 2, weather 0) → slot 8 → row (18, 28, 38, 108, 908).
    let config = city_config(mm2_game::SessionConditions {
        time_of_day: TimeOfDay::new(2).unwrap(),
        weather: Weather::new(0).unwrap(),
    });
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 8);
    assert_eq!(report.fog.path, "city/test_fog.csv");
    let bound = report.fog.bound.expect("slot 8's row binds");
    assert_eq!(bound.color, [18.0, 28.0, 38.0]);
    assert_eq!(bound.start, 108.0);
    assert_eq!(bound.end, 908.0);
    assert_eq!(report.fog.absent, None);
    assert_eq!(report.smoke_detail(), "lt08(fallback) fog=108-908 sky=none");

    let fogs = camera_fogs(&mut app);
    assert_eq!(fogs.len(), 2, "chase + free cameras both fogged");
    for fog in &fogs {
        let FogFalloff::Linear { start, end } = fog.falloff.clone() else {
            panic!("authored distances bind as a linear falloff");
        };
        assert_eq!(start, 108.0);
        assert_eq!(end, 908.0);
        let c = fog.color.to_srgba();
        assert!((c.red - 18.0 / 255.0).abs() < 1e-3);
        assert!((c.green - 28.0 / 255.0).abs() < 1e-3);
        assert!((c.blue - 38.0 / 255.0).abs() < 1e-3);
        assert_eq!(fog.directional_light_color, Color::NONE);
    }
}

/// The fog channel follows the *effective* slot, not the configured
/// one: the authored event row (tod 1, weather 2 → slot 6) selects fog
/// row 6 even though the configured conditions point at slot 15.
#[test]
fn authored_event_conditions_select_the_fog_row() {
    let tmp = city_install();
    write_event(tmp.path());
    write(tmp.path(), "city/test_fog.csv", fog_table(&fog_rows()));
    let config = SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "test".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..city_config(SessionConditions::default())
    };
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 6, "the authored row's tod 1 ×4 + weather 2");
    let bound = report.fog.bound.expect("slot 6's row binds");
    assert_eq!((bound.start, bound.end), (106.0, 906.0));
}

/// A table whose selected row cannot interpolate (`end <= start`)
/// binds nothing and says so — the validation issue is counted, the
/// cameras stay unfogged.
#[test]
fn degenerate_fog_row_binds_nothing() {
    let tmp = city_install();
    let mut rows = fog_rows();
    rows[0] = (10, 20, 30, 500, 500); // slot 0: end == start
    write(tmp.path(), "city/test_fog.csv", fog_table(&rows));
    let config = city_config(SessionConditions::default());
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();

    let report = app.world().resource::<EnvironmentReport>();
    assert!(report.fog.bound.is_none());
    assert_eq!(report.fog.absent, Some("degenerate"));
    assert_eq!(report.fog.issues, 1, "the DegenerateBand finding");
    assert_eq!(report.smoke_detail(), "lt00(fallback) fog=none sky=none");
    assert!(camera_fogs(&mut app).is_empty());
}

// ---------------------------------------------------------------------------
// `.sky` dome (F18-A.4)
// ---------------------------------------------------------------------------

/// A PKG3 chunk: `FILE` + lp name + u32 length + payload.
fn push_chunk(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    out.extend_from_slice(b"FILE");
    push_lp(out, name);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

/// A minimal PKG3 dome: a two-triangle fan (radius 40 m, apex at
/// y = 40) plus a `shaders` chunk of `paint_jobs` float-shader paint
/// jobs naming `texture/dome_<i>`.
fn dome_pkg(paint_jobs: u8) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections
    geo.extend_from_slice(&4u32.to_le_bytes()); // total vertices
    geo.extend_from_slice(&6u32.to_le_bytes()); // total indices
    geo.extend_from_slice(&1u32.to_le_bytes()); // nSections, again
    geo.extend_from_slice(&0x112u32.to_le_bytes()); // fvf: XYZ|NORMAL|1 tex
    geo.extend_from_slice(&1u16.to_le_bytes()); // nStrips
    geo.extend_from_slice(&0u16.to_le_bytes()); // section flags
    geo.extend_from_slice(&0i32.to_le_bytes()); // shader offset → job's shader 0
    geo.extend_from_slice(&3i32.to_le_bytes()); // prim type: triangles
    geo.extend_from_slice(&4u32.to_le_bytes()); // strip vertices
    for (pos, uv) in [
        ([0.0f32, 40.0, 0.0], [0.5f32, 0.0]),
        ([-40.0, 0.0, 0.0], [0.0, 1.0]),
        ([40.0, 0.0, 0.0], [1.0, 1.0]),
        ([0.0, 0.0, 40.0], [0.5, 0.5]),
    ] {
        push_f32s(&mut geo, &pos);
        push_f32s(&mut geo, &[0.0, 1.0, 0.0]);
        push_f32s(&mut geo, &uv);
    }
    geo.extend_from_slice(&6u32.to_le_bytes()); // strip indices
    for i in [0u16, 1, 2, 0, 2, 3] {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut sh = Vec::new();
    sh.extend_from_slice(&(paint_jobs as u32).to_le_bytes()); // type: float shaders, N jobs
    sh.extend_from_slice(&1u32.to_le_bytes()); // shaders per paint job
    for i in 0..paint_jobs {
        push_lp(&mut sh, &format!("dome_{i}"));
        push_f32s(&mut sh, &[1.0; 4]); // diffuse
        push_f32s(&mut sh, &[0.2; 4]); // ambient
        push_f32s(&mut sh, &[0.0; 4]); // specular
        push_f32s(&mut sh, &[0.0; 4]); // emissive
        push_f32s(&mut sh, &[8.0]); // shininess
    }

    let mut out = b"PKG3".to_vec();
    push_chunk(&mut out, "testdome_h", &geo);
    push_chunk(&mut out, "shaders", &sh);
    out
}

/// A synthetic `.sky` + dome pkg for `city/test`: model `testdome`,
/// `paint_jobs` paint jobs whose textures are `texture/dome_<i>.png`
/// (`textures = false` synthesizes the missing-texture path).
fn write_dome(d: &Path, paint_jobs: u8, textures: bool) {
    write(d, "city/test.sky", "testdome 10 0.9 0.25\n");
    write(d, "geometry/testdome.pkg", dome_pkg(paint_jobs));
    if textures {
        for i in 0..paint_jobs {
            write(
                d,
                &format!("texture/dome_{i}.png"),
                include_bytes!("../../../assets/texture/dev_road.png"),
            );
        }
    }
}

/// The session's dome root — `(entity, angle, rate, y, transform)` —
/// `None` when no dome spawned.
fn the_dome(app: &mut App) -> Option<(Entity, f32, f32, f32, Transform)> {
    app.world_mut()
        .query::<(Entity, &SkyDome, &Transform)>()
        .iter(app.world())
        .map(|(e, d, t)| (e, d.angle, d.rotation_rate, d.y, *t))
        .next()
}

/// A `.sky` whose model resolves binds a session-owned dome: the
/// authored transform fields land on `SkyDome`/its scale, the slot's
/// paint job selects the texture (slot 7 → job `7 % 2` = `dome_1`),
/// the material is unlit/fog-exempt/double-sided, and the report names
/// the binding.
#[test]
fn authored_sky_binds_a_session_dome() {
    let tmp = city_install();
    write_dome(tmp.path(), 2, true);
    let config = city_config(mm2_game::SessionConditions {
        time_of_day: TimeOfDay::new(1).unwrap(),
        weather: Weather::new(3).unwrap(),
    });
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().phase(),
        SessionPhase::Playing
    ));

    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.slot, 7);
    assert_eq!(report.sky.path, "city/test.sky");
    assert_eq!(report.sky.model.as_deref(), Some("testdome"));
    assert_eq!(report.sky.paint, Some(1));
    assert_eq!(report.sky.texture.as_deref(), Some("dome_1"));
    assert_eq!(report.sky.absent, None);
    assert_eq!(
        report.smoke_detail(),
        "lt07(fallback) fog=none sky=testdome:dome_1"
    );

    // One session-stamped dome root: authored height/rotation on
    // `SkyDome`, authored radius 40 → designed 900 m scale, Y squashed
    // ×0.9.
    let (dome_ent, _angle, rate, dome_y, xf) = the_dome(&mut app).expect("a dome spawned");
    assert_eq!(rate, 0.25);
    assert_eq!(dome_y, 10.0);
    assert!((xf.scale.x - 22.5).abs() < 1e-3, "900 / authored-40");
    assert!((xf.scale.y - 20.25).abs() < 1e-3, "squashed ×0.9");
    assert!(
        app.world().get::<SessionEntity>(dome_ent).is_some(),
        "session-stamped for teardown"
    );

    // The render child carries the painted material — unlit,
    // fog-exempt, double-sided — not a default StandardMaterial.
    let mut parts = app
        .world_mut()
        .query::<(&ChildOf, &MeshMaterial3d<StandardMaterial>)>();
    let mats: Vec<Handle<StandardMaterial>> = parts
        .iter(app.world())
        .filter(|(c, _)| c.parent() == dome_ent)
        .map(|(_, m)| m.0.clone())
        .collect();
    assert_eq!(mats.len(), 1);
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let mat = materials.get(&mats[0]).unwrap();
    assert!(mat.unlit);
    assert!(!mat.fog_enabled);
    assert!(mat.cull_mode.is_none());
    assert!(mat.base_color_texture.is_some(), "dome_1 bound");
}

/// `drive_sky_dome` re-centres the dome on the active camera and
/// advances the authored rotation — world Y stays `HatYOffset`. The
/// read is the propagated `GlobalTransform`: a moved `Transform` lands
/// in the dome's focus one update later.
#[test]
fn dome_follows_the_active_camera() {
    let tmp = city_install();
    write_dome(tmp.path(), 16, true);
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update(); // session loads, dome spawns
    let angle0 = the_dome(&mut app).expect("a dome spawned").1;

    let mut cams = app.world_mut().query::<(&Camera, &mut Transform)>();
    for (c, mut t) in cams.iter_mut(app.world_mut()) {
        if c.is_active {
            t.translation = Vec3::new(120.0, 40.0, -75.0);
        }
    }
    app.update(); // PostUpdate propagates the moved pose
    app.update(); // the dome follows it

    let (_e, angle, rate, _y, xf) = the_dome(&mut app).expect("a dome spawned");
    assert_eq!(xf.translation, Vec3::new(120.0, 10.0, -75.0));
    assert!(
        (angle - (angle0 + 2.0 * rate / 60.0)).abs() < 1e-4,
        "two more 60 Hz steps of the authored rate, got {angle}"
    );
    assert_eq!(xf.rotation, Quat::from_rotation_y(angle));
}

/// A camera *parented* to the vehicle — the authored cockpit camera's
/// shape — still re-centres the dome on its world pose. The child's
/// `Transform` is the car-space eye offset, so a `Transform` read
/// would park the dome near the world origin however far the car
/// drove (the F22-B.1 review finding).
#[test]
fn dome_follows_a_vehicle_child_camera() {
    let tmp = city_install();
    write_dome(tmp.path(), 16, true);
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update(); // session loads, dome spawns

    // The session's own cameras step aside — the test camera is the
    // only active view, parented to a vehicle stand-in parked far
    // from the origin.
    let mut cams = app.world_mut().query::<&mut Camera>();
    for mut c in cams.iter_mut(app.world_mut()) {
        c.is_active = false;
    }
    let vehicle = app
        .world_mut()
        .spawn(Transform::from_translation(Vec3::new(120.0, 5.0, -75.0)))
        .id();
    let cam = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: true,
                ..default()
            },
            Transform::from_translation(Vec3::new(0.0, 1.2, -0.5)),
        ))
        .id();
    app.world_mut().entity_mut(vehicle).add_child(cam);
    app.update(); // PostUpdate propagates the child's world pose
    app.update(); // `drive_sky_dome` reads it

    let (_e, _angle, _rate, _y, xf) = the_dome(&mut app).expect("a dome spawned");
    // World eye pose (120, 6.2, -75.5) → dome x/z recentered, y stays
    // the authored `HatYOffset`; the car-local read would have parked
    // it at (0, 10, -0.5).
    assert_eq!(xf.translation, Vec3::new(120.0, 10.0, -75.5));
}

/// A city without a `.sky` spawns no dome and says so — never a
/// fabricated backdrop.
#[test]
fn a_city_without_sky_reports_absent() {
    let tmp = city_install();
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.path, "city/test.sky");
    assert_eq!(report.sky.model, None);
    assert_eq!(report.sky.absent, Some("missing"));
    assert!(report.smoke_detail().ends_with(" sky=none"));
    assert!(the_dome(&mut app).is_none());
}

/// A `.sky` outside the four-token grammar is unparseable — reported,
/// never a default dome.
#[test]
fn an_unparseable_sky_reports_absent() {
    let tmp = city_install();
    write(tmp.path(), "city/test.sky", "dome 0 0.9\n");
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.absent, Some("unparseable"));
    assert!(report.smoke_detail().ends_with(" sky=none"));
}

/// A `.sky` naming a model the VFS cannot provide is absent, not a
/// substitute.
#[test]
fn an_unresolvable_dome_model_reports_absent() {
    let tmp = city_install();
    write(tmp.path(), "city/test.sky", "nodome 0 0.9 0.25\n");
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.model.as_deref(), Some("nodome"));
    assert_eq!(report.sky.absent, Some("model unavailable"));
    assert!(the_dome(&mut app).is_none());
}

/// A dome authoring fewer paint jobs than the 16-slot grid wraps the
/// slot — a one-job dome always draws job 0 whatever the weather.
#[test]
fn a_dome_with_fewer_paint_jobs_wraps_the_slot() {
    let tmp = city_install();
    write_dome(tmp.path(), 1, true);
    let config = city_config(mm2_game::SessionConditions {
        time_of_day: TimeOfDay::new(3).unwrap(),
        weather: Weather::new(2).unwrap(),
    }); // slot 14 → paint 14 % 1 = 0
    let mut app = city_app(config, vfs_of(tmp.path()));
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.paint, Some(0));
    assert_eq!(report.sky.texture.as_deref(), Some("dome_0"));
    assert_eq!(report.sky.absent, None);
}

/// A dome texture the VFS cannot resolve draws the shared fallback
/// material — warned — rather than vanishing.
#[test]
fn a_missing_dome_texture_draws_the_fallback() {
    let tmp = city_install();
    write_dome(tmp.path(), 1, false);
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.absent, None);
    assert_eq!(report.sky.texture.as_deref(), Some("dome_0"));
    let (dome_ent, ..) = the_dome(&mut app).expect("the dome still spawns");
    let mut parts = app
        .world_mut()
        .query::<(&ChildOf, &MeshMaterial3d<StandardMaterial>)>();
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let mat = parts
        .iter(app.world())
        .filter(|(c, _)| c.parent() == dome_ent)
        .filter_map(|(_, m)| materials.get(&m.0))
        .next()
        .expect("dome material");
    assert!(mat.unlit);
    assert!(mat.base_color_texture.is_none(), "shared fallback");
}

/// A non-finite `.sky` transform field is counted and degenerate — no
/// dome.
#[test]
fn a_degenerate_sky_reports_absent() {
    let tmp = city_install();
    write(tmp.path(), "city/test.sky", "testdome 0 nan 0.25\n");
    write(tmp.path(), "geometry/testdome.pkg", dome_pkg(1));
    let mut app = city_app(
        city_config(SessionConditions::default()),
        vfs_of(tmp.path()),
    );
    app.update();
    let report = app.world().resource::<EnvironmentReport>();
    assert_eq!(report.sky.absent, Some("degenerate"));
    assert_eq!(report.sky.issues, 1);
    assert!(the_dome(&mut app).is_none());
}
