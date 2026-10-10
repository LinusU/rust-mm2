//! Human steering before the car simulation. Recovered from retail
//! `mmInput::GetSteering` (0x52de70) and `mmPlayer::Update` (0x405760).
//! See docs/research/vehicle-physics/05-player-input.md. AI inputs bypass
//! these device filters; network inputs carry the already filtered bytes.

use serde::{Deserialize, Serialize};

/// Per-car `.asnode` steering tuning, separate from wheel geometry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PlayerSteeringConfig {
    pub speed_sensitive: u8,
    pub speed_low: f32,
    pub speed_high: f32,
    pub delta_out: [f32; 2],
    pub delta_in: [f32; 2],
    pub exponent: [f32; 2],
    /// Mouse tuning when authored. Missing analog constructor defaults
    /// remain unauthored rather than borrowing keyboard exponents.
    pub mouse_divisor: Option<[f32; 2]>,
    pub mouse_exponent: Option<[f32; 2]>,
}

impl Default for PlayerSteeringConfig {
    fn default() -> Self {
        Self {
            speed_sensitive: 2,
            speed_low: 5.0,
            speed_high: 100.0,
            delta_out: [3.5, 2.5],
            delta_in: [2.5, 1.5],
            exponent: [2.0, 1.0],
            mouse_divisor: None,
            mouse_exponent: None,
        }
    }
}

impl PlayerSteeringConfig {
    /// Retail's deliberate interpolation quirk: no subtraction of the
    /// low-speed threshold. The factor can exceed one at high speed.
    pub fn speed_factor(&self, speed: f32) -> f32 {
        match self.speed_sensitive {
            0 => 0.0,
            1 => 1.0,
            _ => {
                let span = self.speed_high - self.speed_low;
                if span > 0.0 && speed.is_finite() {
                    speed.abs().clamp(self.speed_low, self.speed_high) / span
                } else {
                    1.0
                }
            }
        }
    }

    pub fn interpolate(&self, values: [f32; 2], speed: f32) -> f32 {
        values[0] + (values[1] - values[0]) * self.speed_factor(speed)
    }

    /// Keyboard and gamepad use separate state values but the same law.
    /// The first frame away from zero takes the inward rate because the
    /// original treats sign(0) as zero. Shape the output, never the state.
    pub fn discrete_step(&self, state: f32, target: f32, speed: f32, dt: f32) -> (f32, f32) {
        let target = target.clamp(-1.0, 1.0);
        let outward = target.abs() >= state.abs() && sign(state) == sign(target);
        let rate = self
            .interpolate(
                if outward {
                    self.delta_out
                } else {
                    self.delta_in
                },
                speed,
            )
            .max(0.0);
        let step = rate * dt.clamp(0.0001, 0.1);
        let next = state + (target - state).clamp(-step, step);
        let exponent = self.interpolate(self.exponent, speed).max(0.01);
        (next, sign(next) * next.abs().powf(exponent))
    }

    /// Mouse is an absolute position with a divisor and exponent, not
    /// the keyboard's rate ramp. User sensitivity is applied upstream.
    pub fn mouse(&self, target: f32, speed: f32) -> f32 {
        let divisor = self
            .mouse_divisor
            .map_or(1.0, |p| self.interpolate(p, speed))
            .max(0.01);
        let exponent = self
            .mouse_exponent
            .map_or(1.0, |p| self.interpolate(p, speed))
            .max(0.01);
        let x = target / divisor;
        (sign(x) * x.abs().powf(exponent)).clamp(-1.0, 1.0)
    }
}

fn sign(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Human recording and network format both truncate toward zero.
pub fn quantize_steering(value: f32) -> f32 {
    (value.clamp(-1.0, 1.0) * 127.0).trunc() / 127.0
}

pub fn quantize_pedal(value: f32) -> f32 {
    (value.clamp(0.0, 1.0) * 255.0).trunc() / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beetle() -> PlayerSteeringConfig {
        PlayerSteeringConfig {
            speed_high: 44.6,
            delta_out: [2.573, 0.8],
            delta_in: [5.0, 5.0],
            exponent: [1.2, 1.2],
            ..Default::default()
        }
    }

    #[test]
    fn first_step_reversal_release_and_shaping_match_retail() {
        let c = beetle();
        let (state, output) = c.discrete_step(0.0, 1.0, 40.0, 1.0 / 60.0);
        assert!((state - 5.0 / 60.0).abs() < 1e-6);
        assert!((output - state.powf(1.2)).abs() < 1e-6);
        assert!((c.discrete_step(1.0, -1.0, 40.0, 0.1).0 - 0.5).abs() < 1e-6);
        assert!((c.discrete_step(-1.0, 0.0, 40.0, 0.1).0 + 0.5).abs() < 1e-6);
    }

    #[test]
    fn beetle_lock_time_is_speed_dependent_without_losing_lock() {
        let time_to_lock = |speed| {
            let mut state = 0.0;
            for frame in 1..600 {
                state = beetle().discrete_step(state, 1.0, speed, 1.0 / 60.0).0;
                if state == 1.0 {
                    return frame as f32 / 60.0;
                }
            }
            panic!("keyboard did not reach full lock");
        };
        assert!((time_to_lock(0.0) - 0.4).abs() < 0.05);
        assert!((time_to_lock(40.0) - 1.25).abs() < 0.1);
        assert!(beetle().speed_factor(100.0) > 1.0);
    }

    #[test]
    fn recording_quantization_truncates_negative_steering_toward_zero() {
        assert_eq!(quantize_steering(-0.5), -63.0 / 127.0);
        assert_eq!(quantize_pedal(0.5), 127.0 / 255.0);
        assert_eq!(quantize_steering(1.0), 1.0);
    }
}
