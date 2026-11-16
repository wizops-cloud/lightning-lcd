# Publishing a release

*English · [Français](RELEASING.fr.md)*

This document describes the whole process, from the `main` branch to the published release.
Everything is driven by `scripts/release.sh`; nothing is published before its last command.

## Overview

| Step | Who | What |
| --- | --- | --- |
| CI (`.github/workflows/ci.yml`) | every push to `main`, every PR | version consistency, shell scripts, `fmt`, `clippy -D warnings`, tests (including `heic`) |
| Preparation | maintainer | version, `CHANGELOG.md` and `CHANGELOG.fr.md`, metainfo, RPM spec: one commit on `main` |
| Tag | `scripts/release.sh tag` | checks, then pushes the annotated tag `vX.Y.Z` |
| CD (`.github/workflows/release.yml`) | triggered by the tag | checks again, tests, builds, attests, creates the release **as a draft** |
| Verification | `scripts/release.sh verify` | downloads the draft: checksums, attestations, contents, version of the binaries |
| Publication | `scripts/release.sh publish` | verifies again, then publishes |

## Version numbers

[Semantic Versioning](https://semver.org/). While the version is `0.x`: `0.x+1.0` for a new
feature or an incompatible change (configuration, D-Bus interface, command line), `0.x.y+1` for
fixes. The version is written in four places, which `scripts/check-version.sh` compares (in CI on
every push):

- `Cargo.toml` (`[workspace.package]`), hence `Cargo.lock`;
- `packaging/rpm/lightning-lcd.spec` (`Version:` and `%changelog`);
- `packaging/cloud.wizops.Lightning.metainfo.xml` (first `<release>` entry, with the date);
- `CHANGELOG.md` (section `## [X.Y.Z] - YYYY-MM-DD` and link at the bottom of the file), and its
  translation `CHANGELOG.fr.md`, maintained by hand.

## 1. Prepare

1. In `CHANGELOG.md`, rename the content of `[Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` (leave an
   empty `[Unreleased]` section above it) and update the two links at the bottom. This section
   becomes the release notes as is: write it for the user, in English, stating what was verified
   on the hardware and what was not. Copy the same section, translated, into `CHANGELOG.fr.md`.
2. Set the version in `Cargo.toml`, then run `cargo check --workspace` to regenerate `Cargo.lock`.
3. Update the RPM spec (`Version:`, `Release: 1`, `%changelog` entry) and add the `<release>`
   entry at the top of the metainfo, with the same date as the changelog.
4. If `Cargo.lock` changed since the last version: regenerate
   `packaging/flatpak/cargo-sources.json` and review the `License:` field of the spec (see the
   README).
5. Check, then commit and push to `main`:

   ```sh
   scripts/check-version.sh vX.Y.Z
   cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
   git commit -am "Version X.Y.Z" && git push origin main
   ```

6. For a version that touches the driver or the protocol: try it on the real screen before
   tagging (connection, one content of each family, clean stop). The CI cannot do that.

## 2. Tag

Wait until the CI of the commit is green, then:

```sh
scripts/release.sh check    # optional: the checks only
scripts/release.sh tag
```

`tag` refuses to go on if: the version is not consistent, the tree is not clean, you are not on
`main` up to date with `origin/main`, the tag already exists, the CI of the commit is not green,
or the `git config user.email` address is outside the project's domain (it appears in the public
tag; set `RELEASE_EMAIL_DOMAIN=` for a fork).

## 3. Verify the draft

The tag triggers the CD (about ten minutes). It fails if the tag does not match the version, is
not on `main`, if a test fails or if a binary does not report the right version. It produces:

- `lightning-lcd-X.Y.Z-x86_64-linux.tar.gz` (binaries, `install.sh`, integration files);
- `lightning`, `lightning-settings` (bare binaries) and `SHA256SUMS`;
- one build provenance attestation per file (Sigstore, through GitHub);
- a draft release whose notes are the changelog section.

```sh
scripts/release.sh verify
```

Also read the notes on the draft page, and try the archive on a machine if the packaging changed
(`./install.sh --prefix /tmp/trial --no-udev --no-service`).

## 4. Publish

```sh
scripts/release.sh publish
```

Then: check the public page, close the milestone if any, announce.

## If something goes wrong

- **The CD fails or the draft is wrong, before publication**: delete the draft and the tag
  (`gh release delete vX.Y.Z --cleanup-tag --yes`, `git tag -d vX.Y.Z`), fix on `main`, start
  again at step 2. Nothing was made public except the tag.
- **After publication**: a published version is never replaced and its tag is never moved. Fix
  and publish `X.Y.Z+1`; if the version is dangerous, say so in its notes and switch it back to
  "pre-release".

## Pre-releases

A `vX.Y.Z-rc.N` tag (created by hand, the `Cargo.toml` version already being `X.Y.Z`) produces a
draft marked "pre-release", with the same checks. Running the *Release* workflow manually
(Actions tab) builds the archive without publishing anything: useful to try a packaging change.
