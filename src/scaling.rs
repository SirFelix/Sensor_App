use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScalingMode {
    /// value = gain * raw + intercept
    Linear,
    /// Maps [raw_lo, raw_hi] onto [eng_lo, eng_hi], e.g. 4-20 mA -> 0-15000 psi.
    Range,
    /// Least-squares polynomial through the calibration points.
    Polynomial,
    /// Piecewise-linear interpolation between calibration points.
    Table,
}

impl ScalingMode {
    pub const ALL: [ScalingMode; 4] = [
        ScalingMode::Linear,
        ScalingMode::Range,
        ScalingMode::Polynomial,
        ScalingMode::Table,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScalingMode::Linear => "Linear (gain + intercept)",
            ScalingMode::Range => "Range map (raw span -> value span)",
            ScalingMode::Polynomial => "Polynomial fit (correlated points)",
            ScalingMode::Table => "Lookup table (piecewise linear)",
        }
    }

    pub fn uses_points(self) -> bool {
        matches!(self, ScalingMode::Polynomial | ScalingMode::Table)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalPoint {
    pub raw: f64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scaling {
    pub mode: ScalingMode,
    pub gain: f64,
    pub intercept: f64,
    pub raw_lo: f64,
    pub raw_hi: f64,
    pub eng_lo: f64,
    pub eng_hi: f64,
    pub points: Vec<CalPoint>,
    pub poly_order: usize,
    /// Fitted in normalised raw space u = (raw - center) / span for numerical stability.
    pub fit: Option<PolyFit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolyFit {
    pub coeffs: Vec<f64>,
    pub center: f64,
    pub span: f64,
    pub r_squared: f64,
    pub max_residual: f64,
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            mode: ScalingMode::Linear,
            gain: 1.0,
            intercept: 0.0,
            raw_lo: 0.0,
            raw_hi: 10.0,
            eng_lo: 0.0,
            eng_hi: 10.0,
            points: vec![
                CalPoint { raw: 0.0, value: 0.0 },
                CalPoint { raw: 5.0, value: 5.0 },
                CalPoint { raw: 10.0, value: 10.0 },
            ],
            poly_order: 2,
            fit: None,
        }
    }
}

impl Scaling {
    pub fn apply(&self, raw: f64) -> f64 {
        match self.mode {
            ScalingMode::Linear => self.gain * raw + self.intercept,
            ScalingMode::Range => {
                let span = self.raw_hi - self.raw_lo;
                if span == 0.0 {
                    return self.eng_lo;
                }
                self.eng_lo + (raw - self.raw_lo) * (self.eng_hi - self.eng_lo) / span
            }
            ScalingMode::Polynomial => match &self.fit {
                Some(fit) => fit.eval(raw),
                None => raw,
            },
            ScalingMode::Table => interpolate(&self.sorted_points(), raw),
        }
    }

    pub fn sorted_points(&self) -> Vec<CalPoint> {
        let mut pts: Vec<CalPoint> = self
            .points
            .iter()
            .copied()
            .filter(|p| p.raw.is_finite() && p.value.is_finite())
            .collect();
        pts.sort_by(|a, b| a.raw.total_cmp(&b.raw));
        pts
    }

    /// Recomputes the polynomial fit. Call after editing points or order.
    pub fn refit(&mut self) -> Result<(), String> {
        self.fit = None;
        if self.mode != ScalingMode::Polynomial {
            return Ok(());
        }
        let pts = self.sorted_points();
        let fit = fit_polynomial(&pts, self.poly_order)?;
        self.fit = Some(fit);
        Ok(())
    }

    pub fn describe(&self) -> String {
        match self.mode {
            ScalingMode::Linear => format!("y = {} * x + {}", fmt_g(self.gain), fmt_g(self.intercept)),
            ScalingMode::Range => format!(
                "{} .. {} raw  ->  {} .. {}",
                fmt_g(self.raw_lo),
                fmt_g(self.raw_hi),
                fmt_g(self.eng_lo),
                fmt_g(self.eng_hi)
            ),
            ScalingMode::Polynomial => match &self.fit {
                Some(fit) => fit.describe(),
                None => "No fit yet".into(),
            },
            ScalingMode::Table => format!("{} points, linear between, extrapolated at ends", self.sorted_points().len()),
        }
    }
}

impl PolyFit {
    pub fn eval(&self, raw: f64) -> f64 {
        let u = (raw - self.center) / self.span;
        self.coeffs.iter().rev().fold(0.0, |acc, c| acc * u + c)
    }

    /// Expands the normalised polynomial back into plain raw-space coefficients for display.
    #[allow(clippy::needless_range_loop)]
    pub fn raw_coeffs(&self) -> Vec<f64> {
        let n = self.coeffs.len();
        let mut out = vec![0.0; n];
        // sum_k c_k * ((x - m)/s)^k, expanded with the binomial theorem.
        for (k, ck) in self.coeffs.iter().enumerate() {
            let scale = ck / self.span.powi(k as i32);
            let mut binom = 1.0;
            for j in 0..=k {
                if j > 0 {
                    binom = binom * (k - j + 1) as f64 / j as f64;
                }
                out[j] += scale * binom * (-self.center).powi((k - j) as i32);
            }
        }
        out
    }

    pub fn describe(&self) -> String {
        let coeffs = self.raw_coeffs();
        let scale = coeffs.iter().fold(0.0f64, |m, c| m.max(c.abs()));
        let terms: Vec<String> = coeffs
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, c)| c.abs() > scale * 1e-12)
            .map(|(k, c)| match k {
                0 => fmt_g(*c),
                1 => format!("{}·x", fmt_g(*c)),
                _ => format!("{}·x^{k}", fmt_g(*c)),
            })
            .collect();
        let terms = if terms.is_empty() { vec!["0".to_string()] } else { terms };
        format!(
            "y = {}   (R² = {:.6}, max residual {})",
            terms.join(" + "),
            self.r_squared,
            fmt_g(self.max_residual)
        )
    }
}

pub fn fit_polynomial(points: &[CalPoint], order: usize) -> Result<PolyFit, String> {
    if points.len() < 2 {
        return Err("Need at least 2 calibration points".into());
    }
    let order = order.min(points.len() - 1).min(6);
    let n = order + 1;
    let lo = points.iter().map(|p| p.raw).fold(f64::INFINITY, f64::min);
    let hi = points.iter().map(|p| p.raw).fold(f64::NEG_INFINITY, f64::max);
    if hi <= lo {
        return Err("Calibration points need at least two different raw values".into());
    }
    let center = 0.5 * (lo + hi);
    let span = 0.5 * (hi - lo);

    // Normal equations A^T A c = A^T y with A_ij = u_i^j.
    let mut ata = vec![vec![0.0; n]; n];
    let mut aty = vec![0.0; n];
    for p in points {
        let u = (p.raw - center) / span;
        let mut pows = vec![1.0; 2 * n - 1];
        for k in 1..pows.len() {
            pows[k] = pows[k - 1] * u;
        }
        for i in 0..n {
            aty[i] += pows[i] * p.value;
            for j in 0..n {
                ata[i][j] += pows[i + j];
            }
        }
    }
    let coeffs = solve(ata, aty).ok_or("Calibration points are degenerate for this order")?;

    let fit = PolyFit { coeffs, center, span, r_squared: 1.0, max_residual: 0.0 };
    let mean = points.iter().map(|p| p.value).sum::<f64>() / points.len() as f64;
    let mut ss_res = 0.0;
    let mut ss_tot = 0.0;
    let mut max_res: f64 = 0.0;
    for p in points {
        let r = p.value - fit.eval(p.raw);
        ss_res += r * r;
        ss_tot += (p.value - mean).powi(2);
        max_res = max_res.max(r.abs());
    }
    let r_squared = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 1.0 };
    Ok(PolyFit { r_squared, max_residual: max_res, ..fit })
}

#[allow(clippy::needless_range_loop)]
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

fn interpolate(pts: &[CalPoint], raw: f64) -> f64 {
    match pts.len() {
        0 => raw,
        1 => pts[0].value,
        n => {
            let i = match pts.partition_point(|p| p.raw <= raw) {
                0 => 1,
                i if i >= n => n - 1,
                i => i,
            };
            let (a, b) = (pts[i - 1], pts[i]);
            if b.raw == a.raw {
                return a.value;
            }
            a.value + (raw - a.raw) * (b.value - a.value) / (b.raw - a.raw)
        }
    }
}

pub fn fmt_g(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-4..1e7).contains(&a) {
        format!("{v:.5e}")
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(v: &[(f64, f64)]) -> Vec<CalPoint> {
        v.iter().map(|&(raw, value)| CalPoint { raw, value }).collect()
    }

    #[test]
    fn range_maps_4_20ma_to_psi() {
        let s = Scaling {
            mode: ScalingMode::Range,
            raw_lo: 4.0,
            raw_hi: 20.0,
            eng_lo: 0.0,
            eng_hi: 15000.0,
            ..Default::default()
        };
        assert_eq!(s.apply(4.0), 0.0);
        assert_eq!(s.apply(12.0), 7500.0);
        assert_eq!(s.apply(20.0), 15000.0);
    }

    #[test]
    fn quadratic_fit_is_exact() {
        let points = pts(&[(0.0, 1.0), (1.0, 3.0), (2.0, 9.0), (3.0, 19.0), (10.0, 201.0)]);
        let fit = fit_polynomial(&points, 2).unwrap();
        for p in &points {
            assert!((fit.eval(p.raw) - p.value).abs() < 1e-8);
        }
        let raw = fit.raw_coeffs();
        assert!((raw[0] - 1.0).abs() < 1e-8);
        assert!((raw[1] - 0.0).abs() < 1e-8);
        assert!((raw[2] - 2.0).abs() < 1e-8);
        assert!((fit.r_squared - 1.0).abs() < 1e-12);
    }

    #[test]
    fn fit_order_is_capped_by_point_count() {
        let fit = fit_polynomial(&pts(&[(1.0, 2.0), (3.0, 6.0)]), 5).unwrap();
        assert_eq!(fit.coeffs.len(), 2);
        assert!((fit.eval(2.0) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn table_interpolates_and_extrapolates() {
        let s = Scaling {
            mode: ScalingMode::Table,
            points: pts(&[(10.0, 100.0), (0.0, 0.0), (5.0, 20.0)]),
            ..Default::default()
        };
        assert_eq!(s.apply(2.5), 10.0);
        assert_eq!(s.apply(7.5), 60.0);
        assert_eq!(s.apply(12.0), 132.0);
        assert_eq!(s.apply(-1.0), -4.0);
    }
}
