//! Compress tall least-squares problems in blocks. Only the triangular factor
//! and transformed right-hand sides are needed to refit any subset of columns.
const MAX: usize = 24;
type Matrix = [[f64; MAX]; MAX];

fn householder(a: &mut Matrix, rows: usize, columns: usize, width: usize) {
    for k in 0..columns.min(rows) {
        let norm = (k..rows).map(|i| a[i][k] * a[i][k]).sum::<f64>().sqrt();
        if norm == 0.0 {
            continue;
        }
        let diagonal = -norm.copysign(a[k][k]);
        let mut v = [0.0; MAX];
        v[k] = a[k][k] - diagonal;
        for i in k + 1..rows {
            v[i] = a[i][k];
        }
        let scale = 1.0 / (-diagonal * v[k]);
        for j in k + 1..width {
            let dot = (k..rows).map(|i| v[i] * a[i][j]).sum::<f64>() * scale;
            for i in k..rows {
                a[i][j] -= dot * v[i];
            }
        }
        a[k][k] = diagonal;
        for row in &mut a[k + 1..rows] {
            row[k] = 0.0;
        }
    }
}

fn backsolve(a: &Matrix, n: usize, mut y: [f64; MAX]) -> Option<[f64; MAX]> {
    for i in (0..n).rev() {
        if a[i][i].abs() < 1e-9 {
            return None;
        }
        y[i] = (y[i] - (i + 1..n).map(|j| a[i][j] * y[j]).sum::<f64>()) / a[i][i];
    }
    Some(y)
}

// Cholesky on the normalized Gram matrix for the small baseline regressions.
fn cholesky_solve(a: &Matrix, rows: usize, n: usize) -> Option<[f64; MAX]> {
    let mut lower = [[0.0; MAX]; MAX];
    let mut y = [0.0; MAX];
    for i in 0..n {
        for j in 0..=i {
            let gram: f64 = a[..rows].iter().map(|row| row[i] * row[j]).sum();
            let v = gram - (0..j).map(|k| lower[i][k] * lower[j][k]).sum::<f64>();
            lower[i][j] = if i == j {
                if v <= 1e-14 {
                    return None;
                }
                v.sqrt()
            } else {
                v / lower[j][j]
            };
        }
        let rhs: f64 = a[..rows].iter().map(|row| row[i] * row[n]).sum();
        y[i] = (rhs - (0..i).map(|j| lower[i][j] * y[j]).sum::<f64>()) / lower[i][i];
    }
    for i in (0..n).rev() {
        y[i] = (y[i] - (i + 1..n).map(|j| lower[j][i] * y[j]).sum::<f64>()) / lower[i][i];
    }
    Some(y)
}

pub(crate) struct Regression<const P: usize> {
    a: Matrix,
    rows: usize,
    weight: f64,
    norms: [f64; P],
    pub rate_rms: [f64; 2],
}

impl<const P: usize> Regression<P> {
    pub fn from_rows(data: impl Iterator<Item = ([f64; P], [f64; 2], f64)>) -> Self {
        assert!(P + 2 <= MAX);
        let mut result = Self {
            a: [[0.0; MAX]; MAX],
            rows: 0,
            weight: 0.0,
            norms: [0.0; P],
            rate_rms: [0.0; 2],
        };
        for (features, rates, retention) in data {
            if retention < 1.0 {
                // Square-root recursive least squares: discount the previous
                // information factor and right-hand sides before the QR update.
                let scale = retention.sqrt();
                for row in &mut result.a[..result.rows] {
                    for value in &mut row[..P + 2] {
                        *value *= scale;
                    }
                }
                for sum in result.norms.iter_mut().chain(&mut result.rate_rms) {
                    *sum *= retention;
                }
            }
            result.weight = retention * result.weight + 1.0;
            result.a[result.rows][..P].copy_from_slice(&features);
            result.a[result.rows][P..P + 2].copy_from_slice(&rates);
            for (sum, x) in result.norms.iter_mut().zip(features) {
                *sum += x * x;
            }
            for (sum, x) in result.rate_rms.iter_mut().zip(rates) {
                *sum += x * x;
            }
            result.rows += 1;
            if result.rows == MAX {
                householder(&mut result.a, MAX, P, P + 2);
                result.rows = P;
            }
        }
        householder(&mut result.a, result.rows, P, P + 2);
        result.rows = result.rows.min(P);
        for v in &mut result.norms {
            *v = v.sqrt();
        }
        for v in &mut result.rate_rms {
            *v = (*v / result.weight.max(1.0)).sqrt();
        }
        result
    }

    pub fn fit(&self, terms: &[usize], die: usize, baseline: bool) -> Option<[f64; P]> {
        self.solve(terms, die, baseline)
            .map(|(coefficients, _)| coefficients)
    }

    pub fn solve(
        &self,
        terms: &[usize],
        die: usize,
        baseline: bool,
    ) -> Option<([f64; P], [f64; P])> {
        let n = terms.len();
        if n > self.rows || terms.iter().any(|&j| self.norms[j] == 0.0) {
            return None;
        }
        let mut a = [[0.0; MAX]; MAX];
        for (row, src) in a.iter_mut().zip(&self.a).take(self.rows) {
            for (k, &j) in terms.iter().enumerate() {
                row[k] = src[j] / self.norms[j];
            }
            row[n] = src[P + die];
        }
        let normalized = if baseline {
            cholesky_solve(&a, self.rows, n)?
        } else {
            householder(&mut a, self.rows, n, n + 1);
            backsolve(&a, n, std::array::from_fn(|i| a[i][n]))?
        };
        let mut effects = [0.0; P];
        let mut coefficients = [0.0; P];
        for (k, &j) in terms.iter().enumerate() {
            coefficients[j] = normalized[k] / self.norms[j];
            if !baseline {
                // Rᵀv = e_k. The increase in RMS residual on removing this
                // column discounts the part already explained by other terms.
                let mut v = [0.0; MAX];
                for i in k..n {
                    let rhs = if i == k { 1.0 } else { 0.0 };
                    v[i] = (rhs - (k..i).map(|r| a[r][i] * v[r]).sum::<f64>()) / a[i][i];
                }
                let variance = v[k..n].iter().map(|x| x * x).sum::<f64>();
                effects[j] = normalized[k].abs() / (self.weight * variance).sqrt();
            }
        }
        coefficients
            .iter()
            .all(|c| c.is_finite())
            .then_some((coefficients, effects))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_solvers_recover_two_known_solutions_across_blocks() {
        let fit = Regression::from_rows((0..85).map(|x| {
            let t = x as f64 / 20.0;
            ([1.0, t, t * t], [2.0 - t + 0.5 * t * t, 7.0 * t], 1.0)
        }));
        for baseline in [false, true] {
            for (die, expected) in [[2.0, -1.0, 0.5], [0.0, 7.0, 0.0]].iter().enumerate() {
                let c = fit.fit(&[0, 1, 2], die, baseline).unwrap();
                for (got, want) in c.iter().zip(expected) {
                    assert!((got - want).abs() < 1e-10);
                }
            }
        }
        let singular = Regression::from_rows((0..64).map(|_| ([1.0, 2.0], [3.0, 4.0], 1.0)));
        assert!(singular.fit(&[0, 1], 0, false).is_none());
        assert!(singular.fit(&[0, 1], 0, true).is_none());
    }

    #[test]
    fn forgetting_matches_the_exponentially_weighted_mean_across_blocks() {
        let mut sum = 0.0;
        let mut weight = 0.0;
        let fit = Regression::from_rows((0..80).map(|i| {
            let y = f64::from(i > 39);
            sum = sum * 0.95 + y;
            weight = weight * 0.95 + 1.0;
            ([1.0], [y, 0.0], 0.95)
        }));
        assert!((fit.fit(&[0], 0, false).unwrap()[0] - sum / weight).abs() < 1e-10);
    }
}
