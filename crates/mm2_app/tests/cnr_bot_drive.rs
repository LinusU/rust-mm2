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
use mm2_app::cnr_bots::CnrBot;
use mm2_app::contracts;
use mm2_content::cnr::{CnrSettings, GoldMass};
use mm2_game::gold::{CnrVariant, EndRule, GoldEvent, GoldMatch};
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
        let mut session = Session::new();
        session
            .begin(SessionConfig {
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
            })
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let gold_id = session.mint_object_id();
        let mut rules = CnrSettings {
            variant,
            gold_mass: GoldMass::QuarterTon,
            ..CnrSettings::default()
        }
        .rules(HZ);
        rules.end = EndRule::None;
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
            .add_systems(FixedUpdate, cnr_bot_drive)
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
