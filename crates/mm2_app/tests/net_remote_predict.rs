//! NETPLAY-05A-2: remote-input prediction policies measured on the
//! production vehicle step. A reference car is driven by the true
//! per-tick commands with zero delay; the same scripted commands then reach
//! a second car only after a delay of `d` ticks, so every tick of it is
//! driven by [`RemotePredictor`] from commands `d` ticks old. The error is
//! the distance and speed gap to the reference car. Run with
//! `--nocapture` for the table recorded in `docs/netcode-redesign.md`.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::netdrive::decode_tick_command;
use mm2_app::netremote::{Basis, Policy, RemotePredictor};
use mm2_net::{Message, RemoteCommand, RemoteCommands, RemoteSeat, Source, TickCommand};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

const PLAYER: u16 = 2;
/// Ticks of straight full throttle before any scenario event.
const RUNUP: u64 = 150;

fn world() -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .insert_resource(VehicleProbeLayers(LayerMask(0b01)));
    app.finish();
    app.cleanup();
    let w = app.world_mut();
    w.spawn((
        RigidBody::Static,
        Collider::cuboid(2000.0, 1.0, 2000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let car = w
        .spawn((
            vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(0.0, 1.2, 0.0)),
            Transform::from_xyz(0.0, 1.2, 0.0),
        ))
        .id();
    app.update();
    (app, car)
}

fn q(throttle: f32, brake: f32, steer: f32) -> TickCommand {
    TickCommand {
        throttle: (throttle * 255.0) as u8,
        brake: (brake * 255.0) as u8,
        steer: (steer * 127.0) as i8,
        handbrake: 0,
        auto_reverse: true,
        forced_gear: None,
    }
}

/// A named script: the true command of host tick `t` (1-based).
struct Scenario {
    name: &'static str,
    ticks: u64,
    at: fn(u64) -> TickCommand,
}

const SCENARIOS: [Scenario; 4] = [
    Scenario {
        name: "steady cruise",
        ticks: RUNUP + 90,
        at: |_| q(1.0, 0.0, 0.0),
    },
    Scenario {
        name: "sudden braking",
        ticks: RUNUP + 90,
        at: |t| {
            if t <= RUNUP {
                q(1.0, 0.0, 0.0)
            } else {
                q(0.0, 1.0, 0.0)
            }
        },
    },
    Scenario {
        name: "steering reversal",
        ticks: RUNUP + 120,
        at: |t| match t - RUNUP.min(t) {
            0 => q(1.0, 0.0, 0.0),
            1..=40 => q(0.6, 0.0, 0.8),
            41..=80 => q(0.6, 0.0, -0.8),
            _ => q(0.6, 0.0, 0.0),
        },
    },
    Scenario {
        name: "evasive near-miss",
        ticks: RUNUP + 120,
        at: |t| match t - RUNUP.min(t) {
            0 => q(1.0, 0.0, 0.0),
            1..=15 => q(0.0, 0.0, 1.0),
            16..=45 => q(0.0, 0.0, -1.0),
            46..=60 => q(0.0, 0.0, 1.0),
            _ => q(0.5, 0.0, 0.0),
        },
    },
];

#[derive(Clone, Copy)]
struct Sample {
    pos: Vec3,
    vel: Vec3,
}

fn sample(app: &App, car: Entity) -> Sample {
    Sample {
        pos: app.world().get::<Position>(car).unwrap().0,
        vel: app.world().get::<LinearVelocity>(car).unwrap().0,
    }
}

fn drive(app: &mut App, car: Entity, command: &TickCommand) {
    *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = decode_tick_command(command);
    app.update();
}

fn reference(s: &Scenario) -> Vec<Sample> {
    let (mut app, car) = world();
    (1..=s.ticks)
        .map(|t| {
            drive(&mut app, car, &(s.at)(t));
            sample(&app, car)
        })
        .collect()
}

/// The frame the host would send at tick `now`: the seat's last 16 ticks.
fn frame(s: &Scenario, now: u64) -> Message {
    let first = now.saturating_sub(15).max(1);
    Message::RemoteCommands {
        generation: 1,
        tick: now,
        seats: vec![RemoteSeat {
            player: PLAYER,
            epoch: 0,
            first_tick: first,
            commands: (first..=now)
                .map(|t| RemoteCommand {
                    command: (s.at)(t),
                    source: Source::Command,
                })
                .collect(),
        }],
    }
}

#[derive(Default, Clone, Copy)]
struct Error {
    max_pos: f32,
    final_pos: f32,
    max_vel: f32,
}

/// Run `s` with commands reaching the client `delay` ticks late.
fn delayed(
    s: &Scenario,
    policy: Policy,
    delay: u64,
    stall: Option<(u64, u64)>,
    truth: &[Sample],
) -> Error {
    let (mut app, car) = world();
    let mut store = RemoteCommands::new();
    store.set_generation(Some(1));
    let predictor = RemotePredictor::new(policy);
    let mut err = Error::default();
    for t in 1..=s.ticks {
        // A stall: nothing newer than `cut` arrives, and only the first
        // `horizon` ticks after it are scored.
        let cut = stall.map_or(u64::MAX, |(cut, _)| cut);
        if t > delay && t - delay <= cut {
            // Everything the host applied through `t - delay` has arrived.
            assert!(store.absorb(&frame(s, t - delay)));
        }
        if stall.is_some_and(|(cut, horizon)| t > cut + delay + horizon) {
            break;
        }
        let got = predictor.command_for(&mut store, PLAYER, t);
        if delay == 0 {
            assert_eq!(got.basis, Basis::Authoritative);
            assert_eq!(got.command, (s.at)(t));
        }
        drive(&mut app, car, &got.command);
        let (mine, want) = (sample(&app, car), truth[t as usize - 1]);
        err.final_pos = mine.pos.distance(want.pos);
        if stall.is_some() && t <= cut + delay {
            continue; // before the stall bites, the lag is steady
        }
        err.max_pos = err.max_pos.max(err.final_pos);
        err.max_vel = err.max_vel.max(mine.vel.distance(want.vel));
    }
    err
}

const DELAYS: [u64; 4] = [0, 3, 6, 12];

fn policies() -> Vec<(String, Policy)> {
    let mut out = vec![("hold-last".to_string(), Policy::HoldLast)];
    for (hold, fade) in [(3, 6), (3, 12), (6, 12), (6, 24), (12, 24)] {
        out.push((
            format!("age-aware hold={hold} fade={fade}"),
            Policy::AgeAware { hold, fade },
        ));
    }
    out
}

/// Stalls: the feed stops 5 ticks into the action; scored over the next
/// 15, 30 and 60 ticks (0.25 s, 0.5 s, 1 s). Delivery lag is 3 ticks.
const HORIZONS: [u64; 3] = [15, 30, 60];

#[test]
fn prediction_policies_are_measured_against_a_zero_delay_reference() {
    let mut lag_totals: Vec<(String, f32)> = Vec::new();
    let mut stall_totals: Vec<(String, f32)> = Vec::new();
    let add = |v: &mut Vec<(String, f32)>, n: &str, x: f32| match v.iter_mut().find(|(k, _)| k == n)
    {
        Some((_, t)) => *t += x,
        None => v.push((n.to_string(), x)),
    };
    for s in &SCENARIOS {
        let truth = reference(s);
        println!("\n== {} ==", s.name);
        println!(
            "{:<26} {:>14} {:>10} {:>10} {:>10}",
            "policy", "case", "max pos m", "final pos", "max vel"
        );
        for (name, policy) in policies() {
            for d in DELAYS {
                let e = delayed(s, policy, d, None, &truth);
                println!(
                    "{name:<26} {:>14} {:>10.3} {:>10.3} {:>10.3}",
                    format!("lag {d}"),
                    e.max_pos,
                    e.final_pos,
                    e.max_vel
                );
                if d == 0 {
                    // Zero delay is the reference: nothing is predicted.
                    assert!(e.max_pos == 0.0 && e.max_vel == 0.0, "{name} drifted");
                }
                if d == 3 {
                    add(&mut lag_totals, &name, e.max_pos);
                }
            }
            for h in HORIZONS {
                let e = delayed(s, policy, 3, Some((RUNUP + 5, h)), &truth);
                println!(
                    "{name:<26} {:>14} {:>10.3} {:>10.3} {:>10.3}",
                    format!("stall {h}"),
                    e.max_pos,
                    e.final_pos,
                    e.max_vel
                );
                add(&mut stall_totals, &name, e.max_pos);
            }
        }
    }
    println!("\n== summed max position error: lag 3 / stalls (all scenarios) ==");
    for ((n, l), (_, st)) in lag_totals.iter().zip(&stall_totals) {
        println!("{n:<26} {l:>10.3} {st:>10.3}");
    }
    // The choice recorded in docs/netcode-redesign.md: no age-aware
    // setting measured beats hold-last over the stall scenarios.
    let hold_last = stall_totals[0].1;
    for (n, t) in &stall_totals[1..] {
        assert!(hold_last <= *t, "{n} ({t}) beat hold-last ({hold_last})");
    }
}
