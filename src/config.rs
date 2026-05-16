//! Config loading: devices, items, bindings.
//!
//! Three TOML files live in `~/.config/redlight/` (or `$XDG_CONFIG_HOME/redlight/`):
//! - `devices.toml`  — known peripherals
//! - `items.toml`    — synced units (folders/files) with include/exclude filters
//! - `bindings.toml` — pairs (item × device) with path + role

use std::collections::HashMap;
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Bridge {
    Fs,
    Mtp,
    Adb,
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
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub category: Option<String>,
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
    pub devices: HashMap<String, Device>,
    pub items: HashMap<String, Item>,
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

    fn validate(&self) -> Result<()> {
        for b in &self.bindings {
            if !self.devices.contains_key(&b.device) {
                bail!(
                    "binding for item '{}' references unknown device '{}'",
                    b.item,
                    b.device
                );
            }
            if !self.items.contains_key(&b.item) {
                bail!(
                    "binding on device '{}' references unknown item '{}'",
                    b.device,
                    b.item
                );
            }
        }
        Ok(())
    }
}

fn load_devices(path: &Path) -> Result<HashMap<String, Device>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: HashMap<String, DeviceRaw> =
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

fn load_items(path: &Path) -> Result<HashMap<String, Item>> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: HashMap<String, ItemRaw> =
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(raw
        .into_iter()
        .map(|(name, r)| {
            let item = Item {
                name: name.clone(),
                kind: r.kind,
                category: r.category,
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
        let raw: HashMap<String, DeviceRaw> = toml::from_str(s).unwrap();
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
include = ["**/*.mp3"]
exclude = ["**/draft-*"]

[contrat]
kind = "file"
"#;
        let raw: HashMap<String, ItemRaw> = toml::from_str(s).unwrap();
        assert_eq!(raw["music"].kind, ItemKind::Folder);
        assert_eq!(raw["music"].include, vec!["**/*.mp3".to_string()]);
        assert_eq!(raw["contrat"].kind, ItemKind::File);
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
        let r = toml::from_str::<HashMap<String, DeviceRaw>>(s);
        assert!(r.is_err());
    }

    #[test]
    fn reject_unknown_bridge() {
        let s = r#"
[foo]
type = "host"
bridge = "smb"
"#;
        let r = toml::from_str::<HashMap<String, DeviceRaw>>(s);
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

    fn make_item(name: &str) -> Item {
        Item {
            name: name.into(),
            kind: ItemKind::Folder,
            category: None,
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

    #[test]
    fn validation_rejects_binding_to_unknown_device() {
        let cfg = Config {
            devices: HashMap::new(),
            items: HashMap::from([("music".into(), make_item("music"))]),
            bindings: vec![Binding {
                item: "music".into(),
                device: "ghost".into(),
                path: None,
                role: Role::ReadWrite,
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("unknown device 'ghost'"));
    }

    #[test]
    fn validation_rejects_binding_to_unknown_item() {
        let cfg = Config {
            devices: HashMap::from([("tardis".into(), make_device("tardis", DeviceType::Host))]),
            items: HashMap::new(),
            bindings: vec![Binding {
                item: "ghost".into(),
                device: "tardis".into(),
                path: None,
                role: Role::ReadWrite,
            }],
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("unknown item 'ghost'"));
    }

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
        let device = make_device("jarvis", DeviceType::Phone);
        let binding = Binding {
            item: "music".into(),
            device: "jarvis".into(),
            path: Some("/Music".into()),
            role: Role::ReadOnly,
        };
        assert_eq!(binding.resolved_path(&device), PathBuf::from("/Music"));
    }

    #[test]
    fn binding_resolved_path_uses_phone_default() {
        let device = make_device("jarvis", DeviceType::Phone);
        let binding = Binding {
            item: "music".into(),
            device: "jarvis".into(),
            path: None,
            role: Role::ReadOnly,
        };
        assert_eq!(
            binding.resolved_path(&device),
            PathBuf::from("/storage/emulated/0/")
        );
    }

    #[test]
    fn binding_resolved_path_uses_drive_default() {
        let device = make_device("materia", DeviceType::Drive);
        let binding = Binding {
            item: "music".into(),
            device: "materia".into(),
            path: None,
            role: Role::ReadWrite,
        };
        assert_eq!(binding.resolved_path(&device), PathBuf::from("/"));
    }
}
