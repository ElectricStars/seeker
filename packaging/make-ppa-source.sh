#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds an unsigned Launchpad source package for Seeker.
#
#   packaging/make-ppa-source.sh TAG OUTDIR
#   e.g. packaging/make-ppa-source.sh v0.1.1 ../ppa-seeker
#
# Produces in OUTDIR:
#   seeker_<ver>.orig.tar.xz          the tagged upstream source (git archive)
#   seeker_<ver>.orig-vendor.tar.xz   all crates, vendored for Linux targets
#   seeker_<debver>.debian.tar.xz, .dsc, _source.changes, _source.buildinfo
# The debian/ directory and the version come from the current checkout, so
# update debian/changelog (dch) first. Needs: git, cargo, cargo-vendor-filterer,
# python3, dpkg-dev, debhelper, xz-utils.
set -eu
TAG=$1
OUT=$(realpath -m "$2")
REPO=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)
VER=${TAG#v}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

git -C "$REPO" archive --format=tar --prefix="seeker-$VER/" "$TAG" | xz -9 > "$WORK/seeker_$VER.orig.tar.xz"
tar -C "$WORK" -xf "$WORK/seeker_$VER.orig.tar.xz"
SRC="$WORK/seeker-$VER"

# Vendor only what Linux builds need; drop prebuilt Windows test DLLs.
(cd "$SRC" && cargo vendor-filterer \
    --platform='*-unknown-linux-gnu' --platform='*-unknown-linux-gnueabihf' \
    --exclude-crate-path='libloading#tests' vendor >/dev/null)
(cd "$SRC" && tar --sort=name --mtime="@$(git -C "$REPO" log -1 --format=%ct "$TAG")" \
    --owner=0 --group=0 --numeric-owner -cf - vendor) | xz -T0 -9 > "$WORK/seeker_$VER.orig-vendor.tar.xz"

cp -a "$REPO/debian" "$SRC/"
python3 "$REPO/packaging/gen-copyright.py" "$SRC/vendor" > "$SRC/debian/copyright"
(cd "$SRC" && dpkg-buildpackage -S -us -uc -d -sa)
DEBVER=$(dpkg-parsechangelog -l "$REPO/debian/changelog" -S Version)
for f in "seeker_$VER.orig.tar.xz" "seeker_$VER.orig-vendor.tar.xz" "seeker_$DEBVER.debian.tar.xz" "seeker_$DEBVER.dsc" \
         "seeker_${DEBVER}_source.changes" "seeker_${DEBVER}_source.buildinfo"; do
    mv "$WORK/$f" "$OUT/"
done
echo "Source package written to $OUT"
