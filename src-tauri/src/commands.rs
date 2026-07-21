use std::process::Command;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::config::{AllFansBoost, Config, FanMode};
use crate::curve::{self, CurvePoint};
use crate::state::{AppState, Inner, Sample};
use crate::thermal::{self, ThermalError};
use crate::tray;

type CmdResult<T> = Result<T, ThermalError>;

const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/60-thermal-berry.rules";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanInfo {
    pub id: String,
    pub label: String,
    pub max_rpm: Option<u32>,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub vendor: String,
    pub driver: String,
    pub model: Option<String>,
    pub fans: Vec<FanInfo>,
    pub sensors: Vec<String>,
    pub supports_auto_curve: bool,
    /// true if all fans accept writes (udev rule installed).
    pub write_access: bool,
    /// Vendors available for the manual fallback in Settings.
    pub available_vendors: Vec<String>,
}

#[tauri::command]
pub fn get_device_info(state: State<'_, Arc<AppState>>) -> CmdResult<DeviceInfo> {
    let inner = state.lock();
    let device = inner.device.as_ref().ok_or(ThermalError::DeviceNotFound)?;
    let fans: Vec<FanInfo> = device
        .fans()
        .iter()
        .map(|f| FanInfo {
            id: f.id().to_string(),
            label: f.label().to_string(),
            max_rpm: f.max_rpm(),
            writable: f.is_writable(),
        })
        .collect();
    Ok(DeviceInfo {
        vendor: device.vendor_name().to_string(),
        driver: device.driver_name().to_string(),
        model: device.model(),
        write_access: !fans.is_empty() && fans.iter().all(|f| f.writable),
        fans,
        sensors: device
            .sensors()
            .iter()
            .map(|s| s.label().to_string())
            .collect(),
        supports_auto_curve: device.supports_auto_curve(),
        available_vendors: thermal::VENDORS.iter().map(|v| v.to_string()).collect(),
    })
}

#[tauri::command]
pub fn get_history(state: State<'_, Arc<AppState>>) -> Vec<Sample> {
    let inner = state.lock();
    inner.history.iter().cloned().collect()
}

/// Historical telemetry from the database (beyond the in-RAM ring buffer),
/// for long-range charts and stats.
#[tauri::command]
pub fn get_history_range(
    state: State<'_, Arc<AppState>>,
    from_ms: u64,
    to_ms: u64,
) -> CmdResult<Vec<Sample>> {
    let inner = state.lock();
    inner.db.query_range(from_ms, to_ms)
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> Config {
    state.lock().config.clone()
}

#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    config: Config,
) -> CmdResult<Config> {
    let mut inner = state.lock();
    inner.config = config;
    persist(&inner)?;
    tray::sync_mode(&app, inner.config.mode);
    Ok(inner.config.clone())
}

/// Sets the manual boost of a fan (0-100%). Changes the mode to Manual.
#[tauri::command]
pub fn set_fan_boost(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    fan_id: String,
    percent: u8,
) -> CmdResult<()> {
    if percent > 100 {
        return Err(ThermalError::InvalidValue(format!("{percent}% > 100%")));
    }
    let mut inner = state.lock();
    let device = inner.device.as_ref().ok_or(ThermalError::DeviceNotFound)?;
    let fans = device.fans();
    let fan = fans
        .iter()
        .find(|f| f.id() == fan_id)
        .ok_or_else(|| ThermalError::InvalidValue(format!("unknown fan: {fan_id}")))?;
    fan.set_boost(curve::percent_to_boost(percent, fan.boost_range()))?;
    inner.config.mode = FanMode::Manual;
    inner.config.manual_boosts.insert(fan_id, percent);
    persist(&inner)?;
    tray::sync_mode(&app, FanMode::Manual);
    Ok(())
}

/// Sets every fan to the same manual boost percentage in one shot. Used by
/// the "boost all fans" switch (see `set_all_fans_boost`) so flipping it
/// applies one percentage to the whole device instead of fan-by-fan.
pub fn apply_manual_boost_all(state: &AppState, percent: u8) -> CmdResult<()> {
    if percent > 100 {
        return Err(ThermalError::InvalidValue(format!("{percent}% > 100%")));
    }
    let mut inner = state.lock();
    let device = inner.device.as_ref().ok_or(ThermalError::DeviceNotFound)?;
    for fan in device.fans() {
        fan.set_boost(curve::percent_to_boost(percent, fan.boost_range()))?;
        inner.config.manual_boosts.insert(fan.id().to_string(), percent);
    }
    inner.curve_states.clear();
    inner.config.mode = FanMode::Manual;
    persist(&inner)
}

/// Toggles the "boost all fans" switch: turning it on applies `percent` to
/// every fan (mode Manual); turning it off returns every fan to Auto mode
/// (the curve engine). The switch state itself is persisted so both the
/// tray menu and Settings reflect it after a restart. Shared by the
/// `set_all_fans_boost` command (percent field in Settings) and the tray
/// menu's toggle (which reuses the last saved percent).
pub fn apply_all_fans_boost(state: &AppState, enabled: bool, percent: u8) -> CmdResult<()> {
    if percent > 100 {
        return Err(ThermalError::InvalidValue(format!("{percent}% > 100%")));
    }
    if enabled {
        apply_manual_boost_all(state, percent)?;
    } else {
        apply_mode(state, FanMode::Auto)?;
    }
    let mut inner = state.lock();
    inner.config.all_fans_boost = AllFansBoost { enabled, percent };
    persist(&inner)
}

#[tauri::command]
pub fn set_all_fans_boost(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    enabled: bool,
    percent: u8,
) -> CmdResult<Config> {
    apply_all_fans_boost(&state, enabled, percent)?;
    let inner = state.lock();
    tray::sync_mode(&app, inner.config.mode);
    tray::sync_all_fans_boost(&app, inner.config.all_fans_boost.enabled);
    Ok(inner.config.clone())
}

/// Shared by the set_mode command and the tray menu.
pub fn apply_mode(state: &AppState, mode: FanMode) -> CmdResult<()> {
    let mut inner = state.lock();
    let device = inner.device.as_ref().ok_or(ThermalError::DeviceNotFound)?;
    match mode {
        // Auto: the monitor loop applies the curve on each tick; here we
        // just clear the hysteresis so the first tick applies immediately.
        FanMode::Auto => {}
        // Manual: re-applies the last saved percentages.
        FanMode::Manual => {
            for fan in device.fans() {
                let pct = inner
                    .config
                    .manual_boosts
                    .get(fan.id())
                    .copied()
                    .unwrap_or(0);
                fan.set_boost(curve::percent_to_boost(pct, fan.boost_range()))?;
            }
        }
        // Disabled: boost 0 on all = hands full control back to the EC; the
        // monitor loop only writes fans in Auto, so nothing touches them again.
        FanMode::Disabled => {
            for fan in device.fans() {
                fan.set_boost(fan.boost_range().0)?;
            }
        }
    }
    inner.curve_states.clear();
    inner.config.mode = mode;
    persist(&inner)
}

#[tauri::command]
pub fn set_mode(app: AppHandle, state: State<'_, Arc<AppState>>, mode: FanMode) -> CmdResult<()> {
    apply_mode(&state, mode)?;
    tray::sync_mode(&app, mode);
    Ok(())
}

#[tauri::command]
pub fn save_curve(
    state: State<'_, Arc<AppState>>,
    fan_id: String,
    points: Vec<CurvePoint>,
) -> CmdResult<()> {
    if points.iter().any(|p| p.percent > 100) {
        return Err(ThermalError::InvalidValue(
            "percent > 100 in the curve".into(),
        ));
    }
    if points.len() < 2 {
        return Err(ThermalError::InvalidValue(
            "a curve needs at least 2 points".into(),
        ));
    }
    let mut sorted = points;
    sorted.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));
    let mut inner = state.lock();
    inner.config.curves.insert(fan_id.clone(), sorted);
    inner.curve_states.remove(&fan_id);
    persist(&inner)
}

/// Manual fallback from Settings: forces a specific vendor.
#[tauri::command]
pub fn select_vendor(state: State<'_, Arc<AppState>>, vendor: String) -> CmdResult<DeviceInfo> {
    let device = thermal::create_device(&vendor)?;
    {
        let mut inner = state.lock();
        inner.device = Some(device);
        inner.config.vendor_override = Some(vendor);
        persist(&inner)?;
    }
    get_device_info(state)
}

#[tauri::command]
pub fn check_write_access(state: State<'_, Arc<AppState>>) -> bool {
    let inner = state.lock();
    match inner.device.as_ref() {
        Some(device) => {
            let fans = device.fans();
            !fans.is_empty() && fans.iter().all(|f| f.is_writable())
        }
        None => false,
    }
}

/// Installs (with a single pkexec prompt) the udev rule that makes
/// fan*_boost writable for the current user, reloads it and applies it immediately.
///
/// Async + spawn_blocking: pkexec blocks until the user answers the password
/// dialog, which must not freeze the main thread (UI) nor pin an async worker.
#[tauri::command]
pub async fn install_udev_rule() -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(install_udev_rule_blocking)
        .await
        .map_err(|e| ThermalError::InvalidValue(format!("install task failed: {e}")))?
}

fn install_udev_rule_blocking() -> CmdResult<()> {
    let user = std::env::var("USER")
        .map_err(|_| ThermalError::InvalidValue("could not determine $USER".into()))?;
    if !user
        .chars()
        .all(|c| c.is_alphanumeric() || ".-_".contains(c))
    {
        return Err(ThermalError::InvalidValue(format!("invalid user: {user}")));
    }
    let rule = format!(
        "# Installed by thermal-berry: allows controlling fan boost without root (user: {user})\n\
         ACTION==\"add|change\", SUBSYSTEM==\"hwmon\", ATTR{{name}}==\"alienware_wmi\", \
         RUN+=\"/bin/sh -c 'chown {user} /sys%p/fan*_boost'\"\n"
    );
    let script = format!(
        "set -e\n\
         printf '%s' '{rule}' > {UDEV_RULE_PATH}\n\
         udevadm control --reload-rules\n\
         udevadm trigger --subsystem-match=hwmon --action=change\n",
        rule = rule.replace('\'', "'\\''"),
    );
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(&script)
        .status()
        .map_err(|e| ThermalError::Io {
            path: "pkexec".into(),
            source: e,
        })?;
    if status.success() {
        Ok(())
    } else {
        // pkexec: 126 = dialog cancelled, 127 = not authorized.
        Err(ThermalError::PermissionDenied {
            path: UDEV_RULE_PATH.into(),
        })
    }
}

fn persist(inner: &Inner) -> CmdResult<()> {
    inner.db.save_config(&inner.config)
}
