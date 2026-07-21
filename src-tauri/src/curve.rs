use serde::{Deserialize, Serialize};

/// Hysteresis in °C: when cooling down, the fan isn't lowered until the
/// temperature drops at least this much below the point where the current
/// value was set. When heating up, it responds immediately (priority to cooling).
const HYSTERESIS_C: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurvePoint {
    pub temp_c: f32,
    pub percent: u8,
}

/// Default curve: quiet at idle, aggressive near throttling.
pub fn default_points() -> Vec<CurvePoint> {
    [(40.0, 0), (60.0, 25), (75.0, 55), (85.0, 80), (95.0, 100)]
        .into_iter()
        .map(|(temp_c, percent)| CurvePoint { temp_c, percent })
        .collect()
}

/// Linearly interpolates the boost % for a temperature. Out of range,
/// it saturates to the first/last point; with no points it returns 0 (pure auto mode).
pub fn evaluate(points: &[CurvePoint], temp_c: f32) -> u8 {
    let mut sorted: Vec<CurvePoint> = points.to_vec();
    sorted.sort_by(|a, b| a.temp_c.total_cmp(&b.temp_c));

    let Some(first) = sorted.first() else {
        return 0;
    };
    let last = sorted.last().unwrap();
    if temp_c <= first.temp_c {
        return first.percent.min(100);
    }
    if temp_c >= last.temp_c {
        return last.percent.min(100);
    }

    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if temp_c <= b.temp_c {
            let span = b.temp_c - a.temp_c;
            if span <= f32::EPSILON {
                return b.percent.min(100);
            }
            let t = (temp_c - a.temp_c) / span;
            let pct = a.percent as f32 + t * (b.percent as f32 - a.percent as f32);
            return (pct.round() as u8).min(100);
        }
    }
    last.percent.min(100)
}

/// Hysteresis state per fan for curve mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hysteresis {
    applied: Option<(f32, u8)>, // (temp at which it was applied, % applied)
}

impl Hysteresis {
    /// Decides the % to apply for `temp_c`. Increases are applied immediately;
    /// decreases only when the temperature drops HYSTERESIS_C below the
    /// point where the current value was set, to avoid oscillation.
    pub fn update(&mut self, points: &[CurvePoint], temp_c: f32) -> u8 {
        let target = evaluate(points, temp_c);
        match self.applied {
            None => {
                self.applied = Some((temp_c, target));
                target
            }
            Some((applied_temp, applied_pct)) => {
                if target > applied_pct || temp_c <= applied_temp - HYSTERESIS_C {
                    self.applied = Some((temp_c, target));
                    target
                } else {
                    applied_pct
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.applied = None;
    }
}

/// Conversion % (0-100) → boost value within the vendor's range.
pub fn percent_to_boost(percent: u8, range: (u8, u8)) -> u8 {
    let (min, max) = range;
    let pct = percent.min(100) as u32;
    (min as u32 + pct * (max as u32 - min as u32) / 100) as u8
}

/// Inverse conversion, to show the current boost as %.
pub fn boost_to_percent(boost: u8, range: (u8, u8)) -> u8 {
    let (min, max) = range;
    if max <= min {
        return 0;
    }
    let clamped = boost.clamp(min, max) as u32;
    (((clamped - min as u32) * 100 + (max as u32 - min as u32) / 2) / (max as u32 - min as u32))
        as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts() -> Vec<CurvePoint> {
        default_points()
    }

    #[test]
    fn evaluate_clamps_below_and_above() {
        assert_eq!(evaluate(&pts(), 20.0), 0);
        assert_eq!(evaluate(&pts(), 110.0), 100);
    }

    #[test]
    fn evaluate_hits_exact_points() {
        assert_eq!(evaluate(&pts(), 60.0), 25);
        assert_eq!(evaluate(&pts(), 85.0), 80);
    }

    #[test]
    fn evaluate_interpolates_between_points() {
        // Halfway between (60,25) and (75,55) -> 40
        assert_eq!(evaluate(&pts(), 67.5), 40);
    }

    #[test]
    fn evaluate_empty_returns_zero() {
        assert_eq!(evaluate(&[], 80.0), 0);
    }

    #[test]
    fn evaluate_handles_unsorted_input() {
        let mut reversed = pts();
        reversed.reverse();
        assert_eq!(evaluate(&reversed, 67.5), 40);
    }

    #[test]
    fn hysteresis_rises_immediately() {
        let mut h = Hysteresis::default();
        assert_eq!(h.update(&pts(), 60.0), 25);
        assert_eq!(h.update(&pts(), 75.0), 55);
    }

    #[test]
    fn hysteresis_holds_small_drops() {
        let mut h = Hysteresis::default();
        h.update(&pts(), 75.0);
        // Drops 1°C: within the band, keeps 55%
        assert_eq!(h.update(&pts(), 74.0), 55);
        // Drops 2°C from the applied point: recalculates now
        assert!(h.update(&pts(), 73.0) < 55);
    }

    #[test]
    fn percent_boost_roundtrip() {
        assert_eq!(percent_to_boost(0, (0, 255)), 0);
        assert_eq!(percent_to_boost(100, (0, 255)), 255);
        assert_eq!(percent_to_boost(50, (0, 255)), 127);
        assert_eq!(boost_to_percent(255, (0, 255)), 100);
        assert_eq!(boost_to_percent(127, (0, 255)), 50);
        assert_eq!(boost_to_percent(10, (10, 10)), 0);
    }
}
