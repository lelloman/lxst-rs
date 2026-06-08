use std::process::Command;

#[test]
fn version_flag_prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_rnphone"))
        .arg("--version")
        .output()
        .expect("run rnphone --version");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("rnphone "));
}

#[test]
fn systemd_flag_prints_unit() {
    let output = Command::new(env!("CARGO_BIN_EXE_rnphone"))
        .arg("--systemd")
        .output()
        .expect("run rnphone --systemd");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[Unit]"));
    assert!(stdout.contains("Reticulum Telephone Service"));
}

#[test]
fn list_devices_uses_real_enumerator() {
    let output = Command::new(env!("CARGO_BIN_EXE_rnphone"))
        .arg("--list-devices")
        .output()
        .expect("run rnphone --list-devices");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Available audio devices:"));
    assert!(!stdout.contains("not wired yet"));
}
