//! Closed-form handling diagnostics for a [`VehicleConfig`].
//!
//! Nothing here simulates: every figure is solved analytically from the
//! config so a whole vehicle roster can be audited in milliseconds, and so
//! tests can assert on handling properties without running physics.
//!
//! The two figures that matter most for arcade playability:
//!
//! * [`HandlingMetrics::rollover_margin`] — how much more lateral grip the
//!   tires make than the chassis can take before it tips. Below `1.0` the
//!   car reaches its tipping point before its tires let go, so a merely
//!   hard corner puts it on its roof.
//! * [`HandlingMetrics::breakover_angle`] — the crest angle the belly
//!   clears between the axles. Road seams and intersection crowns are
//!   shallow, but so is a car body slung between distant axles.
//!
//! Units: metres, kilograms, seconds, radians, newtons — as everywhere in
//! this crate. Geometry is chassis space (+x right, +y up, -z forward), so
//! the front axle sits at the *smallest* z.

use crate::config::VehicleConfig;

/// Standard gravity used to turn loads into accelerations.
const G: f32 = 9.81;

/// Resting state and ride quality of one wheel.
#[derive(Debug, Clone, Copy)]
pub struct WheelMetrics {
    /// Chassis-space y of the contact patch once the car has settled.
    pub contact_y: f32,
    /// Share of the car's weight this wheel carries at rest, newtons.
    pub static_load: f32,
    /// Suspension compression at rest, metres.
    pub rest_compression: f32,
    /// `rest_compression / travel` — how much travel is spent holding the
    /// car up. Around `0.3`–`0.5` leaves room for both bump and rebound.
    pub sag_fraction: f32,
    /// Undamped natural frequency of the corner, Hz. Road cars sit near
    /// `1.0`–`1.5`; much above that rides like a go-kart, much below it
    /// wallows.
    pub natural_frequency: f32,
    /// Damping ratio of the corner. Below ~`0.3` the car pogos after every
    /// bump; above ~`1.0` the suspension is locked solid and the chassis
    /// takes the impact instead.
    pub damping_ratio: f32,
    /// Whether the spring cannot hold the static load inside its travel —
    /// the car rests on the bump stops and rides on the chassis.
    pub bottoms_out: bool,
}

/// Analytic handling summary of a whole vehicle.
#[derive(Debug, Clone)]
pub struct HandlingMetrics {
    /// Per-wheel figures, parallel to [`VehicleConfig::wheels`].
    pub wheels: Vec<WheelMetrics>,
    /// Chassis-space y of the ground under a settled car.
    pub ground_y: f32,
    /// Centre-of-mass height above that ground plane, metres.
    pub com_height: f32,
    /// Half the track width, metres — the tipping lever arm.
    pub half_track: f32,
    /// Lateral acceleration at which the inside wheels lift, in g, from
    /// geometry alone: the static rollover threshold `half_track /
    /// com_height`.
    pub tip_threshold_g: f32,
    /// The same threshold once [`AssistConfig::roll_resistance`] has
    /// shortened the lever that lateral tire force pulls on. This is what
    /// the car actually does.
    ///
    /// [`AssistConfig::roll_resistance`]: crate::config::AssistConfig::roll_resistance
    pub assisted_tip_threshold_g: f32,
    /// Lateral acceleration the tires can actually generate, in g.
    pub peak_lateral_g: f32,
    /// `assisted_tip_threshold_g / peak_lateral_g`. Below `1.0` the car
    /// tips before it slides; a playable arcade car wants comfortably
    /// above `1.0`.
    pub rollover_margin: f32,
    /// Lowest point of the chassis collider above the ground plane, metres.
    pub belly_clearance: f32,
    /// Approach angle at the front overhang, radians (π/2 when nothing
    /// hangs past the front axle).
    pub approach_angle: f32,
    /// Departure angle at the rear overhang, radians.
    pub departure_angle: f32,
    /// Crest angle the belly clears between the axles, radians. A car that
    /// catches on intersection crowns has a small one.
    pub breakover_angle: f32,
    /// Lateral acceleration the high-speed steering lock asks for at
    /// [`SteeringConfig::high_speed`], in g — compare against
    /// `peak_lateral_g`.
    ///
    /// [`SteeringConfig::high_speed`]: crate::config::SteeringConfig::high_speed
    pub high_speed_steer_demand_g: f32,
}

/// Lower convex envelope of `points` projected onto the side view, as
/// `(z, y)` vertices ordered by `z`. This is the underside of the chassis
/// as a crest or kerb meets it.
fn lower_envelope(points: &[[f32; 3]]) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = points.iter().map(|p| (p[2], p[1])).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup();

    // Monotone chain, keeping only right turns so the chain hugs the
    // underside of the point set.
    let mut hull: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        while hull.len() >= 2 {
            let a = hull[hull.len() - 2];
            let b = hull[hull.len() - 1];
            let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
            if cross <= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(p);
    }
    hull
}

/// Height of the envelope at `z`, or `None` when `z` is outside its span.
fn envelope_height(envelope: &[(f32, f32)], z: f32) -> Option<f32> {
    let seg = envelope.windows(2).find(|w| w[0].0 <= z && z <= w[1].0)?;
    let (z0, y0) = seg[0];
    let (z1, y1) = seg[1];
    let span = z1 - z0;
    if span.abs() < 1e-6 {
        return Some(y0.min(y1));
    }
    Some(y0 + (y1 - y0) * (z - z0) / span)
}

/// Indices of the wheels on the front axle (chassis `-z` half).
fn front_wheels(config: &VehicleConfig) -> Vec<bool> {
    let zs: Vec<f32> = config.wheels.iter().map(|w| w.position[2]).collect();
    let z_min = zs.iter().copied().fold(f32::MAX, f32::min);
    let z_max = zs.iter().copied().fold(f32::MIN, f32::max);
    let z_mid = (z_min + z_max) * 0.5;
    zs.iter().map(|z| *z < z_mid).collect()
}

impl HandlingMetrics {
    /// Solve the resting state and stability figures for `config`.
    pub fn of(config: &VehicleConfig) -> Self {
        let n = config.wheels.len();
        if n == 0 {
            return Self::empty();
        }
        let is_front = front_wheels(config);
        let zs: Vec<f32> = config.wheels.iter().map(|w| w.position[2]).collect();
        let front_z = zs.iter().copied().fold(f32::MAX, f32::min);
        let rear_z = zs.iter().copied().fold(f32::MIN, f32::max);

        // Longitudinal weight split about the centre of mass, then shared
        // evenly across the wheels of each axle.
        let n_front = is_front.iter().filter(|f| **f).count().max(1);
        let n_rear = (n - is_front.iter().filter(|f| **f).count()).max(1);
        let com_z = config.center_of_mass[2];
        let front_share = ((rear_z - com_z) / (rear_z - front_z).max(1e-3)).clamp(0.05, 0.95);
        let weight = config.mass * G;

        let mut wheels = Vec::with_capacity(n);
        for (i, wheel) in config.wheels.iter().enumerate() {
            let suspension = wheel.suspension.as_ref().unwrap_or(&config.suspension);
            let axle_share = if is_front[i] {
                front_share / n_front as f32
            } else {
                (1.0 - front_share) / n_rear as f32
            };
            let static_load = weight * axle_share;

            // The spring settles where its force matches the static load.
            let free_compression = static_load / suspension.spring_rate.max(1e-3);
            let rest_compression = free_compression.min(suspension.travel);
            let corner_mass = (static_load / G).max(1e-3);
            let omega = (suspension.spring_rate / corner_mass).sqrt();
            // Damping is asymmetric; rebound is what controls a bounce.
            let critical = 2.0 * (suspension.spring_rate * corner_mass).sqrt();

            wheels.push(WheelMetrics {
                contact_y: wheel.position[1]
                    - (suspension.travel - rest_compression)
                    - wheel.radius,
                static_load,
                rest_compression,
                sag_fraction: rest_compression / suspension.travel.max(1e-3),
                natural_frequency: omega / std::f32::consts::TAU,
                damping_ratio: suspension.damping_rebound / critical.max(1e-3),
                bottoms_out: free_compression >= suspension.travel,
            });
        }

        let ground_y = wheels.iter().map(|w| w.contact_y).sum::<f32>() / n as f32;
        let com_height = (config.center_of_mass[1] - ground_y).max(1e-3);
        let half_track = (config.track_width * 0.5).max(1e-3);
        let tip_threshold_g = half_track / com_height;
        // The assist raises where lateral force is applied, so the lever
        // that tips the car is only the part of the centre-of-mass height
        // it does not cancel.
        let roll_arm = com_height * (1.0 - config.assists.roll_resistance.clamp(0.0, 1.0));
        let assisted_tip_threshold_g = half_track / roll_arm.max(1e-3);

        // Grip is load-weighted: a wheel carrying more weight contributes
        // proportionally more of the car's cornering force.
        let total_load: f32 = wheels.iter().map(|w| w.static_load).sum::<f32>().max(1e-3);
        let peak_lateral_g = config
            .wheels
            .iter()
            .zip(&wheels)
            .map(|(w, m)| {
                let tires = w.tires.as_ref().unwrap_or(&config.tires);
                tires.lateral_grip * m.static_load
            })
            .sum::<f32>()
            / total_load;

        let (belly_clearance, approach_angle, departure_angle, breakover_angle) =
            Self::hull_clearances(config, ground_y, front_z, rear_z);

        let v = config.steering.high_speed.max(1.0);
        let high_speed_steer_demand_g =
            v * v * config.steering.high_speed_max_angle / config.wheelbase.max(1e-3) / G;

        Self {
            wheels,
            ground_y,
            com_height,
            half_track,
            tip_threshold_g,
            assisted_tip_threshold_g,
            peak_lateral_g,
            rollover_margin: assisted_tip_threshold_g / peak_lateral_g.max(1e-3),
            belly_clearance,
            approach_angle,
            departure_angle,
            breakover_angle,
            high_speed_steer_demand_g,
        }
    }

    /// Ground clearance and the three ramp angles, from the collider hull
    /// (or the fallback cuboid when the config has no hull points).
    fn hull_clearances(
        config: &VehicleConfig,
        ground_y: f32,
        front_z: f32,
        rear_z: f32,
    ) -> (f32, f32, f32, f32) {
        let cuboid;
        let points: &[[f32; 3]] = match &config.collider_points {
            Some(p) if !p.is_empty() => p,
            _ => {
                let [w, h, d] = config.chassis_size;
                let (hw, hh, hd) = (w * 0.5, h * 0.5, d * 0.5);
                cuboid = [
                    [-hw, -hh, -hd],
                    [hw, -hh, -hd],
                    [-hw, -hh, hd],
                    [hw, -hh, hd],
                    [-hw, hh, -hd],
                    [hw, hh, -hd],
                    [-hw, hh, hd],
                    [hw, hh, hd],
                ];
                &cuboid
            }
        };

        // The belly is the hull's *lower surface*, not its vertex set: a
        // box-shaped hull has no vertex between the axles, yet its floor
        // still spans them. Projecting to the side view (z, y) and taking
        // the lower convex envelope recovers that floor exactly, because
        // the hull's underside is convex in that projection.
        let belly = lower_envelope(points);

        let quarter = std::f32::consts::FRAC_PI_2;
        let mut clearance = f32::MAX;
        let mut approach = quarter;
        let mut departure = quarter;
        let mut breakover = quarter;

        // Approach and departure are ramps pivoting on the outer contact
        // patches. Height over distance is a ratio of two functions linear
        // in z, so it is monotone along each envelope segment and its
        // minimum is always at a vertex.
        let mut ramp = |z: f32, y: f32| {
            let height = y - ground_y;
            clearance = clearance.min(height);
            if height <= 0.0 {
                approach = 0.0;
                departure = 0.0;
                breakover = 0.0;
                return;
            }
            if z < front_z {
                approach = approach.min((height / (front_z - z)).atan());
            } else if z > rear_z {
                departure = departure.min((height / (z - rear_z)).atan());
            } else {
                let run = (z - front_z).min(rear_z - z);
                if run > 1e-3 {
                    breakover = breakover.min((height / run).atan());
                }
            }
        };
        for (z, y) in &belly {
            ramp(*z, *y);
        }
        // Between the axles the run `min(z − front, rear − z)` kinks at the
        // midpoint, so that one interior point can beat every vertex.
        let mid_z = (front_z + rear_z) * 0.5;
        if let Some(mid_y) = envelope_height(&belly, mid_z) {
            ramp(mid_z, mid_y);
        }

        if clearance == f32::MAX {
            clearance = 0.0;
        }
        (clearance, approach, departure, breakover)
    }

    fn empty() -> Self {
        Self {
            wheels: Vec::new(),
            ground_y: 0.0,
            com_height: 0.0,
            half_track: 0.0,
            tip_threshold_g: 0.0,
            assisted_tip_threshold_g: 0.0,
            peak_lateral_g: 0.0,
            rollover_margin: 0.0,
            belly_clearance: 0.0,
            approach_angle: 0.0,
            departure_angle: 0.0,
            breakover_angle: 0.0,
            high_speed_steer_demand_g: 0.0,
        }
    }

    /// Every way this config reads as unplayable, as human-readable lines.
    /// Empty means the car is within the arcade envelope.
    ///
    /// The thresholds are deliberately loose: they catch cars that are
    /// broken, not cars that are merely quirky.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.rollover_margin < 1.25 {
            out.push(format!(
                "tips before it slides: rollover margin {:.2} (tips at {:.2} g, grips to {:.2} g)",
                self.rollover_margin, self.assisted_tip_threshold_g, self.peak_lateral_g
            ));
        }
        if self.belly_clearance < 0.15 {
            out.push(format!(
                "belly clearance {:.2} m is too low to clear road seams",
                self.belly_clearance
            ));
        }
        if self.breakover_angle.to_degrees() < 10.0 {
            out.push(format!(
                "breakover {:.0}° — the belly catches on intersection crowns",
                self.breakover_angle.to_degrees()
            ));
        }
        let ends = self.approach_angle.min(self.departure_angle).to_degrees();
        if ends < 15.0 {
            out.push(format!(
                "approach/departure {ends:.0}° — the overhangs catch on incline changes"
            ));
        }
        for (i, w) in self.wheels.iter().enumerate() {
            if w.bottoms_out {
                out.push(format!("wheel {i} rests on the bump stops"));
            }
            if w.damping_ratio < 0.3 {
                out.push(format!(
                    "wheel {i} damping ratio {:.2} — the car will pogo",
                    w.damping_ratio
                ));
            }
            if w.damping_ratio > 1.3 {
                out.push(format!(
                    "wheel {i} damping ratio {:.2} — the suspension is locked solid",
                    w.damping_ratio
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_settles_on_its_springs() {
        let m = HandlingMetrics::of(&VehicleConfig::default());
        assert_eq!(m.wheels.len(), 4);
        for w in &m.wheels {
            assert!(!w.bottoms_out);
            assert!(w.sag_fraction > 0.0 && w.sag_fraction < 1.0);
            assert!(w.natural_frequency > 0.5 && w.natural_frequency < 3.0);
            assert!(w.damping_ratio > 0.3 && w.damping_ratio < 1.3);
        }
        assert!(m.peak_lateral_g > 1.0);
        assert!(m.tip_threshold_g > 1.0);
    }

    #[test]
    fn roll_resistance_is_what_keeps_the_default_car_on_its_wheels() {
        let mut cfg = VehicleConfig::default();

        // Without the assist the default car reaches its tipping point at
        // the very moment its tires reach their limit — every hard corner
        // is a coin flip between sliding and rolling.
        cfg.assists.roll_resistance = 0.0;
        let bare = HandlingMetrics::of(&cfg);
        assert!(
            bare.rollover_margin < 1.1,
            "margin without the assist {:.2}",
            bare.rollover_margin
        );

        cfg.assists.roll_resistance = default_assists().roll_resistance;
        let assisted = HandlingMetrics::of(&cfg);
        assert!(
            assisted.rollover_margin > 2.0,
            "margin with the assist {:.2}",
            assisted.rollover_margin
        );
        // The assist changes only the lever arm, never the grip.
        assert_eq!(bare.peak_lateral_g, assisted.peak_lateral_g);
        assert_eq!(bare.tip_threshold_g, assisted.tip_threshold_g);
    }

    fn default_assists() -> crate::config::AssistConfig {
        VehicleConfig::default().assists
    }

    #[test]
    fn a_tall_narrow_car_is_reported_as_tippy() {
        let mut cfg = VehicleConfig::default();
        cfg.center_of_mass[1] += 1.0;
        cfg.track_width = 1.2;
        cfg.assists.roll_resistance = 0.0;
        let m = HandlingMetrics::of(&cfg);
        assert!(m.tip_threshold_g < m.peak_lateral_g);
        assert!(m.rollover_margin < 1.0);
        assert!(
            m.problems().iter().any(|p| p.contains("tips before")),
            "{:?}",
            m.problems()
        );
    }

    #[test]
    fn com_height_is_measured_from_the_settled_contact_patch() {
        let cfg = VehicleConfig::default();
        let m = HandlingMetrics::of(&cfg);
        // The default rig hangs its hardpoints 0.15 m below the origin with
        // 0.34 m wheels, so the ground sits well below the body.
        assert!(m.ground_y < 0.0, "ground at {}", m.ground_y);
        assert!(
            (m.com_height - (cfg.center_of_mass[1] - m.ground_y)).abs() < 1e-5,
            "com height {} inconsistent with ground {}",
            m.com_height,
            m.ground_y
        );
    }

    #[test]
    fn a_low_slung_hull_between_distant_axles_has_a_shallow_breakover() {
        let mut cfg = VehicleConfig::default();
        let m0 = HandlingMetrics::of(&cfg);
        // A flat floor 5 cm above the contact plane, spanning the wheelbase.
        let floor = m0.ground_y + 0.05;
        cfg.collider_points = Some(vec![
            [-0.9, floor, -2.0],
            [0.9, floor, -2.0],
            [-0.9, floor, 2.0],
            [0.9, floor, 2.0],
            [-0.9, floor + 1.0, -2.0],
            [0.9, floor + 1.0, -2.0],
            [-0.9, floor + 1.0, 2.0],
            [0.9, floor + 1.0, 2.0],
        ]);
        let m = HandlingMetrics::of(&cfg);
        assert!((m.belly_clearance - 0.05).abs() < 1e-3);
        assert!(
            m.breakover_angle.to_degrees() < 10.0,
            "breakover {} deg",
            m.breakover_angle.to_degrees()
        );
        assert!(
            m.approach_angle.to_degrees() < 20.0,
            "approach {} deg",
            m.approach_angle.to_degrees()
        );
    }

    #[test]
    fn a_pogo_suspension_is_reported() {
        let mut cfg = VehicleConfig::default();
        cfg.suspension.damping_rebound = 200.0;
        let m = HandlingMetrics::of(&cfg);
        assert!(m.wheels[0].damping_ratio < 0.3);
        assert!(m.problems().iter().any(|p| p.contains("pogo")));
    }
}
