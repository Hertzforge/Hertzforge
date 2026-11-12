//! The core's guards on any law's answer (design D11). Each is a plain function
//! of numbers so it can be tested on its own; the engine applies them in the
//! pulse's order.
//!
//! - [`growth`]: how much each drive grew over the drive last played, beyond
//!   the level step, on its peak and per third-octave band.
//! - [`predict`]: the response the identified plant predicts for a drive,
//!   `Y = H D` line by line, for every control and limit row.
//! - [`DivergenceWatch`]: an error that rises on consecutive pulses at one level.

use hertzforge_mimo_shock_law_math::spectrum::{irfft, rfft};
use hertzforge_sdm::{MatrixStack, C64};
use serde::{Deserialize, Serialize};

/// Which part of a drive grew too much.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Growth {
    /// The drive, in drive order.
    pub drive: usize,
    /// `peak` or the band's centre in Hz.
    pub what: String,
    /// The growth beyond the level step, dB.
    pub db: f64,
}

/// Third-octave band edges over `[lo, hi]` Hz as line ranges (each at least
/// one line).
pub fn bands(fs_hz: f64, record_len: usize, lo_hz: f64, hi_hz: f64) -> Vec<(usize, usize)> {
    let df = fs_hz / record_len as f64;
    let lines = record_len / 2 + 1;
    let first = ((lo_hz / df).ceil() as usize).clamp(1, lines - 1);
    let last = ((hi_hz / df).floor() as usize).min(lines - 1);
    let step = 2f64.powf(1.0 / 3.0);
    let mut out = Vec::new();
    let mut a = first;
    let mut edge = first as f64 * df;
    while a <= last {
        edge *= step;
        // Contiguous: each band starts where the last ended, and holds a line.
        let b = (((edge / df).floor() as usize).max(a)).min(last);
        out.push((a, b));
        a = b + 1;
    }
    out
}

fn band_rms(spec: &[C64], bands: &[(usize, usize)]) -> Vec<f64> {
    bands
        .iter()
        .map(|&(a, b)| {
            (spec[a..=b].iter().map(|v| v.norm_sqr()).sum::<f64>() / (b - a + 1) as f64).sqrt()
        })
        .collect()
}

/// The largest growth of each drive in `next` over `last` (both channel-major,
/// `record_len` a drive), with `step` the amplitude ratio of the next level
/// over the last: the peak's growth and every band's, in dB beyond the step.
/// A band below `floor_rel` of the last drive's largest band does not count.
/// Returns the worst growth found per drive.
pub fn growth(
    next: &[f64],
    last: &[f64],
    record_len: usize,
    step: f64,
    bands: &[(usize, usize)],
    floor_rel: f64,
) -> Result<Vec<Growth>, String> {
    let n = record_len;
    let n_d = last.len() / n;
    let db = |a: f64, b: f64| 20.0 * (a / b).log10();
    let mut out = Vec::with_capacity(n_d);
    for d in 0..n_d {
        let (x, l) = (&next[d * n..(d + 1) * n], &last[d * n..(d + 1) * n]);
        let pk_l = crate::judge::peak(l) * step;
        let pk_x = crate::judge::peak(x);
        let mut worst = Growth {
            drive: d,
            what: "peak".into(),
            db: if pk_l > 0.0 {
                db(pk_x, pk_l)
            } else if pk_x > 0.0 {
                f64::INFINITY
            } else {
                0.0
            },
        };
        let (bx, bl) = (band_rms(&rfft(x)?, bands), band_rms(&rfft(l)?, bands));
        let floor = bl.iter().fold(0.0f64, |a, v| a.max(*v)) * floor_rel * step;
        let df_centre = |i: usize| -> String { format!("band {}", i) };
        for (i, (vx, vl)) in bx.iter().zip(&bl).enumerate() {
            let base = (vl * step).max(floor);
            if base > 0.0 && *vx > 0.0 {
                let g = db(*vx, base);
                if g > worst.db {
                    worst = Growth {
                        drive: d,
                        what: df_centre(i),
                        db: g,
                    };
                }
            }
        }
        out.push(worst);
    }
    Ok(out)
}

/// The response `Y = H D` the plant `h` (outputs by drives on the grid)
/// predicts for `drives` (channel-major): one record per output.
pub fn predict(
    h: &MatrixStack,
    drives: &[f64],
    record_len: usize,
) -> Result<Vec<Vec<f64>>, String> {
    let (lines, n_o, n_d) = h.dims();
    let n = record_len;
    let ds: Vec<Vec<C64>> = (0..n_d)
        .map(|d| rfft(&drives[d * n..(d + 1) * n]))
        .collect::<Result<_, _>>()?;
    (0..n_o)
        .map(|o| {
            let y: Vec<C64> = (0..lines)
                .map(|f| {
                    let row = &h.line(f)[o * n_d..(o + 1) * n_d];
                    row.iter().zip(&ds).map(|(hv, d)| hv * d[f]).sum()
                })
                .collect();
            irfft(&y, n)
        })
        .collect()
}

/// The divergence watch: a run whose watched errors rise on consecutive pulses
/// at one level ends in the controlled stop.
///
/// Two errors are watched every pulse, each the worst over the controls: the
/// judged error ([`crate::ControlVerdict::error_db`]: the waveform error for a
/// waveform target, the largest SRS deviation for an SRS target) and the
/// response's peak over its target's peak at the level (the peak ratio), so a
/// waveform growing behind a good SRS is seen. A pulse rises when either grew
/// more than `rise_db` over the previous pulse at the same level; `pulses`
/// rising pulses in a row trip the watch. A level change starts it afresh.
#[derive(Debug, Clone, PartialEq)]
pub struct DivergenceWatch {
    pulses: u32,
    rise_db: f64,
    level_db: Option<f64>,
    last: Option<(f64, f64)>,
    rising: u32,
}

/// What tripped the watch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    /// Rising pulses in a row.
    pub pulses: u32,
    /// The judged error and the peak ratio on the last pulse, dB.
    pub error_db: f64,
    pub peak_ratio_db: f64,
}

impl DivergenceWatch {
    pub fn new(pulses: u32, rise_db: f64) -> Self {
        DivergenceWatch {
            pulses: pulses.max(1),
            rise_db,
            level_db: None,
            last: None,
            rising: 0,
        }
    }

    /// Note one judged pulse at `level_db`; `Some` when the watch trips.
    pub fn observe(
        &mut self,
        level_db: f64,
        error_db: f64,
        peak_ratio_db: f64,
    ) -> Option<Divergence> {
        if self.level_db != Some(level_db) {
            self.level_db = Some(level_db);
            self.last = None;
            self.rising = 0;
        }
        if let Some((e0, p0)) = self.last {
            if error_db - e0 > self.rise_db || peak_ratio_db - p0 > self.rise_db {
                self.rising += 1;
            } else {
                self.rising = 0;
            }
        }
        self.last = Some((error_db, peak_ratio_db));
        (self.rising >= self.pulses).then_some(Divergence {
            pulses: self.rising,
            error_db,
            peak_ratio_db,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_engine_divergence_watch_trips_on_rises_at_one_level_only() {
        let mut w = DivergenceWatch::new(2, 1.0);
        assert!(w.observe(-12.0, -20.0, 0.0).is_none());
        assert!(w.observe(-12.0, -18.5, 0.0).is_none()); // one rise
        assert!(w.observe(-12.0, -17.0, 0.0).is_some()); // two in a row
                                                         // A level change starts afresh.
        let mut w = DivergenceWatch::new(2, 1.0);
        w.observe(-12.0, -20.0, 0.0);
        w.observe(-12.0, -18.0, 0.0);
        assert!(w.observe(-9.0, -10.0, 0.0).is_none());
        // The peak ratio alone also rises.
        let mut w = DivergenceWatch::new(2, 1.0);
        w.observe(0.0, 3.0, 0.0);
        w.observe(0.0, 2.0, 1.5);
        assert!(w.observe(0.0, 1.0, 3.0).is_some());
        // A fall resets the count.
        let mut w = DivergenceWatch::new(2, 1.0);
        w.observe(0.0, -20.0, 0.0);
        w.observe(0.0, -18.0, 0.0);
        w.observe(0.0, -19.0, 0.0);
        assert!(w.observe(0.0, -17.5, 0.0).is_none());
    }

    #[test]
    fn mimo_shock_engine_growth_counts_the_level_step_and_each_band() {
        let n = 1024;
        let fs = 1024.0;
        let tone = |f: f64, a: f64| -> Vec<f64> {
            (0..n)
                .map(|i| a * (std::f64::consts::TAU * f * i as f64 / fs).sin())
                .collect()
        };
        let b = bands(fs, n, 10.0, 400.0);
        let last: Vec<f64> = tone(50.0, 1.0)
            .iter()
            .zip(tone(200.0, 0.1))
            .map(|(a, c)| a + c)
            .collect();
        // Twice the level: no growth beyond the step.
        let next: Vec<f64> = last.iter().map(|v| 2.0 * v).collect();
        let g = growth(&next, &last, n, 2.0, &b, 1e-3).unwrap();
        assert!(g[0].db.abs() < 1e-9, "{g:?}");
        // The small band grows ten times while the peak barely moves.
        let next: Vec<f64> = tone(50.0, 1.0)
            .iter()
            .zip(tone(200.0, 1.0))
            .map(|(a, c)| a + c)
            .collect();
        let g = growth(&next, &last, n, 1.0, &b, 1e-3).unwrap();
        assert!(g[0].db > 19.0 && g[0].what.starts_with("band"), "{g:?}");
    }

    #[test]
    fn mimo_shock_engine_prediction_is_h_times_d() {
        let n = 64;
        let mut h = MatrixStack::zeros(n / 2 + 1, 1, 2);
        for f in 0..=n / 2 {
            h.set(f, 0, 0, C64::new(2.0, 0.0));
            h.set(f, 0, 1, C64::new(-1.0, 0.0));
        }
        let d: Vec<f64> = (0..2 * n).map(|i| ((i * 7) % 11) as f64).collect();
        let y = predict(&h, &d, n).unwrap();
        for i in 0..n {
            assert!((y[0][i] - (2.0 * d[i] - d[n + i])).abs() < 1e-9);
        }
    }
}
