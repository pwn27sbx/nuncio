#!/usr/bin/env bash
# S12: the pages name only what exists. Every `install.sh --flag` a page mentions is a flag the
# script accepts, INSTALL.md unlinks before `pacman -R`, names the three leftovers, and runs no
# unit command.
#
# Run: bash packaging/tests/docs.sh
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/../.." && pwd)
fails=0
ok() { printf 'ok   %s\n' "$*"; }
no() { printf 'FAIL %s\n' "$*"; fails=$((fails + 1)); }

# A page that names a flag the script does not parse is a copy-paste failure waiting to happen.
help=$(bash "$root/packaging/install.sh" --help 2>&1)
for doc in README.md INSTALL.md INTEGRATIONS.md packaging/README.md packaging/nunciod.service; do
	[[ -f $root/$doc ]] || continue
	while read -r flag; do
		[[ -n $flag ]] || continue
		if grep -q -- "$flag" <<<"$help"; then
			ok "$doc: $flag is a flag install.sh accepts"
		else
			no "$doc names $flag, which install.sh rejects"
		fi
	done < <(grep -oE 'install\.sh( +\[?--[a-z-]+\]?)+' "$root/$doc" | grep -oE -- '--[a-z-]+' | sort -u)
done

unlink=$(grep -n 'Unlink first' "$root/INSTALL.md" | head -1 | cut -d: -f1)
remove=$(grep -n '^sudo pacman -R nuncio$' "$root/INSTALL.md" | head -1 | cut -d: -f1)
if [[ -n $unlink && -n $remove && $unlink -lt $remove ]]; then
	ok "INSTALL.md: unlink is named before pacman -R"
else
	no "INSTALL.md: 'Unlink first' must come before 'sudo pacman -R nuncio'"
fi

for leftover in '~/.local/share/nuncio' 'the keyring item `nuncio` / `store.data_key`' \
	'~/.config/systemd/user/default.target.wants/nunciod.service'; do
	if grep -qF -- "$leftover" "$root/INSTALL.md"; then
		ok "INSTALL.md names the leftover: $leftover"
	else
		no "INSTALL.md does not name the leftover: $leftover"
	fi
done

# A command block in the user page runs no unit command: the app starts the daemon itself.
if awk '/^```/{f=!f;next} f && /systemctl/' "$root/INSTALL.md" | grep -q .; then
	no "INSTALL.md: a command block still runs systemctl"
else
	ok "INSTALL.md: no systemctl inside a command block"
fi

if ((fails > 0)); then
	echo "docs.sh: $fails failed"
	exit 1
fi
echo "docs.sh: all docs checks passed"
