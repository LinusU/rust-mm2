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
//! `u32` line, `u32` car, `f32` impacts).

use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use mm2_app::neural::env::{DrivingEnv, EnvConfig, Rewards, Starts, StepOut};
use mm2_app::neural::observe::{ACT_DIM, CAR_DIM, MOTION_DIM, OBS_DIM, OBS_VERSION};
use mm2_app::neural::policy::{Policy, Scratch};
use mm2_assets::{InstallMount, Vfs, mount_install};

const PROTOCOL: u32 = 1;

#[derive(Parser)]
#[command(about = "Training environment for the neural opponent driver")]
struct Args {
    /// Path to a Midtown Madness 2 installation.
    #[arg(long)]
    mm2_path: PathBuf,
    /// City to drive in.
    #[arg(long, default_value = "sf")]
    city: String,
    /// Cars driving at once.
    #[arg(long, default_value_t = 16)]
    agents: usize,
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
    let config = EnvConfig {
        city: args.city.clone(),
        // An evaluation stages its own fields.
        agents: if matches!(args.mode, Mode::Eval { .. }) {
            0
        } else {
            args.agents
        },
        seed: args.seed,
        decision_ticks: args.decision_ticks,
        threads: args.threads,
        ..EnvConfig::default()
    };
    let started = Instant::now();
    let mut env = match DrivingEnv::new(&vfs, config) {
        Ok(env) => env,
        Err(e) => {
            eprintln!("environment: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "mm2_gym: {} ready in {:.1}s — {} agents, {} lines, {} cars, obs {OBS_DIM}, act {ACT_DIM}",
        args.city,
        started.elapsed().as_secs_f32(),
        env.agents(),
        env.lines().len(),
        env.cars().len(),
    );
    match args.mode {
        Mode::Bench { steps } => bench(&mut env, steps, args.decision_ticks),
        Mode::Eval {
            policy,
            only,
            max_seconds,
        } => {
            let policy = std::fs::read(&policy)
                .map_err(|e| e.to_string())
                .and_then(|b| Policy::parse(&b, OBS_DIM, ACT_DIM).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| {
                    eprintln!("policy: {e}");
                    std::process::exit(2);
                });
            eval(
                &mut env,
                &policy,
                only.as_deref(),
                max_seconds,
                args.decision_ticks,
            );
        }
        Mode::Serve => {
            if let Err(e) = serve(&mut env) {
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

/// Race each field once and report every car's outcome.
fn eval(
    env: &mut DrivingEnv,
    policy: &Policy,
    only: Option<&str>,
    max_seconds: f32,
    decision_ticks: u32,
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
    let (mut cars, mut finished, mut seconds, mut metres) = (0u32, 0u32, 0.0f32, 0.0f32);
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
                cars += 1;
                if e.end as u32 == 0 {
                    finished += 1;
                    seconds += e.steps as f32 * seconds_per_step;
                    metres += e.metres;
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
                    })
                );
            }
            obs = out.obs;
        }
        for (i, &l) in lines.iter().enumerate() {
            if !reported[i] {
                cars += 1;
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
    eprintln!(
        "eval: {finished}/{cars} cars finished; finishers averaged {:.1} m/s",
        metres / seconds.max(1e-3)
    );
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
