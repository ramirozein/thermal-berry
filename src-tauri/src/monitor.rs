use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use crate::config::FanMode;
use crate::curve;
use crate::state::{AppState, FanReading, Sample, TempReading};
use crate::tray;

pub const TELEMETRY_EVENT: &str = "telemetry";

/// How often old telemetry is purged from the database.
const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

/// Starts the monitoring thread: each tick reads sensors/fans, applies the
/// curve if applicable, saves the sample in the ring buffer + database and
/// emits it to the frontend. Once an hour it purges telemetry older than the
/// configured retention.
///
/// The interval is re-read from config on each tick, so changes from
/// Settings apply live; the sleep happens in 250ms slices so lowering the
/// interval doesn't have to wait for the previous long sleep.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    thread::spawn(move || {
        let mut last_purge: Option<Instant> = None;
        let mut last_status = String::new();
        loop {
            let started = Instant::now();
            let interval = {
                let inner = state.lock();
                Duration::from_secs(inner.config.interval_secs())
            };

            // Catches panics from a single tick (e.g. an unexpected hwmon
            // read) so this thread never dies silently: a dead monitor
            // thread means both live telemetry and curve-based fan control
            // stop with no indication to the user.
            let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tick(&state)));
            match tick_result {
                Ok(Some(sample)) => {
                    if let Err(e) = app.emit(TELEMETRY_EVENT, &sample) {
                        eprintln!("thermal-berry: error emitting telemetry: {e}");
                    }
                    let status = status_line(&sample);
                    if status != last_status {
                        tray::update_status(&app, &status);
                        last_status = status;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    let msg = e
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| e.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    eprintln!("thermal-berry: monitor tick panicked, skipping this cycle: {msg}");
                }
            }

            if last_purge.is_none_or(|t| t.elapsed() >= PURGE_INTERVAL) {
                last_purge = Some(Instant::now());
                let inner = state.lock();
                let retention_ms =
                    inner.config.retention_days() as u64 * 24 * 3600 * 1000;
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Err(e) =
                    inner.db.purge_older_than(now_ms.saturating_sub(retention_ms))
                {
                    eprintln!("thermal-berry: error purging old telemetry: {e}");
                }
            }

            while started.elapsed() < interval {
                let remaining = interval.saturating_sub(started.elapsed());
                thread::sleep(remaining.min(Duration::from_millis(250)));
                // If the user lowered the interval, cut the sleep short.
                let current = {
                    let inner = state.lock();
                    Duration::from_secs(inner.config.interval_secs())
                };
                if started.elapsed() >= current {
                    break;
                }
            }
        }
    });
}

/// Compact summary for the tray menu/tooltip, e.g. "CPU 52°C · GPU 48°C · 2400 RPM".
fn status_line(sample: &Sample) -> String {
    let mut parts: Vec<String> = sample
        .temps
        .iter()
        .map(|t| format!("{} {:.0}°C", t.label, t.celsius))
        .collect();
    parts.extend(
        sample
            .fans
            .iter()
            .filter_map(|f| f.rpm.map(|rpm| format!("{rpm} RPM"))),
    );
    if parts.is_empty() {
        "No readings".to_string()
    } else {
        parts.join(" · ")
    }
}

fn tick(state: &Arc<AppState>) -> Option<Sample> {
    let mut inner = state.lock();
    let device = inner.device.as_ref()?;

    let temps: Vec<TempReading> = device
        .sensors()
        .iter()
        .filter_map(|s| {
            s.read_celsius().ok().map(|celsius| TempReading {
                label: s.label().to_string(),
                celsius,
            })
        })
        .collect();

    let fans = device.fans();
    let fan_readings: Vec<FanReading> = fans
        .iter()
        .map(|f| FanReading {
            id: f.id().to_string(),
            label: f.label().to_string(),
            rpm: f.read_rpm().ok(),
            boost_percent: f
                .read_boost()
                .ok()
                .map(|b| curve::boost_to_percent(b, f.boost_range())),
            max_rpm: f.max_rpm(),
        })
        .collect();

    if inner.config.mode == FanMode::Auto {
        // Reference temp per fan: the sensor at the same index if it exists
        // (fan1↔temp1: CPU Fan↔CPU), otherwise the maximum temperature.
        let max_temp = temps.iter().map(|t| t.celsius).fold(f32::MIN, f32::max);
        for (i, fan) in fans.iter().enumerate() {
            let temp = temps.get(i).map(|t| t.celsius).unwrap_or(max_temp);
            if temp == f32::MIN {
                continue; // no temperature readings this tick
            }
            let points = match inner.config.curves.get(fan.id()) {
                Some(p) if !p.is_empty() => p.clone(),
                _ => curve::default_points(),
            };
            let hysteresis = inner.curve_states.entry(fan.id().to_string()).or_default();
            let percent = hysteresis.update(&points, temp);
            let boost = curve::percent_to_boost(percent, fan.boost_range());
            if let Err(e) = fan.set_boost(boost) {
                eprintln!("thermal-berry: curve could not write {}: {e}", fan.id());
            }
        }
    }

    let sample = Sample {
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        temps,
        fans: fan_readings,
    };
    inner.push_sample(sample.clone());
    if let Err(e) = inner.db.insert_sample(&sample) {
        eprintln!("thermal-berry: error persisting sample: {e}");
    }
    Some(sample)
}
