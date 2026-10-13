"""Draw a trouble spot from above.

    mm2_gym --mm2-path $MM2_RETAIL --city sf --event race3 \\
        map --at=-780,62,45 > spot.json
    uv run --with matplotlib python -m mm2rl.plot spot.json spot.png [x,z ...]

The picture is a height map of everything a car can stand on or run
into (dark low, light high; white where there is nothing), with the
event's opponent lines drawn over it and any extra `x,z` points marked.
North (−Z) is up, as on the game's map.
"""

from __future__ import annotations

import json
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


def main() -> None:
    spot = json.load(open(sys.argv[1]))
    cells, step = spot["cells"], spot["step"]
    cx, cy, cz = spot["centre"]
    half = (cells - 1) * step / 2
    heights = np.array(
        [np.nan if h is None else h for h in spot["heights"]], dtype=float
    ).reshape(cells, cells)
    fig, ax = plt.subplots(figsize=(11, 11))
    extent = [cx - half, cx + half, cz + half, cz - half]
    image = ax.imshow(
        heights, extent=extent, origin="upper", cmap="gray",
        vmin=cy - 6, vmax=cy + 8,
    )
    ax.contour(
        np.linspace(cx - half, cx + half, cells),
        np.linspace(cz - half, cz + half, cells),
        heights, levels=np.arange(cy - 6, cy + 8, 1.0), colors="tab:blue",
        linewidths=0.3,
    )
    for k, line in enumerate(spot["lines"]):
        points = np.array(line["points"])
        if len(points) < 2:
            continue
        ax.plot(points[:, 0], points[:, 2], "-", lw=1.2, color=f"C{k % 10}",
                label=line["label"], marker=".", ms=3)
    for arg in sys.argv[3:]:
        x, z = (float(v) for v in arg.split(","))
        ax.plot([x], [z], "rx", ms=12, mew=2.5)
    ax.set_xlim(cx - half, cx + half)
    ax.set_ylim(cz - half, cz + half)
    ax.invert_yaxis()
    ax.set_xlabel("x")
    ax.set_ylabel("z")
    ax.legend(fontsize=6, loc="upper right")
    fig.colorbar(image, ax=ax, shrink=0.6, label="height (m)")
    fig.savefig(sys.argv[2], dpi=110, bbox_inches="tight")


if __name__ == "__main__":
    main()
