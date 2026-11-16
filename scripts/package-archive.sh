#!/usr/bin/env bash
# Assemble l'archive de release à partir de binaires déjà construits.
#
#   scripts/package-archive.sh VERSION DOSSIER_BINAIRES DOSSIER_SORTIE
#
# Produit dans DOSSIER_SORTIE :
#   lightning-lcd-VERSION-x86_64-linux.tar.gz   binaires, packaging/, install.sh, README, LICENCE
#   lightning, lightning-settings               binaires seuls
#   SHA256SUMS                                  sommes de contrôle des trois fichiers
set -euo pipefail

[[ $# == 3 ]] || { sed -n '2,9p' "$0" >&2; exit 2; }
VERSION=${1#v}
BIN=$(realpath "$2")
OUT=$(realpath -m "$3")
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ARCH=$(uname -m)
NAME="lightning-lcd-$VERSION-$ARCH-linux"

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
DIR="$STAGE/$NAME"
install -Dm755 "$BIN/lightning" "$DIR/bin/lightning"
install -Dm755 "$BIN/lightning-settings" "$DIR/bin/lightning-settings"
install -Dm755 "$ROOT/packaging/install.sh" "$DIR/install.sh"
for f in 70-lightning-lcd.rules lightning-dashboard.service cloud.wizops.Lightning.desktop \
    config.example.toml config.example.fr.toml icons/cloud.wizops.Lightning.svg icons/cloud.wizops.Lightning-symbolic.svg; do
    install -Dm644 "$ROOT/packaging/$f" "$DIR/packaging/$f"
done
install -m644 "$ROOT/README.md" "$ROOT/README.fr.md" "$ROOT/LICENSE" "$DIR/"

mkdir -p "$OUT"
# Archive reproductible : ordre, dates et propriétaires fixés.
EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || date +%s)}
tar --sort=name --mtime="@$EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$STAGE" -cf - "$NAME" | gzip -n -9 >"$OUT/$NAME.tar.gz"
install -m755 "$BIN/lightning" "$BIN/lightning-settings" "$OUT/"
(cd "$OUT" && sha256sum "$NAME.tar.gz" lightning lightning-settings >SHA256SUMS)
cat "$OUT/SHA256SUMS"
