use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

#[test]
fn cli_help_prints_program_name() {
    Command::cargo_bin("rl")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Redlight"));
}

#[test]
fn cli_version_prints_version() {
    Command::cargo_bin("rl")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn init_with_flags_creates_parseable_config() {
    let tmp = TempDir::new().unwrap();

    Command::cargo_bin("rl")
        .unwrap()
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["init", "--name", "tardis", "--description", "Test machine"])
        .assert()
        .success()
        .stdout(contains("Configuration written"));

    let cfg_dir = tmp.path().join("redlight");
    assert!(cfg_dir.join("devices.toml").exists());
    assert!(cfg_dir.join("items.toml").exists());
    assert!(cfg_dir.join("bindings.toml").exists());

    let devices = std::fs::read_to_string(cfg_dir.join("devices.toml")).unwrap();
    assert!(devices.contains("[tardis]"));
    assert!(devices.contains("type = \"host\""));
    assert!(devices.contains("description = \"Test machine\""));
    assert!(devices.contains("match.hostname"));
}

#[test]
fn init_refuses_to_overwrite_existing_config() {
    let tmp = TempDir::new().unwrap();
    let cfg_dir = tmp.path().join("redlight");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("devices.toml"), "[existing]\n").unwrap();

    Command::cargo_bin("rl")
        .unwrap()
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["init", "--name", "tardis"])
        .assert()
        .failure()
        .stderr(contains("Config already exists"));
}
