use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::commands::{self, UDEV_RULE_PATH};
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

/// Async + spawn_blocking for the same reason as `install_udev_rule`: pkexec
/// blocks on the user's password dialog.
#[tauri::command]
pub async fn uninstall_app(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
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
    app.exit(0);
    Ok(())
}

fn uninstall_blocking(state: &Arc<AppState>, app: &AppHandle) -> CmdResult<()> {
    commands::begin_maintenance(state)?;
    commands::publish_state(app, state);
    let result = (|| {
        let method = detect_install_method();
        run_privileged_removal(method)?;
        // The monitor and IPC share this mutex; no connection remains open
        // when deletion starts, and maintenance keeps subsequent ticks paused.
        drop(state.lock().db.take());
        if let Some(db_dir) = Db::path().and_then(|p| p.parent().map(PathBuf::from)) {
            fs::remove_dir_all(&db_dir).map_err(|e| ThermalError::Io {
                path: db_dir.display().to_string(),
                source: e,
            })?;
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
    inner.control.maintenance = inner.db.is_none();
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
    let purge_package = match method {
        InstallMethod::Deb => format!("dpkg --purge {BIN_NAME}\n"),
        InstallMethod::AppImage | InstallMethod::Unknown => String::new(),
    };
    let script = format!(
        "set -e\n\
         rm -f {UDEV_RULE_PATH}\n\
         udevadm control --reload-rules\n\
         udevadm trigger --subsystem-match=hwmon --action=change\n\
         {purge_package}"
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
