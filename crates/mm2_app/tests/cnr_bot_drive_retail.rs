//! CNRBOT-C original-content validation: local Cops & Robbers bots
//! drive the retail SF and London street graphs through the retail
//! city's collision, with production vehicles and the real gold step.
//!
//! Evidence level: original-content validation (retail nav graph,
//! collision, vehicle data, deadly water; no rendering, no audio, no
//! play-test). Opt-in (`MM2_RETAIL=<dir>`); skipped without the
//! operator's install, and says so.
//!
//! Legs automated here, per city: (1) one bot drives from a street
//! start to the loose gold, takes it and delivers it; (2) a second bot
//! pursues that carrier. What is *not* claimed: that a steal succeeds
//! in a given street layout — the test reports whether the pursuer
//! reached the carrier and whether the gold changed hands, and asserts
//! only the contact.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::city::load_city;
use mm2_app::cnr::{
    CnrEvent, CnrHost, cnr_host_step, enroll_cnr_participants, reconcile_gold_load,
};
use mm2_app::cnr_bot_drive::{CnrBotBrain, CnrBotDriver, cnr_bot_drive};
use mm2_app::cnr_bots::{
    BotSlot, CnrBot, PlannedBot, lane_slots, load_graph, reachable_pool, spawn_bots,
    stock_vehicle_ids,
};
use mm2_app::contracts;
use mm2_assets::Vfs;
use mm2_content::cnr::{CnrContent, CnrSettings, GoldMass};
use mm2_formats::{psdl::Psdl, water::WaterDef};
use mm2_game::gold::{CnrVariant, DropCause, GoldEvent, Side};
use mm2_game::{
    ImpactEvent, Player, PlayerId, Session, SessionAuthority, SessionConfig, SessionEntity,
    SessionMode, SessionPhase, WorldMode,
};
use mm2_vehicle::{VehicleInput, VehiclePlugin};

const HZ: u32 = 60;

#[derive(Resource)]
struct VfsRes(Vfs);

#[derive(Resource, Default)]
struct Events(Vec<GoldEvent>);

fn collect(mut reader: MessageReader<CnrEvent>, mut out: ResMut<Events>) {
    out.0.extend(reader.read().map(|e| e.0));
}

fn mount(retail: &std::path::Path) -> Vfs {
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, retail, &mm2_assets::InstallMount::default()).unwrap();
    vfs
}

/// Whether a car on the road can meet `p`: a routable vehicle lane
/// passes within 15 m horizontally and 3 m vertically — the measure
/// `docs/research/cnr.md` uses for "on a lane". Most retail sites are
/// not (plazas, parks, another level), and a street-following bot
/// cannot take or deliver those; that is a property of the data, so the
/// legs below run on a seed whose opening draw is all on lanes.
fn on_lane(graph: &mm2_game::NavGraph, p: Vec3) -> bool {
    graph
        .nearest_lane(p.into(), &mm2_game::LaneQuery::vehicles(30.0))
        .is_some_and(|h| {
            (h.point[0] - p.x).hypot(h.point[2] - p.z) <= 15.0 && (h.point[1] - p.y).abs() <= 3.0
        })
}

/// The first `n` seeds whose opening gold and hideout both lie on a lane.
pub(crate) fn lane_seeds(vfs: &Vfs, city: &str, graph: &mm2_game::NavGraph, n: usize) -> Vec<u64> {
    let content = CnrContent::load(vfs, city);
    let settings = CnrSettings::default();
    (0..4096u64)
        .filter(|&seed| {
            let host = CnrHost::from_content(
                &content,
                &settings,
                HZ,
                1,
                mm2_game::ObjectId {
                    generation: 1,
                    slot: 1,
                },
                seed,
                &[],
            )
            .unwrap();
            let sites = host.game.sites();
            on_lane(graph, sites.gold) && on_lane(graph, sites.hideout)
        })
        .take(n)
        .collect()
}

/// The retail city under a local Cops & Robbers session that is
/// `Playing`, with the bot driver and the gold step wired as in the app.
fn city_app(
    retail: &std::path::Path,
    city: &str,
    seed: u64,
    reachable_only: bool,
    variant: CnrVariant,
) -> (App, Vec<BotSlot>) {
    let vfs = mount(retail);
    let surfaces = mm2_content::load_surface_tables(&vfs).unwrap();
    let (psdl_bytes, _) = vfs.read_path(&format!("city/{city}.psdl")).unwrap();
    let psdl = Psdl::parse(&psdl_bytes).unwrap();
    let (water_bytes, _) = vfs.read_path(&format!("city/{city}.water")).unwrap();
    let def = WaterDef::parse(std::str::from_utf8(&water_bytes).unwrap()).unwrap();
    let water = mm2_app::water::CityWater::build(&def, &psdl, surfaces.as_ref());
    let graph = load_graph(&vfs, city).unwrap();
    let slots: Vec<BotSlot> = lane_slots(&graph, Some(&water))
        .into_iter()
        .filter(|(_, wet)| !wet)
        .map(|(s, _)| s)
        .collect();

    let mut session = Session::new();
    session
        .begin(SessionConfig {
            world: WorldMode::City {
                psdl: format!("city/{city}.psdl"),
            },
            mode: SessionMode::CopsAndRobbers(CnrSettings {
                variant,
                ..CnrSettings::default()
            }),
            authority: SessionAuthority::Local,
            seed,
            ..SessionConfig::default()
        })
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let gold = session.mint_object_id();
    let mut content = CnrContent::load(&vfs, city);
    if reachable_only {
        content.sites = reachable_pool(&graph, &content.sites);
    }
    let host = CnrHost::from_content(
        &content,
        &CnrSettings {
            variant,
            gold_mass: GoldMass::QuarterTon,
            ..CnrSettings::default()
        },
        HZ,
        session.generation(),
        gold,
        seed,
        &[],
    )
    .unwrap();

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
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(VfsRes(vfs))
        .insert_resource(session)
        .insert_resource(host)
        .insert_resource(water)
        .insert_resource(CnrBotBrain::new(Some(std::sync::Arc::new(graph))))
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
    app.finish();
    app.cleanup();
    let psdl = format!("city/{city}.psdl");
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>| {
                let mut s = Session::new();
                load_city(
                    &mut commands,
                    &vfs.0,
                    &psdl,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    SessionEntity(1),
                    &mut s,
                )
                .expect("retail city loads");
            },
        )
        .unwrap();
    for _ in 0..3 {
        app.update();
    }
    (app, slots)
}

/// The lane start nearest `distance` metres (XZ) from `from`, not in
/// `taken`.
fn start_near(slots: &[BotSlot], from: Vec3, distance: f32, taken: &[BotSlot]) -> BotSlot {
    *slots
        .iter()
        .filter(|s| !taken.contains(s))
        .min_by(|a, b| {
            let d = |s: &BotSlot| {
                ((s.position.x - from.x).hypot(s.position.z - from.z) - distance).abs()
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap()
}

fn spawn(app: &mut App, slots: Vec<BotSlot>) -> Vec<(Entity, PlayerId)> {
    let mut plan = Some(
        slots
            .into_iter()
            .map(|slot| {
                let vfs = &app.world().resource::<VfsRes>().0;
                let ids = stock_vehicle_ids(vfs);
                PlannedBot {
                    def: mm2_content::load_opponent(vfs, &ids[0], 0).unwrap(),
                    slot,
                }
            })
            .collect::<Vec<_>>(),
    );
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mut session: ResMut<Session>| {
                spawn_bots(
                    &mut commands,
                    &vfs.0,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    plan.take().unwrap(),
                    SessionEntity(1),
                    &mut session,
                );
            },
        )
        .unwrap();
    app.update();
    let world = app.world_mut();
    let mut q = world.query::<(Entity, &CnrBot, &Player)>();
    let mut v: Vec<_> = q.iter(world).map(|(e, b, p)| (b.index, e, p.id)).collect();
    v.sort();
    v.into_iter().map(|(_, e, id)| (e, id)).collect()
}

fn pos(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Position>(e).unwrap().0
}

/// What one city/seed run did.
#[derive(Debug)]
struct Outcome {
    picked: Option<u32>,
    delivered: Option<u32>,
    /// Closest the second bot came to the carrier while it carried, m.
    closest: f32,
    /// Whether the second bot ended up with the gold by ramming.
    stolen: bool,
    streets: u64,
    reanchors: u32,
    refused: u32,
    why_stopped: &'static str,
    /// Why A took no gold, when that is the map's doing and not the
    /// driver's: see [`classify`].
    unreachable: Option<&'static str>,
}

/// Two bots in `city` on `seed`: A starts ~60 m from the gold, B ~150 m. A is run until it delivers, sits [`GIVE_UP`] seconds inside a
/// 15 m circle, has not taken the gold by [`PICKUP_BY`], or [`BUDGET`]
/// seconds pass.
fn run(retail: &std::path::Path, city: &str, seed: u64) -> Outcome {
    let (mut app, slots) = city_app(retail, city, seed, false, CnrVariant::FreeForAll);
    let gold = app.world().resource::<CnrHost>().game.sites().gold;
    let a_slot = start_near(&slots, gold, 60.0, &[]);
    let b_slot = start_near(&slots, gold, 150.0, &[a_slot]);
    let bots = spawn(&mut app, vec![a_slot, b_slot]);
    let ((a, a_id), (b, b_id)) = (bots[0], bots[1]);
    let mut out = Outcome {
        picked: None,
        delivered: None,
        closest: f32::MAX,
        stolen: false,
        streets: 0,
        reanchors: 0,
        refused: 0,
        why_stopped: "budget",
        unreachable: None,
    };
    let (mut still_at, mut still_since) = (pos(&app, a), 0u32);
    // `MM2_TRACE=<city>:<seed>` prints A's progress every five seconds.
    let trace = std::env::var("MM2_TRACE").is_ok_and(|t| t == format!("{city}:{seed}"));
    if trace {
        let graph = load_graph(&mount(retail), city).unwrap();
        let (line, kind) =
            mm2_app::cnr_bot_drive::plan_line(Some(&graph), pos(&app, a), gold, false);
        eprintln!("  line {kind:?} of {} points; the last eight:", line.len());
        for p in &line[line.len().saturating_sub(8)..] {
            eprintln!("  line {:.0},{:.1},{:.0}", p.x, p.y, p.z);
        }
    }
    for tick in 0..HZ * BUDGET {
        app.update();
        for e in &app.world().resource::<Events>().0 {
            match e {
                GoldEvent::Picked {
                    player,
                    recovered: false,
                    ..
                } if *player == a_id => {
                    out.picked.get_or_insert(tick);
                }
                GoldEvent::Picked { player, .. } if *player == b_id => out.stolen = true,
                GoldEvent::Delivered { player, .. } if *player == a_id => {
                    out.delivered.get_or_insert(tick);
                }
                _ => {}
            }
        }
        if trace && tick % (HZ * 5) == 0 {
            let (p, g) = (
                pos(&app, a),
                app.world().resource::<CnrHost>().game.gold_position(),
            );
            let d = app.world().get::<CnrBotDriver>(a).unwrap();
            eprintln!(
                "  t={:>3}s A={:.0},{:.0},{:.0} B={:?} gold={g:?} flat={:.0} dy={:.1} kind={:?} left={:?} re={} ref={}",
                tick / HZ,
                p.x,
                p.y,
                p.z,
                pos(&app, b).round(),
                g.map(|g| (g.x - p.x).hypot(g.z - p.z)).unwrap_or(-1.0),
                g.map(|g| g.y - p.y).unwrap_or(0.0),
                d.line_kind(),
                d.line_remaining(),
                d.reanchors,
                d.refused
            );
        }
        if out.delivered.is_some() {
            out.why_stopped = "delivered";
            break;
        }
        if out.picked.is_some() {
            out.closest = out.closest.min(pos(&app, a).distance(pos(&app, b)));
        }
        // Given up on: A has not left a 15 m circle for GIVE_UP seconds,
        // or has not taken the gold within PICKUP_BY seconds.
        if tick % 30 == 0 {
            let here = pos(&app, a);
            if here.distance(still_at) > 15.0 {
                (still_at, still_since) = (here, tick);
            }
        }
        if tick > still_since + HZ * GIVE_UP {
            out.why_stopped = "stationary";
            break;
        }
        if out.picked.is_none() && tick > HZ * PICKUP_BY {
            out.why_stopped = "no pickup";
            break;
        }
    }
    out.unreachable = classify(&mut app, a, gold, &out);
    let (da, db) = (
        app.world().get::<CnrBotDriver>(a).unwrap(),
        app.world().get::<CnrBotDriver>(b).unwrap(),
    );
    out.streets = app.world().resource::<CnrBotBrain>().stats.streets;
    out.reanchors = da.reanchors + db.reanchors;
    out.refused = da.refused + db.refused;
    eprintln!("{city} seed {seed}: {out:?}");
    out
}

/// The policy for a bot that never took the gold (#1681). A bot is not
/// asked to take what no car can: a run with no pickup by A is
/// explained, and not a driver failure, when
///
/// - `"rival took it"`: B, on the same map, took the gold first;
/// - `"unreachable level"`: the street graph has no way from A's level
///   to the gold's ([`LineKind::Unreachable`]; the retail london site
///   under a flyover, seed 12);
/// - `"walled site"`: the gold is farther than the pickup radius from A,
///   which stands at the end of the road line, with solid world between
///   (a city-room wall: the retail sf site east of the elevated road,
///   seeds 93 and 122).
///
/// Anything else with no pickup is a finding.
fn classify(app: &mut App, a: Entity, gold: Vec3, out: &Outcome) -> Option<&'static str> {
    if out.picked.is_some() {
        return None;
    }
    if out.stolen {
        return Some("rival took it");
    }
    let d = app.world().get::<CnrBotDriver>(a).unwrap();
    if d.line_kind() == Some(mm2_app::cnr_bot_drive::LineKind::Unreachable) {
        return Some("unreachable level");
    }
    let from = pos(app, a) + Vec3::Y * 1.5;
    let to = gold + Vec3::Y * 1.5;
    let (dir, reach) = (Dir3::new(to - from).ok()?, from.distance(to));
    let blocked = app
        .world_mut()
        .run_system_once(move |sq: SpatialQuery, bodies: Query<&RigidBody>| {
            sq.cast_ray_predicate(
                from,
                dir,
                reach,
                true,
                &SpatialQueryFilter::default(),
                &|e| bodies.get(e).is_ok_and(|b| b.is_static()),
            )
            .is_some()
        })
        .unwrap();
    (reach > 5.0 && blocked).then_some("walled site")
}

/// Seconds of simulated time one run may take, and how long A may go
/// without getting closer to the end of its line.
const BUDGET: u32 = 300;
const GIVE_UP: u32 = 60;
const PICKUP_BY: u32 = 120;
/// Seeds tried per city. A run is chaotic: the same seed's bots diverge by
/// centimetres at the first landing and then amplify, so whether any one
/// seed delivers or sees the pursuer flips with every change to the
/// physics stack (Bevy 0.19 / Avian 0.7 vs Bevy 0.20 / Avian 0.8 measured
/// over these nine seeds: sf 5 delivered / 2 pursued on both, london 4 / 3
/// on both, while the first three seeds alone went 1 / 1 to 1 / 0). Nine
/// seeds make the "at least one" thresholds a statement about the drivers
/// rather than about which seeds land well.
const SEEDS: usize = 9;

#[test]
fn retail_bots_pick_up_deliver_and_pursue_in_each_city() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the retail bot drive was NOT checked");
        return;
    };
    for city in ["sf", "london"] {
        let vfs = mount(&retail);
        let graph = load_graph(&vfs, city).unwrap();
        let seeds = lane_seeds(&vfs, city, &graph, SEEDS);
        assert!(!seeds.is_empty(), "{city}: seeds with an all-on-lane draw");
        let outcomes: Vec<(u64, Outcome)> = seeds
            .into_iter()
            .map(|seed| (seed, run(&retail, city, seed)))
            .collect();
        // Every run reports (see the eprintln above). The legs work on
        // the seeds where the map lets them, so at least one seed must
        // deliver and at least one must see the pursuer close on the
        // carrier; a seed that does neither is a finding, not a pass.
        let delivered = outcomes
            .iter()
            .filter(|(_, o)| o.delivered.is_some())
            .count();
        let pursued = outcomes.iter().filter(|(_, o)| o.closest < 30.0).count();
        eprintln!(
            "{city}: {delivered} of {} seeds delivered, {pursued} saw the pursuer within 30 m",
            outcomes.len()
        );
        assert!(delivered > 0, "{city}: no seed delivered: {outcomes:?}");
        for (seed, o) in &outcomes {
            assert!(
                o.picked.is_some() || o.delivered.is_some() || o.unreachable.is_some(),
                "{city} seed {seed}: A never took the gold and nothing on the map explains it: {o:?}"
            );
        }
        assert!(
            pursued > 0,
            "{city}: no pursuer closed on a carrier: {outcomes:?}"
        );
        assert!(
            outcomes.iter().all(|(_, o)| o.streets > 0),
            "{city}: street lines"
        );
    }
}

/// What one steal hunt saw.
#[derive(Debug, Default)]
struct Steal {
    /// Frame (from the first step) A took the loose gold.
    picked: Option<u32>,
    /// Frame B's hit knocked the gold out of A's car.
    knocked: Option<u32>,
    /// Frame B held the gold it had knocked loose.
    stolen: Option<u32>,
    /// Closest B came to A while A carried, m.
    closest: f32,
}

/// Two bots, A ~60 m and B ~150 m from the gold (as [`run`]), both on
/// the production driver, vehicle and collision; nothing is placed or
/// written after the start. Ends when B holds the gold A carried
/// (a `Dropped { Knocked { by: B } }` of A's gold, then `Picked` by B),
/// when A delivers, or after [`BUDGET`] seconds.
fn hunt(retail: &std::path::Path, city: &str, seed: u64) -> Steal {
    let (mut app, slots) = city_app(retail, city, seed, false, CnrVariant::FreeForAll);
    let gold = app.world().resource::<CnrHost>().game.sites().gold;
    let a_slot = start_near(&slots, gold, 60.0, &[]);
    let b_slot = start_near(&slots, gold, 150.0, &[a_slot]);
    let bots = spawn(&mut app, vec![a_slot, b_slot]);
    let ((a, a_id), (b, b_id)) = (bots[0], bots[1]);
    let mut out = Steal {
        closest: f32::MAX,
        ..Steal::default()
    };
    let mut seen = 0usize;
    for tick in 0..HZ * BUDGET {
        app.update();
        let events = app.world().resource::<Events>().0.clone();
        for e in &events[seen..] {
            match e {
                GoldEvent::Picked {
                    player,
                    recovered: false,
                    ..
                } if *player == a_id => {
                    out.picked.get_or_insert(tick);
                }
                GoldEvent::Dropped {
                    player,
                    cause: DropCause::Knocked { by: Some(by) },
                    ..
                } if *player == a_id && *by == b_id => {
                    out.knocked.get_or_insert(tick);
                }
                GoldEvent::Picked { player, .. } if *player == b_id && out.knocked.is_some() => {
                    out.stolen.get_or_insert(tick);
                }
                GoldEvent::Delivered { .. } => return out,
                _ => {}
            }
        }
        seen = events.len();
        if out.stolen.is_some() {
            return out;
        }
        if out.picked.is_some() {
            out.closest = out.closest.min(pos(&app, a).distance(pos(&app, b)));
        }
        if out.picked.is_none() && tick > HZ * PICKUP_BY {
            break;
        }
    }
    out
}

/// CNRBOT-E: a steal by ramming on a retail street. Bot B takes the gold
/// off bot A's car by driving into it, with production physics, the nav
/// graph and the neural driver; nothing is placed or scripted. The seeds
/// were found by hunting the first [`STEAL_SEARCH`] all-on-lane seeds of
/// each city (`docs/research/cnr.md`): only London 122 and 164 steal, so
/// both are pinned and each must reproduce. A change to physics, the
/// graph or the driver that moves them fails here; re-run the hunt with
/// `MM2_STEAL_SEARCH=1` to find new seeds.
#[test]
fn a_bot_steals_the_gold_by_ramming_on_a_retail_street() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the ramming steal was NOT checked");
        return;
    };
    if std::env::var_os("MM2_STEAL_SEARCH").is_some() {
        for city in ["sf", "london"] {
            let vfs = mount(&retail);
            let graph = load_graph(&vfs, city).unwrap();
            for seed in lane_seeds(&vfs, city, &graph, STEAL_SEARCH) {
                eprintln!("{city} seed {seed}: {:?}", hunt(&retail, city, seed));
            }
        }
        return;
    }
    for seed in [122, 164] {
        let s = hunt(&retail, "london", seed);
        eprintln!("london seed {seed}: {s:?}");
        let (picked, knocked, stolen) = (
            s.picked.expect("A never took the gold"),
            s.knocked.expect("B never knocked the gold out of A"),
            s.stolen.expect("B never held the gold it knocked loose"),
        );
        assert!(picked < knocked && knocked <= stolen, "{s:?}");
    }
}

/// Seeds per city the hunt searches (`MM2_STEAL_SEARCH`).
const STEAL_SEARCH: usize = 12;

/// CNRBOT-E finding (#1686): ten bots in London on the default seed
/// scored nothing in four minutes, because the drawn gold lies in a
/// room 54 m from the nearest lane. A session with bots draws from the
/// street-reachable sites ([`reachable_pool`], designed not original),
/// and the field then scores on its own: no bot is placed at the gold
/// and nothing writes the match.
#[test]
fn ten_london_bots_score_on_the_default_seed() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the London bot field was NOT checked");
        return;
    };
    let (mut app, slots) = city_app(&retail, "london", 0, true, CnrVariant::FreeForAll);
    let sites = app.world().resource::<CnrHost>().game.sites();
    let vfs = mount(&retail);
    let graph = load_graph(&vfs, "london").unwrap();
    assert!(
        [sites.gold, sites.hideout, sites.bank]
            .iter()
            .all(|p| on_lane(&graph, *p)),
        "the opening draw is all on lanes: {sites:?}"
    );
    let candidates: Vec<_> = slots.into_iter().map(|s| (s, false)).collect();
    let chosen = mm2_app::cnr_bots::choose_slots(
        &candidates,
        10,
        0,
        Vec3::ZERO,
        &[sites.gold, sites.hideout, sites.bank],
    )
    .unwrap();
    spawn(&mut app, chosen);
    let mut points = 0;
    for _ in 0..HZ * 180 {
        app.update();
        points = app
            .world()
            .resource::<Events>()
            .0
            .iter()
            .map(|e| match e {
                GoldEvent::Delivered { points, .. } => *points,
                _ => 0,
            })
            .sum::<u32>();
        if points > 0 {
            break;
        }
    }
    assert!(points > 0, "no bot scored in 180 s of London on seed 0");
}

/// A person at the wheel of a retail car: routes over the street graph
/// to `goal` and steers at a point ahead on the line. It writes only
/// the pedals — the same `VehicleInput` a keyboard would.
struct Hands {
    graph: mm2_game::NavGraph,
    line: Vec<Vec3>,
    planned_for: Vec3,
    age: u32,
}

impl Hands {
    fn new(graph: mm2_game::NavGraph) -> Self {
        Self {
            graph,
            line: Vec::new(),
            planned_for: Vec3::ZERO,
            age: 0,
        }
    }

    fn drive(&mut self, app: &mut App, car: Entity, goal: Vec3) {
        let world = app.world();
        let (p, q) = (
            world.get::<Position>(car).unwrap().0,
            world.get::<Rotation>(car).unwrap().0,
        );
        self.age += 1;
        let moved = (goal.x - self.planned_for.x).hypot(goal.z - self.planned_for.z);
        if self.line.len() < 2 || self.age >= 45 || moved > 20.0 {
            self.line = self
                .graph
                .drive_line(p, goal, &mm2_game::RouteOptions::default(), 5.0)
                .unwrap_or_else(|_| vec![p, goal]);
            self.planned_for = goal;
            self.age = 0;
        }
        let near = (0..self.line.len())
            .min_by(|&a, &b| {
                self.line[a]
                    .distance_squared(p)
                    .total_cmp(&self.line[b].distance_squared(p))
            })
            .unwrap_or(0);
        let aim = self.line[near..]
            .iter()
            .find(|l| l.distance(p) >= 18.0)
            .copied()
            .unwrap_or(goal);
        let fwd = q * Vec3::NEG_Z;
        let want = aim - p;
        let angle = (fwd.x * want.z - fwd.z * want.x).atan2(fwd.x * want.x + fwd.z * want.z);
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = VehicleInput {
            throttle: if angle.abs() > 0.6 { 0.3 } else { 0.7 },
            steering: (angle * 2.0).clamp(-1.0, 1.0),
            ..default()
        };
    }
}

/// Spawn the retail bots at `slots`, then turn `human` of them into a
/// person's car (no tag, no driver, local control) and seat everyone on
/// `sides[i]` before the first step, so the split is the test's.
fn spawn_sided(
    app: &mut App,
    slots: Vec<BotSlot>,
    human: usize,
    sides: &[Side],
) -> Vec<(Entity, PlayerId)> {
    let mut plan = Some(
        slots
            .into_iter()
            .map(|slot| {
                let vfs = &app.world().resource::<VfsRes>().0;
                let ids = stock_vehicle_ids(vfs);
                PlannedBot {
                    def: mm2_content::load_opponent(vfs, &ids[0], 0).unwrap(),
                    slot,
                }
            })
            .collect::<Vec<_>>(),
    );
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  vfs: Res<VfsRes>,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mut session: ResMut<Session>| {
                spawn_bots(
                    &mut commands,
                    &vfs.0,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    plan.take().unwrap(),
                    SessionEntity(1),
                    &mut session,
                );
            },
        )
        .unwrap();
    let world = app.world_mut();
    let mut q = world.query::<(Entity, &CnrBot, &Player)>();
    let mut v: Vec<_> = q.iter(world).map(|(e, b, p)| (b.index, e, p.id)).collect();
    v.sort();
    let cars: Vec<(Entity, PlayerId)> = v.into_iter().map(|(_, e, id)| (e, id)).collect();
    let (person, _) = cars[human];
    world
        .entity_mut(person)
        .remove::<(CnrBot, CnrBotDriver)>()
        .get_mut::<Player>()
        .unwrap()
        .control = mm2_game::PlayerControl::Local;
    for (i, (_, id)) in cars.iter().enumerate() {
        world
            .resource_mut::<CnrHost>()
            .game
            .join(*id, sides[i])
            .unwrap();
    }
    app.update();
    cars
}

fn home(app: &App, side: Side) -> Vec3 {
    app.world()
        .resource::<CnrHost>()
        .game
        .sites()
        .target(side.delivery_target())
}

fn carrier(app: &App) -> Option<PlayerId> {
    app.world().resource::<CnrHost>().game.carrier()
}

/// What one human-carrier run saw.
#[derive(Debug, Default)]
struct HumanRun {
    /// The human took the gold from the ground.
    human_picked: bool,
    /// An enemy bot took it off the human by ramming.
    robbed: bool,
    /// The human took it off an enemy bot by ramming.
    robbed_bot: bool,
    /// An ally of the carrier took the gold from it.
    ally_took: bool,
    /// An ally of the carrier knocked it loose (host impact rule: any
    /// hard car-on-car hit drops the gold; there is no teammate rule).
    ally_knocked: bool,
    reanchors: u32,
}

/// Leg (a): the person starts nearest the gold, takes it by driving and
/// carries it home over the streets; an enemy bot and an ally bot start
/// further out and play the objectives the planner gives them. Leg (b)
/// (`human_robs`): a bot starts nearest the gold and the person drives
/// to wherever that carrier is. Nothing is placed after the start.
fn run_human(
    retail: &std::path::Path,
    city: &str,
    seed: u64,
    variant: CnrVariant,
    human_robs: bool,
) -> HumanRun {
    let (mine, theirs) = match variant {
        CnrVariant::CopsVsRobbers => (Side::Robbers, Side::Cops),
        _ => (Side::Red, Side::Blue),
    };
    let (mut app, slots) = city_app(retail, city, seed, false, variant);
    let gold = app.world().resource::<CnrHost>().game.sites().gold;
    let near = start_near(&slots, gold, 60.0, &[]);
    let mid = start_near(&slots, gold, 110.0, &[near]);
    let far = start_near(&slots, gold, 160.0, &[near, mid]);
    let graph = load_graph(&mount(retail), city).unwrap();
    let mut hands = Hands::new(graph);
    // Index 0 is the person (leg a) or the enemy carrier-to-be (leg b).
    let (cars, human, side_of_first) = if human_robs {
        let c = spawn_sided(&mut app, vec![near, far, mid], 1, &[theirs, mine, theirs]);
        (c.clone(), c[1], theirs)
    } else {
        let c = spawn_sided(&mut app, vec![near, far, mid], 0, &[mine, theirs, mine]);
        (c.clone(), c[0], mine)
    };
    let (person, p_id) = human;
    // The bot that carries (leg b) / the enemy (leg a) / the ally.
    let (lead, lead_id) = cars[0];
    let (enemy_id, ally_id) = if human_robs {
        (cars[0].1, cars[2].1)
    } else {
        (cars[1].1, cars[2].1)
    };
    let _ = (lead, lead_id, side_of_first);
    let mut out = HumanRun::default();
    let mut seen = 0usize;
    for _ in 0..HZ * 240 {
        let who = carrier(&app);
        let goal = if human_robs {
            match who {
                Some(c) if c != p_id => {
                    app.world()
                        .get::<Position>(
                            cars.iter()
                                .find(|(_, id)| *id == c)
                                .map(|(e, _)| *e)
                                .unwrap(),
                        )
                        .unwrap()
                        .0
                }
                _ => gold,
            }
        } else if who == Some(p_id) {
            home(&app, mine)
        } else {
            gold
        };
        hands.drive(&mut app, person, goal);
        app.update();
        let events = app.world().resource::<Events>().0.clone();
        for e in &events[seen..] {
            match e {
                GoldEvent::Picked {
                    player,
                    recovered: false,
                    ..
                } if *player == p_id && !human_robs => out.human_picked = true,
                GoldEvent::Dropped {
                    player,
                    cause: DropCause::Knocked { by: Some(by) },
                    ..
                } => {
                    if human_robs {
                        // The carrier is bot 0; the other bot on its
                        // side is its ally, the person its enemy.
                        if *player == enemy_id && *by == p_id {
                            out.robbed_bot = true;
                        } else if *player == enemy_id && *by == ally_id {
                            out.ally_knocked = true;
                        }
                    } else if *player == p_id && *by == enemy_id {
                        out.robbed = true;
                    } else if *player == p_id && *by == ally_id {
                        out.ally_knocked = true;
                    }
                }
                _ => {}
            }
        }
        seen = events.len();
        // A teammate knocking its own carrier loose is the theft; an ally
        // that merely reaches loose gold first is ordinary play.
        out.ally_took = out.ally_knocked;
        if (human_robs && out.robbed_bot) || (!human_robs && (out.robbed || out.ally_took)) {
            break;
        }
        if !human_robs
            && app
                .world()
                .resource::<CnrHost>()
                .game
                .score(p_id)
                .unwrap_or(0)
                > 0
        {
            break;
        }
    }
    for (e, _) in &cars {
        if let Some(d) = app.world().get::<CnrBotDriver>(*e) {
            out.reanchors += d.reanchors;
        }
    }
    eprintln!("{city} seed {seed} {variant:?} human_robs={human_robs}: {out:?}");
    out
}

/// CNRBOT-E human-carrier legs on the retail streets, in each team
/// variant: a driven human carrier robbed by an enemy bot, and a human
/// robbing an enemy bot carrier; an ally bot never takes the human's
/// gold. Whether a theft happens in a given street layout is a property
/// of the map, so — as above — the test reports every run and asserts
/// only that a person at the wheel takes the gold by driving; it does
/// not claim a robbery in either direction.
#[test]
fn retail_human_carriers_are_robbed_by_enemy_bots_and_robbing_them() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; the human-carrier legs were NOT checked");
        return;
    };
    for city in ["sf", "london"] {
        let vfs = mount(&retail);
        let graph = load_graph(&vfs, city).unwrap();
        let seeds = lane_seeds(&vfs, city, &graph, SEEDS);
        let (mut robbed, mut robbed_bot, mut delivered_runs, mut ally_thefts) = (0, 0, 0, 0);
        for seed in seeds.into_iter().take(2) {
            for variant in [CnrVariant::CopsVsRobbers, CnrVariant::RobbersVsRobbers] {
                let a = run_human(&retail, city, seed, variant, false);
                let b = run_human(&retail, city, seed, variant, true);
                assert!(
                    !(a.ally_took || b.ally_took),
                    "{city} seed {seed} {variant:?}: an ally took the gold"
                );
                ally_thefts += usize::from(a.ally_took || b.ally_took);
                robbed += usize::from(a.robbed);
                robbed_bot += usize::from(b.robbed_bot);
                delivered_runs += usize::from(a.human_picked);
            }
        }
        eprintln!(
            "{city}: human picked up in {delivered_runs} runs, robbed by a bot in {robbed}, \
             robbed a bot in {robbed_bot}, \
             ally thefts {ally_thefts}"
        );
        assert!(delivered_runs > 0, "{city}: the person never took the gold");
    }
}

/// Task #2190: the scripted carrier that ended circling the sf upper
/// road 325 deck at (-1843, 49, -331), 24 m above the seed-1291 hideout.
/// Starts a carrier there (the gold is handed over through the host's
/// own pickup verdict, at the gold's site) and runs the production
/// `scripted_drive` with physics. Before the fix the planned line,
/// which ends 7.7 m (XZ) from where it starts, read as a closed lap,
/// the hideout counted as "behind" the car and the car aimed straight at
/// it off the deck, circling for the whole budget.
#[test]
fn a_scripted_carrier_leaves_the_sf_upper_deck_and_delivers_to_the_hideout() {
    use mm2_app::scripted::scripted_drive;
    use mm2_game::gold::Contact;
    use mm2_game::{Mm2Vfs, PlayerVehicle};

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let (mut app, _) = city_app(&retail, "sf", 1291, false, CnrVariant::FreeForAll);
    let vfs = mount(&retail);
    app.insert_resource(Mm2Vfs(vfs));
    app.add_systems(FixedUpdate, scripted_drive.before(cnr_bot_drive));
    let sites = app.world().resource::<CnrHost>().game.sites();
    let start = Vec3::new(-1843.0, 49.0, -331.0);
    // The failing run's heading is not recorded: face the hideout.
    let yaw = f32::atan2(-(sites.hideout.x - start.x), -(sites.hideout.z - start.z));
    let bots = spawn(
        &mut app,
        vec![BotSlot {
            position: start,
            yaw,
        }],
    );
    let (bot, id) = bots[0];
    app.world_mut()
        .entity_mut(bot)
        .remove::<CnrBotDriver>()
        .insert(PlayerVehicle);
    for _ in 0..3 {
        app.update();
    }
    let round = app.world().resource::<CnrHost>().game.round();
    let verdicts = app
        .world_mut()
        .resource_mut::<CnrHost>()
        .game
        .resolve_pickups(&[Contact {
            player: id,
            round,
            position: sites.gold,
        }]);
    eprintln!("pickup {verdicts:?}");
    let mut best = f32::MAX;
    for _ in 0..(HZ * 240) {
        app.update();
        let p = pos(&app, bot);
        best = best.min(p.distance(sites.hideout));
        if p.distance(sites.hideout) <= mm2_game::cnr_options::DELIVERY_RADIUS_M {
            break;
        }
    }
    assert!(
        best <= mm2_game::cnr_options::DELIVERY_RADIUS_M,
        "closest approach {best} m to {:?}",
        sites.hideout
    );
}

/// The `--bot-thief` wedge on the sf hill road near (-1212, 95, 149)
/// on seed 1291 (#2206), run as the scripted carrier so the same
/// production `scripted_drive` plans from the pose the logs recorded:
/// the car stands on the steep east-west road facing west, and the
/// road line to the hideout begins behind its nose. Before the fix the
/// freshly planned guide started at the first anchor the nose faced —
/// the north-south leg beyond a wall — and the car pinned itself
/// against the roadside for thousands of ticks, then fell to the next
/// wedge at (-1241, 86, 146); it now turns round to the line's own
/// start and is away within 40 s.
#[test]
fn a_scripted_driver_leaves_the_sf_hill_road_it_was_wedged_on() {
    use mm2_app::scripted::scripted_drive;
    use mm2_game::gold::Contact;
    use mm2_game::{Mm2Vfs, PlayerVehicle};

    /// Bound on the time to get 60 m from the wedge pose (the failing
    /// run was still within 40 m after 60 s).
    const LEAVE_SECONDS: u32 = 40;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let (mut app, _) = city_app(&retail, "sf", 1291, false, CnrVariant::FreeForAll);
    let vfs = mount(&retail);
    app.insert_resource(Mm2Vfs(vfs));
    app.add_systems(FixedUpdate, scripted_drive.before(cnr_bot_drive));
    let sites = app.world().resource::<CnrHost>().game.sites();
    // The pose and heading of the logged tick-5024 escape.
    let start = Vec3::new(-1212.0, 96.5, 148.7);
    let bots = spawn(
        &mut app,
        vec![BotSlot {
            position: start,
            yaw: 1.21,
        }],
    );
    let (bot, id) = bots[0];
    app.world_mut()
        .entity_mut(bot)
        .remove::<CnrBotDriver>()
        .insert(PlayerVehicle);
    for _ in 0..3 {
        app.update();
    }
    let round = app.world().resource::<CnrHost>().game.round();
    let verdicts = app
        .world_mut()
        .resource_mut::<CnrHost>()
        .game
        .resolve_pickups(&[Contact {
            player: id,
            round,
            position: sites.gold,
        }]);
    assert!(
        matches!(verdicts[0], mm2_game::gold::PickupVerdict::Granted { .. }),
        "{verdicts:?}"
    );
    let mut left = None;
    for tick in 0..(HZ * 240) {
        app.update();
        let p = pos(&app, bot);
        if left.is_none() && p.distance(start) > 60.0 {
            left = Some(tick);
            break;
        }
    }
    let ticks = left.expect("the car never got 60 m from the wedge pose");
    assert!(
        ticks <= HZ * LEAVE_SECONDS,
        "took {ticks} ticks to get 60 m from the wedge pose"
    );
}

/// The lossy Cops & Robbers wedge on the sf ground road under a bridge
/// near (-1480, 35, 905) on seed 49 (#2230), run as the scripted carrier.
/// The car stands on the ground 15 m below the bridge deck, which is the
/// nearest lane in 3D; the road line to the hideout used to start on that
/// deck, so the car drove at an aim 16 m over its roof and never moved
/// for thousands of ticks. Its line now starts on the ground lane under
/// it and it is away within 40 s.
#[test]
fn a_scripted_driver_under_the_sf_bridge_plans_from_its_own_road() {
    use mm2_app::scripted::scripted_drive;
    use mm2_game::gold::Contact;
    use mm2_game::{Mm2Vfs, PlayerVehicle};

    const LEAVE_SECONDS: u32 = 40;

    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let (mut app, _) = city_app(&retail, "sf", 49, false, CnrVariant::FreeForAll);
    let vfs = mount(&retail);
    app.insert_resource(Mm2Vfs(vfs));
    app.add_systems(FixedUpdate, scripted_drive.before(cnr_bot_drive));
    let sites = app.world().resource::<CnrHost>().game.sites();
    let start = Vec3::new(-1479.8, 35.3, 905.6);
    let bots = spawn(
        &mut app,
        vec![BotSlot {
            position: start,
            yaw: -2.957,
        }],
    );
    let (bot, id) = bots[0];
    app.world_mut()
        .entity_mut(bot)
        .remove::<CnrBotDriver>()
        .insert(PlayerVehicle);
    for _ in 0..3 {
        app.update();
    }
    let round = app.world().resource::<CnrHost>().game.round();
    let verdicts = app
        .world_mut()
        .resource_mut::<CnrHost>()
        .game
        .resolve_pickups(&[Contact {
            player: id,
            round,
            position: sites.gold,
        }]);
    assert!(
        matches!(verdicts[0], mm2_game::gold::PickupVerdict::Granted { .. }),
        "{verdicts:?}"
    );
    let left = (0..HZ * 240).find(|_| {
        app.update();
        pos(&app, bot).distance(start) > 60.0
    });
    let ticks = left.expect("the car never got 60 m from the wedge pose");
    assert!(
        ticks <= HZ * LEAVE_SECONDS,
        "took {ticks} ticks to get 60 m from the wedge pose"
    );
}
