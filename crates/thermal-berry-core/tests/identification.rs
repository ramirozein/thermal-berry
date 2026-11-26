use thermal_berry_core::{
    config::FanMode,
    identify::{identify, SESSION_SECS},
    model::{Inputs, Temperature, ThermalModel},
    observation::Observation,
};

fn truth(coupled: bool) -> ThermalModel {
    let mut cpu = [0.0; 17];
    cpu[0] = 0.62;
    cpu[1] = -0.014;
    cpu[5] = 0.0032;
    cpu[14] = -0.00006;
    let mut gpu = [0.0; 17];
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
        cpu,
        gpu: Some(gpu),
    }
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
        rows.push(Observation {
            timestamp_ms: 2_000 * k,
            cpu_temp_c: Some(t.cpu + noise * gaussian()),
            gpu_temp_c: t.gpu.map(|g| g + noise * gaussian()),
            cpu_percent: Some(u.cpu_percent),
            gpu_percent: u.gpu_percent,
            fan_percent: Some(u.fan_percent),
            rpm: Some(2200),
            mode: Some(FanMode::Manual),
            mem_percent: None,
            gpu_mem_percent: None,
            gpu_power_w: None,
        });
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
            + cpu[14] * u.fan_percent * t.cpu;
        let dg = gpu[0]
            + gpu[1] * t.cpu
            + gpu[2] * g
            + gpu[5] * u.cpu_percent
            + gpu[7] * lg
            + gpu[15] * u.fan_percent * g;
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
        let data = session(&expected, 0.0);
        let report = identify(&data, false).unwrap();
        assert!(report.accepted, "{report:?}");
        check_coefficients(&report.model, &expected);
        eprintln!("coupled={coupled} {scores:?}", scores = report.sindy);
    }
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
        let stage = thermal_berry_core::calibration::stage(k * 2);
        Inputs {
            cpu_percent: f64::from(stage.cpu_percent),
            gpu_percent: Some(f64::from(stage.gpu_percent)),
            fan_percent: f64::from(stage.fan_percent),
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
#[ignore = "timing benchmark, run with --release --ignored --nocapture"]
fn timing() {
    use std::{hint::black_box, time::Instant};
    let model = truth(true);
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
    };
    let start = Instant::now();
    for _ in 0..10000 {
        black_box(model.forecast(black_box(t), black_box(u), 300.0).unwrap());
    }
    let forecast = start.elapsed() / 10000;
    eprintln!("median of 15 fits including validation {fit:?}, 5-minute forecast {forecast:?}, serialized model {} bytes",serde_json::to_vec(&report.model).unwrap().len());
}

#[test]
fn unannounced_fast_load_steps_can_fail_the_operational_forecast_gate() {
    let fast = session_with_periods(&truth(true), 0.2, [37, 53, 31]);
    let report = identify(&fast, false).unwrap();
    eprintln!("fast steps: CPU RMSE 60s {}", report.sindy.cpu.rmse_60s);
    assert!(!report.accepted);
    assert!(report.sindy.cpu.rmse_60s >= 3.0);
}
