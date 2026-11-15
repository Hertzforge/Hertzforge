//! `paired`: the simplest MIMO Shock law, and the one to copy when you want to
//! try an idea of your own.
//!
//! # What it does
//!
//! Each control point is paired with one drive: control 0 with drive 0,
//! control 1 with drive 1, and so on (a test with more controls than drives
//! leaves the extra controls unpaired; they are judged, but no drive answers
//! to them). The law keeps one drive spectrum per drive, held at full level.
//!
//! - **Seed.** Before the first pulse, each paired drive is the control's
//!   target divided by that one path of the identified plant, `D = R / H_cc`,
//!   line by line. No cross path is used.
//! - **After each pulse.** On every line, the paired drive is scaled by the
//!   ratio of the wanted to the measured magnitude at its control, both
//!   smoothed over a few lines, raised to a weight under one and limited to a
//!   few dB a pulse. The phase of the drive as played is kept.
//!
//! Nothing here knows that a drive also moves the other controls, so on a
//! strongly coupled structure the pairs work against each other. That is the
//! point of it: it is the single-channel habit applied to every pair, and it
//! shows what a law with no coupling model can and cannot do.
//!
//! # What a law is given and returns
//!
//! The controller identifies the plant on the pulse's own spectral lines and
//! builds the law once with [`Setup`]: the sample rate and record length, the
//! number of drives and controls, the band, each control's full-level target
//! waveform ([`Setup::reference`]; for an SRS test, a waveform synthesized to
//! meet the SRS), the plant `H` as a stack of `controls x drives` complex
//! matrices (one per line) and the lines it trusts. It then calls:
//!
//! - `seed(level_db)`: the drives of the first pulse, at the first level;
//! - `iterate(inputs)`: after each pulse, with the drives as played, the
//!   responses as measured (both at the level played, `inputs.level_db`) and
//!   the level of the next pulse (`inputs.next_level_db`). Return the drives
//!   of the next pulse at that next level, in volts.
//!
//! Every record is `record_len` samples. A spectrum is the one-sided transform
//! of a record (`record_len / 2 + 1` lines, line `k` at `k fs / record_len`);
//! [`spectrum::rows_rfft`] and [`spectrum::rows_irfft`] go there and back and
//! scale at the same time, which is how this law moves between "as played" and
//! "full level".
//!
//! The controller checks everything you return (the count and length of the
//! drives, finite values, each drive's ceiling, how much a drive grows from one
//! pulse to the next, the predicted response) before anything is played. A
//! law's answer is a request; you cannot hurt the shaker from here.
//!
//! # Making your own
//!
//! Copy this folder to `laws/mimo-shock-<your-name>/`, change the package name
//! in its `Cargo.toml`, change [`describe`](Paired::describe) (name, summary,
//! parameters, values), and change `new`, `seed` and `iterate`. Build it with
//! `scripts/build_laws.sh`; the law file lands in
//! `<target>/wasm32-wasip2/release/`. `docs/control-laws.md` ("Trying a new
//! idea") has the commands that run it against the standard plants and the
//! check.

use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};

/// The law's state, kept between pulses.
pub struct Paired {
    /// Samples per record.
    record_len: usize,
    /// The drive paired with each control (`pair[c]` drives control `c`).
    pair: Vec<usize>,
    /// The lines the law may drive: inside the band and trusted.
    mask: Vec<bool>,
    /// Each control's target spectrum at full level.
    reference: Vec<Vec<C64>>,
    /// Each drive's spectrum at full level: what the law believes the next
    /// pulse needs.
    drive: Vec<Vec<C64>>,
    // The parameters.
    weight: f64,
    smooth_lines: usize,
    max_step_db: f64,
}

impl ShockMimoLaw for Paired {
    fn describe() -> ShockMimoManifest {
        ManifestBuilder::new("paired", "0.1.0")
            .author("Hertzforge contributors")
            .summary(
                "Pairs each control with one drive and, after every pulse, scales that drive's \
                 spectrum by the smoothed ratio of wanted to measured magnitude at its control. \
                 It uses no cross path of the plant, so on a coupled structure the pairs work \
                 against each other.",
            )
            // Which tests the law will run: a classical waveform at every
            // control, an SRS at every control (it follows the waveform the
            // controller synthesized for each SRS), or both.
            .targets(&[TargetKind::Waveform, TargetKind::Srs])
            // Parameters: the operator sees these, the controller checks them
            // against these ranges and hands `new` every one, filled with its
            // default when the operator left it out.
            .number(
                "weight",
                "Weight",
                "",
                "The power the magnitude ratio is raised to, 0 to 1.",
                0.0,
                1.0,
                0.05,
                0.8,
            )
            .integer(
                "smooth_lines",
                "Smoothing",
                "lines",
                "Lines the magnitudes are smoothed over (odd).",
                1,
                99,
                15,
            )
            .number(
                "max_step_db",
                "Largest step",
                "dB",
                "The largest change of a line in one pulse.",
                0.5,
                12.0,
                0.5,
                6.0,
            )
            // Values: up to eight numbers a call may report, shown and
            // recorded beside each pulse.
            .value(
                "drive_rms_full_level",
                "Drive rms at full level",
                "V",
                "Rms of every drive together, referred to full level.",
            )
            .build()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let n = setup.record_len;
        let lines = n / 2 + 1;
        // Pair control c with drive c, as far as both exist.
        let pair: Vec<usize> = (0..setup.n_controls.min(setup.n_drives)).collect();
        // Drive only lines inside the band that the identification trusts.
        let df = setup.fs_hz / n as f64;
        let mask: Vec<bool> = (0..lines)
            .map(|k| {
                let f = k as f64 * df;
                f >= setup.band_lo_hz && f <= setup.band_hi_hz && setup.plant.trusted[k]
            })
            .collect();
        let reference = spectrum::rows_rfft(&setup.reference, 1.0)?;
        // The seed: invert each pair's own path, nothing anywhere else.
        let mut drive = vec![vec![C64::new(0.0, 0.0); lines]; setup.n_drives];
        for (c, &p) in pair.iter().enumerate() {
            for f in 0..lines {
                let h = setup.plant.frf.get(f, c, p);
                if mask[f] && h.norm() > 0.0 {
                    drive[p][f] = reference[c][f] / h;
                }
            }
        }
        Ok(Paired {
            record_len: n,
            pair,
            mask,
            reference,
            drive,
            weight: params.number("weight")?,
            smooth_lines: params.integer("smooth_lines")?.max(1) as usize,
            max_step_db: params.number("max_step_db")?,
        })
    }

    fn seed(&mut self, level_db: f64) -> Result<Outputs, String> {
        // The drive spectra are at full level: scale to the first level.
        self.play(level_scale(level_db))
    }

    fn iterate(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        // Refer what was played and measured to full level.
        let s = level_scale(inputs.level_db);
        let y = spectrum::rows_rfft(&inputs.responses, s)?;
        let d = spectrum::rows_rfft(&inputs.drives, s)?;
        // The step limit as a natural log of a magnitude ratio.
        let lim = self.max_step_db / 20.0 * std::f64::consts::LN_10;
        for (c, &p) in self.pair.iter().enumerate() {
            let want: Vec<f64> = self.reference[c].iter().map(|v| v.norm_sqr()).collect();
            let got: Vec<f64> = y[c].iter().map(|v| v.norm_sqr()).collect();
            let want = spectrum::smooth(&want, self.smooth_lines);
            let got = spectrum::smooth(&got, self.smooth_lines);
            for f in 0..want.len() {
                if !self.mask[f] || want[f] <= 0.0 || got[f] <= 0.0 {
                    continue;
                }
                // Half the log power ratio is the log magnitude ratio.
                let step = (0.5 * self.weight * (want[f] / got[f]).ln()).clamp(-lim, lim);
                self.drive[p][f] = d[p][f] * step.exp();
            }
        }
        // Build the next pulse at the level the controller scheduled.
        self.play(level_scale(inputs.next_level_db))
    }
}

impl Paired {
    /// Every drive's record at amplitude scale `s`, with the drive rms
    /// referred to full level as the one reported value.
    fn play(&self, s: f64) -> Result<Outputs, String> {
        let drives = spectrum::rows_irfft(&self.drive, self.record_len, s)?;
        let rms = spectrum::rms_all(&drives) / s;
        Ok(Outputs {
            drives,
            delays_s: None,
            values: vec![("drive_rms_full_level".into(), rms)],
        })
    }
}

export_shock_mimo_law!(Paired);

#[cfg(test)]
mod tests;
