//! The lobby screen: one pure view model that the joined and the hosted
//! lobby both render, so what a player sees (the roster with each seat's
//! car, paint and readiness, the offered session, what the keys do) is a
//! value tests can read rather than a format string inside a system.
//!
//! [`LobbyView`] holds only display facts the wire already carries —
//! the roster, the advertisement, our own seat. It decides nothing:
//! readiness and picks stay roster state the host validates, and the
//! start gate stays the lobby's own (`HostCommand::Start`). The view
//! only *reports* who the gate is waiting on, from the same roster the
//! gate reads.
//!
//! [`cycle_pick`] is the client's vehicle/paint chooser: a pure step
//! over the garage the catalog lists, so the pick a key sends is always
//! one the host's validator (`net::vehicle_validator`) accepts.

use mm2_content::VehicleCatalog;
use mm2_net::RosterEntry;

/// Which side of the lobby this screen belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyRole {
    /// We listen: `Enter` starts the session, `Esc` stops hosting.
    Host,
    /// We dialed in: `Enter` toggles ready, `Esc` leaves.
    Client,
}

/// One roster line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbySeat {
    pub name: String,
    /// `"vpbug (paint 2)"`, `"dev car"`, or `"—"` while unpicked.
    pub car: String,
    /// `None` for the host seat: a host starts the session rather than
    /// readying for it.
    pub ready: Option<bool>,
    pub is_host: bool,
    pub is_you: bool,
    /// A host-owned Cops & Robbers bot: it needs no readiness and its
    /// car is drawn when the session starts.
    pub is_bot: bool,
}

/// Everything the lobby screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyView {
    pub role: LobbyRole,
    /// Where the lobby listens / was dialed.
    pub address: String,
    /// The offered session's one-line summary (mode, city, difficulty).
    pub session: String,
    pub seats: Vec<LobbySeat>,
    /// The latest refusal or link-loss notice.
    pub notice: Option<String>,
}

/// The display form of a roster pick.
pub fn describe_pick(pick: Option<&mm2_net::VehiclePick>) -> String {
    match pick {
        None => "—".to_string(),
        Some(p) if p.vehicle.is_empty() => "dev car".to_string(),
        Some(p) => format!("{} (paint {})", p.vehicle, p.paint),
    }
}

impl LobbyView {
    /// The joined lobby: the host's seat (its name and car are not on
    /// the wire before `Start`, so the dialed address stands for it)
    /// followed by the broadcast roster, ours marked.
    pub fn client(
        peer: std::net::SocketAddr,
        our_id: u16,
        roster: &[RosterEntry],
        session: Option<&str>,
        notice: Option<&str>,
    ) -> Self {
        let mut seats = vec![LobbySeat {
            name: format!("host {peer}"),
            car: "—".to_string(),
            ready: None,
            is_host: true,
            is_you: false,
            is_bot: false,
        }];
        seats.extend(roster.iter().map(|e| LobbySeat {
            name: e.driver.clone(),
            car: describe_pick(e.pick.as_ref()),
            ready: Some(e.ready),
            is_host: false,
            is_you: e.player_id == our_id,
            is_bot: false,
        }));
        Self {
            role: LobbyRole::Client,
            address: peer.to_string(),
            session: session
                .unwrap_or("waiting for the host to advertise a session")
                .to_string(),
            seats,
            notice: notice.map(str::to_string),
        }
    }

    /// The hosted lobby: our own seat first (its pick is the session
    /// config's), then the remote players.
    pub fn host(
        addr: std::net::SocketAddr,
        driver: &str,
        host_pick: &mm2_net::VehiclePick,
        roster: &[RosterEntry],
        session: &str,
        notice: Option<&str>,
    ) -> Self {
        let mut seats = vec![LobbySeat {
            name: driver.to_string(),
            car: describe_pick(Some(host_pick)),
            ready: None,
            is_host: true,
            is_you: true,
            is_bot: false,
        }];
        seats.extend(roster.iter().map(|e| LobbySeat {
            name: e.driver.clone(),
            car: describe_pick(e.pick.as_ref()),
            ready: Some(e.ready),
            is_host: false,
            is_you: false,
            is_bot: false,
        }));
        Self {
            role: LobbyRole::Host,
            address: addr.to_string(),
            session: session.to_string(),
            seats,
            notice: notice.map(str::to_string),
        }
    }

    /// Add the host's Cops & Robbers bots to the roster: after every
    /// human, named as the match names them ("Bot 1"…). Their cars are
    /// drawn at session start, so the roster shows none yet.
    pub fn with_bots(mut self, bots: u8) -> Self {
        self.seats.extend((0..u16::from(bots)).map(|i| {
            LobbySeat {
                name: crate::cnr_bots::net_bot_name(mm2_game::PlayerId(
                    crate::cnr_bots::NET_BOT_PLAYER_BASE + i,
                ))
                .unwrap_or_else(|| format!("Bot {}", i + 1)),
                car: "chosen at start".to_string(),
                ready: None,
                is_host: false,
                is_you: false,
                is_bot: true,
            }
        }));
        self
    }

    /// Names the start is waiting on: remote seats not yet ready.
    pub fn waiting_on(&self) -> Vec<&str> {
        self.seats
            .iter()
            .filter(|s| s.ready == Some(false))
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Whether our own seat is ready (a client only).
    pub fn you_ready(&self) -> bool {
        self.seats.iter().any(|s| s.is_you && s.ready == Some(true))
    }

    /// The screen's text: a title, the offered session, one aligned row
    /// per seat, the gate's status and the key legend.
    pub fn render(&self) -> String {
        let title = match self.role {
            LobbyRole::Host => format!("HOSTING LOBBY — {}", self.address),
            LobbyRole::Client => format!("LOBBY — {}", self.address),
        };
        let name_w = self
            .seats
            .iter()
            .map(|s| s.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(6);
        let car_w = self
            .seats
            .iter()
            .map(|s| s.car.chars().count())
            .max()
            .unwrap_or(0)
            .max(3);
        let mut out = format!("{title}\nsession: {}\n\n", self.session);
        out.push_str(&format!(
            "   {:<name_w$}  {:<car_w$}  status\n",
            "driver", "car"
        ));
        for s in &self.seats {
            let status = match (s.is_host, s.ready) {
                (true, _) => "host",
                (false, _) if s.is_bot => "bot",
                (false, Some(true)) => "READY",
                (false, _) => "not ready",
            };
            out.push_str(&format!(
                " {} {:<name_w$}  {:<car_w$}  {status}\n",
                if s.is_you { ">" } else { " " },
                s.name,
                s.car,
            ));
        }
        let waiting = self.waiting_on();
        out.push('\n');
        match self.role {
            LobbyRole::Host if self.seats.iter().all(|s| s.is_host || s.is_bot) => {
                out.push_str("waiting for players to join\n");
            }
            LobbyRole::Host if waiting.is_empty() => out.push_str("everyone is ready\n"),
            LobbyRole::Host => out.push_str(&format!("waiting on: {}\n", waiting.join(", "))),
            LobbyRole::Client if self.you_ready() => {
                out.push_str("you are ready — waiting for the host to start\n");
            }
            LobbyRole::Client => out.push_str("press enter when you are ready\n"),
        }
        if let Some(n) = &self.notice {
            out.push_str(n);
            out.push('\n');
        }
        out.push_str(match self.role {
            LobbyRole::Host => "\nenter: start session    esc: stop hosting",
            LobbyRole::Client => {
                "\nenter: toggle ready    left/right: car    up/down: paint    esc: leave lobby"
            }
        });
        out
    }
}

/// The cars a client may pick: ready, listed catalog entries with their
/// paint counts — the same set the host's validator accepts.
pub fn garage(catalog: &VehicleCatalog) -> Vec<(String, u8)> {
    catalog
        .entries
        .iter()
        .filter(|e| e.is_ready() && e.canonical_info)
        .map(|e| {
            let paints = u8::try_from(e.paints.len().max(1)).unwrap_or(u8::MAX);
            (e.id.clone(), paints)
        })
        .collect()
}

/// Step the pick: `car` moves along the garage (wrapping, paint reset),
/// `paint` moves along the current car's paints (wrapping). A current
/// car outside the garage (the dev car) snaps to the first entry.
/// `None` when the garage is empty, so there is nothing to offer.
pub fn cycle_pick(
    garage: &[(String, u8)],
    current: (&str, u8),
    car: i32,
    paint: i32,
) -> Option<(String, u8)> {
    if garage.is_empty() {
        return None;
    }
    let len = garage.len() as i32;
    let Some(at) = garage.iter().position(|(id, _)| id == current.0) else {
        return Some((garage[0].0.clone(), 0));
    };
    if car != 0 {
        let to = (at as i32 + car).rem_euclid(len) as usize;
        return Some((garage[to].0.clone(), 0));
    }
    let paints = i32::from(garage[at].1.max(1));
    let to = (i32::from(current.1) + paint).rem_euclid(paints);
    Some((garage[at].0.clone(), to as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_net::VehiclePick;

    fn entry(id: u16, name: &str, ready: bool, car: &str, paint: u8) -> RosterEntry {
        RosterEntry {
            player_id: id,
            driver: name.to_string(),
            build: String::new(),
            ready,
            pick: (!car.is_empty() || paint > 0).then(|| VehiclePick {
                vehicle: car.to_string(),
                paint,
            }),
        }
    }

    fn peer() -> std::net::SocketAddr {
        "127.0.0.1:4000".parse().unwrap()
    }

    #[test]
    fn a_client_sees_every_seat_with_car_and_readiness() {
        let roster = [
            entry(1, "alice", true, "vpbug", 2),
            entry(2, "bob", false, "", 0),
        ];
        let view = LobbyView::client(peer(), 2, &roster, Some("sf, cruise, amateur"), None);
        let text = view.render();
        assert!(text.contains("sf, cruise, amateur"), "{text}");
        assert!(
            text.contains("alice") && text.contains("vpbug (paint 2)"),
            "{text}"
        );
        assert!(text.contains("READY"), "{text}");
        assert!(text.contains("press enter when you are ready"), "{text}");
        assert_eq!(view.seats.iter().filter(|s| s.is_you).count(), 1);
        assert_eq!(view.waiting_on(), vec!["bob"]);
        assert!(text.lines().any(|l| l.starts_with(" > bob")), "{text}");
    }

    #[test]
    fn the_hosts_bots_are_named_seats_the_start_never_waits_on() {
        let host_pick = VehiclePick {
            vehicle: "vpbug".into(),
            paint: 0,
        };
        let alone = LobbyView::host(peer(), "hal", &host_pick, &[], "sf, cops", None).with_bots(2);
        let text = alone.render();
        assert!(text.contains("Bot 1") && text.contains("Bot 2"), "{text}");
        assert!(text.contains("waiting for players to join"), "{text}");
        assert!(alone.waiting_on().is_empty());
        let roster = [entry(1, "alice", false, "vpbug", 0)];
        let view = LobbyView::client(peer(), 1, &roster, None, None).with_bots(1);
        assert_eq!(view.waiting_on(), vec!["alice"], "a bot is never waited on");
        assert_eq!(view.seats.last().map(|s| s.name.as_str()), Some("Bot 1"));
        assert_eq!(view.seats.iter().filter(|s| s.is_bot).count(), 1);
    }

    #[test]
    fn a_ready_client_is_told_to_wait_for_the_host() {
        let roster = [entry(1, "alice", true, "vpbug", 0)];
        let text = LobbyView::client(peer(), 1, &roster, None, Some("pick refused")).render();
        assert!(text.contains("waiting for the host to start"), "{text}");
        assert!(text.contains("pick refused"), "{text}");
        assert!(text.contains("waiting for the host to advertise"), "{text}");
    }

    #[test]
    fn the_host_sees_who_the_start_waits_on() {
        let host_pick = VehiclePick {
            vehicle: "vpbug".into(),
            paint: 0,
        };
        let alone = LobbyView::host(peer(), "hal", &host_pick, &[], "sf, cruise", None).render();
        assert!(alone.contains("waiting for players to join"), "{alone}");
        let roster = [
            entry(1, "alice", true, "vpbug", 0),
            entry(2, "bob", false, "", 0),
        ];
        let text = LobbyView::host(peer(), "hal", &host_pick, &roster, "sf, cruise", None).render();
        assert!(text.contains("waiting on: bob"), "{text}");
        assert!(text.contains("enter: start session"), "{text}");
        let all = [entry(1, "alice", true, "vpbug", 0)];
        let text = LobbyView::host(peer(), "hal", &host_pick, &all, "sf, cruise", None).render();
        assert!(text.contains("everyone is ready"), "{text}");
    }

    fn cars() -> Vec<(String, u8)> {
        vec![("vpbug".into(), 3), ("vpmustang".into(), 1)]
    }

    #[test]
    fn the_pick_cycles_cars_and_paints_within_the_garage() {
        let g = cars();
        assert_eq!(
            cycle_pick(&g, ("vpbug", 2), 1, 0),
            Some(("vpmustang".into(), 0))
        );
        assert_eq!(
            cycle_pick(&g, ("vpbug", 0), -1, 0),
            Some(("vpmustang".into(), 0))
        );
        assert_eq!(
            cycle_pick(&g, ("vpbug", 2), 0, 1),
            Some(("vpbug".into(), 0))
        );
        assert_eq!(
            cycle_pick(&g, ("vpbug", 0), 0, -1),
            Some(("vpbug".into(), 2))
        );
        assert_eq!(
            cycle_pick(&g, ("vpmustang", 0), 0, 1),
            Some(("vpmustang".into(), 0))
        );
        // The dev car is not in the garage: any step lands on its first car.
        assert_eq!(cycle_pick(&g, ("", 0), 1, 0), Some(("vpbug".into(), 0)));
        assert_eq!(cycle_pick(&[], ("", 0), 1, 0), None);
    }
}
