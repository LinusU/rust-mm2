"""PPO trainer for the neural opponent driver.

    uv run python -m mm2rl.train --run pro --workers 16

A run lives in `runs/<name>/`. Starting a run that already has a
checkpoint resumes it, with whatever worker count this invocation asks
for, so stopping and restarting costs at most the rollout in flight:

    uv run python -m mm2rl.ctl stop pro      # or Ctrl-C / SIGTERM

Each iteration collects `--horizon` decisions from every agent in every
`mm2_gym` worker, then runs a few epochs of clipped-surrogate updates.
The actor's mean is `tanh`-bounded and its exported weights are exactly
what the game evaluates (`mm2_app::neural::policy`).
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import time
from collections import Counter, defaultdict, deque
from pathlib import Path

import numpy as np
import torch
from torch import nn

from .envs import ENDS, VecEnv
from .export import export_policy

ROOT = Path(__file__).resolve().parents[2]


def mlp(sizes: list[int]) -> nn.Sequential:
    layers: list[nn.Module] = []
    for a, b in zip(sizes[:-1], sizes[1:]):
        layers += [nn.Linear(a, b), nn.Tanh()]
    return nn.Sequential(*layers)


class Agent(nn.Module):
    def __init__(self, obs_dim: int, act_dim: int, hidden: list[int], critic_hidden: list[int]):
        super().__init__()
        # Actor: every layer tanh, the last one bounding the action mean.
        self.actor = mlp([obs_dim, *hidden, act_dim])
        self.critic = nn.Sequential(
            *mlp([obs_dim, *critic_hidden]), nn.Linear(critic_hidden[-1], 1)
        )
        self.log_std = nn.Parameter(torch.full((act_dim,), -0.7))
        for module in self.modules():
            if isinstance(module, nn.Linear):
                nn.init.orthogonal_(module.weight, 2**0.5)
                nn.init.zeros_(module.bias)
        last = self.actor[-2]
        nn.init.orthogonal_(last.weight, 0.01)
        with torch.no_grad():
            # Start out on the throttle with the handbrake off.
            last.bias.copy_(torch.tensor([0.0, 0.6, -1.0])[:act_dim])
        nn.init.orthogonal_(self.critic[-1].weight, 1.0)

    def value(self, obs: torch.Tensor) -> torch.Tensor:
        return self.critic(obs).squeeze(-1)

    def dist(self, obs: torch.Tensor) -> torch.distributions.Normal:
        return torch.distributions.Normal(self.actor(obs), self.log_std.exp())


class ReturnScale:
    """Running scale of discounted returns, so the critic's targets stay
    near unit size whatever the reward weights are."""

    def __init__(self, n: int, gamma: float):
        self.gamma = gamma
        self.ret = np.zeros(n, dtype=np.float64)
        self.mean, self.var, self.count = 0.0, 1.0, 1e-4

    def resize(self, n: int) -> None:
        self.ret = np.zeros(n, dtype=np.float64)

    def update(self, reward: np.ndarray, done: np.ndarray) -> None:
        self.ret = self.ret * self.gamma + reward
        batch_mean, batch_var, batch_count = self.ret.mean(), self.ret.var(), len(self.ret)
        delta = batch_mean - self.mean
        total = self.count + batch_count
        self.mean += delta * batch_count / total
        self.var = (
            self.var * self.count + batch_var * batch_count
            + delta**2 * self.count * batch_count / total
        ) / total
        self.count = total
        self.ret[done > 0] = 0.0

    @property
    def scale(self) -> float:
        return float(np.sqrt(self.var) + 1e-8)

    def state(self) -> dict:
        return {"mean": self.mean, "var": self.var, "count": self.count}

    def load(self, state: dict) -> None:
        self.mean, self.var, self.count = state["mean"], state["var"], state["count"]


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run", required=True, help="run name under training/runs/")
    p.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 2),
                   help="mm2_gym processes, one core each")
    p.add_argument("--agents", type=int, default=32, help="cars per worker")
    p.add_argument("--cities", default="sf,london")
    p.add_argument("--install", type=Path, default=Path(os.environ.get("MM2_RETAIL", ROOT / "retail")))
    p.add_argument("--gym", type=Path, default=ROOT / "target/release/examples/mm2_gym")
    p.add_argument("--horizon", type=int, default=256, help="decisions per agent per iteration")
    p.add_argument("--epochs", type=int, default=4)
    p.add_argument("--minibatch", type=int, default=8192)
    p.add_argument("--lr", type=float, default=3e-4)
    p.add_argument("--gamma", type=float, default=0.995)
    p.add_argument("--lam", type=float, default=0.95)
    p.add_argument("--clip", type=float, default=0.2)
    p.add_argument("--entropy", type=float, default=0.002)
    p.add_argument("--vf-coef", type=float, default=0.5)
    p.add_argument("--max-grad-norm", type=float, default=0.5)
    p.add_argument("--target-kl", type=float, default=0.03,
                   help="stop an iteration's epochs early past this KL")
    p.add_argument("--hidden", default="256,256")
    p.add_argument("--critic-hidden", default="256,256")
    p.add_argument("--init", type=Path, help="start a new run from this checkpoint's weights")
    p.add_argument("--configure", help="JSON reward/start overrides sent to every worker")
    p.add_argument("--total-steps", type=float, default=float("inf"))
    p.add_argument("--save-minutes", type=float, default=5.0)
    p.add_argument("--snapshot-steps", type=float, default=25e6,
                   help="keep a numbered checkpoint this often")
    p.add_argument("--recycle-iters", type=int, default=40,
                   help="restart one worker every this many iterations")
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--device", default="cpu")
    p.add_argument("--threads", type=int, default=4, help="torch CPU threads")
    return p.parse_args()


def main() -> None:
    args = parse_args()
    run = Path(__file__).resolve().parents[1] / "runs" / args.run
    run.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(args.threads)
    torch.manual_seed(args.seed)
    device = torch.device(args.device)

    latest = run / "latest.pt"
    state = torch.load(latest, map_location="cpu", weights_only=False) if latest.exists() else None
    hidden = state["hidden"] if state else [int(x) for x in args.hidden.split(",")]
    critic_hidden = state["critic_hidden"] if state else [int(x) for x in args.critic_hidden.split(",")]

    configure = json.loads(args.configure) if args.configure else None
    env = VecEnv(
        args.gym, args.install, args.cities.split(","), args.workers, args.agents,
        # A resumed run must not replay the starts it already saw.
        seed=args.seed + (int(state["iteration"]) if state else 0),
        configure=configure,
    )
    n, obs_dim, act_dim = env.n, env.obs_dim, env.act_dim
    agent = Agent(obs_dim, act_dim, hidden, critic_hidden).to(device)
    optim = torch.optim.Adam(agent.parameters(), lr=args.lr, eps=1e-5)
    scale = ReturnScale(n, args.gamma)
    steps, iteration = 0, 0
    if state:
        agent.load_state_dict(state["agent"])
        optim.load_state_dict(state["optim"])
        for group in optim.param_groups:
            group["lr"] = args.lr
        scale.load(state["scale"])
        steps, iteration = state["steps"], state["iteration"]
        print(f"resumed {args.run} at {steps / 1e6:.2f}M steps, iteration {iteration}")
    elif args.init:
        init = torch.load(args.init, map_location="cpu", weights_only=False)
        agent.load_state_dict(init["agent"])
        scale.load(init["scale"])
        print(f"initialised from {args.init}")

    (run / "pid").write_text(str(os.getpid()))
    stop_file = run / "STOP"
    stop_file.unlink(missing_ok=True)
    stopping = False

    def request_stop(*_):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGTERM, request_stop)

    def save(path: Path) -> None:
        tmp = path.with_suffix(".tmp")
        torch.save(
            {
                "agent": agent.state_dict(),
                "optim": optim.state_dict(),
                "scale": scale.state(),
                "steps": steps,
                "iteration": iteration,
                "hidden": hidden,
                "critic_hidden": critic_hidden,
                "obs_dim": obs_dim,
                "act_dim": act_dim,
                "args": {k: str(v) for k, v in vars(args).items()},
            },
            tmp,
        )
        tmp.replace(path)
        export_policy(agent.actor, obs_dim, act_dim, path.with_suffix(".mm2p"))

    horizon = args.horizon
    obs_buf = torch.zeros((horizon, n, obs_dim))
    act_buf = torch.zeros((horizon, n, act_dim))
    logp_buf = torch.zeros((horizon, n))
    rew_buf = torch.zeros((horizon, n))
    done_buf = torch.zeros((horizon, n))
    val_buf = torch.zeros((horizon, n))

    recent: deque = deque(maxlen=4000)
    by_line: dict[str, Counter] = defaultdict(Counter)
    log = (run / "log.jsonl").open("a")
    last_save = time.time()
    next_snapshot = (steps // args.snapshot_steps + 1) * args.snapshot_steps
    obs = torch.from_numpy(env.observe().copy())
    print(f"{args.run}: {args.workers} workers × {args.agents} agents = {n}, "
          f"obs {obs_dim}, act {act_dim}, actor {hidden}, device {device}")

    while steps < args.total_steps and not stopping and not stop_file.exists():
        started = time.time()
        with torch.no_grad():
            for t in range(horizon):
                dist = agent.dist(obs.to(device))
                action = dist.sample()
                obs_buf[t] = obs
                act_buf[t] = action.cpu()
                logp_buf[t] = dist.log_prob(action).sum(-1).cpu()
                val_buf[t] = agent.value(obs.to(device)).cpu()
                next_obs, reward, done, finals, final_index, episodes = env.step(
                    action.cpu().numpy()
                )
                reward = reward.copy()
                scale.update(reward, done)
                reward /= scale.scale
                if len(final_index):
                    # An episode that was only cut short keeps the value
                    # of what would have come next.
                    cut = agent.value(torch.from_numpy(finals.copy()).to(device)).cpu().numpy()
                    reward[final_index] += args.gamma * cut
                rew_buf[t] = torch.from_numpy(reward)
                done_buf[t] = torch.from_numpy((done > 0).astype(np.float32))
                obs = torch.from_numpy(next_obs.copy())
                for e in episodes:
                    recent.append(e)
                    by_line[e.line][ENDS[e.end]] += 1
                    by_line[e.line]["metres"] += e.metres
            last_value = agent.value(obs.to(device)).cpu()

            adv_buf = torch.zeros((horizon, n))
            gae = torch.zeros(n)
            for t in reversed(range(horizon)):
                next_value = last_value if t == horizon - 1 else val_buf[t + 1]
                alive = 1.0 - done_buf[t]
                delta = rew_buf[t] + args.gamma * next_value * alive - val_buf[t]
                gae = delta + args.gamma * args.lam * alive * gae
                adv_buf[t] = gae
            ret_buf = adv_buf + val_buf
        collected = time.time() - started

        b_obs = obs_buf.reshape(-1, obs_dim).to(device)
        b_act = act_buf.reshape(-1, act_dim).to(device)
        b_logp = logp_buf.reshape(-1).to(device)
        b_adv = adv_buf.reshape(-1).to(device)
        b_ret = ret_buf.reshape(-1).to(device)
        b_val = val_buf.reshape(-1).to(device)
        batch = b_obs.shape[0]
        kl, clipped, pg_loss, v_loss, entropy = 0.0, 0.0, 0.0, 0.0, 0.0
        for _epoch in range(args.epochs):
            order = torch.randperm(batch, device=device)
            for start in range(0, batch, args.minibatch):
                idx = order[start:start + args.minibatch]
                dist = agent.dist(b_obs[idx])
                logp = dist.log_prob(b_act[idx]).sum(-1)
                ratio = (logp - b_logp[idx]).exp()
                adv = b_adv[idx]
                adv = (adv - adv.mean()) / (adv.std() + 1e-8)
                pg_loss = -torch.min(
                    ratio * adv, ratio.clamp(1 - args.clip, 1 + args.clip) * adv
                ).mean()
                value = agent.value(b_obs[idx])
                v_clip = b_val[idx] + (value - b_val[idx]).clamp(-args.clip, args.clip)
                v_loss = 0.5 * torch.max(
                    (value - b_ret[idx]) ** 2, (v_clip - b_ret[idx]) ** 2
                ).mean()
                entropy = dist.entropy().sum(-1).mean()
                loss = pg_loss + args.vf_coef * v_loss - args.entropy * entropy
                optim.zero_grad()
                loss.backward()
                nn.utils.clip_grad_norm_(agent.parameters(), args.max_grad_norm)
                optim.step()
                with torch.no_grad():
                    kl = float(((ratio - 1) - (logp - b_logp[idx])).mean())
                    clipped = float(((ratio - 1).abs() > args.clip).float().mean())
            if kl > args.target_kl:
                break
        with torch.no_grad():
            agent.log_std.clamp_(-2.5, 0.0)

        steps += batch
        iteration += 1
        explained = 1.0 - float((b_ret - b_val).var() / (b_ret.var() + 1e-8))
        ends = Counter(ENDS[e.end] for e in recent)
        total = max(1, len(recent))
        metres = sum(e.metres for e in recent)
        decisions = max(1, sum(e.steps for e in recent))
        row = {
            "steps": steps,
            "iteration": iteration,
            "fps": round(batch / (time.time() - started)),
            "collect_fps": round(batch / collected),
            "ep_return": float(np.mean([e.ret for e in recent])) if recent else 0.0,
            "ep_metres": metres / total,
            # Mean speed over whole episodes, standstills included (m/s at 20 Hz).
            "speed": metres / decisions * 20.0,
            "impact_per_km": sum(e.impacts for e in recent) / max(1.0, metres) * 1000.0,
            **{f"end_{k}": ends.get(k, 0) / total for k in ENDS},
            "kl": kl,
            "clipped": clipped,
            "pg_loss": float(pg_loss.detach()),
            "v_loss": float(v_loss.detach()),
            "entropy": float(entropy.detach()),
            "explained": explained,
            "std": [round(float(x), 3) for x in agent.log_std.exp()],
            "ret_scale": scale.scale,
            "time": time.time(),
        }
        log.write(json.dumps(row) + "\n")
        log.flush()
        if iteration % 5 == 0 or iteration < 5:
            print(
                f"{steps / 1e6:8.2f}M it {iteration:5d} {row['fps']:6d} fps "
                f"ret {row['ep_return']:7.2f} m {row['ep_metres']:6.0f} "
                f"v {row['speed']:5.1f} imp/km {row['impact_per_km']:6.1f} "
                f"fin {row['end_finished'] + row['end_time']:.2f} "
                f"stall {row['end_stalled']:.2f} off {row['end_off_route']:.2f} "
                f"up {row['end_upended']:.2f} lost {row['end_lost']:.2f} "
                f"kl {kl:.4f} ev {explained:.2f} std {row['std']}",
                flush=True,
            )

        if args.recycle_iters and iteration % args.recycle_iters == 0:
            index = (iteration // args.recycle_iters) % len(env.workers)
            env.recycle(index)
            obs = torch.from_numpy(env.observe().copy())
            scale.resize(n)
        if time.time() - last_save > args.save_minutes * 60:
            save(latest)
            (run / "lines.json").write_text(json.dumps(by_line, indent=1, sort_keys=True))
            last_save = time.time()
        if steps >= next_snapshot:
            save(run / f"ckpt_{int(next_snapshot / 1e6):05d}M.pt")
            next_snapshot += args.snapshot_steps

    save(latest)
    (run / "lines.json").write_text(json.dumps(by_line, indent=1, sort_keys=True))
    (run / "pid").unlink(missing_ok=True)
    env.close()
    print(f"{args.run}: stopped at {steps / 1e6:.2f}M steps, checkpoint saved")


if __name__ == "__main__":
    main()
