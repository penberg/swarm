# Swarm Reference Manual

## Repositories

A repository is the top-level codebase record in `swarm`.

Each repository maps to one remote git repository hosted on a forge.

The canonical repository identifier is:

```text
host/owner/name
```

Example: `github.com/penberg/swarm`

`swarm repo` commands:
- register a repository with `swarm`
- remove a repository from `swarm`
- list repositories known to `swarm`

Repository commands do not create workspaces and do not start agents.

## Workspaces

A workspace is a local checkout for a registered repository.

Each workspace belongs to exactly one repository and is backed by a git worktree on disk.

`swarm workspace` commands:
- create a new local worktree for a repository
- list workspaces for a repository
- inspect workspace metadata
- remove a workspace

## Sessions

A session is a command environment attached to one workspace.

Sessions are the operational unit that `swarm` runs, attaches to, and eventually shows as terminal tabs in the TUI.

Each session belongs to exactly one workspace.

`swarm session` commands:
- create a new session for a workspace
- list sessions
- inspect session metadata
- attach to an existing session
- stop a running session

## Prune

`swarm prune` removes local state that is no longer useful.

`swarm prune` commands:
- prune stopped, failed, or exited sessions

## On-Disk Format

`swarm` stores persistent state in the XDG data directory:

```text
~/.local/share/swarm/
```

Layout:

```text
~/.local/share/swarm/
  index.db
  repos/
    <host>/
      <owner>/
        <name>/
          meta.toml
          repo.db
          workspaces/
            <workspace-name>/
          sessions/
            <session-id>/
              meta.toml
              log
```

### `index.db`

Global repository index. Used by `swarm repo list`, `swarm repo add`, and `swarm repo remove`.

Stores one row per repository:
- `host`
- `owner`
- `name`
- `alias`
- `created_at`

Canonical identity: the `(host, owner, name)` tuple. Aliases are shortcuts, not primary identifiers.

### `repos/<host>/<owner>/<name>/`

Per-repository directory. Example:

```text
~/.local/share/swarm/repos/github.com/penberg/swarm/
```

Contains repository-specific state and worktree directories.

### `meta.toml`

Repository metadata. Example:

```toml
host = "github.com"
owner = "penberg"
name = "swarm"
canonical = "github.com/penberg/swarm"
alias = "swarm"
```

### `repo.db`

Repository-local database. Stores:
- workspaces
- sessions
- artifacts
- event history

### `workspaces/<workspace-name>/`

Git worktree checkout directories. Example:

```text
~/.local/share/swarm/repos/github.com/penberg/swarm/workspaces/main/
```

The workspace name is the identifier within that repository.

### `sessions/<session-id>/`

Per-session runtime state. Example:

```text
~/.local/share/swarm/repos/github.com/penberg/swarm/sessions/01JSESSIONEXAMPLE/
```

This directory is intended to hold session-local files such as:
- metadata
- logs
- PTY or attachment state

Sessions are stored under the repository shard because they are attached to workspaces in that repository.

### Notes

- `index.db` is global. `repo.db` is per-repository.
- On-disk paths derive from the canonical identifier, not the alias.
- Workspace paths derive from the workspace name.
- Session paths derive from a session identifier.

## Command Line Reference

### `swarm session`

Manage workspace sessions.

Future TUI terminal tabs should map to sessions, not directly to workspaces.

### `swarm session create`

Create a session for a workspace.

#### Usage

```text
swarm session create <workspace> -- <command> [args...]
```

#### Arguments

- `<workspace>`: Workspace reference.

#### Behavior

- Resolves the target workspace.
- Creates a session record in `repo.db`.
- Starts the requested command in the workspace directory.
- Persists enough metadata for later inspection or attachment.

#### Examples

```text
swarm session create swarm/main -- bash
swarm session create swarm/github-actions -- cargo test
```

### `swarm session list`

List sessions.

#### Usage

```text
swarm session list
swarm session list <workspace>
swarm session list --json
```

#### Arguments

- `<workspace>`: Optional workspace reference.

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Lists sessions known to local `swarm` state.
- When `<workspace>` is provided, filters to that workspace.

### `swarm session info`

Show metadata for one session.

#### Usage

```text
swarm session info <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Prints session metadata such as workspace, command, status, and timestamps.

### `swarm session attach`

Attach to an existing session.

#### Usage

```text
swarm session attach <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Connects to a running session.
- Intended for interactive terminal use.
- Press `Ctrl-]` to detach without terminating the session.
- `Ctrl-D` is passed through to the attached process and may cause shells to exit.

### `swarm session stop`

Stop a running session.

#### Usage

```text
swarm session stop <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Requests session termination.
- Marks the session as stopped in local state.

### `swarm prune`

Prune local state.

### `swarm prune sessions`

Prune stopped, failed, or exited sessions.

#### Usage

```text
swarm prune sessions
```

#### Behavior

- Scans all repositories known to local `swarm` state.
- Removes sessions whose status is `stopped`, `failed`, or `exited`.
- Leaves `starting` and `running` sessions untouched.

#### Expected Output

```text
Pruned 2 sessions
```

### `swarm workspace`

Manage repository workspaces. Alias: `swarm ws`.

### `swarm workspace create`

Create a workspace for a repository.

#### Usage

```text
swarm workspace create <repository> [name]
swarm ws create <repository> [name]
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.
- `[name]`: Optional workspace name.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Creates a workspace record in `repo.db`.
- Creates a git worktree under `repos/<host>/<owner>/<name>/workspaces/<workspace-name>/`.
- Rejects duplicate workspace names within the same repository.

#### Examples

```text
swarm workspace create swarm
swarm workspace create swarm feature-x
swarm ws create github.com/penberg/swarm review-docs
```

#### Expected Output

```text
Created workspace main for swarm
```

### `swarm workspace list`

List workspaces for a repository.

#### Usage

```text
swarm workspace list <repository>
swarm ws list <repository>
swarm workspace list <repository> --json
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Reads workspace records from `repo.db`.
- Prints one workspace per row.

#### Examples

```text
swarm workspace list swarm
swarm ws list github.com/penberg/swarm --json
```

### `swarm workspace info`

Show metadata for one workspace.

#### Usage

```text
swarm workspace info <workspace>
swarm ws info <workspace>
```

#### Arguments

- `<workspace>`: Workspace reference.

#### Behavior

- Prints workspace metadata: repository, name, path, creation time.

### `swarm workspace remove`

Remove a workspace.

#### Usage

```text
swarm workspace remove <workspace>
swarm ws remove <workspace>
```

#### Arguments

- `<workspace>`: Workspace reference.

#### Behavior

- Removes the workspace record from the repository-local database.
- Removes the corresponding git worktree.
- Removes the workspace directory on disk.

### `swarm repo add`

Register a repository with `swarm`.

#### Usage

```text
swarm repo add <host/owner/name> [--alias <name>]
```

#### Arguments

- `<host/owner/name>`: Canonical repository identifier.
  `swarm` also accepts pasted repository URLs such as `https://github.com/owner/repo`,
  `git@github.com:owner/repo.git`, and GitHub links with extra path segments like pull requests.

#### Options

- `--alias <name>`: Optional local shorthand for the repository. If omitted, `swarm` uses the repository name as the default alias.

#### Behavior

- Validates `host/owner/name` format.
- Normalizes pasted repository URLs to the canonical `host/owner/name` format.
- Creates a repository record in `index.db`.
- Defaults the alias to the repository name if `--alias` is omitted.
- Rejects duplicates.

#### Examples

```text
swarm repo add github.com/penberg/swarm
swarm repo add https://github.com/penberg/swarm
swarm repo add https://github.com/penberg/swarm/pull/123
swarm repo add github.com/penberg/other --alias other
```

#### Expected Output

```text
Added repo swarm
```

### `swarm repo list`

List repositories registered with `swarm`.

#### Usage

```text
swarm repo list
swarm repo list --json
```

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Reads repository records from `index.db`.
- Prints one repository per row.

#### Examples

```text
swarm repo list
swarm repo list --json
```

#### Expected Output

```text
ALIAS            REPOSITORY
swarm            github.com/penberg/swarm
```

### `swarm repo remove`

Remove a repository from `swarm`.

#### Usage

```text
swarm repo remove <repository>
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Removes the row from `index.db`.
- Removes the repository directory under `repos/<host>/<owner>/<name>/`.

#### Examples

```text
swarm repo remove swarm
swarm repo remove github.com/penberg/swarm
```

#### Expected Output

```text
Removed repo swarm
```

## Exit Codes

- `0`: Command succeeded.
- `1`: Runtime or IO failure.
- `2`: Invalid arguments or unknown command usage.
- `3`: Repository not found.
- `4`: Repository already exists or alias conflict.
