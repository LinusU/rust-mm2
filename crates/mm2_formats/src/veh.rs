//! Typed views over the block-structured vehicle tuning files
//! (`vehCarSim`, `vehTrailer`, `asNode`) parsed by [`crate::tune`].
//!
//! These types decode the fields the runtime conversion needs, keep raw
//! values for diagnostics, and report unknown fields so authored data is
//! never silently dropped.

use crate::tune::{TuneBlock, TuneFile};
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
