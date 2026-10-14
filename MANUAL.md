# Splay Reference Manual

## Repositories

A repository is the top-level codebase record in `splay`.

Each repository maps to one remote git repository hosted on a forge.

The canonical repository identifier is:

```text
host/owner/name
```

Example: `github.com/splay-sh/splay`

`splay repo` commands:
- register a repository with `splay`
- sync a repository's local bare clone from the remote
- remove a repository from `splay`
- archive a repository to hide it from the sidebar, and unarchive it
- list repositories known to `splay`

Repository commands do not create workspaces or sessions.

## Workspaces

A workspace is a local checkout for a registered repository.

Each workspace belongs to exactly one repository and is backed by a git worktree on disk.

`splay workspace` commands:
- create a new local worktree for a repository
- clone a workspace into a new local worktree
- list workspaces for a repository
- inspect workspace metadata
- remove a workspace

## Sessions

A session is a command environment attached to one workspace.

Sessions are the operational unit that `splay` runs, attaches to, and eventually shows as terminal tabs in the TUI.

Each session belongs to exactly one workspace.

`splay session` commands:
- create a new session for a workspace
- list sessions
- inspect session metadata
- attach to an existing session
- stop a running session

## Prune

`splay prune` removes local state that is no longer useful.

`splay prune` commands:
- prune stopped, failed, or exited sessions

## On-Disk Format

`splay` stores persistent state in the XDG data directory:

```text
~/.local/share/splay/
```

Layout:

```text
~/.local/share/splay/
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

Global repository index. Used by `splay repo list`, `splay repo add`, and `splay repo remove`.

Stores one row per repository:
- `host`
- `owner`
- `name`
- `alias`
- `created_at`
- `last_used_at`: Unix timestamp of when the repository was added or one of its workspaces was last selected in the UI, or `NULL`.
- `archived_at`: Unix timestamp of when the repository was archived, or `NULL`.

Canonical identity: the `(host, owner, name)` tuple. Aliases are shortcuts, not primary identifiers.

Also stores a `workspace_usage` table with one row per workspace selected in the UI:
- `repository`: canonical `host/owner/name`
- `workspace`: workspace name
- `last_used_at`: Unix timestamp of when the workspace was last selected

### `repos/<host>/<owner>/<name>/`

Per-repository directory. Example:

```text
~/.local/share/splay/repos/github.com/splay-sh/splay/
```

Contains repository-specific state and worktree directories.

### `meta.toml`

Repository metadata. Example:

```toml
host = "github.com"
owner = "penberg"
name = "splay"
canonical = "github.com/splay-sh/splay"
alias = "splay"
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
~/.local/share/splay/repos/github.com/splay-sh/splay/workspaces/main/
```

The workspace name is the identifier within that repository.

### `sessions/<session-id>/`

Per-session runtime state. Example:

```text
~/.local/share/splay/repos/github.com/splay-sh/splay/sessions/01JSESSIONEXAMPLE/
```

This directory is intended to hold session-local files such as:
- metadata
- logs
- PTY or attachment state
- how the session's command exited (`exit`), which the session's supervisor
  records instead of writing to the database

Sessions are stored under the repository shard because they are attached to workspaces in that repository.

### Notes

- `index.db` is global. `repo.db` is per-repository.
- On-disk paths derive from the canonical identifier, not the alias.
- Workspace paths derive from the workspace name.
- Session paths derive from a session identifier.

### Migrating from Swarm

Splay was previously called Swarm and kept its state in `~/.local/share/swarm/`.
The first time `splay` opens its state and finds only the old
directory, it:

- moves `~/.local/share/swarm/` to `~/.local/share/splay/`,
- rewrites the absolute workspace and session paths stored in each `repo.db`,
- runs `git worktree repair` so worktrees and their bare clone point at each other again, and
- leaves a `~/.local/share/swarm` symlink to the new directory.

The migration refuses to run while sessions are still running from the old
directory. Stop them first, then start Splay again.

## Command Line Reference

### `splay session`

Manage workspace sessions.

Future TUI terminal tabs should map to sessions, not directly to workspaces.

### `splay session create`

Create a session for a workspace.

#### Usage

```text
splay session create <workspace> -- <command> [args...]
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
splay session create splay/main -- bash
splay session create splay/github-actions -- cargo test
```

### `splay session list`

List sessions.

#### Usage

```text
splay session list
splay session list <workspace>
splay session list --json
```

#### Arguments

- `<workspace>`: Optional workspace reference.

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Lists sessions known to local `splay` state.
- When `<workspace>` is provided, filters to that workspace.

### `splay session info`

Show metadata for one session.

#### Usage

```text
splay session info <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Prints session metadata such as workspace, command, status, and timestamps.

### `splay session attach`

Attach to an existing session.

#### Usage

```text
splay session attach <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Connects to a running session.
- Intended for interactive terminal use.
- Press `Ctrl-]` to detach without terminating the session.
- `Ctrl-D` is passed through to the attached process and may cause shells to exit.

### `splay session stop`

Stop a running session.

#### Usage

```text
splay session stop <session>
```

#### Arguments

- `<session>`: Session identifier.

#### Behavior

- Requests session termination.
- Marks the session as stopped in local state.

### `splay prune`

Prune local state.

### `splay prune sessions`

Prune stopped, failed, or exited sessions.

#### Usage

```text
splay prune sessions
```

#### Behavior

- Scans all repositories known to local `splay` state.
- Removes sessions whose status is `stopped`, `failed`, or `exited`.
- Leaves `starting` and `running` sessions untouched.

#### Expected Output

```text
Pruned 2 sessions
```

### `splay workspace`

Manage repository workspaces. Alias: `splay ws`.

### `splay workspace create`

Create a workspace for a repository.

#### Usage

```text
splay workspace create <repository> [name]
splay ws create <repository> [name]
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.
- `[name]`: Optional workspace name.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Creates a workspace record in `repo.db`.
- Creates a git worktree under `repos/<host>/<owner>/<name>/workspaces/<workspace-name>/`.
- Starts a default session for the new workspace using the user's login shell.
- Rejects duplicate workspace names within the same repository.

#### Examples

```text
splay workspace create splay
splay workspace create splay feature-x
splay ws create github.com/splay-sh/splay review-docs
```

#### Expected Output

```text
Created workspace main for splay
Created session 01JSESSIONEXAMPLE
```

### `splay workspace clone`

Clone a workspace into a new workspace.

#### Usage

```text
splay workspace clone <workspace> <name>
splay ws clone <workspace> <name>
```

#### Arguments

- `<workspace>`: Source workspace reference.
- `<name>`: New workspace name.

#### Behavior

- Resolves the source workspace.
- Creates a new git worktree under `repos/<host>/<owner>/<name>/workspaces/<workspace-name>/`.
- Creates a new branch named `<name>` from the source workspace's current committed `HEAD`.
- Starts a default session for the cloned workspace using the user's login shell.
- Rejects duplicate workspace names within the same repository.

#### Examples

```text
splay workspace clone splay:main feature-x
splay ws clone splay/bugfix bugfix-copy
```

#### Expected Output

```text
Cloned workspace splay:main to feature-x for splay
Created session 01JSESSIONEXAMPLE
```

### `splay workspace list`

List workspaces for a repository.

#### Usage

```text
splay workspace list <repository>
splay ws list <repository>
splay workspace list <repository> --json
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Reads workspace records from `repo.db`.
- Prints one workspace per row, newest first.

#### Examples

```text
splay workspace list splay
splay ws list github.com/splay-sh/splay --json
```

### `splay workspace info`

Show metadata for one workspace.

#### Usage

```text
splay workspace info <workspace>
splay ws info <workspace>
```

#### Arguments

- `<workspace>`: Workspace reference.

#### Behavior

- Prints workspace metadata: repository, name, path, creation time.

### `splay workspace remove`

Remove a workspace.

#### Usage

```text
splay workspace remove <workspace>
splay ws remove <workspace>
```

#### Arguments

- `<workspace>`: Workspace reference.

#### Behavior

- Removes the workspace record from the repository-local database.
- Removes the corresponding git worktree.
- Removes the workspace directory on disk.

### `splay repo add`

Register a repository with `splay`.

#### Usage

```text
splay repo add <host/owner/name|remote-url> [--alias <name>]
```

#### Arguments

- `<host/owner/name|remote-url>`: Canonical repository identifier or full git remote URL.

#### Options

- `--alias <name>`: Optional local shorthand for the repository. If omitted, `splay` uses the repository name as the default alias.

#### Behavior

- Validates `host/owner/name` format or parses a full git remote URL.
- Creates a repository record in `index.db`.
- Stores the exact remote URL when one is provided explicitly.
- Defaults the alias to the repository name if `--alias` is omitted.
- Rejects duplicates.

#### Examples

```text
splay repo add github.com/splay-sh/splay
splay repo add github.com/penberg/other --alias other
splay repo add git@github.com:penberg/private-repo.git --alias private
```

#### Expected Output

```text
Added repo splay
```

### `splay repo list`

List repositories registered with `splay`.

#### Usage

```text
splay repo list
splay repo list --json
```

#### Options

- `--json`: Emit machine-readable output.

#### Behavior

- Reads repository records from `index.db`.
- Prints one repository per row, including archived repositories, which are marked `(archived)`.

#### Examples

```text
splay repo list
splay repo list --json
```

#### Expected Output

```text
ALIAS            REPOSITORY
splay            github.com/splay-sh/splay
```

### `splay repo sync`

Sync a registered repository from its remote.

#### Usage

```text
splay repo sync <repository>
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Creates or updates the local bare clone at `repos/<host>/<owner>/<name>/source.git`.
- Fetches all remotes and prunes deleted refs when the bare clone already exists.

#### Examples

```text
splay repo sync splay
splay repo sync github.com/splay-sh/splay
```

#### Expected Output

```text
Synced repo splay
```

### `splay repo remove`

Remove a repository from `splay`.

#### Usage

```text
splay repo remove <repository>
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Removes the row from `index.db`.
- Removes the repository directory under `repos/<host>/<owner>/<name>/`.

#### Examples

```text
splay repo remove splay
splay repo remove github.com/splay-sh/splay
```

#### Expected Output

```text
Removed repo splay
```

### `splay repo archive`

Archive a repository. Archived repositories are not shown in the sidebar.

#### Usage

```text
splay repo archive <repository>
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Sets `archived_at` in `index.db`.
- Keeps the repository directory, workspaces and sessions.

The repository menu in the sidebar has the same action.

#### Expected Output

```text
Archived repo splay
```

### `splay repo unarchive`

Unarchive a repository, showing it in the sidebar again.

#### Usage

```text
splay repo unarchive <repository>
```

#### Arguments

- `<repository>`: Repository alias or canonical `host/owner/name`.

#### Behavior

- Resolves the repository by alias first, then by canonical identifier.
- Clears `archived_at` in `index.db`.

#### Expected Output

```text
Unarchived repo splay
```

## Exit Codes

- `0`: Command succeeded.
- `1`: Runtime or IO failure.
- `2`: Invalid arguments or unknown command usage.
- `3`: Repository not found.
- `4`: Repository already exists or alias conflict.
