## Ce qui change

<!-- Résumé, et l'issue concernée : « Closes #N ». -->

Closes #

## Vérifications

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` et
      `cargo test --workspace` passent
- [ ] Testé sans l'écran (`--no-lcd`, `--demo`, `--output window`/`http`) quand c'est possible
- [ ] Testé sur le matériel : <!-- modèle de carte, PID, version du firmware — ou « non » -->
- [ ] Captures régénérées (`scripts/screenshots.sh`) si l'interface change
- [ ] README / `docs/PROTOCOL.md` mis à jour si besoin

## Sécurité du matériel

- [ ] Aucune écriture dans la flash de l'écran (`transport`/`transported`), aucune commande
      firmware non documentée dans `docs/PROTOCOL.md`
- [ ] Aucune donnée personnelle (nom de machine, chemins `/home/…`, numéros de série, photos)

## Non vérifié

<!-- Ce qui n'a pas pu être testé, et pourquoi. -->
