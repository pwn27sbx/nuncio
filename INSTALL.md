# Installing nuncio

nuncio is a WhatsApp client for **Omarchy** (or Arch Linux) on x86_64. This page is the whole
story: install, first run, update, uninstall, and what to do when something goes wrong.

nuncio is an unofficial client and WhatsApp's Terms of Service forbid it. Accounts have been
banned for using clients like this one. **Use a spare number**, never the number that carries
your two-factor authentication.

## Install

Open a terminal (**Super + Return** on Omarchy), paste this line and press Enter:

```sh
curl -L -o /tmp/nuncio.pkg.tar.zst https://github.com/pwn27sbx/nuncio/releases/latest/download/nuncio-x86_64.pkg.tar.zst && sudo pacman -U /tmp/nuncio.pkg.tar.zst
```

`pacman` prints **"package is not signed"** and asks to continue. That is expected: nuncio is not
in an official repository, so the package is unsigned. It comes over HTTPS from this project's
release page.

The package installs four programs (`nunciod`, `nuncio`, `nuncio-tui`, `nuncio-desktop`), two
launchers, the daemon's `systemd --user` unit and its D-Bus activations, and an autostart entry
for the desktop app.

### On Arch without Omarchy

The package needs GTK 4, libadwaita and glibc, and a **Secret Service** to hold the device keys:

```sh
sudo pacman -S gnome-keyring
```

Omarchy ships one already. Without it, linking a device fails with
`the keyring (Secret Service) is locked or unavailable`.

## First run

The package does **not** start the daemon for you, and the command line reaches the daemon
through D-Bus, so link **before** anything starts it:

```sh
nuncio link --qr --consent
```

`--consent` is required: linking a device is the one action that needs your explicit yes, and
without it the command only prints the warning. A QR code appears; scan it from WhatsApp on your
phone (**Linked devices → Link a device**). Once it says the device is linked:

```sh
systemctl --user enable --now nunciod.service
```

Then open either app:

- **`nuncio-tui`** in a terminal, or **nuncio (terminal)** from your launcher;
- **nuncio** (the GTK app) from your launcher, or D-Bus starts it.

If `nuncio link` refuses because `nunciod is running`, stop it first:

```sh
systemctl --user stop nunciod
```

### The Omarchy hotkey and bar badge (optional, not packaged)

These are files in your own `~/.config`, so no package can install them:

```sh
base=https://raw.githubusercontent.com/pwn27sbx/nuncio/v0.1.0/contrib/omarchy
curl -fsSL -o ~/.config/hypr/nuncio.lua --create-dirs "$base/hypr/nuncio.lua"
echo 'require("hypr.nuncio")' >> ~/.config/hypr/bindings.lua
mkdir -p ~/.config/omarchy/plugins/pwn27sbx.nuncio-unread
curl -fsSL -o ~/.config/omarchy/plugins/pwn27sbx.nuncio-unread/manifest.json "$base/bar-widget/manifest.json"
curl -fsSL -o ~/.config/omarchy/plugins/pwn27sbx.nuncio-unread/BarWidget.qml "$base/bar-widget/BarWidget.qml"
hyprctl reload
```

`packaging/install.sh --uninstall` knows how to remove both if you installed them unchanged. The
details, and the key they bind, are in `contrib/omarchy/README.md` of the same tag.

## Update

Run the install line again. The URL always points at the latest release.

The CDN behind `/releases/latest/` can serve the previous file for a few minutes after a release;
if `pacman -Qi nuncio` shows the version you just installed over, wait and run the line again.

## Uninstall

```sh
sudo pacman -R nuncio
```

That removes the programs, the launchers, the units and the autostart entry. **Your messages stay**
in `~/.local/share/nuncio` (the database and the media), and the device stays linked on your
phone.

To unlink the device from WhatsApp and delete everything first:

```sh
nuncio logout --confirm
```

`logout` talks to WhatsApp (the phone stops showing the device) and wipes the database, the media,
the backups and the keyring entry. `nuncio purge --confirm` does the same locally, without
contacting WhatsApp, for a device you already removed on the phone. Both need `nunciod` running,
so run them **before** `pacman -R`.

## If something goes wrong

- **`the keyring (Secret Service) is locked or unavailable`**: unlock your login keyring (it
  normally unlocks with your session), or install a Secret Service (see above).
- **`nunciod is running` when linking**: `systemctl --user stop nunciod`, link, then start it.
- **The apps start but show `reconnecting…`**: `systemctl --user status nunciod` says why. A
  session WhatsApp stopped (logged out, banned, another device replaced it) is shown in the app's
  status line, in fixed text.
- **Logs**: `journalctl --user -u nunciod`. They never contain message text, full phone numbers or
  keys.
- **Your data**: `~/.local/share/nuncio` (directory `0700`, files `0600`). Back it up by copying
  that folder while `nunciod` is stopped, plus the keyring entry (the sealed database is useless
  without it).

## Building from source instead

See [packaging/README.md](../packaging/README.md): `base-devel`, `go` 1.26+, `rust` 1.92+,
`pkgconf`, then `packaging/package-source.sh` and `makepkg`. This is for contributors; the
install line above needs no toolchain.
