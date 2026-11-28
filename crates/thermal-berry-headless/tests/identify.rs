#![cfg(target_os = "linux")]
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{fs, io::Write, os::unix::fs::DirBuilderExt, process::Command};
use thermal_berry_core::config::FanMode;
use thermal_berry_core::observation::{FanObservation, Observation};

// A synthetic open-loop session used only in a temporary user data directory.
fn observations(dual: bool) -> Vec<Observation> {
    let (mut c, mut g) = (52.0, 48.0);
    (0..1350)
        .map(|k| {
            let lc = [12.0, 52.0, 86.0][k / 131 % 3];
            let lg = [84.0, 14.0, 54.0][k / 97 % 3];
            let f = [42.0, 82.0, 55.0, 70.0][k / 79 % 4];
            let f2 = [70.0, 40.0, 80.0, 55.0][k / 103 % 4];
            let mut row = Observation {
                timestamp_ms: 2000 * k as u64,
                cpu_temp_c: Some(c),
                gpu_temp_c: Some(g),
                cpu_percent: Some(lc),
                gpu_percent: Some(lg),
                fan_percent: Some(f),
                rpm: Some(2200),
                fans: vec![],
                mode: Some(FanMode::Manual),
                mem_percent: None,
                gpu_mem_percent: None,
                gpu_power_w: None,
            };
            let cooling = if dual {
                row.fan_percent = None;
                row.rpm = None;
                row.fans = vec![
                    FanObservation {
                        id: "right".into(),
                        percent: Some(f2),
                        rpm: Some(2100),
                    },
                    FanObservation {
                        id: "left".into(),
                        percent: Some(f),
                        rpm: Some(2300),
                    },
                ];
                (0.000045 * f + 0.000025 * f2, 0.000025 * f + 0.000055 * f2)
            } else {
                (0.00006 * f, 0.00007 * f)
            };
            let dc = 0.62 - 0.014 * c + 0.003 * g + 0.0032 * lc + 0.0013 * lg - cooling.0 * c;
            let dg = 0.69 - 0.015 * g + 0.0025 * c + 0.0038 * lg + 0.001 * lc - cooling.1 * g;
            c += 2.0 * dc;
            g += 2.0 * dg;
            row
        })
        .collect()
}

#[test]
fn legacy_jsonl_and_sqlite_fits_preserve_accepted_versions() {
    check_fit(false);
}

#[test]
fn dual_jsonl_and_sqlite_fits_preserve_channel_identity_and_accepted_versions() {
    check_fit(true);
}

fn check_fit(dual: bool) {
    let root = std::env::temp_dir().join(format!(
        "thermal-berry-cli-fit-{}-{dual}",
        std::process::id()
    ));
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
    let data = observations(dual);
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
    assert_eq!(
        report["model"]["cpu"].as_array().unwrap().len(),
        if dual { 21 } else { 17 }
    );
    if dual {
        assert_eq!(
            report["model"]["fan_ids"],
            serde_json::json!(["left", "right"])
        );
    } else {
        assert!(report["model"].get("fan_ids").is_none());
    }
    assert!(!db_path.exists());

    let range = ["identify", "--from", "0", "--to", "2698000"];
    assert_eq!(cli(&range).status.code(), Some(1));
    let mut db = Connection::open(&db_path).unwrap();
    let tx = db.transaction().unwrap();
    for row in data {
        let timestamp = i64::try_from(row.timestamp_ms).unwrap();
        tx.execute("INSERT INTO load_readings (timestamp_ms,cpu_temp_c,gpu_temp_c,cpu_percent,gpu_percent,fan_percent,rpm,mode)
            VALUES (?1,?2,?3,?4,?5,?6,?7,'manual')", params![
                timestamp, row.cpu_temp_c, row.gpu_temp_c,
                row.cpu_percent, row.gpu_percent, row.fan_percent, row.rpm
            ]).unwrap();
        for fan in row.fans {
            tx.execute("INSERT INTO fan_readings (timestamp_ms,fan_id,label,rpm,boost_percent) VALUES (?1,?2,'System fan',?3,?4)",
                params![timestamp,fan.id,fan.rpm,fan.percent]).unwrap();
        }
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
    let saved: Value = serde_json::from_slice(&fit.stdout).unwrap();
    assert_eq!(saved["accepted"], true);
    assert_eq!(saved["terms"], report["terms"]);
    assert_eq!(saved["model"]["fan_ids"], report["model"]["fan_ids"]);
    for die in ["cpu", "gpu"] {
        let stored = saved["model"][die].as_array().unwrap();
        let imported = report["model"][die].as_array().unwrap();
        assert_eq!(stored.len(), imported.len());
        for (a, b) in stored.iter().zip(imported) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-10);
        }
    }
    let mappings = || {
        db.prepare("SELECT channel, fan_id FROM thermal_model_fans ORDER BY channel")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, u8>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let expected = if dual {
        vec![(1, "left".to_string()), (2, "right".to_string())]
    } else {
        vec![]
    };
    assert_eq!(mappings(), expected);
    if dual {
        let terms: String = db
            .query_row(
                "SELECT terms_json FROM thermal_models WHERE die='gpu'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let names: Vec<String> = serde_json::from_str(&terms).unwrap();
        assert!(names.contains(&"F1*T_gpu".into()) && names.contains(&"F2*T_gpu".into()));
        db.execute_batch(
            "CREATE TRIGGER refuse_mapping BEFORE INSERT ON thermal_model_fans WHEN NEW.channel=2
                          BEGIN SELECT RAISE(ABORT, 'simulated mapping failure'); END",
        )
        .unwrap();
        assert_eq!(cli(&range).status.code(), Some(1));
        assert_eq!(rows(), (2, 2, 1));
        assert_eq!(mappings(), expected);
        db.execute("DROP TRIGGER refuse_mapping", []).unwrap();
    }
    db.execute(
        "CREATE TRIGGER refuse_gpu BEFORE INSERT ON thermal_models WHEN NEW.die = 'gpu'
        BEGIN SELECT RAISE(ABORT, 'simulated disk error'); END",
        [],
    )
    .unwrap();
    assert_eq!(cli(&range).status.code(), Some(1));
    assert_eq!(rows(), (2, 2, 1));
    assert_eq!(mappings(), expected);
    db.execute(
        if dual {
            "UPDATE fan_readings SET boost_percent=40 WHERE fan_id='right'"
        } else {
            "UPDATE load_readings SET fan_percent = 40"
        },
        [],
    )
    .unwrap();
    assert_eq!(cli(&range).status.code(), Some(1));
    assert_eq!(rows(), (2, 2, 1));
    assert_eq!(mappings(), expected);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
