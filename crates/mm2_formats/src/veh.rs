//! Typed views over the block-structured vehicle tuning files
//! (`vehCarSim`, `vehTrailer`, `asNode`, `aiVehicleData`) parsed by
//! [`crate::tune`].
//!
//! These types decode the fields the runtime conversion needs, keep raw
//! values for diagnostics, and report unknown fields so authored data is
//! never silently dropped.

use crate::tune::{TuneBlock, TuneFile, TuneValue};
use std::fmt;

/// Which axle(s) the engine drives (`DrivetrainType` in vehCarSim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrivetrainType {
    /// `DrivetrainType 0` — rear wheels driven.
    Rwd,
    /// `DrivetrainType 1` — front wheels driven.
    Fwd,
    /// `DrivetrainType 2` — all wheels driven.
    Awd,
}

impl DrivetrainType {
    pub fn from_i32(v: i64) -> Option<Self> {
        match v {
            0 => Some(Self::Rwd),
            1 => Some(Self::Fwd),
            2 => Some(Self::Awd),
            _ => None,
        }
    }
}

/// Error while decoding a typed tuning structure.
#[derive(Debug)]
pub struct VehError {
    /// Class/block name context, e.g. `vehCarSim.Engine`.
    pub context: String,
    pub message: String,
}

impl fmt::Display for VehError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

impl std::error::Error for VehError {}

type VehResult<T> = Result<T, VehError>;

fn err<T>(ctx: &str, msg: impl Into<String>) -> VehResult<T> {
    Err(VehError {
        context: ctx.into(),
        message: msg.into(),
    })
}

fn req_f32(b: &TuneBlock, ctx: &str, name: &str) -> VehResult<f32> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) => Ok(n as f32),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => err(ctx, format!("missing required field {name:?}")),
    }
}

fn req_vec3(b: &TuneBlock, ctx: &str, name: &str) -> VehResult<[f32; 3]> {
    match b.vec3(name) {
        Some(v) => Ok(v),
        None => err(ctx, format!("missing or malformed vec3 field {name:?}")),
    }
}

fn req_block<'a>(b: &'a TuneBlock, ctx: &str, name: &str) -> VehResult<&'a TuneBlock> {
    b.block(name).ok_or_else(|| VehError {
        context: ctx.into(),
        message: format!("missing required block {name:?}"),
    })
}

/// Collect field names in `block` not present in `known`, as warnings.
fn unknown_fields(b: &TuneBlock, ctx: &str, known: &[&str], warnings: &mut Vec<String>) {
    for name in b.field_names() {
        if !known.contains(&name) {
            warnings.push(format!("{ctx}: unrecognised field {name:?}"));
        }
    }
    for name in b.block_names() {
        if !known.contains(&name) {
            warnings.push(format!("{ctx}: unrecognised block {name:?}"));
        }
    }
}

/// `Engine { ... }` tuning.
#[derive(Debug, Clone)]
pub struct VehEngine {
    /// `MaxHorsePower` — rated power in mechanical horsepower.
    pub max_horsepower: f32,
    /// `IdleRPM`.
    pub idle_rpm: f32,
    /// `OptRPM` — peak-power RPM (top of the original power curve).
    pub opt_rpm: f32,
    /// `MaxRPM` — redline.
    pub max_rpm: f32,
    /// `GCL` — gear-change lift / rev-drop factor, when present.
    pub gcl: Option<f32>,
    /// `AngInertia` — engine rotating inertia (unmapped, retained).
    pub ang_inertia: Option<f32>,
}

/// `Trans { ... }` tuning.
///
/// `low`, `high` and `reverse` are top-speed values in mph per gear band:
/// `low` is the speed reached in first gear, `high` the top speed in top
/// gear, `reverse` the reverse top speed. Ratios are derived from these by
/// the conversion layer.
#[derive(Debug, Clone)]
pub struct VehTrans {
    pub manual_num_gears: u32,
    pub auto_num_gears: u32,
    /// Reverse top speed in mph.
    pub reverse_mph: f32,
    /// First-gear top speed in mph.
    pub low_mph: f32,
    /// Top-gear top speed in mph.
    pub high_mph: f32,
    /// Log/linear interpolation bias between `low` and `high` (0..1).
    pub gear_bias: f32,
    pub upshift_bias: f32,
    pub downshift_bias_min: f32,
    pub downshift_bias_max: f32,
    /// Shift duration in seconds.
    pub gear_change_time: f32,
}

/// `WheelFront`/`WheelBack` tuning.
#[derive(Debug, Clone)]
pub struct VehWheel {
    /// `SuspensionExtent` — suspension travel in metres.
    pub suspension_extent: f32,
    /// `SuspensionLimit` — bound-stop length in metres.
    pub suspension_limit: f32,
    /// `SuspensionFactor` — relative spring stiffness factor.
    pub suspension_factor: f32,
    /// `SuspensionDampCoef` — damping coefficient.
    pub suspension_damp_coef: f32,
    /// `SteeringLimit` — maximum steer angle in radians.
    pub steering_limit: f32,
    /// `SteeringOffset` — throttle-dependent steer reduction.
    pub steering_offset: f32,
    /// `BrakeCoef` — fraction of the brake budget on this axle.
    pub brake_coef: f32,
    /// `HandbrakeCoef` — handbrake force multiplier on this axle.
    pub handbrake_coef: f32,
    /// `CamberLimit` (unmapped, retained).
    pub camber_limit: Option<f32>,
    /// `WobbleLimit` (unmapped, retained).
    pub wobble_limit: Option<f32>,
    /// `TireDispLimitLong` — longitudinal slip-displacement limit.
    pub tire_disp_limit_long: f32,
    /// `TireDampCoefLong` — longitudinal slip damping.
    pub tire_damp_coef_long: f32,
    /// `TireDragCoefLong` — longitudinal rolling drag coefficient.
    pub tire_drag_coef_long: f32,
    /// `TireDispLimitLat` — lateral slip-displacement limit.
    pub tire_disp_limit_lat: f32,
    /// `TireDampCoefLat` — lateral slip damping.
    pub tire_damp_coef_lat: f32,
    /// `TireDragCoefLat` — lateral drag coefficient.
    pub tire_drag_coef_lat: f32,
    /// `OptimumSlipPercent` — peak-grip slip fraction.
    pub optimum_slip_percent: f32,
    /// `StaticFric` — static grip coefficient.
    pub static_fric: f32,
    /// `SlidingFric` — sliding grip coefficient.
    pub sliding_fric: f32,
}

/// `Aero { ... }` tuning.
#[derive(Debug, Clone)]
pub struct VehAero {
    /// `Drag` — drag coefficient (dimensionless; combined with a reference
    /// area by the conversion layer).
    pub drag: f32,
    /// `Down` — downforce coefficient.
    pub down: f32,
    /// `AngCDamp` (unmapped, retained).
    pub ang_c_damp: Option<[f32; 3]>,
    /// `AngVelDamp` (unmapped, retained).
    pub ang_vel_damp: Option<[f32; 3]>,
    /// `AngVel2Damp` (unmapped, retained).
    pub ang_vel2_damp: Option<[f32; 3]>,
}

/// `Drivetrain`/`Freetrain` end-train tuning (mostly inertia/brake loss).
#[derive(Debug, Clone)]
pub struct VehEndTrain {
    pub ang_inertia: Option<f32>,
    pub brake_dynamic_coef: Option<f32>,
    pub brake_static_coef: Option<f32>,
}

/// `AxleFront`/`AxleBack` tuning.
#[derive(Debug, Clone)]
pub struct VehAxle {
    /// `TorqueCoef` — torque split fraction to this axle when both axles
    /// are driven (only meaningful for 4WD).
    pub torque_coef: f32,
    /// `DampCoef` — differential damping.
    pub damp_coef: f32,
}

/// Fully decoded `vehCarSim` tuning.
#[derive(Debug, Clone)]
pub struct VehCarSim {
    pub mass: f32,
    /// `InertiaBox` — box dimensions (m) used for the inertia tensor.
    pub inertia_box: [f32; 3],
    /// `CenterOfGravity` — centre-of-mass offset from the model origin.
    pub center_of_gravity: [f32; 3],
    /// `BoundFriction` — chassis collider friction.
    pub bound_friction: Option<f32>,
    /// `BoundElasticity` — chassis collider restitution.
    pub bound_elasticity: Option<f32>,
    /// `DrivetrainType` (0 RWD / 1 FWD / 2 4WD).
    pub drivetrain_type: DrivetrainType,
    /// `SSSValue` — speed-sensitive steering factor (unmapped, retained).
    pub sss_value: Option<f32>,
    /// `SSSThreshold` (unmapped, retained).
    pub sss_threshold: Option<f32>,
    /// `CarFrictionHandling` (unmapped, retained).
    pub car_friction_handling: Option<f32>,
    pub aero: VehAero,
    pub engine: VehEngine,
    pub trans: VehTrans,
    pub drivetrain: Option<VehEndTrain>,
    pub freetrain: Option<VehEndTrain>,
    pub wheel_front: VehWheel,
    pub wheel_back: VehWheel,
    pub axle_front: Option<VehAxle>,
    pub axle_back: Option<VehAxle>,
    /// Unknown/unmapped fields encountered while decoding.
    pub warnings: Vec<String>,
}

fn decode_wheel(b: &TuneBlock, ctx: &str, warnings: &mut Vec<String>) -> VehResult<VehWheel> {
    let w = VehWheel {
        suspension_extent: req_f32(b, ctx, "SuspensionExtent")?,
        suspension_limit: req_f32(b, ctx, "SuspensionLimit")?,
        suspension_factor: req_f32(b, ctx, "SuspensionFactor")?,
        suspension_damp_coef: req_f32(b, ctx, "SuspensionDampCoef")?,
        steering_limit: req_f32(b, ctx, "SteeringLimit")?,
        steering_offset: b.f32("SteeringOffset").unwrap_or(0.0),
        brake_coef: req_f32(b, ctx, "BrakeCoef")?,
        handbrake_coef: b.f32("HandbrakeCoef").unwrap_or(0.0),
        camber_limit: b.f32("CamberLimit"),
        wobble_limit: b.f32("WobbleLimit"),
        tire_disp_limit_long: req_f32(b, ctx, "TireDispLimitLong")?,
        tire_damp_coef_long: req_f32(b, ctx, "TireDampCoefLong")?,
        tire_drag_coef_long: req_f32(b, ctx, "TireDragCoefLong")?,
        tire_disp_limit_lat: req_f32(b, ctx, "TireDispLimitLat")?,
        tire_damp_coef_lat: req_f32(b, ctx, "TireDampCoefLat")?,
        tire_drag_coef_lat: req_f32(b, ctx, "TireDragCoefLat")?,
        optimum_slip_percent: req_f32(b, ctx, "OptimumSlipPercent")?,
        static_fric: req_f32(b, ctx, "StaticFric")?,
        sliding_fric: req_f32(b, ctx, "SlidingFric")?,
    };
    unknown_fields(
        b,
        ctx,
        &[
            "SuspensionExtent",
            "SuspensionLimit",
            "SuspensionFactor",
            "SuspensionDampCoef",
            "SteeringLimit",
            "SteeringOffset",
            "BrakeCoef",
            "HandbrakeCoef",
            "CamberLimit",
            "WobbleLimit",
            "TireDispLimitLong",
            "TireDampCoefLong",
            "TireDragCoefLong",
            "TireDispLimitLat",
            "TireDampCoefLat",
            "TireDragCoefLat",
            "OptimumSlipPercent",
            "StaticFric",
            "SlidingFric",
        ],
        warnings,
    );
    Ok(w)
}

fn decode_end_train(b: &TuneBlock, ctx: &str, warnings: &mut Vec<String>) -> VehEndTrain {
    unknown_fields(
        b,
        ctx,
        &["AngInertia", "BrakeDynamicCoef", "BrakeStaticCoef"],
        warnings,
    );
    VehEndTrain {
        ang_inertia: b.f32("AngInertia"),
        brake_dynamic_coef: b.f32("BrakeDynamicCoef"),
        brake_static_coef: b.f32("BrakeStaticCoef"),
    }
}

fn decode_axle(b: &TuneBlock, ctx: &str, warnings: &mut Vec<String>) -> VehAxle {
    unknown_fields(b, ctx, &["TorqueCoef", "DampCoef"], warnings);
    VehAxle {
        torque_coef: b.f32("TorqueCoef").unwrap_or(0.0),
        damp_coef: b.f32("DampCoef").unwrap_or(0.0),
    }
}

impl VehCarSim {
    /// Decode the root `vehCarSim` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "vehCarSim" {
            return err(
                "vehCarSim",
                format!("expected vehCarSim root block, found {:?}", file.root.name),
            );
        }
        let root = &file.root;
        let ctx = "vehCarSim";
        let mut warnings = Vec::new();

        let mass = req_f32(root, ctx, "Mass")?;
        let inertia_box = req_vec3(root, ctx, "InertiaBox")?;
        let center_of_gravity = root.vec3("CenterOfGravity").unwrap_or_else(|| {
            warnings.push("vehCarSim: CenterOfGravity missing, using [0, -0.1, 0]".into());
            [0.0, -0.1, 0.0]
        });
        let drivetrain_type = match root.field("DrivetrainType") {
            Some(f) => match f.values.first().and_then(|v| v.number) {
                Some(n) => match DrivetrainType::from_i32(n as i64) {
                    Some(d) => d,
                    None => {
                        warnings.push(format!(
                            "vehCarSim: unknown DrivetrainType {n}, assuming RWD"
                        ));
                        DrivetrainType::Rwd
                    }
                },
                None => return err(ctx, "DrivetrainType has no numeric value"),
            },
            None => {
                warnings.push("vehCarSim: DrivetrainType missing, assuming RWD".into());
                DrivetrainType::Rwd
            }
        };

        let aero = match root.block("Aero") {
            Some(b) => {
                unknown_fields(
                    b,
                    "vehCarSim.Aero",
                    &["AngCDamp", "AngVelDamp", "AngVel2Damp", "Drag", "Down"],
                    &mut warnings,
                );
                VehAero {
                    drag: b.f32("Drag").unwrap_or(0.0),
                    down: b.f32("Down").unwrap_or(0.0),
                    ang_c_damp: b.vec3("AngCDamp"),
                    ang_vel_damp: b.vec3("AngVelDamp"),
                    ang_vel2_damp: b.vec3("AngVel2Damp"),
                }
            }
            None => {
                warnings.push("vehCarSim: Aero block missing, zero drag assumed".into());
                VehAero {
                    drag: 0.0,
                    down: 0.0,
                    ang_c_damp: None,
                    ang_vel_damp: None,
                    ang_vel2_damp: None,
                }
            }
        };

        let engine_b = req_block(root, ctx, "Engine")?;
        unknown_fields(
            engine_b,
            "vehCarSim.Engine",
            &[
                "AngInertia",
                "MaxHorsePower",
                "IdleRPM",
                "OptRPM",
                "MaxRPM",
                "GCL",
            ],
            &mut warnings,
        );
        let engine = VehEngine {
            max_horsepower: req_f32(engine_b, "vehCarSim.Engine", "MaxHorsePower")?,
            idle_rpm: req_f32(engine_b, "vehCarSim.Engine", "IdleRPM")?,
            opt_rpm: req_f32(engine_b, "vehCarSim.Engine", "OptRPM")?,
            max_rpm: req_f32(engine_b, "vehCarSim.Engine", "MaxRPM")?,
            gcl: engine_b.f32("GCL"),
            ang_inertia: engine_b.f32("AngInertia"),
        };

        let trans_b = req_block(root, ctx, "Trans")?;
        unknown_fields(
            trans_b,
            "vehCarSim.Trans",
            &[
                "ManualNumGears",
                "AutoNumGears",
                "Reverse",
                "Low",
                "High",
                "GearBias",
                "UpshiftBias",
                "DownshiftBiasMin",
                "DownshiftBiasMax",
                "GearChangeTime",
            ],
            &mut warnings,
        );
        let trans = VehTrans {
            manual_num_gears: trans_b.f32("ManualNumGears").unwrap_or(0.0).max(0.0) as u32,
            auto_num_gears: req_f32(trans_b, "vehCarSim.Trans", "AutoNumGears")?.max(1.0) as u32,
            reverse_mph: req_f32(trans_b, "vehCarSim.Trans", "Reverse")?,
            low_mph: req_f32(trans_b, "vehCarSim.Trans", "Low")?,
            high_mph: req_f32(trans_b, "vehCarSim.Trans", "High")?,
            gear_bias: trans_b.f32("GearBias").unwrap_or(0.5),
            upshift_bias: trans_b.f32("UpshiftBias").unwrap_or(0.05),
            downshift_bias_min: trans_b.f32("DownshiftBiasMin").unwrap_or(0.05),
            downshift_bias_max: trans_b.f32("DownshiftBiasMax").unwrap_or(0.3),
            gear_change_time: trans_b.f32("GearChangeTime").unwrap_or(0.8),
        };

        let wheel_front = decode_wheel(
            req_block(root, ctx, "WheelFront")?,
            "vehCarSim.WheelFront",
            &mut warnings,
        )?;
        let wheel_back = decode_wheel(
            req_block(root, ctx, "WheelBack")?,
            "vehCarSim.WheelBack",
            &mut warnings,
        )?;

        let drivetrain = root
            .block("Drivetrain")
            .map(|b| decode_end_train(b, "vehCarSim.Drivetrain", &mut warnings));
        let freetrain = root
            .block("Freetrain")
            .map(|b| decode_end_train(b, "vehCarSim.Freetrain", &mut warnings));
        let axle_front = root
            .block("AxleFront")
            .map(|b| decode_axle(b, "vehCarSim.AxleFront", &mut warnings));
        let axle_back = root
            .block("AxleBack")
            .map(|b| decode_axle(b, "vehCarSim.AxleBack", &mut warnings));

        unknown_fields(
            root,
            ctx,
            &[
                "Mass",
                "InertiaBox",
                "CenterOfGravity",
                "BoundFriction",
                "BoundElasticity",
                "DrivetrainType",
                "SSSValue",
                "SSSThreshold",
                "CarFrictionHandling",
                "Aero",
                "Engine",
                "Trans",
                "Drivetrain",
                "Freetrain",
                "WheelFront",
                "WheelBack",
                "AxleFront",
                "AxleBack",
            ],
            &mut warnings,
        );

        Ok(VehCarSim {
            mass,
            inertia_box,
            center_of_gravity,
            bound_friction: root.f32("BoundFriction"),
            bound_elasticity: root.f32("BoundElasticity"),
            drivetrain_type,
            sss_value: root.f32("SSSValue"),
            sss_threshold: root.f32("SSSThreshold"),
            car_friction_handling: root.f32("CarFrictionHandling"),
            aero,
            engine,
            trans,
            drivetrain,
            freetrain,
            wheel_front,
            wheel_back,
            axle_front,
            axle_back,
            warnings,
        })
    }
}

/// Decoded `vehTrailer` tuning (trailers hauled by vpsemi/vpcentury).
#[derive(Debug, Clone)]
pub struct VehTrailer {
    pub mass: f32,
    pub inertia_box: [f32; 3],
    /// `CarHitchOffset` — hitch attach point on the towing vehicle (car
    /// space), when authored. `vpcentury` omits it; the conversion layer
    /// derives a fallback.
    pub car_hitch_offset: Option<[f32; 3]>,
    /// `TrailerHitchOffset` — hitch attach point on the trailer (trailer
    /// space), when authored.
    pub trailer_hitch_offset: Option<[f32; 3]>,
    /// `CenterOfGravity` when authored.
    pub center_of_gravity: Option<[f32; 3]>,
    pub wheel_front: VehWheel,
    pub wheel_back: VehWheel,
    pub warnings: Vec<String>,
}

impl VehTrailer {
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "vehTrailer" {
            return err(
                "vehTrailer",
                format!("expected vehTrailer root block, found {:?}", file.root.name),
            );
        }
        let root = &file.root;
        let ctx = "vehTrailer";
        let mut warnings = Vec::new();
        let inertia_box = root.vec3("InertiaBox").unwrap_or([1.0, 1.0, 1.0]);
        let car_hitch_offset = root.vec3("CarHitchOffset");
        let trailer_hitch_offset = root.vec3("TrailerHitchOffset");
        if car_hitch_offset.is_none() {
            warnings.push(
                "vehTrailer: CarHitchOffset missing; conversion must derive a fallback".into(),
            );
        }
        if trailer_hitch_offset.is_none() {
            warnings.push(
                "vehTrailer: TrailerHitchOffset missing; conversion must derive a fallback".into(),
            );
        }
        let t = VehTrailer {
            mass: req_f32(root, ctx, "Mass")?,
            inertia_box,
            car_hitch_offset,
            trailer_hitch_offset,
            center_of_gravity: root.vec3("CenterOfGravity"),
            wheel_front: decode_wheel(
                req_block(root, ctx, "WheelFront")?,
                "vehTrailer.WheelFront",
                &mut warnings,
            )?,
            wheel_back: decode_wheel(
                req_block(root, ctx, "WheelBack")?,
                "vehTrailer.WheelBack",
                &mut warnings,
            )?,
            warnings,
        };
        unknown_fields(
            root,
            ctx,
            &[
                "Mass",
                "InertiaBox",
                "CarHitchOffset",
                "TrailerHitchOffset",
                "WheelFront",
                "WheelBack",
                "Drivetrain",
                "CenterOfGravity",
            ],
            &mut Vec::new(), // trailer extras are informational only
        );
        Ok(t)
    }
}

/// Light accessor for `asNode` steering-assist files.
///
/// The stock files carry `SpeedBaseLow`/`SpeedBaseHi` (m/s) thresholds plus
/// sensitivity/filter values; the conversion layer only uses the speed
/// thresholds and leaves the rest unmapped.
#[derive(Debug, Clone)]
pub struct AsNode {
    pub speed_base_low: Option<f32>,
    pub speed_base_hi: Option<f32>,
    pub raw: TuneFile,
}

impl AsNode {
    pub fn from_tune(file: TuneFile) -> Self {
        let root = &file.root;
        AsNode {
            speed_base_low: root.f32("SpeedBaseLow"),
            speed_base_hi: root.f32("SpeedBaseHi"),
            raw: file,
        }
    }
}

/// MSVC's non-finite float serialization (`1.#QNAN0`, `-1.#INF000`,
/// `1.#IND`) — retail `va_garbagetruck.aivehicledata` authors
/// `MaxAng 1.#QNAN0 …`, which Rust's `f64::parse` rejects. Decode it as
/// the NaN it prints rather than dropping the component.
fn msvc_float(v: &TuneValue) -> Option<f64> {
    if let Some(n) = v.number {
        return Some(n);
    }
    let upper = v.raw.to_ascii_uppercase();
    let body = upper.trim_start_matches(['+', '-']);
    let neg = upper.starts_with('-');
    if body.contains("#QNAN") || body.contains("#IND") || body.contains("#SNAN") {
        Some(f64::NAN)
    } else if body.contains("#INF") {
        Some(if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        })
    } else {
        None
    }
}

/// `vec3` variant tolerant of MSVC non-finite literals; `None` when the
/// field is absent or any component is genuinely non-numeric.
fn vec3_loose(b: &TuneBlock, name: &str) -> Option<[f32; 3]> {
    let f = b.field(name)?;
    if f.values.len() < 3 {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (i, v) in f.values[..3].iter().enumerate() {
        out[i] = msvc_float(v)? as f32;
    }
    Some(out)
}

/// Optional-`vec3` accessor: `None` when absent, the decoded value when
/// well-formed, and `None` plus a warning when present but not three
/// numeric components — a malformed optional vector is reported, never
/// silently equated with an absent one.
fn opt_vec3(b: &TuneBlock, ctx: &str, name: &str, warnings: &mut Vec<String>) -> Option<[f32; 3]> {
    let f = b.field(name)?;
    match vec3_loose(b, name) {
        Some(v) => Some(v),
        None => {
            warnings.push(format!(
                "{ctx}: {name} is not three numeric values ({})",
                f.values
                    .iter()
                    .map(|v| v.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            None
        }
    }
}

/// Integer scalar: the authored value must exist and be integral —
/// silently truncating `3.5` to `3` would hide malformed data.
fn req_i64(b: &TuneBlock, ctx: &str, name: &str) -> VehResult<i64> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 => Ok(n as i64),
            Some(_) => err(ctx, format!("field {name:?} value is not an integer")),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => err(ctx, format!("missing required field {name:?}")),
    }
}

/// Optional integer scalar (same rules as [`req_i64`]); `None` when absent.
fn opt_i64(b: &TuneBlock, ctx: &str, name: &str) -> VehResult<Option<i64>> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 => Ok(Some(n as i64)),
            Some(n) => err(ctx, format!("field {name:?} value {n} is not an integer")),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => Ok(None),
    }
}

// ---------- damage & recovery records (F05-A) ----------

/// Structural problems [`VehCarDamage::validate`],
/// [`VehStuck::validate`] and [`VehGyro::validate`] report — authored-data
/// anomalies, not decode failures.
#[derive(Debug, Clone, PartialEq)]
pub enum DamageIssue {
    /// A field that must hold a finite number carries NaN/infinity.
    NonFinite { field: &'static str },
    /// A field that must be non-negative is negative.
    Negative { field: &'static str, value: f64 },
    /// `MedDamage` exceeds `MaxDamage` — the mid tier would sit past
    /// the destruction bound. Every retail file orders them
    /// `MedDamage < MaxDamage`.
    MedAboveMax { med: f64, max: f64 },
}

impl fmt::Display for DamageIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DamageIssue::NonFinite { field } => write!(f, "{field} is not finite"),
            DamageIssue::Negative { field, value } => {
                write!(f, "{field} is negative ({value})")
            }
            DamageIssue::MedAboveMax { med, max } => {
                write!(f, "MedDamage {med} exceeds MaxDamage {max}")
            }
        }
    }
}

fn check_f32(issues: &mut Vec<DamageIssue>, field: &'static str, v: f32, allow_negative: bool) {
    if !v.is_finite() {
        issues.push(DamageIssue::NonFinite { field });
    } else if !allow_negative && v < 0.0 {
        issues.push(DamageIssue::Negative {
            field,
            value: v as f64,
        });
    }
}

fn check_vec3(issues: &mut Vec<DamageIssue>, field: &'static str, v: [f32; 3]) {
    if v.iter().any(|c| !c.is_finite()) {
        issues.push(DamageIssue::NonFinite { field });
    }
}

/// The particle spec embedded in `vehCarDamage` — the damage smoke /
/// debris emitter the effect runs from `SmokeOffset` (and
/// `SmokeOffset2`). The field vocabulary is the `dgBangerData`
/// `BirthRule` set plus `LifeVar`, `DampVar`, `Height`, `Intensity` and
/// `Color` extras; semantics follow the authored names — what the
/// original does with each is unverified (UNK-13).
#[derive(Debug, Clone)]
pub struct DamageEffect {
    pub position: [f32; 3],
    pub position_var: [f32; 3],
    pub velocity: [f32; 3],
    pub velocity_var: [f32; 3],
    pub life: f32,
    pub life_var: f32,
    pub mass: f32,
    pub mass_var: f32,
    pub radius: f32,
    pub radius_var: f32,
    pub drag: f32,
    pub drag_var: f32,
    pub damp: f32,
    pub damp_var: f32,
    pub d_radius: f32,
    pub d_radius_var: f32,
    pub d_alpha: f32,
    pub d_alpha_var: f32,
    pub d_rotation: f32,
    pub d_rotation_var: f32,
    pub initial_blast: i64,
    pub spew_rate: f32,
    pub spew_time_limit: f32,
    pub gravity: f32,
    pub tex_frame_start: i64,
    pub tex_frame_end: i64,
    pub birth_flags: i64,
    pub height: f32,
    pub intensity: f32,
    /// `Color` — a packed colour word preserved verbatim (`-167772161`
    /// on retail reads as a negative 32-bit ARGB-ish value).
    pub color: i64,
}

/// Fully decoded `vehCarDamage` tuning — the damage model and its
/// embedded effect spec (`tune/vehicle/<vp_*>.vehcardamage`, F05-A).
///
/// One flat block per file: damage thresholds, two smoke-emitter
/// pivots, the `TextelDamageRadius` decal extent and a flat
/// [`DamageEffect`] particle spec. All 20 retail records carry the same
/// field set; `MirrorPivot` appears on 7. The damage unit reads as
/// impulse-scale — `MaxDamage` ranges 238k (`vpauditt`) to 3.28M
/// (`vpsemi`) tracking vehicle mass — but the exact original
/// accumulation semantics are unverified (UNK-13).
#[derive(Debug, Clone)]
pub struct VehCarDamage {
    /// `MaxDamage` — accumulated damage at which the vehicle is
    /// destroyed (the DMG-1/DMG-2 "damage meter" bound).
    pub max_damage: f32,
    /// `MedDamage` — mid-tier bound; `MedDamage < MaxDamage` on every
    /// retail record (the meter's yellow band / damaged-visual tier).
    pub med_damage: f32,
    /// `ImpactThreshold` — impacts at or below this do not damage
    /// (1500 on every retail record).
    pub impact_threshold: f32,
    /// `RegenerateRate` — damage healed per second; 0 on every retail
    /// record (the C&R healing of DMG-4 would drive it elsewhere).
    pub regenerate_rate: f32,
    /// `TextelDamageRadius` — radius of the damage decal/deformation
    /// around an impact point, metres (inferred).
    pub textel_damage_radius: f32,
    /// `SmokeOffset` — first smoke-emitter pivot in car space.
    pub smoke_offset: [f32; 3],
    /// `SmokeOffset2` — second smoke-emitter pivot in car space.
    pub smoke_offset2: [f32; 3],
    /// `DoublePivot` — 0 on every retail record (whether both pivots
    /// emit — inferred).
    pub double_pivot: i64,
    /// `MirrorPivot` — authored on 7 retail records (vpbus, vpcab,
    /// vpcaddie, vpcoop, vpcop, vpddbus, vpsemi — the tall bodies),
    /// 0 on all of them; semantics unverified.
    pub mirror_pivot: Option<i64>,
    /// The embedded particle spec — flat fields sharing the root
    /// block, not a nested `BirthRule` sub-block.
    pub effect: DamageEffect,
    /// Unknown/unmapped fields encountered while decoding.
    pub warnings: Vec<String>,
}

impl VehCarDamage {
    /// Decode the root `vehCarDamage` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "vehCarDamage" {
            return err(
                "vehCarDamage",
                format!(
                    "expected vehCarDamage root block, found {:?}",
                    file.root.name
                ),
            );
        }
        let root = &file.root;
        let ctx = "vehCarDamage";
        let mut warnings = Vec::new();

        unknown_fields(
            root,
            ctx,
            &[
                "MaxDamage",
                "MedDamage",
                "ImpactThreshold",
                "RegenerateRate",
                "TextelDamageRadius",
                "SmokeOffset",
                "SmokeOffset2",
                "DoublePivot",
                "MirrorPivot",
                "Position",
                "PositionVar",
                "Velocity",
                "VelocityVar",
                "Life",
                "LifeVar",
                "Mass",
                "MassVar",
                "Radius",
                "RadiusVar",
                "Drag",
                "DragVar",
                "Damp",
                "DampVar",
                "DRadius",
                "DRadiusVar",
                "DAlpha",
                "DAlphaVar",
                "DRotation",
                "DRotationVar",
                "InitialBlast",
                "SpewRate",
                "SpewTimeLimit",
                "Gravity",
                "TexFrameStart",
                "TexFrameEnd",
                "BirthFlags",
                "Height",
                "Intensity",
                "Color",
            ],
            &mut warnings,
        );

        Ok(VehCarDamage {
            max_damage: req_f32(root, ctx, "MaxDamage")?,
            med_damage: req_f32(root, ctx, "MedDamage")?,
            impact_threshold: req_f32(root, ctx, "ImpactThreshold")?,
            regenerate_rate: req_f32(root, ctx, "RegenerateRate")?,
            textel_damage_radius: req_f32(root, ctx, "TextelDamageRadius")?,
            smoke_offset: req_vec3(root, ctx, "SmokeOffset")?,
            smoke_offset2: req_vec3(root, ctx, "SmokeOffset2")?,
            double_pivot: req_i64(root, ctx, "DoublePivot")?,
            mirror_pivot: opt_i64(root, ctx, "MirrorPivot")?,
            effect: DamageEffect {
                position: req_vec3(root, ctx, "Position")?,
                position_var: req_vec3(root, ctx, "PositionVar")?,
                velocity: req_vec3(root, ctx, "Velocity")?,
                velocity_var: req_vec3(root, ctx, "VelocityVar")?,
                life: req_f32(root, ctx, "Life")?,
                life_var: req_f32(root, ctx, "LifeVar")?,
                mass: req_f32(root, ctx, "Mass")?,
                mass_var: req_f32(root, ctx, "MassVar")?,
                radius: req_f32(root, ctx, "Radius")?,
                radius_var: req_f32(root, ctx, "RadiusVar")?,
                drag: req_f32(root, ctx, "Drag")?,
                drag_var: req_f32(root, ctx, "DragVar")?,
                damp: req_f32(root, ctx, "Damp")?,
                damp_var: req_f32(root, ctx, "DampVar")?,
                d_radius: req_f32(root, ctx, "DRadius")?,
                d_radius_var: req_f32(root, ctx, "DRadiusVar")?,
                d_alpha: req_f32(root, ctx, "DAlpha")?,
                d_alpha_var: req_f32(root, ctx, "DAlphaVar")?,
                d_rotation: req_f32(root, ctx, "DRotation")?,
                d_rotation_var: req_f32(root, ctx, "DRotationVar")?,
                initial_blast: req_i64(root, ctx, "InitialBlast")?,
                spew_rate: req_f32(root, ctx, "SpewRate")?,
                spew_time_limit: req_f32(root, ctx, "SpewTimeLimit")?,
                gravity: req_f32(root, ctx, "Gravity")?,
                tex_frame_start: req_i64(root, ctx, "TexFrameStart")?,
                tex_frame_end: req_i64(root, ctx, "TexFrameEnd")?,
                birth_flags: req_i64(root, ctx, "BirthFlags")?,
                height: req_f32(root, ctx, "Height")?,
                intensity: req_f32(root, ctx, "Intensity")?,
                color: req_i64(root, ctx, "Color")?,
            },
            warnings,
        })
    }

    /// Authored-consistency checks; empty on a well-formed record.
    pub fn validate(&self) -> Vec<DamageIssue> {
        let mut issues = Vec::new();
        check_f32(&mut issues, "MaxDamage", self.max_damage, false);
        check_f32(&mut issues, "MedDamage", self.med_damage, false);
        check_f32(&mut issues, "ImpactThreshold", self.impact_threshold, false);
        check_f32(&mut issues, "RegenerateRate", self.regenerate_rate, false);
        check_f32(
            &mut issues,
            "TextelDamageRadius",
            self.textel_damage_radius,
            false,
        );
        check_vec3(&mut issues, "SmokeOffset", self.smoke_offset);
        check_vec3(&mut issues, "SmokeOffset2", self.smoke_offset2);
        if self.med_damage > self.max_damage {
            issues.push(DamageIssue::MedAboveMax {
                med: self.med_damage as f64,
                max: self.max_damage as f64,
            });
        }
        issues
    }
}

/// Fully decoded `vehStuck` tuning — the authored stuck-detection
/// thresholds (`tune/vehicle/<vp_*>.vehstuck`, F05-A). One flat block
/// per file; all 20 retail records carry the same six fields.
/// Field semantics are unverified (UNK-13): `TimeThresh` reads as the
/// seconds the condition must hold, `PosThresh`/`MoveThresh` as the
/// displacement bounds the original compares progress against, and
/// `Turn`/`Rotation`/`Translation` as the angular/linear test weights —
/// all inferred from names, not recovered behaviour.
#[derive(Debug, Clone)]
pub struct VehStuck {
    /// `Turn` — ~1.57 (≈ π/2) on most retail records.
    pub turn: f32,
    /// `Rotation` — 0 on most retail records.
    pub rotation: f32,
    /// `Translation` — small linear threshold.
    pub translation: f32,
    /// `TimeThresh` — seconds the stuck condition must persist.
    pub time_thresh: f32,
    /// `PosThresh` — positional bound (metres, inferred).
    pub pos_thresh: f32,
    /// `MoveThresh` — movement bound under `PosThresh` (inferred).
    pub move_thresh: f32,
    /// Unknown/unmapped fields encountered while decoding.
    pub warnings: Vec<String>,
}

impl VehStuck {
    /// Decode the root `vehStuck` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "vehStuck" {
            return err(
                "vehStuck",
                format!("expected vehStuck root block, found {:?}", file.root.name),
            );
        }
        let root = &file.root;
        let ctx = "vehStuck";
        let mut warnings = Vec::new();
        unknown_fields(
            root,
            ctx,
            &[
                "Turn",
                "Rotation",
                "Translation",
                "TimeThresh",
                "PosThresh",
                "MoveThresh",
            ],
            &mut warnings,
        );
        Ok(VehStuck {
            turn: req_f32(root, ctx, "Turn")?,
            rotation: req_f32(root, ctx, "Rotation")?,
            translation: req_f32(root, ctx, "Translation")?,
            time_thresh: req_f32(root, ctx, "TimeThresh")?,
            pos_thresh: req_f32(root, ctx, "PosThresh")?,
            move_thresh: req_f32(root, ctx, "MoveThresh")?,
            warnings,
        })
    }

    /// Authored-consistency checks; empty on a well-formed record.
    pub fn validate(&self) -> Vec<DamageIssue> {
        let mut issues = Vec::new();
        check_f32(&mut issues, "Turn", self.turn, true);
        check_f32(&mut issues, "Rotation", self.rotation, true);
        check_f32(&mut issues, "Translation", self.translation, true);
        check_f32(&mut issues, "TimeThresh", self.time_thresh, false);
        check_f32(&mut issues, "PosThresh", self.pos_thresh, false);
        check_f32(&mut issues, "MoveThresh", self.move_thresh, false);
        issues
    }
}

/// Fully decoded `vehGyro` tuning — the rollover-recovery gyro assist
/// (`tune/vehicle/<vp_*>.vehgyro`, F05-A). `Drift`, `Spin180` and
/// `Reverse180` are authored on every retail record; `Roll` and `Pitch`
/// appear on 17 of 21 (absent on vpdune, vpford, vpmustang99 and
/// vpvwcup_angel). The names read as assisted air/righting rotations —
/// semantics unverified (UNK-13).
#[derive(Debug, Clone)]
pub struct VehGyro {
    /// `Drift` — small drift assist factor.
    pub drift: f32,
    /// `Spin180` — half-spin assist rate (rad/s, inferred).
    pub spin180: f32,
    /// `Reverse180` — reverse half-spin assist rate (rad/s, inferred).
    pub reverse180: f32,
    /// `Roll` — roll-righting assist (absent on 4 retail records).
    pub roll: Option<f32>,
    /// `Pitch` — pitch-righting assist (absent on 4 retail records).
    pub pitch: Option<f32>,
    /// Unknown/unmapped fields encountered while decoding.
    pub warnings: Vec<String>,
}

impl VehGyro {
    /// Decode the root `vehGyro` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "vehGyro" {
            return err(
                "vehGyro",
                format!("expected vehGyro root block, found {:?}", file.root.name),
            );
        }
        let root = &file.root;
        let ctx = "vehGyro";
        let mut warnings = Vec::new();
        unknown_fields(
            root,
            ctx,
            &["Drift", "Spin180", "Reverse180", "Roll", "Pitch"],
            &mut warnings,
        );
        Ok(VehGyro {
            drift: req_f32(root, ctx, "Drift")?,
            spin180: req_f32(root, ctx, "Spin180")?,
            reverse180: req_f32(root, ctx, "Reverse180")?,
            roll: root.f32("Roll"),
            pitch: root.f32("Pitch"),
            warnings,
        })
    }

    /// Authored-consistency checks; empty on a well-formed record.
    pub fn validate(&self) -> Vec<DamageIssue> {
        let mut issues = Vec::new();
        check_f32(&mut issues, "Drift", self.drift, true);
        check_f32(&mut issues, "Spin180", self.spin180, false);
        check_f32(&mut issues, "Reverse180", self.reverse180, false);
        if let Some(v) = self.roll {
            check_f32(&mut issues, "Roll", v, true);
        }
        if let Some(v) = self.pitch {
            check_f32(&mut issues, "Pitch", v, true);
        }
        issues
    }
}

/// Fully decoded `aiVehicleData` tuning — the ambient-traffic vehicle
/// record (`tune/vehicle/<va_*>.aivehicledata`, F10-A).
///
/// One flat block per file: mass, full-extent `Size` (same convention
/// as `dgBangerData`), collision spring/damper coefficients and damage
/// thresholds. There is no drivetrain, wheel or gearing data — ambient
/// vehicles are not driven through `vehCarSim` physics, matching the
/// spec's "ambient vehicle data may differ from player tuning". All 23
/// retail records carry the same field set; `CG` is absent on three
/// (`va_cablecar_f`, `va_garbagetruck`, `va_ug_l`).
#[derive(Debug, Clone)]
pub struct AiVehicleData {
    /// `Mass` in kg.
    pub mass: f32,
    /// `Size` — bound full extents (width, height, length).
    pub size: [f32; 3],
    /// `MaxAng` — authored on every retail file, all-zero except
    /// `va_garbagetruck`'s NaN first component. Semantics unverified
    /// (likely an angular-velocity cap); retained verbatim.
    pub max_ang: Option<[f32; 3]>,
    /// `Elasticity` — collision restitution coefficient.
    pub elasticity: f32,
    /// `Friction` — collision friction coefficient.
    pub friction: f32,
    /// `MaxDamage` — damage-energy bound (the F05 damage consumer, not
    /// traffic, owns its interpretation).
    pub max_damage: f32,
    /// `PtxThresh` — particle-trigger impulse threshold (same name as
    /// `dgBangerData`'s).
    pub ptx_thresh: f32,
    /// `Spring`/`Damping` — collision-response coefficients.
    pub spring: f32,
    /// See [`Self::spring`].
    pub damping: f32,
    /// `Limit` — 0.07 on every retail file; semantics unverified.
    pub limit: f32,
    /// `RubberSpring`/`RubberDamp` — secondary (tyre?) response
    /// coefficients; exact original use unverified.
    pub rubber_spring: f32,
    /// See [`Self::rubber_spring`].
    pub rubber_damp: f32,
    /// `CG` — bound centre offset; absent on three retail files.
    pub cg: Option<[f32; 3]>,
    /// Unknown/unmapped fields encountered while decoding.
    pub warnings: Vec<String>,
}

impl AiVehicleData {
    /// Decode the root `aiVehicleData` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> VehResult<Self> {
        if file.root.name != "aiVehicleData" {
            return err(
                "aiVehicleData",
                format!(
                    "expected aiVehicleData root block, found {:?}",
                    file.root.name
                ),
            );
        }
        let root = &file.root;
        let ctx = "aiVehicleData";
        let mut warnings = Vec::new();

        let max_ang = opt_vec3(root, ctx, "MaxAng", &mut warnings);

        unknown_fields(
            root,
            ctx,
            &[
                "Mass",
                "Size",
                "MaxAng",
                "Elasticity",
                "Friction",
                "MaxDamage",
                "PtxThresh",
                "Spring",
                "Damping",
                "Limit",
                "RubberSpring",
                "RubberDamp",
                "CG",
            ],
            &mut warnings,
        );

        Ok(AiVehicleData {
            mass: req_f32(root, ctx, "Mass")?,
            size: req_vec3(root, ctx, "Size")?,
            max_ang,
            elasticity: req_f32(root, ctx, "Elasticity")?,
            friction: req_f32(root, ctx, "Friction")?,
            max_damage: req_f32(root, ctx, "MaxDamage")?,
            ptx_thresh: req_f32(root, ctx, "PtxThresh")?,
            spring: req_f32(root, ctx, "Spring")?,
            damping: req_f32(root, ctx, "Damping")?,
            limit: req_f32(root, ctx, "Limit")?,
            rubber_spring: req_f32(root, ctx, "RubberSpring")?,
            rubber_damp: req_f32(root, ctx, "RubberDamp")?,
            cg: opt_vec3(root, ctx, "CG", &mut warnings),
            warnings,
        })
    }
}
