//! `mm2` — executable shell for the MM2-inspired engine.
//!
//! Usage:
//!   cargo run -p mm2_app --bin mm2 -- --dev-world
//!   cargo run -p mm2_app --bin mm2 -- --dev-world --mods examples/mods
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path "/path/to/Midtown Madness 2" [--city london]
//!   cargo run -p mm2_app --bin mm2 -- --mm2-path <dir> --mods <dir> --vehicle-config <toml>
//!
//! Smoke modes print `smoke=<kind> … status=<pass|fail|unavailable>`
//! records (see `mm2_app::smoke`) and exit 0/3/4 respectively; usage
//! errors exit 2.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use avian3d::prelude::*;
use bevy::audio::AddAudioSource;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use clap::Parser;
use mm2_app::session::{SelectedCar, SessionControl, SpawnPoint, TunedVehicle};
use mm2_app::{
    audio, banger, breakaway, camera, car_visual, city, cnr, contracts, controls, crowd, damage,
    damage_fx, dash, display_trial, environment, hud, hudmap, input, lesson, menu, nav_overlay,
    navarrow, net, netdrive, oppind, opponents, pause, pedestrian, perf, police, police_debug,
    precip, profile, progression, pvs, race, racestat, racetime, recovery, results, scripted,
    sequence, session, settings, smoke, spark_fx, stuck, texel_fx, traffic, wheel_fx,
};
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_content::{VehicleCatalog, VehicleDef};
use mm2_game::{
    BangerStateChanged, CameraPose, DamageEvent, DevOverrides, ImpactEvent, Mm2Vfs, PartDetached,
    RaceStarted, RecoveryEvent, Session, SessionConfig, SessionPhase, StuckEvent, VehicleSelection,
    WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{VehicleConfig, VehicleDebugEnabled, VehiclePlugin};
use tracing::{error, info, warn};

use camera::CameraMode;

#[derive(Parser, Debug)]
#[command(name = "mm2", about = "MM2-inspired open engine — development build")]
struct Cli {
    /// Spawn the synthetic development playground (no MM2 data needed).
    #[arg(long)]
    dev_world: bool,

    /// Path to a Midtown Madness 2 installation (directory containing the
    /// .ar archives and/or loose files).
    #[arg(long)]
    mm2_path: Option<PathBuf>,

    /// Directory containing mod folders (each with a mod.toml).
    #[arg(long)]
    mods: Option<PathBuf>,

    /// City to load through the VFS (install or mods). Defaults to
    /// `london` when --mm2-path is given. A specifically requested city
    /// the VFS cannot provide is a hard failure, never a silent dev
    /// world.
    #[arg(long)]
    city: Option<String>,

    /// Start an authored event: `<table>:<row>` with table one of
    /// `checkpoint`, `blitz`, `circuit`, `crash` and row the 0-based
    /// table row (`mm2-inspect events <install>` lists them). Implies
    /// the event's `--city`; an event that cannot resolve or build is
    /// a load failure, never a silent roam.
    #[arg(long, value_name = "table:row")]
    event: Option<String>,

    /// Play Cops & Robbers in the city (F27): `ffa` (every player
    /// scores alone), `cops` (Cops vs. Robbers) or `robbers` (Robbers
    /// vs. Robbers). Alone it is a one-seat match; with `--host` it is
    /// the session the lobby advertises and starts (closed to late
    /// joins). A city whose authored site pool cannot seed a round is a
    /// load failure.
    #[arg(
        long,
        value_name = "ffa|cops|robbers",
        conflicts_with_all = ["event", "dev_world", "join"]
    )]
    cnr: Option<String>,

    /// Gold mass for `--cnr`: `weightless` (default), `quarter` or `half`.
    #[arg(long, value_name = "weightless|quarter|half", requires = "cnr")]
    cnr_gold: Option<String>,

    /// Match limit for `--cnr`: `none` (default), a time limit
    /// `5m|10m|20m|30m` or a point limit `100pts|250pts|500pts|1000pts`.
    #[arg(long, value_name = "none|<n>m|<n>pts", requires = "cnr")]
    cnr_limit: Option<String>,

    /// Drive the Professional parameter block instead of Amateur.
    #[arg(long)]
    pro: bool,

    /// Stock/modded vehicle id or unique display-name alias to load
    /// (`--list-cars` shows the roster). Requires `--mm2-path` or mods
    /// providing vehicle data.
    #[arg(long)]
    car: Option<String>,

    /// Paint variant for the selected vehicle (zero-based index).
    /// Defaults to the bound profile's remembered paint, else 0.
    #[arg(long)]
    paint: Option<usize>,

    /// Bind a driver profile by id (`driver-<n>`) or unique display
    /// name: its remembered vehicle/paint and rank apply unless
    /// `--car`/`--paint`/`--pro` override, the session's selections are
    /// saved back to it, and it becomes the store's `active` profile
    /// for later runs.
    #[arg(long, value_name = "id|name", conflicts_with = "new_profile")]
    profile: Option<String>,

    /// Create a new driver profile and bind it. `--pro` fixes its rank
    /// at Professional; `--sandbox` makes it a dev identity whose
    /// results never feed progression.
    #[arg(long, value_name = "name")]
    new_profile: Option<String>,

    /// Create the `--new-profile` driver as a sandbox profile (F16):
    /// selections still persist, but its results are ineligible for
    /// rewards and records.
    #[arg(long, requires = "new_profile")]
    sandbox: bool,

    /// Directory holding the profile store instead of the OS user-data
    /// directory. Passing it also enables profile binding on
    /// smoke/evidence runs (which otherwise never touch the store).
    #[arg(long, value_name = "dir")]
    profile_dir: Option<PathBuf>,

    /// Run without a profile — no profile reads or writes even when an
    /// `active` profile exists.
    #[arg(long, conflicts_with_all = ["profile", "new_profile", "profile_dir"])]
    no_profile: bool,

    /// Print the discovered vehicle roster and exit without opening a
    /// window.
    #[arg(long)]
    list_cars: bool,

    /// Load `--city` (or the `--cue` waves) through the production
    /// loader without a window, print which source (the original or a
    /// mod) served each file it read, and exit. The city and audio
    /// counterpart of `mm2-inspect deps` for cars and events.
    #[arg(
        long,
        conflicts_with_all = ["list_cars", "dev_world", "event", "cnr", "join", "headless"]
    )]
    trace_deps: bool,

    /// With `--trace-deps` instead of `--city`: resolve this cue wave stem
    /// (a cardata sample name such as `roadskid1`, repeatable) through the
    /// production `WaveBank` and decode it, so the trace credits the
    /// source that serves the audio file.
    #[arg(
        long = "cue",
        value_name = "stem",
        requires = "trace_deps",
        conflicts_with = "city"
    )]
    cue: Vec<String>,

    /// With `--trace-deps` instead of `--city`: like `--cue`, but resolve
    /// the stem as a siren-program sample (the `sirens/`-scoped wave map,
    /// repeatable).
    #[arg(
        long = "siren-cue",
        value_name = "stem",
        requires = "trace_deps",
        conflicts_with = "city"
    )]
    siren_cue: Vec<String>,

    /// With `--trace-deps`: exit non-zero unless the load read at least
    /// one file from this mod (repeatable) — proof the mod is live for
    /// the city.
    #[arg(long = "expect-mod", requires = "trace_deps")]
    expect_mod: Vec<String>,

    /// Optional TOML vehicle tuning file. With `--car` it is a full
    /// handling override applied *after* the import (wheel positions and
    /// radii stay pinned to the imported rig; a wheel-count mismatch is
    /// rejected). Without `--car` it configures the synthetic dev car.
    /// A requested file that fails to load or validate is an error, never
    /// a silent fallback.
    #[arg(long)]
    vehicle_config: Option<PathBuf>,

    /// Save a screenshot of the primary window after `--frames` frames and
    /// exit (visual smoke testing). The run waits for the capture to
    /// actually land on disk before reporting `pass`.
    #[arg(long, requires = "frames", conflicts_with = "headless")]
    screenshot: Option<PathBuf>,

    /// Frames to run before taking the screenshot / exiting in smoke mode.
    #[arg(long)]
    frames: Option<u32>,

    /// Record every frame's wall time — split into fixed-step physics,
    /// `Update` and render/present — to this CSV when the app exits, and
    /// print a percentile summary (diagnostic aid for stutter reports:
    /// play the event, then read the file). Works with or without
    /// `--frames`; meaningless headless.
    #[arg(long, value_name = "csv", conflicts_with = "headless")]
    perf_log: Option<PathBuf>,

    /// Shadow quality for this run, over the saved Options setting
    /// (`off`, `low` or `high`; the default is `high`). A capture or a
    /// measurement names what it rendered with; the saved file changes
    /// only if the Options screen edits it afterwards.
    #[arg(long, value_enum, conflicts_with = "headless")]
    shadows: Option<settings::ShadowQuality>,

    /// Anti-aliasing for this run, over the saved Options setting
    /// (`off`, `2` or `4` samples; the default is `4`). Like `--shadows`,
    /// it is not saved.
    #[arg(long, value_enum, conflicts_with = "headless")]
    msaa: Option<settings::Antialiasing>,

    /// Present without vsync (`AutoNoVsync`, i.e. `Immediate` where the
    /// surface has it) for this run, whatever the saved VSync setting
    /// says, so frame time reports what the CPU and GPU cost instead of
    /// the display's refresh interval.
    /// Pair with `--perf-log` to measure; not for playing (it tears).
    #[arg(long, conflicts_with = "headless")]
    no_vsync: bool,

    /// Start with the free camera active at `x,y,z[,yaw-deg,pitch-deg]`
    /// (screenshot/diagnostic aid).
    #[arg(long, value_name = "x,y,z[,yaw,pitch]", conflicts_with = "headless")]
    cam: Option<String>,

    /// Spawn the player vehicle at `x,y,z[,yaw-deg]` instead of the
    /// world/authored start (diagnostic aid; yaw 0 faces -Z). Replaces
    /// the roam spawn and any authored event slot.
    #[arg(long, value_name = "x,y,z[,yaw]")]
    spawn: Option<String>,

    /// Bound simultaneously active bangers at `n` instead of the
    /// recovered ×32 default (diagnostic aid — exercises pool reclaim
    /// without needing 32 real collisions).
    #[arg(long, value_name = "n")]
    banger_pool: Option<usize>,

    /// Pause the session once it reaches `Playing` (diagnostic aid —
    /// a `--frames`/`--screenshot` capture freezes live input, so this
    /// is how the pause overlay gets rendered). Meaningless headless.
    #[arg(long, conflicts_with = "headless")]
    pause: bool,

    /// Pause the session once it reaches `Playing` with the
    /// full-screen HUD map up (diagnostic aid — how a capture renders
    /// HUD-4's Q pause map while live input is frozen; render-only like
    /// `--pause`). Headless it lands on the record's `map=` field
    /// instead of a screen.
    #[arg(long, conflicts_with = "pause")]
    pause_map: bool,

    /// Start in the authored cockpit/dash view (diagnostic aid — how a
    /// `--frames`/`--screenshot` capture renders the F22-B.1 interior;
    /// render-only like `--cam`). Falls back to the chase camera when
    /// the vehicle carries no `camPovCS` record.
    #[arg(long, conflicts_with = "cam")]
    cockpit: bool,

    /// Start on the authored `_far.camtrackcs` chase lens (diagnostic
    /// aid — how a `--frames`/`--screenshot` capture renders the F22-B.3
    /// far view; render-only like `--cam`). Falls back to the chase
    /// camera's near lens when the vehicle carries no far record.
    #[arg(long, conflicts_with_all = ["cam", "cockpit"])]
    far: bool,

    /// Start with the rear-view mirror strip up (diagnostic aid — how a
    /// `--frames`/`--screenshot` capture renders the F22-B.2 mirror
    /// while live input is frozen; render-only like `--cam`). Headless
    /// it lands on the record's `mir=` field instead of a screen.
    #[arg(long)]
    mirror: bool,

    /// Start with the driving HUD switched off — the state the
    /// documented `H` toggle produces (diagnostic aid — how a
    /// `--frames`/`--screenshot` capture renders the F22-A.3 HUD-off
    /// view while live input is frozen; render-only like `--cam`).
    /// Headless it lands on the record's `hud=` field instead of a
    /// screen.
    #[arg(long)]
    no_hud: bool,

    /// Sweep the local participant through an event session's remaining
    /// triggers — one gate per update — until the run resolves to the
    /// results screen (diagnostic aid: how a `--frames`/`--screenshot`
    /// capture reaches `Results` while live input is frozen; the
    /// results it produces are record-ineligible). No-op outside an
    /// event session.
    #[arg(long, conflicts_with = "bot")]
    finish: bool,

    /// Queue the session's own restart intent on the first `Playing`
    /// frame (diagnostic aid — exercises the production
    /// `Unloading → Menu → begin` teardown path, the same lifecycle a
    /// disabled-in-Blitz/Checkpoint restart takes; the restarted run is
    /// record-ineligible). One-shot.
    #[arg(long)]
    restart: bool,

    /// Queue the session's restart intent once the session clock
    /// reaches `ticks` fixed steps (60 Hz — the `smoke` record's
    /// `ticks=` field): a delayed `--restart` for evidence legs that
    /// need real race progress banked before the teardown. The
    /// restarted run is record-ineligible. One-shot.
    #[arg(long, value_name = "ticks")]
    restart_at: Option<u64>,

    /// Teleport the player vehicle (and any trailer) back to the
    /// spawn point once the session clock reaches `ticks` fixed
    /// steps — the scheduled form of the `R` reset key for
    /// `--frames`/`--screenshot` captures where live input is frozen
    /// (diagnostic aid for the reset-transition camera leg; the run
    /// is record-ineligible). One-shot. On a `--join` client the
    /// teleport is the host's to grant, so the flag sends the same
    /// `ResetRequest` the `R` key does.
    #[arg(long, value_name = "ticks")]
    reset_at: Option<u64>,

    /// Destroy a car through the damage pipeline once the session
    /// clock reaches `ticks` fixed steps: the local car, or the one
    /// `--wreck-seat` names. It takes the mode's real disabled outcome
    /// (a Blitz/Checkpoint breakdown, a Cruise reset, ...), so a
    /// multi-process leg can watch a remote human's wreck without
    /// driving into a wall. Authority only (inert on a `--join`
    /// client; the host decides damage); the run is record-ineligible.
    /// One-shot.
    #[arg(long, value_name = "ticks")]
    wreck_at: Option<u64>,

    /// With `--wreck-at`: wreck the car on this wire seat (the host is
    /// 0, joined clients take roster ids from 1) instead of the local
    /// one.
    #[arg(long, value_name = "id", requires = "wreck_at")]
    wreck_seat: Option<u16>,

    /// Advance the documented `C` view chain once when the session
    /// clock reaches `ticks` fixed steps — how a
    /// `--frames`/`--screenshot` capture inspects a mid-drive camera
    /// transition while live input is frozen. Render-only like
    /// `--far`/`--cockpit`. One-shot.
    #[arg(long, value_name = "ticks")]
    cam_cycle_at: Option<u64>,

    /// Press the local vehicle's authored horn once on the first
    /// `Playing` frame (diagnostic aid — exercises the F07 voice path
    /// in a `--frames` capture where live input is frozen; the record's
    /// `aud=` field reports presses, voices and attached sinks).
    /// Headless runs count the voice but never attach a sink.
    #[arg(long)]
    horn: bool,

    /// Weather selector for the session's conditions — the authored
    /// 0-3 grid value (`clear`/`cloudy`/`foggy`/`rainy` on the measured
    /// ltNN grid, WLD-21). Sets the cruise session's environment; an
    /// authored event's own conditions take precedence (RACE-2).
    #[arg(long, value_name = "0-3")]
    weather: Option<u8>,

    /// Time-of-day selector — the authored 0-3 grid value
    /// (`morning`/`noon`/`evening`/`night` on the measured ltNN grid).
    #[arg(long, value_name = "0-3")]
    time_of_day: Option<u8>,

    /// Multiply every tire contact's grip by `f` for the session — pins
    /// the environment traction modifier outright, overriding the
    /// weather-derived wetness factor (so `--traction 1` dries a rainy
    /// session). Must be finite and non-negative.
    #[arg(long, value_name = "f")]
    traction: Option<f32>,

    /// Run without a window or GPU: simulate `--frames` updates
    /// (default 600), print a `smoke=headless-physics` record and exit.
    #[arg(long)]
    headless: bool,

    /// End a `--headless` run once this process has applied `n`
    /// replicated impact rows, instead of waiting out `--frames` (the
    /// ceiling). A multi-process leg waits on the wire, and a frame
    /// count is not a clock. The record prints `stop=impacts`.
    #[arg(long, value_name = "n", requires = "headless")]
    until_impacts: Option<u64>,

    /// End a `--headless` run once this process has observed `n`
    /// authority resets (`net=` `rst<n>`: on a client, epoch-declared
    /// teleports applied to its own seat or a copy). The record prints
    /// `stop=resets`.
    #[arg(long, value_name = "n", requires = "headless")]
    until_resets: Option<u64>,

    /// End a `--headless` run once `n` engine impairment episodes on
    /// this process have been repaired (the record's `imp=` `<n>r`: on a
    /// predicted client, the authority repairing the wire's damage
    /// byte). The record prints `stop=repaired`.
    #[arg(long, value_name = "n", requires = "headless")]
    until_repaired: Option<u64>,

    /// End a `--headless` run once a remote copy it held is gone — a
    /// peer left the session. The record prints `stop=peer-left`.
    #[arg(long, requires = "headless")]
    until_peer_left: bool,

    /// With `--until-peer-left`: also wait until this process has
    /// emitted `n` impacts of its own (the record's `impacts=`).
    #[arg(
        long,
        value_name = "n",
        requires = "until_peer_left",
        default_value_t = 0
    )]
    with_impacts: u64,

    /// Wall-clock bound, in seconds, on a `--headless` run's
    /// `--until-*` condition: the run ends when it passes, whether or
    /// not the condition came. The record prints `stop=deadline`.
    #[arg(long, value_name = "secs", requires = "headless")]
    deadline: Option<u64>,

    /// Scripted course-follower: the player vehicle steers at the live
    /// race objective (the RACE-6 target / next ordered gate) instead of
    /// waiting for keyboard/gamepad input. An evidence driver for
    /// completions — an event session still goes through the real
    /// countdown, checkpoint and result path.
    #[arg(long)]
    bot: bool,

    /// Limit the evidence driver's forward speed (m/s), using throttle/brake inputs.
    #[arg(long, requires = "bot", value_parser = parse_bot_speed)]
    bot_speed: Option<f32>,

    /// Explicit native .opp driving guide for evidence runs; never creates an opponent.
    #[arg(long, requires_all = ["bot", "event"], value_parser = parse_bot_route)]
    bot_route: Option<PathBuf>,

    /// Stationary control: the player vehicle holds its handbrake for
    /// the whole session — it never races, so an event run measures what
    /// the opponents do with no competing local driver (the isolation
    /// leg `--bot`'s driving and the default driver's blind full
    /// throttle cannot provide). Works windowed too.
    #[arg(long, conflicts_with = "bot")]
    parked: bool,

    /// Pursuit control (F25-C): the player vehicle holds full throttle
    /// and steers at the nearest other vehicle — a driven car-to-car
    /// collision for multi-process runs, whose neighbours are `--parked`
    /// victims. An evidence driver, not a gameplay feature. Works
    /// windowed too.
    #[arg(long, conflicts_with_all = ["bot", "parked", "seq"])]
    ram: bool,

    /// Staged audio-sequence driver (F07-AC02): the player vehicle
    /// runs a scripted idle → accelerate → coast → brake → reverse
    /// program through the production input path, and the headless
    /// record's `seq=` field reports the per-stage drivetrain,
    /// engine-mix and clutch evidence. An evidence driver, not a
    /// gameplay feature. Works windowed too.
    #[arg(long, conflicts_with_all = ["bot", "parked"])]
    seq: bool,

    /// Disable the authored `.cpvs` room-PVS render culling (F18-A.5)
    /// — the retail `cityLevel::EnablePVS(false)` counterpart and the
    /// escape hatch for comparing culled vs unculled captures.
    #[arg(long)]
    no_pvs: bool,

    /// Draw the city's BAI navigation graph over the imported geometry
    /// (F09-B debug overlay): lane polylines, travel-direction
    /// chevrons, intersection markers and aimap-closed roads in red.
    #[arg(long)]
    nav: bool,

    /// Draw each police car's pursuit state, the place it last saw its
    /// target and the road line it follows (F20 debug overlay): a pole
    /// over every cop coloured by phase, a line to the chase goal and
    /// the planned route.
    #[arg(long)]
    police_debug: bool,

    /// Line the stock pedestrian archetypes up in front of the player,
    /// each stepping through the authored animation states in place
    /// (F19-A.5 evidence view — not a crowd and not gameplay).
    #[arg(long)]
    ped_lab: bool,

    /// Highlight a route between two BAI road indices `<from>:<to>` on
    /// the nav overlay (implies --nav).
    #[arg(long, value_name = "from:to")]
    nav_route: Option<String>,

    /// Join a multiplayer lobby at `addr` (`host:port`, e.g. from an
    /// `mm2-host` `listening=` line) and park in it (F24-B). The
    /// session — world, mode, difficulty, conditions, seed — comes
    /// from the host's wire advertisement, so the session-shaping
    /// flags conflict; the local `--car`/`--paint` name the pick
    /// offered to the lobby, and evidence flags (`--frames`,
    /// `--headless`, `--bot`, `--cam`, …) still apply locally but
    /// never ride the wire.
    #[arg(
        long,
        value_name = "addr",
        conflicts_with_all = [
            "city",
            "event",
            "dev_world",
            "pro",
            "weather",
            "time_of_day",
            "vehicle_config",
            "menu",
        ]
    )]
    join: Option<SocketAddr>,

    /// Driver name on a joined lobby's roster (default: the bound
    /// profile's name, else `player`).
    #[arg(long, requires = "join")]
    driver: Option<String>,

    /// Mark ready as soon as the lobby admits us — with it the host's
    /// `start` can fire immediately — and again whenever the host
    /// closes a session, so the next round can start unattended.
    #[arg(long, requires = "join")]
    ready: bool,

    /// Host a multiplayer lobby inside the app (F24-B.8): the
    /// session-shaping flags name the advertised session — world,
    /// mode, difficulty, conditions, seed — and this process plays the
    /// host seat when `start` fires. Operator surface: stdin
    /// `start`/`cancel`/`quit`, or Enter/Esc on the windowed lobby
    /// line. Developer-override flags are never network-legal — a
    /// config carrying any refuses to advertise (exit 2).
    #[arg(long, conflicts_with_all = ["join", "menu"])]
    host: bool,

    /// Address the hosted lobby listens on (requires --host). The
    /// default is loopback + ephemeral port — a LAN/public bind is an
    /// explicit operator choice, never the default.
    #[arg(long, requires = "host", default_value = "127.0.0.1:0")]
    bind: SocketAddr,

    /// Address a lobby opened from the menu's *Host lobby* row listens
    /// on. Same rule as `--bind`: loopback + ephemeral port unless the
    /// operator names a wider bind. Only the menu reads it.
    #[arg(long, conflicts_with = "host", default_value = "127.0.0.1:0")]
    menu_bind: SocketAddr,

    /// Session seed the lobby replicates to every client — default is
    /// clock-derived like `mm2-host`; pass a value to reproduce a run.
    #[arg(long, requires = "host")]
    seed: Option<u64>,

    /// Force the menu front-end even alongside the capture flags —
    /// `--menu --frames N --screenshot out.png` renders the shell
    /// itself for visual evidence instead of launching a world.
    /// Session-shaping flags still take precedence: a requested session
    /// never parks in the menu, and `--menu` is ignored (with a
    /// warning) when one is present.
    #[arg(long, conflicts_with = "headless")]
    menu: bool,

    /// Open a menu screen directly for reproducible visual captures.
    #[arg(long, requires = "menu", value_parser = ["root", "profiles", "races", "garage", "options", "controls"])]
    menu_screen: Option<String>,
}

/// Smoke-test capture: run N frames, take the screenshot (if requested),
/// report a `smoke=visual` record, then exit.
#[derive(Resource)]
struct SmokeTest {
    /// `dev-world` or the city's logical path — the record's `world=`.
    world: String,
    screenshot: Option<PathBuf>,
    frames_left: u32,
    /// Capture whose on-disk arrival we're still waiting for.
    pending: Option<PathBuf>,
    /// Frames left to wait for `pending` before calling it a failure.
    capture_wait: u32,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let cli = Cli::parse();

    // `--cam x,y,z[,yaw,pitch]` starts the free camera at a fixed pose
    // (angles in degrees) — a diagnostic/screenshot aid.
    let cam_start = cli.cam.as_deref().map(|s| {
        let parts: Result<Vec<f32>, _> = s.split(',').map(|p| p.trim().parse::<f32>()).collect();
        match parts {
            Ok(f) if f.len() == 3 || f.len() == 5 => Ok(CameraPose {
                position: Vec3::new(f[0], f[1], f[2]),
                yaw: f.get(3).copied().unwrap_or(0.0).to_radians(),
                pitch: f.get(4).copied().unwrap_or(0.0).to_radians(),
            }),
            _ => Err(()),
        }
    });
    let cam_start = match cam_start {
        Some(Ok(c)) => Some(c),
        Some(Err(())) => {
            error!("invalid --cam: expected x,y,z[,yaw,pitch]");
            std::process::exit(2);
        }
        None => None,
    };

    // `--spawn x,y,z[,yaw-deg]` pins the player vehicle's start pose
    // (yaw in degrees, same convention as the roam spawn's
    // `Quat::from_rotation_y` angle).
    let spawn_pose = cli.spawn.as_deref().map(|s| {
        let parts: Result<Vec<f32>, _> = s.split(',').map(|p| p.trim().parse::<f32>()).collect();
        match parts {
            Ok(f) if f.len() == 3 || f.len() == 4 => Ok(mm2_game::SpawnPose {
                position: Vec3::new(f[0], f[1], f[2]),
                yaw: f.get(3).copied().unwrap_or(0.0).to_radians(),
            }),
            _ => Err(()),
        }
    });
    let spawn_pose = match spawn_pose {
        Some(Ok(p)) => Some(p),
        Some(Err(())) => {
            error!("invalid --spawn: expected x,y,z[,yaw]");
            std::process::exit(2);
        }
        None => None,
    };

    // `--traction f` pins the session's environment traction modifier —
    // quarantined in DevOverrides and *overriding* the session-legal
    // wetness writer (F18-B.1's weather→traction factor). A negative or
    // non-finite multiplier is a usage error, never a clamp.
    let traction = match cli.traction {
        Some(f) if f.is_finite() && f >= 0.0 => Some(f),
        Some(_) => {
            error!("invalid --traction: expected a finite, non-negative multiplier");
            std::process::exit(2);
        }
        None => None,
    };

    // `--weather`/`--time-of-day` set the session's cruise conditions —
    // session-legal selectors on the authored 0-3 grid (RACE-4). An
    // authored event's own conditions take precedence while it runs
    // (RACE-2), so the flags only feed the fallback there. Out-of-range
    // selectors are usage errors, never a clamp.
    let weather = match cli.weather.map(mm2_game::Weather::new).transpose() {
        Ok(w) => w.unwrap_or_default(),
        Err(e) => {
            error!("invalid --weather: {e}");
            std::process::exit(2);
        }
    };
    let time_of_day = match cli.time_of_day.map(mm2_game::TimeOfDay::new).transpose() {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => {
            error!("invalid --time-of-day: {e}");
            std::process::exit(2);
        }
    };
    if cli.event.is_some() && (cli.weather.is_some() || cli.time_of_day.is_some()) {
        warn!(
            "--weather/--time-of-day set the cruise fallback; the event's authored conditions take precedence (RACE-2)"
        );
    }

    // One mounting policy shared with mm2-inspect: mods > loose install
    // files > archives. The VFS is always built — mods work in the dev
    // world without an MM2 installation.
    let mut vfs = Vfs::new();
    let mut has_mm2 = false;
    if let Some(dir) = &cli.mm2_path {
        match mount_install(&mut vfs, dir, &InstallMount::default()) {
            Ok(report) => {
                info!(
                    archives = report.archives.len(),
                    skipped = report.skipped.len(),
                    loose = report.loose_files,
                    "mounted MM2 installation"
                );
                for (path, err) in &report.skipped {
                    warn!(archive = %path.display(), error = %err, "skipped archive");
                }
                has_mm2 = true;
            }
            Err(e) => {
                error!(path = %dir.display(), error = %e, "failed to mount MM2 installation");
                std::process::exit(2);
            }
        }
    }
    // The app's own synthetic assets (dev-world textures) sit above the
    // install content but below mods, so a mod can replace them.
    // Found next to the binary, not through the working directory, so the
    // app runs the same from any shell location (F30-AC04).
    let app_assets_dir = mm2_app::app_assets::locate_for_process();
    match &app_assets_dir {
        Some(app_assets) => match vfs.mount_dir(app_assets, mm2_assets::priority::OVERRIDE) {
            Ok(()) => info!(dir = %app_assets.display(), "mounted app assets"),
            Err(e) => warn!(dir = %app_assets.display(), error = %e, "failed to mount app assets"),
        },
        None => warn!(
            "app assets directory not found; the dev world ground falls back to a flat colour"
        ),
    }
    // F30-AC05: the app's own writes (profile store, perf log, screenshot)
    // never go into the original installation or the app's directory.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let write_guards = mm2_app::write_guard::protected_dirs(
        cli.mm2_path.as_deref(),
        app_assets_dir.as_deref(),
        exe_dir.as_deref(),
    );
    let cwd = std::env::current_dir().ok();
    let guard_write = |what: &'static str, path: &Path| {
        if let Err(refusal) = mm2_app::write_guard::check(what, path, &write_guards, cwd.as_deref())
        {
            error!(error = %refusal, "write destination refused");
            std::process::exit(2);
        }
    };
    if let Some(dir) = &cli.profile_dir {
        guard_write("the profile store", dir);
    }
    if let Some(path) = &cli.perf_log {
        guard_write("the performance log", path);
    }
    if let Some(path) = &cli.screenshot {
        guard_write("a screenshot", path);
    }
    // Cmd/Ctrl+P writes beside the working directory, which may be the
    // install itself: fall back to the user-data directory, else no hotkey.
    let user_screenshots = mm2_game::profile::ProfileStore::default_root()
        .and_then(|root| root.parent().map(|data| data.join("screenshots")));
    let screenshot_dir = mm2_app::write_guard::first_allowed(
        "a screenshot",
        Path::new(SCREENSHOT_DIR),
        user_screenshots.as_deref(),
        &write_guards,
        cwd.as_deref(),
    );
    match &screenshot_dir {
        Some(dir) if dir.as_path() != Path::new(SCREENSHOT_DIR) => warn!(
            dir = %dir.display(),
            "the working directory is protected; Cmd/Ctrl+P screenshots go to the user-data directory"
        ),
        Some(_) => {}
        None => {
            warn!("no writable screenshot directory outside the install; Cmd/Ctrl+P is disabled")
        }
    }
    // The default user-data root is checked too: an odd HOME/XDG_DATA_HOME
    // can put it inside the install. A refused root means no store at all
    // (an explicit `--profile-dir` was already refused above).
    let profile_root = match profile::guarded_store_root(
        cli.profile_dir.clone(),
        &write_guards,
        cwd.as_deref(),
    ) {
        Ok(root) => root,
        Err(refusal) => {
            error!(error = %refusal, "default profile store refused; running without persistence");
            None
        }
    };
    let mut has_mods = false;
    let mut mod_ids: Vec<String> = Vec::new();
    // Stays false (the safe answer) unless the mounted mods were
    // classified cosmetic-only below.
    let mut mods_cosmetic_only = false;
    // Why `--mods` mounted nothing, when it was asked to. Play carries on
    // without mods; `--trace-deps` must not, or its report would read as
    // "no mod serves this" for a directory that never mounted.
    let mut mods_problem: Option<String> = None;
    if let Some(mods) = &cli.mods {
        if !mods.is_dir() {
            mods_problem = Some(format!("{} is not a directory", mods.display()));
        }
        match mount_mods(&mut vfs, mods) {
            Ok(manifests) => {
                has_mods = !manifests.is_empty();
                mod_ids = manifests.iter().map(|m| m.id.clone()).collect();
                for m in &manifests {
                    info!(mod_id = %m.id, dir = %mods.display(), "mounted mod");
                }
                // F29 req 5: say what each mod can change. Gameplay mods
                // make records ineligible and move the multiplayer
                // fingerprint; cosmetic-only ones do neither.
                let reports = mm2_content::fingerprint::mod_reports(&vfs);
                for r in &reports {
                    if let Some(why) = r.contradiction() {
                        warn!(mod_id = %r.id, "mod manifest {why}; the files decide");
                    }
                    if r.is_cosmetic_only() {
                        info!(mod_id = %r.id, cosmetic = r.cosmetic, shadowed = r.shadowed, "mod is cosmetic-only");
                    } else {
                        info!(
                            mod_id = %r.id,
                            gameplay = r.gameplay,
                            cosmetic = r.cosmetic,
                            shadowed = r.shadowed,
                            example = r.example.as_deref().unwrap_or(""),
                            "mod changes gameplay content: records are not kept and multiplayer peers must run the same mods"
                        );
                    }
                }
                mods_cosmetic_only = reports
                    .iter()
                    .all(mm2_content::fingerprint::ModReport::is_cosmetic_only);
            }
            Err(e) => {
                warn!(dir = %mods.display(), error = %e, "failed to mount mods; running with none");
                mods_problem = Some(format!("{}: {e}", mods.display()));
            }
        }
    }

    // `--list-cars` needs the VFS only — no window, no GPU.
    if cli.list_cars {
        let catalog = VehicleCatalog::scan(&vfs);
        print_roster(&catalog, &mm2_content::scan_garage(&vfs));
        return;
    }

    // `--trace-deps` needs the VFS only; the city loads into a throwaway
    // world, so no window or GPU is involved.
    if cli.trace_deps {
        if let Some(why) = &mods_problem {
            error!("--mods mounted nothing, so the trace would not show any mod: {why}");
            std::process::exit(2);
        }
        let (loaded, trace) = if cli.cue.is_empty() && cli.siren_cue.is_empty() {
            let Some(city) = cli.city.as_deref() else {
                error!(
                    "--trace-deps needs a target: --city <stem>, --cue <stem> or --siren-cue <stem>"
                );
                std::process::exit(2);
            };
            let stem = city.to_ascii_lowercase();
            let (loaded, trace) =
                mm2_app::city::trace_city_reads(&vfs, &format!("city/{stem}.psdl"));
            (loaded.map_err(|e| e.to_string()), trace)
        } else {
            mm2_app::audio::trace_cue_reads(&vfs, &cli.cue, &cli.siren_cue)
        };
        print!("{}", trace.render());
        let label = match loaded {
            Ok(label) => label,
            Err(e) => {
                println!("load failed after {} read(s): {e}", trace.accesses.len());
                std::process::exit(2);
            }
        };
        print!("{}", trace.render_summary(&label));
        let absent = trace.absent_mods(&cli.expect_mod);
        if !absent.is_empty() {
            error!("no file was read from mod(s) {absent:?}");
            std::process::exit(2);
        }
        return;
    }

    // `--event <table>:<row>` selects an authored event in the chosen
    // city (default london). A `crash:<row>` loads as a Crash Course
    // lesson (F21-B.6); a row that cannot build fails the session
    // explicitly, never falls back to a plain race.
    let event_ref = cli.event.as_deref().map(|s| {
        match mm2_game::EventRef::parse(s, cli.city.as_deref().unwrap_or("london")) {
            Some(event_ref) => event_ref,
            None => {
                error!("invalid --event {s:?}: expected checkpoint|blitz|circuit|crash:<row>");
                std::process::exit(2);
            }
        }
    });

    // `--cnr` names the Cops & Robbers session mode; a name that is not
    // one of the host menu's choices is a usage error.
    let cnr_settings = cli.cnr.as_deref().map(|variant| {
        mm2_game::cnr_options::CnrSettings::parse(
            variant,
            cli.cnr_gold.as_deref(),
            cli.cnr_limit.as_deref(),
        )
        .unwrap_or_else(|e| {
            error!("invalid --cnr: {e}");
            std::process::exit(2);
        })
    });

    // World mode. A specifically requested `--city` always means City —
    // even without an install (a mod may provide it, and a VFS miss is a
    // hard failure rather than a silent dev world). `--event` implies
    // its city the same way. `--dev-world` wins over both.
    let mode = if cli.dev_world {
        if cli.city.is_some() {
            warn!("--city is ignored with --dev-world");
        }
        // `--event` still applies: an event over the dev world is a
        // valid developer/test rig — its data resolves through the
        // same VFS and missing data fails explicitly.
        WorldMode::DevWorld
    } else if cli.city.is_some() || cli.event.is_some() || has_mm2 {
        WorldMode::City {
            psdl: format!(
                "city/{}.psdl",
                cli.city.as_deref().unwrap_or("london").to_ascii_lowercase()
            ),
        }
    } else {
        warn!("no --mm2-path and no --dev-world; starting the dev world");
        WorldMode::DevWorld
    };
    let world_label = match &mode {
        WorldMode::DevWorld => "dev-world".to_string(),
        WorldMode::City { psdl } => psdl.clone(),
    };
    let smoke_requested = cli.headless || cli.frames.is_some() || cli.screenshot.is_some();
    let smoke_kind = if cli.headless {
        smoke::KIND_HEADLESS_PHYSICS
    } else {
        smoke::KIND_VISUAL
    };
    let record = |status: smoke::SmokeStatus, detail: String| smoke::SmokeRecord {
        kind: smoke_kind,
        world: world_label.clone(),
        status,
        detail,
    };
    if smoke_requested {
        println!("{}", smoke::header());
    }

    // F16-A: bind a driver profile. `--profile`/`--new-profile` (or a
    // `--profile-dir`) request one explicitly; an interactive run with
    // no profile flag still binds the store's `active` marker so a
    // chosen driver persists across launches. Smoke/evidence runs never
    // bind implicitly — their records must stay reproducible — but do
    // honor an explicit request. `--no-profile` opts out entirely.
    // Explicit failures are usage errors; implicit ones degrade to a
    // profile-less run.
    let profile_request = if let Some(name) = &cli.new_profile {
        Some(profile::ProfileRequest::Create {
            name: name.clone(),
            rank: if cli.pro {
                mm2_game::Difficulty::Professional
            } else {
                mm2_game::Difficulty::Amateur
            },
            kind: if cli.sandbox {
                mm2_game::ProfileKind::Sandbox
            } else {
                mm2_game::ProfileKind::Standard
            },
        })
    } else {
        cli.profile.clone().map(profile::ProfileRequest::Select)
    };
    let profile_explicit = profile_request.is_some() || cli.profile_dir.is_some();
    let active_profile = if cli.no_profile || (smoke_requested && !profile_explicit) {
        None
    } else {
        match profile_root.clone() {
            Some(root) => match mm2_game::ProfileStore::open(&root) {
                Ok(store) => {
                    let request = profile_request.unwrap_or(profile::ProfileRequest::Active);
                    match profile::resolve(&store, &request) {
                        Ok(bound) => bound,
                        Err(e) if profile_explicit => {
                            error!(error = %e, "profile request failed");
                            std::process::exit(2);
                        }
                        Err(e) => {
                            warn!(error = %e, "profile unavailable; running without one");
                            None
                        }
                    }
                }
                Err(e) if profile_explicit => {
                    error!(dir = %root.display(), error = %e, "cannot open profile store");
                    std::process::exit(2);
                }
                Err(e) => {
                    warn!(dir = %root.display(), error = %e, "profile store unavailable; running without a profile");
                    None
                }
            },
            None if profile_explicit => {
                error!("no profile store location determinable; pass --profile-dir");
                std::process::exit(2);
            }
            None => None,
        }
    };
    if let Some(slot) = &active_profile {
        info!(
            profile = %slot.profile.id,
            name = %slot.profile.name,
            recovered = slot.recovered_from_backup,
            "driver profile bound"
        );
    }

    // Vehicle selection: explicit `--car`, else the bound profile's
    // remembered vehicle — a soft preference that falls back to the
    // stock default when the saved id no longer resolves (changed
    // install, missing mod content) — else the documented stock default.
    let (vehicle_source, mut paint, difficulty) = profile::choose_launch(
        cli.car.as_deref(),
        cli.paint,
        cli.pro,
        active_profile.as_ref().map(|p| &p.profile),
    );
    let mut selected: Option<VehicleDef> = None;
    match vehicle_source {
        profile::VehicleSource::Explicit(query) => {
            match mm2_content::load_by_id(&vfs, query, paint) {
                Ok(def) => {
                    info!(car = %def.id, name = %def.display_name, paint, "vehicle loaded");
                    selected = Some(def);
                }
                Err(e) => {
                    error!(car = %query, error = %e, "vehicle failed to load");
                    if smoke_requested {
                        println!(
                            "{}",
                            record(smoke::SmokeStatus::Fail, format!("vehicle {query}: {e}"))
                                .line()
                        );
                    }
                    std::process::exit(2);
                }
            }
        }
        profile::VehicleSource::Remembered(choice) => {
            match mm2_content::load_by_id(&vfs, &choice.id, paint) {
                Ok(def) => {
                    info!(car = %def.id, name = %def.display_name, paint, "profile vehicle restored");
                    selected = Some(def);
                }
                Err(e) => match mm2_content::load_by_id(&vfs, &choice.id, 0) {
                    Ok(def) => {
                        warn!(car = %choice.id, error = %e, "saved paint unavailable; using paint 0");
                        paint = 0;
                        selected = Some(def);
                    }
                    Err(e) => {
                        warn!(car = %choice.id, error = %e, "remembered vehicle unavailable; using the stock default");
                    }
                },
            }
        }
        profile::VehicleSource::Default => {}
    }
    if selected.is_none() && cli.car.is_none() {
        if has_mm2 {
            match default_stock_car(&vfs, paint) {
                Some(def) => {
                    info!(car = %def.id, name = %def.display_name, "default stock vehicle loaded");
                    selected = Some(def);
                }
                None => {
                    warn!("no usable stock vehicle found; using the synthetic dev car");
                }
            }
        } else if cli.paint.is_some_and(|p| p != 0) {
            warn!("--paint has no effect without --car / an MM2 installation");
        }
    }

    // F16-B.3: `--car` and remembered selections bypass the menu that
    // will enforce garage gates in F17 — surface a locked, unlisted or
    // unrecorded choice honestly, mirroring the locked `--event` warn.
    if let (Some(slot), Some(def)) = (&active_profile, &selected)
        && let Some(note) = profile::vehicle_gate_note(&vfs, &slot.profile, &def.id, paint)
    {
        let reason = match note {
            profile::VehicleGateNote::Uncatalogued => "not in the vehicle catalog",
            profile::VehicleGateNote::Unlisted => "not on the select roster",
            profile::VehicleGateNote::Locked => "still locked for this profile",
            profile::VehicleGateNote::LockedPaint => "paint still locked for this profile",
        };
        warn!(car = %def.id, paint, "{reason}");
    }

    // Handling config: `--vehicle-config` is a full override applied after
    // the import when a car was selected; otherwise it tunes the dev car.
    let vehicle = match &cli.vehicle_config {
        Some(path) => match VehicleConfig::load(path) {
            Ok(cfg) => match &selected {
                Some(def) => match mm2_content::assemble::apply_handling_override(&def.config, cfg)
                {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        error!(error = %e, "incompatible --vehicle-config override");
                        std::process::exit(2);
                    }
                },
                None => cfg,
            },
            Err(e) => {
                error!(error = %e, "invalid --vehicle-config");
                std::process::exit(2);
            }
        },
        None => selected
            .as_ref()
            .map(|d| d.config.clone())
            .unwrap_or_default(),
    };
    for w in selected
        .as_ref()
        .map(|d| d.report.warnings.iter().chain(d.model.warnings.iter()))
        .into_iter()
        .flatten()
    {
        warn!(car = ?selected.as_ref().map(|d| d.id.as_str()), "{w}");
    }

    // `--nav-route from:to` probes the nav graph between two BAI road
    // indices and implies --nav.
    let nav_route = cli.nav_route.as_deref().map(|s| {
        match s
            .split_once(':')
            .and_then(|(a, b)| a.parse::<u16>().ok().zip(b.parse::<u16>().ok()))
        {
            Some(pair) => pair,
            None => {
                error!("invalid --nav-route: expected <from>:<to> road indices");
                std::process::exit(2);
            }
        }
    });
    let nav_overlay_cfg = if cli.nav || nav_route.is_some() {
        if cli.dev_world {
            warn!("--nav has no effect on the dev world");
        }
        Some(mm2_game::NavOverlay { route: nav_route })
    } else {
        None
    };

    // The session's typed configuration (F01-A): world + mode +
    // difficulty/conditions/densities/seed + vehicle + authority.
    // `world`, `vehicle`, `conditions` (F18-A.2 lighting) and the `dev`
    // overrides have runtime consumers today — the rest are the
    // contract F11+ builds against. Developer tweaks stay quarantined
    // in `dev`.
    let mut session_config = SessionConfig {
        world: mode,
        mode: match (&event_ref, cnr_settings) {
            (Some(event_ref), _) => mm2_game::SessionMode::Event(event_ref.clone()),
            (None, Some(settings)) => mm2_game::SessionMode::CopsAndRobbers(settings),
            (None, None) => mm2_game::SessionMode::Cruise,
        },
        // The bound profile's rank supplies the difficulty unless
        // `--pro` overrides (DRV-2/3); no profile keeps Amateur.
        difficulty,
        // The cruise/dev conditions fallback (F18-A) — an authored
        // event's own conditions take precedence while it runs (RACE-2).
        conditions: mm2_game::SessionConditions {
            time_of_day,
            weather,
        },
        vehicle: VehicleSelection {
            id: selected.as_ref().map(|d| d.id.clone()),
            paint,
        },
        dev: DevOverrides {
            vehicle_config: cli.vehicle_config.clone(),
            camera: cam_start,
            nav_overlay: nav_overlay_cfg,
            spawn: spawn_pose,
            banger_pool: cli.banger_pool,
            traction,
            pause: cli.pause,
            pause_map: cli.pause_map,
            finish: cli.finish,
            restart: cli.restart,
            restart_at: cli.restart_at,
            reset_at: cli.reset_at,
            wreck_at: cli.wreck_at,
            wreck_seat: cli.wreck_seat,
            bot_speed: cli.bot_speed,
            bot_route: cli.bot_route.clone(),
            no_pvs: cli.no_pvs,
            horn: cli.horn,
            cockpit: cli.cockpit,
            far: cli.far,
            cam_cycle_at: cli.cam_cycle_at,
            mirror: cli.mirror,
            no_hud: cli.no_hud,
            police_debug: cli.police_debug,
            ped_lab: cli.ped_lab,
        },
        // A mounted mod that changes gameplay content makes
        // records/unlocks ineligible — a result under it is not
        // comparable to stock (designed policy); cosmetic-only mods
        // (F29 req 5) leave eligibility alone.
        mods_active: has_mods,
        mods_cosmetic_only,
        ..SessionConfig::default()
    };

    // `--join`: park at `Menu` inside a joined lobby instead of
    // beginning `session_config` — the host's wire advertisement owns
    // world/mode/difficulty/conditions/seed (F24-B.7). Of the launch
    // config only `vehicle` (the pick we offer the roster) and `dev`
    // (this process's local-only overrides) still carry over. Join is
    // a handshake-bounded call; a refused/unreachable lobby is a named
    // failure, never a silent local session.
    let mut lobby = if let Some(addr) = cli.join {
        let fingerprint = match mm2_content::fingerprint::gameplay(&vfs) {
            Ok(fp) => fp,
            Err(e) => {
                error!(error = %e, "fingerprinting content for --join");
                std::process::exit(2);
            }
        };
        let driver = cli
            .driver
            .clone()
            .or_else(|| active_profile.as_ref().map(|s| s.profile.name.clone()))
            .unwrap_or_else(|| "player".to_string());
        if driver.len() > mm2_net::MAX_STRING {
            error!(
                "--driver is {} bytes; the wire bound is {}",
                driver.len(),
                mm2_net::MAX_STRING
            );
            std::process::exit(2);
        }
        let hello = mm2_net::hello(smoke::COMMIT.to_string(), driver, fingerprint.hash);
        let link = match net::LobbyLink::join(addr, &hello, has_mods, session_config.dev.clone()) {
            Ok(link) => link.keep_ready(cli.ready),
            Err(e) => {
                error!(peer = %addr, error = %e, "join failed");
                std::process::exit(1);
            }
        };
        // Offer our pick immediately — the roster should show what
        // we'll drive, and the host's catalog gate confirms it can
        // spawn. `--ready` opts into the start gate.
        match net::encode_pick(&session_config.vehicle) {
            Ok(pick) => drop(link.ctl().set_vehicle(&pick.vehicle, pick.paint)),
            Err(e) => {
                error!(error = %e, "cannot offer the --car/--paint pick");
                std::process::exit(2);
            }
        }
        if cli.ready {
            let _ = link.ctl().set_ready(true);
        }
        info!(peer = %addr, driver = %link.driver(), id = link.player_id(), "joined lobby");
        Some(link)
    } else {
        None
    };

    // `--host`: park at `Menu` hosting a lobby — the operator's `start`
    // mints the generation the host seat and every client begin under
    // (F24-B.8). The flag-time gates are the same `mm2-host` runs: the
    // advertised world must resolve through the VFS, an authored event
    // must survive `event_race_setup`, the catalog feeds the roster's
    // pick validator, and `advertise` refuses a config carrying dev
    // overrides — never network-legal.
    let mut host_link = if cli.host {
        session_config.seed = cli.seed.unwrap_or_else(net::fresh_seed);
        if let WorldMode::City { psdl } = &session_config.world
            && vfs.resolve(psdl).is_none()
        {
            error!(city = %psdl, "cannot host a world the VFS cannot resolve");
            std::process::exit(2);
        }
        if let mm2_game::SessionMode::Event(event_ref) = &session_config.mode
            && let Err(e) = race::event_race_setup(&vfs, event_ref, session_config.difficulty)
        {
            error!(error = %e, "--event cannot run on this install");
            std::process::exit(2);
        }
        if let Err(e) = session_config.validate() {
            error!(error = %e, "invalid session configuration");
            std::process::exit(2);
        }
        if matches!(
            &session_config.mode,
            mm2_game::SessionMode::CopsAndRobbers(_)
        ) && let Err(e) = net::check_session(&vfs, &session_config)
        {
            error!(error = %e, "--cnr cannot run on this install");
            std::process::exit(2);
        }
        let fingerprint = match mm2_content::fingerprint::gameplay(&vfs) {
            Ok(fp) => fp,
            Err(e) => {
                error!(error = %e, "fingerprinting content for --host");
                std::process::exit(2);
            }
        };
        let catalog = VehicleCatalog::scan(&vfs);
        let driver = active_profile
            .as_ref()
            .map(|s| s.profile.name.clone())
            .unwrap_or_else(|| "player".to_string());
        let link = match net::HostLink::open(
            cli.bind,
            &session_config,
            driver,
            fingerprint.hash,
            Some(net::vehicle_validator(&catalog)),
        ) {
            Ok(link) => link,
            Err(e) => {
                error!(error = %e, "hosting failed");
                std::process::exit(2);
            }
        };
        println!(
            "listening={} fingerprint={} seed={} session={:?}",
            link.addr(),
            fingerprint.display(),
            session_config.seed,
            link.summary()
        );
        // stdin `start`/`cancel`/`quit` reaches the lobby update loop
        // from whatever context the app runs in — headless and
        // windowed alike.
        net::stdin_commands(link.command_sender());
        Some(link)
    } else {
        None
    };

    // Capability checks with their own status: a requested city with no
    // data source at all is `unavailable` (missing data), and a visual
    // smoke with no display is `unavailable` (no GPU/windowing). Neither
    // is a failure — and neither is allowed to fake a pass.
    if smoke_requested {
        if matches!(&session_config.world, WorldMode::City { .. }) && !has_mm2 && !has_mods {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Unavailable,
                    "requested city needs MM2 data: pass --mm2-path or --mods".into(),
                )
                .line()
            );
            std::process::exit(smoke::SmokeStatus::Unavailable.exit_code());
        }
        if !cli.headless && !display_available() {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Unavailable,
                    "no display detected (DISPLAY/WAYLAND_DISPLAY unset)".into(),
                )
                .line()
            );
            std::process::exit(smoke::SmokeStatus::Unavailable.exit_code());
        }
    }

    // Headless physics smoke: no window, no GPU. Runs and exits here —
    // `vfs`/`selected` move in, the process exits on the record. A
    // joined lobby parks there too: `headless_lobby` waits on the wire
    // — the host's `Start` is what builds a world.
    if cli.headless {
        let driver = if cli.bot {
            smoke::Driver::Scripted
        } else if cli.parked {
            smoke::Driver::Parked
        } else if cli.ram {
            smoke::Driver::Ram
        } else if cli.seq {
            smoke::Driver::Sequence
        } else {
            smoke::Driver::Hold
        };
        let budget = smoke::RunBudget {
            frames: cli.frames.unwrap_or(600),
            stop: smoke::StopWhen {
                impacts_applied: cli.until_impacts,
                resets_observed: cli.until_resets,
                repaired: cli.until_repaired,
                peer_left: cli.until_peer_left,
                peer_left_after_impacts: cli.with_impacts,
                deadline: cli.deadline.map(Duration::from_secs),
            },
        };
        let car = SelectedCar {
            def: selected,
            paint,
        };
        let rec = if let Some(link) = lobby.take() {
            smoke::headless_lobby(link, vfs, car, &vehicle, budget, driver, active_profile)
        } else if let Some(link) = host_link.take() {
            smoke::headless_host(link, vfs, car, &vehicle, budget, driver, active_profile)
        } else {
            // The stop conditions watch the wire; a lone process has none,
            // and `headless_smoke` runs exactly `--frames` updates.
            if budget.stop.is_armed() {
                println!(
                    "{}",
                    record(
                        smoke::SmokeStatus::Fail,
                        "--until-impacts/--until-resets/--until-repaired/--until-peer-left/--deadline need --join or --host: \
                         a single-process run only counts --frames"
                            .into(),
                    )
                    .line()
                );
                std::process::exit(smoke::SmokeStatus::Fail.exit_code());
            }
            smoke::headless_smoke(
                &session_config,
                vfs,
                car,
                &vehicle,
                budget.frames,
                driver,
                active_profile,
            )
        };
        println!("{}", rec.line());
        std::process::exit(rec.status.exit_code());
    }

    // F17-A.1: a bare `mm2` (with content but no explicit session
    // request) boots into the menu front-end — profile/mode/content
    // selection over the real catalogs, then `Session::begin`. Any
    // session-shaping flag (`--city`, `--event`, `--dev-world`,
    // `--spawn`, `--cam`, a tuning/physics override, `--bot`, the nav
    // overlay) stays a direct launch, as does every smoke/evidence
    // run: their records must stay reproducible and unattended. The
    // one exception is `--menu`, which pairs with the capture flags to
    // render the shell itself (`--frames`/`--screenshot` become a menu
    // visual smoke instead of a world one). A joined lobby owns the
    // `Menu` parking spot instead of the shell — `--join` already
    // conflicts `--menu`, and its quit path returns to the lobby.
    let menu_mode = (!smoke_requested || cli.menu)
        && lobby.is_none()
        && host_link.is_none()
        && cli.city.is_none()
        && cli.event.is_none()
        && !cli.dev_world
        && cli.spawn.is_none()
        && cli.cam.is_none()
        && cli.vehicle_config.is_none()
        && cli.banger_pool.is_none()
        && cli.traction.is_none()
        && cli.weather.is_none()
        && cli.time_of_day.is_none()
        && !cli.pause
        && !cli.pause_map
        && !cli.finish
        && !cli.restart
        && cli.restart_at.is_none()
        && !cli.horn
        && !cli.no_pvs
        && !cli.mirror
        && !cli.no_hud
        && !cli.nav
        && !cli.police_debug
        && !cli.ped_lab
        && cli.nav_route.is_none()
        && !cli.bot
        && !cli.parked
        && !cli.ram
        && !cli.seq;
    if cli.menu && !menu_mode {
        warn!("--menu ignored: a session-shaping flag requested a direct launch");
    }

    // Menu → Loading: the session resource the app drives through
    // `SessionPhase` transitions (`load_session_world` takes it to
    // Ready → Playing, or Failed). An invalid config is a usage error,
    // not a smoke fail. In menu mode the session parks at `Menu` and
    // the shell owns `begin`.
    let mut session = Session::new();
    if !menu_mode
        && lobby.is_none()
        && host_link.is_none()
        && let Err(e) = session.begin(session_config)
    {
        error!(error = %e, "invalid session configuration");
        std::process::exit(2);
    }

    // Menu seeding — computed before `selected`/`active_profile` move
    // into the app. The store handle is shared with the bound profile
    // when there is one so the Driver screen can list/create/delete;
    // `--no-profile` keeps it off entirely.
    let menu_vehicle = VehicleSelection {
        id: selected.as_ref().map(|d| d.id.clone()),
        paint,
    };
    let menu_bound = active_profile.as_ref().map(|s| s.profile.clone());
    let menu_store = if cli.no_profile {
        None
    } else {
        active_profile
            .as_ref()
            .map(|s| s.store.clone())
            .or_else(|| {
                profile_root.clone().and_then(|root| {
                    match mm2_game::ProfileStore::open(&root) {
                        Ok(store) => Some(store),
                        Err(e) => {
                            warn!(dir = %root.display(), error = %e, "profile store unavailable for the menu");
                            None
                        }
                    }
                })
            })
    };

    // Graphics settings live in `settings.json` in the profile store's
    // root and belong to the machine, not the driver, so `--no-profile`
    // does not hide them. Evidence runs (`--frames`/`--screenshot`)
    // neither read nor write the user's file unless `--profile-dir`
    // names one — a capture must not depend on, or change, how the
    // person who ran it plays. `--shadows`/`--msaa` override for the run.
    let settings_path = (!smoke_requested || cli.profile_dir.is_some())
        .then(|| profile_root.clone())
        .flatten()
        .map(|root| settings::settings_path(&root));
    let on_disk = settings_path
        .as_deref()
        .map(settings::GraphicsSettings::load)
        .unwrap_or_default();
    let mut graphics = on_disk;
    if let Some(shadows) = cli.shadows {
        graphics.shadows = shadows;
    }
    if let Some(antialiasing) = cli.msaa {
        graphics.antialiasing = antialiasing;
    }
    if cli.no_vsync {
        graphics.vsync = false;
    }
    // The flags are for this run: a later save must not write them into
    // the file.
    let run_overrides = settings::RunOverrides::between(on_disk, graphics);

    // Driving controls persist beside it as `controls.json` under the
    // same evidence-run rule: a capture neither reads nor writes them.
    let controls_path = settings_path
        .as_deref()
        .and_then(Path::parent)
        .map(controls::controls_path);
    let control_settings = controls_path
        .as_deref()
        .map(controls::ControlSettings::load)
        .unwrap_or_default();

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "rust-mm2".into(),
                    resolution: graphics.window_size.logical().into(),
                    // Built from the settings so the first frame already
                    // has the saved mode (`apply_display_settings` then
                    // finds nothing to change).
                    present_mode: graphics.present_mode(),
                    mode: graphics.display.window_mode(),
                    ..default()
                }),
                ..default()
            })
            .disable::<bevy::log::LogPlugin>(),
    )
    .add_plugins(PhysicsPlugins::default())
    .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
    .insert_resource(Gravity(
        Vec3::NEG_Y * if cli.mm2_path.is_some() { 19.6 } else { 9.81 },
    ))
    .insert_resource(ClearColor(Color::srgb(0.5, 0.65, 0.85)))
    .insert_resource(session)
    .insert_resource(SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
    .insert_resource(Mm2Vfs(vfs))
    .insert_resource(TunedVehicle(vehicle))
    .insert_resource(SelectedCar {
        def: selected,
        paint,
    })
    .init_resource::<car_visual::HeadlightsOn>()
    .insert_resource(if cam_start.is_some() {
        CameraMode::Free
    } else if cli.cockpit {
        CameraMode::Cockpit
    } else if cli.far {
        CameraMode::ChaseFar
    } else {
        CameraMode::Chase
    })
    // F22-B.2: `--mirror` arms the rear-view strip from spawn — the
    // capture path for it while live input is frozen.
    .insert_resource(camera::RearView(cli.mirror))
    // F22-A.2: the opponent-indicator toggle — session-agnostic like
    // `RearView`, on by designed default (HUD-3's `I` flips it).
    .init_resource::<oppind::OpponentIndicators>()
    // F22-A.3: the HUD master gate — session-agnostic like the other
    // instrument toggles; `--no-hud` starts it off for captures.
    .insert_resource(hud::HudVisible(!cli.no_hud))
    .insert_resource(ScreenshotDir(screenshot_dir))
    .add_plugins(VehiclePlugin)
    .insert_resource(graphics)
    .insert_resource(control_settings.clone())
    .insert_resource(controls::ControlsSave(controls_path.clone()))
    .insert_resource(
        settings::SettingsFile::new(settings_path.clone()).with_overrides(run_overrides.clone()),
    )
    .add_plugins(settings::GraphicsSettingsPlugin)
    .add_plugins(display_trial::DisplayTrialPlugin)
    .add_message::<ImpactEvent>()
    .add_message::<netdrive::RemoteImpact>()
    .add_message::<DamageEvent>()
    .add_message::<StuckEvent>()
    .add_message::<PartDetached>()
    .add_message::<RecoveryEvent>()
    .add_message::<cnr::CnrEvent>()
    .add_message::<RaceStarted>()
    .add_message::<BangerStateChanged>()
    .init_resource::<contracts::ImpactFilter>()
    .init_resource::<damage::DamageReport>()
    .init_resource::<stuck::StuckReport>()
    .init_resource::<breakaway::BreakReport>()
    .init_resource::<recovery::RecoveryReport>()
    .init_resource::<damage_fx::SmokeFxReport>()
    .init_resource::<spark_fx::SparkFxReport>()
    .init_resource::<precip::PrecipReport>()
    .init_resource::<wheel_fx::WheelFxReport>()
    .init_resource::<texel_fx::TexelDamageReport>()
    // F07-A.2: decoded-PCM audio — `PcmAudio` registers with the
    // `AudioPlugin` DefaultPlugins brings; voices spawn as entities and
    // teardown owns them (`aud=` on the smoke record).
    .init_resource::<audio::AudioReport>()
    .add_message::<audio::HornRequest>()
    .add_audio_source::<audio::PcmAudio>()
    .init_resource::<mm2_game::ResultLedger>()
    .init_resource::<mm2_game::BangerPool>()
    .init_resource::<SessionControl>()
    .init_resource::<session::SessionNote>()
    .init_resource::<pause::PauseMenu>()
    .init_resource::<results::ResultsMenu>()
    .add_systems(FixedUpdate, advance_session_tick)
    // Sample the human steering ramp once per simulation tick. Update
    // collects devices; Avian advances bodies later in FixedPostUpdate.
    .add_systems(
        FixedUpdate,
        input::apply_player_input
            .run_if(not(capturing))
            .run_if(not(resource_exists::<scripted::ScriptedDrive>))
            .run_if(not(resource_exists::<input::ParkedDrive>))
            .run_if(not(resource_exists::<input::RamDrive>))
            .run_if(not(resource_exists::<sequence::SequenceDrive>)),
    )
    // F27-B.2b: the gold/hideout/bank markers follow the host's match
    // (idle until a `CnrHost` exists and markers are spawned).
    .add_systems(Update, cnr::sync_cnr_markers)
    // F27-B.4c: the match readout (clock, points, the gold) — idle
    // until a Cops & Robbers session spawned its panel.
    .add_systems(Update, mm2_app::cnrhud::update_cnr_scoreboard)
    // F23-AC06: the input-capability record and each pad the OS reports
    // go to the log, so a run says what it knew about its devices.
    .add_systems(Startup, mm2_app::devices::log_input_capabilities)
    // F30 low-GPU edge case: the texture ceiling follows the render
    // device's own limit, before the first loader decodes anything.
    .add_systems(PreStartup, mm2_app::texture_budget::adopt_render_device)
    .add_systems(First, mm2_app::texture_budget::adopt_render_device)
    .add_systems(Update, mm2_app::devices::log_pad_connections)
    // Drawbridge leaves pose after the solver step, like the lane
    // followers: the angular velocity they leave carries the next step.
    .init_resource::<mm2_app::worldclock::WorldClock>()
    .init_resource::<mm2_app::cablecar::CableCircuits>()
    .add_systems(
        FixedLast,
        (
            mm2_app::worldclock::capture_world_start,
            mm2_app::worldclock::advance_world_clock,
            mm2_app::drawbridge::drive_drawbridges,
            mm2_app::movers::drive_movers,
            mm2_app::cablecar::drive_cable_cars,
        )
            .chain(),
    )
    .add_systems(
        FixedLast,
        (
            contracts::collect_impacts,
            // F05-B.1: damage accumulates off the deduplicated impact
            // stream (apply → outcome) — independent consumers of the
            // solver's edge stream like the bangers below.
            // `--wreck-at` follows the apply so its destruction reaches
            // the outcome in the same pass.
            (damage::apply_impact_damage, damage::dev_wreck_at).chain(),
            // F05-B.9: the same deduplicated stream feeds each rig's
            // `ImpactsTable`→`ApplyDamage` — the skin splats the tick
            // the hit lands — the headless record's `txl=` field.
            texel_fx::apply_texel_damage,
            // F05-B.2: `vehstuck` detection arms off the same deduped
            // impact stream damage reads — before `resolve_disabled`
            // so a wreck the outcome is about to repair/reset never
            // starts an episode.
            stuck::track_stuck,
            // F05-B.3: authored breakaway parts read the same deduped
            // impact stream; detach before `resolve_disabled` so a
            // wrecking blow can shed a panel the same tick the repair
            // puts it back (one bounded event per part per attachment).
            breakaway::detach_breaks,
            // Blitz/Checkpoint breakdown episodes run down and repair
            // right after the outcome starts them, before the
            // impairment sync reads the wreck's state.
            (damage::resolve_disabled, damage::resolve_breakdown).chain(),
            // F05-B.7: damage→engine-impairment coupling (DSN-25) —
            // after `resolve_disabled` so a wreck's repair clears the
            // factor the same tick the tier leaves `Disabled`.
            damage::sync_impairment,
            stuck::resolve_stuck,
            // F05-B.5: water/OOB recovery — observe the wheel contacts
            // the physics step left, then resolve fired episodes to the
            // dry-grounded anchor (track → resolve, like the detectors
            // above). F27-B.2 follows in the same group: the gold
            // rules over the host's cars, then the carrier's load
            // reconciled against them — idle until a Cops & Robbers
            // match (`CnrHost`) exists.
            (
                recovery::track_recovery,
                recovery::resolve_recovery,
                cnr::enroll_cnr_participants,
                cnr::cnr_host_step,
                cnr::end_decided_match,
                cnr::reconcile_gold_load,
            )
                .chain(),
            // Banger activation/settle consume the same contact edges
            // the impact pipeline reads — independent consumers of the
            // solver's edge stream.
            banger::activate_bangers,
            banger::settle_bangers,
            contracts::publish_vehicle_telemetry,
            // Teleport re-anchoring must precede the race driver so a
            // reset never sweeps a checkpoint (AC02).
            race::reanchor_teleported_participants,
            race::advance_race,
            // F21-B.4: a lesson's leg swap reads the terminal state
            // `advance_race` just wrote (idle without a `LessonDriver`).
            lesson::drive_lesson,
            // F10-A.2: lane-following runs after the solver step; the
            // recycler reads the poses it leaves. The F10-B.6 handover
            // reads the same contact edges the impact pipeline and the
            // bangers consume, then runs before the driver so a knocked
            // car is solver-owned from the tick it flips
            // (knock → drive → maintain). The drape rests each lane
            // pose on the road straight after the driver writes it, so
            // the recycler and the next solver step see the draped
            // pose. The F10-B.7 signal update reads the junction
            // controller the driver just advanced, so it runs last
            // (knock → drive → drape → maintain → signals).
            traffic::knock_ambient,
            traffic::drive_ambient,
            traffic::drape_ambient,
            traffic::maintain_ambient,
            traffic::drive_signals,
        )
            .chain(),
    )
    .add_systems(
        Update,
        (
            // The session lifecycle: spawn while Loading (once per
            // `begin`), read quit/restart intents, despawn while
            // Unloading and advance the phase machine. Despawn is
            // chained before the driver so teardown is observed
            // complete the same frame.
            session::load_session_world.run_if(session::loading),
            session::session_control_input.run_if(not(capturing)),
            (
                despawn_session_entities.run_if(session::unloading),
                // `--pause` auto-pauses the first `Playing` frame —
                // deliberately ungated by `capturing`: putting a
                // capture into pause is exactly what it is for.
                pause::dev_pause_once,
                // `--pause-map` does the same with HUD-4's full-screen
                // map up — ungated like `--pause`, a capture is the
                // point. Ahead of the driver so the intent is consumed
                // this frame.
                hudmap::dev_pause_map_once,
                // `--finish` sweeps the local participant through the
                // remaining race triggers — deliberately ungated by
                // `capturing` for the same reason: a capture is how the
                // results screen gets rendered.
                results::dev_finish_once,
                // `--restart` queues the session's own restart intent
                // on the first `Playing` frame — the same teardown the
                // pause/results rows and a disabled-in-event restart
                // take. Ahead of the driver so the intent is consumed
                // this frame.
                session::dev_restart_once,
                // `--restart-at` queues the same intent once the
                // session clock reaches its tick — the delayed leg
                // that lets a run bank race progress before teardown.
                session::dev_restart_at,
                // `--reset-at` emits the same `R`-key reset bundle at
                // its tick — ahead of the driver so this frame's
                // fixed step already runs from the teleported pose.
                session::dev_reset_at,
                session::drive_session,
            )
                .chain()
                // The reset apply runs last among Update's `ResetVehicle`
                // writers (this chain emits the `dev_reset_at` bundle):
                // a teleport lands the frame it was written, and the
                // netdrive epoch tracker — ordered after `vehicle_reset`
                // — declares the bump on that frame's `Snap`.
                .before(mm2_vehicle::systems::vehicle_reset),
            input::vehicle_input.run_if(not(capturing)),
            // The evidence drivers own `VehicleInput` while their flag
            // is on — scheduled after the keyboard mapping so they win
            // deterministically. Explicit --bot also opts into motion
            // during capture; ordinary static captures stay frozen.
            // The parked control is chained after the
            // scripted one so a world holding both markers stays
            // deterministic (the CLI flags conflict, so that can only
            // come from a test).
            (
                scripted::scripted_drive.run_if(resource_exists::<scripted::ScriptedDrive>),
                input::parked_drive.run_if(resource_exists::<input::ParkedDrive>),
                input::ram_drive.run_if(resource_exists::<input::RamDrive>),
            )
                .chain()
                .after(input::vehicle_input)
                // A scripted re-anchor emits `ResetVehicle` — same
                // before-apply ordering as every Update writer.
                .before(mm2_vehicle::systems::vehicle_reset)
                .run_if(evidence_capture_input_allowed),
            navarrow::nav_target_input.run_if(not(capturing)),
            (
                camera::toggle_camera.run_if(not(capturing)),
                // F22-B.2: BACKSPACE toggles the mirror strip in the
                // live phases — same frozen-input gate as every other
                // control.
                camera::mirror_input.run_if(not(capturing)),
                // F22-A.2: `I` toggles the opponent indicators — same
                // frozen-input gate and live-phase contract.
                oppind::indicator_input.run_if(not(capturing)),
                // F22-A.3: `H` toggles the driving-HUD layer — same
                // contract again.
                hud::hud_input.run_if(not(capturing)),
                // `--cam-cycle-at` walks the same C chain once at its
                // tick — deliberately ungated like `chase_follow`: a
                // frozen-input capture is what it is for.
                camera::dev_cam_cycle_at,
                camera::chase_follow,
                camera::free_fly.run_if(not(capturing)),
                dash::drive_dash,
                // After the glow-quad owner: the split's cockpit hide
                // keeps the last word over a lit lamp, and outside the
                // cockpit it restores only what it tagged, so ordering
                // the two cannot fight over an unlit one.
                dash::sync_dash_visibility.after(car_visual::update_glows),
                dash::cockpit_look.run_if(not(capturing)),
            ),
            // The `R` bundle is a `ResetVehicle` write — ahead of the
            // apply so the reset lands this frame and a hosted session's
            // epoch tracker declares it on this frame's `Snap`.
            input::reset_input.before(mm2_vehicle::systems::vehicle_reset),
            debug_toggle,
            screenshot_input,
            camera::retarget_hud,
            car_visual::update_wheel_visuals,
            car_visual::update_glows,
            car_visual::toggle_headlights,
            car_visual::trailer_input,
            city::animate_textures,
            (
                race::update_checkpoint_markers,
                navarrow::update_nav_arrow,
                race::update_race_warning,
                race::update_countdown_banner,
            ),
            hud::update_hud,
        ),
    )
    // F22-A.1: the HUD map tracks every frame — eased zoom and a
    // rotating map should animate through pause/countdown alike, and a
    // `--pause-map` capture needs it live while input is frozen
    // (ungated like `chase_follow`). F22-B.2's mirror strip follows
    // the same contract — a `--mirror` capture needs it live too.
    // Own schedule slot: the main Update tuple is at Bevy's system
    // count limit.
    .add_systems(
        Update,
        (
            hudmap::drive_hud_map.after(session::drive_session),
            mm2_app::speedometer::drive_speedometer.after(session::drive_session),
            mm2_app::navarrow3d::drive_nav_arrow_view
                .after(navarrow::update_nav_arrow)
                .after(session::drive_session),
            camera::drive_mirror.after(session::drive_session),
            // F22-A.2: the indicator pool rebinds ungated too — a
            // `--frames`/`--screenshot` run needs the markers live.
            oppind::drive_opponent_indicators.after(session::drive_session),
            // F22-A.4: the authored race timer composes off the same
            // ungated pass so `--frames` captures see real state.
            racetime::update_race_timer.after(session::drive_session),
            // F22-A.6: the standings cluster composes off the same
            // ungated pass — `--frames` captures see the live place,
            // lap and checkpoint list.
            racestat::update_race_stats.after(session::drive_session),
        ),
    )
    // Pause owns the keyboard while `Paused`: `pause_input` runs after
    // `session_control_input` (which ignores `Paused` — Esc while
    // paused is resume, not quit) and before `drive_session` (so the
    // Esc that entered pause is never re-read as a resume in the same
    // update). The physics clock and the overlay are pure phase
    // mirrors — they run after the driver so the update that enters or
    // leaves `Paused` already sees the settled phase.
    .add_systems(
        Update,
        (
            // F22-A.1/HUD-4: the map owns TAB/E/F while `Playing` and
            // Q both ways through its full-screen pause — chained
            // ahead of `pause_input` so the Q/Esc that closes a
            // pause-map is never re-read as a menu resume/back, and
            // the pause intent it queues lands on `drive_session` the
            // same update.
            hudmap::hudmap_input
                .after(session::session_control_input)
                .before(pause::pause_input)
                .before(session::drive_session)
                .run_if(not(capturing)),
            pause::pause_input
                .after(session::session_control_input)
                .before(session::drive_session)
                .run_if(not(capturing))
                .run_if(not(display_trial::display_trial_pending)),
            pause::sync_physics_pause.after(session::drive_session),
            pause::pause_present.after(session::drive_session),
            // Results owns the keyboard while `Results` — same
            // scheduling slot as the pause input so the session
            // control reader and the driver never re-read its keys.
            results::results_input
                .after(session::session_control_input)
                .before(session::drive_session)
                .run_if(not(capturing)),
            results::results_present.after(session::drive_session),
        ),
    )
    // AI opponents own their own `VehicleInput` — `vehicle_input` only
    // writes `PlayerVehicle`, so no ordering is needed. Ordinary static
    // captures freeze them; explicit --bot captures keep the real race moving.
    .add_systems(
        Update,
        opponents::opponent_drive
            .run_if(evidence_capture_input_allowed)
            // A penned opponent re-anchors through `ResetVehicle` —
            // ahead of the apply like the other Update writers.
            .before(mm2_vehicle::systems::vehicle_reset),
    )
    // F20-A.3: the fielded cops' detect → pursue machine. Frozen with
    // the other AI in ordinary static captures; no-ops without a fleet.
    .add_systems(
        Update,
        (police::police_pursuit, police::count_cop_impacts).run_if(evidence_capture_input_allowed),
    )
    // F25-B: the trailer reseat follower reads every `ResetVehicle` the
    // frame produced — the FixedLast resolvers, `vehicle_self_right`
    // and each Update writer — and emits each towed trailer's reseat
    // ahead of the apply, so the whole rig teleports in the same update
    // and the epoch-declared `Snap` carries its settled pose.
    .add_systems(
        Update,
        session::reseat_towed_trailers
            .after(mm2_vehicle::systems::vehicle_self_right)
            .after(input::reset_input)
            .after(session::dev_reset_at)
            .after(scripted::scripted_drive)
            .after(opponents::opponent_drive)
            .after(netdrive::apply_reset_requests)
            .before(mm2_vehicle::systems::vehicle_reset),
    )
    // F05-B.6: authored engine smoke — emission reads the damage
    // state the FixedLast systems leave, then the advance step
    // integrates the puffs it just spawned. Own schedule slot (the
    // main Update tuple is at Bevy's system count limit); no ordering
    // requirement beyond `is_playing`, which both systems gate on.
    .add_systems(
        Update,
        (damage_fx::drive_smoke, damage_fx::advance_smoke).chain(),
    )
    // Light-bar flare sprites turn to the view; own slot for the same
    // reason (the main Update tuple is full).
    .add_systems(Update, car_visual::face_flares)
    // F05-B.8: authored impact sparks — emission consumes the
    // deduplicated impact stream the FixedLast systems publish, then
    // the advance step integrates the streaks it just spawned. Same
    // schedule slot and `is_playing` gate as the smoke pair. The
    // `apply_snapshots` edge keeps a replicated `RemoteImpact`
    // sparking in the frame it arrived rather than a frame late (and
    // keeps the emit/advance frame deterministic under `--frames`).
    .add_systems(
        Update,
        (spark_fx::emit_sparks, spark_fx::advance_sparks)
            .chain()
            .after(netdrive::apply_snapshots),
    )
    // F25-B: replicated texel damage — the v10 `Snap.impacts` rows a
    // remote copy's skin splats from (the local stream feeds
    // `apply_texel_damage` in FixedLast). Same `apply_snapshots` edge
    // as the spark/audio consumers: a replicated hit splats the frame
    // it landed, keeping `--frames` captures deterministic.
    .add_systems(
        Update,
        texel_fx::apply_remote_texels.after(netdrive::apply_snapshots),
    )
    // F18-B.2: authored precipitation — the emitter anchors on the
    // active camera, the cover probe suppresses sheltered spawns and
    // the advance step sweeps contacts so drops land instead of
    // passing through the world. Own slot like the other FX pairs.
    .add_systems(
        Update,
        (precip::emit_precip, precip::advance_precip).chain(),
    )
    // F18-B.4: authored wheel surface particles — grounded-wheel
    // contacts resolve their `ptxindex` channels through the surface
    // tables and emit the session's loaded `tune/effects` specs, then
    // the advance step integrates the puffs it just spawned. Own
    // schedule slot like the other FX pairs; the `is_playing` gate
    // keeps the unload sweep ahead of any rig commands.
    .add_systems(
        Update,
        (wheel_fx::emit_wheel_fx, wheel_fx::advance_wheel_fx).chain(),
    )
    // F07-AC02: `--seq`'s staged input program owns `VehicleInput`
    // like the other evidence drivers — after the keyboard mapping so
    // it wins deterministically, and after `clutch_voices` so a stage
    // boundary attributes the same frame's clutch one-shot to the
    // stage that produced it. Own schedule slot: the main Update
    // tuple is at Bevy's system count limit.
    .add_systems(
        Update,
        sequence::sequence_drive
            .after(input::vehicle_input)
            .after(audio::clutch_voices)
            .run_if(resource_exists::<sequence::SequenceDrive>)
            .run_if(not(capturing)),
    )
    // F07-A.2/B.1: authored-horn voices plus the engine loop rig —
    // live input is frozen during a capture like every other input,
    // while `--horn` stays ungated so a capture run can still fire it.
    // `horn_voices` drains requests a frame later at worst (message
    // double-buffering); the rig builds once a `VehicleAudio` car
    // exists and its loops re-mix every frame off the sim's RPM —
    // ungated during captures because they track simulation state, not
    // input. Sink counting and pause sync are pure phase/sink mirrors
    // after the drivers.
    .add_systems(
        Update,
        (
            audio::horn_input.run_if(not(capturing)),
            audio::dev_horn_once,
            audio::horn_voices,
            // F07-B.7: a `SIREN_FLAG` car's presses toggle its authored
            // siren program, then the drive keeps the voice on the
            // machine's current sample — the same despawn ordering as
            // the rigs. F20-B.1: a chasing cop's lights request its
            // siren first, so the drive voices it the same update.
            (
                audio::siren_follow_lights,
                audio::siren_toggle,
                audio::siren_drive,
            )
                .chain()
                .after(session::drive_session),
            // `.after(drive_session)` — rig/listener commands must not
            // queue on cars or cameras `despawn_session_entities` just
            // killed in the same update (the unload chain flushes
            // before this ordering edge).
            (audio::engine_rigs, audio::engine_drive)
                .chain()
                .after(session::drive_session),
            // F07-B.3: deduplicated impacts → bounded one-shot voices —
            // the same despawn ordering as the rigs (a struck car dying
            // mid-update must not queue voice reads on it). The
            // `apply_snapshots` edge voices a replicated `RemoteImpact`
            // in the frame it landed rather than a frame late.
            audio::impact_voices
                .after(session::drive_session)
                .after(netdrive::apply_snapshots),
            // F07-B.5: committed gear/direction changes → authored
            // clutch one-shots — the same despawn ordering.
            audio::clutch_voices.after(session::drive_session),
            // F07-B.4: wheel contact → skid/rolling loop voices — the
            // same despawn ordering for the same reason. The
            // `apply_snapshots` edge voices a replicated `SurfaceContact`
            // in the frame it landed rather than a frame late — same
            // contract `impact_voices` keeps.
            audio::surface_voices
                .after(session::drive_session)
                .after(netdrive::apply_snapshots),
            // F07-B.6: ambient cars' resolved engine tables → bounded
            // looping voices — the same despawn ordering.
            (audio::ambient_engine_rigs, audio::ambient_engine_drive)
                .chain()
                .after(session::drive_session),
            // F18-B.3: precipitation bed crossfade + the seeded
            // thunder schedule — the same despawn ordering; the mix
            // computes on the voice components, so headless runs read
            // it like the other rigs.
            audio::weather_voices.after(session::drive_session),
            // F18-B.5: the environmental commentary queue — the same
            // despawn ordering; one-shots resolve and play inside the
            // pre-race window.
            // F27-B.4c: the Cops & Robbers announcer beside it — it
            // reads the match's events (authority) or the replica's
            // changes (client).
            // The lesson verdict ask rides the same chain.
            audio::commentary_systems(),
            // Drawbridge, ferry and Underground sounds — the same
            // despawn ordering as the other rigs.
            mm2_app::object_sound::object_sound_voices
                .after(session::drive_session)
                .after(mm2_app::underground::track_listener_room),
            mm2_app::underground::track_listener_room,
            mm2_app::race_audio::race_cue_voices
                .after(session::drive_session)
                .after(netdrive::apply_snapshots),
            // F07-B.2: the spatial listener follows whichever camera is
            // active — after the toggle so a mode switch moves the ear
            // the same frame, and after the session driver for the
            // same despawn reason as the rigs.
            audio::audio_listener
                .after(camera::toggle_camera)
                .after(session::drive_session),
            audio::count_sinks,
            audio::sync_audio_pause.after(session::drive_session),
            audio::reset_audio_report.run_if(session::unloading),
            // F18-B.2: the precipitation evidence counters clear on
            // teardown like the audio report's.
            precip::reset_precip_report.run_if(session::unloading),
            // F18-B.4: same for the wheel-effect report.
            wheel_fx::reset_wheel_fx_report.run_if(session::unloading),
        ),
    )
    // F23: the Options screen's volume levels reach a voice the frame it
    // spawns, before bevy's own audio systems (which run after transform
    // propagation) create its sink; mixers apply the same gain per frame.
    .add_systems(
        PostUpdate,
        audio::level_new_voices.before(bevy::transform::TransformSystems::Propagate),
    )
    // F16-B: drain authoritative results into the bound profile —
    // records finishes, grants rewards, saves on change. Inert without
    // an event or a bound profile.
    .add_systems(Update, progression::record_session_results)
    // The F09-B overlay draws only while a session carries a loaded
    // CityNav resource.
    .add_systems(
        Update,
        nav_overlay::draw_nav_overlay.run_if(resource_exists::<nav_overlay::CityNav>),
    )
    // F20: the `--police-debug` view draws only when the session asked.
    .add_systems(
        Update,
        police_debug::draw_police_debug.run_if(police_debug::enabled),
    )
    // F19-A.5: pedestrian figures animate while the session plays; the
    // `--ped-lab` line-up spawns and tours them only when asked.
    .init_resource::<pedestrian::PedLabSpawned>()
    .add_systems(
        Update,
        (
            pedestrian::spawn_ped_lab.run_if(pedestrian::lab_enabled),
            pedestrian::advance_ped_lab.run_if(pedestrian::lab_enabled),
            // F19-B.2: the sidewalk crowd — recycle/refill, walk, then
            // animate (the lab and the crowd never run together).
            crowd::maintain_pedestrians,
            crowd::react_pedestrians,
            crowd::walk_pedestrians,
            pedestrian::animate_pedestrians,
        )
            .chain(),
    )
    // F18-A.4: the `.sky` dome re-centres on the active camera and
    // advances its authored rotation. F18-A.5's room-PVS culling
    // shares the slot. Both read the active camera's `GlobalTransform`
    // — the cockpit camera is a child of the vehicle, so its
    // `Transform` is the car-local eye offset, not the world pose.
    // Propagation runs in PostUpdate, so the pose is one frame stale;
    // the player `Position` source keeps the room under the car
    // covered regardless.
    .add_systems(
        Update,
        (
            environment::drive_sky_dome,
            pvs::apply_city_pvs.run_if(resource_exists::<pvs::CityPvs>),
        )
            .after(camera::chase_follow)
            .after(camera::free_fly),
    );
    if menu_mode {
        // The shell seeds from the same launch resolution a direct
        // boot uses (`--car`/remembered/default vehicle, `--pro`/rank
        // difficulty) — the menu refines the pick, it doesn't
        // re-derive it. `menu_input` runs `Session::begin` on launch
        // rows and `menu_watch` reopens the shell whenever the session
        // returns to `Menu`, so quitting a menu-launched session
        // comes back here instead of exiting.
        let mut shell = menu::MenuShell::new(menu_vehicle, difficulty);
        shell.screen = match cli.menu_screen.as_deref() {
            Some("profiles") => menu::Screen::Profiles,
            Some("races") => menu::Screen::EventCity,
            Some("garage") => menu::Screen::Garage,
            Some("options") => menu::Screen::Options,
            Some("controls") => menu::Screen::Controls,
            _ => menu::Screen::Root,
        };
        app.insert_resource(shell)
            .insert_resource(net::MenuHostBind(cli.menu_bind))
            .insert_resource(menu::MenuPreviewCapture(cli.frames.is_some()))
            .insert_resource(
                menu::MenuData::new(menu_store, has_mods, menu_bound)
                    .with_mods_cosmetic_only(mods_cosmetic_only)
                    .with_settings(graphics, settings_path)
                    .with_run_overrides(run_overrides)
                    .with_controls(control_settings, controls_path),
            )
            .add_systems(
                Update,
                (
                    menu::menu_dial,
                    menu::menu_watch,
                    // The mouse path queues commands for `menu_input` —
                    // it runs first so a hover/click lands the same
                    // update. Frozen during a capture like every other
                    // input: a `--menu --frames` screenshot must be
                    // reproducible.
                    menu::menu_mouse
                        .run_if(not(capturing))
                        .run_if(not(display_trial::display_trial_pending)),
                    menu::menu_input
                        .run_if(not(capturing))
                        .run_if(not(display_trial::display_trial_pending)),
                    menu::menu_present,
                    menu::menu_preview_motion,
                )
                    .chain(),
            );
    }
    let joining_from_cli = lobby.is_some();
    if let Some(link) = lobby {
        // A joined lobby owns `Menu`-time surface and exit: the pump
        // thread blocks on the socket while `drive_lobby` drains what
        // it queued each update (see `lobby_systems::add_client_systems`).
        // `LobbyText` is the minimal surface until a real lobby menu
        // exists.
        app.insert_resource(link)
            .init_resource::<net::LobbyState>()
            .init_resource::<netdrive::RemoteSnaps>()
            .init_resource::<netdrive::InputSeq>()
            .init_resource::<netdrive::NetDriveReport>();
        net::spawn_lobby_text(&mut app.world_mut().commands());
        app.world_mut().flush();
    }
    if joining_from_cli || menu_mode {
        mm2_app::lobby_systems::add_client_systems(&mut app, capturing);
    }
    let hosting_from_cli = host_link.is_some();
    if let Some(link) = host_link {
        // A hosted lobby owns `Menu`-time surface and exit like a
        // joined one: `drive_host` drains host-loop events and operator
        // commands once per update. `HostText` is the minimal surface
        // until a real lobby menu exists.
        app.insert_resource(link)
            .init_resource::<net::LobbyState>()
            .init_resource::<netdrive::NetDriveReport>()
            .init_resource::<netdrive::WireStall>();
        net::spawn_host_text(&mut app.world_mut().commands());
        app.world_mut().flush();
    }
    if hosting_from_cli || menu_mode {
        mm2_app::lobby_systems::add_host_systems(&mut app, capturing);
    }
    if cli.bot {
        app.insert_resource(scripted::ScriptedDrive);
    }
    if cli.parked {
        app.insert_resource(input::ParkedDrive);
    }
    if cli.ram {
        app.insert_resource(input::RamDrive);
    }
    if cli.seq {
        app.insert_resource(sequence::SequenceDrive::default());
    }
    // F18-A.5: `--no-pvs` reaches the session through
    // `SessionConfig::dev` (retail `EnablePVS` default-on) — the same
    // channel the headless smoke's own app reads.
    if let Some(slot) = active_profile {
        app.insert_resource(slot);
    }
    if cli.screenshot.is_some() || cli.frames.is_some() {
        app.insert_resource(SmokeTest {
            // A menu capture never loaded a world — say so in the record.
            world: if menu_mode {
                "menu".to_string()
            } else {
                world_label.clone()
            },
            screenshot: cli.screenshot.clone(),
            frames_left: cli.frames.unwrap_or(600),
            pending: None,
            capture_wait: 0,
        });
        app.add_systems(Update, smoke_test);
    }
    if let Some(path) = cli.perf_log.clone() {
        // The report names what was asked for; the recorder adds what it
        // can observe (build, host, GPU, content fingerprint).
        let driver = if cli.bot {
            "bot"
        } else if cli.parked {
            "parked"
        } else if cli.ram {
            "ram"
        } else if cli.seq {
            "seq"
        } else {
            "live"
        };
        let mut scene = vec![("driver".to_string(), driver.to_string())];
        for (key, value) in [
            ("city", cli.city.clone()),
            ("event", cli.event.clone()),
            ("car", cli.car.clone()),
            ("paint", cli.paint.map(|p| p.to_string())),
            ("frames", cli.frames.map(|f| f.to_string())),
        ] {
            if let Some(value) = value {
                scene.push((key.to_string(), value));
            }
        }
        let settings = vec![
            (
                "vsync".to_string(),
                if graphics.vsync { "on" } else { "off" }.to_string(),
            ),
            ("msaa".to_string(), format!("{:?}", graphics.antialiasing)),
            ("shadows".to_string(), format!("{:?}", graphics.shadows)),
            (
                "window".to_string(),
                match graphics.display {
                    settings::DisplayMode::Windowed => window_label(graphics.window_size.logical()),
                    settings::DisplayMode::Fullscreen => "borderless fullscreen".to_string(),
                },
            ),
            ("fixed_hz".to_string(), FIXED_HZ.to_string()),
        ];
        perf::enable(
            &mut app,
            path,
            perf::RunContext {
                world: world_label,
                install: has_mm2,
                mods: mod_ids,
                scene,
                settings,
            },
        );
    }
    let exit = app.run();
    if let AppExit::Error(code) = exit {
        std::process::exit(code.get() as i32);
    }
}

/// Whether a windowing system is present for a windowed/visual run.
/// macOS and Windows always have one in a GUI session; a headless Linux
/// box does not.
fn display_available() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        true
    }
}

/// Whether a `--frames` capture is running.
///
/// Live input must not reach the camera or the vehicle while one is: the
/// capture opens a window that can take focus from whatever else is on
/// screen, and stray keystrokes fly the free camera away from the `--cam`
/// pose the capture exists to reproduce. Physics keeps running, so the
/// vehicle still settles. Explicit scripted evidence may opt into motion
/// through `evidence_capture_input_allowed`; live keyboard input stays frozen.
fn capturing(smoke: Option<Res<SmokeTest>>) -> bool {
    smoke.is_some()
}

/// Explicit scripted evidence opts into a driven capture. Keyboard input and
/// ordinary static screenshots retain the frozen-input policy above.
fn evidence_capture_input_allowed(
    smoke: Option<Res<SmokeTest>>,
    scripted: Option<Res<scripted::ScriptedDrive>>,
) -> bool {
    smoke.is_none() || scripted.is_some()
}

/// After N frames, take the screenshot (if requested) and exit.
///
/// A `Failed` world ends the smoke immediately with `status=fail`. A
/// requested screenshot is awaited — `pass` is reported only once the
/// capture file actually exists and is non-empty, never on a fixed delay.
fn smoke_test(
    mut commands: Commands,
    mut st: ResMut<SmokeTest>,
    session: Res<Session>,
    aud: Option<Res<audio::AudioReport>>,
    crowd: Option<Res<crowd::PedCrowd>>,
    walkers: Query<(), With<crowd::PedWalk>>,
    mut exit: MessageWriter<AppExit>,
) {
    let world = st.world.clone();
    // F19-B.2: the sidewalk crowd's live/target population and
    // placement counters, when the session fielded one.
    let ped_detail = crowd
        .filter(|c| c.is_active())
        .map(|c| c.smoke_detail(walkers.iter().count()))
        .unwrap_or_default();
    // F07-A.2/B.1: the capture reports its voice path — `s` counting
    // the sinks the output device attached (0 means the mixer never
    // saw the voice, an honest no-device report), `l` the engine loops
    // spawned and `a` the ones audible at record time.
    let aud_detail = aud
        .filter(|r| r.active())
        .map(|r| {
            format!(
                " aud={}h/{}v/{}s/{}l/{}a",
                r.horns, r.voices, r.sunk, r.loops, r.audible
            )
        })
        .unwrap_or_default();
    let record = |status: smoke::SmokeStatus, detail: String| smoke::SmokeRecord {
        kind: smoke::KIND_VISUAL,
        world: world.clone(),
        status,
        detail,
    };
    if let SessionPhase::Failed(m) = session.phase() {
        println!("{}", record(smoke::SmokeStatus::Fail, m.clone()).line());
        exit.write(AppExit::from_code(
            smoke::SmokeStatus::Fail.exit_code() as u8
        ));
        return;
    }
    if st.frames_left > 0 {
        st.frames_left -= 1;
        return;
    }
    if let Some(path) = st.screenshot.take() {
        // The `landed` check below sees *any* file at the target — a
        // capture from an earlier run would pass on stale pixels while
        // this run's screenshot is still in flight. Clear it first so
        // only this run's write can satisfy the check.
        if let Err(e) = smoke::clear_stale_screenshot(&path) {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Fail,
                    format!("cannot clear screenshot target {}: {e}", path.display()),
                )
                .line()
            );
            exit.write(AppExit::from_code(
                smoke::SmokeStatus::Fail.exit_code() as u8
            ));
            return;
        }
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        st.pending = Some(path);
        // ~15 s at 60 fps — generous for a single frame capture.
        st.capture_wait = 900;
        return;
    }
    if let Some(path) = &st.pending {
        let landed = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
        if landed {
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Pass,
                    format!(
                        "frames=done screenshot={} bytes={bytes}{aud_detail}{ped_detail}",
                        path.display()
                    ),
                )
                .line()
            );
            exit.write(AppExit::Success);
        } else if st.capture_wait == 0 {
            println!(
                "{}",
                record(
                    smoke::SmokeStatus::Fail,
                    format!("screenshot never landed at {}", path.display()),
                )
                .line()
            );
            exit.write(AppExit::from_code(
                smoke::SmokeStatus::Fail.exit_code() as u8
            ));
        }
        st.capture_wait = st.capture_wait.saturating_sub(1);
        return;
    }
    println!(
        "{}",
        record(
            smoke::SmokeStatus::Pass,
            format!("frames=done{aud_detail}{ped_detail}"),
        )
        .line()
    );
    exit.write(AppExit::Success);
}

/// The fixed-step rate (Hz); the perf report's `settings.fixed_hz` row
/// is formatted from this same value.
const FIXED_HZ: f64 = 60.0;

/// `1280x720` — how the perf report names a window size; the window is
/// built from the same [`settings::WindowSize`] value.
fn window_label((w, h): (u32, u32)) -> String {
    format!("{w}x{h}")
}

/// Directory (relative to the working directory, gitignored) that
/// Cmd/Ctrl+P screenshots are saved to.
const SCREENSHOT_DIR: &str = "screenshots";

/// Where Cmd/Ctrl+P screenshots go: [`SCREENSHOT_DIR`] unless that is inside
/// a protected directory (F30-AC05), then the user-data directory; `None`
/// disables the hotkey.
#[derive(Resource)]
struct ScreenshotDir(Option<PathBuf>);

/// `Cmd+P` (or `Ctrl+P`) saves a screenshot to the [`ScreenshotDir`], named
/// after the time and the camera pose it was taken from.
fn screenshot_input(
    keys: Res<ButtonInput<KeyCode>>,
    dir: Res<ScreenshotDir>,
    cameras: Query<(&Camera, &GlobalTransform), hudmap::WorldCamera3d>,
    mut commands: Commands,
) {
    let modifier = keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    if !(modifier && keys.just_pressed(KeyCode::KeyP)) {
        return;
    }
    let Some(dir) = dir.0.as_deref() else {
        error!("screenshot refused: no screenshot directory outside the install");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        error!(dir = %dir.display(), error = %e, "cannot create screenshot directory");
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let cam = camera::active_cam_pose(&cameras).unwrap_or_default();
    let path = dir.join(format!("{secs}_cam_{cam}.png"));
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

/// `F1` toggles vehicle physics debug gizmos.
fn debug_toggle(keys: Res<ButtonInput<KeyCode>>, mut dbg: ResMut<VehicleDebugEnabled>) {
    if keys.just_pressed(KeyCode::F1) {
        dbg.0 = !dbg.0;
    }
}

/// Documented stock default when an installation is present and no `--car`
/// was requested: MM2's own menu default, the New Beetle.
const DEFAULT_CAR: &str = "vpbug";

/// Resolve the default vehicle: `vpbug`, falling back to the first
/// loadable expected-stock entry when a partial install lacks it.
fn default_stock_car(vfs: &Vfs, paint: usize) -> Option<VehicleDef> {
    let catalog = VehicleCatalog::scan(vfs);
    let mut candidates = vec![DEFAULT_CAR];
    candidates.extend(mm2_content::EXPECTED_STOCK_ROSTER.iter().copied());
    for id in candidates {
        let Some(entry) = catalog.entries.iter().find(|e| e.id == id) else {
            continue;
        };
        if !entry.is_ready() {
            continue;
        }
        match mm2_content::load_vehicle(vfs, id, paint) {
            Ok(def) => return Some(def),
            Err(e) => warn!(car = %id, error = %e, "stock candidate failed to load"),
        }
    }
    None
}

/// `--list-cars` output: id, name, class, roster gate, paints, deps.
/// The `gate` column is the garage's authored progression gate —
/// `open`, `reward` (a `<city>_rewards.csv` row unlocks it) or
/// `unlisted` (no canonical `tune/<id>.info`; the original's roster
/// is that scan — vpmoonrover's `.inf` is the retail case, UNK-3).
/// `paints` shows the authored count, `+Ng` marking reward-gated ones.
fn print_roster(catalog: &VehicleCatalog, garage: &mm2_game::GarageTable) {
    if catalog.entries.is_empty() {
        eprintln!(
            "no vehicles discovered — mount an MM2 install with --mm2-path (or mods with --mods)"
        );
        std::process::exit(2);
    }
    println!(
        "{:<14} {:<30} {:<8} {:<8} {:<7} status",
        "id", "name", "class", "gate", "paints"
    );
    for e in &catalog.entries {
        let class = match e.class {
            mm2_content::VehicleClass::Stock => "stock",
            mm2_content::VehicleClass::Mod => "mod",
            mm2_content::VehicleClass::ModOnly => "mod-only",
        };
        let (gate, gated_paints) = match garage.row(&e.id) {
            Some(row) => (
                if !row.listed {
                    "unlisted".to_string()
                } else {
                    match row.gate {
                        mm2_game::VehicleGate::Open => "open".to_string(),
                        mm2_game::VehicleGate::Reward => "reward".to_string(),
                    }
                },
                row.paint_gates
                    .iter()
                    .filter(|g| **g == mm2_game::PaintGate::Reward)
                    .count(),
            ),
            None => ("?".to_string(), 0),
        };
        let paints = if gated_paints > 0 {
            format!("{}+{gated_paints}g", e.paints.len() - gated_paints)
        } else {
            e.paints.len().to_string()
        };
        let status = match &e.status {
            mm2_content::EntryStatus::Ready => "ready".to_string(),
            mm2_content::EntryStatus::Incomplete { missing } => {
                format!("incomplete: {}", missing.join(", "))
            }
        };
        println!(
            "{:<14} {:<30} {:<8} {:<8} {:<7} {}",
            e.id, e.display_name, class, gate, paints, status
        );
    }
    for d in &garage.diagnostics {
        eprintln!("garage: {d}");
    }
    let failures = catalog.stock_audit_failures();
    if !failures.is_empty() {
        eprintln!("\nexpected-stock audit failures:");
        for f in &failures {
            eprintln!("  {f}");
        }
    }
}

fn parse_bot_speed(value: &str) -> Result<f32, String> {
    let speed: f32 = value.parse().map_err(|_| "expected a speed in m/s")?;
    if speed.is_finite() && (2.0..=40.0).contains(&speed) {
        Ok(speed)
    } else {
        Err("bot speed must be finite and within 2..=40 m/s".into())
    }
}

fn parse_bot_route(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    mm2_app::scripted::load_bot_route(&path)?;
    Ok(path)
}

#[cfg(test)]
mod bot_route_cli_tests {
    use super::*;

    #[test]
    fn bot_route_requires_bot_and_event() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n0,0,0,0,0,0,0,0,0\n0,0,10,0,0,0,0,0,0\n").unwrap();
        let path = file.path().to_str().unwrap();
        for args in [
            vec!["mm2", "--bot-route", path],
            vec!["mm2", "--bot", "--bot-route", path],
            vec!["mm2", "--event", "circuit:0", "--bot-route", path],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap_err().kind(),
                clap::error::ErrorKind::MissingRequiredArgument
            );
        }
        let cli =
            Cli::try_parse_from(["mm2", "--bot", "--event", "circuit:0", "--bot-route", path])
                .unwrap();
        assert_eq!(cli.bot_route.as_deref(), Some(file.path()));
    }

    #[test]
    fn bot_route_rejects_parse_diagnostics() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n0,0,0,0,0,0,0,0,0\nwrong\n0,0,10,0,0,0,0,0,0\n").unwrap();
        let error = Cli::try_parse_from([
            "mm2",
            "--bot",
            "--event",
            "blitz:0",
            "--bot-route",
            file.path().to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("at line 3"));
    }

    #[test]
    fn bot_route_rejects_missing_file() {
        let error = Cli::try_parse_from([
            "mm2",
            "--bot",
            "--event",
            "blitz:0",
            "--bot-route",
            "missing-evidence-guide.opp",
        ])
        .unwrap_err();
        assert!(error.to_string().contains("cannot read bot route"));
    }
}

#[cfg(test)]
mod capture_input_tests {
    use super::*;

    #[derive(Resource, Default)]
    struct InputWrites {
        keyboard: u32,
        evidence: u32,
        opponent: u32,
    }

    fn keyboard(mut writes: ResMut<InputWrites>) {
        writes.keyboard += 1;
    }

    fn evidence(mut writes: ResMut<InputWrites>) {
        writes.evidence += 1;
    }

    fn opponent(mut writes: ResMut<InputWrites>) {
        writes.opponent += 1;
    }

    #[test]
    fn explicit_bot_capture_runs_evidence_but_keeps_keyboard_frozen() {
        let mut app = App::new();
        app.init_resource::<InputWrites>()
            .insert_resource(SmokeTest {
                world: "capture-policy".into(),
                screenshot: None,
                frames_left: 20,
                pending: None,
                capture_wait: 0,
            })
            .add_systems(
                Update,
                (
                    keyboard.run_if(not(capturing)),
                    evidence.run_if(evidence_capture_input_allowed),
                    opponent.run_if(evidence_capture_input_allowed),
                ),
            );
        app.update();
        let writes = app.world().resource::<InputWrites>();
        assert_eq!(
            (writes.keyboard, writes.evidence, writes.opponent),
            (0, 0, 0)
        );
        app.insert_resource(scripted::ScriptedDrive);
        app.update();
        let writes = app.world().resource::<InputWrites>();
        assert_eq!(
            (writes.keyboard, writes.evidence, writes.opponent),
            (0, 1, 1)
        );
        app.world_mut().remove_resource::<scripted::ScriptedDrive>();
        app.update();
        let writes = app.world().resource::<InputWrites>();
        assert_eq!(
            (writes.keyboard, writes.evidence, writes.opponent),
            (0, 1, 1)
        );
        app.world_mut().remove_resource::<SmokeTest>();
        app.update();
        let writes = app.world().resource::<InputWrites>();
        assert_eq!(
            (writes.keyboard, writes.evidence, writes.opponent),
            (1, 2, 2)
        );
    }
}

#[cfg(test)]
mod report_label_tests {
    use super::*;

    #[test]
    fn the_report_names_the_window_and_step_rate_the_app_runs_at() {
        assert_eq!(window_label((1280, 720)), "1280x720");
        assert_eq!(
            window_label(settings::WindowSize::default().logical()),
            "1280x720",
            "the shipped window is the report's baseline size"
        );
        // `f64`'s Display drops a whole number's fraction: the row reads `60`.
        assert_eq!(FIXED_HZ.to_string(), "60");
    }
}
