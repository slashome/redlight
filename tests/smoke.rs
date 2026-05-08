use assert_cmd::Command;
use predicates::str::contains;

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
