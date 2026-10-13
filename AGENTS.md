# AGENTS

## Coding Style

- Group imports so there is exactly one `use` per crate.
- For `std`, prefer a single grouped import such as `use std::{...};` instead of multiple `use std::...;` lines.
- Keep imports explicit and stable. Do not use wildcard imports.
- Follow the existing command layout:
  - top-level CLI parsing in `opts.rs`
  - command handlers in `cmd/`
  - storage and domain logic in top-level modules such as `repos.rs`

## Commits

- Do not update `CHANGELOG.md` in commits. It is updated at release time.
