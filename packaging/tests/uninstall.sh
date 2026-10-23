#!/usr/bin/env bash
# Tests of `install.sh --uninstall`, runnable anywhere (no Arch, no root, no systemd, no network).
#
# Usage: packaging/tests/uninstall.sh
#
# install.sh refuses to run outside Arch, so each test sources it (sourcing defines its functions
# and runs nothing) inside a subshell with HOME and XDG_DATA_HOME pointed at a temporary folder,
# and with `systemctl` and `sudo` replaced by stubs on PATH that only record their arguments.
# Nothing here touches the real home folder, the real user session or pacman.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(dirname "$(dirname "$here")")
installer=$root/packaging/install.sh
contrib=$root/contrib/omarchy
line='require("hypr.nuncio")'

work=$(mktemp -d "${TMPDIR:-/tmp}/nuncio-uninstall-test.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
failed=0
n=0

fail() {
	printf 'FAIL %s\n' "$*"
	failed=1
}
pass() { printf 'ok   %s\n' "$*"; }
check() { # check DESCRIPTION COMMAND...: passes when COMMAND succeeds
	local what=$1
	shift
	if "$@"; then pass "$what"; else fail "$what"; fi
}
absent() { [[ ! -e $1 && ! -L $1 ]]; }
# The mode string of ls -l: the one form GNU and BSD ls print alike (stat differs).
# shellcheck disable=SC2012
mode_of() { ls -ld -- "$1" | cut -c1-10; }

# Stubs: systemctl and sudo append their arguments to $work/calls; FAIL_PACMAN=1 makes sudo fail.
stubs=$work/bin
mkdir -p "$stubs"
cat >"$stubs/systemctl" <<'EOF'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >>"$NUNCIO_TEST_CALLS"
[[ ${FAIL_SYSTEMCTL:-0} == 0 ]]
EOF
cat >"$stubs/sudo" <<'EOF'
#!/usr/bin/env bash
printf 'sudo %s\n' "$*" >>"$NUNCIO_TEST_CALLS"
[[ ${FAIL_PACMAN:-0} == 0 ]]
EOF
chmod +x "$stubs/systemctl" "$stubs/sudo"

# new_home: sets h to a fresh, empty home folder.
new_home() {
	n=$((n + 1))
	h=$work/home$n
	mkdir -p "$h"
}

# install_glue HOME: what contrib/omarchy/README.md "Install" does, by hand, into HOME.
install_glue() {
	local h=$1 dest=$1/.config/omarchy/plugins/pwn27sbx.nuncio-unread
	mkdir -p "$h/.config/hypr" "$dest"
	cp "$contrib/hypr/nuncio.lua" "$h/.config/hypr/nuncio.lua"
	printf -- '-- bindings\no.bind("SUPER + RETURN", "Terminal", { launch = "terminal" })\n' >"$h/.config/hypr/bindings.lua"
	printf '%s\n' "$line" >>"$h/.config/hypr/bindings.lua"
	cp "$contrib/bar-widget/manifest.json" "$contrib/bar-widget/BarWidget.qml" "$dest/"
}

# glue HOME: runs install.sh's removal of the Omarchy glue for HOME; output in $work/out.
glue() {
	(
		HOME=$1
		# shellcheck source=/dev/null # install.sh is linted on its own
		source "$installer" && remove_omarchy_glue
	) >"$work/out" 2>&1
}

# uninstall HOME: runs install.sh's whole uninstall (with --yes) for HOME; output in $work/out,
# stub calls in $work/calls.
uninstall() {
	: >"$work/calls"
	(
		HOME=$1
		XDG_DATA_HOME=$1/.local/share
		PATH=$stubs:$PATH
		NUNCIO_TEST_CALLS=$work/calls
		export XDG_DATA_HOME PATH NUNCIO_TEST_CALLS
		# shellcheck source=/dev/null # install.sh is linted on its own
		source "$installer" || exit 99
		# yes is install.sh's --yes flag.
		# shellcheck disable=SC2034
		yes=1
		uninstall_nuncio
	) >"$work/out" 2>&1
}
said() { grep -qF -- "$1" "$work/out"; }

# --- 0. the hashes install.sh compares against are the files this checkout ships ---
for spec in "manifest.json|bar-widget/manifest.json" "BarWidget.qml|bar-widget/BarWidget.qml" "nuncio.lua|hypr/nuncio.lua"; do
	name=${spec%%|*}
	file=$contrib/${spec#*|}
	want=$(sha256sum <"$file")
	got=$(
		# shellcheck source=/dev/null # install.sh is linted on its own
		source "$installer" && shipped_sha256 "$name"
	)
	check "install.sh knows the SHA-256 of contrib/omarchy/${spec#*|}" [ "$got" == "${want%% *}" ]
done

# --- 1. widget and hotkey removed when they are what nuncio ships ---
new_home
install_glue "$h"
glue "$h"
check "1: exits 0" [ $? -eq 0 ]
check "1: the widget folder is removed" absent "$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
check "1: the plugins folder itself stays" test -d "$h/.config/omarchy/plugins"
check "1: hypr/nuncio.lua is removed" absent "$h/.config/hypr/nuncio.lua"
check "1: says what it removed" said "Removed: $h/.config/omarchy/plugins/pwn27sbx.nuncio-unread/BarWidget.qml"
check "1: names the folder it removed" said "Removed: $h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
check "1: names the hotkey file it removed" said "Removed: $h/.config/hypr/nuncio.lua"

# --- 2. a file the user changed is left, with a notice ---
new_home
install_glue "$h"
dest=$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread
printf '// mine\n' >>"$dest/BarWidget.qml"
cp "$dest/BarWidget.qml" "$work/qml.before"
glue "$h"
check "2: exits 0" [ $? -eq 0 ]
check "2: the changed BarWidget.qml is left byte for byte" cmp -s "$dest/BarWidget.qml" "$work/qml.before"
check "2: the unchanged manifest.json stays with it (the widget is left whole)" cmp -s "$dest/manifest.json" "$contrib/bar-widget/manifest.json"
check "2: says the changed file was left" said "Left in place: $dest/BarWidget.qml"
# shellcheck disable=SC2016 # $1 and $2 are the inner shell's arguments.
check "2: says nothing of the widget was removed" bash -c '! grep -qF "Removed: $1" "$2"' _ "$dest" "$work/out"
check "2: the hotkey is still removed" absent "$h/.config/hypr/nuncio.lua"
# A file nuncio does not ship in the widget folder also keeps the folder.
new_home
install_glue "$h"
dest=$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread
printf 'x\n' >"$dest/notes.txt"
glue "$h"
check "2: a folder holding a file nuncio does not ship is left whole" test -f "$dest/notes.txt" -a -f "$dest/manifest.json" -a -f "$dest/BarWidget.qml"
check "2: says the foreign file was left" said "Left in place: $dest/notes.txt"
# A changed nuncio.lua is left, and so is the require line that loads it.
new_home
install_glue "$h"
printf -- '-- mine\n' >>"$h/.config/hypr/nuncio.lua"
cp "$h/.config/hypr/bindings.lua" "$work/bindings.before"
glue "$h"
check "2: a changed nuncio.lua is left" test -f "$h/.config/hypr/nuncio.lua"
check "2: bindings.lua keeps the require line of the nuncio.lua that stays" cmp -s "$h/.config/hypr/bindings.lua" "$work/bindings.before"
check "2: no backup when bindings.lua is not edited" absent "$h/.config/hypr/bindings.lua.nuncio-backup"
check "2: says the changed nuncio.lua was left" said "Left in place: $h/.config/hypr/nuncio.lua"

# --- 3. the require line goes, a backup stays, every other byte is kept ---
new_home
install_glue "$h"
b=$h/.config/hypr/bindings.lua
# Lines that only look like it stay; the file ends without a newline; the line appears twice.
printf -- '-- top\n%s\n  %s\n%s -- mine\nlocal x = "\\t"\n%s\n-- last, no newline' "$line" "$line" "$line" "$line" >"$b"
chmod 640 "$b"
cp -p "$b" "$work/bindings.orig"
printf -- '-- top\n  %s\n%s -- mine\nlocal x = "\\t"\n-- last, no newline' "$line" "$line" >"$work/bindings.want"
glue "$h"
check "3: exits 0" [ $? -eq 0 ]
check "3: only the exact require lines are gone, every other byte is the same" cmp -s "$b" "$work/bindings.want"
check "3: the backup holds the file as it was" cmp -s "$h/.config/hypr/bindings.lua.nuncio-backup" "$work/bindings.orig"
check "3: bindings.lua keeps its mode" [ "$(mode_of "$b")" == "$(mode_of "$work/bindings.orig")" ]
# shellcheck disable=SC2012 # plain names this test made itself.
check "3: no temporary file is left in hypr/" [ "$(ls -A "$h/.config/hypr" | tr '\n' ' ')" == "bindings.lua bindings.lua.nuncio-backup " ]
check "3: says it removed the line and where the backup is" said "Removed: the line $line from $b (backup: $b.nuncio-backup)"
# A second run with the backup name taken keeps the first backup and makes another.
printf '\n%s\n' "$line" >>"$b"
cp "$b" "$work/bindings.second"
glue "$h"
check "3: an existing backup is never overwritten" cmp -s "$h/.config/hypr/bindings.lua.nuncio-backup" "$work/bindings.orig"
check "3: the second backup holds the file as it was then" cmp -s "$h/.config/hypr/bindings.lua.nuncio-backup.1" "$work/bindings.second"
# The exact line as the last line, with no newline after it, goes too.
new_home
install_glue "$h"
b=$h/.config/hypr/bindings.lua
printf -- '-- top\n%s' "$line" >"$b"
glue "$h"
check "3: an exact last line without a newline is removed" [ "$(cat "$b")" == "-- top" ]
check "3: the lines before it keep their bytes" cmp -s "$b" <(printf -- '-- top\n')
# Bytes that are not valid UTF-8 are kept as they are, whatever the locale.
new_home
install_glue "$h"
b=$h/.config/hypr/bindings.lua
printf -- '-- \xff\xfe\x80\n%s\nlocal s = "\x80\xff"\n' "$line" >"$b"
printf -- '-- \xff\xfe\x80\nlocal s = "\x80\xff"\n' >"$work/bindings.want"
LC_ALL=en_US.UTF-8 glue "$h"
check "3: invalid UTF-8: the require line is removed, every other byte kept" cmp -s "$b" "$work/bindings.want"
# shellcheck disable=SC2016 # $1 is the inner shell's argument.
check "3: invalid UTF-8: not mistaken for a NUL byte" bash -c '! grep -qF "NUL byte" "$1"' _ "$work/out"
# A write that fails half way leaves bindings.lua as it was, and no temporary file.
new_home
install_glue "$h"
b=$h/.config/hypr/bindings.lua
cp -p "$b" "$work/bindings.before"
(
	HOME=$h
	# shellcheck source=/dev/null # install.sh is linted on its own
	source "$installer" || exit 99
	# The second write of without_line fails, as on a full disk; every other printf works.
	writes=0
	# shellcheck disable=SC2317,SC2329,SC2059 # called by without_line, so reachable (SC2317 is shellcheck
	# 0.9's name for SC2329); it passes printf's own arguments on.
	printf() {
		if [[ ${FUNCNAME[1]} == without_line ]] && ((++writes > 1)); then return 1; fi
		builtin printf "$@"
	}
	remove_omarchy_glue
) >"$work/out" 2>&1
check "3: a failed write: bindings.lua is byte for byte as it was" cmp -s "$b" "$work/bindings.before"
# shellcheck disable=SC2012 # plain names this test made itself.
check "3: a failed write: no temporary file is left in hypr/" [ "$(ls -A "$h/.config/hypr" | tr '\n' ' ')" == "bindings.lua bindings.lua.nuncio-backup nuncio.lua " ]
check "3: a failed write: says it could not rewrite it" said "Left in place: $b (could not rewrite it; backup: $b.nuncio-backup)"
check "3: a failed write: nuncio.lua stays so the line still loads" test -f "$h/.config/hypr/nuncio.lua"

# --- 4. nothing installed: no change, no error ---
new_home
glue "$h"
check "4: an empty home: exits 0" [ $? -eq 0 ]
check "4: an empty home stays empty" [ -z "$(ls -A "$h")" ]
check "4: an empty home: prints nothing" [ ! -s "$work/out" ]
new_home
mkdir -p "$h/.config/hypr" "$h/.config/omarchy/plugins/other.plugin"
printf -- '-- mine\n' >"$h/.config/hypr/bindings.lua"
cp -p "$h/.config/hypr/bindings.lua" "$work/bindings.before"
glue "$h"
check "4: no widget, no hotkey: exits 0" [ $? -eq 0 ]
check "4: bindings.lua without the line is untouched" cmp -s "$h/.config/hypr/bindings.lua" "$work/bindings.before"
check "4: no backup is made" [ "$(ls -A "$h/.config/hypr")" == "bindings.lua" ]
check "4: other plugins stay" test -d "$h/.config/omarchy/plugins/other.plugin"
check "4: prints nothing" [ ! -s "$work/out" ]

# --- 5. symlinks are never followed out ---
out=$work/outside
mkdir -p "$out/widget" "$out/hypr" "$out/lua"
cp "$contrib/bar-widget/manifest.json" "$contrib/bar-widget/BarWidget.qml" "$out/widget/"
cp "$contrib/hypr/nuncio.lua" "$out/hypr/nuncio.lua"
printf '%s\n' "$line" >"$out/hypr/bindings.lua"
cp "$contrib/hypr/nuncio.lua" "$out/lua/nuncio.lua"
snapshot() { (cd "$out" && find . -print | LC_ALL=C sort && cat hypr/bindings.lua); }
before=$(snapshot)
# The widget folder is a symlink to a folder outside.
new_home
mkdir -p "$h/.config/omarchy/plugins"
ln -s "$out/widget" "$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
glue "$h"
check "5: widget folder symlink: exits 0" [ $? -eq 0 ]
check "5: widget folder symlink: the folder it points to is untouched" [ "$(snapshot)" == "$before" ]
check "5: widget folder symlink: the link stays" test -L "$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
check "5: widget folder symlink: says it was left" said "Left in place: $h/.config/omarchy/plugins/pwn27sbx.nuncio-unread is a symlink"
# omarchy/ itself is a symlink.
new_home
mkdir -p "$h/.config" "$out/omarchy/plugins"
cp -R "$out/widget" "$out/omarchy/plugins/pwn27sbx.nuncio-unread"
ln -s "$out/omarchy" "$h/.config/omarchy"
before=$(snapshot)
glue "$h"
check "5: omarchy/ symlink: nothing behind it is removed" [ "$(snapshot)" == "$before" ]
# A file in the widget folder is a symlink to a matching file outside.
new_home
dest=$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread
mkdir -p "$dest"
cp "$contrib/bar-widget/manifest.json" "$dest/"
ln -s "$out/widget/BarWidget.qml" "$dest/BarWidget.qml"
before=$(snapshot)
glue "$h"
check "5: a symlinked widget file: the file it points to is untouched" [ "$(snapshot)" == "$before" ]
check "5: a symlinked widget file: the folder is left whole" test -L "$dest/BarWidget.qml" -a -f "$dest/manifest.json"
# hypr/ is a symlink to a folder holding the shipped nuncio.lua and the require line.
new_home
mkdir -p "$h/.config"
ln -s "$out/hypr" "$h/.config/hypr"
before=$(snapshot)
glue "$h"
check "5: hypr/ symlink: exits 0" [ $? -eq 0 ]
check "5: hypr/ symlink: nothing behind it changes" [ "$(snapshot)" == "$before" ]
check "5: hypr/ symlink: says it was left" said "Left in place: $h/.config/hypr is a symlink"
# nuncio.lua is a symlink to the shipped file outside; bindings.lua is a symlink outside.
new_home
mkdir -p "$h/.config/hypr"
ln -s "$out/lua/nuncio.lua" "$h/.config/hypr/nuncio.lua"
printf '%s\n' "$line" >"$h/.config/hypr/bindings.lua"
cp "$h/.config/hypr/bindings.lua" "$work/bindings.before"
glue "$h"
check "5: nuncio.lua symlink: the link and its target stay" test -L "$h/.config/hypr/nuncio.lua" -a -f "$out/lua/nuncio.lua"
check "5: nuncio.lua symlink: the require line stays with it" cmp -s "$h/.config/hypr/bindings.lua" "$work/bindings.before"
new_home
mkdir -p "$h/.config/hypr"
cp "$contrib/hypr/nuncio.lua" "$h/.config/hypr/nuncio.lua"
ln -s "$out/hypr/bindings.lua" "$h/.config/hypr/bindings.lua"
before=$(snapshot)
glue "$h"
check "5: bindings.lua symlink: the file it points to is untouched" [ "$(snapshot)" == "$before" ]
check "5: bindings.lua symlink: nuncio.lua stays so the line still loads" test -f "$h/.config/hypr/nuncio.lua"
check "5: bindings.lua symlink: says it was left" said "Left in place: $h/.config/hypr/bindings.lua is a symlink"

# --- 6. the uninstall as before: unit disabled, then the package removed, data left ---
new_home
mkdir -p "$h/.local/share/nuncio"
uninstall "$h"
check "6: exits 0" [ $? -eq 0 ]
check "6: disables the unit, then removes the package, nothing else" \
	[ "$(cat "$work/calls")" == "$(printf 'systemctl --user disable --now nunciod.service\nsudo pacman -R nuncio')" ]
check "6: prints the plan" said "  1. systemctl --user disable --now nunciod.service"
check "6: says it is uninstalled" said "nuncio is uninstalled."
check "6: lists the data it leaves" said "Left behind: $h/.local/share/nuncio"
check "6: the data is not touched" test -d "$h/.local/share/nuncio"
new_home
FAIL_SYSTEMCTL=1 uninstall "$h"
check "6: a unit that was not enabled: still removes the package" [ "$(tail -1 "$work/calls")" == "sudo pacman -R nuncio" ]
check "6: a unit that was not enabled: says so" said "(the service was not enabled or not running)"
# shellcheck disable=SC2016 # $1 is the inner shell's argument.
check "6: no data folder: no Left behind line" bash -c '! grep -qF "Left behind" "$1"' _ "$work/out"
# The glue goes with the package: after pacman -R succeeds, never when it fails.
new_home
install_glue "$h"
uninstall "$h"
check "6: uninstall removes the widget" absent "$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
check "6: uninstall removes the hotkey" absent "$h/.config/hypr/nuncio.lua"
new_home
install_glue "$h"
FAIL_PACMAN=1 uninstall "$h"
check "6: pacman -R fails: exits non-zero" [ $? -ne 0 ]
check "6: pacman -R fails: the widget stays" test -d "$h/.config/omarchy/plugins/pwn27sbx.nuncio-unread"
check "6: pacman -R fails: the hotkey stays" test -f "$h/.config/hypr/nuncio.lua"

if ((failed)); then
	printf 'uninstall.sh: some tests failed\n' >&2
	exit 1
fi
printf 'uninstall.sh: all uninstall tests passed\n'
