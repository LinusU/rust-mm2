#!/usr/bin/env python3
"""Compare measured original-game JSON with handling_trace CSV in the initial car frame."""
import argparse
import csv
import json
import math
from pathlib import Path


def summarize(values):
    return {"samples": len(values),
            "rms": math.sqrt(sum(x*x for x in values) / len(values)),
            "max_absolute": max(abs(x) for x in values),
            "mean": sum(values) / len(values)}


def transitions(samples):
    return [{"frame": current[0], "from": previous[1], "to": current[1]}
            for previous, current in zip(samples, samples[1:])
            if previous[1] != current[1]]


def transition_timing(original, native, hz):
    # Match equal transitions in chronological order. Different decisions
    # remain unmatched rather than treating a different gear/sign as a delay.
    pending = list(native)
    pairs = []
    unmatched = []
    for event in original:
        index = next((i for i, candidate in enumerate(pending)
                      if (candidate["from"], candidate["to"]) ==
                         (event["from"], event["to"])), None)
        if index is None:
            unmatched.append(event)
            continue
        match = pending.pop(index)
        difference = match["frame"] - event["frame"]
        pairs.append({"from": event["from"], "to": event["to"],
                      "original_frame": event["frame"], "native_frame": match["frame"],
                      "native_minus_original_frames": difference,
                      "native_minus_original_seconds": difference / hz})
    return {"original": original, "native": native, "matched": pairs,
            "unmatched_original": unmatched, "unmatched_native": pending}


def contact_summary(samples):
    if not samples:
        return {"samples": 0, "available": False}
    divergence = [s for s in samples if s[1] != s[2]]
    result = {"samples": len(samples), "available": True,
            "scope": "grounded wheel counts; individual flags compared separately when available",
            "divergent_frames": len(divergence),
            "first_divergent_frame": divergence[0][0] if divergence else None,
            "first_divergent_counts": ({"original": divergence[0][1], "native": divergence[0][2]}
                                       if divergence else None),
            "original_any_wheel_unloaded_frames": sum(s[1] < 4 for s in samples),
            "native_any_wheel_unloaded_frames": sum(s[2] < 4 for s in samples),
            "original_all_wheels_airborne_frames": sum(s[1] == 0 for s in samples),
            "native_all_wheels_airborne_frames": sum(s[2] == 0 for s in samples),
            "max_absolute_count_difference": max(abs(s[2] - s[1]) for s in samples)}
    individual = [s for s in samples if len(s) > 3 and s[3] is not None]
    mismatch = [s for s in individual if s[3] != s[4]]
    result["individual_wheels"] = {
        "available": bool(individual), "samples": len(individual),
        "divergent_frames": len(mismatch),
        "first_divergent_frame": mismatch[0][0] if mismatch else None,
        "per_wheel_divergent_frames": [sum(s[3][i] != s[4][i] for s in individual)
                                       for i in range(4)] if individual else None,
    }
    return result


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def unit(vector):
    length = math.sqrt(dot(vector, vector))
    if not math.isfinite(length) or length < 1e-8:
        return None
    return [x / length for x in vector]


def basis(right, up):
    right = unit(right)
    if right is None:
        return None
    projection = dot(up, right)
    up = unit([u - projection * r for u, r in zip(up, right)])
    if up is None:
        return None
    back = [right[1] * up[2] - right[2] * up[1],
            right[2] * up[0] - right[0] * up[2],
            right[0] * up[1] - right[1] * up[0]]
    return right, up, back


def orientation_summary(samples):
    keys = ("rotation_radians", "pitch_radians", "roll_radians")
    return {"available": bool(samples), "samples": len(samples),
            "errors": {key: summarize(values)
                       for index, key in enumerate(keys, 1)
                       if (values := [sample[index] for sample in samples if sample[index] is not None])},
            "interpretation": "Full basis rotation error remains valid through rollover; roll excluded near vertical pitch"}


def recovery_summary(samples, neutral_frame, hz):
    recovery = [s for s in samples if s[0] > neutral_frame]
    recovered_frame = next((window[0][0]
                            for i in range(max(0, len(recovery) - 29))
                            if len(window := recovery[i:i + 30]) == 30
                            and all(window[j][0] == window[0][0] + j for j in range(30))
                            and all(abs(s[3]) < math.radians(5)
                                    and math.hypot(s[1], s[2]) >= 5
                                    for s in window)), None)
    return {"criterion": "30 consecutive samples with |body slip|<5deg at speed>=5m/s after neutral steering",
            "first_frame": recovered_frame,
            "seconds_after_neutral": (None if recovered_frame is None
                                      else (recovered_frame - neutral_frame) / hz)}


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
    frame_errors = {key: [] for key in errors}
    controls = {source: {"steering_sign": [], "gear": []} for source in ("original", "native")}
    contacts = []
    orientations = []
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
        error_lengths = {key: len(values) for key, values in errors.items()}
        delta = [p - o for p, o in zip(row["pos"], origin)]
        x = sum(a * b for a, b in zip(delta, right))
        z = sum(a * b for a, b in zip(delta, back))
        yaw = math.atan2(row["forward"][0], row["forward"][2]) - initial_yaw
        yaw_error = (float(native["yaw"]) - yaw + math.pi) % (2 * math.pi) - math.pi
        orientation_columns = [f"{axis}_{component}" for axis in ("up", "right")
                               for component in ("x", "y", "z")]
        if ("up" in row and "right" in row
                and all(native.get(column) not in (None, "") for column in orientation_columns)):
            in_initial_frame = lambda vector: [dot(vector, right), vector[1], dot(vector, back)]
            old_basis = basis(in_initial_frame(row["right"]), in_initial_frame(row["up"]))
            new_basis = basis([float(native[f"right_{axis}"]) for axis in ("x", "y", "z")],
                              [float(native[f"up_{axis}"]) for axis in ("x", "y", "z")])
            if old_basis is not None and new_basis is not None:
                rotation_cosine = (sum(dot(a, b) for a, b in zip(old_basis, new_basis)) - 1.0) / 2.0
                rotation_error = math.acos(max(-1.0, min(1.0, rotation_cosine)))
                old_horizontal = math.hypot(old_basis[2][0], old_basis[2][2])
                new_horizontal = math.hypot(new_basis[2][0], new_basis[2][2])
                pitch_error = (math.atan2(-new_basis[2][1], new_horizontal)
                               - math.atan2(-old_basis[2][1], old_horizontal))
                roll_error = None
                if min(old_horizontal, new_horizontal) >= 0.01:
                    roll_error = (math.atan2(new_basis[0][1], new_basis[1][1])
                                  - math.atan2(old_basis[0][1], old_basis[1][1])
                                  + math.pi) % (2 * math.pi) - math.pi
                orientations.append((frame, rotation_error, pitch_error, roll_error))
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
        for key, values in errors.items():
            if len(values) > error_lengths[key]:
                frame_errors[key].append((frame, values[-1]))
        for source, steering in (("original", row["steer"]), ("native", float(native["steer"]))):
            steering_byte = round(steering * 127)
            controls[source]["steering_sign"].append((frame, (steering_byte > 0) - (steering_byte < 0)))
        if "original_gear" in native:
            controls["original"]["gear"].append((frame, row["gear"]))
            controls["native"]["gear"].append((frame, int(native["original_gear"])))
        if "ground" in row and "grounded" in native:
            flags = ([int(native[f"wheel{i}_grounded"]) for i in range(4)]
                     if all(f"wheel{i}_grounded" in native for i in range(4)) else None)
            contacts.append((frame, sum(value == 1 for value in row["ground"]), int(native["grounded"]),
                             list(row["ground"]) if flags is not None else None, flags))
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
    # Additional diagnostics leave the established global errors and
    # powerslide schema unchanged. All phase bounds are device-input frames;
    # original observation n includes the update with device input n-1.
    phase_maps = {
        "slalom": {"acceleration": (0, 300), "right_1": (300, 360),
                   "left_1": (360, 420), "right_2": (420, 480),
                   "left_2": (480, 540), "recovery": (540, 900)},
        "lift_turn": {"acceleration": (0, 300), "turn_load": (300, 330),
                      "throttle_lift": (330, 390), "throttle_reapply": (390, 450),
                      "recovery": (450, 900)},
        "brake_turn": {"acceleration": (0, 300), "turn_load": (300, 330),
                       "braking_turn": (330, 390), "throttle_reapply": (390, 420),
                       "countersteer": (420, 480), "recovery": (480, 900)},
    }
    result["orientation"] = orientation_summary(orientations)
    result["wheel_contact_counts"] = contact_summary(contacts)
    result["switch_timing"] = {
        kind: transition_timing(transitions(controls["original"][kind]),
                                transitions(controls["native"][kind]), original["hz"])
        for kind in controls["original"]
        if controls["original"][kind] and controls["native"][kind]
    }
    result["switch_timing"]["interpretation"] = (
        "Steering signs use quantized bytes; gear uses the retail index including reverse/neutral. "
        "Positive timing differences mean native later. Unmatched decisions are reported separately.")
    if original["scenario"] in phase_maps:
        result["phases"] = {}
        for phase, (first, last) in phase_maps[original["scenario"]].items():
            phase_frames = [frame for frame in frames if first < frame <= last]
            if not phase_frames:
                continue
            phase_result = {
                "frame_range": [phase_frames[0], phase_frames[-1]],
                "frames": len(phase_frames),
                "errors": {key: summarize(values)
                           for key, samples in frame_errors.items()
                           if (values := [value for frame, value in samples if first < frame <= last])},
                "wheel_contact_counts": contact_summary([s for s in contacts if first < s[0] <= last]),
                "orientation": orientation_summary([s for s in orientations if first < s[0] <= last]),
                "switch_counts": {
                    source: {kind: sum(first < event["frame"] <= last
                                       for event in transitions(samples))
                             for kind, samples in groups.items() if samples}
                    for source, groups in controls.items()
                },
            }
            for source, samples in slide_samples.items():
                values = [s for s in samples if first < s[0] <= last]
                phase_result[source] = {
                    "peak_abs_lateral_speed_mps": max(abs(s[2]) for s in values),
                    "peak_abs_slip_degrees": max(abs(math.degrees(s[3])) for s in values),
                    "last_slip_degrees": math.degrees(values[-1][3]),
                    "last_forward_speed_mps": values[-1][1],
                    "peak_abs_yaw_rate_radians_per_second": max(abs(s[4]) for s in values),
                    "last_yaw_rate_radians_per_second": values[-1][4],
                }
            result["phases"][phase] = phase_result
        neutral = phase_maps[original["scenario"]]["recovery"][0]
        result["stable_recovery"] = {
            source: recovery_summary(samples, neutral, original["hz"])
            for source, samples in slide_samples.items()
        }
        old_recovery = result["stable_recovery"]["original"]["first_frame"]
        new_recovery = result["stable_recovery"]["native"]["first_frame"]
        result["stable_recovery"]["native_minus_original_frames"] = (
            None if old_recovery is None or new_recovery is None else new_recovery - old_recovery)
        result["pedal_comparison_note"] = (
            "Retail records applied pedals; native records raw device pedals. Mixed phases may activate "
            "automatic swapping/stopped hold, so byte mismatch counts are intentionally unavailable.")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
