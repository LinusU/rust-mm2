//! F22-A.6: the remaining HUD-2 instruments — the place indicator,
//! the laps record (`Ordered`) and the checkpoint list — bound to the
//! authored `digitac_*_half` glyph set.
//!
//! The unit legs bind the authored glyph set in a synthetic mount,
//! drive the cluster off real `RaceState`/`RaceProgress` entities, and
//! assert the composed values: live place off `live_order`, the lap
//! counter, cleared/armed gate states, the `FIN` entry's arm, the
//! `Complete`/stale releases and the `H` gate hiding the cluster while
//! the report keeps composing. The smoke leg runs the real
//! `headless_smoke` pipeline on a synthetic checkpoint event so the
//! `sta=` record field carries the production spawn.

use std::path::Path;

use avian3d::prelude::Position;
use bevy::ecs::system::RunSystemOnce;
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use mm2_app::hud::HudVisible;
use mm2_app::racestat::{
    FinishLabel, GATE_ARMED, GATE_CLEARED, GATE_PENDING, GateDigit, PairDigit, PairKind,
    RaceStatReport, RaceStats, STAT_GLYPH_COUNT, StatGlyph, StatRow, gate_slots, pair_slots,
    spawn_race_stats, update_race_stats,
};
use mm2_app::session::SelectedCar;
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, ParticipantState, Player,
    PlayerControl, PlayerId, PlayerVehicle, RaceDefinition, RacePhase, RaceProgress, RaceStart,
    RaceState, Session, SessionConfig, SessionEntity, SessionMode, SessionPhase, TargetSelection,
    despawn_session_entities,
};
use mm2_vehicle::VehicleConfig;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn stat_app() -> App {
    let mut app = App::new();
    app.init_resource::<Session>()
        .init_resource::<HudVisible>()
        .init_resource::<Assets<Image>>()
        .add_systems(Update, update_race_stats);
    app
}

/// A `Session` stood at `phase` — generation 1 after `begin`, the
/// same generation `RaceState::new(def, 1)` targets.
fn session_at(phase: SessionPhase) -> Session {
    use SessionPhase::*;
    let mut s = Session::new();
    if phase == Menu {
        return s;
    }
    s.begin(SessionConfig::default()).unwrap(); // Loading
    let path: &[SessionPhase] = match phase {
        Ready => &[Ready],
        Countdown => &[Ready, Countdown],
        Playing => &[Ready, Countdown, Playing],
        Paused => &[Ready, Countdown, Playing, Paused],
        Results => &[Ready, Countdown, Playing, Results],
        other => panic!("no session_at path to {other:?}"),
    };
    for step in path {
        s.transition(step.clone()).unwrap();
    }
    s
}

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// Any-order definition: three gates spread around the origin and a
/// separate finish trigger — `FIN` in the list, nearest-gate arming.
fn def_anyorder() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, -60.0), cp(100.0, 0.0), cp(0.0, 60.0)],
        finish: Some(cp(0.0, -200.0)),
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 360,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

/// Ordered (Circuit) definition: two gates, three laps — the laps
/// record the HUD-2 spec scopes to this rule.
fn def_ordered() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, -60.0), cp(0.0, 60.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 3,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 360,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

/// Insert a live race at `phase` against the app's session
/// generation — the same resource `load_session_world` inserts.
fn insert_race(app: &mut App, def: RaceDefinition, phase: RacePhase) {
    let generation = app.world().resource::<Session>().generation();
    let mut race = RaceState::new(def, generation);
    race.phase = phase;
    app.world_mut().insert_resource(race);
}

/// Spawn the local participant as a `PlayerVehicle` — the entity the
/// drive pass prefers — already released to `Racing` at `pos`.
fn spawn_driver(app: &mut App, def: &RaceDefinition, pos: Vec3) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let mut progress = RaceProgress::new(def);
    progress.state = ParticipantState::Racing;
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            PlayerVehicle,
            Player {
                id: PlayerId(0),
                control: PlayerControl::Local,
            },
            progress,
            TargetSelection::default(),
            Position(pos),
        ))
        .id()
}

/// An AI opponent — a participant the running order counts.
fn spawn_opponent(app: &mut App, def: &RaceDefinition, pos: Vec3) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let mut progress = RaceProgress::new(def);
    progress.state = ParticipantState::Racing;
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id: PlayerId(9),
                control: PlayerControl::Ai,
            },
            progress,
            Position(pos),
        ))
        .id()
}

/// Drive one swept segment through a checkpoint — `advance`'s real
/// crediting path, anchor step then crossing step.
fn cross(prog: &mut RaceProgress, def: &RaceDefinition, gate: usize) {
    let cp = &def.checkpoints[gate];
    let c = cp.center;
    prog.advance(def, c - Vec3::X * (cp.radius + 5.0));
    prog.advance(def, c + Vec3::X * (cp.radius + 5.0));
}

/// `spawn_race_stats` takes `&mut Commands` plus the image store —
/// `resource_scope` + a `CommandQueue`, the same dance
/// `tests/racetime.rs`/`tests/oppind.rs` use.
fn spawn_stats(app: &mut App, vfs: &Vfs, def: &RaceDefinition) {
    let w = app.world_mut();
    w.resource_scope(|w, mut images: Mut<Assets<Image>>| {
        let mut queue = CommandQueue::default();
        let report = {
            let mut commands = Commands::new(&mut queue, w);
            spawn_race_stats(&mut commands, vfs, &mut images, SessionEntity(1), def)
        };
        queue.apply(w);
        w.insert_resource(report);
    });
}

fn report(app: &App) -> &RaceStatReport {
    app.world().resource::<RaceStatReport>()
}

fn stats_visible(app: &mut App) -> bool {
    let mut q = app
        .world_mut()
        .query_filtered::<&Visibility, With<RaceStats>>();
    q.single(app.world())
        .is_ok_and(|v| *v == Visibility::Visible)
}

/// `Display` of the named pair row.
fn row_display(app: &mut App, want: StatRow) -> Display {
    let mut q = app
        .world_mut()
        .query_filtered::<(&StatRow, &Node), Without<PairDigit>>();
    q.iter(app.world())
        .find(|(row, _)| **row == want)
        .map(|(_, n)| n.display)
        .expect("the row exists")
}

/// The image one pair cell resolved to — `None` while collapsed.
fn pair_cell(app: &mut App, kind: PairKind, slot: usize) -> (Display, Handle<Image>) {
    let mut q = app
        .world_mut()
        .query_filtered::<(&PairDigit, &ImageNode, &Node), Without<GateDigit>>();
    q.iter(app.world())
        .find(|(c, _, _)| c.kind == kind && c.slot == slot)
        .map(|(_, i, n)| (n.display, i.image.clone()))
        .expect("the cell exists")
}

/// The tint one gate's cells carry (both slots share the state).
fn gate_tint(app: &mut App, gate: usize) -> Color {
    let mut q = app
        .world_mut()
        .query_filtered::<(&GateDigit, &ImageNode), Without<PairDigit>>();
    q.iter(app.world())
        .find(|(c, _)| c.gate == gate)
        .map(|(_, i)| i.color)
        .expect("the gate row exists")
}

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A minimal uncompressed 32bpp TGA — top-left origin, opaque fill.
/// `decode_buffer_image` routes it through Bevy's TGA loader like the
/// authored `digitac_*_half` files.
fn tga32(w: u16, h: u16) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2; // uncompressed true-colour
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28; // 8 alpha bits + top-left origin
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[0, 255, 0, 255]); // BGRA green
    }
    t
}

/// A mount carrying only the authored half-digit set the cluster binds.
fn glyph_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for d in 0..10 {
        write(
            tmp.path(),
            &format!("texture/digitac_{d}_half.tga"),
            tga32(20, 27),
        );
    }
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

// ---------------------------------------------------------------------------
// Slot composition — the designed `n/total` + index layout (DSN-54)
// ---------------------------------------------------------------------------

/// `n/total` pairs drop leading tens zeros and cap at 99 — the half
/// set gives each number two cells.
#[test]
fn pair_slots_compose() {
    use StatGlyph::*;
    assert_eq!(pair_slots(2, 7), [Off, Digit(2), Off, Digit(7)]);
    assert_eq!(pair_slots(12, 15), [Digit(1), Digit(2), Digit(1), Digit(5)]);
    // Over-two-digit values pin at 99 rather than wrap.
    assert_eq!(pair_slots(140, 3), [Digit(9), Digit(9), Off, Digit(3)]);
}

/// Gate indices list 1-based; the tens slot collapses under 10.
#[test]
fn gate_slots_compose() {
    use StatGlyph::*;
    assert_eq!(gate_slots(0), [Off, Digit(1)]);
    assert_eq!(gate_slots(9), [Digit(1), Digit(0)]);
}

// ---------------------------------------------------------------------------
// Binding: authored content through the VFS
// ---------------------------------------------------------------------------

/// The authored half set binds ten glyphs and spawns the
/// session-owned cluster: a `PLACE` row, one row per authored gate,
/// and the `FIN` entry for an AnyOrder definition with a finish.
#[test]
fn full_glyph_set_binds_and_spawns() {
    let mut app = stat_app();
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);

    assert_eq!(report(&app).glyphs, STAT_GLYPH_COUNT);
    assert_eq!(report(&app).absent, None);
    let mut roots = app
        .world_mut()
        .query_filtered::<(&Visibility, &SessionEntity), With<RaceStats>>();
    let (vis, owner) = roots.single(app.world()).unwrap();
    assert_eq!(*vis, Visibility::Hidden, "idle until a live race");
    assert_eq!(owner.0, 1, "session teardown owns the cluster");

    // One two-cell row per authored gate, plus the finish entry.
    let mut gates = app.world_mut().query_filtered::<(), With<GateDigit>>();
    assert_eq!(gates.iter(app.world()).count(), 2 * 3);
    let mut fin = app.world_mut().query_filtered::<(), With<FinishLabel>>();
    assert_eq!(fin.iter(app.world()).count(), 1);
    // AnyOrder → no laps row (HUD-2 scopes it to Circuit).
    assert!(
        app.world_mut()
            .query_filtered::<&StatRow, Without<PairDigit>>()
            .iter(app.world())
            .all(|r| *r != StatRow::Lap),
        "no laps row under AnyOrder"
    );
}

/// An `Ordered` definition spawns the `LAP` row too, and drops `FIN`
/// (Ordered ignores `finish`).
#[test]
fn ordered_definition_spawns_the_lap_row() {
    let mut app = stat_app();
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let mut def = def_ordered();
    def.finish = Some(cp(0.0, -200.0)); // inert under Ordered
    spawn_stats(&mut app, &vfs, &def);
    assert_eq!(report(&app).glyphs, STAT_GLYPH_COUNT);
    let rows: Vec<StatRow> = app
        .world_mut()
        .query_filtered::<&StatRow, Without<PairDigit>>()
        .iter(app.world())
        .copied()
        .collect();
    assert!(rows.contains(&StatRow::Lap));
    assert!(rows.contains(&StatRow::Place));
    let mut fin = app.world_mut().query_filtered::<(), With<FinishLabel>>();
    assert_eq!(fin.iter(app.world()).count(), 0, "Ordered ignores finish");
}

/// Missing artwork binds nothing and says why — `absent:missing-glyphs`,
/// never a substitute glyph.
#[test]
fn missing_glyphs_report_absent() {
    let mut app = stat_app();
    let tmp = tempfile::tempdir().unwrap();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    assert_eq!(report(&app).absent, Some("missing-glyphs"));
    assert_eq!(report(&app).glyphs, 0);
    assert_eq!(report(&app).smoke_detail(), "absent:missing-glyphs");
    let mut roots = app.world_mut().query_filtered::<(), With<RaceStats>>();
    assert_eq!(roots.iter(app.world()).count(), 0, "no half-bound cluster");
}

/// A partial set aborts the whole instrument — the report counts how
/// far the load got.
#[test]
fn partial_glyph_set_reports_absent() {
    let mut app = stat_app();
    let tmp = tempfile::tempdir().unwrap();
    // Everything but `digitac_9_half`.
    for d in 0..9 {
        write(
            tmp.path(),
            &format!("texture/digitac_{d}_half.tga"),
            tga32(20, 27),
        );
    }
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    assert_eq!(report(&app).absent, Some("missing-glyphs"));
    assert_eq!(report(&app).glyphs, 9);
    let mut roots = app.world_mut().query_filtered::<(), With<RaceStats>>();
    assert_eq!(roots.iter(app.world()).count(), 0);
}

// ---------------------------------------------------------------------------
// Driving: authoritative race state → the cluster
// ---------------------------------------------------------------------------

/// A live race composes the place indicator off `live_order` — a
/// two-participant field where the opponent has cleared a gate places
/// the local driver second — and the checkpoint list's demand.
#[test]
fn live_race_composes_place_and_checkpoint_list() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    let opp = spawn_opponent(&mut app, &def, Vec3::new(0.0, 0.0, -40.0));
    // The opponent clears gate 0 through the real swept-segment path.
    {
        let mut p = app.world_mut().get_mut::<RaceProgress>(opp).unwrap();
        cross(&mut p, &def, 0);
    }
    app.update();

    assert!(stats_visible(&mut app));
    assert_eq!(report(&app).place, Some((2, 2)));
    assert_eq!(report(&app).checkpoints, Some((0, 3)));
    assert_eq!(report(&app).lap, None, "AnyOrder has no laps record");
    assert_eq!(row_display(&mut app, StatRow::Place), Display::Flex);

    // The place cells show the authored digits: `2/2`.
    let mut roots = app
        .world_mut()
        .query_filtered::<&mm2_app::racestat::StatDigits, With<RaceStats>>();
    let digits = roots.single(app.world()).unwrap().half.clone();
    let (disp, img) = pair_cell(&mut app, PairKind::Place, 1);
    assert_eq!(disp, Display::Flex);
    assert_eq!(img, digits[2], "place units = authored 2");
    let (disp, img) = pair_cell(&mut app, PairKind::Place, 3);
    assert_eq!(disp, Display::Flex);
    assert_eq!(img, digits[2], "field units = authored 2");
    // Leading tens collapse — `2/2` takes no tens space.
    assert_eq!(pair_cell(&mut app, PairKind::Place, 0).0, Display::None);
}

/// A solo participant (cruise-style event field of one) shows no
/// place — DSN-13's `pos=` contract — while the list still composes.
#[test]
fn solo_field_hides_the_place_row() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    app.update();

    assert!(stats_visible(&mut app));
    assert_eq!(report(&app).place, None);
    assert_eq!(row_display(&mut app, StatRow::Place), Display::None);
    assert_eq!(report(&app).checkpoints, Some((0, 3)));
    assert!(
        report(&app).smoke_detail().starts_with("10g/-/"),
        "place reads `-`: {}",
        report(&app).smoke_detail()
    );
}

/// The checkpoint list marks the authored gate states: the cleared
/// gate dims, the armed objective (the arrow's target — nearest
/// remaining) lights, the rest stay pending.
#[test]
fn checkpoint_list_marks_cleared_and_armed() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    let car = spawn_driver(&mut app, &def, Vec3::ZERO);
    {
        // Clear gate 0 through the real swept-segment credit.
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        cross(&mut p, &def, 0);
    }
    app.update();

    assert_eq!(report(&app).checkpoints, Some((1, 3)));
    assert_eq!(gate_tint(&mut app, 0), GATE_CLEARED);
    // Nearest remaining gate to the origin is gate 2 at (0, 60) —
    // the same target the nav arrow picks.
    assert_eq!(gate_tint(&mut app, 2), GATE_ARMED);
    assert_eq!(gate_tint(&mut app, 1), GATE_PENDING);
}

/// Every gate cleared arms the `FIN` entry — the authored finish
/// trigger becomes the objective (RACE-7).
#[test]
fn finish_entry_arms_when_all_gates_clear() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    let car = spawn_driver(&mut app, &def, Vec3::ZERO);
    {
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        for gate in 0..def.checkpoints.len() {
            cross(&mut p, &def, gate);
        }
    }
    app.update();

    assert_eq!(report(&app).checkpoints, Some((3, 3)));
    for gate in 0..3 {
        assert_eq!(gate_tint(&mut app, gate), GATE_CLEARED);
    }
    let mut q = app
        .world_mut()
        .query_filtered::<&TextColor, With<FinishLabel>>();
    assert_eq!(*q.single(app.world()).unwrap(), TextColor(GATE_ARMED));
    // The driver sits at gate 2's far side — the finish is the target,
    // so `TargetSelection` stays unpicked and the list shows FIN lit.
}

/// Under `Ordered` the laps record composes `current/total`, the next
/// required gate arms, and a resolved participant parks at
/// `laps/laps`.
#[test]
fn ordered_race_composes_lap_and_next_gate() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_ordered();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    let car = spawn_driver(&mut app, &def, Vec3::ZERO);
    {
        // Clear gate 0 through the real swept-segment credit.
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        cross(&mut p, &def, 0);
    }
    app.update();

    assert_eq!(report(&app).lap, Some((1, 3)), "lap 1 of 3 in progress");
    assert_eq!(report(&app).checkpoints, Some((1, 2)));
    assert_eq!(row_display(&mut app, StatRow::Lap), Display::Flex);
    assert_eq!(gate_tint(&mut app, 0), GATE_CLEARED);
    assert_eq!(gate_tint(&mut app, 1), GATE_ARMED, "the next required gate");

    // A resolved participant parks on `laps/laps`, never `laps+1`.
    {
        let mut p = app.world_mut().get_mut::<RaceProgress>(car).unwrap();
        p.lap = def.laps;
        p.state = ParticipantState::Finished {
            race_ticks: 100,
            result: mm2_game::ResultId {
                generation: 1,
                participant: PlayerId(0),
                event: None,
                sequence: 0,
            },
        };
    }
    app.update();
    assert_eq!(report(&app).lap, Some((3, 3)), "resolved → laps/laps");
}

/// `Complete` and stale races release the cluster — every instrument
/// records `-` rather than a frozen or foreign standing.
#[test]
fn complete_and_stale_races_hide_the_cluster() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    spawn_opponent(&mut app, &def, Vec3::new(10.0, 0.0, 0.0));
    app.update();
    assert!(stats_visible(&mut app));
    assert!(report(&app).place.is_some());

    app.world_mut().resource_mut::<RaceState>().phase = RacePhase::Complete;
    app.update();
    assert!(!stats_visible(&mut app));
    assert_eq!(report(&app).place, None);
    assert_eq!(report(&app).checkpoints, None);
    assert_eq!(report(&app).smoke_detail(), "10g/-/-/-");

    // A stale resource — a restart generation the teardown has not
    // swept yet — never shows.
    insert_race(&mut app, def.clone(), RacePhase::Running);
    app.world_mut().resource_mut::<RaceState>().generation = 99;
    app.update();
    assert!(!stats_visible(&mut app));
    assert_eq!(report(&app).place, None);
}

/// No race at all — the cruise case — keeps the cluster dark.
#[test]
fn cruise_session_shows_no_stats() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    app.update();
    assert!(!stats_visible(&mut app));
    assert_eq!(report(&app).place, None);
    assert_eq!(report(&app).checkpoints, None);
}

/// The `H` gate hides the cluster with the rest of the HUD layer
/// while the report keeps composing — `sta=` still reads live values.
#[test]
fn h_gate_hides_the_cluster_but_keeps_composing() {
    let mut app = stat_app();
    app.world_mut()
        .insert_resource(session_at(SessionPhase::Playing));
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    insert_race(&mut app, def.clone(), RacePhase::Running);
    spawn_driver(&mut app, &def, Vec3::ZERO);
    spawn_opponent(&mut app, &def, Vec3::new(10.0, 0.0, 0.0));

    app.world_mut().resource_mut::<HudVisible>().0 = false;
    app.update();
    assert!(!stats_visible(&mut app));
    assert_eq!(report(&app).place, Some((1, 2)));
    assert_eq!(report(&app).checkpoints, Some((0, 3)));

    app.world_mut().resource_mut::<HudVisible>().0 = true;
    app.update();
    assert!(stats_visible(&mut app));
}

/// Session teardown owns the cluster: `despawn_session_entities`
/// drops the stamped root and the whole subtree with it — a restart
/// re-spawns a fresh one rather than reusing stale cells.
#[test]
fn teardown_despawns_the_cluster() {
    let mut app = stat_app();
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    let def = def_anyorder();
    spawn_stats(&mut app, &vfs, &def);
    assert_eq!(
        app.world_mut()
            .query_filtered::<(), With<RaceStats>>()
            .iter(app.world())
            .count(),
        1
    );
    app.world_mut()
        .run_system_once(despawn_session_entities)
        .unwrap();
    assert_eq!(
        app.world_mut()
            .query_filtered::<(), With<RaceStats>>()
            .iter(app.world())
            .count(),
        0,
        "the session-owned root dies with the session"
    );
    assert_eq!(
        app.world_mut()
            .query_filtered::<(), With<GateDigit>>()
            .iter(app.world())
            .count(),
        0,
        "the whole subtree despawns with its root"
    );
}

// ---------------------------------------------------------------------------
// Smoke record: the production `headless_smoke` pipeline reports `sta=`
// ---------------------------------------------------------------------------

const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";
const OPP_HEADER: &str =
    "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n";

fn waypoint_row(x: f32, z: f32) -> String {
    format!("{x},0,{z},0,15,0,0,0,\n")
}

/// Minimal `vehCarSim` tune — the same shape `tests/oppind.rs` writes,
/// one opponent car (`vpt`).
fn vehcarsim() -> String {
    let wheel = |name: &str| {
        format!(
            "  {name} {{\n    SuspensionExtent 0.2\n    SuspensionLimit 0.05\n    SuspensionFactor 1.0\n    SuspensionDampCoef 0.1\n    SteeringLimit 0.5\n    BrakeCoef 0.14\n    TireDispLimitLong 0.075\n    TireDampCoefLong 0.75\n    TireDragCoefLong 0.01\n    TireDispLimitLat 0.075\n    TireDampCoefLat 0.75\n    TireDragCoefLat 0.02\n    OptimumSlipPercent 0.05\n    StaticFric 3.0\n    SlidingFric 2.95\n  }}\n"
        )
    };
    format!(
        "type: a\nvehCarSim {{\n  Mass 1000\n  InertiaBox 2.0 1.3 3.0\n  DrivetrainType 0\n  Aero {{\n    Drag 0.5\n    Down 0.0\n  }}\n  Engine {{\n    MaxHorsePower 200.0\n    IdleRPM 750.0\n    OptRPM 5800.0\n    MaxRPM 8500.0\n  }}\n  Trans {{\n    AutoNumGears 4\n    Reverse 20.0\n    Low 20.0\n    High 75.0\n  }}\n{}{}}}\n",
        wheel("WheelFront"),
        wheel("WheelBack"),
    )
}

/// One quad geometry chunk — the same shape `tests/oppind.rs` writes.
fn quad_geo(c: [f32; 3], hx: f32, hy: f32, hz: f32) -> Vec<u8> {
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    geo.extend_from_slice(&6u32.to_le_bytes());
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&0x112u32.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&(-1i32).to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&4u32.to_le_bytes());
    for p in [
        [c[0] - hx, c[1] - hy, c[2] - hz],
        [c[0] + hx, c[1] - hy, c[2] + hz],
        [c[0] + hx, c[1] + hy, c[2] - hz],
        [c[0] - hx, c[1] + hy, c[2] + hz],
    ] {
        for v in p {
            geo.extend_from_slice(&v.to_le_bytes());
        }
        for n in [0.0f32, 1.0, 0.0] {
            geo.extend_from_slice(&n.to_le_bytes());
        }
        for uv in [0.0f32, 0.0] {
            geo.extend_from_slice(&uv.to_le_bytes());
        }
    }
    geo.extend_from_slice(&6u32.to_le_bytes());
    for i in [0u16, 1, 2, 0, 3, 1] {
        geo.extend_from_slice(&i.to_le_bytes());
    }
    geo
}

fn car_pkg() -> Vec<u8> {
    let mut d = b"PKG3".to_vec();
    for (name, geo) in [
        ("body_h", quad_geo([0.0, 0.5, 0.0], 0.9, 0.5, 1.6)),
        ("whl0_h", quad_geo([0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl1_h", quad_geo([-0.8, 0.3, -1.3], 0.15, 0.3, 0.15)),
        ("whl2_h", quad_geo([0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
        ("whl3_h", quad_geo([-0.8, 0.3, 1.3], 0.15, 0.3, 0.15)),
    ] {
        d.extend_from_slice(b"FILE");
        d.push(name.len() as u8 + 1);
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        d.extend_from_slice(&(geo.len() as u32).to_le_bytes());
        d.extend_from_slice(&geo);
    }
    d
}

fn car_bnd() -> String {
    let mut s = "version: 1.01\nverts: 8\nmaterials: 1\nedges: 0\npolys: 6\n\n".to_string();
    for v in [
        [-0.9f32, 0.05, -1.6],
        [0.9, 0.05, -1.6],
        [0.9, 0.9, -1.6],
        [-0.9, 0.9, -1.6],
        [-0.9, 0.05, 1.6],
        [0.9, 0.05, 1.6],
        [0.9, 0.9, 1.6],
        [-0.9, 0.9, 1.6],
    ] {
        s.push_str(&format!("v {} {} {}\n", v[0], v[1], v[2]));
    }
    s.push_str("mtl default {\n  elasticity: 0.1\n  friction: 0.5\n}\n");
    for quad in [
        [0, 4, 5, 1],
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
    ] {
        s.push_str(&format!(
            "quad {} {} {} {} 0\n",
            quad[0], quad[1], quad[2], quad[3]
        ));
    }
    s
}

fn opp_file(points: &[[f32; 3]]) -> String {
    let mut s = OPP_HEADER.to_string();
    for p in points {
        s.push_str(&format!("{},{},{},0,0,0,0,0,0\n", p[0], p[1], p[2]));
    }
    s
}

/// The `race/testcity/` checkpoint event wiring one `vpt` opponent —
/// the event half of the install, shared by the bound and absent
/// legs.
fn write_event_files(d: &Path) {
    write(
        d,
        "race/testcity/mmracedata.csv",
        format!("{MM_HEADER}\nnone,0,0,0,1,0,0.1,0.0,1,50,1,0,0,0,1,0,0.2,0.0,1,40,1\n"),
    );
    write(
        d,
        "race/testcity/race0.aimap",
        "[Opponent]\n1\nvpt race0-a-0.opp 0.90 0 50.0 0.7 1 1 1 1 0 1.0\n",
    );
    write(
        d,
        "race/testcity/race0waypoints.csv",
        format!(
            "{WAYPOINTS}{}{}{}{}{}",
            waypoint_row(60.0, 140.0),
            waypoint_row(110.0, 140.0),
            waypoint_row(140.0, 140.0),
            waypoint_row(165.0, 140.0),
            waypoint_row(180.0, 140.0),
        ),
    );
    write(
        d,
        "race/testcity/race0-a-0.opp",
        opp_file(&[
            [70.0, 0.0, 140.0],
            [110.0, 0.0, 140.0],
            [140.0, 0.0, 140.0],
            [165.0, 0.0, 140.0],
            [180.0, 0.0, 140.0],
        ]),
    );
    write(d, "tune/vehicle/vpt.vehcarsim", vehcarsim());
    write(d, "geometry/vpt.pkg", car_pkg());
    write(d, "bound/vpt_bound.bnd", car_bnd());
}

/// The event plus the authored glyph set — the smallest install where
/// `sta=` reports a bound cluster.
fn stat_install() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_event_files(tmp.path());
    for d in 0..10 {
        write(
            tmp.path(),
            &format!("texture/digitac_{d}_half.tga"),
            tga32(20, 27),
        );
    }
    tmp
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "testcity".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The event session binds the authored half-digit set through the
/// production pipeline: the `sta=` field reads `10g/…` — the full set
/// bound, and a live composed demand off the authoritative race.
#[test]
fn event_session_records_the_bound_stats() {
    let tmp = stat_install();
    let rec = smoke::headless_smoke(
        &event_config(),
        vfs_of(tmp.path()),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        240,
        smoke::Driver::Parked,
        None,
    );
    let line = rec.line();
    assert_eq!(rec.status, SmokeStatus::Pass, "event smoke: {line}");
    assert!(
        line.contains(" sta=10g/"),
        "the authored glyph set binds the cluster: {line}"
    );
    assert!(
        !line.contains("sta=absent"),
        "a complete install never reports absent: {line}"
    );
    // The checkpoint instrument composes off the live race even for a
    // parked driver.
    assert!(
        line.contains("/c") || line.contains("sta=10g/-/"),
        "the list composes or reads idle: {line}"
    );
}

/// An event session on a mount without the authored textures reports
/// `absent` honestly — never substitute art.
#[test]
fn event_session_without_glyphs_reports_absent() {
    let tmp = tempfile::tempdir().unwrap();
    // The event without the glyph set.
    write_event_files(tmp.path());
    let rec = smoke::headless_smoke(
        &event_config(),
        vfs_of(tmp.path()),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    let line = rec.line();
    assert_eq!(rec.status, SmokeStatus::Pass, "event smoke: {line}");
    assert!(
        line.contains(" sta=absent:missing-glyphs"),
        "no authored art → absent, not a substitute: {line}"
    );
}

/// The dev world never runs the event arm — its record carries no
/// `sta=` field at all.
#[test]
fn dev_world_has_no_stats_field() {
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(rec.status, SmokeStatus::Pass);
    assert!(
        !rec.line().contains(" sta="),
        "no event → no stats report: {}",
        rec.line()
    );
}
