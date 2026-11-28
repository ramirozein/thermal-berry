use serde::{ser::SerializeStruct, Serialize, Serializer};

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
pub const FEATURE_COUNT: usize = TERMS.len() + 4;
pub type Coefficients = [f64; FEATURE_COUNT];

pub(crate) fn term_names(dual: bool) -> Vec<&'static str> {
    if !dual {
        return TERMS.to_vec();
    }
    let mut names = TERMS[..13].to_vec();
    names.extend([
        "F1", "F1*T_cpu", "F1*T_gpu", "F1^2", "F2", "F2*T_cpu", "F2*T_gpu", "F2^2",
    ]);
    names
}

pub(crate) fn candidate_terms(joint: bool, dual: bool) -> Vec<usize> {
    let end = if dual { FEATURE_COUNT } else { TERMS.len() };
    (0..end)
        .filter(|i| joint || matches!(i, 0 | 1 | 3 | 5 | 6 | 9 | 13 | 14 | 16 | 17 | 18 | 20))
        .collect()
}

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
    /// Boost for the second channel in ThermalModel::fan_ids, if present.
    pub second_fan_percent: Option<f64>,
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
        0.0,
        0.0,
        0.0,
        0.0,
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
    if let Some(f2) = u.second_fan_percent {
        terms[17] = f2;
        terms[18] = f2 * c;
        terms[19] = t.gpu.map_or(0.0, |g| f2 * g);
        terms[20] = f2 * f2;
    }
    terms
}

#[derive(Debug, Clone)]
pub struct ThermalModel {
    pub cpu: Coefficients,
    pub gpu: Option<Coefficients>,
    /// Driver IDs in model input order. Legacy single-fan captures have no ID.
    pub fan_ids: Vec<String>,
}

impl Serialize for ThermalModel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let n = if self.fan_ids.len() == 2 {
            FEATURE_COUNT
        } else {
            TERMS.len()
        };
        let mut model = serializer
            .serialize_struct("ThermalModel", if self.fan_ids.is_empty() { 2 } else { 3 })?;
        model.serialize_field("cpu", &self.cpu[..n])?;
        model.serialize_field("gpu", &self.gpu.as_ref().map(|c| &c[..n]))?;
        if !self.fan_ids.is_empty() {
            model.serialize_field("fan_ids", &self.fan_ids)?;
        }
        model.end()
    }
}

impl ThermalModel {
    pub(crate) fn step(&self, t: Temperature, u: Inputs, dt: f64) -> Option<Temperature> {
        if (self.fan_ids.len() == 2) != u.second_fan_percent.is_some()
            || (self.gpu.is_some() && (t.gpu.is_none() || u.gpu_percent.is_none()))
        {
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
