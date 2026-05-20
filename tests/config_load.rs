use std::path::Path;

use redlight::config::{Bridge, Config, DeviceType, ItemKind, Role};

#[test]
fn loads_valid_fixture() {
    let dir = Path::new("tests/fixtures/config-valid");
    let cfg = Config::load(dir).expect("config should load");

    assert_eq!(cfg.devices.len(), 3);

    let tardis = &cfg.devices["tardis"];
    assert_eq!(tardis.device_type, DeviceType::Host);
    assert_eq!(tardis.bridge, Bridge::Fs);
    assert_eq!(
        tardis.description.as_deref(),
        Some("MacBook Pro M2 Max 64GB")
    );

    let jarvis = &cfg.devices["jarvis"];
    assert_eq!(jarvis.device_type, DeviceType::Phone);
    assert_eq!(jarvis.bridge, Bridge::Mtp);
    assert_eq!(jarvis.description.as_deref(), Some("Fairphone 6"));
    assert_eq!(jarvis.matcher.vendor_id.as_deref(), Some("18d1"));
    assert_eq!(jarvis.matcher.serial.as_deref(), Some("ABC123"));

    let materia = &cfg.devices["materia"];
    assert_eq!(materia.device_type, DeviceType::Drive);
    assert_eq!(materia.description.as_deref(), Some("SSD Playstation"));
    assert_eq!(materia.matcher.volume_label.as_deref(), Some("MATERIA"));

    assert_eq!(cfg.items.len(), 2);
    let music = &cfg.items["music"];
    assert_eq!(music.kind, ItemKind::Folder);
    assert_eq!(music.category.as_deref(), Some("audio"));
    assert_eq!(music.include, vec!["**/*.mp3", "**/*.flac"]);
    assert_eq!(music.exclude, vec!["**/draft-*"]);

    assert_eq!(cfg.bindings.len(), 3);
    assert_eq!(cfg.bindings.iter().filter(|b| b.item == "music").count(), 3);
    let to_jarvis = cfg
        .bindings
        .iter()
        .find(|b| b.device == "jarvis")
        .expect("a binding to jarvis");
    assert_eq!(to_jarvis.role, Role::ReadOnly);
}

#[test]
fn loads_multi_host_fixture() {
    let dir = Path::new("tests/fixtures/config-multi-host");
    let cfg = Config::load(dir).expect("multi-host config should load");

    // 3 devices: 2 hosts + 1 drive.
    assert_eq!(cfg.devices.len(), 3);
    let hosts: Vec<_> = cfg
        .devices
        .values()
        .filter(|d| d.device_type == DeviceType::Host)
        .collect();
    assert_eq!(hosts.len(), 2);

    let tardis = &cfg.devices["tardis"];
    assert_eq!(tardis.device_type, DeviceType::Host);
    assert_eq!(tardis.matcher.hostname.as_deref(), Some("tardis.local"));

    let hal9000 = &cfg.devices["hal9000"];
    assert_eq!(hal9000.device_type, DeviceType::Host);
    assert_eq!(
        hal9000.matcher.hostname.as_deref(),
        Some("hal9000.work.local")
    );

    // music is bound on all three (both hosts + materia).
    assert_eq!(cfg.bindings.iter().filter(|b| b.item == "music").count(), 3);
}
