# Installing nuncio

nuncio is a WhatsApp client for **Omarchy** (or Arch Linux) on x86_64. This page is the whole
story for the app: install, link your phone, update, uninstall, and what to do when something goes
wrong. The terminal commands are in [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md).

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

**That is the last terminal step.** The package installs four programs (`nunciod`, `nuncio`,
`nuncio-tui`, `nuncio-desktop`), two launchers, the daemon's `systemd --user` unit and its D-Bus
activations, and an autostart entry for the desktop app. Everything after this line happens in the
app's window.

### On Arch without Omarchy

The package needs GTK 4, libadwaita and glibc, and a **Secret Service** to hold the device keys:

```sh
sudo pacman -S gnome-keyring
```

Omarchy ships one already. Without it, pressing **Link** fails with the keyring sentence below.

## Link this device

1. Open **nuncio** from your launcher. With no device linked, its first screen says **Link this
   device to WhatsApp**, says what linking gives this computer access to, and offers one button,
   **Link**. Nothing is asked of your keyring until you press it.
2. Press **Link**. A QR code appears, and a new one replaces it while your phone has not answered.
3. On your phone: **WhatsApp → Settings → Linked devices → Link a device**, and scan the code.

The chat list appears the moment your phone accepts the code. Nothing to restart, nothing else to
do. You link once per computer.

- **The code links a device to your account** while it is on screen: do not let anyone else scan it.
- **Cancel** (or `Esc`, or closing the window) stops waiting and leaves nothing behind: no device,
  no data, no keyring entry. The screen stays, and **Link** asks for a new code.
- **The code expired or WhatsApp refused it**: the screen says so and offers **New code**.

After that, both apps open on your chats:

- **nuncio** (the GTK app) from your launcher;
- **nuncio (terminal)** from your launcher, or **`nuncio-tui`** in a terminal.

The desktop app adds the daemon to your login items, so it comes back after a logout without you
typing anything. No `systemctl`, no terminal.

## Update

Run the install line again. The URL always points at the latest release.

The CDN behind `/releases/latest/` can serve the previous file for a few minutes after a release;
if `pacman -Qi nuncio` shows the version you just installed over, wait and run the line again.

## Uninstall

**Unlink first**, from the app: **Settings** (Ctrl+,) → **This device** → **Unlink this device**,
then type `unlink`. That tells WhatsApp to unlink this computer, and deletes nuncio's data on it:
the database, the media it downloaded and the keys in your keyring. If the device is already gone
from your phone's **Linked devices**, `nuncio purge --confirm` deletes the same local data without
contacting WhatsApp.

Then remove the package:

```sh
sudo pacman -R nuncio
```

That removes the programs, the launchers, the units and the autostart entry. It does not touch what
your account holds; three paths are worth naming:

- **`~/.local/share/nuncio`** — the folder the daemon kept your data in. The unlink empties it; the
  folder itself stays, because the daemon's sandbox cannot remove it. Delete it if it is empty. If
  you removed the package without unlinking, your database and media are still in it.
- **the keyring item `nuncio` / `store.data_key`** — the key that opens the database. The unlink
  deletes it; skipped that step, it is still there, and so is your data above.
- **`~/.config/systemd/user/default.target.wants/nunciod.service`** — the link the app wrote when it
  added the daemon to your login items. It now points at a unit that is gone; delete it.

The device stays linked on your phone until you unlink it (from the app, or from the phone's
**Linked devices** list). Unlinking deletes your data: to keep a copy, do it before you unlink —
the folder alone is useless without the keyring item.

## If something goes wrong

- **`nuncio could not reach the keyring: unlock it, or on plain Arch install gnome-keyring (Omarchy
  ships a Secret Service).`** The link screen says this after you press **Link**. Unlock your login
  keyring (it normally unlocks with your session), then press **Try again**.
- **`WhatsApp stopped the linking: ask for a new QR`**: WhatsApp ended the pairing — usually an
  expired code, sometimes the account was logged out or banned. Press **New code**. If it keeps
  happening, stop: do not retry with the number that carries your two-factor authentication.
- **The app opens on the link screen although this computer was linked**: the daemon has no device
  any more (the data was removed, or `nuncio purge` ran). Link again.
- **The app says `offline` or `reconnecting…`**: it cannot reach the daemon. Open **nuncio** again:
  it asks the session bus for the daemon, and the bus starts it. If it stays offline, log out of
  your desktop session and back in; the login item starts it. Your messages are not lost.
- **A full log**: it needs a terminal. [docs/COMMAND-LINE.md](docs/COMMAND-LINE.md#when-something-goes-wrong)
  has the command, and every message the CLI prints. Logs never contain message text, full phone
  numbers or keys.
- **Your data**: `~/.local/share/nuncio` (directory `0700`, files `0600`). The database is sealed
  under the key in your keyring, and the sealed database is useless without it.

## Building from source instead

See [packaging/README.md](packaging/README.md): `base-devel`, `go` 1.26+, `rust` 1.92+,
`pkgconf`, then `packaging/package-source.sh` and `makepkg`. This is for contributors; the
install line above needs no toolchain.
