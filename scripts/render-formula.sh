#!/bin/sh
# Usage: render-formula.sh <tag> <sha256> [template]
# Prints the Homebrew formula for <tag>, built from GitHub's source tarball.
set -eu

tag=${1:?usage: render-formula.sh <tag> <sha256> [template]}
sha=${2-}
template=${3:-$(cd "$(dirname "$0")/.." && pwd)/packaging/homebrew/attic.rb.in}

case $tag in
    v*) ;;
    *) echo "tag $tag does not start with v" >&2; exit 1 ;;
esac
if ! printf '%s' "$sha" | grep -Eqx '[0-9a-f]{64}'; then
    echo "sha256 '$sha' is not 64 lowercase hex characters" >&2
    exit 1
fi

url="https://github.com/sergiommarcial/attic/archive/refs/tags/$tag.tar.gz"
sed -e "s|@URL@|$url|" -e "s|@SHA256@|$sha|" "$template"
