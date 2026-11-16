# Contribuer à lightning-lcd

Merci de votre intérêt ! Ce document explique comment construire, tester et proposer une
modification. Les échanges (issues, PR, messages de commit) se font de préférence en français ;
l'anglais est accepté.

## Avant tout : la sécurité du matériel

L'écran équipe une carte graphique coûteuse et son firmware n'est pas documenté. Quelques règles
ne se discutent pas :

- **Jamais d'écriture dans la flash de l'écran.** Les commandes `transport` / `transported`
  (upload de médias) ne sont pas implémentées et ne le seront pas ; une PR qui les ajoute sera
  refusée.
- **Pas de commande firmware inconnue.** N'envoyer que les requêtes décrites dans
  [docs/PROTOCOL.md](docs/PROTOCOL.md). Une nouvelle requête doit y être documentée (ce qu'elle
  fait, comment elle a été observée) dans la même PR.
- **Pas de requête vendor USB en mode étendu** : elle fait réénumérer l'écran (voir PROTOCOL.md).
- **Développer sans l'écran** autant que possible : `--no-lcd`, `--output window` ou `http`, et
  `--demo` pour des métriques factices. Un seul processus pilote l'écran à la fois (verrou dans
  `$XDG_RUNTIME_DIR`) : arrêter le service avant d'essayer une commande directe.
- **Aucun code, logo ni fichier du constructeur** dans le dépôt (pilote décompilé, images de
  l'application Windows…). Le projet est indépendant ; seuls les noms de produits sont cités pour
  désigner le matériel compatible.
- **Aucune donnée personnelle** dans les captures, journaux ou exemples (nom de machine, chemins
  `/home/<utilisateur>`, numéros de série, photos).

## Construire

Prérequis (Fedora) :

```sh
sudo dnf install -y libusb1-devel gtk4-devel libadwaita-devel dbus-daemon
```

Debian / Ubuntu : `libusb-1.0-0-dev libgtk-4-dev libadwaita-1-dev libxkbcommon-dev
libwayland-dev libx11-dev libx11-xcb-dev dbus-daemon`. Rust stable (édition 2024) via `rustup`.

```sh
cargo build --release                 # lightning + lightning-settings
cargo build --release -p lightning    # sans GTK
```

Essayer sans toucher à l'écran :

```sh
cargo run -p lightning -- --no-lcd --demo --output window dashboard
cargo run -p lightning -- --no-lcd --demo dashboard --png /tmp/apercu.png
```

Pour essayer l'application de réglage sans perturber un service déjà installé, lancer démon et
application sur un bus de session privé :

```sh
dbus-run-session -- bash -c '
  target/release/lightning --config /tmp/essai.toml --no-lcd --demo daemon &
  sleep 1; target/release/lightning-settings'
```

## Tester

La CI exécute exactement ces trois commandes ; elles doivent passer avant toute PR :

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace        # les tests D-Bus lancent leur propre dbus-daemon
```

Les tests n'accèdent jamais au matériel. Si votre PR change l'interface, régénérez les captures
avec `scripts/screenshots.sh` (compositeur sans écran, démon de démonstration, bus privé).

## Proposer une modification

1. Ouvrez d'abord une issue pour un changement important, afin d'en discuter.
2. Une branche et une PR par sujet, courte de préférence. Remplissez le modèle de PR : ce qui a
   été vérifié, et sur quel matériel le cas échéant.
3. La PR référence l'issue (`Closes #N`) et passe la CI.

### Style des commits

- Messages en français, première ligne courte (≤ 72 caractères) sous la forme
  `Zone : ce qui change` — par exemple `Diaporama : fondu enchaîné réglable` ou
  `Protocole : relecture du rapport HID conn`.
- Corps facultatif, séparé par une ligne vide : le pourquoi plutôt que le comment.
- Un commit = un changement cohérent qui compile et passe les tests.

### Style du code

- `rustfmt` et `clippy` font foi. Commentaires et messages d'erreur en français.
- Pas de `unwrap()` sur des données venant du matériel, du réseau ou de l'utilisateur.
- Toute logique testable sans matériel (analyse de trames, configuration, ordre des panneaux…)
  a ses tests unitaires.

## Licence

Le projet est sous **GPL-3.0-only**. En proposant une contribution, vous acceptez qu'elle soit
distribuée sous cette licence et vous certifiez en avoir le droit (code écrit par vous, ou sous
licence compatible et cité).
