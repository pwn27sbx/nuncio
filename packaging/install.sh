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
# nuncio`, then removes the Omarchy bar widget and hotkey that contrib/omarchy/README.md copies
# by hand, only where they are still what nuncio ships, and lists the data it leaves (your
# messages, under ~/.local/share/nuncio): delete it with `nuncio purge --confirm` before
# uninstalling, or by hand.
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
	sed -n '2,27p' "$0"
	exit "${1:-0}"
}


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

# --- the Omarchy glue (contrib/omarchy/README.md "Install" copies it by hand) ---
# Removed only at the paths that README names, only while each file is still the one nuncio
# ships (by SHA-256; packaging/tests/uninstall.sh checks these against contrib/omarchy), and
# never through a symlink. Anything else is left in place with one line saying so.
glue_line='require("hypr.nuncio")'

shipped_sha256() { # shipped_sha256 NAME: the SHA-256 of contrib/omarchy's file NAME
	case $1 in
	manifest.json) printf '%s' 53211026bb57e51ea3387e1df09863546779881b0396493b25ddf38417d25c2f ;;
	BarWidget.qml) printf '%s' f6f07ce83b38c0e1b3d992fe0370b444bd7bd16a5c4c739e0a55e63227b93c3a ;;
	nuncio.lua) printf '%s' 32561451d038b8169e098d34c83613cc45f600eb08845de6fbbb53f324ba9b32 ;;
	*) return 1 ;;
	esac
}

is_shipped() { # is_shipped FILE NAME: FILE is a plain file (no symlink) with the bytes of NAME
	local want got
	[[ -f $1 && ! -L $1 ]] || return 1
	want=$(shipped_sha256 "$2") || return 1
	got=$(sha256sum <"$1") || return 1
	[[ ${got%% *} == "$want" ]]
}

symlink_in() { # symlink_in REL...: prints the first $HOME/REL that is a symlink
	local rel
	for rel; do
		if [[ -L $HOME/$rel ]]; then
			printf '%s' "$HOME/$rel"
			return 0
		fi
	done
	return 1
}

remove_widget() {
	local rel=.config/omarchy/plugins/pwn27sbx.nuncio-unread link entry ok=1
	local dir=$HOME/$rel
	if link=$(symlink_in .config .config/omarchy .config/omarchy/plugins "$rel"); then
		say "Left in place: $link is a symlink; remove the bar widget by hand (contrib/omarchy/README.md)"
		return 0
	fi
	[[ -d $dir ]] || return 0
	local entries=()
	shopt -s nullglob dotglob
	entries=("$dir"/*)
	shopt -u nullglob dotglob
	for entry in ${entries[@]+"${entries[@]}"}; do
		case ${entry##*/} in
		manifest.json | BarWidget.qml)
			if ! is_shipped "$entry" "${entry##*/}"; then
				say "Left in place: $entry (changed since nuncio shipped it, or not a plain file)"
				ok=0
			fi
			;;
		*)
			say "Left in place: $entry (not a file nuncio ships)"
			ok=0
			;;
		esac
	done
	if ((!ok)); then
		say "Left in place: $dir (the bar widget stays whole; remove it by hand)"
		return 0
	fi
	for entry in ${entries[@]+"${entries[@]}"}; do
		if ! rm -f -- "$entry"; then
			say "Left in place: $dir (could not remove $entry)"
			return 0
		fi
		say "Removed: $entry"
	done
	if rmdir -- "$dir"; then
		say "Removed: $dir"
	else
		say "Left in place: $dir (could not remove the folder)"
	fi
}

without_line() { # without_line FILE LINE: FILE's bytes minus every line that is exactly LINE
	# Fails when any write fails, so a caller never takes a short file for the whole one.
	local l rc=0
	while IFS= read -r l; do
		[[ $l == "$2" ]] || printf '%s\n' "$l" || rc=1
	done <"$1"
	# A last line without a newline: read fails but has filled l.
	[[ -z $l || $l == "$2" ]] || printf '%s' "$l" || rc=1
	return "$rc"
}

remove_hotkey() {
	local hypr=$HOME/.config/hypr link lua bindings backup tmp i ours=0 has_line
	lua=$hypr/nuncio.lua bindings=$hypr/bindings.lua
	if link=$(symlink_in .config .config/hypr); then
		say "Left in place: $link is a symlink; remove the hotkey by hand (contrib/omarchy/README.md)"
		return 0
	fi
	[[ -d $hypr ]] || return 0
	if [[ -e $lua || -L $lua ]]; then
		if ! is_shipped "$lua" nuncio.lua; then
			say "Left in place: $lua (changed since nuncio shipped it, or not a plain file), and the $glue_line line that loads it"
			return 0
		fi
		ours=1
	fi
	if [[ -L $bindings ]]; then
		say "Left in place: $bindings is a symlink; delete the $glue_line line by hand"
		((ours)) && say "Left in place: $lua (bindings.lua may still load it)"
		return 0
	fi
	has_line=0
	if [[ -f $bindings ]]; then
		# Bytes, not characters: in a UTF-8 locale BSD grep and tr choke on bytes that are not UTF-8.
		if LC_ALL=C grep -qxF -- "$glue_line" "$bindings"; then
			has_line=1
		elif (($? > 1)); then
			say "Left in place: $bindings (could not read it); delete the $glue_line line by hand if it is there"
			((ours)) && say "Left in place: $lua (bindings.lua may still load it)"
			return 0
		fi
	fi
	if ((has_line)); then
		# bash cannot hold a NUL byte, so a file with one would not come back byte for byte.
		if (($(LC_ALL=C tr -d '\000' <"$bindings" | LC_ALL=C wc -c) != $(LC_ALL=C wc -c <"$bindings"))); then
			say "Left in place: $bindings (it holds a NUL byte); delete the $glue_line line by hand"
			((ours)) && say "Left in place: $lua (bindings.lua still loads it)"
			return 0
		fi
		backup=$bindings.nuncio-backup
		for ((i = 1; i < 100; i++)); do
			[[ -e $backup || -L $backup ]] || break
			backup=$bindings.nuncio-backup.$i
		done
		# Written next to it and renamed over it: bindings.lua is either the old file or the new one.
		if [[ -e $backup || -L $backup ]] || ! cp -p -- "$bindings" "$backup" ||
			! tmp=$(mktemp "$hypr/.bindings.lua.nuncio.XXXXXX"); then
			say "Left in place: $bindings (could not make a backup); delete the $glue_line line by hand"
			((ours)) && say "Left in place: $lua (bindings.lua still loads it)"
			return 0
		fi
		if ! cp -p -- "$bindings" "$tmp" || ! without_line "$bindings" "$glue_line" >"$tmp" ||
			! mv -f -- "$tmp" "$bindings"; then
			rm -f -- "$tmp"
			say "Left in place: $bindings (could not rewrite it; backup: $backup); delete the $glue_line line by hand"
			((ours)) && say "Left in place: $lua (bindings.lua still loads it)"
			return 0
		fi
		say "Removed: the line $glue_line from $bindings (backup: $backup)"
	fi
	if ((ours)); then
		if rm -f -- "$lua"; then
			say "Removed: $lua"
		else
			say "Left in place: $lua (could not remove it)"
		fi
	fi
	return 0
}

remove_omarchy_glue() {
	remove_widget
	remove_hotkey
}

uninstall_nuncio() {
	say "This will:"
	say "  1. systemctl --user disable --now nunciod.service"
	say "  2. sudo pacman -R $pkgname"
	say "  3. remove the Omarchy bar widget and hotkey that contrib/omarchy/README.md installs, where"
	say "     they are unchanged (disable the widget first: omarchy plugin disable pwn27sbx.nuncio-unread)"
	say "Your data stays in ~/.local/share/nuncio (delete it with 'nuncio purge --confirm' first, or by hand)."
	confirm "Uninstall nuncio?"
	systemctl --user disable --now nunciod.service || say "(the service was not enabled or not running)"
	sudo pacman -R "$pkgname"
	remove_omarchy_glue
	say "nuncio is uninstalled."
	if [[ -e ${XDG_DATA_HOME:-$HOME/.local/share}/nuncio ]]; then
		say "Left behind: ${XDG_DATA_HOME:-$HOME/.local/share}/nuncio"
	fi
}

# Sourced (packaging/tests/uninstall.sh does, to test the functions above): stop here.
if [[ ${BASH_SOURCE[0]} != "$0" ]]; then
	return 0
fi

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

if ((uninstall)); then
	[[ -z $version && -z $dir ]] || die "--uninstall takes no --version or --dir"
	uninstall_nuncio
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
