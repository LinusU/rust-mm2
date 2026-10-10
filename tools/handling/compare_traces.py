#!/usr/bin/env python3
"""Compare measured original-game JSON with handling_trace CSV in the initial car frame."""
import argparse
import csv
import json
import math
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("original", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--first-frame", type=int, default=1,
                        help="First original frame included in comparison")
    parser.add_argument("--last-frame", type=int, help="Exclude contaminated frames after this original frame")
    parser.add_argument("--native-offset", type=int, default=-1,
                        help="Native row index minus original row index; native row0 is post-step")
    args = parser.parse_args()
    original = json.loads(args.original.read_text())
    old = original["rows"]
    with args.native.open() as source:
        new = list(csv.DictReader(source))
    back = old[0]["forward"]  # Original instrument records AGE row2 (+Z/back).
    length = math.hypot(back[0], back[2])
    back = [back[0] / length, 0.0, back[2] / length]
    right = [back[2], 0.0, -back[0]]
    origin = old[0]["pos"]
    initial_yaw = math.atan2(back[0], back[2])
    errors = {key: [] for key in ("speed_mps", "lateral_speed_mps", "slip_angle_radians",
                                "position_metres", "height_metres", "yaw_radians",
                                "yaw_rate_radians_per_second", "rpm")}
    frames = []
    # Retail telemetry records applied engine/brake inputs. The native
    # instrument records device inputs, before automatic pedal swapping
    # or the stopped-car handbrake hold. These are directly comparable
    # only for the full-throttle powerslide, which activates neither rule.
    compare_pedals = original["scenario"] == "powerslide" and "throttle" in new[0]
    input_mismatches = {"steering_byte": 0,
                        **{key: 0 if compare_pedals else None
                           for key in ("throttle_byte", "brake_byte", "handbrake_byte")}}
    slide_samples = {"original": [], "native": []}
    for row in old:
        frame = row["n"]
        index = frame + args.native_offset
        if (frame < args.first_frame or index < 0 or index >= len(new)
                or (args.last_frame is not None and frame > args.last_frame)):
            continue
        native = new[index]
        delta = [p - o for p, o in zip(row["pos"], origin)]
        x = sum(a * b for a, b in zip(delta, right))
        z = sum(a * b for a, b in zip(delta, back))
        yaw = math.atan2(row["forward"][0], row["forward"][2]) - initial_yaw
        yaw_error = (float(native["yaw"]) - yaw + math.pi) % (2 * math.pi) - math.pi
        # Retail Speed is unsigned; the body velocity determines reverse sign.
        signed_speed = -sum(a * b for a, b in zip(row["vel"], row["forward"]))
        body_back = row["forward"]
        horizontal_length = math.hypot(body_back[0], body_back[2])
        original_lateral = (row["vel"][0] * body_back[2]
                            - row["vel"][2] * body_back[0]) / horizontal_length
        native_yaw = float(native["yaw"])
        native_lateral = (float(native["vx"]) * math.cos(native_yaw)
                          - float(native["vz"]) * math.sin(native_yaw))
        native_forward = (-float(native["vx"]) * math.sin(native_yaw)
                          - float(native["vz"]) * math.cos(native_yaw))
        original_forward = -(row["vel"][0] * body_back[0]
                             + row["vel"][2] * body_back[2]) / horizontal_length
        original_slip = math.atan2(original_lateral, original_forward)
        native_slip = math.atan2(native_lateral, native_forward)
        errors["lateral_speed_mps"].append(native_lateral - original_lateral)
        # Near standstill the heading of velocity is undefined. Use the same
        # measured horizontal-speed gate for both sides rather than hide a skid.
        if min(math.hypot(original_forward, original_lateral),
               math.hypot(native_forward, native_lateral)) >= 1.0:
            errors["slip_angle_radians"].append(
                (native_slip - original_slip + math.pi) % (2 * math.pi) - math.pi)
        for source, longitudinal, lateral, slip, yaw_rate in (
                ("original", original_forward, original_lateral, original_slip, row["omega"][1]),
                ("native", native_forward, native_lateral, native_slip, float(native["yaw_rate"]))):
            slide_samples[source].append((frame, longitudinal, lateral, slip, yaw_rate))
        errors["speed_mps"].append(float(native["speed"]) - signed_speed)
        errors["position_metres"].append(math.hypot(float(native["x"]) - x, float(native["z"]) - z))
        errors["height_metres"].append(float(native["y"]) - delta[1])
        errors["yaw_radians"].append(yaw_error)
        errors["yaw_rate_radians_per_second"].append(float(native["yaw_rate"]) - row["omega"][1])
        errors["rpm"].append(float(native["rpm"]) - row["rpm"])
        input_mismatches["steering_byte"] += round(float(native["steer"]) * 127) != round(row["steer"] * 127)
        for key, old_key in (("throttle", "throttle"), ("brake", "brake"), ("handbrake", "hb")):
            if compare_pedals:
                input_mismatches[key + "_byte"] += round(float(native[key]) * 255) != round(row[old_key] * 255)
        frames.append(frame)
    if not frames:
        raise SystemExit("no overlapping frames")
    result = {
        "original_file": str(args.original), "native_file": str(args.native),
        "car": original["car"], "scenario": original["scenario"],
        "hz": original["hz"], "frames": len(frames),
        "frame_range": [frames[0], frames[-1]], "native_offset": args.native_offset,
        "input_byte_mismatches": input_mismatches,
        "errors": {key: {"samples": len(values),
                          "rms": math.sqrt(sum(x*x for x in values) / len(values)),
                          "max_absolute": max(abs(x) for x in values),
                          "mean": sum(values) / len(values)}
                   for key, values in errors.items() if values},
        "interpretation": "Measured differences; inspect terrain/contact/input contamination before changing tuning",
    }
    if original["scenario"] == "powerslide":
        # Input windows are defined in integer frames by both capture drivers.
        # A powered slide requires at least 15 degrees of sideways motion at
        # 5m/s; report observations rather than assuming the manoeuvre succeeds.
        phases = {"load_up": (300, 330), "handbrake_flick": (330, 348),
                  "powered_sustain": (348, 375), "countersteer": (375, 435),
                  "recovery": (435, 900)}
        result["powerslide"] = {}
        for source, samples in slide_samples.items():
            result["powerslide"][source] = {}
            for phase, (first, last) in phases.items():
                # Row n in the original observes inputs through n-1.
                phase_samples = [s for s in samples if first < s[0] <= last]
                if not phase_samples:
                    continue
                sliding = [s for s in phase_samples
                           if math.hypot(s[1], s[2]) >= 5.0
                           and abs(s[3]) >= math.radians(15.0)]
                result["powerslide"][source][phase] = {
                    "frames": len(phase_samples),
                    "sliding_seconds": len(sliding) / original["hz"],
                    "peak_abs_lateral_speed_mps": max(abs(s[2]) for s in phase_samples),
                    "peak_abs_slip_degrees": max(abs(math.degrees(s[3])) for s in phase_samples),
                    "last_slip_degrees": math.degrees(phase_samples[-1][3]),
                    "last_forward_speed_mps": phase_samples[-1][1],
                    "peak_abs_yaw_rate_radians_per_second": max(abs(s[4]) for s in phase_samples),
                    "last_yaw_rate_radians_per_second": phase_samples[-1][4],
                }
            recovery = [s for s in samples if s[0] > 435]
            recovered_frame = next((window[0][0]
                                    for i in range(max(0, len(recovery) - 29))
                                    if len(window := recovery[i:i + 30]) == 30
                                    and all(abs(s[3]) < math.radians(5)
                                            and math.hypot(s[1], s[2]) >= 5
                                            for s in window)), None)
            result["powerslide"][source]["stable_recovery"] = {
                "criterion": "30 consecutive samples with |body slip|<5deg at speed>=5m/s after neutral steering",
                "first_frame": recovered_frame,
                "seconds_after_neutral": (None if recovered_frame is None
                                          else (recovered_frame - 435) / original["hz"]),
            }
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
