//! The setup prediction per drive and per control (design D8, D9): what the
//! random part (floor and every band at its starting position) and each
//! tone at its starting frequency will ask of each drive under the model,
//! against the drive's rms ceiling and peak ceiling.
//!
//! The random drive is `S = Z G_ref Z^H` per line of the band, `Z` the
//! model's regularized inverse; its rms per drive is MIMO Random's
//! [`hertzforge_mimo_random::guards::drive_rms`]; its peak is the clip (or
//! crest factor) times that. A tone asks `D_k = Z(f_k) R_k` at its starting
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
    /// The random part's peak (clip or crest factor times its rms), volts.
    pub random_peak_v: f64,
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
    let clip = cfg.drive_clip_sigma.unwrap_or(cfg.crest_factor);
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
            DrivePrediction {
                random_rms_v: r,
                random_peak_v: clip * r,
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
