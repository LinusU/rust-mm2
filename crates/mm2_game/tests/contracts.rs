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

/// F13-B standings: the ledger's finishing order ranks `Finished` by
/// the recorded race clock (never recording order), breaks same-tick
/// ties deterministically by participant, and ranks every `TimedOut`
/// below every finish. Unrecorded participants are unplaced.
#[test]
fn standings_order_by_finish_then_participant() {
    let mut s = playing_session();
    let (p0, p1, p2, p3) = (
        s.mint_player_id(),
        s.mint_player_id(),
        s.mint_player_id(),
        s.mint_player_id(),
    );
    let mut ledger = ResultLedger::default();
    let record = |ledger: &mut ResultLedger, s: &mut Session, p: PlayerId, outcome| {
        ledger
            .record(SessionResult {
                id: s.mint_result_id(p),
                tick: s.tick(),
                outcome,
            })
            .unwrap();
    };
    // Recorded deliberately out of finishing order — the standings must
    // come from the recorded ticks, not insertion order.
    record(
        &mut ledger,
        &mut s,
        p2,
        SessionOutcome::TimedOut { race_ticks: 200 },
    );
    record(
        &mut ledger,
        &mut s,
        p1,
        SessionOutcome::Finished { race_ticks: 140 },
    );
    record(
        &mut ledger,
        &mut s,
        p3,
        SessionOutcome::Finished { race_ticks: 90 },
    );
    record(
        &mut ledger,
        &mut s,
        p0,
        SessionOutcome::Finished { race_ticks: 140 },
    );

    let order: Vec<PlayerId> = ledger
        .standings()
        .iter()
        .map(|r| r.id.participant)
        .collect();
    assert_eq!(
        order,
        vec![p3, p0, p1, p2],
        "finished by race_ticks (same-tick tie by participant), timed-out last"
    );
    assert_eq!(ledger.place_of(p3), Some(1));
    assert_eq!(ledger.place_of(p0), Some(2));
    assert_eq!(ledger.place_of(p1), Some(3));
    assert_eq!(ledger.place_of(p2), Some(4));
    let unrecorded = s.mint_player_id();
    assert_eq!(
        ledger.place_of(unrecorded),
        None,
        "a participant with no result is unplaced"
    );
}

fn race_checkpoint(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 10.0,
        height: DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

fn race_def(
    rule: CheckpointRule,
    checkpoints: Vec<Checkpoint>,
    finish: Option<Checkpoint>,
    laps: u32,
) -> RaceDefinition {
    RaceDefinition {
        checkpoints,
        finish,
        rule,
        laps,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 0,
        start_slots: Vec::new(),
    }
}

fn racing(def: &RaceDefinition) -> RaceProgress {
    let mut p = RaceProgress::new(def);
    p.state = ParticipantState::Racing;
    p
}

/// F14-B/F13-B live order (DSN-13): `Finished` participants lock their
/// lead by the recorded clock, active ones rank by `(lap, gate)`
/// progress then distance to the next gate, `TimedOut` trails, and a
/// dead tie orders by `PlayerId` — independent of input order.
#[test]
fn live_order_ranks_ordered_participants() {
    let def = race_def(
        CheckpointRule::Ordered,
        vec![race_checkpoint(0.0, 0.0), race_checkpoint(100.0, 0.0)],
        None,
        2,
    );
    let mut s = playing_session();
    let (p0, p1, p2, p3) = (
        s.mint_player_id(),
        s.mint_player_id(),
        s.mint_player_id(),
        s.mint_player_id(),
    );

    // A later lap outranks sitting next to an earlier lap's next gate.
    let mut behind = racing(&def);
    behind.lap = 0;
    behind.next = 1;
    let mut ahead = racing(&def);
    ahead.lap = 1;
    ahead.next = 0;
    assert_eq!(
        live_order(
            &def,
            [
                (p0, &behind, Vec3::new(99.0, 0.0, 0.0)),
                (p1, &ahead, Vec3::new(-200.0, 0.0, 0.0)),
            ],
        ),
        vec![p1, p0],
    );

    // Equal progress: nearer the next gate leads; an exact tie orders
    // by `PlayerId` regardless of input order.
    let mut a = racing(&def);
    a.next = 1;
    let mut b = racing(&def);
    b.next = 1;
    assert_eq!(
        live_order(
            &def,
            [
                (p1, &b, Vec3::new(50.0, 0.0, 0.0)),
                (p0, &a, Vec3::new(90.0, 0.0, 0.0)),
            ],
        ),
        vec![p0, p1],
    );
    let (lo, hi) = if p0 < p1 { (p0, p1) } else { (p1, p0) };
    for input in [
        [
            (hi, &a, Vec3::new(50.0, 0.0, 0.0)),
            (lo, &b, Vec3::new(50.0, 0.0, 0.0)),
        ],
        [
            (lo, &b, Vec3::new(50.0, 0.0, 0.0)),
            (hi, &a, Vec3::new(50.0, 0.0, 0.0)),
        ],
    ] {
        assert_eq!(live_order(&def, input), vec![lo, hi]);
    }

    // Resolved participants bracket the live field: a finish locks the
    // lead on the recorded clock; a DNF trails anyone still racing.
    let mut fin = racing(&def);
    fin.state = ParticipantState::Finished {
        race_ticks: 100,
        result: s.mint_result_id(p2),
    };
    let mut out = racing(&def);
    out.state = ParticipantState::TimedOut {
        race_ticks: 200,
        result: s.mint_result_id(p3),
    };
    assert_eq!(
        live_order(
            &def,
            [
                (p3, &out, Vec3::ZERO),
                (p0, &behind, Vec3::ZERO),
                (p2, &fin, Vec3::ZERO),
            ],
        ),
        vec![p2, p0, p3],
    );

    // AwaitingStart is zero progress: a countdown grid orders by
    // proximity to the first gate.
    let near = RaceProgress::new(&def);
    let far = RaceProgress::new(&def);
    assert_eq!(
        live_order(
            &def,
            [
                (p1, &far, Vec3::new(-80.0, 0.0, 0.0)),
                (p0, &near, Vec3::new(-20.0, 0.0, 0.0)),
            ],
        ),
        vec![p0, p1],
    );
}

/// F14-B/F13-B live order under `AnyOrder` (DSN-13): cleared count,
/// then distance to the participant's *own* current objective — the
/// nearest remaining gate, or the armed finish once every gate is
/// cleared.
#[test]
fn live_order_any_order_ranks_by_objective() {
    // WPT-2 shape: any-order gates plus a separate finish trigger.
    let def = race_def(
        CheckpointRule::AnyOrder,
        vec![
            race_checkpoint(0.0, 0.0),
            race_checkpoint(100.0, 0.0),
            race_checkpoint(200.0, 0.0),
        ],
        Some(race_checkpoint(300.0, 0.0)),
        1,
    );
    let mut s = playing_session();
    let (p0, p1, p2) = (s.mint_player_id(), s.mint_player_id(), s.mint_player_id());

    // `cleared` is private, so progress comes through the same swept
    // `advance` the runtime feeds: approach off-axis, then dive in —
    // one segment clears only the named gate.
    let with_gates = |xs: &[f32]| {
        let mut p = racing(&def);
        for &x in xs {
            p.advance(&def, Vec3::new(x, 0.0, 30.0));
            p.advance(&def, Vec3::new(x, 0.0, 0.0));
        }
        p
    };
    let one = with_gates(&[0.0]);
    let two = with_gates(&[0.0, 100.0]);
    assert_eq!(one.cleared_count(), 1);
    assert_eq!(two.cleared_count(), 2);
    assert_eq!(
        live_order(
            &def,
            [
                (p0, &one, Vec3::new(50.0, 0.0, 0.0)),
                (p1, &two, Vec3::new(-500.0, 0.0, 0.0)),
            ],
        ),
        vec![p1, p0],
        "more cleared gates outranks any proximity"
    );

    // Equal counts: each participant is measured to its own nearest
    // remaining gate — p1 by gate 2 beats p0 by gate 1.
    assert_eq!(
        live_order(
            &def,
            [
                (p0, &one, Vec3::new(60.0, 0.0, 0.0)),
                (p1, &one, Vec3::new(195.0, 0.0, 0.0)),
            ],
        ),
        vec![p1, p0],
    );

    // Every gate cleared: the armed finish is the objective, and a
    // full-clearance participant outranks anyone still owing gates.
    let all = with_gates(&[0.0, 100.0, 200.0]);
    assert_eq!(all.cleared_count(), 3);
    assert_eq!(
        live_order(
            &def,
            [
                (p0, &all, Vec3::new(250.0, 0.0, 0.0)),
                (p1, &all, Vec3::new(295.0, 0.0, 0.0)),
                (p2, &two, Vec3::new(290.0, 0.0, 0.0)),
            ],
        ),
        vec![p1, p0, p2],
        "closer to the armed finish leads; owing gates trails"
    );
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
