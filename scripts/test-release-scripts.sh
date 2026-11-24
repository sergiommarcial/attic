#!/bin/sh
# Tests check-tag.sh and render-formula.sh. Run: sh scripts/test-release-scripts.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fixture="$tmp/Cargo.toml"
cat > "$fixture" <<'EOF'
[workspace]
members = ["crates/a"]

[workspace.package]
version = "0.1.0"
edition = "2024"
EOF

passed=0
failed=0

pass() { passed=$((passed + 1)); }
fail() { failed=$((failed + 1)); echo "FAIL: $*"; }

expect_ok() {
    if "$@" > "$tmp/out" 2> "$tmp/err"; then pass; else fail "expected success: $*"; cat "$tmp/err"; fi
}

expect_fail() {
    if "$@" > "$tmp/out" 2> "$tmp/err"; then fail "expected failure: $*"; else pass; fi
}

repeat() { printf "%$2s" '' | tr ' ' "$1"; }

check="$here/check-tag.sh"
render="$here/render-formula.sh"

expect_ok sh "$check" v0.1.0 "$fixture"
if [ "$(cat "$tmp/out")" = "0.1.0" ]; then pass; else fail "check-tag prints the version"; fi
expect_fail sh "$check" 0.1.0 "$fixture"
expect_fail sh "$check" v0.1 "$fixture"
expect_fail sh "$check" v0.1.0-rc1 "$fixture"
expect_fail sh "$check" v0.2.0 "$fixture"
if grep -q 'v0.2.0' "$tmp/err" && grep -q '0.1.0' "$tmp/err"; then pass; else fail "mismatch names both versions"; fi

sha=$(repeat a 64)
expect_ok sh "$render" v0.1.0 "$sha"
out=$(cat "$tmp/out")
if printf '%s\n' "$out" | grep -qx '  url "https://github.com/sergiommarcial/attic/archive/refs/tags/v0.1.0.tar.gz"'; then pass; else fail "formula url"; fi
if printf '%s\n' "$out" | grep -qx "  sha256 \"$sha\""; then pass; else fail "formula sha256"; fi
if printf '%s\n' "$out" | grep -q '@'; then fail "placeholder left in formula"; else pass; fi
if [ "$out" = "$(sh "$render" v0.1.0 "$sha")" ]; then pass; else fail "same inputs, same output"; fi
expect_fail sh "$render" v0.1.0 ""
expect_fail sh "$render" v0.1.0 "$(repeat A 64)"
expect_fail sh "$render" v0.1.0 "$(repeat a 63)"
expect_fail sh "$render" 0.1.0 "$sha"

echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
