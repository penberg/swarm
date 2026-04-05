<p align="center">
  <img src=".github/assets/hero.png" width="600" alt="Swarm">
</p>

<h1 align="center">Swarm</h1>
<p align="center"><em>Herd your agents, not your terminals.</em></p>

A workspace manager for parallel development. Register git repositories, spin up isolated worktrees, and run persistent terminal sessions inside them — from the CLI or a native GTK desktop app.

## Installation

Install the latest prebuilt release on supported platforms with:

```console
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/penberg/swarm/releases/latest/download/swarm-installer.sh | sh
```

Then start the GUI with:

```console
swarm
```

## Motivation

Coding agents are most effective when you can run many of them in parallel — one per feature, one per bug fix. But in practice, this quickly turns into chaos. Each agent needs its own copy of the source code so they don't step on each other's changes, and each one runs in its own terminal. Before long, you're drowning in terminal tabs, losing track of which agent is working on what, and manually juggling git branches and directories.

Swarm exists to tame that. It gives you a single place to manage all your repositories, workspaces, and agent sessions — so you can scale up the number of parallel agents without the overhead of keeping it all organized yourself.

## How it works

Swarm organizes work into **repositories**, **workspaces**, and **sessions**. A repository is the git repository that holds your project's source code. A workspace is an isolated copy of that source code, backed by a git worktree, where work happens independently without interfering with other workspaces. A session is a persistent terminal environment running inside a workspace — it could be a coding agent, a shell, or any long-running command.

For example, a typical **workspace-per-feature** workflow looks like this: create a workspace for every feature or bug fix, let a coding agent work on it in a session, submit a pull request, address review comments and iterate — all within the same workspace. When the work is merged to mainline, remove the workspace.

## Building from source

The GTK desktop app depends on GTK 4 development headers and a Zig toolchain for the vendored `libghostty-vt` build. Swarm currently requires `zig 0.15.2`.

On Ubuntu/Debian, install the same system packages used by the release build:

```sh
sudo apt install gcc g++ libcairo2-dev libglib2.0-dev libgtk-4-dev libpango1.0-dev pkg-config
```

Then install Zig 0.15.2 and make sure `zig` is on your `PATH`.

Install the program with:

```sh
cargo install --path .
```

## Documentation

See [MANUAL.md](MANUAL.md) for the full reference.

### CLI Only Quickstart

Using the `swarmctl` CLI tool allows building with `--no-default-features` (avoids the GTK/etc deps).
Usage of the tool then as follows from the CLI:
```
# install to ~/.cargo/bin/swarmctl, add to PATH if you want
cargo install --path . 

# using a repo of mine to make it clear what is 'swarm' related and what isn't:
~/.cargo/bin/swarmctl repo add github.com/harryhaaren/linwhisper
~/.cargo/bin/swarmctl repo list
~/.cargo/bin/swarmctl ws create linwhisper
~/.cargo/bin/swarmctl ws list
~/.cargo/bin/swarmctl ws list linwhisper
~/.cargo/bin/swarmctl session create linwhisper:workspace -- bash
~/.cargo/bin/swarmctl session list
~/.cargo/bin/swarmctl session attach <sessionID>    # eg "s69d2270221a"
~/.cargo/bin/swarmctl session list
~/.cargo/bin/swarmctl session info
~/.cargo/bin/swarmctl session info s69d2270221a
~/.cargo/bin/swarmctl session stop s69d2270221a
~/.cargo/bin/swarmctl session info
~/.cargo/bin/swarmctl session list
~/.cargo/bin/swarmctl session prune
~/.cargo/bin/swarmctl session list
```

## License

Swarm is available under the [MIT license](LICENSE.md).
