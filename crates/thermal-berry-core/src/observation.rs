use crate::config::FanMode;
use serde::{Deserialize, Serialize};

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
    pub mode: Option<FanMode>,
    pub mem_percent: Option<f64>,
    pub gpu_mem_percent: Option<f64>,
    pub gpu_power_w: Option<f64>,
}
