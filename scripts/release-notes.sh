#!/usr/bin/env bash
# Affiche les notes d'une version : sa section du CHANGELOG, sans le titre.
#
#   scripts/release-notes.sh 0.2.0
set -euo pipefail

[[ $# == 1 ]] || { sed -n '2,4p' "$0" >&2; exit 2; }
VERSION=${1#v}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

awk -v version="$VERSION" '
    /^## \[/ { inside = (index($0, "## [" version "]") == 1); next }
    /^\[[^]]+\]: / { next }          # liens de bas de fichier
    inside { lines[++n] = $0 }
    END {
        first = 1; while (first <= n && lines[first] == "") first++
        last = n;  while (last >= first && lines[last] == "") last--
        for (i = first; i <= last; i++) print lines[i]
    }
' "$ROOT/CHANGELOG.md"
