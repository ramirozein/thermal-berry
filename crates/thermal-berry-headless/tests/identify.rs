#![cfg(target_os = "linux")]
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{fs, io::Write, os::unix::fs::DirBuilderExt, process::Command};
use thermal_berry_core::config::FanMode;
use thermal_berry_core::observation::Observation;

// A synthetic open-loop session used only in a temporary user data directory.
fn observations() -> Vec<Observation> {
    let (mut c, mut g) = (52.0, 48.0);
    (0..1350)
        .map(|k| {
            let lc = [12.0, 52.0, 86.0][k / 131 % 3];
            let lg = [84.0, 14.0, 54.0][k / 97 % 3];
            let f = [42.0, 82.0, 55.0, 70.0][k / 79 % 4];
            let row = Observation {
                timestamp_ms: 2000 * k as u64,
                cpu_temp_c: Some(c),
                gpu_temp_c: Some(g),
                cpu_percent: Some(lc),
                gpu_percent: Some(lg),
                fan_percent: Some(f),
                rpm: Some(2200),
                mode: Some(FanMode::Manual),
                mem_percent: None,
                gpu_mem_percent: None,
                gpu_power_w: None,
            };
            let dc = 0.62 - 0.014 * c + 0.003 * g + 0.0032 * lc + 0.0013 * lg - 0.00006 * f * c;
            let dg = 0.69 - 0.015 * g + 0.0025 * c + 0.0038 * lg + 0.001 * lc - 0.00007 * f * g;
            c += 2.0 * dc;
            g += 2.0 * dg;
            row
        })
        .collect()
}

#[test]
fn imported_sessions_are_report_only_and_sqlite_fits_preserve_accepted_versions() {
    let root = std::env::temp_dir().join(format!("thermal-berry-cli-fit-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let db_path = root.join("thermal-berry/thermal-berry.db");
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_thermal-berry-headless"))
            .args(args)
            .env("XDG_DATA_HOME", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .current_dir(&root)
            .output()
            .unwrap()
    };
    let data = observations();
    let mut input = fs::File::create(root.join("capture.jsonl")).unwrap();
    for row in &data {
        serde_json::to_writer(&mut input, row).unwrap();
        writeln!(input).unwrap();
    }
    drop(input);
    let imported = cli(&["identify", "--input", "capture.jsonl"]);
    assert!(
        imported.status.success(),
        "{:?}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let report: Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(report["scope"], "cpu+gpu");
    assert_eq!(report["accepted"], true);
    assert!(!db_path.exists());

    let range = ["identify", "--from", "0", "--to", "2698000"];
    assert_eq!(cli(&range).status.code(), Some(1));
    let mut db = Connection::open(&db_path).unwrap();
    let tx = db.transaction().unwrap();
    for row in data {
        tx.execute("INSERT INTO load_readings (timestamp_ms,cpu_temp_c,gpu_temp_c,cpu_percent,gpu_percent,fan_percent,rpm,mode)
            VALUES (?1,?2,?3,?4,?5,?6,?7,'manual')", params![
                i64::try_from(row.timestamp_ms).unwrap(), row.cpu_temp_c, row.gpu_temp_c,
                row.cpu_percent, row.gpu_percent, row.fan_percent, row.rpm
            ]).unwrap();
    }
    tx.commit().unwrap();
    let fit = cli(&range);
    assert!(
        fit.status.success(),
        "{:?}",
        String::from_utf8_lossy(&fit.stderr)
    );
    let rows = || {
        db.query_row("SELECT COUNT(*), COUNT(DISTINCT die), COUNT(DISTINCT fitted_at_ms) FROM thermal_models", [],
        |r| Ok((r.get::<_,u32>(0)?, r.get::<_,u32>(1)?, r.get::<_,u32>(2)?))).unwrap()
    };
    assert_eq!(rows(), (2, 2, 1));
    db.execute(
        "CREATE TRIGGER refuse_gpu BEFORE INSERT ON thermal_models WHEN NEW.die = 'gpu'
        BEGIN SELECT RAISE(ABORT, 'simulated disk error'); END",
        [],
    )
    .unwrap();
    assert_eq!(cli(&range).status.code(), Some(1));
    assert_eq!(rows(), (2, 2, 1));
    db.execute("UPDATE load_readings SET fan_percent = 40", [])
        .unwrap();
    assert_eq!(cli(&range).status.code(), Some(1));
    assert_eq!(rows(), (2, 2, 1));
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
