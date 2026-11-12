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

/// How much one drive grew over the drive last played, beyond the level step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Growth {
    /// The drive, in drive order.
    pub drive: usize,
    /// The peak's growth, dB.
    pub peak_db: f64,
    /// The largest band's growth, dB, and that band's lowest line.
    pub band_db: f64,
    pub band_line: usize,
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

/// The growth of each drive in `next` over `last` (both channel-major,
/// `record_len` a drive), with `step` the amplitude ratio of the next level
/// over the last: the peak's growth and the largest band's, in dB beyond the
/// step. A band below `floor_rel` of the last drive's largest band (scaled by
/// the step) is measured against that floor instead.
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
        let peak_db = if pk_l > 0.0 {
            db(pk_x, pk_l)
        } else if pk_x > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };
        let (bx, bl) = (band_rms(&rfft(x)?, bands), band_rms(&rfft(l)?, bands));
        let floor = bl.iter().fold(0.0f64, |a, v| a.max(*v)) * floor_rel * step;
        let (mut band_db, mut band_line) = (f64::NEG_INFINITY, 0);
        for ((vx, vl), &(a, _)) in bx.iter().zip(&bl).zip(bands) {
            let base = (vl * step).max(floor);
            let g = if base > 0.0 {
                db(vx.max(1e-300), base)
            } else if *vx > 0.0 {
                f64::INFINITY
            } else {
                0.0
            };
            if g > band_db {
                band_db = g;
                band_line = a;
            }
        }
        out.push(Growth {
            drive: d,
            peak_db,
            band_db,
            band_line,
        });
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

/// The divergence watch: at one level, (a) the waveform error rising on
/// `rises` pulses in a row, or (b) any drive's rms over `drive_ratio` times its
/// rms on the level's first pulse, ends the run in the controlled stop. A level
/// change starts it afresh. For an SRS target the waveform error is the law's
/// own (its value named `waveform_error`), and rule (a) is not applied to a law
/// that does not report one.
#[derive(Debug, Clone, PartialEq)]
pub struct DivergenceWatch {
    rises: u32,
    drive_ratio: f64,
    level_db: Option<f64>,
    last: Option<f64>,
    rising: u32,
    first_rms: Vec<f64>,
}

/// What tripped the watch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    /// `waveform_error_rising` or `drive_growth`.
    pub rule: String,
    /// The facts, in words.
    pub detail: String,
}

impl DivergenceWatch {
    pub fn new(rises: u32, drive_ratio: f64) -> Self {
        DivergenceWatch {
            rises: rises.max(1),
            drive_ratio,
            level_db: None,
            last: None,
            rising: 0,
            first_rms: Vec::new(),
        }
    }

    /// Note one pulse at `level_db`: its waveform error (any unit that rises
    /// with the error), when there is one, and each drive's rms as played;
    /// `Some` when the watch trips.
    pub fn observe(
        &mut self,
        level_db: f64,
        waveform_error: Option<f64>,
        drive_rms: &[f64],
    ) -> Option<Divergence> {
        if self.level_db != Some(level_db) {
            self.level_db = Some(level_db);
            self.last = None;
            self.rising = 0;
            self.first_rms = drive_rms.to_vec();
        }
        for (d, (now, first)) in drive_rms.iter().zip(&self.first_rms).enumerate() {
            if *first > 0.0 && *now > self.drive_ratio * first {
                return Some(Divergence {
                    rule: "drive_growth".into(),
                    detail: format!(
                        "drive {d} plays {:.3} times its rms on the first pulse at {level_db} dB, \
                         over {}",
                        now / first,
                        self.drive_ratio
                    ),
                });
            }
        }
        if let Some(e) = waveform_error {
            match self.last {
                Some(prev) if e > prev => self.rising += 1,
                _ => self.rising = 0,
            }
            self.last = Some(e);
            if self.rising >= self.rises {
                return Some(Divergence {
                    rule: "waveform_error_rising".into(),
                    detail: format!(
                        "the waveform error rose on {} pulses in a row at {level_db} dB",
                        self.rising
                    ),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_engine_divergence_watch_trips_on_three_rises_or_drive_growth() {
        let rms = [1.0, 1.0];
        let mut w = DivergenceWatch::new(3, 1.3);
        for e in [-20.0, -19.0, -18.0] {
            assert!(w.observe(0.0, Some(e), &rms).is_none());
        }
        let d = w.observe(0.0, Some(-17.0), &rms).expect("three rises");
        assert_eq!(d.rule, "waveform_error_rising");
        // Two rises, a fall, two rises: never three in a row.
        let mut w = DivergenceWatch::new(3, 1.3);
        for e in [-20.0, -19.0, -18.0, -18.5, -18.0, -17.0] {
            assert!(w.observe(0.0, Some(e), &rms).is_none());
        }
        // A level change starts afresh.
        let mut w = DivergenceWatch::new(3, 1.3);
        for e in [-20.0, -19.0, -18.0] {
            w.observe(-3.0, Some(e), &rms);
        }
        assert!(w.observe(0.0, Some(-10.0), &[2.0, 2.0]).is_none());
        // Drive growth at one level, with no waveform error at all.
        let mut w = DivergenceWatch::new(3, 1.3);
        assert!(w.observe(0.0, None, &[1.0, 2.0]).is_none());
        assert!(w.observe(0.0, None, &[1.29, 2.0]).is_none());
        let d = w.observe(0.0, None, &[1.0, 2.7]).expect("drive 1 grew");
        assert_eq!(d.rule, "drive_growth");
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
        assert!(
            g[0].peak_db.abs() < 1e-9 && g[0].band_db.abs() < 1e-9,
            "{g:?}"
        );
        // The small band grows ten times while the peak barely moves.
        let next: Vec<f64> = tone(50.0, 1.0)
            .iter()
            .zip(tone(200.0, 1.0))
            .map(|(a, c)| a + c)
            .collect();
        let g = growth(&next, &last, n, 1.0, &b, 1e-3).unwrap();
        assert!(g[0].band_db > 19.0 && g[0].peak_db < 6.0, "{g:?}");
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
