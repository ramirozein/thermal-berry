use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use crate::automatic::EMERGENCY_TEMP_C;
use crate::config::{AutoStrategy, FanMode};
use crate::curve;
use crate::state::{AppState, FanReading, Sample, TempReading};
use crate::tray;

pub const TELEMETRY_EVENT: &str = "telemetry";

const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

/// Starts the monitoring thread: each tick reads sensors/fans, applies the
/// selected automatic strategy, saves the sample in the ring buffer + database and
/// emits it to the frontend. Once an hour it purges telemetry older than the
/// configured retention.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    thread::spawn(move || {
        let mut last_purge: Option<Instant> = None;
        let mut last_status = String::new();
        let mut previous_tick = Instant::now();
        loop {
            let started = Instant::now();
            let interval = {
                let inner = state.lock();
                Duration::from_secs(control_interval(&inner.config))
            };

            let elapsed_secs = previous_tick.elapsed().as_secs_f32();
            previous_tick = Instant::now();
            let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tick(&state, elapsed_secs)
            }));
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
                if inner.control.maintenance || inner.db.is_none() {
                    continue;
                }
                let retention_ms = inner.config.retention_days() as u64 * 24 * 3600 * 1000;
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Err(e) = inner
                    .db
                    .as_ref()
                    .unwrap()
                    .purge_older_than(now_ms.saturating_sub(retention_ms))
                {
                    eprintln!("thermal-berry: error purging old telemetry: {e}");
                }
            }

            while started.elapsed() < interval {
                let remaining = interval.saturating_sub(started.elapsed());
                thread::sleep(remaining.min(Duration::from_millis(250)));
                let current = {
                    let inner = state.lock();
                    Duration::from_secs(control_interval(&inner.config))
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
    if !sample.errors.is_empty()
        || sample
            .control
            .as_ref()
            .is_some_and(|c| c.error.is_some() || c.suspended)
    {
        parts.insert(0, "Control / telemetry error".into());
    }
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

fn control_interval(config: &crate::config::Config) -> u64 {
    if config.mode == FanMode::Auto {
        config.interval_secs().min(2)
    } else {
        config.interval_secs()
    }
}

pub(crate) fn tick(state: &Arc<AppState>, elapsed_secs: f32) -> Option<Sample> {
    let mut inner = state.lock();
    if inner.control.maintenance || inner.db.is_none() {
        return None;
    }
    let mut errors = Vec::new();
    let (readings, fans) = if let Some(device) = inner.device.as_ref() {
        let readings: Vec<Option<TempReading>> = device
            .sensors()
            .iter()
            .map(|sensor| match sensor.read_celsius() {
                Ok(celsius) if celsius.is_finite() && (0.0..=150.0).contains(&celsius) => {
                    Some(TempReading {
                        label: sensor.label().into(),
                        celsius,
                    })
                }
                result => {
                    errors.push(format!(
                        "Sensor {} unavailable: {:?}",
                        sensor.label(),
                        result
                    ));
                    None
                }
            })
            .collect();
        (readings, device.fans())
    } else {
        errors.push("No compatible thermal device detected".into());
        (Vec::new(), Vec::new())
    };
    let missing_sensor = readings.is_empty() || readings.iter().any(Option::is_none);
    if readings.is_empty() {
        errors.push("No temperature sensors reporting".into());
    }
    if fans.is_empty() {
        errors.push("No fans detected".into());
    }
    let temps: Vec<TempReading> = readings.iter().flatten().cloned().collect();
    let mut fan_readings: Vec<FanReading> = fans
        .iter()
        .map(|fan| {
            let rpm = match fan.read_rpm() {
                Ok(value) => Some(value),
                Err(e) => {
                    errors.push(format!("{} RPM: {e}", fan.id()));
                    None
                }
            };
            let boost_percent = match fan.read_boost() {
                Ok(value) => Some(curve::boost_to_percent(value, fan.boost_range())),
                Err(e) => {
                    errors.push(format!("{} boost: {e}", fan.id()));
                    None
                }
            };
            FanReading {
                id: fan.id().into(),
                label: fan.label().into(),
                rpm,
                boost_percent,
                max_rpm: fan.max_rpm(),
            }
        })
        .collect();

    if inner.config.mode == FanMode::Auto && !inner.control.suspended {
        let mut all_applied = !fans.is_empty();
        let max_temp = temps.iter().map(|t| t.celsius).fold(f32::MIN, f32::max);
        for (i, fan) in fans.iter().enumerate() {
            let settings = inner.config.automatic_control;
            let percent = if settings.strategy == AutoStrategy::Adaptive {
                let current = fan_readings[i].boost_percent.unwrap_or(100);
                inner
                    .automatic_states
                    .entry(fan.id().to_string())
                    .or_default()
                    .update(
                        if missing_sensor { f32::NAN } else { max_temp },
                        settings.target_temp_c,
                        elapsed_secs,
                        current,
                    )
            } else if missing_sensor || max_temp >= EMERGENCY_TEMP_C {
                inner.curve_states.remove(fan.id());
                100
            } else {
                let temp = readings
                    .get(i)
                    .and_then(Option::as_ref)
                    .map(|t| t.celsius)
                    .unwrap_or(max_temp);
                let points = match inner.config.curves.get(fan.id()) {
                    Some(p) if !p.is_empty() => p.clone(),
                    _ => curve::default_points(),
                };
                inner
                    .curve_states
                    .entry(fan.id().to_string())
                    .or_default()
                    .update(&points, temp)
            };
            let boost = curve::percent_to_boost(percent, fan.boost_range());
            if let Err(e) = fan.set_boost(boost) {
                inner.automatic_states.remove(fan.id());
                inner.curve_states.remove(fan.id());
                all_applied = false;
                errors.push(format!(
                    "Automatic control could not write {}: {e}",
                    fan.id()
                ));
            } else {
                inner
                    .control
                    .applied_boosts
                    .insert(fan.id().into(), percent);
                inner.control.last_applied_ms = Some(crate::commands::now_ms());
            }
            // Observed boost and last successful command are separate values.
            fan_readings[i].boost_percent = match fan.read_boost() {
                Ok(value) => Some(curve::boost_to_percent(value, fan.boost_range())),
                Err(e) => {
                    errors.push(format!("{} boost readback: {e}", fan.id()));
                    None
                }
            };
        }
        inner.control.applied_mode = if all_applied {
            Some(FanMode::Auto)
        } else {
            None
        };
    }

    let mut sample = Sample {
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        temps,
        fans: fan_readings,
        control: Some(inner.snapshot().control),
        errors,
    };
    if let Err(e) = inner.db.as_ref().unwrap().insert_sample(&sample) {
        sample
            .errors
            .push(format!("Could not persist telemetry: {e}"));
    }
    inner.push_sample(sample.clone());
    Some(sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Db;
    use crate::thermal::{Fan, Result, TempSensor, ThermalDevice, ThermalError};
    use std::sync::Mutex;

    struct TestFan(Arc<Mutex<Vec<u8>>>, bool);

    impl Fan for TestFan {
        fn id(&self) -> &str {
            "fan1"
        }
        fn label(&self) -> &str {
            "Fan"
        }
        fn read_rpm(&self) -> Result<u32> {
            Ok(2000)
        }
        fn read_boost(&self) -> Result<u8> {
            Ok(20)
        }
        fn set_boost(&self, value: u8) -> Result<()> {
            self.0.lock().unwrap().push(value);
            if self.1 {
                return Err(ThermalError::Control("simulated write failure".into()));
            }
            Ok(())
        }
        fn boost_range(&self) -> (u8, u8) {
            (0, 100)
        }
        fn is_writable(&self) -> bool {
            true
        }
        fn max_rpm(&self) -> Option<u32> {
            None
        }
    }

    struct TestSensor(Option<f32>);

    impl TempSensor for TestSensor {
        fn label(&self) -> &str {
            "Sensor"
        }
        fn read_celsius(&self) -> Result<f32> {
            self.0
                .ok_or_else(|| ThermalError::InvalidValue("sensor unavailable".into()))
        }
    }

    struct TestDevice {
        temps: Vec<Option<f32>>,
        writes: Arc<Mutex<Vec<u8>>>,
        fail_writes: bool,
    }

    impl ThermalDevice for TestDevice {
        fn vendor_name(&self) -> &str {
            "test"
        }
        fn driver_name(&self) -> &str {
            "test"
        }
        fn model(&self) -> Option<String> {
            None
        }
        fn fans(&self) -> Vec<Box<dyn Fan>> {
            vec![Box::new(TestFan(self.writes.clone(), self.fail_writes))]
        }
        fn sensors(&self) -> Vec<Box<dyn TempSensor>> {
            self.temps
                .iter()
                .map(|t| Box::new(TestSensor(*t)) as Box<dyn TempSensor>)
                .collect()
        }
        fn supports_auto_curve(&self) -> bool {
            true
        }
    }

    fn test_state(temps: Vec<Option<f32>>, config: Config) -> (Arc<AppState>, Arc<Mutex<Vec<u8>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let device = TestDevice {
            temps,
            writes: writes.clone(),
            fail_writes: false,
        };
        (
            Arc::new(AppState::new(
                Some(Box::new(device)),
                config,
                Db::open_in_memory().unwrap(),
            )),
            writes,
        )
    }

    #[test]
    fn adaptive_uses_hottest_sensor_and_keeps_regulating() {
        let (state, writes) = test_state(vec![Some(50.0), Some(78.0)], Config::default());
        tick(&state, 2.0).unwrap();
        tick(&state, 2.0).unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![26, 32]);
    }

    #[test]
    fn missing_invalid_or_hot_sensor_requests_maximum_for_both_strategies() {
        for strategy in [AutoStrategy::Adaptive, AutoStrategy::Curve] {
            for temps in [
                vec![],
                vec![None, Some(40.0)],
                vec![Some(f32::NAN)],
                vec![Some(95.0)],
            ] {
                let mut config = Config::default();
                config.automatic_control.strategy = strategy;
                let (state, writes) = test_state(temps, config);
                tick(&state, 2.0).unwrap();
                assert_eq!(*writes.lock().unwrap(), vec![100]);
            }
        }
    }

    #[test]
    fn curve_strategy_still_uses_saved_points() {
        let mut config = Config::default();
        config.automatic_control.strategy = AutoStrategy::Curve;
        config.curves.insert(
            "fan1".into(),
            vec![
                curve::CurvePoint {
                    temp_c: 40.0,
                    percent: 20,
                },
                curve::CurvePoint {
                    temp_c: 80.0,
                    percent: 60,
                },
            ],
        );
        let (state, writes) = test_state(vec![Some(60.0)], config);
        tick(&state, 2.0).unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![40]);
    }

    #[test]
    fn manual_and_disabled_never_apply_automatic_control() {
        for mode in [FanMode::Manual, FanMode::Disabled] {
            let mut config = Config::default();
            config.mode = mode;
            let (state, writes) = test_state(vec![Some(95.0)], config);
            tick(&state, 2.0).unwrap();
            assert!(writes.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn telemetry_exposes_sensor_failures_and_the_successful_safety_command() {
        let (state, _) = test_state(vec![None, Some(45.0)], Config::default());
        let sample = tick(&state, 2.0).unwrap();
        assert_eq!(sample.temps.len(), 1);
        assert!(sample.errors.iter().any(|e| e.contains("Sensor")));
        let control = sample.control.unwrap();
        assert_eq!(control.applied_mode, Some(FanMode::Auto));
        assert_eq!(control.applied_boosts["fan1"], 100);
        assert!(control.last_applied_ms.is_some());
    }

    #[test]
    fn automatic_write_failure_is_visible_and_not_confirmed_as_active() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let device = TestDevice {
            temps: vec![Some(70.0)],
            writes,
            fail_writes: true,
        };
        let state = Arc::new(AppState::new(
            Some(Box::new(device)),
            Config::default(),
            Db::open_in_memory().unwrap(),
        ));
        let sample = tick(&state, 2.0).unwrap();
        assert!(sample
            .errors
            .iter()
            .any(|e| e.contains("could not write fan1")));
        let control = sample.control.as_ref().unwrap();
        assert_eq!(control.applied_mode, None);
        assert!(control.applied_boosts.is_empty());
        assert!(status_line(&sample).contains("error"));
    }

    #[test]
    fn missing_device_still_emits_an_unhealthy_sample() {
        let state = Arc::new(AppState::new(
            None,
            Config::default(),
            Db::open_in_memory().unwrap(),
        ));
        let sample = tick(&state, 2.0).unwrap();
        assert!(sample.errors.iter().any(|e| e.contains("No compatible")));
        assert_eq!(sample.control.unwrap().applied_mode, None);
    }

    #[test]
    fn automatic_interval_is_bounded() {
        let mut config = Config::default();
        config.update_interval_secs = 60;
        assert_eq!(control_interval(&config), 2);
        config.update_interval_secs = 1;
        assert_eq!(control_interval(&config), 1);
        config.mode = FanMode::Manual;
        config.update_interval_secs = 60;
        assert_eq!(control_interval(&config), 60);
    }
}
