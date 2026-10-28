//! The `event=lane_pace` record (NETPLAY-10E follow-up): a host+client
//! loopback session prints it with the documented fields, on both roles.
//!
//! Evidence level: synthetic integration (in-process apps, loopback
//! sockets, no retail data).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use mm2_app::lanepace::{self, LanePaceEvery, LanePaceLog};
use mm2_app::net::{LobbyLink, LobbyState};
use mm2_net::{Client, Message, hello};

use crate::net_app::{
    dev_cruise, host_and_link, host_app, host_link, hosted_playing, spin, until_wire,
};

const WAIT: Duration = Duration::from_secs(30);

fn log_lines(app: &App) -> Vec<String> {
    app.world()
        .resource::<LanePaceLog>()
        .lines()
        .cloned()
        .collect()
}

#[test]
fn a_host_prints_a_lane_pace_record_for_each_lane_seat() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    lanepace::add_host_systems(&mut app);
    app.insert_resource(LanePaceEvery(Duration::from_millis(100)));

    let mut lane = Client::join_lane(addr, &hello("pace".to_string(), "lane".to_string(), fp))
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
            .any(|e| e.pick.is_some())
    });
    hosted_playing(&mut app);

    // The peer drains its stream so the lane acknowledges.
    let stop = Arc::new(AtomicBool::new(false));
    let drain = {
        let stop = stop.clone();
        thread::spawn(move || {
            lane.set_timeout(Some(Duration::from_millis(50))).unwrap();
            while !stop.load(Ordering::Relaxed) {
                let _ = lane.recv();
            }
        })
    };

    let want = format!("role=host seat={lane_id} ");
    let deadline = Instant::now() + WAIT;
    while !log_lines(&app).iter().any(|l| l.contains(&want)) {
        assert!(Instant::now() < deadline, "no host lane_pace record");
        app.update();
        thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Relaxed);
    drain.join().unwrap();

    let line = log_lines(&app)
        .into_iter()
        .find(|l| l.contains(&want))
        .unwrap();
    assert!(line.starts_with("event=lane_pace "), "{line}");
    for field in [
        "win_ms=",
        "rate_kbit=",
        "ceil_kbit=",
        "share_kbit=",
        "sent_kbit=",
        "up_cap_kbit=",
        "up_sent_kbit=",
        "headroom_kbit=",
        "wait_p50_ms=",
        "wait_p95_ms=",
        "wait_max_ms=",
        "age_ms=",
        "deferred=",
        "denied=",
        "expired=",
        "dropped=",
        "rtx=",
        "rtx_pct=",
        "in_flight=",
        "adapt=",
    ] {
        assert!(line.contains(field), "{field} missing from {line}");
    }
}

#[test]
fn a_client_prints_a_lane_pace_record() {
    let install = tempfile::tempdir().unwrap();
    let (_host, link, _vfs) = host_and_link(install.path(), &dev_cruise(), "bob");
    assert!(link.lane_counts().is_some(), "the join negotiated no lane");
    let id = link.player_id();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).insert_resource(link);
    lanepace::add_client_systems(&mut app);
    app.insert_resource(LanePaceEvery(Duration::from_millis(50)));

    spin_bounded(&mut app, |a| !log_lines(a).is_empty());
    let line = &log_lines(&app)[0];
    assert!(
        line.starts_with(&format!("event=lane_pace role=client seat={id} ")),
        "{line}"
    );
    for field in ["win_ms=", "tx_dps=", "rx_dps=", "accepted_dps=", "dropped="] {
        assert!(line.contains(field), "{field} missing from {line}");
    }
    assert!(app.world().get_resource::<LobbyLink>().is_some());
}

/// `app.update()` until `pred`, on a wall-clock deadline (the record is
/// wall-clock paced, so a fixed update count would race it).
fn spin_bounded(app: &mut App, pred: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !pred(app) {
        assert!(Instant::now() < deadline, "the record never printed");
        app.update();
        thread::sleep(Duration::from_millis(5));
    }
}
