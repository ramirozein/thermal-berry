#[cfg(target_os = "linux")]
pub mod automatic;
#[cfg(target_os = "linux")]
pub mod commands;
#[cfg(target_os = "linux")]
pub mod config;
#[cfg(target_os = "linux")]
pub mod curve;
#[cfg(target_os = "linux")]
pub mod db;
#[cfg(not(target_os = "linux"))]
mod desktop_uninstall;
mod lifecycle;
mod metrics;
#[cfg(target_os = "linux")]
pub mod monitor;
mod osd;
mod preferences;
#[cfg(target_os = "linux")]
pub mod state;
#[cfg(target_os = "linux")]
pub mod thermal;
pub mod tray;
#[cfg(target_os = "linux")]
pub mod uninstall;
#[cfg(target_os = "linux")]
pub mod update;
#[cfg(target_os = "windows")]
mod windows_hardware;
#[cfg(target_os = "windows")]
mod windows_temperatures;

#[cfg(target_os = "linux")]
mod fan_control;
pub mod readings;

#[cfg(target_os = "linux")]
use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

#[cfg(target_os = "linux")]
use db::Db;
#[cfg(target_os = "linux")]
use state::AppState;

fn builder() -> tauri::Builder<tauri::Wry> {
    let builder = tauri::Builder::default();
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        tray::show_main_window(app)
    }));
    builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(metrics::Metrics::default())
        .manage(lifecycle::Lifecycle::default())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    if tray::available(window.app_handle()) {
                        let _ = window.hide();
                    } else {
                        lifecycle::quit(window.app_handle());
                    }
                } else if window.label() == "osd" {
                    api.prevent_close();
                    osd::close_from_window(window.app_handle());
                }
            }
        })
}

fn setup_desktop(app: &tauri::App) {
    if let Err(error) = tray::setup(app.handle()) {
        eprintln!("thermal-berry: tray unavailable: {error}");
    }
    osd::setup(app.handle());
    app.manage(metrics::spawn(app.handle().clone()));
}

fn on_run(app: &tauri::AppHandle, event: RunEvent) {
    #[cfg(target_os = "macos")]
    if matches!(&event, RunEvent::Reopen { .. }) {
        tray::show_main_window(app);
    }
    if let RunEvent::ExitRequested { api, .. } = &event {
        if lifecycle::defer_exit(app) {
            api.prevent_exit();
        }
    }
    if let RunEvent::Exit = event {
        if let Some(worker) = app.try_state::<metrics::MetricsWorker>() {
            worker.stop();
        }
        #[cfg(target_os = "linux")]
        {
            let state = app.state::<Arc<AppState>>();
            fan_control::return_to_firmware(&state);
            if let Some(history) = app.try_state::<monitor::HistoryWriter>() {
                history.flush();
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    builder()
        .setup(|app| {
            preferences::desktop::setup(app.handle());
            setup_desktop(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            readings::get_capabilities,
            metrics::get_metrics_snapshot,
            osd::get_osd_settings,
            osd::set_osd_settings,
            osd::resize_osd,
            osd::drag_osd,
            lifecycle::quit_app,
            preferences::desktop::get_preferences,
            preferences::desktop::set_preferences,
            preferences::get_autostart,
            preferences::set_autostart,
            desktop_uninstall::uninstall_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(on_run);
}

#[cfg(target_os = "linux")]
pub fn run() {
    let db = Db::open().unwrap_or_else(|e| {
        eprintln!("thermal-berry: could not open database ({e}), running in-memory");
        Db::open_in_memory().expect("in-memory sqlite always opens")
    });
    let config = db.load_config();
    let device = config
        .vendor_override
        .as_deref()
        .and_then(|vendor| thermal::create_device(vendor).ok())
        .or_else(|| thermal::detect_device().ok());
    let state = Arc::new(AppState::new(device, config, db));

    let app = builder()
        .manage(state.clone())
        .setup(move |app| {
            commands::restore_control(&state);
            setup_desktop(app);
            app.manage(monitor::spawn(app.handle().clone(), state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            readings::get_capabilities,
            metrics::get_metrics_snapshot,
            osd::get_osd_settings,
            osd::set_osd_settings,
            osd::resize_osd,
            osd::drag_osd,
            lifecycle::quit_app,
            commands::get_device_info,
            commands::get_history,
            commands::get_history_range,
            commands::get_config,
            commands::get_state,
            commands::set_config,
            commands::set_fan_boost,
            commands::set_all_fans_boost,
            commands::set_mode,
            commands::save_curve,
            commands::select_vendor,
            commands::check_write_access,
            commands::install_udev_rule,
            preferences::get_autostart,
            preferences::set_autostart,
            update::check_for_update,
            update::install_update,
            uninstall::uninstall_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(on_run);
}

#[cfg(test)]
mod ipc_tests {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{Manager, WebviewWindowBuilder};

    use super::metrics::{self, Metrics};

    #[tauri::command]
    fn set_osd_settings() -> bool {
        true
    }

    #[tauri::command]
    fn quit_app() -> bool {
        true
    }

    #[tauri::command]
    fn set_fan_boost() -> bool {
        true
    }

    #[tauri::command]
    fn resize_osd() -> bool {
        true
    }

    #[tauri::command]
    fn get_preferences() -> bool {
        true
    }

    #[tauri::command]
    fn set_preferences() -> bool {
        true
    }

    #[tauri::command]
    fn set_autostart() -> bool {
        true
    }

    #[tauri::command]
    fn uninstall_app() -> bool {
        true
    }

    #[test]
    fn the_osd_can_read_metrics_but_cannot_mutate_application_state() {
        let app = mock_builder()
            .manage(Metrics::default())
            .invoke_handler(tauri::generate_handler![
                metrics::get_metrics_snapshot,
                set_osd_settings,
                quit_app,
                set_fan_boost,
                resize_osd,
                get_preferences,
                set_preferences,
                set_autostart,
                uninstall_app
            ])
            .build(tauri::generate_context!())
            .unwrap();
        let main = app.get_webview_window("main").unwrap_or_else(|| {
            WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap()
        });
        let osd = WebviewWindowBuilder::new(&app, "osd", Default::default())
            .build()
            .unwrap();
        let request = |command: &str| InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: app
                .config()
                .build
                .dev_url
                .clone()
                .unwrap_or_else(|| "http://tauri.localhost".parse().unwrap()),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        assert!(get_ipc_response(&osd, request("get_metrics_snapshot")).is_ok());
        assert!(get_ipc_response(&osd, request("resize_osd")).is_ok());
        assert!(get_ipc_response(&main, request("resize_osd")).is_err());
        assert!(get_ipc_response(&main, request("set_osd_settings")).is_ok());
        assert!(get_ipc_response(&main, request("quit_app")).is_ok());
        for command in [
            "set_osd_settings",
            "quit_app",
            "set_fan_boost",
            "get_preferences",
            "set_preferences",
            "set_autostart",
            "uninstall_app",
        ] {
            assert!(
                get_ipc_response(&osd, request(command)).is_err(),
                "{command}"
            );
        }
        assert_eq!(
            get_ipc_response(&main, request("set_fan_boost")).is_ok(),
            cfg!(target_os = "linux")
        );
        assert!(get_ipc_response(&main, request("set_autostart")).is_ok());
        assert!(get_ipc_response(&main, request("uninstall_app")).is_ok());
        for command in ["get_preferences", "set_preferences"] {
            assert_eq!(
                get_ipc_response(&main, request(command)).is_ok(),
                cfg!(any(target_os = "windows", target_os = "macos")),
                "{command}"
            );
        }
    }
}
