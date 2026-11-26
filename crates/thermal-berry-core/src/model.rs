use serde::Serialize;

pub const TERMS: [&str; 17] = [
    "1",
    "T_cpu",
    "T_gpu",
    "T_cpu^2",
    "T_gpu^2",
    "L_cpu",
    "L_cpu^2",
    "L_gpu",
    "L_gpu^2",
    "L_cpu*T_cpu",
    "L_gpu*T_gpu",
    "L_cpu*T_gpu",
    "L_gpu*T_cpu",
    "F",
    "F*T_cpu",
    "F*T_gpu",
    "F^2",
];
pub(crate) const CPU_TERMS: &[usize] = &[0, 1, 3, 5, 6, 9, 13, 14, 16];
pub(crate) const JOINT_TERMS: &[usize] =
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
pub type Coefficients = [f64; TERMS.len()];

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Temperature {
    pub cpu: f64,
    pub gpu: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub cpu_percent: f64,
    pub gpu_percent: Option<f64>,
    pub fan_percent: f64,
}

pub(crate) fn features(t: Temperature, u: Inputs) -> Coefficients {
    let (c, l, f) = (t.cpu, u.cpu_percent, u.fan_percent);
    let mut terms = [
        1.0,
        c,
        0.0,
        c * c,
        0.0,
        l,
        l * l,
        0.0,
        0.0,
        l * c,
        0.0,
        0.0,
        0.0,
        f,
        f * c,
        0.0,
        f * f,
    ];
    // GPU columns are absent from a CPU-only regression.
    if let (Some(g), Some(v)) = (t.gpu, u.gpu_percent) {
        terms[2] = g;
        terms[4] = g * g;
        terms[7] = v;
        terms[8] = v * v;
        terms[10] = v * g;
        terms[11] = l * g;
        terms[12] = v * c;
        terms[15] = f * g;
    }
    terms
}

#[derive(Debug, Clone, Serialize)]
pub struct ThermalModel {
    pub cpu: Coefficients,
    pub gpu: Option<Coefficients>,
}

impl ThermalModel {
    pub(crate) fn step(&self, t: Temperature, u: Inputs, dt: f64) -> Option<Temperature> {
        if self.gpu.is_some() && (t.gpu.is_none() || u.gpu_percent.is_none()) {
            return None;
        }
        let basis = features(t, u);
        let rate = |c: &Coefficients| c.iter().zip(basis).map(|(a, b)| a * b).sum::<f64>();
        let cpu = t.cpu + dt * rate(&self.cpu);
        let gpu = self.gpu.as_ref().zip(t.gpu).map(|(c, g)| g + dt * rate(c));
        let valid = |x: f64| (0.0..=150.0).contains(&x);
        (valid(cpu) && gpu.is_none_or(valid)).then_some(Temperature { cpu, gpu })
    }
}
