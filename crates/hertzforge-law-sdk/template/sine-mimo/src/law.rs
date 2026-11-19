//! `pair-ratio`: each control paired with one drive, the drive's amplitude
//! following its control's ratio of wanted to measured amplitude.
//!
//! - **Pairs.** Control `i` is paired with drive `i`, for the first
//!   `min(controls, drives)` controls. A control without a drive is not
//!   controlled; a drive without a control is silent.
//! - **Engage.** Each paired drive starts at its control's reference over the
//!   identification's own path from that drive to that control, read at the
//!   identification line nearest the start frequency: `D_i = R_i / H_ii`.
//! - **Each update.** Each paired drive in force is multiplied by
//!   `(|R_i| / |C_i|)^gain`, at most `max_step_db` either way; its phase is
//!   left as it is. While the controller holds the drive (`notch`, `probe`, or
//!   the settle window after engage) the law returns the drive in force.
//!
//! The law knows nothing of the coupling between pairs: on a structure where
//! one drive moves several controls, the pairs work against each other. That
//! is what the built-in `matrix` law is for.

use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::Params;

/// The law's state.
pub struct PairRatio {
    gain: f64,
    max_step_db: f64,
    pairs: usize,
    drives: usize,
    grid: Vec<f64>,
    response: MatrixStack,
}

impl PairRatio {
    /// The identification's path from drive `i` to control `i` at the line
    /// nearest `f_hz`.
    fn own_path(&self, i: usize, f_hz: f64) -> C64 {
        let k = self
            .grid
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - f_hz).abs().total_cmp(&(b.1 - f_hz).abs()))
            .map_or(0, |(k, _)| k);
        let (_, _, cols) = self.response.dims();
        self.response.line(k)[i * cols + i]
    }
}

impl SineMimoLaw for PairRatio {
    fn describe() -> SineMimoManifest {
        ManifestBuilder::new("pair-ratio", "0.1.0")
            .summary(
                "Pairs control i with drive i and moves each paired drive's amplitude by the \
                 ratio of wanted to measured amplitude at its control, raised to a gain.",
            )
            .shapes(&ALL_SHAPES)
            .test_types(&[TestType::Dwell, TestType::Stepped])
            .amplitude_only(true)
            .number(
                "gain",
                "Gain",
                "",
                "The power the ratio of wanted to measured amplitude is raised to each update.",
                0.01,
                1.0,
                0.01,
                0.2,
            )
            .number(
                "max_step_db",
                "Largest step",
                "dB",
                "The most a drive's amplitude moves in one update.",
                0.1,
                6.0,
                0.1,
                1.0,
            )
            .value(
                "error_db",
                "Worst error",
                "dB",
                "The largest control error, either way.",
            )
            .build()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        Ok(PairRatio {
            gain: params.number("gain")?,
            max_step_db: params.number("max_step_db")?,
            pairs: setup.controls.min(setup.drives),
            drives: setup.drives,
            grid: setup.identification.frequency_hz,
            response: setup.identification.response,
        })
    }

    fn engage(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        let mut drive = vec![C64::new(0.0, 0.0); self.drives];
        for (i, d) in drive.iter_mut().enumerate().take(self.pairs) {
            let h = self.own_path(i, inputs.frequency_hz);
            if h.norm() > 0.0 {
                *d = inputs.reference[i] / h;
            }
        }
        Ok(Outputs::drive(drive))
    }

    fn tick(&mut self, inputs: &Inputs) -> Result<Outputs, String> {
        let mut out = Outputs::drive(inputs.drive_in_force.clone());
        let mut worst: f64 = 0.0;
        for i in 0..self.pairs {
            let (want, got) = (inputs.reference[i].norm(), inputs.controls[i].norm());
            if want <= 0.0 || got <= 0.0 {
                continue;
            }
            let error_db = 20.0 * (want / got).log10();
            worst = worst.max(error_db.abs());
            if matches!(inputs.hold, Hold::Free | Hold::Held) {
                let step_db = (self.gain * error_db).clamp(-self.max_step_db, self.max_step_db);
                out.drive[i] *= 10f64.powf(step_db / 20.0);
            }
        }
        out.values.push(("error_db".into(), worst));
        Ok(out)
    }
}
