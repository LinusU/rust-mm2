use bevy::{ecs::world::CommandQueue, prelude::*};
use mm2_app::{
    hud::HudVisible,
    speedometer::{self, RpmFill, Speedometer, SpeedometerText},
};
use mm2_game::{
    AuthorityRole, ObjectId, PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase,
    VehicleTelemetry,
};

#[test]
fn dial_reads_ground_speed_reverse_rpm_and_hud_gate() {
    let mut session = Session::new();
    session.begin(SessionConfig::default()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(session)
        .init_resource::<HudVisible>()
        .add_systems(Update, speedometer::drive_speedometer);
    let mut queue = CommandQueue::default();
    speedometer::spawn_speedometer(
        &mut Commands::new(&mut queue, app.world()),
        &mut Assets::default(),
        SessionEntity(1),
    );
    queue.apply(app.world_mut());
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleTelemetry {
            object: ObjectId {
                generation: 1,
                slot: 0,
            },
            tick: 1,
            authority: AuthorityRole::Authority,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::new(6.0, 100.0, 8.0),
            angular_velocity: Vec3::ZERO,
            forward_speed: -8.0,
            rpm: 3600.0,
            engine_load: 0.0,
            gear: 0,
            reverse: true,
            shifting: false,
            wheels: vec![],
            damage: default(),
        },
    ));
    app.update();
    for (kind, text) in app
        .world_mut()
        .query::<(&SpeedometerText, &Text)>()
        .iter(app.world())
    {
        assert_eq!(
            text.0,
            match kind {
                SpeedometerText::Speed => "36",
                SpeedometerText::Gear => "R",
            }
        );
    }
    let node = app
        .world_mut()
        .query_filtered::<&Node, With<RpmFill>>()
        .single(app.world())
        .unwrap();
    assert_eq!(node.width, Val::Percent(50.0));
    app.world_mut().resource_mut::<HudVisible>().0 = false;
    app.update();
    let visibility = app
        .world_mut()
        .query_filtered::<&Visibility, With<Speedometer>>()
        .single(app.world())
        .unwrap();
    assert_eq!(*visibility, Visibility::Hidden);
    assert_eq!(speedometer::needle_angle(0.0), (-135.0_f32).to_radians());
    assert_eq!(speedometer::needle_angle(150.0), 0.0);
    assert_eq!(speedometer::needle_angle(600.0), 135.0_f32.to_radians());
}
