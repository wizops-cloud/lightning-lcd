<!-- English or French, as you prefer. · En français ou en anglais, au choix. -->

## What changes · Ce qui change

<!-- Summary, and the related issue: "Closes #N". · Résumé, et l'issue concernée. -->

Closes #

## Checks · Vérifications

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
      `cargo test --workspace` pass · passent
- [ ] Tested without the screen (`--no-lcd`, `--demo`, `--output window`/`http`) where possible ·
      Testé sans l'écran quand c'est possible
- [ ] Tested on hardware · Testé sur le matériel : <!-- card model, PID, firmware version, or "no" -->
- [ ] Screenshots regenerated (`scripts/screenshots.sh`) if the interface changes · Captures
      régénérées si l'interface change
- [ ] Documentation updated in both languages (`X.md` and `X.fr.md`), changelog included, if
      needed · Documentation mise à jour dans les deux langues, changelog compris, si besoin

## Hardware safety · Sécurité du matériel

- [ ] No write to the flash memory of the screen (`transport`/`transported`), no firmware command
      that is not documented in `docs/PROTOCOL.md` · Aucune écriture dans la flash de l'écran,
      aucune commande firmware non documentée
- [ ] No personal data (host name, `/home/…` paths, serial numbers, photos) · Aucune donnée
      personnelle

## Not verified · Non vérifié

<!-- What could not be tested, and why. · Ce qui n'a pas pu être testé, et pourquoi. -->
