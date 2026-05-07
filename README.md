# Redlight

> `rl` — synchronise tes appareils par USB, sans cloud, sans app, sans friction.

Redlight est un daemon macOS qui synchronise automatiquement des fichiers et dossiers entre plusieurs périphériques (ordi, téléphone, disque externe) au moment de leur branchement. Aucune app à installer sur les périphériques passifs, aucun cloud, tout passe par USB.

---

## Philosophie

- **Multi-périphériques.** Ordi, téléphone, disque externe — tous sont des devices au même titre. Chacun porte son état local.
- **Le ring.** Pas de hub privilégié : quand deux devices sont co-présents, ils se réconcilient pairwise. Le plus récent gagne.
- **Pas de cloud.** Tout passe par USB, rien ne transite par un serveur tiers.
- **Pas de dossier imposé.** Chaque règle pointe vers les vrais emplacements des fichiers, où qu'ils soient.
- **Les passifs ne font rien.** Téléphone et disque sont passifs, ils portent juste leur manifeste. Seul l'ordi exécute.
- **Prévisible.** Un dossier sync tout son contenu (filtrable via `include`/`exclude`). Un fichier sync uniquement ce fichier.
- **Transparent.** Un manifeste TOML par device décrit l'état connu de chaque fichier suivi.

---

## Modèle

Trois entités :

- **Device** — un périphérique. Type *actif* (porte le daemon) ou *passif* (porte juste son manifeste).
- **Item** — une unité synchronisée (dossier ou fichier), avec filtres `include`/`exclude` et `category` optionnelle.
- **Binding** — un couple (item, device). Définit le `path` sur ce device et le `role` (`read_write` ou `read_only`).

Un sync existe entre deux devices pour un item ssi les deux ont un binding sur cet item.

| Combo | Comportement |
|---|---|
| Actif + Passif | cas standard, l'actif pilote |
| Actif + Actif | supporté, réconciliation pairwise |
| Passif + Passif | ignoré (pas de daemon) |

---

## Fonctionnement

À chaque branchement d'un device connu :

1. Le daemon détecte (IOKit pour les phones USB, DiskArbitration pour les drives)
2. Il identifie le device via son ID matériel ou son label de volume
3. Il charge la config (`devices.toml`, `items.toml`, `bindings.toml`)
4. Pour chaque item bindé sur ce device et sur un autre device présent : diff manifeste vs état réel
5. Réconciliation pairwise, transferts respectant les `role`
6. Mise à jour des manifestes locaux et du log
7. Notification via menu bar

---

## Configuration

`~/.config/redlight/devices.toml` :

```toml
[tardis]
type = "host"
bridge = "fs"

[jarvis]
type = "phone"
bridge = "mtp"
match.vendor_id = "18d1"
match.product_id = "4ee7"
match.serial = "ABC123"

[materia]
type = "drive"
bridge = "fs"
match.volume_label = "MATERIA"
```

`~/.config/redlight/items.toml` :

```toml
[music]
kind = "folder"
category = "audio"
include = ["**/*.mp3", "**/*.flac"]
exclude = ["**/draft-*"]

[contrat-freelance]
kind = "file"
category = "admin"
```

`~/.config/redlight/bindings.toml` :

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
role = "read_write"
```

### Types d'item

| kind | comportement |
|------|-------------|
| `folder` | tout le contenu, récursif, filtrable via `include` / `exclude` |
| `file` | uniquement ce fichier |

### Bridges

| bridge | usage |
|--------|-------|
| `fs` | hôte, disques externes, FUSE |
| `mtp` | Android (défaut) |
| `adb` | Android override (debug USB requis) |

### Roles

| role | comportement |
|------|-------------|
| `read_write` | participe pleinement |
| `read_only` | reçoit, ne pousse jamais |

Chemins par défaut si `path` absent : `$HOME` sur host, racine du device sur disque, `/storage/emulated/0/` sur Android.

---

## Installation

```bash
# Prérequis
brew install libmtp

# Installer redlight
pip install redlight

# Initialiser
rl init

# Démarrer le daemon (au login automatiquement via launchd)
rl start

# Ajouter un device
rl device add jarvis --type phone --bridge mtp

# Ajouter un item
rl item add music --kind folder --include "**/*.mp3"

# Lier item × device
rl bind add --item music --device tardis --path ~/Music --role read_write

# Voir le statut
rl status
```

---

## Commandes `rl`

| commande | description |
|----------|-------------|
| `rl init` | initialise la config et enregistre le launchd agent |
| `rl start` / `rl stop` / `rl restart` | contrôle du daemon |
| `rl status` | état du daemon, devices connectés, dernière sync par binding |
| `rl device add\|remove\|list` | gestion des devices |
| `rl item add\|remove\|list` | gestion des items |
| `rl bind add\|remove\|list` | gestion des bindings |
| `rl sync [--dry-run] [--item N] [--device N]` | force une sync immédiate |
| `rl log [--errors] [--device N] [--item N] [--tail N]` | affiche les logs |
| `rl manifest [--device N] [--item N]` | affiche le manifeste |

---

## Stack technique

| composant | technologie |
|-----------|------------|
| Langage | Python 3.11+ |
| Daemon macOS | `launchd` via `.plist` |
| Détection USB (phones) | `pyobjc-framework-IOKit` |
| Détection volumes (drives) | `pyobjc-framework-DiskArbitration` |
| Bridge MTP | `libmtp` (Homebrew) + subprocess |
| Bridge ADB | `adb` (optionnel) |
| Bridge FS | stdlib |
| Menu bar | `rumps` |
| Config | TOML (`tomllib` stdlib + `tomli-w`) |
| Manifeste | TOML par device |
| CLI | `click` |
| Packaging | `py2app` |

---

## Ce que Redlight ne fait pas (v1)

- Pas de sync Wi-Fi
- Pas de cloud
- Pas d'app sur les périphériques passifs
- Pas de chiffrement (prévu v2)
- Pas de support Windows (prévu v2)
- Pas de daemon Linux (prévu v2)
- Pas de gestion fine des conflits (le plus récent gagne)

---

## Licence

MIT
