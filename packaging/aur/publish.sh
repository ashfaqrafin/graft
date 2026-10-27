#!/bin/sh
# Publish (or update) an AUR package from this folder.
#
#   packaging/aur/publish.sh git-graft        # release package
#   packaging/aur/publish.sh git-graft-git    # latest-commit package
#
# Needs an AUR account with your SSH key added (aur.archlinux.org → My Account).
# For a new release: bump pkgver in the PKGBUILD first; this script refreshes the
# checksum and .SRCINFO, then pushes to aur.archlinux.org.
set -eu

pkg="${1:?usage: publish.sh <git-graft|git-graft-git>}"
here="$(cd "$(dirname "$0")" && pwd)"
src="$here/$pkg"
[ -f "$src/PKGBUILD" ] || { echo "no PKGBUILD in $src" >&2; exit 1; }

cd "$src"
if [ "$pkg" = git-graft ]; then
    updpkgsums # the -git package builds from git and has no checksum
fi
makepkg --printsrcinfo > .SRCINFO

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git clone -q "ssh://aur@aur.archlinux.org/$pkg.git" "$work/repo"
cp PKGBUILD .SRCINFO "$work/repo/"
cd "$work/repo"
git add PKGBUILD .SRCINFO
if git diff --cached --quiet; then
    echo "$pkg: AUR already up to date"
    exit 0
fi
version="$(sed -n 's/^\tpkgver = //p' .SRCINFO)-$(sed -n 's/^\tpkgrel = //p' .SRCINFO)"
git commit -q -m "Update to $version"
git push -q origin HEAD:master
echo "$pkg $version published: https://aur.archlinux.org/packages/$pkg"
