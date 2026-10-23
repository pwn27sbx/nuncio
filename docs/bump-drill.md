# The whatsmeow bump drill (spike S7)

**The recorded number: a cold `go build ./cmd/nunciod` takes 5.1 seconds.**

This file records one measured run of `contrib/bump-drill.sh`. It exists because
`go.mau.fi/whatsmeow` is nuncio's heaviest dependency and it releases often: the
question "how expensive is a bump?" deserves a real number, not an estimate.

## The pin

| | |
|---|---|
| module | `go.mau.fi/whatsmeow` |
| version | `v0.0.0-20261017175727-b2b1e71d45e2` |
| commit | `b2b1e71d45e2ff171153b025b5a42f475bb30ef8` |
| commit date | 2026-10-17T17:57:27Z |
| upstream | `https://github.com/tulir/whatsmeow.git` |

The short commit `b2b1e71d45e2` is the trailing hash segment of the
pseudo-version, which is how `contrib/bump-drill.sh` recovers it without a
network call. The full SHA comes from
`https://proxy.golang.org/go.mau.fi/whatsmeow/@v/v0.0.0-20261017175727-b2b1e71d45e2.info`
(`.Origin.Hash`).

## The recorded run (2026-10-18)

Machine: 16 cores, `linux/amd64`, `CGO_ENABLED=1` (unused: the build needs no
cgo, `CGO_ENABLED=0 go build ./...` also passes).

Exact commands:

```sh
cd ~/Projects/nuncio
./contrib/bump-drill.sh          # the run below
go version                       # go1.27.2-X:nodwarf5
```

Observed output:

```
# nuncio whatsmeow bump drill (spike S7)

go:               go1.27.2-X:nodwarf5
module:           github.com/pwn27sbx/nuncio
whatsmeow before: v0.0.0-20261017175727-b2b1e71d45e2

== pin go.mau.fi/whatsmeow@v0.0.0-20261017175727-b2b1e71d45e2 ==
whatsmeow after:  v0.0.0-20261017175727-b2b1e71d45e2 (b2b1e71d45e2)

== warm the module cache: downloads are not part of the timing ==
packages in the build: 102

== throwaway build cache at /tmp/nuncio-bump-drill-cache-yh64Ju (equivalent to go clean -cache, scoped) ==

== result ==
whatsmeow:  v0.0.0-20261017175727-b2b1e71d45e2 (b2b1e71d45e2)
go:         go1.27.2-X:nodwarf5
packages:   102
cold build: 5.1s   (empty build cache: the standard library and every dependency)
warm build: 0.1s   (same cache, nothing changed)
```

| measurement | seconds |
|---|---|
| packages in the build graph | 102 |
| **cold build (empty build cache)** | **5.1** |
| warm build (same cache, nothing changed) | 0.1 |

## The M1 run (2026-10-19)

The drill re-run on the M1 code (the daemon now owns the session, stores messages and media
references, streams events and serves send, focus, wipe and download_media), as the M1
acceptance asks: "the S7 bump drill still passes on the current code".

Machine: Apple M4, 10 cores, `darwin/arm64`, go1.27.1. Command: `contrib/bump-drill.sh`
(default reference, throwaway `GOCACHE`, no `--global-clean`).

| | |
|---|---|
| whatsmeow | `v0.0.0-20261017175727-b2b1e71d45e2` (`b2b1e71d45e2`), unchanged |
| packages in the build | 438 |
| cold build | **8.4 s** |
| warm build | 0.2 s |
| `go.mod` / `go.sum` | unchanged (same SHA-1 before and after) |

The drill passes. The cold number is not comparable with the M0 run above: it is a different
machine and OS, so it says nothing about whether M1 made the build slower.

## What the two numbers mean

- **Cold (5.1 s)** is the drill's headline: an empty build cache, so the standard
  library and all 102 packages are compiled. This is exactly what
  `go clean -cache` followed by a build costs, which is what the spike asked
  for. It is an upper bound for a bump, not a typical one.
- **Warm (0.1 s)** is the floor: the same command with the cache intact.
- A real bump (change the pin, keep the cache) sits in between: whatsmeow and
  everything that depends on it recompile, the standard library does not.
- **Downloads are excluded on purpose**: `go mod download` runs before the
  timing, because the module cache (`GOMODCACHE`) is a different cache from the
  build cache and the spike is about compile cost.

## How to repeat it

```sh
./contrib/bump-drill.sh                                    # the default pin
./contrib/bump-drill.sh v0.0.0-20261017175727-b2b1e71d45e2 # an explicit reference
./contrib/bump-drill.sh --global-clean                     # destructive variant, see below
```

No `sudo`, no installs. The pin stays in `go.mod` and `go.sum` for review
afterwards (`git diff go.mod go.sum`).

## A real bump: review the sealed store

`internal/store/sealedstore.go` wraps whatsmeow's `*sqlstore.SQLStore` and seals every private
column (ADR 0003). A method it does not override falls through to `SQLStore`, which writes in
plain. So a bump that changes `SQLStore` is a reviewed change, not only a rebuild:

```sh
TMPDIR=/tmp go test ./internal/store/ -run TestSQLStoreMethodSetIsPinned
```

When it fails, it lists every method added, removed or changed. For each one, decide whether it
reads or writes a sealed column (ADR 0003, point 7: sessions, pre-keys, sender keys, app-state key
data, message secrets, privacy tokens, the NCT salt, the event and retry buffers, the device keys).
If it does, override it in `sealedstore.go` with a test in `sealedstore_test.go`; then update
`pinnedSQLStoreMethods` in the same commit. Also compare `sqlstore/upgrades/00-latest-schema.sql`
before and after: a new table or column holding key material needs the same treatment.

`*sqlstore.Container` methods reach the sealed store too, through `SQLStore`: the device ones
(`PutDevice`, `GetDevice` and the rest) are shadowed to fail, and only `Close` and `Upgrade` stay
promoted. `TestSealedStoreRefusesTheContainersDeviceMethods` calls each one, so a new `Container`
method fails it until it is shadowed in `sealedstore.go` or added to `keptContainerMethods`.

Both guards compare method names and signatures only, never the SQL inside them. So every bump
also reviews the diff of `sqlstore/store.go` and `sqlstore/container.go`, even when the tests pass:
a method that keeps its signature but changes its query (another column, a copy in SQL) can still
move key bytes the sealed store does not see.

## Limits worth knowing

- A default run pins the reference already in `go.mod`, so `go.mod` and
  `go.sum` come out unchanged. Pass a different reference to drill a real bump.
- `--global-clean` runs `go clean -cache`, which empties the build cache of
  **every** Go project on the machine, not only nuncio's. The default uses a
  throwaway `GOCACHE` in a `mktemp -d` directory and removes it on exit: the same
  cold measurement, without making unrelated projects rebuild.
- The number is machine-specific. It is a recorded measurement of one run on one
  host, not a portable benchmark.
