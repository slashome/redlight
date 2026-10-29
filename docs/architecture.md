# Redlight — Architecture

## Entities

- **Device** — a peripheral participating in sync (computer, phone, drive). Has a *type* (active or passive) and a *bridge* (how to read/write its files).
- **Item** — a synced unit (folder or file). Carries `include`/`exclude` glob filters and an optional `category`.
- **Binding** — a (item × device) pair. Defines `path` on that device and `role` (`read_write` | `read_only`).

A sync exists between two devices for a given item iff both have a binding to that item.

## Active vs Passive

| Type | Examples | Carries |
|------|----------|---------|
| Active | Tardis (Mac/Linux) | daemon + global config + local manifest |
| Passive | Jarvis (Android), Materia (SSD) | local manifest only |

Rules:
- Active + Passive → standard case, the active drives the sync.
- Active + Active → supported, same model. Pairwise reconciliation. Direct USB rare in practice; usually relayed via a shared SSD courier.
- Passive + Passive → ignored in v1 (no daemon present to drive). Could log the encounter in a future version.

## Reconciliation model — "the ring"

Each device carries its own truth (its local manifest). Reconciliation happens **pairwise** when two devices are co-present:
1. Read both manifests + scan both file states.
2. For each shared item, compute diff per file (hash + mtime + size).
3. Resolve conflicts: most recent `mtime` wins (configurable in v2).
4. Apply transfers respecting `role` constraints.
5. Update both manifests.

The active device propagates state by participating in many pairings, but holds no privileged "hub" status in the model.

> **v0.0.1 implementation note** — `rl sync` iterates host ↔ drive pairs only; drive ↔ drive and phone participation are deferred (see `src/sync/run.rs`). Transitivity through the host covers the standard "two drives both bound to an item via the host" case.

## Bridges

Pluggable per-device interface (Rust trait): `list_files`, `get_file`, `put_file`, `delete_file`, `make_dir`, `get_metadata`.

| Bridge | Used for |
|--------|----------|
| `fs` | host filesystem, external drives, FUSE mounts |
| `mtp` | Android default (libmtp) |
| `adb` | Android override |

Bridge per device is declared in `devices.toml`. Default for Android = `mtp`.

## Files layout

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

## Manifest (`manifest.toml`, one per device)
```toml
version = 1
device = "jarvis"

[items.music.files."Beatles/Yesterday.mp3"]
hash = "5d41402abc4b2a76b9719d911017c592"
mtime = 1715000000
size = 4231823
last_sync = 1715000100
```

The other configuration files are described in [configuration.md](configuration.md).
