use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::curve::CurvePoint;

/// `Auto` regulates temperature or applies an advanced curve; `Manual`
/// applies a fixed per-fan percentage; `Disabled`
/// hands every fan back to the EC once and the app doesn't touch them again
/// until another mode is picked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FanMode {
    Auto,
    Manual,
    Disabled,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoStrategy {
    #[default]
    Adaptive,
    Curve,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutomaticControl {
    pub strategy: AutoStrategy,
    pub target_temp_c: f32,
}

impl Default for AutomaticControl {
    fn default() -> Self {
        Self {
            strategy: AutoStrategy::Adaptive,
            target_temp_c: 70.0,
        }
    }
}

impl AutomaticControl {
    pub fn is_valid(&self) -> bool {
        self.target_temp_c.is_finite() && (50.0..=85.0).contains(&self.target_temp_c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TempUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Switch that sets every fan to the same manual boost percentage, or back
/// to automatic (EC) control when turned off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AllFansBoost {
    pub enabled: bool,
    pub percent: u8,
}

impl Default for AllFansBoost {
    fn default() -> Self {
        Self {
            enabled: false,
            percent: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub revision: u64,
    pub control_epoch: u64,
    pub update_interval_secs: u64,
    pub temp_unit: TempUnit,
    pub theme: Theme,
    pub mode: FanMode,
    pub manual_boosts: HashMap<String, u8>,
    pub automatic_control: AutomaticControl,
    pub curves: HashMap<String, Vec<CurvePoint>>,
    pub vendor_override: Option<String>,
    pub history_retention_days: u32,
    pub all_fans_boost: AllFansBoost,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            revision: 0,
            control_epoch: 0,
            update_interval_secs: 2,
            temp_unit: TempUnit::Celsius,
            theme: Theme::System,
            mode: FanMode::Auto,
            manual_boosts: HashMap::new(),
            automatic_control: AutomaticControl::default(),
            curves: HashMap::new(),
            vendor_override: None,
            history_retention_days: 7,
            all_fans_boost: AllFansBoost::default(),
        }
    }
}

impl Config {
    /// Sanitized interval: minimum 1s to avoid saturation, maximum 60s.
    pub fn interval_secs(&self) -> u64 {
        self.update_interval_secs.clamp(1, 60)
    }

    /// Sanitized retention: at least 1 day, at most 1 year.
    pub fn retention_days(&self) -> u32 {
        self.history_retention_days.clamp(1, 365)
    }
}

impl AllFansBoost {
    /// Sanitized boost percentage, in case a stale/corrupt value ever
    /// exceeds 100 (percent is a plain u8, not itself bounded at 100).
    pub fn percent(&self) -> u8 {
        self.percent.min(100)
    }
}
