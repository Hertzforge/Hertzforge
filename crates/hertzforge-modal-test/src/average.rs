//! The averages of one measurement, and its estimates.
//!
//! Every accepted frame's spectra are kept (the band's lines only), so an undo takes the
//! last frame back out exactly. The cross spectra are the linear average over the kept
//! frames, scaled as `hertzforge_dsp::frf_ext::cross_welch_with` scales them, and the
//! estimates come from the crates that own them, never a new estimator here:
//!
//! * one reference: `CrossSpectra::h1`, `h2` and `coherence` from `hertzforge-dsp`, and
//!   Hv as `|Hv| = sqrt(|H1| |H2|)` with the phase of `Gxy` (as
//!   `frf::h_estimate_hv`: `sqrt(Gyy / Gxx) e^(j angle Gxy)`);
//! * several references: `linalg::h1_frf`, `multiple_coherence` and
//!   `condition_numbers` from `hertzforge-sdm`, on `G_yx = E[Y X^H]` and
//!   `G_xx = E[X X^H]`.
//!
//! The spectra are kept in the textbook convention `X(f) = sum x(t) e^(-j 2 pi f t)`:
//! the conjugates of the dsp crate's own transforms.

use crate::setup::Estimator;
use hertzforge_dsp::frf_ext::CrossSpectra;
use hertzforge_sdm::linalg::{condition_numbers, h1_frf, multiple_coherence, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// One frame's spectra: per channel (references first, then responses), per kept line.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSpectra {
    pub x: Vec<Vec<C64>>,
}

/// The spectra of one frame: each channel times its data window, then the analysis
/// window, transformed and scaled so `E[conj(X) Y]` is the one-sided cross density.
pub fn frame_spectra(
    channels: &[&[f64]],
    data_windows: &[&[f64]],
    analysis: &[f64],
    fs_hz: f64,
    lines: std::ops::RangeInclusive<usize>,
) -> FrameSpectra {
    let n = analysis.len();
    let s2: f64 = analysis.iter().map(|v| v * v).sum();
    let norm = 1.0 / (fs_hz * s2);
    let mut x = Vec::with_capacity(channels.len());
    let mut buf = vec![(0.0, 0.0); n];
    for (c, data) in channels.iter().enumerate() {
        let dw = data_windows[c];
        for i in 0..n {
            buf[i] = (
                data.get(i).copied().unwrap_or(0.0) * dw[i] * analysis[i],
                0.0,
            );
        }
        hertzforge_dsp::fft::fft_in_place(&mut buf);
        let row = lines
            .clone()
            .map(|k| {
                let fold = if k != 0 && k != n / 2 { 2.0 } else { 1.0 };
                let s = (norm * fold).sqrt();
                C64::new(buf[k].0, -buf[k].1) * s
            })
            .collect();
        x.push(row);
    }
    FrameSpectra { x }
}

/// The estimates of one measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Estimates {
    /// `[response][reference][line]`.
    pub frf: Vec<Vec<Vec<C64>>>,
    /// One reference: the ordinary coherence `[response][line]`.
    pub coherence: Option<Vec<Vec<f64>>>,
    /// Several references: the multiple coherence `[response][line]`.
    pub multiple_coherence: Option<Vec<Vec<f64>>>,
    /// The references' autospectra `[reference][line]`.
    pub ref_auto: Vec<Vec<f64>>,
    /// Several references: the force matrix's condition number per line.
    pub condition: Option<Vec<f64>>,
    pub averages: usize,
}

/// The averages of one measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Averager {
    pub refs: usize,
    pub resps: usize,
    pub freqs: Vec<f64>,
    frames: Vec<FrameSpectra>,
}

impl Averager {
    pub fn new(refs: usize, resps: usize, freqs: Vec<f64>) -> Averager {
        Averager {
            refs,
            resps,
            freqs,
            frames: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.frames.len()
    }

    pub fn push(&mut self, f: FrameSpectra) {
        self.frames.push(f);
    }

    /// Take the last frame back out.
    pub fn undo(&mut self) -> bool {
        self.frames.pop().is_some()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// `E[a conj(b)]` over the kept frames at line `l`.
    fn g(&self, a: usize, b: usize, l: usize) -> C64 {
        let n = self.frames.len().max(1) as f64;
        self.frames
            .iter()
            .map(|f| f.x[a][l] * f.x[b][l].conj())
            .sum::<C64>()
            / n
    }

    /// The cross spectra of reference `r` and response `y`, as the dsp crate holds
    /// them: `gxy = E[conj(X) Y]`.
    pub fn cross(&self, r: usize, y: usize) -> CrossSpectra {
        let lines = self.freqs.len();
        let yi = self.refs + y;
        CrossSpectra {
            freqs: self.freqs.clone(),
            gxy: (0..lines)
                .map(|l| {
                    let v = self.g(yi, r, l);
                    (v.re, v.im)
                })
                .collect(),
            gxx: (0..lines).map(|l| self.g(r, r, l).re).collect(),
            gyy: (0..lines).map(|l| self.g(yi, yi, l).re).collect(),
            segments: self.frames.len(),
        }
    }

    /// The estimates. `None` with no frames.
    pub fn estimate(&self, est: Estimator) -> Option<Estimates> {
        if self.frames.is_empty() {
            return None;
        }
        let lines = self.freqs.len();
        let ref_auto = (0..self.refs)
            .map(|r| (0..lines).map(|l| self.g(r, r, l).re).collect())
            .collect();
        if self.refs == 1 {
            let mut frf = Vec::new();
            let mut coh = Vec::new();
            for y in 0..self.resps {
                let cs = self.cross(0, y);
                let to_c = |v: Vec<(f64, f64)>| {
                    v.into_iter()
                        .map(|(a, b)| C64::new(a, b))
                        .collect::<Vec<_>>()
                };
                let h = match est {
                    Estimator::H1 => to_c(cs.h1()),
                    Estimator::H2 => to_c(cs.h2()),
                    Estimator::Hv => {
                        let (h1, h2) = (to_c(cs.h1()), to_c(cs.h2()));
                        h1.iter()
                            .zip(&h2)
                            .map(|(a, b)| {
                                if a.norm() > 0.0 {
                                    a / a.norm() * (a.norm() * b.norm()).sqrt()
                                } else {
                                    C64::new(0.0, 0.0)
                                }
                            })
                            .collect()
                    }
                };
                frf.push(vec![h]);
                coh.push(cs.coherence());
            }
            return Some(Estimates {
                frf,
                coherence: Some(coh),
                multiple_coherence: None,
                ref_auto,
                condition: None,
                averages: self.frames.len(),
            });
        }
        let (d, c) = (self.refs, self.resps);
        let mut g_yx = MatrixStack::zeros(lines, c, d);
        let mut g_xx = HermitianStack::zeros(lines, d);
        let mut g_yy = HermitianStack::zeros(lines, c);
        let mut xx_full = MatrixStack::zeros(lines, d, d);
        for l in 0..lines {
            for i in 0..c {
                for j in 0..d {
                    g_yx.set(l, i, j, self.g(d + i, j, l));
                }
                for j in i..c {
                    g_yy.set(l, i, j, self.g(d + i, d + j, l));
                }
            }
            for i in 0..d {
                for j in 0..d {
                    let v = self.g(i, j, l);
                    xx_full.set(l, i, j, v);
                    if j >= i {
                        g_xx.set(l, i, j, v);
                    }
                }
            }
        }
        let h = h1_frf(&g_yx, &g_xx, Regularization::None).ok()?;
        let mc = multiple_coherence(&g_yy, &g_yx, &g_xx, Regularization::None).ok()?;
        let cond = condition_numbers(&xx_full).ok()?;
        let frf = (0..c)
            .map(|i| {
                (0..d)
                    .map(|j| (0..lines).map(|l| h.get(l, i, j)).collect())
                    .collect()
            })
            .collect();
        let mcoh = (0..c)
            .map(|i| (0..lines).map(|l| mc[l * c + i]).collect())
            .collect();
        Some(Estimates {
            frf,
            coherence: None,
            multiple_coherence: Some(mcoh),
            ref_auto,
            condition: Some(cond),
            averages: self.frames.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Rng;

    fn ones(n: usize) -> Vec<f64> {
        vec![1.0; n]
    }

    /// A random frame through a known two-tap system `y = x[n] + 0.5 x[n-1]`.
    fn frame(rng: &mut Rng, n: usize, noise: f64) -> (Vec<f64>, Vec<f64>) {
        let x: Vec<f64> = (0..n).map(|_| rng.gauss()).collect();
        // Circular, so the frame's DFT relation is exact.
        let y = (0..n)
            .map(|i| x[i] + 0.5 * x[(i + n - 1) % n] + noise * rng.gauss())
            .collect();
        (x, y)
    }

    #[test]
    fn modal_test_averaging_matches_the_dsp_welch_and_reads_the_system() {
        let n = 256;
        let fs = 256.0;
        let mut rng = Rng::new(3);
        let mut av = Averager::new(1, 1, (0..=n / 2).map(|k| k as f64).collect());
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for _ in 0..8 {
            let (x, y) = frame(&mut rng, n, 0.0);
            let w = ones(n);
            av.push(frame_spectra(&[&x, &y], &[&w, &w], &w, fs, 0..=n / 2));
            xs.extend(x);
            ys.extend(y);
        }
        let mine = av.cross(0, 0);
        let dsp = hertzforge_dsp::frf_ext::cross_welch_with(
            &xs,
            &ys,
            fs,
            &ones(n),
            0.0,
            hertzforge_dsp::spectrum_offline::Detrend::None,
        )
        .unwrap();
        for k in 0..=n / 2 {
            assert!((mine.gxy[k].0 - dsp.gxy[k].0).abs() < 1e-9 * (1.0 + dsp.gxx[k]));
            assert!((mine.gxy[k].1 - dsp.gxy[k].1).abs() < 1e-9 * (1.0 + dsp.gxx[k]));
            assert!((mine.gxx[k] - dsp.gxx[k]).abs() < 1e-9 * (1.0 + dsp.gxx[k]));
        }
        let e = av.estimate(Estimator::H1).unwrap();
        for k in [5usize, 40, 100] {
            let w = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
            let want = C64::new(1.0, 0.0) + C64::from_polar(0.5, -w);
            assert!((e.frf[0][0][k] - want).norm() < 1e-9, "line {k}");
            assert!((e.coherence.as_ref().unwrap()[0][k] - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn modal_test_undo_takes_the_last_average_back_out() {
        let n = 128;
        let mut rng = Rng::new(5);
        let mut av = Averager::new(1, 1, (0..=n / 2).map(|k| k as f64).collect());
        let w = ones(n);
        let (x, y) = frame(&mut rng, n, 0.3);
        av.push(frame_spectra(&[&x, &y], &[&w, &w], &w, 128.0, 0..=n / 2));
        let (x2, y2) = frame(&mut rng, n, 0.3);
        av.push(frame_spectra(&[&x2, &y2], &[&w, &w], &w, 128.0, 0..=n / 2));
        let two = av.estimate(Estimator::H1).unwrap();
        let (x3, y3) = frame(&mut rng, n, 0.3);
        av.push(frame_spectra(&[&x3, &y3], &[&w, &w], &w, 128.0, 0..=n / 2));
        assert_eq!(av.count(), 3);
        assert_ne!(av.estimate(Estimator::H1).unwrap().frf, two.frf);
        assert!(av.undo());
        assert_eq!(av.count(), 2);
        assert_eq!(av.estimate(Estimator::H1).unwrap(), two);
        assert!(av.undo() && av.undo());
        assert!(!av.undo());
        assert!(av.estimate(Estimator::H1).is_none());
    }

    #[test]
    fn modal_test_matrix_h1_reads_a_two_input_system() {
        let n = 128;
        let mut rng = Rng::new(9);
        let lines = 0..=n / 2;
        let mut av = Averager::new(2, 1, lines.clone().map(|k| k as f64).collect());
        let w = ones(n);
        for _ in 0..12 {
            let a: Vec<f64> = (0..n).map(|_| rng.gauss()).collect();
            let b: Vec<f64> = (0..n).map(|_| rng.gauss()).collect();
            let y: Vec<f64> = (0..n)
                .map(|i| 2.0 * a[i] - 0.5 * b[(i + n - 1) % n])
                .collect();
            av.push(frame_spectra(
                &[&a, &b, &y],
                &[&w, &w, &w],
                &w,
                128.0,
                lines.clone(),
            ));
        }
        let e = av.estimate(Estimator::H1).unwrap();
        for k in [3usize, 20, 50] {
            let wk = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
            assert!((e.frf[0][0][k] - C64::new(2.0, 0.0)).norm() < 1e-8);
            assert!((e.frf[0][1][k] - C64::from_polar(-0.5, -wk)).norm() < 1e-8);
            assert!((e.multiple_coherence.as_ref().unwrap()[0][k] - 1.0).abs() < 1e-8);
            assert!(e.condition.as_ref().unwrap()[k].is_finite());
        }
    }
}
