#!/usr/bin/env bash
# Installe lightning-lcd depuis l'archive de release, pour l'utilisateur courant.
#
#   ./install.sh [--prefix DOSSIER] [--no-udev] [--no-service] [--uninstall]
#
# - binaires dans PREFIX/bin (défaut : ~/.local/bin) ;
# - application « Lightning » (.desktop, icônes) dans ~/.local/share ;
# - service systemd utilisateur lightning-dashboard.service, activé et démarré
#   (sauf --no-service) ;
# - règle udev de l'écran dans /etc/udev/rules.d (demande sudo ; sauf --no-udev).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PREFIX="$HOME/.local"
UDEV=1
SERVICE=1
UNINSTALL=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) PREFIX=$2; shift ;;
        --no-udev) UDEV=0 ;;
        --no-service) SERVICE=0 ;;
        --uninstall) UNINSTALL=1 ;;
        -h | --help) sed -n '2,11p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 2 ;;
    esac
    shift
done

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT=lightning-dashboard.service
RULE=/etc/udev/rules.d/70-lightning-lcd.rules
# Nom de la règle jusqu'à la version 0.1, remplacée à l'installation.
LEGACY_RULE=/etc/udev/rules.d/70-msi-lightning-lcd.rules
APP=cloud.wizops.Lightning

if [[ $UNINSTALL == 1 ]]; then
    if [[ $SERVICE == 1 ]]; then
        systemctl --user disable --now "$UNIT" 2>/dev/null || true
    fi
    rm -f "$PREFIX/bin/lightning" "$PREFIX/bin/lightning-settings" "$UNIT_DIR/$UNIT" \
        "$DATA/applications/$APP.desktop" \
        "$DATA/icons/hicolor/scalable/apps/$APP.svg" \
        "$DATA/icons/hicolor/symbolic/apps/$APP-symbolic.svg"
    if [[ $SERVICE == 1 ]]; then systemctl --user daemon-reload; fi
    if [[ $UDEV == 1 && ( -f $RULE || -f $LEGACY_RULE ) ]]; then
        sudo rm -f "$RULE" "$LEGACY_RULE" && sudo udevadm control --reload-rules
    fi
    echo "lightning-lcd désinstallé (la configuration ~/.config/lightning est conservée)."
    exit 0
fi

install -Dm755 "$HERE/bin/lightning" "$PREFIX/bin/lightning"
install -Dm755 "$HERE/bin/lightning-settings" "$PREFIX/bin/lightning-settings"

# L'unité lance le binaire installé ci-dessus.
install -d "$UNIT_DIR"
sed "s|^ExecStart=.*|ExecStart=$PREFIX/bin/lightning daemon|" \
    "$HERE/packaging/$UNIT" >"$UNIT_DIR/$UNIT"
chmod 644 "$UNIT_DIR/$UNIT"

install -d "$DATA/applications"
sed "s|^Exec=.*|Exec=$PREFIX/bin/lightning-settings|" \
    "$HERE/packaging/$APP.desktop" >"$DATA/applications/$APP.desktop"
install -Dm644 "$HERE/packaging/icons/$APP.svg" "$DATA/icons/hicolor/scalable/apps/$APP.svg"
install -Dm644 "$HERE/packaging/icons/$APP-symbolic.svg" \
    "$DATA/icons/hicolor/symbolic/apps/$APP-symbolic.svg"
command -v gtk4-update-icon-cache >/dev/null \
    && gtk4-update-icon-cache -q -t -f "$DATA/icons/hicolor" || true
command -v update-desktop-database >/dev/null \
    && update-desktop-database "$DATA/applications" || true

if [[ $UDEV == 1 ]]; then
    echo "Règle udev (accès à l'écran sans root) : sudo est demandé."
    sudo install -m644 "$HERE/packaging/70-lightning-lcd.rules" "$RULE"
    sudo rm -f "$LEGACY_RULE"
    sudo udevadm control --reload-rules
    sudo udevadm trigger --attr-match=idVendor=0db0 --attr-match=idProduct=7881
fi

if [[ $SERVICE == 1 ]]; then
    systemctl --user daemon-reload
    systemctl --user enable --now "$UNIT"
fi

echo "lightning-lcd installé dans $PREFIX/bin."
[[ :$PATH: == *":$PREFIX/bin:"* ]] || echo "Ajoutez $PREFIX/bin à votre PATH."
