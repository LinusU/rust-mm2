//! `mm2_gym` — the neural driver's training environment as a process.
//!
//! Wraps [`mm2_app::neural::env::DrivingEnv`] for the trainer in
//! `training/`. A development tool, so an example rather than a shipped
//! binary. Two modes:
//!
//! ```sh
//! cargo build --release -p mm2_app --example mm2_gym
//!
//! # Measure how fast the environment steps on this machine.
//! target/release/examples/mm2_gym --mm2-path <install> --city sf bench
//!
//! # Serve steps to a trainer over stdin/stdout.
//! target/release/examples/mm2_gym --mm2-path <install> --city sf --seed 7 serve
//! ```
//!
//! # Wire protocol (`serve`)
//!
//! Little-endian binary on stdin/stdout; logs go to stderr. On start the
//! process writes a hello: `b"MM2G"`, then `u32` protocol version,
//! observation size, action size, agent count and observation layout
//! version, then a `u32`-length-prefixed JSON object naming the lines
//! and cars the environment draws from.
//!
//! Each request is one `u32` command:
//!
//! | Command | Payload | Reply |
//! | --- | --- | --- |
//! | 0 observe | — | `agents × obs` `f32` |
//! | 1 step | `agents × act` `f32` | see below |
//! | 2 quit | — | — |
//! | 3 configure | `u32` length + JSON `{"rewards": {…}, "starts": {…}}` | `u32` 0 ok / 1 rejected |
//!
//! A step replies with `agents × obs` `f32` observations, `agents`
//! `f32` rewards, `agents` `u8` done flags (0 running, 1 failed, 2 cut
//! short), a `u32` count and that many final observations (one per
//! flag-2 agent, in agent order), then a `u32` count and that many
//! episode records (`u32` end, `u32` steps, `f32` metres, `f32` return,
//! `u32` line, `u32` car, `f32` impacts, `f32` peak cornering, `f32`
//! top speed, `f32` arc position and three `f32` world coordinates of
//! where it ended).

use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use mm2_app::neural::env::{DrivingEnv, EnvConfig, Rewards, Starts, StepOut};
use mm2_app::neural::observe::{ACT_DIM, CAR_DIM, MOTION_DIM, OBS_DIM, OBS_VERSION};
use mm2_app::neural::policy::{Policy, Scratch};
use mm2_assets::{InstallMount, Vfs, mount_install};

const PROTOCOL: u32 = 3;

#[derive(Parser)]
#[command(about = "Training environment for the neural opponent driver")]
struct Args {
    /// Path to a Midtown Madness 2 installation.
    #[arg(long)]
    mm2_path: PathBuf,
    /// City to drive in.
    #[arg(long, default_value = "sf")]
    city: String,
    /// Event stem (`race3`, `circuit0`) whose world and lines to load;
    /// without it the cruise city with every event's lines.
    #[arg(long)]
    event: Option<String>,
    /// Cars driving at once.
    #[arg(long, default_value_t = 16)]
    agents: usize,
    /// Untrained cars in the way: parked, or driven at traffic pace.
    #[arg(long, default_value_t = 4)]
    drones: usize,
    /// Seed for the environment's random choices.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Physics steps per decision (3 = 20 Hz).
    #[arg(long, default_value_t = 3)]
    decision_ticks: u32,
    /// Physics worker threads.
    #[arg(long, default_value_t = 1)]
    threads: usize,
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Step with a crude line-following driver and report throughput.
    Bench {
        /// Decisions to run.
        #[arg(long, default_value_t = 2000)]
        steps: u32,
    },
    /// Serve the wire protocol on stdin/stdout.
    Serve,
    /// Print the city's race event stems as a JSON array.
    Events,
    /// Walk every line of the city's events and print where each cannot
    /// be driven as drawn — through a wall, over a void, into water.
    Audit,
    /// Print a height map of a square of the event's world and the
    /// lines crossing it, as JSON for `training/mm2rl/plot.py`.
    Map {
        /// Centre `x,y,z`.
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        at: Vec<f32>,
        /// Side of the square (m).
        #[arg(long, default_value_t = 160.0)]
        size: f32,
        /// Sample spacing (m).
        #[arg(long, default_value_t = 1.0)]
        step: f32,
        /// Sample from this far above the centre (m): low enough to
        /// look under a bridge deck, high enough to clear the road.
        #[arg(long, default_value_t = 8.0)]
        above: f32,
    },
    /// Print the points of the event's lines around an arc position, as
    /// `--cam`-ready coordinates — for looking at a trouble spot.
    Where {
        /// Arc position (m) along each line.
        #[arg(long)]
        arc: f32,
        /// Only lines whose label contains this.
        #[arg(long)]
        only: Option<String>,
    },
    /// Race every opponent field of the city from its grid with a
    /// trained policy and print one JSON line per car.
    Eval {
        /// The `.mm2p` policy to drive with.
        #[arg(long)]
        policy: PathBuf,
        /// Only fields whose name contains this (`race3`, `/p`, `cir`).
        #[arg(long)]
        only: Option<String>,
        /// Give up on a field after this many simulated seconds.
        #[arg(long, default_value_t = 900.0)]
        max_seconds: f32,
    },
}

#[derive(serde::Deserialize)]
struct Configure {
    rewards: Option<Rewards>,
    starts: Option<Starts>,
}

fn main() {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "error".into()),
        )
        .init();
    let mut vfs = Vfs::new();
    if let Err(e) = mount_install(&mut vfs, &args.mm2_path, &InstallMount::default()) {
        eprintln!("mount {}: {e}", args.mm2_path.display());
        std::process::exit(2);
    }
    let build = |event: Option<String>, agents: usize| {
        let config = EnvConfig {
            city: args.city.clone(),
            event,
            agents,
            drones: args.drones,
            seed: args.seed,
            decision_ticks: args.decision_ticks,
            threads: args.threads,
            ..EnvConfig::default()
        };
        DrivingEnv::new(&vfs, config).unwrap_or_else(|e| {
            eprintln!("environment: {e}");
            std::process::exit(2);
        })
    };
    let announce = |env: &DrivingEnv| {
        eprintln!(
            "mm2_gym: {}{} — {} agents, {} lines, {} cars, obs {OBS_DIM}, act {ACT_DIM}",
            args.city,
            args.event
                .as_deref()
                .map(|e| format!("/{e}"))
                .unwrap_or_default(),
            env.agents(),
            env.lines().len(),
            env.cars().len(),
        );
    };
    match &args.mode {
        Mode::Events => {
            println!(
                "{}",
                serde_json::json!(mm2_app::neural::env::race_events(&vfs, &args.city))
            );
        }
        Mode::Map {
            at,
            size,
            step,
            above,
        } => {
            let &[x, y, z] = at.as_slice() else {
                eprintln!("--at takes x,y,z");
                std::process::exit(2);
            };
            let centre = bevy::math::Vec3::new(x, y, z);
            let cells = (size / step).round().max(2.0) as usize;
            let mut env = build(args.event.clone(), 0);
            let heights = env.height_map(centre, *step, cells, *above);
            let reach = size * 0.75;
            let lines: Vec<_> = env
                .lines()
                .iter()
                .map(|line| {
                    let points: Vec<[f32; 3]> = line
                        .guide
                        .points()
                        .iter()
                        .filter(|p| (p.x - x).abs() < reach && (p.z - z).abs() < reach)
                        .map(|p| p.to_array())
                        .collect();
                    serde_json::json!({ "label": line.label, "points": points })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "centre": [x, y, z],
                    "step": step,
                    "cells": cells,
                    "heights": heights.iter().map(|h| h.is_finite().then_some(*h)).collect::<Vec<_>>(),
                    "lines": lines,
                })
            );
        }
        Mode::Where { arc, only } => {
            let env = build(args.event.clone(), 0);
            for line in env.lines() {
                if only.as_deref().is_some_and(|o| !line.label.contains(o)) {
                    continue;
                }
                let p = line.guide.point_at(*arc);
                let t = line.guide.tangent_at(*arc);
                println!(
                    "{} arc {arc:.0}/{:.0}: {:.1},{:.1},{:.1} heading {:.0}°",
                    line.label,
                    line.guide.length(),
                    p.x,
                    p.y,
                    p.z,
                    (-t.x).atan2(-t.z).to_degrees(),
                );
            }
        }
        Mode::Audit => {
            let events = match &args.event {
                Some(event) => vec![event.clone()],
                None => mm2_app::neural::env::race_events(&vfs, &args.city),
            };
            for event in events {
                let mut env = build(Some(event), 0);
                for line in 0..env.lines().len() {
                    let found = env.audit_line(line, 4.0);
                    // Runs of the same finding print once, with their extent.
                    let mut i = 0;
                    while i < found.len() {
                        let (start, at, what) = found[i];
                        let mut j = i;
                        while j + 1 < found.len()
                            && found[j + 1].2 == what
                            && found[j + 1].0 - found[j].0 <= 4.5
                        {
                            j += 1;
                        }
                        println!(
                            "{}",
                            serde_json::json!({
                                "city": args.city,
                                "line": env.lines()[line].label,
                                "what": what,
                                "arc": start,
                                "metres": found[j].0 - start + 4.0,
                                "at": [at.x, at.y, at.z],
                            })
                        );
                        i = j + 1;
                    }
                }
            }
        }
        Mode::Bench { steps } => {
            let mut env = build(args.event.clone(), args.agents);
            announce(&env);
            bench(&mut env, *steps, args.decision_ticks);
        }
        Mode::Eval {
            policy,
            only,
            max_seconds,
        } => {
            let policy = std::fs::read(policy)
                .map_err(|e| e.to_string())
                .and_then(|b| Policy::parse(&b, OBS_DIM, ACT_DIM).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| {
                    eprintln!("policy: {e}");
                    std::process::exit(2);
                });
            // Each event races in its own world.
            let events = match &args.event {
                Some(event) => vec![event.clone()],
                None => mm2_app::neural::env::race_events(&vfs, &args.city),
            };
            let mut tally = Tally::default();
            for event in events {
                // An evaluation stages its own fields.
                let mut env = build(Some(event), 0);
                eval(
                    &mut env,
                    &policy,
                    only.as_deref(),
                    *max_seconds,
                    args.decision_ticks,
                    &mut tally,
                );
            }
            eprintln!(
                "eval: {}/{} cars finished; finishers averaged {:.1} m/s",
                tally.finished,
                tally.cars,
                tally.metres / tally.seconds.max(1e-3)
            );
        }
        Mode::Serve => {
            let mut env = build(args.event.clone(), args.agents);
            announce(&env);
            let env = &mut env;
            if let Err(e) = serve(env) {
                // A trainer that went away is the normal way to stop.
                if e.kind() != io::ErrorKind::UnexpectedEof && e.kind() != io::ErrorKind::BrokenPipe
                {
                    eprintln!("mm2_gym: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Steer at the line 16 m ahead and hold a moderate pace — enough of a
/// driver to keep the cars moving through real geometry while the
/// stepping rate is measured.
fn crude_actions(obs: &[f32], agents: usize) -> Vec<f32> {
    let guide = CAR_DIM + MOTION_DIM;
    let mut actions = vec![0.0; agents * ACT_DIM];
    for i in 0..agents {
        let o = &obs[i * OBS_DIM..(i + 1) * OBS_DIM];
        let speed = o[CAR_DIM] * 40.0;
        // Third look-ahead sample (16 m): lateral over distance.
        let aim = o[guide + 4 + 3 * 2];
        actions[i * ACT_DIM] = (aim * 3.0).clamp(-1.0, 1.0);
        actions[i * ACT_DIM + 1] = if speed < 14.0 { 0.7 } else { 0.0 };
        actions[i * ACT_DIM + 2] = -1.0;
    }
    actions
}

fn bench(env: &mut DrivingEnv, steps: u32, decision_ticks: u32) {
    let agents = env.agents();
    let mut obs = env.observations();
    let mut ends = [0u32; 6];
    let mut metres = 0.0;
    let mut reward = 0.0;
    let started = Instant::now();
    for _ in 0..steps {
        let out = env.step(&crude_actions(&obs, agents));
        for e in &out.episodes {
            ends[e.end as usize] += 1;
            metres += e.metres;
        }
        reward += out.reward.iter().sum::<f32>();
        obs = out.obs;
    }
    let secs = started.elapsed().as_secs_f32();
    eprintln!(
        "census (dynamic, kinematic, static, sleeping, contact pairs) = {:?}",
        env.census()
    );
    let transitions = steps as f32 * agents as f32;
    println!(
        "steps={steps} agents={agents} secs={secs:.2} transitions_per_s={:.0} realtime_x={:.1} \
         ends(finished/time/stalled/off/upended/lost)={ends:?} episode_metres={metres:.0} \
         mean_reward={:.4}",
        transitions / secs,
        steps as f32 * decision_ticks as f32 / 60.0 / secs,
        reward / transitions,
    );
}

/// Totals over an evaluation.
#[derive(Default)]
struct Tally {
    cars: u32,
    finished: u32,
    seconds: f32,
    metres: f32,
}

/// Race each field of the environment's event once and report every
/// car's outcome.
fn eval(
    env: &mut DrivingEnv,
    policy: &Policy,
    only: Option<&str>,
    max_seconds: f32,
    decision_ticks: u32,
    tally: &mut Tally,
) {
    const ENDS: [&str; 6] = [
        "finished",
        "time",
        "stalled",
        "off_route",
        "upended",
        "lost",
    ];
    let seconds_per_step = decision_ticks as f32 / 60.0;
    let mut fields: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, line) in env.lines().iter().enumerate() {
        if only.is_some_and(|o| !line.field.contains(o)) {
            continue;
        }
        match fields.iter_mut().find(|(f, _)| *f == line.field) {
            Some((_, lines)) => lines.push(i),
            None => fields.push((line.field.clone(), vec![i])),
        }
    }
    let mut scratch = Scratch::default();
    for (field, lines) in &fields {
        env.stage_field(lines);
        let agents = env.agents();
        let mut actions = vec![0.0; agents * ACT_DIM];
        let mut obs = env.observations();
        let mut reported = vec![false; agents];
        let mut step = 0u32;
        while env.running() > 0 && (step as f32) * seconds_per_step < max_seconds {
            for i in 0..agents {
                policy.act(
                    &obs[i * OBS_DIM..(i + 1) * OBS_DIM],
                    &mut actions[i * ACT_DIM..(i + 1) * ACT_DIM],
                    &mut scratch,
                );
            }
            let out = env.step(&actions);
            step += 1;
            for e in &out.episodes {
                let line = &env.lines()[e.line as usize];
                if let Some(i) = lines.iter().position(|&l| l == e.line as usize) {
                    reported[i] = true;
                }
                tally.cars += 1;
                if e.end as u32 == 0 {
                    tally.finished += 1;
                    tally.seconds += e.steps as f32 * seconds_per_step;
                    tally.metres += e.metres;
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "field": field,
                        "line": line.label,
                        "car": env.cars()[e.car as usize].id,
                        "end": ENDS[e.end as usize],
                        "seconds": e.steps as f32 * seconds_per_step,
                        "metres": e.metres,
                        "length": line.guide.length(),
                        "impacts": e.impacts,
                        "lateral": e.lateral,
                        "top_speed": e.top_speed,
                        "throttle_cap": line.throttle_cap,
                        "at": [e.position.x, e.position.y, e.position.z],
                    })
                );
            }
            obs = out.obs;
        }
        for (i, &l) in lines.iter().enumerate() {
            if !reported[i] {
                tally.cars += 1;
                let line = &env.lines()[l];
                println!(
                    "{}",
                    serde_json::json!({
                        "field": field,
                        "line": line.label,
                        "car": env.cars()[line.car].id,
                        "end": "time",
                        "seconds": max_seconds,
                        "length": line.guide.length(),
                    })
                );
            }
        }
    }
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn write_f32s(w: &mut impl Write, v: &[f32]) -> io::Result<()> {
    for x in v {
        w.write_all(&x.to_le_bytes())?;
    }
    Ok(())
}

fn write_step(w: &mut impl Write, out: &StepOut) -> io::Result<()> {
    write_f32s(w, &out.obs)?;
    write_f32s(w, &out.reward)?;
    w.write_all(&out.done)?;
    w.write_all(&((out.final_obs.len() / OBS_DIM) as u32).to_le_bytes())?;
    write_f32s(w, &out.final_obs)?;
    w.write_all(&(out.episodes.len() as u32).to_le_bytes())?;
    for e in &out.episodes {
        w.write_all(&(e.end as u32).to_le_bytes())?;
        w.write_all(&e.steps.to_le_bytes())?;
        w.write_all(&e.metres.to_le_bytes())?;
        w.write_all(&e.ret.to_le_bytes())?;
        w.write_all(&e.line.to_le_bytes())?;
        w.write_all(&e.car.to_le_bytes())?;
        w.write_all(&e.impacts.to_le_bytes())?;
        w.write_all(&e.lateral.to_le_bytes())?;
        w.write_all(&e.top_speed.to_le_bytes())?;
        w.write_all(&e.arc.to_le_bytes())?;
        for v in e.position.to_array() {
            w.write_all(&v.to_le_bytes())?;
        }
    }
    Ok(())
}

fn serve(env: &mut DrivingEnv) -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = BufWriter::with_capacity(1 << 20, io::stdout().lock());
    let agents = env.agents();

    output.write_all(b"MM2G")?;
    for v in [
        PROTOCOL,
        OBS_DIM as u32,
        ACT_DIM as u32,
        agents as u32,
        OBS_VERSION,
    ] {
        output.write_all(&v.to_le_bytes())?;
    }
    let meta = serde_json::json!({
        "lines": env.lines().iter().map(|l| serde_json::json!({
            "label": l.label,
            "length": l.guide.length(),
            "closed": l.guide.is_closed(),
        })).collect::<Vec<_>>(),
        "cars": env.cars().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
    })
    .to_string();
    output.write_all(&(meta.len() as u32).to_le_bytes())?;
    output.write_all(meta.as_bytes())?;
    output.flush()?;

    let mut raw = vec![0u8; agents * ACT_DIM * 4];
    let mut actions = vec![0.0f32; agents * ACT_DIM];
    loop {
        match read_u32(&mut input)? {
            0 => write_f32s(&mut output, &env.observations())?,
            1 => {
                input.read_exact(&mut raw)?;
                for (a, c) in actions.iter_mut().zip(raw.as_chunks::<4>().0) {
                    *a = f32::from_le_bytes(*c);
                }
                write_step(&mut output, &env.step(&actions))?;
            }
            2 => return Ok(()),
            3 => {
                let len = read_u32(&mut input)? as usize;
                let mut json = vec![0u8; len.min(1 << 20)];
                input.read_exact(&mut json)?;
                let ok = match serde_json::from_slice::<Configure>(&json) {
                    Ok(c) => {
                        if let Some(r) = c.rewards {
                            env.set_rewards(r);
                        }
                        if let Some(s) = c.starts {
                            env.set_starts(s);
                        }
                        0u32
                    }
                    Err(e) => {
                        eprintln!("mm2_gym: configure rejected: {e}");
                        1
                    }
                };
                output.write_all(&ok.to_le_bytes())?;
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown command {other}"),
                ));
            }
        }
        output.flush()?;
    }
}
