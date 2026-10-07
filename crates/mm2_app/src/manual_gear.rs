//! The manual gearbox's held gear (F23-A.2, DSN-77).
//!
//! The sim's gearbox is automatic; a manual driver pins it through
//! [`mm2_vehicle::VehicleInput::forced_gear`]. This holds the gear the
//! driver has chosen between frames — device mappings stay out of the
//! physics systems, which only ever see the pinned index.

use bevy::prelude::*;

/// The gear a manual driver is holding, and which car it is held on.
#[derive(Debug, Default)]
pub struct ManualGear {
    held: Option<(Entity, usize)>,
}

impl ManualGear {
    /// Back to automatic: the next manual frame seeds from the car.
    pub fn release(&mut self) {
        self.held = None;
    }

    /// The gear to pin this frame. A car not held yet (first manual frame,
    /// a respawned car, a policy switch) starts in the gear its automatic
    /// box is in, so turning manual on never lurches; each `up`/`down`
    /// edge then moves one gear, clamped to `0..gears`.
    pub fn command(
        &mut self,
        car: Entity,
        current: usize,
        gears: usize,
        up: bool,
        down: bool,
    ) -> usize {
        let top = gears.saturating_sub(1);
        let mut gear = match self.held {
            Some((held_car, gear)) if held_car == car => gear,
            _ => current,
        }
        .min(top);
        if up {
            gear = (gear + 1).min(top);
        }
        if down {
            gear = gear.saturating_sub(1);
        }
        self.held = Some((car, gear));
        gear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    #[test]
    fn the_first_manual_frame_keeps_the_gear_the_car_is_in() {
        let mut m = ManualGear::default();
        assert_eq!(m.command(car(1), 3, 6, false, false), 3);
        // The sim's own gear no longer steers it once held.
        assert_eq!(m.command(car(1), 0, 6, false, false), 3);
    }

    #[test]
    fn shifts_move_one_gear_and_clamp_at_both_ends() {
        let mut m = ManualGear::default();
        assert_eq!(m.command(car(1), 4, 6, true, false), 5);
        assert_eq!(m.command(car(1), 5, 6, true, false), 5, "top gear holds");
        for _ in 0..8 {
            m.command(car(1), 0, 6, false, true);
        }
        assert_eq!(m.command(car(1), 0, 6, false, true), 0, "first gear holds");
        assert_eq!(m.command(car(1), 0, 6, true, false), 1);
    }

    #[test]
    fn up_and_down_together_cancel() {
        let mut m = ManualGear::default();
        assert_eq!(m.command(car(1), 2, 6, true, true), 2);
    }

    #[test]
    fn a_different_car_or_a_release_reseeds_from_the_car() {
        let mut m = ManualGear::default();
        m.command(car(1), 2, 6, true, false);
        assert_eq!(m.command(car(2), 1, 6, false, false), 1, "new car");
        m.release();
        assert_eq!(m.command(car(2), 4, 6, false, false), 4, "after release");
    }

    #[test]
    fn a_held_gear_past_a_shorter_gearbox_clamps() {
        let mut m = ManualGear::default();
        m.command(car(1), 5, 6, false, false);
        assert_eq!(m.command(car(1), 5, 4, false, false), 3);
        assert_eq!(m.command(car(1), 0, 0, false, false), 0, "no gears");
    }
}
