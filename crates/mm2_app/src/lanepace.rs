//! The `event=lane_pace` record: a LAN or Internet run of the
//! reachability runbook reads the actual rates, queue age and headroom
//! of the lane from the app's own output (NETPLAY-10E follow-up).
//!
//! A system samples the `mm2_net` pacing counters once per
//! [`LanePaceEvery`] (wall clock, so a stalled simulation still reports)
//! and prints one line per lane seat on the host and one line on a
//! client, in the `key=value` style of the other `event=` records. Rates
//! are the difference between two samples, so each line describes the
//! window since the previous one; the first sample only sets the
//! baseline. Lines are also kept in [`LanePaceLog`] (bounded) so a test
//! reads what the process would have printed.
//!
//! Host line, per established seat:
//!
//! `event=lane_pace role=host seat=<id> win_ms=<ms> rate_kbit=<n>
//! ceil_kbit=<n> share_kbit=<n> sent_kbit=<n> up_cap_kbit=<n>
//! up_sent_kbit=<n> headroom_kbit=<n> wait_p50_ms=<f> wait_p95_ms=<f>
//! wait_max_ms=<f> age_ms=<f> deferred=<n> denied=<n> expired=<n>
//! dropped=<n> rtx=<n> rtx_pct=<f> in_flight=<n> adapt=<divisor>`
//!
//! `rate`, `ceil` and `share` are the seat's congestion rate, its ceiling
//! and its share of the host uplink; `sent` is this seat's admitted wire
//! rate over the window and `up_sent` the whole uplink's, so `headroom`
//! is the aggregate cap minus `up_sent`. `wait_*` are the movement
//! queue's wait percentiles over its recent history and `age_ms` the
//! age of its oldest queued datagram now. `deferred`, `denied`,
//! `expired` and `dropped` count, over the window, polls the movement
//! head waited, of those the polls the uplink refused, and datagrams
//! discarded unsent for age or for a newer one (both queues). `rtx` is
//! reliable retransmissions over the window, `rtx_pct` their share of
//! the reliable pieces queued in it.
//!
//! Client line: `event=lane_pace role=client seat=<id> win_ms=<ms>
//! tx_dps=<n> rx_dps=<n> accepted_dps=<n> dropped=<n>`. A client sees
//! datagram counts, not the host's pacing, so it reports datagrams per
//! second sent, received and accepted, and `dropped`, the datagrams the
//! lane received and refused in the window.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use mm2_net::{LaneCounts, PaceStats, ReliableStats};

use crate::net::{HostLink, LobbyLink, LobbyState};

/// Wall-clock spacing of the records.
#[derive(Resource, Debug, Clone, Copy)]
pub struct LanePaceEvery(pub Duration);

impl Default for LanePaceEvery {
    fn default() -> Self {
        Self(Duration::from_secs(2))
    }
}

/// The most recent printed records (bounded).
#[derive(Resource, Debug, Default)]
pub struct LanePaceLog {
    lines: VecDeque<String>,
}

impl LanePaceLog {
    const CAP: usize = 256;

    /// Every kept line, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &String> {
        self.lines.iter()
    }

    fn emit(&mut self, line: String) {
        println!("{line}");
        if self.lines.len() >= Self::CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// One host sample: the uplink and every seat's counters.
#[derive(Debug, Clone)]
pub struct HostSample {
    at: Instant,
    up_spent: u64,
    seats: BTreeMap<u16, (PaceStats, Option<ReliableStats>)>,
}

fn kbit(bytes: u64, secs: f64) -> u64 {
    (bytes as f64 * 8.0 / 1000.0 / secs).round() as u64
}

fn kbit_rate(bytes_per_sec: u32) -> u64 {
    u64::from(bytes_per_sec) * 8 / 1000
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The host lines for the window between `prev` and `cur`, one per seat
/// present in both.
pub fn host_lines(prev: &HostSample, cur: &HostSample, up_cap: u32) -> Vec<String> {
    let secs = cur.at.duration_since(prev.at).as_secs_f64();
    if secs <= 0.0 {
        return Vec::new();
    }
    let up_sent = kbit(cur.up_spent.saturating_sub(prev.up_spent), secs);
    let headroom = kbit_rate(up_cap).saturating_sub(up_sent);
    let mut out = Vec::new();
    for (&seat, (c, cr)) in &cur.seats {
        let Some((p, pr)) = prev.seats.get(&seat) else {
            continue;
        };
        let d = |a: u64, b: u64| a.saturating_sub(b);
        let (rtx, queued, in_flight) = match (cr, pr) {
            (Some(c), Some(p)) => (
                d(c.sender.retransmits, p.sender.retransmits),
                d(c.sender.queued, p.sender.queued),
                c.sender.in_flight,
            ),
            (Some(c), None) => (c.sender.retransmits, c.sender.queued, c.sender.in_flight),
            _ => (0, 0, 0),
        };
        let rtx_pct = if queued == 0 {
            0.0
        } else {
            rtx as f64 * 100.0 / queued as f64
        };
        out.push(format!(
            "event=lane_pace role=host seat={seat} win_ms={:.0} rate_kbit={} ceil_kbit={} \
             share_kbit={} sent_kbit={} up_cap_kbit={} up_sent_kbit={up_sent} \
             headroom_kbit={headroom} wait_p50_ms={:.1} wait_p95_ms={:.1} wait_max_ms={:.1} \
             age_ms={:.1} deferred={} denied={} expired={} dropped={} rtx={rtx} \
             rtx_pct={rtx_pct:.1} in_flight={in_flight} adapt={}",
            secs * 1000.0,
            kbit_rate(c.rate),
            kbit_rate(c.ceiling),
            kbit_rate(c.share),
            kbit(d(c.wire_sent, p.wire_sent), secs),
            kbit_rate(up_cap),
            ms(c.wait_p50),
            ms(c.wait_p95),
            ms(c.wait_max),
            ms(c.movement.oldest_age),
            d(c.movement_deferred, p.movement_deferred),
            d(c.uplink_denied, p.uplink_denied),
            d(
                c.movement.expired + c.bulk.expired,
                p.movement.expired + p.bulk.expired
            ),
            d(
                c.movement.dropped + c.bulk.dropped,
                p.movement.dropped + p.bulk.dropped
            ),
            c.adapt.snapshot_divisor,
        ));
    }
    out
}

/// The client line for the window between two lane counter reads.
pub fn client_line(seat: u16, prev: (Instant, LaneCounts), cur: (Instant, LaneCounts)) -> String {
    let secs = cur.0.duration_since(prev.0).as_secs_f64().max(f64::EPSILON);
    let d = |a: u64, b: u64| a.saturating_sub(b);
    let (p, c) = (prev.1, cur.1);
    let dps = |n: u64| (n as f64 / secs).round() as u64;
    format!(
        "event=lane_pace role=client seat={seat} win_ms={:.0} tx_dps={} rx_dps={} \
         accepted_dps={} dropped={}",
        secs * 1000.0,
        dps(d(c.sent, p.sent)),
        dps(d(c.received, p.received)),
        dps(d(c.accepted, p.accepted)),
        d(c.received, p.received).saturating_sub(d(c.accepted, p.accepted)),
    )
}

/// Host side: sample the uplink and every roster seat's lane.
pub fn report_host_lane_pace(
    host: Res<HostLink>,
    lobby: Res<LobbyState>,
    every: Res<LanePaceEvery>,
    mut log: ResMut<LanePaceLog>,
    mut last: Local<Option<HostSample>>,
) {
    let now = Instant::now();
    if last
        .as_ref()
        .is_some_and(|l| now.duration_since(l.at) < every.0)
    {
        return;
    }
    let (cap, up_spent) = host.uplink();
    let mut seats = BTreeMap::new();
    for entry in &lobby.roster {
        if let Some(p) = host.ctl().lane_pace(entry.player_id) {
            seats.insert(entry.player_id, (p, host.reliable_stats(entry.player_id)));
        }
    }
    let cur = HostSample {
        at: now,
        up_spent,
        seats,
    };
    if let Some(prev) = last.as_ref() {
        for line in host_lines(prev, &cur, cap) {
            log.emit(line);
        }
    }
    *last = Some(cur);
}

/// Client side: sample the joined lane's datagram counters.
pub fn report_client_lane_pace(
    link: Res<LobbyLink>,
    every: Res<LanePaceEvery>,
    mut log: ResMut<LanePaceLog>,
    mut last: Local<Option<(Instant, LaneCounts)>>,
) {
    let now = Instant::now();
    if last.is_some_and(|(at, _)| now.duration_since(at) < every.0) {
        return;
    }
    let Some(counts) = link.lane_counts() else {
        return;
    };
    if let Some(prev) = *last {
        log.emit(client_line(link.player_id(), prev, (now, counts)));
    }
    *last = Some((now, counts));
}

/// Register the host record (once per app).
pub fn add_host_systems(app: &mut App) {
    app.init_resource::<LanePaceEvery>()
        .init_resource::<LanePaceLog>()
        .add_systems(
            Update,
            report_host_lane_pace.run_if(resource_exists::<HostLink>),
        );
}

/// Register the client record (once per app).
pub fn add_client_systems(app: &mut App) {
    app.init_resource::<LanePaceEvery>()
        .init_resource::<LanePaceLog>()
        .add_systems(
            Update,
            report_client_lane_pace.run_if(resource_exists::<LobbyLink>),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_net::SenderStats;

    fn sample(at: Instant, spent: u64, wire: u64, rtx: u64, queued: u64) -> HostSample {
        let pace = PaceStats {
            wire_sent: wire,
            rate: 125_000,
            ceiling: 250_000,
            share: 62_500,
            movement_deferred: wire / 1000,
            wait_p95: Duration::from_millis(12),
            ..PaceStats::default()
        };
        let rel = ReliableStats {
            sender: SenderStats {
                retransmits: rtx,
                queued,
                ..SenderStats::default()
            },
            ..ReliableStats::default()
        };
        HostSample {
            at,
            up_spent: spent,
            seats: BTreeMap::from([(3, (pace, Some(rel)))]),
        }
    }

    #[test]
    fn host_line_reports_window_rates_and_headroom() {
        let t = Instant::now();
        let prev = sample(t, 0, 0, 0, 0);
        let cur = sample(t + Duration::from_secs(2), 100_000, 50_000, 5, 50);
        let lines = host_lines(&prev, &cur, 250_000);
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        // 100 kB over 2 s is 400 kbit/s of a 2000 kbit/s cap.
        assert!(l.contains("up_sent_kbit=400 "), "{l}");
        assert!(l.contains("headroom_kbit=1600 "), "{l}");
        assert!(l.contains("sent_kbit=200 "), "{l}");
        assert!(l.contains("rate_kbit=1000 "), "{l}");
        assert!(l.contains("rtx=5 rtx_pct=10.0 "), "{l}");
        assert!(l.contains("wait_p95_ms=12.0 "), "{l}");
    }

    #[test]
    fn a_zero_window_prints_nothing() {
        let t = Instant::now();
        assert!(host_lines(&sample(t, 0, 0, 0, 0), &sample(t, 1, 1, 1, 1), 1).is_empty());
    }
}
