#!/bin/bash

set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <version>"
    exit 1
fi

VERSION="$1"
TAG="v$VERSION"
ROOT="$(git rev-parse --show-toplevel)"

cd "$ROOT"

if ! git diff --quiet --exit-code || ! git diff --cached --quiet --exit-code; then
    echo "refusing to tag with tracked changes in the worktree"
    exit 1
fi

CURRENT_VERSION="$(
    perl -ne '
        if (/^\[package\]$/) {
            $in_package = 1;
            next;
        }
        if ($in_package && /^\[/) {
            $in_package = 0;
        }
        if ($in_package && /^version = "([^"]+)"$/) {
            print "$1\n";
            exit;
        }
    ' Cargo.toml
)"

if [ -z "$CURRENT_VERSION" ]; then
    echo "failed to read current version from Cargo.toml"
    exit 1
fi

if [ "$CURRENT_VERSION" = "$VERSION" ]; then
    echo "Cargo.toml is already at version $VERSION"
    exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "tag $TAG already exists"
    exit 1
fi

VERSION="$VERSION" perl -0pi -e 's/(\[package\]\n(?:[^\[]*\n)*?version = ")[^"]+(")/$1.$ENV{VERSION}.$2/se' Cargo.toml

if git diff --quiet --exit-code -- Cargo.toml; then
    echo "failed to update Cargo.toml"
    exit 1
fi

cargo check
cargo dist plan --allow-dirty

git add Cargo.toml Cargo.lock
git commit -m "Splay $VERSION"
git tag -a "$TAG" -m "Splay $VERSION"
