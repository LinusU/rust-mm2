#!/usr/bin/env python3
"""Compare retail DirectSound playback with headless production skid requests.

Original n=1 aligns with native frame=0. Retail backend state normally reflects
previous-row wheel slip; --source-lag is reported explicitly, never fitted.
Native metadata has no audio device: nonzero mix is a request, not playback.
"""
import argparse
import gzip
import json
import math
import re
from pathlib import Path


def open_text(path):
    """Read lossless evidence archives or ordinary UTF-8 files."""
    return gzip.open(path, "rt", encoding="utf-8", newline="") if path.suffix == ".gz" else path.open(encoding="utf-8", newline="")


def read_json(path):
    with open_text(path) as source:
        return json.load(source)


def amplitude(control):
    db = math.trunc((min(1.0, max(0.0, control)) - 1.0) * 10000.0)
    return 10.0 ** (db / 2000.0)


def selected(audio):
    if not audio.get("available") or audio.get("q", 0) <= 0:
        return {}
    return {i: amplitude(audio["requestedGainIfCalled"])
            for i, band in enumerate(audio.get("bands", []))
            if band.get("inclusiveSelection")}


def backend(audio):
    return {i: band["backend"].get("linearVolume", 0)
            for i, band in enumerate(audio.get("bands", []))
            if band.get("backend", {}).get("available")
            and band["backend"].get("isPlaying")}


def requested(row):
    result = {}
    for voice in row.get("voices", []):
        band = re.fullmatch(r"Skid\((\d+)\)", voice.get("role", ""))
        if band and voice.get("mix_volume", 0) > 0:
            result[int(band[1])] = voice["mix_volume"]
    return result


def native_sink(row):
    """Positive-gain, unpaused, nonempty sink; device samples are not recorded."""
    result = {}
    for voice in row.get("voices", []):
        band = re.fullmatch(r"Skid\((\d+)\)", voice.get("role", ""))
        sink = voice.get("sink")
        if (band and sink and sink.get("volume_linear", 0) > 0
                and not sink.get("paused") and not sink.get("empty")):
            result[int(band[1])] = sink["volume_linear"]
    return result


def intervals(frames):
    result = []
    for band in sorted({band for values in frames.values() for band in values}):
        ticks = sorted(tick for tick, values in frames.items() if band in values)
        start = previous = None
        for tick in ticks:
            if previous is None or tick != previous + 1:
                if previous is not None:
                    result.append({"band": band, "start": start, "stop_exclusive": previous + 1})
                start = tick
            previous = tick
        if previous is not None:
            result.append({"band": band, "start": start, "stop_exclusive": previous + 1})
    return sorted(result, key=lambda x: (x["start"], x["band"]))


def compare(left, right):
    ticks = sorted(left.keys() & right.keys())
    amplitude_errors = [abs(left[t][b] - right[t][b]) for t in ticks
                        for b in left[t].keys() & right[t].keys()]
    return {"samples": len(ticks),
            "active_band_set_mismatch_ticks": sum(left[t].keys() != right[t].keys() for t in ticks),
            "any_skid_mismatch_ticks": sum(bool(left[t]) != bool(right[t]) for t in ticks),
            "same_band_gain_samples": len(amplitude_errors),
            "same_band_amplitude_mae": sum(amplitude_errors) / len(amplitude_errors) if amplitude_errors else None,
            "same_band_amplitude_max_error": max(amplitude_errors, default=None)}


def timing(source, native):
    remaining = intervals(native)
    matched, unmatched = [], []
    for original in intervals(source):
        candidates = [v for v in remaining if v["band"] == original["band"]
                      and abs(v["start"] - original["start"]) <= 60]
        if not candidates:
            unmatched.append(original)
            continue
        target = min(candidates, key=lambda x: abs(x["start"] - original["start"]))
        remaining.remove(target)
        matched.append({"band": original["band"], "source": original, "native": target,
                        "onset_error_ticks": target["start"] - original["start"],
                        "stop_error_ticks": target["stop_exclusive"] - original["stop_exclusive"]})
    return {"matching": "greedy nearest onset within 60 ticks, same band only; merged bursts may remain unmatched",
            "matched": matched, "unmatched_source": unmatched, "unmatched_native": remaining}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("original", type=Path)
    parser.add_argument("native", type=Path, help="production *.csv.audio.json sidecar")
    parser.add_argument("--source-lag", type=int, default=1)
    parser.add_argument("--native-lag", type=int, default=1, help="physics-clock request delay; use 0 for pre-fix sidecars")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    original = read_json(args.original)
    native = read_json(args.native)
    rows = {r["n"] - 1: r.get("sourceSkidAudio", {}) for r in original["rows"]}
    native_rows = {r["frame"]: r for r in native["frames"]}
    actual = {t: backend(a) for t, a in rows.items()
              if a.get("backendPlaybackSampling") and all(b.get("backend", {}).get("available") for b in a.get("bands", []))}
    source_q = {t: selected(a) for t, a in rows.items() if a.get("available")}
    lagged_source_q = {t + args.source_lag: bands for t, bands in source_q.items()}
    adjusted_actual = {t - args.source_lag: bands for t, bands in actual.items()}
    native_requests = {t: requested(r) for t, r in native_rows.items()}
    has_native_backend = native.get("backend_sink_observed", False)
    native_backend = {t: native_sink(r) for t, r in native_rows.items()} if has_native_backend else {}
    adjusted_requests = {t - args.native_lag: bands for t, bands in native_requests.items()}
    presented_source_q = {t + args.native_lag: bands for t, bands in source_q.items()}
    native_law = {}
    for tick, row in native_rows.items():
        source = rows.get(tick, {})
        contact = row.get("surface_contact", {}).get("skid")
        if not source.get("available"):
            continue
        if contact is None:
            native_law[tick] = {}
            continue
        # Different surface tables/classes are not evidence of a faulty mixer.
        if contact["surface"] != source.get("surfaceClass"):
            continue
        q = contact["slippage"]
        native_law[tick] = {i: amplitude(source["minVolume"] + q * source["volumeSlope"])
                            for i, band in enumerate(source.get("bands", []))
                            if q > 0 and band["minimumSlip"] <= q <= band["maximumSlip"]}
    result = {
        "original": str(args.original), "native": str(args.native),
        "alignment": "source n−1 = native frame; stops are exclusive; band indices are table row ordinals",
        "source_backend_lag_ticks": args.source_lag,
        "native_request_delay_ticks": args.native_lag,
        "native_evidence": (
            "requested mix plus attached sink gain/pause/empty state; positive-gain windows compared; device output not recorded"
            if has_native_backend else "requested production mix only; no native backend playback measured"
        ),
        "surface_scope": "retail sampler observes current class bands; native internal-law check excludes different classes",
        "retail_internal_previous_q_vs_backend": compare(lagged_source_q, actual),
        "native_internal_own_q_vs_requested": compare(native_law, native_requests),
        "handling_source_q_vs_native_requests": compare(presented_source_q, native_requests),
        "raw_backend_vs_native_requests": compare(actual, native_requests),
        "lag_adjusted_backend_vs_native_requests": compare(adjusted_actual, adjusted_requests),
        "source_backend_windows": intervals(actual), "native_request_windows": intervals(native_requests),
        "raw_timing": timing(actual, native_requests),
        "lag_adjusted_timing": timing(adjusted_actual, adjusted_requests),
    }
    if has_native_backend:
        result["native_requests_vs_attached_sink"] = compare(native_requests, native_backend)
        result["source_backend_vs_native_positive_gain_sink"] = compare(actual, native_backend)
        result["native_positive_gain_sink_windows"] = intervals(native_backend)
        result["source_backend_vs_native_sink_timing"] = timing(actual, native_backend)
    encoded = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
