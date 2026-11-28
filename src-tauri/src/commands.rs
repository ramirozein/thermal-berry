use std::process::Command;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::config::{Config, FanMode};
use crate::curve::CurvePoint;
use crate::state::{AppState, Sample, StateSnapshot};
use crate::thermal::{self, ThermalError};
use crate::tray;
pub use thermal_berry_core::control::{
    apply_all_fans_boost, apply_config_patch, apply_fan_boost, apply_mode, begin_maintenance,
    restore_control, ConfigPatch,
};

type CmdResult<T> = Result<T, ThermalError>;

pub(crate) const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/60-thermal-berry.rules";

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
    pub fan_control: bool,
    pub write_access: bool,
    pub available_vendors: Vec<String>,
}

#[tauri::command]
pub async fn get_device_info(state: State<'_, Arc<AppState>>) -> CmdResult<DeviceInfo> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let device = state
            .lock()
            .device
            .clone()
            .ok_or(ThermalError::DeviceNotFound)?;
        Ok(device_info(device.as_ref()))
    })
    .await
    .map_err(|e| ThermalError::Control(format!("device discovery task failed: {e}")))?
}

fn device_info(device: &dyn thermal::ThermalDevice) -> DeviceInfo {
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
    let fan_control = device.supports_fan_control() && !fans.is_empty();
    DeviceInfo {
        vendor: device.vendor_name().to_string(),
        driver: device.driver_name().to_string(),
        model: device.model(),
        fan_control,
        write_access: fan_control && fans.iter().all(|fan| fan.writable),
        fans,
        sensors: device
            .sensors()
            .iter()
            .map(|s| s.label().to_string())
            .collect(),
        supports_auto_curve: device.supports_auto_curve(),
        available_vendors: thermal::VENDORS.iter().map(|v| v.to_string()).collect(),
    }
}

#[tauri::command]
pub fn get_history(state: State<'_, Arc<AppState>>) -> Vec<Sample> {
    let inner = state.lock();
    inner.history.iter().cloned().collect()
}

#[tauri::command]
pub async fn get_history_range(
    state: State<'_, Arc<AppState>>,
    from_ms: u64,
    to_ms: u64,
) -> CmdResult<Vec<Sample>> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || state.history_range(from_ms, to_ms))
        .await
        .map_err(|e| ThermalError::Database(format!("history query task failed: {e}")))?
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> Config {
    state.lock().config.clone()
}

pub(crate) fn state_supports_fan_control(state: &AppState) -> bool {
    state
        .lock()
        .device
        .as_ref()
        .is_some_and(|device| device.supports_fan_control())
}

pub const STATE_EVENT: &str = "state-changed";

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>) -> StateSnapshot {
    state.lock().snapshot()
}

/// Read the latest state on the UI thread. Never hold the device mutex while
/// dispatching menu operations: a tray click may itself be waiting for it.
pub fn publish_state(app: &AppHandle, state: &Arc<AppState>) {
    let state = Arc::clone(state);
    let event_app = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        let snapshot = state.lock().snapshot();
        let fan_control = state_supports_fan_control(&state);
        tray::set_fan_controls_enabled(&event_app, fan_control);
        tray::sync_mode(&event_app, snapshot.config.mode);
        tray::sync_all_fans_boost(&event_app, snapshot.config.all_fans_boost.enabled);
        if let Err(e) = event_app.emit_to("main", STATE_EVENT, snapshot) {
            eprintln!("thermal-berry: could not emit control state: {e}");
        }
    }) {
        eprintln!("thermal-berry: could not publish control state: {e}");
    }
}

pub fn report_control_error(state: &AppState, error: &ThermalError) {
    let mut inner = state.lock();
    inner.control.error = Some(error.to_string());
    inner.config.revision += 1;
}

fn publish_result(app: &AppHandle, state: &Arc<AppState>, result: &CmdResult<()>) {
    if let Err(error) = result {
        report_control_error(state, error);
    }
    publish_state(app, state);
}

#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    patch: ConfigPatch,
) -> CmdResult<()> {
    let result = apply_config_patch(&state, patch);
    publish_result(&app, &state, &result);
    result
}

#[tauri::command]
pub fn set_fan_boost(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    fan_id: String,
    percent: u8,
    expected_epoch: u64,
) -> CmdResult<()> {
    let result = apply_fan_boost(&state, fan_id, percent, expected_epoch);
    publish_result(&app, &state, &result);
    result
}

#[tauri::command]
pub fn set_all_fans_boost(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    enabled: bool,
    percent: u8,
) -> CmdResult<()> {
    let result = apply_all_fans_boost(&state, enabled, percent);
    publish_result(&app, &state, &result);
    result
}

#[tauri::command]
pub fn set_mode(app: AppHandle, state: State<'_, Arc<AppState>>, mode: FanMode) -> CmdResult<()> {
    let result = apply_mode(&state, mode);
    publish_result(&app, &state, &result);
    result
}

#[tauri::command]
pub fn save_curve(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    fan_id: String,
    mut points: Vec<CurvePoint>,
) -> CmdResult<()> {
    points.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));
    let result = apply_config_patch(
        &state,
        ConfigPatch {
            curves: Some([(fan_id, points)].into()),
            ..Default::default()
        },
    );
    publish_result(&app, &state, &result);
    result
}

#[tauri::command]
pub fn select_vendor(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    vendor: String,
) -> CmdResult<DeviceInfo> {
    let device = thermal_berry_core::control::select_vendor(&state, vendor)?;
    publish_state(&app, &state);
    Ok(device_info(device.as_ref()))
}

#[tauri::command]
pub fn check_write_access(state: State<'_, Arc<AppState>>) -> bool {
    thermal_berry_core::control::check_write_access(&state)
}

/// Installs (with a single pkexec prompt) the udev rule that makes
/// fan*_boost writable for the current user, reloads it and applies it immediately.
///
/// Async + spawn_blocking: pkexec blocks until the user answers the password
/// dialog, which must not freeze the main thread (UI) nor pin an async worker.
#[tauri::command]
pub async fn install_udev_rule(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    if !state
        .lock()
        .device
        .as_ref()
        .is_some_and(|device| device.driver_name() == "alienware_wmi")
    {
        return Err(ThermalError::Unsupported);
    }
    let _maintenance = crate::lifecycle::Maintenance::start(&app).map_err(ThermalError::Control)?;
    tauri::async_runtime::spawn_blocking(install_udev_rule_blocking)
        .await
        .map_err(|e| ThermalError::InvalidValue(format!("install task failed: {e}")))?
}

fn install_udev_rule_blocking() -> CmdResult<()> {
    change_udev_access("install")
}

pub(crate) fn change_udev_access(action: &str) -> CmdResult<()> {
    let output = Command::new("/usr/bin/pkexec")
        .args([
            "/bin/bash",
            "-c",
            include_str!("../packaging/linux/udev-access.sh"),
            "thermal-berry-udev",
            action,
        ])
        .output()
        .map_err(|error| ThermalError::from_io(std::path::Path::new("/usr/bin/pkexec"), error))?;
    if output.status.success() {
        Ok(())
    } else if matches!(output.status.code(), Some(126 | 127)) {
        Err(ThermalError::PermissionDenied {
            path: UDEV_RULE_PATH.into(),
        })
    } else {
        Err(ThermalError::Control(format!(
            "udev maintenance failed: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )))
    }
}
