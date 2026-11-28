use crate::automatic::EMERGENCY_TEMP_C;
use crate::config::{AutoStrategy, FanMode};
use crate::curve;
use crate::state::{AppState, FanReading, Inner, TempReading};
use crate::thermal::Fan;

pub(crate) fn apply_automatic(
    inner: &mut Inner,
    fans: &[Box<dyn Fan>],
    readings: &[Option<TempReading>],
    fan_readings: &mut [FanReading],
    errors: &mut Vec<String>,
    elapsed_secs: f32,
) {
    if inner.config.mode != FanMode::Auto
        || inner.control.suspended
        || !inner
            .device
            .as_ref()
            .is_some_and(|device| device.supports_fan_control())
    {
        return;
    }
    let missing_sensor = readings.is_empty() || readings.iter().any(Option::is_none);
    let mut all_applied = !fans.is_empty();
    let max_temp = readings
        .iter()
        .flatten()
        .map(|t| t.celsius)
        .fold(f32::MIN, f32::max);
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
            inner.control.last_applied_ms = Some(crate::readings::now_ms());
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

pub(crate) fn release_fan(fan: &dyn Fan) -> crate::thermal::Result<()> {
    fan.set_boost(fan.boost_range().0)
}

pub fn return_to_firmware(state: &AppState) {
    let mut inner = state.lock();
    inner.shutting_down = true;
    inner.control.maintenance = true;
    if let Some(device) = inner
        .device
        .as_ref()
        .filter(|device| device.supports_fan_control())
    {
        for fan in device.fans() {
            if let Err(error) = release_fan(fan.as_ref()) {
                eprintln!(
                    "thermal-berry: could not return {} to firmware on exit: {error}",
                    fan.id()
                );
            }
        }
    }
}
