# Splay Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Draw the window title bar dark to match the rest of the window instead of GTK's default gray. ([#75](https://github.com/splay-sh/splay/pull/75))

## [0.2.0] - 2026-10-12

Swarm is now Splay. The binaries are `splay` and `splayctl`, and existing
state moves over automatically on first start.

### Added

- Collapse or expand all repositories with one sidebar toggle. ([#71](https://github.com/splay-sh/splay/pull/71))
- Name the workspace in the empty terminal placeholder. ([#71](https://github.com/splay-sh/splay/pull/71))

### Changed

- Rename the project from Swarm to Splay. The binaries are now `splay` and `splayctl`, and the GTK application ID is `com.penberg.splay.ui`.
- Move state from `~/.local/share/swarm` to `~/.local/share/splay` automatically on first start, rewriting stored paths and repairing git worktrees. Stop running Swarm sessions before upgrading.
- New app icon: the four-pane Splay tile.
- Sort sidebar repositories by name. ([#71](https://github.com/splay-sh/splay/pull/71))
- Fold repositories and workspaces not used in the last 7 days behind expanders, and list workspaces newest first. `splayctl workspace list` also prints newest first. ([#71](https://github.com/splay-sh/splay/pull/71))
- Do not select a workspace by default at startup or after removing the selected one. ([#71](https://github.com/splay-sh/splay/pull/71))
- Run UI actions such as creating and closing terminals, creating workspaces, syncing repositories and looking up pull requests off the GTK main thread, so the window stays responsive. ([#70](https://github.com/splay-sh/splay/pull/70))
- Link `libghostty-vt` statically from upstream instead of a vendored copy. ([#56](https://github.com/splay-sh/splay/pull/56))

### Fixed

- Update pull request status indicators in place instead of rebuilding the sidebar, which made it flicker. ([#71](https://github.com/splay-sh/splay/pull/71))
- Clean up archived workspaces whose worktree git no longer recognizes, instead of failing to prune.
- Report which workspaces could not be pruned and why, and keep pruning the rest.
- Launch the installed `splay` binary from the desktop entry.

## [0.1.5] - 2026-04-22

### Added

- Agent CI Docker image manifest. ([#62](https://github.com/penberg/swarm/pull/62))

### Changed

- Refactor session toolchain setup. ([#60](https://github.com/penberg/swarm/pull/60))

### Fixed

- Prevent UI terminal hangs under heavy logging. ([#63](https://github.com/penberg/swarm/pull/63))
- Recover terminal after socket disconnect. ([#64](https://github.com/penberg/swarm/pull/64))

## [0.1.4] - 2026-04-10

### Changed

- Share Cargo target dir across worktrees. ([#55](https://github.com/penberg/swarm/pull/55))

### Fixed

- Fix terminal context menu popover lifecycle. ([#57](https://github.com/penberg/swarm/pull/57))

## [0.1.3] - 2026-04-07

### Added

- Mouse selection and copy in the terminal view. ([#54](https://github.com/penberg/swarm/pull/54))
- Create PR button to the workspace toolbar. ([#52](https://github.com/penberg/swarm/pull/52))

### Changed

- Tighten workspace panel layout. ([#53](https://github.com/penberg/swarm/pull/53))

### Fixed

- Route terminal text input through GTK IM context so dead keys, IME composition, and modifier-consuming keyboard layouts commit the correct characters. ([#51](https://github.com/penberg/swarm/pull/51))

## [0.1.2] - 2026-04-06

### Fixed

- Fix branch monitor file descriptor leak.
- Cache terminal widgets to fix garbled output on tab switch. ([#47](https://github.com/penberg/swarm/pull/47))
- Preserve terminal focus across periodic PR status refreshes. ([#48](https://github.com/penberg/swarm/pull/48))
- Suffix cloned workspace names from the source workspace so cloning no longer fails when a previously removed workspace's directory still exists on disk.

## [0.1.1] - 2026-04-05

### Added

- Support explicit repository remote URLs. ([#45](https://github.com/penberg/swarm/pull/45))
- Persist repository form state in sidebar across navigation. ([#46](https://github.com/penberg/swarm/pull/46))

### Fixed

- Refresh selected workspace PR link when switching workspaces. ([#44](https://github.com/penberg/swarm/pull/44))
- Move session tab refresh off the UI thread to eliminate periodic freezes. ([#43](https://github.com/penberg/swarm/pull/43))

