# Working in this repository

## Run the quality gates before every commit

Not before a push, not at the end of a branch — before each commit, so a
bisect never lands on a broken tree:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

These are exactly what `.github/workflows/ci.yml` runs on every push to
`main` and on every pull request. A commit that fails them locally fails
there too.

Two traps worth knowing:

- **Check the exit status, not the output.** `cargo fmt --check` and
  `cargo clippy` say nothing when they pass, so a script that prints its
  own "ok" line unconditionally will claim success over a failure. Use
  `cmd && echo PASS || echo FAIL`, or just let the command's status stand.
- **Clippy stops at the first crate that fails.** A clean-looking run can
  be hiding later crates entirely; after fixing one crate, run it again
  rather than assuming the rest were fine.

## Clippy is `-D warnings`

Every lint blocks the build, so a warning has to be resolved rather than
left. Prefer changing the code. When a lint genuinely does not fit —
`clippy::too_many_arguments` on a Bevy system that has to thread
`Commands` plus several `Assets<T>` borrows, for instance — put a
targeted `#[allow(...)]` on that item with a comment saying why. Silence
the specific lint where it misfires; never widen the allow to a module or
crate, and never leave the gate red.

## Shared working trees

Several agents work this repository at once through `git worktree`, so
the build artefacts and the git stash stack are shared:

- Never use bare `git stash` / `git stash pop` — you can pop another
  agent's work. Prefer a temporary WIP commit.
- `cargo build` writes one binary that others may be running. If someone
  is testing a build, tell them before you rebuild, and leave the tree at
  a committed state rather than mid-experiment.

## Orientation

- [README.md](README.md) — what works today, how to run it, the workspace
  layout.
- [docs/architecture.md](docs/architecture.md) — crate dependency rules
  and the logical-asset pipeline. Read it before adding a dependency
  between crates: `mm2_formats` depends on nothing project-local,
  `mm2_assets` on `mm2_formats` alone, and `mm2_app` is the only crate
  where everything may meet. Converting a parsed format into a Bevy asset
  belongs in `mm2_app`, never in a parser crate.
- [docs/research/](docs/research/) — notes on the MM2 binary formats. When
  a format's meaning is inferred rather than documented, say so there and
  in the code, and prefer measuring against the retail data over guessing.

## Rendering work

The app can reproduce any view headlessly, which is the fastest way to
confirm a rendering change:

```sh
cargo run -p mm2_app --bin mm2 -- --mm2-path <install> --city sf \
    --cam=-747.5,42.4,275.0,179,-15 --frames 90 --screenshot out.png
```

The HUD prints the active camera's pose in exactly the form `--cam`
accepts, so a screenshot's file name round-trips back into the command
that produced it. Input is frozen while a `--frames` capture runs, so two
runs of the same command render the same frame — compare them directly
before and after a change.
