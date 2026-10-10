#!/usr/bin/env python3
"""Create self-authored PSD0 geometry for matched original/native drive runs.

No original assets are copied. The texture name is a reference resolved by the
user's own installation. Place the result in a separate mod directory, never
write it over the retail installation. Companion city files (AI, instance
placements, routes) are deliberately outside this fixture; turn off traffic,
pedestrians and cops, and reset the test car to an isolated recorded pose.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import struct


def make_track(size, center_x, center_z, height, texture, rooms):
    """One road plus disjoint outer pads retaining companion-file room indices."""
    if not all(math.isfinite(v) for v in (size, center_x, center_z, height)):
        raise ValueError("track dimensions must be finite")
    if size < 2000:
        raise ValueError("track side must be at least 2000 metres")
    if not 2 <= rooms <= 16384:
        raise ValueError("room count includes reserved zero, and must be 2..16384")
    name = texture.encode("ascii") + b"\0"
    if len(name) > 255:
        raise ValueError("texture reference must fit a one-byte length")
    half = size / 2
    # Keep room geometry disjoint: retail's 64x64 spatial buckets overflow
    # when every preserved room spans the entire drive plane. Reserve the
    # outer ring for tiny pads, one per grid cell, outside the main road.
    road_half = half if rooms == 2 else half * 0.8
    if road_half * 2 < 2000:
        raise ValueError("main road must remain at least 2000 metres wide")
    quads = [(center_x, center_z, road_half)]
    if rooms > 2:
        candidates = []
        cell = size / 64
        for row in range(64):
            z = -half + (row + 0.5) * cell
            for col in range(64):
                x = -half + (col + 0.5) * cell
                if abs(x) > road_half + 1 or abs(z) > road_half + 1:
                    candidates.append((center_x + x, center_z + z, 0.5))
        if rooms - 2 > len(candidates):
            raise ValueError("too many preserved rooms for the disjoint outer-pad grid")
        quads += candidates[:rooms - 2]
    # Clockwise in authored XZ, as retail paving is wound (research/psdl).
    vertices = []
    for x, z, radius in quads:
        vertices += [(x - radius, height, z - radius),
                     (x - radius, height, z + radius),
                     (x + radius, height, z + radius),
                     (x + radius, height, z - radius)]
    data = bytearray(b"PSD0")
    data += struct.pack("<II", 2, len(vertices))
    for vertex in vertices:
        data += struct.pack("<3f", *vertex)
    data += struct.pack("<If", 1, height)  # absolute height pool
    data += struct.pack("<I", 2) + bytes([len(name)]) + name
    data += struct.pack("<II", rooms, 0)  # no junctions
    # TextureRef index+1=1, then last RoadFan with 2 triangles/4 refs.
    for room in range(rooms - 1):
        indices = list(range(room * 4, room * 4 + 4))
        attributes = [0x50, 1, 0x80 | (5 << 3) | 2, *indices]
        data += struct.pack("<II", 4, len(attributes))
        for index in indices:
            data += struct.pack("<HH", index, 0)  # no room neighbours
        data += struct.pack("<7H", *attributes)
    # All observed retail SF RoadFan rooms carry Intersection, not Road.
    # Match their topology flag so original room/probe dispatch agrees.
    data += bytes([0] + [0x10] * (rooms - 1))
    data += bytes(rooms)  # no procedural prop rules
    data += struct.pack("<3f", center_x - half, height, center_z - half)
    data += struct.pack("<3f", center_x + half, height, center_z + half)
    data += struct.pack("<3ff", center_x, height, center_z, half * math.sqrt(2))
    data += struct.pack("<I", 0)  # no prop paths
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, help="Separate mod path, e.g. /tmp/track/city/sf.psdl")
    parser.add_argument("--size", type=float, default=4000, help="Square side in metres")
    parser.add_argument("--center-x", type=float, default=0)
    parser.add_argument("--center-z", type=float, default=0)
    parser.add_argument("--height", type=float, default=0)
    parser.add_argument("--texture", default="r4_lo_f", help="Existing installation texture; its material must be _default")
    parser.add_argument("--rooms", type=int, default=2,
                        help="Stored room count including reserved zero; preserve indices with disjoint outer pads")
    args = parser.parse_args()
    data = make_track(args.size, args.center_x, args.center_z, args.height, args.texture, args.rooms)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(data)
    manifest = {
        "format": "PSD0", "purpose": "self-authored flat vehicle handling benchmark",
        "size_metres": args.size, "center_xz": [args.center_x, args.center_z],
        "height_metres": args.height, "texture_reference": args.texture,
        "room_count_including_reserved": args.rooms, "vertices": 4 * (args.rooms - 1),
        "main_road_side_metres": args.size if args.rooms == 2 else args.size * 0.8,
        "preserved_room_layout": "single road with disjoint 1m outer pads on a 64x64 grid",
        "room_flag": "Intersection (0x10), matching retail RoadFan rooms",
        "triangles_per_room": 2, "procedural_props": 0, "prop_paths": 0,
        "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data),
        "verification": "Generated; original/native runtime loading must be verified separately",
    }
    args.output.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
