//! `mm2-host` — the headless dedicated lobby host (F24-B).
//!
//! Runs an `mm2_net` lobby `Host` over an MM2 installation's content:
//! mounts the VFS read-only, computes the gameplay fingerprint the
//! handshake gates joins on, advertises the configured session to every
//! client and prints each lobby event as a `key=value` line. No window,
//! GPU, audio device or Bevy app is ever touched — the process is a
//! lobby driver, nothing more.
//!
//! The printed record contract (which integration tests also drive):
//!
//! ```text
//! listening=<addr> fingerprint=fnv1a64:<hex> seed=<n> session="<summary>"
//! event=joined id=<n> driver="<name>" build="<id>"
//! event=left id=<n> driver="<name>" cause=quit|lost|malformed
//! event=ready id=<n> ready=<bool>
//! event=vehicle id=<n> vehicle="<id>" paint=<n>
//! event=pick_refused id=<n> vehicle="<id>" paint=<n> reason="<text>"
//! event=join_failed peer=<addr> reason="<text>"
//! event=started generation=<n>
//! event=start_refused reason="<text>"
//! event=cancelled generation=<n>
//! ```
//!
//! stdin is the operator's control surface — one command per line:
//! `start` requests session start, `cancel` returns everyone to the
//! lobby, `quit` shuts the host down cleanly. A closed stdin just means
//! unattended operation. `start`'s late-join policy is the session
//! mode's (MP-5, documented — `help:Multiplayer Games`): an event lobby
//! closes to joins once started, a cruise lobby stays open.
//!
//! Usage errors and startup failures exit 2; a host loop that dies on
//! its own exits 1.

use std::io::{self, BufRead};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use mm2_app::{net, race};
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_content::{VehicleCatalog, fingerprint};
use mm2_game::{
    Difficulty, EventRef, SessionAuthority, SessionConditions, SessionConfig, SessionMode,
    TimeOfDay, Weather, WorldMode,
};
use mm2_net::{Host, HostConfig, LateJoin};

#[derive(Parser, Debug)]
#[command(
    name = "mm2-host",
    about = "Headless dedicated lobby host for the MM2-inspired engine"
)]
struct Cli {
    /// Path to a Midtown Madness 2 installation (directory containing
    /// the .ar archives and/or loose files). Mounted read-only; its
    /// gameplay fingerprint is the join gate.
    #[arg(long)]
    mm2_path: PathBuf,

    /// Directory containing mod folders (each with a mod.toml). Gameplay
    /// mods move the fingerprint, so clients must mount the same set.
    #[arg(long)]
    mods: Option<PathBuf>,

    /// Address to listen on. Loopback+ephemeral is the default; passing
    /// a LAN interface is an explicit operator choice — the host never
    /// binds a public interface unprompted.
    #[arg(long, default_value = "127.0.0.1:0")]
    bind: SocketAddr,

    /// Host the synthetic dev world — no city content needed.
    #[arg(long, conflicts_with = "city")]
    dev_world: bool,

    /// City to host (`sf`, `london`, or a mod-provided stem; default
    /// `london`). A city whose `city/<name>.psdl` does not resolve
    /// through the mounted VFS is refused: the host must not advertise
    /// a world it cannot load.
    #[arg(long)]
    city: Option<String>,

    /// Host an authored event instead of cruise: `<table>:<row>` with
    /// table one of `checkpoint`, `blitz`, `circuit`, `crash` and row
    /// the 0-based table row in the `--city` tables (default `london`).
    /// The event is resolved and built through the same path a session
    /// load takes — one that cannot run here is a startup failure,
    /// never an advertised session.
    #[arg(long, value_name = "table:row")]
    event: Option<String>,

    /// Host the Professional parameter block instead of Amateur.
    #[arg(long)]
    pro: bool,

    /// Weather selector for the cruise session (0-3).
    #[arg(long, value_name = "0-3")]
    weather: Option<u8>,

    /// Time-of-day selector for the cruise session (0-3).
    #[arg(long, value_name = "0-3")]
    time_of_day: Option<u8>,

    /// Session seed replicated to every client. Defaults to a
    /// clock-derived value — pass an explicit seed to reproduce a run;
    /// the chosen value is printed in the `listening` record.
    #[arg(long)]
    seed: Option<u64>,
}

fn main() {
    let cli = Cli::parse();

    // Content: install (read-only) plus any mods — the same mounting
    // policy `mm2` and `mm2-inspect` use.
    let mut vfs = Vfs::new();
    if let Err(e) = mount_install(&mut vfs, &cli.mm2_path, &InstallMount::default()) {
        eprintln!("error: mounting {}: {e}", cli.mm2_path.display());
        std::process::exit(2);
    }
    let mut mods_active = false;
    if let Some(mods) = &cli.mods {
        match mount_mods(&mut vfs, mods) {
            Ok(manifests) => {
                mods_active = !manifests.is_empty();
                for m in &manifests {
                    eprintln!("mounted mod {} from {}", m.id, mods.display());
                }
            }
            Err(e) => {
                eprintln!("error: mounting mods {}: {e}", mods.display());
                std::process::exit(2);
            }
        }
    }

    let fingerprint = match fingerprint::gameplay(&vfs) {
        Ok(fp) => fp,
        Err(e) => {
            eprintln!("error: fingerprinting content: {e}");
            std::process::exit(2);
        }
    };

    // The session this lobby runs. `--city` names both the world and
    // the event's city, exactly like `mm2`'s `--event`. A requested
    // city must actually resolve: advertising a world the host cannot
    // load would fail every client at session start instead of at flag
    // time.
    let city = cli.city.as_deref().unwrap_or("london").to_ascii_lowercase();
    let world = if cli.dev_world {
        WorldMode::DevWorld
    } else {
        let psdl = format!("city/{city}.psdl");
        if vfs.resolve(&psdl).is_none() {
            eprintln!("error: city {city:?} has no resolvable {psdl}");
            std::process::exit(2);
        }
        WorldMode::City { psdl }
    };
    let mode = match cli.event.as_deref() {
        Some(arg) => match EventRef::parse(arg, &city) {
            Some(event_ref) => SessionMode::Event(event_ref),
            None => {
                eprintln!(
                    "error: invalid --event {arg:?}: expected checkpoint|blitz|circuit|crash:<row>"
                );
                std::process::exit(2);
            }
        },
        None => SessionMode::Cruise,
    };
    let conditions = SessionConditions {
        time_of_day: match cli.time_of_day.map(TimeOfDay::new).transpose() {
            Ok(t) => t.unwrap_or_default(),
            Err(e) => {
                eprintln!("error: invalid --time-of-day: {e}");
                std::process::exit(2);
            }
        },
        weather: match cli.weather.map(Weather::new).transpose() {
            Ok(w) => w.unwrap_or_default(),
            Err(e) => {
                eprintln!("error: invalid --weather: {e}");
                std::process::exit(2);
            }
        },
    };
    let seed = cli.seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    let config = SessionConfig {
        world,
        mode,
        difficulty: if cli.pro {
            Difficulty::Professional
        } else {
            Difficulty::Amateur
        },
        conditions,
        seed,
        authority: SessionAuthority::Host,
        mods_active,
        ..SessionConfig::default()
    };

    // An event host must not advertise a session it cannot run, so the
    // gate is the same resolution a session load takes — catalog scan,
    // dependency-checked resolve, race-definition build (plus the
    // authored roster/reward surface). An unknown row, missing records
    // or a definition that fails to build are flag-time errors; Crash
    // Course rows refuse here the same way `mm2` refuses them
    // (`RaceBuildError::CrashCourseUnsupported`, F21).
    if let (SessionMode::Event(event_ref), Some(arg)) = (&config.mode, cli.event.as_deref())
        && let Err(e) = race::event_race_setup(&vfs, event_ref, config.difficulty)
    {
        eprintln!("error: --event {arg:?} cannot run here: {e}");
        std::process::exit(2);
    }

    let ad = match net::advertise(&config) {
        Ok(ad) => ad,
        Err(e) => {
            eprintln!("error: session config cannot be advertised: {e}");
            std::process::exit(2);
        }
    };

    // Dedicated host: no local player seat, so the full wire ceiling is
    // available to remote clients (HostConfig::new defaults to it).
    // Vehicle picks are gated on the mounted catalog — a peer cannot
    // roster a car that cannot spawn. An empty install leaves only the
    // dev car legal, which suits a --dev-world lobby.
    let catalog = VehicleCatalog::scan(&vfs);
    let host_config = HostConfig {
        pick_validator: Some(net::vehicle_validator(&catalog)),
        ..HostConfig::new(fingerprint.hash)
    };
    let host = match Host::listen(cli.bind, &host_config) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("error: listening on {}: {e}", cli.bind);
            std::process::exit(2);
        }
    };
    if let Err(e) = host.set_session(ad.clone()) {
        eprintln!("error: advertising session: {e}");
        std::process::exit(2);
    }
    println!(
        "listening={} fingerprint={} seed={seed} session={:?}",
        host.addr(),
        fingerprint.display(),
        ad.summary
    );

    // The operator's control surface: one command per line on stdin.
    // `Host` is !Sync (the event channel is a Receiver), so the reader
    // thread drives the loop through a `HostCtl` handle. A closed
    // stdin ends the thread without touching the host — unattended
    // operation is normal. `start` takes the session mode's late-join
    // policy (MP-5): an event lobby closes to joins once started,
    // a cruise lobby stays open.
    let start_policy = match config.mode {
        SessionMode::Event(_) => LateJoin::Closed,
        SessionMode::Cruise => LateJoin::Open,
    };
    let quitting = Arc::new(AtomicBool::new(false));
    {
        let ctl = host.ctl();
        let quitting = quitting.clone();
        thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                match line.trim() {
                    "start" => drop(ctl.start(start_policy)),
                    "cancel" => drop(ctl.cancel()),
                    "quit" => {
                        quitting.store(true, Ordering::Relaxed);
                        drop(ctl.shutdown());
                        return;
                    }
                    "" => {}
                    other => eprintln!("error: unknown command {other:?}"),
                }
            }
        });
    }

    while let Ok(event) = host.recv() {
        println!("{}", net::describe_host_event(&event));
    }
    if quitting.load(Ordering::Relaxed) {
        return;
    }
    eprintln!("error: host loop ended unexpectedly");
    std::process::exit(1);
}
