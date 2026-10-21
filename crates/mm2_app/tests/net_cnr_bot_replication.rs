//! CNR-MP-BOTS-C: a host's Cops & Robbers bots reach the clients as
//! ordinary non-owned cars through the authoritative snapshot stream.
//!
//! Evidence level: synthetic integration (real loopback wire and the
//! production reconcile/publish/apply systems over a fixture install;
//! no city, no retail data, no rendering).

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use mm2_app::cnr_bots::{BotSlot, CnrBot, CnrBotRoster, NET_BOT_PLAYER_BASE, PlannedBot};
use mm2_app::net::{LobbyLink, LobbyState};
use mm2_app::netdrive::{self, NetPlayer, RemotePick};
use mm2_game::{
    DevOverrides, Mm2Vfs, Player, PlayerId, Session, SessionAuthority, SessionEntity, SessionPhase,
};
use mm2_net::{Host, HostConfig, LateJoin, Message, SnapEntry, VehiclePick, hello};
use mm2_vehicle::VehicleInput;

use crate::net_app::{
    bridge_app, dev_cruise, host_app, host_link, hosted_playing, ready_peer, spin, spin_mut,
    until_begun, until_ready, until_started, until_wire,
};
use crate::support;

const SLOT: BotSlot = BotSlot {
    position: Vec3::new(40.0, 0.0, -60.0),
    yaw: 0.5,
};

fn plan(vfs: &mm2_assets::Vfs, n: usize) -> Vec<PlannedBot> {
    (0..n)
        .map(|i| PlannedBot {
            def: mm2_content::load_opponent(vfs, "vpa", 0).expect("fixture car loads"),
            slot: BotSlot {
                position: SLOT.position + Vec3::X * (30.0 * i as f32),
                yaw: SLOT.yaw,
            },
        })
        .collect()
}

#[test]
fn a_host_publishes_each_bot_once_beside_the_wire_seats() {
    let install = tempfile::tempdir().unwrap();
    support::tuned_car(install.path(), "vpa", 1200.0);
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let planned = std::sync::Arc::new(std::sync::Mutex::new(Some(plan(&vfs, 2))));
    let handoff = planned.clone();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    hosted_playing(&mut app);
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<Mm2Vfs>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mut session: ResMut<Session>| {
                let owner = SessionEntity(session.generation());
                // The production spawn: a hosted session stamps wire ids.
                mm2_app::cnr_bots::spawn_bots(
                    &mut commands,
                    &vfs.0,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    handoff.lock().unwrap().take().expect("runs once"),
                    owner,
                    &mut session,
                );
            },
        )
        .unwrap();
    // Snapshots publish on app updates: run a few after the spawn so the
    // peer has frames that carry the bots.
    for _ in 0..30 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // A bot is a seat on the stream, never a second car for the human.
    let snap = until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { entries, .. }
            if entries.iter().any(|e| e.player == NET_BOT_PLAYER_BASE + 1))
    });
    let Message::Snap { entries, .. } = snap else {
        unreachable!()
    };
    for id in [NET_BOT_PLAYER_BASE, NET_BOT_PLAYER_BASE + 1] {
        assert_eq!(
            entries.iter().filter(|e| e.player == id).count(),
            1,
            "bot {id} is published exactly once"
        );
    }
    let ids: Vec<u16> = entries.iter().map(|e| e.player).collect();
    let mut sorted = ids.clone();
    sorted.dedup();
    assert_eq!(sorted, ids, "no id appears twice on the wire");
    let mut q = app
        .world_mut()
        .query_filtered::<(&Player, &NetPlayer, &CnrBot), ()>();
    let bots: Vec<(PlayerId, u16, u8)> = q
        .iter(app.world())
        .map(|(p, n, b)| (p.id, n.0, b.index))
        .collect();
    assert_eq!(bots.len(), 2);
    for (id, wire, index) in bots {
        assert_eq!(id.0, wire, "the match id is the wire id");
        assert_eq!(wire, NET_BOT_PLAYER_BASE + u16::from(index));
    }
    // The bots take no wire input and are no pick-driven seat.
    let picked = app
        .world_mut()
        .query_filtered::<&NetPlayer, (With<RemotePick>, With<CnrBot>)>()
        .iter(app.world())
        .count();
    assert_eq!(picked, 0, "a bot is not a remote pick");
}

#[test]
fn a_client_holds_one_posed_copy_per_host_bot_and_never_a_second() {
    let install = tempfile::tempdir().unwrap();
    support::tuned_car(install.path(), "vpa", 1200.0);
    let vfs = support::mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(mm2_app::net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("bots-c".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let planned = plan(&vfs, 2);
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        assert_eq!(
            session.config().map(|c| c.authority),
            Some(SessionAuthority::Remote)
        );
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.insert_resource(CnrBotRoster(planned));

    let copies = |a: &mut App| {
        a.world_mut()
            .query_filtered::<(&NetPlayer, &Player, &CnrBot), With<RemotePick>>()
            .iter(a.world())
            .map(|(n, p, b)| (n.0, p.id.0, b.index))
            .collect::<Vec<_>>()
    };
    spin_mut(&mut app, |a| copies(a).len() == 2);
    // Many more updates must not mint a second copy of anyone.
    for _ in 0..20 {
        app.update();
    }
    let mut found = copies(&mut app);
    found.sort_unstable();
    assert_eq!(
        found,
        vec![
            (NET_BOT_PLAYER_BASE, NET_BOT_PLAYER_BASE, 0),
            (NET_BOT_PLAYER_BASE + 1, NET_BOT_PLAYER_BASE + 1, 1),
        ]
    );
    let seats: Vec<u16> = app
        .world_mut()
        .query_filtered::<&NetPlayer, With<RemotePick>>()
        .iter(app.world())
        .map(|n| n.0)
        .collect();
    assert_eq!(seats.len(), 3, "the host's seat 0 and the two bots");
    assert!(
        app.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .all(|e| e.player_id < NET_BOT_PLAYER_BASE),
        "a bot is never a roster peer"
    );

    // The host's snapshot poses a copy and gives it its drive state.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 9,
            entries: vec![SnapEntry {
                player: NET_BOT_PLAYER_BASE + 1,
                pos: [77.0, 2.0, -31.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                steer: 250,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    app.update();
    let mut q = app
        .world_mut()
        .query_filtered::<(&NetPlayer, &netdrive::RemoteLerp, &VehicleInput), With<CnrBot>>();
    let posed: Vec<_> = q
        .iter(app.world())
        .filter(|(n, ..)| n.0 == NET_BOT_PLAYER_BASE + 1)
        .collect();
    assert_eq!(posed.len(), 1);
    assert!(
        (posed[0].1.to_pos - Vec3::new(77.0, 2.0, -31.0)).length() < 1e-3,
        "bot 2's copy follows the host: {:?}",
        posed[0].1.to_pos
    );
    // The other bot kept its planned start: no entry, no move.
    let untouched = app
        .world_mut()
        .query_filtered::<(&NetPlayer, &netdrive::RemoteLerp), With<CnrBot>>()
        .iter(app.world())
        .find(|(n, _)| n.0 == NET_BOT_PLAYER_BASE)
        .map(|(_, l)| l.to_pos)
        .unwrap();
    assert!((untouched - SLOT.position).xz().length() < 1.0);
}
