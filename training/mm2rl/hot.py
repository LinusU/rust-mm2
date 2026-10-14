"""Where a run's episodes have been failing.

    uv run python -m mm2rl.hot runs/<name>/lines.json

Prints one row per event, worst first: failures per kilometre driven
over the run's last ten minutes, how the failed episodes ended, and the
25 m stretches of line where most of them did. A stretch that collects
failures out of proportion is a place to look at before it is a thing
to train on — `mm2_gym … where --arc <m>` gives its coordinates and
`mm2_gym … map` with `mm2rl.plot` draws it. More than once it was the
line that was wrong, not the driving.

A quarter of episodes start just before a recent failure, so a hard
spot is over-sampled: read the bins as where, not how often.
"""

from __future__ import annotations

import collections
import json
import sys

FAILURES = ("stalled", "off_route", "upended", "lost")
ENDS = ("finished", "time", *FAILURES)


def main() -> None:
    lines = json.load(open(sys.argv[1]))
    events: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    bins: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for label, stats in lines.items():
        city, event, _difficulty, _slot = label.split("/")
        key = f"{city}/{event}"
        for end in ENDS:
            events[key][end] += stats.get(end, 0)
        events[key]["metres"] += stats.get("metres", 0)
        for name, count in stats.items():
            if name.startswith("at"):
                bins[key][int(name[2:])] += count

    rows = []
    for key, counts in events.items():
        failed = sum(counts[end] for end in FAILURES)
        rows.append((failed / max(1.0, counts["metres"]) * 1000.0, key, counts))
    rows.sort(reverse=True)

    print("fail/km  event               episodes  stalled  off  upended  lost   worst stretches")
    for rate, key, counts in rows:
        episodes = sum(counts[end] for end in ENDS)
        worst = ", ".join(f"{arc} m: {n}" for arc, n in bins[key].most_common(4))
        print(
            f"{rate:7.2f}  {key:18s}  {episodes:8d}  {counts['stalled']:7d}  "
            f"{counts['off_route']:3d}  {counts['upended']:7d}  {counts['lost']:4d}   {worst}"
        )
    metres = sum(counts["metres"] for _, _, counts in rows)
    failed = sum(counts[end] for _, _, counts in rows for end in FAILURES)
    print(f"all events: {failed / max(1.0, metres) * 1000.0:.2f} failures/km over {metres / 1000.0:.0f} km")


if __name__ == "__main__":
    main()
