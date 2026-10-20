//! #1758: the "Sleep with the fishes" HUD message for the local car's
//! drowning — armed by a `Submerged` `RecoveryEvent` for the local
//! participant only, and expiring on session time.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use mm2_app::drowned::{
    DROWNED_MESSAGE_TICKS, DrownedMessage, spawn_drowned_message, update_drowned_message,
};
use mm2_app::hud::HudVisible;
use mm2_game::{
    ObjectId, ObjectIdentity, Player, PlayerControl, RecoveryCause, RecoveryEvent, Session,
    SessionConfig, SessionEntity, SessionPhase, advance_session_tick,
};

struct Rig {
    app: App,
    local: ObjectId,
    ai: ObjectId,
}

fn rig() -> Rig {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    for step in [
        SessionPhase::Ready,
        SessionPhase::Countdown,
        SessionPhase::Playing,
    ] {
        session.transition(step).unwrap();
    }
    let generation = session.generation();
    let local = session.mint_object_id();
    let ai = session.mint_object_id();
    let local_player = session.mint_player_id();
    let ai_player = session.mint_player_id();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_message::<RecoveryEvent>()
        .init_resource::<HudVisible>()
        .insert_resource(session)
        .add_systems(Update, update_drowned_message);
    for (object, id, control) in [
        (local, local_player, PlayerControl::Local),
        (ai, ai_player, PlayerControl::Ai),
    ] {
        app.world_mut().spawn((
            SessionEntity(generation),
            ObjectIdentity(object),
            Player { id, control },
        ));
    }
    let mut commands = app.world_mut().commands();
    spawn_drowned_message(&mut commands, SessionEntity(generation));
    app.world_mut().flush();
    Rig { app, local, ai }
}

impl Rig {
    fn fire(&mut self, object: ObjectId, cause: RecoveryCause, generation: Option<u64>) {
        let generation =
            generation.unwrap_or_else(|| self.app.world().resource::<Session>().generation());
        self.app
            .world_mut()
            .resource_mut::<Messages<RecoveryEvent>>()
            .write(RecoveryEvent {
                object,
                generation,
                tick: 0,
                cause,
                landing: None,
            });
        self.app.update();
    }

    fn shown(&mut self) -> bool {
        let mut q = self
            .app
            .world_mut()
            .query_filtered::<&Visibility, With<DrownedMessage>>();
        *q.single(self.app.world()).unwrap() == Visibility::Visible
    }

    fn advance(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.app
                .world_mut()
                .run_system_once(advance_session_tick)
                .unwrap();
        }
        self.app.update();
    }
}

#[test]
fn a_local_submersion_shows_the_message_and_it_expires() {
    let mut r = rig();
    r.app.update();
    assert!(!r.shown(), "hidden until a drowning");
    let local = r.local;
    r.fire(local, RecoveryCause::Submerged, None);
    assert!(r.shown());
    r.advance(DROWNED_MESSAGE_TICKS - 1);
    assert!(r.shown(), "still up just before the deadline");
    r.advance(1);
    assert!(!r.shown(), "expired");
}

#[test]
fn out_of_bounds_remote_and_stale_events_do_not_show_it() {
    let mut r = rig();
    let (local, ai) = (r.local, r.ai);
    r.fire(local, RecoveryCause::OutOfBounds, None);
    assert!(!r.shown(), "falling out of the world is not a drowning");
    r.fire(ai, RecoveryCause::Submerged, None);
    assert!(!r.shown(), "another participant's drowning is not ours");
    r.fire(local, RecoveryCause::Submerged, Some(999));
    assert!(!r.shown(), "an older session's event is stale");
}

#[test]
fn the_message_follows_the_hud_gate() {
    let mut r = rig();
    let local = r.local;
    r.fire(local, RecoveryCause::Submerged, None);
    r.app.world_mut().resource_mut::<HudVisible>().0 = false;
    r.app.update();
    assert!(!r.shown(), "H off hides it");
    r.app.world_mut().resource_mut::<HudVisible>().0 = true;
    r.app.update();
    assert!(r.shown(), "H on brings the unexpired message back");
}
