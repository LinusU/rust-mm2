//! One physics step of the original car model ([`crate::original`]) on
//! an Avian body.
//!
//! The order is the original's `vehCar::Update`: inputs to the wheels,
//! engine and gearbox, aero, then each drivetrain integrating its spin
//! against last step's tyre reactions before its wheels make this step's
//! tyre forces, and the gyro last. The adapter integrates cached body
//! forces once before Avian advances the pose and solves its contacts;
//! the car's body has no damping beyond `vehAero`'s.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::config::{OriginalHandling, VehicleConfig};
use crate::original::{
    self, Bristles, Coupling, EngineConstants, FIRST_GEAR, MPS_TO_MPH, REVERSE_GEAR, TrainInput,
    TyreInput, WheelConstants,
};
use crate::surface::{TireConditions, TireSurface};
use crate::vehicle::{
    DriveDirection, HumanDriver, OriginalState, Vehicle, VehicleInput, VehicleState, WheelState,
};

/// What a step reads from the world besides the car itself.
pub(crate) struct StepWorld<'a, 'w, 's> {
    pub spatial: &'a SpatialQuery<'w, 's>,
    pub surfaces: &'a Query<'w, 's, &'static TireSurface>,
    pub conditions: &'a TireConditions,
    pub dt: f32,
}

/// Forward speed below which a human holding the brake is put into
/// reverse (`mmGame::ApplyPlayerInput`), m/s.
const AUTO_REVERSE_SPEED: f32 = 5.0;
/// Brake level that engages auto-reverse, and that the (swapped)
/// reverse pedal must stay above to keep it.
const AUTO_REVERSE_PEDAL: f32 = 0.8;
/// Throttle level above which a held brake does not engage reverse.
const AUTO_REVERSE_THROTTLE: f32 = 0.1;
/// Speed below which a human's car is held on the handbrake while off
/// the throttle (`mmPlayer::Update`), mph.
const STOPPED_HOLD_MPH: f32 = 4.0;
/// A car without auto reverse (AI, or a human who turned it off) reverses
/// once stopped this nearly, m/s, with the brake past
/// [`AI_REVERSE_PEDAL`] and the throttle released.
const AI_REVERSE_SPEED: f32 = 0.25;
const AI_REVERSE_PEDAL: f32 = 0.05;

/// How far ahead of the car, in seconds of its ground speed, air
/// levelling looks for the ground it will land on.
const AIR_LOOKAHEAD: f32 = 0.25;
/// How far down air levelling looks for that ground, m; beyond it the
/// car levels to the horizon.
const AIR_GROUND_REACH: f32 = 40.0;
/// Ground steeper than this (normal `y`, about 35°) is a wall or a
/// bank, not something to land parallel to.
const AIR_GROUND_NORMAL_Y: f32 = 0.82;

/// One wheel's contact this step.
#[derive(Clone, Copy, Default)]
struct Contact {
    grounded: bool,
    point: Vec3,
    normal: Vec3,
    lateral: Vec3,
    forward: Vec3,
    entity: Option<Entity>,
    load: f32,
    mu_static: f32,
    mu_sliding: f32,
    drag: f32,
    grip: f32,
    implicit: f32,
    implicit_force: f32,
}

/// Body mass as seen at `point` along `dir`: what the bump stop pushes
/// against.
fn effective_mass(mass: f32, inv_inertia: Mat3, arm: Vec3, dir: Vec3) -> f32 {
    let rn = arm.cross(dir);
    1.0 / (1.0 / mass.max(1e-3) + rn.dot(inv_inertia * rn)).max(1e-6)
}

/// Step the original model for one car.
#[allow(clippy::too_many_arguments)] // the car, its tuning and state, its input and the world: one step's whole context
pub(crate) fn step(
    entity: Entity,
    cfg: &VehicleConfig,
    orig: &OriginalHandling,
    state: &mut VehicleState,
    input: &VehicleInput,
    human: Option<HumanDriver>,
    engine_scale: f32,
    forces: &mut impl WriteRigidBodyForces,
    world: &StepWorld,
) {
    let dt = world.dt;
    let pos = forces.position().0;
    let rot = forces.rotation().0;
    let linvel = forces.linear_velocity();
    let angvel = forces.angular_velocity();
    let right = rot * Vec3::X;
    let up = rot * Vec3::Y;
    let back = rot * Vec3::Z;
    let forward = -back;
    let com = pos + rot * Vec3::from(cfg.center_of_mass);
    let inertia = Vec3::from(cfg.inertia.unwrap_or([cfg.mass; 3]));
    let rot_m = Mat3::from_quat(rot);
    let inv_inertia =
        rot_m * Mat3::from_diagonal(inertia.max(Vec3::splat(1e-3)).recip()) * rot_m.transpose();

    let fwd_speed = linvel.dot(forward);
    let speed = fwd_speed.abs();
    let previous_speed = state.forward_speed.abs();
    state.forward_speed = fwd_speed;

    let mut body_force = Vec3::ZERO;
    let mut body_torque = Vec3::ZERO;
    let mut implicit_a = Mat3::ZERO;
    let mut implicit_b = Mat3::ZERO;
    let mut implicit_c = Mat3::ZERO;

    let os = state
        .original
        .get_or_insert_with(|| OriginalState::new(orig));
    if os.wheels.len() != orig.wheels.len() {
        *os = OriginalState::new(orig);
    }
    // ApplyPlayerInput runs before vehCarSim replaces its Speed with the
    // current pre-integration velocity. Our public telemetry is taken
    // after integration, so player decisions need one additional cache.
    let player_speed = os.player_speed;
    os.steering_speed = player_speed;
    os.player_speed = previous_speed;
    if let Some(momentum) = os.angular_momentum.as_mut() {
        // Avian contacts change angular velocity after our one retail
        // velocity update; retain their angular impulse in world momentum.
        let collision_delta = angvel - os.integrated_angular_velocity;
        *momentum += rot_m * Mat3::from_diagonal(inertia) * rot_m.transpose() * collision_delta;
    }
    let n_wheels = orig.wheels.len().min(cfg.wheels.len()).min(4);
    if n_wheels == 0 {
        return;
    }

    // --- inputs ------------------------------------------------------------
    let throttle_in = input.throttle.clamp(0.0, 1.0);
    let brake_in = input.brake.clamp(0.0, 1.0);
    let steering = input.steering.clamp(-1.0, 1.0);
    let auto_reverse = human.is_some_and(|h| h.auto_reverse) && input.forced_gear.is_none();
    let ai_reverse = human.is_none();
    if human.is_some() && !auto_reverse {
        // TRANS / Auto Reverse off clear the human pedal swap. The AI's
        // brake-to-reverse fallback does not apply to a manual driver.
        state.direction = DriveDirection::Forward;
    }
    let old_direction = state.direction;
    if !cfg.trailer {
        state.direction = match state.direction {
            DriveDirection::Forward
                if auto_reverse
                    && player_speed < AUTO_REVERSE_SPEED
                    && brake_in > AUTO_REVERSE_PEDAL
                    && throttle_in < AUTO_REVERSE_THROTTLE =>
            {
                DriveDirection::Reverse
            }
            DriveDirection::Forward
                if ai_reverse
                    && fwd_speed <= AI_REVERSE_SPEED
                    && brake_in > AI_REVERSE_PEDAL
                    && throttle_in < AI_REVERSE_PEDAL =>
            {
                DriveDirection::Reverse
            }
            DriveDirection::Reverse if auto_reverse && brake_in < AUTO_REVERSE_PEDAL => {
                DriveDirection::Forward
            }
            DriveDirection::Reverse
                if ai_reverse
                    && (throttle_in > AI_REVERSE_PEDAL || brake_in < AI_REVERSE_PEDAL) =>
            {
                DriveDirection::Forward
            }
            d => d,
        };
    }
    let reversing = state.direction == DriveDirection::Reverse;
    // ApplyPlayerInput writes pedals before it changes PedalsSwapped.
    // A human's newly selected reverse gear therefore uses the previous
    // pedal mapping for this sample; the swap takes effect next sample.
    let pedals_swapped = if human.is_some() {
        old_direction == DriveDirection::Reverse
    } else {
        reversing
    };
    let (throttle, brake) = if pedals_swapped {
        (brake_in, throttle_in)
    } else {
        (throttle_in, brake_in)
    };
    let mut handbrake = input.handbrake.clamp(0.0, 1.0);
    if human.is_some() && player_speed * MPS_TO_MPH < STOPPED_HOLD_MPH && throttle == 0.0 {
        handbrake = 1.0;
    }

    // Gear: reverse while reversing, first out of it, the manual box's
    // pinned gear, otherwise the automatic's own choice.
    let gearbox = &orig.gearbox;
    let pt = &mut os.powertrain;
    let forward_gears = gearbox.ratios.len().saturating_sub(FIRST_GEAR).max(1);
    if reversing {
        pt.set_gear(REVERSE_GEAR);
    } else if let Some(g) = input.forced_gear {
        pt.set_gear(FIRST_GEAR + g.min(forward_gears - 1));
    } else if pt.gear < FIRST_GEAR {
        pt.set_gear(FIRST_GEAR);
    }

    // Wheel angles and brake torques (`vehCarSim::Update` → `SetInputs`).
    let brake_rear = if brake > original::BRAKE_STAND_PEDAL
        && throttle > original::BRAKE_STAND_PEDAL
        && speed < original::BRAKE_STAND_SPEED
    {
        0.0
    } else {
        brake
    };
    let mut constants =
        [WheelConstants::of(&orig.wheels[0], cfg.wheels[0].radius, orig.gravity); 4];
    let mut steer = [0.0f32; 4];
    let mut brake_torque = [0.0f32; 4];
    for i in 0..n_wheels {
        let ow = &orig.wheels[i];
        let k = WheelConstants::of(ow, cfg.wheels[i].radius, orig.gravity);
        steer[i] = original::wheel_steer(ow, steering);
        let (b, hb) = if ow.rear {
            // The handbrake relieves the rear wheel on the outside of
            // the steer: full lock right leaves only the inside one
            // braked. (The original's right wheel also keeps a default
            // `HandbrakeCoef` of 1.0 whatever the file says — a copy
            // omission; the converter reproduces that constructor default.)
            let relief = if ow.side < 0.0 && steering > 0.0 {
                1.0 - steering
            } else if ow.side > 0.0 && steering < 0.0 {
                1.0 + steering
            } else {
                1.0
            };
            (brake_rear, handbrake * relief)
        } else {
            (brake, 0.0)
        };
        brake_torque[i] = hb * k.handbrake_max + b * k.brake_max;
        constants[i] = k;
    }
    let front_lock = orig
        .wheels
        .iter()
        .find(|w| !w.rear)
        .map_or(0.0, |w| w.steering_limit);
    state.steer_angle = steering * front_lock;

    // --- engine and gearbox -------------------------------------------------
    let engine = EngineConstants::of(&orig.engine);
    let ratio_now = gearbox.ratios.get(pt.gear).copied().unwrap_or(0.0);
    pt.update_engine(&engine, ratio_now, throttle, engine_scale, dt);
    pt.update_gearbox(
        gearbox,
        throttle,
        state.grounded,
        input.forced_gear.is_none(),
        dt,
    );
    let ratio = gearbox.ratios.get(pt.gear).copied().unwrap_or(0.0);

    // --- aero -----------------------------------------------------------------
    let mut alpha = original::aero_damping(rot.inverse() * angvel, &orig.aero, dt);
    // Retail fades each car-axis damping term by the world-axis rate.
    let local_omega = rot.inverse() * angvel;
    for axis in 0..3 {
        let local_fade = local_omega[axis].abs().min(1.0);
        if local_fade > 0.0 {
            alpha[axis] *= angvel[axis].abs().min(1.0) / local_fade;
        }
    }
    body_torque += rot * (alpha * inertia);
    body_force += -speed * orig.aero.drag * linvel - speed * speed * orig.aero.down * up;

    // --- contacts: probe, suspension, surface --------------------------------
    let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);
    let mut contacts = [Contact::default(); 4];
    // Wheels past their bump stop this step: contact point, normal and
    // how far the body has to move along it to sit back at the stop.
    let mut overshoots: Vec<(Vec3, Vec3, f32)> = Vec::new();
    for i in 0..n_wheels {
        let w = &cfg.wheels[i];
        let ow = &orig.wheels[i];
        let k = &constants[i];
        let ows = &mut os.wheels[i];
        let extent = ow.suspension_extent.max(1e-3);
        let limit = ow.suspension_limit.max(0.0);
        // The hardpoint sits at the top of the travel; the pivot — where
        // the wheel rests — `Limit` below it.
        let wheel_rotation = Quat::from_rotation_y(-steer[i]);
        let inner_edge = ow.side * ow.width * 0.5;
        let pivot_shift = inner_edge * (wheel_rotation * Vec3::X - Vec3::X);
        let pivot = Vec3::from(w.position) - Vec3::Y * limit + pivot_shift;
        let center = pos + rot * pivot;
        let wheel_right = rot * (wheel_rotation * Vec3::X);
        let above = limit + original::PROBE_ABOVE;
        let start = center + up * above;
        let length = extent + w.radius + above;
        let hit = Dir3::new(-up)
            .ok()
            .and_then(|dir| world.spatial.cast_ray(start, dir, length, true, &filter));
        let ground = hit.and_then(|h| {
            let n = h.normal.normalize_or_zero();
            if up.dot(n) < original::MIN_UP_DOT_N {
                return None;
            }
            let rear = wheel_right.cross(n);
            if rear.length_squared() < 0.02 {
                return None;
            }
            let rear = rear.normalize();
            Some((h, n, rear))
        });
        let probe = ground.map(|(h, _, _)| above + w.radius - h.distance);
        let sus = original::suspension(ows.x, probe, ow, k, dt);
        ows.x = sus.x;
        let Some((h, n, rear)) = ground else {
            continue;
        };
        let point = start - up * h.distance;
        let v_n = forces.velocity_at_point(point).dot(n);
        let cos_theta = up.dot(n);
        let bump = original::bump_stop_force(
            effective_mass(cfg.mass, inv_inertia, point - com, n),
            v_n,
            sus.overshoot,
            dt,
        );
        let depth = sus.overshoot * cos_theta.max(0.0);
        if depth > 0.0 {
            overshoots.push((point, n, depth));
        }
        let surface = world.surfaces.get(h.entity).ok();
        let grip = (surface.map_or(1.0, |s| s.grip) * world.conditions.traction).max(0.0);
        let mut friction = orig.surface_friction * grip;
        if n.y.abs() < original::STEEP_NORMAL_Y {
            friction = 0.0;
        }
        contacts[i] = Contact {
            grounded: true,
            point,
            normal: n,
            lateral: n.cross(rear),
            forward: -rear,
            entity: Some(h.entity),
            load: sus.force + bump,
            implicit: if sus.overshoot > 0.0 {
                if v_n < 0.0 { -bump / v_n } else { 0.0 }
            } else if sus.force > 0.0 && probe.is_some_and(|x| x >= -extent) {
                (1.0 + k.k2 * sus.x) * k.cs + dt * k.ks / cos_theta
            } else {
                0.0
            },
            implicit_force: if sus.x < limit {
                k.ks * sus.xdot * dt
            } else {
                0.0
            },
            mu_static: ow.static_fric * friction,
            mu_sliding: ow.sliding_fric * friction,
            drag: surface.map_or(0.0, |s| s.drag).clamp(0.0, 1e4),
            grip,
        };
    }
    let grounded_count = contacts
        .iter()
        .take(n_wheels)
        .filter(|c| c.grounded)
        .count();
    let all_down = n_wheels > 0 && grounded_count == n_wheels;
    state.grounded = grounded_count > 0;

    // --- drivetrains -----------------------------------------------------------
    let coupling = pt.clutch.then_some(Coupling {
        ratio,
        torque: pt.torque,
        inertia: engine.inertia,
        omega: pt.engine_omega,
        omega_max: engine.omega_max,
    });
    let free_inertia = cfg.mass * original::FREE_INERTIA_PER_KG;
    let driven: Vec<usize> = orig
        .driven
        .iter()
        .copied()
        .filter(|&i| i < n_wheels)
        .collect();
    let reactions: Vec<f32> = driven.iter().map(|&i| os.wheels[i].reaction).collect();
    let out = original::step_train(&TrainInput {
        omega: os.drive_omega,
        bias: os.drive_bias,
        brake_torque: driven.iter().map(|&i| brake_torque[i]).sum(),
        reactions: &reactions,
        coupling,
        free_inertia,
        train: &orig.drivetrain,
        dt,
    });
    os.drive_omega = out.omega;
    os.drive_bias = out.bias;
    if let Some(e) = out.engine_omega {
        pt.engine_omega = e;
    }
    for (j, &i) in driven.iter().enumerate() {
        os.wheels[i].omega = if driven.len() < 2 {
            out.omega
        } else if j % 2 == 0 {
            out.bias * out.omega
        } else {
            out.omega / out.bias
        };
    }
    for &i in orig.free.iter().filter(|&&i| i < n_wheels) {
        let ows = &mut os.wheels[i];
        ows.omega = original::step_train(&TrainInput {
            omega: ows.omega,
            bias: 1.0,
            brake_torque: brake_torque[i],
            reactions: &[ows.reaction],
            coupling: None,
            free_inertia,
            train: &orig.freetrain,
            dt,
        })
        .omega;
    }

    // --- tyres --------------------------------------------------------------------
    let mut wheel_states = std::mem::take(&mut state.wheels);
    wheel_states.resize(cfg.wheels.len(), WheelState::default());
    for i in 0..n_wheels {
        let w = &cfg.wheels[i];
        let ow = &orig.wheels[i];
        let ows = &mut os.wheels[i];
        let c = contacts[i];
        let ws = &mut wheel_states[i];
        let spin = ws.spin + ows.omega * dt;
        let compression =
            (ows.x + ow.suspension_extent).clamp(0.0, ow.suspension_extent + ow.suspension_limit);
        *ws = WheelState {
            contact_normal: up,
            spin,
            surface_grip: 1.0,
            compression,
            ..Default::default()
        };
        if !c.grounded {
            // A wheel off the ground forgets its bristles.
            ows.bristles = Bristles::default();
            ows.reaction = 0.0;
            continue;
        }
        let v = forces.velocity_at_point(c.point);
        let v_lat = v.dot(c.lateral);
        let v_fwd = v.dot(c.forward);
        let rolling = ows.omega * w.radius;
        let tyre = original::tyre(
            &mut ows.bristles,
            ow,
            &constants[i],
            &TyreInput {
                v_lat,
                v_fwd,
                v_slip: v_fwd - rolling,
                load: c.load,
                mu_static: c.mu_static,
                mu_sliding: c.mu_sliding,
                rolling_speed: rolling.abs(),
                dt,
            },
        );
        ows.reaction = tyre.f_long * w.radius;
        // Wading drag, on surfaces that author one (water only on retail).
        let drag_lat = -v_lat.abs() * ow.tire_drag_coef_lat * c.load * v_lat * c.drag;
        let drag_long = -v_fwd.abs() * ow.tire_drag_coef_long * c.load * v_fwd * c.drag;
        let lateral = tyre.f_lat + drag_lat;
        let longitudinal = tyre.f_long + drag_long;
        // Everything acts at the contact patch, the load along the ground
        // normal.
        // addImplicit includes its force predictor only when the
        // suspension contributes a nonzero Jacobian (D > 0).
        let predictor = if c.implicit > 0.0 {
            c.implicit_force
        } else {
            0.0
        };
        let patch_force =
            c.normal * (c.load + predictor) + c.lateral * lateral + c.forward * longitudinal;
        let arm = c.point - com;
        body_force += patch_force;
        body_torque += arm.cross(patch_force);
        if c.implicit > 0.0 {
            let j = Mat3::from_cols(
                c.normal * c.normal.x,
                c.normal * c.normal.y,
                c.normal * c.normal.z,
            ) * c.implicit;
            let cross = Mat3::from_cols(arm.cross(Vec3::X), arm.cross(Vec3::Y), arm.cross(Vec3::Z));
            implicit_a += j;
            implicit_b += cross * j;
            implicit_c += cross * j * cross.transpose();
        }

        let opt = ow.optimum_slip.max(1e-4);
        let s_long = tyre.s_long.abs();
        *ws = WheelState {
            grounded: true,
            contact_point: c.point,
            contact_normal: c.normal,
            contact_entity: c.entity,
            compression,
            suspension_force: c.load,
            vel_long: v_fwd,
            vel_lat: v_lat,
            slip_angle: tyre.s_lat.atan(),
            traction_demand: original::sgn(tyre.s_long)
                * if s_long <= opt {
                    s_long / opt
                } else {
                    1.0 + s_long - opt
                },
            lateral_force: lateral,
            cornering: if c.mu_static * c.load > 0.0 {
                (tyre.f_lat / (c.mu_static * c.load)).clamp(-1.0, 1.0)
            } else {
                0.0
            },
            longitudinal_force: longitudinal,
            surface_grip: c.grip,
            surface_drag: c.drag,
            spin,
            slip_visual: tyre.slip_visual,
        };
    }
    state.wheels = wheel_states;

    // --- gyro (`vehGyro::Update`) ---------------------------------------------
    if let Some(gyro) = &cfg.gyro {
        let drive = os.drive_omega;
        if all_down {
            // Drift turns the car into the steer with the driven wheels'
            // spin — so with speed, and in a burnout too.
            let mut yaw = -inertia.y * gyro.drift * steering * steering.abs() * drive;
            if handbrake > 0.01 && (gyro.spin180 > 0.0 || gyro.reverse180 > 0.0) {
                let k = if drive <= 0.0 {
                    gyro.reverse180
                } else {
                    gyro.spin180
                };
                yaw -= inertia.y * k * steering * drive;
            }
            body_torque += up * yaw;
        } else if brake > 0.01 {
            // Airborne levelling while braking — authored 0 on retail.
            let pitch = gyro.pitch.unwrap_or(0.0).max(0.0);
            let roll = gyro.roll.unwrap_or(0.0).max(0.0);
            if pitch > 0.0 || roll > 0.0 {
                let w = brake * brake;
                body_torque += right * (inertia.x * pitch * w * back.y)
                    - back * (inertia.z * roll * w * right.y);
            }
        }
    }

    // --- air levelling (designed assist) ---------------------------------------
    // Not the original's: a car cresting a rise at speed leaves it
    // nose-down and holds that attitude to the ground, so the hull
    // arrives first and digs in. See `AssistConfig::air_control`. It
    // levels to the ground the car is coming down on rather than the
    // horizon: off an SF crest the street falls away at 25°, and a car
    // held level lands on its tail and slams its nose into the slope.
    if grounded_count == 0 && cfg.assists.air_control > 0.0 {
        let w = cfg.assists.air_control;
        let ahead = Vec3::new(linvel.x, 0.0, linvel.z) * AIR_LOOKAHEAD;
        let target = world
            .spatial
            .cast_ray(com + ahead, Dir3::NEG_Y, AIR_GROUND_REACH, true, &filter)
            .map(|h| h.normal.normalize_or_zero())
            .filter(|n| n.y >= AIR_GROUND_NORMAL_Y)
            .unwrap_or(Vec3::Y);
        let tilt = up.cross(target);
        let level_rate = angvel - target * angvel.dot(target);
        let i = (inertia.x + inertia.z) * 0.5;
        body_torque += i * (tilt * w * w - level_rate * 2.0 * w);
    }

    // Retail push corrections change position only, merging overlapping
    // contact corrections rather than introducing temporary velocities.
    let mut push = Vec3::ZERO;
    for (_, normal, depth) in overshoots {
        let missing = depth - push.dot(normal);
        if missing > 0.0 {
            push += normal * missing;
        }
    }
    os.push = push;
    os.pending_force = body_force;
    os.pending_torque = body_torque;
    os.implicit_a = implicit_a;
    os.implicit_b = implicit_b;
    os.implicit_c = implicit_c;

    // --- reporting ---------------------------------------------------------------
    let pt = &os.powertrain;
    state.gear = pt.gear.saturating_sub(FIRST_GEAR);
    state.rpm = pt.rpm;
    state.shifting = pt.shift_lag_left();
    let full = engine.full_throttle(pt.engine_omega);
    state.engine_load = if full > 1.0 && pt.clutch && pt.torque > 0.0 {
        (pt.torque / full).clamp(0.0, 1.0)
    } else {
        0.0
    };
}

/// This marker scopes Avian's custom velocity integration to retail cars.
/// Body contacts and position integration remain in Avian's six substeps.
#[derive(Component)]
#[require(CustomVelocityIntegration)]
pub(crate) struct OriginalBodyIntegration;

pub(crate) fn prepare_integration(
    mut commands: Commands,
    vehicles: Query<(Entity, &Vehicle, Has<OriginalBodyIntegration>)>,
) {
    for (entity, vehicle, marked) in &vehicles {
        if vehicle.config.original.is_some() && !marked {
            commands.entity(entity).insert(OriginalBodyIntegration);
        } else if vehicle.config.original.is_none() && marked {
            commands
                .entity(entity)
                .remove::<(OriginalBodyIntegration, CustomVelocityIntegration)>();
        }
    }
}

/// Integrate the previous wheel update before Avian advances the pose.
/// The suspension Jacobians couple translation, pitch and roll; adding an
/// explicit damper to each spring cannot reproduce this update.
pub(crate) fn integrate(
    time: Res<Time>,
    gravity: Res<Gravity>,
    mut vehicles: Query<
        (&Vehicle, &mut VehicleState, Forces),
        Without<crate::vehicle::RemoteReplica>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (vehicle, mut state, mut forces) in &mut vehicles {
        let cfg = &vehicle.config;
        let (Some(orig), Some(os)) = (&cfg.original, state.original.as_mut()) else {
            continue;
        };
        let mass = cfg.mass.max(1e-3);
        let rotation = Mat3::from_quat(forces.rotation().0);
        let inertia = rotation
            * Mat3::from_diagonal(Vec3::from(cfg.inertia.unwrap_or([mass; 3])))
            * rotation.transpose();
        let linear_accel = forces.accumulated_linear_acceleration();
        let angular_accel = forces.accumulated_angular_acceleration();
        let dp = dt * (os.pending_force + mass * (Vec3::NEG_Y * orig.gravity + linear_accel));
        let torque = os.pending_torque + inertia * angular_accel;
        let t1 = (Mat3::IDENTITY + os.implicit_a * (dt / mass)).inverse();
        let m2 = inertia + dt * os.implicit_c
            - (dt * dt / mass) * os.implicit_b * t1 * os.implicit_b.transpose();
        let angular = forces.angular_velocity();
        let mut dw = m2.inverse() * (dt * torque - (dt / mass) * os.implicit_b * (t1 * dp));
        if os.implicit_a == Mat3::ZERO {
            // Retail's airborne mode keeps world angular momentum instead
            // of angular velocity fixed as the anisotropic body rotates.
            let momentum = os.angular_momentum.unwrap_or(inertia * angular);
            let local_next = rotation.transpose() * (inertia.inverse() * (momentum + dt * torque));
            let bound = Vec3::splat(orig.max_angular_speed);
            dw = rotation * local_next.clamp(-bound, bound) - angular;
        }
        // Match retail's back-substitution, including its documented + sign.
        let dv = t1 * (dp + dt * os.implicit_b.transpose() * dw) / mass;
        *forces.linear_velocity_mut() += dv;
        *forces.angular_velocity_mut() += dw;
        os.integrated_angular_velocity = angular + dw;
        os.angular_momentum = Some(inertia * os.integrated_angular_velocity);
        forces.reset_accumulated_linear_acceleration();
        forces.reset_accumulated_angular_acceleration();
        // The custom marker bypasses Avian's velocity/gyro integration.
        // Cancel gravity also in the acceleration accumulator, leaving
        // collision impulses as the only within-frame velocity changes.
        forces.apply_linear_acceleration(-gravity.0);
    }
}

type OriginalVehicles<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Vehicle,
        &'static mut VehicleState,
        &'static VehicleInput,
        Option<&'static HumanDriver>,
        Option<&'static crate::vehicle::EngineImpairment>,
        Forces,
    ),
    Without<crate::vehicle::RemoteReplica>,
>;

/// Evaluate the next update using the pose Avian just advanced. Forces are
/// cached instead of integrated until the following tick, as in retail.
pub(crate) fn evaluate(
    time: Res<Time>,
    spatial: SpatialQuery,
    surfaces: Query<&'static TireSurface>,
    conditions: Res<TireConditions>,
    mut vehicles: OriginalVehicles,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let world = StepWorld {
        spatial: &spatial,
        surfaces: &surfaces,
        conditions: &conditions,
        dt,
    };
    for (entity, vehicle, mut state, input, human, impairment, mut forces) in &mut vehicles {
        let Some(orig) = &vehicle.config.original else {
            continue;
        };
        let scale = impairment.map_or(1.0, |i| i.0);
        let scale = if scale.is_finite() {
            scale.clamp(0.0, 1.0)
        } else {
            1.0
        };
        step(
            entity,
            &vehicle.config,
            orig,
            &mut state,
            input,
            human.copied(),
            scale,
            &mut forces,
            &world,
        );
    }
}

/// Bump stops use a positional projection, preserving actual velocities.
/// Avian synchronizes this corrected physics pose to Transform afterwards.
pub(crate) fn push_out(
    mut vehicles: Query<(&mut VehicleState, &mut Position), Without<crate::vehicle::RemoteReplica>>,
) {
    for (mut state, mut position) in &mut vehicles {
        if let Some(os) = state.original.as_mut() {
            position.0 += std::mem::take(&mut os.push);
        }
    }
}
