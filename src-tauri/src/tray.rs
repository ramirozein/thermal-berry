use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

#[cfg(target_os = "linux")]
use crate::{commands, config::FanMode, state::AppState};
#[cfg(target_os = "linux")]
use std::sync::Arc;

const TRAY_ID: &str = "main";
const MENU_OPEN: &str = "open";
const MENU_OSD: &str = "osd";
const MENU_QUIT: &str = "quit";
#[cfg(target_os = "linux")]
const MENU_BOOST_ALL: &str = "boost_all";
#[cfg(target_os = "linux")]
const MENU_MODE_AUTO: &str = "mode_auto";
#[cfg(target_os = "linux")]
const MENU_MODE_DISABLED: &str = "mode_disabled";

#[derive(Default)]
struct Summary {
    system: String,
    thermal: String,
    rendered: String,
}

struct TrayHandles<R: Runtime> {
    status: MenuItem<R>,
    osd: CheckMenuItem<R>,
    summary: Mutex<Summary>,
    #[cfg(target_os = "linux")]
    boost_all: CheckMenuItem<R>,
    #[cfg(target_os = "linux")]
    mode_auto: CheckMenuItem<R>,
    #[cfg(target_os = "linux")]
    mode_disabled: CheckMenuItem<R>,
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Thermal Berry", true, None::<&str>)?;
    let osd = CheckMenuItem::with_id(
        app,
        MENU_OSD,
        "On-screen display",
        true,
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&status, &PredefinedMenuItem::separator(app)?, &open, &osd],
    )?;

    #[cfg(target_os = "linux")]
    let (boost_all, mode_auto, mode_disabled) = {
        let state = app.state::<Arc<AppState>>();
        let (mode, boost_enabled) = {
            let inner = state.lock();
            (inner.config.mode, inner.config.all_fans_boost.enabled)
        };
        let boost = CheckMenuItem::with_id(
            app,
            MENU_BOOST_ALL,
            "Boost all fans",
            true,
            boost_enabled,
            None::<&str>,
        )?;
        let auto = CheckMenuItem::with_id(
            app,
            MENU_MODE_AUTO,
            "Auto mode",
            true,
            mode == FanMode::Auto,
            None::<&str>,
        )?;
        let disabled = CheckMenuItem::with_id(
            app,
            MENU_MODE_DISABLED,
            "Disable fan control",
            true,
            mode == FanMode::Disabled,
            None::<&str>,
        )?;
        menu.append_items(&[
            &PredefinedMenuItem::separator(app)?,
            &boost,
            &auto,
            &disabled,
        ])?;
        (boost, auto, disabled)
    };
    menu.append_items(&[&PredefinedMenuItem::separator(app)?, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().cloned().expect("bundled icon"))
        .tooltip("Thermal Berry")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_QUIT => crate::lifecycle::quit(app),
            MENU_OSD => crate::osd::toggle_from_tray(app),
            #[cfg(target_os = "linux")]
            id @ (MENU_BOOST_ALL | MENU_MODE_AUTO | MENU_MODE_DISABLED) => apply_control(app, id),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayHandles {
        status,
        osd,
        summary: Mutex::new(Summary::default()),
        #[cfg(target_os = "linux")]
        boost_all,
        #[cfg(target_os = "linux")]
        mode_auto,
        #[cfg(target_os = "linux")]
        mode_disabled,
    });
    Ok(())
}

pub fn available(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(target_os = "linux")]
fn apply_control(app: &AppHandle, id: &str) {
    let state = app.state::<Arc<AppState>>();
    let result = if id == MENU_MODE_DISABLED {
        commands::apply_mode(&state, FanMode::Disabled)
    } else {
        let (enabled, percent) = {
            let inner = state.lock();
            (
                id == MENU_BOOST_ALL && !inner.config.all_fans_boost.enabled,
                inner.config.all_fans_boost.percent,
            )
        };
        commands::apply_all_fans_boost(&state, enabled, percent)
    };
    if let Err(error) = result {
        commands::report_control_error(&state, &error);
        eprintln!("thermal-berry: tray control failed: {error}");
    }
    commands::publish_state(app, &state);
}

#[cfg(target_os = "linux")]
pub fn sync_mode<R: Runtime>(app: &AppHandle<R>, mode: FanMode) {
    if let Some(handles) = app.try_state::<TrayHandles<R>>() {
        let _ = handles.mode_auto.set_checked(mode == FanMode::Auto);
        let _ = handles.mode_disabled.set_checked(mode == FanMode::Disabled);
    }
}

#[cfg(target_os = "linux")]
pub fn sync_all_fans_boost<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    if let Some(handles) = app.try_state::<TrayHandles<R>>() {
        let _ = handles.boost_all.set_checked(enabled);
    }
}

pub fn sync_osd(app: &AppHandle, enabled: bool) {
    if let Some(handles) = app.try_state::<TrayHandles<tauri::Wry>>() {
        let _ = handles.osd.set_checked(enabled);
    }
}

pub fn update_system_status(app: &AppHandle, text: String) {
    update_summary(app, Some(text), None);
}

#[cfg(target_os = "linux")]
pub fn update_status(app: &AppHandle, text: &str) {
    update_summary(app, None, Some(text.into()));
}

fn update_summary(app: &AppHandle, system: Option<String>, thermal: Option<String>) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(handles) = handle.try_state::<TrayHandles<tauri::Wry>>() {
            let mut summary = handles.summary.lock().unwrap();
            if let Some(system) = system {
                summary.system = system;
            }
            if let Some(thermal) = thermal {
                summary.thermal = thermal;
            }
            let text = [summary.system.as_str(), summary.thermal.as_str()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            if summary.rendered == text {
                return;
            }
            summary.rendered = text.clone();
            let _ = handles.status.set_text(&text);
            if let Some(tray) = handle.tray_by_id(TRAY_ID) {
                let _ = tray.set_tooltip(Some(text));
            }
        }
    });
}
