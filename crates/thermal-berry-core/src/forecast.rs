use crate::model::{Inputs, Temperature, ThermalModel};
use serde::Serialize;

/// Empirical radius about the point forecast and its measured coverage on
/// a later contiguous block. The radius is in degrees Celsius.
#[derive(Debug, Serialize)]
pub struct ResidualBand {
    pub radius_c: f64,
    pub holdout_coverage: f64,
}

#[derive(Debug, Serialize)]
pub struct ForecastInterval {
    pub horizon_s: u64,
    pub calibration_origins: usize,
    pub holdout_origins: usize,
    pub cpu: ResidualBand,
    pub gpu: Option<ResidualBand>,
}

impl ThermalModel {
    /// Conditional forecast at constant observed load and fan boost.
    pub fn forecast(
        &self,
        mut t: Temperature,
        u: Inputs,
        horizon_seconds: f64,
    ) -> Option<Temperature> {
        let pct = |p: f64| (0.0..=100.0).contains(&p);
        if !(0.0..=300.0).contains(&horizon_seconds)
            || !(0.0..=150.0).contains(&t.cpu)
            || !t.gpu.is_none_or(|v| (0.0..=150.0).contains(&v))
            || !pct(u.cpu_percent)
            || !pct(u.fan_percent)
            || !u.gpu_percent.is_none_or(pct)
            || (self.gpu.is_some() && (t.gpu.is_none() || u.gpu_percent.is_none()))
        {
            return None;
        }
        if self.gpu.is_none() {
            t.gpu = None;
        }
        let mut remaining = horizon_seconds;
        while remaining > 0.0 {
            let dt = remaining.min(2.0);
            t = self.step(t, u, dt)?;
            remaining -= dt;
        }
        Some(t)
    }
}
