# Contributing to lightning-lcd

*English · [Français](CONTRIBUTING.fr.md)*

Thank you for your interest! This document explains how to build, test and submit a
change. Issues and PRs can be written in English or in French, as you prefer. Existing code
comments and commit messages are in French; both languages are accepted. Any user documentation
you change must be changed in both languages (`X.md` in English and `X.fr.md` in French), and
changelog entries go into both `CHANGELOG.md` and `CHANGELOG.fr.md`.

## First of all: hardware safety

The screen is fitted to an expensive graphics card and its firmware is undocumented. A few rules
are not negotiable:

- **Never write to the screen's flash.** The `transport` / `transported` commands
  (media upload) are not implemented and will not be; a PR that adds them will be
  rejected.
- **No unknown firmware command.** Only send the requests described in
  [docs/PROTOCOL.md](docs/PROTOCOL.md). A new request must be documented there (what it
  does, how it was observed) in the same PR.
- **No USB vendor request in extended mode**: it makes the screen re-enumerate (see PROTOCOL.md).
- **Develop without the screen** as much as possible: `--no-lcd`, `--output window` or `http`, and
  `--demo` for fake metrics. Only one process drives the screen at a time (lock in
  `$XDG_RUNTIME_DIR`): stop the service before trying a direct command.
- **No vendor code, logo or file** in the repository (decompiled driver, images from
  the Windows application, etc.). The project is independent; product names are only mentioned to
  identify the compatible hardware.
- **No personal data** in captures, logs or examples (host name, `/home/<user>`
  paths, serial numbers, photos).

## Building

Prerequisites (Fedora):

```sh
sudo dnf install -y libusb1-devel gtk4-devel libadwaita-devel dbus-daemon
```

Debian / Ubuntu: `libusb-1.0-0-dev libgtk-4-dev libadwaita-1-dev libxkbcommon-dev
libwayland-dev libx11-dev libx11-xcb-dev dbus-daemon`. Stable Rust (2024 edition) via `rustup`.

```sh
cargo build --release                 # lightning + lightning-settings
cargo build --release -p lightning    # without GTK
```

Trying it without touching the screen:

```sh
cargo run -p lightning -- --no-lcd --demo --output window dashboard
cargo run -p lightning -- --no-lcd --demo dashboard --png /tmp/apercu.png
```

To try the settings application without disturbing an already installed service, run the daemon
and the application on a private session bus:

```sh
dbus-run-session -- bash -c '
  target/release/lightning --config /tmp/essai.toml --no-lcd --demo daemon &
  sleep 1; target/release/lightning-settings'
```

## Testing

CI runs exactly these three commands; they must pass before any PR:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace        # the D-Bus tests start their own dbus-daemon
```

The tests never access the hardware. If your PR changes the interface, regenerate the screenshots
with `scripts/screenshots.sh` (headless compositor, demo daemon, private bus).

## Submitting a change

1. For a significant change, open an issue first so it can be discussed.
2. One branch and one PR per topic, preferably short. Fill in the PR template: what was
   verified, and on which hardware if applicable.
3. The PR references the issue (`Closes #N`) and passes CI.
4. A user-visible change adds a line to the `[Unreleased]` section of
   [CHANGELOG.md](CHANGELOG.md) (in English) and of [CHANGELOG.fr.md](CHANGELOG.fr.md) (in
   French). The release process is described in [RELEASING.md](RELEASING.md).

### Commit style

- Existing messages are in French; English is accepted too. Short first line (≤ 72 characters)
  in the form `Area: what changes` (`Zone : ce qui change` in French), for example
  `Diaporama : fondu enchaîné réglable` or `Protocole : relecture du rapport HID conn`.
- Optional body, separated by a blank line: the why rather than the how.
- One commit = one coherent change that compiles and passes the tests.

### Code style

- `rustfmt` and `clippy` are authoritative. Existing comments are in French; English is accepted
  too. User-visible messages are written in French in
  the code and translated to English in `crates/lightning-i18n/po/en-*.po`.
- No `unwrap()` on data coming from the hardware, the network or the user.
- Any logic that can be tested without hardware (frame parsing, configuration, panel order, etc.)
  has its unit tests.

## License

The project is licensed under **GPL-3.0-only**. By submitting a contribution, you agree that it
is distributed under this license and you certify that you have the right to do so (code written
by you, or under a compatible license and credited).
