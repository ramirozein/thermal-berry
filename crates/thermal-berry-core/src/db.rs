use crate::storage::{
    data_directory,
    unix::{private_directory, set_private_mode},
};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OpenFlags};

use crate::config::{Config, FanMode};
use crate::curve::CurvePoint;
use crate::observation::Observation;
use crate::state::{FanReading, Sample, TempReading};
use crate::thermal::ThermalError;

type DbResult<T> = Result<T, ThermalError>;

const MAX_RANGE_ROWS: i64 = 20_000;

pub struct Db {
    conn: Mutex<Connection>,
    file: Option<PathBuf>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL            -- JSON-encoded scalar
);
CREATE TABLE IF NOT EXISTS manual_boosts (
    fan_id  TEXT PRIMARY KEY,
    percent INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS curve_points (
    fan_id   TEXT NOT NULL,
    position INTEGER NOT NULL,
    temp_c   REAL NOT NULL,
    percent  INTEGER NOT NULL,
    PRIMARY KEY (fan_id, position)
);
CREATE TABLE IF NOT EXISTS temp_readings (
    timestamp_ms INTEGER NOT NULL,
    label        TEXT NOT NULL,
    celsius      REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_temp_readings_ts ON temp_readings(timestamp_ms);
CREATE TABLE IF NOT EXISTS fan_readings (
    timestamp_ms  INTEGER NOT NULL,
    fan_id        TEXT NOT NULL,
    label         TEXT NOT NULL,
    rpm           INTEGER,
    boost_percent INTEGER,
    max_rpm       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_fan_readings_ts ON fan_readings(timestamp_ms);
CREATE TABLE IF NOT EXISTS load_readings (
    timestamp_ms INTEGER PRIMARY KEY, cpu_percent REAL, gpu_percent REAL, mem_percent REAL,
    cpu_temp_c REAL, gpu_temp_c REAL, fan_percent REAL, rpm INTEGER, mode TEXT NOT NULL,
    gpu_mem_percent REAL, gpu_power_w REAL
);
CREATE TABLE IF NOT EXISTS thermal_models (
    die TEXT NOT NULL, coefficients_json TEXT NOT NULL, terms_json TEXT NOT NULL,
    fitted_at_ms INTEGER NOT NULL, rmse_1step REAL NOT NULL, rmse_60s REAL NOT NULL,
    samples_used INTEGER NOT NULL, PRIMARY KEY (fitted_at_ms, die)
);
CREATE TABLE IF NOT EXISTS thermal_model_fans (
    fitted_at_ms INTEGER NOT NULL, channel INTEGER NOT NULL, fan_id TEXT NOT NULL,
    PRIMARY KEY (fitted_at_ms, channel)
);
";

fn db_err(e: impl std::fmt::Display) -> ThermalError {
    ThermalError::Database(e.to_string())
}

fn prepare_private_database(path: &Path) -> std::io::Result<()> {
    let directory = path.parent().expect("database has a parent");
    let uid = private_directory(directory)?;
    let file_options = || {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        options
    };
    let database = file_options()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    set_private_mode(&database, uid, 0o600, false)?;

    for name in [
        "thermal-berry.db-wal",
        "thermal-berry.db-shm",
        "thermal-berry.db-journal",
    ] {
        match file_options().open(directory.join(name)) {
            Ok(file) => set_private_mode(&file, uid, 0o600, false)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl Db {
    pub fn path() -> Option<PathBuf> {
        data_directory()
            .ok()
            .map(|dir| dir.join("thermal-berry.db"))
    }

    pub fn open() -> DbResult<Self> {
        let path = Self::path().ok_or_else(|| db_err("no data directory available"))?;
        prepare_private_database(&path).map_err(db_err)?;
        let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = Connection::open_with_flags(&path, flags).map_err(db_err)?;
        let mut db = Self::init(conn, true)?;
        db.file = Some(path);
        Ok(db)
    }

    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory().map_err(db_err)?, false)
    }

    fn init(conn: Connection, import_legacy: bool) -> DbResult<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_err)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_err)?;
        conn.execute_batch(SCHEMA).map_err(db_err)?;
        let db = Self::from_connection(conn);
        if import_legacy {
            db.import_legacy_json();
        }
        Ok(db)
    }

    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
            file: None,
        }
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn history_writer(&self) -> DbResult<Option<Self>> {
        self.file
            .as_ref()
            .map(|path| {
                Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                )
                .map(Self::from_connection)
                .map_err(db_err)
            })
            .transpose()
    }

    fn import_legacy_json(&self) {
        let empty: bool = self
            .connection()
            .query_row("SELECT COUNT(*) = 0 FROM settings", [], |r| r.get(0))
            .unwrap_or(false);
        if !empty {
            return;
        }
        let Some(json_path) = dirs::config_dir()
            .map(|d| d.join("thermal-berry").join("config.json"))
            .filter(|p| p.exists())
        else {
            return;
        };
        let Ok(raw) = fs::read_to_string(&json_path) else {
            return;
        };
        match serde_json::from_str::<Config>(&raw) {
            Ok(config) => {
                if let Err(e) = self.save_config(&config) {
                    eprintln!("thermal-berry: could not import legacy config.json: {e}");
                    return;
                }
                let _ = fs::rename(&json_path, json_path.with_extension("json.migrated"));
                eprintln!("thermal-berry: imported legacy config.json into SQLite");
            }
            Err(e) => eprintln!("thermal-berry: ignoring corrupt legacy config.json: {e}"),
        }
    }

    #[cfg(test)]
    pub(crate) fn reject_writes(&self) {
        self.connection()
            .pragma_update(None, "query_only", true)
            .unwrap();
    }

    pub fn load_config(&self) -> Config {
        let mut config = Config::default();
        let connection = self.connection();

        let read = |key: &str| -> Option<String> {
            connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                    r.get(0)
                })
                .ok()
        };

        if let Some(v) = read("updateIntervalSecs").and_then(|v| serde_json::from_str(&v).ok()) {
            config.update_interval_secs = v;
        }
        if let Some(v) = read("tempUnit").and_then(|v| serde_json::from_str(&v).ok()) {
            config.temp_unit = v;
        }
        if let Some(v) = read("theme").and_then(|v| serde_json::from_str(&v).ok()) {
            config.theme = v;
        }
        if let Some(v) = read("mode").and_then(|v| serde_json::from_str(&v).ok()) {
            config.mode = v;
        }
        if let Some(v) = read("vendorOverride").and_then(|v| serde_json::from_str(&v).ok()) {
            config.vendor_override = v;
        }
        if let Some(v) = read("historyRetentionDays").and_then(|v| serde_json::from_str(&v).ok()) {
            config.history_retention_days = v;
        }
        if let Some(v) = read("allFansBoost").and_then(|v| serde_json::from_str(&v).ok()) {
            config.all_fans_boost = v;
        }
        if config.mode != crate::config::FanMode::Manual {
            config.all_fans_boost.enabled = false;
        }

        if let Some(v) = read("predictive.targets")
            .and_then(|v| serde_json::from_str::<crate::config::PredictiveTargets>(&v).ok())
            .filter(|v| v.is_valid())
        {
            config.predictive = v;
        }
        if let Some(v) = read("automaticControl")
            .and_then(|v| serde_json::from_str::<crate::config::AutomaticControl>(&v).ok())
            .filter(|v| v.is_valid())
        {
            config.automatic_control = v;
        }

        let mut stmt = match connection.prepare("SELECT fan_id, percent FROM manual_boosts") {
            Ok(s) => s,
            Err(_) => return config,
        };
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?))) {
            config.manual_boosts = rows.flatten().collect();
        }
        drop(stmt);

        let mut stmt = match connection
            .prepare("SELECT fan_id, temp_c, percent FROM curve_points ORDER BY fan_id, position")
        {
            Ok(s) => s,
            Err(_) => return config,
        };
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                CurvePoint {
                    temp_c: r.get(1)?,
                    percent: r.get(2)?,
                },
            ))
        }) {
            for (fan_id, point) in rows.flatten() {
                config.curves.entry(fan_id).or_default().push(point);
            }
        }
        config
    }

    pub fn save_config(&self, config: &Config) -> DbResult<()> {
        let connection = self.connection();
        let tx = connection.unchecked_transaction().map_err(db_err)?;
        {
            let mut upsert = tx
                .prepare("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")
                .map_err(db_err)?;
            let scalars = [
                (
                    "predictive.targets",
                    serde_json::to_string(&config.predictive).map_err(db_err)?,
                ),
                (
                    "updateIntervalSecs",
                    config.update_interval_secs.to_string(),
                ),
                (
                    "tempUnit",
                    serde_json::to_string(&config.temp_unit).unwrap(),
                ),
                ("theme", serde_json::to_string(&config.theme).unwrap()),
                ("mode", serde_json::to_string(&config.mode).unwrap()),
                (
                    "automaticControl",
                    serde_json::to_string(&config.automatic_control).unwrap(),
                ),
                (
                    "vendorOverride",
                    serde_json::to_string(&config.vendor_override).unwrap(),
                ),
                (
                    "historyRetentionDays",
                    config.history_retention_days.to_string(),
                ),
                (
                    "allFansBoost",
                    serde_json::to_string(&config.all_fans_boost).unwrap(),
                ),
            ];
            for (key, value) in scalars {
                upsert.execute(params![key, value]).map_err(db_err)?;
            }

            tx.execute("DELETE FROM manual_boosts", [])
                .map_err(db_err)?;
            let mut insert = tx
                .prepare("INSERT INTO manual_boosts (fan_id, percent) VALUES (?1, ?2)")
                .map_err(db_err)?;
            for (fan_id, percent) in &config.manual_boosts {
                insert.execute(params![fan_id, percent]).map_err(db_err)?;
            }

            tx.execute("DELETE FROM curve_points", []).map_err(db_err)?;
            let mut insert = tx
                .prepare(
                    "INSERT INTO curve_points (fan_id, position, temp_c, percent) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(db_err)?;
            for (fan_id, points) in &config.curves {
                for (position, p) in points.iter().enumerate() {
                    insert
                        .execute(params![fan_id, position as i64, p.temp_c, p.percent])
                        .map_err(db_err)?;
                }
            }
        }
        tx.commit().map_err(db_err)
    }

    pub fn insert_sample(&self, sample: &Sample) -> DbResult<()> {
        self.insert_tick(sample, None)
    }

    pub fn insert_tick(&self, sample: &Sample, observation: Option<&Observation>) -> DbResult<()> {
        let timestamp = i64::try_from(sample.timestamp_ms).map_err(db_err)?;
        if observation.is_some_and(|o| o.timestamp_ms != sample.timestamp_ms) {
            return Err(db_err(
                "observation and raw readings must belong to the same tick",
            ));
        }
        if let Some(o) = observation.filter(|o| !o.fans.is_empty()) {
            if o.fans.len() != sample.fans.len()
                || o.fans.iter().enumerate().any(|(i, f)| {
                    o.fans[..i].iter().any(|previous| previous.id == f.id)
                        || !sample.fans.iter().any(|raw| {
                            raw.id == f.id
                                && raw.rpm == f.rpm
                                && raw.boost_percent.map(f64::from) == f.percent
                        })
                })
            {
                return Err(db_err("observed fan channels differ from the raw tick"));
            }
        }
        let connection = self.connection();
        let tx = connection.unchecked_transaction().map_err(db_err)?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO temp_readings (timestamp_ms, label, celsius) \
                     VALUES (?1, ?2, ?3)",
                )
                .map_err(db_err)?;
            for t in &sample.temps {
                insert
                    .execute(params![timestamp, t.label, t.celsius])
                    .map_err(db_err)?;
            }
            let mut insert = tx
                .prepare(
                    "INSERT INTO fan_readings \
                     (timestamp_ms, fan_id, label, rpm, boost_percent, max_rpm) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(db_err)?;
            for f in &sample.fans {
                insert
                    .execute(params![
                        timestamp,
                        f.id,
                        f.label,
                        f.rpm,
                        f.boost_percent,
                        f.max_rpm
                    ])
                    .map_err(db_err)?;
            }
        }
        if let Some(o) = observation {
            tx.execute(
                "INSERT INTO load_readings
                 (timestamp_ms, cpu_percent, gpu_percent, mem_percent, cpu_temp_c, gpu_temp_c,
                  fan_percent, rpm, mode, gpu_mem_percent, gpu_power_w)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    timestamp,
                    o.cpu_percent,
                    o.gpu_percent,
                    o.mem_percent,
                    o.cpu_temp_c,
                    o.gpu_temp_c,
                    o.fan_percent,
                    o.rpm,
                    o.mode.map(|m| m.as_str()).unwrap_or("unknown"),
                    o.gpu_mem_percent,
                    o.gpu_power_w
                ],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)
    }

    pub fn save_identification(&self, report: &crate::identify::Report) -> DbResult<()> {
        if !report.accepted {
            return Err(db_err("model did not pass M1 validation"));
        }
        let conn = self.connection();
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        let fitted = crate::readings::now_ms() as i64;
        let save = |die: &str,
                    c: &crate::model::Coefficients,
                    e: &crate::identify::Errors|
         -> DbResult<()> {
            let mut terms = Vec::new();
            let mut values = Vec::new();
            for (term, &value) in report.terms.iter().zip(c) {
                if value != 0.0 {
                    terms.push(term);
                    values.push(value);
                }
            }
            tx.execute(
                "INSERT INTO thermal_models (die, coefficients_json, terms_json, fitted_at_ms,
                    rmse_1step, rmse_60s, samples_used) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    die,
                    serde_json::to_string(&values).map_err(db_err)?,
                    serde_json::to_string(&terms).map_err(db_err)?,
                    fitted,
                    e.rmse_1step,
                    e.rmse_60s,
                    report.samples_used as i64
                ],
            )
            .map_err(db_err)?;
            Ok(())
        };
        save("cpu", &report.model.cpu, &report.sindy.cpu)?;
        if let (Some(c), Some(e)) = (&report.model.gpu, &report.sindy.gpu) {
            save("gpu", c, e)?;
        }
        for (i, id) in report.model.fan_ids.iter().enumerate() {
            tx.execute("INSERT INTO thermal_model_fans (fitted_at_ms, channel, fan_id) VALUES (?1, ?2, ?3)",
                params![fitted, i as i64 + 1, id]).map_err(db_err)?;
        }
        tx.commit().map_err(db_err)
    }

    pub fn observations(&self, from_ms: u64, to_ms: u64) -> DbResult<Vec<Observation>> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT timestamp_ms, cpu_temp_c, gpu_temp_c, cpu_percent, gpu_percent, fan_percent,
                    rpm, mode, mem_percent, gpu_mem_percent, gpu_power_w,
                    (SELECT json_group_array(json_object('id', fan_id, 'percent', boost_percent,
                                                         'rpm', fr.rpm) ORDER BY fan_id)
                     FROM fan_readings fr WHERE fr.timestamp_ms = load_readings.timestamp_ms)
             FROM load_readings
             WHERE timestamp_ms BETWEEN ?1 AND ?2 ORDER BY timestamp_ms LIMIT ?3"
        ).map_err(db_err)?;
        let rows = statement
            .query_map(
                params![
                    i64::try_from(from_ms).map_err(db_err)?,
                    i64::try_from(to_ms).map_err(db_err)?,
                    MAX_RANGE_ROWS + 1
                ],
                |r| {
                    let mode: String = r.get(7)?;
                    Ok(Observation {
                        timestamp_ms: u64::try_from(r.get::<_, i64>(0)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cpu_temp_c: r.get(1)?,
                        gpu_temp_c: r.get(2)?,
                        cpu_percent: r.get(3)?,
                        gpu_percent: r.get(4)?,
                        fan_percent: r.get(5)?,
                        rpm: r.get(6)?,
                        fans: serde_json::from_str(&r.get::<_, String>(11)?).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                11,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        mode: match mode.as_str() {
                            "manual" => Some(FanMode::Manual),
                            "disabled" => Some(FanMode::Disabled),
                            "auto" => Some(FanMode::Auto),
                            "unknown" => None,
                            _ => return Err(rusqlite::Error::InvalidQuery),
                        },
                        mem_percent: r.get(8)?,
                        gpu_mem_percent: r.get(9)?,
                        gpu_power_w: r.get(10)?,
                    })
                },
            )
            .map_err(db_err)?;
        let rows: Vec<_> = rows.collect::<rusqlite::Result<_>>().map_err(db_err)?;
        if rows.len() > MAX_RANGE_ROWS as usize {
            return Err(db_err("select a shorter calibration session"));
        }
        Ok(rows)
    }

    pub fn query_range(&self, from_ms: u64, to_ms: u64) -> DbResult<Vec<Sample>> {
        if let Some(path) = &self.file {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(db_err)?;
            return Self::read_range(&reader, from_ms, to_ms);
        }
        Self::read_range(&self.connection(), from_ms, to_ms)
    }

    fn read_range(connection: &Connection, from_ms: u64, to_ms: u64) -> DbResult<Vec<Sample>> {
        let tx = connection.unchecked_transaction().map_err(db_err)?;
        let mut samples: BTreeMap<u64, Sample> = BTreeMap::new();

        let mut stmt = tx
            .prepare(
                "SELECT timestamp_ms, label, celsius FROM temp_readings \
                 WHERE timestamp_ms BETWEEN ?1 AND ?2 ORDER BY timestamp_ms LIMIT ?3",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![from_ms as i64, to_ms as i64, MAX_RANGE_ROWS], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    TempReading {
                        label: r.get(1)?,
                        celsius: r.get(2)?,
                    },
                ))
            })
            .map_err(db_err)?;
        for row in rows.flatten() {
            samples
                .entry(row.0)
                .or_insert_with(|| Sample {
                    timestamp_ms: row.0,
                    temps: Vec::new(),
                    fans: Vec::new(),
                    control: None,
                    errors: Vec::new(),
                })
                .temps
                .push(row.1);
        }

        let mut stmt = tx
            .prepare(
                "SELECT timestamp_ms, fan_id, label, rpm, boost_percent, max_rpm \
                 FROM fan_readings \
                 WHERE timestamp_ms BETWEEN ?1 AND ?2 ORDER BY timestamp_ms LIMIT ?3",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![from_ms as i64, to_ms as i64, MAX_RANGE_ROWS], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    FanReading {
                        id: r.get(1)?,
                        label: r.get(2)?,
                        rpm: r.get(3)?,
                        boost_percent: r.get(4)?,
                        max_rpm: r.get(5)?,
                    },
                ))
            })
            .map_err(db_err)?;
        for row in rows.flatten() {
            samples
                .entry(row.0)
                .or_insert_with(|| Sample {
                    timestamp_ms: row.0,
                    temps: Vec::new(),
                    fans: Vec::new(),
                    control: None,
                    errors: Vec::new(),
                })
                .fans
                .push(row.1);
        }

        Ok(samples.into_values().collect())
    }

    pub fn purge_older_than(&self, cutoff_ms: u64) -> DbResult<()> {
        let connection = self.connection();
        let tx = connection.unchecked_transaction().map_err(db_err)?;
        tx.execute(
            "DELETE FROM temp_readings WHERE timestamp_ms < ?1",
            [cutoff_ms as i64],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM fan_readings WHERE timestamp_ms < ?1",
            [cutoff_ms as i64],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM load_readings WHERE timestamp_ms < ?1",
            [cutoff_ms as i64],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")
            .map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FanMode;
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    fn sample(ts: u64) -> Sample {
        Sample {
            control: None,
            errors: Vec::new(),
            timestamp_ms: ts,
            temps: vec![TempReading {
                label: "CPU".into(),
                celsius: 55.5,
            }],
            fans: vec![FanReading {
                id: "fan1".into(),
                label: "CPU Fan".into(),
                rpm: Some(2200),
                boost_percent: Some(40),
                max_rpm: Some(6000),
            }],
        }
    }

    #[test]
    fn config_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let mut config = Config {
            update_interval_secs: 5,
            mode: FanMode::Manual,
            ..Config::default()
        };
        config.automatic_control.target_temp_c = 65.0;
        config.predictive.gpu_target_c = 72.0;
        config.automatic_control.strategy = crate::config::AutoStrategy::Curve;
        config.vendor_override = Some("alienware".into());
        config.manual_boosts.insert("fan1".into(), 42);
        config.curves.insert(
            "fan2".into(),
            vec![
                CurvePoint {
                    temp_c: 40.0,
                    percent: 0,
                },
                CurvePoint {
                    temp_c: 90.0,
                    percent: 100,
                },
            ],
        );
        config.all_fans_boost.enabled = true;
        config.all_fans_boost.percent = 75;
        db.save_config(&config).unwrap();

        let loaded = db.load_config();
        assert_eq!(loaded.update_interval_secs, 5);
        assert_eq!(loaded.mode, FanMode::Manual);
        assert_eq!(loaded.automatic_control, config.automatic_control);
        assert_eq!(loaded.predictive, config.predictive);
        assert_eq!(loaded.vendor_override.as_deref(), Some("alienware"));
        assert_eq!(loaded.manual_boosts.get("fan1"), Some(&42));
        assert_eq!(loaded.curves.get("fan2").unwrap().len(), 2);
        assert!(loaded.all_fans_boost.enabled);
        assert_eq!(loaded.all_fans_boost.percent, 75);
    }

    #[test]
    fn empty_db_returns_defaults() {
        let db = Db::open_in_memory().unwrap();
        let config = db.load_config();
        assert_eq!(config.update_interval_secs, 2);
        assert_eq!(
            (
                config.predictive.cpu_target_c,
                config.predictive.gpu_target_c
            ),
            (70.0, 75.0)
        );
        assert_eq!(config.mode, FanMode::Auto);
        assert!(!config.all_fans_boost.enabled);
    }

    #[test]
    fn invalid_automatic_settings_fall_back_to_defaults() {
        let db = Db::open_in_memory().unwrap();
        let mut config = Config::default();
        config.automatic_control.target_temp_c = 99.0;
        db.save_config(&config).unwrap();
        assert_eq!(
            db.load_config().automatic_control,
            crate::config::AutomaticControl::default(),
        );
    }

    #[test]
    fn sample_roundtrip_and_range() {
        let db = Db::open_in_memory().unwrap();
        db.insert_sample(&sample(1000)).unwrap();
        db.insert_sample(&sample(2000)).unwrap();
        db.insert_sample(&sample(3000)).unwrap();

        let range = db.query_range(1500, 2500).unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].timestamp_ms, 2000);
        assert_eq!(range[0].temps[0].label, "CPU");
        assert_eq!(range[0].fans[0].rpm, Some(2200));
    }

    #[test]
    fn an_existing_file_supports_independent_history_reads_and_writes() {
        let path = std::env::temp_dir().join(format!(
            "thermal-berry-history-{}-{:?}.db",
            std::process::id(),
            std::thread::current().id()
        ));
        {
            let original = Db::init(Connection::open(&path).unwrap(), false).unwrap();
            original.save_config(&Config::default()).unwrap();
            original.insert_sample(&sample(100)).unwrap();
        }
        {
            let mut db = Db::init(Connection::open(&path).unwrap(), false).unwrap();
            db.file = Some(path.clone());
            let writer = db.history_writer().unwrap().unwrap();
            assert_eq!(db.load_config().mode, FanMode::Auto);
            writer.insert_sample(&sample(200)).unwrap();

            let connection = writer.connection();
            let pending = connection.unchecked_transaction().unwrap();
            pending.execute("DELETE FROM temp_readings", []).unwrap();
            let saved = db.query_range(100, 200).unwrap();
            assert_eq!(saved.len(), 2);
            assert_eq!(saved[0].temps.len(), 1);
            assert_eq!(saved[1].fans[0].rpm, Some(2200));
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn private_database_reopens_existing_history_and_rejects_symlinks() {
        let directory = std::env::temp_dir().join(format!(
            "thermal-berry-permissions-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let data = directory.join("data");
        let path = data.join("thermal-berry.db");
        fs::create_dir(&data).unwrap();
        {
            let old = Db::init(Connection::open(&path).unwrap(), false).unwrap();
            old.save_config(&Config::default()).unwrap();
            old.insert_sample(&sample(100)).unwrap();
        }
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        prepare_private_database(&path).unwrap();
        assert_eq!(fs::metadata(&data).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

        let conn = Connection::open(&path).unwrap();
        assert!(rusqlite::version_number() >= 3_054_001);
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let reopened = Db::init(conn, false).unwrap();
        assert_eq!(reopened.load_config().mode, FanMode::Auto);
        assert_eq!(reopened.query_range(100, 100).unwrap().len(), 1);
        reopened.insert_sample(&sample(200)).unwrap();
        for suffix in ["-wal", "-shm"] {
            let sidecar = data.join(format!("thermal-berry.db{suffix}"));
            assert_eq!(fs::metadata(sidecar).unwrap().mode() & 0o777, 0o600);
        }
        drop(reopened);

        let other = data.join("other");
        fs::rename(&path, &other).unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        assert!(prepare_private_database(&path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn purge_removes_old_samples() {
        let db = Db::open_in_memory().unwrap();
        db.insert_sample(&sample(1000)).unwrap();
        db.insert_sample(&sample(5000)).unwrap();
        db.purge_older_than(3000).unwrap();

        let all = db.query_range(0, u64::MAX / 2).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].timestamp_ms, 5000);
    }
}

#[cfg(test)]
mod predictive_storage_tests {
    use super::*;
    use crate::config::FanMode;

    #[test]
    fn additive_migration_keeps_existing_history_and_does_not_invent_loads() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO settings VALUES ('updateIntervalSecs','9');
            CREATE TABLE temp_readings (timestamp_ms INTEGER NOT NULL, label TEXT NOT NULL, celsius REAL NOT NULL);
            INSERT INTO temp_readings VALUES (2000,'CPU',56.5);").unwrap();
        let db = Db::init(conn, false).unwrap();
        assert_eq!(db.load_config().update_interval_secs, 9);
        assert_eq!(db.query_range(0, 3000).unwrap()[0].temps[0].celsius, 56.5);
        assert!(db.observations(0, 3000).unwrap().is_empty());
    }

    #[test]
    fn joined_ticks_are_atomic_nullable_and_age_out_with_legacy_history() {
        let db = Db::open_in_memory().unwrap();
        let sample = Sample {
            timestamp_ms: 12345,
            temps: vec![TempReading {
                label: "CPU".into(),
                celsius: 42.0,
            }],
            fans: vec![],
            control: None,
            errors: vec![],
        };
        let o = Observation {
            fans: vec![],
            timestamp_ms: sample.timestamp_ms,
            mode: Some(FanMode::Manual),
            cpu_temp_c: Some(42.0),
            cpu_percent: Some(30.0),
            mem_percent: Some(40.0),
            gpu_temp_c: None,
            gpu_percent: None,
            fan_percent: None,
            rpm: None,
            gpu_mem_percent: None,
            gpu_power_w: None,
        };
        let mut other_tick = o.clone();
        other_tick.timestamp_ms += 1;
        assert!(db.insert_tick(&sample, Some(&other_tick)).is_err());
        assert!(db.query_range(0, 20000).unwrap().is_empty());
        db.insert_tick(&sample, Some(&o)).unwrap();
        assert!(db.insert_tick(&sample, Some(&o)).is_err());
        assert_eq!(db.observations(0, 20000).unwrap(), vec![o]);
        assert_eq!(db.query_range(0, 20000).unwrap()[0].temps.len(), 1);
        db.purge_older_than(12346).unwrap();
        assert!(db.observations(0, 20000).unwrap().is_empty());
        assert!(db.query_range(0, 20000).unwrap().is_empty());
    }
}
