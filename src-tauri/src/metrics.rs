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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    sequence: u64,
    cpu_percent: Reading<f32>,
    memory: Reading<Memory>,
    temperatures: Reading<Vec<Temperature>>,
    temperature_expires_ms: Option<u64>,
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
                status: if cfg!(target_os = "linux") {
                    ReadingStatus::Pending
                } else {
                    ReadingStatus::Unavailable
                },
            },
            temperature_expires_ms: None,
        }
    }
}

#[derive(Default)]
pub struct Metrics(Mutex<MetricsSnapshot>);

pub struct MetricsWorker(mpsc::Sender<()>);

impl MetricsWorker {
    pub fn stop(&self) {
        let _ = self.0.send(());
    }
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
                    if stopped.recv_timeout(INTERVAL) != Err(mpsc::RecvTimeoutError::Timeout) {
                        return;
                    }
                }
            }
        };
        system.refresh_cpu_usage();
        let mut previous_cpu = Instant::now();
        loop {
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
                if let Some(cpu) = cpu {
                    snapshot.cpu_percent = cpu;
                }
            });
            crate::tray::update_system_status(&app, summary);
            crate::osd::refresh_position(&app);
            if matches!(
                stopped.recv_timeout(INTERVAL.max(MINIMUM_CPU_UPDATE_INTERVAL)),
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
            ) {
                break;
            }
        }
    });
    MetricsWorker(stop)
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
    publish(app, |snapshot| {
        snapshot.temperatures =
            Reading::sampled((!values.is_empty()).then_some(values), sample.timestamp_ms);
        if !device_available {
            snapshot.temperatures.status = ReadingStatus::Unavailable;
        }
        snapshot.temperature_expires_ms = Some(
            sample
                .timestamp_ms
                .saturating_add((interval.as_millis() as u64 * 5 / 2).max(5000)),
        );
    });
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
