use serde::Serialize;

#[cfg(target_os = "linux")]
use crate::{
    curve,
    state::FanReading,
    thermal::{Fan, ThermalDevice},
};
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempReading {
    pub label: String,
    pub celsius: f32,
}

/// Result of one read-only acquisition. Failed sensors keep their positions
/// so consumers can distinguish a missing reading from another sensor.
pub struct Readings {
    pub timestamp_ms: u64,
    pub temperatures: Vec<Option<TempReading>>,
    pub errors: Vec<String>,
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
pub(crate) type Fans = Arc<[Box<dyn Fan>]>;

#[cfg(target_os = "linux")]
pub(crate) struct HardwareReadings {
    pub readings: Readings,
    pub fans: Fans,
    pub fan_readings: Vec<FanReading>,
}

#[cfg(target_os = "linux")]
pub(crate) fn read_hardware(device: Option<&dyn ThermalDevice>) -> HardwareReadings {
    let timestamp_ms = now_ms();
    let mut errors = Vec::new();
    let (readings, fans) = if let Some(device) = device {
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
    let fan_control = device.is_some_and(|device| device.supports_fan_control());
    if fan_control && readings.is_empty() {
        errors.push("No temperature sensors reporting".into());
    }
    if fan_control && fans.is_empty() {
        errors.push("No fans detected".into());
    }
    let fan_readings: Vec<FanReading> = fans
        .iter()
        .map(|fan| {
            let rpm = match fan.read_rpm() {
                Ok(value) => Some(value),
                Err(e) => {
                    errors.push(format!("{} RPM: {e}", fan.id()));
                    None
                }
            };
            let boost_percent = if fan_control {
                match fan.read_boost() {
                    Ok(value) => Some(curve::boost_to_percent(value, fan.boost_range())),
                    Err(e) => {
                        errors.push(format!("{} boost: {e}", fan.id()));
                        None
                    }
                }
            } else {
                None
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

    HardwareReadings {
        readings: Readings {
            timestamp_ms,
            temperatures: readings,
            errors,
        },
        fans: fans.into(),
        fan_readings,
    }
}
