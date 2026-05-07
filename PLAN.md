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

### Bridges

Pluggable per-device interface. Common ABC: `list_files`, `get_file`, `put_file`, `delete_file`, `make_dir`, `get_metadata`.

| Bridge | Used for |
|--------|----------|
| `fs` | host filesystem, external drives, FUSE mounts |
| `mtp` | Android default (libmtp) |
| `adb` | Android override |

Bridge per device is declared in `devices.toml`. Default for Android = `mtp`.

### Files layout

On **Tardis** (active host, Mac/Linux):
```
~/.config/redlight/
  ├── devices.toml          # known devices
  ├── items.toml            # item definitions
  ├── bindings.toml         # item × device pairs
  └── manifest.toml         # Tardis's own local manifest
~/.local/state/redlight/
  ├── sync_log.toml
  └── daemon.log
~/Library/LaunchAgents/com.redlight.daemon.plist   (macOS)
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
    ├── rl/
    │   ├── __init__.py
    │   ├── cli.py
    │   ├── daemon.py
    │   ├── config.py
    │   ├── manifest.py
    │   ├── sync_engine.py
    │   ├── bridges/
    │   │   ├── __init__.py
    │   │   ├── base.py
    │   │   ├── fs.py
    │   │   ├── mtp.py
    │   │   └── adb.py
    │   ├── usb_watcher.py
    │   ├── volume_watcher.py
    │   ├── menu_bar.py
    │   └── logger.py
    ├── tests/
    ├── resources/
    │   └── com.redlight.daemon.plist
    ├── README.md
    ├── PLAN.md
    ├── pyproject.toml
    └── .gitignore
  ```
- [ ] `pyproject.toml`: click, rumps, pyobjc-framework-IOKit, pyobjc-framework-DiskArbitration, tomli, tomli-w
- [ ] Python 3.11+ virtualenv
- [ ] Homebrew prereqs documented: `libmtp`
- [ ] `.gitignore` (venv, `__pycache__`, `.DS_Store`, dist, build)
- [ ] Pre-commit: black, ruff, mypy
- [ ] CI minimale: GitHub Actions lint + tests

---

## Phase 1 — Core data model

### 1.1 Config loading
- [ ] `Device`, `Item`, `Binding` dataclasses
- [ ] Parse `devices.toml`, `items.toml`, `bindings.toml`
- [ ] Schema validation: device types, bridge names, item kinds, role values, glob syntax
- [ ] Cross-validation: every binding references a known device + item
- [ ] Path normalization with defaults:
  - host devices → `$HOME` if no path
  - drive devices → device root if no path
  - phone devices → `/storage/emulated/0/` if no path
- [ ] `~` expansion, absolute path resolution
- [ ] Hot reload (file watch)

### 1.2 Manifest
- [ ] `Manifest` class: `load`, `save`, `get_entry`, `update_entry`, `remove_entry`
- [ ] Atomic writes (temp + rename)
- [ ] Format versioning (`version` field, migration path)
- [ ] One manifest per device, lives on the device itself

### 1.3 Logger
- [ ] Structured TOML entries: `{ timestamp, device, item, operation, file, status, error }`
- [ ] Rotation (max 500 entries on disk)
- [ ] Levels: INFO, WARNING, ERROR
- [ ] Mirrored to host (`~/.local/state/redlight/sync_log.toml`) and to each touched device (`/.redlight/sync_log.toml`)

---

## Phase 2 — Bridges

### 2.1 Bridge ABC (`bridges/base.py`)
- [ ] `list_files(path) → [FileMeta]` (recursive, with mtime + size)
- [ ] `get_file(remote_path, local_dest)`
- [ ] `put_file(local_src, remote_path)`
- [ ] `delete_file(path)`
- [ ] `make_dir(path)`
- [ ] `get_metadata(path) → FileMeta`
- [ ] `read_text(path)` / `write_text(path, content)` for manifest bootstrap
- [ ] Context manager: `with bridge.connect(): ...`

### 2.2 `FsBridge`
- [ ] Pure POSIX: `os`, `pathlib`, `shutil`
- [ ] Used for host + mounted drives (`/Volumes/*`, `/media/*`, `/mnt/*`)
- [ ] No special connect/disconnect logic

### 2.3 `MtpBridge`
- [ ] Subprocess wrapping libmtp tools (`mtp-detect`, `mtp-files`, `mtp-getfile`, `mtp-sendfile`, `mtp-delfile`)
- [ ] Parse output reliably (consider piping through `--quiet` and structured flags)
- [ ] Error mapping: device disconnected, locked file, no space
- [ ] Alternative path: bind to a FUSE mount (`jmtpfs`, `go-mtpfs`) and reuse `FsBridge` semantics — evaluate which is more reliable

### 2.4 `AdbBridge` (optional, override)
- [ ] Wrap `adb push`, `adb pull`, `adb shell ls`, `adb shell stat`
- [ ] Faster than MTP, but requires USB debugging enabled

### 2.5 Bridge registry
- [ ] Lookup by name from `devices.toml`
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

### 4.1 USB watcher (phones)
- [ ] `pyobjc-framework-IOKit` listens on `IOServiceMatching("IOUSBDevice")`
- [ ] Plug callback: identify by vendor/product/serial, match against `devices.toml`
- [ ] Unplug callback: tear down bridge cleanly
- [ ] Filter non-storage USB events (keyboards, dongles)

### 4.2 Volume watcher (drives)
- [ ] `pyobjc-framework-DiskArbitration` for mount/unmount
- [ ] Match volume label or UUID against `devices.toml`
- [ ] Mount callback → start sync; unmount callback → finalize cleanly

### 4.3 Daemon main loop
- [ ] Background process, single instance (lockfile)
- [ ] Signal handling: SIGTERM finishes current sync, then exits
- [ ] Queue: serialize syncs across devices (no parallel)
- [ ] Per-sync timeout (30 min default), abort + log

### 4.4 launchd integration (macOS)
- [ ] `com.redlight.daemon.plist` in `~/Library/LaunchAgents/`
- [ ] `RunAtLoad = true`, `KeepAlive = true`
- [ ] stdout/stderr → `~/.local/state/redlight/daemon.log`
- [ ] `rl start` → `launchctl load`, `rl stop` → `launchctl unload`

### 4.5 IPC
- [ ] Unix socket at `/tmp/redlight.sock`
- [ ] JSON line protocol
- [ ] Commands: `status`, `sync_now`, `reload_config`, `shutdown`

### 4.6 Linux daemon (later)
- [ ] systemd user service equivalent of launchd
- [ ] udev rules for plug detection
- [ ] Out of scope v1, planned v2

---

## Phase 5 — CLI `rl`

### 5.1 Framework
- [ ] `click` groups: `device`, `item`, `bind`, plus top-level (`init`, `start`, `stop`, `status`, `sync`, `log`, `manifest`)
- [ ] Entry point in `pyproject.toml`
- [ ] Contextual help everywhere

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

## Phase 6 — Menu bar (macOS)

### 6.1 Icon + states
- [ ] rumps-based menu bar app
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

- [ ] py2app build, bundles libmtp where possible
- [ ] `.icns` icon
- [ ] `create-dmg` for distribution
- [ ] Post-install: `brew install libmtp` if absent, then `rl init`
- [ ] Signing (Apple Developer ID — optional)
- [ ] Update check via GitHub releases API

---

## Phase 8 — Tests & robustness

### 8.1 Unit
- [ ] `test_config.py` — parse valid/invalid TOML, cross-references, defaults
- [ ] `test_manifest.py` — load/save, atomic write, version migration
- [ ] `test_bridges_fs.py` — full coverage with tmpdir
- [ ] `test_bridges_mtp.py` — mocked subprocess
- [ ] `test_sync_engine.py` — diff, conflicts, role enforcement, include/exclude

### 8.2 Integration
- [ ] FS-only synthetic devices (two tmpdirs simulating two devices) → end-to-end sync
- [ ] Disconnect mid-sync simulation
- [ ] Insufficient space simulation

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
- Windows support (planned v2)
- Linux daemon (planned v2 — systemd + udev)
- Wi-Fi / cloud sync (never)
- Conflict resolution beyond "most recent wins" (v2: keep both, prompt user)
- Direct active+active USB transfer (works via SSD courier in v1)
- Passive+passive logging (e.g., Jarvis + Materia via OTG)

---

## Per-phase smoke test

| Phase | Test |
|-------|------|
| 1 | `python -c "from rl.config import load; print(load())"` |
| 2 | `python -c "from rl.bridges.fs import FsBridge; print(FsBridge('/tmp').list_files('.'))"` |
| 3 | `rl sync --dry-run` on two FS devices |
| 4 | Plug Jarvis or Materia, check daemon log |
| 5 | `rl device list`, `rl bind list`, `rl status` |
| 6 | Launch app, check menu bar icon |
