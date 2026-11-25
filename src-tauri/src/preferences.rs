use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TempUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("Autostart: {e}"))
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    if !crate::lifecycle::allow_settings(&app) {
        return Err("Autostart settings are unavailable during shutdown or maintenance".into());
    }
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| format!("Autostart: {e}"))
}

pub(crate) fn save_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let data = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let parent = path.parent().ok_or("Settings directory is unavailable")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let result = fs::write(&temporary, data).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
pub mod desktop {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use tauri::{Manager, State};

    use super::*;

    #[derive(Clone, Copy, Serialize, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    pub struct Preferences {
        pub update_interval_secs: u64,
        pub temp_unit: TempUnit,
        theme: Theme,
    }

    impl Default for Preferences {
        fn default() -> Self {
            Self {
                update_interval_secs: 2,
                temp_unit: TempUnit::Celsius,
                theme: Theme::System,
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    pub struct PreferencesPatch {
        update_interval_secs: Option<u64>,
        temp_unit: Option<TempUnit>,
        theme: Option<Theme>,
    }

    pub struct DesktopPreferences {
        path: Option<PathBuf>,
        current: Mutex<Preferences>,
    }

    pub fn setup(app: &AppHandle) {
        let path = app
            .path()
            .app_config_dir()
            .ok()
            .map(|directory| directory.join("preferences.json"));
        let current = path
            .as_ref()
            .map(|path| match fs::read(path) {
                Ok(data) => serde_json::from_slice::<Preferences>(&data)
                    .map(|mut preferences| {
                        preferences.update_interval_secs =
                            preferences.update_interval_secs.clamp(1, 60);
                        preferences
                    })
                    .unwrap_or_else(|error| {
                        eprintln!("thermal-berry: could not parse preferences: {error}");
                        Preferences::default()
                    }),
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("thermal-berry: could not read preferences: {error}");
                    }
                    Preferences::default()
                }
            })
            .unwrap_or_default();
        app.manage(DesktopPreferences {
            path,
            current: Mutex::new(current),
        });
    }

    #[tauri::command]
    pub fn get_preferences(state: State<'_, DesktopPreferences>) -> Preferences {
        *state.current.lock().unwrap()
    }

    #[tauri::command]
    pub fn set_preferences(
        app: AppHandle,
        state: State<'_, DesktopPreferences>,
        patch: PreferencesPatch,
    ) -> Result<(), String> {
        let mut current = state.current.lock().unwrap();
        if !crate::lifecycle::allow_settings(&app) {
            return Err("Preferences are unavailable during shutdown".into());
        }
        let next = Preferences {
            update_interval_secs: patch
                .update_interval_secs
                .map(|secs| secs.clamp(1, 60))
                .unwrap_or(current.update_interval_secs),
            temp_unit: patch.temp_unit.unwrap_or(current.temp_unit),
            theme: patch.theme.unwrap_or(current.theme),
        };
        let path = state
            .path
            .as_deref()
            .ok_or("Preferences directory is unavailable")?;
        save_json(path, &next)?;
        *current = next;
        if let Some(worker) = app.try_state::<crate::metrics::MetricsWorker>() {
            worker.refresh();
        }
        Ok(())
    }
}
