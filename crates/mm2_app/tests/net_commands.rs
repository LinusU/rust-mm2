//! NETPLAY-02C: the client's fixed-tick command stream through the
//! production `capture_and_send` system and the real worker handoff, with
//! a recording sink in place of the socket. Render rate must not change
//! which commands exist (F25 AC1, client half: the host consuming them is
//! NETPLAY-02B).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::netcmd::{self, CommandStream, Handoff, SEND_INTERVAL_TICKS};
use mm2_app::netplay_timeline::NetplayTimeline;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_net::{Message, TickCommand};
use mm2_vehicle::{HumanDriver, VehicleInput};

const TICKS: usize = 180;

/// The recorded controls for fixed tick `n` (0-based): every field moves
/// every tick, so a dropped or doubled tick shows.
fn recorded(n: usize) -> VehicleInput {
    VehicleInput {
        throttle: (n % 11) as f32 / 10.0,
        brake: (n % 7) as f32 / 6.0,
        steering: ((n % 21) as f32 - 10.0) / 10.0,
        handbrake: if n.is_multiple_of(13) { 1.0 } else { 0.0 },
        forced_gear: n.is_multiple_of(5).then_some(n % 4),
    }
}

#[derive(Resource, Default)]
struct Ticks(usize);

fn feed(mut ticks: ResMut<Ticks>, mut car: Query<&mut VehicleInput, With<PlayerVehicle>>) {
    for mut input in &mut car {
        *input = recorded(ticks.0);
    }
    ticks.0 += 1;
}

type Delivered = Arc<Mutex<BTreeMap<u64, TickCommand>>>;

fn app(frame_secs: f64) -> (App, Delivered) {
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
        // Through the codec, as the socket would carry it.
        let msg = Message::InputWindow {
            generation: job.generation,
            epoch: job.epoch,
            first_seq: job.first_seq,
            commands: job.commands.clone(),
        };
        let Message::InputWindow {
            first_seq,
            commands,
            ..
        } = Message::decode(&msg.encode().unwrap()).unwrap()
        else {
            unreachable!()
        };
        let mut map = sink.lock().unwrap();
        for (i, c) in commands.into_iter().enumerate() {
            // Repeats must agree with what was delivered before.
            let prior = map.insert(first_seq + i as u64, c);
            assert!(
                prior.is_none_or(|p| p == c),
                "seq {} changed",
                first_seq + i as u64
            );
        }
        Ok(())
    }));

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            frame_secs,
        )))
        .insert_resource(session)
        .insert_resource(NetplayTimeline { enabled: true })
        .insert_resource(stream)
        .init_resource::<Ticks>()
        .add_systems(FixedUpdate, (feed, netcmd::capture_and_send).chain());
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleInput::default(),
        HumanDriver::default(),
    ));
    (app, delivered)
}

/// Run until `TICKS` fixed ticks have happened; returns the delivered map.
fn run(frame_secs: f64) -> (usize, usize, BTreeMap<u64, TickCommand>, u64, u64) {
    let (mut app, delivered) = app(frame_secs);
    let mut frames = 0;
    while app.world().resource::<Ticks>().0 < TICKS {
        app.update();
        frames += 1;
        // The test runs far faster than real time; let the worker drain so
        // a loaded machine does not fill the (deliberately lossy) handoff
        // queue. Bounded: a stuck worker shows as a failed count below.
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = app.world().resource::<CommandStream>();
        while stream.transmitted_windows() + stream.send_errors() < stream.queued_windows()
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(frames < 10_000, "fixed schedule never ran");
    }
    // The worker thread delivers asynchronously: bounded deadline.
    let ticks = app.world().resource::<Ticks>().0;
    let want = (ticks as u64 / SEND_INTERVAL_TICKS) * SEND_INTERVAL_TICKS;
    let deadline = Instant::now() + Duration::from_secs(10);
    while delivered
        .lock()
        .unwrap()
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        < want
        && Instant::now() < deadline
    {
        std::thread::yield_now();
    }
    let stream = app.world().resource::<CommandStream>();
    let (generated, handoff_dropped) = (stream.generated_slots(), stream.handoff_dropped());
    let got = delivered.lock().unwrap().clone();
    (frames, ticks, got, generated, handoff_dropped)
}

#[test]
fn every_render_rate_produces_the_same_per_tick_timeline() {
    let expected: Vec<TickCommand> = (0..TICKS + 2)
        .map(|n| netcmd::tick_command(&recorded(n), Some(&HumanDriver::default())))
        .collect();
    let mut timelines = Vec::new();
    for hz in [30.0, 60.0, 144.0] {
        let (frames, ticks, got, generated, dropped) = run(1.0 / hz);
        assert!(
            (TICKS..=TICKS + 1).contains(&ticks),
            "{hz} Hz ran {ticks} ticks"
        );
        // One command per fixed tick: none dropped, collapsed or doubled.
        assert_eq!(generated, ticks as u64, "{hz} Hz over {frames} frames");
        assert_eq!(dropped, 0, "{hz} Hz");
        // Everything up to the last send boundary arrived, contiguous
        // from seq 1, and equals the recorded controls tick for tick.
        let last = *got.keys().next_back().unwrap();
        assert!(
            last + SEND_INTERVAL_TICKS > ticks as u64,
            "{hz} Hz last {last}"
        );
        for seq in 1..=last {
            assert_eq!(
                got.get(&seq),
                Some(&expected[seq as usize - 1]),
                "{hz} Hz seq {seq}"
            );
        }
        timelines.push((hz, got));
    }
    // Intermediate ticks survive at every rate: the steering sweep is
    // present command for command, not resampled per rendered frame.
    let (_, base) = &timelines[0];
    for (hz, other) in &timelines[1..] {
        for (seq, c) in other {
            if let Some(b) = base.get(seq) {
                assert_eq!(b, c, "{hz} Hz vs 30 Hz at seq {seq}");
            }
        }
    }
}
