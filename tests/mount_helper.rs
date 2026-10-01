use std::{ffi::CStr, process::Command};

fn current_user() -> String {
    let entry = unsafe { libc::getpwuid(libc::geteuid()) };
    assert!(!entry.is_null());
    unsafe { CStr::from_ptr((*entry).pw_name) }
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn daemon_reports_missing_token_without_mounting() {
    let directory = tempfile::tempdir().unwrap();
    let options = format!("token_file={}/missing-token", directory.path().display());
    let output = Command::new(env!("CARGO_BIN_EXE_mount-myboxfs"))
        .args([
            current_user().as_str(),
            "/nonexistent/mybox-mount",
            "-o",
            &options,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no token for the selected user"));
}

#[test]
fn foreground_reports_tracing_diagnostics_without_exposing_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let options = format!(
        "foreground,token_file={}/missing-token",
        directory.path().display()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_mount-myboxfs"))
        .env("RUST_LOG", "info")
        .args([
            current_user().as_str(),
            "/nonexistent/mybox-mount",
            "-o",
            &options,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("MYBOX mount helper failed"));
    assert!(stderr.contains("no token for the selected user"));
}

#[test]
fn fake_mount_does_not_read_credentials_or_require_fuse() {
    let output = Command::new(env!("CARGO_BIN_EXE_mount-myboxfs"))
        .args([
            current_user().as_str(),
            "/nonexistent/mybox-mount",
            "-f",
            "-o",
            "_netdev,ro",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_inline_credentials_without_printing_them() {
    let output = Command::new(env!("CARGO_BIN_EXE_mount-myboxfs"))
        .args([
            current_user().as_str(),
            "/mnt/mybox",
            "-o",
            "token=do-not-log-this",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("do-not-log-this"));
}
