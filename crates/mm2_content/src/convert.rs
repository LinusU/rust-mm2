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
//! * `CenterOfGravity` locates the model *from* the centre of mass, so in
//!   plan view the centre of mass sits at `-CenterOfGravity` from the
//!   model origin (the original's static load split; see
//!   docs/vehicle-handling.md "Centre of mass"). Imported original
//!   configs use all three negated authored coordinates verbatim.
//! * `Trans.Low`/`High`/`Reverse` are per-band top speeds in mph; combined
//!   gear ratios are derived so the engine sits at `OptRPM` at each band's
//!   top speed, geometrically interpolated (biased by `GearBias`).
//! * The generic scalar summaries retain the earlier arcade mapping.
//!   Actual imported simulation uses `original`: 746 W/hp, the recovered
//!   √5 torque polynomial, retail gearbox and tyre/suspension laws.

use mm2_formats::bnd::BndFile;
use mm2_formats::veh::{
    AsNode, DrivetrainType, VehCarSim, VehGyro, VehStuck, VehTrailer, VehWheel,
};
use mm2_vehicle::config::{
    AeroConfig, AssistConfig, BrakeConfig, EngineConfig, GyroConfig, OriginalAero, OriginalAxle,
    OriginalEngine, OriginalGearbox, OriginalHandling, OriginalTrain, OriginalWheel,
    SteeringConfig, SuspensionConfig, TireConfig, TransmissionConfig, VehicleConfig, WheelConfig,
};

use mm2_vehicle::original;

const ORIGINAL_GRAVITY: f32 = 19.6;
const ORIGINAL_ROAD_FRICTION: f32 = 0.9;
const ORIGINAL_HP_TO_W: f32 = 746.0;
const ORIGINAL_MAX_ANGULAR_SPEED: f32 = 4.0 * std::f32::consts::PI;

const MPH_TO_MPS: f32 = 0.44704;
const HP_TO_W: f32 = 745.7;
const G: f32 = 9.81;
/// Standard air density for aero forces.
const AIR_DENSITY: f32 = 1.225;
/// Rest compression target as a fraction of suspension travel (adapted).
/// A smaller fraction is a stiffer spring on the same travel: `0.35` is
/// about 13% more natural frequency than the `0.45` it was, with more
/// of the travel left for bumps.
const SAG_FRACTION: f32 = 0.35;
/// Lateral/longitudinal grip scale from MM2 static friction (adapted).
/// Lateral is the arcade one: MM2 cars corner far harder than real ones,
/// and `0.9` is as far as the roll assist holds the tall cars down —
/// see docs/vehicle-handling.md "Grip scale".
const GRIP_LAT_SCALE: f32 = 0.9;
const GRIP_LONG_SCALE: f32 = 0.60;
/// Torque-peak placement and height relative to the power peak (adapted).
const TORQUE_PEAK_RPM_FRAC: f32 = 0.72;
const TORQUE_PEAK_FACTOR: f32 = 1.15;
/// Driveline efficiency applied to every imported car (adapted constant).
const DRIVELINE_EFFICIENCY: f32 = 0.85;
/// Shape imposed on the underside of an imported collision hull.
///
/// MM2 car bounds are the real body shell: a flat floor slung between
/// axles that sit well inside it, 0.13-0.25 m off the road on most of the
/// roster. That geometry catches on the crown of every intersection and
/// on road seams the wheels ride over without noticing — the Mustang
/// clears an 8° crest, and the F-350 is only bearable because its big
/// wheels and long travel give it 17°.
const MIN_GROUND_CLEARANCE: f32 = 0.25;
const APPROACH_ANGLE: f32 = 25.0 * std::f32::consts::PI / 180.0;
const DEPARTURE_ANGLE: f32 = 25.0 * std::f32::consts::PI / 180.0;
const BREAKOVER_ANGLE: f32 = 15.0 * std::f32::consts::PI / 180.0;
/// Ceiling on chassis panel friction. Scraping a wall is how an arcade
/// racer takes a corner too fast; at the authored values (up to 0.8 on the
/// London Cab) the body grabs the wall instead of sliding down it and the
/// car spins. Tires still do all the gripping.
const MAX_COLLIDER_FRICTION: f32 = 0.3;
/// Ceiling on chassis restitution. MM2's `BoundElasticity` (0.3-0.5 on
/// stock cars) feeds its own car-versus-car impulse solver, not a
/// coefficient of restitution against road geometry; used as one, a car
/// body trampolines off every kerb it touches. A car should thud.
const MAX_RESTITUTION: f32 = 0.1;
/// Suspension damping ratio band imported cars are mapped into: below
/// ~0.5 a car wallows and pogos off road seams, above ~1.0 the springs
/// stop moving and the chassis takes every impact instead.
const DAMPING_RATIO_MIN: f32 = 0.55;
const DAMPING_RATIO_MAX: f32 = 1.0;
/// Share of the lateral-force roll moment cancelled on imported cars
/// (adapted arcade policy — see `AssistConfig::roll_resistance`).
const ROLL_RESISTANCE: f32 = 0.95;
/// Steepest pitch gradient, radians per g of longitudinal acceleration,
/// an imported car keeps its full squat and dive on (adapted). The stock
/// roster runs 0.020 (City Bus) to 0.073 (London Cab); the Moon Rover,
/// 0.63 m of centre-of-mass height over a 0.86 m wheelbase on soft
/// springs, is 0.576 — it stood on its tail under its own drive. Lever
/// beyond this is cancelled by `AssistConfig::pitch_resistance`, which is
/// therefore zero on every other stock car — see
/// docs/vehicle-handling.md "Pitch resistance".
const MAX_PITCH_GRADIENT: f32 = 0.08;
/// Margin a car's traction-limited acceleration keeps below the
/// acceleration that tips it onto its rear wheels (adapted). `1.2` means
/// the drive may use at most 1/1.2 of the way to the wheelie; lever beyond
/// that is cancelled by `AssistConfig::pitch_resistance` — see
/// docs/vehicle-handling.md "Wheelies".
const WHEELIE_MARGIN: f32 = 1.2;
/// Share of a driven tire's grip the drive may use, for imported cars
/// (see `AssistConfig::traction_control`).
const TRACTION_CONTROL: f32 = 0.85;
/// How briskly an imported car levels itself in the air, rad/s — roughly
/// half a second to flat, so a jump lands on its wheels and not its nose.
const AIR_LEVELLING_RATE: f32 = 8.0;
/// How firmly a released handbrake slide is pulled back into line, rad/s
/// (see `AssistConfig::slide_recovery`).
const SLIDE_RECOVERY: f32 = 2.5;
/// Seconds an upended car waits before flopping back onto its wheels —
/// the fallback when the vehicle ships no `vehstuck` record; authored
/// cars right on their own `TimeThresh`.
const SELF_RIGHT_DELAY: f32 = 2.0;
/// Ceiling on gear change time. MM2 authors 0.8-1.0 s, which is most of a
/// second of interrupted drive on every upshift; five of them between rest
/// and top speed make acceleration arrive in steps.
const MAX_SHIFT_TIME: f32 = 0.35;
/// Multiple of the authored `SteeringLimit` a car gets as full lock
/// (adapted). Stock locks run 0.35–0.6 rad and a keyboard driver holds
/// full lock through every city corner, so the lock is the tightest
/// corner a car can take below the speeds the grip cap governs. At
/// `1.5` a car turns 10–20° more in a second from 30 km/h; beyond about
/// 50 km/h the grip cap decides and this changes nothing — see
/// docs/vehicle-handling.md "Grip-limited steering".
const STEERING_LOCK_SCALE: f32 = 1.5;
/// Multiple of tire grip the steering lock may demand (adapted arcade
/// policy — stock locks ask for 8-64 g at their high-speed limit). The
/// cap already allows the front tires their peak slip on top, so any
/// margin here only ploughs.
const STEERING_GRIP_LIMIT: f32 = 1.0;

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
    /// Wheel width from model/pivot bounds, metres (steering inner-edge pivot).
    pub width: f32,
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
    /// Decoded `vehstuck` record when present — `TimeThresh` feeds the
    /// self-right delay so an authored car rights on its own bound
    /// (F05-B.2); the rest of the record is the game-side detector's.
    pub stuck: Option<&'a VehStuck>,
    /// Decoded `vehgyro` record when present — the authored
    /// spin/drift/righting rates carry verbatim onto `config.gyro`
    /// (F05-B.4); absent keeps `None`, never a fabricated assist.
    pub gyro: Option<&'a VehGyro>,
}

/// Output of one conversion.
pub struct Converted {
    pub config: VehicleConfig,
    pub report: ConversionReport,
}

/// Raise the underside of a collision hull so the body clears road seams
/// and crests instead of snagging on them.
///
/// Each vertex is lifted to whatever height its position demands: beyond
/// an axle that is the ramp the overhang has to clear, between the axles
/// it is the crest rising from the nearer axle, and everywhere it is at
/// least [`MIN_GROUND_CLEARANCE`]. The visual body is untouched, so a car
/// still *looks* slammed — it just stops tripping over the road.
fn clear_underside(points: &mut [[f32; 3]], ground_y: f32, front_z: f32, rear_z: f32) {
    let top = points.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    for p in points.iter_mut() {
        let ahead = front_z - p[2];
        let behind = p[2] - rear_z;
        let ramp = if ahead > 0.0 {
            ahead * APPROACH_ANGLE.tan()
        } else if behind > 0.0 {
            behind * DEPARTURE_ANGLE.tan()
        } else {
            (p[2] - front_z).min(rear_z - p[2]) * BREAKOVER_ANGLE.tan()
        };
        let floor = ground_y + ramp.max(MIN_GROUND_CLEARANCE);
        // Never lift the floor through the roof of a very low body.
        p[1] = p[1].max(floor.min(top - 0.05));
    }
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

    // The original places the model at the inertial origin plus
    // `CenterOfGravity`, so in model space the centre of mass is at
    // `-CenterOfGravity`: `vehWheel::ComputeConstants` gives each wheel
    // `|z - CenterOfGravity.z| / 2|z|` of the weight, loading the front
    // (-z) axle for a positive offset. Read the other way the Moon Rover's
    // +0.4 put its mass behind its rear axle. Height is the exception:
    // `-y` from the model origin puts every stock car's mass below its
    // wheel hubs (the London Cab's under the road), so it stays bound
    // centre plus the authored offset — see docs/vehicle-handling.md
    // "Centre of mass".
    let cog = sim.center_of_gravity;
    let com = [-cog[0], bcenter[1] + cog[1], -cog[2]];
    report.derived(
        "vehCarSim.CenterOfGravity",
        "center_of_mass.x/z",
        format!(
            "-CenterOfGravity from the model origin ({:.2}, {:.2})",
            com[0], com[2]
        ),
    );
    report.adapted(
        "vehCarSim.CenterOfGravity",
        "center_of_mass.y",
        format!(
            "bound centre {:.2} + authored offset {:.2}; the original's -y from the model origin sits below the wheel hubs",
            bcenter[1], cog[1]
        ),
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
    let mut contact_ys: Vec<f32> = Vec::with_capacity(input.wheels.len());
    // Static spring compression per axle, front then rear: what sets how
    // far the body pitches for a given load transfer.
    let mut axle_sag = [0.0f32; 2];
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

        // Where this wheel's contact patch ends up once the spring has
        // settled under its share of the weight. `SuspensionFactor` moves
        // it off the design sag, so solve it rather than assume it.
        let rest_compression = (wheel_load / spring_rate).min(travel);
        contact_ys.push(position[1] - (travel - rest_compression) - wg.radius);
        let axle = &mut axle_sag[usize::from(!front)];
        *axle = axle.max(rest_compression);

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
        shift_time: sim.trans.gear_change_time.min(MAX_SHIFT_TIME),
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
    report.adapted(
        "vehCarSim.Trans.GearChangeTime",
        "transmission.shift_time",
        format!(
            "{:.2} s authored, capped at {MAX_SHIFT_TIME} s",
            sim.trans.gear_change_time
        ),
    );
    report.adapted(
        "-",
        "transmission.efficiency/upshift_rpm/downshift_rpm",
        format!("efficiency {DRIVELINE_EFFICIENCY}, shift at OptRPM"),
    );

    // --- steering ----------------------------------------------------------
    let low_angle = (sim.wheel_front.steering_limit * STEERING_LOCK_SCALE).clamp(0.05, 0.75);
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
        grip_limit: STEERING_GRIP_LIMIT,
    };
    report.adapted(
        "vehCarSim.WheelFront.SteeringLimit",
        "steering.low_speed_max_angle",
        format!(
            "{:.2} rad authored × {STEERING_LOCK_SCALE} = {low_angle:.2} rad",
            sim.wheel_front.steering_limit
        ),
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
    let ground_y = if contact_ys.is_empty() {
        0.0
    } else {
        contact_ys.iter().sum::<f32>() / contact_ys.len() as f32
    };
    // The unmodified bound is kept beside the reshaped hull: world
    // contact uses `collider_points` (underside lifted against snags),
    // while props are struck through `striker_points` — the shape the
    // original bound-vs-bound prop collision used.
    let striker_points = input.bound.map(|b| {
        b.verts
            .iter()
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>()
    });
    let collider_points = striker_points.clone().map(|mut pts| {
        clear_underside(&mut pts, ground_y, front_z, rear_z);
        pts
    });
    report.adapted(
        "bound/<id>_bound.bnd (underside)",
        "collider_points",
        format!(
            "floor raised to {MIN_GROUND_CLEARANCE} m with {:.0}°/{:.0}° approach/departure and {:.0}° breakover",
            APPROACH_ANGLE.to_degrees(),
            DEPARTURE_ANGLE.to_degrees(),
            BREAKOVER_ANGLE.to_degrees(),
        ),
    );
    report.adapted(
        "vehCarSim.BoundFriction",
        "collider_friction",
        format!("capped at {MAX_COLLIDER_FRICTION} so a body scrape slides along a wall rather than grabbing it"),
    );
    report.adapted(
        "vehCarSim.BoundElasticity",
        "collider_restitution",
        format!("scaled into 0..{MAX_RESTITUTION}; MM2's value drives its own impact solver, not road bounce"),
    );
    report.imported(
        "bound/<id>_bound.bnd",
        "collider_points/striker_points/chassis_size",
        format!(
            "{} hull verts from {bounds_src} (striker keeps the unmodified bound)",
            collider_points.as_ref().map(|p| p.len()).unwrap_or(0)
        ),
    );

    // Pitch gradient — radians of pitch per g of longitudinal
    // acceleration. Load transfer `m·a·h/L` compresses one axle and
    // extends the other by its static sag in proportion to the load it
    // carries, so the body pitches `h/L² · (sag_f/share_f + sag_r/share_r)`
    // per g. Longitudinal force is raised toward the centre of mass just
    // far enough to bring that inside `MAX_PITCH_GRADIENT`; zero on every
    // car already inside it.
    let com_height = com[1] - ground_y;
    let pitch_gradient = (com_height.max(0.0) / (wheelbase * wheelbase)
        * (axle_sag[0] / front_share + axle_sag[1] / back_share))
        .max(0.0);
    let gradient_resistance = if pitch_gradient > MAX_PITCH_GRADIENT {
        (1.0 - MAX_PITCH_GRADIENT / pitch_gradient).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Wheelie limit. Drive force at the contact patch pitches the car up
    // about the rear axle, and the front wheels leave the road at
    // `a = g·b/h` — `b` the centre of mass's distance ahead of the rear
    // axle, `h` its height. A rear-driven car is traction-limited to
    // roughly `grip · traction_control · g` with all its weight on the
    // rear wheels, which is exactly where it stands up. Lever beyond what
    // keeps `WHEELIE_MARGIN` between the two is cancelled; a front-driven
    // car unloads its own driven wheels first and needs nothing.
    let rear_driven_grip = {
        let grips: Vec<f32> = wheels
            .iter()
            .filter(|w| w.driven && w.position[2] > z_mid)
            .filter_map(|w| w.tires.as_ref().map(|t| t.longitudinal_grip))
            .collect();
        (!grips.is_empty()).then(|| grips.iter().sum::<f32>() / grips.len() as f32)
    };
    // `a·h·margin < g·b` with `a = grip·tc·g` reduces to a length: the
    // lever the drive force would need, against the arm that holds the
    // nose down.
    let wheelie_lever = rear_driven_grip
        .map(|grip| grip * TRACTION_CONTROL * WHEELIE_MARGIN * com_height.max(0.0))
        .unwrap_or(0.0);
    let rear_arm = (rear_z - com_z_mm2).max(0.05);
    let wheelie_resistance = if wheelie_lever > rear_arm {
        (1.0 - rear_arm / wheelie_lever).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pitch_resistance = gradient_resistance.max(wheelie_resistance);

    let assists = AssistConfig {
        // Every stock MM2 car carries its mass about as high as its track
        // is wide while running tires good for 2.25 g — geometry that puts
        // the car on its roof in any committed corner. See
        // `AssistConfig::roll_resistance`.
        roll_resistance: ROLL_RESISTANCE,
        pitch_resistance,
        yaw_stability: 2.0,
        traction_control: TRACTION_CONTROL,
        countersteer: 0.3,
        air_control: AIR_LEVELLING_RATE,
        self_right_delay: input
            .stuck
            .map(|s| s.time_thresh)
            .unwrap_or(SELF_RIGHT_DELAY),
        slide_recovery: SLIDE_RECOVERY,
    };
    report.defaulted(
        "assists",
        "fixed modern arcade policy (yaw stability, TC, countersteer, air control, slide recovery)",
    );
    if let Some(s) = input.stuck {
        report.imported(
            "vehstuck.TimeThresh",
            "assists.self_right_delay",
            format!(
                "authored {}s persistence bound drives the upended-car recovery",
                s.time_thresh
            ),
        );
    }
    report.adapted(
        "vehCarSim.CenterOfGravity + track width",
        "assists.roll_resistance",
        format!(
            "{ROLL_RESISTANCE} of the lateral-force roll moment cancelled; stock geometry tips below its grip limit without it"
        ),
    );
    report.adapted(
        "vehCarSim.CenterOfGravity + whlN.mtx origins",
        "assists.pitch_resistance",
        format!(
            "{pitch_resistance:.2} of the longitudinal-force pitch moment cancelled; {pitch_gradient:.3} rad/g pitch gradient (centre of mass {com_height:.2} m over a {wheelbase:.2} m wheelbase), kept at or below {MAX_PITCH_GRADIENT}; {wheelie_resistance:.2} needed to hold a {WHEELIE_MARGIN} margin under the wheelie ({rear_arm:.2} m to the rear axle)"
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

    // F05-B.4: the authored `vehgyro` record verbatim — spin rates,
    // drift relief, optional airborne righting.  `VehGyro::validate`
    // calls a negative Drift/Spin180/Reverse180 malformed; none exists
    // on the retail roster but a mod could author one — warn and clamp
    // rather than sink the whole car.  Pitch/Roll keep the authored
    // sign the decoder allows (negative rates are inert at use).
    let gyro = input.gyro.map(|g| {
        let mut bad: Vec<String> = Vec::new();
        let cfg = GyroConfig {
            spin180: gyro_rate("Spin180", g.spin180, &mut bad),
            reverse180: gyro_rate("Reverse180", g.reverse180, &mut bad),
            drift: gyro_rate("Drift", g.drift, &mut bad),
            pitch: gyro_axis("Pitch", g.pitch, &mut bad),
            roll: gyro_axis("Roll", g.roll, &mut bad),
        };
        report.warnings.extend(bad);
        cfg
    });
    if let Some(g) = &gyro {
        report.imported(
            "vehgyro.Spin180/Reverse180/Drift (+Pitch/Roll)",
            "gyro",
            format!(
                "authored gyro gains — spin {:.2}/{:.2}, drift {:.2}, righting {}",
                g.spin180,
                g.reverse180,
                g.drift,
                match (g.pitch, g.roll) {
                    (Some(p), Some(r)) => format!("pitch {p:.2}, roll {r:.2}"),
                    _ => "absent".into(),
                },
            ),
        );
    }

    let mut config = VehicleConfig {
        name: input.display_name.to_string(),
        mass,
        center_of_mass: com,
        wheelbase,
        track_width,
        chassis_size: size,
        inertia: Some(inertia),
        collider_points,
        striker_points,
        collider_friction: sim.bound_friction.unwrap_or(0.5).min(MAX_COLLIDER_FRICTION),
        collider_restitution: (sim.bound_elasticity.unwrap_or(0.1) * MAX_RESTITUTION)
            .clamp(0.0, MAX_RESTITUTION),
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
        gyro,
        trailer: false,
        // `mmDashView`'s gauge full-scales bind to the carsim, not the
        // dash record — the speedo's MaxSpeed is the authored top-gear
        // top speed (HUD-1/F22-B.1; inferred binding, DSN-47).
        top_speed_mps: (sim.trans.high_mph > 0.0 && sim.trans.high_mph.is_finite())
            .then_some(sim.trans.high_mph * MPH_TO_MPS),
        original: None,
        player_steering: Some(player_steering(input.asnode)),
    };

    report.add(
        "tune/<id>.asnode",
        "player_steering",
        if input.asnode.is_some() {
            Provenance::Imported
        } else {
            Provenance::Defaulted
        },
        "retail human-device steering ramp; independent of wheel steering limits",
    );
    apply_original(input, &mut config, &mut report);

    Ok(Converted { config, report })
}

/// Put `config` on the original game's car model: build its
/// [`OriginalHandling`] from the authored tuning, and restate the arcade
/// fields as a summary of what that model does for the consumers that
/// plan against them (AI pace and steering, handling analysis, FX).
///
/// The original needs the four wheels it was written for — front and
/// rear, left and right. A rig without them keeps the arcade model.
fn apply_original(
    input: &ConvertInput<'_>,
    config: &mut VehicleConfig,
    report: &mut ConversionReport,
) {
    let sim = input.sim;
    let geoms = input.wheels;
    if geoms.len() != 4 {
        report.warnings.push(format!(
            "{}: {} physics wheels — the original model needs four; arcade handling kept",
            input.id,
            geoms.len()
        ));
        return;
    }
    let z_min = geoms.iter().map(|w| w.origin[2]).fold(f32::MAX, f32::min);
    let z_max = geoms.iter().map(|w| w.origin[2]).fold(f32::MIN, f32::max);
    let z_mid = (z_min + z_max) * 0.5;
    let is_rear: Vec<bool> = geoms.iter().map(|w| w.origin[2] >= z_mid).collect();
    let find = |rear: bool, left: bool| {
        geoms
            .iter()
            .enumerate()
            .find(|(i, w)| is_rear[*i] == rear && (w.origin[0] < 0.0) == left)
            .map(|(i, _)| i)
    };
    let (Some(fl), Some(fr), Some(rl), Some(rr)) = (
        find(false, true),
        find(false, false),
        find(true, true),
        find(true, false),
    ) else {
        report.warnings.push(format!(
            "{}: wheel rig is not front/rear × left/right — arcade handling kept",
            input.id
        ));
        return;
    };

    // Remove audit rows for generic mechanisms that this path replaces.
    report.entries.retain(|e| {
        !matches!(
            e.dest.as_str(),
            "assists" | "assists.roll_resistance" | "assists.pitch_resistance" | "center_of_mass"
        ) && !matches!(
            e.source.as_str(),
            "vehCarSim.SSS*/CarFrictionHandling/Aero.Ang*"
                | "vehCarSim.Wheel*.CamberLimit/WobbleLimit/TireDisp*"
        )
    });
    report.unsupported("vehCarSim.SSS*/CarFrictionHandling", "legacy switches retained in the parsed data; player steering uses recovered .asnode tuning");
    report.unsupported(
        "vehCarSim.Wheel*.CamberLimit/WobbleLimit",
        "visual camber and wobble are not simulated; tyre displacement and damping are imported",
    );
    // The recovered static-world response needs the authored shell, not
    // the raised underside used to keep the former arcade solver from snagging.
    config.collider_points = config.striker_points.clone();
    report
        .entries
        .retain(|e| e.source != "bound/<id>_bound.bnd (underside)");
    report.imported(
        "bound/<id>_bound.bnd",
        "collider_points",
        "authored hull retained for retail static-world contact response",
    );
    let mass = config.mass;
    let cog = sim.center_of_gravity;
    let gravity = ORIGINAL_GRAVITY;

    // --- wheels -------------------------------------------------------------
    let wheels: Vec<OriginalWheel> = geoms
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let wt = if is_rear[i] {
                &sim.wheel_back
            } else {
                &sim.wheel_front
            };
            // `ComputeConstants`: the axles are taken as symmetric about
            // the model origin, so `CenterOfGravity.z` alone splits the
            // weight — `|z − CoG.z| / 2|z|` of it per wheel.
            let z = g.origin[2];
            let static_load = if z.abs() > 1e-3 {
                gravity * 0.25 * mass * (z - cog[2]).abs() / z.abs()
            } else {
                gravity * 0.25 * mass
            };
            OriginalWheel {
                static_load: static_load.max(1.0),
                rear: is_rear[i],
                side: if g.origin[0] < 0.0 { -1.0 } else { 1.0 },
                width: g.width,
                suspension_extent: wt.suspension_extent.max(0.01),
                suspension_limit: wt.suspension_limit.max(0.0),
                suspension_factor: wt.suspension_factor.max(0.75),
                suspension_damp_coef: wt.suspension_damp_coef.max(0.0),
                steering_limit: wt.steering_limit,
                steering_offset: wt.steering_offset,
                brake_coef: wt.brake_coef.max(0.0),
                // Unauthored, the constructor's `1.0`.
                handbrake_coef: if is_rear[i] {
                    if i == rr {
                        1.0
                    } else {
                        wt.handbrake_coef.max(0.0)
                    }
                } else {
                    0.0
                },
                tire_disp_limit_lat: wt.tire_disp_limit_lat.max(1e-3),
                tire_disp_limit_long: wt.tire_disp_limit_long.max(1e-3),
                tire_damp_coef_lat: wt.tire_damp_coef_lat.max(0.0),
                tire_damp_coef_long: wt.tire_damp_coef_long.max(0.0),
                tire_drag_coef_lat: wt.tire_drag_coef_lat.max(0.0),
                tire_drag_coef_long: wt.tire_drag_coef_long.max(0.0),
                optimum_slip: wt.optimum_slip_percent.max(0.01),
                static_fric: wt.static_fric.max(0.0),
                sliding_fric: wt.sliding_fric.max(0.0),
            }
        })
        .collect();
    report.derived(
        "vehCarSim.Mass+CenterOfGravity.z + whlN.mtx",
        "original.wheels[].static_load",
        format!(
            "{:.0} / {:.0} N front/rear: the share of mass × {gravity} m/s² split by CenterOfGravity.z",
            wheels[fl].static_load, wheels[rl].static_load
        ),
    );
    report.imported(
        "vehCarSim.WheelFront/WheelBack",
        "original.wheels[]",
        "suspension, steering, brake and stick–slip tyre tokens verbatim",
    );
    report.imported(
        "vehCarSim.AxleFront/AxleBack",
        "original.axles[]",
        "authored anti-roll stiffness and damping with left/right wheel pairs",
    );

    // --- drivetrains ----------------------------------------------------------
    let (driven, free) = match sim.drivetrain_type {
        DrivetrainType::Rwd => (vec![rl, rr], vec![fl, fr]),
        DrivetrainType::Fwd => (vec![fl, fr], vec![rl, rr]),
        DrivetrainType::Awd => (vec![fl, fr, rl, rr], vec![]),
    };
    let train = |t: Option<&mm2_formats::veh::VehEndTrain>| OriginalTrain {
        ang_inertia: t.and_then(|t| t.ang_inertia).unwrap_or(5000.0).max(0.0),
        brake_dynamic_coef: t.and_then(|t| t.brake_dynamic_coef).unwrap_or(1.0).max(0.0),
        brake_static_coef: t.and_then(|t| t.brake_static_coef).unwrap_or(1.2).max(0.0),
    };
    let drivetrain = train(sim.drivetrain.as_ref());
    let freetrain = train(sim.freetrain.as_ref());

    // --- engine and gearbox -------------------------------------------------
    let engine = OriginalEngine {
        max_power_w: sim.engine.max_horsepower.max(1.0) * ORIGINAL_HP_TO_W,
        idle_rpm: sim.engine.idle_rpm.max(1.0),
        opt_rpm: sim.engine.opt_rpm.max(sim.engine.idle_rpm * 2.0 + 1.0),
        max_rpm: sim.engine.max_rpm.max(sim.engine.opt_rpm * 1.01),
        ang_inertia: sim.engine.ang_inertia.unwrap_or(1.0).max(1e-3),
        gear_change_lag: sim.engine.gcl.unwrap_or(0.25).max(0.0),
    };
    let constants = original::EngineConstants::of(&engine);
    // Ratios are anchored on the primary drivetrain's first wheel.
    let radius = geoms[driven[0]].radius.max(0.05);
    let ratios = original::gear_ratios(
        engine.opt_rpm,
        radius,
        sim.trans.reverse_mph,
        sim.trans.low_mph,
        sim.trans.high_mph,
        sim.trans.auto_num_gears.max(3) as usize,
        sim.trans.gear_bias,
    );
    let shifts = original::shift_points(
        &ratios,
        &constants,
        engine.max_rpm,
        sim.trans.upshift_bias,
        sim.trans.downshift_bias_min,
        sim.trans.downshift_bias_max,
    );
    report.derived(
        "vehCarSim.Trans.Low/High/Reverse/GearBias/AutoNumGears",
        "original.gearbox.ratios",
        format!(
            "{:?} (R, N, forward) at OptRPM on r={radius:.3} m; upshifts {:?} rpm",
            ratios.iter().map(|r| format!("{r:.2}")).collect::<Vec<_>>(),
            shifts
                .upshift
                .iter()
                .map(|r| format!("{r:.0}"))
                .collect::<Vec<_>>(),
        ),
    );
    let gearbox = OriginalGearbox {
        ratios,
        upshift_rpm: shifts.upshift,
        downshift_full_rpm: shifts.downshift_full,
        downshift_zero_rpm: shifts.downshift_zero,
        equal_power_rpm: shifts.equal_power,
        gear_change_time: sim.trans.gear_change_time.max(0.0),
    };

    let aero = OriginalAero {
        ang_c_damp: sim.aero.ang_c_damp.unwrap_or([0.0; 3]),
        ang_vel_damp: sim.aero.ang_vel_damp.unwrap_or([0.0; 3]),
        ang_vel2_damp: sim.aero.ang_vel2_damp.unwrap_or([0.0; 3]),
        drag: sim.aero.drag.max(0.0),
        down: sim.aero.down.max(0.0),
    };
    report.imported(
        "vehCarSim.Aero.AngCDamp/AngVelDamp/AngVel2Damp/Drag/Down",
        "original.aero",
        "rotational damping per car axis (x pitch, y yaw, z roll) and drag verbatim",
    );

    // --- the arcade fields, restated ----------------------------------------
    // Contact patches rest on the modelled pivots (`x = 0` carries `L`).
    // The render origin is body CoM + authored CenterOfGravity.
    config.center_of_mass = [-cog[0], -cog[1], -cog[2]];
    report.imported(
        "vehCarSim.CenterOfGravity",
        "center_of_mass",
        format!(
            "-CenterOfGravity from the model origin ({:.2}, {:.2}, {:.2}), verbatim authored offset",
            config.center_of_mass[0], config.center_of_mass[1], config.center_of_mass[2]
        ),
    );
    let mu = |w: &OriginalWheel| w.static_fric * ORIGINAL_ROAD_FRICTION * gravity / G;
    let front_lock = wheels[fl].steering_limit.max(0.01);
    for (i, (wc, ow)) in config.wheels.iter_mut().zip(&wheels).enumerate() {
        let k = original::WheelConstants::of(ow, geoms[i].radius, gravity);
        wc.position = [
            geoms[i].origin[0],
            geoms[i].origin[1] + ow.suspension_limit,
            geoms[i].origin[2],
        ];
        wc.steered = !ow.rear && ow.steering_limit > 0.0;
        wc.steer_scale = 1.0;
        wc.driven = driven.contains(&i);
        wc.handbrake = ow.rear && ow.handbrake_coef > 0.0;
        wc.suspension = Some(SuspensionConfig {
            spring_rate: k.ks,
            damping_compression: k.cs,
            damping_rebound: k.cs,
            travel: ow.suspension_extent + ow.suspension_limit,
            force_apply_offset: 0.0,
            max_force: ow.static_load * 10.0,
        });
        wc.tires = Some(TireConfig {
            lateral_grip: mu(ow),
            longitudinal_grip: mu(ow),
            peak_slip_angle: ow.optimum_slip.atan(),
            peak_slip_ratio: ow.optimum_slip,
            slide_fraction: (ow.sliding_fric / ow.static_fric.max(0.01)).clamp(0.0, 1.0),
            rolling_resistance: 0.0,
            load_sensitivity: 0.0,
        });
    }
    let brake_force = wheels
        .iter()
        .map(|w| w.brake_coef * w.static_fric * w.static_load)
        .sum::<f32>()
        / 4.0;
    config.brakes.max_brake_force = brake_force;
    config.steering = SteeringConfig {
        low_speed_max_angle: front_lock,
        high_speed_max_angle: front_lock,
        high_speed: 40.0,
        input_rate: config.steering.input_rate,
        return_rate: config.steering.return_rate,
        response_curve: 1.0,
        grip_limit: 0.0,
    };
    config.aero = AeroConfig {
        drag_coefficient: aero.drag * 2.0,
        downforce_coefficient: aero.down * 2.0,
    };
    config.transmission = TransmissionConfig {
        gear_ratios: gearbox.ratios[original::FIRST_GEAR..].to_vec(),
        reverse_ratio: gearbox.ratios[original::REVERSE_GEAR].abs(),
        final_drive: 1.0,
        shift_time: engine.gear_change_lag,
        efficiency: 1.0,
        upshift_rpm: gearbox.upshift_rpm.get(original::FIRST_GEAR).copied(),
        downshift_rpm: gearbox
            .downshift_full_rpm
            .get(original::FIRST_GEAR + 1)
            .copied(),
    };
    config.engine.redline_rpm = engine.max_rpm;
    config.engine.idle_rpm = engine.idle_rpm;
    config.engine.engine_brake_nm = 0.75 * constants.torque_opt;
    // None of the arcade assists exist in the original: the tires, the
    // gyro and the aero damping are the whole of its handling. Only optional self-righting remains a modern recovery policy.
    config.assists = AssistConfig {
        roll_resistance: 0.0,
        pitch_resistance: 0.0,
        yaw_stability: 0.0,
        traction_control: 0.0,
        countersteer: 0.0,
        slide_recovery: 0.0,
        air_control: 0.0,
        ..config.assists
    };
    report.adapted(
        "-",
        "assists",
        "original model: no traction control, yaw damper, slide recovery, countersteer or roll/pitch assist; air levelling disabled; optional self-righting remains",
    );

    report.imported(
        "vehCarSim.BoundFriction/BoundElasticity",
        "original.bound_friction/bound_elasticity",
        "raw hull materials; paired friction and elasticity multiply (research 01 contact solver)",
    );
    config.original = Some(OriginalHandling {
        bound_friction: sim.bound_friction.unwrap_or(0.3),
        bound_elasticity: sim.bound_elasticity.unwrap_or(0.2),
        gravity,
        surface_friction: ORIGINAL_ROAD_FRICTION,
        wheels,
        driven,
        free,
        axles: [
            ([fl, fr], sim.axle_front.as_ref()),
            ([rl, rr], sim.axle_back.as_ref()),
        ]
        .into_iter()
        .map(|(wheels, tuning)| OriginalAxle {
            wheels,
            torque_coef: tuning.map_or(0.0, |a| a.torque_coef),
            damp_coef: tuning.map_or(0.0, |a| a.damp_coef),
        })
        .collect(),
        drivetrain,
        freetrain,
        engine,
        gearbox,
        aero,
        max_angular_speed: ORIGINAL_MAX_ANGULAR_SPEED,
    });
}

/// Read human steering separately from wheel/AI steering. Missing fields
/// use the recovered mmPlayer constructor values; absent mouse tuning is
/// explicit because its constructor defaults have not been transcribed.
fn player_steering(asnode: Option<&AsNode>) -> mm2_vehicle::player_input::PlayerSteeringConfig {
    use mm2_vehicle::player_input::PlayerSteeringConfig;
    let mut out = PlayerSteeringConfig::default();
    let Some(asnode) = asnode else {
        return out;
    };
    let root = &asnode.raw.root;
    let value = |name: &str| root.f32(name).filter(|v| v.is_finite());
    let pair = |lo: &str, hi: &str, default: [f32; 2]| {
        [
            value(lo).filter(|v| *v >= 0.0).unwrap_or(default[0]),
            value(hi).filter(|v| *v >= 0.0).unwrap_or(default[1]),
        ]
    };
    out.speed_sensitive = value("SpeedSensitive")
        .filter(|v| matches!(*v, 0.0 | 1.0 | 2.0))
        .map_or(2, |v| v as u8);
    let low = asnode.speed_base_low.unwrap_or(out.speed_low);
    let high = asnode.speed_base_hi.unwrap_or(out.speed_high);
    if low >= 0.0 && high > low {
        out.speed_low = low;
        out.speed_high = high;
    }
    out.delta_out = pair(
        "DiscreteSteeringDeltaOutLo",
        "DiscreteSteeringDeltaOutHi",
        out.delta_out,
    );
    out.delta_in = pair(
        "DiscreteSteeringDeltaInLo",
        "DiscreteSteeringDeltaInHi",
        out.delta_in,
    );
    out.exponent = pair(
        "DiscreteSteeringFilterLo",
        "DiscreteSteeringFilterHi",
        out.exponent,
    );
    let optional_pair = |lo: &str, hi: &str| -> Option<[f32; 2]> {
        Some([
            value(lo).filter(|v| *v > 0.0)?,
            value(hi).filter(|v| *v > 0.0)?,
        ])
    };
    out.mouse_divisor = optional_pair("MouseSensitivityLow", "MouseSensitivityHi");
    out.mouse_exponent = optional_pair("MouseSteerFilterLow", "MouseSteerFilterHi");
    out
}

/// A `vehgyro` scalar that must be nonnegative: malformed values warn
/// and clamp to 0 rather than sink the car (F05-B.4).
fn gyro_rate(name: &str, v: f32, bad: &mut Vec<String>) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        bad.push(format!("vehgyro.{name} {v} out of range — clamped to 0"));
        0.0
    }
}

/// An optional `vehgyro` axis rate: the authored sign is kept (the
/// decoder allows it) but a non-finite value warns and drops.
fn gyro_axis(name: &str, v: Option<f32>, bad: &mut Vec<String>) -> Option<f32> {
    match v {
        Some(v) if v.is_finite() => Some(v),
        Some(v) => {
            bad.push(format!("vehgyro.{name} {v} non-finite — dropped"));
            None
        }
        None => None,
    }
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
    use mm2_formats::tune::TuneFile;

    /// A stock-shaped `vehCarSim` record carrying `cog` as its
    /// `CenterOfGravity`.
    fn sim_with_cog(cog: [f32; 3]) -> VehCarSim {
        let wheel = "SuspensionExtent 0.15\nSuspensionLimit 0.05\nSuspensionFactor 1.0\n\
            SuspensionDampCoef 0.1\nSteeringLimit 0.4\nSteeringOffset 0.25\nBrakeCoef 0.6\n\
            HandbrakeCoef 2.0\nTireDispLimitLong 0.125\nTireDampCoefLong 0.25\n\
            TireDragCoefLong 0.02\nTireDispLimitLat 0.125\nTireDampCoefLat 0.25\n\
            TireDragCoefLat 0.05\nOptimumSlipPercent 0.16\n\
            StaticFric 3.0\nSlidingFric 2.7\n";
        let text = format!(
            "vehCarSim {{\nMass 1000.0\nInertiaBox 2.0 2.0 3.0\n\
             CenterOfGravity {} {} {}\nDrivetrainType 2\n\
             Aero {{\nDrag 0.5\nDown 0.0\n}}\n\
             Engine {{\nMaxHorsePower 260.0\nIdleRPM 750.0\nOptRPM 5800.0\nMaxRPM 8500.0\n}}\n\
             Trans {{\nAutoNumGears 6\nReverse 30.0\nLow 20.0\nHigh 90.0\nGearBias 0.5\n\
             GearChangeTime 0.8\n}}\n\
             WheelFront {{\n{wheel}}}\nWheelBack {{\n{wheel}}}\n}}\n",
            cog[0], cog[1], cog[2],
        );
        VehCarSim::from_tune(&TuneFile::parse(&text).unwrap()).unwrap()
    }

    /// Four wheels of radius 0.35 at `z = ±half_wheelbase`, origin on the
    /// road like a stock `whlN.mtx`.
    fn wheels(half_wheelbase: f32) -> Vec<WheelGeom> {
        [(-0.75, -1.0), (0.75, -1.0), (-0.75, 1.0), (0.75, 1.0)]
            .iter()
            .enumerate()
            .map(|(index, (x, side))| WheelGeom {
                index,
                origin: [*x, 0.35, side * half_wheelbase],
                radius: 0.35,
                width: 0.2,
            })
            .collect()
    }

    fn convert_with(sim: &VehCarSim, wheels: &[WheelGeom], body_top: f32) -> Converted {
        convert(&ConvertInput {
            id: "vptest",
            display_name: "Test",
            sim,
            asnode: None,
            wheels,
            bound: None,
            body_aabb: ([-0.9, 0.1, -2.0], [0.9, body_top, 2.0]),
            stuck: None,
            gyro: None,
        })
        .unwrap()
    }

    #[test]
    fn center_of_gravity_z_points_the_other_way_from_the_model() {
        // The original's static load split (`vehWheel::ComputeConstants`)
        // gives each wheel `|z - CenterOfGravity.z| / 2|z|` of the weight,
        // so a positive offset loads the *front* (-z) axle: the centre of
        // mass sits at -CenterOfGravity.z. The Moon Rover's +0.4 read the
        // other way put its mass behind its rear axle.
        let sim = sim_with_cog([0.0, -0.1, 0.4]);
        let cfg = convert_with(&sim, &wheels(1.3), 1.5).config;
        assert!((cfg.center_of_mass[2] - -0.4).abs() < 1e-6);
        let rate = |i: usize| cfg.wheels[i].suspension.as_ref().unwrap().spring_rate;
        assert!(
            rate(0) > rate(2),
            "front springs should carry the forward mass: {} against {}",
            rate(0),
            rate(2)
        );
    }

    #[test]
    fn original_cars_keep_authored_pitch_levers() {
        // Short wheelbases and rearward mass do not trigger the old artificial
        // pitch cancellation: original suspension and tyre forces carry them.
        for (cog, half_wheelbase) in [([0.0, -0.1, 0.0], 0.43), ([0.0, -0.1, -1.0], 1.3)] {
            let converted = convert_with(&sim_with_cog(cog), &wheels(half_wheelbase), 1.5);
            assert!(converted.config.original.is_some());
            assert_eq!(converted.config.assists.pitch_resistance, 0.0);
            assert_eq!(converted.config.assists.roll_resistance, 0.0);
            assert_eq!(converted.config.center_of_mass, cog.map(|v| -v));
            assert!(
                !converted
                    .report
                    .entries
                    .iter()
                    .any(|e| e.dest == "assists.pitch_resistance")
            );
        }
    }

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
    fn clear_underside_opens_up_a_slammed_floor() {
        // A body pan 5 cm off the road, overhanging both axles — the shape
        // every stock MM2 bound has.
        let (front_z, rear_z) = (-1.2, 1.4);
        let mut pts = vec![
            [-0.9, 0.05, -2.0],
            [0.9, 0.05, -2.0],
            [-0.9, 0.05, 2.0],
            [0.9, 0.05, 2.0],
            [-0.9, 1.4, -1.0],
            [0.9, 1.4, -1.0],
            [-0.9, 1.4, 1.0],
            [0.9, 1.4, 1.0],
        ];
        let roof: Vec<[f32; 3]> = pts[4..].to_vec();
        clear_underside(&mut pts, 0.0, front_z, rear_z);

        // The roof is left alone; only the floor comes up.
        assert_eq!(&pts[4..], &roof[..]);
        for p in &pts[..4] {
            let overhang = (front_z - p[2]).max(p[2] - rear_z);
            let wanted = (overhang * APPROACH_ANGLE.tan()).max(MIN_GROUND_CLEARANCE);
            assert!(
                p[1] >= wanted - 1e-4,
                "vertex at z={} lifted to {}, wanted {wanted}",
                p[2],
                p[1]
            );
        }
    }

    #[test]
    fn clear_underside_leaves_a_very_low_body_a_hull_to_be() {
        // A pancake thinner than the clearance we would like: raising its
        // floor to the target would push it through its own roof.
        let mut pts = vec![
            [-0.9, 0.02, -1.0],
            [0.9, 0.02, -1.0],
            [-0.9, 0.02, 1.0],
            [0.9, 0.02, 1.0],
            [-0.9, 0.10, 0.0],
            [0.9, 0.10, 0.0],
        ];
        clear_underside(&mut pts, 0.0, -1.0, 1.0);
        let top = pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        let bottom = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(
            bottom < top,
            "hull collapsed to a plane: bottom {bottom}, top {top}"
        );
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
    #[test]
    fn legacy_summary_overrides_reach_the_native_model() {
        let sim = sim_with_cog([0.0, -0.1, 0.0]);
        let imported = convert_with(&sim, &wheels(1.3), 1.5).config;
        let source = imported.original.as_ref().unwrap();
        let mut over = imported.clone();
        over.engine.peak_torque_nm *= 0.5;
        over.engine.max_power_w = over.engine.max_power_w.map(|p| p * 0.5);
        over.mass *= 2.0;
        for wheel in &mut over.wheels {
            wheel.tires.as_mut().unwrap().longitudinal_grip *= 0.5;
        }
        let result = crate::assemble::apply_handling_override(&imported, over).unwrap();
        let native = result.original.as_ref().unwrap();
        assert_eq!(native.engine.max_power_w, source.engine.max_power_w * 0.5);
        for (now, before) in native.wheels.iter().zip(&source.wheels) {
            assert_eq!(now.static_load, before.static_load * 2.0);
            assert_eq!(now.static_fric, before.static_fric * 0.5);
            assert_eq!(now.sliding_fric, before.sliding_fric * 0.5);
        }
        assert_eq!(result.wheels[0].position, imported.wheels[0].position);
        assert_eq!(
            result.inertia,
            imported.inertia.map(|axes| axes.map(|v| v * 2.0))
        );
        let mut explicit = imported.clone();
        explicit.engine.max_power_w = explicit.engine.max_power_w.map(|p| p * 0.5);
        explicit.original.as_mut().unwrap().engine.max_power_w *= 0.75;
        let result = crate::assemble::apply_handling_override(&imported, explicit).unwrap();
        assert_eq!(
            result.original.unwrap().engine.max_power_w,
            source.engine.max_power_w * 0.75
        );
    }

    #[test]
    fn native_hull_materials_preserve_authored_values_and_validate() {
        let mut sim = sim_with_cog([0.0; 3]);
        sim.bound_friction = Some(0.9);
        sim.bound_elasticity = Some(0.5);
        let mut cfg = convert_with(&sim, &wheels(1.3), 1.5).config;
        let original = cfg.original.as_ref().unwrap();
        assert_eq!(original.bound_friction, 0.9);
        assert_eq!(original.bound_elasticity, 0.5);
        for bad in [f32::NAN, f32::INFINITY, -0.1] {
            cfg.original.as_mut().unwrap().bound_friction = bad;
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .iter()
                    .any(|e| e.contains("original.bound_friction"))
            );
            cfg.original.as_mut().unwrap().bound_friction = 0.9;
            cfg.original.as_mut().unwrap().bound_elasticity = bad;
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .iter()
                    .any(|e| e.contains("original.bound_elasticity"))
            );
            cfg.original.as_mut().unwrap().bound_elasticity = 0.5;
        }
    }

    #[test]
    fn native_configuration_rejects_bad_indices_arrays_and_numbers() {
        let sim = sim_with_cog([0.0, -0.1, 0.0]);
        let baseline = convert_with(&sim, &wheels(1.3), 1.5).config;
        baseline.validate().unwrap();
        for (wheels, torque_coef, damp_coef, field) in [
            ([0, 4], 1.0, 0.5, "wheels"),
            ([0, 0], 1.0, 0.5, "wheels"),
            ([0, 1], -1.0, 0.5, "torque_coef"),
            ([0, 1], 1.0, f32::NAN, "damp_coef"),
        ] {
            let mut config = baseline.clone();
            config.original.as_mut().unwrap().axles = vec![OriginalAxle {
                wheels,
                torque_coef,
                damp_coef,
            }];
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .iter()
                    .any(|p| p.contains(&format!("original.axles[0].{field}")))
            );
        }
        let mut config = baseline.clone();
        config.original.as_mut().unwrap().driven.push(99);
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|p| p.contains("driven/free"))
        );
        let mut config = baseline.clone();
        config
            .original
            .as_mut()
            .unwrap()
            .gearbox
            .upshift_rpm
            .clear();
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|p| p.contains("upshift_rpm"))
        );
        let mut config = baseline;
        config.original.as_mut().unwrap().wheels[0].static_load = f32::NAN;
        assert!(
            config
                .validate()
                .unwrap_err()
                .iter()
                .any(|p| p.contains("static_load"))
        );
    }
}
