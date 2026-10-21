# Packaging nuncio for Arch Linux / Omarchy

**Status: draft.** Nothing here has been built or installed yet: these files were written on a
Mac and every check in [What to test at home](#what-to-test-at-home) is still open. No release
exists and none is made by anything in this folder.

The approach is [Sealwax](https://github.com/PixDevsApps/sealwax)'s (MIT): a tarball of the
committed tree, every dependency fetched in `prepare()`, then an offline build and offline tests,
with `options=('!debug' '!lto')`. Its approach was copied, not its files.

## Files

| File | Installed as | What |
| --- | --- | --- |
| `PKGBUILD.in` | — | The recipe; `package-source.sh` fills it in |
| `package-source.sh` | — | Writes the source tarball and the filled `PKGBUILD` |
| `nunciod.service` | `/usr/lib/systemd/user/nunciod.service` | The daemon as a `systemd --user` service |
| `io.github.pwn27sbx.nuncio.service` | `/usr/share/dbus-1/services/` | D-Bus activation of the daemon (`nuncio` starts it through the bus) |
| `io.github.pwn27sbx.nuncio.Desktop.desktop` | `/usr/share/applications/` | The GTK app's launcher and `whatsapp:` link handler |
| `io.github.pwn27sbx.nuncio.Desktop.service` | `/usr/share/dbus-1/services/` | D-Bus activation of the GTK app, as a background `GApplication` service |
| `nuncio-tui.desktop` | `/usr/share/applications/` | The terminal app's launcher (`Terminal=true`) |
| `nuncio-autostart.desktop` | `/etc/xdg/autostart/io.github.pwn27sbx.nuncio.Desktop.desktop` | Starts the GTK app at login in the background, for its notifications |
| `install.sh` | — | The one-command installer (draft) |
| `MANIFEST` | — | Every path the package installs |
| `check.sh` | — | Static checks of all of the above |

## Build and install locally (on Arch)

You need `base-devel`, `git`, `go` (1.26 or later), `rust` (1.92 or later) and `pkgconf`. The
packaging files must be committed: the tarball is `git archive HEAD`.

```sh
packaging/package-source.sh            # writes .local/package/{nuncio-<version>.tar.gz,PKGBUILD}
cd .local/package
makepkg --cleanbuild --force -si       # builds, runs the tests, installs with pacman
systemctl --user enable --now nunciod.service
```

With Rust from rustup's own installer (not a pacman package), add `--nodeps` to `makepkg`, as for
Sealwax: makepkg's check of the build dependencies fails, pacman still checks the runtime ones.

What the build does: `prepare()` is the only step with network access (`go mod download` into
`$srcdir/gomodcache`, `cargo fetch --locked` for the host). `build()` and `check()` run with
`GOPROXY=off`, `GOTOOLCHAIN=local`, `CGO_ENABLED=0`, `-trimpath -mod=readonly -buildmode=pie`, and
`cargo --frozen`, so a missing dependency is an error, never a download. `check()` runs
`go test ./...` and `cargo test --workspace`; the GTK smoke test skips itself without a display.

## The one-command install (once a release exists)

```sh
curl -fLO https://github.com/pwn27sbx/nuncio/raw/v<version>/packaging/install.sh
less install.sh                         # read it first
bash install.sh --version <version>
```

It prints what it will do, asks, downloads `nuncio-<version>-1-x86_64.pkg.tar.zst` and
`SHA256SUMS` from the release, checks the SHA-256, runs `sudo pacman -U` and enables the daemon
(`--no-enable` skips that). It never pipes a download into a shell, refuses without a terminal
unless `--yes`, and does nothing outside Arch. To try it before any release, point it at a local
build: `bash install.sh --dir .local/package` (with a `SHA256SUMS` next to the package:
`sha256sum nuncio-*.pkg.tar.zst > SHA256SUMS`).

## Uninstall

```sh
bash install.sh --uninstall             # or by hand:
systemctl --user disable --now nunciod.service
sudo pacman -R nuncio
```

Disable the service **before** `pacman -R`: otherwise `~/.config/systemd/user/default.target.wants/`
keeps a dangling link. Your messages and keys are not package files: `nuncio purge --confirm`
(before uninstalling) or deleting `~/.local/share/nuncio` removes them. `MANIFEST` lists every
package file; after `pacman -R` none of them may exist.

## How a release would be made (not done; needs the owner's OK)

1. Commit, tag `v<version>` (the version in `Cargo.toml`, which `cmd/nunciod` must repeat).
2. On Arch: `packaging/package-source.sh`, then `makepkg --cleanbuild --force` in `.local/package`.
3. `sha256sum nuncio-<version>-1-x86_64.pkg.tar.zst > SHA256SUMS`.
4. A GitHub release `v<version>` with the package and `SHA256SUMS` as assets.

No CI builds it: delivery and CI are the owner's decision (Q14).

## Decisions

- **The daemon starts through its user unit.** `Type=dbus` with `BusName=io.github.pwn27sbx.nuncio`:
  nunciod listens on its socket before it requests the name, so the unit is "started" exactly when
  clients can connect. `Restart=on-failure` only (a clean exit, such as after `nuncio logout`, is
  never restarted), 30 s doubling to 10 min between restarts and at most 4 starts in 30 min: each
  restart is a new WhatsApp connection (ban-risk rule). The package does not enable it for every
  user; `install.sh` asks and enables it for you. `StartLimitBurst=4` per 30 min counts every
  start, manual (`systemctl --user start`) and D-Bus-activated ones too, not only restarts:
  during first-time setup (link, stop, start again), if it refuses to start, run
  `systemctl --user reset-failed nunciod` and start it again.
- **The unit's sandbox** keeps exactly what nunciod needs: the data directory
  `~/.local/share/nuncio` (read-write), `$XDG_RUNTIME_DIR/nuncio` (systemd creates it), the
  session bus and the keyring over D-Bus (connecting to a Unix socket works on a read-only mount),
  TCP to WhatsApp. Each line is commented in `nunciod.service`. In a user unit these settings need
  unprivileged user namespaces (systemd then turns on `PrivateUsers=` by itself); Arch's kernels
  allow them. A custom `XDG_DATA_HOME` needs a drop-in, since a unit cannot read the variable.
- **Autostart: both.** The daemon at login through `WantedBy=default.target` (feeds the bar
  widget's `UnreadCount`); the GTK app through
  `/etc/xdg/autostart/io.github.pwn27sbx.nuncio.Desktop.desktop` (the only system-wide XDG
  autostart directory; `/usr/share` is not in `$XDG_CONFIG_DIRS`), started with
  `--gapplication-service`: it runs in the background with no window, so notifications arrive
  without opening it, and the next launch shows the window. To turn the app's autostart off for
  yourself, copy the file to `~/.config/autostart/` and add `Hidden=true`. On Hyprland the entry
  runs only if the session starts XDG autostart (Omarchy's uwsm does, through
  `xdg-desktop-autostart.target`; to verify).
- **Two identities, never mixed.** The daemon is `io.github.pwn27sbx.nuncio` (its bus name, its
  D-Bus service file, the unit's `BusName=`); the GTK app is `io.github.pwn27sbx.nuncio.Desktop`
  (its `GApplication` id, `.desktop` file name, `StartupWMClass`, `DBusActivatable=true`, its own
  D-Bus service with `Exec=/usr/bin/nuncio-desktop --gapplication-service`, and the autostart
  entry). `check.sh` fails if a daemon file names the app id or an app file claims the daemon's
  name.
- **License files:** GPL-3.0-only is in Arch's `licenses` package, so its text is not copied
  (Arch guidelines); `CREDITS.md` goes to `/usr/share/licenses/nuncio/` for the third-party code
  the static binaries carry.

## The app id collision and its rename

Until M4 the GTK app's id was `io.github.pwn27sbx.nuncio`, the same string as the daemon's D-Bus
name (`internal/ipc/connect.go`). `GApplication` owns its app id on the session bus, so in any real
Linux session whichever started second lost: `nunciod` exited with "another nunciod already owns
…" while the app ran, and the app found the name owned by something that is not a `GApplication`
while the daemon ran. The Mac never showed it (no session bus, and the smoke test registers no
application). The daemon keeps `io.github.pwn27sbx.nuncio`; the app becomes
`io.github.pwn27sbx.nuncio.Desktop`, and these files already use that id.

**The code change (resolved):** `APP_ID` in `crates/nuncio-desktop/src/args.rs` is
`io.github.pwn27sbx.nuncio.Desktop`, and `nuncio-desktop` accepts `--gapplication-service` (its
argument parser used to reject every unknown flag), so the app's D-Bus service and the autostart
entry start it; `check.sh`'s last group checks that `APP_ID` matches.

**The icon:** `Icon=io.github.pwn27sbx.nuncio.Desktop` in every `.desktop` file names the icon in
`icons/hicolor/`, original artwork (a sealed envelope with an "n" on the wax seal; nuncio is not
affiliated with WhatsApp and uses none of its marks): a full-colour
`scalable/apps/io.github.pwn27sbx.nuncio.Desktop.svg` (128×128 viewBox) and a monochrome
`symbolic/apps/io.github.pwn27sbx.nuncio.Desktop-symbolic.svg` (16×16) that GTK recolours.
`package()` installs both under `/usr/share/icons/hicolor/`; pacman's hooks refresh the icon
cache. `check.sh` checks that they parse as XML, have those viewBoxes, hold no `<image`,
`<script` or remote `xlink:href`, and that every `Icon=` names an installed icon.

## What to test at home

1. `packaging/check.sh` (with `desktop-file-validate` and `systemd-analyze` installed it also runs
   them), `shellcheck packaging/*.sh`, `namcap PKGBUILD` and `namcap nuncio-*.pkg.tar.zst`.
2. `makepkg --cleanbuild --force` succeeds, including `check()`; `strings` on the binaries finds no
   `/home/` path.
3. On a clean VM: `bash install.sh --dir …` (then, after a release, `--version`) installs with one
   command; `pacman -Ql nuncio` equals `MANIFEST`.
4. `systemctl --user status nunciod` is active; `busctl --user status io.github.pwn27sbx.nuncio`
   shows it; `systemd-analyze --user security nunciod` scores it; the keyring works (link a test
   device: `systemctl --user stop nunciod`, `nuncio link`, start it again).
5. Kill it (`systemctl --user kill -s SIGKILL nunciod`): it comes back after 30 s, not sooner.
   `nuncio logout --confirm`: it stops and stays stopped.
6. With the unit stopped, `nuncio chats` starts it through D-Bus activation.
7. With both running, `busctl --user list | grep pwn27sbx` shows two names:
   `io.github.pwn27sbx.nuncio` (nunciod) and `io.github.pwn27sbx.nuncio.Desktop` (the app).
   `xdg-open 'whatsapp://send?phone=…&text=hola'` opens the app with the draft; the launcher shows
   nuncio and nuncio (terminal); `gapplication launch io.github.pwn27sbx.nuncio.Desktop` works.
8. Log out and in: the daemon and the app start, the app with no window; launching it shows it.
9. `bash install.sh --uninstall`: no path from `MANIFEST` remains, no unit is left enabled.
