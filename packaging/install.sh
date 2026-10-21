#!/usr/bin/env bash
# nuncio installer for Arch Linux / Omarchy (DRAFT: no release exists yet).
#
# Usage:
#   install.sh --version VERSION [--yes] [--no-enable]   download a release and install it
#   install.sh --dir DIR [--yes] [--no-enable]           install from a local folder instead
#   install.sh --uninstall [--yes]                       stop the daemon and remove the package
#
# What it does, in order (it prints this plan and asks before doing anything):
#   1. downloads nuncio-VERSION-1-x86_64.pkg.tar.zst and SHA256SUMS from the GitHub release
#      vVERSION (or takes both from DIR, e.g. makepkg's output and a SHA256SUMS you wrote);
#   2. checks the package's SHA-256 against its line in SHA256SUMS and stops on a mismatch;
#   3. runs `sudo pacman -U` on that file (pacman asks again and shows the dependencies);
#   4. unless --no-enable: `systemctl --user enable --now nunciod.service` for you.
# --uninstall runs `systemctl --user disable --now nunciod.service`, then `sudo pacman -R
# nuncio`, and lists the data it leaves (your messages, under ~/.local/share/nuncio): delete it
# with `nuncio purge --confirm` before uninstalling, or by hand.
#
# Without a terminal to confirm on it refuses, unless --yes. It never runs a downloaded script:
# the only thing it downloads is the package and its checksum list, which pacman installs. The
# checksum list comes from the same release as the package, so it proves the download is intact,
# not who made it.
#
# Run it as yourself, not as root: it calls sudo for pacman only, and the daemon is your own
# systemd --user service.
set -euo pipefail

repo=pwn27sbx/nuncio
pkgname=nuncio
arch=x86_64

say() { printf '%s\n' "$*"; }
die() {
	printf 'install.sh: %s\n' "$*" >&2
	exit 1
}

usage() {
	sed -n '2,25p' "$0"
	exit "${1:-0}"
}

version="" dir="" yes=0 enable=1 uninstall=0
while (($#)); do
	case $1 in
	--version)
		(($# >= 2)) || die "--version needs a value"
		version=$2
		shift
		;;
	--dir)
		(($# >= 2)) || die "--dir needs a value"
		dir=$2
		shift
		;;
	--yes) yes=1 ;;
	--no-enable) enable=0 ;;
	--uninstall) uninstall=1 ;;
	-h | --help) usage 0 ;;
	*) die "unknown argument: $1 (see --help)" ;;
	esac
	shift
done

# Guard: only on Arch (or Omarchy, which is Arch) with pacman. Anywhere else, nothing runs.
if [[ $(uname -s) != Linux ]] || [[ ! -f /etc/arch-release ]] || ! command -v pacman >/dev/null; then
	die "this installer is for Arch Linux / Omarchy (pacman) only; nothing was done"
fi
if ((EUID == 0)); then
	die "run it as your own user, not root (it uses sudo for pacman only); nothing was done"
fi
[[ $(uname -m) == "$arch" ]] || die "the package is built for $arch, this machine is $(uname -m); nothing was done"

confirm() {
	if ((yes)); then
		return 0
	fi
	if [[ ! -r /dev/tty ]] || ! { : </dev/tty; } 2>/dev/null; then
		die "no terminal to confirm on; run it again with --yes to proceed without asking. Nothing was done."
	fi
	local answer
	printf '%s [y/N] ' "$1" >/dev/tty
	read -r answer </dev/tty || answer=""
	[[ $answer == y || $answer == Y || $answer == yes ]] || die "cancelled; nothing was done"
}

if ((uninstall)); then
	[[ -z $version && -z $dir ]] || die "--uninstall takes no --version or --dir"
	say "This will:"
	say "  1. systemctl --user disable --now nunciod.service"
	say "  2. sudo pacman -R $pkgname"
	say "Your data stays in ~/.local/share/nuncio (delete it with 'nuncio purge --confirm' first, or by hand)."
	confirm "Uninstall nuncio?"
	systemctl --user disable --now nunciod.service || say "(the service was not enabled or not running)"
	sudo pacman -R "$pkgname"
	say "nuncio is uninstalled."
	if [[ -e ${XDG_DATA_HOME:-$HOME/.local/share}/nuncio ]]; then
		say "Left behind: ${XDG_DATA_HOME:-$HOME/.local/share}/nuncio"
	fi
	exit 0
fi

if [[ -n $version && -n $dir ]] || [[ -z $version && -z $dir ]]; then
	die "give exactly one of --version VERSION or --dir DIR (see --help)"
fi
if [[ -n $version ]]; then
	[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "VERSION looks like 0.1.0, not '$version'"
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/nuncio-install.XXXXXX")
trap 'rm -rf "$work"' EXIT

if [[ -n $version ]]; then
	file=$pkgname-$version-1-$arch.pkg.tar.zst
	base=https://github.com/$repo/releases/download/v$version
	source_desc="the GitHub release v$version ($base)"
else
	[[ -d $dir ]] || die "no such directory: $dir"
	shopt -s nullglob
	candidates=("$dir"/"$pkgname"-[0-9]*-"$arch".pkg.tar.zst)
	shopt -u nullglob
	((${#candidates[@]} == 1)) || die "$dir must hold exactly one $pkgname-*-$arch.pkg.tar.zst (found ${#candidates[@]})"
	file=$(basename "${candidates[0]}")
	[[ -f $dir/SHA256SUMS ]] || die "$dir has no SHA256SUMS (make it with: cd $dir && sha256sum $file > SHA256SUMS)"
	source_desc="the local folder $dir"
fi

say "This will:"
say "  1. take $file and SHA256SUMS from $source_desc"
say "  2. check the package's SHA-256 against SHA256SUMS"
say "  3. sudo pacman -U $file"
if ((enable)); then
	say "  4. systemctl --user enable --now nunciod.service (the daemon starts now and at every login)"
fi
confirm "Install nuncio?"

if [[ -n $version ]]; then
	for name in "$file" SHA256SUMS; do
		curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
			--output "$work/$name" "$base/$name" || die "could not download $base/$name"
	done
else
	cp -- "$dir/$file" "$dir/SHA256SUMS" "$work/"
fi

# Exactly the line for this file, so a SHA256SUMS listing several files still checks this one.
line=$(awk -v f="$file" '$2 == f || $2 == "*" f' "$work/SHA256SUMS")
[[ -n $line ]] || die "SHA256SUMS has no line for $file; nothing was installed"
(cd "$work" && sha256sum --check --strict --status <<<"$line") ||
	die "SHA-256 mismatch for $file; nothing was installed"
say "SHA-256 matches."

sudo pacman -U "$work/$file"

if ((enable)); then
	systemctl --user daemon-reload
	systemctl --user enable --now nunciod.service
	say "nunciod is running: systemctl --user status nunciod.service"
fi
say "Done. Link this device with 'nuncio link' (stop the daemon first: systemctl --user stop nunciod)."
