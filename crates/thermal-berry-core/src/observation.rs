use crate::config::FanMode;
use serde::{Deserialize, Serialize};

/// A driver channel. Its identifier, rather than the enumeration order or
/// display label, binds this measurement to a model input.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FanObservation {
    pub id: String,
    pub percent: Option<f64>,
    pub rpm: Option<u32>,
}

pub(crate) fn valid_fan_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}

#[cfg(target_os = "linux")]
impl From<&crate::state::FanReading> for FanObservation {
    fn from(fan: &crate::state::FanReading) -> Self {
        Self {
            id: fan.id.clone(),
            percent: fan.boost_percent.map(f64::from),
            rpm: fan.rpm,
        }
    }
}

/// One local acquisition. Missing inputs are retained as SQL/JSON nulls.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub timestamp_ms: u64,
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub fan_percent: Option<f64>,
    pub rpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fans: Vec<FanObservation>,
    pub mode: Option<FanMode>,
    pub mem_percent: Option<f64>,
    pub gpu_mem_percent: Option<f64>,
    pub gpu_power_w: Option<f64>,
}

#[cfg(target_os = "linux")]
impl Observation {
    pub(crate) fn set_fans(&mut self, fans: &[crate::state::FanReading]) {
        self.fans = fans.iter().map(FanObservation::from).collect();
        self.fans.sort_by(|a, b| a.id.cmp(&b.id));
        (self.fan_percent, self.rpm) = match self.fans.as_slice() {
            [fan] => (fan.percent, fan.rpm),
            _ => (None, None),
        };
    }
}
