# Journal des modifications

*[English](CHANGELOG.md) · Français*

Tous les changements notables du projet sont consignés ici. Le format suit
[Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le projet applique le
[versionnage sémantique](https://semver.org/lang/fr/). Ce fichier est la traduction de
[CHANGELOG.md](CHANGELOG.md), dont chaque release GitHub tire ses notes.

## [Unreleased]

## [0.1.0] - 2026-11-16

Première version publique. Vérifiée sur le matériel avec l'écran de la MSI RTX 5090 Lightning Z
(`0db0:7881`) sous Fedora avec GNOME sur Wayland. Non vérifié : les autres identifiants USB
déclarés, les constructions RPM et Flatpak en dehors des tests du projet, et les autres
distributions.

### Ajouté

- **Pilote** de l'écran LCD USB `0db0:7881` (1280×800) : canal de contrôle HID, authentification
  RSA, images JPEG, reconnexion automatique, verrou à processus unique. L'écriture dans la mémoire
  flash n'est délibérément pas implémentée.
- **Démon** (`lightning daemon`) piloté par D-Bus (`cloud.wizops.Lightning1`), avec des réglages
  appliqués à chaud et enregistrés dans `~/.config/lightning/config.toml`, et une unité systemd
  utilisateur.
- **Contenus** : tableau de bord de l'état du PC (GPU, CPU, mémoire, disques, réseau), image,
  vidéo (ffmpeg), diaporama de photos, horloge plein écran, lecture en cours (MPRIS), couleur
  unie, et retour à l'animation d'origine de l'écran.
- **Diaporama** : dossiers locaux, ordre aléatoire ou chronologique, orientation EXIF, fond
  flouté, fondu enchaîné, légende et horloge ; prise en charge facultative du HEIC
  (`--features heic`) ; import Google Photos par l'API Picker, jeton conservé dans le trousseau.
- **Application de réglage** `lightning-settings` (GTK 4 + libadwaita) : aperçu en direct, cartes
  de contenu, choix et ordre des panneaux (boutons et glisser-déposer), thèmes sombre et clair,
  mise en page adaptative.
- **Ligne de commande** : commandes directes (`status`, `color`, `image`, `video`, `stream`,
  `dashboard`, `brightness`, `restore`) et `lightning ctl` pour piloter le démon.
- **Sorties d'aperçu** : fenêtre de bureau et serveur HTTP local (flux MJPEG, dernière image,
  état JSON).
- **Rotation logicielle** : 0°, 90°, 180° et 270°, avec une mise en page portrait.
- **Autres écrans** : les 40 identifiants USB déclarés par le pilote du constructeur (`0db0:7880`
  à `0db0:7919`) ne sont acceptés qu'avec `--any-supported-pid` ; seul `0db0:7881` a été testé.
- Interface et messages en **anglais et en français**, selon la langue de la session.
- **Paquets** : archive de release avec `install.sh`, spec RPM pour Fedora, manifeste Flatpak de
  l'application de réglage.

[Unreleased]: https://github.com/wizops-cloud/lightning-lcd/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wizops-cloud/lightning-lcd/releases/tag/v0.1.0
