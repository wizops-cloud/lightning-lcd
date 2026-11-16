#!/usr/bin/env bash
# Prépare les sources du paquet RPM et construit le SRPM (et, avec --rpm, les RPM binaires).
#
#   packaging/rpm/build-srpm.sh [--rpm] [--ref REF] [DOSSIER_SORTIE]
#
# - Source0 : `git archive` de REF (défaut : HEAD), préfixe lightning-lcd-VERSION/ ;
# - Source1 : `cargo vendor` des dépendances Rust, pour une construction hors ligne
#   (mock, COPR) ;
# - SRPM (et RPM) dans DOSSIER_SORTIE (défaut : target/rpm-dist).
# La version est lue dans le spec ; elle doit correspondre à celle du Cargo.toml.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
SPEC="$ROOT/packaging/rpm/lightning-lcd.spec"
REF=HEAD
BINARY=0
OUT="$ROOT/target/rpm-dist"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --rpm) BINARY=1 ;;
        --ref) REF=$2; shift ;;
        -h | --help) sed -n '2,10p' "$0"; exit 0 ;;
        -*) echo "option inconnue : $1" >&2; exit 2 ;;
        *) OUT=$1 ;;
    esac
    shift
done
OUT=$(realpath -m "$OUT")

VERSION=$(rpmspec -q --srpm --qf '%{version}' "$SPEC")
CRATE=$(git -C "$ROOT" show "$REF:Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
[[ $VERSION == "$CRATE" ]] || { echo "spec $VERSION ≠ Cargo.toml $CRATE" >&2; exit 1; }

SOURCES="$OUT/SOURCES"
mkdir -p "$SOURCES"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

git -C "$ROOT" archive --format=tar.gz --prefix="lightning-lcd-$VERSION/" \
    -o "$SOURCES/lightning-lcd-$VERSION.tar.gz" "$REF"

# Dépendances embarquées, depuis l'arbre exact de REF.
tar -xzf "$SOURCES/lightning-lcd-$VERSION.tar.gz" -C "$WORK"
(cd "$WORK/lightning-lcd-$VERSION" && cargo vendor --locked --quiet vendor >/dev/null)
tar -C "$WORK/lightning-lcd-$VERSION" --sort=name --owner=0 --group=0 --numeric-owner \
    -cJf "$SOURCES/lightning-lcd-$VERSION-vendor.tar.xz" vendor

rpmbuild -bs "$SPEC" --define "_sourcedir $SOURCES" --define "_srcrpmdir $OUT"
if [[ $BINARY == 1 ]]; then
    rpmbuild --rebuild "$OUT/lightning-lcd-$VERSION"-*.src.rpm \
        --define "_topdir $OUT/build" --define "_rpmdir $OUT"
fi
ls -1 "$OUT"/*.rpm "$OUT"/*/*.rpm 2>/dev/null || true
