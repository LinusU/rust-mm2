//! Multi-group `Snap` demand against the lane's wall-clock rate controller
//! (#2101, follow-up to #2085).
//!
//! The app sends each seat an owner group (`Movement`) and, for a seat with
//! a far island in range, a world group (`Bulk`) as one baselined state per
//! tick. An unpaced headless sim steps about 5x wall clock, so the host
//! offered the controller 5x the demand a 60 Hz player sends and the lane's
//! rate collapsed in `net_cnr_bots_mp`. These tests reproduce both demands
//! over loopback without retail data and measure what a second group costs.
//! Evidence level: synthetic integration over loopback.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::lane_tests::{eventually, hello_for, host, start_session};
use crate::{Client, MAX_SNAP_IMPACTS, Message, PaceStats, Priority, SnapEntry, SnapImpact};

/// Drive one seat with `groups` groups per tick at `hz` for `secs`, a client
/// reading throughout, and return the host's pacing stats for the seat.
fn drive(groups: usize, hz: u32, secs: f64, heavy: bool) -> (PaceStats, u64) {
    let host = host();
    let mut client = Client::join_lane(host.addr(), &hello_for("multigroup")).unwrap();
    let id = client.player_id();
    eventually("a lane", || host.lane_established(id));
    let generation = start_session(&host, &mut [&mut client]);
    client.set_timeout(Some(Duration::from_millis(5))).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = client.recv();
            }
        })
    };
    let ctl = host.ctl();
    let period = Duration::from_secs_f64(1.0 / f64::from(hz));
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut next = Instant::now();
    let mut tick = 0u64;
    while Instant::now() < end {
        tick += 1;
        let ids: Vec<(u8, Message)> = (0..groups)
            .map(|g| (g as u8, group(generation, tick, g, heavy)))
            .collect();
        let prios: Vec<Priority> = (0..groups)
            .map(|g| {
                if g == 0 {
                    Priority::Movement
                } else {
                    Priority::Bulk
                }
            })
            .collect();
        ctl.send_snap_states_prio(id, generation, tick, 1, &ids, &prios)
            .unwrap();
        next += period;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let pace = host.lane_pace(id).unwrap();
    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    (pace, tick)
}

/// A group of `Snap` rows: group 0 holds the owner's row, the others a
/// handful of ambient cars that move every tick, plus (`heavy`) a full
/// impact list.
fn group(generation: u64, tick: u64, g: usize, heavy: bool) -> Message {
    let count = if g == 0 { 1 } else { 6 };
    let entries = (0..count)
        .map(|n| SnapEntry {
            player: (g * 100 + n) as u16,
            pos: [tick as f32 * 0.5 + n as f32, g as f32, 0.0],
            vel: [15.0, 0.0, 0.0],
            ..SnapEntry::default()
        })
        .collect();
    let impacts = if g == 0 || !heavy {
        Vec::new()
    } else {
        (0..MAX_SNAP_IMPACTS)
            .map(|i| SnapImpact {
                seat: 1,
                id: u64::from(i) + tick * 64,
                tick,
                epoch: 0,
                ordinal: 0,
                point: [
                    tick as f32 * 0.37 + i as f32,
                    (tick * 7 + u64::from(i)) as f32 * 0.11,
                    1.5,
                ],
                normal: [0.0, 1.0, 0.0],
                severity: 1.0 + (tick % 13) as f32,
                audio_id: 0,
            })
            .collect()
    };
    Message::Snap {
        generation,
        tick,
        entries,
        trailers: Vec::new(),
        impacts,
        race: None,
    }
}

/// Bytes per tick of a seat's lane, from the host's payload counter.
fn per_tick(pace: &PaceStats, ticks: u64) -> f64 {
    pace.bytes_sent as f64 / ticks as f64
}

#[test]
fn a_second_group_costs_bytes_but_not_the_ceiling_at_the_paced_rate() {
    let (one, one_ticks) = drive(1, 30, 2.0, false);
    let (two, two_ticks) = drive(2, 30, 2.0, false);
    let (one_b, two_b) = (per_tick(&one, one_ticks), per_tick(&two, two_ticks));
    eprintln!(
        "bytes per tick: 1 group {one_b:.0}, 2 groups {two_b:.0} ({:.2}x)",
        two_b / one_b
    );
    // The world group carries six rows to the owner group's one, every
    // field changing each tick: it must cost more, and not more than the
    // rows it carries plus framing.
    assert!(two_b > one_b, "{two_b} vs {one_b}");
    assert!(two_b < one_b * 3.0, "{two_b} vs {one_b}");
    for (what, p) in [("1 group", one), ("2 groups", two)] {
        assert_eq!(p.rate, p.ceiling, "{what}: rate left the ceiling: {p:?}");
        assert_eq!(p.decreases, 0, "{what}: {p:?}");
    }
}

#[test]
fn five_times_the_demand_does_not_collapse_a_lossless_lane() {
    // The unpaced headless sim steps about 5x wall clock (#2085). Heavy
    // world groups at 150 Hz exceed the ceiling, so the queue sheds; the
    // controller must not read that as congestion on a clean link.
    let (p, ticks) = drive(2, 150, 3.0, true);
    eprintln!(
        "5x heavy: {ticks} ticks, {:.0} B/tick, rate {} of {}, decreases {}",
        per_tick(&p, ticks),
        p.rate,
        p.ceiling,
        p.decreases
    );
    assert!(p.rate * 2 >= p.ceiling, "rate collapsed: {p:?}");
}
