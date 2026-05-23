# Configuration

Cette page documente le schéma des trois fichiers TOML qui pilotent Redlight, les valeurs autorisées pour chaque champ, et donne des exemples concrets de patterns courants. Pour la vue d'ensemble, voir le [README](../README.md).

Les trois fichiers vivent dans `~/.config/redlight/` (ou `$XDG_CONFIG_HOME/redlight/`) :
- `devices.toml` — les périphériques connus
- `items.toml` — les dossiers / fichiers à synchroniser
- `bindings.toml` — où vit chaque item sur chaque device

---

## `devices.toml`

Un bloc par périphérique. La clé (`[tardis]`) est le **nom court** que tu utilises partout ailleurs ; `description` est libre pour la lisibilité humaine.

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
description = "SSD Playstation"
match.volume_label = "MATERIA"
```

### Champs

| champ | type | obligatoire | description |
|---|---|---|---|
| `type` | `"host"` \| `"phone"` \| `"drive"` | oui | nature du device |
| `bridge` | `"fs"` \| `"mtp"` \| `"adb"` | oui | comment Redlight parle au device |
| `description` | string | non | libellé libre |
| `match.vendor_id` / `match.product_id` / `match.serial` | string | phone : `serial` obligatoire | identification USB (sortie de `system_profiler` / `lsusb`) |
| `match.volume_label` / `match.volume_uuid` | string | drive : au moins un des deux | label de partition / UUID filesystem |

### Compatibilité bridge ↔ type

| type | bridges autorisés |
|---|---|
| `host` | `fs` |
| `drive` | `fs` |
| `phone` | `mtp` (défaut) ou `adb` |

| bridge | usage | setup phone |
|--------|-------|-------------|
| `fs`   | hôte, disques externes | — |
| `mtp`  | Android, défaut, via `jmtpfs` (FUSE) | aucun |
| `adb`  | Android, alternative recommandée pour power users : plus rapide, plus fiable, mtime précis | activer Developer Options + USB Debugging (~30s, une fois) |

ADB n'est **pas** installé par défaut : il n'est tiré qu'à la demande si tu déclares `bridge = "adb"` sur un device.

---

## `items.toml`

Un bloc par unité à synchroniser. La clé (`[music]`) est le **nom de l'item**.

```toml
[music]
kind = "folder"
category = "audio"
description = "Ma collection FLAC trié par artiste"
include = ["**/*.mp3", "**/*.flac"]
exclude = ["**/draft-*"]

[contrat-freelance]
kind = "file"
category = "admin"
```

### Champs

| champ | type | obligatoire | description |
|---|---|---|---|
| `kind` | `"folder"` \| `"file"` | oui | dossier récursif ou fichier unique |
| `category` | string | non | tag libre |
| `description` | string | non | libellé libre |
| `include` | liste de globs | non | allowlist (vide = tout autorisé) |
| `exclude` | liste de globs | non | denylist (toujours appliquée) |

### Types d'item

| kind | comportement |
|------|-------------|
| `folder` | tout le contenu, récursif, filtrable via `include` / `exclude` |
| `file` | uniquement ce fichier |

### Filtres glob

Syntaxe gitignore-style :
- `**` — zéro ou plusieurs composants de chemin
- `*` — n'importe quel nom de fichier sans `/`
- `?` — un caractère
- `[abc]` — caractère parmi `abc`

Un fichier passe s'il :
1. matche au moins un pattern `include` (si la liste est non vide), **et**
2. ne matche aucun pattern `exclude`

### Exclusions automatiques

Quel que soit ton `exclude`, Redlight refuse toujours de synchroniser :
- son propre dossier de bookkeeping (`.redlight/`)
- les détritus macOS (`.DS_Store`, `.Spotlight-V100/`, `.Trashes/`, `.fseventsd/`, `._*`, …)
- les détritus Windows (`Thumbs.db`, `desktop.ini`, `$RECYCLE.BIN/`, `System Volume Information/`)
- les détritus Linux (`lost+found/`, `.Trash-*/`)

---

## `bindings.toml`

Tableau de couples (item × device). Chaque entrée dit "cet item, sur ce device, vit à ce path, avec ce rôle".

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

### Champs

| champ | type | obligatoire | description |
|---|---|---|---|
| `item` | string | oui | nom d'un item déclaré dans `items.toml` |
| `device` | string | oui | nom d'un device déclaré dans `devices.toml` |
| `path` | string | non | chemin sur le device (voir défauts ci-dessous) |
| `role` | `"read_write"` \| `"read_only"` | oui | participation à la sync |
| `include` | liste de globs | non | allowlist spécifique au device (intersection avec l'item) |
| `exclude` | liste de globs | non | denylist spécifique au device (union avec l'item) |

### Roles

| role | comportement |
|------|-------------|
| `read_write` | participe pleinement |
| `read_only` | reçoit, ne pousse jamais |

### Chemins par défaut

Si `path` est absent :
- **host** → `$HOME`
- **drive** → racine du device (`/`)
- **phone** → `/storage/emulated/0/` (racine du stockage utilisateur Android)

Binder à la racine d'un device est tout à fait légitime (voir l'exemple "clé USB" ci-dessous) — les exclusions automatiques empêchent le bookkeeping Redlight de polluer le binding.

### Filtres par binding

Les `include` / `exclude` peuvent vivre **à deux niveaux** :

- **Au niveau de l'item** (`items.toml`) : définissent ce qu'est l'item en général, partagé par tous les devices qui le bindent.
- **Au niveau du binding** (`bindings.toml`) : restreignent **spécifiquement** ce qui va sur ce device.

Combinaison :
- **Includes** : un fichier passe ssi il matche un pattern de l'item **ET** un pattern du binding (si non vides chacun). C'est une **intersection**.
- **Excludes** : un fichier est bloqué si **n'importe quel** pattern (item, binding ou système) matche. C'est une **union**.

Cas d'usage typique : tu as une collection musique complète sur tardis/hal9000/materia, mais tu veux **un sous-ensemble** sur jarvis (le téléphone manque de place pour tout) :

```toml
# items.toml
[music]
kind = "folder"
include = ["**/*.mp3", "**/*.flac"]   # ce qu'est "music"

# bindings.toml
[[binding]]
item = "music"
device = "jarvis"
path = "/storage/emulated/0/Music"
role = "read_only"
include = [                            # subset spécifique au phone
  "**/Beatles/**",
  "**/Daft Punk/**",
  "**/Top hits 2024/**",
  "**/Favorites/**",
]
```

Effet sur jarvis : seuls les fichiers qui matchent `(*.mp3 OU *.flac) ET (sous Beatles/Daft Punk/Top hits 2024/Favorites)` sont synchronisés. Sur tardis/hal9000/materia (sans `include` binding), toute la musique reste synchronisée normalement.

Tu peux aussi exclure ponctuellement, ex. tu veux toute la musique sur jarvis sauf audiobooks :
```toml
[[binding]]
item = "music"
device = "jarvis"
exclude = ["**/Audiobooks/**", "**/Podcasts/**"]
```

### Règles de validation

- Chaque binding doit référencer un `item` et un `device` connus
- Pas de doublon `(item, device)` — un seul binding par paire
- `bridge` doit être compatible avec `type` (cf. table plus haut)
- `phone` requiert `match.serial` ; `drive` requiert `match.volume_label` ou `match.volume_uuid`

Si la config est invalide, `rl init` / `rl doctor` te listent **toutes** les erreurs d'un coup, pas une par run.

---

## Exemple : synchroniser une clé USB entière

Cas d'usage : tu as une clé USB `xfiles` dont tu veux que **tout le contenu** soit reflété dans un sous-dossier dédié sur tes autres devices. Materia (ton SSD) ne participe pas — tu ne veux pas que cette clé pollue ton SSD principal.

`devices.toml` (ajout) :
```toml
[xfiles]
type = "drive"
bridge = "fs"
description = "Clé USB X-Files"
match.volume_label = "XFILES"
```

`items.toml` (ajout) :
```toml
[xfiles]
kind = "folder"
description = "Contenu intégral de la clé X-Files"
```

`bindings.toml` (ajout) :
```toml
[[binding]]
item = "xfiles"
device = "xfiles"
path = "/"                                          # racine de la clé
role = "read_write"

[[binding]]
item = "xfiles"
device = "tardis"
path = "~/documents/xfiles"                         # sous-dossier sur l'ordi
role = "read_write"

[[binding]]
item = "xfiles"
device = "jarvis"
path = "/storage/emulated/0/saves/xfiles"           # sous-dossier sur le phone
role = "read_only"                                  # le phone reçoit, ne pousse pas
```

`materia` n'a aucun binding sur `xfiles` → la clé ne se reflète pas sur le SSD. Les fichiers de bookkeeping (`xfiles:/.redlight/manifest.toml`) sont automatiquement filtrés et n'apparaissent pas dans le sous-dossier `xfiles/` des autres devices.

---

## Exemple : 2 ordinateurs + un SSD navette

Cas d'usage : tu as un Mac perso (tardis) et un Mac de travail (hal9000), un SSD (materia) que tu emportes avec toi. Tu veux que ta collection musique soit synchronisée entre les deux machines, materia servant de courrier — un Mac n'est jamais directement en présence de l'autre.

Le modèle Redlight le supporte directement : on déclare **les deux machines comme des `type = "host"`**, le SSD comme drive, et trois bindings. Au runtime, chaque machine identifie quelle entrée host elle est via le champ `match.hostname` (comparé à `gethostname()`).

`devices.toml` :
```toml
[tardis]
type = "host"
bridge = "fs"
description = "MacBook Pro perso"
match.hostname = "tardis.local"

[hal9000]
type = "host"
bridge = "fs"
description = "Mac Pro travail"
match.hostname = "hal9000.work.local"

[materia]
type = "drive"
bridge = "fs"
description = "SSD navette perso ↔ taf"
match.volume_label = "MATERIA"
```

`items.toml` :
```toml
[music]
kind = "folder"
```

`bindings.toml` :
```toml
[[binding]]
item = "music"
device = "tardis"
path = "~/Music"
role = "read_write"

[[binding]]
item = "music"
device = "hal9000"
path = "~/Music"        # même chemin relatif au $HOME, même si l'utilisateur diffère
role = "read_write"

[[binding]]
item = "music"
device = "materia"
path = "/Music"
role = "read_write"
```

**Comment ça se déroule :**

1. Chez toi, tardis tourne avec ce config. Tu branches materia. Le daemon de tardis sync `~/Music` (tardis) ↔ `/Music` (materia).
2. Tu débranches materia, tu pars au travail.
3. Au taf, hal9000 tourne avec le **même** config (synchronisé via dotfiles, par exemple). Tu branches materia. Le daemon de hal9000 sync `~/Music` (hal9000) ↔ `/Music` (materia).
4. Net effet : la musique présente sur tardis a transité par materia jusqu'à hal9000, et inversement.

Trois remarques :

- Le **même fichier de config** marche sur les deux machines. `match.hostname` permet à chaque daemon de savoir laquelle des entrées host est lui.
- Si tu n'as qu'**une seule** machine, tu peux omettre `match.hostname` et déclarer un seul host — Redlight prend ce host par défaut.
- Si tu en as **plusieurs** et qu'aucune n'a `match.hostname` qui matche, le daemon refuse de démarrer avec un message clair pointant vers la correction nécessaire. Pas de "qui tire le premier" silencieux.
