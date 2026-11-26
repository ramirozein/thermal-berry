use std::sync::Arc;

use serde::Deserialize;

use crate::config::{
    AllFansBoost, AutomaticControl, Config, FanMode, PredictiveTargets, TempUnit, Theme,
};
use crate::curve::{self, CurvePoint};
use crate::readings::now_ms;
use crate::state::{AppState, Inner};
use crate::thermal::{self, ThermalError};

type CmdResult<T> = Result<T, ThermalError>;

fn control_device(inner: &Inner) -> CmdResult<&dyn thermal::ThermalDevice> {
    let device = inner
        .device
        .as_deref()
        .ok_or(ThermalError::DeviceNotFound)?;
    if !device.supports_fan_control() {
        return Err(ThermalError::Unsupported);
    }
    Ok(device)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigPatch {
    pub update_interval_secs: Option<u64>,
    pub temp_unit: Option<TempUnit>,
    pub theme: Option<Theme>,
    pub history_retention_days: Option<u32>,
    pub automatic_control: Option<AutomaticControl>,
    pub predictive: Option<PredictiveTargets>,
    pub curves: Option<std::collections::HashMap<String, Vec<CurvePoint>>>,
    pub all_fans_boost_percent: Option<u8>,
}

fn ensure_running(inner: &Inner) -> CmdResult<()> {
    if inner.control.maintenance || inner.db.is_none() {
        return Err(ThermalError::Control("maintenance is in progress".into()));
    }
    Ok(())
}

fn save_config(inner: &Inner, config: &Config) -> CmdResult<()> {
    inner
        .db
        .as_ref()
        .ok_or_else(|| ThermalError::Database("database is closed".into()))?
        .save_config(config)
}

fn validate_config(config: &Config) -> CmdResult<()> {
    if !config.automatic_control.is_valid() || !config.predictive.is_valid() {
        return Err(ThermalError::InvalidValue(
            "target temperature must be between 50 and 85°C".into(),
        ));
    }
    if config.all_fans_boost.percent > 100
        || config.manual_boosts.values().any(|p| *p > 100)
        || (config.all_fans_boost.enabled && config.mode != FanMode::Manual)
    {
        return Err(ThermalError::InvalidValue(
            "boost must be 0–100% and global boost requires Manual mode".into(),
        ));
    }
    if config.curves.values().any(|points| {
        points.len() < 2
            || points
                .iter()
                .any(|p| !p.temp_c.is_finite() || p.percent > 100)
    }) {
        return Err(ThermalError::InvalidValue(
            "curves need at least two finite temperature points with boost between 0 and 100%"
                .into(),
        ));
    }
    Ok(())
}

pub fn apply_config_patch(state: &AppState, patch: ConfigPatch) -> CmdResult<()> {
    let mut inner = state.lock();
    ensure_running(&inner)?;
    if patch.automatic_control.is_some()
        || patch.curves.is_some()
        || patch.all_fans_boost_percent.is_some()
    {
        let device = control_device(&inner)?;
        if let Some(curves) = &patch.curves {
            let fans = device.fans();
            if !device.supports_auto_curve()
                || curves
                    .keys()
                    .any(|id| !fans.iter().any(|fan| fan.id() == id))
            {
                return Err(ThermalError::InvalidValue(
                    "curve requires a supported fan".into(),
                ));
            }
        }
    }
    let mut next = inner.config.clone();
    if let Some(v) = patch.update_interval_secs {
        next.update_interval_secs = v.clamp(1, 60);
    }
    if let Some(v) = patch.temp_unit {
        next.temp_unit = v;
    }
    if let Some(v) = patch.theme {
        next.theme = v;
    }
    if let Some(v) = patch.history_retention_days {
        next.history_retention_days = v.clamp(1, 365);
    }
    if let Some(targets) = patch.predictive {
        next.predictive = targets;
    }
    if let Some(v) = patch.automatic_control {
        next.automatic_control = v;
    }
    if let Some(v) = patch.curves {
        next.curves.extend(v);
    }
    if let Some(v) = patch.all_fans_boost_percent {
        if next.all_fans_boost.enabled {
            return Err(ThermalError::InvalidValue(
                "use set_all_fans_boost to change an active boost".into(),
            ));
        }
        next.all_fans_boost.percent = v;
    }
    validate_config(&next)?;
    save_config(&inner, &next)?;
    if next.automatic_control != inner.config.automatic_control
        || next.curves != inner.config.curves
    {
        next.control_epoch += 1;
        inner.automatic_states.clear();
        inner.curve_states.clear();
        if next.mode == FanMode::Auto {
            inner.control.applied_mode = None;
        }
    }
    next.revision += 1;
    inner.config = next;
    Ok(())
}

/// Hardware cannot participate in SQLite transactions. Snapshot every affected
/// fan first, then roll back writes if either the device or persistence fails.
fn transition(inner: &mut Inner, mut next: Config, writes: Vec<(String, u8)>) -> CmdResult<()> {
    ensure_running(inner)?;
    if next.mode == FanMode::Manual {
        next.manual_boosts.extend(writes.iter().cloned());
    }
    validate_config(&next)?;
    let fans = control_device(inner)?.fans();
    if fans.is_empty() {
        return Err(ThermalError::DeviceNotFound);
    }
    let mut previous = Vec::new();
    for (id, _) in &writes {
        let fan = fans
            .iter()
            .find(|f| f.id() == id)
            .ok_or_else(|| ThermalError::InvalidValue(format!("unknown fan: {id}")))?;
        if !fan.is_writable() {
            return Err(ThermalError::PermissionDenied { path: id.clone() });
        }
        previous.push(fan.read_boost()?);
    }
    let mut attempted = 0;
    let result = (|| {
        for (id, percent) in &writes {
            let fan = fans.iter().find(|f| f.id() == id).unwrap();
            attempted += 1;
            fan.set_boost(curve::percent_to_boost(*percent, fan.boost_range()))
                .map_err(|e| ThermalError::Control(format!("could not write {id}: {e}")))?;
        }
        save_config(inner, &next)
    })();
    if let Err(error) = result {
        let mut recovery_errors = Vec::new();
        for ((id, _), boost) in writes.iter().zip(previous.iter()).take(attempted).rev() {
            let fan = fans.iter().find(|f| f.id() == id).unwrap();
            if let Err(e) = fan.set_boost(*boost) {
                recovery_errors.push(format!("{id}: {e}"));
            }
        }
        inner.automatic_states.clear();
        inner.curve_states.clear();
        let recovery = if recovery_errors.is_empty() {
            "Previous fan boosts restored; configuration unchanged.".to_string()
        } else {
            inner.control.suspended = true;
            inner.control.applied_mode = None;
            inner.control.applied_boosts.clear();
            format!("Rollback failed ({}). Control suspended; observed boosts are shown in telemetry. Select a mode to retry.", recovery_errors.join("; "))
        };
        let message = format!("{error}. {recovery}");
        inner.control.error = Some(message.clone());
        inner.config.revision += 1;
        inner.config.control_epoch += 1;
        return Err(if matches!(error, ThermalError::Database(_)) {
            ThermalError::Database(message)
        } else {
            ThermalError::Control(message)
        });
    }
    next.revision = inner.config.revision + 1;
    inner.config = next;
    inner.automatic_states.clear();
    inner.curve_states.clear();
    inner.control.error = None;
    inner.control.suspended = false;
    // Auto is confirmed by the monitor only after its first successful cycle.
    inner.control.applied_mode = if inner.config.mode == FanMode::Auto {
        None
    } else {
        Some(inner.config.mode)
    };
    if !writes.is_empty() {
        inner.control.last_applied_ms = Some(now_ms());
        inner.control.applied_boosts.extend(writes);
    }
    Ok(())
}

pub fn apply_fan_boost(
    state: &AppState,
    fan_id: String,
    percent: u8,
    expected_epoch: u64,
) -> CmdResult<()> {
    if percent > 100 {
        return Err(ThermalError::InvalidValue(format!("{percent}% > 100%")));
    }
    let mut inner = state.lock();
    ensure_running(&inner)?;
    if expected_epoch != inner.config.control_epoch {
        return Err(ThermalError::Control(
            "manual adjustment expired after a newer control change".into(),
        ));
    }
    let mut next = inner.config.clone();
    let fans = control_device(&inner)?.fans();
    if !fans.iter().any(|f| f.id() == fan_id) {
        return Err(ThermalError::InvalidValue(format!("unknown fan: {fan_id}")));
    }
    // Preserve the observed state of the other fans when leaving Auto/global boost.
    for fan in &fans {
        let pct = if fan.id() == fan_id {
            percent
        } else {
            curve::boost_to_percent(fan.read_boost()?, fan.boost_range())
        };
        next.manual_boosts.insert(fan.id().to_string(), pct);
    }
    next.mode = FanMode::Manual;
    next.all_fans_boost.enabled = false;
    transition(&mut inner, next, vec![(fan_id, percent)])
}

pub fn apply_all_fans_boost(state: &AppState, enabled: bool, percent: u8) -> CmdResult<()> {
    let mut inner = state.lock();
    let mut next = inner.config.clone();
    next.all_fans_boost = AllFansBoost { enabled, percent };
    next.control_epoch += 1;
    let writes = if enabled {
        next.mode = FanMode::Manual;
        let fans = control_device(&inner)?.fans();
        for fan in &fans {
            next.manual_boosts.insert(fan.id().to_string(), percent);
        }
        fans.iter().map(|f| (f.id().to_string(), percent)).collect()
    } else {
        next.mode = FanMode::Auto;
        Vec::new()
    };
    transition(&mut inner, next, writes)
}

fn mode_writes(inner: &Inner, config: &Config) -> CmdResult<Vec<(String, u8)>> {
    let fans = control_device(inner)?.fans();
    Ok(match config.mode {
        FanMode::Auto => Vec::new(),
        FanMode::Manual => fans
            .iter()
            .map(|f| {
                (
                    f.id().to_string(),
                    if config.all_fans_boost.enabled {
                        config.all_fans_boost.percent
                    } else {
                        config.manual_boosts.get(f.id()).copied().unwrap_or(0)
                    },
                )
            })
            .collect(),
        FanMode::Disabled => fans.iter().map(|f| (f.id().to_string(), 0)).collect(),
    })
}

pub fn apply_mode(state: &AppState, mode: FanMode) -> CmdResult<()> {
    let mut inner = state.lock();
    let mut next = inner.config.clone();
    next.mode = mode;
    next.all_fans_boost.enabled = false;
    next.control_epoch += 1;
    let writes = mode_writes(&inner, &next)?;
    transition(&mut inner, next, writes)
}

pub fn restore_control(state: &AppState) {
    restore_saved_mode(&mut state.lock());
}

fn restore_saved_mode(inner: &mut Inner) {
    if inner
        .device
        .as_ref()
        .is_some_and(|device| !device.supports_fan_control())
    {
        inner.control.applied_mode = None;
        inner.control.applied_boosts.clear();
        inner.control.error = None;
        inner.control.suspended = false;
        return;
    }
    let config = inner.config.clone();
    let result = mode_writes(inner, &config).and_then(|writes| transition(inner, config, writes));
    if let Err(e) = result {
        inner.control.applied_mode = None;
        inner.control.suspended = true;
        inner.control.error = Some(format!(
            "Could not restore saved fan control: {e}. Select a mode to retry."
        ));
        inner.config.revision += 1;
    }
}

/// Quiesce both control and persistence under the monitor's mutex before
/// authorization. Cancellation leaves an explicit Disabled mode to retry from.
pub fn begin_maintenance(state: &AppState) -> CmdResult<()> {
    let mut inner = state.lock();
    ensure_running(&inner)?;
    if inner
        .device
        .as_ref()
        .is_some_and(|device| device.supports_fan_control())
    {
        let mut next = inner.config.clone();
        next.mode = FanMode::Disabled;
        next.all_fans_boost.enabled = false;
        next.control_epoch += 1;
        let writes = mode_writes(&inner, &next)?;
        transition(&mut inner, next, writes)?;
    }
    inner.control.maintenance = true;
    Ok(())
}

pub fn select_vendor(
    state: &AppState,
    vendor: String,
) -> CmdResult<Arc<dyn thermal::ThermalDevice>> {
    let device: Arc<dyn thermal::ThermalDevice> = Arc::from(thermal::create_device(&vendor)?);
    {
        let mut inner = state.lock();
        ensure_running(&inner)?;
        let mut next = inner.config.clone();
        next.vendor_override = Some(vendor);
        next.revision += 1;
        next.control_epoch += 1;
        if inner.device.as_ref().is_some_and(|previous| {
            previous.supports_fan_control() && previous.vendor_name() != device.vendor_name()
        }) {
            return Err(ThermalError::Unsupported);
        }
        save_config(&inner, &next)?;
        inner.config = next;
        inner.device = Some(device.clone());
        restore_saved_mode(&mut inner);
    }
    Ok(device)
}

pub fn check_write_access(state: &AppState) -> bool {
    let inner = state.lock();
    match inner.device.as_ref() {
        Some(device) if device.supports_fan_control() => {
            let fans = device.fans();
            !fans.is_empty() && fans.iter().all(|f| f.is_writable())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::thermal::{Fan, Result, TempSensor, ThermalDevice};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Hardware {
        boosts: [u8; 2],
        writes: Vec<(usize, u8)>,
        fail_writes: Vec<(usize, u8)>,
        fail_reads: bool,
    }

    struct TestFan(usize, Arc<Mutex<Hardware>>);
    impl Fan for TestFan {
        fn id(&self) -> &str {
            if self.0 == 0 {
                "fan1"
            } else {
                "fan2"
            }
        }
        fn label(&self) -> &str {
            self.id()
        }
        fn read_rpm(&self) -> Result<u32> {
            Ok(2000)
        }
        fn read_boost(&self) -> Result<u8> {
            let hw = self.1.lock().unwrap();
            if hw.fail_reads {
                return Err(ThermalError::Control("read failed".into()));
            }
            Ok(hw.boosts[self.0])
        }
        fn set_boost(&self, value: u8) -> Result<()> {
            let mut hw = self.1.lock().unwrap();
            hw.writes.push((self.0, value));
            // A driver may change hardware before returning an error.
            hw.boosts[self.0] = value;
            if hw.fail_writes.contains(&(self.0, value)) {
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

    struct TestSensor;
    impl TempSensor for TestSensor {
        fn label(&self) -> &str {
            "CPU"
        }
        fn read_celsius(&self) -> Result<f32> {
            Ok(60.0)
        }
    }

    struct TestDevice(Arc<Mutex<Hardware>>);
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
            (0..2)
                .map(|i| Box::new(TestFan(i, self.0.clone())) as Box<dyn Fan>)
                .collect()
        }
        fn sensors(&self) -> Vec<Box<dyn TempSensor>> {
            vec![Box::new(TestSensor)]
        }
        fn supports_auto_curve(&self) -> bool {
            true
        }

        fn supports_fan_control(&self) -> bool {
            true
        }
    }

    fn fixture(config: Config) -> (Arc<AppState>, Arc<Mutex<Hardware>>) {
        let hw = Arc::new(Mutex::new(Hardware {
            boosts: [20, 30],
            ..Default::default()
        }));
        let db = Db::open_in_memory().unwrap();
        db.save_config(&config).unwrap();
        (
            Arc::new(AppState::new(
                Some(Box::new(TestDevice(hw.clone()))),
                config,
                db,
            )),
            hw,
        )
    }

    #[test]
    fn individual_control_confirms_and_persists_mode_and_other_fans() {
        let (state, hw) = fixture(Config::default());
        apply_fan_boost(&state, "fan1".into(), 65, 0).unwrap();
        let inner = state.lock();
        assert_eq!(inner.config.mode, FanMode::Manual);
        assert!(!inner.config.all_fans_boost.enabled);
        assert_eq!(inner.config.manual_boosts["fan2"], 30);
        assert_eq!(inner.control.applied_mode, Some(FanMode::Manual));
        assert_eq!(inner.control.applied_boosts["fan1"], 65);
        let saved = inner.db.as_ref().unwrap().load_config();
        assert_eq!(saved.mode, FanMode::Manual);
        assert_eq!(saved.manual_boosts, inner.config.manual_boosts);
        assert_eq!(hw.lock().unwrap().boosts, [65, 30]);
    }

    #[test]
    fn obsolete_manual_requests_cannot_override_newer_control_changes() {
        for change in 0..4 {
            let (state, hw) = fixture(Config::default());
            match change {
                0 => apply_mode(&state, FanMode::Auto).unwrap(),
                1 => apply_all_fans_boost(&state, true, 80).unwrap(),
                2 => apply_mode(&state, FanMode::Disabled).unwrap(),
                _ => apply_config_patch(
                    &state,
                    ConfigPatch {
                        automatic_control: Some(AutomaticControl {
                            target_temp_c: 75.0,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap(),
            }
            let before = state.lock().snapshot();
            let writes = hw.lock().unwrap().writes.clone();
            assert!(apply_fan_boost(&state, "fan1".into(), 90, 0).is_err());
            assert_eq!(hw.lock().unwrap().writes, writes);
            assert_eq!(state.lock().config.mode, before.config.mode);
        }
    }

    #[test]
    fn independent_fan_adjustments_share_the_same_control_epoch() {
        let (state, hw) = fixture(Config::default());
        apply_fan_boost(&state, "fan1".into(), 55, 0).unwrap();
        apply_fan_boost(&state, "fan2".into(), 75, 0).unwrap();
        assert_eq!(hw.lock().unwrap().boosts, [55, 75]);
    }

    #[test]
    fn preference_patches_preserve_control_and_concurrent_preferences() {
        let (state, hw) = fixture(Config::default());
        apply_all_fans_boost(&state, true, 85).unwrap();
        let theme_state = state.clone();
        let theme = std::thread::spawn(move || {
            apply_config_patch(
                &theme_state,
                ConfigPatch {
                    theme: Some(Theme::Dark),
                    ..Default::default()
                },
            )
            .unwrap()
        });
        let unit_state = state.clone();
        let unit = std::thread::spawn(move || {
            apply_config_patch(
                &unit_state,
                ConfigPatch {
                    temp_unit: Some(TempUnit::Fahrenheit),
                    ..Default::default()
                },
            )
            .unwrap()
        });
        theme.join().unwrap();
        unit.join().unwrap();
        let inner = state.lock();
        assert_eq!(inner.config.mode, FanMode::Manual);
        assert!(inner.config.all_fans_boost.enabled);
        assert_eq!(inner.config.all_fans_boost.percent, 85);
        assert_eq!(inner.config.theme, Theme::Dark);
        assert_eq!(inner.config.temp_unit, TempUnit::Fahrenheit);
        assert_eq!(hw.lock().unwrap().boosts, [85, 85]);
        assert_eq!(inner.db.as_ref().unwrap().load_config().theme, Theme::Dark);
    }

    #[test]
    fn predictive_targets_are_validated_atomically_without_actuation() {
        let (state, hw) = fixture(Config::default());
        let patch = |cpu_target_c, gpu_target_c| ConfigPatch {
            predictive: Some(PredictiveTargets {
                cpu_target_c,
                gpu_target_c,
            }),
            ..Default::default()
        };
        let before = hw.lock().unwrap().boosts;
        for (c, g) in [(49.0, 75.0), (70.0, 86.0), (70.0, f32::NAN)] {
            assert!(apply_config_patch(&state, patch(c, g)).is_err());
        }
        assert_eq!(state.lock().config.predictive, PredictiveTargets::default());
        apply_config_patch(&state, patch(50.0, 85.0)).unwrap();
        let inner = state.lock();
        assert_eq!(inner.config.predictive.cpu_target_c, 50.0);
        assert_eq!(
            inner.db.as_ref().unwrap().load_config().predictive,
            inner.config.predictive
        );
        assert_eq!(inner.config.control_epoch, 0);
        assert_eq!(hw.lock().unwrap().boosts, before);
    }

    #[test]
    fn preferences_cannot_change_control_fields_or_an_active_global_boost() {
        for field in [
            "mode",
            "manualBoosts",
            "allFansBoost",
            "controlEpoch",
            "revision",
        ] {
            assert!(
                serde_json::from_value::<ConfigPatch>(serde_json::json!({(field): null})).is_err()
            );
        }
        let (state, _) = fixture(Config::default());
        apply_all_fans_boost(&state, true, 80).unwrap();
        assert!(apply_config_patch(
            &state,
            ConfigPatch {
                all_fans_boost_percent: Some(10),
                ..Default::default()
            }
        )
        .is_err());
        assert_eq!(state.lock().config.all_fans_boost.percent, 80);
        let invalid = Config {
            all_fans_boost: AllFansBoost {
                enabled: true,
                percent: 50,
            },
            ..Default::default()
        };
        assert!(validate_config(&invalid).is_err());
    }

    #[test]
    fn a_partial_device_write_rolls_back_even_the_failing_fan() {
        let (state, hw) = fixture(Config::default());
        hw.lock().unwrap().fail_writes.push((1, 80));
        let error = apply_all_fans_boost(&state, true, 80).unwrap_err();
        assert_eq!(error.kind(), "control");
        assert!(error.to_string().contains("Previous fan boosts restored"));
        assert_eq!(hw.lock().unwrap().boosts, [20, 30]);
        let inner = state.lock();
        assert_eq!(inner.config.mode, FanMode::Auto);
        assert!(!inner.config.all_fans_boost.enabled);
        assert_eq!(inner.db.as_ref().unwrap().load_config().mode, FanMode::Auto);
        assert!(!inner.control.suspended);
    }

    #[test]
    fn persistence_failure_restores_hardware_and_reports_database_stage() {
        let (state, hw) = fixture(Config::default());
        state.lock().db.as_ref().unwrap().reject_writes();
        let error = apply_all_fans_boost(&state, true, 90).unwrap_err();
        assert_eq!(error.kind(), "database");
        assert_eq!(hw.lock().unwrap().boosts, [20, 30]);
        assert_eq!(state.lock().config.mode, FanMode::Auto);
        assert!(!state.lock().config.all_fans_boost.enabled);
        assert_eq!(
            state.lock().db.as_ref().unwrap().load_config().mode,
            FanMode::Auto
        );
    }

    #[test]
    fn failed_rollback_suspends_control_until_an_explicit_retry() {
        let (state, hw) = fixture(Config::default());
        hw.lock().unwrap().fail_writes = vec![(1, 80), (0, 20)];
        apply_all_fans_boost(&state, true, 80).unwrap_err();
        assert!(state.lock().control.suspended);
        assert_eq!(state.lock().control.applied_mode, None);
        let writes = hw.lock().unwrap().writes.len();
        let sample = crate::monitor::tick(&state, 2.0).unwrap();
        assert!(sample.control.unwrap().suspended);
        assert_eq!(hw.lock().unwrap().writes.len(), writes);
        hw.lock().unwrap().fail_writes.clear();
        apply_mode(&state, FanMode::Auto).unwrap();
        assert!(!state.lock().control.suspended);
        crate::monitor::tick(&state, 2.0).unwrap();
        assert_eq!(state.lock().control.applied_mode, Some(FanMode::Auto));
    }

    #[test]
    fn startup_reapplies_saved_individual_and_global_boosts() {
        for global in [false, true] {
            let mut config = Config {
                mode: FanMode::Manual,
                ..Default::default()
            };
            config.manual_boosts = [("fan1".into(), 45), ("fan2".into(), 65)].into();
            config.all_fans_boost = AllFansBoost {
                enabled: global,
                percent: 85,
            };
            let (state, hw) = fixture(config);
            restore_control(&state);
            assert_eq!(
                hw.lock().unwrap().boosts,
                if global { [85, 85] } else { [45, 65] }
            );
            let inner = state.lock();
            assert_eq!(inner.control.applied_mode, Some(FanMode::Manual));
            assert_eq!(inner.config.all_fans_boost.enabled, global);
            assert!(inner.control.error.is_none());
            if global {
                assert_eq!(inner.config.manual_boosts["fan1"], 85);
            }
        }
    }

    #[test]
    fn startup_failure_does_not_claim_the_saved_mode_was_applied() {
        let config = Config {
            mode: FanMode::Manual,
            manual_boosts: [("fan1".into(), 80)].into(),
            ..Default::default()
        };
        let (state, hw) = fixture(config);
        hw.lock().unwrap().fail_writes.push((0, 80));
        restore_control(&state);
        assert!(state.lock().control.suspended);
        assert!(state
            .lock()
            .control
            .error
            .as_ref()
            .unwrap()
            .contains("Could not restore"));
        assert_eq!(state.lock().control.applied_mode, None);
        assert_eq!(hw.lock().unwrap().boosts, [20, 30]);
    }

    #[test]
    fn unreadable_snapshot_aborts_before_writing_any_fan() {
        let (state, hw) = fixture(Config::default());
        hw.lock().unwrap().fail_reads = true;
        assert!(apply_all_fans_boost(&state, true, 80).is_err());
        assert!(hw.lock().unwrap().writes.is_empty());
        assert_eq!(state.lock().config.mode, FanMode::Auto);
    }

    #[test]
    fn maintenance_blocks_monitor_persistence_and_reentrant_operations() {
        let (state, hw) = fixture(Config::default());
        crate::monitor::tick(&state, 2.0).unwrap();
        begin_maintenance(&state).unwrap();
        let history_len = state.lock().history.len();
        let writes = hw.lock().unwrap().writes.len();
        assert!(crate::monitor::tick(&state, 2.0).is_none());
        assert!(apply_mode(&state, FanMode::Auto).is_err());
        assert!(apply_config_patch(
            &state,
            ConfigPatch {
                theme: Some(Theme::Dark),
                ..Default::default()
            }
        )
        .is_err());
        assert!(begin_maintenance(&state).is_err());
        assert!(state.lock().control.maintenance);
        assert_eq!(hw.lock().unwrap().writes.len(), writes);
        assert_eq!(state.lock().history.len(), history_len);
        assert_eq!(hw.lock().unwrap().boosts, [0, 0]);
        state.lock().finish_maintenance();
        assert!(!state.lock().control.maintenance);
        assert_eq!(state.lock().config.mode, FanMode::Disabled);
        crate::monitor::tick(&state, 2.0).unwrap();
        assert_eq!(hw.lock().unwrap().writes.len(), writes);
        assert_eq!(state.lock().history.len(), history_len + 1);
    }

    #[test]
    fn deletion_failure_keeps_a_closed_database_and_monitor_paused() {
        let (state, _) = fixture(Config::default());
        begin_maintenance(&state).unwrap();
        drop(state.lock().db.take());
        state.lock().finish_maintenance();
        assert!(state.lock().control.maintenance);
        assert!(crate::monitor::tick(&state, 2.0).is_none());
        assert!(apply_mode(&state, FanMode::Auto).is_err());
    }
}
