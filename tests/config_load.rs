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

    let jarvis = &cfg.devices["jarvis"];
    assert_eq!(jarvis.device_type, DeviceType::Phone);
    assert_eq!(jarvis.bridge, Bridge::Mtp);
    assert_eq!(jarvis.matcher.vendor_id.as_deref(), Some("18d1"));
    assert_eq!(jarvis.matcher.serial.as_deref(), Some("ABC123"));

    let materia = &cfg.devices["materia"];
    assert_eq!(materia.device_type, DeviceType::Drive);
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
