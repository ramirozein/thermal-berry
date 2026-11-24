use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Monitor, PhysicalPosition, PhysicalRect,
    PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const LABEL: &str = "osd";
const WIDTH: f64 = 232.0;
const HEIGHT: f64 = 72.0;
const MARGIN: f64 = 16.0;

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Preferences {
    enabled: bool,
    monitor: Option<String>,
    corner: Corner,
    offset: Option<[i32; 2]>,
    always_on_top: bool,
    click_through: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            monitor: None,
            corner: Corner::TopRight,
            offset: None,
            always_on_top: true,
            click_through: false,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreferencesPatch {
    enabled: Option<bool>,
    monitor: Option<String>,
    corner: Option<Corner>,
    always_on_top: Option<bool>,
    click_through: Option<bool>,
}

struct Osd {
    path: Option<PathBuf>,
    config: Mutex<Config>,
}

struct Config {
    wayland: bool,
    preferences: Preferences,
    always_on_top_supported: bool,
    click_through_supported: bool,
    position_supported: bool,
    placement: Option<(PhysicalRect<i32, u32>, f64, PhysicalPosition<i32>)>,
}

impl Config {
    fn new(preferences: Preferences, wayland: bool) -> Self {
        Self {
            wayland,
            preferences,
            always_on_top_supported: !wayland,
            click_through_supported: !wayland,
            position_supported: !wayland,
            placement: None,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    preferences: Preferences,
    monitors: Vec<Display>,
    tray_available: bool,
    position_supported: bool,
    always_on_top_supported: bool,
    click_through_supported: bool,
}

#[derive(Clone, Serialize)]
struct Display {
    id: String,
    name: String,
}

pub fn setup(app: &AppHandle) {
    let path = app
        .path()
        .app_config_dir()
        .ok()
        .map(|directory| directory.join("osd.json"));
    let preferences = path.as_deref().map(load_preferences).unwrap_or_default();
    let enabled = preferences.enabled;
    let wayland = cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
            || std::env::var_os("WAYLAND_DISPLAY").is_some());
    app.manage(Osd {
        path,
        config: Mutex::new(Config::new(preferences, wayland)),
    });
    if enabled {
        spawn(app, |app| {
            let state = app.state::<Osd>();
            let mut config = state.config.lock().unwrap();
            let preferences = config.preferences.clone();
            match apply(app, &mut config, &preferences) {
                Ok(()) => crate::tray::sync_osd(app, true),
                Err(error) => {
                    config.preferences.enabled = false;
                    eprintln!("thermal-berry: could not restore OSD: {error}");
                }
            }
        });
    }
}

#[cfg(target_os = "linux")]
pub fn remove_preferences(app: &AppHandle) -> Result<(), crate::thermal::ThermalError> {
    if let Some(state) = app.try_state::<Osd>() {
        let _config = state.config.lock().unwrap();
        if let Some(path) = &state.path {
            crate::uninstall::remove_file_if_present(path)?;
        }
    }
    Ok(())
}

fn load_preferences(path: &Path) -> Preferences {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
        Err(error) => {
            eprintln!("thermal-berry: could not read OSD settings: {error}");
            Preferences::default()
        }
    }
}

fn save_preferences(path: Option<&Path>, preferences: &Preferences) -> Result<(), String> {
    let path = path.ok_or("OSD settings directory is unavailable")?;
    let temporary = path.with_extension("json.tmp");
    let data = serde_json::to_vec(preferences).map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or("OSD settings directory is unavailable")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let result = fs::write(&temporary, data).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|error| error.to_string())
}

fn displays(app: &AppHandle) -> Vec<(Display, Monitor)> {
    let monitors = app.available_monitors().unwrap_or_default();
    let mut names = std::collections::HashMap::new();
    monitors
        .into_iter()
        .map(|monitor| {
            let name = monitor.name().cloned().unwrap_or_else(|| "Display".into());
            let count = names.entry(name.clone()).or_insert(0);
            *count += 1;
            let display = Display {
                id: format!("{name}#{count}"),
                name: if *count == 1 {
                    name
                } else {
                    format!("{name} ({count})")
                },
            };
            (display, monitor)
        })
        .collect()
}

fn settings(app: &AppHandle, config: &Config) -> Settings {
    Settings {
        preferences: config.preferences.clone(),
        monitors: displays(app)
            .into_iter()
            .map(|(display, _)| display)
            .collect(),
        tray_available: crate::tray::available(app),
        position_supported: config.position_supported,
        always_on_top_supported: config.always_on_top_supported,
        click_through_supported: config.click_through_supported,
    }
}

fn spawn(app: &AppHandle, task: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || task(&handle));
}

#[tauri::command]
pub fn resize_osd(window: WebviewWindow, width: u32, height: u32) -> Result<(), String> {
    if window.label() != LABEL || !(1..=4096).contains(&width) || !(1..=4096).contains(&height) {
        return Err("invalid OSD window or dimensions".into());
    }
    let size = LogicalSize::new(width, height);
    let width = Some(tauri::LogicalUnit(width as f64).into());
    let height = Some(tauri::LogicalUnit(height as f64).into());
    window
        .set_size_constraints(tauri::WindowSizeConstraints {
            min_width: width,
            max_width: width,
            min_height: height,
            max_height: height,
        })
        .and_then(|()| window.set_size(size))
        .and_then(|()| if window.is_visible()? { Ok(()) } else { window.show() })
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn drag_osd(window: WebviewWindow) -> Result<(), String> {
    if window.label() != LABEL {
        return Err("invalid OSD window".into());
    }
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_osd_settings(app: AppHandle) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Osd>();
        let config = state.config.lock().unwrap();
        settings(&app, &config)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_osd_settings(app: AppHandle, patch: PreferencesPatch) -> Result<Settings, String> {
    tauri::async_runtime::spawn_blocking(move || update(&app, |_| patch))
        .await
        .map_err(|error| error.to_string())?
}

fn update(
    app: &AppHandle,
    patch: impl FnOnce(&Preferences) -> PreferencesPatch,
) -> Result<Settings, String> {
    let state = app.state::<Osd>();
    let mut config = state.config.lock().unwrap();
    if !crate::lifecycle::allow_settings(app) {
        return Err("OSD settings are unavailable during maintenance or shutdown".into());
    }
    let previous = config.preferences.clone();
    let patch = patch(&previous);
    let reposition = patch.monitor.is_some() || patch.corner.is_some();
    let mut next = Preferences {
        enabled: patch.enabled.unwrap_or(previous.enabled),
        monitor: match patch.monitor {
            Some(monitor) => (!monitor.is_empty()).then_some(monitor),
            None => previous.monitor.clone(),
        },
        corner: patch.corner.unwrap_or(previous.corner),
        offset: if reposition { None } else { previous.offset },
        always_on_top: patch.always_on_top.unwrap_or(previous.always_on_top),
        click_through: patch.click_through.unwrap_or(previous.click_through),
    };
    if reposition {
        config.placement = None;
    } else if !next.enabled && previous.enabled {
        if let Some(window) = app.get_webview_window(LABEL) {
            if let Some(offset) = place(app, &window, &mut config, &previous) {
                next.offset = Some(offset);
            }
        }
    }
    if let Err(error) = apply(app, &mut config, &next) {
        let _ = apply(app, &mut config, &previous);
        return Err(error.to_string());
    }
    if let Err(error) = save_preferences(state.path.as_deref(), &next) {
        let message = format!("OSD changes could not be saved: {error}");
        let _ = app.emit_to("main", "osd-error", &message);
        eprintln!("thermal-berry: {message}");
    }
    config.preferences = next;
    crate::tray::sync_osd(app, config.preferences.enabled);
    let current = settings(app, &config);
    let _ = app.emit_to("main", "osd-settings-changed", &current);
    Ok(current)
}

pub fn toggle_from_tray(app: &AppHandle) {
    spawn(app, |app| {
        if let Err(error) = update(app, |previous| PreferencesPatch {
            enabled: Some(!previous.enabled),
            ..Default::default()
        }) {
            let enabled = app
                .state::<Osd>()
                .config
                .lock()
                .unwrap()
                .preferences
                .enabled;
            crate::tray::sync_osd(app, enabled);
            let _ = app.emit_to("main", "osd-error", &error);
            eprintln!("thermal-berry: could not toggle OSD: {error}");
        }
    });
}

pub fn close_from_window(app: &AppHandle) {
    spawn(app, |app| {
        if let Err(error) = update(app, |_| PreferencesPatch {
            enabled: Some(false),
            ..Default::default()
        }) {
            let _ = app.emit_to("main", "osd-error", &error);
        }
    });
}

fn create_window(app: &AppHandle, transparent: bool) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Thermal Berry OSD")
        .decorations(false)
        .shadow(false)
        .transparent(transparent)
        .skip_taskbar(true)
        .min_inner_size(1.0, 1.0)
        .focused(false)
        .visible(false)
        .inner_size(WIDTH, HEIGHT)
        .build()
}

fn apply(app: &AppHandle, config: &mut Config, preferences: &Preferences) -> tauri::Result<()> {
    let existing = app.get_webview_window(LABEL);
    if !preferences.enabled {
        config.placement = None;
        return existing.map_or(Ok(()), |window| window.destroy());
    }
    let opening = existing.is_none();
    let window = match existing {
        Some(window) => window,
        None => create_window(app, true).or_else(|_| create_window(app, false))?,
    };
    if config.always_on_top_supported
        && window.set_always_on_top(preferences.always_on_top).is_err()
    {
        config.always_on_top_supported = false;
    }
    if config.click_through_supported
        && window
            .set_ignore_cursor_events(preferences.click_through)
            .is_err()
    {
        config.click_through_supported = false;
        let _ = window.set_ignore_cursor_events(false);
    }
    if config.placement.is_none() {
        place(app, &window, config, preferences);
    }
    if opening {
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(750));
            if matches!(window.is_visible(), Ok(false)) {
                if let Err(error) = window.show() {
                    eprintln!("thermal-berry: could not show OSD: {error}");
                }
            }
        });
    }
    Ok(())
}

pub fn refresh_position(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_none() {
        return;
    }
    spawn(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let state = app.state::<Osd>();
            let Ok(mut config) = state.config.try_lock() else {
                return;
            };
            let preferences = config.preferences.clone();
            if preferences.enabled {
                if let Some(offset) = place(app, &window, &mut config, &preferences) {
                    config.preferences.offset = Some(offset);
                    if let Err(error) = save_preferences(state.path.as_deref(), &config.preferences)
                    {
                        eprintln!("thermal-berry: could not save OSD position: {error}");
                    }
                }
            }
        }
    });
}

fn place(
    app: &AppHandle,
    window: &WebviewWindow,
    config: &mut Config,
    preferences: &Preferences,
) -> Option<[i32; 2]> {
    if config.wayland {
        return None;
    }
    let options = displays(app);
    let preferred = preferences.monitor.as_ref().and_then(|id| {
        options
            .iter()
            .find(|(display, _)| &display.id == id)
            .map(|(_, monitor)| monitor)
    });
    let primary = app.primary_monitor().ok().flatten();
    let monitor = preferred
        .or(primary.as_ref())
        .or_else(|| options.first().map(|(_, monitor)| monitor))?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let size = window
        .outer_size()
        .ok()
        .unwrap_or_else(|| LogicalSize::new(WIDTH, HEIGHT).to_physical(scale));
    let current = window.outer_position().ok();
    let previous = config
        .placement
        .as_ref()
        .filter(|(previous_area, previous_scale, _)| {
            previous_area.position == area.position
                && previous_area.size == area.size
                && *previous_scale == scale
        });
    let requested = match (previous, current) {
        (Some(_), Some(current)) => current,
        _ => preferences.offset.map_or_else(
            || corner_position(area, size, scale, preferences.corner),
            |[x, y]| {
                PhysicalPosition::new(
                    area.position
                        .x
                        .saturating_add((f64::from(x) * scale).round() as i32),
                    area.position
                        .y
                        .saturating_add((f64::from(y) * scale).round() as i32),
                )
            },
        ),
    };
    let position = clamp_position(area, size, requested);
    config.position_supported = current == Some(position) || window.set_position(position).is_ok();
    if !config.position_supported {
        return None;
    }
    let moved = previous.is_some_and(|(_, _, last)| Some(*last) != current);
    config.placement = Some((area.clone(), scale, position));
    let offset = [
        ((f64::from(position.x) - f64::from(area.position.x)) / scale).round() as i32,
        ((f64::from(position.y) - f64::from(area.position.y)) / scale).round() as i32,
    ];
    (moved && preferences.offset != Some(offset)).then_some(offset)
}

fn clamp_position(
    area: &PhysicalRect<i32, u32>,
    size: PhysicalSize<u32>,
    position: PhysicalPosition<i32>,
) -> PhysicalPosition<i32> {
    let clamp = |value: i32, origin: i32, span: u32, window: u32| {
        value.clamp(
            origin,
            origin.saturating_add_unsigned(span.saturating_sub(window)),
        )
    };
    PhysicalPosition::new(
        clamp(position.x, area.position.x, area.size.width, size.width),
        clamp(position.y, area.position.y, area.size.height, size.height),
    )
}

fn corner_position(
    area: &PhysicalRect<i32, u32>,
    size: PhysicalSize<u32>,
    scale: f64,
    corner: Corner,
) -> PhysicalPosition<i32> {
    let margin = (MARGIN * scale).round() as i32;
    let axis = |origin: i32, length: u32, window: u32, far: bool| {
        let remaining = length.saturating_sub(window) as i32;
        origin.saturating_add(if far {
            (remaining - margin).max(0)
        } else {
            margin.min(remaining)
        })
    };
    PhysicalPosition::new(
        axis(
            area.position.x,
            area.size.width,
            size.width,
            matches!(corner, Corner::TopRight | Corner::BottomRight),
        ),
        axis(
            area.position.y,
            area.size.height,
            size.height,
            matches!(corner, Corner::BottomLeft | Corner::BottomRight),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_to_the_selected_monitor_and_scales_the_margin() {
        let work = PhysicalRect {
            position: PhysicalPosition::new(-1920, 24),
            size: PhysicalSize::new(1920, 1000),
        };
        let size = PhysicalSize::new(400, 160);
        assert_eq!(
            corner_position(&work, size, 2.0, Corner::TopRight),
            PhysicalPosition::new(-432, 56)
        );
        assert_eq!(
            corner_position(&work, size, 1.0, Corner::BottomLeft),
            PhysicalPosition::new(-1904, 848)
        );
    }

    #[test]
    fn free_position_stays_inside_the_selected_screen() {
        let area = PhysicalRect {
            position: PhysicalPosition::new(-1920, 24),
            size: PhysicalSize::new(1920, 1000),
        };
        let size = PhysicalSize::new(320, 136);
        let inside = PhysicalPosition::new(-1085, 220);
        assert_eq!(clamp_position(&area, size, inside), inside);
        assert_eq!(
            clamp_position(&area, size, PhysicalPosition::new(-2000, 1100)),
            PhysicalPosition::new(-1920, 888)
        );
        assert_eq!(
            clamp_position(&area, size, PhysicalPosition::new(800, 0)),
            PhysicalPosition::new(-320, 24)
        );
    }

    #[test]
    fn restores_enabled_preference_and_defaults_missing_fields() {
        let saved = br#"{"enabled":true,"monitor":"Display#1","corner":"bottomLeft"}"#;
        let restored: Preferences = serde_json::from_slice(saved).unwrap();
        assert!(restored.enabled && restored.always_on_top);
        assert!(!restored.click_through);
        assert_eq!(restored.monitor.as_deref(), Some("Display#1"));
    }
}
