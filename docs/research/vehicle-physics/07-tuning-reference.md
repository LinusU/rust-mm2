# Tuning reference: every physics token, what it does, retail values

The retail loaders read `tune/vehicle/<car>.vehcarsim`,
`<car>.vehgyro` and `<car>.vehstuck` through `datParser` field
registrations (type 4 = int, 5 = float, 7 = vec3). A token absent from
the file keeps the **constructor default** listed here. Tokens the
loader does not register are never consumed (several spare ids, e.g.
`vpftruck`, carry an older schema — `SuspensionSpring`, `GearRatios`,
`DrivetrainLeft` … — that no shipped `FileIO` registers; how the
parser recovers from them was not traced). All
**verified_original** from the `FileIO`/constructor bodies named in
each table.

`vpXXX_opp.vehcarsim` files exist but are never read: opponents load
the player file (RACE-13, `docs/research/opponent-ai.md`).

## `vehCarSim` (`FileIO` `0x4ccc60`, ctor `0x4cb650`)

| Token | Off | Default | Effect | Doc |
| --- | --- | --- | --- | --- |
| `Mass` | `0x244` | 2000 | body mass (kg); also scales every wheel's static load | 01, 02 |
| `InertiaBox` | `0x228` | 2 1 3 | box dimensions → `InitBoxMass` | 01 |
| `CenterOfGravity` | `0x204` | 0 0 0 | model origin = centre of mass + CoG (mass sits at −CoG); `z` splits the static wheel loads | 02, 04 |
| `BoundFriction` | `0x238` | 0.3 | body-contact friction | 01 |
| `BoundElasticity` | `0x23c` | 0.2 | body-contact restitution | 01 |
| `DrivetrainType` | `0x254` | 0 | 0 rear, 1 front, 2 all-wheel drive | 03 |
| `SSSValue` | `0x1558` | 1.0 | steering scale at/above `SSSThreshold` | 04 |
| `SSSThreshold` | `0x155c` | 0 | m/s; 0 = off (all retail cars) | 04 |
| `CarFrictionHandling` | `0x153c` | 1.0 | remaps surface friction < 1 | 02 |
| `Aero` `{}` | `0x14f0` | | | |
| `Engine` `{}` | `0x25c` | | | |
| `Trans` `{}` | `0x2e0` | | | |
| `Drivetrain` `{}` | `0x3d4` | | | |
| `Freetrain` `{}` | `0x420` | | copied to the right freetrain | |
| `WheelFront` `{}` | `0x4b8` (`whl0`) | | copied to `whl1` except `HandbrakeCoef`, `WobbleLimit` | 02 |
| `WheelBack` `{}` | `0x990` (`whl2`) | | copied to `whl3` likewise | 02 |
| `AxleFront`/`AxleBack` `{}` | `0xe68`/`0xf04` | | | |

## `vehEngine` (`FileIO` `0x4d9230`, ctor `0x4d8c00`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `AngInertia` | `0x34` | 1.0 | engine inertia; reflected `g²·I` into the drivetrain; free-rev rate |
| `MaxHorsePower` | `0x18` | 200 | peak power at `OptRPM` (×746 W) |
| `IdleRPM` | `0x1c` | 750 | zero-throttle torque crosses 0 here; clutch opens below, closes above 2× |
| `OptRPM` | `0x20` | 5000 | peak power; also anchors the gear ratios |
| `MaxRPM` | `0x24` | 8000 | torque reaches 0; hard speed limiter on the drivetrain |
| `GCL` | `0x28` | 0.25 | seconds of zero engine torque after every gear change |

## `vehTransmission` (`FileIO` `0x4cf730`, ctor `0x4cf0e0`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `ManualNumGears` | `0x50` | 7 | gears incl. R and N |
| `AutoNumGears` | `0x54` | 6 | gears incl. R and N |
| `Reverse` | `0xd8` | 20 | mph at `OptRPM` in reverse |
| `Low` | `0xdc` | 20 | mph at `OptRPM` in first |
| `High` | `0xe0` | 75 | mph at `OptRPM` in top |
| `GearBias` | `0xf0` | 0.5 | middle-gear spacing exponent bias |
| `UpshiftBias` | `0xe4` | 0.05 | upshift `(1+b)` × equal-power rpm |
| `DownshiftBiasMin` | `0xe8` | 0.05 | full-throttle downshift `(1−b)` × post-shift rpm |
| `DownshiftBiasMax` | `0xec` | 0.3 | zero-throttle downshift `(1−b)` × post-shift rpm |
| `GearChangeTime` | `0x2c` | 0.8 | minimum time in gear before the automatic shifts again |

## `vehDrivetrain` (`Drivetrain` and `Freetrain`; `FileIO` `0x4da560`, ctor `0x4d9d50`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `AngInertia` | `0x40` | 5000 | implicit spin stiffness: `Δω = dt·τ/(I + dt·AngInertia)` |
| `BrakeDynamicCoef` | `0x44` | 1.0 | brake multiplier while the train turns |
| `BrakeStaticCoef` | `0x48` | 1.2 | brake multiplier while it is stopped |

## `vehWheel` (`FileIO` `0x4d41b0`, ctor `0x4d2180`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `SuspensionExtent` | `0x88` | 0.2 | droop travel = static sag (m) |
| `SuspensionLimit` | `0x84` | 0.1 | bump travel before the bump stop (m) |
| `SuspensionFactor` | `0x8c` | 1.0 | progressivity (clamped ≥ 0.75); bump-stop force `L(1+F·Limit/Extent)` |
| `SuspensionDampCoef` | `0x90` | 0.1 | damper; true ratio ≈ 4.43× this |
| `SteeringLimit` | `0x6c` | 0.39 | lock (rad); rear wheels steer opposite |
| `SteeringOffset` | `0x80` | 0 | Ackermann gain |
| `BrakeCoef` | `0x78` | 1.0 | brake torque = coef·StaticFric·r·L |
| `HandbrakeCoef` | `0x7c` | 1.0 | as above (left wheels only — right keep 1.0) |
| `CamberLimit` | `0x70` | −1 | visual camber |
| `WobbleLimit` | `0x74` | 0 | visual wobble (left wheels only) |
| `TireDispLimitLong` | `0x58` | 0.075 | long bristle: `k = 2L/this` |
| `TireDampCoefLong` | `0x60` | 0.25 | fraction of critical (quarter mass) |
| `TireDragCoefLong` | `0x68` | 0.02 | surface drag (water only in retail) |
| `TireDispLimitLat` | `0x54` | 0.075 | lateral bristle |
| `TireDampCoefLat` | `0x5c` | 0.25 | |
| `TireDragCoefLat` | `0x64` | 0.05 | |
| `OptimumSlipPercent` | `0x250` | 0.14 | slip ratio of peak grip |
| `StaticFric` | `0x254` | 2.0 | peak μ (× surface friction) |
| `SlidingFric` | `0x258` | 1.9 | sliding μ floor |

## `vehAero` (`FileIO` `0x4d96d0`, ctor `0x4d9310`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `AngCDamp` | `0x20` | 0 0 0 | constant angular deceleration per car axis (rad/s²) |
| `AngVelDamp` | `0x2c` | 0 0 0 | linear (1/s) |
| `AngVel2Damp` | `0x38` | 0 0 0 | quadratic (1/rad) |
| `Drag` | `0x44` | 0 | `F = −Drag·|v_fwd|·v` |
| `Down` | `0x48` | 0 | `F = −Down·v_fwd²` along the car's up axis |

## `vehAxle` (`FileIO` `0x4d9c90`)

| Token | Off | Effect |
| --- | --- | --- |
| `TorqueCoef` | `0x94` | anti-roll stiffness × roll inertia (nonzero on several retail cars) |
| `DampCoef` | `0x98` | anti-roll damping |

## `vehGyro` (`FileIO` `0x4d5ed0`, ctor `0x4d5b70`; all default 0)

| Token | Off | Effect |
| --- | --- | --- |
| `Drift` | `0x1c` | yaw accel `Drift·s|s|·ω_drive` (all wheels down) |
| `Spin180` | `0x20` | handbrake yaw accel `k·s·ω_drive` rolling forward |
| `Reverse180` | `0x24` | the same rolling backward / stopped |
| `Pitch` | `0x28` | airborne pitch levelling while braking |
| `Roll` | `0x2c` | airborne roll levelling while braking |

## `vehStuck` (`FileIO` `0x4d6500`, ctor `0x4d5fa0`)

| Token | Off | Default | Effect |
| --- | --- | --- | --- |
| `Turn` | `0x40` | 1.57 | in-place yaw rate when wedged (rad/s at full steer) |
| `Rotation` | `0x44` | 0.39 | impulse-recovery twist (0 in retail → flip path) |
| `Translation` | `0x48` | 0.1 | lift on flip / impulse |
| `TimeThresh` | `0x2c` | 0.3 | seconds before acting |
| `PosThresh` | `0x30` | 1.25 | must have stayed within this (m) |
| `MoveThresh` | `0x34` | 1.75 | moving further disarms (m) |

## Retail roster

The 21 shipped player cars (`EXPECTED_STOCK_ROSTER`). `—` = token
absent (constructor default applies; `vpmoonrover` has no gyro or
stuck file at all, so its gyro does nothing).

### Body

| car | Mass | InertiaBox | CenterOfGravity | DrivetrainType | BoundFriction | BoundElasticity | CarFrictionHandling | SSSValue | SSSThreshold |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 2500 | 3 1.5 4 | 0 -0.15 0.2 | 2 | 0.3 | 0.5 | 1 | 1 | 0 |
| `vpauditt` | 1300 | 2.5 0.5 4 | 0 0 0 | 0 | 0.2 | 0.3 | — | 1 | 0 |
| `vpbug` | 1000 | 2 2 3 | 0 -0.1 0 | 1 | 0.5 | 0.5 | 1 | 1 | 0 |
| `vpbullet` | 1300 | 2.5 2 5 | 0 -0.1 0.15 | 0 | 0.3 | 0.5 | 1 | 1 | 0 |
| `vpbus` | 5000 | 4.58 2.5 10.91 | 0 -0.5 0.7 | 0 | 0.2 | 0.3 | 1 | 1 | 0 |
| `vpcab` | 1000 | 2 1.51 3 | 0 0.2 -0.4 | 0 | 0.8 | 0.3 | 1 | 1 | 0 |
| `vpcaddie` | 1300 | 2.5 2 3 | 0 -0.15 0.15 | 0 | 0.5 | 0 | 1 | 1 | 0 |
| `vpcentury` | 3500 | 3.5 2 5 | 0 -0.2 0.5 | 0 | 0.9 | 0.5 | 1 | 1 | 0 |
| `vpcoop` | 800 | 1.41 1.2 3 | 0 0 0.2 | 1 | 0.5 | 0.5 | 1 | 0 | 0 |
| `vpcoop2k` | 800 | 1.41 1.2 3 | 0 0 0.2 | 1 | 0.5 | 0.5 | 1 | 0 | 0 |
| `vpcop` | 1300 | 2 1 3 | 0 -0.1 -0.1 | 0 | 0.8 | 0.5 | 1 | 1 | 0 |
| `vpdb7` | 1573 | 2 1.3 3 | 0 0 -0.2 | 1 | 0.5 | 0.5 | 1 | 1 | 0 |
| `vpddbus` | 4915 | 3.52 2.5 10.91 | 0 -0.3 0.44 | 0 | 0.2 | 0.3 | 1 | 1 | 0 |
| `vpdune` | 1000 | 2 2 3 | 0 -0.1 0 | 1 | 0.5 | 0.5 | 1 | 1 | 0 |
| `vpford` | 2500 | 4.5 2.5 7 | 0 -0.3 0.4 | 0 | 0.331 | 0.5 | 1 | 1 | 0 |
| `vpmoonrover` | 2500 | 4.5 2.5 7 | 0 -0.3 0.4 | 2 | 0.9 | 0.5 | — | 1 | 0 |
| `vpmustang99` | 1300 | 2.5 1.1 3 | 0 -0.1 0.2 | 0 | 0.4 | 0.5 | 1 | 1 | 0 |
| `vppanoz` | 1300 | 2 1 3 | 0 0 0 | 1 | 0.9 | 0.5 | 1 | 1 | 0 |
| `vppanozgt` | 1200 | 3 2.5 4 | 0 -0.03 0.01 | 0 | 0.2 | 0.3 | 1 | 1 | 0 |
| `vpsemi` | 3500 | 4.2 4.2 4.2 | 0 -0.1 0.15 | 0 | 0.2 | 0.3 | 1 | 1 | 0 |
| `vpvwcup` | 1000 | 2 2 3 | 0 -0.1 0 | 1 | 0.5 | 0.5 | 1 | 1 | 0 |

### Engine and gearbox

| car | MaxHorsePower | IdleRPM | OptRPM | MaxRPM | AngInertia | GCL | AutoNumGears | ManualNumGears | Reverse | Low | High | GearBias | UpshiftBias | DownshiftBiasMin | DownshiftBiasMax | GearChangeTime |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 550 | 750 | 4000 | 5000 | 0.655 | 0.25 | 7 | 7 | 35 | 25 | 85 | 0 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpauditt` | 551 | 750 | 6128 | 7615 | 1 | 0.25 | 6 | 7 | 35.0001 | 20 | 120.0001 | 0.5 | 0.05 | 0.05 | 0.3 | 1 |
| `vpbug` | 260 | 750 | 5800 | 8500 | 1 | 0.25 | 6 | 7 | 30 | 20 | 90 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpbullet` | 550 | 750 | 6128 | 7615 | 1 | 0.25 | 6 | 6 | 30 | 30 | 110 | 0.5 | 0.05 | 0.05 | 0.3 | 1 |
| `vpbus` | 450 | 750 | 5000 | 8000 | 1 | 0.25 | 6 | 6 | 30 | 20 | 83 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpcab` | 300 | 750 | 5800 | 8500 | 1 | 0.25 | 5 | 6 | 23 | 20 | 95 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpcaddie` | 550 | 750 | 6128 | 7615 | 1 | 0.25 | 6 | 6 | 35.0001 | 20 | 110 | 0.5 | 0.05 | 0.05 | 0.3 | 1 |
| `vpcentury` | 750 | 750 | 5000 | 8000 | 1 | 0.25 | 8 | 8 | 20 | 35.1001 | 75 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpcoop` | 250 | 750 | 5800 | 8500 | 1 | 0.25 | 6 | 6 | 30 | 30 | 80.1 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpcoop2k` | 300 | 750 | 5800 | 8500 | 1 | 0.25 | 6 | 7 | 30 | 30 | 108 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpcop` | 750 | 750 | 6500 | 7700 | 1 | 0.1 | 6 | 7 | 35 | 30 | 140 | 0.5 | 0.05 | 0.05 | 0.3 | 0.1 |
| `vpdb7` | 550 | 750 | 7021 | 8368 | 1 | 0.25 | 7 | 8 | 35 | 40 | 150.0001 | 0.5 | 0.002 | 0.05 | 0.3 | 0.8 |
| `vpddbus` | 456 | 750 | 5000 | 8000 | 1 | 0.25 | 6 | 6 | 30 | 15 | 65 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpdune` | 400 | 750 | 5800 | 8500 | 1 | 0.25 | 6 | 7 | 30 | 40 | 106 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpford` | 550 | 750 | 4000 | 5000 | 1 | 0.25 | 6 | 7 | 35 | 25 | 85 | 0 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpmoonrover` | 550 | 750 | 4000 | 5000 | 1 | 0.25 | 6 | 7 | 35 | 25 | 85 | 0 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpmustang99` | 500 | 750 | 6507 | 7709 | 1 | 0.25 | 6 | 7 | 35 | 30 | 115 | 0.5 | 0.05 | 0.05 | 0.3 | 1 |
| `vppanoz` | 650 | 750 | 8000 | 9473 | 1 | 0.27 | 6 | 7 | 40.3 | 35.5 | 150.6999 | 0.5 | 0.05 | 0.05 | 0.3 | 0.91 |
| `vppanozgt` | 902 | 750 | 5800 | 9400 | 1 | 0.25 | 6 | 8 | 50.1 | 20 | 180 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpsemi` | 896 | 750 | 5000 | 8000 | 1 | 0.25 | 7 | 7 | 20 | 30 | 85 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |
| `vpvwcup` | 550 | 750 | 5800 | 8500 | 1 | 0.25 | 6 | 7 | 30 | 40 | 122 | 0.5 | 0.05 | 0.05 | 0.3 | 0.8 |

### Trains and aero

| car | AngInertia | AngInertia | BrakeDynamicCoef | BrakeStaticCoef | Drag | Down | AngCDamp | AngVelDamp | AngVel2Damp | TorqueCoef | TorqueCoef |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 7400 | 10000 | 1 | 1.2 | 0.47 | 0.2 | 1.88 2.74 2 | 0 0 0 | 2 0 1 | 1 | 1 |
| `vpauditt` | 2000 | 2000 | 1 | 1.2 | 0.1 | 0.59 | 1 4 3 | 0 0 0 | 0.5 5 1.5 | 0 | 0 |
| `vpbug` | 2000 | 2000 | 1 | 1.2 | 0.5 | 0 | 1.3 1.4 1 | 0 0 0 | 0.14 2 1 | 0 | 0 |
| `vpbullet` | 4460 | 2000 | 1 | 1.2 | 0.3 | 0 | 2 5 3 | 0 0 0 | 0 5 2 | 1 | 1 |
| `vpbus` | 30000 | 30000 | 1 | 1.2 | 1.96 | 0 | 0 0 0 | 6 0 0 | 6 0 0 | 0 | 0 |
| `vpcab` | 5000 | 2000 | 1 | 1.2 | 0.3 | 0 | 3.74 1 7.35 | 3.31 0 0 | 5.01 2 6.12 | 0 | 0 |
| `vpcaddie` | 4460 | 2000 | 1 | 1.2 | 0.3 | 0 | 2 5 2 | 0 0 0 | 0 5 0 | 1 | 1 |
| `vpcentury` | 30000 | 30000 | 1 | 1.2 | 0 | 0 | 6 2 6 | 3 0 0 | 2.01 0 2 | 0 | 0 |
| `vpcoop` | 3560 | 2000 | 1 | 1.2 | 0.54 | 0 | 0.5 2 1 | 0 0 0 | 0 5 0 | 0 | 0 |
| `vpcoop2k` | 3560 | 2000 | 1 | 1.2 | 0.54 | 0 | 0.5 2 1 | 0 0 0 | 0 5 0 | 0 | 0 |
| `vpcop` | 2000 | 2100 | 1 | 1.2 | 0 | 1 | 4 4 3 | 0 0 0 | 4 4 4 | 0 | 0 |
| `vpdb7` | 5060 | 1000 | 1 | 1.2 | 0 | 0 | 9 4 3 | 0 0 0 | 4 5 0 | 0 | 0 |
| `vpddbus` | 9820 | 10000 | 1 | 1.2 | 0 | 0 | 0 0 0 | 0.65 0 0 | 0 0 0 | 0 | 0 |
| `vpdune` | 2000 | 2000 | 1 | 1.2 | 0.5 | 0 | 1.3 4 1 | 0 0 0 | 0 4.01 1 | 0 | 0 |
| `vpford` | 10000 | 10000 | 1 | 1.2 | 0.47 | 0.2 | 1 2.4 0.8 | 0 0 0 | 1 0 1 | 1 | 1 |
| `vpmoonrover` | 25000 | 10000 | 1 | 1.2 | 0.47 | 0.2 | 1 2.4 0.8 | 0 0 0 | 1 0 1 | 1 | 1 |
| `vpmustang99` | 4020 | 3420 | 1 | 1.2 | 0 | 0 | 0.47 5 4 | 0 0 0 | 0.4 1.5 0.5 | 0 | 0 |
| `vppanoz` | 3530 | 2710 | 1 | 1.2 | 0 | 1 | 2 1 1 | 0 0 0 | 1 1 1 | 0 | 0 |
| `vppanozgt` | 3290 | 2000 | 2 | 1.2 | 0 | 1 | 10.01 2 0 | 0 0 6 | 5 1.7 6 | 0 | 0 |
| `vpsemi` | 30000 | 30000 | 1 | 1.2 | 3 | 0 | 0 0 0 | 6 0 6 | 6 0 6 | 1 | 1 |
| `vpvwcup` | 2000 | 2000 | 1 | 1.2 | 0.5 | 0 | 1.3 1.4 1 | 0 0 0 | 0.14 2 1 | 0 | 0 |

### Front wheels

| car | SuspensionExtent | SuspensionLimit | SuspensionFactor | SuspensionDampCoef | SteeringLimit | SteeringOffset | BrakeCoef | TireDispLimitLat | TireDampCoefLat | TireDispLimitLong | TireDampCoefLong | OptimumSlipPercent | StaticFric | SlidingFric |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 0.2 | 0.2 | 1.5 | 0.046 | 0.5 | 0.25 | 0.062 | 0.075 | 0.25 | 0.15 | 0.25 | 0.28 | 1.97 | 1.54 |
| `vpauditt` | 0.05 | 0.05 | 1 | 0.101 | 0.5 | 0.26 | 1.15 | 0.125 | 0.25 | 0.125 | 0.25 | 0.3 | 3 | 2.7 |
| `vpbug` | 0.15 | 0.05 | 1 | 0.1 | 0.4 | 0.25 | 0.625 | 0.125 | 0.25 | 0.125 | 0.25 | 0.16 | 3 | 2.7 |
| `vpbullet` | 0.2 | 0.1 | 1.3 | 0.02 | 0.4 | 0.26 | 0.4 | 0.125 | 0.25 | 0.125 | 0.25 | 0.2 | 3 | 2.7 |
| `vpbus` | 0.2 | 0.3 | 1 | 0.1 | 0.6 | 0 | 0.5 | 0.125 | 0.25 | 0.125 | 0.25 | 0.08 | 1.2 | 1 |
| `vpcab` | 0.1 | 0.1 | 0.75 | 0.01 | 0.45 | 0.25 | 0.996 | 0.125 | 0.25 | 0.125 | 0.25 | 0.4 | 2.5 | 2 |
| `vpcaddie` | 0.18 | 0.1 | 1.3 | 0.024 | 0.35 | 0.26 | 0.516 | 0.125 | 0.25 | 0.125 | 0.25 | 0.321 | 3 | 2.8 |
| `vpcentury` | 0.1 | 0.2 | 1 | 0.1 | 0.45 | 0 | 0.5 | 0.125 | 0.25 | 0.125 | 0.25 | 0.739 | 3 | 2.9 |
| `vpcoop` | 0.1 | 0.07 | 1 | 0.1 | 0.42 | 0.25 | 0.866 | 0.125 | 0.25 | 0.125 | 0.25 | 0.141 | 3.02 | 2.9 |
| `vpcoop2k` | 0.1 | 0.07 | 1 | 0.1 | 0.5 | 0.25 | 0.866 | 0.125 | 0.25 | 0.125 | 0.25 | 0.141 | 3.02 | 2.9 |
| `vpcop` | 0.1 | 0.1 | 1 | 0.01 | 0.5 | 0.25 | 0.448 | 0.125 | 0.25 | 0.125 | 0.25 | 0.2 | 3 | 2.8 |
| `vpdb7` | 0.15 | 0.1 | 1 | 0.1 | 0.4 | 0.25 | 0.429 | 0.125 | 0.25 | 0.125 | 0.25 | 0.2 | 3 | 2.9 |
| `vpddbus` | 0.2 | 0.3 | 1 | 0.1 | 0.4 | 0 | 0.5 | 0.125 | 0.25 | 0.125 | 0.25 | 0.17 | 3 | 2.9 |
| `vpdune` | 0.2 | 0.1 | 1.3 | 0.02 | 0.4 | 0.25 | 0.625 | 0.125 | 0.25 | 0.125 | 0.25 | 0.16 | 3 | 2.7 |
| `vpford` | 0.2 | 0.2 | 1.5 | 0.046 | 0.5 | 0.25 | 0.273 | 0.125 | 0.25 | 0.125 | 0.25 | 0.28 | 3 | 2.7 |
| `vpmoonrover` | 0.2 | 0.2 | 1.5 | 0.046 | 0.5 | 0.25 | 0.132 | 0.075 | 0.5 | 0.075 | 0.5 | 0.28 | 3 | 2.7 |
| `vpmustang99` | 0.13 | 0.11 | 1 | 0.1 | 0.4 | 0.25 | 0.611 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 3 | 2.98 |
| `vppanoz` | 0.1 | 0.1 | 1 | 0.1 | 0.5 | 0.25 | 0.672 | 0.125 | 0.25 | 0.125 | 0.25 | 0.18 | 3 | 2.75 |
| `vppanozgt` | 0.2 | 0.02 | 1 | 0.1 | 0.55 | 0 | 0.5 | 0.125 | 0.25 | 0.125 | 0.25 | 0.3 | 3 | 2.59 |
| `vpsemi` | 0.2 | 0.4 | 1 | 0.1 | 0.476 | 0 | 0.5 | 0.125 | 0.25 | 0.125 | 0.25 | 0.499 | 2 | 1.9 |
| `vpvwcup` | 0.15 | 0.05 | 1 | 0.1 | 0.4 | 0.25 | 0.625 | 0.125 | 0.25 | 0.125 | 0.25 | 0.16 | 3 | 2.7 |

### Rear wheels

| car | SuspensionExtent | SuspensionLimit | SuspensionFactor | SuspensionDampCoef | SteeringLimit | BrakeCoef | HandbrakeCoef | TireDispLimitLat | TireDampCoefLat | TireDispLimitLong | TireDampCoefLong | OptimumSlipPercent | StaticFric | SlidingFric |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 0.23 | 0.23 | 1.5 | 0.021 | 0 | 0.4 | 2 | 0.075 | 0.25 | 0.075 | 0.25 | 0.129 | 2.04 | 1.59 |
| `vpauditt` | 0.05 | 0.05 | 1 | 0.097 | 0.04 | 1.15 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.11 | 3 | 1.5 |
| `vpbug` | 0.15 | 0.05 | 1 | 0.1 | 0.03 | 0.606 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.13 | 3 | 2.8 |
| `vpbullet` | 0.2 | 0.1 | 1.3 | 0.03 | 0 | 0.4 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.057 | 3 | 1.4 |
| `vpbus` | 0.2 | 0.1 | 1 | 0.1 | 0 | 0.5 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 2 | 1.9 |
| `vpcab` | 0.1 | 0.05 | 0.75 | 0.01 | 0 | 0.762 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 2.5 | 2 |
| `vpcaddie` | 0.18 | 0.1 | 1.31 | 0.032 | 0 | 1.763 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.057 | 3 | 1.7 |
| `vpcentury` | 0.2 | 0.1 | 1 | 0.1 | 0 | 0.5 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.484 | 3 | 2.9 |
| `vpcoop` | 0.1 | 0.07 | 1 | 0.1 | 0 | 0.788 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 3.04 | 2.9 |
| `vpcoop2k` | 0.1 | 0.07 | 1 | 0.1 | 0 | 0.788 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 3.04 | 2.9 |
| `vpcop` | 0.1 | 0.1 | 1 | 0.01 | 0 | 0.499 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.2 | 3 | 2.8 |
| `vpdb7` | 0.15 | 0.1 | 1 | 0.1 | 0 | 0.422 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.08 | 3 | 1.6 |
| `vpddbus` | 0.2 | 0.1 | 1 | 0.1 | 0 | 0.5 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.14 | 3 | 2.9 |
| `vpdune` | 0.2 | 0.1 | 1.3 | 0.03 | 0.03 | 0.606 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.093 | 3 | 1.9 |
| `vpford` | 0.33 | 0.33 | 1.5 | 0.021 | 0 | 0.5 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.16 | 3 | 2.7 |
| `vpmoonrover` | 0.33 | 0.33 | 1.5 | 0.021 | 0 | 0.5 | 1 | 0.075 | 0.5 | 0.075 | 0.5 | 0.16 | 3 | 2.7 |
| `vpmustang99` | 0.13 | 0.1 | 1 | 0.1 | 0 | 0.649 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.15 | 3 | 2.6 |
| `vppanoz` | 0.1 | 0.1 | 1 | 0.1 | 0 | 0.61 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.18 | 3 | 2.4 |
| `vppanozgt` | 0.2 | 0.1 | 1 | 0.1 | 0 | 0.5 | 1 | 0.125 | 0.25 | 0.125 | 0.25 | 0.2 | 3 | 1.7 |
| `vpsemi` | 0.1 | 0.1 | 1 | 0.1 | 0 | 0.5 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.421 | 2 | 1.9 |
| `vpvwcup` | 0.15 | 0.05 | 1 | 0.1 | 0.03 | 0.606 | 2 | 0.125 | 0.25 | 0.125 | 0.25 | 0.13 | 3 | 2.8 |

### Gyro and stuck

| car | Drift | Spin180 | Reverse180 | Pitch | Roll | Turn | Rotation | Translation | TimeThresh | PosThresh | MoveThresh |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 0.12 | 0.4 | 0.7 | 0 | 0 | 3.1416 | 0 | 0.1 | 0.9998 | 1.25 | 1.75 |
| `vpauditt` | 0.2 | 0.3 | 3 | 0 | 0 | 1.57 | 0 | 0.1 | 2 | 1.25 | 1.75 |
| `vpbug` | 0.2 | 0.8 | 2.617 | — | — | 1.57 | 0 | 0.1 | 2 | 1.25 | 1.75 |
| `vpbullet` | 0.2 | 0.85 | 4 | 0 | 0 | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpbus` | 0.025 | 0.408 | 0.65 | 0 | 0 | 2.0646 | 0 | 0.1 | 0.9998 | 1.25 | 1.75 |
| `vpcab` | 0.15 | 0.682 | 2.964 | — | — | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpcaddie` | 0.18 | 0.56 | 3.2 | 0 | 0 | 1.57 | 0 | 0.1 | 2 | 1.25 | 1.75 |
| `vpcentury` | 0.05 | 0.15 | 2 | 0 | 0 | 2 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpcoop` | 0.2 | 0.939 | 6.141 | 0 | 0 | 1.57 | 0 | 0.164 | 2 | 1.25 | 1.75 |
| `vpcoop2k` | 0.727 | 1.608 | 4.134 | 0 | 0 | 1.57 | 0 | 0.1 | 2 | 1.25 | 1.75 |
| `vpcop` | 0.14 | 1.5 | 4 | 0 | 0 | 1.57 | 0 | 0.1 | 2 | 1.25 | 1.75 |
| `vpdb7` | 0.2 | 0.3 | 3 | 0 | 0 | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpddbus` | 0.04 | 1 | 1 | 0 | 0 | 0.74 | 0 | 0.1 | 1.1714 | 1.25 | 1.75 |
| `vpdune` | 0.25 | 0.514 | 1.998 | 0 | 0 | 3.1416 | 0 | 0.1 | 1.0004 | 1.25 | 1.75 |
| `vpford` | 0.15 | 0.15 | 0.65 | — | — | 3.0986 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpmoonrover` | — | — | — | — | — | — | — | — | — | — | — |
| `vpmustang99` | 0.2 | 0.452 | 1.8 | 0 | 0 | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vppanoz` | 0.2 | 0.6 | 1.6 | 0 | 0 | 3.1416 | 0 | 0.1 | 0.9999 | 1.25 | 1.75 |
| `vppanozgt` | 0.03 | 0.5 | 0.073 | 0 | 0 | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpsemi` | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0.1 | 1 | 1.25 | 1.75 |
| `vpvwcup` | 0.2 | 0.8 | 2.617 | 0 | 0 | 3.1416 | 0 | 0.1 | 1 | 1.25 | 1.75 |

## Derived numbers

Computed from the formulas in 02–04 with each car's real wheel pivots
(`mm2-inspect car`) — what the authored values *mean* in the original
solver. `L` is the static preload per wheel; `Σ preload / weight` is
`2(Lfront + Lrear)/(m·19.6)`, which is 1 whenever `CenterOfGravity.z`
lies inside a symmetric wheelbase. The **Moon Rover** is the exception:
its `CoG.z = 0.4` on a 0.86 m wheelbase gives 2.25× its weight in
preload, so it rides on extended springs (02 § Static load). Suspension
frequency and damping ratio are at 19.6 m/s² for the quarter mass
`L/19.6`. "Grip" is `19.6·μ` on tarmac (`friction 0.9`) — the peak
horizontal acceleration the tyres can sustain, quoted in real g for
comparison. The two yaw columns are the gyro's yaw *acceleration* at
full steer with the driven wheels rolling at the given speed
(04 § vehGyro); `ratio` is the overall gear ratio from `Low`/`High`.

| car | L front / rear (N) | Σ preload / weight | ks front (N/m) | f susp front (Hz) | ζ front | μ tarmac F / R | grip ≈ μ·19.6 | kLat front (N/m) | Drift yaw @30 m/s | Spin180 yaw @20 m/s | ratio 1st / top |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `vp4x4` | 13458 / 11021 | 1.00 | 84111 | 1.76 | 0.20 | 1.77 / 1.84 | 35.4 m/s² (3.6 g) | 358874 | 5.8 rad/s² | 12.9 rad/s² | 23.2 / 6.83 |
| `vpauditt` | 6370 / 6370 | 1.00 | 127400 | 3.15 | 0.45 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 101920 | 18.2 rad/s² | 18.2 rad/s² | 23.7 / 3.95 |
| `vpbug` | 4900 / 4900 | 1.00 | 32667 | 1.82 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 78400 | 17.9 rad/s² | 47.6 rad/s² | 22.8 / 5.07 |
| `vpbullet` | 6997 / 5736 | 1.00 | 41979 | 1.73 | 0.09 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 111945 | 16.4 rad/s² | 46.6 rad/s² | 17.5 / 4.76 |
| `vpbus` | 29492 / 20130 | 1.01 | 147458 | 1.58 | 0.44 | 1.08 / 1.80 | 28.2 m/s² (2.9 g) | 471866 | 1.2 rad/s² | 12.8 rad/s² | 37.5 / 9.03 |
| `vpcab` | 3537 / 6263 | 1.00 | 30945 | 2.08 | 0.04 | 2.25 / 2.25 | 44.1 m/s² (4.5 g) | 56585 | 11.9 rad/s² | 36.1 rad/s² | 25.7 / 5.41 |
| `vpcaddie` | 7034 / 5706 | 1.00 | 46613 | 1.81 | 0.11 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 112540 | 15.7 rad/s² | 32.7 rad/s² | 24.6 / 4.48 |
| `vpcentury` | 19765 / 12881 | 0.95 | 197648 | 2.23 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 316237 | 2.8 rad/s² | 5.7 rad/s² | 17.6 / 8.23 |
| `vpcoop` | 4687 / 3168 | 1.00 | 46865 | 2.23 | 0.44 | 2.72 / 2.74 | 53.4 m/s² (5.4 g) | 74984 | 22.5 rad/s² | 70.3 rad/s² | 12.1 / 4.53 |
| `vpcoop2k` | 4629 / 3168 | 0.99 | 46288 | 2.23 | 0.44 | 2.72 / 2.74 | 53.4 m/s² (5.4 g) | 74061 | 81.7 rad/s² | 120.4 rad/s² | 12.1 / 3.36 |
| `vpcop` | 5918 / 6822 | 1.00 | 59182 | 2.23 | 0.04 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 94691 | 12.1 rad/s² | 86.7 rad/s² | 17.6 / 3.76 |
| `vpdb7` | 6573 / 8861 | 1.00 | 43818 | 1.82 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 105163 | 16.5 rad/s² | 16.5 rad/s² | 14.9 / 3.98 |
| `vpddbus` | 28168 / 19999 | 1.00 | 140840 | 1.58 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 450688 | 2.3 rad/s² | 38.2 rad/s² | 40.8 / 9.42 |
| `vpdune` | 4900 / 4900 | 1.00 | 29400 | 1.73 | 0.09 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 78400 | 20.1 rad/s² | 27.6 rad/s² | 12.7 / 4.78 |
| `vpford` | 14436 / 10015 | 1.00 | 90227 | 1.76 | 0.20 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 230982 | 9.2 rad/s² | 6.2 rad/s² | 18.3 / 5.37 |
| `vpmoonrover` | 18696 / 36320 | 2.25 | 116848 | 1.76 | 0.20 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 498553 | 0.0 rad/s² | 0.0 rad/s² | 14.6 / 4.30 |
| `vpmustang99` | 7277 / 5477 | 1.00 | 55978 | 1.95 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 116435 | 17.3 rad/s² | 26.1 rad/s² | 17.6 / 4.59 |
| `vppanoz` | 6370 / 6370 | 1.00 | 63700 | 2.23 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 101920 | 17.6 rad/s² | 35.2 rad/s² | 18.0 / 4.24 |
| `vppanozgt` | 5923 / 5836 | 1.00 | 29615 | 1.58 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 94769 | 2.7 rad/s² | 29.7 rad/s² | 22.9 / 2.54 |
| `vpsemi` | 18269 / 16031 | 1.00 | 91347 | 1.58 | 0.44 | 1.80 / 1.80 | 35.3 m/s² (3.6 g) | 292311 | 0.0 rad/s² | 0.0 rad/s² | 18.2 / 6.41 |
| `vpvwcup` | 4900 / 4900 | 1.00 | 32667 | 1.82 | 0.44 | 2.70 / 2.70 | 52.9 m/s² (5.4 g) | 78400 | 19.7 rad/s² | 52.5 rad/s² | 10.4 / 3.40 |


Reading the table: every car has roughly 5 g of real-world grip
(3.6 g for the 4×4 and the semi) because `μ ≈ 2.7` acts on a load
computed at 2 g; suspensions sit at 1.6–3.2 Hz with ζ ≈ 0.44 on most
cars but 0.04 on the London Cab and the police car (their
`SuspensionDampCoef` is 0.01); and the gyro's yaw push is in the tens
of rad/s² — comparable to or larger than what the tyres produce.
