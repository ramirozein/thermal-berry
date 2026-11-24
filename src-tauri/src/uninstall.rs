use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};

use crate::commands;
use crate::db::Db;
use crate::state::AppState;
use crate::thermal::ThermalError;

type CmdResult<T> = Result<T, ThermalError>;

const BIN_NAME: &str = "thermal-berry";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMethod {
    Deb,
    AppImage,
    Unknown,
}

#[tauri::command]
pub async fn uninstall_app(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    let maintenance = crate::lifecycle::Maintenance::start(&app)?;
    let state = Arc::clone(state.inner());
    let removal_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = uninstall_blocking(&state, &removal_app);
        if let Err(e) = &result {
            commands::report_control_error(&state, e);
        }
        commands::publish_state(&removal_app, &state);
        result
    })
    .await
    .map_err(|e| ThermalError::Control(format!("uninstall task failed: {e}")))?;
    result?;
    crate::osd::remove_preferences(&app)?;
    maintenance.complete(false);
    Ok(())
}

fn uninstall_blocking(state: &Arc<AppState>, app: &AppHandle) -> CmdResult<()> {
    commands::begin_maintenance(state)?;
    commands::publish_state(app, state);
    if let Some(history) = app.try_state::<crate::monitor::HistoryWriter>() {
        history.flush();
    }
    let result = (|| {
        let method = detect_install_method();
        run_privileged_removal(method)?;
        state.close_database();
        if let Some(db_dir) = Db::path().and_then(|p| p.parent().map(PathBuf::from)) {
            if let Err(error) = fs::remove_dir_all(&db_dir) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(ThermalError::from_io(&db_dir, error));
                }
            }
        }
        if let Some(config) = dirs::config_dir() {
            for name in ["config.json", "config.json.migrated"] {
                remove_file_if_present(&config.join("thermal-berry").join(name))?;
            }
        }
        remove_autostart_entry();
        if method == InstallMethod::AppImage {
            remove_appimage_files();
        }
        Ok(())
    })();
    if let Err(error) = result {
        return Err(recover_after_failure(state, error));
    }
    Ok(())
}

pub(crate) fn recover_after_failure(state: &AppState, error: ThermalError) -> ThermalError {
    let mut inner = state.lock();
    // Authorization cancellation resumes read-only monitoring in Disabled.
    // Once deletion starts the connection stays closed and monitoring paused.
    inner.control.maintenance = inner.shutting_down || inner.db.is_none();
    let message = format!(
        "Uninstall failed: {error}. Fan control remains disabled.{}",
        if inner.control.maintenance {
            " Monitoring remains paused because the database is closed; close the app."
        } else {
            " Select a mode to resume control."
        }
    );
    inner.control.error = Some(message.clone());
    inner.config.revision += 1;
    ThermalError::Control(message)
}

fn detect_install_method() -> InstallMethod {
    if std::env::var_os("APPIMAGE").is_some() {
        return InstallMethod::AppImage;
    }
    let is_deb = Command::new("dpkg")
        .args(["-s", BIN_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if is_deb {
        InstallMethod::Deb
    } else {
        InstallMethod::Unknown
    }
}

fn run_privileged_removal(method: InstallMethod) -> CmdResult<()> {
    commands::change_udev_access(if method == InstallMethod::Deb {
        "remove-deb"
    } else {
        "remove"
    })
}

pub(crate) fn remove_file_if_present(path: &std::path::Path) -> CmdResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ThermalError::from_io(path, error)),
    }
}

fn remove_autostart_entry() {
    if let Some(home) = dirs::home_dir() {
        let _ = fs::remove_file(
            home.join(".config")
                .join("autostart")
                .join(format!("{BIN_NAME}.desktop")),
        );
    }
}

fn remove_appimage_files() {
    if let Ok(appimage_path) = std::env::var("APPIMAGE") {
        let _ = fs::remove_file(appimage_path);
    }
    if let Some(home) = dirs::home_dir() {
        let _ = fs::remove_file(
            home.join(".local/share/applications")
                .join(format!("{BIN_NAME}.desktop")),
        );
    }
}
