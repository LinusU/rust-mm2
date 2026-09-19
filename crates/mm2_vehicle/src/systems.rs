//! Physics systems driving the vehicle each physics step.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::sim;
use crate::vehicle::{
    DriveDirection, ResetVehicle, Vehicle, VehicleInput, VehicleState, WheelState,
};

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

        // Countersteer assist: during a slide, steer toward the velocity
        // direction by a fraction of the body slip angle.
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
            DriveDirection::Forward
                if fwd_speed <= 0.25 && input.brake > 0.05 && input.throttle < 0.05 =>
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

        // First pass: probes + suspension, collecting per-wheel data.
        let mut grounded_any = false;
        let wheel_count = cfg.wheels.len().min(state.wheels.len());
        for i in 0..wheel_count {
            let wheel = &cfg.wheels[i];
            let ws = &mut state.wheels[i];
            let hardpoint = pos + rot * Vec3::from(wheel.position);
            let cast_len = cfg.suspension.travel + wheel.radius;
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
            let compression = (cast_len - hit.distance).clamp(0.0, cfg.suspension.travel);
            if compression <= 0.0 {
                continue;
            }
            let contact = hardpoint + dir * hit.distance;
            let normal = hit.normal.normalize_or_zero();

            // Velocity at the contact point.
            let contact_vel = forces.velocity_at_point(contact);
            let closing = -contact_vel.dot(normal); // >0 = compressing

            let damping = if closing > 0.0 {
                cfg.suspension.damping_compression
            } else {
                cfg.suspension.damping_rebound
            };
            let mut sus_force = cfg.suspension.spring_rate * compression + damping * closing;
            sus_force = sus_force.clamp(0.0, cfg.suspension.max_force);

            ws.grounded = true;
            grounded_any = true;
            ws.contact_point = contact;
            ws.contact_normal = normal;
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
        let new_gear =
            sim::select_gear(state.gear, wheel_rps.abs(), &cfg.transmission, &cfg.engine);
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

        // --- second pass: tire forces -----------------------------------------
        let reference_load = cfg.mass * 9.81 / wheel_count.max(1) as f32;
        for i in 0..wheel_count {
            let wheel = &cfg.wheels[i];
            let ws = state.wheels[i];
            if !ws.grounded {
                continue;
            }

            // Tire frame: forward rotated by the steer angle about the chassis
            // up axis, then projected onto the contact plane. Positive steer
            // angle turns right, i.e. forward tips toward +X.
            let steer = if wheel.steered {
                state.steer_angle
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

            let load = sim::load_adjusted_grip(ws.suspension_force, reference_load, &cfg.tires);
            let traction_limit = cfg.tires.longitudinal_grip * load;

            // Handbrake reduces lateral bite on locked wheels — that's what
            // makes the back step out.
            let hb = if wheel.handbrake {
                input.handbrake
            } else {
                0.0
            };
            let lateral = sim::lateral_force(slip, load, &cfg.tires) * (1.0 - 0.6 * hb);

            // Longitudinal: engine / reverse / engine-brake / brakes /
            // rolling resistance. The drive request is computed separately
            // so traction control can cap just the power side.
            let mut longitudinal = sim::rolling_resistance(vel_long, load, &cfg.tires);
            let mut drive_request = 0.0f32;
            if wheel.driven && input.throttle > 0.0 && !reversing && state.shifting <= 0.0 {
                let torque = sim::engine_torque(state.rpm, &cfg.engine);
                let ratio = cfg
                    .transmission
                    .gear_ratios
                    .get(state.gear)
                    .copied()
                    .unwrap_or(1.0);
                let wheel_torque =
                    torque * ratio * cfg.transmission.final_drive * cfg.transmission.efficiency;
                drive_request +=
                    wheel_torque / wheel.radius / driven_count.max(1) as f32 * input.throttle;
            }
            if wheel.driven && reversing {
                let torque = sim::engine_torque(state.rpm.max(cfg.engine.idle_rpm), &cfg.engine);
                let wheel_torque = torque
                    * cfg.transmission.reverse_ratio
                    * cfg.transmission.final_drive
                    * cfg.transmission.efficiency;
                drive_request -=
                    wheel_torque / wheel.radius / driven_count.max(1) as f32 * input.brake;
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
                longitudinal +=
                    -vel_long.signum() * wheel_torque / wheel.radius / driven_count.max(1) as f32;
            }

            // Traction control caps the drive request to a fraction of the
            // tire's limit (0 = off). It only governs the power side.
            if cfg.assists.traction_control > 0.0 {
                let cap = traction_limit * cfg.assists.traction_control;
                drive_request = drive_request.clamp(-cap, cap);
            }
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
            longitudinal += -vel_long.signum()
                * hb
                * cfg.brakes.max_brake_force
                * cfg.brakes.handbrake_strength;

            // Combined force is limited by the tire's traction curve (see
            // `sim::longitudinal_force`); over-demand slides rather than
            // hard-clamping.
            let (longitudinal_clamped, demand_ratio) =
                sim::longitudinal_force(longitudinal, load, &cfg.tires, 1.0);

            // Friction ellipse: combined force can't exceed μ·load.
            let total = (lateral * lateral + longitudinal_clamped * longitudinal_clamped).sqrt();
            let max_total = cfg.tires.lateral_grip * load;
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
            let sus_point = ws.contact_point
                + ws.contact_normal * cfg.suspension.force_apply_offset * wheel.radius;
            forces.apply_force_at_point(ws.contact_normal * ws.suspension_force, sus_point);
            forces.apply_force_at_point(
                tire_right * lateral + tire_fwd * longitudinal,
                ws.contact_point,
            );

            let ws = &mut state.wheels[i];
            ws.vel_long = vel_long;
            ws.vel_lat = vel_lat;
            ws.slip_angle = slip;
            ws.traction_demand = demand_ratio;
            ws.lateral_force = lateral;
            ws.longitudinal_force = longitudinal;
            ws.spin += (vel_long / wheel.radius.max(0.01)) * dt;
        }

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

        // Air control: gently level the car while airborne.
        if !grounded_any && cfg.assists.air_control > 0.0 {
            let axis = up.cross(Vec3::Y);
            let torque = axis * cfg.assists.air_control * inertia_y
                - angvel * cfg.assists.air_control * 0.4 * inertia_y;
            forces.apply_torque(torque.clamp_length_max(inertia_y * 20.0));
        }
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
pub fn vehicle_reset(mut events: MessageReader<ResetVehicle>, mut vehicles: ResetQuery) {
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
        }
    }
}
