# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). Each release on GitHub takes its notes from the
matching section of this file.

## [Unreleased]

## [0.1.0] - 2026-11-16

First public release. Verified on real hardware with the MSI RTX 5090 Lightning Z screen
(`0db0:7881`) on Fedora with GNOME on Wayland. Not verified: the other declared USB IDs, the
RPM and Flatpak builds outside the project's own tests, and other distributions.

### Added

- **Driver** for the USB LCD screen `0db0:7881` (1280×800): HID control channel, RSA
  authentication, JPEG frames, automatic reconnection, single-process lock. Flash writes are
  deliberately not implemented.
- **Daemon** (`lightning daemon`) controlled over D-Bus (`cloud.wizops.Lightning1`), with
  settings applied live and saved to `~/.config/lightning/config.toml`, and a systemd user unit.
- **Contents**: live PC dashboard (GPU, CPU, memory, disks, network), image, video (ffmpeg),
  photo slideshow, full-screen clock, now playing (MPRIS), solid colour, and handing the screen
  back to its original animation.
- **Slideshow**: local folders, random or chronological order, EXIF orientation, blurred
  background, cross-fade, caption and clock; optional HEIC support (`--features heic`); Google
  Photos import through the Picker API, with the token kept in the keyring.
- **Settings application** `lightning-settings` (GTK 4 + libadwaita): live preview, content cards,
  panel selection and ordering (buttons and drag and drop), dark and light themes, adaptive layout.
- **Command line**: direct commands (`status`, `color`, `image`, `video`, `stream`, `dashboard`,
  `brightness`, `restore`) and `lightning ctl` to drive the daemon.
- **Preview outputs**: desktop window and local HTTP server (MJPEG stream, last frame, JSON status).
- **Software rotation**: 0°, 90°, 180° and 270°, with a portrait layout.
- **Other screens**: the 40 USB IDs declared by the vendor driver (`0db0:7880` to `0db0:7919`)
  are accepted only with `--any-supported-pid`; only `0db0:7881` has been tested.
- **English and French** interface and messages, following the session language.
- **Packaging**: release archive with `install.sh`, Fedora RPM spec, Flatpak manifest for the
  settings application.

[Unreleased]: https://github.com/wizops-cloud/lightning-lcd/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wizops-cloud/lightning-lcd/releases/tag/v0.1.0
