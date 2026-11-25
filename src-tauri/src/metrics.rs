use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{MemoryRefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::readings::now_ms;

pub const INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadingStatus {
    #[default]
    Pending,
    Available,
    Unavailable,
    TemporaryError,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading<T> {
    value: Option<T>,
    status: ReadingStatus,
    timestamp_ms: Option<u64>,
}

impl<T> Reading<T> {
    fn sampled(value: Option<T>, timestamp_ms: u64) -> Self {
        Self {
            status: if value.is_some() {
                ReadingStatus::Available
            } else {
                ReadingStatus::TemporaryError
            },
            value,
            timestamp_ms: Some(timestamp_ms),
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    used_bytes: u64,
    total_bytes: u64,
    percent: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Temperature {
    pub id: String,
    pub label: String,
    pub celsius: f32,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Serialize)]
pub(crate) struct HardwareModel {
    pub vendor: &'static str,
    pub model: String,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Serialize)]
pub(crate) struct FanSpeed {
    pub id: String,
    pub label: String,
    pub rpm: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    sequence: u64,
    cpu_percent: Reading<f32>,
    memory: Reading<Memory>,
    temperatures: Reading<Vec<Temperature>>,
    temperature_expires_ms: Option<u64>,
    #[cfg(target_os = "windows")]
    thermal_zones: Reading<Vec<Temperature>>,
    #[cfg(target_os = "windows")]
    fan_speeds: Reading<Vec<FanSpeed>>,
    #[cfg(target_os = "windows")]
    hardware_expires_ms: Option<u64>,
    #[cfg(target_os = "windows")]
    hardware_model: Option<HardwareModel>,
    interval_ms: u64,
    temp_unit: crate::preferences::TempUnit,
}

impl Default for MetricsSnapshot {
    fn default() -> Self {
        Self {
            sequence: 0,
            cpu_percent: Reading::default(),
            memory: Reading::default(),
            temperatures: Reading {
                value: None,
                timestamp_ms: None,
                status: if cfg!(any(target_os = "linux", target_os = "windows")) {
                    ReadingStatus::Pending
                } else {
                    ReadingStatus::Unavailable
                },
            },
            temperature_expires_ms: None,
            #[cfg(target_os = "windows")]
            thermal_zones: Reading { value: None, status: ReadingStatus::Pending, timestamp_ms: None },
            #[cfg(target_os = "windows")]
            fan_speeds: Reading { value: None, status: ReadingStatus::Pending, timestamp_ms: None },
            #[cfg(target_os = "windows")]
            hardware_expires_ms: None,
            #[cfg(target_os = "windows")]
            hardware_model: None,
            interval_ms: INTERVAL.as_millis() as u64,
            temp_unit: crate::preferences::TempUnit::Celsius,
        }
    }
}

#[derive(Default)]
pub struct Metrics(Mutex<MetricsSnapshot>);

enum WorkerSignal {
    Stop,
    #[cfg(not(target_os = "linux"))]
    Refresh,
}

pub struct MetricsWorker {
    system: mpsc::Sender<WorkerSignal>,
    #[cfg(target_os = "windows")]
    temperatures: mpsc::Sender<WorkerSignal>,
    #[cfg(target_os = "windows")]
    hardware: mpsc::Sender<WorkerSignal>,
}

impl MetricsWorker {
    pub fn stop(&self) {
        let _ = self.system.send(WorkerSignal::Stop);
        #[cfg(target_os = "windows")]
        let _ = self.temperatures.send(WorkerSignal::Stop);
        #[cfg(target_os = "windows")]
        let _ = self.hardware.send(WorkerSignal::Stop);
    }

    #[cfg(not(target_os = "linux"))]
    pub fn refresh(&self) {
        let _ = self.system.send(WorkerSignal::Refresh);
        #[cfg(target_os = "windows")]
        let _ = self.temperatures.send(WorkerSignal::Refresh);
        #[cfg(target_os = "windows")]
        let _ = self.hardware.send(WorkerSignal::Refresh);
    }
}

fn should_stop(stopped: &mpsc::Receiver<WorkerSignal>, interval: Duration) -> bool {
    matches!(
        stopped.recv_timeout(interval),
        Ok(WorkerSignal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected)
    )
}

#[tauri::command]
pub fn get_metrics_snapshot(state: State<'_, Metrics>) -> MetricsSnapshot {
    state.0.lock().unwrap().clone()
}

fn publish(app: &AppHandle, change: impl FnOnce(&mut MetricsSnapshot)) {
    let state = app.state::<Metrics>();
    let snapshot = {
        let mut current = state.0.lock().unwrap();
        change(&mut current);
        current.sequence += 1;
        current.clone()
    };
    if let Err(error) = app.emit("metrics-updated", &snapshot) {
        eprintln!("thermal-berry: metrics event failed: {error}");
    }
}

pub fn spawn(app: AppHandle) -> MetricsWorker {
    #[cfg(target_os = "windows")]
    let temperatures = spawn_windows_temperatures(app.clone());
    #[cfg(target_os = "windows")]
    let hardware = spawn_windows_hardware(app.clone());
    let (stop, stopped) = mpsc::channel();
    std::thread::spawn(move || {
        let mut system = loop {
            match System::new() {
                Ok(system) => break system,
                Err(error) => {
                    eprintln!("thermal-berry: could not initialize system metrics: {error}");
                    let timestamp = now_ms();
                    publish(&app, |snapshot| {
                        snapshot.cpu_percent = Reading::sampled(None, timestamp);
                        snapshot.memory = Reading::sampled(None, timestamp);
                    });
                    if should_stop(&stopped, INTERVAL) {
                        return;
                    }
                }
            }
        };
        system.refresh_cpu_usage();
        let mut previous_cpu = Instant::now();
        loop {
            #[cfg(target_os = "linux")]
            let (interval, temp_unit) = (INTERVAL, crate::preferences::TempUnit::Celsius);
            #[cfg(not(target_os = "linux"))]
            let (interval, temp_unit) = {
                use crate::preferences::desktop::{get_preferences, DesktopPreferences};
                let preferences = get_preferences(app.state::<DesktopPreferences>());
                (
                    Duration::from_secs(preferences.update_interval_secs),
                    preferences.temp_unit,
                )
            };
            let interval = interval.max(MINIMUM_CPU_UPDATE_INTERVAL);
            let timestamp = now_ms();
            system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
            let memory = memory_reading(system.used_memory(), system.total_memory());
            let cpu = if previous_cpu.elapsed() >= MINIMUM_CPU_UPDATE_INTERVAL {
                system.refresh_cpu_usage();
                previous_cpu = Instant::now();
                Some(Reading::sampled(
                    (!system.cpus().is_empty())
                        .then(|| system.global_cpu_usage())
                        .filter(|usage| (0.0..=100.0).contains(usage)),
                    timestamp,
                ))
            } else {
                None
            };
            let summary = format!(
                "CPU {} · RAM {}",
                cpu.as_ref()
                    .and_then(|reading| reading.value)
                    .map_or("—".into(), |v| format!("{v:.0}%")),
                memory
                    .as_ref()
                    .map_or("—".into(), |v| format!("{:.0}%", v.percent)),
            );
            publish(&app, |snapshot| {
                snapshot.memory = Reading::sampled(memory, timestamp);
                snapshot.interval_ms = interval.as_millis() as u64;
                snapshot.temp_unit = temp_unit;
                if let Some(cpu) = cpu {
                    snapshot.cpu_percent = cpu;
                }
            });
            crate::tray::update_system_status(&app, summary);
            crate::osd::refresh_position(&app);
            if should_stop(&stopped, interval) {
                break;
            }
        }
    });
    MetricsWorker {
        system: stop,
        #[cfg(target_os = "windows")]
        temperatures,
        #[cfg(target_os = "windows")]
        hardware,
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_temperatures(app: AppHandle) -> mpsc::Sender<WorkerSignal> {
    use crate::preferences::desktop::{get_preferences, DesktopPreferences};
    use crate::windows_temperatures;

    let (stop, stopped) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let preferences = get_preferences(app.state::<DesktopPreferences>());
            let interval = Duration::from_secs(preferences.update_interval_secs);
            let readings: Vec<_> = windows_temperatures::gpu_temperature().into_iter().collect();
            let available = !readings.is_empty();
            publish_temperatures(&app, readings, now_ms(), interval, available);
            if should_stop(&stopped, interval) {
                break;
            }
        }
    });
    stop
}

#[cfg(target_os = "windows")]
fn spawn_windows_hardware(app: AppHandle) -> mpsc::Sender<WorkerSignal> {
    use crate::preferences::desktop::{get_preferences, DesktopPreferences};
    use crate::windows_hardware::Wmi;

    let (stop, stopped) = mpsc::channel();
    std::thread::spawn(move || {
        let wmi = Wmi::local();
        let model = wmi.as_ref().and_then(Wmi::hardware_model);
        loop {
            let preferences = get_preferences(app.state::<DesktopPreferences>());
            let interval = Duration::from_secs(preferences.update_interval_secs);
            let zones = wmi.as_ref().map_or(Ok(Vec::new()), Wmi::temperatures);
            let fans = wmi.as_ref().map_or(Ok(Vec::new()), Wmi::fan_speeds);
            let timestamp = now_ms();
            publish(&app, |snapshot| {
                snapshot.hardware_model.clone_from(&model);
                snapshot.thermal_zones = hardware_reading(zones, timestamp);
                snapshot.fan_speeds = hardware_reading(fans, timestamp);
                snapshot.hardware_expires_ms = Some(reading_expiry(timestamp, interval));
            });
            if wmi.is_none() || should_stop(&stopped, interval) {
                break;
            }
        }
    });
    stop
}

#[cfg(target_os = "windows")]
fn hardware_reading<T>(result: windows::core::Result<Vec<T>>, timestamp: u64) -> Reading<Vec<T>> {
    match result {
        Ok(values) if !values.is_empty() => Reading::sampled(Some(values), timestamp),
        Ok(_) => Reading { value: None, status: ReadingStatus::Unavailable, timestamp_ms: Some(timestamp) },
        Err(_) => Reading::sampled(None, timestamp),
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn has_windows_fan_readings(metrics: &Metrics) -> bool {
    let snapshot = metrics.0.lock().unwrap();
    snapshot.hardware_expires_ms.is_some_and(|expires| now_ms() <= expires)
        && matches!(snapshot.fan_speeds.status, ReadingStatus::Available)
}

fn memory_reading(used_bytes: u64, total_bytes: u64) -> Option<Memory> {
    if total_bytes == 0 || used_bytes > total_bytes {
        return None;
    }
    Some(Memory {
        used_bytes,
        total_bytes,
        percent: used_bytes as f64 / total_bytes as f64 * 100.0,
    })
}

#[cfg(target_os = "linux")]
pub fn publish_linux_temperatures(
    app: &AppHandle,
    sample: &crate::state::Sample,
    interval: Duration,
    device_available: bool,
) {
    let mut labels = std::collections::HashMap::new();
    let values: Vec<_> = sample
        .temps
        .iter()
        .map(|reading| {
            let count = labels.entry(&reading.label).or_insert(0);
            *count += 1;
            Temperature {
                id: format!("hwmon:{}:{count}", reading.label),
                label: reading.label.clone(),
                celsius: reading.celsius,
            }
        })
        .collect();
    publish_temperatures(app, values, sample.timestamp_ms, interval, device_available);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn publish_temperatures(
    app: &AppHandle,
    values: Vec<Temperature>,
    timestamp_ms: u64,
    interval: Duration,
    device_available: bool,
) {
    publish(app, |snapshot| {
        snapshot.temperatures =
            Reading::sampled((!values.is_empty()).then_some(values), timestamp_ms);
        if !device_available {
            snapshot.temperatures.status = ReadingStatus::Unavailable;
        }
        snapshot.temperature_expires_ms =
            Some(reading_expiry(timestamp_ms, interval));
    });
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn reading_expiry(timestamp: u64, interval: Duration) -> u64 {
    timestamp.saturating_add((interval.as_millis() as u64 * 5 / 2).max(5000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_has_no_cpu_value() {
        let snapshot = MetricsSnapshot::default();
        assert!(snapshot.cpu_percent.value.is_none());
        assert!(matches!(
            snapshot.cpu_percent.status,
            ReadingStatus::Pending
        ));
    }

    #[test]
    fn memory_percent_and_missing_values() {
        assert_eq!(memory_reading(3, 4).unwrap().percent, 75.0);
        assert_eq!(memory_reading(0, 4).unwrap().percent, 0.0);
        assert!(memory_reading(0, 0).is_none());
        assert!(memory_reading(5, 4).is_none());
    }
}
