# Swarm Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

