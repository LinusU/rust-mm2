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

PROTOCOL = 1
EPISODE = struct.Struct("<IIffIIf")
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


@dataclass
class Worker:
    proc: subprocess.Popen
    agents: int
    obs_dim: int
    act_dim: int
    city: str
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


def spawn_worker(
    gym: Path,
    install: Path,
    city: str,
    agents: int,
    seed: int,
    decision_ticks: int = 3,
) -> Worker:
    proc = subprocess.Popen(
        [
            str(gym),
            "--mm2-path", str(install),
            "--city", city,
            "--agents", str(agents),
            "--seed", str(seed),
            "--decision-ticks", str(decision_ticks),
            "serve",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        bufsize=1 << 20,
    )
    worker = Worker(proc, agents, 0, 0, city)
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
    """Every worker's agents as one batch."""

    def __init__(
        self,
        gym: Path,
        install: Path,
        cities: list[str],
        workers: int,
        agents: int,
        seed: int,
        configure: dict | None = None,
    ):
        self.gym, self.install = gym, install
        self.agents_per_worker = agents
        self.configure = configure
        self.seed = seed
        self.spawned = 0
        self.workers: list[Worker] = []
        for i in range(workers):
            self.workers.append(self._spawn(cities[i % len(cities)]))
        self.obs_dim = self.workers[0].obs_dim
        self.act_dim = self.workers[0].act_dim
        self.n = workers * agents

    def _spawn(self, city: str) -> Worker:
        self.spawned += 1
        worker = spawn_worker(
            self.gym, self.install, city, self.agents_per_worker,
            self.seed * 100_003 + self.spawned,
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
                end, steps, metres, ret, line, car, impacts = EPISODE.unpack(
                    w.read(EPISODE.size)
                )
                episodes.append(
                    Episode(end, steps, metres, ret, w.lines[line], w.cars[car], impacts)
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
        """Replace one worker with a fresh process — a city whose props
        have all been knocked over is not the city the game starts in."""
        old = self.workers[index]
        self.workers[index] = self._spawn(old.city)
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
