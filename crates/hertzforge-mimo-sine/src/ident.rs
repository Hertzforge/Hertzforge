//! Identification before the run: independent random on every drive at low
//! level, the response over the drive at every control and limit channel.
//!
//! The excitation and the estimator are MIMO Shock's, ported: a run of
//! realizations, each one period of `record_len` samples played twice (the
//! first period lets the structure settle, the second is measured), every
//! realization flat on the band's lines with an independent random phase per
//! line and per drive. The measured period's transform is then exactly the
//! structure's response on the grid's lines (no window, no leakage), and the
//! estimator is the MIMO H1 (`H = G_yx G_xx^+`) with the multiple coherence of
//! each response on all drives.
//!
//! The drive-to-response lag is measured from the same records (the peak of
//! the summed cross-correlations) and taken out of the response, so the
//! identification is the response over the drive that produced it; the run's
//! trackers take the same lag out of their reference phase, so a linear
//! structure in steady state gives `C = H D`.
//!
//! The level the identification was taken at is stated by response: the
//! largest measured peak at a control over the largest peak of the full-level
//! reference. Independent drives excite modes an in-phase reference barely
//! moves, so a level stated by drive would mislead.

use crate::law::LawIdentification;
use hertzforge_dsp::fft;
use hertzforge_sdm::linalg::{h1_frf, multiple_coherence, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// The finest line spacing an identification aims for, Hz: a lightly damped
/// mode (Q 50 at 40 Hz is 0.8 Hz wide) needs several lines across it.
pub const TARGET_LINE_SPACING_HZ: f64 = 0.25;

/// The longest record, samples.
pub const MAX_RECORD_LEN: usize = 1 << 18;

/// The share of each drive's ceiling the probe plays at, rms.
pub const PROBE_RMS_OF_CEILING: f64 = 0.01;

/// The grid reaches this factor past each end of the test's band.
pub const GRID_MARGIN: f64 = 1.25;

/// A small deterministic generator (splitmix64), so an identification is the
/// same every run with the same seed.
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

/// The one-sided spectrum of a real power-of-two record in the
/// negative-exponent convention (`hertzforge_dsp::fft` is the positive one, so
/// its output is conjugated).
pub fn rfft(x: &[f64]) -> Vec<C64> {
    let n = x.len();
    let mut buf: Vec<(f64, f64)> = x.iter().map(|&v| (v, 0.0)).collect();
    fft::fft_in_place(&mut buf);
    buf[..n / 2 + 1]
        .iter()
        .map(|c| C64::new(c.0, -c.1))
        .collect()
}

/// The real record of `n` samples whose one-sided spectrum (this module's
/// convention) is `half`.
pub fn irfft(half: &[C64], n: usize) -> Vec<f64> {
    let mut spec = vec![(0.0f64, 0.0f64); n];
    spec[0] = (half[0].re, 0.0);
    spec[n / 2] = (half[n / 2].re, 0.0);
    for k in 1..n / 2 {
        spec[k] = (half[k].re, -half[k].im);
        spec[n - k] = (half[k].re, half[k].im);
    }
    fft::ifft(&spec).iter().map(|c| c.0).collect()
}

/// The record length for a rate: the shortest power of two whose line
/// spacing is at most [`TARGET_LINE_SPACING_HZ`], at most [`MAX_RECORD_LEN`].
pub fn record_len_for(fs_hz: f64) -> usize {
    let mut n = 1024usize;
    while fs_hz / n as f64 > TARGET_LINE_SPACING_HZ && n < MAX_RECORD_LEN {
        n *= 2;
    }
    n
}

/// The band the identification covers for a test band: [`GRID_MARGIN`] past
/// each end, under 0.45 of the rate.
pub fn grid_band(test_band: (f64, f64), fs_hz: f64) -> (f64, f64) {
    (
        test_band.0 / GRID_MARGIN,
        (test_band.1 * GRID_MARGIN).min(0.45 * fs_hz),
    )
}

/// The lines a band covers: `lo <= k df <= hi`, never line 0 or the Nyquist
/// line.
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

/// The identification drive: one row per drive of `2 * realizations *
/// record_len` samples, each realization at `rms[d]` volts rms and its peak at
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
            let mut x = irfft(&spec, record_len);
            let r = (x.iter().map(|v| v * v).sum::<f64>() / record_len as f64).sqrt();
            if r.is_nan() || r <= 0.0 {
                return Err("the identification band holds no line of the grid".into());
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

/// The drive-to-response lag, samples: the peak of the cross-correlations of
/// every response with every drive over the first realization's measured
/// period, summed in magnitude, searched over the first half of the period.
pub fn measure_lag(drives: &[Vec<f64>], responses: &[Vec<f64>], record_len: usize) -> usize {
    let n = record_len;
    let at = n;
    let xs: Vec<Vec<C64>> = drives.iter().map(|r| rfft(&r[at..at + n])).collect();
    let mut sum = vec![0.0f64; n];
    for y in responses {
        let ys = rfft(&y[at..at + n]);
        for x in &xs {
            let cross: Vec<C64> = ys.iter().zip(x).map(|(a, b)| a * b.conj()).collect();
            let r = irfft(&cross, n);
            for (s, v) in sum.iter_mut().zip(r) {
                *s += v.abs();
            }
        }
    }
    let half = n / 2;
    let mut best = (0usize, f64::NEG_INFINITY);
    for (k, v) in sum.iter().enumerate().take(half) {
        if *v > best.1 {
            best = (k, *v);
        }
    }
    best.0
}

/// The largest absolute value of each row over the measured periods.
pub fn measured_peaks(rows: &[Vec<f64>], record_len: usize, realizations: usize) -> Vec<f64> {
    rows.iter()
        .map(|r| {
            (0..realizations)
                .flat_map(|k| {
                    let at = (2 * k + 1) * record_len;
                    r[at..(at + record_len).min(r.len())].iter()
                })
                .fold(0.0f64, |a, v| a.max(v.abs()))
        })
        .collect()
}

/// What an identification gave, on its grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    /// The grid, Hz.
    pub grid_hz: Vec<f64>,
    /// Every response (controls, then limit channels) over every drive, the
    /// lag taken out: `lines x responses x drives`.
    pub frf: MatrixStack,
    /// Each response's multiple coherence, response after response
    /// (`responses * lines`).
    pub coherence: Vec<f64>,
}

/// The H1 estimate on the lines of `grid` from the measured periods, with
/// `lag` samples taken out. `drives` are the rows the excitation built;
/// `responses` the rows recorded alongside them, sample for sample, each at
/// least as long.
pub fn estimate(
    drives: &[Vec<f64>],
    responses: &[Vec<f64>],
    record_len: usize,
    realizations: usize,
    fs_hz: f64,
    grid: &[bool],
    lag: usize,
) -> Result<Estimate, String> {
    let n = record_len;
    let (n_d, n_o) = (drives.len(), responses.len());
    let need = 2 * realizations * n;
    if drives.iter().any(|r| r.len() < need) || responses.iter().any(|r| r.len() < need) {
        return Err("the identification rows are shorter than the shot".into());
    }
    let idx: Vec<usize> = grid
        .iter()
        .enumerate()
        .filter_map(|(k, on)| on.then_some(k))
        .collect();
    let lines = idx.len();
    if lines < 2 {
        return Err("the identification grid has fewer than two lines".into());
    }
    let df = fs_hz / n as f64;
    let mut gxx = MatrixStack::zeros(lines, n_d, n_d);
    let mut gyx = MatrixStack::zeros(lines, n_o, n_d);
    let mut gyy = MatrixStack::zeros(lines, n_o, n_o);
    for k in 0..realizations {
        let at = (2 * k + 1) * n;
        let xs: Vec<Vec<C64>> = drives.iter().map(|r| rfft(&r[at..at + n])).collect();
        let ys: Vec<Vec<C64>> = responses.iter().map(|r| rfft(&r[at..at + n])).collect();
        for (l, &f) in idx.iter().enumerate() {
            // The response window starts at the drive's own sample, so the lag
            // is in it as a delay; take it out.
            let w = std::f64::consts::TAU * f as f64 * df * lag as f64 / fs_hz;
            let rot = C64::from_polar(1.0, w);
            let lx = gxx.line_mut(l);
            for i in 0..n_d {
                for j in 0..n_d {
                    lx[i * n_d + j] += xs[i][f] * xs[j][f].conj();
                }
            }
            let ly = gyx.line_mut(l);
            for i in 0..n_o {
                for j in 0..n_d {
                    ly[i * n_d + j] += ys[i][f] * rot * xs[j][f].conj();
                }
            }
            let lyy = gyy.line_mut(l);
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
    let lm =
        multiple_coherence(&gyy, &gyx, &gxx, Regularization::None).map_err(|e| e.to_string())?;
    let mut coherence = vec![0.0; n_o * lines];
    for f in 0..lines {
        for o in 0..n_o {
            coherence[o * lines + f] = lm[f * n_o + o].clamp(0.0, 1.0);
        }
    }
    Ok(Estimate {
        grid_hz: idx.iter().map(|&k| k as f64 * df).collect(),
        frf,
        coherence,
    })
}

/// The rows `from..to` of a stack.
pub fn rows_of(stack: &MatrixStack, from: usize, to: usize) -> MatrixStack {
    let (lines, _, cols) = stack.dims();
    let mut out = MatrixStack::zeros(lines, to - from, cols);
    for f in 0..lines {
        out.line_mut(f)
            .copy_from_slice(&stack.line(f)[from * cols..to * cols]);
    }
    out
}

/// The identification as a law takes it: the control rows, the limit rows,
/// each set's coherence, and a line trusted when every control's coherence on
/// it reaches `trust_coherence`.
pub fn law_identification(
    est: &Estimate,
    controls: usize,
    trust_coherence: f64,
) -> LawIdentification {
    let (lines, n_o, _) = est.frf.dims();
    let trusted = (0..lines)
        .map(|f| (0..controls).all(|c| est.coherence[c * lines + f] >= trust_coherence))
        .collect();
    LawIdentification {
        frequency_hz: est.grid_hz.clone(),
        response: rows_of(&est.frf, 0, controls),
        limit_response: rows_of(&est.frf, controls, n_o),
        coherence: est.coherence[..controls * lines].to_vec(),
        limit_coherence: est.coherence[controls * lines..].to_vec(),
        trusted,
    }
}

/// Each control's mean multiple coherence over the lines inside `band`.
pub fn band_coherence(id: &LawIdentification, controls: usize, band: (f64, f64)) -> Vec<f64> {
    let lines = id.frequency_hz.len();
    let (lo, hi) = if (band.1 - band.0).abs() < 1e-9 {
        (band.0 / 2f64.sqrt(), band.1 * 2f64.sqrt())
    } else {
        band
    };
    let inside: Vec<usize> = (0..lines)
        .filter(|&k| id.frequency_hz[k] >= lo && id.frequency_hz[k] <= hi)
        .collect();
    (0..controls)
        .map(|c| {
            if inside.is_empty() {
                return 0.0;
            }
            inside
                .iter()
                .map(|&k| id.coherence[c * lines + k])
                .sum::<f64>()
                / inside.len() as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structure of known response (a gain and a delay per path, then a
    /// common lag) played periodically: the estimate returns it on every line
    /// with the lag taken out, coherence one, and the lag is found.
    #[test]
    fn mimo_sine_ident_recovers_a_known_response_and_its_lag() {
        let (n, real, fs) = (1024usize, 3usize, 1024.0);
        let band = band_lines(fs, n, 10.0, 200.0);
        let rows = excitation(n, &band, &[0.1, 0.1], &[1.0, 1.0], real, 7).unwrap();
        let gains = [[1.0, -0.5], [0.25, 2.0]];
        let delays = [[0usize, 1], [2, 0]];
        let lag = 5usize;
        let len = rows[0].len();
        let resp: Vec<Vec<f64>> = (0..2)
            .map(|o| {
                (0..len)
                    .map(|t| {
                        (0..2)
                            .map(|d| {
                                let k = t as isize - (delays[o][d] + lag) as isize;
                                if k < 0 {
                                    0.0
                                } else {
                                    gains[o][d] * rows[d][k as usize]
                                }
                            })
                            .sum()
                    })
                    .collect()
            })
            .collect();
        assert_eq!(measure_lag(&rows, &resp, n), lag);
        let est = estimate(&rows, &resp, n, real, fs, &band, lag).unwrap();
        for (l, f) in est.grid_hz.iter().enumerate() {
            for o in 0..2 {
                for d in 0..2 {
                    let want = C64::from_polar(
                        gains[o][d],
                        -std::f64::consts::TAU * f * delays[o][d] as f64 / fs,
                    );
                    let got = est.frf.get(l, o, d);
                    assert!(
                        (got - want).norm() < 1e-9,
                        "{f} Hz {o},{d}: {got} vs {want}"
                    );
                }
            }
        }
        assert!(est.coherence.iter().all(|c| *c > 1.0 - 1e-9));
    }

    #[test]
    fn mimo_sine_ident_excitation_keeps_rms_and_peak_cap() {
        let n = 2048;
        let band = band_lines(2048.0, n, 5.0, 500.0);
        let rows = excitation(n, &band, &[0.5, 0.05], &[1.0, 1.0], 2, 3).unwrap();
        // Drive 0: the peak cap binds before the rms.
        assert!(rows[0].iter().all(|v| v.abs() <= 1.0 + 1e-12));
        let r1 = (rows[1][..n].iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        assert!((r1 - 0.05).abs() < 1e-12);
    }
}
