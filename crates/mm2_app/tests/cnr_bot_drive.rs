//! CNRBOT-C synthetic integration: local Cops & Robbers bots drive the
//! production vehicle through real physics on flat ground, and the real
//! gold step (`cnr_host_step`) decides every pickup, steal and delivery.
//!
//! Evidence level: synthetic integration (real `VehicleInput`, physics,
//! impact pipeline and neural policy; fabricated flat world, no street
//! graph so lines are the documented straight fallback). The retail
//! legs live in `cnr_bot_drive_retail`.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::cnr::{
    CnrEvent, CnrHost, cnr_host_step, enroll_cnr_participants, reconcile_gold_load,
};
use mm2_app::cnr_bot_drive::{CnrBotBrain, CnrBotDriver, REPLAN_FRAMES, cnr_bot_drive};
use mm2_app::cnr_bots::{CnrBot, dev_drop_bot_at};
use mm2_app::contracts;
use mm2_content::cnr::{CnrSettings, GoldMass};
use mm2_game::gold::{CnrVariant, DropCause, EndRule, GoldEvent, GoldMatch, Side};
use mm2_game::{
    ImpactEvent, ObjectIdentity, Player, PlayerControl, PlayerId, Session, SessionAuthority,
    SessionConfig, SessionMode, SessionPhase, WorldMode,
};
use mm2_vehicle::{VehicleConfig, VehicleInput, VehiclePlugin, vehicle_bundle};

const HZ: u32 = 60;

#[derive(Resource, Default)]
struct Events(Vec<GoldEvent>);

fn collect(mut reader: MessageReader<CnrEvent>, mut out: ResMut<Events>) {
    out.0.extend(reader.read().map(|e| e.0));
}

/// Flat ground, a local Cops & Robbers session already `Playing`, the
/// gold step chained after the impact pipeline as in the app, and the
/// bot driver in `FixedUpdate`.
struct Rig {
    app: App,
    gold: Vec3,
    hideout: Vec3,
    /// Furthest a watched car may move in one step, m.
    jump_limit: f32,
}

fn pool() -> Vec<Vec3> {
    vec![
        Vec3::new(260.0, 0.0, 0.0),
        Vec3::new(-260.0, 0.0, 40.0),
        Vec3::new(0.0, 0.0, 260.0),
        Vec3::new(0.0, 0.0, -260.0),
        Vec3::new(180.0, 0.0, 180.0),
    ]
}

impl Rig {
    fn new(variant: CnrVariant, with_brain: bool) -> Self {
        Self::build(variant, with_brain, EndRule::None, |_| {})
    }

    /// [`Rig::new`] with the match's end rule and the session's dev
    /// overrides chosen.
    fn build(
        variant: CnrVariant,
        with_brain: bool,
        end: EndRule,
        dev: impl FnOnce(&mut mm2_game::DevOverrides),
    ) -> Self {
        let mut config = SessionConfig {
            world: WorldMode::City {
                psdl: "city/testcity.psdl".to_string(),
            },
            mode: SessionMode::CopsAndRobbers(CnrSettings {
                variant,
                ..CnrSettings::default()
            }),
            authority: SessionAuthority::Local,
            seed: 5,
            ..SessionConfig::default()
        };
        dev(&mut config.dev);
        let mut session = Session::new();
        session.begin(config).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let gold_id = session.mint_object_id();
        let mut rules = CnrSettings {
            variant,
            gold_mass: GoldMass::QuarterTon,
            ..CnrSettings::default()
        }
        .rules(HZ);
        rules.end = end;
        let game = GoldMatch::new(session.generation(), gold_id, rules, pool(), 9, &[]).unwrap();
        let sites = game.sites();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(bevy::mesh::MeshPlugin)
            .add_plugins(bevy::gizmos::GizmoPlugin)
            .add_plugins(PhysicsPlugins::default())
            .add_plugins(TransformPlugin)
            .add_plugins(VehiclePlugin)
            .insert_resource(Time::<Fixed>::from_hz(f64::from(HZ)))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / f64::from(HZ),
            )))
            .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
            .insert_resource(session)
            .insert_resource(CnrHost::new(game))
            .add_message::<ImpactEvent>()
            .add_message::<CnrEvent>()
            .add_message::<mm2_game::RecoveryEvent>()
            .init_resource::<Events>()
            .init_resource::<contracts::ImpactFilter>()
            .add_systems(
                FixedUpdate,
                (
                    mm2_game::advance_session_tick,
                    cnr_bot_drive,
                    dev_drop_bot_at,
                ),
            )
            .add_systems(
                FixedLast,
                (
                    contracts::collect_impacts,
                    enroll_cnr_participants,
                    cnr_host_step,
                    reconcile_gold_load,
                    collect,
                )
                    .chain(),
            );
        if with_brain {
            app.insert_resource(CnrBotBrain::new(None));
        }
        app.finish();
        app.cleanup();
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(3000.0, 1.0, 3000.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        Self {
            app,
            gold: sites.gold,
            hideout: sites.hideout,
            jump_limit: 4.0,
        }
    }

    /// A bot on `side` (as enrolment would seat it, were the split not
    /// chosen), on the ground at `at`, facing `toward`.
    fn bot_on(&mut self, index: u8, at: Vec3, toward: Vec3, side: Side) -> (Entity, PlayerId) {
        let (entity, id) = self.bot(index, at, toward);
        self.seat(id, side);
        (entity, id)
    }

    /// A bot on the ground at `at`, facing `toward`.
    fn bot(&mut self, index: u8, at: Vec3, toward: Vec3) -> (Entity, PlayerId) {
        let config = VehicleConfig::default();
        let flat = toward - at;
        let yaw = (-flat.x).atan2(-flat.z);
        let (object, id, role) = {
            let mut session = self.app.world_mut().resource_mut::<Session>();
            (
                session.mint_object_id(),
                session.mint_player_id(),
                session.authority_role(),
            )
        };
        let entity = self
            .app
            .world_mut()
            .spawn((
                CnrBot { index },
                CnrBotDriver::default(),
                ObjectIdentity(object),
                Player {
                    id,
                    control: PlayerControl::Ai,
                },
                role,
                vehicle_bundle(&config),
                Transform::from_translation(Vec3::new(at.x, 1.2, at.z))
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ))
            .id();
        (entity, id)
    }

    /// A human-driven car on the ground at `at`, facing `toward`, seated
    /// on `side`. Its pedals are whatever the test writes to its
    /// `VehicleInput`; nothing else moves it.
    fn human(&mut self, at: Vec3, toward: Vec3, side: Side) -> (Entity, PlayerId) {
        let config = VehicleConfig::default();
        let flat = toward - at;
        let yaw = (-flat.x).atan2(-flat.z);
        let (object, id, role) = {
            let mut session = self.app.world_mut().resource_mut::<Session>();
            (
                session.mint_object_id(),
                session.mint_player_id(),
                session.authority_role(),
            )
        };
        self.seat(id, side);
        let entity = self
            .app
            .world_mut()
            .spawn((
                ObjectIdentity(object),
                Player {
                    id,
                    control: PlayerControl::Local,
                },
                role,
                vehicle_bundle(&config),
                Transform::from_translation(Vec3::new(at.x, 1.2, at.z))
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ))
            .id();
        (entity, id)
    }

    /// Seat a participant on `side` through the match's own join, as
    /// the lobby would, so the split is the test's and not the
    /// enrolment order's.
    fn seat(&mut self, id: PlayerId, side: Side) {
        self.app
            .world_mut()
            .resource_mut::<CnrHost>()
            .game
            .join(id, side)
            .unwrap();
    }

    /// Drive `e` as a person would: full throttle, steering toward
    /// `target` by the angle between its heading and the bearing.
    fn steer_toward(&mut self, e: Entity, target: Vec3) {
        let world = self.app.world();
        let (p, q) = (
            world.get::<Position>(e).unwrap().0,
            world.get::<Rotation>(e).unwrap().0,
        );
        let fwd = q * Vec3::NEG_Z;
        let want = target - p;
        let cross = fwd.x * want.z - fwd.z * want.x;
        let dot = fwd.x * want.x + fwd.z * want.z;
        // Positive cross: the target is to the car's clockwise side
        // seen from above (screen right, `-Z` forward).
        let angle = cross.atan2(dot);
        let mut input = self.app.world_mut().get_mut::<VehicleInput>(e).unwrap();
        *input = VehicleInput {
            throttle: 1.0,
            steering: (angle * 2.0).clamp(-1.0, 1.0),
            ..default()
        };
    }

    /// Step with `drive` called before each step (a human's hands),
    /// like [`Rig::run_until`] watching for teleports.
    fn run_driving(
        &mut self,
        ticks: u32,
        watch: &[Entity],
        mut drive: impl FnMut(&mut Rig),
        mut done: impl FnMut(&Rig) -> bool,
    ) -> u32 {
        let mut last: Vec<Vec3> = watch.iter().map(|e| self.pos(*e)).collect();
        for t in 0..ticks {
            drive(self);
            self.app.update();
            for (i, e) in watch.iter().enumerate() {
                let p = self.pos(*e);
                assert!(
                    p.is_finite() && p.distance(last[i]) < self.jump_limit,
                    "car {i} jumped {:?} -> {p:?} at tick {t}",
                    last[i]
                );
                last[i] = p;
            }
            if done(self) {
                return t;
            }
        }
        ticks
    }

    fn pos(&self, e: Entity) -> Vec3 {
        self.app.world().get::<Position>(e).unwrap().0
    }

    fn game(&self) -> &GoldMatch {
        &self.app.world().resource::<CnrHost>().game
    }

    fn events(&self) -> &[GoldEvent] {
        &self.app.world().resource::<Events>().0
    }

    /// Step until `done` or `ticks` pass, asserting no step moves any
    /// of `watch` further than a car could drive — a teleport shows.
    fn run_until(
        &mut self,
        ticks: u32,
        watch: &[Entity],
        mut done: impl FnMut(&Rig) -> bool,
    ) -> u32 {
        let mut last: Vec<Vec3> = watch.iter().map(|e| self.pos(*e)).collect();
        for t in 0..ticks {
            self.app.update();
            for (i, e) in watch.iter().enumerate() {
                let p = self.pos(*e);
                assert!(
                    p.is_finite() && p.distance(last[i]) < self.jump_limit,
                    "bot {i} jumped {:?} -> {p:?} at tick {t}",
                    last[i]
                );
                last[i] = p;
            }
            if done(self) {
                return t;
            }
        }
        ticks
    }
}

fn driver(rig: &Rig, e: Entity) -> &CnrBotDriver {
    rig.app.world().get::<CnrBotDriver>(e).unwrap()
}

#[test]
fn a_bot_picks_up_loose_gold_and_delivers_it_by_driving() {
    let mut rig = Rig::new(CnrVariant::FreeForAll, true);
    let start = rig.gold - Vec3::new(-110.0, 0.0, 0.0).normalize() * 110.0 + Vec3::Y * 0.0;
    let (bot, id) = rig.bot(0, start, rig.gold);
    let before = rig.pos(bot);
    // Pickup: only by entering the trigger with the hull.
    let t1 = rig.run_until(60 * 60, &[bot], |r| r.game().carrier() == Some(id));
    assert_eq!(rig.game().carrier(), Some(id), "picked up after {t1} ticks");
    assert!(rig.pos(bot).distance(before) > 50.0, "it drove there");
    // Delivery: scored by the same host step, in a later round.
    let t2 = rig.run_until(60 * 90, &[bot], |r| r.game().round() >= 1);
    assert!(rig.game().round() >= 1, "delivered after {t2} ticks");
    assert!(rig.game().score(id).unwrap() > 0);
    let kinds: Vec<_> = rig
        .events()
        .iter()
        .filter(|e| matches!(e, GoldEvent::Picked { .. } | GoldEvent::Delivered { .. }))
        .collect();
    assert!(kinds.len() >= 2, "{kinds:?}");
    let d = driver(&rig, bot);
    assert_eq!(d.reanchors, 0, "no teleport helped it");
    // Bounded replanning: a first line, one per goal switch, and at most
    // one more per `REPLAN_FRAMES` of driving — never one per frame.
    let ticks = t1 + t2;
    let bound = 1 + 2 * 2 + ticks / REPLAN_FRAMES;
    assert!(
        d.plans <= bound,
        "plans {} > {bound} in {ticks} ticks",
        d.plans
    );
    let stats = rig.app.world().resource::<CnrBotBrain>().stats;
    assert_eq!(stats.streets, 0, "no street graph in this world");
    assert_eq!(u64::from(d.plans), stats.straight + stats.direct);
}

/// The point on the way from `a` to `b`, `f` of the distance along,
/// shifted `side` metres to the left of the way.
fn along(a: Vec3, b: Vec3, f: f32, side: f32) -> Vec3 {
    let way = (b - a).normalize_or_zero();
    a.lerp(b, f) + Vec3::new(-way.z, 0.0, way.x) * side
}

#[test]
fn a_bot_chases_a_moving_carrier_and_steals_the_gold_by_ramming() {
    let mut rig = Rig::new(CnrVariant::FreeForAll, true);
    let (gold, hideout) = (rig.gold, rig.hideout);
    // The carrier-to-be sits on the gold; the chaser lies in wait off
    // the carrier's way to the hideout.
    let (a, a_id) = rig.bot(0, gold, hideout);
    let (b, b_id) = rig.bot(1, along(gold, hideout, 0.8, 60.0), gold);
    // A picks up where it stands: the host step sees it in reach.
    rig.run_until(30, &[a, b], |r| r.game().carrier() == Some(a_id));
    assert_eq!(
        rig.game().carrier(),
        Some(a_id),
        "the first bot took the gold"
    );
    let before = rig.events().len();

    let t = rig.run_until(60 * 60, &[a, b], |r| r.game().carrier() == Some(b_id));
    let events = &rig.events()[before..];
    assert_eq!(
        rig.game().carrier(),
        Some(b_id),
        "stolen after {t} ticks: {events:?}"
    );
    // The steal is the host's ramming rule: a knocked drop by B, then
    // B's own pickup.
    assert!(
        events.iter().any(|e| matches!(
            e,
            GoldEvent::Dropped { player, cause: mm2_game::gold::DropCause::Knocked { by: Some(by) }, .. }
                if *player == a_id && *by == b_id
        )),
        "{events:?}"
    );
    // Neither bot was teleported, and the chase did not re-plan per frame.
    assert_eq!(driver(&rig, a).reanchors + driver(&rig, b).reanchors, 0);
    let bound = 3 + t / REPLAN_FRAMES;
    assert!(
        driver(&rig, b).plans <= bound,
        "{} > {bound}",
        driver(&rig, b).plans
    );
}

#[test]
fn a_bot_walled_off_is_re_anchored_without_scoring() {
    let mut rig = Rig::new(CnrVariant::FreeForAll, true);
    let gold = rig.gold;
    // A wall across the way, between the bot and the gold.
    rig.app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(2.0, 8.0, 2000.0),
        Transform::from_xyz(gold.x + 50.0, 4.0, gold.z),
    ));
    let (bot, id) = rig.bot(0, gold + Vec3::new(110.0, 0.0, 0.0), gold);
    // The re-anchor is a disclosed teleport; everything else is not.
    rig.jump_limit = f32::INFINITY;
    let mut nearest = f32::MAX;
    let mut landed = Vec::new();
    let mut last = rig.pos(bot);
    for _ in 0..60 * 75 {
        rig.app.update();
        let p = rig.pos(bot);
        nearest = nearest.min(p.distance(gold));
        if p.distance(last) > 6.0 {
            landed.push(p);
        }
        last = p;
        if driver(&rig, bot).reanchors >= 1 {
            break;
        }
    }
    let d = driver(&rig, bot);
    assert!(d.reanchors >= 1, "a walled bot is put back on its line");
    assert!(
        nearest > 45.0,
        "never got past the wall before its first re-anchor: {nearest}"
    );
    assert!(
        landed.iter().all(|p| p.distance(gold) > 25.0),
        "re-anchors land clear of the trigger: {landed:?}"
    );
    assert_eq!(rig.game().carrier(), None, "no score by teleport");
    assert!(
        !rig.events()
            .iter()
            .any(|e| matches!(e, GoldEvent::Picked { .. })),
        "{:?}",
        rig.events()
    );
    assert_eq!(rig.game().score(id), Some(0));
}

#[test]
fn nothing_drives_outside_a_local_match_and_races_are_untouched() {
    // No bot brain: the car is a plain AI car.
    let mut rig = Rig::new(CnrVariant::FreeForAll, false);
    let (bot, _) = rig.bot(0, rig.gold + Vec3::new(80.0, 0.0, 0.0), rig.gold);
    let start = rig.pos(bot);
    rig.run_until(180, &[bot], |_| false);
    assert!(rig.pos(bot).distance(start) < 1.0, "idle without a brain");

    // With one, a race opponent (an AI car that is no bot) keeps its own
    // input and is not seated in the match.
    let mut rig = Rig::new(CnrVariant::FreeForAll, true);
    let racer = {
        let (object, id) = {
            let mut s = rig.app.world_mut().resource_mut::<Session>();
            (s.mint_object_id(), s.mint_player_id())
        };
        rig.app
            .world_mut()
            .spawn((
                ObjectIdentity(object),
                Player {
                    id,
                    control: PlayerControl::Ai,
                },
                vehicle_bundle(&VehicleConfig::default()),
                Transform::from_xyz(0.0, 1.2, 0.0),
            ))
            .insert(VehicleInput {
                throttle: 0.25,
                ..default()
            })
            .id()
    };
    rig.bot(0, rig.gold + Vec3::new(80.0, 0.0, 0.0), rig.gold);
    rig.run_until(120, &[], |_| false);
    assert_eq!(
        rig.app.world().get::<VehicleInput>(racer).unwrap().throttle,
        0.25
    );
    assert_eq!(rig.game().standings().len(), 1, "only the bot is seated");
}

#[test]
fn a_paused_or_finished_session_holds_every_bot() {
    for phase in [SessionPhase::Paused, SessionPhase::Results] {
        let mut rig = Rig::new(CnrVariant::FreeForAll, true);
        let (bot, _) = rig.bot(0, rig.gold + Vec3::new(80.0, 0.0, 0.0), rig.gold);
        rig.run_until(240, &[bot], |_| false);
        rig.app
            .world_mut()
            .resource_mut::<Session>()
            .transition(phase.clone())
            .unwrap();
        rig.run_until(2, &[bot], |_| false);
        let input = rig.app.world().get::<VehicleInput>(bot).unwrap();
        assert_eq!((input.throttle, input.handbrake), (0.0, 1.0), "{phase:?}");
    }
}

/// CNRBOT-E synthetic leg: a full field of 15 bots in each variant. The
/// bots start in a ring around the loose gold and nothing is scripted
/// afterwards: at least one scores by driving (the step watches for
/// teleports), nobody is re-anchored into a marker, and the field's
/// replanning and policy evaluations stay bounded.
#[test]
fn a_field_of_fifteen_bots_scores_with_bounded_replanning_in_every_variant() {
    for variant in [
        CnrVariant::FreeForAll,
        CnrVariant::CopsVsRobbers,
        CnrVariant::RobbersVsRobbers,
    ] {
        let mut rig = Rig::new(variant, true);
        let centre = rig.gold;
        let mut bots = Vec::new();
        for i in 0..15u8 {
            let a = f32::from(i) / 15.0 * std::f32::consts::TAU;
            let ring = 60.0 + 12.0 * f32::from(i % 3);
            let at = centre + Vec3::new(a.cos() * ring, 0.0, a.sin() * ring);
            bots.push(rig.bot(i, at, centre));
        }
        let watch: Vec<Entity> = bots.iter().map(|b| b.0).collect();
        let ticks = 60 * 60;
        rig.run_until(ticks, &watch, |r| {
            bots.iter()
                .any(|(_, id)| r.game().score(*id).unwrap_or(0) > 0)
        });
        let scorer = bots
            .iter()
            .find(|(_, id)| rig.game().score(*id).unwrap_or(0) > 0);
        assert!(
            scorer.is_some(),
            "{variant:?}: nobody scored in {ticks} ticks"
        );
        // The score came from the host step's own pickup event, never a
        // direct ownership write.
        assert!(
            rig.events()
                .iter()
                .any(|e| matches!(e, GoldEvent::Picked { .. } | GoldEvent::Delivered { .. })),
            "{variant:?}: {:?}",
            rig.events()
        );
        for (e, _) in &bots {
            assert_eq!(
                driver(&rig, *e).reanchors,
                0,
                "{variant:?}: no teleport helped"
            );
        }
        let stats = rig.app.world().resource::<CnrBotBrain>().stats;
        let plans = stats.straight + stats.direct + stats.streets;
        // One line per bot, a switch or two each, and the rate limit.
        let bound = 15 * (1 + 2 * 2) + 15 * u64::from(ticks / REPLAN_FRAMES);
        assert!(plans <= bound, "{variant:?}: {plans} plans > {bound}");
        assert!(
            stats.policy_calls <= 15 * u64::from(ticks),
            "{variant:?}: {} policy calls",
            stats.policy_calls
        );
        assert!(stats.policy_calls > 0, "{variant:?}: the policy drove");
    }
}

/// The two teams of a team variant, as (human's side, other side).
fn teams(variant: CnrVariant) -> (Side, Side) {
    match variant {
        CnrVariant::CopsVsRobbers => (Side::Robbers, Side::Cops),
        CnrVariant::RobbersVsRobbers => (Side::Red, Side::Blue),
        CnrVariant::FreeForAll => unreachable!("free-for-all has no teams"),
    }
}

/// Where `side` delivers in this rig's world.
fn home(rig: &Rig, side: Side) -> Vec3 {
    let sites = rig.game().sites();
    sites.target(side.delivery_target())
}

const TEAM_VARIANTS: [CnrVariant; 2] = [CnrVariant::CopsVsRobbers, CnrVariant::RobbersVsRobbers];

/// Whether `events` hold a drop of the gold by `loser` knocked by `by`.
fn knocked(events: &[GoldEvent], loser: PlayerId, by: PlayerId) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            GoldEvent::Dropped { player, cause: DropCause::Knocked { by: Some(b) }, .. }
                if *player == loser && *b == by
        )
    })
}

/// CNRBOT-E human-carrier leg (a): a driven human carries the gold to
/// the hideout while a bot of each side waits off the way. The enemy
/// bot robs the human by ramming; the ally is never the one who takes
/// it. Nothing is placed after the start: the human's pedals are the
/// only hands, the bots drive themselves and the host step rules.
#[test]
fn a_driven_human_carrier_is_robbed_by_an_enemy_bot_and_never_by_an_ally() {
    for variant in TEAM_VARIANTS {
        let (mine, theirs) = teams(variant);
        let mut rig = Rig::new(variant, true);
        let (gold, hideout) = (rig.gold, rig.hideout);
        let (human, h_id) = rig.human(gold, hideout, mine);
        let (enemy, e_id) = rig.bot_on(0, along(gold, hideout, 0.2, 6.0), gold, theirs);
        let (ally, a_id) = rig.bot_on(1, along(gold, hideout, 0.3, -60.0), gold, mine);
        rig.run_driving(
            30,
            &[human, enemy, ally],
            |r| r.steer_toward(human, hideout),
            |r| r.game().carrier() == Some(h_id),
        );
        assert_eq!(
            rig.game().carrier(),
            Some(h_id),
            "{variant:?}: human picked up"
        );
        let before = rig.events().len();
        let t = rig.run_driving(
            60 * 60,
            &[human, enemy, ally],
            |r| r.steer_toward(human, hideout),
            |r| r.game().carrier() != Some(h_id),
        );
        let events = &rig.events()[before..];
        assert!(
            knocked(events, h_id, e_id),
            "{variant:?}: the enemy bot robs the human ({t} ticks): {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                GoldEvent::Dropped { cause: DropCause::Knocked { by: Some(b) }, .. } if *b == a_id
            )) && !events
                .iter()
                .any(|e| matches!(e, GoldEvent::Picked { player, .. } if *player == a_id)),
            "{variant:?}: the ally took part in the robbery: {events:?}"
        );
        assert_eq!(rig.game().carrier(), Some(e_id), "{variant:?}: {events:?}");
        assert_eq!(
            driver(&rig, enemy).reanchors + driver(&rig, ally).reanchors,
            0
        );
    }
}

/// The control for (a): with only an ally near, the human carrier is
/// left alone all the way to the hideout and scores.
#[test]
fn an_ally_bot_never_robs_the_human_carrier() {
    for variant in TEAM_VARIANTS {
        let (mine, _) = teams(variant);
        let mut rig = Rig::new(variant, true);
        let (gold, hideout) = (rig.gold, rig.hideout);
        let (human, h_id) = rig.human(gold, hideout, mine);
        let (ally, a_id) = rig.bot_on(0, along(gold, hideout, 0.4, 25.0), gold, mine);
        rig.run_driving(
            60 * 90,
            &[human, ally],
            |r| r.steer_toward(human, hideout),
            |r| r.game().score(h_id).unwrap_or(0) > 0,
        );
        assert!(
            rig.game().score(h_id).unwrap_or(0) > 0,
            "{variant:?}: delivered"
        );
        assert!(
            !rig.events()
                .iter()
                .any(|e| matches!(e, GoldEvent::Dropped { .. })
                    || matches!(e, GoldEvent::Picked { player, .. } if *player == a_id)),
            "{variant:?}: {:?}",
            rig.events()
        );
    }
}

/// CNRBOT-E human-carrier leg (b): a bot carries the gold toward the
/// hideout. A human on the other side robs it by driving into it.
#[test]
fn a_human_robs_an_enemy_bot_carrier_by_driving_into_it() {
    for variant in TEAM_VARIANTS {
        let (mine, theirs) = teams(variant);
        let mut rig = Rig::new(variant, true);
        let gold = rig.gold;
        let hideout = home(&rig, theirs);
        let (bot, b_id) = rig.bot_on(0, gold, hideout, theirs);
        let (human, h_id) = rig.human(along(gold, hideout, 0.8, 60.0), gold, mine);
        rig.run_driving(
            30,
            &[bot, human],
            |_| {},
            |r| r.game().carrier() == Some(b_id),
        );
        assert_eq!(
            rig.game().carrier(),
            Some(b_id),
            "{variant:?}: bot picked up"
        );
        let before = rig.events().len();
        let t = rig.run_driving(
            60 * 60,
            &[bot, human],
            |r| {
                let at = r.pos(bot);
                r.steer_toward(human, at)
            },
            |r| r.game().carrier() == Some(h_id),
        );
        let events = &rig.events()[before..];
        assert_eq!(
            rig.game().carrier(),
            Some(h_id),
            "{variant:?}: robbed after {t} ticks: {events:?}"
        );
        assert!(knocked(events, b_id, h_id), "{variant:?}: {events:?}");
        assert_eq!(driver(&rig, bot).reanchors, 0);
    }
}

/// A human on the carrier's own side is no robber to the bot: the
/// bot carries on to the hideout with the human driving alongside it
/// at the same hideout, and no steal is recorded.
#[test]
fn an_ally_human_does_not_rob_a_bot_carrier_driving_to_the_same_hideout() {
    for variant in TEAM_VARIANTS {
        let (mine, _) = teams(variant);
        let mut rig = Rig::new(variant, true);
        let gold = rig.gold;
        let hideout = home(&rig, mine);
        let (bot, b_id) = rig.bot_on(0, gold, hideout, mine);
        let (human, h_id) = rig.human(along(gold, hideout, 0.1, 30.0), hideout, mine);
        rig.run_driving(
            60 * 90,
            &[bot, human],
            |r| r.steer_toward(human, hideout),
            |r| r.game().score(b_id).unwrap_or(0) > 0,
        );
        assert!(
            rig.game().score(b_id).unwrap_or(0) > 0,
            "{variant:?}: delivered"
        );
        assert!(
            !rig.events().iter().any(|e| matches!(
                e,
                GoldEvent::Picked { player, .. } if *player == h_id
            )),
            "{variant:?}: {:?}",
            rig.events()
        );
    }
}

/// Six bots ringed round the loose gold, seated and driving.
fn ring(rig: &mut Rig, n: u8) -> Vec<(Entity, PlayerId)> {
    let centre = rig.gold;
    (0..n)
        .map(|i| {
            let a = f32::from(i) / f32::from(n) * std::f32::consts::TAU;
            let ring = 60.0 + 12.0 * f32::from(i % 3);
            let at = centre + Vec3::new(a.cos() * ring, 0.0, a.sin() * ring);
            rig.bot(i, at, centre)
        })
        .collect()
}

/// CNRBOT-E removal leg: the carrier's seat vanishes mid-match, as a
/// disconnect does. The host step runs its production leave: the gold
/// drops where the car was (no lockout), the seat stays on the board
/// disconnected with its points, the other bots keep driving, one of
/// them takes the dropped gold and delivers, and the match ends with a
/// connected winner — no panic, no stale entity reference.
#[test]
fn removing_the_carrying_bot_drops_its_gold_and_the_match_goes_on() {
    let mut rig = Rig::build(CnrVariant::FreeForAll, true, EndRule::Points(100), |_| {});
    let bots = ring(&mut rig, 6);
    let watch: Vec<Entity> = bots.iter().map(|b| b.0).collect();
    rig.run_until(60 * 60, &watch, |r| r.game().carrier().is_some());
    let carrier = rig.game().carrier().expect("somebody picked the gold up");
    let (victim, _) = *bots.iter().find(|b| b.1 == carrier).unwrap();
    let seats = rig.game().standings().len();
    let last = rig.pos(victim);
    let before = rig.events().len();

    // The same removal a disconnect makes: the car is simply gone.
    rig.app.world_mut().entity_mut(victim).despawn();
    let survivors: Vec<Entity> = watch.iter().copied().filter(|e| *e != victim).collect();
    rig.run_until(2, &survivors, |_| false);

    assert_eq!(rig.game().carrier(), None, "the gold is no longer carried");
    let dropped = rig
        .game()
        .gold_position()
        .expect("the gold is on the ground");
    assert!(
        dropped.distance(last) < rig.jump_limit,
        "dropped at {dropped:?}, the car was at {last:?}"
    );
    let events = &rig.events()[before..];
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GoldEvent::Left { player } if *player == carrier)),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            GoldEvent::Dropped { player, cause: mm2_game::gold::DropCause::Disconnected, .. }
                if *player == carrier
        )),
        "{events:?}"
    );
    let standings = rig.game().standings();
    assert_eq!(standings.len(), seats, "the seat list is unchanged");
    let gone = standings.iter().find(|s| s.player == carrier).unwrap();
    assert!(!gone.connected, "the leaver is marked gone");
    assert!(
        standings
            .iter()
            .filter(|s| s.player != carrier)
            .all(|s| s.connected),
        "nobody else left: {standings:?}"
    );
    assert!(rig.game().outcome().is_none(), "the match is not over yet");

    // The rest of the field carries on: somebody else takes the gold,
    // delivers it, and wins.
    rig.run_until(60 * 120, &survivors, |r| r.game().outcome().is_some());
    let outcome = rig.game().outcome().expect("the match ended");
    let winner = format!("{outcome:?}");
    assert!(
        !winner.contains(&format!("{carrier:?}")),
        "the leaver cannot win: {winner}"
    );
    assert!(
        rig.events().iter().any(|e| matches!(
            e,
            GoldEvent::Picked { player, .. } if *player != carrier
        )),
        "{:?}",
        rig.events()
    );
    for (e, id) in &bots {
        if *id != carrier {
            assert!(rig.app.world().get::<CnrBotDriver>(*e).is_some());
        }
    }
}

/// The evidence knob names no bot: with nobody carrying, the lowest
/// index goes. Its points stay, the others keep their seats and drive,
/// and a second knob firing is not a thing (one-shot).
#[test]
fn the_drop_knob_removes_the_lowest_bot_once_and_the_rest_keep_driving() {
    let mut rig = Rig::build(CnrVariant::FreeForAll, true, EndRule::None, |dev| {
        dev.cnr_drop_bot_at = Some(3);
    });
    let bots = ring(&mut rig, 4);
    let watch: Vec<Entity> = bots.iter().map(|b| b.0).collect();
    rig.run_until(2, &watch, |_| false);
    assert!(rig.game().standings().iter().all(|s| s.connected));
    let survivors: Vec<Entity> = watch[1..].to_vec();
    let start: Vec<Vec3> = survivors.iter().map(|e| rig.pos(*e)).collect();
    rig.run_until(240, &survivors, |_| false);
    assert!(
        rig.app.world().get_entity(bots[0].0).is_err(),
        "bot 0 was removed"
    );
    let standings = rig.game().standings();
    assert_eq!(standings.len(), 4);
    assert_eq!(
        standings.iter().filter(|s| !s.connected).count(),
        1,
        "{standings:?}"
    );
    assert!(
        !standings
            .iter()
            .find(|s| s.player == bots[0].1)
            .unwrap()
            .connected
    );
    for (e, from) in survivors.iter().zip(&start) {
        assert!(rig.pos(*e).distance(*from) > 5.0, "a survivor kept driving");
    }
}
