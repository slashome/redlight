# Redlight

> `rl` — sync your devices over USB, no cloud, no app, no friction.

Redlight is a cross-platform program (macOS + Linux in v1, Windows in v2) that automatically synchronises files and folders across multiple peripherals (computer, phone, external drive) whenever they're plugged in. No app to install on passive devices, no cloud, everything goes over USB.

---

## Philosophy

- **Multi-peripheral.** Computer, phone, external drive — all are devices at the same level. Each carries its own local state.
- **The ring.** Target model: no privileged hub, any two co-present devices reconcile pairwise and the most recent wins. *(In v0.0.1 `rl sync` only orchestrates host ↔ drive pairs; two drives sharing an item via the host are still kept in agreement by transitivity — see "What Redlight doesn't do" below.)*
- **No cloud.** Everything flows over USB, nothing transits through a third-party server.
- **No imposed folder.** Each rule points at the actual location of files, wherever they live.
- **Passive devices do nothing.** Phone and drive are passive, they only carry their manifest. Only the host executes.
- **Predictable.** A folder syncs all its contents (filterable via `include`/`exclude`). A file syncs that one file only.
- **Transparent.** A TOML manifest per device records the known state of every tracked file.

---

## Model

Three entities:

- **Device** — a peripheral. Type *active* (carries the daemon) or *passive* (carries only its manifest).
- **Item** — a synced unit (folder or file), with `include`/`exclude` filters and an optional `category`.
- **Binding** — a (item, device) pair. Defines the `path` on that device and the `role` (`read_write` or `read_only`).

A sync exists between two devices for an item iff both have a binding to it.

| Combo | Behavior |
|---|---|
| Active + Passive | standard case, the active drives |
| Active + Active | planned by the model, out of scope in v0.0.1 |
| Passive + Passive | ignored (no daemon) |

For the detailed TOML schema, filters, automatic exclusions, and config examples (full USB key sync, etc.), see **[docs/configuration.md](docs/configuration.md)**.

---

## How it works

Every time a known device is plugged in:

1. The daemon detects it (IOKit + DiskArbitration on macOS, udev on Linux)
2. It identifies the device by hardware ID or volume label
3. It loads the config (`devices.toml`, `items.toml`, `bindings.toml`)
4. For each item bound on this device and on another present device: diff manifest vs. live state
5. Pairwise reconciliation, transfers respecting the `role`s
6. Update local manifests and log
7. Notification via the menu bar

---

## Installation

> ⚠️ No distribution channel is active yet. Everything below is the target.

### macOS

```bash
brew install redlight                                      # coming soon
# or
curl -fsSL https://slashome.me/apps/redlight.sh | sh       # coming soon
```

### Linux

```bash
curl -fsSL https://slashome.me/apps/redlight.sh | sh       # coming soon
# .deb / .rpm / AUR : see the GitHub Releases page          # coming soon
```

The installer will download a pre-built binary and automatically install the required system dependencies (`libmtp`, `jmtpfs`, `macfuse` on Mac) via the OS package manager. If you enable a device with `bridge = "adb"`, it will also offer to install `android-platform-tools`.

### First-time setup

```bash
rl init                                                          # creates the config, registers the system service
rl start                                                         # starts the daemon (auto at login afterwards)
rl device add jarvis --type phone --bridge mtp                   # declare a device
rl item add music --kind folder --include "**/*.mp3"             # declare an item
rl bind add --item music --device tardis --path ~/Music --role read_write
rl status
```

---

## `rl` commands

| command | description |
|---------|-------------|
| `rl init` | initialise the config and register the agent (launchd / systemd user) |
| `rl start` / `rl stop` / `rl restart` | daemon control |
| `rl status` | daemon state, connected devices, last sync per binding |
| `rl doctor` | verify system prerequisites (jmtpfs, adb…) are available |
| `rl device add\|remove\|list` | manage devices |
| `rl item add\|remove\|list` | manage items |
| `rl bind add\|remove\|list` | manage bindings |
| `rl sync [--dry-run] [--drive-mount NAME=PATH]` | force an immediate sync |
| `rl log [--errors] [--device N] [--item N] [--tail N]` | show logs |
| `rl manifest [--device N] [--item N]` | show the manifest |

---

## Technical stack

| component | technology |
|-----------|------------|
| Language | Rust (2024 edition) |
| Daemon | `launchd` (macOS) / `systemd --user` (Linux) / Windows service (v2) |
| CLI | `clap` |
| Config & manifest | `serde` + `toml` |
| USB & volume detection | `io-kit-sys` + `core-foundation` (macOS) / `udev` (Linux) |
| MTP bridge | `jmtpfs` (FUSE) on top of `libmtp` |
| ADB bridge | `adb` (Android Platform Tools, opt-in) |
| FS bridge | stdlib + `walkdir` |
| Menu bar / tray | `tray-icon` (cross-platform) |
| Async runtime | `tokio` |
| Logs | `tracing` + `tracing-subscriber` |
| Glob filters | `globset` |
| Hashing | `md-5` |
| Packaging | `cargo-bundle` (macOS) / `cargo-deb` (Linux) / `cargo-wix` (Windows v2) |

---

## What Redlight doesn't do (v1)

- No Wi-Fi sync
- No cloud
- No app on passive devices
- No encryption (planned v2)
- No Windows support (planned v2)
- No fine-grained conflict resolution (most recent wins)
- No direct drive ↔ drive sync in `rl sync` — transitivity through the host covers the standard case
- No phone sync via `rl sync` — USB auto-detection in Phase 4; meanwhile: `cargo run --example mtp_probe` or `adb_probe`

---

## Contributing

```bash
# Rust toolchain
brew install rust                                                       # macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh          # Linux/macOS via rustup

# System dependencies (hard)
brew install libmtp jmtpfs macfuse                                      # macOS
sudo apt install libmtp-dev jmtpfs fuse libudev-dev pkg-config          # Debian/Ubuntu
sudo dnf install libmtp-devel jmtpfs fuse systemd-devel pkg-config      # Fedora

# Optional — only when working on the ADB bridge
brew install android-platform-tools                                     # macOS
sudo apt install android-tools-adb                                      # Debian/Ubuntu

# Build & tests
git clone https://github.com/slashome/redlight && cd redlight
cargo build
cargo test
cargo run -- --help
```

More in-depth docs: [docs/configuration.md](docs/configuration.md).

---

## License

MIT
