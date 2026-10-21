#!/usr/bin/env bash
# Build the source tarball of the committed tree (HEAD) and a PKGBUILD for it.
#
# Usage: packaging/package-source.sh [OUT_DIR]     (default OUT_DIR: .local/package)
#
# Writes OUT_DIR/nuncio-<version>.tar.gz (`git archive HEAD`, gzip without a timestamp, so the
# same commit gives the same bytes) and OUT_DIR/PKGBUILD from packaging/PKGBUILD.in, with
# @PKGVER@ (the workspace version in Cargo.toml), @SHA256@ (the tarball's) and
# @SOURCE_DATE_EPOCH@ (HEAD's commit time) filled in. Uncommitted changes are not packaged; it
# says so. It only reads the repository and writes OUT_DIR: no network, no makepkg, no install.
#
# The same steps as Sealwax's tools/package-source.py (MIT,
# https://github.com/PixDevsApps/sealwax), in shell.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out=${1:-$root/.local/package}

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml")
if [[ -z $version ]]; then
	echo "package-source: no [workspace.package] version in Cargo.toml" >&2
	exit 1
fi
# nunciod reports its own version in every frame; a package whose daemon says another version
# would be confusing, so the two must agree.
daemon=$(sed -n 's/^const daemonVersion = "nunciod\/\(.*\)"$/\1/p' "$root/cmd/nunciod/main.go")
if [[ $daemon != "$version" ]]; then
	echo "package-source: Cargo.toml says $version but cmd/nunciod says ${daemon:-nothing}" >&2
	exit 1
fi

if [[ -n $(git -C "$root" status --porcelain --untracked-files=no) ]]; then
	echo "warning: the working tree has uncommitted changes; only HEAD is packaged" >&2
fi
commit=$(git -C "$root" rev-parse HEAD)
epoch=$(git -C "$root" log -1 --format=%ct HEAD)

mkdir -p "$out"
archive=$out/nuncio-$version.tar.gz
git -C "$root" archive --format=tar --prefix="nuncio-$version/" HEAD | gzip -n -9 >"$archive"

if command -v sha256sum >/dev/null; then
	digest=$(sha256sum "$archive" | cut -d' ' -f1)
else
	digest=$(shasum -a 256 "$archive" | cut -d' ' -f1)
fi

recipe=$(cat "$root/packaging/PKGBUILD.in")
for placeholder in @PKGVER@ @SHA256@ @SOURCE_DATE_EPOCH@; do
	if [[ $recipe != *"$placeholder"* ]]; then
		echo "package-source: $placeholder is missing from packaging/PKGBUILD.in" >&2
		exit 1
	fi
done
recipe=${recipe//@PKGVER@/$version}
recipe=${recipe//@SHA256@/$digest}
recipe=${recipe//@SOURCE_DATE_EPOCH@/$epoch}
if grep -qE '@[A-Z_]+@' <<<"$recipe"; then
	echo "package-source: an unknown placeholder is left in the PKGBUILD" >&2
	exit 1
fi
printf '%s\n' "$recipe" >"$out/PKGBUILD"

echo "$archive"
echo "commit $commit"
echo "SHA256 $digest"
echo "SOURCE_DATE_EPOCH $epoch"
echo "Build on Arch: cd $out && makepkg --cleanbuild --force   (-si also installs it)"
