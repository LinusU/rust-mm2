#!/usr/bin/env python3
"""Package reviewed city captures losslessly; never infer that a run is clean.

Usage: package_city_evidence.py plan.json NEW_OUTPUT_DIRECTORY [--allow-partial]
Plan: {"provenance": {...}, "cases": [{"id": "london-vpbug-hb",
"status": "reviewed_clean", "review_note": "...", "original": "...json",
"native": "...csv", "route": "...json"}]}. Input paths are relative to plan.
Native sidecars default to CSV + .audio.json/.contacts.json/.provenance.json.
Only explicitly reviewed_clean cases may be included. This status attests to
capture contamination review, NOT acceptable handling error or native playback.
A final package requires all four requested cars in both cities. --allow-partial
labels a development package; it cannot create final eight-case evidence.
"""
import argparse
import csv
import gzip
import hashlib
import io
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import tempfile


CARS = {"vpbug", "vppanoz", "vpbullet", "vpcoop"}
EXPECTED = {(city, car) for city in ("sf", "london") for car in CARS}


def raw_bytes(path):
    data = path.read_bytes()
    return gzip.decompress(data) if path.suffix == ".gz" else data


def decode_json(data):
    value = json.loads(data, parse_constant=lambda s: (_ for _ in ()).throw(ValueError(s)))
    pending = [value]
    while pending:
        item = pending.pop()
        if isinstance(item, dict):
            pending.extend(item.values())
        elif isinstance(item, list):
            pending.extend(item)
        elif isinstance(item, float) and not math.isfinite(item):
            raise ValueError("non-finite JSON number")
    return value


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--allow-partial", action="store_true")
    args = parser.parse_args()
    if args.output.exists():
        raise SystemExit("Output already exists; choose a new directory to preserve previous evidence")
    plan_data = raw_bytes(args.plan)
    plan = decode_json(plan_data)
    if not plan.get("provenance"):
        raise SystemExit("Plan must state capture/build provenance")
    base = args.plan.resolve().parent
    scripts = Path(__file__).resolve().parent
    cases, identities, ids = [], set(), set()
    # Validate every input before writing a package. No filename glob selection.
    for case in plan["cases"]:
        name = case["id"]
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", name) or name in ids:
            raise ValueError("Invalid or duplicate case id: " + name)
        ids.add(name)
        if case.get("status") != "reviewed_clean" or not case.get("review_note", "").strip():
            raise ValueError(name + ": explicit reviewed_clean status and contamination review_note required")
        paths = {key: base / case[key] for key in ("original", "native", "route")}
        for key in ("audio", "contacts", "provenance"):
            paths[key] = base / case[key] if key in case else Path(str(paths["native"]) + "." + key + ".json")
        payload = {key: raw_bytes(path) for key, path in paths.items()}
        backend_keys = {"backend_native", "backend_audio", "backend_provenance"}
        if backend_keys.intersection(case):
            if not backend_keys.issubset(case):
                raise ValueError(name + ": live evidence requires CSV, audio and provenance paths")
            for key in ("backend_native", "backend_audio", "backend_provenance"):
                paths[key] = base / case[key]
                payload[key] = raw_bytes(paths[key])
            if payload["backend_native"] != payload["native"]:
                raise ValueError(name + ": live audio replay changed the ordinary physics CSV")
        route = decode_json(payload["route"])
        original = decode_json(payload["original"])
        provenance = decode_json(payload["provenance"])
        audio = decode_json(payload["audio"])
        contacts = decode_json(payload["contacts"])
        identity = (route["city"], route["car"])
        if identity not in EXPECTED or identity in identities:
            raise ValueError(name + ": unexpected or repeated city/car identity")
        identities.add(identity)
        if original.get("route") != route or provenance.get("route") != route:
            raise ValueError(name + ": route differs between source, native and supplied route")
        if original.get("route_sha256") != digest(payload["route"]):
            raise ValueError(name + ": source frozen route raw SHA256 mismatch")
        for evidence in (original, provenance):
            if (evidence.get("city"), evidence.get("car")) != identity or evidence.get("hz") != 60:
                raise ValueError(name + ": city/car/frequency mismatch")
        if not original.get("car_identity_verified") or not original.get("complete"):
            raise ValueError(name + ": unverified car or incomplete source capture")
        if "diagnostic_waypoint_plan" not in provenance or provenance["diagnostic_waypoint_plan"] is not None:
            raise ValueError(name + ": explicit ordinary frozen-input provenance required; planner runs are excluded")
        if provenance.get("diagnostic_measured_steering"):
            raise ValueError(name + ": measured-input diagnostic is not a normal keyboard replay")
        if provenance.get("diagnostic_initial_yaw"):
            raise ValueError(name + ": alternate initial-yaw diagnostic is not a normal native replay")
        if provenance.get("diagnostic_settled_source"):
            raise ValueError(name + ": seeded source-state diagnostic is not a normal native replay")
        if provenance.get("diagnostic_substeps") is not None:
            raise ValueError(name + ": alternate substep diagnostic is not a normal native replay")
        if provenance.get("diagnostic_pose"):
            raise ValueError(name + ": alternate pose diagnostic is not a normal native replay")
        if provenance.get("diagnostic_engine_source"):
            raise ValueError(name + ": engine state diagnostic is not a normal native replay")
        frames = route["frames"]
        if frames < 1800 or route.get("settle_frames", 600) != 600:
            raise ValueError(name + ": expected at least30seconds and600frame handbrake settlement")
        if (original.get("settle_frames") != 600 or not original.get("settle_handbrake_held")
                or provenance.get("settle_frames") != 600 or provenance.get("settle_handbrake") != 1):
            raise ValueError(name + ": source/native did not record600frame handbrake settlement")
        if [row["n"] for row in original["rows"]] != list(range(frames + 1)):
            raise ValueError(name + ": source rows missing or nonsequential")
        native = list(csv.DictReader(io.StringIO(payload["native"].decode("utf-8"))))
        if [int(row["frame"]) for row in native] != list(range(frames)):
            raise ValueError(name + ": native rows missing or nonsequential")
        for row in native:
            if any(value and not math.isfinite(float(value)) for value in row.values()):
                raise ValueError(name + ": non-finite native numeric value")
        if [row["frame"] for row in audio["frames"]] != list(range(frames)):
            raise ValueError(name + ": audio rows missing or nonsequential")
        if "backend_audio" in payload:
            backend = decode_json(payload["backend_audio"])
            backend_provenance = decode_json(payload["backend_provenance"])
            if backend_provenance != provenance:
                # Live audio has different descriptive limitations only.
                ignored = {"limitations"}
                if ({k: v for k, v in backend_provenance.items() if k not in ignored}
                        != {k: v for k, v in provenance.items() if k not in ignored}):
                    raise ValueError(name + ": live audio physics provenance differs")
            if (not backend.get("backend_sink_observed") or backend.get("device_output_recorded")
                    or [row["frame"] for row in backend["frames"]] != list(range(frames))):
                raise ValueError(name + ": incomplete or misclassified live sink evidence")
            if any(row.get("report", {}).get(key, 0) != 0
                   for row in backend["frames"] for key in ("failed", "dropped")):
                raise ValueError(name + ": live audio failed or dropped a voice")
        if any(row.get("report", {}).get(key, 0) != 0
               for row in audio["frames"] for key in ("failed", "dropped", "sunk")):
            raise ValueError(name + ": native requested audio failed/dropped or unexpectedly attached a sink")
        # Ground proximity/speculation alone is expected. Props or any solved
        # contact reject a clean-route claim, including negative penetration.
        for pair in contacts:
            if pair.get("banger") is not None or not (pair.get("name") or "").startswith("city-ground"):
                raise ValueError(name + ": native non-ground contact requires a new reviewed route")
            if any(point.get("penetration", 0) > 0 or point.get("normal_impulse", 0) != 0 for point in pair["points"]):
                raise ValueError(name + ": native body contact rejects clean-route packaging")
        cases.append((case, paths, payload, identity, frames))
    if not cases or (not args.allow_partial and identities != EXPECTED):
        raise ValueError("Final evidence requires exactly SF/London × Beetle/Panoz/Fastback/Mini; use --allow-partial for development")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    # Staging prevents a failed comparator from leaving an apparently final pack.
    with tempfile.TemporaryDirectory(prefix=".city-evidence-", dir=args.output.parent) as temporary:
        output = Path(temporary) / "package"
        output.mkdir()
        manifest = {"format": 1, "scope": "development_partial" if args.allow_partial else "eight_city_cases",
                    "status_meaning": "reviewed_clean describes capture contamination only; metrics remain unthresholded",
                    "numeric_storage": "lossless UTF-8 source bytes; gzip level9 mtime0; raw and archived SHA256 recorded",
                    "alignment": "original row n aligns native frame n−1; original0 settled pre-input",
                    "audio_scope": "original DirectSound backend vs native requested production metadata; optional attached native sink evidence is separately reported, without recorded device output",
                    "provenance": plan["provenance"], "plan_raw_sha256": digest(plan_data), "files": [], "cases": []}

        def write(relative, data, source=None, compress=False):
            destination = output / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            stored = gzip.compress(data, compresslevel=9, mtime=0) if compress else data
            destination.write_bytes(stored)
            manifest["files"].append({"path": relative, "source": str(source) if source else "generated",
                                      "raw_bytes": len(data), "raw_sha256": digest(data),
                                      "stored_bytes": len(stored), "stored_sha256": digest(stored),
                                      "compression": "gzip" if compress else "none"})

        write("package-plan.json", plan_data, args.plan)
        for script in ("package_city_evidence.py", "compare_traces.py", "compare_city_audio.py", "audit_source_integration.py", "capture_original.ts"):
            write("tools/" + script, (scripts / script).read_bytes(), scripts / script)
        for case, paths, payload, identity, frames in cases:
            prefix = case["id"] + "/"
            names = {"original": "original.json.gz", "native": "native.csv.gz", "route": "route.json",
                     "audio": "native.audio.json.gz", "contacts": "native.contacts.json.gz", "provenance": "native.provenance.json"}
            for key, filename in names.items():
                write(prefix + filename, payload[key], paths[key], compress=filename.endswith(".gz"))
            if "backend_audio" in payload:
                for key, filename in (("backend_audio", "native.backend-audio.json.gz"),
                                      ("backend_provenance", "native.backend-provenance.json")):
                    write(prefix + filename, payload[key], paths[key], compress=filename.endswith(".gz"))
                backend_result = output / prefix / "backend-audio-comparison.json"
                subprocess.run([sys.executable, str(scripts / "compare_city_audio.py"),
                                str(output / prefix / names["original"]),
                                str(output / prefix / "native.backend-audio.json.gz"),
                                "--output", str(backend_result)], check=True)
                data = decode_json(backend_result.read_bytes())
                data.update(original=names["original"], native="native.backend-audio.json.gz")
                write(prefix + "backend-audio-comparison.json", (json.dumps(data, indent=2) + "\n").encode())
            trace = subprocess.run([sys.executable, str(scripts / "compare_traces.py"), str(output / prefix / names["original"]),
                                    str(output / prefix / names["native"]), "--world-coordinate"], check=True, capture_output=True).stdout
            write(prefix + "handling-comparison.json", trace)
            audio_result = output / prefix / "audio-comparison.json"
            subprocess.run([sys.executable, str(scripts / "compare_city_audio.py"), str(output / prefix / names["original"]),
                            str(output / prefix / names["audio"]), "--output", str(audio_result)], check=True)
            write(prefix + "audio-comparison.json", audio_result.read_bytes())
            # Generated reports should refer to portable packaged inputs.
            for filename, replacements in [("handling-comparison.json", {"original_file": names["original"], "native_file": names["native"]}),
                                           ("audio-comparison.json", {"original": names["original"], "native": names["audio"]})]:
                report = output / prefix / filename
                data = decode_json(report.read_bytes())
                data.update(replacements)
                if replacements:
                    manifest["files"] = [entry for entry in manifest["files"] if entry["path"] != prefix + filename]
                    write(prefix + filename, (json.dumps(data, indent=2) + "\n").encode())
            manifest["cases"].append({"id": case["id"], "city": identity[0], "car": identity[1], "frames": frames,
                                       "status": case["status"], "review_note": case["review_note"],
                                       "backend_audio": prefix + "backend-audio-comparison.json" if "backend_audio" in payload else None,
                                       "handling": prefix + "handling-comparison.json", "audio": prefix + "audio-comparison.json"})
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        output.rename(args.output)
    print(str(args.output / "manifest.json"))


if __name__ == "__main__":
    main()
