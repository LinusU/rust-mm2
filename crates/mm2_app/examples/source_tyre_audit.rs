//! Audit tire/train kernels and the f32 coupled solve at captured ORIGINAL states.
//! Usage: source_tyre_audit <original.json[.gz]> <native.provenance.json[.gz]> [output.json] [source-derived.json]
//! This is a prescribed-state arithmetic diagnostic, not a native trajectory replay.

use bevy::math::{Mat3, Quat, Vec3};
use mm2_vehicle::{
    config::VehicleConfig,
    original::{self, Bristles, TyreInput, WheelConstants},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{error::Error, fs, io, path::Path, process::Command};

#[derive(Deserialize)]
struct Capture {
    car: String,
    city: String,
    complete: bool,
    hz: u32,
    integration_frames: usize,
    car_identity_verified: bool,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Provenance {
    car: String,
    city: String,
    hz: u32,
    vehicle_config: VehicleConfig,
}

#[derive(Deserialize)]
struct Row {
    n: usize,
    dt: f32,
    ground: [u8; 4],
    #[serde(rename = "surfaceFriction")]
    surface_friction: [f32; 4],
    diagnostics: Diagnostics,
    rpm: f32,
    gear: usize,
    spin: f32,
    throttle: f32,
    brake: f32,
    hb: f32,
    pos: [f32; 3],
    vel: [f32; 3],
    omega: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
}

#[derive(Deserialize)]
struct Diagnostics {
    wheels: [Wheel; 4],
    ics: Value,
}

#[derive(Deserialize)]
struct Wheel {
    #[serde(rename = "vLat")]
    v_lat: f32,
    #[serde(rename = "vFwd")]
    v_fwd: f32,
    #[serde(rename = "vSlip")]
    v_slip: f32,
    #[serde(rename = "N")]
    load: f32,
    #[serde(rename = "Fs")]
    suspension_force: f32,
    #[serde(rename = "dLat")]
    d_lat: f32,
    #[serde(rename = "dLong")]
    d_long: f32,
    #[serde(rename = "FLat")]
    f_lat: f32,
    #[serde(rename = "FLong")]
    f_long: f32,
    #[serde(rename = "slipVisual")]
    slip_visual: f32,
    #[serde(rename = "bottomedOut")]
    bottomed_out: u8,
    radius: f32,
    #[serde(rename = "D")]
    implicit: f32,
    #[serde(rename = "contactPoint")]
    contact_point: [f32; 3],
    #[serde(rename = "probeNormal")]
    normal: [f32; 3],
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    let bytes = if path.extension().is_some_and(|extension| extension == "gz") {
        let result = Command::new("gzip").arg("-dc").arg(path).output()?;
        if !result.status.success() {
            return Err(invalid(format!("gzip failed for {}", path.display())).into());
        }
        result.stdout
    } else {
        fs::read(path)?
    };
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate(source: &Capture, provenance: &Provenance) -> Result<(), Box<dyn Error>> {
    if source.car != provenance.car || source.city != provenance.city {
        return Err(invalid("source/provenance car or city mismatch").into());
    }
    if !source.complete || !source.car_identity_verified || source.hz != 60 || provenance.hz != 60 {
        return Err(invalid("requires a complete, identity-verified 60 Hz capture").into());
    }
    provenance
        .vehicle_config
        .validate()
        .map_err(|errors| invalid(errors.join("; ")))?;
    let cfg = &provenance.vehicle_config;
    if cfg.inertia.is_none() {
        return Err(invalid("requires authored retail diagonal inertia").into());
    }
    if cfg.wheels.len() != 4 || cfg.original.as_ref().is_none_or(|o| o.wheels.len() != 4) {
        return Err(invalid("requires four authored retail wheels").into());
    }
    if source.rows.len() < 2 || source.rows.len() != source.integration_frames + 1 {
        return Err(invalid("capture is missing integration rows").into());
    }
    let dt = source.rows[0].dt;
    if !dt.is_finite() || (dt - 1.0 / 60.0).abs() > 2e-9 {
        return Err(invalid("capture clock does not match 60 Hz").into());
    }
    for (index, row) in source.rows.iter().enumerate() {
        if row.n != index || row.dt != dt || row.ground != [1; 4] {
            return Err(invalid(format!(
                "row {index}: nonsequential, variable dt or airborne"
            ))
            .into());
        }
        for key in [
            "impulse",
            "angularImpulse",
            "netPush",
            "netTurn",
            "totalPush",
            "lastTotalPush",
        ] {
            let values = row.diagnostics.ics[key]
                .as_array()
                .filter(|values| values.len() == 3)
                .ok_or_else(|| invalid(format!("row {index}: missing ICS {key}")))?;
            if values.iter().any(|value| value.as_f64() != Some(0.0)) {
                return Err(
                    invalid(format!("row {index}: collision/push correction {key}")).into(),
                );
            }
        }
        if [row.rpm, row.spin, row.throttle, row.brake, row.hb]
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(invalid(format!("row {index}: nonfinite powertrain input")).into());
        }
        for values in [row.pos, row.vel, row.omega, row.right, row.up, row.forward] {
            if values.iter().any(|v| !v.is_finite()) {
                return Err(invalid(format!("row {index}: nonfinite body vector")).into());
            }
        }
        for key in ["force", "force2", "torque", "torque2"] {
            if row.diagnostics.ics[key].as_array().is_none_or(|values| {
                values.len() != 3
                    || values
                        .iter()
                        .any(|v| v.as_f64().is_none_or(|v| !v.is_finite()))
            }) {
                return Err(invalid(format!("row {index}: invalid {key}")).into());
            }
        }
        for (i, wheel) in row.diagnostics.wheels.iter().enumerate() {
            if !wheel.radius.is_finite() || wheel.radius <= 0.0 {
                return Err(invalid(format!("row {index}: invalid wheel radius")).into());
            }
            if !wheel.implicit.is_finite()
                || wheel.implicit < 0.0
                || wheel
                    .normal
                    .iter()
                    .chain(&wheel.contact_point)
                    .any(|v| !v.is_finite())
            {
                return Err(
                    invalid(format!("row {index}: invalid wheel Jacobian geometry")).into(),
                );
            }
            let values = [
                wheel.v_lat,
                wheel.v_fwd,
                wheel.v_slip,
                wheel.load,
                wheel.suspension_force,
                wheel.d_lat,
                wheel.d_long,
                wheel.f_lat,
                wheel.f_long,
                wheel.slip_visual,
                row.surface_friction[i],
            ];
            if values.iter().any(|value| !value.is_finite())
                || wheel.load < 0.0
                || row.surface_friction[i] < 0.0
                || wheel.bottomed_out != 0
                || wheel.load != wheel.suspension_force
            {
                return Err(invalid(format!(
                    "row {index}, wheel {i}: invalid or bump-stop state"
                ))
                .into());
            }
        }
    }
    Ok(())
}

fn statistics(values: &[f64]) -> Value {
    json!({
        "rms": (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt(),
        "max_absolute": values.iter().map(|v| v.abs()).fold(0.0, f64::max),
    })
}

fn audit_primary_train(source: &Capture, cfg: &VehicleConfig, derived: Option<&Value>) -> Value {
    let tuning = cfg.original.as_ref().expect("validated original tuning");
    let engine = original::EngineConstants::of(&tuning.engine);
    let mut frames = Vec::new();
    let mut spin_errors = Vec::new();
    let mut bias_errors = Vec::new();
    let mut skipped = std::collections::BTreeMap::<&str, usize>::new();
    for window in source.rows.windows(3) {
        let earlier = &window[0];
        let previous = &window[1];
        let row = &window[2];
        let ratio = derived
            .and_then(|d| d["constants"]["transmission"]["autoRatios"][row.gear].as_f64())
            .map(|v| v as f32)
            .unwrap_or_else(|| tuning.gearbox.ratios.get(row.gear).copied().unwrap_or(0.0));
        let omega = -previous.spin;
        let engine_omega = omega * ratio;
        let phase_rpm = engine_omega * 60.0 / std::f32::consts::TAU;
        let earlier_phase_rpm = (-earlier.spin * ratio) * 60.0 / std::f32::consts::TAU;
        let reason = if tuning.driven.len() != 2 {
            Some("requires a two-wheel primary train")
        } else if row.gear < 2
            || row.gear != previous.gear
            || previous.gear != earlier.gear
            || ratio <= 0.0
        {
            Some("neutral/reverse/gear transition")
        } else if engine_omega <= 2.0 * engine.omega_idle || engine_omega >= engine.omega_max {
            Some("clutch attachment or limiter uncertain")
        } else if (row.rpm - phase_rpm).abs() > 0.01
            || (previous.rpm - earlier_phase_rpm).abs() > 0.01
        {
            Some("displayed RPM does not verify unblended attached phase")
        } else if row.brake != 0.0 || row.hb != 0.0 || row.throttle < 0.0 || row.throttle > 1.0 {
            Some("braking/handbrake or invalid throttle outside this audit")
        } else {
            None
        };
        if let Some(reason) = reason {
            *skipped.entry(reason).or_default() += 1;
            continue;
        }
        let left = tuning.driven[0];
        let left_previous = &previous.diagnostics.wheels[left];
        let left_current = &row.diagnostics.wheels[left];
        // Recorded spin is AGE-signed; native train omega is its negative.
        // Wheel omega/bias are reconstructed from rounded vFwd/vSlip and radius.
        let previous_bias =
            ((left_previous.v_fwd - left_previous.v_slip) / left_previous.radius) / omega;
        let current_bias =
            ((left_current.v_fwd - left_current.v_slip) / left_current.radius) / (-row.spin);
        if !previous_bias.is_finite() || !current_bias.is_finite() {
            *skipped
                .entry("nonfinite reconstructed differential bias")
                .or_default() += 1;
            continue;
        }
        let full_throttle = if let Some(d) = derived {
            // Source-derived curve constants are private inside EngineConstants;
            // explicitly evaluate the recovered curve for this second diagnostic.
            let e = &d["constants"]["engine"];
            let get = |key: &str| e[key].as_f64().expect("validated source constant") as f32;
            let opt = get("omegaOptimal");
            let max = get("omegaMax");
            let shape = (get("a") * opt - engine_omega) * (get("b") * opt + engine_omega);
            if engine_omega <= opt {
                get("p") * shape
            } else if engine_omega <= max {
                get("p")
                    * (max - engine_omega)
                    * shape
                    * (engine_omega + max - 2.0 * opt)
                    * get("invOmegaRangeSquared")
            } else {
                0.0
            }
        } else {
            engine.full_throttle(engine_omega)
        };
        let torque = row.throttle * full_throttle
            + (1.0 - row.throttle) * engine.zero_throttle(engine_omega);
        let reactions: Vec<_> = tuning
            .driven
            .iter()
            .map(|&i| {
                let wheel = &previous.diagnostics.wheels[i];
                wheel.f_long * wheel.radius
            })
            .collect();
        let output = original::step_train(&original::TrainInput {
            omega,
            bias: previous_bias,
            brake_torque: 0.0,
            reactions: &reactions,
            coupling: Some(original::Coupling {
                ratio,
                torque,
                inertia: engine.inertia,
                omega: engine_omega,
                omega_max: engine.omega_max,
            }),
            free_inertia: cfg.mass * original::FREE_INERTIA_PER_KG,
            train: &tuning.drivetrain,
            dt: row.dt,
        });
        let spin_error = f64::from(output.omega) + f64::from(row.spin);
        let bias_error = f64::from(output.bias) - f64::from(current_bias);
        spin_errors.push(spin_error);
        bias_errors.push(bias_error);
        frames.push(json!({
            "n": row.n, "gear": row.gear, "shaft_omega_error_rad_s": spin_error,
            "differential_bias_error_reconstructed": bias_error,
            "input": {"omega": omega, "engine_omega_reconstructed": engine_omega,
                "ratio": ratio, "torque": torque, "throttle": row.throttle,
                "reactions_reconstructed": reactions, "bias_reconstructed": previous_bias},
            "source": {"shaft_omega": -row.spin, "rpm": row.rpm,
                "phase_rpm_reconstructed": phase_rpm, "bias_reconstructed": current_bias},
            "rust": {"shaft_omega": output.omega, "bias": output.bias},
        }));
    }
    json!({
        "scope": "Actual Rust step_train and engine torque curve at prescribed SOURCE states. Forward unchanged-gear samples with two consecutive unblended-RPM phase checks, engine above twice idle, below limiter, no brake/HB; power_scale assumed1. Previous engine omega=-source spin*authored ratio. Differential bias and reactions reconstructed from rounded source wheel fields, not raw train caches; this uncertainty precludes bit-exact conclusions.",
        "constants_mode": if derived.is_some() { "captured source ratios and full-throttle curve; native engine inertia/idle/coast curve and train inertia" } else { "native authored ratio and actual EngineConstants torque methods" },
        "eligible_updates": frames.len(), "skipped_by_reason": skipped,
        "shaft_omega_error_rad_s": if spin_errors.is_empty() { Value::Null } else { statistics(&spin_errors) },
        "differential_bias_error_reconstructed": if bias_errors.is_empty() { Value::Null } else { statistics(&bias_errors) },
        "near_first_native_divergence": frames.iter().filter(|r| {
            r["n"].as_u64().is_some_and(|n| n == 74 || n == 75)
        }).collect::<Vec<_>>(),
        "frames": frames,
    })
}

fn vector_statistics(errors: &[[f64; 3]]) -> Value {
    json!({"per_axis": (0..3).map(|i| {
        statistics(&errors.iter().map(|e| e[i]).collect::<Vec<_>>())
    }).collect::<Vec<_>>()})
}

fn float_ulp_distance(a: f32, b: f32) -> u64 {
    let ordered = |v: f32| {
        let bits = v.to_bits();
        if bits & 0x8000_0000 != 0 {
            -i64::from(bits & 0x7fff_ffff)
        } else {
            i64::from(bits)
        }
    };
    ordered(a).abs_diff(ordered(b))
}

fn audit_coupled_integration(
    source: &Capture,
    cfg: &VehicleConfig,
    project_quaternion: bool,
) -> Value {
    let tuning = cfg.original.as_ref().expect("validated original tuning");
    let mass = cfg.mass;
    let diagonal_inertia = Vec3::from(cfg.inertia.expect("authored original inertia"));
    let mut frames = Vec::new();
    let mut velocity_errors = Vec::new();
    let mut angular_errors = Vec::new();
    let mut exact_velocity = 0;
    let mut exact_angular = 0;
    let mut max_velocity_ulp = 0;
    let mut max_angular_ulp = 0;
    for pair in source.rows.windows(2) {
        let row = &pair[0];
        let next = &pair[1];
        let mut a = Mat3::ZERO;
        let mut b = Mat3::ZERO;
        let mut c = Mat3::ZERO;
        for wheel in &row.diagnostics.wheels {
            let normal = Vec3::from(wheel.normal);
            let arm = Vec3::from(wheel.contact_point) - Vec3::from(row.pos);
            // Match the production Jacobian assembly, including f32 operation order.
            let j = Mat3::from_cols(normal * normal.x, normal * normal.y, normal * normal.z)
                * wheel.implicit;
            let cross = Mat3::from_cols(arm.cross(Vec3::X), arm.cross(Vec3::Y), arm.cross(Vec3::Z));
            a += j;
            b += cross * j;
            c += cross * j * cross.transpose();
        }
        // Source 'forward' is historical AGE BACK. No sign reversal.
        let raw_rotation = Mat3::from_cols(
            Vec3::from(row.right),
            Vec3::from(row.up),
            Vec3::from(row.forward),
        );
        let rotation = if project_quaternion {
            Mat3::from_quat(Quat::from_mat3(&raw_rotation).normalize())
        } else {
            raw_rotation
        };
        let inertia = rotation * Mat3::from_diagonal(diagonal_inertia) * rotation.transpose();
        let read = |key: &str| {
            Vec3::from_array(std::array::from_fn(|i| {
                row.diagnostics.ics[key][i]
                    .as_f64()
                    .expect("validated force vector") as f32
            }))
        };
        let force = read("force") + read("force2");
        let torque = read("torque") + read("torque2");
        let dt = row.dt;
        let dp = dt * (force + mass * (Vec3::NEG_Y * tuning.gravity));
        let t1 = (Mat3::IDENTITY + a * (dt / mass)).inverse();
        let m2 = inertia + dt * c - (dt * dt / mass) * b * t1 * b.transpose();
        let dw = m2.inverse() * (dt * torque - (dt / mass) * b * (t1 * dp));
        let dv = t1 * (dp + dt * b.transpose() * dw) / mass;
        let predicted_v = (Vec3::from(row.vel) + dv).to_array();
        let predicted_w = (Vec3::from(row.omega) + dw).to_array();
        let ev: [f64; 3] =
            std::array::from_fn(|i| f64::from(predicted_v[i]) - f64::from(next.vel[i]));
        let ew: [f64; 3] =
            std::array::from_fn(|i| f64::from(predicted_w[i]) - f64::from(next.omega[i]));
        let velocity_ulps: [u64; 3] =
            std::array::from_fn(|i| float_ulp_distance(predicted_v[i], next.vel[i]));
        let angular_ulps: [u64; 3] =
            std::array::from_fn(|i| float_ulp_distance(predicted_w[i], next.omega[i]));
        exact_velocity += predicted_v
            .iter()
            .zip(next.vel)
            .filter(|(a, b)| a.to_bits() == b.to_bits())
            .count();
        exact_angular += predicted_w
            .iter()
            .zip(next.omega)
            .filter(|(a, b)| a.to_bits() == b.to_bits())
            .count();
        max_velocity_ulp = max_velocity_ulp.max(*velocity_ulps.iter().max().unwrap());
        max_angular_ulp = max_angular_ulp.max(*angular_ulps.iter().max().unwrap());
        velocity_errors.push(ev);
        angular_errors.push(ew);
        frames.push(json!({"n": next.n, "velocity_error_m_s": ev,
            "angular_velocity_error_rad_s": ew, "velocity_ULPs": velocity_ulps,
            "angular_velocity_ULPs": angular_ulps,
            "predicted_velocity": predicted_v, "predicted_angular_velocity": predicted_w}));
    }
    json!({
        "scope": "Rust f32 Mat3 Jacobian/inverse/cofactor expressions matching original_step, evaluated at captured SOURCE poses, forces and velocities. Source normals/arms/D reconstruct matrices; raw cached A/B/C not recorded. No impulses, no airborne branch. Prescribed-state kernel audit, not closed-loop native equivalence; ULP distances near zero can be large despite tiny absolute residuals.",
        "body_basis": if project_quaternion { "source AGE rows projected to normalized Quat for inertia only; Jacobians remain source raw geometry" } else { "raw source AGE rows; source forward field means BACK" },
        "transitions": frames.len(), "components": frames.len() * 3,
        "velocity_error_m_s": vector_statistics(&velocity_errors),
        "angular_velocity_error_rad_s": vector_statistics(&angular_errors),
        "exact_velocity_components": exact_velocity, "exact_angular_components": exact_angular,
        "max_velocity_ULPs": max_velocity_ulp, "max_angular_velocity_ULPs": max_angular_ulp,
        "near_first_native_divergence": frames.iter().filter(|r| {
            r["n"].as_u64().is_some_and(|n| n == 74 || n == 75)
        }).collect::<Vec<_>>(),
        "frames": frames,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if !(3..=5).contains(&args.len()) {
        return Err(invalid("usage: source_tyre_audit <original.json[.gz]> <native.provenance.json[.gz]> [output.json] [source-derived.json]").into());
    }
    let source: Capture = read_json(Path::new(&args[1]))?;
    let provenance: Provenance = read_json(Path::new(&args[2]))?;
    validate(&source, &provenance)?;
    let derived: Option<Value> = args
        .get(4)
        .map(|path| read_json(Path::new(path)))
        .transpose()?;
    if let Some(d) = &derived {
        if d["car"].as_str() != Some(&source.car)
            || d["city"].as_str() != Some(&source.city)
            || d["car_identity_verified"].as_bool() != Some(true)
        {
            return Err(invalid("source-derived constants identity mismatch").into());
        }
        for key in [
            "omegaOptimal",
            "omegaMax",
            "p",
            "a",
            "b",
            "invOmegaRangeSquared",
        ] {
            if d["constants"]["engine"][key]
                .as_f64()
                .is_none_or(|v| !v.is_finite() || v <= 0.0)
            {
                return Err(invalid(format!("invalid source engine constant {key}")).into());
            }
        }
        let ratios = d["constants"]["transmission"]["autoRatios"]
            .as_array()
            .ok_or_else(|| invalid("missing source transmission ratios"))?;
        if ratios.len()
            < provenance
                .vehicle_config
                .original
                .as_ref()
                .unwrap()
                .gearbox
                .ratios
                .len()
            || ratios
                .iter()
                .any(|v| v.as_f64().is_none_or(|v| !v.is_finite()))
        {
            return Err(invalid("invalid source transmission ratios").into());
        }
    }
    let cfg = &provenance.vehicle_config;
    let tuning = cfg.original.as_ref().expect("validated original tuning");
    let constants: Vec<_> = tuning
        .wheels
        .iter()
        .zip(&cfg.wheels)
        .map(|(wheel, geometry)| WheelConstants::of(wheel, geometry.radius, tuning.gravity))
        .collect();
    let mut errors = Vec::new();
    let mut force_errors = Vec::new();
    let mut bristle_errors = Vec::new();
    let mut visual_errors = Vec::new();
    for pair in source.rows.windows(2) {
        let previous = &pair[0];
        let row = &pair[1];
        for (i, ((wheel, old), k)) in row
            .diagnostics
            .wheels
            .iter()
            .zip(&previous.diagnostics.wheels)
            .zip(&constants)
            .enumerate()
        {
            let authored = &tuning.wheels[i];
            let mut bristles = Bristles {
                lat: old.d_lat,
                long: old.d_long,
            };
            let input = TyreInput {
                v_lat: wheel.v_lat,
                v_fwd: wheel.v_fwd,
                v_slip: wheel.v_slip,
                load: wheel.load,
                mu_static: authored.static_fric * row.surface_friction[i],
                mu_sliding: authored.sliding_fric * row.surface_friction[i],
                rolling_speed: (wheel.v_fwd - wheel.v_slip).abs(),
                dt: row.dt,
            };
            let result = original::tyre(&mut bristles, authored, k, &input);
            // Subtract in double so the residual itself does not hide an f32 ULP.
            let force = [
                f64::from(result.f_lat) - f64::from(wheel.f_lat),
                f64::from(result.f_long) - f64::from(wheel.f_long),
            ];
            let displacement = [
                f64::from(bristles.lat) - f64::from(wheel.d_lat),
                f64::from(bristles.long) - f64::from(wheel.d_long),
            ];
            let visual = f64::from(result.slip_visual) - f64::from(wheel.slip_visual);
            force_errors.extend(force);
            bristle_errors.extend(displacement);
            visual_errors.push(visual);
            errors.push(json!({
                "n": row.n, "wheel": i,
                "force_error_N": force, "bristle_error_m": displacement,
                "slip_visual_error": visual,
                "input": {"v_lat": input.v_lat, "v_fwd": input.v_fwd,
                    "v_slip": input.v_slip, "load": input.load,
                    "mu_static": input.mu_static, "mu_sliding": input.mu_sliding,
                    "rolling_speed_reconstructed": input.rolling_speed,
                    "previous_bristles": [old.d_lat, old.d_long]},
                "rust": {"forces": [result.f_lat, result.f_long],
                    "bristles": [bristles.lat, bristles.long], "slip_visual": result.slip_visual},
                "source": {"forces": [wheel.f_lat, wheel.f_long],
                    "bristles": [wheel.d_lat, wheel.d_long], "slip_visual": wheel.slip_visual},
            }));
        }
    }
    let mut worst: Vec<_> = errors.iter().collect();
    worst.sort_by(|a, b| {
        let maximum = |r: &&Value| {
            r["force_error_N"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap().abs())
                .fold(0.0, f64::max)
        };
        maximum(b).total_cmp(&maximum(a))
    });
    let coupled_audit = audit_coupled_integration(&source, cfg, false);
    let coupled_quaternion_audit = audit_coupled_integration(&source, cfg, true);
    let train_audit = audit_primary_train(&source, cfg, None);
    let source_constant_train_audit = derived
        .as_ref()
        .map(|d| audit_primary_train(&source, cfg, Some(d)));
    let summary = json!({
        "scope": "Actual Rust original::tyre f32 law at captured ORIGINAL states, using each previous source bristle independently. Not closed-loop native trajectory equivalence or bit-exact x87 replay. Rolling speed is reconstructed by f32(vFwd-vSlip), not directly recorded; source drag is not replayed.",
        "car": source.car, "city": source.city, "wheel_updates": errors.len(),
        "coupled_integration_raw_source_basis": coupled_audit,
        "coupled_integration_projected_basis": coupled_quaternion_audit,
        "primary_train": train_audit,
        "primary_train_source_derived": source_constant_train_audit,
        "force_error_N": statistics(&force_errors),
        "bristle_error_m": statistics(&bristle_errors),
        "slip_visual_error": statistics(&visual_errors),
        "worst_force_rows": worst.into_iter().take(12).collect::<Vec<_>>(),
        "near_first_native_divergence": errors.iter().filter(|r| {
            r["n"].as_u64().is_some_and(|n| n == 74 || n == 75)
        }).collect::<Vec<_>>(),
    });
    if let Some(path) = args.get(3) {
        fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"summary": summary, "frames": errors}))?,
        )?;
    }
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
