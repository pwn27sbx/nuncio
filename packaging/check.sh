#!/usr/bin/env bash
# Static checks of the packaging files, runnable anywhere (no Linux, no makepkg, no network).
#
# Usage: packaging/check.sh
#
# 1. PKGBUILD.in holds the required build options (Sealwax-style offline build).
# 2. package() installs every path in MANIFEST, and MANIFEST lists every path package() installs.
# 3. Every file package() copies from the source tree exists in this checkout.
# 4. The unit, D-Bus service and desktop entries carry the lines nuncio depends on; the app icon
#    is valid, clean SVG, installed, and named by every Icon=.
# 5. The shell scripts parse (bash -n), and shellcheck passes when it is installed.
# Exits non-zero on the first failing group, after printing every failure in it.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(dirname "$here")
pkgbuild=$here/PKGBUILD.in
failed=0

fail() {
	printf 'FAIL %s\n' "$*"
	failed=1
}
pass() { printf 'ok   %s\n' "$*"; }
end_group() {
	if ((failed)); then
		printf 'check.sh: %s failed\n' "$1" >&2
		exit 1
	fi
}

has() { # has FILE FIXED-STRING DESCRIPTION
	if grep -qF -- "$2" "$1"; then pass "$3"; else fail "$3 ($(basename "$1") lacks: $2)"; fi
}

# --- 1. required PKGBUILD options ---
has "$pkgbuild" "options=('!debug' '!lto')" "PKGBUILD: options !debug !lto"
has "$pkgbuild" 'cargo fetch --locked' "PKGBUILD: cargo fetch --locked in prepare()"
has "$pkgbuild" 'cargo build --frozen --release' "PKGBUILD: offline cargo release build"
has "$pkgbuild" 'cargo test --frozen' "PKGBUILD: offline cargo tests"
has "$pkgbuild" 'go mod download' "PKGBUILD: go mod download in prepare()"
has "$pkgbuild" 'export CGO_ENABLED=0' "PKGBUILD: CGO_ENABLED=0"
has "$pkgbuild" '-trimpath' "PKGBUILD: -trimpath"
has "$pkgbuild" '-mod=readonly' "PKGBUILD: -mod=readonly"
has "$pkgbuild" 'export GOPROXY=off' "PKGBUILD: GOPROXY=off in build() and check()"
has "$pkgbuild" 'export GOTOOLCHAIN=local' "PKGBUILD: GOTOOLCHAIN=local"
for placeholder in @PKGVER@ @SHA256@ @SOURCE_DATE_EPOCH@; do
	has "$pkgbuild" "$placeholder" "PKGBUILD: placeholder $placeholder"
done
# The network steps are in prepare() only: build() and check() never fetch.
offline=$(sed -n '/^build() {/,/^}/p; /^check() {/,/^}/p' "$pkgbuild")
if grep -qE 'go mod download|cargo fetch|curl|wget|GOPROXY=https' <<<"$offline"; then
	fail "PKGBUILD: build() or check() fetches something"
else
	pass "PKGBUILD: build() and check() fetch nothing"
fi
end_group "PKGBUILD options"

# --- 2. MANIFEST <-> package() ---
body=$(sed -n '/^package() {/,/^}/p' "$pkgbuild" | sed -e ':a' -e '/\\$/N; s/\\\n[[:space:]]*/ /; ta')
installed=$(grep -oE '"\$pkgdir(/[^"]+)"' <<<"$body" | sed -E 's/^"\$pkgdir//; s/"$//' | sort)
manifest=$(grep -vE '^[[:space:]]*(#|$)' "$here/MANIFEST" | sort)
missing=$(comm -23 <(printf '%s\n' "$manifest") <(printf '%s\n' "$installed"))
extra=$(comm -13 <(printf '%s\n' "$manifest") <(printf '%s\n' "$installed"))
[[ -z $missing ]] || fail "MANIFEST paths that package() does not install: $(tr '\n' ' ' <<<"$missing")"
[[ -z $extra ]] || fail "package() installs paths missing from MANIFEST: $(tr '\n' ' ' <<<"$extra")"
if [[ -z $missing && -z $extra ]]; then
	pass "MANIFEST matches package(): $(grep -c . <<<"$manifest") paths"
fi
end_group "MANIFEST"

# --- 3. sources of package() exist ---
# Each `install -Dm... SOURCE "$pkgdir/..."`: SOURCE relative to the source tree. Build outputs
# (build/, target/) do not exist before makepkg and are skipped.
while read -r src; do
	case $src in
	build/* | target/*) continue ;;
	esac
	if [[ -f $root/$src ]]; then pass "source exists: $src"; else fail "package() copies a missing file: $src"; fi
done < <(grep -oE 'install -Dm[0-7]+ [^ ]+' <<<"$body" | awk '{print $3}')
end_group "package() sources"

# --- 4. the lines the integration depends on ---
unit=$here/nunciod.service
has "$unit" 'Type=dbus' "unit: Type=dbus"
has "$unit" 'BusName=io.github.pwn27sbx.nuncio' "unit: BusName matches the daemon's name (internal/ipc DaemonName)"
has "$unit" 'ExecStart=/usr/bin/nunciod' "unit: ExecStart"
has "$unit" 'Restart=on-failure' "unit: Restart=on-failure"
has "$unit" 'RuntimeDirectory=nuncio' "unit: RuntimeDirectory for the socket"
has "$unit" 'WantedBy=default.target' "unit: WantedBy=default.target"
restart_sec=$(sed -n 's/^RestartSec=\([0-9]*\)s$/\1/p' "$unit")
if [[ -n $restart_sec ]] && ((restart_sec >= 10)); then
	pass "unit: RestartSec >= 10s (${restart_sec}s)"
else
	fail "unit: RestartSec must be at least 10s (ban-risk rule)"
fi
if grep -qE '^(ProtectHome|PrivateNetwork|IPAddressDeny|PrivateUsers)=' "$unit"; then
	fail "unit: a setting that hides the data dir or the network is on"
else
	pass "unit: no ProtectHome/PrivateNetwork/IPAddressDeny"
fi
# The daemon's name in the code and in the packaging files must be the same string.
code_name=$(sed -n 's/^[[:space:]]*DaemonName = "\(.*\)"$/\1/p' "$root/internal/ipc/connect.go")
has "$here/io.github.pwn27sbx.nuncio.service" "Name=$code_name" "D-Bus service: Name= is the code's DaemonName ($code_name)"
has "$here/io.github.pwn27sbx.nuncio.service" 'SystemdService=nunciod.service' "D-Bus service: hands activation to the unit"
# The GTK app's id and bus name: its own, never the daemon's (they collided once; README).
app_id=io.github.pwn27sbx.nuncio.Desktop
if [[ $app_id == "$code_name" ]]; then
	fail "the app id $app_id is the daemon's bus name"
fi
app=$here/$app_id.desktop
app_service=$here/$app_id.service
has "$app" 'Exec=nuncio-desktop %U' "app entry: Exec with %U (crates/nuncio-desktop/README.md)"
has "$app" 'MimeType=x-scheme-handler/whatsapp;' "app entry: whatsapp: handler"
has "$app" "StartupWMClass=$app_id" "app entry: StartupWMClass is the app id"
has "$app" 'DBusActivatable=true' "app entry: DBusActivatable=true"
has "$app_service" "Name=$app_id" "app D-Bus service: Name= is the app id"
has "$app_service" 'Exec=/usr/bin/nuncio-desktop --gapplication-service' "app D-Bus service: starts it as a GApplication service"
has "$here/nuncio-autostart.desktop" 'Exec=nuncio-desktop --gapplication-service' "autostart entry: starts it in the background"
# No file names the other's identity: the daemon's files never carry the app id, and the app's
# files never carry the daemon's name as a whole value.
if grep -qF "$app_id" "$unit" "$here/io.github.pwn27sbx.nuncio.service"; then
	fail "a daemon file names the app id"
else
	pass "the daemon's files do not name the app id"
fi
if grep -qE "=${code_name//./\\.}(;|$)" "$app" "$app_service" "$here/nuncio-autostart.desktop" "$here/nuncio-tui.desktop"; then
	fail "an app file claims the daemon's name $code_name"
else
	pass "the app's files do not claim the daemon's name"
fi
has "$here/nuncio-tui.desktop" 'Terminal=true' "TUI entry: Terminal=true"
for entry in "$here"/*.desktop; do
	if [[ $(head -1 "$entry") == '[Desktop Entry]' ]] && grep -q '^Type=Application$' "$entry" &&
		grep -q '^Name=' "$entry" && grep -q '^Exec=' "$entry"; then
		pass "$(basename "$entry"): has the required keys"
	else
		fail "$(basename "$entry"): missing [Desktop Entry], Type, Name or Exec"
	fi
done
if command -v desktop-file-validate >/dev/null; then
	for entry in "$here"/*.desktop; do
		if desktop-file-validate "$entry"; then pass "desktop-file-validate $(basename "$entry")"; else fail "desktop-file-validate $(basename "$entry")"; fi
	done
else
	printf 'skip desktop-file-validate (not installed)\n'
fi
if command -v systemd-analyze >/dev/null; then
	if systemd-analyze --user verify "$unit"; then pass "systemd-analyze verify"; else fail "systemd-analyze verify"; fi
else
	printf 'skip systemd-analyze verify (not installed)\n'
fi
end_group "integration files"

# --- 4b. the app icon ---
# icon_check FILE VIEWBOX: parses as XML, has that viewBox on <svg>, and holds no raster, script
# or remote reference (a hand-written icon needs none of them).
icon_check() {
	python3 -I -c '
import sys, xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
if root.tag != "{http://www.w3.org/2000/svg}svg":
    sys.exit("root element is %s, not svg" % root.tag)
if root.get("viewBox") != sys.argv[2]:
    sys.exit("viewBox is %r, expected %r" % (root.get("viewBox"), sys.argv[2]))
' "$1" "$2"
}
icon_dir=$here/icons/hicolor
# One "path|viewBox" per icon (a plain list: macOS's /bin/bash 3.2 has no associative arrays).
for spec in "scalable/apps/$app_id.svg|0 0 128 128" "symbolic/apps/$app_id-symbolic.svg|0 0 16 16"; do
	rel=${spec%%|*}
	viewbox=${spec#*|}
	icon=$icon_dir/$rel
	if [[ ! -f $icon ]]; then
		fail "icon missing: icons/hicolor/$rel"
		continue
	fi
	if err=$(icon_check "$icon" "$viewbox" 2>&1); then
		pass "icon $rel: valid SVG, viewBox $viewbox"
	else
		fail "icon $rel: ${err##*$'\n'}"
	fi
	if grep -qiE '<image|<script|xlink:href="http|href="http' "$icon"; then
		fail "icon $rel: holds an <image>, a <script> or a remote reference"
	else
		pass "icon $rel: no raster, script or remote reference"
	fi
	if grep -qF "\"\$pkgdir/usr/share/icons/hicolor/$rel\"" <<<"$body"; then
		pass "icon $rel: package() installs it"
	else
		fail "icon $rel: package() does not install it to /usr/share/icons/hicolor/$rel"
	fi
done
# Every Icon= names an icon that package() installs (by its name, without the extension).
installed_icons=$(grep -oE '^/usr/share/icons/hicolor/[^/]+/apps/[^/]+\.svg$' <<<"$installed" |
	sed -E 's|.*/||; s|\.svg$||' | sort -u)
for entry in "$here"/*.desktop; do
	while read -r name; do
		[[ -n $name ]] || continue
		if grep -qxF -- "$name" <<<"$installed_icons"; then
			pass "$(basename "$entry"): Icon=$name is installed"
		else
			fail "$(basename "$entry"): Icon=$name names no installed icon"
		fi
	done < <(sed -n 's/^Icon=//p' "$entry")
done
end_group "app icon"

# --- 5. scripts parse ---
for script in "$here"/*.sh; do
	if bash -n "$script"; then pass "bash -n $(basename "$script")"; else fail "bash -n $(basename "$script")"; fi
done
filled=$(sed -e 's/@PKGVER@/0.0.0/; s/@SHA256@/0/; s/@SOURCE_DATE_EPOCH@/0/' "$pkgbuild")
if bash -n <<<"$filled"; then pass "bash -n PKGBUILD (placeholders filled)"; else fail "bash -n PKGBUILD"; fi
if command -v shellcheck >/dev/null; then
	for script in "$here"/*.sh; do
		if shellcheck "$script"; then pass "shellcheck $(basename "$script")"; else fail "shellcheck $(basename "$script")"; fi
	done
else
	printf 'skip shellcheck (not installed)\n'
fi
end_group "scripts"

# --- 6. the code agrees with the packaging ---
# `const APP_ID: &str = "...";` (pub or not) in any .rs file under crates/nuncio-desktop/src.
desktop_src=$root/crates/nuncio-desktop/src
app_id_defs=$(grep -rhE --include='*.rs' '^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?const APP_ID: &str = "[^"]*";' "$desktop_src" |
	sed -E 's/.*const APP_ID: &str = "([^"]*)";.*/\1/' | sort -u)
app_id_count=$(grep -c . <<<"$app_id_defs")
if ((app_id_count == 0)); then
	fail "no 'const APP_ID: &str = \"...\";' found under crates/nuncio-desktop/src"
elif ((app_id_count > 1)); then
	fail "crates/nuncio-desktop/src defines APP_ID with different values: $(tr '\n' ' ' <<<"$app_id_defs")"
elif [[ $app_id_defs == "$app_id" ]]; then
	pass "crates/nuncio-desktop APP_ID is $app_id"
else
	fail "crates/nuncio-desktop APP_ID is $app_id_defs, the packaging says $app_id"
fi
end_group "code agreement"

printf 'check.sh: all packaging checks passed\n'
