use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::config::FanMode;
use crate::readings::{now_ms, read_hardware, Fans, HardwareReadings, Readings};
use crate::state::{AppState, FanReading, Sample, HISTORY_CAPACITY};
use crate::thermal::ThermalDevice;
use crate::tray;

pub const TELEMETRY_EVENT: &str = "telemetry";

const SENSOR_TIMEOUT: Duration = Duration::from_millis(500);
const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

pub fn spawn(app: AppHandle, state: Arc<AppState>) -> HistoryWriter {
    let mut reader = SensorWorker::spawn();
    let (history, history_errors) = spawn_history_writer(&state);
    let history_worker = HistoryWriter(history.clone());
    thread::spawn(move || {
        let mut last_purge: Option<Instant> = None;
        let mut last_status = String::new();
        let mut previous_tick = Instant::now();
        loop {
            if state.lock().shutting_down {
                break;
            }
            let started = Instant::now();
            let interval = {
                let inner = state.lock();
                Duration::from_secs(control_interval(&inner.config))
            };

            let elapsed_secs = previous_tick.elapsed().as_secs_f32();
            previous_tick = Instant::now();
            let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let input = TickInput::snapshot(&state)?;
                let readings = reader.read(input.device.as_ref());
                finish_tick(&state, &input, readings, elapsed_secs, |sample| {
                    if let Ok(error) = history_errors.try_recv() {
                        sample.errors.push(error);
                    }
                    if history
                        .try_send(HistoryJob::Sample(sample.clone()))
                        .is_err()
                    {
                        sample.errors.push(
                            "Could not persist telemetry: history writer unavailable or busy"
                                .into(),
                        );
                    }
                })
                .map(|sample| (sample, input.device.is_some()))
            }));
            match tick_result {
                Ok(Some((sample, device_available))) => {
                    crate::metrics::publish_linux_temperatures(
                        &app,
                        &sample,
                        interval,
                        device_available,
                    );
                    if let Err(e) = app.emit_to("main", TELEMETRY_EVENT, &sample) {
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
                if !inner.control.maintenance && inner.db.is_some() {
                    let retention_ms = inner.config.retention_days() as u64 * 24 * 3600 * 1000;
                    if history
                        .try_send(HistoryJob::Purge(now_ms().saturating_sub(retention_ms)))
                        .is_err()
                    {
                        last_purge = None;
                    }
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
    history_worker
}

enum HistoryJob {
    Sample(Sample),
    Purge(u64),
    Flush(SyncSender<()>),
}

pub struct HistoryWriter(SyncSender<HistoryJob>);

impl HistoryWriter {
    pub(crate) fn flush(&self) {
        let (reply, done) = mpsc::sync_channel(0);
        if self.0.send(HistoryJob::Flush(reply)).is_ok() {
            let _ = done.recv();
        }
    }
}

fn spawn_history_writer(state: &Arc<AppState>) -> (SyncSender<HistoryJob>, Receiver<String>) {
    let (jobs, work) = mpsc::sync_channel(HISTORY_CAPACITY);
    let (report, errors) = mpsc::sync_channel(1);
    let state = Arc::clone(state);
    thread::spawn(move || {
        while let Ok(job) = work.recv() {
            if let HistoryJob::Flush(reply) = job {
                let _ = reply.send(());
                continue;
            }
            let _history_io = state.history_io.read().unwrap();
            let db = state.lock().db.clone();
            let Some(db) = db else { continue };
            let writer = state.history_db.lock().unwrap();
            let connection = writer.as_ref().unwrap_or(db.as_ref());
            let result = match job {
                HistoryJob::Sample(sample) => connection.insert_sample(&sample),
                HistoryJob::Purge(cutoff) => connection.purge_older_than(cutoff),
                HistoryJob::Flush(_) => unreachable!(),
            };
            if let Err(error) = result {
                let message = format!("Could not persist telemetry: {error}");
                eprintln!("thermal-berry: {message}");
                let _ = report.try_send(message);
            }
        }
    });
    (jobs, errors)
}

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

struct TickInput {
    device: Option<Arc<dyn ThermalDevice>>,
    control_epoch: u64,
}

impl TickInput {
    fn snapshot(state: &AppState) -> Option<Self> {
        let inner = state.lock();
        if inner.control.maintenance || inner.db.is_none() {
            return None;
        }
        Some(Self {
            device: inner.device.clone(),
            control_epoch: inner.config.control_epoch,
        })
    }
}

struct ReadRequest {
    device: Arc<dyn ThermalDevice>,
    reply: SyncSender<HardwareReadings>,
}

struct SensorWorker {
    requests: SyncSender<ReadRequest>,
    pending: Option<Receiver<HardwareReadings>>,
    last_fans: Option<(Arc<dyn ThermalDevice>, Fans)>,
}

impl SensorWorker {
    fn spawn() -> Self {
        let (requests, incoming) = mpsc::sync_channel::<ReadRequest>(1);
        thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                let reading = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    read_hardware(Some(request.device.as_ref()))
                }));
                if let Ok(readings) = reading {
                    let _ = request.reply.send(readings);
                }
            }
        });
        Self {
            requests,
            pending: None,
            last_fans: None,
        }
    }

    fn read(&mut self, device: Option<&Arc<dyn ThermalDevice>>) -> HardwareReadings {
        let Some(device) = device else {
            self.last_fans = None;
            return read_hardware(None);
        };
        // A timed-out read may still be in the kernel. Discard its reply and
        // allow at most one acquisition in flight, even across mode changes.
        if let Some(pending) = self.pending.take() {
            if matches!(pending.try_recv(), Err(TryRecvError::Empty)) {
                self.pending = Some(pending);
                return self.unavailable(device);
            }
        }
        let (reply, response) = mpsc::sync_channel(1);
        if self
            .requests
            .try_send(ReadRequest {
                device: device.clone(),
                reply,
            })
            .is_ok()
        {
            match response.recv_timeout(SENSOR_TIMEOUT) {
                Ok(readings) => {
                    self.last_fans = Some((device.clone(), readings.fans.clone()));
                    return readings;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => self.pending = Some(response),
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            }
        }
        self.unavailable(device)
    }

    fn unavailable(&mut self, device: &Arc<dyn ThermalDevice>) -> HardwareReadings {
        let fans = match &self.last_fans {
            Some((previous, fans)) if Arc::ptr_eq(previous, device) => fans.clone(),
            // Fall back to the current device's fan handles for the safety
            // command, without retrying the timed-out sensor acquisition.
            _ => device.fans().into(),
        };
        let fan_readings = fans
            .iter()
            .map(|fan| FanReading {
                id: fan.id().into(),
                label: fan.label().into(),
                rpm: None,
                boost_percent: None,
                max_rpm: None,
            })
            .collect();
        HardwareReadings {
            readings: Readings {
                timestamp_ms: now_ms(),
                temperatures: Vec::new(),
                errors: vec![
                    "Hardware readings are unavailable or did not complete in time".into(),
                ],
            },
            fans,
            fan_readings,
        }
    }
}

fn finish_tick(
    state: &AppState,
    input: &TickInput,
    acquisition: HardwareReadings,
    elapsed_secs: f32,
    persist: impl FnOnce(&mut Sample),
) -> Option<Sample> {
    let mut inner = state.lock();
    let same_device = match (&input.device, &inner.device) {
        (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
        (None, None) => true,
        _ => false,
    };
    if inner.control.maintenance
        || inner.db.is_none()
        || !same_device
        || inner.config.control_epoch != input.control_epoch
    {
        return None;
    }
    let HardwareReadings {
        readings:
            Readings {
                timestamp_ms,
                temperatures,
                mut errors,
            },
        fans,
        mut fan_readings,
    } = acquisition;
    crate::fan_control::apply_automatic(
        &mut inner,
        &fans,
        &temperatures,
        &mut fan_readings,
        &mut errors,
        elapsed_secs,
    );

    let mut sample = Sample {
        timestamp_ms,
        temps: temperatures.into_iter().flatten().collect(),
        fans: fan_readings,
        control: Some(inner.snapshot().control),
        errors,
    };
    persist(&mut sample);
    inner.push_sample(sample.clone());
    Some(sample)
}

#[cfg(test)]
pub(crate) fn tick(state: &Arc<AppState>, elapsed_secs: f32) -> Option<Sample> {
    let input = TickInput::snapshot(state)?;
    let readings = read_hardware(input.device.as_deref());
    let sample = finish_tick(state, &input, readings, elapsed_secs, |_| {})?;
    let db = state.lock().db.clone()?;
    db.insert_sample(&sample).unwrap();
    Some(sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AutoStrategy, Config};
    use crate::curve;
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

    struct BlockingRead {
        started: mpsc::Sender<()>,
        release: Mutex<Receiver<()>>,
    }

    struct TestSensor(Option<f32>, Option<Arc<BlockingRead>>);

    impl TempSensor for TestSensor {
        fn label(&self) -> &str {
            "Sensor"
        }
        fn read_celsius(&self) -> Result<f32> {
            if let Some(blocking) = &self.1 {
                let _ = blocking.started.send(());
                let _ = blocking.release.lock().unwrap().recv();
            }
            self.0
                .ok_or_else(|| ThermalError::InvalidValue("sensor unavailable".into()))
        }
    }

    struct TestDevice {
        temps: Vec<Option<f32>>,
        writes: Arc<Mutex<Vec<u8>>>,
        fail_writes: bool,
        blocking: Option<Arc<BlockingRead>>,
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
                .map(|t| Box::new(TestSensor(*t, self.blocking.clone())) as Box<dyn TempSensor>)
                .collect()
        }
        fn supports_auto_curve(&self) -> bool {
            true
        }

        fn supports_fan_control(&self) -> bool {
            true
        }
    }

    fn test_state(temps: Vec<Option<f32>>, config: Config) -> (Arc<AppState>, Arc<Mutex<Vec<u8>>>) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let device = TestDevice {
            temps,
            writes: writes.clone(),
            fail_writes: false,
            blocking: None,
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
    fn adaptive_uses_hottest_sensor_and_waits_between_increases() {
        let (state, writes) = test_state(vec![Some(50.0), Some(78.0)], Config::default());
        for _ in 0..6 {
            tick(&state, 2.0).unwrap();
        }
        assert_eq!(*writes.lock().unwrap(), vec![25, 25, 25, 25, 25, 30]);
    }

    #[test]
    fn adaptive_holds_boost_while_cooling_then_removes_it_at_the_lower_boundary() {
        let (state, writes) = test_state(vec![Some(50.0), Some(78.0)], Config::default());
        tick(&state, 2.0).unwrap();
        for (temp, expected) in [(76.0, 25), (74.0, 25), (74.0, 30)] {
            state.lock().device = Some(Arc::new(TestDevice {
                temps: vec![Some(50.0), Some(temp)],
                writes: writes.clone(),
                fail_writes: false,
                blocking: None,
            }));
            for _ in 0..5 {
                tick(&state, 2.0).unwrap();
            }
            assert_eq!(writes.lock().unwrap().last(), Some(&expected));
        }
        state.lock().device = Some(Arc::new(TestDevice {
            temps: vec![Some(50.0), Some(68.0)],
            writes: writes.clone(),
            fail_writes: false,
            blocking: None,
        }));
        let sample = tick(&state, 0.0).unwrap();
        assert_eq!(writes.lock().unwrap().last(), Some(&0));
        assert_eq!(sample.control.unwrap().applied_boosts["fan1"], 0);
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
            blocking: None,
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
    fn stale_acquisition_cannot_write_after_a_mode_change() {
        let (state, writes) = test_state(vec![Some(95.0)], Config::default());
        let input = TickInput::snapshot(&state).unwrap();
        let readings = read_hardware(input.device.as_deref());
        crate::commands::apply_mode(&state, FanMode::Disabled).unwrap();
        crate::commands::apply_mode(&state, FanMode::Auto).unwrap();

        assert!(finish_tick(&state, &input, readings, 2.0, |_| unreachable!()).is_none());
        assert_eq!(*writes.lock().unwrap(), vec![0]);
    }

    #[test]
    fn stale_acquisition_cannot_write_to_a_replaced_device() {
        let (state, writes) = test_state(vec![Some(95.0)], Config::default());
        let input = TickInput::snapshot(&state).unwrap();
        let readings = read_hardware(input.device.as_deref());
        let (replacement, replacement_writes) = test_state(vec![Some(50.0)], Config::default());
        state.lock().device = replacement.lock().device.clone();

        assert!(finish_tick(&state, &input, readings, 2.0, |_| unreachable!()).is_none());
        assert!(writes.lock().unwrap().is_empty());
        assert!(replacement_writes.lock().unwrap().is_empty());
    }

    #[test]
    fn returning_to_firmware_rejects_an_in_flight_sample() {
        let (state, writes) = test_state(vec![Some(95.0)], Config::default());
        let input = TickInput::snapshot(&state).unwrap();
        let readings = read_hardware(input.device.as_deref());
        crate::fan_control::return_to_firmware(&state);

        assert!(finish_tick(&state, &input, readings, 2.0, |_| unreachable!()).is_none());
        assert!(TickInput::snapshot(&state).is_none());
        assert_eq!(*writes.lock().unwrap(), vec![0]);
    }

    #[test]
    fn a_stalled_sensor_leaves_state_readable_and_requests_maximum_boost() {
        let (state, writes) = test_state(vec![Some(45.0)], Config::default());
        let (started, reading) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        state.lock().device = Some(Arc::new(TestDevice {
            temps: vec![Some(45.0)],
            writes: writes.clone(),
            fail_writes: false,
            blocking: Some(Arc::new(BlockingRead {
                started,
                release: Mutex::new(blocked),
            })),
        }));
        let worker_state = state.clone();
        let tick = thread::spawn(move || {
            let mut reader = SensorWorker::spawn();
            let input = TickInput::snapshot(&worker_state).unwrap();
            let readings = reader.read(input.device.as_ref());
            finish_tick(&worker_state, &input, readings, 2.0, |_| {}).unwrap()
        });

        reading.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(state.inner.try_lock().is_ok());
        let sample = tick.join().unwrap();
        let _ = release.send(());
        assert!(sample.temps.is_empty());
        assert!(sample.errors.iter().any(|error| error.contains("in time")));
        assert_eq!(*writes.lock().unwrap(), vec![100]);
    }

    #[test]
    fn waiting_for_history_does_not_delay_control_or_the_live_sample() {
        let (state, writes) = test_state(vec![Some(95.0)], Config::default());
        let (history, errors) = spawn_history_writer(&state);
        let blocked_writer = state.history_db.lock().unwrap();
        let input = TickInput::snapshot(&state).unwrap();
        let readings = read_hardware(input.device.as_deref());
        let sample = finish_tick(&state, &input, readings, 2.0, |sample| {
            history
                .try_send(HistoryJob::Sample(sample.clone()))
                .unwrap();
        })
        .unwrap();
        assert_eq!(*writes.lock().unwrap(), vec![100]);
        let db = state.lock().db.clone().unwrap();
        assert!(db
            .query_range(sample.timestamp_ms, sample.timestamp_ms)
            .unwrap()
            .is_empty());
        drop(blocked_writer);

        HistoryWriter(history).flush();
        assert!(errors.try_recv().is_err());
        let saved = db
            .query_range(sample.timestamp_ms, sample.timestamp_ms)
            .unwrap();
        assert_eq!(saved[0].temps[0].celsius, 95.0);
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
