use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands;
use crate::config::FanMode;
use crate::state::AppState;

pub const TRAY_ID: &str = "main";

const MENU_STATUS: &str = "status";
const MENU_OPEN: &str = "open";
const MENU_BOOST_ALL: &str = "boost_all";
const MENU_MODE_AUTO: &str = "mode_auto";
const MENU_MODE_DISABLED: &str = "mode_disabled";
const MENU_QUIT: &str = "quit";

/// Handles to the menu items that change after creation. Kept in Tauri's
/// managed state so the monitor thread and commands can update them.
///
/// Manual mode isn't offered here on purpose: it's for tuning individual
/// fans (Manual Control in the window), not a quick tray action. The tray
/// only ever has one of these three checked at a time — see `sync_mode` /
/// `sync_all_fans_boost` and the `MENU_MODE_AUTO` / `MENU_MODE_DISABLED`
/// handlers below.
pub struct TrayHandles<R: Runtime> {
    status: MenuItem<R>,
    boost_all: CheckMenuItem<R>,
    mode_auto: CheckMenuItem<R>,
    mode_disabled: CheckMenuItem<R>,
}

/// Builds the tray icon with its menu. The window close button hides to the
/// tray (configured in lib.rs); the only way to quit the app is the tray's
/// Quit entry, so background monitoring/curve control keeps running.
pub fn setup(app: &AppHandle, state: Arc<AppState>) -> tauri::Result<()> {
    let (mode, boost_all_enabled) = {
        let inner = state.lock();
        (inner.config.mode, inner.config.all_fans_boost.enabled)
    };

    let status = MenuItem::with_id(app, MENU_STATUS, "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Thermal Berry", true, None::<&str>)?;
    let boost_all = CheckMenuItem::with_id(
        app, MENU_BOOST_ALL, "Boost all fans", true, boost_all_enabled, None::<&str>,
    )?;
    let mode_auto = CheckMenuItem::with_id(
        app, MENU_MODE_AUTO, "Auto mode", true, mode == FanMode::Auto, None::<&str>,
    )?;
    let mode_disabled = CheckMenuItem::with_id(
        app, MENU_MODE_DISABLED, "Disable fan control", true, mode == FanMode::Disabled, None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &boost_all,
            &mode_auto,
            &mode_disabled,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    app.manage(TrayHandles {
        status,
        boost_all,
        mode_auto,
        mode_disabled,
    });

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(app.default_window_icon().cloned().expect("bundled icon"))
        .tooltip("Thermal Berry")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_QUIT => app.exit(0),
            MENU_BOOST_ALL => {
                let (enabled, percent) = {
                    let inner = state.lock();
                    (!inner.config.all_fans_boost.enabled, inner.config.all_fans_boost.percent)
                };
                if let Err(e) = commands::apply_all_fans_boost(&state, enabled, percent) {
                    eprintln!("thermal-berry: tray could not toggle boost: {e}");
                }
                // Re-sync from the real config: on error this reverts the click.
                let inner = state.lock();
                sync_mode(app, inner.config.mode);
                sync_all_fans_boost(app, inner.config.all_fans_boost.enabled);
            }
            MENU_MODE_AUTO => {
                // Only one mode is ever active: picking Auto also turns off
                // the boost-all switch, so the items never disagree.
                let percent = state.lock().config.all_fans_boost.percent;
                if let Err(e) = commands::apply_all_fans_boost(&state, false, percent) {
                    eprintln!("thermal-berry: tray could not set auto mode: {e}");
                }
                // Re-sync from the real config: on error this reverts the click.
                let inner = state.lock();
                sync_mode(app, inner.config.mode);
                sync_all_fans_boost(app, inner.config.all_fans_boost.enabled);
            }
            MENU_MODE_DISABLED => {
                // Same mutual-exclusion rule: disabling control also turns
                // off the boost-all switch before switching mode.
                let percent = state.lock().config.all_fans_boost.percent;
                if let Err(e) = commands::apply_all_fans_boost(&state, false, percent) {
                    eprintln!("thermal-berry: tray could not clear boost: {e}");
                }
                if let Err(e) = commands::apply_mode(&state, FanMode::Disabled) {
                    eprintln!("thermal-berry: tray could not disable fan control: {e}");
                }
                // Re-sync from the real config: on error this reverts the click.
                let inner = state.lock();
                sync_mode(app, inner.config.mode);
                sync_all_fans_boost(app, inner.config.all_fans_boost.enabled);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.webview_windows().values().next() {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Reflects the active mode in the tray's check items.
pub fn sync_mode<R: Runtime>(app: &AppHandle<R>, mode: FanMode) {
    let Some(handles) = app.try_state::<TrayHandles<R>>() else {
        return;
    };
    let _ = handles.mode_auto.set_checked(mode == FanMode::Auto);
    let _ = handles.mode_disabled.set_checked(mode == FanMode::Disabled);
}

/// Reflects the "boost all fans" switch in the tray's check item. Called
/// after the tray toggles it and after Settings changes it via `set_config`.
pub fn sync_all_fans_boost<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let Some(handles) = app.try_state::<TrayHandles<R>>() else {
        return;
    };
    let _ = handles.boost_all.set_checked(enabled);
}

/// Updates the status line at the top of the tray menu and the icon tooltip.
/// Called by the monitor thread on each tick.
pub fn update_status<R: Runtime>(app: &AppHandle<R>, text: &str) {
    if let Some(handles) = app.try_state::<TrayHandles<R>>() {
        let _ = handles.status.set_text(text);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
    }
}
