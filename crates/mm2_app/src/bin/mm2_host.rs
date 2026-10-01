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
//! ```
//!
//! Usage errors and startup failures exit 2; a host loop that dies on
//! its own exits 1.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use mm2_app::net;
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_content::{VehicleCatalog, fingerprint};
use mm2_game::{
    Difficulty, SessionAuthority, SessionConditions, SessionConfig, SessionMode, TimeOfDay,
    Weather, WorldMode,
};
use mm2_net::{Host, HostConfig, HostEvent, LeaveCause};

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

    // The session this lobby runs. Cruise only for now — event hosting
    // lands with its own F24-B leg. A requested city must actually
    // resolve: advertising a world the host cannot load would fail every
    // client at session start instead of at flag time.
    let world = if cli.dev_world {
        WorldMode::DevWorld
    } else {
        let city = cli.city.as_deref().unwrap_or("london").to_ascii_lowercase();
        let psdl = format!("city/{city}.psdl");
        if vfs.resolve(&psdl).is_none() {
            eprintln!("error: city {city:?} has no resolvable {psdl}");
            std::process::exit(2);
        }
        WorldMode::City { psdl }
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
        mode: SessionMode::Cruise,
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

    while let Ok(event) = host.recv() {
        println!("{}", describe(&event));
    }
    eprintln!("error: host loop ended unexpectedly");
    std::process::exit(1);
}

fn describe(event: &HostEvent) -> String {
    match event {
        HostEvent::Joined { id, driver, build } => {
            format!("event=joined id={id} driver={driver:?} build={build:?}")
        }
        HostEvent::Left { id, driver, cause } => {
            let cause = match cause {
                LeaveCause::Quit => "quit",
                LeaveCause::Lost => "lost",
                LeaveCause::Malformed => "malformed",
            };
            format!("event=left id={id} driver={driver:?} cause={cause}")
        }
        HostEvent::ReadyChanged { id, ready } => {
            format!("event=ready id={id} ready={ready}")
        }
        HostEvent::VehicleChanged { id, vehicle, paint } => {
            format!("event=vehicle id={id} vehicle={vehicle:?} paint={paint}")
        }
        HostEvent::VehicleRefused {
            id,
            vehicle,
            paint,
            reason,
        } => {
            format!(
                "event=pick_refused id={id} vehicle={vehicle:?} paint={paint} reason={reason:?}"
            )
        }
        HostEvent::JoinFailed { peer, reason } => {
            format!("event=join_failed peer={peer} reason={reason:?}")
        }
    }
}
