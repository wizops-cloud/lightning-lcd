# Security policy

*English · [Français](SECURITY.fr.md)*

## Supported versions

Only the latest published version (and the `main` branch) receives security fixes.

## Reporting a vulnerability

**Do not disclose a vulnerability in a public issue.** Preferably use GitHub's private reporting
(*Security* tab → *Report a vulnerability*) or write to **contact@wizops.fr** with:

- the version (`lightning --version`, `lightning-settings --version`) and the distribution;
- a description of the problem and its impact;
- the steps to reproduce it, ideally without the hardware (`--no-lcd --demo`).

We acknowledge reports within 7 days and aim for a fix within 30 days depending on severity.
Reporters are credited in the release notes, unless you prefer to remain anonymous.

## Scope

In scope, in particular:

- the `lightning daemon` daemon and its D-Bus interface `cloud.wizops.Lightning1` (session bus:
  any process of the user can drive it, by design; an escalation beyond that user is not);
- the HTTP preview (`--output http`), read-only and **unauthenticated**: it listens on
  `127.0.0.1` by default; exposing it on the network is an explicit choice of the user;
- the udev rule (`TAG+="uaccess"` limited to `0db0:7881`) and the systemd user unit;
- the storage of the Google Photos token (`~/.config/lightning/google.json`, mode 0600) and the
  download of the selected photos;
- the decoding of images, videos (through `ffmpeg`) and configuration files.

Out of scope: defects in the firmware of the screen itself (to be reported to the vendor), and
attacks that already assume physical access or the user's account.
