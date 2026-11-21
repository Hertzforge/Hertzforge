//! The setup prediction per drive and per control (design D8, D9): what the
//! random part (floor and every band at its starting position) and each
//! tone at its starting frequency will ask of each drive under the model,
//! against the drive's rms ceiling and peak ceiling.
//!
//! The random drive is `S = Z G_ref Z^H` per line of the band, `Z` the
//! model's regularized inverse; its rms per drive is MIMO Random's
//! [`hertzforge_mimo_random::guards::drive_rms`]; its peak is the
//! held-peak factor ([`held_peak_factor`]) times that. A tone asks `D_k = Z(f_k) R_k` at its starting
//! frequency (stated phases, or zero). Peaks add (theory 4.4); mean squares
//! add, a tone of peak `A` contributing `A^2 / 2`.

use crate::band::{increment, weights_at};
use crate::budget::model_drive;
use crate::config::{MimoMixedConfig, MimoMixedConfigError};
use crate::mat::{CMat, Z_REGULARIZATION};
use hertzforge_mimo_random::guards::drive_rms;
use hertzforge_mixed::PartClock;
use hertzforge_sdm::{HermitianStack, MatrixStack};

/// One drive's prediction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrivePrediction {
    /// The random part's rms, volts.
    pub random_rms_v: f64,
    /// The random part's peak, volts: its rms times `random_peak_factor`.
    pub random_peak_v: f64,
    /// The random part's peak over its rms: the held-peak factor
    /// ([`held_peak_factor`]), at least the configured crest factor, at
    /// most the sigma clip when one is set.
    pub random_peak_factor: f64,
    /// The sum of the tones' amplitudes on the drive, volts peak.
    pub tone_peak_sum_v: f64,
    /// The tones' rms, volts.
    pub tone_rms_v: f64,
    /// Random and tones together, rms, volts.
    pub total_rms_v: f64,
    /// Random peak plus the tones' sum, volts.
    pub peak_v: f64,
    /// The drive's rms ceiling, volts.
    pub ceiling_vrms: f64,
    /// The drive's peak ceiling, volts.
    pub ceiling_peak_v: f64,
}

impl DrivePrediction {
    /// Whether the random part's rms is within the rms ceiling and the summed
    /// peak within the peak ceiling.
    pub fn within(&self) -> bool {
        self.random_rms_v <= self.ceiling_vrms && self.peak_v <= self.ceiling_peak_v
    }
}

/// One control's prediction, at full level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlPrediction {
    /// The random part's rms (floor and bands), EU.
    pub random_rms: f64,
    /// The tones' rms, EU.
    pub tone_rms: f64,
    /// Both, rms, EU.
    pub total_rms: f64,
}

/// The setup prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct MimoMixedPrediction {
    /// Per drive.
    pub drives: Vec<DrivePrediction>,
    /// Per control.
    pub controls: Vec<ControlPrediction>,
    /// The random part's drive spectral matrix, `m` by `m`, full level.
    pub random_drive: HermitianStack,
    /// Each tone's drive vector at its starting frequency (parts order of
    /// the tones), volts peak.
    pub tone_drives: Vec<Vec<hertzforge_sdm::C64>>,
}

impl MimoMixedPrediction {
    /// Whether every drive is within its ceilings.
    pub fn within(&self) -> bool {
        self.drives.iter().all(DrivePrediction::within)
    }

    /// The random part's peak per drive, volts at full level (what the
    /// budget forecast takes).
    pub fn random_peaks_v(&self) -> Vec<f64> {
        self.drives.iter().map(|d| d.random_peak_v).collect()
    }
}

/// The reference the random part plays at the start: the floor plus every
/// band at its starting center.
pub fn random_reference(cfg: &MimoMixedConfig) -> Result<HermitianStack, MimoMixedConfigError> {
    let proj = cfg.projection(0);
    let mut clock = PartClock::new(&proj)?;
    let now = clock.tick(0.0).to_vec();
    let floor = &cfg.reference.sdm;
    let mut g = floor.clone();
    let df = cfg.df_hz();
    let mut w = vec![0.0; floor.lines()];
    for (i, _, b) in cfg.bands() {
        let Some(c) = now[i].freq_hz else { continue };
        weights_at(&b.width, c, df, &mut w);
        for (l, &wl) in w.iter().enumerate() {
            if wl > 0.0 {
                let gf = CMat::from_hermitian(floor, l);
                let add = increment(&b.statement, &gf).scaled(wl);
                CMat::from_hermitian(&g, l)
                    .add(&add)
                    .store_hermitian(&mut g, l);
            }
        }
    }
    Ok(g)
}

/// How long the run's step holds a random peak before it has fallen by
/// `1/e`, seconds: the held peak the ceiling rule subtracts is each drive's
/// largest look-ahead peak, decayed at this rate.
pub const RANDOM_PEAK_RELEASE_S: f64 = 1.0;

/// The chance, at any moment of the run, that the random part's held peak
/// is over the forecast's random peak.
pub const HELD_PEAK_EXCEEDANCE: f64 = 0.01;

/// The random part's held peak over its rms that the run's held peak
/// exceeds with chance `p` at a given moment.
///
/// The step holds, per drive, the largest magnitude of the next `ahead_s`
/// of random drive, and before that every earlier magnitude decayed by
/// `exp(-age / release_s)`. A forecast of "rms times a crest factor of 3"
/// sits under that: a gaussian drive of a few hundred hertz exceeds 3
/// sigma many times a second, and the held peak keeps each excursion for
/// about a second, so the run's budget is short of the forecast's most of
/// the time (found: the shared-ceiling run with every peak ceiling 1.05
/// times the forecast need limited every tone in most chunks). The held
/// peak is over `k` sigma when the drive crossed `k exp(age / release_s)`
/// sigma at some age, or crosses `k` sigma in the look-ahead. Rice's rate
/// of crossings of `+-u` sigma is `2 nu0 exp(-u^2 / 2)`, `nu0` the zero
/// upcrossing rate `sqrt(m2 / m0)` of the drive's spectrum; with crossings
/// taken as Poisson the chance of none is `exp(-L(k))`,
/// `L(k) = 2 nu0 (ahead_s exp(-k^2 / 2) + int_0^inf exp(-k^2 exp(2 a /
/// release_s) / 2) da)`. The factor solves `1 - exp(-L(k)) = p`. With no
/// spectrum (`nu0` zero or not finite) it is 0.
pub fn held_peak_factor(nu0_hz: f64, release_s: f64, ahead_s: f64, p: f64) -> f64 {
    if !(nu0_hz.is_finite() && nu0_hz > 0.0 && release_s > 0.0 && p > 0.0 && p < 1.0) {
        return 0.0;
    }
    let want = -(1.0 - p).ln();
    let rate = |k: f64| -> f64 {
        // The look-back integral by the trapezoid rule, out to where the
        // level is 40 sigma (the integrand is under 1e-300 beyond).
        let a_max = release_s * (40.0 / k).ln().max(0.0);
        let steps = 4000;
        let h = a_max / steps as f64;
        let g = |a: f64| (-(k * (a / release_s).exp()).powi(2) / 2.0).exp();
        let mut back = 0.5 * (g(0.0) + g(a_max));
        for i in 1..steps {
            back += g(i as f64 * h);
        }
        back *= h;
        2.0 * nu0_hz * (ahead_s.max(0.0) * (-k * k / 2.0).exp() + back)
    };
    let (mut lo, mut hi) = (0.5f64, 40.0f64);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if rate(mid) > want {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Predict, from the model `Hhat` (`n` by `m` on the grid).
pub fn predict(
    cfg: &MimoMixedConfig,
    model: &MatrixStack,
) -> Result<MimoMixedPrediction, MimoMixedConfigError> {
    let (n, m) = (cfg.n(), cfg.m());
    let df = cfg.df_hz();
    let band = cfg.reference.band;
    let g = random_reference(cfg)?;
    let mut s = HermitianStack::zeros(g.lines(), m);
    for l in band.0..band.1.min(g.lines()).min(model.lines()) {
        let Ok(z) = CMat::from_stack(model, l).pinv(Z_REGULARIZATION) else {
            continue;
        };
        CMat::from_hermitian(&g, l)
            .congruence(&z)
            .store_hermitian(&mut s, l);
    }
    let rms = drive_rms(&s, band, df);
    let ahead_s = 2.0 * cfg.control_tick_s();
    let proj = cfg.projection(0);
    let mut clock = PartClock::new(&proj)?;
    let now = clock.tick(0.0).to_vec();
    let mut tone_drives = Vec::new();
    let mut tone_ms_controls = vec![0.0; n];
    for (i, _, t) in cfg.tones() {
        let f = now[i].freq_hz.unwrap_or(0.0);
        let r = t.reference_at(n, f).unwrap_or_default();
        for (ms, v) in tone_ms_controls.iter_mut().zip(&r) {
            *ms += v.norm_sqr() / 2.0;
        }
        tone_drives.push(model_drive(model, df, f, &r).unwrap_or_default());
    }
    let drives = (0..m)
        .map(|j| {
            let sum: f64 = tone_drives
                .iter()
                .map(|d| d.get(j).map_or(0.0, |v| v.norm()))
                .sum();
            let ms: f64 = tone_drives
                .iter()
                .map(|d| d.get(j).map_or(0.0, |v| v.norm_sqr() / 2.0))
                .sum();
            let r = rms[j];
            let (mut m0, mut m2) = (0.0, 0.0);
            for l in band.0..band.1.min(s.lines()) {
                let v = s.get(l, j, j).re.max(0.0);
                let f = l as f64 * df;
                m0 += v;
                m2 += f * f * v;
            }
            let nu0 = if m0 > 0.0 { (m2 / m0).sqrt() } else { 0.0 };
            let held = held_peak_factor(nu0, RANDOM_PEAK_RELEASE_S, ahead_s, HELD_PEAK_EXCEEDANCE);
            let mut clip = held.max(cfg.crest_factor);
            if let Some(c) = cfg.drive_clip_sigma {
                clip = clip.min(c);
            }
            DrivePrediction {
                random_rms_v: r,
                random_peak_v: clip * r,
                random_peak_factor: clip,
                tone_peak_sum_v: sum,
                tone_rms_v: ms.sqrt(),
                total_rms_v: (r * r + ms).sqrt(),
                peak_v: clip * r + sum,
                ceiling_vrms: cfg.drives[j].ceiling_vrms,
                ceiling_peak_v: cfg.drives[j].peak_v,
            }
        })
        .collect();
    let controls = (0..n)
        .map(|i| {
            let ms: f64 = (band.0..band.1.min(g.lines()))
                .map(|l| g.get(l, i, i).re.max(0.0))
                .sum::<f64>()
                * df;
            let t = tone_ms_controls[i];
            ControlPrediction {
                random_rms: ms.sqrt(),
                tone_rms: t.sqrt(),
                total_rms: (ms + t).sqrt(),
            }
        })
        .collect();
    Ok(MimoMixedPrediction {
        drives,
        controls,
        random_drive: s,
        tone_drives,
    })
}
