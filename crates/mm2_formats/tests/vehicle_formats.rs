//! Synthetic-fixture tests for the vehicle-related format parsers. These use
//! self-authored input only; no proprietary MM2 data is required.

use mm2_formats::bnd::BndFile;
use mm2_formats::info::InfoFile;
use mm2_formats::mtx::Mtx;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::{AiVehicleData, DrivetrainType, VehCarSim, VehTrailer};

const INFO: &str = "BaseName=vpbug\r\n\
Description=VW New Beetle\r\n\
Colors=Yellow|Blue|Silver|Red\r\n\
Flags=0\r\n\
Order=-1\r\n\
ScoringBias=20.0\r\n\
UnlockScore=0\r\n\
UnlockFlags=0\r\n\
Horsepower=150\r\n\
Top Speed=91\r\n\
Durability=760000\r\n\
Mass=4250\r\n\
UIDist=5.5\r\n\
ForceFeedbackModifier=0.24\r\n\
RoadForceModifier=2.0\r\n";

const CARSIM: &str = "type: a\r\n\
vehCarSim {\r\n\
  Mass 1000.000000\r\n\
  InertiaBox 2.0 2.0 3.0\r\n\
  CenterOfGravity 0.0 -0.1 0.0\r\n\
  BoundFriction 0.5\r\n\
  BoundElasticity 0.5\r\n\
  DrivetrainType 1\r\n\
  SSSValue 1.0\r\n\
  SSSThreshold 0.0\r\n\
  CarFrictionHandling 1.0\r\n\
  Aero {\r\n\
    AngCDamp 1.3 1.4 1.0\r\n\
    AngVelDamp 0.0 0.0 0.0\r\n\
    AngVel2Damp 0.14 2.0 1.0\r\n\
    Drag 0.5\r\n\
    Down 0.0\r\n\
  }\r\n\
  Engine {\r\n\
    AngInertia 1.0\r\n\
    MaxHorsePower 260.0\r\n\
    IdleRPM 750.0\r\n\
    OptRPM 5800.0\r\n\
    MaxRPM 8500.0\r\n\
    GCL 0.25\r\n\
  }\r\n\
  Trans {\r\n\
    ManualNumGears 7\r\n\
    AutoNumGears 6\r\n\
    Reverse 30.0\r\n\
    Low 20.0\r\n\
    High 90.0\r\n\
    GearBias 0.5\r\n\
    UpshiftBias 0.05\r\n\
    DownshiftBiasMin 0.05\r\n\
    DownshiftBiasMax 0.3\r\n\
    GearChangeTime 0.8\r\n\
  }\r\n\
  Drivetrain {\r\n\
    AngInertia 2000.0\r\n\
    BrakeDynamicCoef 1.0\r\n\
    BrakeStaticCoef 1.2\r\n\
  }\r\n\
  WheelFront {\r\n\
    SuspensionExtent 0.15\r\n\
    SuspensionLimit 0.05\r\n\
    SuspensionFactor 1.0\r\n\
    SuspensionDampCoef 0.1\r\n\
    SteeringLimit 0.4\r\n\
    SteeringOffset 0.25\r\n\
    BrakeCoef 0.625\r\n\
    HandbrakeCoef 2.0\r\n\
    TireDispLimitLong 0.125\r\n\
    TireDampCoefLong 0.25\r\n\
    TireDragCoefLong 0.02\r\n\
    TireDispLimitLat 0.125\r\n\
    TireDampCoefLat 0.25\r\n\
    TireDragCoefLat 0.05\r\n\
    OptimumSlipPercent 0.16\r\n\
    StaticFric 3.0\r\n\
    SlidingFric 2.7\r\n\
  }\r\n\
  WheelBack {\r\n\
    SuspensionExtent 0.15\r\n\
    SuspensionLimit 0.05\r\n\
    SuspensionFactor 1.0\r\n\
    SuspensionDampCoef 0.1\r\n\
    SteeringLimit 0.03\r\n\
    SteeringOffset 0.0\r\n\
    BrakeCoef 0.606\r\n\
    HandbrakeCoef 2.0\r\n\
    TireDispLimitLong 0.125\r\n\
    TireDampCoefLong 0.25\r\n\
    TireDragCoefLong 0.02\r\n\
    TireDispLimitLat 0.125\r\n\
    TireDampCoefLat 0.25\r\n\
    TireDragCoefLat 0.05\r\n\
    OptimumSlipPercent 0.13\r\n\
    StaticFric 3.0\r\n\
    SlidingFric 2.8\r\n\
  }\r\n\
  AxleFront {\r\n\
    TorqueCoef 0.0\r\n\
    DampCoef 0.0\r\n\
  }\r\n\
  AxleBack {\r\n\
    TorqueCoef 0.0\r\n\
    DampCoef 0.0\r\n\
  }\r\n\
}\r\n";

#[test]
fn info_parses_keys_with_spaces_and_lists() {
    let info = InfoFile::parse(INFO);
    assert_eq!(info.get("BaseName"), Some("vpbug"));
    assert_eq!(info.get("Description"), Some("VW New Beetle"));
    assert_eq!(info.list("Colors"), vec!["Yellow", "Blue", "Silver", "Red"]);
    assert_eq!(info.f32("Top Speed"), Some(91.0));
    assert_eq!(info.f32("Mass"), Some(4250.0));
    assert!(info.diagnostics.is_empty());
}

#[test]
fn info_reports_bad_lines_and_duplicates() {
    let info = InfoFile::parse("BaseName=x\nno equals here\nBaseName=y\n");
    assert_eq!(info.get("BaseName"), Some("x"));
    assert_eq!(info.diagnostics.len(), 2);
}

#[test]
fn tune_parses_nested_blocks_and_fields() {
    let tune = TuneFile::parse(CARSIM).unwrap();
    assert_eq!(tune.type_tag.as_deref(), Some("a"));
    let root = &tune.root;
    assert_eq!(root.name, "vehCarSim");
    assert_eq!(root.f32("Mass"), Some(1000.0));
    assert_eq!(root.vec3("InertiaBox"), Some([2.0, 2.0, 3.0]));
    let engine = root.block("Engine").unwrap();
    assert_eq!(engine.f32("MaxHorsePower"), Some(260.0));
    let front = root.block("WheelFront").unwrap();
    let back = root.block("WheelBack").unwrap();
    // Same-named keys in different blocks stay scoped.
    assert_eq!(front.f32("SteeringLimit"), Some(0.4));
    assert_eq!(back.f32("SteeringLimit"), Some(0.03));
}

#[test]
fn tune_rejects_malformed_input() {
    assert!(TuneFile::parse("vehCarSim { Mass").is_err());
    assert!(TuneFile::parse("vehCarSim { Mass 1.0").is_err());
    assert!(TuneFile::parse("{ Mass 1.0 }").is_err());
    assert!(TuneFile::parse("vehCarSim { Mass 1.0 } }").is_err());
}

#[test]
fn vehcarsim_decodes_typed_fields() {
    let tune = TuneFile::parse(CARSIM).unwrap();
    let sim = VehCarSim::from_tune(&tune).unwrap();
    assert_eq!(sim.mass, 1000.0);
    assert_eq!(sim.drivetrain_type, DrivetrainType::Fwd);
    assert_eq!(sim.engine.max_horsepower, 260.0);
    assert_eq!(sim.engine.opt_rpm, 5800.0);
    assert_eq!(sim.trans.auto_num_gears, 6);
    assert_eq!(sim.trans.low_mph, 20.0);
    assert_eq!(sim.trans.high_mph, 90.0);
    assert_eq!(sim.wheel_front.steering_limit, 0.4);
    assert_eq!(sim.wheel_back.steering_limit, 0.03);
    assert_eq!(sim.wheel_front.static_fric, 3.0);
    assert_eq!(sim.wheel_back.sliding_fric, 2.8);
}

#[test]
fn vehcarsim_rejects_missing_required_fields() {
    // Missing Engine block entirely.
    let tune = TuneFile::parse(
        "vehCarSim { Mass 1000.0 InertiaBox 1 1 1 Trans { AutoNumGears 1 Reverse 5 Low 5 High 5 } }",
    );
    let tune = match tune {
        Ok(t) => t,
        Err(_) => {
            // Missing wheels etc may already fail at parse level; fine either way.
            return;
        }
    };
    assert!(VehCarSim::from_tune(&tune).is_err());
}

#[test]
fn vehtrailer_decodes_hitch_offsets() {
    let src = "type: a\nvehTrailer {\n\
        Mass 2000.0\n\
        InertiaBox 2.5 3.6 10.5\n\
        CarHitchOffset -0.07 0.4 2.25\n\
        TrailerHitchOffset -0.07 0.58 -5.86\n\
        WheelFront {\n\
          SuspensionExtent 0.1\n\
          SuspensionLimit 0.1\n\
          SuspensionFactor 1.0\n\
          SuspensionDampCoef 0.2\n\
          SteeringLimit 0.39\n\
          BrakeCoef 1.0\n\
          TireDispLimitLong 0.125\n\
          TireDampCoefLong 0.25\n\
          TireDragCoefLong 0.02\n\
          TireDispLimitLat 0.125\n\
          TireDampCoefLat 0.25\n\
          TireDragCoefLat 0.05\n\
          OptimumSlipPercent 0.14\n\
          StaticFric 2.0\n\
          SlidingFric 1.9\n\
        }\n\
        WheelBack {\n\
          SuspensionExtent 0.33\n\
          SuspensionLimit 0.1\n\
          SuspensionFactor 1.0\n\
          SuspensionDampCoef 0.2\n\
          SteeringLimit 0.18\n\
          BrakeCoef 1.0\n\
          TireDispLimitLong 0.125\n\
          TireDampCoefLong 0.25\n\
          TireDragCoefLong 0.02\n\
          TireDispLimitLat 0.125\n\
          TireDampCoefLat 0.25\n\
          TireDragCoefLat 0.05\n\
          OptimumSlipPercent 0.14\n\
          StaticFric 2.0\n\
          SlidingFric 1.9\n\
        }\n\
    }\n";
    let tune = TuneFile::parse(src).unwrap();
    let trailer = VehTrailer::from_tune(&tune).unwrap();
    assert_eq!(trailer.mass, 2000.0);
    assert_eq!(trailer.trailer_hitch_offset.unwrap()[2], -5.86);
    assert_eq!(trailer.wheel_back.steering_limit, 0.18);

    // vpcentury-style trailer without hitch offsets is still valid.
    let no_hitch = src.replace("CarHitchOffset -0.07 0.4 2.25", "");
    let tune = TuneFile::parse(&no_hitch).unwrap();
    let trailer = VehTrailer::from_tune(&tune).unwrap();
    assert!(trailer.car_hitch_offset.is_none());
    assert!(
        trailer
            .warnings
            .iter()
            .any(|w| w.contains("CarHitchOffset"))
    );
}

#[test]
fn mtx_parses_48_byte_records() {
    let mut data = Vec::new();
    for v in [
        -0.88f32, 0.01, -1.52, // bounds min
        -0.63, 0.60, -0.85, // bounds max
        0.0, 0.0, 0.0, // pivot
        -0.7541, 0.3071, -1.184, // origin
    ] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mtx = Mtx::parse(&data).unwrap();
    assert_eq!(mtx.origin, [-0.7541, 0.3071, -1.184]);
    assert!((mtx.wheel_radius() - 0.295).abs() < 1e-3);
    assert!((mtx.wheel_width() - 0.25).abs() < 1e-3);
    assert!(Mtx::parse(&data[..20]).is_err());
}

const BND: &str = "version: 1.01\n\
verts: 4\n\
materials: 1\n\
edges: 0\n\
polys: 2\n\
\n\
v\t0.0\t0.0\t0.0\n\
v\t1.0\t0.0\t0.0\n\
v\t1.0\t1.0\t0.0\n\
v\t0.0\t1.0\t0.0\n\
\n\
mtl default {\n\
\telasticity: 0.1\n\
\tfriction: 0.5\n\
\teffect: none\n\
\tsound: 0\n\
}\n\
\n\
quad 0 1 2 3 0\n\
tri 0 2 3 0\n";

#[test]
fn bnd_parses_verts_materials_polys() {
    let bnd = BndFile::parse(BND).unwrap();
    assert_eq!(bnd.version, "1.01");
    assert_eq!(bnd.verts.len(), 4);
    assert_eq!(bnd.materials.len(), 1);
    assert_eq!(bnd.materials[0].friction, Some(0.5));
    assert_eq!(bnd.polys.len(), 2);
    assert_eq!(bnd.polys[0].indices.len(), 4);
    assert_eq!(bnd.polys[1].indices.len(), 3);
    let (min, max) = bnd.aabb().unwrap();
    assert_eq!(min, [0.0, 0.0, 0.0]);
    assert_eq!(max, [1.0, 1.0, 0.0]);
}

#[test]
fn bnd_rejects_bad_input() {
    assert!(BndFile::parse("version: 1.01\nverts: 2\nv 0 0 0\n").is_err());
    assert!(BndFile::parse("\0binary").is_err());
    // Out-of-range vertex index.
    assert!(BndFile::parse("verts: 1\npolys: 1\nv 0 0 0\ntri 0 1 2 0\n").is_err());
}

/// Minimal ambient-vehicle record — the retail `va_*` shape (all fields
/// present, `CG` authored).
const AIVEHICLE: &str = "type: a\r\n\
aiVehicleData {\r\n\
  Mass 585.095459\r\n\
  Size 1.838961 1.122816 4.286561\r\n\
  MaxAng 0.000000 0.000000 0.000000\r\n\
  Elasticity 0.9100000\r\n\
  Friction 0.1500000\r\n\
  MaxDamage 70807.632813\r\n\
  PtxThresh 70807.640625\r\n\
  Spring 17701.910156\r\n\
  Damping 1239.133667\r\n\
  Limit 0.070000\r\n\
  RubberSpring 12391.335938\r\n\
  RubberDamp 619.566833\r\n\
  CG 0.000026 0.819497 0.170570\r\n\
}\r\n";

#[test]
fn aivehicledata_decodes_the_retail_field_set() {
    let tune = TuneFile::parse(AIVEHICLE).unwrap();
    let data = AiVehicleData::from_tune(&tune).unwrap();
    assert_eq!(data.mass, 585.095_46);
    assert_eq!(data.size, [1.838961, 1.122816, 4.286561]);
    assert_eq!(data.max_ang, Some([0.0, 0.0, 0.0]));
    assert_eq!(data.limit, 0.07);
    assert_eq!(data.rubber_damp, 619.566_83);
    assert_eq!(data.cg, Some([0.000026, 0.819497, 0.170570]));
    assert!(data.warnings.is_empty(), "{:?}", data.warnings);
}

#[test]
fn aivehicledata_decodes_msvc_non_finite_literals() {
    // Retail `va_garbagetruck.aivehicledata` authors `MaxAng 1.#QNAN0 …`
    // — MSVC's NaN print — which `f64::parse` rejects. The component
    // decodes as the NaN it prints, not as missing/zero.
    let src = AIVEHICLE.replace(
        "MaxAng 0.000000 0.000000 0.000000",
        "MaxAng 1.#QNAN0 0.000000 -1.#INF000",
    );
    let tune = TuneFile::parse(&src).unwrap();
    let data = AiVehicleData::from_tune(&tune).unwrap();
    let max_ang = data.max_ang.expect("MaxAng decodes");
    assert!(max_ang[0].is_nan());
    assert_eq!(max_ang[1], 0.0);
    assert_eq!(max_ang[2], f32::NEG_INFINITY);
    assert!(data.warnings.is_empty(), "{:?}", data.warnings);
}

#[test]
fn aivehicledata_tolerates_absent_cg_and_flags_garbage() {
    // va_cablecar_f / va_ug_l / va_garbagetruck ship without CG.
    let src = AIVEHICLE.replace("CG 0.000026 0.819497 0.170570\r\n", "");
    let tune = TuneFile::parse(&src).unwrap();
    let data = AiVehicleData::from_tune(&tune).unwrap();
    assert_eq!(data.cg, None);

    // A present-but-non-numeric MaxAng is a warning, not a failure or a
    // silently zeroed value.
    let src = AIVEHICLE.replace(
        "MaxAng 0.000000 0.000000 0.000000",
        "MaxAng someday 0.0 0.0",
    );
    let tune = TuneFile::parse(&src).unwrap();
    let data = AiVehicleData::from_tune(&tune).unwrap();
    assert_eq!(data.max_ang, None);
    assert!(
        data.warnings.iter().any(|w| w.contains("MaxAng")),
        "{:?}",
        data.warnings
    );

    // Missing required fields and wrong roots still fail.
    let src = AIVEHICLE.replace("Mass 585.095459\r\n", "");
    let tune = TuneFile::parse(&src).unwrap();
    assert!(AiVehicleData::from_tune(&tune).is_err());
    let tune = TuneFile::parse("vehCarSim { Mass 1.0 }").unwrap();
    assert!(AiVehicleData::from_tune(&tune).is_err());
}
