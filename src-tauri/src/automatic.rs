pub const EMERGENCY_TEMP_C: f32 = 90.0;
const EMERGENCY_RELEASE_C: f32 = 85.0;
const DEADBAND_C: f32 = 2.0;
const ADJUSTMENT_INTERVAL_SECS: f32 = 10.0;
const BOOST_STEP: u8 = 5;

#[derive(Debug, Default, Clone, Copy)]
pub struct AutomaticController {
    percent: Option<u8>,
    emergency: bool,
    window_elapsed_secs: f32,
    window_start_temp: Option<f32>,
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
            self.percent = Some(100);
            self.window_elapsed_secs = 0.0;
            self.window_start_temp = None;
            return 100;
        }

        let first_reading = self.percent.is_none();
        let percent = self.percent.get_or_insert(current.min(100));
        if temp_c <= target_c - DEADBAND_C {
            // Zero removes added boost; firmware still controls base cooling.
            *percent = 0;
            self.window_elapsed_secs = 0.0;
            self.window_start_temp = Some(temp_c);
            return 0;
        }

        if let Some(start_temp) = self.window_start_temp {
            // A resume from suspend must not accumulate missed adjustments.
            self.window_elapsed_secs += elapsed_secs.clamp(0.0, 2.0);
            if self.window_elapsed_secs >= ADJUSTMENT_INTERVAL_SECS {
                if temp_c > target_c + DEADBAND_C && temp_c >= start_temp {
                    *percent = (*percent + BOOST_STEP).min(100);
                }
                self.window_elapsed_secs = 0.0;
                self.window_start_temp = Some(temp_c);
            }
        } else {
            self.window_start_temp = Some(temp_c);
            if first_reading && temp_c > target_c + DEADBAND_C {
                *percent = (*percent + BOOST_STEP).min(100);
            }
        }
        *percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advance_window(control: &mut AutomaticController, temp_c: f32, target_c: f32) -> u8 {
        let mut percent = 0;
        for _ in 0..5 {
            percent = control.update(temp_c, target_c, 2.0, percent);
        }
        percent
    }

    #[test]
    fn sustained_heat_increases_only_after_ten_seconds() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(78.0, 70.0, 0.0, 20), 25);
        for _ in 0..4 {
            assert_eq!(control.update(78.0, 70.0, 2.0, 25), 25);
        }
        assert_eq!(control.update(78.0, 70.0, 2.0, 25), 30);
        assert_eq!(advance_window(&mut control, 80.0, 70.0), 35);
    }

    #[test]
    fn falling_temperature_holds_boost_and_refreshes_the_reference() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(80.0, 70.0, 2.0, 20), 25);
        assert_eq!(advance_window(&mut control, 78.0, 70.0), 25);
        assert_eq!(advance_window(&mut control, 76.0, 70.0), 25);
        assert_eq!(advance_window(&mut control, 76.0, 70.0), 30);
    }

    #[test]
    fn deadband_holds_output_including_the_upper_boundary() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(70.0, 70.0, 2.0, 60), 60);
        for temp in [71.5, 68.5, 72.0, 72.0] {
            assert_eq!(advance_window(&mut control, temp, 70.0), 60);
        }
    }

    #[test]
    fn reaching_the_lower_boundary_removes_boost_without_waiting() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(60.0, 50.0, 2.0, 60), 65);
        assert_eq!(control.update(48.0, 50.0, 0.0, 65), 0);
        assert_eq!(control.update(45.0, 50.0, 2.0, 0), 0);
    }

    #[test]
    fn returning_from_zero_requires_a_new_window_above_the_upper_boundary() {
        let mut control = AutomaticController::default();
        control.update(60.0, 50.0, 0.0, 60);
        for _ in 0..4 {
            control.update(60.0, 50.0, 2.0, 65);
        }
        assert_eq!(control.update(48.0, 50.0, 2.0, 65), 0);
        assert_eq!(advance_window(&mut control, 52.0, 50.0), 0);
        for _ in 0..4 {
            assert_eq!(control.update(53.0, 50.0, 2.0, 0), 0);
        }
        assert_eq!(control.update(53.0, 50.0, 2.0, 0), 5);
    }

    #[test]
    fn an_initial_cool_reading_removes_existing_boost() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(48.0, 50.0, 0.0, 60), 0);
    }

    #[test]
    fn response_depends_on_elapsed_time_not_sampling_frequency() {
        let mut slow = AutomaticController::default();
        let mut fast = AutomaticController::default();
        slow.update(78.0, 70.0, 0.0, 20);
        fast.update(78.0, 70.0, 0.0, 20);
        for second in 1..=20 {
            let actual = fast.update(78.0, 70.0, 1.0, 20);
            if second % 2 == 0 {
                assert_eq!(slow.update(78.0, 70.0, 2.0, 20), actual);
            }
        }
        assert_eq!(fast.update(78.0, 70.0, 0.0, 20), 35);
    }

    #[test]
    fn emergency_bypasses_the_wait_and_has_hysteresis() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(80.0, 85.0, 0.0, 20), 0);
        assert_eq!(control.update(90.0, 85.0, 0.0, 0), 100);
        assert_eq!(control.update(88.0, 85.0, 0.0, 100), 100);
        assert_eq!(control.update(86.0, 85.0, 0.0, 100), 100);
        assert_eq!(control.update(85.0, 85.0, 0.0, 100), 100);
        assert_eq!(control.update(83.0, 85.0, 0.0, 100), 0);
    }

    #[test]
    fn emergency_release_starts_a_fresh_temperature_window() {
        let mut control = AutomaticController::default();
        control.update(90.0, 70.0, 0.0, 0);
        assert_eq!(control.update(85.0, 70.0, 2.0, 100), 100);
        assert_eq!(advance_window(&mut control, 80.0, 70.0), 100);
        assert_eq!(control.update(68.0, 70.0, 0.0, 100), 0);
    }

    #[test]
    fn invalid_temperature_requests_full_cooling() {
        for (temp, target) in [
            (f32::NAN, 70.0),
            (f32::INFINITY, 70.0),
            (f32::NEG_INFINITY, 70.0),
            (50.0, f32::NAN),
            (50.0, f32::INFINITY),
        ] {
            let mut control = AutomaticController::default();
            assert_eq!(control.update(temp, target, 0.0, 0), 100);
            assert_eq!(control.update(68.0, 70.0, 0.0, 100), 0);
        }
    }

    #[test]
    fn boost_is_bounded_and_cooldown_clears_saturation() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(89.0, 70.0, 0.0, 98), 100);
        for _ in 0..100 {
            assert_eq!(advance_window(&mut control, 89.0, 70.0), 100);
        }
        assert_eq!(control.update(50.0, 70.0, 0.0, 100), 0);
        assert_eq!(advance_window(&mut control, 78.0, 70.0), 5);
        let mut control = AutomaticController::default();
        assert_eq!(control.update(78.0, 70.0, 0.0, 255), 100);
    }

    #[test]
    fn suspension_does_not_accumulate_missed_adjustments() {
        let mut control = AutomaticController::default();
        assert_eq!(control.update(78.0, 70.0, 0.0, 20), 25);
        assert_eq!(control.update(78.0, 70.0, 600.0, 25), 25);
        assert_eq!(control.update(78.0, 70.0, -10.0, 25), 25);
        for _ in 0..3 {
            assert_eq!(control.update(78.0, 70.0, 2.0, 25), 25);
        }
        assert_eq!(control.update(78.0, 70.0, 2.0, 25), 30);
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
