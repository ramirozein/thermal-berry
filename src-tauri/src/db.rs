use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OpenFlags};

use crate::config::Config;
use crate::curve::CurvePoint;
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
";

fn db_err(e: impl std::fmt::Display) -> ThermalError {
    ThermalError::Database(e.to_string())
}

fn prepare_private_database(path: &Path) -> std::io::Result<()> {
    let directory = path.parent().expect("database has a parent");
    fs::create_dir_all(
        directory
            .parent()
            .expect("application directory has a parent"),
    )?;
    match DirBuilder::new().mode(0o700).create(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    // SAFETY: geteuid takes no arguments and returns the current effective UID.
    let uid = unsafe { libc::geteuid() };
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)?;
    set_private_mode(&dir, uid, 0o700, true)?;

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

fn set_private_mode(file: &File, uid: u32, mode: u32, directory: bool) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != uid
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "unsafe database path",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(mode))
}

impl Db {
    pub fn path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("thermal-berry").join("thermal-berry.db"))
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
        conn.pragma_update(None, "journal_mode", "WAL").ok();
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
            let scalars: [(&str, String); 8] = [
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
                    .execute(params![sample.timestamp_ms as i64, t.label, t.celsius])
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
                        sample.timestamp_ms as i64,
                        f.id,
                        f.label,
                        f.rpm,
                        f.boost_percent,
                        f.max_rpm
                    ])
                    .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)
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
        let mut config = Config::default();
        config.update_interval_secs = 5;
        config.mode = FanMode::Manual;
        config.automatic_control.target_temp_c = 65.0;
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
