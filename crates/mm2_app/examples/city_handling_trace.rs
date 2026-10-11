//! Headless original-input replay through the production city and vehicle loaders.
//! city_handling_trace <install> <route.json> <output.csv>
//! Route frames are actual 60 Hz ticks; CSV frame0 is after tick1.
mod city_pose_diagnostic;
use avian3d::prelude::*;
use bevy::{
    audio::{AddAudioSource, AudioSinkPlayback},
    prelude::*,
    time::TimeUpdateStrategy,
};
use mm2_app::audio::{
    self, AudioReport, PcmAudio, SurfaceAudio, SurfaceContact, SurfaceVoice, WaveBank,
};
use mm2_assets::{InstallMount, Vfs, mount_install};
use mm2_game::{Mm2Vfs, PlayerVehicle, Session, SessionEntity, Weather};
use mm2_vehicle::player_input::quantize_steering;
use mm2_vehicle::{HumanDriver, VehicleInput, VehiclePlugin, VehicleState, vehicle_bundle};
use serde::Deserialize;
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Route {
    car: String,
    city: String,
    initial_model_pose: [f32; 4],
    frames: usize,
    #[serde(default = "settlement")]
    settle_frames: usize,
    schedule: Vec<(usize, Vec<(u32, bool)>)>,
    /// Original's room identity is provenance only: native collision uses geometry.
    #[serde(alias = "original_room")]
    initial_room: u16,
}
fn settlement() -> usize {
    600
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let install = args.get(1).expect("installation path");
    let route_path = args.get(2).expect("shared route JSON");
    let output = args.get(3).expect("output CSV path");
    let route_bytes = std::fs::read(route_path).expect("read route");
    let route: Route = serde_json::from_slice(&route_bytes).expect("parse route");
    let plan_path = std::env::var("MM2_PLAN").ok();
    let plan = plan_path.as_ref().map(|path| {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
    });
    let realtime_audio = std::env::var("MM2_REALTIME_AUDIO").is_ok_and(|v| v == "1");
    let exact_initial_yaw = std::env::var("MM2_EXACT_INITIAL_YAW").is_ok_and(|v| v == "1");
    let diagnostic_substeps = std::env::var("MM2_DIAGNOSTIC_SUBSTEPS").is_ok_and(|v| v == "1");
    let diagnostic_pose = std::env::var("MM2_DIAGNOSTIC_POSE").ok();
    let settled_source_path = std::env::var("MM2_DIAGNOSTIC_SETTLED_SOURCE").ok();
    let settled_source = settled_source_path
        .as_ref()
        .map(|path| validated_settled_source(path, &route, route_path, &route_bytes));
    let engine_source_path = std::env::var("MM2_DIAGNOSTIC_ENGINE_SOURCE").ok();
    let engine_reference_path = std::env::var("MM2_DIAGNOSTIC_ENGINE_REFERENCE")
        .ok()
        .or_else(|| settled_source_path.clone());
    let engine_source = engine_source_path.as_ref().map(|path| {
        validated_engine_source(
            path,
            engine_reference_path.as_deref().expect(
                "engine snapshot requires MM2_DIAGNOSTIC_ENGINE_REFERENCE or settled source",
            ),
            &route,
            route_path,
            &route_bytes,
        )
    });
    let measured_path = std::env::var("MM2_MEASURED_STEERING").ok();
    let measured_steering = measured_path.as_ref().map(|path| {
        assert!(
            std::env::var_os("MM2_PLAN").is_none(),
            "measured steering cannot combine with route planning"
        );
        let source: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read measured steering source"))
                .expect("parse measured steering source");
        assert_eq!(source["car"], route.car, "measured source car mismatch");
        assert_eq!(source["city"], route.city, "measured source city mismatch");
        assert_eq!(
            source["car_identity_verified"], true,
            "measured source car identity unverified"
        );
        assert_eq!(source["hz"], 60, "measured source must be60Hz");
        assert_eq!(source["complete"], true, "measured source incomplete");
        assert_eq!(
            source["route"],
            serde_json::from_slice::<serde_json::Value>(&route_bytes).unwrap(),
            "measured source route differs"
        );
        let hash_output = std::process::Command::new("shasum")
            .args(["-a", "256"])
            .arg(route_path)
            .output()
            .expect("diagnostic route hash requires shasum");
        assert!(hash_output.status.success(), "route hash failed");
        let hash_text = String::from_utf8(hash_output.stdout).unwrap();
        let hash = hash_text.split_whitespace().next().unwrap();
        assert_eq!(
            source["route_sha256"], hash,
            "measured source route SHA256 mismatch"
        );
        let rows = source["rows"].as_array().expect("source rows");
        assert_eq!(
            rows.len(),
            route.frames + 1,
            "one source pre-input row plus each tick required"
        );
        rows.iter()
            .enumerate()
            .map(|(n, row)| {
                assert_eq!(row["n"].as_u64(), Some(n as u64), "source row order");
                let value = row["steer"].as_f64().expect("source steering") as f32;
                assert!(
                    value.is_finite() && (-1.0..=1.0).contains(&value),
                    "invalid measured steering"
                );
                assert!(
                    (value * 127.0 - (value * 127.0).round()).abs() < 0.0001,
                    "source steering must be signed-byte quantized"
                );
                (value * 127.0).round() * (1.0 / 127.0)
            })
            .collect::<Vec<_>>()
    });
    assert!(
        matches!(route.city.as_str(), "sf" | "london"),
        "city must be sf or london"
    );
    assert!(
        (1..=36000).contains(&route.frames),
        "frames must be 1..36000"
    );
    assert!(
        (180..=36000).contains(&route.settle_frames),
        "settlement must be 180..36000"
    );
    assert!(
        route.initial_model_pose.iter().all(|v| v.is_finite()),
        "finite initial model pose required"
    );
    let mut last = None;
    for (frame, changes) in &route.schedule {
        assert!(*frame < route.frames, "event beyond final tick");
        assert!(
            last.is_none_or(|previous| *frame > previous),
            "event frames must be strictly increasing"
        );
        assert!(
            changes
                .iter()
                .all(|(key, _)| matches!(key, 32 | 37 | 38 | 39 | 40)),
            "only space and arrow virtual keys supported"
        );
        last = Some(*frame);
    }
    let mut vfs = Vfs::new();
    mount_install(&mut vfs, install.as_ref(), &InstallMount::default()).expect("mount retail");
    let cfg = mm2_content::load_vehicle(&vfs, &route.car, 0)
        .expect("load requested car")
        .config;
    assert!(cfg.original.is_some(), "route requires original handling");
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(TransformPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 19.6))
        .add_plugins(VehiclePlugin)
        .add_plugins(audio::NativeSkidTimingPlugin);
    if diagnostic_substeps {
        app.insert_resource(SubstepCount(1));
    }
    if let Some(mode) = diagnostic_pose.as_deref() {
        assert!(
            plan.is_none(),
            "pose diagnostic cannot combine with planning"
        );
        city_pose_diagnostic::install(&mut app, mode);
    }
    if realtime_audio {
        app.add_plugins(bevy::audio::AudioPlugin::default())
            .add_audio_source::<PcmAudio>();
    }
    app.finish();
    app.cleanup();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let mut session = Session::new();
    let loaded = {
        let mut commands = Commands::new(&mut queue, app.world());
        mm2_app::city::load_city(
            &mut commands,
            &vfs,
            &format!("city/{}.psdl", route.city),
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
            &mut session,
        )
        .expect("production city load")
    };
    queue.apply(app.world_mut());
    app.insert_resource(meshes)
        .insert_resource(images)
        .insert_resource(materials);
    if let Some(tables) = mm2_content::load_surface_tables(&vfs).expect("surface tables") {
        app.insert_resource(tables);
    }
    if let Some(audio) = SurfaceAudio::load(&vfs, Weather::default(), Some(&route.car)) {
        app.insert_resource(audio);
    }
    app.insert_resource(WaveBank::index(&vfs))
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session)
        .insert_resource(Assets::<PcmAudio>::default())
        .insert_resource(AudioReport::default())
        .add_systems(Update, audio::surface_voices)
        .add_systems(
            PostUpdate,
            (audio::level_new_voices, audio::sync_audio_pause)
                .chain()
                .before(bevy::transform::TransformSystems::Propagate),
        );
    if realtime_audio {
        app.add_systems(Update, audio::count_sinks);
    }
    let [x, y, z, yaw] = route.initial_model_pose;
    let rotation = if exact_initial_yaw {
        // Diagnostic matches the recorder's JS double half-angle, avoiding
        // f32 pi's tiny horizontal bias at authored 180-degree headings.
        let (sin, cos) = (f64::from(yaw).to_radians() * 0.5).sin_cos();
        Quat::from_xyzw(0.0, sin as f32, 0.0, cos as f32)
    } else {
        Quat::from_rotation_y(yaw.to_radians())
    };
    let initial_position = Vec3::new(x, y, z);
    let car = app
        .world_mut()
        .spawn((
            vehicle_bundle(&cfg),
            HumanDriver::default(),
            PlayerVehicle,
            Position(initial_position),
            Rotation(rotation),
            Transform::from_translation(initial_position).with_rotation(rotation),
        ))
        .id();
    app.world_mut()
        .get_mut::<VehicleInput>(car)
        .unwrap()
        .handbrake = 1.0;
    let mut pacing = realtime_audio.then(|| (Instant::now(), 0_u64));
    // Prime the clock; no physics step occurs on this update.
    update_app(&mut app, &mut pacing);
    for _ in 0..route.settle_frames {
        update_app(&mut app, &mut pacing);
    }
    if let Some(source) = settled_source.as_ref() {
        seed_settled_source(app.world_mut(), car, &cfg, &source["rows"][0]);
    }
    let engine_history = engine_source
        .as_ref()
        .map(|source| seed_engine_source(app.world_mut(), car, &source["rows"][0]));
    let com_offset = Vec3::from(cfg.center_of_mass);
    let start_model = app.world().get::<Position>(car).unwrap().0;
    let start_rotation = app.world().get::<Rotation>(car).unwrap().0;
    let start = start_model + start_rotation * com_offset;
    let provenance = serde_json::json!({
        "car":route.car,"city":route.city,"route":serde_json::from_slice::<serde_json::Value>(&route_bytes).unwrap(),
        "hz":60,"settle_frames":route.settle_frames,"settle_handbrake":1,
        "diagnostic_waypoint_plan":plan_path.as_ref().map(|path|serde_json::json!({"source":path,"plan":plan,"ordinary_frozen_keyboard_replay":false})),
        "diagnostic_measured_steering":measured_path.as_ref().map(|path|serde_json::json!({"source":path,"alignment":"original row frame+1 signed byte reconstructed with retail f32 reciprocal; override only final VehicleInput.steering after normal keyboard filter","validation":"car identity,city,route equality,route SHA256,60Hz,completeness and byte quantization"})),
        "initial_pose_convention":"model origin, yaw degrees around positive Y",
        "diagnostic_initial_yaw":exact_initial_yaw,
        "diagnostic_settled_source":settled_source_path.as_ref().map(|path|serde_json::json!({
            "source":path,"partial_state":true,"ordinary_native_spawn":false,
            "restored":"source n0 COM pose/basis through normalized quaternion, velocities, wheel compression/bristles/reaction/spin, pending force/torque, impulses/push, reconstructed suspension A/B/C",
            "retained_native":"full powertrain including engine RPM/clutch/time-in-gear, drive bias, player/steering speed caches, native collision entity references, skid class history",
            "derived":"world angular momentum from normalized pose inertia times source omega; individual wheel omega from (vFwd-vSlip)/radius; drive omega negated source spin",
            "limitations":"Mat3-to-Quat normalization cannot retain original raw basis drift; source A/B/C reconstructed from captured D/n/point; source hidden drivetrain state unavailable"
        })),
        "diagnostic_engine_source":engine_source_path.as_ref().map(|path|serde_json::json!({
            "source":path,"reference":engine_reference_path,"partial_state":true,
            "capture_kind":"route-start-powertrain-snapshot","ordinary_native_spawn":false,
            "validation":"exact n0/n1 physical state match to complete verified reference; same full route/hash/identity; settled inactive shift and detached clutch",
            "restored":"raw actual engine omega, displayed RPM, torque, transmission time-in-gear",
            "retained_native":"inactive private shift timer/rpmAtShift; remaining drivetrain, body, wheel and input state",
            "before_after":engine_history
        })),
        "diagnostic_substeps":diagnostic_substeps.then_some(1),
        "diagnostic_pose":diagnostic_pose.as_ref().map(|mode|serde_json::json!({"mode":mode,"ordinary_frozen_keyboard_replay":false,"description":"single full-step COM accumulation and/or normalized quaternion orientation endpoint; retains six contact solver substeps"})),
        "actualsubsteps":app.world().resource::<SubstepCount>().0,
        "settled_model_position":start_model.to_array(),"settled_com":start.to_array(),
        "settled_velocity":app.world().get::<LinearVelocity>(car).unwrap().0.to_array(),
        "settled_angular_velocity":app.world().get::<AngularVelocity>(car).unwrap().0.to_array(),
        "settled_grounded_wheels":app.world().get::<VehicleState>(car).unwrap().wheels.iter().filter(|w|w.grounded).count(),
        "settled_right":(start_rotation*Vec3::X).to_array(),"settled_up":(start_rotation*Vec3::Y).to_array(),
        "settled_back":(start_rotation*Vec3::Z).to_array(),
        "initial_room":route.initial_room,"vehicle_config":cfg,
        "rooms":loaded.report.rooms,"props_spawned":loaded.report.props_spawned,"props_failed":loaded.report.props_failed,
        "systems":"production city::load_city and VehiclePlugin; no alternate collision rules",
        "limitations":["no traffic or pedestrian actors","INST props loaded; app dormant-prop activation and breakaway systems are absent",if realtime_audio { "no rendering or recovery systems; live audio sink state recorded, device output not recorded" } else { "no rendering, audio backend playback or recovery systems; requested surface audio metadata recorded" }],
        "alignment":"original n0 is settled pre-input; native frame0 is original n1 post-step",
    });
    std::fs::write(
        format!("{output}.provenance.json"),
        serde_json::to_vec_pretty(&provenance).unwrap(),
    )
    .unwrap();
    let mut out = BufWriter::new(File::create(output).expect("create trace"));
    write_header(&mut out).unwrap();
    let ramp = cfg.player_steering.unwrap_or_default();
    let mut steering = 0.0;
    let mut held = [false; 5];
    let mut event = 0;
    let mut waypoint = 1usize;
    let mut frozen = Vec::new();
    let mut contact_rows = Vec::new();
    let mut audio_rows = Vec::new();
    for frame in 0..route.frames {
        if let Some((at, changes)) = route.schedule.get(event)
            && *at == frame
        {
            for (key, pressed) in changes {
                held[key_index(*key)] = *pressed;
            }
            event += 1;
        }
        if let Some(plan) = &plan {
            let points = plan["points"].as_array().unwrap();
            let p = app.world().get::<Position>(car).unwrap().0;
            let r = app.world().get::<Rotation>(car).unwrap().0;
            let velocity = app.world().get::<LinearVelocity>(car).unwrap().0;
            let omega = app.world().get::<AngularVelocity>(car).unwrap().0.y;
            let point = |i: usize| {
                Vec2::new(
                    points[i][0].as_f64().unwrap() as f32,
                    points[i][1].as_f64().unwrap() as f32,
                )
            };
            let here = Vec2::new(p.x, p.z);
            while waypoint + 1 < points.len() && here.distance(point(waypoint)) < 12.0 {
                waypoint += 1;
            }
            let previous = point(waypoint - 1);
            let next = point(waypoint);
            let segment = next - previous;
            let progress =
                ((here - previous).dot(segment) / segment.length_squared()).clamp(0.0, 1.0);
            let lookahead = (8.0 + velocity.length() * 0.5).min(19.0);
            let mut along = progress * segment.length() + lookahead;
            let mut at = waypoint;
            let mut start_point = previous;
            while along > (point(at) - start_point).length() && at + 1 < points.len() {
                along -= (point(at) - start_point).length();
                start_point = point(at);
                at += 1;
            }
            let aim = start_point + (point(at) - start_point).normalize() * along;
            let delta = aim - here;
            let back = r * Vec3::Z;
            let yaw = back.x.atan2(back.z);
            let desired = (-delta.x).atan2(-delta.y);
            let error = (desired - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let command = error - omega * 0.38;
            let target_speed = plan["speed"].as_f64().unwrap() as f32;
            let slow = target_speed / (1.0 + error.abs() * 1.5);
            let speed = velocity.length();
            let planned_hb = plan["handbrake"].as_array().is_some_and(|period| {
                frame >= period[0].as_u64().unwrap() as usize
                    && frame < period[1].as_u64().unwrap() as usize
            });
            let planned = [
                planned_hb,
                command > 0.035,
                speed < slow - 0.5,
                command < -0.035,
                speed > slow + 1.0,
            ];
            let mut changes = Vec::new();
            for i in 0..5 {
                if held[i] != planned[i] {
                    changes.push(([32, 37, 38, 39, 40][i], planned[i]));
                }
            }
            if !changes.is_empty() {
                frozen.push((frame, changes));
            }
            held = planned;
        }
        let target = f32::from(u8::from(held[3])) - f32::from(u8::from(held[1]));
        let speed = app
            .world()
            .get::<VehicleState>(car)
            .unwrap()
            .human_steering_speed();
        let (next, filtered) = ramp.discrete_step(steering, target, speed, 1.0 / 60.0);
        steering = next;
        let mut input = VehicleInput {
            throttle: f32::from(u8::from(held[2])),
            brake: f32::from(u8::from(held[4])),
            steering: quantize_steering(filtered),
            handbrake: f32::from(u8::from(held[0])),
            ..default()
        };
        if let Some(measured) = &measured_steering {
            input.steering = measured[frame + 1];
        }
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = input;
        update_app(&mut app, &mut pacing);
        write_row(&mut out, app.world(), car, frame, start, com_offset, input).unwrap();
        audio_rows.push(sample_audio(app.world_mut(), car, frame, realtime_audio));
        for pair in app.world().resource::<ContactGraph>().active_pairs() {
            if pair.body1 != Some(car) && pair.body2 != Some(car) {
                continue;
            }
            let other = if pair.body1 == Some(car) {
                pair.collider2
            } else {
                pair.collider1
            };
            let points:Vec<_>=pair.manifolds.iter().flat_map(|m|m.points.iter().map(move |p|serde_json::json!({
                "penetration":p.penetration,"normal_speed":p.normal_speed,"normal_impulse":p.normal_impulse,
                "point":p.point.to_array(),"normal":m.normal.to_array()
            }))).collect();
            if !points.is_empty() {
                contact_rows.push(serde_json::json!({"frame":frame,"entity":other.to_bits(),
                    "name":app.world().get::<Name>(other).map(Name::as_str),
                    "banger":app.world().get::<mm2_game::banger::Banger>(other).map(|v|format!("{:?}",v.phase)),
                    "surface_material":app.world().get::<mm2_game::SurfaceMaterial>(other).map(|v|format!("{v:?}")),
                    "tire_surface":app.world().get::<mm2_vehicle::surface::TireSurface>(other).map(|v|format!("{v:?}")),
                    "original_contact_material":app.world().get::<mm2_vehicle::surface::OriginalContactMaterial>(other).map(|v|format!("{v:?}")),
                    "points":points}));
            }
        }
    }
    out.flush().unwrap();
    std::fs::write(
        format!("{output}.contacts.json"),
        serde_json::to_vec(&contact_rows).unwrap(),
    )
    .unwrap();
    let backend_sink_observed = audio_rows.iter().any(|row| {
        row["voices"]
            .as_array()
            .is_some_and(|voices| voices.iter().any(|voice| voice["sink"].is_object()))
    });
    let requested_audio = audio_rows.iter().any(|row| {
        row["voices"].as_array().is_some_and(|voices| {
            voices
                .iter()
                .any(|voice| voice["mix_volume"].as_f64().is_some_and(|gain| gain > 0.0))
        })
    });
    std::fs::write(
        format!("{output}.audio.json"),
        serde_json::to_vec(&serde_json::json!({
            "semantics":if realtime_audio { "production requested surface mix and attached AudioSink state; default output device enabled; device output not recorded" } else { "requested production surface audio selection/mix and spawned playback settings; AudioPlugin omitted, no output device or sink/backend playback measured" },
            "systems":"NativeSkidTimingPlugin snapshot before physics First; surface_voices Update; level_new_voices then sync_audio_pause PostUpdate before propagation/audio queue; sampled after app.update",
            "surface_table":app.world().get_resource::<SurfaceAudio>().map(|v|v.path.as_str()),
            "wave_bank":"production VFS index and bounded PcmAudio decode",
            "mix_semantics":if realtime_audio { "SurfaceVoice mix volume is requested linear amplitude after native DirectSound attenuation conversion; attached sink records current backend gain/speed/pause/empty state" } else { "SurfaceVoice mix volume is requested linear amplitude after native DirectSound attenuation conversion; PlaybackSettings describe creation/bus scaling, with no sink updates" },
            "mode":if realtime_audio { "real_time_audio_sink_verification" } else { "headless_requested_mix" },
            "device_output_recorded":false,
            "backend_sink_observed":backend_sink_observed,
            "frames":audio_rows
        })).unwrap(),
    ).unwrap();
    assert!(
        !realtime_audio || !requested_audio || backend_sink_observed,
        "real-time audio requested voices but no output sink attached; saved metadata is not backend verification (check the default audio device)"
    );
    if plan.is_some() {
        let mut frozen_route: serde_json::Value = serde_json::from_slice(&route_bytes).unwrap();
        frozen_route["schedule"] = serde_json::to_value(frozen).unwrap();
        std::fs::write(
            format!("{output}.route.json"),
            serde_json::to_vec_pretty(&frozen_route).unwrap(),
        )
        .unwrap();
    }
    eprintln!(
        "Recorded {} ticks in {} using {}; settled COM {:?}; provenance {}.provenance.json",
        route.frames, route.city, route.car, start, output
    );
}
fn key_index(key: u32) -> usize {
    match key {
        32 => 0,
        37 => 1,
        38 => 2,
        39 => 3,
        40 => 4,
        _ => unreachable!(),
    }
}

fn write_header(out: &mut impl Write) -> std::io::Result<()> {
    write!(
        out,
        "frame,time,x,y,z,vx,vy,vz,yaw,yaw_rate,speed,rpm,gear,steer,grounded,original_gear,shaft_spin,throttle,brake,handbrake,up_x,up_y,up_z,right_x,right_y,right_z,omega_x,omega_z,last_push_x,last_push_y,last_push_z"
    )?;
    for i in 0..4 {
        write!(
            out,
            ",wheel{i}_grounded,wheel{i}_x,wheel{i}_load,wheel{i}_f_lat,wheel{i}_f_long,wheel{i}_bristle_lat,wheel{i}_bristle_long,wheel{i}_omega"
        )?;
    }
    write!(
        out,
        ",force_x,force_y,force_z,torque_x,torque_y,torque_z,impulse_x,impulse_y,impulse_z,angular_impulse_x,angular_impulse_y,angular_impulse_z,applied_push_x,applied_push_y,applied_push_z,com_x,com_y,com_z,model_x,model_y,model_z,back_x,back_y,back_z,body_contact_count,body_contact_depth,body_contact_speed,body_contact_impulse,body_contact_normal_x,body_contact_normal_y,body_contact_normal_z,body_contact_point_x,body_contact_point_y,body_contact_point_z"
    )?;
    for i in 0..4 {
        write!(
            out,
            ",wheel{i}_slip_visual,wheel{i}_contact_x,wheel{i}_contact_y,wheel{i}_contact_z,wheel{i}_normal_x,wheel{i}_normal_y,wheel{i}_normal_z,wheel{i}_vel_lat,wheel{i}_vel_long,wheel{i}_surface_grip,wheel{i}_surface_drag"
        )?;
    }
    for matrix in ["implicit_a", "implicit_b", "implicit_c"] {
        for i in 0..9 {
            write!(out, ",{matrix}_{i}")?;
        }
    }
    writeln!(out)
}
fn write_row(
    out: &mut impl Write,
    world: &World,
    car: Entity,
    frame: usize,
    start: Vec3,
    com_offset: Vec3,
    input: VehicleInput,
) -> std::io::Result<()> {
    let model = world.get::<Position>(car).unwrap().0;
    let r = world.get::<Rotation>(car).unwrap().0;
    let com = model + r * com_offset;
    let p = com - start;
    let v = world.get::<LinearVelocity>(car).unwrap().0;
    let w = world.get::<AngularVelocity>(car).unwrap().0;
    let back = r * Vec3::Z;
    let up = r * Vec3::Y;
    let right = r * Vec3::X;
    let st = world.get::<VehicleState>(car).unwrap();
    let os = st.original.as_ref().unwrap();
    assert!(
        st.rpm.is_finite()
            && os.drive_omega.is_finite()
            && os.pending_force.is_finite()
            && os.pending_torque.is_finite(),
        "nonfinite powertrain/force at tick {frame}"
    );
    assert!(
        com.is_finite() && v.is_finite() && w.is_finite(),
        "nonfinite state at tick {frame}"
    );
    write!(
        out,
        "{frame},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.3},{},{:.6},{},{},{:.6},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
        frame as f64 / 60.0,
        p.x,
        p.y,
        p.z,
        v.x,
        v.y,
        v.z,
        back.x.atan2(back.z),
        w.y,
        st.forward_speed,
        st.rpm,
        st.gear,
        input.steering,
        st.wheels.iter().filter(|a| a.grounded).count(),
        os.powertrain.gear,
        os.drive_omega,
        input.throttle,
        input.brake,
        input.handbrake,
        up.x,
        up.y,
        up.z,
        right.x,
        right.y,
        right.z,
        w.x,
        w.z,
        os.last_push.x,
        os.last_push.y,
        os.last_push.z
    )?;
    for (a, b) in st.wheels.iter().zip(&os.wheels) {
        write!(
            out,
            ",{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
            u8::from(a.grounded),
            b.x,
            a.suspension_force,
            a.lateral_force,
            a.longitudinal_force,
            b.bristles.lat,
            b.bristles.long,
            b.omega
        )?;
    }
    for value in [
        os.pending_force,
        os.pending_torque,
        os.collision_impulse,
        os.collision_angular_impulse,
        os.applied_push,
        com,
        model,
        back,
    ] {
        for x in value.to_array() {
            write!(out, ",{x:.6}")?;
        }
    }
    let mut count = 0;
    let mut depth = 0.0f32;
    let mut speed = 0.0f32;
    let mut impulse = 0.0f32;
    let mut contact_normal = Vec3::ZERO;
    let mut contact_point = Vec3::ZERO;
    for pair in world.resource::<ContactGraph>().active_pairs() {
        if pair.body1 != Some(car) && pair.body2 != Some(car) {
            continue;
        }
        for (manifold, point) in pair
            .manifolds
            .iter()
            .flat_map(|m| m.points.iter().map(move |p| (m, p)))
        {
            if point.penetration >= 0.0 {
                count += 1;
                if point.penetration >= depth {
                    depth = point.penetration;
                    contact_normal = if pair.body1 == Some(car) {
                        -manifold.normal
                    } else {
                        manifold.normal
                    };
                    contact_point = point.point;
                }
                speed = speed.max(-point.normal_speed);
                impulse += point.normal_impulse;
            }
        }
    }
    write!(out, ",{count},{depth:.6},{speed:.6},{impulse:.6}")?;
    for value in [contact_normal, contact_point] {
        for x in value.to_array() {
            write!(out, ",{x:.6}")?;
        }
    }
    for wheel in &st.wheels {
        write!(out, ",{:.6}", wheel.slip_visual)?;
        for value in [wheel.contact_point, wheel.contact_normal] {
            for x in value.to_array() {
                write!(out, ",{x:.6}")?;
            }
        }
        write!(
            out,
            ",{:.6},{:.6},{:.6},{:.6}",
            wheel.vel_lat, wheel.vel_long, wheel.surface_grip, wheel.surface_drag
        )?;
    }
    for matrix in [os.implicit_a, os.implicit_b, os.implicit_c] {
        for value in matrix.to_cols_array() {
            write!(out, ",{value:.6}")?;
        }
    }
    writeln!(out)
}

fn validated_settled_source(
    path: &str,
    route: &Route,
    route_path: &str,
    route_bytes: &[u8],
) -> serde_json::Value {
    assert!(
        std::env::var_os("MM2_PLAN").is_none(),
        "settled source cannot combine with route planning"
    );
    let source: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read settled source"))
            .expect("parse settled source");
    assert_eq!(source["car"], route.car, "settled source car mismatch");
    assert_eq!(source["city"], route.city, "settled source city mismatch");
    assert_eq!(
        source["car_identity_verified"], true,
        "settled source car identity unverified"
    );
    assert_eq!(source["complete"], true, "settled source incomplete");
    assert_eq!(source["hz"], 60, "settled source must be60Hz");
    assert_eq!(
        source["route"],
        serde_json::from_slice::<serde_json::Value>(route_bytes).unwrap(),
        "settled source route differs"
    );
    let hash = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(route_path)
        .output()
        .expect("route SHA256");
    assert!(hash.status.success(), "route hash failed");
    let hash = String::from_utf8(hash.stdout).unwrap();
    assert_eq!(
        source["route_sha256"],
        hash.split_whitespace().next().unwrap(),
        "settled route SHA256 mismatch"
    );
    let rows = source["rows"].as_array().expect("source rows");
    assert_eq!(
        rows.len(),
        route.frames + 1,
        "complete source tick sequence required"
    );
    for (n, row) in rows.iter().enumerate() {
        assert_eq!(row["n"].as_u64(), Some(n as u64), "source row order");
    }
    source
}

/// Two-row numeric probe is useful only when its boundary exactly matches
/// a complete reference recording; it is never accepted as route evidence.
fn validated_engine_source(
    path: &str,
    reference_path: &str,
    route: &Route,
    route_path: &str,
    route_bytes: &[u8],
) -> serde_json::Value {
    let reference = validated_settled_source(reference_path, route, route_path, route_bytes);
    let source: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("read engine snapshot"))
            .expect("parse engine snapshot");
    assert_eq!(source["capture_kind"], "route-start-powertrain-snapshot");
    for key in [
        "car",
        "city",
        "car_identity_verified",
        "hz",
        "route",
        "route_sha256",
    ] {
        assert_eq!(
            source[key], reference[key],
            "engine snapshot identity differs: {key}"
        );
    }
    let rows = source["rows"].as_array().expect("engine snapshot rows");
    assert_eq!(rows.len(), 2, "engine snapshot must contain only n0 and n1");
    for (n, row) in rows.iter().enumerate() {
        assert_eq!(row["n"].as_u64(), Some(n as u64));
        for key in [
            "pos", "vel", "omega", "right", "up", "forward", "rpm", "gear", "spin", "steer",
            "ground",
        ] {
            assert_eq!(
                row[key], reference["rows"][n][key],
                "engine snapshot boundary differs n{n}: {key}"
            );
        }
    }
    source
}

fn seed_engine_source(
    world: &mut World,
    car: Entity,
    row: &serde_json::Value,
) -> serde_json::Value {
    let source = &row["diagnostics"]["powertrain"];
    let number = |key: &str| {
        let value = source[key].as_f64().expect("raw engine numeric field") as f32;
        assert!(value.is_finite(), "nonfinite raw engine field {key}");
        value
    };
    assert_eq!(source["inGearChange"].as_u64(), Some(0));
    assert_eq!(source["gearChanged"].as_u64(), Some(0));
    assert_eq!(source["clutchAttached"].as_bool(), Some(false));
    assert_eq!(source["isAutomatic"].as_u64(), Some(1));
    assert!(
        number("throttle").abs() < 1e-6,
        "snapshot must precede route throttle"
    );
    let engine_omega = number("engineOmega");
    let rpm = row["rpm"].as_f64().expect("source displayed RPM") as f32;
    assert!(engine_omega >= 0.0 && rpm.is_finite());
    assert!(
        (engine_omega * 60.0 / std::f32::consts::TAU - rpm).abs() < 0.001,
        "settled RPM unexpectedly differs from raw engine omega"
    );
    let mut state = world.get_mut::<VehicleState>(car).unwrap();
    let p = &mut state.original.as_mut().expect("original state").powertrain;
    assert!(
        !p.clutch && !p.gear_changed && p.shift_lag_left() == 0.0,
        "native powertrain must also have an inactive shift and detached clutch"
    );
    assert_eq!(row["gear"].as_u64(), Some(p.gear as u64));
    let before = serde_json::json!({"engine_omega":p.engine_omega,"rpm":p.rpm,
        "torque":p.torque,"time_in_gear":p.time_in_gear});
    p.engine_omega = engine_omega;
    p.rpm = rpm;
    p.torque = number("torque");
    p.time_in_gear = number("timeInGear");
    assert!(p.time_in_gear >= 0.0);
    let after = serde_json::json!({"engine_omega":p.engine_omega,"rpm":p.rpm,
        "torque":p.torque,"time_in_gear":p.time_in_gear});
    state.rpm = rpm;
    serde_json::json!({"before":before,"after":after,"raw_source":source})
}

fn seed_settled_source(
    world: &mut World,
    car: Entity,
    cfg: &mm2_vehicle::VehicleConfig,
    row: &serde_json::Value,
) {
    let number = |value: &serde_json::Value| {
        let value = value.as_f64().expect("source numeric field") as f32;
        assert!(value.is_finite(), "nonfinite settled source field");
        value
    };
    let vector = |value: &serde_json::Value| {
        let a = value.as_array().expect("source vector");
        assert_eq!(a.len(), 3);
        Vec3::new(number(&a[0]), number(&a[1]), number(&a[2]))
    };
    let basis = Mat3::from_cols(
        vector(&row["right"]),
        vector(&row["up"]),
        vector(&row["forward"]),
    );
    assert!(
        basis.determinant() > 0.9 && basis.determinant() < 1.1,
        "invalid source AGE basis (forward column is back axis)"
    );
    let rotation = Quat::from_mat3(&basis).normalize();
    let com = vector(&row["pos"]);
    let position = com - rotation * Vec3::from(cfg.center_of_mass);
    let velocity = vector(&row["vel"]);
    let omega = vector(&row["omega"]);
    let ics = &row["diagnostics"]["ics"];
    assert!(
        vector(&ics["netPush"]).length() < 1e-6,
        "pending source netPush cannot be represented by this partial seed"
    );
    assert!(
        vector(&ics["netTurn"]).length() < 1e-6,
        "pending source netTurn cannot be represented by this partial seed"
    );
    assert!(
        row["ground"].as_array().is_some_and(|flags| {
            flags.len() == 4 && flags.iter().all(|flag| flag.as_u64() == Some(1))
        }),
        "settled seed currently requires all four wheels grounded"
    );
    let source_wheels = row["diagnostics"]["wheels"]
        .as_array()
        .expect("source wheel diagnostics");
    assert_eq!(source_wheels.len(), 4);
    let mut a = Mat3::ZERO;
    let mut b = Mat3::ZERO;
    let mut c = Mat3::ZERO;
    {
        let mut state = world.get_mut::<VehicleState>(car).unwrap();
        let os = state.original.as_mut().expect("original state");
        assert_eq!(
            row["gear"].as_u64(),
            Some(os.powertrain.gear as u64),
            "retained native gearbox differs from source n0"
        );
        for (i, wheel) in source_wheels.iter().enumerate() {
            let n = vector(&wheel["probeNormal"]);
            let d = number(&wheel["D"]);
            assert!(d >= 0.0 && n.length_squared() > 0.5);
            let j = Mat3::from_cols(n * (d * n.x), n * (d * n.y), n * (d * n.z));
            let arm = vector(&wheel["contactPoint"]) - com;
            let cross = Mat3::from_cols(arm.cross(Vec3::X), arm.cross(Vec3::Y), arm.cross(Vec3::Z));
            a += j;
            b += cross * j;
            c += cross * j * cross.transpose();
            let target = &mut os.wheels[i];
            target.x = number(&wheel["x"]);
            target.bristles.lat = number(&wheel["dLat"]);
            target.bristles.long = number(&wheel["dLong"]);
            target.reaction = number(&wheel["FLong"]) * cfg.wheels[i].radius;
            target.omega =
                (number(&wheel["vFwd"]) - number(&wheel["vSlip"])) / cfg.wheels[i].radius;
        }
        os.drive_omega = -number(&row["spin"]);
        os.pending_force = vector(&ics["force"]) + vector(&ics["force2"]);
        os.pending_torque = vector(&ics["torque"]) + vector(&ics["torque2"]);
        os.collision_impulse = vector(&ics["impulse"]);
        os.collision_angular_impulse = vector(&ics["angularImpulse"]);
        os.applied_push = vector(&ics["totalPush"]);
        os.last_push = vector(&ics["lastTotalPush"]);
        os.push = Vec3::ZERO;
        os.implicit_a = a;
        os.implicit_b = b;
        os.implicit_c = c;
        let r = Mat3::from_quat(rotation);
        os.angular_momentum = Some(
            r * Mat3::from_diagonal(Vec3::from(cfg.inertia.expect("authored inertia")))
                * r.transpose()
                * omega,
        );
        os.integrated_angular_velocity = omega;
        os.step_start_position = position;
        os.step_start_rotation = rotation;
        state.forward_speed = velocity.dot(-(rotation * Vec3::Z));
        for (i, wheel) in source_wheels.iter().enumerate() {
            state.wheels[i].compression = number(&wheel["x"]);
            state.wheels[i].slip_visual = number(&wheel["slipVisual"]);
            state.wheels[i].grounded = true;
        }
        state.grounded = true;
    }
    world.get_mut::<Position>(car).unwrap().0 = position;
    world.get_mut::<Rotation>(car).unwrap().0 = rotation;
    world.get_mut::<LinearVelocity>(car).unwrap().0 = velocity;
    world.get_mut::<AngularVelocity>(car).unwrap().0 = omega;
    *world.get_mut::<Transform>(car).unwrap() =
        Transform::from_translation(position).with_rotation(rotation);
}

/// Pace only the optional backend mode; manual fixed dt remains identical.
/// Slow frames never skip physics ticks or route input events.
fn update_app(app: &mut App, pacing: &mut Option<(Instant, u64)>) {
    if let Some((start, tick)) = pacing {
        let deadline = *start + Duration::from_secs_f64(*tick as f64 / 60.0);
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        *tick += 1;
    }
    app.update();
}

fn sample_audio(
    world: &mut World,
    car: Entity,
    frame: usize,
    realtime_audio: bool,
) -> serde_json::Value {
    let contact = world
        .get::<SurfaceContact>(car)
        .copied()
        .unwrap_or_default();
    let mut query = world.query::<(
        Entity,
        &SurfaceVoice,
        &ChildOf,
        Option<&PlaybackSettings>,
        Option<&AudioPlayer<PcmAudio>>,
        Option<&AudioSink>,
        Option<&SpatialAudioSink>,
    )>();
    let voices:Vec<_> = query.iter(world).filter(|(_,_,parent,_,_,_,_)| parent.parent()==car).map(|(entity,voice,parent,playback,player,sink,spatial)| {
        let clip=player.and_then(|v|world.resource::<Assets<PcmAudio>>().get(&v.0));
        let mut value = serde_json::json!({
            "entity":entity.to_bits(),"parent":parent.parent().to_bits(),
            "role":format!("{:?}",voice.role),"mix_volume":voice.mix.volume,"mix_speed":voice.mix.speed,
            "audio_player_handle":player.map(|v|format!("{:?}",v.0.id())),
            "decoded_duration_seconds":clip.map(PcmAudio::duration),
            "playback":playback.map(|v|serde_json::json!({"mode":format!("{:?}",v.mode),"volume_linear":v.volume.to_linear(),"speed":v.speed,"paused":v.paused,"muted":v.muted,"spatial":v.spatial,"spatial_scale":format!("{:?}",v.spatial_scale),"start_seconds":v.start_position.map(|d|d.as_secs_f64()),"duration_seconds":v.duration.map(|d|d.as_secs_f64())}))
        });
        if realtime_audio {
            let live: Option<&dyn AudioSinkPlayback> = sink.map(|v| v as &dyn AudioSinkPlayback)
                .or_else(|| spatial.map(|v| v as &dyn AudioSinkPlayback));
            value["sink"] = live.map(|v| serde_json::json!({
                "kind":if sink.is_some() { "AudioSink" } else { "SpatialAudioSink" },
                "volume_linear":v.volume().to_linear(), "speed":v.speed(),
                "paused":v.is_paused(), "empty":v.empty(),
                "audible_requested":v.volume().to_linear()>0.0 && !v.is_paused() && !v.empty(),
                "device_output_recorded":false
            })).unwrap_or(serde_json::Value::Null);
        }
        value
    }).collect();
    let report = world.resource::<AudioReport>();
    serde_json::json!({"frame":frame,"surface_contact":{"roll":contact.roll,"skid":contact.skid.map(|v|serde_json::json!({"surface":v.surface,"slippage":v.slippage,"wheel_speed":v.wheel_speed}))},"voices":voices,"report":{"voices":report.voices,"sunk":report.sunk,"failed":report.failed,"dropped":report.dropped,"skids":report.skids,"rolling":report.rolling}})
}
