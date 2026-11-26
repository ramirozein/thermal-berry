//! Offline identification. Threshold selection sees only earlier folds. The
//! final contiguous holdout is used once, including for the baseline models.
use crate::config::FanMode;
use crate::forecast::{ForecastInterval, ResidualBand};
use crate::linalg::Regression;
use crate::model::{
    features, Coefficients, Inputs, Temperature, ThermalModel, CPU_TERMS, JOINT_TERMS, TERMS,
};
use crate::observation::Observation;
use serde::Serialize;

pub const SESSION_SECS: u64 = 45 * 60;
pub const MAX_SAMPLES: usize = 20_000;
type Fit = Regression<{ TERMS.len() }>;

#[derive(Debug, Clone, Copy)]
struct Frame {
    ms: u64,
    t: Temperature,
    u: Inputs,
}

#[derive(Debug, Clone, Serialize)]
pub struct Errors {
    pub rmse_1step: f64,
    pub rmse_30s: Option<f64>,
    pub rmse_300s: Option<f64>,
    pub rmse_60s: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scores {
    pub cpu: Errors,
    pub gpu: Option<Errors>,
    pub origins_1step: usize,
    pub origins_60s: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub scope: &'static str,
    pub samples_used: usize,
    pub holdout_samples: usize,
    pub threshold_fraction: f64,
    pub ticks_per_equation: usize,
    pub terms: &'static [&'static str],
    pub model: ThermalModel,
    pub sindy: Scores,
    pub persistence: Scores,
    pub arx: Option<Scores>,
    pub rc: Option<Scores>,
    pub rls: Option<Scores>,
    pub rls_half_life_s: f64,
    pub residual_intervals_90: Vec<ForecastInterval>,
    pub monotone: bool,
    pub accepted: bool,
}

fn frames(rows: &[Observation], cpu_only: bool) -> Result<Vec<Frame>, String> {
    if rows.len() < 2 || rows.len() > MAX_SAMPLES {
        return Err("expected a bounded 45-minute calibration session".into());
    }
    let joint = !cpu_only
        && rows
            .iter()
            .any(|r| r.gpu_percent.is_some() && r.gpu_temp_c.is_some());
    let mut frames = Vec::with_capacity(rows.len());
    let mut previous_ms = None;
    for r in rows {
        if r.mode != Some(FanMode::Manual) {
            return Err("identification requires open-loop manual calibration".into());
        }
        if r.timestamp_ms > i64::MAX as u64 {
            return Err("timestamp outside SQLite range".into());
        }
        if previous_ms.is_some_and(|t| t >= r.timestamp_ms) {
            return Err("timestamps must increase strictly".into());
        }
        previous_ms = Some(r.timestamp_ms);
        let percentages = [
            r.fan_percent,
            r.cpu_percent,
            r.gpu_percent,
            r.mem_percent,
            r.gpu_mem_percent,
        ];
        if percentages
            .into_iter()
            .flatten()
            .any(|v| !(0.0..=100.0).contains(&v))
            || [r.cpu_temp_c, r.gpu_temp_c]
                .into_iter()
                .flatten()
                .any(|v| !(0.0..90.0).contains(&v))
            || r.gpu_power_w.is_some_and(|p| !p.is_finite() || p < 0.0)
            || r.rpm == Some(0)
            || r.fan_percent.is_some_and(|f| f < 30.0)
        {
            return Err("session contains an invalid, stopped-fan or unsafe reading".into());
        }
        let (Some(cpu), Some(cpu_percent), Some(fan_percent), Some(_)) =
            (r.cpu_temp_c, r.cpu_percent, r.fan_percent, r.rpm)
        else {
            continue;
        };
        if joint && (r.gpu_temp_c.is_none() || r.gpu_percent.is_none()) {
            continue;
        }
        frames.push(Frame {
            ms: r.timestamp_ms,
            t: Temperature {
                cpu,
                gpu: r.gpu_temp_c.filter(|_| joint),
            },
            u: Inputs {
                cpu_percent,
                gpu_percent: r.gpu_percent.filter(|_| joint),
                fan_percent,
            },
        });
    }
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else {
        return Err("no complete readings for the selected dies and shared fan".into());
    };
    if last.ms - first.ms < SESSION_SECS * 1000 - 4000 {
        return Err("45 minutes of complete calibration coverage required".into());
    }
    if frames
        .windows(2)
        .any(|w| !(1000..=4000).contains(&(w[1].ms - w[0].ms)))
        || first.ms - rows[0].timestamp_ms > 4000
        || rows.last().unwrap().timestamp_ms - last.ms > 4000
    {
        return Err("invalid sampling interval or measurement gap exceeding two ticks".into());
    }
    check_excitation(&frames)?;
    Ok(frames)
}

pub fn validate_session(rows: &[Observation], cpu_only: bool) -> Result<(), String> {
    frames(rows, cpu_only).map(|_| ())
}

fn check_excitation(frames: &[Frame]) -> Result<(), String> {
    let mut fan_levels = [0usize; 101];
    let mut cpu_levels = [0usize; 3];
    let mut gpu_levels = [0usize; 3];
    let mut crossed = [0usize; 2];
    let band = |p: f64| {
        if p < 30.0 {
            0
        } else if p < 70.0 {
            1
        } else {
            2
        }
    };
    let joint = frames[0].t.gpu.is_some();
    for frame in frames {
        let u = frame.u;
        let cpu = band(u.cpu_percent);
        cpu_levels[cpu] += 1;
        fan_levels[u.fan_percent.round() as usize] += 1;
        if let Some(gpu) = u.gpu_percent.map(band) {
            gpu_levels[gpu] += 1;
            if cpu == 0 && gpu == 2 {
                crossed[0] += 1;
            }
            if cpu == 2 && gpu == 0 {
                crossed[1] += 1;
            }
        }
    }
    let mut levels = 0;
    let mut next = 0;
    for (p, &count) in fan_levels.iter().enumerate() {
        if p >= next && count >= 3 {
            levels += 1;
            next = p + 5;
        }
    }
    if levels < 4
        || cpu_levels.iter().any(|n| *n < 3)
        || (joint && gpu_levels.iter().chain(&crossed).any(|n| *n < 3))
    {
        return Err("insufficient independent excitation: four fan levels, three levels per load and both crossed loads required".into());
    }
    Ok(())
}

fn regression(frames: &[Frame], ticks: usize, half_life_s: Option<f64>) -> Fit {
    // Sum the raw Euler equations over a block before least squares. This is
    // exact for the discrete model even across load changes. It reduces the
    // influence of temperature quantization without filtering the readings.
    Fit::from_rows(frames.windows(ticks + 1).map(|block| {
        let mut sum = [0.0; TERMS.len()];
        let mut rates = [0.0; 2];
        let duration = (block[ticks].ms - block[0].ms) as f64 / 1000.0;
        for w in block.windows(2) {
            let (a, b) = (w[0], w[1]);
            let dt = (b.ms - a.ms) as f64 / 1000.0;
            let weight = dt / duration;
            for (total, value) in sum.iter_mut().zip(features(a.t, a.u)) {
                *total += weight * value;
            }
            rates[0] += weight * (b.t.cpu - a.t.cpu) / dt;
            if let Some((after, before)) = b.t.gpu.zip(a.t.gpu) {
                rates[1] += weight * (after - before) / dt;
            }
        }
        let dt = (block[ticks].ms - block[ticks - 1].ms) as f64 / 1000.0;
        (
            sum,
            rates,
            half_life_s.map_or(1.0, |half| (-dt / half).exp2()),
        )
    }))
}

fn sparse(fit: &Fit, terms: &[usize], die: usize, fraction: f64) -> Option<Coefficients> {
    let mut active = terms.to_vec();
    loop {
        let (c, effects) = fit.solve(&active, die, false)?;
        let threshold = fraction * fit.rate_rms[die];
        let Some(&weakest) = active
            .iter()
            .min_by(|&&a, &&b| effects[a].total_cmp(&effects[b]))
        else {
            return Some(c);
        };
        if effects[weakest] > threshold {
            return Some(c);
        }
        // In a narrow operating range x and x² can explain the same change.
        // If both can be removed within the residual threshold, try the
        // lower-degree explanation first. An identifiable square stays eligible.
        let square = match weakest {
            1 => Some(3),
            2 => Some(4),
            5 => Some(6),
            7 => Some(8),
            13 => Some(16),
            _ => None,
        };
        let remove = square
            .filter(|j| active.contains(j) && effects[*j] <= threshold)
            .unwrap_or(weakest);
        active.retain(|&term| term != remove);
    }
}

fn fit_model(fit: &Fit, joint: bool, fraction: f64) -> Option<ThermalModel> {
    let terms = if joint { JOINT_TERMS } else { CPU_TERMS };
    Some(ThermalModel {
        cpu: sparse(fit, terms, 0, fraction)?,
        gpu: if joint {
            Some(sparse(fit, terms, 1, fraction)?)
        } else {
            None
        },
    })
}

// During identification the held-out calibration inputs are known. Replay
// them to measure the dynamics separately from unpredictable changes of load.
// Operational forecasts in the final report use only the inputs at their origin.
fn replay_error(model: &ThermalModel, frames: &[Frame]) -> Option<f64> {
    let mut sum = [0.0; 2];
    let mut count = 0;
    for origin in (0..frames.len() - 1).step_by(5) {
        let end = frames[origin].ms + 60_000;
        if end > frames.last()?.ms {
            break;
        }
        let mut predicted = frames[origin].t;
        for w in frames[origin..].windows(2) {
            let (a, b) = (w[0], w[1]);
            predicted = model.step(predicted, a.u, (b.ms - a.ms) as f64 / 1000.0)?;
            if b.ms >= end {
                sum[0] += (predicted.cpu - b.t.cpu).powi(2);
                if let Some((p, t)) = predicted.gpu.zip(b.t.gpu) {
                    sum[1] += (p - t).powi(2);
                }
                count += 1;
                break;
            }
        }
    }
    (count > 0).then(|| (sum[0] / count as f64).sqrt() + (sum[1] / count as f64).sqrt())
}

fn visit_residuals(
    model: &ThermalModel,
    frames: &[Frame],
    stride: usize,
    horizon_ms: Option<u64>,
    mut observe: impl FnMut(f64, Option<f64>),
) -> Option<usize> {
    let mut count = 0;
    let mut end = 1;
    for i in (0..frames.len() - 1).step_by(stride) {
        let origin = frames[i];
        let target_ms = horizon_ms.map_or(frames[i + 1].ms, |h| origin.ms + h);
        while end < frames.len() && frames[end].ms < target_ms {
            end += 1;
        }
        if end == frames.len() {
            break;
        }
        let (a, b) = (frames[end - 1], frames[end]);
        // Interpolate the held-out observation only, to score the exact horizon
        // despite sampling jitter. Forecast inputs are frozen at the origin.
        let weight = (target_ms - a.ms) as f64 / (b.ms - a.ms) as f64;
        let actual = |a: f64, b: f64| a + (b - a) * weight;
        let dt = (target_ms - origin.ms) as f64 / 1000.0;
        let prediction = model.forecast(origin.t, origin.u, dt)?;
        let gpu = prediction
            .gpu
            .zip(a.t.gpu.zip(b.t.gpu))
            .map(|(p, (lo, hi))| p - actual(lo, hi));
        observe(prediction.cpu - actual(a.t.cpu, b.t.cpu), gpu);
        count += 1;
    }
    (count > 0).then_some(count)
}

fn horizon_error(
    model: &ThermalModel,
    frames: &[Frame],
    stride: usize,
    horizon_ms: Option<u64>,
) -> Option<([f64; 2], usize)> {
    let mut sum = [0.0; 2];
    let count = visit_residuals(model, frames, stride, horizon_ms, |cpu, gpu| {
        sum[0] += cpu * cpu;
        if let Some(g) = gpu {
            sum[1] += g * g;
        }
    })?;
    Some((sum.map(|s| (s / count as f64).sqrt()), count))
}

fn residual_interval(
    horizon_s: u64,
    earlier: &ThermalModel,
    calibration: &[Frame],
    final_model: &ThermalModel,
    holdout: &[Frame],
) -> Option<ForecastInterval> {
    let horizon = Some(horizon_s * 1000);
    let mut cpu = Vec::new();
    let mut gpu = Vec::new();
    let calibration_origins = visit_residuals(earlier, calibration, 1, horizon, |c, g| {
        cpu.push(c.abs());
        if let Some(g) = g {
            gpu.push(g.abs());
        }
    })?;
    let radius = |mut errors: Vec<f64>| {
        let rank = (9 * errors.len()).div_ceil(10) - 1;
        *errors.select_nth_unstable_by(rank, f64::total_cmp).1
    };
    let cpu_radius = radius(cpu);
    let gpu_radius = (!gpu.is_empty()).then(|| radius(gpu));
    let mut covered = [0usize; 2];
    let holdout_origins = visit_residuals(final_model, holdout, 1, horizon, |c, g| {
        covered[0] += usize::from(c.abs() <= cpu_radius);
        covered[1] += usize::from(g.zip(gpu_radius).is_some_and(|(v, r)| v.abs() <= r));
    })?;
    let band = |radius_c, count| ResidualBand {
        radius_c,
        holdout_coverage: count as f64 / holdout_origins as f64,
    };
    Some(ForecastInterval {
        horizon_s,
        calibration_origins,
        holdout_origins,
        cpu: band(cpu_radius, covered[0]),
        gpu: gpu_radius.map(|r| band(r, covered[1])),
    })
}

fn score(model: &ThermalModel, frames: &[Frame]) -> Option<Scores> {
    let (one, origins_1step) = horizon_error(model, frames, 1, None)?;
    let (minute, origins_60s) = horizon_error(model, frames, 1, Some(60_000))?;
    let short = horizon_error(model, frames, 1, Some(30_000));
    let long = horizon_error(model, frames, 1, Some(300_000));
    let errors = |die: usize| Errors {
        rmse_1step: one[die],
        rmse_60s: minute[die],
        rmse_30s: short.map(|s| s.0[die]),
        rmse_300s: long.map(|s| s.0[die]),
    };
    Some(Scores {
        cpu: errors(0),
        gpu: model.gpu.map(|_| errors(1)),
        origins_1step,
        origins_60s,
    })
}

fn monotone(model: &ThermalModel, frames: &[Frame]) -> bool {
    // The instantaneous derivatives with respect to load and fan are affine.
    // Their extrema on the measured operating box occur at its corners.
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 5];
    for a in frames {
        for (range, x) in bounds.iter_mut().zip([
            a.t.cpu,
            a.t.gpu.unwrap_or(0.0),
            a.u.cpu_percent,
            a.u.gpu_percent.unwrap_or(0.0),
            a.u.fan_percent,
        ]) {
            range[0] = range[0].min(x);
            range[1] = range[1].max(x);
        }
    }
    for w in std::iter::once(&model.cpu).chain(model.gpu.as_ref()) {
        for mask in 0..32 {
            let [c, g, l, v, f] = std::array::from_fn(|i| bounds[i][(mask >> i) & 1]);
            let cpu = w[5] + 2.0 * w[6] * l + w[9] * c + w[11] * g;
            let gpu = w[7] + 2.0 * w[8] * v + w[10] * g + w[12] * c;
            let fan = w[13] + w[14] * c + w[15] * g + 2.0 * w[16] * f;
            if cpu < -1e-8 || gpu < -1e-8 || fan > 1e-8 {
                return false;
            }
        }
    }
    // Also check that the coupled dynamics preserve the input ordering at
    // the minute horizon from measured initial states within the load range.
    for frame in frames.iter().step_by((frames.len() / 32).max(1)) {
        let Some(reference) = model.forecast(frame.t, frame.u, 60.0) else {
            return false;
        };
        for axis in 0..3 {
            let mut u = frame.u;
            let input = match axis {
                0 => &mut u.cpu_percent,
                1 => match &mut u.gpu_percent {
                    Some(value) => value,
                    None => continue,
                },
                _ => &mut u.fan_percent,
            };
            let range = bounds[axis + 2];
            let raised = (*input + 5.0).min(range[1]);
            if raised <= *input {
                continue;
            }
            *input = raised;
            let Some(changed) = model.forecast(frame.t, u, 60.0) else {
                return false;
            };
            let ordered = |a: f64, b: f64| {
                if axis == 2 {
                    b <= a + 1e-7
                } else {
                    b >= a - 1e-7
                }
            };
            if !ordered(reference.cpu, changed.cpu)
                || reference
                    .gpu
                    .zip(changed.gpu)
                    .is_some_and(|(a, b)| !ordered(a, b))
            {
                return false;
            }
        }
    }
    true
}

fn baseline(fit: &Fit, joint: bool, rc_only: bool) -> Option<ThermalModel> {
    let arx: &[usize] = if joint {
        &[0, 1, 2, 5, 7, 13]
    } else {
        &[0, 1, 5, 13]
    };
    let cpu_terms: &[usize] = if rc_only { &[0, 1, 5, 13, 14] } else { arx };
    let gpu_terms: &[usize] = if rc_only { &[0, 2, 7, 13, 15] } else { arx };
    Some(ThermalModel {
        cpu: fit.fit(cpu_terms, 0, true)?,
        gpu: if joint {
            Some(fit.fit(gpu_terms, 1, true)?)
        } else {
            None
        },
    })
}

pub fn identify(rows: &[Observation], cpu_only: bool) -> Result<Report, String> {
    let data = frames(rows, cpu_only)?;
    let n = data.len();
    let joint = data[0].t.gpu.is_some();
    let train_end = n * 4 / 5;
    let train = &data[..train_end];
    let holdout = &data[train_end..];
    check_excitation(train)?;
    // Fractions of the training derivative RMS, not hardware-specific coefficients.
    let candidates = [0.0, 0.002, 0.006, 0.02, 0.06, 0.15, 0.3];
    let mut trials = Vec::new();
    for ticks in [1, 5, 15] {
        let mut losses = [0.0; 7];
        for (end, next) in [(n * 2 / 5, n * 3 / 5), (n * 3 / 5, train_end)] {
            let fit = regression(&data[..end], ticks, None);
            for (loss, &fraction) in losses.iter_mut().zip(&candidates) {
                let Some(model) = fit_model(&fit, joint, fraction) else {
                    *loss = f64::INFINITY;
                    continue;
                };
                let validation = &data[end..next];
                let one = horizon_error(&model, validation, 5, None);
                let minute = replay_error(&model, validation);
                *loss += match (one, minute) {
                    (Some((a, _)), Some(b)) => a[0] + a[1] + b,
                    _ => f64::INFINITY,
                };
            }
        }
        trials.extend(
            candidates
                .into_iter()
                .zip(losses)
                .map(|(fraction, loss)| (ticks, fraction, loss)),
        );
    }
    let minimum = trials
        .iter()
        .map(|trial| trial.2)
        .fold(f64::INFINITY, f64::min);
    if !minimum.is_finite() {
        return Err("rank-deficient excitation or divergent temporal validation".into());
    }
    let (ticks_per_equation, threshold_fraction, _) = trials
        .into_iter()
        .filter(|trial| trial.2 <= minimum * 1.01 + 1e-9)
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.total_cmp(&a.2)))
        .unwrap();
    let fit = regression(train, ticks_per_equation, None);
    let model =
        fit_model(&fit, joint, threshold_fraction).ok_or("rank-deficient training session")?;
    let sindy = score(&model, holdout).ok_or("model diverged on the unseen holdout")?;
    // Only out-of-fold errors prior to the final holdout determine the widths.
    let last_fold = n * 3 / 5;
    let fold_fit = regression(&data[..last_fold], ticks_per_equation, None);
    let earlier =
        fit_model(&fold_fit, joint, threshold_fraction).ok_or("rank-deficient calibration fold")?;
    let residual_intervals_90 = [30, 60, 300]
        .into_iter()
        .filter_map(|s| {
            residual_interval(s, &earlier, &data[last_fold..train_end], &model, holdout)
        })
        .collect();
    let persistence = score(
        &ThermalModel {
            cpu: [0.0; 17],
            gpu: joint.then_some([0.0; 17]),
        },
        holdout,
    )
    .unwrap();
    let arx = baseline(&fit, joint, false).and_then(|m| score(&m, holdout));
    let rc = baseline(&fit, joint, true).and_then(|m| score(&m, holdout));
    let rls_half_life_s = (train.last().unwrap().ms - train[0].ms) as f64 / 2000.0;
    let weighted = regression(train, ticks_per_equation, Some(rls_half_life_s));
    let refit = |c: &Coefficients, die| {
        let active: Vec<_> = (0..c.len()).filter(|&j| c[j] != 0.0).collect();
        weighted.fit(&active, die, false)
    };
    let rls = (|| {
        let cpu = refit(&model.cpu, 0)?;
        let gpu = if let Some(c) = &model.gpu {
            Some(refit(c, 1)?)
        } else {
            None
        };
        score(&ThermalModel { cpu, gpu }, holdout)
    })();
    let monotone = monotone(&model, &data);
    let meets = |e: &Errors| e.rmse_1step < 1.5 && e.rmse_60s < 3.0;
    let sparse_outputs = std::iter::once(&model.cpu)
        .chain(model.gpu.as_ref())
        .all(|c| {
            let active = c.iter().filter(|v| **v != 0.0).count();
            active > 0
                && active
                    < if joint {
                        JOINT_TERMS.len()
                    } else {
                        CPU_TERMS.len()
                    }
        });
    let accepted = monotone
        && sparse_outputs
        && meets(&sindy.cpu)
        && sindy.gpu.as_ref().is_none_or(meets)
        && arx.is_some()
        && rc.is_some();
    Ok(Report {
        scope: if joint { "cpu+gpu" } else { "cpu-only" },
        samples_used: train.len() - 1,
        holdout_samples: holdout.len(),
        threshold_fraction,
        ticks_per_equation,
        terms: &TERMS,
        model,
        sindy,
        persistence,
        arx,
        rc,
        rls,
        rls_half_life_s,
        residual_intervals_90,
        monotone,
        accepted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolvable_quadratic_is_retained_without_a_spurious_linear_term() {
        let fit = Fit::from_rows((0..120).map(|i| {
            let load = (i % 91) as f64;
            let mut x = [0.0; 17];
            x[0] = 1.0;
            x[5] = load;
            x[6] = load * load;
            (x, [0.02 + 0.0001 * x[6], 0.0], 1.0)
        }));
        let c = sparse(&fit, &[0, 5, 6], 0, 0.006).unwrap();
        assert!((c[0] - 0.02).abs() < 1e-11 && (c[6] - 0.0001).abs() < 1e-12);
        assert_eq!(c[5], 0.0);
    }

    #[test]
    fn rejects_wrong_load_or_cooling_direction_on_either_die() {
        let frames = [10.0, 60.0, 80.0].map(|load| Frame {
            ms: 0,
            t: Temperature {
                cpu: 50.0,
                gpu: Some(45.0),
            },
            u: Inputs {
                cpu_percent: load,
                gpu_percent: Some(load),
                fan_percent: load,
            },
        });
        let zero = ThermalModel {
            cpu: [0.0; 17],
            gpu: Some([0.0; 17]),
        };
        for die in 0..2 {
            for (term, coefficient) in [(5, -0.001), (7, -0.001), (13, 0.001)] {
                let mut bad = zero.clone();
                let coefficients = if die == 0 {
                    &mut bad.cpu
                } else {
                    bad.gpu.as_mut().unwrap()
                };
                coefficients[term] = coefficient;
                assert!(!monotone(&bad, &frames));
            }
        }

        let mut bad = zero;
        bad.cpu[5] = 0.001;
        bad.gpu.as_mut().unwrap()[1] = -0.001;
        assert!(!monotone(&bad, &frames));
    }
}
