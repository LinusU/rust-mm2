//! Conversion from decoded MM2 tuning (`vehCarSim`, `vehTrailer`, `asNode`,
//! wheel `.mtx`, `.bnd`, `.info`) into the engine-independent
//! [`mm2_vehicle::VehicleConfig`].
//!
//! This module owns the **only** MM2→runtime handling mapping. Every value
//! it emits is tagged in a [`ConversionReport`] as imported, converted,
//! derived, adapted or defaulted — see `docs/vehicle-handling.md` for the
//! full mapping table.
//!
//! ## Conventions
//!
//! * Vehicle space uses **identity** mapping from MM2 coordinates: MM2's
//!   `-Z` forward matches Bevy's `-Z` forward, so no axis is mirrored for
//!   vehicles (unlike the city importer, which mirrors Z for map layout).
//!   `CenterOfGravity` is therefore mirrored on Z after being evaluated in
//!   MM2 space.
//! * `CenterOfGravity` is an offset from the bound-box centre — verified
//!   against every stock car (produces plausible COM heights).
//! * `Trans.Low`/`High`/`Reverse` are per-band top speeds in mph; combined
//!   gear ratios are derived so the engine sits at `OptRPM` at each band's
//!   top speed, geometrically interpolated (biased by `GearBias`).
//! * `MaxHorsePower` is power (745.7 W/hp) peaking at `OptRPM`; a torque
//!   peak of `1.15 × P/ω_opt` at `0.72 × OptRPM` gives the curve its shape.

use mm2_formats::bnd::BndFile;
use mm2_formats::veh::{AsNode, DrivetrainType, VehCarSim, VehTrailer, VehWheel};
use mm2_vehicle::config::{
    AeroConfig, AssistConfig, BrakeConfig, EngineConfig, SteeringConfig, SuspensionConfig,
    TireConfig, TransmissionConfig, VehicleConfig, WheelConfig,
};

const MPH_TO_MPS: f32 = 0.44704;
const HP_TO_W: f32 = 745.7;
const G: f32 = 9.81;
/// Standard air density for aero forces.
const AIR_DENSITY: f32 = 1.225;
/// Rest compression target as a fraction of suspension travel.
const SAG_FRACTION: f32 = 0.45;
/// Lateral/longitudinal grip scale from MM2 static friction (adapted).
const GRIP_LAT_SCALE: f32 = 0.55;
const GRIP_LONG_SCALE: f32 = 0.60;
/// Torque-peak placement and height relative to the power peak (adapted).
const TORQUE_PEAK_RPM_FRAC: f32 = 0.72;
const TORQUE_PEAK_FACTOR: f32 = 1.15;
/// Driveline efficiency applied to every imported car (adapted constant).
const DRIVELINE_EFFICIENCY: f32 = 0.85;
/// Suspension damping ratio band imported cars are mapped into: below
/// ~0.3 a car pogos off road seams, above ~1.0 the springs stop moving and
/// the chassis takes every impact instead.
const DAMPING_RATIO_MIN: f32 = 0.40;
const DAMPING_RATIO_MAX: f32 = 0.90;
/// Share of the lateral-force roll moment cancelled on imported cars
/// (adapted arcade policy — see `AssistConfig::roll_resistance`).
const ROLL_RESISTANCE: f32 = 0.85;

/// How each emitted value was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Copied directly (possibly unit-converted).
    Imported,
    /// Computed from authored values (formula documented).
    Derived,
    /// Mapped through an intentional arcade adaptation.
    Adapted,
    /// No authored value; an engine default applies.
    Defaulted,
    /// Authored value exists but is intentionally not mapped.
    Unsupported,
}

/// One row of the conversion audit trail.
#[derive(Debug, Clone)]
pub struct ReportEntry {
    /// Source field, e.g. `vehCarSim.Engine.MaxHorsePower`.
    pub source: String,
    /// Runtime destination, e.g. `engine.max_power_w`.
    pub dest: String,
    /// How the value was obtained.
    pub provenance: Provenance,
    /// Short explanation (formula or policy).
    pub note: String,
}

/// The full conversion audit for one vehicle.
#[derive(Debug, Default)]
pub struct ConversionReport {
    pub entries: Vec<ReportEntry>,
    pub warnings: Vec<String>,
}

impl ConversionReport {
    fn add(&mut self, source: &str, dest: &str, provenance: Provenance, note: impl Into<String>) {
        self.entries.push(ReportEntry {
            source: source.into(),
            dest: dest.into(),
            provenance,
            note: note.into(),
        });
    }

    /// `source` → `dest`, imported.
    pub fn imported(&mut self, source: &str, dest: &str, note: impl Into<String>) {
        self.add(source, dest, Provenance::Imported, note);
    }
    /// `source` → `dest`, derived.
    pub fn derived(&mut self, source: &str, dest: &str, note: impl Into<String>) {
        self.add(source, dest, Provenance::Derived, note);
    }
    /// `source` → `dest`, adapted.
    pub fn adapted(&mut self, source: &str, dest: &str, note: impl Into<String>) {
        self.add(source, dest, Provenance::Adapted, note);
    }
    /// `dest` defaulted.
    pub fn defaulted(&mut self, dest: &str, note: impl Into<String>) {
        self.add("-", dest, Provenance::Defaulted, note);
    }
    /// `source` intentionally unmapped.
    pub fn unsupported(&mut self, source: &str, note: impl Into<String>) {
        self.add(source, "-", Provenance::Unsupported, note);
    }
}

/// Wheel transform data needed for conversion.
#[derive(Debug, Clone)]
pub struct WheelGeom {
    /// Part index (whl0 → 0).
    pub index: usize,
    /// Wheel centre in car space (mtx origin).
    pub origin: [f32; 3],
    /// Wheel radius (from geometry bounds or mtx bounds).
    pub radius: f32,
}

/// Inputs for one car conversion.
pub struct ConvertInput<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub sim: &'a VehCarSim,
    pub asnode: Option<&'a AsNode>,
    /// Wheel positions/radii, ordered by wheel index.
    pub wheels: &'a [WheelGeom],
    /// Parsed bound when present.
    pub bound: Option<&'a BndFile>,
    /// Body AABB `(min, max)` from the model, for fallback bounds.
    pub body_aabb: ([f32; 3], [f32; 3]),
}

/// Output of one conversion.
pub struct Converted {
    pub config: VehicleConfig,
    pub report: ConversionReport,
}

/// Rebound damping rate (N·s/m) for one corner.
///
/// Damping is set as a fraction of critical, and critical damping depends
/// on the corner's *mass*: `wheel_load` is a weight in newtons, so the
/// gravity has to come back out before the geometric mean.
///
/// MM2 authors `SuspensionDampCoef` between about 0.01 (London Cab) and
/// 0.1 (most cars). Mapped straight through, the low end lands near
/// ζ = 0.1 and the car pogos off every road seam. The authored value still
/// orders the roster; the band keeps every car on it drivable.
fn suspension_damping(spring_rate: f32, wheel_load: f32, damp_coef: f32) -> f32 {
    let corner_mass = (wheel_load / G).max(1e-3);
    let critical = 2.0 * (spring_rate * corner_mass).sqrt();
    let t = (damp_coef / 0.1).clamp(0.0, 1.0);
    let zeta = DAMPING_RATIO_MIN + (DAMPING_RATIO_MAX - DAMPING_RATIO_MIN) * t;
    zeta * critical
}

fn aabb_min_max(
    bound: Option<&BndFile>,
    body: ([f32; 3], [f32; 3]),
) -> ([f32; 3], [f32; 3], &'static str) {
    if let Some(b) = bound
        && let Some((min, max)) = b.aabb()
    {
        return (min, max, "bound");
    }
    (body.0, body.1, "model")
}

/// Convert a full set of decoded inputs into a [`VehicleConfig`].
pub fn convert(input: &ConvertInput<'_>) -> Result<Converted, String> {
    let sim = input.sim;
    let mut report = ConversionReport::default();
    report.warnings.extend(sim.warnings.iter().cloned());

    if input.wheels.is_empty() {
        return Err(format!("{}: no wheel transforms found", input.id));
    }

    let (bmin, bmax, bounds_src) = aabb_min_max(input.bound, input.body_aabb);
    let bcenter = [
        (bmin[0] + bmax[0]) * 0.5,
        (bmin[1] + bmax[1]) * 0.5,
        (bmin[2] + bmax[2]) * 0.5,
    ];
    let size = [
        (bmax[0] - bmin[0]).max(0.05),
        (bmax[1] - bmin[1]).max(0.05),
        (bmax[2] - bmin[2]).max(0.05),
    ];

    // --- mass / inertia / centre of mass ---------------------------------
    let mass = sim.mass.max(50.0);
    report.imported("vehCarSim.Mass", "mass", format!("{mass} kg"));

    // COM = bound centre + CenterOfGravity (verified on stock data). The
    // vehicle space is an identity map of MM2 coordinates: both use -Z
    // forward.
    let cog = sim.center_of_gravity;
    let com = [
        bcenter[0] + cog[0],
        bcenter[1] + cog[1],
        bcenter[2] + cog[2],
    ];
    report.derived(
        "vehCarSim.CenterOfGravity",
        "center_of_mass",
        format!("bound centre {bcenter:?} + authored offset {cog:?}"),
    );

    let ib = sim.inertia_box;
    let inertia = [
        mass * (ib[1] * ib[1] + ib[2] * ib[2]) / 12.0,
        mass * (ib[0] * ib[0] + ib[2] * ib[2]) / 12.0,
        mass * (ib[0] * ib[0] + ib[1] * ib[1]) / 12.0,
    ];
    report.derived(
        "vehCarSim.InertiaBox",
        "inertia",
        format!("box {ib:?} m → principal tensor {inertia:?} kg·m²"),
    );

    // --- wheel rig ---------------------------------------------------------
    // Front axle = wheels on the forward (-z) half of the wheel z-extent.
    let z_min = input
        .wheels
        .iter()
        .map(|w| w.origin[2])
        .fold(f32::MAX, f32::min);
    let z_max = input
        .wheels
        .iter()
        .map(|w| w.origin[2])
        .fold(f32::MIN, f32::max);
    let z_mid = (z_min + z_max) * 0.5;
    let wheelbase = (z_max - z_min).abs().max(0.2);
    let track_width = input
        .wheels
        .iter()
        .map(|w| w.origin[0].abs() * 2.0)
        .fold(0.0f32, f32::max)
        .max(0.4);
    report.derived(
        "geometry/<id>_whlN.mtx origins",
        "wheelbase/track_width",
        format!("wheelbase {wheelbase:.2} m, track {track_width:.2} m"),
    );

    // Static load per axle from the longitudinal weight distribution.
    // Front axle load = m·g·(z_rear − z_com)/(z_rear − z_front) in MM2 z.
    let front_z = z_min;
    let rear_z = z_max;
    let com_z_mm2 = com[2];
    let front_share = ((rear_z - com_z_mm2) / (rear_z - front_z).max(0.1)).clamp(0.05, 0.95);
    let back_share = 1.0 - front_share;

    // Axle torque split for AWD (TorqueCoef on each axle), else even split.
    let (front_drive_share, back_drive_share) = match sim.drivetrain_type {
        DrivetrainType::Fwd => (1.0, 0.0),
        DrivetrainType::Rwd => (0.0, 1.0),
        DrivetrainType::Awd => {
            let f = sim
                .axle_front
                .as_ref()
                .map(|a| a.torque_coef)
                .unwrap_or(0.0);
            let b = sim.axle_back.as_ref().map(|a| a.torque_coef).unwrap_or(0.0);
            if f + b > 0.0 {
                (f / (f + b), b / (f + b))
            } else {
                (0.5, 0.5)
            }
        }
    };
    report.derived(
        "vehCarSim.DrivetrainType+Axle*.TorqueCoef",
        "wheels[].driven/drive_share",
        format!(
            "{:?} → front {:.0}% / rear {:.0}% of drive torque",
            sim.drivetrain_type,
            front_drive_share * 100.0,
            back_drive_share * 100.0
        ),
    );

    // Brake normalisation: axle BrakeCoef values are relative; the per-wheel
    // share of the total foot-brake budget is coef/Σcoef.
    let n_front = input.wheels.iter().filter(|w| w.origin[2] < z_mid).count();
    let n_back = input.wheels.len() - n_front;
    let brake_sum = (n_front as f32 * sim.wheel_front.brake_coef
        + n_back as f32 * sim.wheel_back.brake_coef)
        .max(1e-3);

    let total_brake_force = mass
        * G
        * (0.55 * (sim.wheel_front.static_fric + sim.wheel_back.static_fric) * 0.5 * 1.2).max(0.8);
    report.derived(
        "vehCarSim.Mass+Wheel*.StaticFric",
        "brakes.max_brake_force",
        format!("{total_brake_force:.0} N total ≈ 1.1× tyre grip limit"),
    );

    let mut wheels = Vec::with_capacity(input.wheels.len());
    for wg in input.wheels {
        let front = wg.origin[2] < z_mid;
        let wt: &VehWheel = if front {
            &sim.wheel_front
        } else {
            &sim.wheel_back
        };
        let axle_wheels = (if front { n_front } else { n_back }).max(1) as f32;
        let axle_share = if front { front_share } else { back_share };
        let wheel_load = mass * G * axle_share / axle_wheels;

        // Suspension: travel = extent + limit; spring rate supports the
        // static wheel load at SAG_FRACTION of travel, scaled by the
        // authored SuspensionFactor.
        let travel = (wt.suspension_extent + wt.suspension_limit).max(0.05);
        let sag = (SAG_FRACTION * travel).max(0.01);
        let spring_rate = wheel_load / sag * wt.suspension_factor.max(0.05);
        let damping_base = suspension_damping(spring_rate, wheel_load, wt.suspension_damp_coef);
        let suspension = SuspensionConfig {
            spring_rate,
            damping_compression: damping_base * 0.8,
            damping_rebound: damping_base,
            travel,
            force_apply_offset: 0.3,
            max_force: wheel_load * 10.0,
        };

        // Hardpoint raised so the wheel rests at `origin` under sag.
        let raise = (travel * (1.0 - SAG_FRACTION) + wg.radius - wg.origin[1]).max(0.0);
        let position = [wg.origin[0], wg.origin[1] + raise, wg.origin[2]];

        let steer_scale = if front {
            1.0
        } else {
            (wt.steering_limit / sim.wheel_front.steering_limit.max(1e-3)).clamp(0.0, 1.0)
        };
        let steered = wt.steering_limit > 0.001;

        let tires = TireConfig {
            lateral_grip: wt.static_fric * GRIP_LAT_SCALE,
            longitudinal_grip: wt.static_fric * GRIP_LONG_SCALE,
            peak_slip_angle: wt.optimum_slip_percent.clamp(0.04, 0.6),
            peak_slip_ratio: (wt.optimum_slip_percent * 1.25).clamp(0.05, 0.6),
            slide_fraction: (wt.sliding_fric / wt.static_fric.max(0.01)).clamp(0.4, 0.95),
            rolling_resistance: wt.tire_drag_coef_long.clamp(0.0, 0.1),
            load_sensitivity: 0.3,
        };

        wheels.push(WheelConfig {
            position,
            radius: wg.radius,
            driven: if front {
                front_drive_share > 0.0
            } else {
                back_drive_share > 0.0
            },
            steered,
            steer_scale,
            brake_bias: wt.brake_coef / brake_sum,
            handbrake: wt.handbrake_coef > 0.0 && !front,
            handbrake_coef: Some(wt.handbrake_coef / brake_sum),
            drive_share: Some(if front {
                front_drive_share / axle_wheels
            } else {
                back_drive_share / axle_wheels
            }),
            suspension: Some(suspension),
            tires: Some(tires),
        });
    }
    report.derived(
        "vehCarSim.WheelFront/WheelBack + whlN.mtx",
        "wheels[]",
        format!(
            "{} wheels ({} front), per-axle suspension/tires/brakes",
            wheels.len(),
            n_front
        ),
    );

    // --- engine -------------------------------------------------------------
    let power_w = sim.engine.max_horsepower * HP_TO_W;
    let opt_rpm = sim.engine.opt_rpm;
    let omega_opt = opt_rpm * std::f32::consts::TAU / 60.0;
    let torque_at_opt = if omega_opt > 0.0 {
        power_w / omega_opt
    } else {
        0.0
    };
    let peak_torque_nm = torque_at_opt * TORQUE_PEAK_FACTOR;
    let peak_torque_rpm = (opt_rpm * TORQUE_PEAK_RPM_FRAC).max(sim.engine.idle_rpm * 1.2);
    let engine = EngineConfig {
        idle_rpm: sim.engine.idle_rpm,
        redline_rpm: sim.engine.max_rpm.max(opt_rpm * 1.05),
        peak_torque_rpm,
        peak_torque_nm,
        peak_power_rpm: Some(opt_rpm),
        max_power_w: Some(power_w),
        redline_torque_fraction: 0.8,
        rpm_response: 8.0,
        engine_brake_nm: torque_at_opt * 0.35,
    };
    report.derived(
        "vehCarSim.Engine.MaxHorsePower/OptRPM",
        "engine.max_power_w/peak_power_rpm",
        format!(
            "{:.0} hp → {power_w:.0} W at {opt_rpm:.0} rpm",
            sim.engine.max_horsepower
        ),
    );
    report.adapted(
        "vehCarSim.Engine (curve shape)",
        "engine.peak_torque_nm/peak_torque_rpm",
        format!("torque peak {peak_torque_nm:.0} N·m at {peak_torque_rpm:.0} rpm"),
    );
    report.imported(
        "vehCarSim.Engine.IdleRPM/MaxRPM",
        "engine.idle_rpm/redline_rpm",
        "",
    );

    // --- transmission ---------------------------------------------------------
    let n_gears = sim.trans.auto_num_gears.max(1) as usize;
    let driven_radius = input
        .wheels
        .iter()
        .zip(&wheels)
        .find(|(_, wc)| wc.driven)
        .map(|(wg, _)| wg.radius)
        .or_else(|| input.wheels.first().map(|w| w.radius))
        .unwrap_or(0.34)
        .max(0.05);
    let bias = sim.trans.gear_bias.clamp(0.05, 0.95);
    let mut gear_ratios = Vec::with_capacity(n_gears);
    for i in 0..n_gears {
        // Band top speed for gear i: geometric interpolation Low→High with
        // GearBias warping the exponent (0.5 = uniform).
        let t = if n_gears > 1 {
            i as f32 / (n_gears - 1) as f32
        } else {
            1.0
        };
        let t_w = t + (2.0 * bias - 1.0) * t * (1.0 - t);
        let v_mph = sim.trans.low_mph * (sim.trans.high_mph / sim.trans.low_mph.max(0.1)).powf(t_w);
        let v_ms = (v_mph * MPH_TO_MPS).max(1.0);
        let wheel_rps = v_ms / (std::f32::consts::TAU * driven_radius);
        gear_ratios.push((opt_rpm / 60.0 / wheel_rps).max(0.5));
    }
    let rev_ms = (sim.trans.reverse_mph * MPH_TO_MPS).max(1.0);
    let reverse_ratio =
        (opt_rpm / 60.0 / (rev_ms / (std::f32::consts::TAU * driven_radius))).max(0.5);
    let transmission = TransmissionConfig {
        gear_ratios,
        reverse_ratio,
        final_drive: 1.0,
        shift_time: sim.trans.gear_change_time,
        efficiency: DRIVELINE_EFFICIENCY,
        upshift_rpm: Some(opt_rpm),
        downshift_rpm: Some(opt_rpm * 0.5),
    };
    report.derived(
        "vehCarSim.Trans.Low/High/Reverse/GearBias/AutoNumGears",
        "transmission.gear_ratios",
        format!(
            "{n_gears} gears {low:.0}–{high:.0} mph at OptRPM on r={driven_radius:.2} m wheels: {ratios:?}",
            low = sim.trans.low_mph,
            high = sim.trans.high_mph,
            ratios = transmission
                .gear_ratios
                .iter()
                .map(|r| format!("{r:.2}"))
                .collect::<Vec<_>>()
        ),
    );
    report.imported(
        "vehCarSim.Trans.GearChangeTime",
        "transmission.shift_time",
        format!("{:.2} s", sim.trans.gear_change_time),
    );
    report.adapted(
        "-",
        "transmission.efficiency/upshift_rpm/downshift_rpm",
        format!("efficiency {DRIVELINE_EFFICIENCY}, shift at OptRPM"),
    );

    // --- steering ----------------------------------------------------------
    let low_angle = sim.wheel_front.steering_limit.clamp(0.05, 0.7);
    let high_angle =
        (low_angle * (1.0 - sim.wheel_front.steering_offset * 0.8)).clamp(0.04, low_angle);
    let high_speed = input
        .asnode
        .and_then(|a| a.speed_base_hi)
        .unwrap_or(sim.trans.high_mph * MPH_TO_MPS)
        .clamp(15.0, 90.0);
    let steering = SteeringConfig {
        low_speed_max_angle: low_angle,
        high_speed_max_angle: high_angle,
        high_speed,
        input_rate: 4.0,
        return_rate: 7.0,
        response_curve: 1.4,
    };
    report.imported(
        "vehCarSim.WheelFront.SteeringLimit",
        "steering.low_speed_max_angle",
        format!("{low_angle:.2} rad"),
    );
    report.adapted(
        "vehCarSim.WheelFront.SteeringOffset + asNode.SpeedBaseHi",
        "steering.high_speed_max_angle/high_speed",
        format!("{high_angle:.2} rad above {high_speed:.1} m/s; input/return adapted"),
    );

    // --- aero ------------------------------------------------------------------
    let frontal_area = size[0] * size[1] * 0.85;
    let aero = AeroConfig {
        drag_coefficient: sim.aero.drag * frontal_area * AIR_DENSITY,
        downforce_coefficient: sim.aero.down.max(0.0) * frontal_area * AIR_DENSITY,
    };
    report.derived(
        "vehCarSim.Aero.Drag/Down",
        "aero.*",
        format!("Cd·A·ρ with A={frontal_area:.2} m² ({bounds_src} bounds)"),
    );

    // --- collider --------------------------------------------------------------
    let collider_points = input.bound.map(|b| {
        b.verts
            .iter()
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>()
    });
    report.imported(
        "bound/<id>_bound.bnd",
        "collider_points/chassis_size",
        format!(
            "{} hull verts from {bounds_src}",
            collider_points.as_ref().map(|p| p.len()).unwrap_or(0)
        ),
    );

    let assists = AssistConfig {
        // Every stock MM2 car carries its mass about as high as its track
        // is wide while running tires good for 1.65 g — geometry that puts
        // the car on its roof in any committed corner. See
        // `AssistConfig::roll_resistance`.
        roll_resistance: ROLL_RESISTANCE,
        yaw_stability: 2.0,
        traction_control: 0.85,
        countersteer: 0.3,
        air_control: 3.0,
    };
    report.defaulted(
        "assists",
        "fixed modern arcade policy (yaw stability, TC, countersteer, air control)",
    );
    report.adapted(
        "vehCarSim.CenterOfGravity + track width",
        "assists.roll_resistance",
        format!(
            "{ROLL_RESISTANCE} of the lateral-force roll moment cancelled; stock geometry tips below its grip limit without it"
        ),
    );
    report.unsupported(
        "vehCarSim.SSS*/CarFrictionHandling/Aero.Ang*",
        "legacy steering/angular damping fields replaced by the assist policy",
    );
    report.unsupported(
        "vehCarSim.Wheel*.CamberLimit/WobbleLimit/TireDisp*",
        "cam/wobble and slip-displacement internals have no direct analog",
    );

    let config = VehicleConfig {
        name: input.display_name.to_string(),
        mass,
        center_of_mass: com,
        wheelbase,
        track_width,
        chassis_size: size,
        inertia: Some(inertia),
        collider_points,
        collider_friction: sim.bound_friction.unwrap_or(0.5),
        collider_restitution: sim.bound_elasticity.unwrap_or(0.1).clamp(0.0, 0.4),
        wheels,
        suspension: SuspensionConfig {
            // Global fallback; every imported wheel carries its own.
            spring_rate: 55_000.0,
            damping_compression: 4_500.0,
            damping_rebound: 5_500.0,
            travel: 0.3,
            force_apply_offset: 0.3,
            max_force: mass * G,
        },
        engine,
        transmission,
        tires: TireConfig {
            lateral_grip: 1.6,
            longitudinal_grip: 1.7,
            peak_slip_angle: 0.16,
            peak_slip_ratio: 0.18,
            slide_fraction: 0.75,
            rolling_resistance: 0.012,
            load_sensitivity: 0.3,
        },
        steering,
        brakes: BrakeConfig {
            max_brake_force: total_brake_force,
            handbrake_strength: 0.7,
        },
        aero,
        assists,
        trailer: false,
    };

    Ok(Converted { config, report })
}

/// Convert trailer tuning into a `VehicleConfig` driving the trailer body.
///
/// The trailer has no engine: wheels are undriven/unsteered-by-input, and
/// rear wheels get their authored (tiller) steering handled by the joint,
/// not the input path — in the current model they are simply passive.
pub fn convert_trailer(
    id: &str,
    t: &VehTrailer,
    wheels: &[WheelGeom],
    bound: Option<&BndFile>,
    body_aabb: ([f32; 3], [f32; 3]),
) -> Result<Converted, String> {
    let mut report = ConversionReport::default();
    report.warnings.extend(t.warnings.iter().cloned());
    if wheels.is_empty() {
        return Err(format!("{id}: trailer has no wheel transforms"));
    }
    let (bmin, bmax, _) = aabb_min_max(bound, body_aabb);
    let size = [
        (bmax[0] - bmin[0]).max(0.05),
        (bmax[1] - bmin[1]).max(0.05),
        (bmax[2] - bmin[2]).max(0.05),
    ];
    let mass = t.mass.max(50.0);
    let ib = t.inertia_box;
    let inertia = [
        mass * (ib[1] * ib[1] + ib[2] * ib[2]) / 12.0,
        mass * (ib[0] * ib[0] + ib[2] * ib[2]) / 12.0,
        mass * (ib[0] * ib[0] + ib[1] * ib[1]) / 12.0,
    ];
    let z_min = wheels.iter().map(|w| w.origin[2]).fold(f32::MAX, f32::min);
    let z_max = wheels.iter().map(|w| w.origin[2]).fold(f32::MIN, f32::max);
    let z_mid = (z_min + z_max) * 0.5;
    let wheelbase = (z_max - z_min).abs().max(0.2);
    let track_width = wheels
        .iter()
        .map(|w| w.origin[0].abs() * 2.0)
        .fold(0.0f32, f32::max)
        .max(0.4);

    let mut ws = Vec::with_capacity(wheels.len());
    for wg in wheels {
        let front = wg.origin[2] < z_mid;
        let wt = if front { &t.wheel_front } else { &t.wheel_back };
        let wheel_load = mass * G / wheels.len() as f32;
        let travel = (wt.suspension_extent + wt.suspension_limit).max(0.05);
        let spring_rate =
            wheel_load / (SAG_FRACTION * travel).max(0.01) * wt.suspension_factor.max(0.05);
        let damping_base = suspension_damping(spring_rate, wheel_load, wt.suspension_damp_coef);
        let raise = (travel * (1.0 - SAG_FRACTION) + wg.radius - wg.origin[1]).max(0.0);
        ws.push(WheelConfig {
            position: [wg.origin[0], wg.origin[1] + raise, wg.origin[2]],
            radius: wg.radius,
            driven: false,
            steered: false,
            steer_scale: 1.0,
            brake_bias: 1.0 / wheels.len() as f32,
            handbrake: false,
            handbrake_coef: None,
            drive_share: None,
            suspension: Some(SuspensionConfig {
                spring_rate,
                damping_compression: damping_base * 0.8,
                damping_rebound: damping_base,
                travel,
                force_apply_offset: 0.3,
                max_force: wheel_load * 10.0,
            }),
            tires: Some(TireConfig {
                lateral_grip: wt.static_fric * GRIP_LAT_SCALE,
                longitudinal_grip: wt.static_fric * GRIP_LONG_SCALE,
                peak_slip_angle: wt.optimum_slip_percent.clamp(0.04, 0.6),
                peak_slip_ratio: (wt.optimum_slip_percent * 1.25).clamp(0.05, 0.6),
                slide_fraction: (wt.sliding_fric / wt.static_fric.max(0.01)).clamp(0.4, 0.95),
                rolling_resistance: wt.tire_drag_coef_long.clamp(0.0, 0.1),
                load_sensitivity: 0.3,
            }),
        });
    }

    let config = VehicleConfig {
        name: format!("{id} trailer"),
        mass,
        center_of_mass: [0.0, (bmin[1] + bmax[1]) * 0.5, 0.0],
        wheelbase,
        track_width,
        chassis_size: size,
        inertia: Some(inertia),
        collider_points: bound.map(|b| {
            b.verts
                .iter()
                .map(|v| [v[0], v[1], v[2]])
                .collect::<Vec<_>>()
        }),
        collider_friction: 0.5,
        collider_restitution: 0.05,
        wheels: ws,
        trailer: true,
        ..VehicleConfig::default()
    };
    report.imported(
        "vehTrailer.Mass/InertiaBox",
        "mass/inertia",
        format!("{mass} kg"),
    );
    Ok(Converted { config, report })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Damping ratio implied by a rate, as the physics crate measures it.
    fn zeta(damping: f32, spring_rate: f32, wheel_load: f32) -> f32 {
        let corner_mass = wheel_load / G;
        damping / (2.0 * (spring_rate * corner_mass).sqrt())
    }

    #[test]
    fn suspension_damping_lands_in_the_playable_band() {
        // A 300 kg corner on a spring that settles it a hand's width down.
        let wheel_load = 300.0 * G;
        let spring_rate = wheel_load / 0.09;

        // The London Cab's authored coefficient is the roster's lowest and
        // used to map to ζ ≈ 0.1 — a car that pogos off every road seam.
        let soft = suspension_damping(spring_rate, wheel_load, 0.01);
        // Most cars author 0.1.
        let firm = suspension_damping(spring_rate, wheel_load, 0.1);

        for (label, d) in [("soft", soft), ("firm", firm)] {
            let z = zeta(d, spring_rate, wheel_load);
            assert!(
                (DAMPING_RATIO_MIN..=DAMPING_RATIO_MAX).contains(&z),
                "{label} damping ratio {z} outside the band"
            );
        }
        // The authored value still orders the roster.
        assert!(firm > soft);
        // And an absurd coefficient saturates rather than locking solid.
        let wild = suspension_damping(spring_rate, wheel_load, 100.0);
        assert!(zeta(wild, spring_rate, wheel_load) <= DAMPING_RATIO_MAX + 1e-5);
    }

    #[test]
    fn suspension_damping_scales_with_the_corner_it_carries() {
        // Critical damping goes as sqrt(k·m): four times the mass on the
        // same spring needs twice the damping for the same ratio.
        let spring_rate = 30_000.0;
        let light = suspension_damping(spring_rate, 250.0 * G, 0.1);
        let heavy = suspension_damping(spring_rate, 1000.0 * G, 0.1);
        assert!(
            (heavy / light - 2.0).abs() < 1e-3,
            "expected 2x damping, got {}",
            heavy / light
        );
    }
}
