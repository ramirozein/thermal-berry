//! Open-loop calibration with one or two separately measured fan channels.
//! Release every override before waiting for SQLite.
use crate::{
    acquisition::{LoadProbes, Probe, SENSOR_TIMEOUT},
    config::FanMode,
    curve::{boost_to_percent, percent_to_boost},
    db::Db,
    identify::SESSION_SECS,
    monitor::SensorWorker,
    observation::{valid_fan_id, Observation},
    state::Sample,
    thermal::{Fan, Result, ThermalDevice, ThermalError},
};
use std::{
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_secs(2);
const SPIN_UP_LIMIT: Duration = Duration::from_secs(10);

fn error(text: impl Into<String>) -> ThermalError {
    ThermalError::Control(text.into())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    pub cpu_percent: u8,
    pub gpu_percent: u8,
    pub fan_percent: u8,
    pub second_fan_percent: Option<u8>,
}

pub fn stage(elapsed_secs: u64, dual: bool) -> Stage {
    // Different periods keep either load excited while the other one is low.
    let cpu = [15, 55, 85];
    let gpu = [85, 15, 55];
    let fan = [40, 80, 55, 70];
    Stage {
        cpu_percent: cpu[(elapsed_secs / 240 % 3) as usize],
        gpu_percent: gpu[(elapsed_secs / 180 % 3) as usize],
        fan_percent: fan[(elapsed_secs / 120 % 4) as usize],
        second_fan_percent: dual.then(|| [70, 40, 80, 55][(elapsed_secs / 154 % 4) as usize]),
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

pub fn controllable_fans(device: &dyn ThermalDevice) -> Result<Vec<Arc<dyn Fan>>> {
    if !device.supports_fan_control() {
        return Err(ThermalError::Unsupported);
    }
    let mut fans = device.fans();
    if !(1..=2).contains(&fans.len()) {
        return Err(error(format!(
            "calibration supports one or two fan channels; detected {}",
            fans.len()
        )));
    }
    fans.sort_by(|a, b| a.id().cmp(b.id()));
    if fans.iter().any(|f| !valid_fan_id(f.id())) || fans.windows(2).any(|w| w[0].id() == w[1].id())
    {
        return Err(error("fan channels must have distinct stable identifiers"));
    }
    for fan in &fans {
        if !fan.is_writable() {
            return Err(error(format!(
                "{} requires existing write access",
                fan.id()
            )));
        }
        let range = fan.boost_range();
        if range.0 >= range.1 {
            return Err(error(format!("{} has an invalid boost range", fan.id())));
        }
        let measured =
            [40, 55, 70, 80].map(|p| boost_to_percent(percent_to_boost(p, range), range));
        if measured[0] < 30 || measured.windows(2).any(|w| w[1] < w[0] + 5) {
            return Err(error(format!(
                "{} cannot apply four distinct calibration boosts above the safety floor",
                fan.id()
            )));
        }
    }
    Ok(fans.into_iter().map(Arc::from).collect())
}

struct FanControl {
    fans: Vec<Arc<dyn Fan>>,
    readback: Probe<Result<Vec<u8>>>,
    applied: Vec<u8>,
    pending_release: Vec<bool>,
}

impl FanControl {
    fn new(fans: Vec<Arc<dyn Fan>>) -> Self {
        let readers = fans.clone();
        let readback = Probe::spawn(move || {
            move || {
                readers
                    .iter()
                    .map(|f| {
                        f.read_boost()
                            .map_err(|e| error(format!("{} boost readback: {e}", f.id())))
                    })
                    .collect()
            }
        });
        Self {
            pending_release: vec![false; fans.len()],
            fans,
            readback,
            applied: Vec::new(),
        }
    }

    fn write(&mut self, targets: &[u8]) -> Result<()> {
        if targets.len() != self.fans.len() {
            return Err(error("fan target count changed"));
        }
        // The driver may change hardware even on a failed write. Treat a
        // partially applied stage as a fault of the entire cooling system.
        self.pending_release.fill(true);
        let mut requested = Vec::with_capacity(targets.len());
        for (fan, &percent) in self.fans.iter().zip(targets) {
            let boost = percent_to_boost(percent, fan.boost_range());
            fan.set_boost(boost)
                .map_err(|e| error(format!("{} boost write: {e}", fan.id())))?;
            requested.push(boost);
        }
        // Read all channels after the last write. A shared register masquerading
        // as two controls must not confirm two different requested boosts.
        let deadline = Instant::now() + SENSOR_TIMEOUT;
        let observed = self
            .readback
            .request(deadline)
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| error("calibration boost readback timed out"))??;
        self.applied.clear();
        for ((fan, target), value) in self.fans.iter().zip(requested).zip(observed) {
            if value != target {
                return Err(error(format!(
                    "{} calibration boost readback mismatch",
                    fan.id()
                )));
            }
            self.applied
                .push(boost_to_percent(value, fan.boost_range()));
        }
        Ok(())
    }

    fn spin_up(
        &mut self,
        mut idle: Vec<String>,
        cpu_only: bool,
        mut next: impl FnMut(Instant) -> Result<(Sample, Observation)>,
    ) -> Result<()> {
        if idle.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        self.write(&vec![100; self.fans.len()])?;
        // No workloads or training samples until every tachometer confirms rotation.
        for second in 1..=SPIN_UP_LIMIT.as_secs() {
            // Reserve time for the final sensor read within the startup deadline.
            let (sample, observation) = next(start + Duration::from_secs(second) - SENSOR_TIMEOUT)?;
            // Spin-up verifies cooling. Load measurements are required when recording starts.
            check_sensors(&sample, &observation, cpu_only)?;
            if start.elapsed() > SPIN_UP_LIMIT {
                break;
            }
            idle = feedback(
                &self.fans,
                &sample,
                &self.applied,
                Rotation::Starting(&idle),
            )?;
            if idle.is_empty() {
                return Ok(());
            }
        }
        Err(error(format!("fan spin-up timed out: {}", idle.join(", "))))
    }

    fn check_recording_signals(
        &self,
        sample: &Sample,
        o: &Observation,
        cpu_only: bool,
    ) -> Result<()> {
        check_sensors(sample, o, cpu_only)?;
        feedback(&self.fans, sample, &self.applied, Rotation::Running)?;
        Ok(())
    }

    fn read_tick(
        &self,
        scheduled: Instant,
        previous_ms: Option<u64>,
        cpu_only: bool,
        read: &mut impl FnMut() -> (Sample, Observation),
        running: &mut impl FnMut() -> bool,
    ) -> Result<(Sample, Observation)> {
        let mut acquire = || {
            let (sample, observation) = read();
            if previous_ms
                .is_some_and(|t| !(1000..=4000).contains(&sample.timestamp_ms.saturating_sub(t)))
            {
                return Err(error(
                    "calibration clock discontinuity or gap exceeded two ticks",
                ));
            }
            Ok((sample, observation))
        };
        let (sample, observation) = acquire()?;
        let Some(missing) = missing_utilization(&observation, cpu_only) else {
            return Ok((sample, observation));
        };
        // Only utilization can be retried. Check fresh temperatures, rotation and
        // boost on the incomplete acquisition before holding the current stage.
        self.check_recording_signals(&sample, &observation, cpu_only)?;
        let retry_at = scheduled + TICK - SENSOR_TIMEOUT;
        if Instant::now() + sysinfo::MINIMUM_CPU_UPDATE_INTERVAL > retry_at {
            return Err(error(format!(
                "{missing} unavailable; too late for a fresh reading in this tick"
            )));
        }
        wait_until(retry_at, running)?;
        if Instant::now() >= scheduled + TICK {
            return Err(error(format!(
                "{missing} unavailable; calibration retry window expired"
            )));
        }
        let (fresh, measured) = acquire()?;
        if fresh.timestamp_ms <= sample.timestamp_ms {
            return Err(error(
                "calibration clock discontinuity during utilization retry",
            ));
        }
        if let Some(still_missing) = missing_utilization(&measured, cpu_only) {
            self.check_recording_signals(&fresh, &measured, cpu_only)?;
            return Err(error(format!(
                "{still_missing} still unavailable after calibration retry"
            )));
        }
        Ok((fresh, measured))
    }

    fn apply_stage(
        &mut self,
        sample: &mut Sample,
        observation: &mut Observation,
        stage: Stage,
        cpu_only: bool,
    ) -> Result<()> {
        self.check_recording_signals(sample, observation, cpu_only)?;
        require_utilization(observation, cpu_only)?;
        match stage.second_fan_percent {
            Some(second) => self.write(&[stage.fan_percent, second])?,
            None => self.write(&[stage.fan_percent])?,
        }
        for (fan, &percent) in self.fans.iter().zip(&self.applied) {
            let reading = sample
                .fans
                .iter_mut()
                .find(|r| r.id == fan.id())
                .ok_or_else(|| error("calibration fan changed"))?;
            reading.boost_percent = Some(percent);
        }
        observation.set_fans(&sample.fans);
        observation.mode = Some(FanMode::Manual);
        Ok(())
    }

    fn finish(&mut self, failed: bool) -> Result<()> {
        let mut errors = Vec::new();
        if failed {
            for (fan, &pending) in self.fans.iter().zip(&self.pending_release) {
                if pending {
                    if let Err(e) = fan.set_boost(fan.boost_range().1) {
                        errors.push(format!("{} maximum boost: {e}", fan.id()));
                    }
                }
            }
        }
        for (fan, pending) in self.fans.iter().zip(&mut self.pending_release) {
            if *pending {
                match crate::fan_control::release_fan(fan.as_ref()) {
                    Ok(()) => *pending = false,
                    Err(e) => errors.push(format!("{} firmware handoff: {e}", fan.id())),
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(error(errors.join(", ")))
        }
    }
}

impl Drop for FanControl {
    fn drop(&mut self) {
        if let Err(e) = self.finish(true) {
            eprintln!("Failed to return calibration fan to firmware: {e}");
        }
    }
}

fn check_sensors(sample: &Sample, o: &Observation, cpu_only: bool) -> Result<()> {
    if let Some(fault) = sample.errors.first() {
        return Err(error(format!("calibration sensor error: {fault}")));
    }
    let thermal = |name: &str, value: f64| -> Result<()> {
        if !(0.0..85.0).contains(&value) {
            return Err(error(format!(
                "{name} temperature {value}°C outside the calibration range [0, 85)°C"
            )));
        }
        Ok(())
    };
    if sample.temps.is_empty() {
        return Err(error("calibration has no hardware temperature readings"));
    }
    for sensor in &sample.temps {
        thermal(&sensor.label, f64::from(sensor.celsius))?;
    }
    thermal(
        "CPU",
        o.cpu_temp_c
            .ok_or_else(|| error("CPU temperature (cpu_temp_c) unavailable"))?,
    )?;
    if let Some(t) = o.gpu_temp_c {
        thermal("GPU", t)?;
    } else if !cpu_only {
        return Err(error("GPU temperature (gpu_temp_c) unavailable"));
    }
    for (name, value) in [
        ("cpu_percent", o.cpu_percent),
        ("gpu_percent", o.gpu_percent),
    ] {
        if let Some(v) = value {
            if !(0.0..=100.0).contains(&v) {
                return Err(error(format!("{name} reading {v} outside 0–100%")));
            }
        }
    }
    Ok(())
}

fn missing_utilization(o: &Observation, cpu_only: bool) -> Option<&'static str> {
    match (
        o.cpu_percent.is_none(),
        !cpu_only && o.gpu_percent.is_none(),
    ) {
        (true, true) => Some("CPU and GPU utilization (cpu_percent, gpu_percent)"),
        (true, false) => Some("CPU utilization (cpu_percent)"),
        (false, true) => Some("GPU utilization (gpu_percent)"),
        (false, false) => None,
    }
}

fn require_utilization(o: &Observation, cpu_only: bool) -> Result<()> {
    match missing_utilization(o, cpu_only) {
        None => Ok(()),
        Some(field) => Err(error(format!("{field} unavailable"))),
    }
}

fn safe_environment(sample: &Sample, o: &Observation, cpu_only: bool) -> Result<()> {
    check_sensors(sample, o, cpu_only)?;
    require_utilization(o, cpu_only)
}

enum Rotation<'a> {
    Preflight,
    Starting(&'a [String]),
    Running,
}

// Match by driver identity, even if the hardware enumerates its fans in a different order.
fn feedback(
    fans: &[Arc<dyn Fan>],
    sample: &Sample,
    applied: &[u8],
    phase: Rotation<'_>,
) -> Result<Vec<String>> {
    if sample.fans.len() != fans.len() {
        return Err(error("calibration fan count changed"));
    }
    let mut idle = Vec::new();
    for (index, fan) in fans.iter().enumerate() {
        let id = fan.id();
        let reading = sample
            .fans
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| error(format!("missing calibration fan {id}")))?;
        let percent = reading
            .boost_percent
            .filter(|p| *p <= 100)
            .ok_or_else(|| error(format!("{id} needs a measured boost")))?;
        if applied
            .get(index)
            .is_some_and(|expected| *expected != percent)
        {
            return Err(error(format!("{id} lost the applied boost")));
        }
        match reading.rpm {
            None => return Err(error(format!("{id} needs a measured RPM"))),
            Some(0) => {
                let permitted = match phase {
                    Rotation::Preflight => percent == 0,
                    Rotation::Starting(previously_idle) => previously_idle.iter().any(|s| s == id),
                    Rotation::Running => false,
                };
                if !permitted {
                    return Err(error(format!("{id} stopped at {percent}% boost")));
                }
                idle.push(id.to_owned());
            }
            Some(_) => {}
        }
    }
    Ok(idle)
}

/// Read-only readiness check. Returns the IDs that must spin up before logging or applying load.
pub fn preflight(
    fans: &[Arc<dyn Fan>],
    sample: &Sample,
    observation: &Observation,
    cpu_only: bool,
) -> Result<Vec<String>> {
    safe_environment(sample, observation, cpu_only)?;
    feedback(fans, sample, &[], Rotation::Preflight)
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
    running: impl FnMut() -> bool,
    announce: impl FnMut(Option<Stage>),
) -> Result<RecordedSession> {
    let fans = controllable_fans(device.as_ref())?;
    let mut sampler = Sampler::new();
    let mut read = || sampler.read(Some(&device));
    let _ = read();
    thread::sleep(TICK);
    record_session(fans, db, cpu_only, read, running, announce)
}

fn record_session(
    fans: Vec<Arc<dyn Fan>>,
    db: &Db,
    cpu_only: bool,
    mut read: impl FnMut() -> (Sample, Observation),
    mut running: impl FnMut() -> bool,
    mut announce: impl FnMut(Option<Stage>),
) -> Result<RecordedSession> {
    let (sample, o) = read();
    let cpu_only = cpu_only || o.gpu_temp_c.is_none() || o.gpu_percent.is_none();
    eprintln!(
        "Calibration scope: {}",
        if cpu_only { "cpu-only" } else { "cpu+gpu" }
    );
    let idle = preflight(&fans, &sample, &o, cpu_only)?;
    for (index, fan) in fans.iter().enumerate() {
        eprintln!("F{} = {}", index + 1, fan.id());
    }
    if !idle.is_empty() {
        eprintln!(
            "Waiting for positive RPM before starting workloads: {}",
            idle.join(", ")
        );
    }
    let dual = fans.len() == 2;
    thread::scope(|scope| {
        let (queue, records) = mpsc::sync_channel::<(Sample, Observation)>(2);
        let writer = scope.spawn(move || -> Result<()> {
            for (sample, observation) in records {
                db.insert_tick(&sample, Some(&observation))?;
            }
            Ok(())
        });
        let mut control = FanControl::new(fans);
        let result = (|| {
            if !running() {
                return Err(error("calibration interrupted"));
            }
            control.spin_up(idle, cpu_only, |due| {
                wait_until(due, &mut running)?;
                let reading = read();
                if !running() {
                    return Err(error("calibration interrupted"));
                }
                Ok(reading)
            })?;
            // Both preflight and spin-up refresh CPU usage. The first training
            // tick needs a fresh measurement interval, just like later ticks.
            let start = Instant::now() + TICK;
            let mut earliest = start;
            let mut first = None;
            let mut last = None;
            let mut previous_stage = None;
            for tick in 0..SESSION_SECS / 2 {
                let due = start + Duration::from_secs(tick * 2);
                wait_until(due.max(earliest), &mut running)?;
                let (mut sample, mut observation) =
                    control.read_tick(due, last, cpu_only, &mut read, &mut running)?;
                // A recovered sample has its real acquisition time. Keep at least
                // one second to the following sample, as required by M1.
                earliest = Instant::now() + Duration::from_secs(1);
                if !running() {
                    return Err(error("calibration interrupted"));
                }
                let mut stage = stage(tick * 2, dual);
                if cpu_only {
                    stage.gpu_percent = 0;
                }
                control.apply_stage(&mut sample, &mut observation, stage, cpu_only)?;
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
        drop(control); // Retry any failed handoff before waiting for the history writer.
        announce(None);
        drop(queue);
        let saved = writer
            .join()
            .map_err(|_| error("calibration writer panicked"))?;
        if let Err(handoff) = release {
            return Err(match result {
                Err(cause) => error(format!("{cause}; {handoff}")),
                Ok(_) => handoff,
            });
        }
        saved?;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FanReading, TempReading};
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Mutex,
    };

    #[test]
    #[ignore = "requires live CPU/GPU sensors; all fan writes go to test doubles"]
    fn sustained_read_only_cpu_gpu_sampling() {
        let device: Arc<dyn ThermalDevice> = crate::thermal::detect_device().unwrap().into();
        let mut sampler = Sampler::new();
        let _ = sampler.read(Some(&device));
        thread::sleep(TICK);
        let a = TestFan::new("simulated-a");
        let b = TestFan::new("simulated-b");
        let fans = controllable_fans(&TestDevice(vec![a.clone(), b.clone()])).unwrap();
        let db = Db::open_in_memory().unwrap();
        let stop = Instant::now() + Duration::from_secs(54);
        let mut unavailable = 0;
        let result = record_session(
            fans,
            &db,
            false,
            || {
                let (mut sample, mut observation) = sampler.read(Some(&device));
                if observation.gpu_percent.is_none() {
                    unavailable += 1;
                }
                sample.fans = [&a, &b]
                    .iter()
                    .map(|f| FanReading {
                        id: f.id.into(),
                        label: "Simulated fan".into(),
                        boost_percent: Some(f.read_boost().unwrap()),
                        rpm: Some(f.read_rpm().unwrap()),
                        max_rpm: None,
                    })
                    .collect();
                observation.set_fans(&sample.fans);
                (sample, observation)
            },
            || Instant::now() < stop,
            |_| {},
        );
        let reason = result.err().unwrap().to_string();
        assert!(reason.contains("interrupted"), "{reason}");
        let rows = db.observations(0, i64::MAX as u64).unwrap();
        assert!(rows.len() >= 25);
        assert!(rows.iter().all(|r| r.cpu_percent.is_some()
            && r.cpu_temp_c.is_some()
            && r.gpu_percent.is_some()
            && r.gpu_temp_c.is_some()));
        assert!(rows
            .windows(2)
            .all(|w| (1000..=4000).contains(&(w[1].timestamp_ms - w[0].timestamp_ms))));
        eprintln!("{} complete joint samples; {unavailable} missing GPU utilization readings during acquisition", rows.len());
    }

    #[derive(Clone)]
    struct TestFan {
        id: &'static str,
        writes: Arc<Mutex<Vec<u8>>>,
        register: Arc<AtomicU8>,
        refuse: Option<u8>,
        delay: Duration,
        writable: bool,
        range: (u8, u8),
    }

    impl TestFan {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                writes: Arc::default(),
                register: Arc::default(),
                refuse: None,
                delay: Duration::ZERO,
                writable: true,
                range: (0, 100),
            }
        }
    }

    impl Fan for TestFan {
        fn id(&self) -> &str {
            self.id
        }
        fn label(&self) -> &str {
            "System fan"
        }
        fn read_rpm(&self) -> Result<u32> {
            Ok(1800)
        }
        fn read_boost(&self) -> Result<u8> {
            thread::sleep(self.delay);
            Ok(self.register.load(Ordering::SeqCst))
        }
        fn set_boost(&self, value: u8) -> Result<()> {
            self.writes.lock().unwrap().push(value);
            if self.refuse == Some(value) {
                return Err(error("simulated write failure"));
            }
            self.register.store(value, Ordering::SeqCst);
            Ok(())
        }
        fn boost_range(&self) -> (u8, u8) {
            self.range
        }
        fn is_writable(&self) -> bool {
            self.writable
        }
        fn max_rpm(&self) -> Option<u32> {
            None
        }
    }

    struct TestDevice(Vec<TestFan>);
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
            self.0
                .iter()
                .cloned()
                .map(|f| Box::new(f) as Box<dyn Fan>)
                .collect()
        }
        fn sensors(&self) -> Vec<Box<dyn crate::thermal::TempSensor>> {
            Vec::new()
        }
        fn supports_auto_curve(&self) -> bool {
            false
        }
        fn supports_fan_control(&self) -> bool {
            true
        }
    }

    #[test]
    fn topology_uses_driver_ids_and_checks_all_channels_before_writing() {
        let a = TestFan::new("intake");
        let b = TestFan::new("outlet");
        let fans = controllable_fans(&TestDevice(vec![b.clone(), a.clone()])).unwrap();
        assert_eq!([fans[0].id(), fans[1].id()], ["intake", "outlet"]);
        for invalid in [
            vec![],
            vec![a.clone(), a.clone()],
            vec![TestFan {
                range: (0, 7),
                ..a.clone()
            }],
            vec![b.clone(), a.clone(), TestFan::new("third")],
            vec![
                a.clone(),
                TestFan {
                    writable: false,
                    ..b.clone()
                },
            ],
        ] {
            assert!(controllable_fans(&TestDevice(invalid)).is_err());
        }
        assert!(a.writes.lock().unwrap().is_empty() && b.writes.lock().unwrap().is_empty());
    }

    #[test]
    fn firmware_handoff_on_success_failed_write_and_unwind() {
        for (writable, unwind, expected) in [
            (true, false, vec![40, 0]),
            (false, false, vec![40, 100, 0]),
            (true, true, vec![40, 100, 0]),
        ] {
            let fan = Arc::new(TestFan {
                refuse: (!writable).then_some(40),
                ..TestFan::new("only")
            });
            let result = std::panic::catch_unwind(|| {
                let mut control = FanControl::new(vec![fan.clone()]);
                let write = control.write(&[40]);
                assert_eq!(write.is_ok(), writable);
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
    fn aliased_registers_and_partial_writes_release_both_channels() {
        for alias in [false, true] {
            let a = Arc::new(TestFan::new("left"));
            let b = Arc::new(if alias {
                TestFan {
                    register: a.register.clone(),
                    ..TestFan::new("right")
                }
            } else {
                TestFan {
                    refuse: Some(70),
                    ..TestFan::new("right")
                }
            });
            let mut control = FanControl::new(vec![a.clone(), b.clone()]);
            assert!(control.write(&[40, 70]).is_err());
            control.finish(true).unwrap();
            assert_eq!(*a.writes.lock().unwrap(), [40, 100, 0]);
            assert_eq!(*b.writes.lock().unwrap(), [70, 100, 0]);
        }
        let a = Arc::new(TestFan {
            refuse: Some(0),
            ..TestFan::new("left")
        });
        let b = Arc::new(TestFan::new("right"));
        let mut control = FanControl::new(vec![a.clone(), b.clone()]);
        control.write(&[40, 70]).unwrap();
        assert!(control
            .finish(true)
            .unwrap_err()
            .to_string()
            .contains("left"));
        drop(control);
        assert_eq!(*a.writes.lock().unwrap(), [40, 100, 0, 100, 0]);
        assert_eq!(*b.writes.lock().unwrap(), [70, 100, 0]);
    }

    #[test]
    fn stalled_boost_readback_can_release_firmware_before_the_read_finishes() {
        let fan = Arc::new(TestFan {
            delay: SENSOR_TIMEOUT * 3,
            ..TestFan::new("fan1")
        });
        let mut control = FanControl::new(vec![fan.clone()]);
        assert!(control
            .write(&[40])
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        control.finish(true).unwrap();
        assert_eq!(*fan.writes.lock().unwrap(), vec![40, 100, 0]);
    }

    fn readings(channels: &[(&str, u8, u32)]) -> (Sample, Observation) {
        let sample = Sample {
            timestamp_ms: 0,
            temps: vec![TempReading {
                label: "CPU".into(),
                celsius: 50.0,
            }],
            fans: channels
                .iter()
                .map(|&(id, p, r)| FanReading {
                    id: id.into(),
                    label: "System fan".into(),
                    rpm: Some(r),
                    boost_percent: Some(p),
                    max_rpm: None,
                })
                .collect(),
            control: None,
            errors: vec![],
        };
        let mut o = Observation {
            timestamp_ms: 0,
            cpu_temp_c: Some(50.0),
            gpu_temp_c: Some(48.0),
            cpu_percent: Some(15.0),
            gpu_percent: Some(0.0),
            fan_percent: None,
            rpm: None,
            fans: Vec::new(),
            mode: None,
            mem_percent: None,
            gpu_mem_percent: None,
            gpu_power_w: None,
        };
        o.set_fans(&sample.fans);
        (sample, o)
    }

    #[test]
    fn firmware_idle_needs_confirmed_rotation_before_recording_and_each_tick() {
        let a = TestFan::new("fan1");
        let b = TestFan::new("fan2");
        let fans = controllable_fans(&TestDevice(vec![b.clone(), a.clone()])).unwrap();
        let (mut sample, o) = readings(&[("fan2", 0, 0), ("fan1", 0, 0)]);
        let idle = preflight(&fans, &sample, &o, false).unwrap();
        assert_eq!(idle, ["fan1", "fan2"]);
        assert!(a.writes.lock().unwrap().is_empty() && b.writes.lock().unwrap().is_empty());
        sample.fans[0].boost_percent = Some(40);
        assert!(preflight(&fans, &sample, &o, false).is_err());
        sample.fans[0].rpm = None;
        assert!(preflight(&fans, &sample, &o, true).is_err());
        let mut control = FanControl::new(fans);
        let mut n = 0;
        control
            .spin_up(idle, false, |_| {
                n += 1;
                let (sample, mut o) = readings(&[
                    ("fan2", 100, if n == 1 { 0 } else { 1600 }),
                    ("fan1", 100, 1700),
                ]);
                if n == 1 {
                    o.gpu_percent = None;
                }
                Ok((sample, o))
            })
            .unwrap();
        assert_eq!(n, 2);
        let (mut sample, mut o) = readings(&[("fan2", 100, 1600), ("fan1", 100, 1700)]);
        control
            .apply_stage(&mut sample, &mut o, stage(0, true), false)
            .unwrap();
        assert_eq!(
            (o.fan_percent, o.rpm, o.mode),
            (None, None, Some(FanMode::Manual))
        );
        assert_eq!(
            o.fans
                .iter()
                .map(|f| (f.id.as_str(), f.percent.unwrap(), f.rpm.unwrap()))
                .collect::<Vec<_>>(),
            [("fan1", 40.0, 1700), ("fan2", 70.0, 1600)]
        );
        let db = Db::open_in_memory().unwrap();
        let mut inconsistent = o.clone();
        inconsistent.fans[1] = inconsistent.fans[0].clone();
        assert!(db.insert_tick(&sample, Some(&inconsistent)).is_err());
        db.insert_tick(&sample, Some(&o)).unwrap();
        assert_eq!(db.observations(0, 1).unwrap(), [o.clone()]);
        for (id, p, rpm) in [
            ("fan2", 70, Some(0)),
            ("fan2", 40, Some(1600)),
            ("fan2", 70, None),
            ("other", 70, Some(1600)),
        ] {
            sample.fans[0].id = id.into();
            sample.fans[0].rpm = rpm;
            sample.fans[0].boost_percent = Some(p);
            assert!(control
                .apply_stage(&mut sample, &mut o, stage(2, true), true)
                .is_err());
        }
        control.finish(true).unwrap();
        assert_eq!(*a.writes.lock().unwrap(), [100, 40, 100, 0]);
        assert_eq!(*b.writes.lock().unwrap(), [100, 70, 100, 0]);
    }

    #[test]
    fn recorder_starts_with_a_fresh_load_sample_and_releases_both_fans_on_interrupt() {
        use std::cell::Cell;
        for initially_stopped in [true, false] {
            let a = TestFan::new("fan1");
            let b = TestFan::new("fan2");
            let fans = controllable_fans(&TestDevice(vec![b.clone(), a.clone()])).unwrap();
            let db = Db::open_in_memory().unwrap();
            let mut previous_cpu_read = Instant::now() - TICK;
            let started = Cell::new(false);
            let stopped = Cell::new(false);
            let result = record_session(
                fans,
                &db,
                false,
                || {
                    let feedback = |fan: &TestFan| {
                        let boost = fan.read_boost().unwrap();
                        (
                            fan.id,
                            boost,
                            if initially_stopped && boost == 0 {
                                0
                            } else {
                                1800
                            },
                        )
                    };
                    let (mut sample, mut o) = readings(&[feedback(&b), feedback(&a)]);
                    sample.timestamp_ms = crate::readings::now_ms();
                    o.timestamp_ms = sample.timestamp_ms;
                    // The real load probe returns no CPU percentage on a second
                    // refresh inside sysinfo's minimum measurement interval.
                    if previous_cpu_read.elapsed() < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
                        o.cpu_percent = None;
                    } else {
                        previous_cpu_read = Instant::now();
                    }
                    (sample, o)
                },
                || !started.get(),
                |stage| {
                    if stage.is_some() {
                        started.set(true);
                    } else {
                        stopped.set(true);
                    }
                },
            );
            assert!(result.err().unwrap().to_string().contains("interrupted"));
            assert!(started.get() && stopped.get());
            let rows = db.observations(0, i64::MAX as u64).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].mode, Some(FanMode::Manual));
            assert_eq!(rows[0].cpu_percent, Some(15.0));
            assert_eq!(
                (rows[0].fans[0].percent, rows[0].fans[1].percent),
                (Some(40.0), Some(70.0))
            );
            assert_eq!(a.read_boost().unwrap(), 0);
            assert_eq!(b.read_boost().unwrap(), 0);
        }
    }

    #[test]
    fn recorder_recovers_a_missing_gpu_load_with_a_new_paired_sample() {
        use std::cell::Cell;
        let a = TestFan::new("fan1");
        let b = TestFan::new("fan2");
        let fans = controllable_fans(&TestDevice(vec![a.clone(), b.clone()])).unwrap();
        let db = Db::open_in_memory().unwrap();
        let acquisitions = Cell::new(0);
        let start = Instant::now();
        let mut recovered_ms = 0;
        let result = record_session(
            fans,
            &db,
            false,
            || {
                let n = acquisitions.get() + 1;
                acquisitions.set(n);
                let (mut sample, mut o) = readings(&[
                    ("fan1", a.read_boost().unwrap(), 1800),
                    ("fan2", b.read_boost().unwrap(), 1700),
                ]);
                sample.timestamp_ms = start.elapsed().as_millis() as u64;
                o.timestamp_ms = sample.timestamp_ms;
                match n {
                    3 => o.gpu_percent = None,
                    4 => {
                        // Hold the last confirmed stage until the fresh acquisition succeeds.
                        assert_eq!(*a.writes.lock().unwrap(), [40]);
                        assert_eq!(*b.writes.lock().unwrap(), [70]);
                        recovered_ms = o.timestamp_ms;
                        o.cpu_percent = Some(38.0);
                        o.gpu_percent = Some(82.0);
                        o.gpu_temp_c = Some(54.0);
                    }
                    _ => {}
                }
                (sample, o)
            },
            || acquisitions.get() < 6,
            |_| {},
        );
        assert!(result.err().unwrap().to_string().contains("interrupted"));
        let rows = db.observations(0, i64::MAX as u64).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].gpu_percent, Some(0.0));
        assert_eq!(
            (
                rows[1].timestamp_ms,
                rows[1].cpu_percent,
                rows[1].gpu_percent,
                rows[1].gpu_temp_c
            ),
            (recovered_ms, Some(38.0), Some(82.0), Some(54.0))
        );
        assert!(rows
            .windows(2)
            .all(|w| (1000..=4000).contains(&(w[1].timestamp_ms - w[0].timestamp_ms))));
        assert_eq!(*a.writes.lock().unwrap(), [40, 40, 40, 100, 0]);
        assert_eq!(*b.writes.lock().unwrap(), [70, 70, 70, 100, 0]);
    }

    #[test]
    fn a_missing_load_never_hides_a_thermal_or_fan_fault() {
        let a = Arc::new(TestFan::new("a"));
        let b = Arc::new(TestFan::new("b"));
        let mut control = FanControl::new(vec![a.clone(), b.clone()]);
        control.write(&[40, 70]).unwrap();
        let (sample, mut o) = readings(&[("a", 40, 1700), ("b", 70, 1800)]);
        o.gpu_percent = None;
        for fault in [
            "gpu_temp_c",
            "85",
            "b stopped",
            "b lost the applied boost",
            "sensor timeout",
            "cpu_percent",
        ] {
            let (mut sample, mut o) = (sample.clone(), o.clone());
            match fault {
                "gpu_temp_c" => o.gpu_temp_c = None,
                "85" => sample.temps[0].celsius = 85.0,
                "b stopped" => sample.fans[1].rpm = Some(0),
                "b lost the applied boost" => sample.fans[1].boost_percent = Some(0),
                "sensor timeout" => sample.errors.push(fault.into()),
                _ => o.cpu_percent = Some(f64::NAN),
            }
            let mut reads = 0;
            let outcome = control.read_tick(
                Instant::now(),
                None,
                false,
                &mut || {
                    reads += 1;
                    (sample.clone(), o.clone())
                },
                &mut || true,
            );
            assert!(outcome.unwrap_err().to_string().contains(fault));
            assert_eq!(reads, 1);
        }
        control.finish(true).unwrap();
        assert_eq!(*a.writes.lock().unwrap(), [40, 100, 0]);
        assert_eq!(*b.writes.lock().unwrap(), [70, 100, 0]);
    }

    #[test]
    fn a_retry_rechecks_all_signals_and_reports_a_persistent_loss() {
        let a = Arc::new(TestFan::new("a"));
        let mut control = FanControl::new(vec![a.clone()]);
        control.write(&[40]).unwrap();
        for fault in [
            "gpu_percent",
            "gpu_temp_c",
            "a stopped",
            "clock discontinuity",
            "interrupted",
        ] {
            let mut reads = 0;
            let tick = control.read_tick(
                Instant::now() - Duration::from_secs(1),
                None,
                false,
                &mut || {
                    reads += 1;
                    let (mut sample, mut o) = readings(&[("a", 40, 1700)]);
                    sample.timestamp_ms = reads * 500;
                    o.gpu_percent = None;
                    if reads == 2 {
                        match fault {
                            "gpu_temp_c" => {
                                o.gpu_percent = Some(15.0);
                                o.gpu_temp_c = None;
                            }
                            "a stopped" => sample.fans[0].rpm = Some(0),
                            "clock discontinuity" => sample.timestamp_ms = 0,
                            _ => {}
                        }
                    }
                    o.timestamp_ms = sample.timestamp_ms;
                    (sample, o)
                },
                &mut || fault != "interrupted",
            );
            let result = tick.and_then(|(mut s, mut o)| {
                control.apply_stage(&mut s, &mut o, stage(0, false), false)
            });
            let message = result.unwrap_err().to_string();
            assert!(message.contains(fault), "{message}");
            assert_eq!(reads, if fault == "interrupted" { 1 } else { 2 });
        }
        control.finish(true).unwrap();
        assert_eq!(*a.writes.lock().unwrap(), [40, 100, 0]);
    }

    #[test]
    fn full_schedule_persists_measured_boosts_for_each_driver_range() {
        let first = TestFan {
            range: (0, 255),
            ..TestFan::new("blade-a")
        };
        let second = TestFan {
            range: (20, 32),
            ..TestFan::new("blade-b")
        };
        let fans = controllable_fans(&TestDevice(vec![second.clone(), first.clone()])).unwrap();
        let mut control = FanControl::new(fans);
        let db = Db::open_in_memory().unwrap();
        for elapsed in (0..SESSION_SECS).step_by(2) {
            let stage = stage(elapsed, true);
            let percent = |fan: &TestFan| boost_to_percent(fan.read_boost().unwrap(), fan.range);
            let (mut sample, mut o) = readings(&[
                (second.id, percent(&second), 1700),
                (first.id, percent(&first), 1800),
            ]);
            sample.timestamp_ms = elapsed * 1000;
            o.timestamp_ms = sample.timestamp_ms;
            o.cpu_percent = Some(stage.cpu_percent.into());
            o.gpu_percent = Some(stage.gpu_percent.into());
            control
                .apply_stage(&mut sample, &mut o, stage, false)
                .unwrap();
            db.insert_tick(&sample, Some(&o)).unwrap();
        }
        control.finish(false).unwrap();
        assert_eq!(first.read_boost().unwrap(), 0);
        assert_eq!(second.read_boost().unwrap(), 20);
        let rows = db.observations(0, SESSION_SECS * 1000).unwrap();
        assert_eq!(rows.len(), 1350);
        assert_eq!(rows[0].fans[1].percent, Some(67.0));
        crate::identify::validate_session(&rows, false).unwrap();
    }

    #[test]
    fn spin_up_times_out_and_detects_a_previously_rotating_fan_stopping() {
        for stall in [false, true] {
            let a = Arc::new(TestFan::new("a"));
            let b = Arc::new(TestFan::new("b"));
            let mut control = FanControl::new(vec![a.clone(), b.clone()]);
            let mut attempts = 0;
            let error = control
                .spin_up(vec!["b".into()], false, |_| {
                    attempts += 1;
                    Ok(readings(&[
                        ("a", 100, if stall { 0 } else { 1700 }),
                        ("b", 100, 0),
                    ]))
                })
                .unwrap_err();
            assert!(error
                .to_string()
                .contains(if stall { "a stopped" } else { "timed out: b" }));
            assert_eq!(attempts, if stall { 1 } else { SPIN_UP_LIMIT.as_secs() });
            control.finish(true).unwrap();
            assert_eq!(*a.writes.lock().unwrap(), [100, 100, 0]);
            assert_eq!(*b.writes.lock().unwrap(), [100, 100, 0]);
        }
    }

    #[test]
    fn calibration_requires_valid_loads_and_thermal_headroom() {
        let (mut sample, o) = readings(&[("shared", 40, 1800)]);
        assert!(safe_environment(&sample, &o, false).is_ok());
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
        ] {
            assert!(safe_environment(&sample, &changed, false).is_err());
        }
        let missing = Observation {
            gpu_percent: None,
            ..o.clone()
        };
        assert!(safe_environment(&sample, &missing, false).is_err());
        assert!(safe_environment(&sample, &missing, true).is_ok());
        sample.errors.push("Sensor timeout".into());
        assert!(safe_environment(&sample, &o, true).is_err());
        sample.errors.clear();
        sample.temps[0].celsius = f32::INFINITY;
        assert!(safe_environment(&sample, &o, true).is_err());
    }
}
