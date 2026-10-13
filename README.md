<p align="center">
  <img src=".github/assets/logo.svg" width="128" alt="Splay logo">
</p>

<h1 align="center">Splay</h1>
<p align="center"><em>Fan out your agents.</em></p>

Splay is a workspace manager for parallel coding agent development, similar to [cmux](https://github.com/manaflow-ai/cmux), [Superset](https://github.com/superset-sh/superset), [Conductor](https://www.conductor.build/), and [Orca](https://github.com/stablyai/orca). Register git repositories, spin up isolated worktrees, and run persistent terminal sessions inside them from a native GTK desktop app or the CLI.

Splay was previously called Swarm. Read the [announcement blog post](https://penberg.org/blog/swarm.html) for more background.

## Features

- **Works with any coding agent** — run Claude Code, Codex, OpenCode, or any other CLI tool inside a session. Sessions are just terminals, so anything that runs in a shell runs under Splay.
- **Workspace per feature using git worktrees** — every workspace is an isolated git worktree, so parallel agents never share a checkout or step on each other's branches.
- **Fast workspace cloning** — branch off an existing workspace into a new worktree and branch in one command.
- **Multi-repository management** — register any number of GitHub repositories and manage their workspaces and sessions from one place.
- **Ghostty-backed persistent terminals** — sessions are backed by `libghostty-vt` and survive disconnects; detach and reattach from the GUI or CLI without killing the process.

## Installation

Splay runs on Linux (x86-64, glibc 2.39 or newer, GTK 4). Install the latest release with:

```console
curl -fsSL https://splay.sh/install.sh | sh
```

This puts `splay` and `splayctl` in `~/.local/bin` and adds Splay to your app launcher.

## Getting started

Start the GUI with:

```console
splay
```

<p align="center">
  <img src=".github/assets/hero.png" width="760" alt="Splay">
</p>

Or drive Splay from the command line with `splayctl`:

```console
splayctl --help
```

For more information, check out the [documentation section](#documentation).

## Motivation

Coding agents are most effective when you can run many of them in parallel — one per feature, one per bug fix. But in practice, this quickly turns into chaos. Each agent needs its own copy of the source code so they don't step on each other's changes, and each one runs in its own terminal. Before long, you're drowning in terminal tabs, losing track of which agent is working on what, and manually juggling git branches and directories.

Splay exists to tame that. It gives you a single place to manage all your repositories, workspaces, and agent sessions — so you can scale up the number of parallel agents without the overhead of keeping it all organized yourself.

## How it works

Splay organizes work into **repositories**, **workspaces**, and **sessions**. A repository is the git repository that holds your project's source code. A workspace is an isolated copy of that source code, backed by a git worktree, where work happens independently without interfering with other workspaces. A session is a persistent terminal environment running inside a workspace — it could be a coding agent, a shell, or any long-running command.

For example, a typical **workspace-per-feature** workflow looks like this: create a workspace for every feature or bug fix, let a coding agent work on it in a session, submit a pull request, address review comments and iterate — all within the same workspace. When the work is merged to mainline, remove the workspace.

## FAQ

### How is Splay different from _X_?

Splay manages git worktrees and runs terminal sessions in them, and nothing else. The other tools bundle more into the app: Superset has a diff viewer and editor, cmux has a browser, Conductor has a review workflow, and Orca has an editor and a browser. With Splay, you use whatever tools you already run in a terminal.

The tools also differ in which platforms they run on, how they are built, and how they are licensed:

| | Runs on | Built with | License | Notes |
|---|---|---|---|---|
| [Superset](https://github.com/superset-sh/superset) | <img src=".github/assets/apple.svg" height="16" alt="macOS" title="macOS"> <img src=".github/assets/linux.svg" height="16" alt="Linux" title="Linux"> | Electron | ELv2 | Requires an account |
| [cmux](https://github.com/manaflow-ai/cmux) | <img src=".github/assets/apple.svg" height="16" alt="macOS" title="macOS"> | Swift | GPL-3.0 | No worktree management |
| [Conductor](https://www.conductor.build/) | <img src=".github/assets/apple.svg" height="16" alt="macOS" title="macOS"> | | Proprietary | No CLI |
| [Orca](https://github.com/stablyai/orca) | <img src=".github/assets/apple.svg" height="16" alt="macOS" title="macOS"> <img src=".github/assets/linux.svg" height="16" alt="Linux" title="Linux"> <img src=".github/assets/windows.svg" height="16" alt="Windows" title="Windows"> | Electron | MIT | Sends telemetry |
| Splay | <img src=".github/assets/linux.svg" height="16" alt="Linux" title="Linux"> | GTK 4 | MIT | |

### I used Swarm. What happens to my workspaces?

Nothing you need to do by hand. The first time Splay starts, it moves `~/.local/share/swarm` to `~/.local/share/splay`, updates the stored paths, and repairs the git worktrees. Stop any sessions still running under Swarm first. See [Migrating from Swarm](MANUAL.md#migrating-from-swarm) for details.

## Building from source

The GTK desktop app depends on GTK 4 development headers and a Zig toolchain for the vendored `libghostty-vt` build. Splay currently requires `zig 0.15.2`.

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

## License

Splay is available under the [MIT license](LICENSE.md).
