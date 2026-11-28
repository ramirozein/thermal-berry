use thermal_berry_core::{
    config::FanMode,
    identify::{identify, validate_session, SESSION_SECS},
    model::{Inputs, Temperature, ThermalModel, FEATURE_COUNT},
    observation::{FanObservation, Observation},
};

fn truth(coupled: bool) -> ThermalModel {
    let mut cpu = [0.0; FEATURE_COUNT];
    cpu[0] = 0.62;
    cpu[1] = -0.014;
    cpu[5] = 0.0032;
    cpu[14] = -0.00006;
    let mut gpu = [0.0; FEATURE_COUNT];
    gpu[0] = 0.69;
    gpu[2] = -0.015;
    gpu[7] = 0.0038;
    gpu[15] = -0.00007;
    if coupled {
        cpu[2] = 0.003;
        cpu[7] = 0.0013;
        gpu[1] = 0.0025;
        gpu[5] = 0.001;
    }
    ThermalModel {
        fan_ids: vec![],
        cpu,
        gpu: Some(gpu),
    }
}

fn dual_truth(coupled: bool) -> ThermalModel {
    let mut model = truth(coupled);
    model.fan_ids = vec!["intake".into(), "outlet".into()];
    model.cpu[14] = -0.000045;
    model.cpu[18] = -0.000025;
    let gpu = model.gpu.as_mut().unwrap();
    gpu[15] = -0.000025;
    gpu[19] = -0.000055;
    model
}

fn session(model: &ThermalModel, noise: f64) -> Vec<Observation> {
    session_with_periods(model, noise, [131, 97, 79])
}

fn session_with_periods(model: &ThermalModel, noise: f64, periods: [u64; 3]) -> Vec<Observation> {
    let [c, g, f] = periods;
    simulate(model, noise, |k| Inputs {
        cpu_percent: [12.0, 52.0, 86.0][((k / c) % 3) as usize],
        gpu_percent: Some([84.0, 14.0, 54.0][((k / g) % 3) as usize]),
        fan_percent: [42.0, 82.0, 55.0, 70.0][((k / f) % 4) as usize],
        second_fan_percent: (model.fan_ids.len() == 2)
            .then(|| [70.0, 40.0, 80.0, 55.0][(k / 103 % 4) as usize]),
    })
}

fn simulate(model: &ThermalModel, noise: f64, input: impl Fn(u64) -> Inputs) -> Vec<Observation> {
    let mut seed = 3089u32;
    let mut uniform = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        f64::from(seed) / f64::from(u32::MAX)
    };
    let mut gaussian = || {
        (-2.0 * uniform().max(f64::MIN_POSITIVE).ln()).sqrt()
            * (std::f64::consts::TAU * uniform()).cos()
    };
    let mut t = Temperature {
        cpu: 52.0,
        gpu: Some(48.0),
    };
    let mut rows = Vec::new();
    for k in 0..SESSION_SECS / 2 {
        let u = input(k);
        let mut row = Observation {
            timestamp_ms: 2_000 * k,
            cpu_temp_c: Some(t.cpu + noise * gaussian()),
            gpu_temp_c: t.gpu.map(|g| g + noise * gaussian()),
            cpu_percent: Some(u.cpu_percent),
            gpu_percent: u.gpu_percent,
            fan_percent: Some(u.fan_percent),
            rpm: Some(2200),
            fans: vec![],
            mode: Some(FanMode::Manual),
            mem_percent: None,
            gpu_mem_percent: None,
            gpu_power_w: None,
        };
        if let Some(f2) = u.second_fan_percent {
            row.fan_percent = None;
            row.rpm = None;
            row.fans = [u.fan_percent, f2]
                .into_iter()
                .zip(&model.fan_ids)
                .map(|(p, id)| FanObservation {
                    id: id.clone(),
                    percent: Some(p),
                    rpm: Some(1000 + (p * 25.0) as u32),
                })
                .collect();
            if k % 2 == 0 {
                row.fans.reverse();
            }
        }
        rows.push(row);
        // Independent plant equations: errors in the fitted feature ordering or
        // in simultaneous integration must be observable in this fixture.
        let g = t.gpu.unwrap();
        let lg = u.gpu_percent.unwrap();
        let cpu = &model.cpu;
        let gpu = model.gpu.as_ref().unwrap();
        let dc = cpu[0]
            + cpu[1] * t.cpu
            + cpu[2] * g
            + cpu[5] * u.cpu_percent
            + cpu[7] * lg
            + cpu[14] * u.fan_percent * t.cpu
            + cpu[18] * u.second_fan_percent.unwrap_or_default() * t.cpu;
        let dg = gpu[0]
            + gpu[1] * t.cpu
            + gpu[2] * g
            + gpu[5] * u.cpu_percent
            + gpu[7] * lg
            + gpu[15] * u.fan_percent * g
            + gpu[19] * u.second_fan_percent.unwrap_or_default() * g;
        t = Temperature {
            cpu: t.cpu + 2.0 * dc,
            gpu: Some(g + 2.0 * dg),
        };
    }
    rows
}

fn check_coefficients(model: &ThermalModel, expected: &ThermalModel) {
    for (die, got, want) in [
        ("cpu", &model.cpu, &expected.cpu),
        (
            "gpu",
            model.gpu.as_ref().unwrap(),
            expected.gpu.as_ref().unwrap(),
        ),
    ] {
        let mut max_relative_error = 0.0_f64;
        for (g, w) in got.iter().zip(want) {
            if *w == 0.0 {
                assert_eq!(*g, 0.0);
            } else {
                let relative_error = (g / w - 1.0).abs();
                max_relative_error = max_relative_error.max(relative_error);
                assert!(relative_error < 0.10, "{g} vs {w}");
            }
        }
        eprintln!(
            "{die} maximum relative coefficient error {:.3}%",
            100.0 * max_relative_error
        );
    }
}

#[test]
fn recovers_known_coupled_and_decoupled_equations() {
    for coupled in [true, false] {
        let expected = truth(coupled);
        let mut data = session(&expected, 0.0);
        if !coupled {
            for row in &mut data {
                row.fans = vec![FanObservation {
                    id: "chassis".into(),
                    percent: row.fan_percent,
                    rpm: row.rpm,
                }];
            }
        }
        let report = identify(&data, false).unwrap();
        assert!(report.accepted, "{report:?}");
        assert_eq!(report.terms.len(), 17);
        if !coupled {
            assert_eq!(report.model.fan_ids, ["chassis"]);
            data[4].fan_percent = Some(99.0);
            assert!(validate_session(&data, false).is_err());
        }
        check_coefficients(&report.model, &expected);
        eprintln!("coupled={coupled} {scores:?}", scores = report.sindy);
    }
}

#[test]
fn physically_separate_cooling_does_not_require_cross_fan_terms() {
    let mut separate = dual_truth(false);
    separate.cpu[18] = 0.0;
    separate.gpu.as_mut().unwrap()[15] = 0.0;
    let report = identify(&session(&separate, 0.0), false).unwrap();
    assert!(report.accepted);
    check_coefficients(&report.model, &separate);
}

#[test]
fn cpu_only_does_not_impute_gpu_and_refuses_unsafe_or_unexcited_data() {
    let mut data = session(&truth(false), 0.0);
    for r in &mut data {
        r.gpu_percent = None;
    }
    let report = identify(&data, false).unwrap();
    assert!(report.accepted && report.model.gpu.is_none() && report.sindy.gpu.is_none());
    assert!(report.residual_intervals_90.iter().all(|i| i.gpu.is_none()));
    let mut bad = data.clone();
    bad.drain(300..305);
    assert!(identify(&bad, false).is_err());
    let mut bad = data.clone();
    bad[1].cpu_temp_c = Some(f64::NAN);
    assert!(identify(&bad, false).is_err());
    let mut bad = data.clone();
    bad[1].rpm = Some(0);
    assert!(identify(&bad, false).is_err());
    let mut bad = data.clone();
    bad[1].mode = Some(FanMode::Auto);
    assert!(identify(&bad, false).is_err());
    for row in &mut data {
        row.fan_percent = Some(50.0);
    }
    assert!(identify(&data, false).is_err());
}

#[test]
fn noisy_holdout_reports_both_dies_and_baselines() {
    let data = session(&truth(true), 0.2);
    let report = identify(&data, false).unwrap();
    eprintln!(
        "noisy accepted={} monotone={} sindy={:?} arx={:?} rc={:?} persistence={:?} rls={:?}",
        report.accepted,
        report.monotone,
        report.sindy,
        report.arx,
        report.rc,
        report.persistence,
        report.rls
    );
    check_coefficients(&report.model, &truth(true));
    assert!(report.accepted && report.monotone && report.rls.is_some());
    assert!(report.arx.is_some() && report.rc.is_some());
    for e in [&report.sindy.cpu, report.sindy.gpu.as_ref().unwrap()] {
        assert!(e.rmse_1step < 1.5 && e.rmse_60s < 3.0);
    }
    let mut shifted = data.clone();
    let holdout_start = shifted.len() * 4 / 5;
    for row in &mut shifted[holdout_start..] {
        row.cpu_temp_c = row.cpu_temp_c.map(|t| t + 2.0);
    }
    let shifted = identify(&shifted, false).unwrap();
    assert_eq!(report.residual_intervals_90.len(), 3);
    for (before, after) in report
        .residual_intervals_90
        .iter()
        .zip(shifted.residual_intervals_90)
    {
        assert_eq!(before.cpu.radius_c, after.cpu.radius_c);
        assert_eq!(
            before.gpu.as_ref().unwrap().radius_c,
            after.gpu.as_ref().unwrap().radius_c
        );
        assert!(before.cpu.radius_c > 0.0 && (0.0..=1.0).contains(&after.cpu.holdout_coverage));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scheduled_calibration_excites_both_dies_and_the_shared_fan() {
    let expected = truth(true);
    let data = simulate(&expected, 0.2, |k| {
        let stage = thermal_berry_core::calibration::stage(k * 2, false);
        Inputs {
            cpu_percent: f64::from(stage.cpu_percent),
            gpu_percent: Some(f64::from(stage.gpu_percent)),
            fan_percent: f64::from(stage.fan_percent),
            second_fan_percent: None,
        }
    });
    let report = identify(&data, false).unwrap();
    eprintln!(
        "M0 schedule on the noisy plant: {:?}, accepted {}",
        report.sindy, report.accepted
    );
    check_coefficients(&report.model, &expected);
    assert!(report.accepted && report.monotone);
}

#[test]
fn independent_fans_are_identified_by_id_on_both_dies_and_in_cpu_only_sessions() {
    let expected = dual_truth(true);
    let report = identify(&session(&expected, 0.0), false).unwrap();
    assert!(report.accepted && report.monotone && report.rls.is_some());
    assert_eq!(report.model.fan_ids, expected.fan_ids);
    assert_eq!(
        &report.terms[13..],
        ["F1", "F1*T_cpu", "F1*T_gpu", "F1^2", "F2", "F2*T_cpu", "F2*T_gpu", "F2^2"]
    );
    check_coefficients(&report.model, &expected);
    let t = Temperature {
        cpu: 60.0,
        gpu: Some(55.0),
    };
    let u = Inputs {
        cpu_percent: 50.0,
        gpu_percent: Some(50.0),
        fan_percent: 40.0,
        second_fan_percent: Some(40.0),
    };
    let reference = report.model.forecast(t, u, 60.0).unwrap();
    for raised in [
        Inputs {
            fan_percent: 70.0,
            ..u
        },
        Inputs {
            second_fan_percent: Some(70.0),
            ..u
        },
    ] {
        let cooler = report.model.forecast(t, raised, 60.0).unwrap();
        assert!(cooler.cpu < reference.cpu && cooler.gpu.unwrap() < reference.gpu.unwrap());
    }
    for horizon in [0.0, 60.0] {
        assert!(report
            .model
            .forecast(
                t,
                Inputs {
                    second_fan_percent: None,
                    ..u
                },
                horizon
            )
            .is_none());
        assert!(truth(true).forecast(t, u, horizon).is_none());
    }
    let mut data = session(&dual_truth(false), 0.0);
    for row in &mut data {
        row.gpu_percent = None;
        row.gpu_temp_c = None;
    }
    let report = identify(&data, false).unwrap();
    assert!(report.accepted && report.model.gpu.is_none() && report.model.cpu[18] < 0.0);
    assert_eq!(report.model.fan_ids, expected.fan_ids);
}

#[cfg(target_os = "linux")]
#[test]
fn scheduled_dual_calibration_recovers_cross_cooling_with_sensor_noise() {
    let model = dual_truth(true);
    let rows = simulate(&model, 0.2, |k| {
        let stage = thermal_berry_core::calibration::stage(2 * k, true);
        Inputs {
            cpu_percent: stage.cpu_percent.into(),
            gpu_percent: Some(stage.gpu_percent.into()),
            fan_percent: stage.fan_percent.into(),
            second_fan_percent: stage.second_fan_percent.map(f64::from),
        }
    });
    let report = identify(&rows, false).unwrap();
    eprintln!(
        "dual M0 on noisy plant: {:?}, monotone {}, accepted {}",
        report.sindy, report.monotone, report.accepted
    );
    assert!(report.accepted && report.monotone && report.rls.is_some());
    check_coefficients(&report.model, &model);
}

#[test]
fn rejects_ambiguous_dual_fan_captures_and_loss_of_feedback() {
    let rows = session(&dual_truth(true), 0.0);
    for malformed in 0..7 {
        let mut data = rows.clone();
        match malformed {
            0 => data[0].fans[0].id = data[0].fans[1].id.clone(),
            1 => data[700].fans[1].id = "swapped".into(),
            2 => {
                data[300].fans.pop();
            }
            3 => data[322].fans[1].rpm = Some(0),
            4 => {
                for r in &mut data[400..404] {
                    r.fans[0].percent = None;
                }
            }
            5 => data[0].fan_percent = Some(60.0),
            _ => data[0].fans.push(FanObservation {
                id: "third".into(),
                percent: Some(50.0),
                rpm: Some(1800),
            }),
        }
        assert!(validate_session(&data, false).is_err());
    }
    for opposite in [false, true] {
        let mut data = rows.clone();
        for row in &mut data {
            let first = row
                .fans
                .iter()
                .find(|f| f.id == "intake")
                .unwrap()
                .percent
                .unwrap();
            row.fans
                .iter_mut()
                .find(|f| f.id == "outlet")
                .unwrap()
                .percent = Some(if opposite { 125.0 - first } else { first });
        }
        assert!(validate_session(&data, false)
            .unwrap_err()
            .contains("independent fan excitation"));
    }
}

#[test]
#[ignore = "timing benchmark, run with --release --ignored --nocapture"]
fn timing() {
    use std::{hint::black_box, time::Instant};
    for model in [truth(true), dual_truth(true)] {
        let data = session(&model, 0.2);
        let report = identify(black_box(&data), false).unwrap();
        let mut timings = Vec::new();
        for _ in 0..15 {
            let start = Instant::now();
            black_box(identify(black_box(&data), false).unwrap());
            timings.push(start.elapsed());
        }
        timings.sort_unstable();
        let fit = timings[timings.len() / 2];
        let t = Temperature {
            cpu: 55.0,
            gpu: Some(52.0),
        };
        let u = Inputs {
            cpu_percent: 55.0,
            gpu_percent: Some(45.0),
            fan_percent: 60.0,
            second_fan_percent: (model.fan_ids.len() == 2).then_some(50.0),
        };
        let start = Instant::now();
        for _ in 0..10000 {
            black_box(model.forecast(black_box(t), black_box(u), 300.0).unwrap());
        }
        let forecast = start.elapsed() / 10000;
        eprintln!("{} fan(s), median of 15 fits including validation {fit:?}, 5-minute forecast {forecast:?}, serialized model {} bytes", model.fan_ids.len().max(1), serde_json::to_vec(&report.model).unwrap().len());
    }
}

#[test]
fn unannounced_fast_load_steps_can_fail_the_operational_forecast_gate() {
    let fast = session_with_periods(&truth(true), 0.2, [37, 53, 31]);
    let report = identify(&fast, false).unwrap();
    eprintln!("fast steps: CPU RMSE 60s {}", report.sindy.cpu.rmse_60s);
    assert!(!report.accepted);
    assert!(report.sindy.cpu.rmse_60s >= 3.0);
}
