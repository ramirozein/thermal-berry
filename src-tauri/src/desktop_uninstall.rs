use tauri::AppHandle;

#[tauri::command]
pub async fn uninstall_app(app: AppHandle) -> Result<(), String> {
    let maintenance = crate::lifecycle::Maintenance::start(&app)?;
    tauri::async_runtime::spawn_blocking(move || uninstall(&app))
        .await
        .map_err(|error| format!("Uninstall task failed: {error}"))??;
    maintenance.complete(false);
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall(_app: &AppHandle) -> Result<(), String> {
    let path = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("uninstall.exe");
    if !path.is_file() {
        return Err("The Thermal Berry uninstaller was not found. If you installed the app, use Settings > Apps > Installed apps in Windows.".into());
    }
    std::process::Command::new(path)
        .spawn()
        .map_err(|error| format!("Could not open the Thermal Berry uninstaller: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall(app: &AppHandle) -> Result<(), String> {
    use objc2_foundation::{NSBundle, NSFileManager};
    use tauri_plugin_autostart::ManagerExt;

    let bundle = NSBundle::mainBundle();
    if bundle
        .bundleIdentifier()
        .map(|id| id.to_string())
        .as_deref()
        != Some(app.config().identifier.as_str())
    {
        return Err(
            "Open the installed Thermal Berry app from Applications to uninstall it.".into(),
        );
    }
    let autostart = app.autolaunch();
    let enabled = autostart
        .is_enabled()
        .map_err(|error| format!("Autostart: {error}"))?;
    if enabled {
        autostart
            .disable()
            .map_err(|error| format!("Could not remove login item: {error}"))?;
    }
    if let Err(error) = NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&bundle.bundleURL(), None)
    {
        let message = format!("Could not move Thermal Berry to the Trash: {error}");
        if enabled {
            autostart
                .enable()
                .map_err(|error| format!("{message}. Could not restore login item: {error}"))?;
        }
        return Err(message);
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn uninstall(_app: &AppHandle) -> Result<(), String> {
    Err("Uninstall is unavailable on this platform.".into())
}
