"""Stop a training run, or see how it is doing.

    uv run python -m mm2rl.ctl stop <run>     # finish the rollout, save, exit
    uv run python -m mm2rl.ctl status [run]   # latest log rows

Stopping loses at most the rollout in flight (a few seconds of
experience); `mm2rl.train --run <run>` picks the run up again, with any
`--workers` count.
"""

from __future__ import annotations

import json
import os
import signal
import sys
import time
from pathlib import Path

RUNS = Path(__file__).resolve().parents[1] / "runs"


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def stop(run: Path) -> None:
    pid_file = run / "pid"
    if not pid_file.exists():
        print(f"{run.name}: not running")
        return
    pid = int(pid_file.read_text())
    (run / "STOP").touch()
    if alive(pid):
        os.kill(pid, signal.SIGTERM)
    for _ in range(600):
        if not alive(pid):
            print(f"{run.name}: stopped")
            return
        time.sleep(0.1)
    print(f"{run.name}: still running after 60 s (pid {pid})")


def status(run: Path) -> None:
    log = run / "log.jsonl"
    if not log.exists():
        print(f"{run.name}: no log yet")
        return
    rows = [json.loads(line) for line in log.read_text().splitlines()[-400:]]
    pid_file = run / "pid"
    running = pid_file.exists() and alive(int(pid_file.read_text()))
    print(f"{run.name}: {'running' if running else 'stopped'}")
    for row in rows[-1:] + rows[-400::100][::-1]:
        age = (time.time() - row["time"]) / 60
        print(
            f"  {row['steps'] / 1e6:8.1f}M  {age:6.0f} min ago  {row['fps']:6d} fps  "
            f"return {row['ep_return']:7.2f}  metres {row['ep_metres']:6.0f}  "
            f"speed {row['speed']:5.1f}  impact/km {row['impact_per_km']:6.1f}  "
            f"stalled {row['end_stalled']:.2f}  off {row['end_off_route']:.2f}"
        )


def main() -> None:
    if len(sys.argv) < 2 or sys.argv[1] not in ("stop", "status"):
        print(__doc__)
        sys.exit(2)
    runs = [RUNS / sys.argv[2]] if len(sys.argv) > 2 else sorted(RUNS.iterdir())
    for run in runs:
        (stop if sys.argv[1] == "stop" else status)(run)


if __name__ == "__main__":
    main()
