# Redlight — Development Plan

## Architecture

### Entities

- **Device** — a peripheral participating in sync (computer, phone, drive). Has a *type* (active or passive) and a *bridge* (how to read/write its files).
- **Item** — a synced unit (folder or file). Carries `include`/`exclude` glob filters and an optional `category`.
- **Binding** — a (item × device) pair. Defines `path` on that device and `role` (`read_write` | `read_only`).

A sync exists between two devices for a given item iff both have a binding to that item.

### Active vs Passive

| Type | Examples | Carries |
|------|----------|---------|
| Active | Tardis (Mac/Linux) | daemon + global config + local manifest |
| Passive | Jarvis (Android), Materia (SSD) | local manifest only |

Rules:
- Active + Passive → standard case, the active drives the sync.
- Active + Active → supported, same model. Pairwise reconciliation. Direct USB rare in practice; usually relayed via a shared SSD courier.
- Passive + Passive → ignored in v1 (no daemon present to drive). Could log the encounter in a future version.

### Reconciliation model — "the ring"

Each device carries its own truth (its local manifest). Reconciliation happens **pairwise** when two devices are co-present:
1. Read both manifests + scan both file states.
2. For each shared item, compute diff per file (hash + mtime + size).
3. Resolve conflicts: most recent `mtime` wins (configurable in v2).
4. Apply transfers respecting `role` constraints.
5. Update both manifests.

The active device propagates state by participating in many pairings, but holds no privileged "hub" status in the model.

> **v0.0.1 implementation note** — `rl sync` iterates host ↔ drive pairs only; drive ↔ drive and phone participation are deferred (see [Deferred / known limitations](#deferred--known-limitations) and `src/sync/run.rs`). Transitivity through the host covers the standard "two drives both bound to an item via the host" case.

### Bridges

Pluggable per-device interface (Rust trait): `list_files`, `get_file`, `put_file`, `delete_file`, `make_dir`, `get_metadata`.

| Bridge | Used for |
|--------|----------|
| `fs` | host filesystem, external drives, FUSE mounts |
| `mtp` | Android default (libmtp) |
| `adb` | Android override |

Bridge per device is declared in `devices.toml`. Default for Android = `mtp`.

### Files layout

On **Tardis** (active host, macOS / Linux):
```
~/.config/redlight/
  ├── devices.toml          # known devices
  ├── items.toml            # item definitions
  ├── bindings.toml         # item × device pairs
  └── manifest.toml         # Tardis's own local manifest
~/.local/state/redlight/
  ├── sync_log.toml
  └── daemon.log
~/Library/LaunchAgents/com.redlight.daemon.plist          # macOS
~/.config/systemd/user/redlight.service                   # Linux
```

On **Jarvis** (Android, passive):
```
/storage/emulated/0/.redlight/
  ├── manifest.toml
  └── sync_log.toml
```

On **Materia** (SSD, passive):
```
/Volumes/Materia/.redlight/
  ├── manifest.toml
  └── sync_log.toml
```

---

## Config schema (TOML)

### `devices.toml`
```toml
[tardis]
type = "host"
bridge = "fs"

[jarvis]
type = "phone"
bridge = "mtp"            # override: "adb"
match.vendor_id = "18d1"
match.product_id = "4ee7"
match.serial = "ABC123"

[materia]
type = "drive"
bridge = "fs"
match.volume_label = "MATERIA"
```

### `items.toml`
```toml
[music]
kind = "folder"
category = "audio"
include = ["**/*.mp3", "**/*.flac"]   # optional allowlist
exclude = ["**/draft-*", "**/*.tmp"]  # optional denylist

[contrat-freelance]
kind = "file"
category = "admin"
```

If `items.toml` exceeds ~100 entries, split into `items/<category>.toml` files.

### `bindings.toml`
```toml
[[binding]]
item = "music"
device = "tardis"
path = "~/Music"
role = "read_write"

[[binding]]
item = "music"
device = "jarvis"
path = "/storage/emulated/0/Music"
role = "read_only"

[[binding]]
item = "music"
device = "materia"
# path omitted → device root
role = "read_write"
```

### `manifest.toml` (per device)
```toml
version = 1
device = "jarvis"

[items.music.files."Beatles/Yesterday.mp3"]
hash = "5d41402abc4b2a76b9719d911017c592"
mtime = 1715000000
size = 4231823
last_sync = 1715000100
```

---

## Phase 0 — Scaffolding

- [ ] Project structure
  ```
  redlight/
    ├── src/
    │   ├── main.rs              # binary entry (calls cli)
    │   ├── lib.rs               # library root, re-exports
    │   ├── cli/
    │   │   └── mod.rs           # clap definitions, dispatch
    │   ├── config.rs            # devices/items/bindings TOML loading
    │   ├── manifest.rs          # per-device manifest TOML
    │   ├── sync/
    │   │   └── mod.rs           # sync engine (diff, reconcile, transfer)
    │   ├── bridges/
    │   │   ├── mod.rs           # Bridge trait
    │   │   ├── fs.rs
    │   │   ├── mtp.rs
    │   │   └── adb.rs
    │   ├── watcher/
    │   │   ├── mod.rs           # Watcher trait + event types
    │   │   ├── macos.rs         # IOKit + DiskArbitration
    │   │   └── linux.rs         # udev
    │   ├── daemon.rs            # daemon main loop
    │   ├── ipc.rs               # Unix socket protocol
    │   └── tray.rs              # tray-icon menu
    ├── tests/
    │   └── smoke.rs             # integration tests
    ├── resources/
    │   ├── com.redlight.daemon.plist   # macOS launchd
    │   └── redlight.service            # Linux systemd user
    ├── README.md
    ├── PLAN.md
    ├── Cargo.toml
    ├── Cargo.lock                # committed (binary crate)
    └── .gitignore
  ```
- [ ] `Cargo.toml`: clap, serde, toml, tokio, anyhow, thiserror, tracing, walkdir, ignore, md-5, notify
- [ ] Platform-conditional deps via `[target.'cfg(target_os = "...")'.dependencies]`:
  - macOS → `io-kit-sys`, `core-foundation`, `objc2`
  - Linux → `udev`
  - cross → `tray-icon`
- [ ] Rust 1.80+ toolchain, edition 2024
- [ ] System prereqs documented: `libmtp` (macOS via brew, Linux via apt/dnf), `libudev-dev` for Linux
- [ ] `.gitignore` (`/target`, `*.local.toml`, `.DS_Store`)
- [ ] CI minimale: GitHub Actions matrix `[ubuntu-latest, macos-latest]` → `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`

---

## Phase 1 — Core data model

### 1.1 Config loading
- [ ] `Device`, `Item`, `Binding` structs with `serde::Deserialize`
- [ ] Parse `devices.toml`, `items.toml`, `bindings.toml` via `toml` crate
- [ ] Schema validation: device types, bridge names, item kinds, role values, glob syntax
- [ ] Cross-validation: every binding references a known device + item
- [ ] Path normalization with defaults:
  - host devices → `$HOME` if no path
  - drive devices → device root if no path
  - phone devices → `/storage/emulated/0/` if no path
- [ ] `~` expansion, absolute path resolution
- [ ] Hot reload via `notify` crate watching `~/.config/redlight/`

### 1.2 Manifest
- [ ] `Manifest` struct: `load`, `save`, `get_entry`, `update_entry`, `remove_entry`
- [ ] Atomic writes (write to temp + `fs::rename`)
- [ ] Format versioning (`version` field, migration path)
- [ ] One manifest per device, lives on the device itself

### 1.3 Logger
- [ ] Structured TOML entries: `{ timestamp, device, item, operation, file, status, error }`
- [ ] Rotation (max 500 entries on disk)
- [ ] Levels: INFO, WARNING, ERROR
- [ ] Mirrored to host (`~/.local/state/redlight/sync_log.toml`) and to each touched device (`/.redlight/sync_log.toml`)

---

## Phase 2 — Bridges

### 2.1 Bridge trait (`src/bridges/mod.rs`)
- [ ] `trait Bridge` with async methods (`#[async_trait]` or native async fn in trait)
- [ ] `list_files(path) -> Vec<FileMeta>` (recursive, with mtime + size)
- [ ] `get_file(remote_path, local_dest)`
- [ ] `put_file(local_src, remote_path)`
- [ ] `delete_file(path)`
- [ ] `make_dir(path)`
- [ ] `get_metadata(path) -> FileMeta`
- [ ] `read_text(path)` / `write_text(path, content)` for manifest bootstrap
- [ ] Lifecycle: `connect() -> ConnectedBridge` (RAII guard), drop = disconnect

### 2.2 `FsBridge`
- [ ] `std::fs` + `walkdir` + `tokio::fs` for async I/O
- [ ] Used for host + mounted drives (`/Volumes/*`, `/media/*`, `/mnt/*`)
- [ ] No connect/disconnect logic, always available

### 2.3 `MtpBridge`
- [ ] `tokio::process::Command` wrapping libmtp tools (`mtp-detect`, `mtp-files`, `mtp-getfile`, `mtp-sendfile`, `mtp-delfile`)
- [ ] Parse output reliably (consider piping through `--quiet` and structured flags)
- [ ] Error mapping: device disconnected, locked file, no space
- [ ] Alternative path: bind to a FUSE mount (`jmtpfs`, `go-mtpfs`) and reuse `FsBridge` semantics — evaluate which is more reliable
- [ ] Future option: link `libmtp-sys` directly via FFI (skip subprocess overhead)

### 2.4 `AdbBridge` (optional, override)
- [ ] Wrap `adb push`, `adb pull`, `adb shell ls`, `adb shell stat`
- [ ] Faster than MTP, but requires USB debugging enabled

### 2.5 Bridge registry
- [ ] Enum dispatch (`enum Bridges { Fs(FsBridge), Mtp(MtpBridge), Adb(AdbBridge) }`) selected from `devices.toml`
- [ ] Health check before sync
- [ ] Clear error when prerequisite missing (libmtp absent, adb not in PATH)

### 2.6 Device bootstrap
- [ ] On first encounter, create `/.redlight/` folder if missing
- [ ] Drop empty `manifest.toml` and `sync_log.toml`
- [ ] Verify write permissions

---

## Phase 3 — Sync engine

### 3.1 Diff per binding
- [ ] Resolve binding to absolute path on its device
- [ ] List source files via bridge
- [ ] Apply `include` / `exclude` glob filters (gitignore semantics: `**`, `*`, `?`, `[abc]`, leading `!` for negation)
- [ ] Compare against device's local manifest:
  - new file (in source, not in manifest)
  - modified (mtime or size changed)
  - deleted (in manifest, not in source)
  - unchanged
- [ ] Hash optimization: compute MD5 only when mtime/size suggest change
- [ ] For `kind = "file"`: skip listing, single-entry diff

### 3.2 Pairwise reconciliation
- [ ] For each item bound on both devices A and B:
  - Diff A side, diff B side
  - For each filename: classify (only-A, only-B, both, conflict)
  - Conflict = both modified since last sync → most recent `mtime` wins
  - Honor roles: `read_only` device never receives pushes from outside, never pushes outward
- [ ] Operation order: creates → updates → deletes
- [ ] Optional `.rl_backup` for overwritten files (config flag)

### 3.3 Transfer execution
- [ ] Per-file: transfer, verify hash, update both manifests, log
- [ ] Survive interruption: manifest update happens after each file, not in batch
- [ ] Resume on partial: skip already-transferred files via manifest

### 3.4 Modes
- [ ] `--dry-run` — print intended ops, no transfer, no manifest write
- [ ] `--item NAME` — restrict to one item
- [ ] `--device NAME` — restrict to one device pairing

### 3.5 Edge cases
- [ ] Locked / open file on host → skip + warning
- [ ] Insufficient space on destination → abort that binding, continue others, log clearly
- [ ] Filenames with unicode (accents, spaces, emojis)
- [ ] Symlinks → ignore by default, configurable via item flag
- [ ] File modified during transfer → rehash at end, retry once, then warn
- [ ] Empty file (0 bytes) — must transfer correctly

---

## Phase 4 — Detection & daemon

### 4.1 Watcher abstraction
- [ ] `trait Watcher`: emits `DeviceConnected { id, kind }` / `DeviceDisconnected { id }` events on a `tokio::sync::mpsc` channel
- [ ] Pluggable backends per platform, selected at compile time via `cfg`
- [ ] Filter non-storage devices (keyboards, dongles) by vendor class

### 4.2 macOS backend (`watcher/macos.rs`)
- [ ] `io-kit-sys` + `core-foundation` for USB phones (`IOServiceMatching("IOUSBDevice")`)
- [ ] `disk_arbitration_sys` (or hand-rolled FFI) for volume mount/unmount
- [ ] Identify by vendor/product/serial or volume label/UUID
- [ ] Run-loop integration with tokio (likely a dedicated thread bridging CFRunLoop → mpsc)

### 4.3 Linux backend (`watcher/linux.rs`)
- [ ] `udev` crate `MonitorBuilder` on subsystem `usb` (phones) and `block` (drives)
- [ ] Match by `ID_VENDOR_ID`, `ID_MODEL_ID`, `ID_SERIAL_SHORT` for phones
- [ ] Match by `ID_FS_LABEL` or `ID_FS_UUID` for drives
- [ ] Trigger mount of drives via `udisksctl` if needed

### 4.4 Daemon main loop
- [ ] Background process, single instance (lockfile in `~/.local/state/redlight/redlight.pid`)
- [ ] Signal handling: SIGTERM finishes current sync, then exits
- [ ] Queue: serialize syncs across devices (no parallel)
- [ ] Per-sync timeout (30 min default), abort + log

### 4.5 launchd integration (macOS)
- [ ] `com.redlight.daemon.plist` in `~/Library/LaunchAgents/`
- [ ] `RunAtLoad = true`, `KeepAlive = true`
- [ ] stdout/stderr → `~/.local/state/redlight/daemon.log`
- [ ] `rl start` → `launchctl load`, `rl stop` → `launchctl unload`

### 4.6 systemd user integration (Linux)
- [ ] `redlight.service` in `~/.config/systemd/user/`
- [ ] `Restart=on-failure`, `Type=simple`, journal logging
- [ ] `rl start` → `systemctl --user start redlight`, `rl stop` → analog
- [ ] `rl init` enables on login (`systemctl --user enable`)

### 4.7 IPC
- [ ] Unix socket at `$XDG_RUNTIME_DIR/redlight.sock` (Linux) / `/tmp/redlight.sock` (macOS)
- [ ] JSON line protocol
- [ ] Commands: `status`, `sync_now`, `reload_config`, `shutdown`

---

## Phase 5 — CLI `rl`

### 5.1 Framework
- [ ] `clap` derive API: top-level enum + nested subcommand enums for `device`, `item`, `bind`
- [ ] Top-level: `init`, `start`, `stop`, `status`, `sync`, `log`, `manifest`
- [ ] Binary declared in `Cargo.toml` (`[[bin]] name = "rl"`)
- [ ] Contextual help everywhere (clap auto-generated)

### 5.2 Commands
- [ ] `rl init` — create config skeleton, install launchd plist, check libmtp, start daemon
- [ ] `rl start` / `rl stop` / `rl restart`
- [ ] `rl status` — daemon state, connected devices, last sync per binding
- [ ] `rl device add|remove|list` — manage devices
- [ ] `rl item add|remove|list` — manage item definitions, including include/exclude
- [ ] `rl bind add|remove|list` — manage (item × device) pairs and roles
- [ ] `rl sync [--dry-run] [--item NAME] [--device NAME]` — force sync
- [ ] `rl log [--errors] [--device NAME] [--item NAME] [--tail N]`
- [ ] `rl manifest [--device NAME] [--item NAME]`

---

## Phase 6 — Menu bar / tray (macOS + Linux)

### 6.1 Icon + states
- [ ] `tray-icon` crate (cross-platform: NSStatusItem on macOS, AppIndicator/SNI on Linux)
- [ ] States: idle (grey), syncing (animated), error (red), recent success (green flash)

### 6.2 Dropdown
```
● Redlight
──────────────
Jarvis — 2 min ago
  ✓ music
  ✓ admin
  ✗ photos (no space)
Materia — 1 day ago
  ✓ music
──────────────
Sync now
──────────────
Preferences…
View logs
──────────────
Quit
```

### 6.3 Notifications
- [ ] Connect → "Jarvis connected, sync starting"
- [ ] Success → "Sync done — 12 files transferred"
- [ ] Error → "Sync error — photos: insufficient space"

### 6.4 Preferences window
- [ ] Items list + enable/disable toggle
- [ ] Bindings table
- [ ] Add / remove via native pickers + drag-and-drop
- [ ] Edit include/exclude patterns (with live preview of matched files)

---

## Phase 7 — Packaging & distribution

- [ ] macOS: `cargo bundle --release` → `.app`; `create-dmg` for `.dmg`; signing (Apple Developer ID, optional)
- [ ] Linux: `cargo deb` for Debian/Ubuntu, `cargo generate-rpm` for Fedora; AUR PKGBUILD
- [ ] Homebrew formula (`slashome/homebrew-tap` puis migration vers core)
- [ ] Installateur `redlight.sh` hébergé sur `slashome.me/apps/`: détecte l'OS, télécharge le binaire de la dernière release GitHub, installe `libmtp` via le gestionnaire de paquets local
- [ ] `.icns` / `.png` icon
- [ ] Update check via GitHub releases API

---

## Phase 8 — Tests & robustness

### 8.1 Unit (in-module `#[cfg(test)] mod tests`)
- [ ] `config` — parse valid/invalid TOML, cross-references, defaults
- [ ] `manifest` — load/save, atomic write, version migration
- [ ] `bridges::fs` — full coverage with `tempfile::TempDir`
- [ ] `bridges::mtp` — mocked process via trait abstraction
- [ ] `sync` — diff, conflicts, role enforcement, include/exclude

### 8.2 Integration (`tests/*.rs`)
- [ ] FS-only synthetic devices (two tempdirs simulating two devices) → end-to-end sync
- [ ] Disconnect mid-sync simulation
- [ ] Insufficient space simulation
- [ ] CLI smoke via `assert_cmd`

### 8.3 Validated edge cases
- [ ] 0-byte file
- [ ] Multi-GB file
- [ ] Directory with 10k+ files
- [ ] Unicode filenames
- [ ] Rapid plug/unplug
- [ ] Two devices co-present
- [ ] Config edited mid-sync
- [ ] Daemon crash mid-sync → clean recovery on restart

---

## Out of scope (v1)

- Encryption (planned v2)
- Windows support (planned v2 — service + WMI/SetupDi)
- Wi-Fi / cloud sync (never)
- Conflict resolution beyond "most recent wins" (v2: keep both, prompt user)
- Direct active+active USB transfer (works via SSD courier in v1)
- Passive+passive logging (e.g., Jarvis + Materia via OTG)

---

## Deferred / known limitations

Things intentionally left for later, tracked here so they don't get
lost. Each one is also marked with a `TODO(version)` comment at the
exact code location.

### Phase 2 — bridges

- `registry::build_bridge` uses `unwrap_or_default()` on MTP matcher
  fields. Harmless today (`jmtpfs` picks the first connected device)
  but will silently produce wrong identifiers once multi-device
  disambiguation lands. Replace with a required-field check.
- `registry::build_bridge` eagerly mounts MTP devices at construction
  time. Acceptable for v1; consider lazy mount on first op if it
  becomes a measurable issue.
- `bootstrap::ensure_bootstrap` conflates "file missing" with any
  other metadata error (e.g. permission denied). Add a
  `Bridge::exists()` method (or surface `io::ErrorKind`) once we need
  the distinction.

### Phase 3 — sync

- `run_sync` only iterates host ↔ drive pairs, not the full pairwise
  ring among all present devices. The host is the de facto hub:
  when two drives are both bound to an item via the host, transitivity
  syncs them via the host. The only configuration this misses is
  "two drives share an item, with no host binding" — implausible
  enough to defer. Revisit if a real use case shows up.
- `run_sync` skips phone devices entirely. They surface in
  `SyncSummary::skipped_devices`. Auto-detection arrives in Phase 4
  with the watcher; in the meantime a manual opt-in flag could be
  added if needed (`--include-phone NAME` mounting via jmtpfs/adb).
- `SyncLog` is written only on the host
  (`$XDG_STATE_HOME/redlight/sync_log.toml`). The spec mirrors it to
  each touched device's `/.redlight/sync_log.toml` so a user looking
  at the drive can see what Redlight did. Mirror-on-device is deferred
  to v0.0.2 — pure bookkeeping, no functional impact.

---

## Per-phase smoke test

| Phase | Test |
|-------|------|
| 0 | `cargo build && cargo run -- --help` |
| 1 | `cargo test --lib config::` |
| 2 | `cargo test --lib bridges::fs::` |
| 3 | `cargo run -- sync --dry-run` on two FS devices |
| 4 | Plug Jarvis or Materia, check daemon log |
| 5 | `rl device list`, `rl bind list`, `rl status` |
| 6 | Launch daemon, check tray icon |
