//! Config loading: devices, items, bindings.
//!
//! Three TOML files live in `~/.config/redlight/` (or `$XDG_CONFIG_HOME/redlight/`):
//! - `devices.toml`  — known peripherals
//! - `items.toml`    — synced units (folders/files) with include/exclude filters
//! - `bindings.toml` — pairs (item × device) with path + role

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

// ============================================================================
// Devices
// ============================================================================

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Host,
    Phone,
    Drive,
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeviceType::Host => "host",
            DeviceType::Phone => "phone",
            DeviceType::Drive => "drive",
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Bridge {
    Fs,
    Mtp,
    Adb,
}

impl fmt::Display for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Bridge::Fs => "fs",
            Bridge::Mtp => "mtp",
            Bridge::Adb => "adb",
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct DeviceMatch {
    #[serde(default)]
    pub vendor_id: Option<String>,
    #[serde(default)]
    pub product_id: Option<String>,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub volume_label: Option<String>,
    #[serde(default)]
    pub volume_uuid: Option<String>,
    /// Only meaningful for `type = "host"`. Lets a config that declares
    /// multiple hosts identify which one is the current machine (matched
    /// against `gethostname()` at runtime).
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct DeviceRaw {
    #[serde(rename = "type")]
    device_type: DeviceType,
    bridge: Bridge,
    #[serde(default, rename = "match")]
    matcher: DeviceMatch,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub device_type: DeviceType,
    pub bridge: Bridge,
    pub matcher: DeviceMatch,
    pub description: Option<String>,
}

// ============================================================================
// Items
// ============================================================================

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Folder,
    File,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ItemRaw {
    kind: ItemKind,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub category: Option<String>,
    pub description: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

// ============================================================================
// Bindings
// ============================================================================

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    ReadWrite,
    ReadOnly,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub item: String,
    pub device: String,
    #[serde(default)]
    pub path: Option<String>,
    pub role: Role,
    /// Extra include patterns specific to this device. Combined by
    /// intersection with the item's `include`: a file passes only if
    /// it matches both. Empty = no binding-level narrowing.
    #[serde(default)]
    pub include: Vec<String>,
    /// Extra exclude patterns specific to this device. Combined by
    /// union with the item's `exclude` and the system excludes: any
    /// match blocks the file. Empty = no binding-level additions.
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BindingsFile {
    #[serde(default)]
    binding: Vec<Binding>,
}

// ============================================================================
// Config (the three files together)
// ============================================================================

#[derive(Debug, Clone)]
pub struct Config {
    pub devices: BTreeMap<String, Device>,
    pub items: BTreeMap<String, Item>,
    pub bindings: Vec<Binding>,
}

impl Config {
    /// Load and validate the config from a directory containing the three TOML files.
    pub fn load(dir: &Path) -> Result<Self> {
        let devices = load_devices(&dir.join("devices.toml"))?;
        let items = load_items(&dir.join("items.toml"))?;
        let bindings = load_bindings(&dir.join("bindings.toml"))?;
        let config = Self {
            devices,
            items,
            bindings,
        };
        config.validate()?;
        Ok(config)
    }

    /// Collect *every* validation error and report them together. The user
    /// shouldn't have to fix one, re-run, fix the next, etc.
    fn validate(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        // 1. Bridge ↔ DeviceType compatibility.
        for (name, d) in &self.devices {
            if !bridge_allowed(d.device_type, d.bridge) {
                errors.push(format!(
                    "device '{}': bridge '{}' is incompatible with type '{}'",
                    name, d.bridge, d.device_type
                ));
            }
        }

        // 2. Each device type has its own identification requirements.
        for (name, d) in &self.devices {
            match d.device_type {
                DeviceType::Host => {
                    // The host is "us": no matcher needed.
                }
                DeviceType::Phone => {
                    if d.matcher.serial.is_none() {
                        errors.push(format!(
                            "device '{}' (phone): match.serial is required to identify the device",
                            name
                        ));
                    }
                }
                DeviceType::Drive => {
                    if d.matcher.volume_label.is_none() && d.matcher.volume_uuid.is_none() {
                        errors.push(format!(
                            "device '{}' (drive): one of match.volume_label or match.volume_uuid is required",
                            name
                        ));
                    }
                }
            }
        }

        // 3. Bindings: cross-references + duplicate (item, device) pairs.
        let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
        for b in &self.bindings {
            if !self.devices.contains_key(&b.device) {
                errors.push(format!(
                    "binding for item '{}' references unknown device '{}'",
                    b.item, b.device
                ));
            }
            if !self.items.contains_key(&b.item) {
                errors.push(format!(
                    "binding on device '{}' references unknown item '{}'",
                    b.device, b.item
                ));
            }
            if !seen.insert((b.item.as_str(), b.device.as_str())) {
                errors.push(format!(
                    "duplicate binding: item '{}' on device '{}' declared more than once",
                    b.item, b.device
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("config validation failed:\n  - {}", errors.join("\n  - "));
        }
    }
}

const fn bridge_allowed(t: DeviceType, b: Bridge) -> bool {
    matches!(
        (t, b),
        (DeviceType::Host, Bridge::Fs)
            | (DeviceType::Drive, Bridge::Fs)
            | (DeviceType::Phone, Bridge::Mtp)
            | (DeviceType::Phone, Bridge::Adb)
    )
}

fn load_devices(path: &Path) -> Result<BTreeMap<String, Device>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: BTreeMap<String, DeviceRaw> =
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(raw
        .into_iter()
        .map(|(name, r)| {
            let device = Device {
                name: name.clone(),
                device_type: r.device_type,
                bridge: r.bridge,
                matcher: r.matcher,
                description: r.description,
            };
            (name, device)
        })
        .collect())
}

fn load_items(path: &Path) -> Result<BTreeMap<String, Item>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: BTreeMap<String, ItemRaw> =
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(raw
        .into_iter()
        .map(|(name, r)| {
            let item = Item {
                name: name.clone(),
                kind: r.kind,
                category: r.category,
                description: r.description,
                include: r.include,
                exclude: r.exclude,
            };
            (name, item)
        })
        .collect())
}

fn load_bindings(path: &Path) -> Result<Vec<Binding>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let f: BindingsFile =
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(f.binding)
}

// ============================================================================
// Path resolution
// ============================================================================

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("redlight")
    } else {
        home_dir()
            .map(|h| h.join(".config").join("redlight"))
            .unwrap_or_else(|| PathBuf::from(".config/redlight"))
    }
}

/// Where Redlight stores host-side mutable state (sync log, daemon log,
/// lockfile). Mirrors `config_dir()` semantics with `$XDG_STATE_HOME`.
pub fn state_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        PathBuf::from(xdg).join("redlight")
    } else {
        home_dir()
            .map(|h| h.join(".local").join("state").join("redlight"))
            .unwrap_or_else(|| PathBuf::from(".local/state/redlight"))
    }
}

pub fn expand_path(s: &str) -> PathBuf {
    expand_path_with(s, home_dir())
}

fn expand_path_with(s: &str, home: Option<PathBuf>) -> PathBuf {
    if s == "~" {
        return home.unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

const fn default_path_for(t: DeviceType) -> &'static str {
    match t {
        DeviceType::Host => "~",
        DeviceType::Phone => "/storage/emulated/0/",
        DeviceType::Drive => "/",
    }
}

impl Binding {
    /// The absolute path on `device` for this binding.
    /// Falls back to per-device-type default when `path` is omitted.
    pub fn resolved_path(&self, device: &Device) -> PathBuf {
        let raw = self
            .path
            .as_deref()
            .unwrap_or_else(|| default_path_for(device.device_type));
        expand_path(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    fn make_item(name: &str) -> Item {
        Item {
            name: name.into(),
            kind: ItemKind::Folder,
            category: None,
            description: None,
            include: vec![],
            exclude: vec![],
        }
    }

    fn make_device(name: &str, t: DeviceType) -> Device {
        Device {
            name: name.into(),
            device_type: t,
            bridge: Bridge::Fs,
            matcher: DeviceMatch::default(),
            description: None,
        }
    }

    fn make_phone(name: &str) -> Device {
        Device {
            name: name.into(),
            device_type: DeviceType::Phone,
            bridge: Bridge::Mtp,
            matcher: DeviceMatch {
                serial: Some("SN-0000".into()),
                ..Default::default()
            },
            description: None,
        }
    }

    fn make_drive(name: &str) -> Device {
        Device {
            name: name.into(),
            device_type: DeviceType::Drive,
            bridge: Bridge::Fs,
            matcher: DeviceMatch {
                volume_label: Some("LABEL".into()),
                ..Default::default()
            },
            description: None,
        }
    }

    fn valid_config() -> Config {
        Config {
            devices: BTreeMap::from([
                ("tardis".into(), make_device("tardis", DeviceType::Host)),
                ("jarvis".into(), make_phone("jarvis")),
                ("materia".into(), make_drive("materia")),
            ]),
            items: BTreeMap::from([("music".into(), make_item("music"))]),
            bindings: vec![Binding {
                item: "music".into(),
                device: "tardis".into(),
                path: Some("~/Music".into()),
                role: Role::ReadWrite,
                include: vec![],
                exclude: vec![],
            }],
        }
    }

    // ---- TOML parsing ----

    #[test]
    fn parse_devices_toml() {
        let s = r#"
[tardis]
type = "host"
bridge = "fs"
description = "MacBook Pro M2 Max 64GB"

[jarvis]
type = "phone"
bridge = "mtp"
match.vendor_id = "18d1"
match.product_id = "4ee7"
match.serial = "ABC123"
"#;
        let raw: BTreeMap<String, DeviceRaw> = toml::from_str(s).unwrap();
        assert_eq!(raw["tardis"].device_type, DeviceType::Host);
        assert_eq!(raw["tardis"].bridge, Bridge::Fs);
        assert_eq!(
            raw["tardis"].description.as_deref(),
            Some("MacBook Pro M2 Max 64GB")
        );
        assert_eq!(raw["jarvis"].description, None);
        assert_eq!(raw["jarvis"].matcher.vendor_id.as_deref(), Some("18d1"));
        assert_eq!(raw["jarvis"].matcher.serial.as_deref(), Some("ABC123"));
    }

    #[test]
    fn parse_items_toml() {
        let s = r#"
[music]
kind = "folder"
category = "audio"
description = "My FLAC collection sorted by artist"
include = ["**/*.mp3"]
exclude = ["**/draft-*"]

[contrat]
kind = "file"
"#;
        let raw: BTreeMap<String, ItemRaw> = toml::from_str(s).unwrap();
        assert_eq!(raw["music"].kind, ItemKind::Folder);
        assert_eq!(
            raw["music"].description.as_deref(),
            Some("My FLAC collection sorted by artist")
        );
        assert_eq!(raw["music"].include, vec!["**/*.mp3".to_string()]);
        assert_eq!(raw["contrat"].kind, ItemKind::File);
        assert_eq!(raw["contrat"].description, None);
        assert!(raw["contrat"].include.is_empty());
    }

    #[test]
    fn parse_bindings_toml() {
        let s = r#"
[[binding]]
item = "music"
device = "tardis"
path = "~/Music"
role = "read_write"

[[binding]]
item = "music"
device = "jarvis"
role = "read_only"
"#;
        let f: BindingsFile = toml::from_str(s).unwrap();
        assert_eq!(f.binding.len(), 2);
        assert_eq!(f.binding[0].role, Role::ReadWrite);
        assert_eq!(f.binding[1].path, None);
    }

    #[test]
    fn reject_unknown_device_type() {
        let s = r#"
[foo]
type = "alien"
bridge = "fs"
"#;
        let r = toml::from_str::<BTreeMap<String, DeviceRaw>>(s);
        assert!(r.is_err());
    }

    #[test]
    fn reject_unknown_bridge() {
        let s = r#"
[foo]
type = "host"
bridge = "smb"
"#;
        let r = toml::from_str::<BTreeMap<String, DeviceRaw>>(s);
        assert!(r.is_err());
    }

    #[test]
    fn reject_unknown_role() {
        let s = r#"
[[binding]]
item = "x"
device = "y"
role = "owner"
"#;
        let r = toml::from_str::<BindingsFile>(s);
        assert!(r.is_err());
    }

    // ---- cross-references ----

    #[test]
    fn baseline_valid_config_passes() {
        valid_config().validate().unwrap();
    }

    #[test]
    fn validation_rejects_binding_to_unknown_device() {
        let mut cfg = valid_config();
        cfg.bindings.push(Binding {
            item: "music".into(),
            device: "ghost".into(),
            path: None,
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("unknown device 'ghost'"));
    }

    #[test]
    fn validation_rejects_binding_to_unknown_item() {
        let mut cfg = valid_config();
        cfg.bindings.push(Binding {
            item: "ghost".into(),
            device: "tardis".into(),
            path: None,
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("unknown item 'ghost'"));
    }

    // ---- bridge ↔ type compatibility ----

    #[test]
    fn reject_mtp_on_host() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("tardis").unwrap().bridge = Bridge::Mtp;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("bridge 'mtp'") && err.contains("type 'host'"));
    }

    #[test]
    fn reject_fs_on_phone() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("jarvis").unwrap().bridge = Bridge::Fs;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("bridge 'fs'") && err.contains("type 'phone'"));
    }

    #[test]
    fn reject_adb_on_drive() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("materia").unwrap().bridge = Bridge::Adb;
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("bridge 'adb'") && err.contains("type 'drive'"));
    }

    #[test]
    fn accept_adb_on_phone() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("jarvis").unwrap().bridge = Bridge::Adb;
        cfg.validate().unwrap();
    }

    // ---- matcher requirements ----

    #[test]
    fn reject_phone_without_serial() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("jarvis").unwrap().matcher = DeviceMatch::default();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("match.serial is required"));
    }

    #[test]
    fn reject_drive_without_label_or_uuid() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("materia").unwrap().matcher = DeviceMatch::default();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("volume_label") && err.contains("volume_uuid"));
    }

    #[test]
    fn accept_drive_with_only_uuid() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("materia").unwrap().matcher = DeviceMatch {
            volume_uuid: Some("1234-ABCD".into()),
            ..Default::default()
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn host_needs_no_matcher() {
        let cfg = valid_config(); // tardis has no matcher set
        cfg.validate().unwrap();
    }

    // ---- binding to device root (legitimate "sync the whole device" case) ----

    #[test]
    fn accept_drive_binding_to_root() {
        // The "USB key fully synced" use case: drive bound to "/" on
        // itself maps to a subfolder on other devices. System excludes
        // (in sync::diff) keep `.redlight/` out of the picture.
        let mut cfg = valid_config();
        cfg.bindings.push(Binding {
            item: "music".into(),
            device: "materia".into(),
            path: Some("/".into()),
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        });
        cfg.validate().unwrap();
    }

    #[test]
    fn accept_drive_binding_default_path() {
        // Defaulting to the device root is fine too.
        let mut cfg = valid_config();
        cfg.bindings.push(Binding {
            item: "music".into(),
            device: "materia".into(),
            path: None,
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        });
        cfg.validate().unwrap();
    }

    // ---- duplicate bindings ----

    #[test]
    fn reject_duplicate_binding() {
        let mut cfg = valid_config();
        cfg.bindings.push(Binding {
            item: "music".into(),
            device: "tardis".into(),
            path: Some("~/Other".into()),
            role: Role::ReadOnly,
            include: vec![],
            exclude: vec![],
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("duplicate binding") && err.contains("music") && err.contains("tardis")
        );
    }

    // ---- multi-error collection ----

    #[test]
    fn reports_all_errors_at_once() {
        let mut cfg = valid_config();
        cfg.devices.get_mut("jarvis").unwrap().bridge = Bridge::Fs; // bridge mismatch
        cfg.devices.get_mut("materia").unwrap().matcher = DeviceMatch::default(); // missing matcher
        cfg.bindings.push(Binding {
            item: "ghost".into(),
            device: "tardis".into(),
            path: None,
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        }); // unknown item

        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("bridge 'fs'") && err.contains("type 'phone'"));
        assert!(err.contains("volume_label"));
        assert!(err.contains("unknown item 'ghost'"));
        // Header line + bullets count should be 3 issues.
        assert_eq!(err.matches("\n  - ").count(), 3);
    }

    // ---- path resolution ----

    #[test]
    fn expand_tilde_slash() {
        let p = expand_path_with("~/Music", Some(PathBuf::from("/test/home")));
        assert_eq!(p, PathBuf::from("/test/home/Music"));
    }

    #[test]
    fn expand_bare_tilde() {
        let p = expand_path_with("~", Some(PathBuf::from("/test/home")));
        assert_eq!(p, PathBuf::from("/test/home"));
    }

    #[test]
    fn expand_no_tilde_unchanged() {
        let p = expand_path_with("/foo/bar", Some(PathBuf::from("/test/home")));
        assert_eq!(p, PathBuf::from("/foo/bar"));
    }

    #[test]
    fn expand_without_home_keeps_tilde() {
        let p = expand_path_with("~/Music", None);
        assert_eq!(p, PathBuf::from("~/Music"));
    }

    #[test]
    fn default_path_per_type() {
        assert_eq!(default_path_for(DeviceType::Host), "~");
        assert_eq!(default_path_for(DeviceType::Phone), "/storage/emulated/0/");
        assert_eq!(default_path_for(DeviceType::Drive), "/");
    }

    #[test]
    fn binding_resolved_path_uses_explicit_when_present() {
        let device = make_phone("jarvis");
        let binding = Binding {
            item: "music".into(),
            device: "jarvis".into(),
            path: Some("/Music".into()),
            role: Role::ReadOnly,
            include: vec![],
            exclude: vec![],
        };
        assert_eq!(binding.resolved_path(&device), PathBuf::from("/Music"));
    }

    #[test]
    fn binding_resolved_path_uses_phone_default() {
        let device = make_phone("jarvis");
        let binding = Binding {
            item: "music".into(),
            device: "jarvis".into(),
            path: None,
            role: Role::ReadOnly,
            include: vec![],
            exclude: vec![],
        };
        assert_eq!(
            binding.resolved_path(&device),
            PathBuf::from("/storage/emulated/0/")
        );
    }

    #[test]
    fn binding_resolved_path_uses_drive_default() {
        let device = make_drive("materia");
        let binding = Binding {
            item: "music".into(),
            device: "materia".into(),
            path: None,
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        };
        assert_eq!(binding.resolved_path(&device), PathBuf::from("/"));
    }
}
