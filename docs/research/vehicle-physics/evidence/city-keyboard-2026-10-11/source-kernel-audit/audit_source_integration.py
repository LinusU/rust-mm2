#!/usr/bin/env python3
"""Audit grounded source-to-source integration using captured original states.

This dependency-free diagnostic uses Python double arithmetic. It reconstructs
suspension Jacobians and feeds ORIGINAL forces/states into the recovered coupled
solve; it does not simulate or establish equivalence of a native trajectory.
"""

import argparse
import gzip
import json
import math
from pathlib import Path
import sys


SCOPE = (
    "Python double arithmetic over captured original source-to-source states; "
    "reconstructed grounded suspension Jacobians, authored mass/inertia/gravity, "
    "and original force/torque inputs. Not a bit-exact x87/native comparison, "
    "orientation audit, closed-loop replay or native trajectory equivalence."
)


def read_json(path):
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt", encoding="utf-8") as stream:
        return json.load(stream)


def finite_number(value, label):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label}: expected a finite number")
    if not math.isfinite(value):
        raise ValueError(f"{label}: expected a finite number")
    return value


def vector(value, label):
    if not isinstance(value, list) or len(value) != 3:
        raise ValueError(f"{label}: expected a three-component vector")
    return [finite_number(v, f"{label}[{i}]") for i, v in enumerate(value)]


def validate(source, provenance):
    """Reject states outside this audit's grounded, no-impulse/no-bump scope."""
    for key in ("car", "city"):
        if not source.get(key) or source[key] != provenance.get(key):
            raise ValueError(f"source/provenance {key} identity mismatch")
    if source.get("complete") is not True:
        raise ValueError("source capture is incomplete")
    if source.get("car_identity_verified") is not True:
        raise ValueError("source car identity was not verified")
    if source.get("hz") != 60 or provenance.get("hz") != 60:
        raise ValueError("this audit requires source/provenance hz=60")

    config = provenance["vehicle_config"]
    mass = finite_number(config["mass"], "vehicle_config.mass")
    inertia = vector(config["inertia"], "vehicle_config.inertia")
    original = config.get("original")
    if not isinstance(original, dict) or len(original.get("wheels", [])) != 4:
        raise ValueError("expected an authored four-wheel original configuration")
    gravity = finite_number(original["gravity"], "original.gravity")
    if mass <= 0 or gravity <= 0 or any(v <= 0 for v in inertia):
        raise ValueError("mass, diagonal inertia and gravity must be positive")

    rows = source["rows"]
    if len(rows) < 2 or len(rows) != source["integration_frames"] + 1:
        raise ValueError("source rows do not cover the complete integration interval")
    dt = finite_number(rows[0]["dt"], "dt")
    if not math.isclose(dt, 1 / 60, rel_tol=0, abs_tol=2e-9):
        raise ValueError("source dt does not match the recorded 60 Hz clock")
    for index, row in enumerate(rows):
        label = f"source row {index}"
        if row["n"] != index:
            raise ValueError(f"{label}: nonsequential sample number")
        if finite_number(row["dt"], label + ".dt") != dt:
            raise ValueError(f"{label}: varying dt is outside this audit")
        if row["ground"] != [1, 1, 1, 1]:
            raise ValueError(f"{label}: airborne wheel is outside this audit")
        for key in ("pos", "vel", "omega", "right", "up", "forward"):
            vector(row[key], label + "." + key)
        # Historical 'forward' is AGE BACK, not the physical forward direction.
        axes = [row["right"], row["up"], row["forward"]]
        for i in range(3):
            for j in range(3):
                expected = 1 if i == j else 0
                if abs(sum(a * b for a, b in zip(axes[i], axes[j])) - expected) > 0.002:
                    raise ValueError(f"{label}: malformed body basis")
        ics = row["diagnostics"]["ics"]
        for key in ("force", "force2", "torque", "torque2", "impulse",
                    "angularImpulse", "netPush", "netTurn", "totalPush",
                    "lastTotalPush"):
            values = vector(ics[key], label + ".ics." + key)
            if key not in ("force", "force2", "torque", "torque2") and any(values):
                raise ValueError(f"{label}: nonzero {key}; collision/push scope rejected")
        wheels = row["diagnostics"]["wheels"]
        if len(wheels) != 4:
            raise ValueError(f"{label}: expected four wheel diagnostics")
        for wheel_index, wheel in enumerate(wheels):
            wl = f"{label}.wheel[{wheel_index}]"
            normal = vector(wheel["probeNormal"], wl + ".probeNormal")
            vector(wheel["contactPoint"], wl + ".contactPoint")
            if abs(sum(v * v for v in normal) - 1) > 0.002:
                raise ValueError(f"{wl}: malformed contact normal")
            if finite_number(wheel["D"], wl + ".D") < 0:
                raise ValueError(f"{wl}: negative suspension Jacobian coefficient")
            load = finite_number(wheel["N"], wl + ".N")
            force = finite_number(wheel["Fs"], wl + ".Fs")
            if wheel["bottomedOut"] != 0 or load != force:
                raise ValueError(f"{wl}: bump-stop state is outside this audit")
    return rows, mass, inertia, gravity, dt


def add(a, b):
    return [x + y for x, y in zip(a, b)]


def scale(a, coefficient):
    return [x * coefficient for x in a]


def transpose(a):
    return [list(column) for column in zip(*a)]


def multiply(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3))
             for j in range(3)] for i in range(3)]


def matrix_vector(a, b):
    return [sum(a[i][k] * b[k] for k in range(3)) for i in range(3)]


def matrix_add(a, b):
    return [add(x, y) for x, y in zip(a, b)]


def matrix_scale(a, coefficient):
    return [scale(row, coefficient) for row in a]


def zero_matrix():
    return [[0.0] * 3 for _ in range(3)]


def solve(a, b):
    """Pivoted 3x3 Gaussian elimination; no NumPy or native solver dependency."""
    augmented = [row[:] + [value] for row, value in zip(a, b)]
    for i in range(3):
        pivot = max(range(i, 3), key=lambda j: abs(augmented[j][i]))
        augmented[i], augmented[pivot] = augmented[pivot], augmented[i]
        divisor = augmented[i][i]
        if not math.isfinite(divisor) or abs(divisor) < 1e-20:
            raise ValueError("singular or nonfinite coupled integration matrix")
        augmented[i] = [x / divisor for x in augmented[i]]
        for j in range(3):
            if j != i:
                coefficient = augmented[j][i]
                augmented[j] = [x - coefficient * y
                                for x, y in zip(augmented[j], augmented[i])]
    return [row[-1] for row in augmented]


def transition(current, following, mass, inertia, gravity, dt):
    a, b, c = zero_matrix(), zero_matrix(), zero_matrix()
    for wheel in current["diagnostics"]["wheels"]:
        normal = wheel["probeNormal"]
        x, y, z = [p - q for p, q in zip(wheel["contactPoint"], current["pos"])]
        cross = [[0, -z, y], [z, 0, -x], [-y, x, 0]]
        jacobian = [[wheel["D"] * normal[i] * normal[j]
                     for j in range(3)] for i in range(3)]
        angular_jacobian = multiply(cross, jacobian)
        a = matrix_add(a, jacobian)
        b = matrix_add(b, angular_jacobian)
        c = matrix_add(c, multiply(angular_jacobian, transpose(cross)))

    rotation = transpose([current["right"], current["up"], current["forward"]])
    world_inertia = multiply(
        [[rotation[i][j] * inertia[j] for j in range(3)] for i in range(3)],
        transpose(rotation),
    )
    identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    linear = matrix_add(identity, matrix_scale(a, dt / mass))
    linear_inverse = transpose([solve(linear, column) for column in identity])
    ics = current["diagnostics"]["ics"]
    force = add(ics["force"], ics["force2"])
    force[1] -= mass * gravity
    momentum_delta = scale(force, dt)
    torque = add(ics["torque"], ics["torque2"])
    angular = matrix_add(
        matrix_add(world_inertia, matrix_scale(c, dt)),
        matrix_scale(multiply(multiply(b, linear_inverse), transpose(b)),
                     -dt * dt / mass),
    )
    rhs = add(scale(torque, dt),
              scale(matrix_vector(b, matrix_vector(linear_inverse, momentum_delta)),
                    -dt / mass))
    angular_delta = solve(angular, rhs)
    # Recovered positive-sign back-substitution, using the unchanged AGE BACK axis.
    velocity_delta = scale(matrix_vector(
        linear_inverse,
        add(momentum_delta, scale(matrix_vector(transpose(b), angular_delta), dt)),
    ), 1 / mass)
    return {
        "n": following["n"],
        "velocity_error_m_s": [v + dv - expected for v, dv, expected in
                               zip(current["vel"], velocity_delta, following["vel"])],
        "angular_velocity_error_rad_s": [v + dv - expected for v, dv, expected in
                                         zip(current["omega"], angular_delta,
                                             following["omega"])],
        "position_update_error_m": [p + dt * v - expected for p, v, expected in
                                    zip(current["pos"], following["vel"],
                                        following["pos"])],
    }


def summarize(errors, key):
    return {
        "rms_per_axis": [math.sqrt(sum(row[key][i] ** 2 for row in errors) / len(errors))
                         for i in range(3)],
        "max_abs_per_axis": [max(abs(row[key][i]) for row in errors) for i in range(3)],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("original_capture", type=Path)
    parser.add_argument("native_provenance", type=Path)
    parser.add_argument("--output", type=Path,
                        help="write summary and all per-transition residuals as JSON")
    args = parser.parse_args()
    try:
        source = read_json(args.original_capture)
        provenance = read_json(args.native_provenance)
        rows, mass, inertia, gravity, dt = validate(source, provenance)
        errors = [transition(a, b, mass, inertia, gravity, dt)
                  for a, b in zip(rows, rows[1:])]
        keys = ("velocity_error_m_s", "angular_velocity_error_rad_s",
                "position_update_error_m")
        summary = {
            "scope": SCOPE,
            "car": source["car"], "city": source["city"],
            "transitions": len(errors), "dt_seconds": dt,
            "inputs": {"original_capture": str(args.original_capture),
                       "native_provenance": str(args.native_provenance)},
            "authored_constants": {"mass": mass, "inertia": inertia,
                                   "gravity": gravity},
            "guards": "complete 60 Hz, matching car/city, four grounded wheels, "
                      "constant dt, no original body impulses/pushes/bump-stop state",
            "errors": {key: summarize(errors, key) for key in keys},
        }
        if args.output:
            args.output.write_text(json.dumps({**summary, "frames": errors},
                                              indent=2, allow_nan=False) + "\n")
        print(json.dumps(summary, indent=2, allow_nan=False))
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f"audit rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
