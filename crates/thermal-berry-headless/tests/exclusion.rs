#![cfg(target_os = "linux")]
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thermal_berry_core::lock::InstanceLock;

#[test]
fn gui_lock_holder() {
    let Some(ready) = std::env::var_os("THERMAL_BERRY_TEST_LOCK_READY") else {
        return;
    };
    let _instance = InstanceLock::acquire("gui").unwrap();
    fs::write(ready, b"ready").unwrap();
    let _ = std::io::stdin().read_exact(&mut [0]);
}

#[test]
fn competing_process_exits_two_before_sqlite_and_a_crash_releases_the_lock() {
    let root = std::env::temp_dir().join(format!("thermal-berry-exclusion-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let ready = root.join("ready");
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "gui_lock_holder"])
        .env("XDG_DATA_HOME", &root)
        .env("THERMAL_BERRY_TEST_LOCK_READY", &ready)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let launch = || {
        Command::new(env!("CARGO_BIN_EXE_thermal-berry-headless"))
            .env("XDG_DATA_HOME", &root)
            .args(["identify", "--input"])
            .arg(PathBuf::from(&root).join("does-not-exist"))
            .output()
            .unwrap()
    };
    let second = launch();
    assert_eq!(second.status.code(), Some(2));
    let message = String::from_utf8(second.stderr).unwrap();
    assert!(
        message.contains(&format!("pid {}, modo gui", holder.id())),
        "{message}"
    );
    assert!(!root.join("thermal-berry/thermal-berry.db").exists());
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert_eq!(launch().status.code(), Some(1));
    fs::remove_dir_all(root).unwrap();
}
