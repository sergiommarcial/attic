#!/bin/sh
# Renders the formula with a placeholder hash into a throwaway tap and runs
# brew style and brew audit on it, so a broken formula fails before release.
# Usage: check-formula.sh [template]
set -eu

here=$(cd "$(dirname "$0")" && pwd)
tap=attic-ci/check
export HOMEBREW_NO_AUTO_UPDATE=1

brew untap "$tap" > /dev/null 2>&1 || true
brew tap-new --no-git "$tap" > /dev/null
trap 'brew untap "$tap" > /dev/null 2>&1 || true' EXIT
brew trust "$tap" > /dev/null
dir="$(brew --repository)/Library/Taps/attic-ci/homebrew-check"
sh "$here/render-formula.sh" v0.0.0 "$(printf '%64s' '' | tr ' ' 0)" ${1:+"$1"} > "$dir/Formula/attic.rb"
brew style "$tap/attic"
brew audit --strict "$tap/attic"
