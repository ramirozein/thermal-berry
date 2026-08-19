use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::commands::UDEV_RULE_PATH;
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
    tauri::async_runtime::spawn_blocking(move || uninstall_blocking(&state))
        .await
        .map_err(|e| ThermalError::InvalidValue(format!("uninstall task failed: {e}")))??;
    app.exit(0);
    Ok(())
}

fn uninstall_blocking(state: &Arc<AppState>) -> CmdResult<()> {
    {
        let inner = state.lock();
        if let Some(device) = inner.device.as_ref() {
            for fan in device.fans() {
                let _ = fan.set_boost(fan.boost_range().0);
            }
        }
    }

    let method = detect_install_method();
    run_privileged_removal(method)?;

    if let Some(db_dir) = Db::path().and_then(|p| p.parent().map(PathBuf::from)) {
        let _ = fs::remove_dir_all(db_dir);
    }
    remove_autostart_entry();
    if method == InstallMethod::AppImage {
        remove_appimage_files();
    }

    Ok(())
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
