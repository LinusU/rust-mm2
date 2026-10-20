//! The "Sleep with the fishes" HUD message (#1758, from #1715 AC3): a
//! short, transient line shown when the *local* player's car drowns.
//!
//! Detection, dwell, anchor and reset all stay in [`crate::recovery`]; this
//! module only listens for the [`RecoveryEvent`] it already emits. A
//! `Submerged` event for the local participant arms the message, which
//! expires [`DROWNED_MESSAGE_TICKS`] of session time later. `OutOfBounds`
//! events and other participants' events (AI, remote) never show it, and
//! nothing goes on the network — every peer renders its own driver's line.
//!
//! The phrase comes from the `.water` comment in `mm2_formats::water`;
//! the original's wording, placement and duration are unrecovered, so all
//! three are designed (DSN-122, UNK-13).

use bevy::prelude::*;
use mm2_game::{
    ObjectIdentity, Player, PlayerControl, RecoveryCause, RecoveryEvent, Session, SessionEntity,
};

/// The line shown on a drowning.
pub const DROWNED_MESSAGE: &str = "Sleep with the fishes";

/// How long the message stays up, in session ticks (fixed 60 Hz): 3 s.
/// Session ticks freeze under pause, so the message does too.
pub const DROWNED_MESSAGE_TICKS: u64 = 3 * 60;

const DROWNED_COLOR: Color = Color::srgb(0.55, 0.8, 1.0);

/// Marker on the session-owned message line. `expires` is the session tick
/// at which it hides again; `None` while it is not showing.
#[derive(Component, Default)]
pub struct DrownedMessage {
    pub expires: Option<u64>,
}

/// Spawn the message line, session-owned and hidden until
/// [`update_drowned_message`] arms it.
pub fn spawn_drowned_message(commands: &mut Commands, owner: SessionEntity) {
    commands.spawn((
        owner,
        DrownedMessage::default(),
        Text::new(DROWNED_MESSAGE),
        TextFont {
            font_size: bevy::text::FontSize::Px(34.0),
            ..default()
        },
        TextColor(DROWNED_COLOR),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(30.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-150.0)),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(4.0)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Arm the message on a current-generation `Submerged` event whose object
/// is the local participant, and hide it once its tick passes. While the
/// `H` HUD gate is off it keeps its clock but stays hidden.
pub fn update_drowned_message(
    mut events: MessageReader<RecoveryEvent>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    participants: Query<(&ObjectIdentity, &Player)>,
    mut message: Query<(&mut DrownedMessage, &mut Visibility)>,
) {
    let generation = session.generation();
    let tick = session.tick();
    let mut drowned = false;
    for event in events.read() {
        drowned |= event.generation == generation
            && event.cause == RecoveryCause::Submerged
            && participants
                .iter()
                .any(|(id, p)| id.0 == event.object && p.control == PlayerControl::Local);
    }
    for (mut state, mut vis) in &mut message {
        if drowned {
            state.expires = Some(tick + DROWNED_MESSAGE_TICKS);
        }
        if state.expires.is_some_and(|e| tick >= e) {
            state.expires = None;
        }
        *vis = if state.expires.is_some() && hud.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}
