//! Designed exterior HUD dial, inspired by MM3. Art is generated locally;
//! telemetry stays shared with the original cockpit instruments.
use bevy::{
    asset::RenderAssetUsages,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
};
use mm2_game::{PlayerVehicle, Session, SessionEntity, SessionPhase, VehicleTelemetry};
use mm2_vehicle::Vehicle;

#[derive(Component)]
pub struct Speedometer;
#[derive(Component, Default)]
pub struct SpeedNeedle {
    pub displayed_kmh: f32,
}
#[derive(Component)]
pub enum SpeedometerText {
    Speed,
    Gear,
}
#[derive(Component)]
pub struct RpmFill;

/// 270 degree sweep: zero at lower-left, 300 at lower-right.
pub fn needle_angle(kmh: f32) -> f32 {
    (-135.0 + kmh.clamp(0.0, 300.0) * 0.9).to_radians()
}

fn artwork(bytes: &[u8], images: &mut Assets<Image>) -> Handle<Image> {
    images.add(
        Image::from_buffer(
            bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::default(),
        )
        .expect("bundled dial artwork is valid PNG"),
    )
}

pub fn spawn_speedometer(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    owner: SessionEntity,
) {
    let dial = artwork(include_bytes!("../assets/speedometer/dial.png"), images);
    let needle = artwork(include_bytes!("../assets/speedometer/needle.png"), images);
    let root = commands
        .spawn((
            owner,
            Speedometer,
            ImageNode::new(dial),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(20.0),
                bottom: Val::Px(16.0),
                width: Val::VMin(32.0),
                min_width: Val::Px(190.0),
                max_width: Val::Px(290.0),
                aspect_ratio: Some(1.0),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        owner,
        ChildOf(root),
        SpeedNeedle::default(),
        ImageNode::new(needle),
        UiTransform {
            rotation: Rot2::radians(needle_angle(0.0)),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
    ));
    for (kind, text, top, left, width, font_size) in [
        (SpeedometerText::Speed, "0", 66.5, 33.0, 34.0, 30.0),
        (SpeedometerText::Gear, "N", 9.0, 81.5, 14.0, 24.0),
    ] {
        commands.spawn((
            owner,
            ChildOf(root),
            kind,
            Text::new(text),
            TextFont {
                font_size: bevy::text::FontSize::Px(font_size),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.93, 0.78)),
            TextLayout::default().with_justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(top),
                left: Val::Percent(left),
                width: Val::Percent(width),
                ..default()
            },
        ));
    }
    let bar = commands
        .spawn((
            owner,
            ChildOf(root),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(20.0),
                bottom: Val::Percent(9.8),
                width: Val::Percent(60.0),
                height: Val::Percent(3.4),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        owner,
        ChildOf(bar),
        RpmFill,
        BackgroundColor(Color::srgb(1.0, 0.64, 0.15)),
        Node {
            width: Val::Percent(0.0),
            height: Val::Percent(100.0),
            ..default()
        },
    ));
}

#[allow(clippy::too_many_arguments)] // HUD presentation borrows separate telemetry/UI components.
pub fn drive_speedometer(
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    time: Res<Time>,
    vehicles: Query<(&VehicleTelemetry, Option<&Vehicle>), With<PlayerVehicle>>,
    mut roots: Query<&mut Visibility, With<Speedometer>>,
    mut needles: Query<(&mut SpeedNeedle, &mut UiTransform)>,
    mut texts: Query<(&SpeedometerText, &mut Text)>,
    mut bars: Query<(&mut Node, &mut BackgroundColor), With<RpmFill>>,
) {
    let vehicle = vehicles.iter().next();
    let show = vehicle.is_some()
        && hud.0
        && matches!(
            session.phase(),
            SessionPhase::Playing | SessionPhase::Countdown | SessionPhase::Paused
        );
    for mut visibility in &mut roots {
        *visibility = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    let Some((telemetry, vehicle)) = vehicle else {
        return;
    };
    // Ground speed reads drifting/reversing correctly and ignores jump velocity.
    let speed = Vec2::new(telemetry.linear_velocity.x, telemetry.linear_velocity.z).length() * 3.6;
    let speed = if speed.is_finite() { speed } else { 0.0 };
    for (mut needle, mut transform) in &mut needles {
        if !matches!(session.phase(), SessionPhase::Paused) {
            needle.displayed_kmh +=
                (speed - needle.displayed_kmh) * (1.0 - (-14.0 * time.delta_secs()).exp());
        }
        transform.rotation = Rot2::radians(needle_angle(needle.displayed_kmh));
    }
    for (kind, mut text) in &mut texts {
        *text = Text::new(match kind {
            SpeedometerText::Speed => format!("{speed:.0}"),
            SpeedometerText::Gear if telemetry.reverse => "R".to_owned(),
            SpeedometerText::Gear => (telemetry.gear + 1).to_string(),
        });
    }
    let redline = vehicle.map_or(7200.0, |v| v.config.engine.redline_rpm);
    let rpm = (telemetry.rpm / redline).clamp(0.0, 1.0);
    for (mut node, mut color) in &mut bars {
        node.width = Val::Percent(rpm * 100.0);
        color.0 = if rpm > 0.9 {
            Color::srgb(1.0, 0.28, 0.12)
        } else {
            Color::srgb(1.0, 0.64, 0.15)
        };
    }
}
