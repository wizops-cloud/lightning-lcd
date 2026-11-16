#!/usr/bin/env bash
# Régénère les captures de la documentation, en français et en anglais :
# docs/screenshots/*.png, docs/screenshots/en/*.png, docs/{dashboard,clock,now-playing}.png
# et docs/en/{dashboard,clock,now-playing}.png.
#
#   scripts/screenshots.sh [--no-build] [--out DOSSIER]
#
# Tout se passe à l'écart de la session de l'utilisateur :
#   - compositeur sans écran (`mutter --headless`) sur son propre bus D-Bus ;
#   - démon `lightning --no-lcd --demo daemon` (données factices, aucun accès à l'écran
#     LCD) sur un bus de session PRIVÉ (`dbus-run-session`), avec une configuration, un
#     cache et des dossiers XDG temporaires ;
#   - faux gestionnaire systemd (scripts/fake-systemd.py) sur ce même bus privé, pour que
#     la page « Service » montre un service installé et en marche : le vrai service
#     utilisateur n'est jamais interrogé ni modifié ;
#   - photos de démonstration dessinées à la volée par ImageMagick (aucun fichier
#     personnel).
#
# Prérequis : mutter, dbus-run-session, gdbus, ImageMagick (`magick`), python3-gobject,
# et de quoi construire l'application (voir le README).
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT="$ROOT/docs/screenshots"
BUILD=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) BUILD=0 ;;
        --out) OUT=$(realpath -m "$2"); shift ;;
        -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 2 ;;
    esac
    shift
done

# Outils système d'abord : une installation tierce de dbus (Homebrew…) ne doit pas passer
# devant celle du système.
export PATH="/usr/bin:$PATH"
for tool in mutter dbus-run-session gdbus magick python3; do
    command -v "$tool" >/dev/null || { echo "outil manquant : $tool" >&2; exit 1; }
done
python3 -c 'import gi; gi.require_version("Gio", "2.0")' 2>/dev/null \
    || { echo "python3-gobject manquant (faux systemd)" >&2; exit 1; }

TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ $BUILD == 1 ]]; then
    cargo build --release --manifest-path "$ROOT/Cargo.toml" -p lightning -p lightning-gtk
fi
LIGHTNING="$TARGET/release/lightning"
SETTINGS="$TARGET/release/lightning-settings"
[[ -x $LIGHTNING && -x $SETTINGS ]] || { echo "binaires absents de $TARGET/release" >&2; exit 1; }

WORK=$(mktemp -d -t lightning-shots.XXXXXX)
DISPLAY_NAME="wayland-lightning-shot-$$"
MUTTER_PID=""
cleanup() {
    local status=$?
    if [[ $status != 0 && -f $WORK/mutter.log ]]; then
        echo "--- journal du compositeur" >&2
        tail -n 30 "$WORK/mutter.log" >&2
    fi
    if [[ -n $MUTTER_PID ]]; then
        # Seul le mutter lancé ici (enfant de notre dbus-run-session) est visé ; son
        # dbus-run-session se termine avec lui.
        pkill -x -P "$MUTTER_PID" mutter 2>/dev/null || true
        for _ in $(seq 50); do
            kill -0 "$MUTTER_PID" 2>/dev/null || break
            sleep 0.2
        done
        kill "$MUTTER_PID" 2>/dev/null || true
        wait "$MUTTER_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# --- Photos de démonstration : paysages dessinés, reproductibles (graine fixe) ----------
ridge() { # graine, ligne de base, amplitude → points d'un polygone de crête
    awk -v s="$1" -v base="$2" -v amp="$3" 'BEGIN {
        srand(s); printf "0,1000 "; y = base
        for (x = 0; x <= 1600; x += 40) {
            y += (rand() - 0.5) * amp
            if (y < base - amp * 3) y = base - amp * 3
            if (y > base + amp * 2) y = base + amp * 2
            printf "%d,%d ", x, y
        }
        printf "1600,1000" }'
}
landscape() { # fichier, graine, ciel haut, ciel bas, soleil, crête lointaine, crête proche
    local out=$1 seed=$2
    local sx=$((400 + seed * 137 % 800)) sy=$((260 + seed * 53 % 160))
    magick -size 1600x1000 "gradient:$3-$4" \
        -fill "$5" -draw "circle $sx,$sy $((sx + 70)),$sy" \
        -fill "$6" -draw "polygon $(ridge "$seed" 620 90)" \
        -fill "$7" -draw "polygon $(ridge $((seed + 100)) 800 70)" \
        \( -size 1600x1000 -seed "$seed" plasma:fractal -blur 0x12 -colorspace gray -level 30%,70% \) \
        -compose soft-light -composite -strip -quality 88 "$out"
}
PALETTES=(
    '#16324f #f2a65a #ffe6a8 #3a4a6b #1d2638'
    '#0e4d64 #a8e0e9 #ffffff #2f6f73 #173f3f'
    '#3b1f4a #ff8a5c #ffd29d #5a2d4f #2a1426'
    '#1c3d2b #d8e8b0 #fff7d6 #3d6b45 #1e3a24'
    '#2b2d42 #8d99ae #edf2f4 #4a4e69 #22223b'
    '#5f0f40 #fb8b24 #ffd6a5 #9a031e #3d0a26'
)
PHOTOS="$WORK/photos"
seed=1
for album in "Vacances 2026" "Famille" "Montagne"; do
    mkdir -p "$PHOTOS/$album"
    for i in 1 2 3 4; do
        # shellcheck disable=SC2086
        landscape "$PHOTOS/$album/photo-$i.jpg" "$seed" ${PALETTES[$((seed % ${#PALETTES[@]}))]}
        seed=$((seed + 1))
    done
done
echo "photos de démonstration : $(find "$PHOTOS" -name '*.jpg' | wc -l)"

# --- Environnement isolé : configuration, cache, icônes ---------------------------------
mkdir -p "$WORK/config" "$WORK/cache" "$WORK/data/icons/hicolor/scalable/apps" \
    "$WORK/data/icons/hicolor/symbolic/apps"
# Barre de titre comme sous GNOME par défaut (bouton de fermeture seul).
mkdir -p "$WORK/config/gtk-4.0"
printf '[Settings]\ngtk-decoration-layout=:close\n' >"$WORK/config/gtk-4.0/settings.ini"
cp "$ROOT/packaging/icons/cloud.wizops.Lightning.svg" "$WORK/data/icons/hicolor/scalable/apps/"
cp "$ROOT/packaging/icons/cloud.wizops.Lightning-symbolic.svg" "$WORK/data/icons/hicolor/symbolic/apps/"
cat >"$WORK/daemon.toml" <<EOF
[dashboard]
fps = 2.0

[daemon]
mode = "dashboard"
image_path = "$PHOTOS/Montagne/photo-2.jpg"
image_fit = "cover"

[slideshow]
folders = ["$PHOTOS/Vacances 2026", "$PHOTOS/Famille", "$PHOTOS/Montagne"]
EOF
: >"$WORK/empty.toml"
# Bus de session minimal, sans activation automatique : ni portails, ni gvfs, rien d'autre
# que ce que le script lance lui-même.
cat >"$WORK/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$WORK</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

# Langue imposée : français (docs/…) puis anglais (docs/en/…, docs/screenshots/en/…),
# quelle que soit celle de la session.
in_lang() { # langue (fr|en), commande…
    local locale=fr_FR.UTF-8
    [[ $1 == en ]] && locale=en_US.UTF-8
    shift
    env -u LANGUAGE -u LC_ALL -u LC_MESSAGES LANG="$locale" "$@"
}

for lang in fr en; do
    DOCS="$ROOT/docs"
    [[ $lang == en ]] && DOCS="$ROOT/docs/en"
    mkdir -p "$DOCS"
# --- Tableau de bord ----------------------------------------------------------------------
in_lang $lang env XDG_CONFIG_HOME="$WORK/config" XDG_CACHE_HOME="$WORK/cache" \
    "$LIGHTNING" --config "$WORK/empty.toml" --no-lcd --demo dashboard --png "$DOCS/dashboard.png"

# --- Horloge et lecture en cours (images de l'écran, démon de démonstration privé) -------
in_lang $lang env XDG_CONFIG_HOME="$WORK/config" XDG_CACHE_HOME="$WORK/cache" \
    dbus-run-session --config-file="$WORK/bus.conf" -- bash -c '
        set -euo pipefail
        cp "$2" "$4/screens.toml"
        "$1" --config "$4/screens.toml" --no-lcd --demo daemon &
        daemon=$!
        trap "kill $daemon 2>/dev/null || true; wait 2>/dev/null || true" EXIT
        gdbus wait --session --timeout 15 cloud.wizops.Lightning.Daemon
        "$1" ctl clock >/dev/null
        sleep 1.5
        "$1" ctl frame "$4/clock.jpg" >/dev/null
        "$1" ctl now-playing >/dev/null
        sleep 1.5
        "$1" ctl frame "$4/now-playing.jpg" >/dev/null
    ' _ "$LIGHTNING" "$WORK/empty.toml" "" "$WORK"
magick "$WORK/clock.jpg" "$DOCS/clock.png"
magick "$WORK/now-playing.jpg" "$DOCS/now-playing.png"
done
mkdir -p "$OUT"

# --- Compositeur sans écran ---------------------------------------------------------------
dbus-run-session --config-file="$WORK/bus.conf" -- mutter --headless --wayland --no-x11 --virtual-monitor 1700x1200 \
    --wayland-display "$DISPLAY_NAME" >"$WORK/mutter.log" 2>&1 &
MUTTER_PID=$!
for _ in $(seq 50); do
    [[ -S "$XDG_RUNTIME_DIR/$DISPLAY_NAME" ]] && break
    sleep 0.2
done
[[ -S "$XDG_RUNTIME_DIR/$DISPLAY_NAME" ]] || { cat "$WORK/mutter.log" >&2; exit 1; }

# --- Une série de captures : démon privé + faux systemd + application ---------------------
shoot() { # langue, dossier, étiquette, thème, [taille]
    local lang=$1 out=$2 tag=$3 scheme=$4 size=${5:-}
    echo "== captures « $tag » ($lang, $scheme${size:+, $size})"
    mkdir -p "$out"
    cp "$WORK/daemon.toml" "$WORK/daemon-$tag.toml"
    in_lang "$lang" env -u DISPLAY \
        LIGHTNING_SCREENSHOT_BUS=1 \
        XDG_CONFIG_HOME="$WORK/config" XDG_CACHE_HOME="$WORK/cache" XDG_DATA_HOME="$WORK/data" \
        GSETTINGS_BACKEND=memory GTK_A11Y=none NO_AT_BRIDGE=1 GDK_DEBUG=no-portals \
        WAYLAND_DISPLAY="$DISPLAY_NAME" GDK_BACKEND=wayland \
        dbus-run-session --config-file="$WORK/bus.conf" -- bash -c '
            set -euo pipefail
            "$1" --config "$2" --no-lcd --demo daemon &
            daemon=$!
            python3 "$3" &
            systemd=$!
            trap "kill $daemon $systemd 2>/dev/null || true; wait 2>/dev/null || true" EXIT
            gdbus wait --session --timeout 15 cloud.wizops.Lightning.Daemon
            gdbus wait --session --timeout 15 org.freedesktop.systemd1
            # Laisse le diaporama et les mini-historiques se remplir.
            sleep 3
            args=(--screenshot "$5" --tag "$6" --color-scheme "$7")
            [[ -n $8 ]] && args+=(--size "$8")
            "$4" "${args[@]}"
        ' _ "$LIGHTNING" "$WORK/daemon-$tag.toml" "$ROOT/scripts/fake-systemd.py" "$SETTINGS" \
        "$out" "$tag" "$scheme" "$size"
}

shoot fr "$OUT" sombre dark
shoot fr "$OUT" clair light
shoot fr "$OUT" etroit dark 420x780
shoot en "$OUT/en" dark dark
shoot en "$OUT/en" light light
shoot en "$OUT/en" narrow dark 420x780

echo "captures écrites dans $OUT, $OUT/en, $ROOT/docs et $ROOT/docs/en"
