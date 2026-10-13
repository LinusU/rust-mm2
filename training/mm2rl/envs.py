"""The `mm2_gym` environment processes, seen as one batch of agents.

Each worker is an `mm2_gym serve` process holding one city and a field
of cars. `VecEnv` steps them together: actions go out to every worker,
then every worker's reply is read back, so the physics of all of them
runs at once on separate cores.
"""

from __future__ import annotations

import json
import struct
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

PROTOCOL = 2
EPISODE = struct.Struct("<IIffIIfffff")
ENDS = ["finished", "time", "stalled", "off_route", "upended", "lost"]


@dataclass
class Episode:
    end: int
    steps: int
    metres: float
    ret: float
    line: str
    car: str
    impacts: float
    arc: float
    position: tuple[float, float, float]


@dataclass
class Worker:
    proc: subprocess.Popen
    agents: int
    obs_dim: int
    act_dim: int
    city: str
    event: str | None = None
    lines: list[str] = field(default_factory=list)
    cars: list[str] = field(default_factory=list)

    def read(self, n: int) -> bytes:
        data = self.proc.stdout.read(n)
        if data is None or len(data) != n:
            raise RuntimeError(
                f"mm2_gym ({self.city}) closed its pipe "
                f"(exit status {self.proc.poll()})"
            )
        return data

    def read_f32(self, *shape: int) -> np.ndarray:
        count = int(np.prod(shape))
        return np.frombuffer(self.read(count * 4), dtype="<f4").reshape(shape)

    def read_u32(self) -> int:
        return struct.unpack("<I", self.read(4))[0]

    def send(self, payload: bytes) -> None:
        self.proc.stdin.write(payload)
        self.proc.stdin.flush()


def list_events(gym: Path, install: Path, city: str) -> list[str]:
    """The race events of `city` an environment can load."""
    out = subprocess.run(
        [str(gym), "--mm2-path", str(install), "--city", city, "events"],
        check=True, capture_output=True, text=True,
    )
    return json.loads(out.stdout)


def spawn_worker(
    gym: Path,
    install: Path,
    city: str,
    event: str | None,
    agents: int,
    drones: int,
    seed: int,
    decision_ticks: int = 3,
) -> Worker:
    command = [
        str(gym),
        "--mm2-path", str(install),
        "--city", city,
        "--agents", str(agents),
        "--drones", str(drones),
        "--seed", str(seed),
        "--decision-ticks", str(decision_ticks),
    ]
    if event:
        command += ["--event", event]
    proc = subprocess.Popen(
        command + ["serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        bufsize=1 << 20,
    )
    worker = Worker(proc, agents, 0, 0, city, event)
    if worker.read(4) != b"MM2G":
        raise RuntimeError("mm2_gym did not say hello")
    protocol, obs_dim, act_dim, n, _layout = struct.unpack("<5I", worker.read(20))
    if protocol != PROTOCOL or n != agents:
        raise RuntimeError(f"mm2_gym protocol {protocol}, {n} agents: unexpected")
    meta = json.loads(worker.read(worker.read_u32()))
    worker.obs_dim, worker.act_dim = obs_dim, act_dim
    worker.lines = [f"{city}/{line['label']}" for line in meta["lines"]]
    worker.cars = meta["cars"]
    return worker


class VecEnv:
    """Every worker's agents as one batch.

    Each worker holds one race event's world — its course barricades,
    parked cars and bridges — and that event's lines. `worlds` is the
    list of `(city, event)` pairs to draw from; `pick` chooses the next
    one whenever a worker is started or recycled.
    """

    def __init__(
        self,
        gym: Path,
        install: Path,
        worlds: list[tuple[str, str | None]],
        workers: int,
        agents: int,
        drones: int,
        seed: int,
        configure: dict | None = None,
        pick=None,
    ):
        self.gym, self.install = gym, install
        self.agents_per_worker = agents
        self.drones = drones
        self.configure = configure
        self.seed = seed
        self.spawned = 0
        self.worlds = worlds
        self.pick = pick or (lambda i: worlds[i % len(worlds)])
        self.workers: list[Worker] = []
        for _ in range(workers):
            self.workers.append(self._spawn())
        self.obs_dim = self.workers[0].obs_dim
        self.act_dim = self.workers[0].act_dim
        self.n = workers * agents

    def _spawn(self) -> Worker:
        city, event = self.pick(self.spawned)
        self.spawned += 1
        worker = spawn_worker(
            self.gym, self.install, city, event, self.agents_per_worker,
            self.drones, self.seed * 100_003 + self.spawned,
        )
        if self.configure:
            payload = json.dumps(self.configure).encode()
            worker.send(struct.pack("<II", 3, len(payload)) + payload)
            if worker.read_u32() != 0:
                raise RuntimeError("mm2_gym rejected the configuration")
        return worker

    def observe(self) -> np.ndarray:
        for w in self.workers:
            w.send(struct.pack("<I", 0))
        return np.concatenate(
            [w.read_f32(w.agents, w.obs_dim) for w in self.workers]
        )

    def step(self, actions: np.ndarray):
        """Returns obs, reward, done, final_obs, final_index, episodes.

        `final_obs[k]` is what agent `final_index[k]` saw when its
        episode was cut short (done == 2).
        """
        a = self.agents_per_worker
        actions = np.ascontiguousarray(actions, dtype="<f4")
        for i, w in enumerate(self.workers):
            w.send(struct.pack("<I", 1) + actions[i * a:(i + 1) * a].tobytes())
        obs, reward, done, finals, final_index, episodes = [], [], [], [], [], []
        for i, w in enumerate(self.workers):
            obs.append(w.read_f32(a, w.obs_dim))
            reward.append(w.read_f32(a))
            d = np.frombuffer(w.read(a), dtype=np.uint8)
            done.append(d)
            count = w.read_u32()
            if count:
                finals.append(w.read_f32(count, w.obs_dim))
                final_index.append(np.flatnonzero(d == 2) + i * a)
            for _ in range(w.read_u32()):
                end, steps, metres, ret, line, car, impacts, arc, x, y, z = EPISODE.unpack(
                    w.read(EPISODE.size)
                )
                episodes.append(
                    Episode(end, steps, metres, ret, w.lines[line], w.cars[car],
                            impacts, arc, (x, y, z))
                )
        empty = np.zeros((0, self.obs_dim), dtype=np.float32)
        return (
            np.concatenate(obs),
            np.concatenate(reward),
            np.concatenate(done),
            np.concatenate(finals) if finals else empty,
            np.concatenate(final_index) if final_index else np.zeros(0, dtype=np.int64),
            episodes,
        )

    def recycle(self, index: int) -> None:
        """Replace one worker with a fresh process on another event —
        the batch keeps moving through every race's world, and a city
        whose props have all been knocked over is not the city the game
        starts in."""
        old = self.workers[index]
        self.workers[index] = self._spawn()
        _close(old)

    def close(self) -> None:
        for w in self.workers:
            _close(w)
        self.workers = []


def _close(w: Worker) -> None:
    try:
        w.send(struct.pack("<I", 2))
        w.proc.stdin.close()
        w.proc.wait(timeout=5)
    except Exception:
        w.proc.kill()
