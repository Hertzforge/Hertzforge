//! `paired`: the simplest law, and the template a contributor copies.
//!
//! Each control is paired with one drive. The seed inverts that one path (the
//! pair's own frequency response); after each pulse, that drive's spectrum is
//! scaled per line by the ratio of the wanted to the measured magnitude at its
//! control, each smoothed over a few lines, raised to a weight and limited per
//! pulse. Nothing else of the plant is used, so the law cannot know that a
//! drive also moves the other controls: on a coupled plant the pairs fight.
//!
//! A new law usually needs four pieces, all visible here: its parameters, its
//! state, a seed, and an iteration that turns one pulse's measurements into
//! the next pulse's drives at the next level.

use crate::spectrum::{rms_all, rows_irfft, rows_rfft, smooth};
use crate::{
    check_inputs, check_setup, drive_mask, level_scale, LawSteps, PulseInputs, PulseOutputs, Setup,
    C64,
};

/// Parameters of `paired`.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedParams {
    /// The drive paired with each control. Every drive is used once at most.
    pub drive_of: Vec<usize>,
    /// Lines the magnitudes are smoothed over (odd).
    pub smooth_lines: usize,
    /// The weight on the magnitude ratio, 0 to 1.
    pub weight: f64,
    /// The largest change of a line in one pulse, dB.
    pub max_step_db: f64,
}

impl PairedParams {
    /// Control `c` paired with drive `c`, for the first `min(n_c, n_d)` controls.
    pub fn diagonal(n_controls: usize, n_drives: usize) -> Self {
        PairedParams {
            drive_of: (0..n_controls.min(n_drives)).collect(),
            smooth_lines: 15,
            weight: 0.8,
            max_step_db: 6.0,
        }
    }
}

/// The `paired` law's state: one drive spectrum per drive, at full level.
pub struct PairedLaw {
    setup: Setup,
    params: PairedParams,
    mask: Vec<bool>,
    reference: Vec<Vec<C64>>,
    drive: Vec<Vec<C64>>,
}

impl PairedLaw {
    /// The law for a setup.
    pub fn new(setup: Setup, params: PairedParams) -> Result<Self, String> {
        let lines = check_setup(&setup)?;
        if params.drive_of.len() > setup.n_controls
            || params.drive_of.iter().any(|&d| d >= setup.n_drives)
        {
            return Err("each paired control needs a drive that exists".into());
        }
        let mut used = vec![false; setup.n_drives];
        for &d in &params.drive_of {
            if std::mem::replace(&mut used[d], true) {
                return Err(format!("drive {d} is paired with two controls"));
            }
        }
        let mask = drive_mask(&setup);
        let reference = rows_rfft(&setup.reference, 1.0)?;
        // Seed: the pair's own path inverted, nothing on other lines.
        let mut drive = vec![vec![C64::new(0.0, 0.0); lines]; setup.n_drives];
        for (c, &p) in params.drive_of.iter().enumerate() {
            for f in 0..lines {
                let h = setup.plant.frf.get(f, c, p);
                if mask[f] && h.norm() > 0.0 {
                    drive[p][f] = reference[c][f] / h;
                }
            }
        }
        Ok(PairedLaw {
            setup,
            params,
            mask,
            reference,
            drive,
        })
    }
}

impl LawSteps for PairedLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let s = level_scale(level_db);
        let drives = rows_irfft(&self.drive, self.setup.record_len, s)?;
        Ok(PulseOutputs {
            values: vec![("drive_rms_full_level", rms_all(&drives) / s)],
            drives,
            delays_s: None,
        })
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        check_inputs(&self.setup, inputs)?;
        let s = level_scale(inputs.level_db);
        let y = rows_rfft(&inputs.responses, s)?;
        let d = rows_rfft(&inputs.drives, s)?;
        let lim = self.params.max_step_db / 20.0 * std::f64::consts::LN_10;
        for (c, &p) in self.params.drive_of.iter().enumerate() {
            let want: Vec<f64> = self.reference[c].iter().map(|v| v.norm_sqr()).collect();
            let got: Vec<f64> = y[c].iter().map(|v| v.norm_sqr()).collect();
            let want = smooth(&want, self.params.smooth_lines);
            let got = smooth(&got, self.params.smooth_lines);
            for f in 0..want.len() {
                if !self.mask[f] || want[f] <= 0.0 || got[f] <= 0.0 {
                    continue;
                }
                // Half the log power ratio is the log magnitude ratio.
                let step = (0.5 * self.params.weight * (want[f] / got[f]).ln()).clamp(-lim, lim);
                self.drive[p][f] = d[p][f] * step.exp();
            }
        }
        let s_next = level_scale(inputs.next_level_db);
        let drives = rows_irfft(&self.drive, self.setup.record_len, s_next)?;
        Ok(PulseOutputs {
            values: vec![("drive_rms_full_level", rms_all(&drives) / s_next)],
            drives,
            delays_s: None,
        })
    }
}
