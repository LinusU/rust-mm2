"""Carry a checkpoint over to a changed observation layout.

    uv run python -m mm2rl.reshape old.pt new.pt <new-size> <map>

`<map>` lists, for each new observation column, the old column it
continues (`a-b` for a run of columns) or `x` for a column the old
layout did not have (`xN` for N of them). New columns start with zero
weights, so the converted network computes exactly what the old one did
until training teaches it to use them. Optimiser state is dropped.

Example — layout 2 (118) to layout 3 (126), where the 16 rays at
columns 60–75 became 24:

    uv run python -m mm2rl.reshape v3.pt v4.pt 126 \\
        0-59 x 60 x 61 62 x2 63 64 x2 65 66 67 68 x2 69-75 76-117

and layout 3 to layout 4 (132), which samples the guide at two more
distances, six columns after the old guide block's last:

    uv run python -m mm2rl.reshape v4.pt v5.pt 132 0-59 x6 60-125
"""

from __future__ import annotations

import sys

import torch


def parse(spec: list[str]) -> list[int]:
    columns: list[int] = []
    for token in spec:
        if token.startswith("x"):
            columns += [-1] * int(token[1:] or 1)
        elif "-" in token:
            a, b = token.split("-")
            columns += list(range(int(a), int(b) + 1))
        else:
            columns.append(int(token))
    return columns


def main() -> None:
    source, target, size = sys.argv[1], sys.argv[2], int(sys.argv[3])
    columns = parse(sys.argv[4:])
    if len(columns) != size:
        sys.exit(f"the map names {len(columns)} columns, the new layout has {size}")
    state = torch.load(source, map_location="cpu", weights_only=False)
    old = state["obs_dim"]
    kept = [c for c in columns if c >= 0]
    if max(kept) >= old or len(set(kept)) != len(kept):
        sys.exit("the map repeats a column or names one the old layout lacks")
    agent = state["agent"]
    for name in ("actor.0.weight", "critic.0.weight"):
        weight = agent[name]
        new = torch.zeros((weight.shape[0], size), dtype=weight.dtype)
        for column, source_column in enumerate(columns):
            if source_column >= 0:
                new[:, column] = weight[:, source_column]
        agent[name] = new
    state["obs_dim"] = size
    state.pop("optim", None)
    torch.save(state, target)
    print(f"{target}: {old} -> {size} columns, {size - len(kept)} new, "
          f"{old - len(kept)} dropped, {state['steps'] / 1e6:.1f}M steps")


if __name__ == "__main__":
    main()
