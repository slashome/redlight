# Configuration

This page documents the schema of the three TOML files that drive Redlight, the allowed values for each field, and shows concrete examples of common patterns. For the overview, see the [README](../README.md).

The three files live in `~/.config/redlight/` (or `$XDG_CONFIG_HOME/redlight/`):
- `devices.toml` — known peripherals
- `items.toml` — folders / files to sync
- `bindings.toml` — where each item lives on each device

---

## `devices.toml`

One block per peripheral. The key (`[tardis]`) is the **short name** you'll use everywhere else; `description` is free-form for human readability.

```toml
[tardis]
type = "host"
bridge = "fs"
description = "MacBook Pro M2 Max 64GB"

[jarvis]
type = "phone"
bridge = "mtp"
description = "Fairphone 6"
match.vendor_id = "18d1"
match.product_id = "4ee7"
match.serial = "ABC123"

[materia]
type = "drive"
bridge = "fs"
description = "Playstation SSD"
match.volume_label = "MATERIA"
```

### Fields

| field | type | required | description |
|---|---|---|---|
| `type` | `"host"` \| `"phone"` \| `"drive"` | yes | device kind |
| `bridge` | `"fs"` \| `"mtp"` \| `"adb"` | yes | how Redlight talks to the device |
| `description` | string | no | free-form label |
| `match.vendor_id` / `match.product_id` / `match.serial` | string | phone: `serial` required | USB identification (from `system_profiler` / `lsusb`) |
| `match.volume_label` / `match.volume_uuid` | string | drive: at least one of the two | partition label / filesystem UUID |
| `match.hostname` | string | needed only when multiple hosts | matched against `gethostname()` at runtime |

### Bridge ↔ type compatibility

| type | allowed bridges |
|---|---|
| `host` | `fs` |
| `drive` | `fs` |
| `phone` | `mtp` (default) or `adb` |

| bridge | usage | phone setup |
|--------|-------|-------------|
| `fs`   | host, external drives | — |
| `mtp`  | Android, default, via `jmtpfs` (FUSE) | none |
| `adb`  | Android, recommended for power users: faster, more reliable, accurate mtime | enable Developer Options + USB Debugging (~30s, once) |

ADB is **not** installed by default: it's pulled in on demand if you declare `bridge = "adb"` on a device.

---

## `items.toml`

One block per unit to sync. The key (`[music]`) is the **item's name**.

```toml
[music]
kind = "folder"
category = "audio"
description = "My FLAC collection sorted by artist"
include = ["**/*.mp3", "**/*.flac"]
exclude = ["**/draft-*"]

[contrat-freelance]
kind = "file"
category = "admin"
```

### Fields

| field | type | required | description |
|---|---|---|---|
| `kind` | `"folder"` \| `"file"` | yes | recursive folder or single file |
| `category` | string | no | free-form tag |
| `description` | string | no | free-form label |
| `include` | list of globs | no | allowlist (empty = everything allowed) |
| `exclude` | list of globs | no | denylist (always applied) |

### Item kinds

| kind | behavior |
|------|----------|
| `folder` | all contents, recursive, filterable via `include` / `exclude` |
| `file` | this one file only |

### Glob filters

gitignore-style syntax:
- `**` — zero or more path components
- `*` — any filename without `/`
- `?` — one character
- `[abc]` — any character among `abc`

A file passes iff:
1. it matches at least one `include` pattern (if the list is non-empty), **and**
2. it doesn't match any `exclude` pattern

### Automatic exclusions

Whatever your `exclude` says, Redlight always refuses to sync:
- its own bookkeeping folder (`.redlight/`)
- macOS detritus (`.DS_Store`, `.Spotlight-V100/`, `.Trashes/`, `.fseventsd/`, `._*`, …)
- Windows detritus (`Thumbs.db`, `desktop.ini`, `$RECYCLE.BIN/`, `System Volume Information/`)
- Linux detritus (`lost+found/`, `.Trash-*/`)

---

## `bindings.toml`

Array of (item × device) pairs. Each entry says "this item, on this device, lives at this path, with this role".

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
path = "/Music"
role = "read_write"
```

### Fields

| field | type | required | description |
|---|---|---|---|
| `item` | string | yes | name of an item declared in `items.toml` |
| `device` | string | yes | name of a device declared in `devices.toml` |
| `path` | string | no | path on the device (see defaults below) |
| `role` | `"read_write"` \| `"read_only"` | yes | participation in sync |
| `include` | list of globs | no | device-specific allowlist (intersection with the item's) |
| `exclude` | list of globs | no | device-specific denylist (union with the item's) |

### Roles

| role | behavior |
|------|----------|
| `read_write` | participates fully |
| `read_only` | receives, never pushes |

### Default paths

If `path` is absent:
- **host** → `$HOME`
- **drive** → device root (`/`)
- **phone** → `/storage/emulated/0/` (Android user storage root)

Binding to the root of a device is perfectly legitimate (see the "USB key" example below) — automatic exclusions keep Redlight's bookkeeping from polluting the binding.

### Per-binding filters

`include` / `exclude` can live **at two levels**:

- **At the item level** (`items.toml`): define what the item is in general, shared by every device that binds it.
- **At the binding level** (`bindings.toml`): narrow **specifically** what goes to this device.

Combination:
- **Includes**: a file passes iff it matches one of the item's patterns **AND** one of the binding's (when each is non-empty). It's an **intersection**.
- **Excludes**: a file is blocked if **any** pattern (item, binding, or system) matches. It's a **union**.

Typical use case: full music collection on tardis/hal9000/materia, but **a subset** on jarvis (the phone is short on space):

```toml
# items.toml
[music]
kind = "folder"
include = ["**/*.mp3", "**/*.flac"]   # what "music" is

# bindings.toml
[[binding]]
item = "music"
device = "jarvis"
path = "/storage/emulated/0/Music"
role = "read_only"
include = [                            # phone-specific subset
  "**/Beatles/**",
  "**/Daft Punk/**",
  "**/Top hits 2024/**",
  "**/Favorites/**",
]
```

Effect on jarvis: only files that match `(*.mp3 OR *.flac) AND (under Beatles/Daft Punk/Top hits 2024/Favorites)` are synced. On tardis/hal9000/materia (no binding `include`), all music keeps syncing normally.

You can also exclude punctually, e.g. you want all music on jarvis except audiobooks:
```toml
[[binding]]
item = "music"
device = "jarvis"
exclude = ["**/Audiobooks/**", "**/Podcasts/**"]
```

### Validation rules

- Each binding must reference a known `item` and `device`
- No duplicate `(item, device)` — one binding per pair
- `bridge` must be compatible with `type` (see table above)
- `phone` requires `match.serial`; `drive` requires `match.volume_label` or `match.volume_uuid`

If the config is invalid, `rl init` / `rl doctor` list **every** error at once, not one per run.

---

## Example: sync a whole USB key

Use case: you have a USB key `xfiles` and you want **all of its contents** mirrored into a dedicated subfolder on your other devices. Materia (your SSD) doesn't participate — you don't want this key to pollute your main SSD.

`devices.toml` (additions):
```toml
[xfiles]
type = "drive"
bridge = "fs"
description = "X-Files USB key"
match.volume_label = "XFILES"
```

`items.toml` (additions):
```toml
[xfiles]
kind = "folder"
description = "Full X-Files key contents"
```

`bindings.toml` (additions):
```toml
[[binding]]
item = "xfiles"
device = "xfiles"
path = "/"                                          # key root
role = "read_write"

[[binding]]
item = "xfiles"
device = "tardis"
path = "~/documents/xfiles"                         # subfolder on the computer
role = "read_write"

[[binding]]
item = "xfiles"
device = "jarvis"
path = "/storage/emulated/0/saves/xfiles"           # subfolder on the phone
role = "read_only"                                  # phone receives, never pushes
```

`materia` has no binding on `xfiles` → the key doesn't mirror onto the SSD. Bookkeeping files (`xfiles:/.redlight/manifest.toml`) are auto-filtered and don't appear in the `xfiles/` subfolders on the other devices.

---

## Example: 2 computers + a courier SSD

Use case: you have a personal Mac (tardis) and a work Mac (hal9000), plus a SSD (materia) you carry between them. You want your music collection in sync across both machines, with materia acting as a courier — neither Mac is ever directly co-present with the other.

The Redlight model supports this directly: declare **both machines as `type = "host"`**, the SSD as a drive, and three bindings. At runtime, each machine identifies which host entry it is by comparing `match.hostname` against `gethostname()`.

`devices.toml`:
```toml
[tardis]
type = "host"
bridge = "fs"
description = "Personal MacBook Pro"
match.hostname = "tardis.local"

[hal9000]
type = "host"
bridge = "fs"
description = "Work Mac Pro"
match.hostname = "hal9000.work.local"

[materia]
type = "drive"
bridge = "fs"
description = "Personal ↔ work SSD courier"
match.volume_label = "MATERIA"
```

`items.toml`:
```toml
[music]
kind = "folder"
```

`bindings.toml`:
```toml
[[binding]]
item = "music"
device = "tardis"
path = "~/Music"
role = "read_write"

[[binding]]
item = "music"
device = "hal9000"
path = "~/Music"        # same path relative to $HOME, even if the user account differs
role = "read_write"

[[binding]]
item = "music"
device = "materia"
path = "/Music"
role = "read_write"
```

**How it plays out:**

1. At home, tardis runs with this config. You plug in materia. The tardis daemon syncs `~/Music` (tardis) ↔ `/Music` (materia).
2. You unplug materia and head to work.
3. At work, hal9000 runs with the **same** config (synced via dotfiles, for example). You plug in materia. The hal9000 daemon syncs `~/Music` (hal9000) ↔ `/Music` (materia).
4. Net effect: the music present on tardis has travelled through materia to hal9000, and vice versa.

Three notes:

- The **same config file** works on both machines. `match.hostname` lets each daemon know which host entry it is.
- If you only have **one** machine, you can omit `match.hostname` and declare a single host — Redlight picks that host by default.
- If you have **several** and none of their `match.hostname` matches the current machine, the daemon refuses to start with a clear error pointing you to the fix. No silent "first wins".
