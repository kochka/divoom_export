//! End-to-end smoke test of the compiled binary: offline decode and exit codes.
mod common;

use std::process::Command;

#[test]
fn logout_removes_only_saved_credentials_without_authenticating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("divoom-export/credentials.toml");
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "broken credentials file").unwrap();
    let other = dir.path().join("divoom-export/config.toml");
    std::fs::write(&other, "keep me").unwrap();
    for expected in ["saved credentials removed", "no saved credentials"] {
        let out = Command::new(env!("CARGO_BIN_EXE_divoom-export"))
            .args(["auth", "logout"])
            .env("XDG_CONFIG_HOME", dir.path())
            .env("DIVOOM_PASSWORD_MD5", "invalid but unused")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        let output = String::from_utf8(out.stdout).unwrap();
        assert!(output.contains(expected));
        assert!(output.contains("environment credentials are still set"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep me");
    }
}

#[test]
fn auth_login_errors_do_not_save_credentials() {
    let dir = tempfile::tempdir().unwrap();
    for digest in [None, Some("nothex")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_divoom-export"));
        command
            .args(["auth", "login"])
            .env("XDG_CONFIG_HOME", dir.path())
            .env("DIVOOM_EMAIL", "a@b.c")
            .env_remove("DIVOOM_PASSWORD_MD5")
            .stdin(std::process::Stdio::null());
        if let Some(value) = digest {
            command.env("DIVOOM_PASSWORD_MD5", value);
        }
        let out = command.output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("DIVOOM_PASSWORD_MD5"));
        assert!(!err.contains("nothex"));
        assert!(!dir.path().join("divoom-export/credentials.toml").exists());
    }
}

#[test]
fn decode_command_writes_webp_and_missing_env_is_configuration_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("art.dat");
    let output = dir.path().join("art.webp");
    std::fs::write(&input, common::format9_fixture(4, 80, 7)).unwrap();
    let bin = env!("CARGO_BIN_EXE_divoom-export");

    let out = Command::new(bin)
        .args([
            "decode",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&output).unwrap();
    let info = divoom_export::export::inspect_webp(&bytes).unwrap();
    assert_eq!(info.frame_count, 4);

    // Unsupported format → exit 1 with a typed message.
    std::fs::write(&input, [42u8, 0, 0, 0]).unwrap();
    let out = Command::new(bin)
        .args([
            "decode",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unsupported Divoom format 42"));

    // Missing credentials without a TTY → configuration error, no network.
    let out = Command::new(bin)
        .args(["auth", "check"])
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("DIVOOM_EMAIL")
        .env_remove("DIVOOM_PASSWORD_MD5")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("DIVOOM_EMAIL"));

    // Bad hash format is rejected before any request.
    let out = Command::new(bin)
        .args(["auth", "check"])
        .env("XDG_CONFIG_HOME", dir.path())
        .env("DIVOOM_EMAIL", "a@b.c")
        .env("DIVOOM_PASSWORD_MD5", "nothex")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("32 hexadecimal"));
    assert!(!err.contains("nothex"));

    // Missing selector and --all → clap usage error (exit 2).
    let out = Command::new(bin)
        .args(["playlist", "sync"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
