//! F23-A.5: the in-session keys are settings. Each toggle system reads
//! its key from `ControlSettings` (shipped keys when a harness never
//! inserted it): a remap moves the control, the old key stops, the
//! alternate slot works, and the live-phase gates are unchanged — so a
//! paused page listening for the new key cannot also fire the control.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::audio::HornRequest;
use mm2_app::camera::{RearView, mirror_input};
use mm2_app::car_visual::{HeadlightsOn, toggle_headlights};
use mm2_app::controls::{ControlSettings, DriveAction};
use mm2_app::hud::{HudVisible, hud_input};
use mm2_app::oppind::{OpponentIndicators, indicator_input};
use mm2_game::{Session, SessionConfig, SessionPhase};

fn session_at(phase: SessionPhase) -> Session {
    let mut s = Session::new();
    s.begin(SessionConfig::default()).unwrap(); // Loading
    let path: &[SessionPhase] = match phase {
        SessionPhase::Playing => &[
            SessionPhase::Ready,
            SessionPhase::Countdown,
            SessionPhase::Playing,
        ],
        SessionPhase::Paused => &[
            SessionPhase::Ready,
            SessionPhase::Countdown,
            SessionPhase::Playing,
            SessionPhase::Paused,
        ],
        other => panic!("no path to {other:?}"),
    };
    for step in path {
        s.transition(step.clone()).unwrap();
    }
    s
}

#[derive(Resource, Default)]
struct Horns(usize);

fn count_horns(mut requests: MessageReader<HornRequest>, mut horns: ResMut<Horns>) {
    horns.0 += requests.read().count();
}

fn app(phase: SessionPhase, controls: Option<ControlSettings>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<RearView>()
        .init_resource::<HudVisible>()
        .init_resource::<OpponentIndicators>()
        .init_resource::<HeadlightsOn>()
        .init_resource::<Horns>()
        .add_message::<HornRequest>()
        .insert_resource(session_at(phase))
        .add_systems(
            Update,
            (
                mirror_input,
                hud_input,
                indicator_input,
                toggle_headlights,
                mm2_app::audio::horn_input,
                count_horns.after(mm2_app::audio::horn_input),
            ),
        );
    if let Some(c) = controls {
        app.insert_resource(c);
    }
    app
}

fn tap(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

/// Every toggle's observable state, in a fixed order.
fn state(app: &App) -> [usize; 5] {
    let w = app.world();
    [
        w.resource::<RearView>().0 as usize,
        w.resource::<HudVisible>().0 as usize,
        w.resource::<OpponentIndicators>().0 as usize,
        w.resource::<HeadlightsOn>().0 as usize,
        w.resource::<Horns>().0,
    ]
}

fn flipped(app: &mut App, key: KeyCode) -> [usize; 5] {
    let before = state(app);
    tap(app, key);
    let after = state(app);
    std::array::from_fn(|i| after[i].abs_diff(before[i]))
}

/// The five controls and where each one lives by default.
const SHIPPED: [(DriveAction, KeyCode); 5] = [
    (DriveAction::Mirror, KeyCode::Backspace),
    (DriveAction::Hud, KeyCode::KeyH),
    (DriveAction::Indicators, KeyCode::KeyI),
    (DriveAction::Headlights, KeyCode::KeyL),
    (DriveAction::Horn, KeyCode::Enter),
];

/// With no settings resource at all the documented keys still work, each
/// moving exactly its own control.
#[test]
fn a_harness_without_settings_keeps_the_shipped_keys() {
    let mut app = app(SessionPhase::Playing, None);
    for (i, (_, key)) in SHIPPED.iter().enumerate() {
        let mut want = [0; 5];
        want[i] = 1;
        assert_eq!(flipped(&mut app, *key), want, "{key:?}");
    }
}

/// A remap moves the control: the new key flips it, the key it left
/// does nothing.
#[test]
fn a_remapped_key_moves_the_control_and_frees_the_old_key() {
    let moved = [
        KeyCode::KeyM,
        KeyCode::KeyU,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyN,
    ];
    let mut controls = ControlSettings::default();
    for ((action, _), key) in SHIPPED.iter().zip(moved) {
        controls.rebind(*action, 0, key).unwrap();
    }
    let mut app = app(SessionPhase::Playing, Some(controls));
    for (_, old) in SHIPPED {
        assert_eq!(flipped(&mut app, old), [0; 5], "{old:?} is free now");
    }
    for (i, key) in moved.iter().enumerate() {
        let mut want = [0; 5];
        want[i] = 1;
        assert_eq!(flipped(&mut app, *key), want, "{key:?}");
    }
}

/// The alternate slot answers beside the primary.
#[test]
fn the_alternate_slot_answers_too() {
    let mut controls = ControlSettings::default();
    controls
        .rebind(DriveAction::Headlights, 1, KeyCode::KeyK)
        .unwrap();
    let mut app = app(SessionPhase::Playing, Some(controls));
    assert_eq!(flipped(&mut app, KeyCode::KeyK), [0, 0, 0, 1, 0]);
    assert_eq!(flipped(&mut app, KeyCode::KeyL), [0, 0, 0, 1, 0]);
}

/// A pause page listening for the new key must not also fire the control
/// under it: nothing but the live phases answers, headlights included
/// (they used to read `L` in any phase).
#[test]
fn a_paused_session_answers_none_of_them() {
    let mut app = app(SessionPhase::Paused, None);
    for (_, key) in SHIPPED {
        assert_eq!(flipped(&mut app, key), [0; 5], "{key:?}");
    }
}
