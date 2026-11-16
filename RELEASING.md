# Publier une version

Ce document décrit le processus complet, de la branche `main` à la release publiée. Tout est
outillé par `scripts/release.sh` ; rien n'est publié sans la dernière commande.

## Vue d'ensemble

| Étape | Qui | Quoi |
| --- | --- | --- |
| CI (`.github/workflows/ci.yml`) | chaque push sur `main`, chaque PR | cohérence de la version, scripts shell, `fmt`, `clippy -D warnings`, tests (y compris `heic`) |
| Préparation | mainteneur | version, `CHANGELOG.md`, metainfo, spec RPM : un commit sur `main` |
| Étiquette | `scripts/release.sh tag` | vérifications puis étiquette annotée `vX.Y.Z` poussée |
| CD (`.github/workflows/release.yml`) | déclenchée par l'étiquette | revérifie, teste, construit, atteste, crée la release **en brouillon** |
| Contrôle | `scripts/release.sh verify` | télécharge le brouillon : sommes, attestations, contenu, version des binaires |
| Publication | `scripts/release.sh publish` | refait le contrôle puis publie |

## Numérotation

[Semantic Versioning](https://semver.org/lang/fr/). Tant que la version est `0.x` : `0.x+1.0`
pour une nouveauté ou un changement incompatible (configuration, interface D-Bus, ligne de
commande), `0.x.y+1` pour des corrections. La version est écrite à quatre endroits, que
`scripts/check-version.sh` compare (en CI à chaque push) :

- `Cargo.toml` (`[workspace.package]`), d'où `Cargo.lock` ;
- `packaging/rpm/lightning-lcd.spec` (`Version:` et `%changelog`) ;
- `packaging/cloud.wizops.Lightning.metainfo.xml` (première entrée `<release>`, avec la date) ;
- `CHANGELOG.md` (section `## [X.Y.Z] - AAAA-MM-JJ` et lien en bas de fichier).

## 1. Préparer

1. Dans `CHANGELOG.md`, renommer le contenu de `[Unreleased]` en `## [X.Y.Z] - AAAA-MM-JJ`
   (laisser une section `[Unreleased]` vide au-dessus) et mettre à jour les deux liens du bas.
   Cette section devient telle quelle les notes de la release : l'écrire pour l'utilisateur, en
   anglais, en disant ce qui a été vérifié sur le matériel et ce qui ne l'a pas été.
2. Mettre la version dans `Cargo.toml`, puis `cargo check --workspace` pour régénérer `Cargo.lock`.
3. Mettre à jour la spec RPM (`Version:`, `Release: 1`, entrée `%changelog`) et ajouter l'entrée
   `<release>` en tête de la metainfo, avec la même date que le CHANGELOG.
4. Si `Cargo.lock` a changé depuis la dernière version : régénérer
   `packaging/flatpak/cargo-sources.json` et relire le champ `License:` de la spec (voir le README).
5. Vérifier puis committer et pousser sur `main` :

   ```sh
   scripts/check-version.sh vX.Y.Z
   cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
   git commit -am "Version X.Y.Z" && git push origin main
   ```

6. Pour une version qui touche au pilote ou au protocole : essai sur l'écran réel avant
   l'étiquette (connexion, un contenu de chaque famille, arrêt propre). La CI ne peut pas le faire.

## 2. Étiqueter

Attendre que la CI du commit soit verte, puis :

```sh
scripts/release.sh check    # facultatif : les vérifications seules
scripts/release.sh tag
```

`tag` refuse de continuer si : la version n'est pas cohérente, l'arbre n'est pas propre, on n'est
pas sur `main` à jour avec `origin/main`, l'étiquette existe déjà, la CI du commit n'est pas
verte, ou l'adresse `git config user.email` n'est pas du domaine du projet (elle figure dans
l'étiquette publique ; `RELEASE_EMAIL_DOMAIN=` pour un fork).

## 3. Contrôler le brouillon

L'étiquette déclenche la CD (une dizaine de minutes). Elle échoue si l'étiquette ne correspond
pas à la version, n'est pas sur `main`, si un test échoue ou si un binaire n'annonce pas la bonne
version. Elle produit :

- `lightning-lcd-X.Y.Z-x86_64-linux.tar.gz` (binaires, `install.sh`, fichiers d'intégration) ;
- `lightning`, `lightning-settings` (binaires seuls) et `SHA256SUMS` ;
- une attestation de provenance par fichier (Sigstore, via GitHub) ;
- une release en brouillon, dont les notes sont la section du CHANGELOG.

```sh
scripts/release.sh verify
```

Relire aussi les notes sur la page du brouillon, et essayer l'archive sur une machine si le
packaging a changé (`./install.sh --prefix /tmp/essai --no-udev --no-service`).

## 4. Publier

```sh
scripts/release.sh publish
```

Ensuite : vérifier la page publique, fermer le jalon éventuel, annoncer.

## Si quelque chose ne va pas

- **La CD échoue ou le brouillon est mauvais, avant publication** : supprimer le brouillon et
  l'étiquette (`gh release delete vX.Y.Z --cleanup-tag --yes`, `git tag -d vX.Y.Z`), corriger sur
  `main`, recommencer à l'étape 2. Rien n'a été rendu public hormis l'étiquette.
- **Après publication** : on ne remplace jamais une version publiée et on ne déplace jamais son
  étiquette. Corriger et publier `X.Y.Z+1` ; si la version est dangereuse, la marquer comme telle
  dans ses notes et la repasser en « pre-release ».

## Préversions

Une étiquette `vX.Y.Z-rc.N` (posée à la main, la version du `Cargo.toml` étant déjà `X.Y.Z`)
produit un brouillon marqué « pre-release », avec les mêmes contrôles. Le lancement manuel du
workflow *Release* (onglet Actions) construit l'archive sans rien publier : utile pour essayer un
changement du packaging.
