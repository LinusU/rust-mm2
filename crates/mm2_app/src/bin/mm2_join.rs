//! `mm2-join` — the headless lobby client (F24-B.6), the joining-side
//! counterpart of `mm2-host`.
//!
//! Mounts an MM2 installation read-only, computes the gameplay
//! fingerprint the handshake gates on, joins a lobby at `--connect`
//! and prints each lobby transition as a `key=value` line — the
//! client-side record contract `tests/net_join.rs` drives. No window,
//! GPU, audio device or Bevy app is ever touched: the process is a
//! lobby participant. Building the session a `Start` announces is
//! F25/F26 scope — a started session is reported, not spawned.
//!
//! The printed record contract:
//!
//! ```text
//! connected=<addr> id=<n> driver="<name>" fingerprint=fnv1a64:<hex>
//! session="<summary>"                              (each advertisement)
//! event=roster players=<n> ready=<n>
//! event=pick_refused reason="<text>"
//! event=started generation=<n> session="<summary>"
//! event=cancelled generation=<n>
//! event=join_failed reason="<text>"                (connect/handshake failed)
//! event=session_refused reason="<text>"            (session unrunnable here)
//! event=closed reason="<text>"                     (the host went away)
//! ```
//!
//! stdin is the driver's control surface — one command per line:
//! `vehicle <id>[:<paint>]` (a bare `vehicle` picks the dev car),
//! `ready`, `unready`, `quit` (a clean leave). A closed stdin means
//! unattended participation. `--vehicle`/`--ready` do the same once at
//! startup.
//!
//! Every advertised session — the lobby's `Session` and the running
//! one inside `Start` — is decoded and checked against *this* install
//! (`net::accept` + `net::check_session`): a session this mount cannot
//! run gets `session_refused` and a clean leave rather than a held
//! seat that could never spawn. The handshake fingerprint means an
//! honest host's content is identical to ours; the gate is the
//! defense-in-depth for one that is not.
//!
//! Exit codes: 0 on a clean `quit`; 1 for a refused join, a refused
//! session or a lost host; 2 for usage and startup failures.

use std::io::{self, BufRead};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

use clap::Parser;
use mm2_app::{net, smoke};
use mm2_assets::{InstallMount, Vfs, mount_install, mount_mods};
use mm2_content::fingerprint;
use mm2_net::{Client, ClientCtl, MAX_STRING, Message, SessionAdvertisement, hello};

/// Bound on the wait for the host's close after our `Leave` — the
/// honest host disconnects a leaver promptly; this is the backstop for
/// one that does not. The exit code has already been decided when it
/// fires, so the watchdog just takes it.
const LEAVE_WATCHDOG: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(
    name = "mm2-join",
    about = "Headless lobby client for the MM2-inspired engine"
)]
struct Cli {
    /// Path to a Midtown Madness 2 installation (directory containing
    /// the .ar archives and/or loose files). Mounted read-only; its
    /// gameplay fingerprint is the join gate.
    #[arg(long)]
    mm2_path: PathBuf,

    /// Directory containing mod folders (each with a mod.toml). Gameplay
    /// mods move the fingerprint — the client must mount the same set
    /// the host does.
    #[arg(long)]
    mods: Option<PathBuf>,

    /// Host address to join — the `listening=` record `mm2-host`
    /// prints. Loopback or LAN is the operator's explicit choice; the
    /// client never listens.
    #[arg(long)]
    connect: SocketAddr,

    /// Driver name the lobby roster shows.
    #[arg(long, default_value = "driver")]
    driver: String,

    /// Pick a vehicle right after joining: `<id>` or `<id>:<paint>`;
    /// an empty id picks the synthetic dev car. The host's validator
    /// rules on legality — a refused pick arrives as `pick_refused`.
    #[arg(long, value_name = "id[:paint]")]
    vehicle: Option<String>,

    /// Mark ready right after joining (sent after `--vehicle`).
    #[arg(long)]
    ready: bool,
}

/// The `<id>`/`<id>:<paint>` grammar `--vehicle` and the `vehicle`
/// stdin command share. An empty id is the dev car; no `:paint` means
/// paint 0.
fn parse_pick(arg: &str) -> Result<(String, u8), String> {
    match arg.split_once(':') {
        Some((id, paint)) => paint
            .parse::<u8>()
            .map(|paint| (id.to_string(), paint))
            .map_err(|_| format!("invalid paint {paint:?}: expected 0-255")),
        None => Ok((arg.to_string(), 0)),
    }
}

/// Decode one advertised session blob and prove this install can run
/// it — the check every `Session`/`Start` payload gets.
fn check_advertised(vfs: &Vfs, ad: &SessionAdvertisement) -> Result<(), String> {
    let config = net::accept(ad).map_err(|e| e.to_string())?;
    net::check_session(vfs, &config).map_err(|e| e.to_string())
}

/// Mark the process as leaving: `code` is the exit status taken once
/// the host closes our socket (0 = `quit`, 1 = refused session). The
/// `Leave` prompts the host's close and wakes the blocked `recv`; the
/// watchdog bounds the wait on a host that never closes.
fn leave(leaving: &Arc<AtomicI32>, ctl: &ClientCtl, code: i32) {
    leaving.store(code, Ordering::Relaxed);
    let _ = ctl.leave();
    thread::spawn(move || {
        thread::sleep(LEAVE_WATCHDOG);
        std::process::exit(code);
    });
}

fn main() {
    let cli = Cli::parse();

    // Content: install (read-only) plus any mods — the same mounting
    // policy `mm2`, `mm2-host` and `mm2-inspect` use.
    let mut vfs = Vfs::new();
    if let Err(e) = mount_install(&mut vfs, &cli.mm2_path, &InstallMount::default()) {
        eprintln!("error: mounting {}: {e}", cli.mm2_path.display());
        std::process::exit(2);
    }
    if let Some(mods) = &cli.mods {
        match mount_mods(&mut vfs, mods) {
            Ok(manifests) => {
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

    // The driver name rides a bounded wire field — refuse an over-long
    // one as a usage error rather than letting the Hello encode fail.
    if cli.driver.len() > MAX_STRING {
        eprintln!(
            "error: --driver is {} bytes; the wire bound is {MAX_STRING}",
            cli.driver.len()
        );
        std::process::exit(2);
    }
    let pick = match cli.vehicle.as_deref().map(parse_pick).transpose() {
        Ok(pick) => pick,
        Err(e) => {
            eprintln!("error: invalid --vehicle: {e}");
            std::process::exit(2);
        }
    };

    let mut client = match Client::join(
        cli.connect,
        &hello(
            smoke::COMMIT.to_string(),
            cli.driver.clone(),
            fingerprint.hash,
        ),
    ) {
        Ok(client) => client,
        Err(e) => {
            println!("event=join_failed reason={e:?}");
            std::process::exit(1);
        }
    };
    println!(
        "connected={} id={} driver={:?} fingerprint={}",
        cli.connect,
        client.player_id(),
        cli.driver,
        fingerprint.display(),
    );
    let ctl = match client.ctl() {
        Ok(ctl) => ctl,
        Err(e) => {
            eprintln!("error: cloning the socket's send half: {e}");
            std::process::exit(1);
        }
    };

    // The driver's control surface: one command per line on stdin,
    // reaching the blocked recv loop through `ClientCtl` — the same
    // shape `mm2-host`'s stdin reader drives `HostCtl` with.
    let leaving = Arc::new(AtomicI32::new(-1));
    {
        let ctl = ctl.clone();
        let leaving = leaving.clone();
        thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("ready") => drop(ctl.set_ready(true)),
                    Some("unready") => drop(ctl.set_ready(false)),
                    Some("vehicle") => match parse_pick(words.next().unwrap_or("")) {
                        Ok((id, paint)) => drop(ctl.set_vehicle(&id, paint)),
                        Err(e) => eprintln!("error: {e}"),
                    },
                    Some("quit") => {
                        leave(&leaving, &ctl, 0);
                        return;
                    }
                    Some(other) => eprintln!("error: unknown command {other:?}"),
                    None => {}
                }
            }
        });
    }

    // Startup picks readies: `vehicle` then `ready`, same wire order a
    // driver would use.
    if let Some((id, paint)) = pick {
        let _ = ctl.set_vehicle(&id, paint);
    }
    if cli.ready {
        let _ = ctl.set_ready(true);
    }

    loop {
        match client.recv() {
            Ok(Message::Session(ad)) => {
                println!("session={:?}", ad.summary);
                if let Err(e) = check_advertised(&vfs, &ad) {
                    println!("event=session_refused reason={e:?}");
                    leave(&leaving, &ctl, 1);
                }
            }
            Ok(Message::Roster { players }) => println!(
                "event=roster players={} ready={}",
                players.len(),
                players.iter().filter(|p| p.ready).count()
            ),
            Ok(Message::VehicleRefused { reason }) => {
                println!("event=pick_refused reason={reason:?}")
            }
            Ok(Message::Start {
                generation,
                session,
            }) => match check_advertised(&vfs, &session) {
                Ok(()) => println!(
                    "event=started generation={generation} session={:?}",
                    session.summary
                ),
                Err(e) => {
                    println!("event=session_refused reason={e:?}");
                    leave(&leaving, &ctl, 1);
                }
            },
            Ok(Message::Cancel { generation }) => {
                println!("event=cancelled generation={generation}")
            }
            // The host→client set is closed — `join` consumed the
            // Welcome and the variants above are everything else it
            // sends. Anything else is ignored.
            Ok(_) => {}
            Err(e) => match leaving.load(Ordering::Relaxed) {
                // Our own `Leave` prompted this close.
                code @ (0 | 1) => std::process::exit(code),
                _ => {
                    println!("event=closed reason={e:?}");
                    std::process::exit(1);
                }
            },
        }
    }
}
