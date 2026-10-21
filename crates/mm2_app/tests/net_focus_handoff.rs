//! NETPLAY-02G: the command handoff in the integrated fixed schedule never
//! waits on the socket, and window focus loss does not invent or strand
//! input in the stream. Loopback / in-process only: the "socket" is a sink
//! closure the test can park.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::keyboard::{Key, KeyboardFocusLost, KeyboardInput};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::CameraMode;
use mm2_app::input;
use mm2_app::netcmd::{self, CommandStream, Handoff, SEND_INTERVAL_TICKS};
use mm2_app::netplay_timeline::NetplayTimeline;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_net::TickCommand;
use mm2_vehicle::{HumanDriver, VehicleInput};

const FRAME: Duration = Duration::from_micros(16_667);

type Delivered = Arc<Mutex<BTreeMap<u64, TickCommand>>>;

/// A client app whose sink records into `delivered` and, while `parked`
/// is set, blocks inside the write (a stalled socket or contended lock).
fn client(parked: Arc<AtomicBool>, entered: Arc<AtomicBool>) -> (App, Delivered) {
    let mut session = Session::new();
    session
        .begin_generation(SessionConfig::default(), 5)
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let delivered: Delivered = Delivered::default();
    let sink = Arc::clone(&delivered);
    let mut stream = CommandStream::default();
    stream.set_handoff(Handoff::spawn(move |job| {
        while parked.load(Ordering::SeqCst) {
            entered.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut map = sink.lock().unwrap();
        for (i, c) in job.commands.iter().enumerate() {
            map.insert(job.first_seq + i as u64, *c);
        }
        Ok(())
    }));
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .insert_resource(session)
        .insert_resource(NetplayTimeline { enabled: true })
        .insert_resource(CameraMode::Chase)
        .insert_resource(stream)
        .add_systems(Update, input::vehicle_input)
        .add_systems(FixedUpdate, netcmd::capture_and_send);
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleInput::default(),
        HumanDriver::default(),
    ));
    (app, delivered)
}

fn wait(what: &str, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pred() {
        assert!(Instant::now() < deadline, "{what}: deadline");
        std::thread::yield_now();
    }
}

/// A worker parked inside its write must not delay the fixed schedule:
/// every tick is still generated, the render loop keeps its pace, excess
/// windows are dropped at the queue instead of waited on, and once the
/// socket recovers the newest slots arrive.
#[test]
fn a_parked_writer_never_delays_the_fixed_schedule() {
    let parked = Arc::new(AtomicBool::new(true));
    let entered = Arc::new(AtomicBool::new(false));
    let (mut app, delivered) = client(Arc::clone(&parked), Arc::clone(&entered));
    // Warm up until the worker is parked inside its first write.
    let mut frames = 0;
    while !entered.load(Ordering::SeqCst) {
        app.update();
        frames += 1;
        assert!(frames < 10_000);
        std::thread::yield_now();
    }
    let before = app.world().resource::<CommandStream>().generated_slots();
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    for _ in 0..600 {
        let t = Instant::now();
        app.update();
        worst = worst.max(t.elapsed());
    }
    let stream = app.world().resource::<CommandStream>();
    // 600 frames of 1/60 s are 600 fixed ticks, none waited on.
    assert_eq!(stream.generated_slots() - before, 600);
    assert!(
        stream.handoff_dropped() > 0,
        "the queue never filled, so the test proved nothing"
    );
    // Bounded by a generous deadline, not a tight timing: a blocked
    // producer would sit here until the 10 s park lasted, i.e. forever.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(worst < Duration::from_secs(1), "worst frame {worst:?}");
    assert!(delivered.lock().unwrap().is_empty(), "the park leaked");

    parked.store(false, Ordering::SeqCst);
    // Recovery: keep ticking (offers are no longer refused once the worker
    // drains) until a window at or past the release point lands.
    let target = app.world().resource::<CommandStream>().generated_slots();
    wait("the newest slots reach the sink", || {
        app.update();
        delivered
            .lock()
            .unwrap()
            .keys()
            .next_back()
            .is_some_and(|&k| k >= target)
    });
    let got = delivered.lock().unwrap();
    // What did arrive is contiguous within each window and ends at the
    // newest even seq.
    let last = *got.keys().next_back().unwrap();
    assert_eq!(last % SEND_INTERVAL_TICKS, 0);
}

fn key(app: &mut App, window: Entity, code: KeyCode, state: ButtonState) {
    app.world_mut().write_message(KeyboardInput {
        key_code: code,
        logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
        state,
        text: None,
        repeat: false,
        window,
    });
}

fn set_focus(app: &mut App, window: Entity, focused: bool) {
    app.world_mut()
        .entity_mut(window)
        .get_mut::<Window>()
        .unwrap()
        .focused = focused;
    if !focused {
        app.world_mut().write_message(KeyboardFocusLost);
    }
}

/// Losing focus with throttle held must end the held command at once (no
/// invented transitions: nothing before the loss changes, nothing after it
/// is non-neutral), and regaining focus must not resurrect the key; only a
/// fresh press drives again. Every tick has exactly one slot throughout.
#[test]
fn focus_loss_and_refocus_leave_the_stream_neutral_until_a_fresh_press() {
    let (mut app, delivered) = client(Arc::default(), Arc::default());
    let window = app.world_mut().spawn(Window::default()).id();
    let generated = |app: &App| app.world().resource::<CommandStream>().generated_slots();

    for _ in 0..4 {
        app.update();
    }
    let idle_end = generated(&app);
    key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed);
    for _ in 0..20 {
        app.update();
    }
    let held_end = generated(&app);
    set_focus(&mut app, window, false);
    for _ in 0..20 {
        app.update();
    }
    let lost_end = generated(&app);
    set_focus(&mut app, window, true);
    for _ in 0..20 {
        app.update();
    }
    let refocus_end = generated(&app);
    key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed);
    for _ in 0..10 {
        app.update();
    }
    let end = generated(&app);
    // Flush the unsent tail so every generated slot is delivered.
    let want = end - end % SEND_INTERVAL_TICKS;
    wait("delivery", || {
        delivered
            .lock()
            .unwrap()
            .keys()
            .next_back()
            .is_some_and(|&k| k >= want)
    });
    let got = delivered.lock().unwrap().clone();
    // One slot per generated tick, contiguous from seq 1.
    for seq in 1..=want {
        assert!(got.contains_key(&seq), "seq {seq} missing");
    }
    let throttle = |seq: u64| got[&seq].throttle;
    // Idle before the press; one frame of slack for the press landing.
    for seq in 1..=idle_end {
        assert_eq!(
            throttle(seq),
            0,
            "invented input before the press, seq {seq}"
        );
    }
    assert!(
        (idle_end + 2..=held_end).all(|s| throttle(s) > 0),
        "the held key did not drive"
    );
    // The loss frame is neutral from its first tick; refocus stays neutral.
    for seq in held_end + 2..=refocus_end.min(want) {
        assert_eq!(throttle(seq), 0, "stuck or resurrected control, seq {seq}");
    }
    assert!(lost_end < refocus_end && refocus_end < end);
    assert!(
        (refocus_end + 2..=want).any(|s| throttle(s) > 0),
        "a fresh press never drove"
    );
}
