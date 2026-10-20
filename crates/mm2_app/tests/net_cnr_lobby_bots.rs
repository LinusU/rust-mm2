//! CNR-MP-BOTS-A: the host's Cops & Robbers lobby bot count rides the
//! lobby's session advertisement. Joiners read it back, it survives a
//! dropped datagram (the host repeats the advertisement), and a client
//! can neither set nor change it.

use std::thread;
use std::time::{Duration, Instant};

use mm2_app::net;
use mm2_content::cnr::CnrSettings;
use mm2_game::{
    ConfigError, MAX_CNR_LOBBY_BOTS, SessionAuthority, SessionConfig, SessionMode, WorldMode,
};
use mm2_net::{
    Client, Host, HostConfig, HostEvent, Impair, ImpairProxy, LeaveCause, LinkDir, Message, hello,
};

const FP: u64 = 0x1801;
const WAIT: Duration = Duration::from_secs(10);

fn cnr_config(bots: u8) -> SessionConfig {
    SessionConfig {
        world: WorldMode::City {
            psdl: "city/sf.psdl".to_string(),
        },
        mode: SessionMode::CopsAndRobbers(CnrSettings::default()),
        authority: SessionAuthority::Host,
        cnr_bots: bots,
        ..SessionConfig::default()
    }
}

fn join(addr: std::net::SocketAddr) -> Client {
    let client = Client::join(addr, &hello("bots-a".to_string(), "joiner".to_string(), FP))
        .expect("join failed");
    client.set_timeout(Some(WAIT)).unwrap();
    client
}

/// The next advertised session the client hears, decoded.
fn next_ad(client: &mut Client) -> SessionConfig {
    let deadline = Instant::now() + WAIT;
    loop {
        match client.recv().expect("lobby closed") {
            Message::Session(ad) => return net::accept(&ad).expect("ad decodes"),
            _ => assert!(Instant::now() < deadline, "no session advertisement"),
        }
    }
}

#[test]
fn the_lobby_bot_count_round_trips_through_the_advertisement() {
    for bots in [0, 1, MAX_CNR_LOBBY_BOTS] {
        let ad = net::advertise(&cnr_config(bots)).unwrap();
        let back = net::accept(&ad).unwrap();
        assert_eq!(back.cnr_bots, bots);
        assert_eq!(back.authority, SessionAuthority::Remote);
        assert_eq!(
            ad.summary.contains("lobby bot"),
            bots > 0,
            "joiners see the roster in the summary: {}",
            ad.summary
        );
    }
    let ad = net::advertise(&cnr_config(3)).unwrap();
    assert!(ad.summary.contains("3 lobby bots"), "{}", ad.summary);
}

#[test]
fn a_full_lobby_or_another_mode_refuses_the_bot_count() {
    let over = cnr_config(MAX_CNR_LOBBY_BOTS + 1);
    assert!(matches!(
        net::advertise(&over),
        Err(net::SessionWireError::Invalid(
            ConfigError::CnrLobbyBotsFull(_)
        ))
    ));
    let msg = ConfigError::CnrLobbyBotsFull(9).to_string();
    assert!(msg.contains("at most 8"), "{msg}");

    // Never silently applied to another network game.
    let cruise = SessionConfig {
        mode: SessionMode::Cruise,
        cnr_bots: 2,
        ..cnr_config(0)
    };
    assert!(matches!(
        net::advertise(&cruise),
        Err(net::SessionWireError::Invalid(ConfigError::CnrBotsNeedCnr))
    ));

    // A forged blob is bounded on the receiving side too.
    let mut forged = net::advertise(&cnr_config(3)).unwrap();
    let text = String::from_utf8(forged.params.clone()).unwrap();
    forged.params = text
        .replace("\"cnr_bots\":3", "\"cnr_bots\":9")
        .into_bytes();
    assert!(net::accept(&forged).is_err());
}

#[test]
fn an_advertisement_without_the_field_means_no_bots() {
    let ad = net::advertise(&cnr_config(0)).unwrap();
    let text = String::from_utf8(ad.params.clone()).unwrap();
    let stripped = text.replace(",\"cnr_bots\":0", "");
    assert_ne!(text, stripped, "the field is on the wire");
    let old = mm2_net::SessionAdvertisement {
        summary: ad.summary,
        params: stripped.into_bytes(),
    };
    assert_eq!(net::accept(&old).unwrap().cnr_bots, 0);
}

#[test]
fn a_joiner_reads_the_hosts_bot_count_before_start() {
    let mut host = Host::listen_loopback(&HostConfig::new(FP)).unwrap();
    host.set_session(net::advertise(&cnr_config(4)).unwrap())
        .unwrap();
    let mut client = join(host.addr());
    assert_eq!(next_ad(&mut client).cnr_bots, 4);

    // The host's edit reaches the joiner too.
    host.set_session(net::advertise(&cnr_config(6)).unwrap())
        .unwrap();
    loop {
        if next_ad(&mut client).cnr_bots == 6 {
            break;
        }
    }
    host.shutdown();
}

#[test]
fn a_dropped_advertisement_of_the_bot_count_is_repeated_until_it_lands() {
    let mut host = Host::listen_loopback(&HostConfig::new(FP)).unwrap();
    host.set_session(net::advertise(&cnr_config(2)).unwrap())
        .unwrap();
    let proxy = ImpairProxy::loopback_seeded(host.addr(), 0x1801).unwrap();
    let mut client = join(proxy.addr());
    assert_eq!(next_ad(&mut client).cnr_bots, 2);

    // The host's edit is written into a dead lane, so the one-shot
    // frame is gone; the lane recovers and the repeat must deliver it.
    proxy.set(
        LinkDir::Down,
        Impair {
            loss: 1.0,
            ..Impair::default()
        },
    );
    let before = proxy.stats(LinkDir::Down).dropped;
    host.set_session(net::advertise(&cnr_config(7)).unwrap())
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while proxy.stats(LinkDir::Down).dropped <= before {
        assert!(Instant::now() < deadline, "the edit was never dropped");
        thread::sleep(Duration::from_millis(2));
    }
    proxy.set(LinkDir::Down, Impair::default());
    let deadline = Instant::now() + WAIT;
    loop {
        // Keepalive-borne repeats keep arriving until the new count does.
        if next_ad(&mut client).cnr_bots == 7 {
            break;
        }
        assert!(Instant::now() < deadline, "the repeat never landed");
    }
    host.shutdown();
}

#[test]
fn a_client_cannot_set_the_lobby_bot_count() {
    let mut host = Host::listen_loopback(&HostConfig::new(FP)).unwrap();
    host.set_session(net::advertise(&cnr_config(3)).unwrap())
        .unwrap();
    let mut bad = join(host.addr());
    assert_eq!(next_ad(&mut bad).cnr_bots, 3);

    // `Session` is host→client: a client sending one is dropped.
    bad.send(&Message::Session(
        net::advertise(&cnr_config(MAX_CNR_LOBBY_BOTS)).unwrap(),
    ))
    .unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        match host.recv_timeout(Duration::from_millis(200)) {
            Ok(HostEvent::Left { cause, .. }) => {
                assert_eq!(cause, LeaveCause::Malformed);
                break;
            }
            Ok(_) => {}
            Err(_) => assert!(Instant::now() < deadline, "the forged ad was not refused"),
        }
    }

    // The advertised count is untouched for the next joiner.
    let mut honest = join(host.addr());
    assert_eq!(next_ad(&mut honest).cnr_bots, 3);
    host.shutdown();
}
