pub mod commands;
pub mod config;
pub mod curve;
pub mod db;
pub mod monitor;
pub mod state;
pub mod thermal;
pub mod tray;
pub mod update;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

use db::Db;
use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db = Db::open().unwrap_or_else(|e| {
        eprintln!("thermal-berry: could not open database ({e}), running in-memory");
        Db::open_in_memory().expect("in-memory sqlite always opens")
    });
    let config = db.load_config();
    let device = match config.vendor_override.as_deref() {
        Some(vendor) => thermal::create_device(vendor).ok(),
        None => thermal::detect_device().ok(),
    };
    let state = Arc::new(AppState::new(device, config, db));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state.clone())
        .setup(move |app| {
            tray::setup(app.handle(), state.clone())?;
            monitor::spawn(app.handle().clone(), state);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_device_info,
            commands::get_history,
            commands::get_history_range,
            commands::get_config,
            commands::set_config,
            commands::set_fan_boost,
            commands::set_all_fans_boost,
            commands::set_mode,
            commands::save_curve,
            commands::select_vendor,
            commands::check_write_access,
            commands::install_udev_rule,
            update::check_for_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            let state = app_handle.state::<Arc<AppState>>();
            let inner = state.lock();
            if let Some(device) = inner.device.as_ref() {
                for fan in device.fans() {
                    let _ = fan.set_boost(fan.boost_range().0);
                }
            }
        }
    });
}
