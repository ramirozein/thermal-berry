//! Open-loop calibration, scoped to one shared fan. User settings are never
//! overwritten. The firmware owns the fan again before waiting for SQLite.
use crate::{
    acquisition::{LoadProbes, Probe, SENSOR_TIMEOUT},
    config::FanMode,
    curve::{boost_to_percent, percent_to_boost},
    db::Db,
    identify::SESSION_SECS,
    monitor::SensorWorker,
    observation::Observation,
    state::Sample,
    thermal::{Fan, Result, ThermalDevice, ThermalError},
};
use std::{
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_secs(2);

fn error(text: impl Into<String>) -> ThermalError {
    ThermalError::Control(text.into())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    pub cpu_percent: u8,
    pub gpu_percent: u8,
    pub fan_percent: u8,
}

pub fn stage(elapsed_secs: u64) -> Stage {
    // Different periods keep either load excited while the other one is low.
    let cpu = [15, 55, 85];
    let gpu = [85, 15, 55];
    let fan = [40, 80, 55, 70];
    Stage {
        cpu_percent: cpu[(elapsed_secs / 240 % 3) as usize],
        gpu_percent: gpu[(elapsed_secs / 180 % 3) as usize],
        fan_percent: fan[(elapsed_secs / 120 % 4) as usize],
    }
}

struct Sampler {
    sensors: SensorWorker,
    loads: LoadProbes,
}

impl Sampler {
    fn new() -> Self {
        Self {
            sensors: SensorWorker::spawn(),
            loads: LoadProbes::spawn(),
        }
    }

    fn read(&mut self, device: Option<&Arc<dyn ThermalDevice>>) -> (Sample, Observation) {
        let pending = self.loads.request();
        let hardware = self.sensors.read(device);
        let sample = Sample {
            timestamp_ms: hardware.readings.timestamp_ms,
            temps: hardware
                .readings
                .temperatures
                .into_iter()
                .flatten()
                .collect(),
            fans: hardware.fan_readings,
            errors: hardware.readings.errors,
            control: None,
        };
        let observation = pending.join(&sample, None);
        (sample, observation)
    }
}

/// Diagnostic acquisition only. The probe has not applied a control mode.
pub fn probe(device: Option<&Arc<dyn ThermalDevice>>) -> (Sample, Observation) {
    let mut sampler = Sampler::new();
    let _ = sampler.read(device);
    thread::sleep(TICK);
    sampler.read(device)
}

pub fn shared_fan(device: &dyn ThermalDevice) -> Result<Box<dyn Fan>> {
    if !device.supports_fan_control() {
        return Err(ThermalError::Unsupported);
    }
    let mut fans = device.fans();
    if fans.len() != 1 {
        return Err(error(format!(
            "calibration requires one shared fan; detected {}",
            fans.len()
        )));
    }
    let fan = fans.pop().unwrap();
    if !fan.is_writable() {
        return Err(error("the shared fan requires existing write access"));
    }
    Ok(fan)
}

struct Override {
    fan: Arc<dyn Fan>,
    readback: Probe<Result<u8>>,
    active: bool,
}

impl Override {
    fn new(fan: Arc<dyn Fan>) -> Self {
        let reader = Arc::clone(&fan);
        let readback = Probe::spawn(move || move || reader.read_boost());
        Self {
            fan,
            readback,
            active: false,
        }
    }

    fn write(&mut self, percent: u8) -> Result<u8> {
        let range = self.fan.boost_range();
        let requested = percent_to_boost(percent, range);
        self.active = true; // A driver can apply a write even when it returns an error.
        self.fan.set_boost(requested)?;
        let deadline = Instant::now() + SENSOR_TIMEOUT;
        let observed = self
            .readback
            .request(deadline)
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| error("calibration boost readback timed out"))??;
        if requested != observed {
            return Err(error("calibration boost readback mismatch"));
        }
        Ok(boost_to_percent(observed, range))
    }

    fn release(&mut self) -> Result<()> {
        if self.active {
            crate::fan_control::release_fan(self.fan.as_ref())?;
            self.active = false;
        }
        Ok(())
    }

    fn finish(&mut self, failed: bool) -> Result<()> {
        if self.active && failed {
            if let Err(e) = self.fan.set_boost(self.fan.boost_range().1) {
                eprintln!("Calibration maximum boost failed: {e}");
            }
        }
        self.release()
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        if let Err(e) = self.finish(thread::panicking()) {
            eprintln!("Failed to return calibration fan to firmware: {e}");
        }
    }
}

pub fn safe_reading(sample: &Sample, o: &Observation, cpu_only: bool) -> Result<()> {
    let safe_temp = |v: f64| (0.0..85.0).contains(&v);
    let pct = |v: f64| (0.0..=100.0).contains(&v);
    if let Some(fault) = sample.errors.first() {
        return Err(error(format!("calibration sensor error: {fault}")));
    }
    if sample.temps.is_empty()
        || sample
            .temps
            .iter()
            .any(|t| !safe_temp(f64::from(t.celsius)))
        || !o.cpu_temp_c.is_some_and(safe_temp)
        || o.gpu_temp_c.is_some_and(|v| !safe_temp(v))
    {
        return Err(error("calibration needs valid temperatures below 85°C"));
    }
    if !o.fan_percent.is_some_and(pct) || !o.rpm.is_some_and(|rpm| rpm > 0) {
        return Err(error(
            "calibration needs measured boost and positive RPM for the shared fan",
        ));
    }
    if !o.cpu_percent.is_some_and(pct)
        || o.gpu_percent.is_some_and(|v| !pct(v))
        || (!cpu_only && (o.gpu_percent.is_none() || o.gpu_temp_c.is_none()))
    {
        return Err(error(
            "calibration needs valid load and temperature for each selected die",
        ));
    }
    Ok(())
}

pub struct RecordedSession {
    pub start_ms: u64,
    pub end_ms: u64,
    pub cpu_only: bool,
}

fn wait_until(due: Instant, running: &mut impl FnMut() -> bool) -> Result<()> {
    loop {
        if !running() {
            return Err(error("calibration interrupted"));
        }
        let remaining = due.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

/// The caller holds the instance lock and a sleep inhibitor. Polling the
/// callback also detects interruption or loss of the inhibitor between ticks.
pub fn record(
    device: Arc<dyn ThermalDevice>,
    db: &Db,
    cpu_only: bool,
    mut running: impl FnMut() -> bool,
    mut announce: impl FnMut(Option<Stage>),
) -> Result<RecordedSession> {
    let fan = shared_fan(device.as_ref())?;
    let mut sampler = Sampler::new();
    let _ = sampler.read(Some(&device));
    thread::sleep(TICK);
    let (preflight, o) = sampler.read(Some(&device));
    let cpu_only = cpu_only || o.gpu_temp_c.is_none() || o.gpu_percent.is_none();
    eprintln!(
        "Calibration scope: {}",
        if cpu_only { "cpu-only" } else { "cpu+gpu" }
    );
    safe_reading(&preflight, &o, cpu_only)?;
    thread::scope(|scope| {
        let (queue, records) = mpsc::sync_channel::<(Sample, Observation)>(2);
        let writer = scope.spawn(move || -> Result<()> {
            for (sample, observation) in records {
                db.insert_tick(&sample, Some(&observation))?;
            }
            Ok(())
        });
        let mut control = Override::new(Arc::from(fan));
        let result = (|| {
            let start = Instant::now();
            let mut first = None;
            let mut last = None;
            let mut previous_stage = None;
            for tick in 0..SESSION_SECS / 2 {
                let due = start + Duration::from_secs(tick * 2);
                wait_until(due, &mut running)?;
                let (mut sample, mut observation) = sampler.read(Some(&device));
                if last.is_some_and(|t| {
                    !(1000..=4000).contains(&sample.timestamp_ms.saturating_sub(t))
                }) {
                    return Err(error(
                        "calibration clock discontinuity or gap exceeded two ticks",
                    ));
                }
                safe_reading(&sample, &observation, cpu_only)?;
                if sample.fans.len() != 1 || sample.fans[0].id != control.fan.id() {
                    return Err(error("calibration fan changed"));
                }
                if !running() {
                    return Err(error("calibration interrupted"));
                }
                let mut stage = stage(tick * 2);
                if cpu_only {
                    stage.gpu_percent = 0;
                }
                let percent = control.write(stage.fan_percent)?;
                sample.fans[0].boost_percent = Some(percent);
                observation.fan_percent = Some(f64::from(percent));
                observation.mode = Some(FanMode::Manual);
                if previous_stage != Some(stage) {
                    announce(Some(stage));
                    previous_stage = Some(stage);
                }
                first.get_or_insert(sample.timestamp_ms);
                last = Some(sample.timestamp_ms);
                queue
                    .try_send((sample, observation))
                    .map_err(|_| error("calibration history writer failed or fell behind"))?;
            }
            wait_until(start + Duration::from_secs(SESSION_SECS), &mut running)?;
            Ok(RecordedSession {
                start_ms: first.unwrap(),
                end_ms: last.unwrap(),
                cpu_only,
            })
        })();
        let release = control.finish(result.is_err());
        announce(None);
        drop(queue);
        let saved = writer
            .join()
            .map_err(|_| error("calibration writer panicked"))?;
        release?;
        saved?;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TempReading;
    use std::sync::Mutex;

    struct TestFan {
        writes: Mutex<Vec<u8>>,
        echo: bool,
        delay: Duration,
    }

    impl Fan for TestFan {
        fn id(&self) -> &str {
            "fan1"
        }
        fn label(&self) -> &str {
            "Shared"
        }
        fn read_rpm(&self) -> Result<u32> {
            Ok(1800)
        }
        fn read_boost(&self) -> Result<u8> {
            thread::sleep(self.delay);
            Ok(if self.echo {
                *self.writes.lock().unwrap().last().unwrap_or(&0)
            } else {
                0
            })
        }
        fn set_boost(&self, value: u8) -> Result<()> {
            self.writes.lock().unwrap().push(value);
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

    #[test]
    fn firmware_handoff_on_success_failed_readback_and_unwind() {
        for (echo, unwind, expected) in [
            (true, false, vec![40, 0]),
            (false, false, vec![40, 100, 0]),
            (true, true, vec![40, 100, 0]),
        ] {
            let fan = Arc::new(TestFan {
                writes: Mutex::default(),
                echo,
                delay: Duration::ZERO,
            });
            let result = std::panic::catch_unwind(|| {
                let mut control = Override::new(fan.clone());
                let write = control.write(40);
                assert_eq!(write.is_ok(), echo);
                if unwind {
                    panic!("simulated failure in acquisition");
                }
                control.finish(write.is_err()).unwrap();
            });
            assert_eq!(result.is_err(), unwind);
            assert_eq!(*fan.writes.lock().unwrap(), expected);
        }
    }

    #[test]
    fn stalled_boost_readback_can_release_firmware_before_the_read_finishes() {
        let fan = Arc::new(TestFan {
            writes: Mutex::default(),
            echo: true,
            delay: SENSOR_TIMEOUT * 3,
        });
        let mut control = Override::new(fan.clone());
        assert!(control
            .write(40)
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        control.finish(true).unwrap();
        assert_eq!(*fan.writes.lock().unwrap(), vec![40, 100, 0]);
    }

    #[test]
    fn calibration_requires_feedback_valid_loads_and_thermal_headroom() {
        let mut sample = Sample {
            timestamp_ms: 0,
            temps: vec![TempReading {
                label: "CPU".into(),
                celsius: 50.0,
            }],
            fans: vec![],
            control: None,
            errors: vec![],
        };
        let o = Observation {
            timestamp_ms: 0,
            cpu_temp_c: Some(50.0),
            gpu_temp_c: Some(48.0),
            cpu_percent: Some(15.0),
            gpu_percent: Some(0.0),
            fan_percent: Some(40.0),
            rpm: Some(1800),
            mode: None,
            mem_percent: None,
            gpu_mem_percent: None,
            gpu_power_w: None,
        };
        assert!(safe_reading(&sample, &o, false).is_ok());
        for changed in [
            Observation {
                cpu_temp_c: Some(f64::NAN),
                ..o.clone()
            },
            Observation {
                gpu_temp_c: Some(85.0),
                ..o.clone()
            },
            Observation {
                cpu_temp_c: None,
                ..o.clone()
            },
            Observation {
                cpu_percent: Some(f64::INFINITY),
                ..o.clone()
            },
            Observation {
                gpu_percent: Some(101.0),
                ..o.clone()
            },
            Observation {
                fan_percent: None,
                ..o.clone()
            },
            Observation {
                rpm: Some(0),
                ..o.clone()
            },
        ] {
            assert!(safe_reading(&sample, &changed, false).is_err());
        }
        let missing = Observation {
            gpu_percent: None,
            ..o.clone()
        };
        assert!(safe_reading(&sample, &missing, false).is_err());
        assert!(safe_reading(&sample, &missing, true).is_ok());
        sample.errors.push("Sensor timeout".into());
        assert!(safe_reading(&sample, &o, true).is_err());
        sample.errors.clear();
        sample.temps[0].celsius = f32::INFINITY;
        assert!(safe_reading(&sample, &o, true).is_err());
    }
}
