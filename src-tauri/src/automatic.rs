//! Feedback control of fan boost. A constant temperature above the target
//! keeps increasing cooling, unlike a temperature-to-percentage curve.

pub const EMERGENCY_TEMP_C: f32 = 90.0;
const EMERGENCY_RELEASE_C: f32 = 85.0;
const DEADBAND_C: f32 = 2.0;

#[derive(Debug, Default, Clone, Copy)]
pub struct AutomaticController {
    percent: Option<f32>,
    emergency: bool,
}

impl AutomaticController {
    pub fn update(&mut self, temp_c: f32, target_c: f32, elapsed_secs: f32, current: u8) -> u8 {
        // Invalid input must never result in reduced cooling.
        if !temp_c.is_finite() || !target_c.is_finite() || temp_c >= EMERGENCY_TEMP_C {
            self.emergency = true;
        } else if temp_c <= EMERGENCY_RELEASE_C {
            self.emergency = false;
        }
        if self.emergency {
            self.percent = Some(100.0);
            return 100;
        }

        // Bound elapsed time so a resume from suspend cannot create a large
        // control jump. Fractional percentages survive between samples.
        let dt = elapsed_secs.clamp(0.0, 2.0);
        let percent = self.percent.get_or_insert(current.min(100) as f32);
        let error = temp_c - target_c;
        let rate = if error > DEADBAND_C {
            (0.5 * (error - DEADBAND_C)).min(10.0)
        } else if error < -DEADBAND_C {
            (0.2 * (error + DEADBAND_C)).max(-2.0)
        } else {
            0.0
        };
        *percent = (*percent + rate * dt).clamp(0.0, 100.0);
        percent.round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sustained_heat_increases_cooling_at_the_same_temperature() {
        let mut control = AutomaticController::default();
        let first = control.update(78.0, 70.0, 2.0, 20);
        let next = control.update(78.0, 70.0, 2.0, first);
        assert!(next > first);
    }

    #[test]
    fn deadband_holds_output_and_cooling_releases_slowly() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(70.0, 70.0, 2.0, 60), 60);
        assert_eq!(control.update(71.5, 70.0, 2.0, 60), 60);
        assert_eq!(control.update(68.5, 70.0, 2.0, 60), 60);
        assert_eq!(control.update(50.0, 70.0, 2.0, 60), 56);
    }

    #[test]
    fn response_depends_on_elapsed_time_not_sampling_frequency() {
        let mut slow = AutomaticController::default();
        let mut fast = AutomaticController::default();
        let expected = slow.update(78.0, 70.0, 2.0, 20);
        fast.update(78.0, 70.0, 1.0, 20);
        assert_eq!(fast.update(78.0, 70.0, 1.0, 20), expected);
    }

    #[test]
    fn emergency_bypasses_ramping_and_has_hysteresis() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(90.0, 70.0, 0.0, 0), 100);
        assert_eq!(control.update(88.0, 70.0, 2.0, 100), 100);
        assert_eq!(control.update(85.0, 85.0, 2.0, 100), 100);
        assert_eq!(control.update(60.0, 70.0, 2.0, 100), 97);
    }

    #[test]
    fn invalid_temperature_requests_full_cooling() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(f32::NAN, 70.0, 2.0, 0), 100);
        assert_eq!(control.update(f32::INFINITY, 70.0, 2.0, 0), 100);
    }

    #[test]
    fn bounds_prevent_windup_and_resume_jumps() {
        let mut control = AutomaticController::default();
        for _ in 0..100 {
            control.update(89.0, 70.0, 2.0, 0);
        }
        assert_eq!(control.update(50.0, 70.0, 600.0, 100), 96);
        for _ in 0..100 {
            control.update(50.0, 70.0, 2.0, 0);
        }
        assert_eq!(control.update(78.0, 70.0, 2.0, 0), 6);
    }

    #[test]
    fn simulated_loads_settle_near_target_with_different_boosts() {
        let mut control = AutomaticController::default();
        let mut temp = 60.0;
        let mut percent = 0;
        let mut outputs = Vec::new();
        // Simple thermal plant: ambient + heat load - fan cooling, with
        // thermal inertia. Validate feedback against two sustained loads.
        for load in [70.0, 90.0] {
            for _ in 0..900 {
                percent = control.update(temp, 70.0, 2.0, percent);
                temp += (25.0 + load - 0.7 * percent as f32 - temp) * 0.04;
            }
            assert!((temp - 70.0).abs() <= 2.1, "temperature: {temp}");
            outputs.push(percent);
        }
        assert!(outputs[1] > outputs[0]);
    }
}
