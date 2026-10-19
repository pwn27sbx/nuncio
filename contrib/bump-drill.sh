#!/usr/bin/env bash
# nuncio spike S7 — the whatsmeow bump drill.
#
# Answers one question with a real number: after pinning go.mau.fi/whatsmeow to
# a specific commit, how long does `go build ./cmd/nunciod` take with a cold Go
# build cache?
#
# Usage: contrib/bump-drill.sh [--global-clean] [ref]
#
#   ref            module reference to pin (default: the commit below).
#   --global-clean run `go clean -cache` first, which empties the build cache of
#                  *every* Go project on this machine. The default uses a
#                  throwaway GOCACHE instead: the same cold measurement, without
#                  making unrelated projects rebuild.
#
# No sudo, no installs. The pin stays in go.mod and go.sum for review.
set -euo pipefail

# The commit this drill was recorded against and what go.mod should hold.
default_ref="v0.0.0-20261017175727-b2b1e71d45e2"

global_clean=0
ref=""
for arg in "$@"; do
	case "$arg" in
	--global-clean) global_clean=1 ;;
	-h | --help)
		sed -n '2,17p' "$0"
		exit 0
		;;
	-*)
		echo "unknown option: $arg" >&2
		exit 2
		;;
	*) ref="$arg" ;;
	esac
done
ref="${ref:-$default_ref}"

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

now() { date +%s.%N; }
since() { awk -v s="$1" -v e="$2" 'BEGIN { printf "%.1f", e - s }'; }

echo "# nuncio whatsmeow bump drill (spike S7)"
echo
echo "go:               $(go version | awk '{print $3}')"
echo "module:           $(go list -m)"
echo "whatsmeow before: $(go list -m -f '{{.Version}}' go.mau.fi/whatsmeow 2>/dev/null || echo none)"

echo
echo "== pin go.mau.fi/whatsmeow@$ref =="
go get "go.mau.fi/whatsmeow@$ref"
whatsmeow=$(go list -m -f '{{.Version}}' go.mau.fi/whatsmeow)
# The commit is the trailing hash segment of the pseudo-version; no network call
# is needed to recover it.
whatsmeow_hash=$(printf '%s' "$whatsmeow" | sed -n 's/.*-\([0-9a-f]\{12\}\)$/\1/p')
echo "whatsmeow after:  $whatsmeow ${whatsmeow_hash:+($whatsmeow_hash)}"

echo
echo "== warm the module cache: downloads are not part of the timing =="
go mod download

# Measured before the build cache is touched, so it cannot warm it.
deps=$(go list -deps ./cmd/nunciod | wc -l)
echo "packages in the build: $deps"

if [ "$global_clean" = 1 ]; then
	echo
	echo "== go clean -cache (GLOBAL: other Go projects on this machine rebuild too) =="
	go clean -cache
	cache=$(go env GOCACHE)
else
	cache=$(mktemp -d "${TMPDIR:-/tmp}/nuncio-bump-drill-cache-XXXXXX")
	trap 'rm -rf "$cache"' EXIT
	echo
	echo "== throwaway build cache at $cache (equivalent to go clean -cache, scoped) =="
fi

binary=$(mktemp -d "${TMPDIR:-/tmp}/nuncio-bump-drill-bin-XXXXXX")/nunciod
trap 'rm -rf "${cache:-}" "$(dirname "$binary")"' EXIT

start=$(now)
GOCACHE="$cache" go build -o "$binary" ./cmd/nunciod
end=$(now)
cold=$(since "$start" "$end")

start=$(now)
GOCACHE="$cache" go build -o "$binary" ./cmd/nunciod
end=$(now)
warm=$(since "$start" "$end")

echo
echo "== result =="
echo "whatsmeow:  $whatsmeow ${whatsmeow_hash:+($whatsmeow_hash)}"
echo "go:         $(go version | awk '{print $3}')"
echo "packages:   $deps"
echo "cold build: ${cold}s   (empty build cache: the standard library and every dependency)"
echo "warm build: ${warm}s   (same cache, nothing changed)"
echo
echo "go.mod and go.sum now pin the reference above; review with 'git diff go.mod go.sum'."
