"""Write an actor network as the `.mm2p` file the game loads.

    uv run python -m mm2rl.export runs/pro/latest.pt out.mm2p

Layout (little-endian), read by `mm2_app::neural::policy`: `b"MM2P"`,
`u32` version, observation size, action size, layer count; then per
layer `u32` inputs, `u32` outputs, `outputs × inputs` `f32` weights
row-major and `outputs` `f32` biases. Every layer is followed by `tanh`.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

import torch
from torch import nn

VERSION = 1


def export_policy(actor: nn.Sequential, obs_dim: int, act_dim: int, path: Path) -> None:
    layers = [m for m in actor if isinstance(m, nn.Linear)]
    out = bytearray(b"MM2P")
    out += struct.pack("<4I", VERSION, obs_dim, act_dim, len(layers))
    for layer in layers:
        weight = layer.weight.detach().cpu().float().contiguous()
        out += struct.pack("<2I", weight.shape[1], weight.shape[0])
        out += weight.numpy().astype("<f4").tobytes()
        out += layer.bias.detach().cpu().float().numpy().astype("<f4").tobytes()
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_bytes(out)
    tmp.replace(path)


def main() -> None:
    from .train import Agent

    checkpoint, target = Path(sys.argv[1]), Path(sys.argv[2])
    state = torch.load(checkpoint, map_location="cpu", weights_only=False)
    agent = Agent(state["obs_dim"], state["act_dim"], state["hidden"], state["critic_hidden"])
    agent.load_state_dict(state["agent"])
    export_policy(agent.actor, state["obs_dim"], state["act_dim"], target)
    print(f"{target}: {target.stat().st_size} bytes, {state['steps'] / 1e6:.1f}M steps")


if __name__ == "__main__":
    main()
