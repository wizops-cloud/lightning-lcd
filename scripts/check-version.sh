#!/usr/bin/env bash
# Vérifie que la version est la même partout où elle est écrite.
#
#   scripts/check-version.sh            # cohérence des fichiers ; affiche la version
#   scripts/check-version.sh v0.2.0     # en plus : l'étiquette correspond et le CHANGELOG
#                                       # a une section datée pour cette version
#
# Sources comparées : Cargo.toml (workspace), packaging/rpm/lightning-lcd.spec,
# packaging/cloud.wizops.Lightning.metainfo.xml (première entrée <release>), CHANGELOG.md.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
fail() { echo "erreur : $*" >&2; exit 1; }

cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
spec_version=$(sed -n 's/^Version:[[:space:]]*//p' packaging/rpm/lightning-lcd.spec | head -1)
meta_line=$(grep -m1 '<release version=' packaging/cloud.wizops.Lightning.metainfo.xml || true)
meta_version=$(sed -n 's/.*<release version="\([^"]*\)".*/\1/p' <<<"$meta_line")
meta_date=$(sed -n 's/.*date="\([^"]*\)".*/\1/p' <<<"$meta_line")

[[ $cargo_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "version du Cargo.toml illisible : « $cargo_version »"
[[ $spec_version == "$cargo_version" ]] || fail "spec RPM en $spec_version, Cargo.toml en $cargo_version"
[[ $meta_version == "$cargo_version" ]] || fail "metainfo en « $meta_version », Cargo.toml en $cargo_version"
[[ $meta_date =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail "date de release illisible dans la metainfo : « $meta_date »"
grep -q "^## \[Unreleased\]" CHANGELOG.md || fail "CHANGELOG.md : section [Unreleased] absente"

if [[ $# -ge 1 ]]; then
    tag=$1
    [[ $tag == "v$cargo_version" ]] || fail "étiquette $tag ≠ v$cargo_version (Cargo.toml)"
    heading=$(grep -m1 "^## \[$cargo_version\] - " CHANGELOG.md) || fail "CHANGELOG.md : pas de section « ## [$cargo_version] - AAAA-MM-JJ »"
    changelog_date=${heading##* - }
    [[ $changelog_date =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail "CHANGELOG.md : date illisible pour $cargo_version"
    [[ $changelog_date == "$meta_date" ]] || fail "dates différentes : CHANGELOG $changelog_date, metainfo $meta_date"
    grep -q "^\[$cargo_version\]: " CHANGELOG.md || fail "CHANGELOG.md : lien « [$cargo_version]: … » absent en bas de fichier"
    [[ -n $(scripts/release-notes.sh "$cargo_version") ]] || fail "CHANGELOG.md : section $cargo_version vide"
fi
echo "$cargo_version"
