//! The shared speech floor (F08-B.2): a deterministic script over the
//! production [`speech_arbiter`] with synthetic clips — no install, no
//! audio device. Evidence level: synthetic integration. Advances
//! F08-AC03 (no incoherent overlap) and F08-AC06 (restarts leave no
//! queued or playing dialogue); the wiring of the real sources is
//! covered by the commentary tests in `audio.rs`, `cnrvoice.rs` and
//! `lesson_voice.rs`.

use std::time::Duration;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::audio::{AudioReport, CommentaryVoice, PcmAudio, reset_audio_report};
use mm2_app::speech::{
    MAX_QUEUED_LINES, SpeechArbiter, SpeechLine, SpeechPriority, speech_arbiter,
};
use mm2_game::{Session, SessionConfig, SessionPhase};

fn playing_session(generation: Option<u64>) -> Session {
    let mut session = Session::new();
    match generation {
        None => session.begin(SessionConfig::default()).unwrap(),
        Some(g) => session
            .begin_generation(SessionConfig::default(), g)
            .unwrap(),
    }
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    session
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            0.1,
        )))
        .insert_resource(playing_session(None))
        .init_resource::<AudioReport>()
        .init_resource::<SpeechArbiter>()
        .add_systems(Update, speech_arbiter);
    app.finish();
    app.cleanup();
    app
}

fn line(stem: &str, secs: f32, priority: SpeechPriority) -> SpeechLine {
    SpeechLine {
        stem: stem.to_owned(),
        clip: Handle::<PcmAudio>::default(),
        secs,
        priority,
        max_age: None,
        suppress_repeats: true,
    }
}

fn submit(app: &mut App, line: SpeechLine) -> bool {
    let generation = app.world().resource::<Session>().generation();
    let world = app.world_mut();
    world.resource_scope(|world, mut arbiter: Mut<SpeechArbiter>| {
        let mut report = world.resource_mut::<AudioReport>();
        arbiter.submit(generation, line, &mut report)
    })
}

fn speaking(app: &mut App) -> Vec<String> {
    let mut stems: Vec<String> = app
        .world_mut()
        .query::<&CommentaryVoice>()
        .iter(app.world())
        .map(|v| v.stem.clone())
        .collect();
    stems.sort();
    stems
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

/// Equal priority never talks over the line in progress: the second
/// waits out the clip and the gap, and starts once.
#[test]
fn lines_of_one_priority_take_turns() {
    let mut app = app();
    submit(&mut app, line("a", 1.0, SpeechPriority::Ambience));
    submit(&mut app, line("b", 1.0, SpeechPriority::Ambience));
    run(&mut app, 3);
    assert_eq!(speaking(&mut app), ["a"]);
    // 1 s clip + 0.25 s gap at 0.1 s a frame.
    run(&mut app, 14);
    // (No audio plugin here, so a finished clip's entity lingers.)
    assert_eq!(speaking(&mut app), ["a", "b"]);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.interrupted, r.suppressed), (2, 0, 0));
}

/// A higher priority line cuts the one in progress off and counts it;
/// the highest waiting line goes first regardless of arrival order.
#[test]
fn a_higher_priority_line_interrupts_and_jumps_the_queue() {
    let mut app = app();
    submit(&mut app, line("weather", 5.0, SpeechPriority::Ambience));
    run(&mut app, 3);
    assert_eq!(speaking(&mut app), ["weather"]);
    submit(&mut app, line("tod", 5.0, SpeechPriority::Ambience));
    submit(&mut app, line("gate", 2.0, SpeechPriority::Checkpoint));
    submit(&mut app, line("win", 2.0, SpeechPriority::Result));
    run(&mut app, 2);
    // The result outranks the gate: it interrupts the weather line.
    assert_eq!(speaking(&mut app), ["win"]);
    assert_eq!(app.world().resource::<AudioReport>().interrupted, 1);
    // The gate outranks the pre-race line still waiting, so it speaks
    // next — it only waits for the result to finish.
    run(&mut app, 30);
    assert_eq!(speaking(&mut app), ["gate", "win"]);
}

/// A wave that just spoke is not started again; an opted-out line is.
#[test]
fn a_repeat_of_a_recent_wave_is_suppressed() {
    let mut app = app();
    submit(&mut app, line("same", 0.2, SpeechPriority::Ambience));
    submit(&mut app, line("same", 0.2, SpeechPriority::Ambience));
    run(&mut app, 12);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.suppressed), (1, 1));
    let mut call = line("call", 0.2, SpeechPriority::Call);
    call.suppress_repeats = false;
    submit(&mut app, call);
    let mut again = line("call", 0.2, SpeechPriority::Call);
    again.suppress_repeats = false;
    submit(&mut app, again);
    run(&mut app, 12);
    assert_eq!(app.world().resource::<AudioReport>().commentary, 3);
}

/// Long queued speech stays bounded: a full queue evicts its lowest
/// priority line for a higher one and refuses an equal newcomer.
#[test]
fn the_queue_is_bounded_across_sources() {
    let mut app = app();
    // Hold the floor so nothing leaves the queue.
    submit(&mut app, line("hold", 60.0, SpeechPriority::Ambience));
    run(&mut app, 2);
    for i in 0..MAX_QUEUED_LINES {
        assert!(submit(
            &mut app,
            line(&format!("q{i}"), 1.0, SpeechPriority::Ambience)
        ));
    }
    assert!(!submit(
        &mut app,
        line("late", 1.0, SpeechPriority::Ambience)
    ));
    assert!(submit(
        &mut app,
        line("gate", 1.0, SpeechPriority::Checkpoint)
    ));
    let arbiter = app.world().resource::<SpeechArbiter>();
    assert_eq!(arbiter.queued(), MAX_QUEUED_LINES);
    assert_eq!(app.world().resource::<AudioReport>().dropped, 2);
}

/// A line that waited past its age is dropped, not spoken late.
#[test]
fn a_stale_line_is_dropped() {
    let mut app = app();
    submit(&mut app, line("hold", 60.0, SpeechPriority::Ambience));
    let mut stale = line("old", 1.0, SpeechPriority::Ambience);
    stale.max_age = Some(1.0);
    submit(&mut app, stale);
    run(&mut app, 20);
    assert_eq!(app.world().resource::<SpeechArbiter>().queued(), 0);
    let r = app.world().resource::<AudioReport>();
    assert_eq!((r.commentary, r.dropped), (1, 1));
}

/// Two restarts: whatever was queued or playing under an old session
/// never speaks in the next one (F08-AC06) — by teardown reset, and by
/// the generation stamp when no teardown ran.
#[test]
fn restarts_leave_no_queued_or_playing_dialogue() {
    let mut app = app();
    submit(&mut app, line("one", 30.0, SpeechPriority::Ambience));
    submit(&mut app, line("queued", 1.0, SpeechPriority::Ambience));
    run(&mut app, 3);
    assert_eq!(app.world().resource::<SpeechArbiter>().queued(), 1);

    // Restart 1: the production unload reset.
    app.world_mut().run_system_once(reset_audio_report).unwrap();
    assert_eq!(app.world().resource::<SpeechArbiter>().queued(), 0);
    assert!(!app.world().resource::<SpeechArbiter>().is_speaking());

    // Restart 2: a new generation arrives with lines still queued.
    submit(&mut app, line("two", 30.0, SpeechPriority::Ambience));
    submit(&mut app, line("stale", 1.0, SpeechPriority::Ambience));
    run(&mut app, 3);
    app.insert_resource(playing_session(Some(9)));
    run(&mut app, 30);
    let arbiter = app.world().resource::<SpeechArbiter>();
    assert_eq!(arbiter.queued(), 0);
    assert!(!arbiter.is_speaking());
    // Nothing from before the restart was spoken after it: the one
    // line counted is "two", which started before the second restart.
    assert_eq!(app.world().resource::<AudioReport>().commentary, 1);
}
