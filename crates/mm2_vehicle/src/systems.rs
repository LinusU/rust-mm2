//! Physics systems driving the vehicle each physics step.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::sim;
use crate::vehicle::{
    DriveDirection, ResetVehicle, Teleported, Vehicle, VehicleInput, VehicleState, WheelState,
};

/// What the steered axle can do with steering lock: how much grip it makes
/// and how much slip it needs to make it.
pub(crate) struct FrontAxle {
    /// Mean lateral grip coefficient of the steered tires.
    pub lateral_grip: f32,
    /// Slip the steered tires need beyond what the unsteered ones are
    /// already giving — the understeer term of the bicycle model.
    pub slip_allowance: f32,
}

impl FrontAxle {
    pub(crate) fn of(cfg: &crate::config::VehicleConfig) -> Self {
        let mut grip = (0.0f32, 0usize);
        let mut steered_slip = (0.0f32, 0usize);
        let mut fixed_slip = (0.0f32, 0usize);
        for w in &cfg.wheels {
            let t = w.tires.as_ref().unwrap_or(&cfg.tires);
            if w.steered {
                grip.0 += t.lateral_grip;
                grip.1 += 1;
                steered_slip.0 += t.peak_slip_angle;
                steered_slip.1 += 1;
            } else {
                fixed_slip.0 += t.peak_slip_angle;
                fixed_slip.1 += 1;
            }
        }
        let mean = |(sum, n): (f32, usize), fallback: f32| {
            if n == 0 { fallback } else { sum / n as f32 }
        };
        let front_slip = mean(steered_slip, cfg.tires.peak_slip_angle);
        let rear_slip = mean(fixed_slip, front_slip);
        Self {
            lateral_grip: mean(grip, cfg.tires.lateral_grip),
            slip_allowance: (front_slip - rear_slip).max(0.0),
        }
    }
}

/// Angular inertia about a horizontal axis, kg·m² — the mean of the
/// pitch and roll principal moments, which is what levelling the car in
/// the air works against.
fn leveling_inertia(cfg: &crate::config::VehicleConfig) -> f32 {
    let [ix, _, iz] = cfg.inertia.unwrap_or_else(|| {
        let [w, h, d] = cfg.chassis_size;
        let m = cfg.mass;
        [
            m * (h * h + d * d) / 12.0,
            m * (w * w + d * d) / 12.0,
            m * (w * w + h * h) / 12.0,
        ]
    });
    (ix + iz) * 0.5
}

/// Vehicle raycasts exclude the vehicle's own collider.
fn wheel_filter(vehicle: Entity) -> SpatialQueryFilter {
    SpatialQueryFilter::default().with_excluded_entities([vehicle])
}

type VehicleQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Vehicle,
        &'static mut VehicleState,
        &'static VehicleInput,
        Forces,
    ),
>;

/// Core simulation: runs inside [`PhysicsSchedule`] at the fixed physics rate.
pub fn vehicle_simulation(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut vehicles: VehicleQuery,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, vehicle, mut state, input, mut forces) in &mut vehicles {
        let cfg = &vehicle.config;
        let pos = forces.position().0;
        let rot = forces.rotation().0;
        let linvel = forces.linear_velocity();
        let angvel = forces.angular_velocity();
        let com_world = pos + rot * Vec3::from(cfg.center_of_mass);
        let forward = rot * Vec3::NEG_Z;
        let right = rot * Vec3::X;
        let up = rot * Vec3::Y;

        let vel = linvel;
        let fwd_speed = vel.dot(forward);
        state.forward_speed = fwd_speed;

        // --- steering target -------------------------------------------------
        let shaped = sim::steering_response(input.steering, cfg.steering.response_curve);
        let max_angle = sim::max_steer_angle(fwd_speed.abs(), &cfg.steering);
        let mut target = shaped * max_angle;

        // Grip-limited lock: an angle demanding far more cornering force
        // than the tires can make does not turn harder, it just ploughs.
        // Capping by what the tires can deliver gives a speed-sensitive
        // lock derived from the car rather than authored.
        let front = FrontAxle::of(cfg);
        let grip_cap = sim::grip_limited_steer_angle(
            fwd_speed.abs(),
            cfg.wheelbase,
            front.lateral_grip,
            cfg.steering.grip_limit,
            front.slip_allowance,
        );
        target = target.clamp(-grip_cap, grip_cap);

        // Countersteer assist: during a slide, steer toward the velocity
        // direction by a fraction of the body slip angle. It is added
        // after the grip cap on purpose — a car already sideways is not
        // the steady-state corner the cap models, and catching it needs
        // more lock than that corner would.
        let lat_speed = vel.dot(right);
        let body_slip = sim::slip_angle(fwd_speed.abs().max(0.5), lat_speed);
        if fwd_speed.abs() > 5.0 && body_slip.abs() > 0.1 {
            target += body_slip * cfg.assists.countersteer;
            target = target.clamp(-max_angle, max_angle);
        }
        state.steer_angle = sim::step_steer_angle(state.steer_angle, target, &cfg.steering, dt);

        // --- drivetrain direction --------------------------------------------
        // Deliberate, hysteretic policy (see `DriveDirection`): the brake
        // pedal only becomes "reverse throttle" once the car is essentially
        // stopped; throttle or releasing the brake selects forward again.
        // Speed is *not* part of the exit condition, so holding the pedal
        // through a standstill cannot oscillate between brake and reverse.
        match state.direction {
            // Trailers have no reverse gear: brake input stays a brake.
            DriveDirection::Forward
                if !cfg.trailer
                    && fwd_speed <= 0.25
                    && input.brake > 0.05
                    && input.throttle < 0.05 =>
            {
                state.direction = DriveDirection::Reverse;
            }
            DriveDirection::Reverse if input.throttle > 0.05 || input.brake < 0.05 => {
                state.direction = DriveDirection::Forward;
            }
            _ => {}
        }
        let reversing = state.direction == DriveDirection::Reverse;

        // --- drivetrain bookkeeping ------------------------------------------
        let mut driven_ground_speed = 0.0f32;
        let mut driven_count = 0usize;
        let filter = wheel_filter(entity);

        // Drive-torque shares: explicit per-wheel fractions when configured,
        // otherwise an even split across driven wheels.
        let wheel_count_cfg = cfg.wheels.len();
        let share_sum: f32 = cfg.wheels.iter().filter_map(|w| w.drive_share).sum();
        let use_explicit_shares = share_sum > 0.0
            && cfg
                .wheels
                .iter()
                .all(|w| !w.driven || w.drive_share.is_some());
        let n_driven_cfg = cfg.wheels.iter().filter(|w| w.driven).count().max(1) as f32;

        // First pass: probes + suspension, collecting per-wheel data.
        let mut grounded_any = false;
        let wheel_count = wheel_count_cfg.min(state.wheels.len());
        for i in 0..wheel_count {
            let wheel = &cfg.wheels[i];
            let suspension = wheel.suspension.as_ref().unwrap_or(&cfg.suspension);
            let ws = &mut state.wheels[i];
            let hardpoint = pos + rot * Vec3::from(wheel.position);
            let cast_len = suspension.travel + wheel.radius;
            let dir = Dir3::new_unchecked((rot * Vec3::NEG_Y).normalize_or_zero());

            // A probe that loses contact must not keep reporting last step's
            // compression/forces/velocities — only the visual spin persists.
            *ws = WheelState {
                contact_normal: Vec3::Y,
                spin: ws.spin,
                ..Default::default()
            };

            let Some(hit) = spatial_query.cast_ray(hardpoint, dir, cast_len, true, &filter) else {
                continue;
            };
            let compression = (cast_len - hit.distance).clamp(0.0, suspension.travel);
            if compression <= 0.0 {
                continue;
            }
            let contact = hardpoint + dir * hit.distance;
            let normal = hit.normal.normalize_or_zero();

            // Velocity at the contact point.
            let contact_vel = forces.velocity_at_point(contact);
            let closing = -contact_vel.dot(normal); // >0 = compressing

            let damping = if closing > 0.0 {
                suspension.damping_compression
            } else {
                suspension.damping_rebound
            };
            let mut sus_force = suspension.spring_rate * compression + damping * closing;
            sus_force = sus_force.clamp(0.0, suspension.max_force);

            ws.grounded = true;
            grounded_any = true;
            ws.contact_point = contact;
            ws.contact_normal = normal;
            ws.contact_entity = Some(hit.entity);
            ws.compression = compression;
            ws.suspension_force = sus_force;

            if wheel.driven {
                driven_ground_speed += contact_vel.dot(forward);
                driven_count += 1;
            }
        }
        state.grounded = grounded_any;

        // --- engine RPM / gear from average driven wheel speed ---------------
        let mean_wheel_speed = if driven_count > 0 {
            driven_ground_speed / driven_count as f32
        } else {
            fwd_speed
        };
        let wheel_radius = cfg
            .wheels
            .iter()
            .find(|w| w.driven)
            .or_else(|| cfg.wheels.first())
            .map(|w| w.radius)
            .unwrap_or(0.34)
            .max(0.01);
        let wheel_rps = mean_wheel_speed / (std::f32::consts::TAU * wheel_radius);
        // `forced_gear` is an explicit control command (AI/network/dev
        // tuning): it pins the gearbox instead of running the selector.
        let new_gear = match input.forced_gear {
            Some(g) => g.min(cfg.transmission.gear_ratios.len().saturating_sub(1)),
            None => sim::select_gear(state.gear, wheel_rps.abs(), &cfg.transmission, &cfg.engine),
        };
        if new_gear != state.gear {
            state.shifting = cfg.transmission.shift_time;
        }
        state.gear = new_gear;
        // RPM follows the wheel-implied value at `rpm_response` (1/s)
        // rather than snapping — smooths shift blips and load changes.
        let target_rpm =
            sim::engine_rpm(wheel_rps.abs(), state.gear, &cfg.transmission, &cfg.engine);
        state.rpm += (target_rpm - state.rpm) * (1.0 - (-cfg.engine.rpm_response * dt).exp());
        state.shifting = (state.shifting - dt).max(0.0);

        // A gear change interrupts drive rather than switching it off; see
        // `SHIFT_TORQUE_FLOOR`. `shifting` counts down from `shift_time`,
        // so this ramps from the floor back to full as the gear engages.
        let shift_torque = if state.shifting > 0.0 && cfg.transmission.shift_time > 0.0 {
            let remaining = (state.shifting / cfg.transmission.shift_time).clamp(0.0, 1.0);
            crate::config::SHIFT_TORQUE_FLOOR
                + (1.0 - crate::config::SHIFT_TORQUE_FLOOR) * (1.0 - remaining)
        } else {
            1.0
        };

        // --- second pass: tire forces -----------------------------------------
        let reference_load = cfg.mass * 9.81 / wheel_count.max(1) as f32;
        // `engine_load` bookkeeping: what the drivetrain could deliver at
        // this rpm/gear across the contacting driven wheels vs. what was
        // actually requested (post-traction-control).
        let mut drive_available = 0.0f32;
        let mut drive_delivered = 0.0f32;
        for i in 0..wheel_count {
            let wheel = &cfg.wheels[i];
            let ws = state.wheels[i];
            if !ws.grounded {
                continue;
            }

            let tires = wheel.tires.as_ref().unwrap_or(&cfg.tires);

            // Tire frame: forward rotated by the steer angle about the chassis
            // up axis, then projected onto the contact plane. Positive steer
            // angle turns right, i.e. forward tips toward +X.
            let steer = if wheel.steered {
                state.steer_angle * wheel.steer_scale
            } else {
                0.0
            };
            let tire_fwd_flat = Quat::from_axis_angle(up, -steer) * forward;
            let n = ws.contact_normal;
            let tire_fwd = (tire_fwd_flat - n * tire_fwd_flat.dot(n)).normalize_or_zero();
            let tire_right = tire_fwd.cross(n).normalize_or_zero();

            let contact_vel = forces.velocity_at_point(ws.contact_point);
            let vel_long = contact_vel.dot(tire_fwd);
            let vel_lat = contact_vel.dot(tire_right);
            let slip = sim::slip_angle(vel_long, vel_lat);

            let load = sim::load_adjusted_grip(ws.suspension_force, reference_load, tires);
            let traction_limit = tires.longitudinal_grip * load;

            // This wheel's share of total drive torque (0 for undriven).
            let drive_share = if !wheel.driven {
                0.0
            } else if use_explicit_shares {
                wheel.drive_share.unwrap_or(0.0) / share_sum
            } else {
                1.0 / n_driven_cfg
            };

            // Handbrake reduces lateral bite on locked wheels — that's what
            // makes the back step out.
            let hb = if wheel.handbrake {
                input.handbrake
            } else {
                0.0
            };
            let lateral = sim::lateral_force(slip, load, tires) * (1.0 - 0.6 * hb);

            // What the drivetrain could push through this wheel at the
            // current rpm/gear — the `engine_load` denominator. Uses the
            // same torque/ratio/direction terms as the request below.
            let wheel_drive_available = if wheel.driven {
                let (torque, ratio) = if reversing {
                    (
                        sim::engine_torque(state.rpm.max(cfg.engine.idle_rpm), &cfg.engine),
                        cfg.transmission.reverse_ratio,
                    )
                } else {
                    (
                        sim::engine_torque(state.rpm, &cfg.engine),
                        cfg.transmission
                            .gear_ratios
                            .get(state.gear)
                            .copied()
                            .unwrap_or(1.0),
                    )
                };
                torque * ratio * cfg.transmission.final_drive * cfg.transmission.efficiency
                    / wheel.radius
                    * drive_share
                    * shift_torque
            } else {
                0.0
            };
            drive_available += wheel_drive_available;

            // Longitudinal: engine / reverse / engine-brake / brakes /
            // rolling resistance. The drive request is computed separately
            // so traction control can cap just the power side.
            let mut longitudinal = sim::rolling_resistance(vel_long, load, tires);
            let mut drive_request = 0.0f32;
            if wheel.driven && input.throttle > 0.0 && !reversing {
                drive_request += wheel_drive_available * input.throttle;
            }
            if wheel.driven && reversing {
                drive_request -= wheel_drive_available * input.brake;
            }
            // Engine braking through the driven wheels at closed throttle.
            if wheel.driven
                && !reversing
                && input.throttle <= 0.0
                && vel_long.abs() > 0.5
                && cfg.engine.engine_brake_nm > 0.0
            {
                let ratio = cfg
                    .transmission
                    .gear_ratios
                    .get(state.gear)
                    .copied()
                    .unwrap_or(1.0);
                let wheel_torque = cfg.engine.engine_brake_nm
                    * ratio
                    * cfg.transmission.final_drive
                    * cfg.transmission.efficiency;
                longitudinal += -vel_long.signum() * wheel_torque / wheel.radius * drive_share;
            }

            // Traction control caps the drive request to a fraction of the
            // tire's limit (0 = off). It only governs the power side.
            if cfg.assists.traction_control > 0.0 {
                let cap = traction_limit * cfg.assists.traction_control;
                drive_request = drive_request.clamp(-cap, cap);
            }
            drive_delivered += drive_request;
            longitudinal += drive_request;

            // Foot brake only in forward direction — in reverse the pedal is
            // the throttle, not a brake.
            if !reversing {
                longitudinal += -vel_long.signum()
                    * input.brake
                    * cfg.brakes.max_brake_force
                    * wheel.brake_bias;
            }
            // Handbrake locks the wheel hard.
            let hb_strength = wheel
                .handbrake_coef
                .unwrap_or(cfg.brakes.handbrake_strength);
            longitudinal += -vel_long.signum() * hb * cfg.brakes.max_brake_force * hb_strength;

            // Combined force is limited by the tire's traction curve (see
            // `sim::longitudinal_force`); over-demand slides rather than
            // hard-clamping.
            let (longitudinal_clamped, demand_ratio) =
                sim::longitudinal_force(longitudinal, load, tires, 1.0);

            // Friction ellipse: combined force can't exceed μ·load.
            let total = (lateral * lateral + longitudinal_clamped * longitudinal_clamped).sqrt();
            let max_total = tires.lateral_grip * load;
            let (lateral, longitudinal) = if total > max_total && total > 0.0 {
                (
                    lateral * max_total / total,
                    longitudinal_clamped * max_total / total,
                )
            } else {
                (lateral, longitudinal_clamped)
            };

            // Suspension force is applied slightly above the contact patch
            // (fraction of the wheel radius) so roll stays plausible.
            let suspension = wheel.suspension.as_ref().unwrap_or(&cfg.suspension);
            let sus_point =
                ws.contact_point + ws.contact_normal * suspension.force_apply_offset * wheel.radius;
            forces.apply_force_at_point(ws.contact_normal * ws.suspension_force, sus_point);

            // Lateral force is raised toward the centre of mass by the
            // roll-resistance assist, shortening the lever that tips the
            // car (see `AssistConfig::roll_resistance`). Raising it along
            // the contact normal leaves the yaw moment untouched, so the
            // car still turns exactly as hard — it just stops rolling over
            // to do it. Measuring the height against the *contact normal*
            // rather than world up keeps this correct on banked road.
            let com_above_patch = (com_world - ws.contact_point)
                .dot(ws.contact_normal)
                .max(0.0);
            let lateral_point = ws.contact_point
                + ws.contact_normal * com_above_patch * cfg.assists.roll_resistance;
            forces.apply_force_at_point(tire_right * lateral, lateral_point);
            // Longitudinal force stays at the patch: squat and dive under
            // power and braking are wanted, and they do not tip the car.
            forces.apply_force_at_point(tire_fwd * longitudinal, ws.contact_point);

            let ws = &mut state.wheels[i];
            ws.vel_long = vel_long;
            ws.vel_lat = vel_lat;
            ws.slip_angle = slip;
            ws.traction_demand = demand_ratio;
            ws.lateral_force = lateral;
            ws.longitudinal_force = longitudinal;
            ws.spin += (vel_long / wheel.radius.max(0.01)) * dt;
        }

        // Delivered vs. available drive force — 0 when no driven wheel
        // touches the ground, ~1 at full demand.
        state.engine_load = if drive_available > 1.0 {
            (drive_delivered / drive_available).abs().clamp(0.0, 1.0)
        } else {
            0.0
        };

        // --- chassis-level forces ---------------------------------------------
        // Aero drag + downforce at the centre of mass.
        let drag = -vel * cfg.aero.drag_coefficient * vel.length() * 0.5;
        forces.apply_force_at_point(drag, com_world);
        forces.apply_force_at_point(
            Vec3::NEG_Y * cfg.aero.downforce_coefficient * vel.length_squared() * 0.5,
            com_world,
        );

        // Yaw stability: damp yaw rate when the car is gripping (less while
        // sliding or handbraking, so drifts stay possible).
        let inertia_y = cfg.mass * cfg.wheelbase * cfg.wheelbase / 12.0;
        let drift_factor = 1.0 / (1.0 + body_slip.abs() * 8.0 + input.handbrake * 4.0);
        let yaw_torque = -angvel.y * cfg.assists.yaw_stability * inertia_y * drift_factor;
        forces.apply_torque(Vec3::Y * yaw_torque);

        // Air control: level the car before it lands.
        //
        // A car that crests a rise at speed leaves it nose-down and keeps
        // that attitude all the way to the ground, so the front of the
        // hull arrives ahead of the wheels, digs into the road and stops
        // the car dead instead of landing it. Levelling is what makes a
        // jump survivable.
        //
        // `air_control` is the natural frequency (rad/s) of a critically
        // damped rotation of the car's up axis back to vertical: it
        // settles in roughly `4 / air_control` seconds. Scaling by the
        // car's own pitch/roll inertia is what makes that figure mean the
        // same thing on a Mini and on a fire truck.
        if !grounded_any && cfg.assists.air_control > 0.0 {
            let w = cfg.assists.air_control;
            // `up × Y` is horizontal and vanishes when the car is level,
            // so this rights pitch and roll without ever fighting a
            // deliberate spin.
            let tilt = up.cross(Vec3::Y);
            let level_rate = angvel - Vec3::Y * angvel.y;
            forces.apply_torque(leveling_inertia(cfg) * (tilt * w * w - level_rate * 2.0 * w));
        }
    }
}

/// Cosine of the tilt past which a car counts as upended: an up axis
/// leaning more than ~70 degrees off vertical is on its side or roof, not
/// merely cresting a steep bank.
const UPENDED_TILT: f32 = 0.35;
/// A car still sliding is still in play; recovery waits for it to stop.
const UPENDED_MAX_SPEED: f32 = 2.0;

type SelfRightQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Vehicle,
        &'static mut VehicleState,
        &'static mut Position,
        &'static mut Rotation,
        &'static mut LinearVelocity,
        &'static mut AngularVelocity,
        &'static mut Transform,
    ),
>;

/// Flop an upended car back onto its wheels once it has come to rest.
///
/// Without this a roll is the end of the drive: a car on its roof has no
/// wheel in contact, so nothing in the simulation can push it back over
/// and the player is stranded. Recovery keeps the car's heading and drops
/// it upright on whatever surface is below it.
pub fn vehicle_self_right(time: Res<Time>, mut vehicles: SelfRightQuery) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (vehicle, mut state, mut pos, mut rot, mut lv, mut av, mut transform) in &mut vehicles {
        let delay = vehicle.config.assists.self_right_delay;
        if delay <= 0.0 {
            state.upended_for = 0.0;
            continue;
        }
        let upright = (rot.0 * Vec3::Y).y;
        if upright > UPENDED_TILT || lv.0.length() > UPENDED_MAX_SPEED {
            state.upended_for = 0.0;
            continue;
        }
        state.upended_for += dt;
        if state.upended_for < delay {
            continue;
        }

        // Keep the heading, discard every other rotation.
        let yaw = {
            let f = rot.0 * Vec3::NEG_Z;
            // On its roof the forward axis can point steeply up or down;
            // its horizontal part still says which way the car faces.
            let flat = Vec3::new(f.x, 0.0, f.z);
            if flat.length_squared() > 1e-6 {
                (-flat.x).atan2(-flat.z)
            } else {
                0.0
            }
        };
        let level = Quat::from_rotation_y(yaw);

        // Drop it onto whatever is underneath rather than guessing a
        // height: the car may be on a bridge, a kerb or a hillside. The
        // resting hull is already touching that surface, so its lowest
        // corner says where the surface is — no raycast needed, which also
        // keeps this system's mutable `Position` clear of `SpatialQuery`.
        let surface_y = crate::analysis::hull_points(&vehicle.config)
            .iter()
            .map(|p| (pos.0 + rot.0 * Vec3::from(*p)).y)
            .fold(f32::MAX, f32::min);
        let ground_y = crate::analysis::HandlingMetrics::of(&vehicle.config).ground_y;

        pos.0.y = surface_y - ground_y + 0.05;
        rot.0 = level;
        lv.0 = Vec3::ZERO;
        av.0 = Vec3::ZERO;
        transform.translation = pos.0;
        transform.rotation = level;
        state.upended_for = 0.0;
    }
}

type ResetQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Vehicle,
        &'static mut Position,
        &'static mut Rotation,
        &'static mut LinearVelocity,
        &'static mut AngularVelocity,
        &'static mut Transform,
        &'static mut VehicleState,
    ),
>;

/// Handle [`ResetVehicle`] messages: teleport + clear motion state.
/// Every teleported entity is marked [`Teleported`] in the same pass so
/// swept-segment consumers (the race runtime, F11-B) re-anchor instead
/// of counting the jump as a crossing — the marker and the `Position`
/// write are atomic, so the break can never land a step early or be
/// missed outright.
pub fn vehicle_reset(
    mut commands: Commands,
    mut events: MessageReader<ResetVehicle>,
    mut vehicles: ResetQuery,
) {
    for ev in events.read() {
        for (entity, vehicle, mut pos, mut rot, mut lv, mut av, mut transform, mut state) in
            &mut vehicles
        {
            if let Some(target) = ev.entity
                && target != entity
            {
                continue;
            }
            pos.0 = ev.position;
            rot.0 = Quat::from_rotation_y(ev.yaw);
            lv.0 = Vec3::ZERO;
            av.0 = Vec3::ZERO;
            transform.translation = ev.position;
            transform.rotation = Quat::from_rotation_y(ev.yaw);
            *state = VehicleState::new(&vehicle.config);
            commands.entity(entity).insert(Teleported);
        }
    }
}
