#!/bin/sh
# Usage: check-tag.sh <tag> [Cargo.toml]
# Prints the workspace version when <tag> is exactly v<version>; fails otherwise.
set -eu

tag=${1:?usage: check-tag.sh <tag> [Cargo.toml]}
cargo_toml=${2:-$(cd "$(dirname "$0")/.." && pwd)/Cargo.toml}

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$cargo_toml" | head -n 1)
if [ -z "$version" ]; then
    echo "no version found in $cargo_toml" >&2
    exit 1
fi
if [ "$tag" != "v$version" ]; then
    echo "tag $tag does not match Cargo.toml version $version" >&2
    exit 1
fi
echo "$version"
