//! Identification on the pulse grid (design D4).
//!
//! One cold shot plays independent band-limited random on every drive at once.
//! The shot is a run of realizations, each one period of `record_len` samples
//! played twice: the first period lets the structure settle, the second is
//! measured. Every realization has a flat spectrum on the band's lines with an
//! independent random phase per line and per drive, so the measured period's
//! transform is exactly the plant's response on the pulse's own lines (no
//! window, no leakage), and the path delays are in `H`. The estimator is the
//! MIMO H1 (`H = G_yx G_xx^+`) with `nfft = record_len`, the multiple coherence
//! of each output on all drives, and the condition number per line.

use hertzforge_mimo_shock_law_math::spectrum::{irfft, rfft};
use hertzforge_sdm::linalg::{condition_numbers, h1_frf, multiple_coherence, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// A small deterministic generator (splitmix64), so an identification shot is
/// the same every run with the same seed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform on `[0, 1)`.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The lines a band covers on the grid: `lo <= k df <= hi`, never line 0 or the
/// Nyquist line.
pub fn band_lines(fs_hz: f64, record_len: usize, lo_hz: f64, hi_hz: f64) -> Vec<bool> {
    let lines = record_len / 2 + 1;
    let df = fs_hz / record_len as f64;
    (0..lines)
        .map(|k| {
            let f = k as f64 * df;
            k > 0 && k < lines - 1 && f >= lo_hz && f <= hi_hz
        })
        .collect()
}

/// The identification drive: `n_d` rows of `2 * realizations * record_len`
/// samples, each row's every realization at `rms[d]` volts rms and its peak at
/// most `peak_cap[d]` volts.
pub fn excitation(
    record_len: usize,
    band: &[bool],
    rms: &[f64],
    peak_cap: &[f64],
    realizations: usize,
    seed: u64,
) -> Result<Vec<Vec<f64>>, String> {
    let mut rng = Rng::new(seed);
    let n_d = rms.len();
    let mut rows = vec![Vec::with_capacity(2 * realizations * record_len); n_d];
    for _ in 0..realizations {
        for (d, row) in rows.iter_mut().enumerate() {
            let spec: Vec<C64> = band
                .iter()
                .map(|&on| {
                    if on {
                        C64::from_polar(1.0, std::f64::consts::TAU * rng.uniform())
                    } else {
                        C64::new(0.0, 0.0)
                    }
                })
                .collect();
            let mut x = irfft(&spec, record_len)?;
            let r = (x.iter().map(|v| v * v).sum::<f64>() / record_len as f64).sqrt();
            if r <= 0.0 {
                return Err("the identification band holds no line of the pulse grid".into());
            }
            let pk = x.iter().fold(0.0f64, |a, v| a.max(v.abs())) / r;
            let scale = (rms[d] / r).min(peak_cap[d] / (pk * r));
            x.iter_mut().for_each(|v| *v *= scale);
            row.extend_from_slice(&x);
            row.extend_from_slice(&x);
        }
    }
    Ok(rows)
}

/// What one identification shot gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    /// H, outputs by drives, on `record_len / 2 + 1` lines.
    pub frf: MatrixStack,
    /// Multiple coherence of each output on all drives, output after output
    /// (`outputs * lines`).
    pub coherence: Vec<f64>,
    /// Condition number of the control rows of H per line (the first
    /// `n_controls` outputs).
    pub condition: Vec<f64>,
}

/// The H1 estimate from the measured periods. `drives` are the rows the
/// excitation built; `outputs` the measured rows aligned to them (sample `j` of
/// an output row answers sample `j` of the drives), each at least as long.
pub fn estimate(
    drives: &[Vec<f64>],
    outputs: &[Vec<f64>],
    n_controls: usize,
    record_len: usize,
    realizations: usize,
) -> Result<Estimate, String> {
    let n = record_len;
    let lines = n / 2 + 1;
    let (n_d, n_o) = (drives.len(), outputs.len());
    let need = 2 * realizations * n;
    if drives.iter().any(|r| r.len() < need) || outputs.iter().any(|r| r.len() < need) {
        return Err("the identification rows are shorter than the shot".into());
    }
    let mut gxx = MatrixStack::zeros(lines, n_d, n_d);
    let mut gyx = MatrixStack::zeros(lines, n_o, n_d);
    let mut gyy = MatrixStack::zeros(lines, n_o, n_o);
    for k in 0..realizations {
        let at = (2 * k + 1) * n;
        let xs: Vec<Vec<C64>> = drives
            .iter()
            .map(|r| rfft(&r[at..at + n]))
            .collect::<Result<_, _>>()?;
        let ys: Vec<Vec<C64>> = outputs
            .iter()
            .map(|r| rfft(&r[at..at + n]))
            .collect::<Result<_, _>>()?;
        for f in 0..lines {
            let lx = gxx.line_mut(f);
            for i in 0..n_d {
                for j in 0..n_d {
                    lx[i * n_d + j] += xs[i][f] * xs[j][f].conj();
                }
            }
            let ly = gyx.line_mut(f);
            for i in 0..n_o {
                for j in 0..n_d {
                    ly[i * n_d + j] += ys[i][f] * xs[j][f].conj();
                }
            }
            let lyy = gyy.line_mut(f);
            for i in 0..n_o {
                for j in 0..n_o {
                    lyy[i * n_o + j] += ys[i][f] * ys[j][f].conj();
                }
            }
        }
    }
    let gxx = HermitianStack::from_full(&gxx).map_err(|e| e.to_string())?;
    let gyy = HermitianStack::from_full(&gyy).map_err(|e| e.to_string())?;
    let frf = h1_frf(&gyx, &gxx, Regularization::None).map_err(|e| e.to_string())?;
    // Line-major from the estimator; output-major here.
    let lm =
        multiple_coherence(&gyy, &gyx, &gxx, Regularization::None).map_err(|e| e.to_string())?;
    let mut coherence = vec![0.0; n_o * lines];
    for f in 0..lines {
        for o in 0..n_o {
            coherence[o * lines + f] = lm[f * n_o + o];
        }
    }
    let mut ctl = MatrixStack::zeros(lines, n_controls, n_d);
    for f in 0..lines {
        let src = frf.line(f);
        ctl.line_mut(f).copy_from_slice(&src[..n_controls * n_d]);
    }
    let condition = condition_numbers(&ctl).map_err(|e| e.to_string())?;
    Ok(Estimate {
        frf,
        coherence,
        condition,
    })
}

/// The rows `rows` of a stack: outputs `from..to`.
pub fn rows_of(stack: &MatrixStack, from: usize, to: usize) -> MatrixStack {
    let (lines, _, cols) = stack.dims();
    let mut out = MatrixStack::zeros(lines, to - from, cols);
    for f in 0..lines {
        out.line_mut(f)
            .copy_from_slice(&stack.line(f)[from * cols..to * cols]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plant of known FRF (a delay and a gain per path), played periodically:
    /// the estimate returns it on every excited line, coherence one.
    #[test]
    fn mimo_shock_engine_identification_recovers_a_known_plant_on_the_grid() {
        let n = 256;
        let band = band_lines(1024.0, n, 10.0, 400.0);
        let r = 12;
        let drives = excitation(n, &band, &[0.1, 0.1], &[1.0, 1.0], r, 7).unwrap();
        // y0 = 2 x0(t - 3) + 0.5 x1, y1 = -x0 + x1(t - 1), y2 = x1 (a limit).
        let shift = |x: &[f64], k: usize| -> Vec<f64> {
            let mut out = vec![0.0; x.len()];
            for (i, o) in out.iter_mut().enumerate() {
                if i >= k {
                    *o = x[i - k];
                }
            }
            out
        };
        let (x0, x1) = (&drives[0], &drives[1]);
        let s0 = shift(x0, 3);
        let s1 = shift(x1, 1);
        let y0: Vec<f64> = s0.iter().zip(x1).map(|(a, b)| 2.0 * a + 0.5 * b).collect();
        let y1: Vec<f64> = x0.iter().zip(&s1).map(|(a, b)| -a + b).collect();
        let e = estimate(&drives, &[y0, y1, x1.clone()], 2, n, r).unwrap();
        let lines = n / 2 + 1;
        let df = 1024.0 / n as f64;
        for f in (0..lines).filter(|&f| band[f]) {
            let w = std::f64::consts::TAU * f as f64 * df / 1024.0;
            let want00 = C64::from_polar(2.0, -w * 3.0);
            let want11 = C64::from_polar(1.0, -w);
            assert!((e.frf.get(f, 0, 0) - want00).norm() < 1e-9);
            assert!((e.frf.get(f, 0, 1) - C64::new(0.5, 0.0)).norm() < 1e-9);
            assert!((e.frf.get(f, 1, 0) - C64::new(-1.0, 0.0)).norm() < 1e-9);
            assert!((e.frf.get(f, 1, 1) - want11).norm() < 1e-9);
            assert!((e.frf.get(f, 2, 1) - C64::new(1.0, 0.0)).norm() < 1e-9);
            assert!(e.coherence[f] > 0.999_999);
        }
        assert_eq!(e.condition.len(), lines);
    }

    #[test]
    fn mimo_shock_engine_identification_drive_keeps_its_rms_and_peak_cap() {
        let n = 512;
        let band = band_lines(2048.0, n, 5.0, 900.0);
        let rows = excitation(n, &band, &[0.2, 0.05], &[0.5, 10.0], 3, 1).unwrap();
        for (d, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), 2 * 3 * n);
            let pk = row.iter().fold(0.0f64, |a, v| a.max(v.abs()));
            let cap = [0.5, 10.0][d];
            assert!(pk <= cap * (1.0 + 1e-12), "drive {d} peak {pk}");
            // The two periods of a realization are equal.
            assert_eq!(&row[..n], &row[n..2 * n]);
        }
        // Drive 1 is not capped: its rms is the one asked.
        let r1 = (rows[1][..n].iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        assert!((r1 - 0.05).abs() < 1e-12);
    }
}
