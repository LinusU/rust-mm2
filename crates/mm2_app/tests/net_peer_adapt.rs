//! NETPLAY-10B follow-up: the snapshot producer honours each seat's
//! `Host::peer_adapt`. A lane seat whose uplink share is starved is sent
//! every `snapshot_divisor`-th snapshot and gets the rate back once the
//! cap is restored; a seat on the control stream (never adapted) stays at
//! the full rate; and a frame carrying an impact is never thinned.
//!
//! Evidence level: synthetic integration (in-process host app, loopback
//! sockets, no retail data).

use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use mm2_app::net::{HostLink, LobbyState};
use mm2_app::netdrive::{self, NetDriveReport};
use mm2_game::{ImpactEvent, ImpactId, ObjectId};
use mm2_net::{Client, Message, hello};

use crate::net_app::{
    dev_cruise, host_app, host_link, hosted_playing, ready_peer, spin, until_wire,
};

static FILLER_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const WAIT: Duration = Duration::from_secs(30);

fn seat_snaps(app: &App, seat: u16) -> u64 {
    app.world()
        .resource::<NetDriveReport>()
        .seat_snaps
        .get(&seat)
        .copied()
        .unwrap_or(0)
}

/// A ~4 kB filler `Snap` group for a tick the session never reaches: the
/// load that makes a capped uplink congest (the real snapshots of a
/// two-seat cruise are far below the 12 kB/s floor of the cap).
fn filler(generation: u64, tick: u64) -> Message {
    Message::Snap {
        generation,
        tick,
        entries: Vec::new(),
        trailers: Vec::new(),
        impacts: vec![
            mm2_net::SnapImpact {
                seat: 1,
                id: 1,
                tick,
                epoch: 0,
                ordinal: 0,
                point: [0.0; 3],
                normal: [0.0; 3],
                severity: 1.0,
                audio_id: 0,
            };
            mm2_net::MAX_SNAP_IMPACTS as usize
        ],
        race: None,
    }
}

/// Update the app for `window` of wall time; the snapshots each seat got
/// and how many updates ran.
fn run(app: &mut App, window: Duration, seats: [u16; 2], load: bool) -> ([u64; 2], u64) {
    let before = seats.map(|s| seat_snaps(app, s));
    let started = Instant::now();
    let mut updates = 0;
    let generation = app
        .world()
        .resource::<mm2_game::Session>()
        .wire_generation();
    let ctl = app.world().resource::<HostLink>().ctl().clone();
    let mut next_load = 0u64;
    while started.elapsed() < window {
        if load && started.elapsed().as_millis() as u64 >= next_load * 16 {
            let tick = (1 << 40) + FILLER_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = ctl.send_snap_groups_prio(
                seats[0],
                generation,
                tick,
                &[filler(generation, tick)],
                &[mm2_net::Priority::Movement],
            );
            next_load += 1;
        }
        app.update();
        updates += 1;
        thread::sleep(Duration::from_millis(1));
    }
    let after = seats.map(|s| seat_snaps(app, s));
    ([after[0] - before[0], after[1] - before[1]], updates)
}

#[test]
fn a_starved_lane_seat_is_thinned_and_recovers_while_a_healthy_seat_is_not() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    app.insert_resource(netdrive::SnapshotRate(mm2_net::Cadence::default()));

    // The healthy seat: control stream, never adapted.
    let healthy = ready_peer(addr, "healthy", fp);
    let healthy_id = healthy.player_id();
    // The lane seat the cap will starve.
    let mut lane = Client::join_lane(addr, &hello("adapt".to_string(), "lane".to_string(), fp))
        .expect("lane join failed");
    lane.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let lane_id = lane.player_id();
    {
        let ctl = lane.ctl().unwrap();
        ctl.set_vehicle("", 0).unwrap();
        ctl.set_ready(true).unwrap();
    }
    until_wire(
        &mut lane,
        |m| matches!(m, Message::Roster { players: r } if r.iter().any(|e| e.driver == "lane" && e.ready && e.pick.is_some())),
    );
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .filter(|e| e.pick.is_some())
            .count()
            >= 2
    });
    hosted_playing(&mut app);

    // Both peers drain their streams so the lane acknowledges and the
    // control stream never backs up.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drains: Vec<_> = [healthy, lane]
        .into_iter()
        .map(|mut c| {
            let stop = stop.clone();
            thread::spawn(move || {
                c.set_timeout(Some(Duration::from_millis(50))).unwrap();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = c.recv();
                }
            })
        })
        .collect();
    let finish = |stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
                  drains: Vec<thread::JoinHandle<()>>| {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for d in drains {
            d.join().unwrap();
        }
    };

    let seats = [lane_id, healthy_id];
    let window = Duration::from_millis(1000);
    let hz = f64::from(mm2_net::SNAPSHOT_HZ);
    let lane_ready = |app: &App| app.world().resource::<HostLink>().ctl().peer_adapt(lane_id);

    // Baseline: full adapt, both seats at the snapshot rate.
    let deadline = Instant::now() + WAIT;
    while lane_ready(&app).is_none() {
        assert!(Instant::now() < deadline, "the lane never established");
        app.update();
        thread::sleep(Duration::from_millis(5));
    }
    let (base, _) = run(&mut app, window, seats, false);
    assert!(
        base.iter().all(|&n| n as f64 >= hz * 0.6),
        "baseline rate: {base:?}"
    );

    // Starve the uplink until the lane seat's divisor rises.
    let ctl = app.world().resource::<HostLink>().ctl().clone();
    ctl.set_uplink_cap(2 * 1024);
    let deadline = Instant::now() + WAIT;
    let mut divisor = 1;
    while divisor <= 1 {
        assert!(
            Instant::now() < deadline,
            "the starved seat was never adapted"
        );
        run(&mut app, Duration::from_millis(100), seats, true);
        divisor = lane_ready(&app).map_or(1, |a| a.snapshot_divisor);
    }
    let divided0 = app.world().resource::<NetDriveReport>().snaps_seat_divided;
    let (thin, _) = run(&mut app, window, seats, true);
    let divided1 = app.world().resource::<NetDriveReport>().snaps_seat_divided;
    assert!(divided1 > divided0, "nothing was withheld");
    assert!(
        (thin[0] as f64) < base[0] as f64 * 0.8,
        "the starved seat kept its rate: {thin:?} vs {base:?}"
    );
    assert!(
        thin[1] as f64 >= hz * 0.6,
        "the healthy seat was thinned: {thin:?}"
    );

    // An impact is never thinned: one forced frame per update, every seat.
    let generation = app
        .world()
        .resource::<mm2_game::Session>()
        .wire_generation();
    let tick = app.world().resource::<mm2_game::Session>().tick();
    let divided2 = app.world().resource::<NetDriveReport>().snaps_seat_divided;
    let before = seats.map(|s| seat_snaps(&app, s));
    let impact_updates = 20;
    for n in 0..impact_updates {
        app.world_mut().write_message(ImpactEvent {
            id: ImpactId(1000 + n),
            generation,
            tick,
            participants: (ObjectId::WORLD, ObjectId::WORLD),
            point: Vec3::ZERO,
            normal: Vec3::Z,
            severity: 5.0,
            surface: Default::default(),
        });
        app.update();
    }
    let after = seats.map(|s| seat_snaps(&app, s));
    assert_eq!(
        [after[0] - before[0], after[1] - before[1]],
        [impact_updates, impact_updates],
        "a frame carrying an impact must reach every seat"
    );
    assert_eq!(
        app.world().resource::<NetDriveReport>().snaps_seat_divided,
        divided2,
        "an impact frame was thinned"
    );

    // Restore the cap: the lane seat returns to the full rate.
    ctl.set_uplink_cap(mm2_net::HOST_UPLINK_BYTES_PER_SEC);
    let deadline = Instant::now() + WAIT;
    loop {
        let (now, _) = run(&mut app, Duration::from_millis(500), seats, false);
        if now[0] as f64 >= hz * 0.5 * 0.6 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the seat never recovered: {now:?}"
        );
    }
    let deadline = Instant::now() + WAIT;
    while lane_ready(&app).is_some_and(|a| a.snapshot_divisor > 1) {
        assert!(Instant::now() < deadline, "the divisor never returned to 1");
        run(&mut app, Duration::from_millis(200), seats, false);
    }
    let (back, _) = run(&mut app, window, seats, false);
    finish(stop, drains);
    assert!(
        back[0] as f64 >= hz * 0.6,
        "the recovered seat's rate: {back:?}"
    );
}
