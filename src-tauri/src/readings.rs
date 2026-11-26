use serde::Serialize;
pub use thermal_berry_core::readings::{now_ms, Readings, TempReading};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    host_name: Option<String>,
    platform: &'static str,
    cpu_usage: bool,
    memory: bool,
    temperatures: bool,
    fan_rpm: bool,
    fan_control: bool,
    write_access: bool,
    linux_maintenance: bool,
    #[cfg(target_os = "linux")]
    device: Option<crate::commands::DeviceInfo>,
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub fn get_capabilities(_metrics: tauri::State<'_, crate::metrics::Metrics>) -> Capabilities {
    #[cfg(target_os = "windows")]
    let (temperatures, fan_rpm) = crate::metrics::windows_reading_capabilities(&_metrics);
    #[cfg(not(target_os = "windows"))]
    let (temperatures, fan_rpm) = (false, false);
    Capabilities {
        host_name: sysinfo::System::host_name().ok(),
        platform: std::env::consts::OS,
        cpu_usage: true,
        memory: true,
        temperatures,
        fan_rpm,
        fan_control: false,
        write_access: false,
        linux_maintenance: false,
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn get_capabilities(
    state: tauri::State<'_, std::sync::Arc<crate::state::AppState>>,
) -> Result<Capabilities, crate::thermal::ThermalError> {
    let device = crate::commands::get_device_info(state).await;
    let device = match device {
        Ok(device) => Some(device),
        Err(crate::thermal::ThermalError::DeviceNotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(Capabilities {
        host_name: sysinfo::System::host_name().ok(),
        platform: std::env::consts::OS,
        cpu_usage: true,
        memory: true,
        temperatures: device.as_ref().is_some_and(|d| !d.sensors.is_empty()),
        fan_rpm: device.as_ref().is_some_and(|d| !d.fans.is_empty()),
        fan_control: device.as_ref().is_some_and(|d| d.fan_control),
        write_access: device.as_ref().is_some_and(|d| d.write_access),
        linux_maintenance: true,
        device,
    })
}
