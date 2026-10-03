//! Physics-step evidence for smoke runs. Measures travel, never teleports.
use avian3d::prelude::{LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use mm2_game::{PlayerVehicle, Session};
use mm2_vehicle::{Teleported, VehicleInput};

#[derive(Resource, Default)]
pub(crate) struct MotionEvidence {
    last: Option<(u64, Vec3)>,
    pub distance: f32,
    pub steps: u64,
    pub resets: u64,
    pub throttle_steps: u64,
    pub brake_steps: u64,
    pub steer_steps: u64,
    pub finite: bool,
    initialized: bool,
}

impl MotionEvidence {
    fn sample(&mut self, generation: u64, pos: Vec3, valid: bool, teleported: bool) {
        if !self.initialized {
            self.finite = true;
            self.initialized = true;
        }
        self.finite &= valid && pos.is_finite();
        self.steps += 1;
        if teleported {
            self.resets += 1;
        } else if let Some((old_generation, old_pos)) = self.last
            && generation == old_generation
            && valid
            && old_pos.is_finite()
            && pos.is_finite()
        {
            self.distance += (pos.xz() - old_pos.xz()).length();
        }
        self.last = Some((generation, pos));
    }
}

type MotionPlayers<'w, 's> = Query<
    'w,
    's,
    (
        &'static Position,
        &'static Rotation,
        &'static LinearVelocity,
        &'static VehicleInput,
        Has<Teleported>,
    ),
    With<PlayerVehicle>,
>;

/// Runs after the solver, before the race consumer removes `Teleported`.
pub(crate) fn sample_motion(
    session: Res<Session>,
    mut report: ResMut<MotionEvidence>,
    player: MotionPlayers<'_, '_>,
) {
    if !session.is_playing() {
        return;
    }
    for (pos, rot, vel, input, teleported) in &player {
        report.sample(
            session.generation(),
            pos.0,
            rot.0.is_finite() && vel.0.is_finite(),
            teleported,
        );
        report.throttle_steps += u64::from(input.throttle > 0.1);
        report.brake_steps += u64::from(input.brake > 0.1);
        report.steer_steps += u64::from(input.steering.abs() > 0.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_resets_and_new_sessions() {
        let mut report = MotionEvidence::default();
        report.sample(1, Vec3::ZERO, true, false);
        report.sample(1, Vec3::X * 10.0, true, false);
        report.sample(1, Vec3::X * 1000.0, true, true);
        report.sample(1, Vec3::X * 1005.0, true, false);
        report.sample(2, Vec3::ZERO, true, false);
        assert_eq!(report.distance, 15.0);
        assert_eq!(report.resets, 1);
        assert!(report.finite);
    }
    #[test]
    fn retains_nonfinite_failure() {
        let mut report = MotionEvidence::default();
        report.sample(1, Vec3::ZERO, true, false);
        report.sample(1, Vec3::splat(f32::NAN), false, false);
        report.sample(1, Vec3::X, true, false);
        assert!(!report.finite);
        assert_eq!(report.distance, 0.0);
    }
}
