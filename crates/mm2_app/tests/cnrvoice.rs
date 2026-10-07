//! F27-B.4c commentary: the Cops & Robbers announcer, driven through the
//! production `cnr_commentary_voices` system on a synthetic install that
//! has the retail table's shape (`aud/spchdata/cnrsf.csv`, one wave pool
//! per speaker). Headless: no output device, so a spawned voice and its
//! stem are the evidence, not audible sound.
//!
//! `MM2_RETAIL`-gated at the bottom: every family of both city tables
//! resolves every wave its window names on the operator's install.

use std::path::Path;
use std::time::Duration;

use bevy::audio::PlaybackMode;
use bevy::audio::PlaybackSettings;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::audio::{AudioReport, AudioVoice, CommentaryVoice, PcmAudio, VoiceKind, WaveBank};
use mm2_app::cnr::{CnrEvent, CnrHost};
use mm2_app::cnrnet::CnrReplica;
use mm2_app::cnrvoice::{CnrCommentary, MAX_PENDING_CALLS};
use mm2_assets::Vfs;
use mm2_formats::spchdata::CueTable;
use mm2_game::gold::{
    CarrierLoad, CnrVariant, Contact, DropCause, EndRule, GoldEvent, GoldMatch, GoldRules, Side,
};
use mm2_game::{Mm2Vfs, ObjectId, PlayerId, Session, SessionConfig, SessionEntity, SessionPhase};

const A: PlayerId = PlayerId(1);
const B: PlayerId = PlayerId(2);

/// The retail `cnrsf.csv`, verbatim in shape (rows `prefix,end,add`).
const TABLE: &str = "Name prefix/type header,end sufix value,sufix add value\n\
BLUETEAMHASGOLD header,,\nAS1\\AS1ROBROB ,10,9\n\
REDTEAMHASGOLD header,,\nAS1\\AS1ROBROB ,5,4\n\
BLUETEAMSTASHEDGOLD header,,\nAS1\\AS1ROBROB ,4,3\n\
REDTEAMDROPPEDGOLD header,,\nAS1\\AS1ROBROB ,6,5\n\
REDTEAMSTASHEDGOLD header,,\nAS1\\AS1ROBROB ,9,8\n\
BLUETEAMDROPPEDGOLD header,,\nAS1\\AS1ROBROB ,3,2\n\
ROBGETLOOT header,,\nAS1\\AS1COPS,3,0\n\
ROBDROPLOOT header,,\nAS1\\AS1COPS,6,5\n\
ROBSTASHLOOT header,,\nAS1\\AS1COPS,10,9\n\
ROBRECOVERLOOT header,,\nAS1\\AS1COPS,9,8\n\
COPGETLOOT header,,\nAS1\\AS1COPS,9,8\n\
COPDROPLOOT header,,\nAS1\\AS1COPS,5,4\n\
COPSTASHLOOT header,,\nAS1\\AS1COPS,4,3\n\
COPRECOVERLOOT header,,\nAS1\\AS1COPS,11,10\n";

fn pcm_wav(rate: u32, frames: usize) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 2).to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let pcm = vec![0x20u8; frames * 2];
    let mut body = Vec::from(&b"WAVE"[..]);
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    body.extend_from_slice(&pcm);
    let mut out = Vec::from(&b"RIFF"[..]);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn write(root: &Path, logical: &str, bytes: &[u8]) {
    let path = root.join(logical);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// The table plus the shipped pool: `as1cops01`–`11`, `as1robrob01`–`10`,
/// each `secs` long. `skip` names stems left out.
fn install(secs: f32, skip: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "aud/spchdata/cnrsf.csv", TABLE.as_bytes());
    let frames = (22050.0 * secs) as usize;
    for (stem, last) in [("as1cops", 11), ("as1robrob", 10)] {
        for n in 1..=last {
            let name = format!("{stem}{n:02}");
            if skip.contains(&name.as_str()) {
                continue;
            }
            write(
                tmp.path(),
                &format!("aud/aud11/as1/{name}.11k.wav"),
                &pcm_wav(22050, frames),
            );
        }
    }
    tmp
}

fn rules() -> GoldRules {
    GoldRules {
        variant: CnrVariant::CopsVsRobbers,
        end: EndRule::None,
        load: CarrierLoad {
            added_mass_kg: 250.0,
            handling_scalar: 0.9,
        },
        pickup_points: 25,
        delivery_points: 100,
        pickup_radius: 5.0,
        delivery_radius: 12.0,
        drop_lockout_ticks: 120,
    }
}

fn game() -> GoldMatch {
    let pool = (0..8)
        .map(|i| Vec3::new(i as f32 * 100.0, 0.0, (i * i) as f32 * 7.0))
        .collect();
    GoldMatch::new(
        1,
        ObjectId {
            generation: 1,
            slot: 9,
        },
        rules(),
        pool,
        5,
        &[(A, Side::Robbers), (B, Side::Cops)],
    )
    .unwrap()
}

/// A Playing session, the install's wave bank, the announcer bound from
/// the install's table and fixed 1/60 s updates.
fn app(dir: &Path, phase: SessionPhase) -> App {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    let bank = WaveBank::index(&vfs);
    let voice = CnrCommentary::load(&vfs, "sf", 7).expect("the install ships the table");
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    if phase == SessionPhase::Paused {
        session.transition(SessionPhase::Paused).unwrap();
    }
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(bank)
        .insert_resource(voice)
        .init_resource::<Assets<PcmAudio>>()
        .init_resource::<AudioReport>()
        .add_message::<CnrEvent>()
        .add_systems(Update, mm2_app::cnrvoice::cnr_commentary_voices);
    app.finish();
    app.cleanup();
    app
}

fn host(app: &mut App) {
    app.insert_resource(CnrHost::new(game()));
}

fn emit(app: &mut App, event: GoldEvent) {
    app.world_mut()
        .resource_mut::<Messages<CnrEvent>>()
        .write(CnrEvent(event));
}

fn picked(player: PlayerId, side: Side, recovered: bool) -> GoldEvent {
    GoldEvent::Picked {
        player,
        side,
        recovered,
        points: 25,
        round: 0,
    }
}

fn dropped(player: PlayerId) -> GoldEvent {
    GoldEvent::Dropped {
        player,
        at: Vec3::ZERO,
        cause: DropCause::Destroyed,
        round: 0,
    }
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

fn stems(app: &mut App) -> Vec<String> {
    let mut v: Vec<String> = app
        .world_mut()
        .query::<(&CommentaryVoice, &AudioVoice)>()
        .iter(app.world())
        .inspect(|(_, v)| assert_eq!(v.kind, VoiceKind::Commentary))
        .map(|(c, _)| c.stem.clone())
        .collect();
    v.sort();
    v
}

/// The host voices the original's own family for each event, drawn from
/// the wave window its row names — `ROBDROPLOOT 6,5` is the single wave
/// `as1cops06`, `COPSTASHLOOT 4,3` is `as1cops04` — as session-stamped
/// despawn one-shots.
#[test]
fn the_host_voices_each_event_from_its_families_own_waves() {
    let dir = install(0.1, &[]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    host(&mut app);
    // A robber drops, a cop recovers, the cops stash: three lines, in order.
    emit(&mut app, dropped(A));
    run(&mut app, 30);
    emit(&mut app, picked(B, Side::Cops, true));
    run(&mut app, 30);
    emit(
        &mut app,
        GoldEvent::Delivered {
            player: B,
            side: Side::Cops,
            points: 100,
            round: 0,
        },
    );
    run(&mut app, 30);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed, r.dropped), (3, 0, 0));
    // ROBDROPLOOT, COPRECOVERLOOT, COPSTASHLOOT: one wave each.
    assert_eq!(stems(&mut app), ["as1cops04", "as1cops06", "as1cops11"]);
    let generation = app.world().resource::<Session>().generation();
    let mut q = app
        .world_mut()
        .query::<(&SessionEntity, &PlaybackSettings)>();
    for (stamp, settings) in q.iter(app.world()) {
        assert_eq!(stamp.0, generation);
        assert!(matches!(settings.mode, PlaybackMode::Despawn));
    }
}

/// A family with a window of several waves draws only inside it — and
/// replays the same line for the same seed.
#[test]
fn a_wide_window_draws_inside_itself_and_replays_for_a_seed() {
    let pick = || {
        let dir = install(0.1, &[]);
        let mut app = app(dir.path(), SessionPhase::Playing);
        host(&mut app);
        emit(&mut app, picked(A, Side::Robbers, false));
        run(&mut app, 10);
        stems(&mut app)
    };
    let first = pick();
    assert_eq!(first, pick());
    assert!(
        ["as1cops01", "as1cops02", "as1cops03"].contains(&first[0].as_str()),
        "ROBGETLOOT 3,0 names waves 1..=3: {first:?}"
    );
}

/// Events that carry no announcement (a new round, a roster change)
/// say nothing, and a dropper the match does not know has no side to
/// call it for.
#[test]
fn events_with_nothing_to_announce_stay_silent() {
    let dir = install(0.1, &[]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    host(&mut app);
    emit(&mut app, GoldEvent::Left { player: A });
    emit(&mut app, dropped(PlayerId(9)));
    run(&mut app, 30);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed, r.dropped), (0, 0, 0));
    assert!(stems(&mut app).is_empty());
}

/// A burst keeps the first lines up to the bound and counts the rest
/// dropped, rather than narrating the past.
#[test]
fn a_burst_keeps_a_bounded_queue_and_counts_the_overflow() {
    let dir = install(0.1, &[]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    host(&mut app);
    for _ in 0..MAX_PENDING_CALLS + 2 {
        emit(&mut app, picked(A, Side::Robbers, false));
    }
    app.update();
    let c = app.world().resource::<CnrCommentary>();
    // One line started this frame; the bound held for the rest.
    assert_eq!(c.pending(), MAX_PENDING_CALLS - 1);
    assert_eq!(app.world().resource::<AudioReport>().dropped, 2);
}

/// A line that waits behind a long clip past the stale bound is dropped,
/// counted, not spoken late.
#[test]
fn a_line_overtaken_by_play_is_dropped_not_spoken_late() {
    let dir = install(4.0, &[]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    host(&mut app);
    for _ in 0..3 {
        emit(&mut app, picked(A, Side::Robbers, false));
    }
    // 4 s clips: the second starts at ~4.25 s, the third would at ~8.5 s,
    // after it passed the 6 s bound.
    run(&mut app, 60 * 12);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.dropped, r.failed), (2, 1, 0));
}

/// A wave the install does not ship is one counted failure and silence —
/// no substitute line.
#[test]
fn a_missing_wave_counts_failed_and_plays_nothing() {
    // `ROBDROPLOOT 6,5` can only name `as1cops06`.
    let dir = install(0.1, &["as1cops06"]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    host(&mut app);
    emit(&mut app, dropped(A));
    run(&mut app, 30);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed), (0, 1));
    assert!(stems(&mut app).is_empty());
}

/// A pause speaks nothing and keeps nothing: an event in a paused frame
/// is not heard after resuming.
#[test]
fn a_paused_session_speaks_nothing() {
    let dir = install(0.1, &[]);
    let mut app = app(dir.path(), SessionPhase::Paused);
    host(&mut app);
    emit(&mut app, picked(A, Side::Robbers, false));
    run(&mut app, 30);
    assert_eq!(app.world().resource::<AudioReport>().commentary, 0);
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 30);
    assert_eq!(app.world().resource::<AudioReport>().commentary, 0);
}

/// A joined client has no match, only the replica: its first frame is a
/// baseline (silent), and each later change voices the line the
/// authority's events earned — the pickup, the drop and the stash.
#[test]
fn a_client_voices_the_changes_between_replica_frames() {
    let dir = install(0.1, &[]);
    let mut app = app(dir.path(), SessionPhase::Playing);
    let mut m = game();
    app.insert_resource(CnrReplica(m.view()));
    run(&mut app, 10);
    assert_eq!(
        app.world().resource::<AudioReport>().commentary,
        0,
        "the first frame is a baseline, not news"
    );
    let position = m.gold_position().unwrap();
    let contact = Contact {
        player: A,
        round: m.round(),
        position,
    };
    m.resolve_pickups(&[contact]);
    app.insert_resource(CnrReplica(m.view()));
    run(&mut app, 30);
    m.dislodge(A, position, DropCause::Destroyed).unwrap();
    app.insert_resource(CnrReplica(m.view()));
    run(&mut app, 30);
    // The same frame again says nothing more.
    app.insert_resource(CnrReplica(m.view()));
    run(&mut app, 30);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.failed, r.dropped), (2, 0, 0));
    let heard = stems(&mut app);
    // ROBGETLOOT (waves 1..=3) then ROBDROPLOOT (wave 6).
    assert!(heard.contains(&"as1cops06".to_string()), "{heard:?}");
    assert!(
        heard
            .iter()
            .any(|s| ["as1cops01", "as1cops02", "as1cops03"].contains(&s.as_str())),
        "{heard:?}"
    );
}

/// A city that ships no table binds no announcer — nothing to voice
/// with, nothing invented.
#[test]
fn a_city_without_the_table_binds_no_announcer() {
    let tmp = tempfile::tempdir().unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(tmp.path(), 0).unwrap();
    assert!(CnrCommentary::load(&vfs, "sf", 7).is_none());
}

/// Retail: every family of both city tables names only waves the install
/// ships, across the whole `add + 1 ..= end` window — the evidence the
/// window reading (ledger AUD-12) rests on. Skipped without the
/// operator's install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_every_family_window_names_only_shipped_waves() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let mut bank = WaveBank::index(&vfs);
    let mut waves = Assets::<PcmAudio>::default();
    let mut checked = 0;
    for city in ["sf", "london"] {
        let path = mm2_content::cnr::CnrContent::commentary_path(city);
        let table =
            CueTable::parse(&String::from_utf8_lossy(&vfs.read_logical(&path).unwrap())).unwrap();
        for cue in mm2_content::cnr::COMMENTARY_CUES {
            let row = &table.section(cue).unwrap().rows[0];
            assert!(row.add < row.end, "{city} {cue}: an empty window");
            for n in row.add + 1..=row.end {
                let stem = mm2_game::cue_wave_stem("", &row.prefix, n);
                bank.load(&vfs, &mut waves, &stem)
                    .unwrap_or_else(|e| panic!("{city} {cue} wave {n}: {e}"));
                checked += 1;
            }
        }
    }
    assert!(checked >= 28, "both tables walked ({checked} waves)");
}
