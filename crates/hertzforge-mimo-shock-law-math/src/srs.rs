//! The SRS pieces the laws need: the SRS of each control, the error in dB, and
//! a gain known at the SRS's natural frequencies carried to the spectral lines.

use hertzforge_dsp::srs::srs_analyze;

/// Maximax absolute-acceleration SRS of every row at `natural_hz`, each
/// divided by `scale`.
pub fn srs_rows(
    rows: &[Vec<f64>],
    fs_hz: f64,
    natural_hz: &[f64],
    q: f64,
    scale: f64,
) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|r| {
            srs_analyze(r, fs_hz, natural_hz, q)
                .into_iter()
                .map(|v| v / scale)
                .collect()
        })
        .collect()
}

/// `20 log10(measured / target)` per point; a non-positive value reads as
/// -400 dB rather than an infinity.
pub fn error_db(measured: &[f64], target: &[f64]) -> Vec<f64> {
    measured
        .iter()
        .zip(target)
        .map(|(&m, &t)| {
            if m > 0.0 && t > 0.0 {
                20.0 * (m / t).log10()
            } else {
                -400.0
            }
        })
        .collect()
}

/// The largest `abs(error_db)` over every control and frequency.
pub fn worst_error_db(measured: &[Vec<f64>], target: &[Vec<f64>]) -> f64 {
    measured
        .iter()
        .zip(target)
        .flat_map(|(m, t)| error_db(m, t))
        .fold(0.0, |a, e| a.max(e.abs()))
}

/// Values known at the ascending frequencies `at_hz`, carried to every line
/// `k * df` by linear interpolation in log frequency; lines below the first
/// frequency take the first value and lines above the last take the last. The
/// zero line is treated as 1 Hz.
pub fn to_lines_log(at_hz: &[f64], values: &[f64], lines: usize, df: f64) -> Vec<f64> {
    let la: Vec<f64> = at_hz.iter().map(|f| f.max(1e-12).ln()).collect();
    let mut out = Vec::with_capacity(lines);
    let mut j = 0usize;
    for k in 0..lines {
        let lf = (k as f64 * df).max(1.0).ln();
        if lf <= la[0] {
            out.push(values[0]);
            continue;
        }
        if lf >= la[la.len() - 1] {
            out.push(values[values.len() - 1]);
            continue;
        }
        while j + 1 < la.len() && la[j + 1] < lf {
            j += 1;
        }
        let t = (lf - la[j]) / (la[j + 1] - la[j]);
        out.push(values[j] + t * (values[j + 1] - values[j]));
    }
    out
}

/// A weight by level from a table of `(level_db, weight)` sorted by level:
/// linear in dB between rows, the end rows held beyond them.
pub fn weight_at(table: &[(f64, f64)], level_db: f64) -> f64 {
    if table.is_empty() {
        return 1.0;
    }
    if level_db <= table[0].0 {
        return table[0].1;
    }
    for w in table.windows(2) {
        if level_db <= w[1].0 {
            let t = (level_db - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + t * (w[1].1 - w[0].1);
        }
    }
    table[table.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_log_interpolation_holds_ends_and_hits_knots() {
        let at = [100.0, 200.0, 400.0];
        let v = [1.0, 2.0, 4.0];
        let out = to_lines_log(&at, &v, 600, 1.0);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[50], 1.0);
        assert!((out[200] - 2.0).abs() < 1e-12);
        // Halfway in log between 100 and 200 Hz.
        let mid = (100.0f64 * 200.0).sqrt();
        let k = mid.round() as usize;
        let t = ((k as f64).ln() - 100f64.ln()) / (2f64.ln());
        assert!((out[k] - (1.0 + t)).abs() < 1e-12);
        assert_eq!(out[599], 4.0);
    }

    #[test]
    fn mimo_shock_weight_table_interpolates_in_db() {
        let t = [(-12.0, 0.5), (0.0, 0.85)];
        assert_eq!(weight_at(&t, -20.0), 0.5);
        assert!((weight_at(&t, -6.0) - 0.675).abs() < 1e-12);
        assert_eq!(weight_at(&t, 3.0), 0.85);
    }
}
