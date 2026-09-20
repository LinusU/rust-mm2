//! F01-B shared-contract tests: stable object/player ids, the authority
//! boundary, impact dedup/bounds, surface state and result identity.

use bevy::prelude::*;
use mm2_game::*;

fn playing_session() -> Session {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap();
    s.transition(SessionPhase::Ready).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    s
}

fn unload_to_menu(s: &mut Session) {
    s.transition(SessionPhase::Unloading).unwrap();
    s.transition(SessionPhase::Menu).unwrap();
}

#[test]
fn object_ids_are_unique_within_a_generation() {
    let mut s = playing_session();
    let a = s.mint_object_id();
    let b = s.mint_object_id();
    assert_ne!(a, b, "slots must differ");
    assert_eq!(a.generation, s.generation());
    assert_eq!(b.generation, s.generation());
    // The world sentinel can never collide with a minted id.
    assert!(!a.is_world() && !b.is_world());
    assert!(ObjectId::WORLD.is_world());
}

#[test]
fn ids_from_an_older_generation_are_detectably_stale() {
    let mut s = playing_session();
    let stale = s.mint_object_id();
    unload_to_menu(&mut s);
    s.begin(SessionConfig::default()).unwrap();
    let fresh = s.mint_object_id();
    // Same slot, different generation — the generation is what makes the
    // stale id distinguishable rather than silently colliding.
    assert_eq!(stale.slot, fresh.slot);
    assert_ne!(stale, fresh);
    assert_ne!(stale.generation, s.generation());
}

#[test]
fn local_and_other_player_identities_coexist() {
    // AC05: participants are `Player` components queried by id/control —
    // no "the one local player" assumption.
    let mut s = playing_session();
    let local = s.mint_player_id();
    let remote = s.mint_player_id();
    assert_ne!(local, remote);

    let mut app = App::new();
    app.world_mut().spawn(Player {
        id: local,
        control: PlayerControl::Local,
    });
    app.world_mut().spawn(Player {
        id: remote,
        control: PlayerControl::Remote,
    });
    let mut q = app.world_mut().query::<&Player>();
    let players: Vec<Player> = q.iter(app.world()).copied().collect();
    assert_eq!(players.len(), 2);
    assert!(
        players
            .iter()
            .any(|p| p.id == local && p.control == PlayerControl::Local)
    );
    assert!(
        players
            .iter()
            .any(|p| p.id == remote && p.control == PlayerControl::Remote)
    );
}

#[test]
fn authority_role_follows_session_authority() {
    // Local and Host simulate their own rules; Remote predicts. This is
    // the boundary rule systems read — not the transport.
    for (authority, expected) in [
        (SessionAuthority::Local, AuthorityRole::Authority),
        (SessionAuthority::Host, AuthorityRole::Authority),
        (SessionAuthority::Remote, AuthorityRole::Predicted),
    ] {
        assert_eq!(
            authority.is_authoritative(),
            expected.is_authority(),
            "{authority:?}"
        );
        let mut s = Session::new();
        s.begin(SessionConfig {
            authority,
            ..SessionConfig::default()
        })
        .unwrap();
        assert_eq!(s.authority_role(), expected);
    }
    // No config (Menu) cannot pretend to predict for a server — default
    // is authoritative so rules are never silently bypassed.
    assert_eq!(Session::new().authority_role(), AuthorityRole::Authority);
}

#[test]
fn impact_dedup_collapses_pair_flapping() {
    let mut app = App::new();
    let a = app.world_mut().spawn_empty().id();
    let b = app.world_mut().spawn_empty().id();
    let c = app.world_mut().spawn_empty().id();

    let mut dedup = ImpactDedup::new(24);
    assert!(dedup.allow(a, b, 10), "first edge reports");
    // The same pair flapping start/stop inside the window does not
    // re-report — order within the pair does not matter.
    assert!(!dedup.allow(a, b, 11));
    assert!(!dedup.allow(b, a, 20));
    // A different pair is unaffected.
    assert!(dedup.allow(a, c, 11));
    // Once the cooldown elapses a genuinely new contact reports again.
    assert!(dedup.allow(a, b, 10 + 24));

    dedup.clear();
    assert!(dedup.allow(a, b, 10 + 24), "teardown forgets cooldowns");
}

#[test]
fn impact_policy_bounds_emission() {
    let policy = ImpactPolicy::default();
    assert!(policy.min_severity > 0.0);
    assert!(policy.pair_cooldown_ticks > 0);
    assert!(policy.max_per_tick > 0);
}

#[test]
fn result_ids_are_unique_and_ledger_deduplicates() {
    let mut s = playing_session();
    let p0 = s.mint_player_id();
    let p1 = s.mint_player_id();

    let r0 = s.mint_result_id(p0);
    let r1 = s.mint_result_id(p0);
    let r2 = s.mint_result_id(p1);
    // Sequence distinguishes same-participant results; participant
    // distinguishes same-sequence results. A cruise session carries no
    // event.
    assert_ne!(r0, r1);
    assert_ne!(r1, r2);
    assert!(r0.event.is_none());
    assert_eq!(r0.generation, s.generation());

    let mut ledger = ResultLedger::default();
    assert!(ledger.is_empty());
    ledger
        .record(SessionResult {
            id: r0.clone(),
            tick: s.tick(),
            outcome: SessionOutcome::Finished { race_ticks: 100 },
        })
        .unwrap();
    // The exact delivery repeated is a duplicate, not a new result.
    let dup = ledger
        .record(SessionResult {
            id: r0.clone(),
            tick: s.tick(),
            outcome: SessionOutcome::Finished { race_ticks: 100 },
        })
        .unwrap_err();
    assert_eq!(dup.0, r0);
    assert!(ledger.contains(&r0));
    ledger
        .record(SessionResult {
            id: r1,
            tick: s.tick(),
            outcome: SessionOutcome::Finished { race_ticks: 120 },
        })
        .unwrap();
    assert_eq!(ledger.len(), 2);
}

#[test]
fn result_ids_carry_the_event_and_survive_restart() {
    let mut s = Session::new();
    s.begin(SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "sf".into(),
            table: EventTableKind::Circuit,
            index: 2,
        }),
        ..SessionConfig::default()
    })
    .unwrap();
    s.transition(SessionPhase::Ready).unwrap();
    s.transition(SessionPhase::Playing).unwrap();
    let p = s.mint_player_id();
    let first = s.mint_result_id(p);
    assert_eq!(
        first.event.as_ref().map(|e| e.index),
        Some(2),
        "the event is part of the identity"
    );

    unload_to_menu(&mut s);
    s.begin(SessionConfig::default()).unwrap();
    let second = s.mint_result_id(p);
    // A result minted after restart can never collide with the earlier
    // one: generation and event both differ.
    assert_ne!(first, second);
    assert!(second.event.is_none());
}

#[test]
fn surface_state_defaults_to_unmodified() {
    let s = SurfaceState::default();
    assert_eq!(s.material, SurfaceMaterial::Unspecified);
    assert_eq!(s.traction, 1.0);
    let s = SurfaceState::of(SurfaceMaterial::Authored(7));
    assert_eq!(s.material, SurfaceMaterial::Authored(7));
    assert_eq!(s.traction, 1.0);
}
