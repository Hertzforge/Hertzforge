//! Per-line ratio correction: the math of a single-output Random control law.
//!
//! - **Seed.** The starting drive is the full-level target divided by the
//!   plant power gain, line by line: `drive[k] = target[k] / max(|H|^2[k],
//!   floor)`, with the floor at a thousandth of the in-band peak `|H|^2` so a
//!   deep anti-resonance cannot ask for an unbounded drive. Lines outside the
//!   band are zero.
//! - **Each loop.** Every in-band line whose coherence is at or above the
//!   floor moves its drive by `gain * 10 * log10(target_at_level /
//!   measured)` dB, limited to `+/- max_step_db`. Lines below the coherence
//!   floor hold their drive, and so do lines the limiter is holding down.
//! - **Level.** There is no separate level loop: each line is corrected
//!   toward the target at the scheduled level, so the level follows. A level
//!   change scales the whole drive at once by the power ratio of the new
//!   level to the old. The reported level command is the in-band drive power
//!   relative to the full-level seed, in dB.
//!
//! This crate is the law's math and its declaration (name, parameters,
//! diagnostics) and nothing else: no dependencies, plain slices in and plain
//! numbers out. The law file `laws/ratio` wraps it in the law interface, and
//! the controller's built-in native `ratio` law wraps it in the controller's
//! own law trait, so the two are one implementation. The controller applies
//! its own protections to the drive either returns.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// The law's name in its manifest.
pub const NAME: &str = "per-line ratio";
/// The law's version in its manifest.
pub const VERSION: &str = "0.1.0";
/// The law's author in its manifest.
pub const AUTHOR: &str = "Hertzforge contributors";
/// What the law does, for its manifest.
pub const SUMMARY: &str = "Moves each trusted in-band line's drive toward the target by a \
    fraction of the measured error in dB, limited per loop. Lines below the coherence floor \
    hold their drive. The level follows from the per-line correction; a level change scales \
    the whole drive at once.";

/// A real-number parameter the law declares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberParam {
    /// Stable key.
    pub key: &'static str,
    /// Name shown on the form.
    pub label: &'static str,
    /// Unit shown next to the box. May be empty.
    pub unit: &'static str,
    /// Help text for the form.
    pub help: &'static str,
    /// Smallest accepted value.
    pub min: f64,
    /// Largest accepted value.
    pub max: f64,
    /// How far one step of the box moves the value (a hint for the form).
    pub step: f64,
    /// The value used when none is given.
    pub default: f64,
}

/// A named number the law reports each loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticDecl {
    /// Stable key.
    pub key: &'static str,
    /// Name shown on a plot or table.
    pub label: &'static str,
    /// Unit.
    pub unit: &'static str,
    /// Help text.
    pub help: &'static str,
}

/// The parameters, in form order.
pub const PARAMS: &[NumberParam] = &[
    NumberParam {
        key: "gain",
        label: "Correction gain",
        unit: "",
        help: "The fraction of each line's measured error, in dB, corrected per loop. 1 \
               corrects the whole error at once; smaller values average out estimation noise \
               over several loops.",
        min: 0.0,
        max: 1.0,
        step: 0.05,
        default: 0.5,
    },
    NumberParam {
        key: "max_step_db",
        label: "Largest step per loop",
        unit: "dB",
        help: "The most any one line's drive may move in one loop.",
        min: 0.1,
        max: 12.0,
        step: 0.1,
        default: 2.0,
    },
    NumberParam {
        key: "coherence_floor",
        label: "Coherence floor",
        unit: "",
        help: "Lines whose coherence is below this hold their drive instead of chasing a \
               measurement that cannot be trusted.",
        min: 0.0,
        max: 1.0,
        step: 0.05,
        default: 0.5,
    },
];

/// The diagnostics: the counts behind [`Report::untrusted`] and
/// [`Report::clamped`].
pub const DIAGNOSTICS: &[DiagnosticDecl] = &[
    DiagnosticDecl {
        key: "untrusted_lines",
        label: "Untrusted lines",
        unit: "lines",
        help: "In-band lines held this loop because their coherence was below the floor.",
    },
    DiagnosticDecl {
        key: "clamped_lines",
        label: "Clamped lines",
        unit: "lines",
        help: "In-band lines whose correction was limited to the largest step this loop.",
    },
];

/// Smallest value a spectral density is taken to have when forming a ratio.
const TINY: f64 = 1e-30;
/// The seed's plant-gain floor, as a fraction of the in-band peak `|H|^2`.
const H_FLOOR_FRACTION: f64 = 1e-3;

/// The parameter values a law is built with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatioParams {
    /// Fraction of each line's error, in dB, corrected per loop.
    pub gain: f64,
    /// The most a line's drive may move in one loop, dB.
    pub max_step_db: f64,
    /// Lines with coherence below this hold their drive.
    pub coherence_floor: f64,
}

impl Default for RatioParams {
    /// The declared defaults.
    fn default() -> Self {
        RatioParams {
            gain: PARAMS[0].default,
            max_step_db: PARAMS[1].default,
            coherence_floor: PARAMS[2].default,
        }
    }
}

impl RatioParams {
    /// Set the parameter `key` to `value`, checked against its declaration.
    /// Leaves the values unchanged on an unknown key or an out-of-range value.
    pub fn set(&mut self, key: &str, value: f64) -> Result<(), String> {
        let Some(p) = PARAMS.iter().find(|p| p.key == key) else {
            return Err(format!("`{key}` is not a parameter of this law"));
        };
        if !value.is_finite() || value < p.min || value > p.max {
            return Err(format!(
                "{key} is {value}; it runs from {} to {}",
                p.min, p.max
            ));
        }
        match key {
            "gain" => self.gain = value,
            "max_step_db" => self.max_step_db = value,
            _ => self.coherence_floor = value,
        }
        Ok(())
    }
}

/// One loop's measurement, on the law's line grid.
#[derive(Debug, Clone, Copy)]
pub struct Measurement<'a> {
    /// The measured composite control PSD at the running level.
    pub measured_psd: &'a [f64],
    /// Estimated plant power gain `|H|^2` per line.
    pub h_mag2: &'a [f64],
    /// Ordinary coherence per line, 0 to 1.
    pub coherence: &'a [f64],
    /// Lines the limiter is holding down. Empty means none; otherwise one per
    /// line.
    pub notched_bins: &'a [bool],
    /// Line spacing, Hz.
    pub df_hz: f64,
}

/// What one call did besides moving the drive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// In-band lines held because their coherence was below the floor.
    pub untrusted: Vec<usize>,
    /// In-band lines whose correction was limited to the largest step.
    pub clamped: Vec<usize>,
    /// The RMS over the band of `10 * log10(measured / target_at_level)`, dB,
    /// when there was a measurement.
    pub inband_err_db: Option<f64>,
    /// The measurement's in-band RMS, when there was a measurement.
    pub inband_rms: Option<f64>,
}

/// The law's state for one run.
#[derive(Debug, Clone)]
pub struct RatioLaw {
    target: Vec<f64>,
    lo: usize,
    hi: usize,
    params: RatioParams,
    /// The full-level seed drive: the reference the level command is read
    /// against.
    reference: Vec<f64>,
    drive: Vec<f64>,
    /// The scheduled level as an amplitude ratio to the full target.
    level_scale: f64,
}

impl RatioLaw {
    /// Build the law for a run: the full-level `target` PSD on the line grid,
    /// the half-open in-band window `band`, and the line spacing.
    pub fn new(
        target: Vec<f64>,
        band: (usize, usize),
        df_hz: f64,
        params: RatioParams,
    ) -> Result<Self, String> {
        let n = target.len();
        let (lo, hi) = band;
        if lo >= hi || hi > n {
            return Err(format!("band [{lo}, {hi}) does not fit {n} lines"));
        }
        let full_rms = (target[lo..hi].iter().sum::<f64>() * df_hz).max(0.0).sqrt();
        if full_rms <= 0.0 || full_rms.is_nan() {
            return Err("the target has no in-band energy".into());
        }
        Ok(RatioLaw {
            target,
            lo,
            hi,
            params,
            reference: vec![0.0; n],
            drive: vec![0.0; n],
            level_scale: 1.0,
        })
    }

    /// The drive proposed by the last call, one value per line.
    pub fn drive(&self) -> &[f64] {
        &self.drive
    }

    /// The level the law is commanding: the in-band drive power relative to
    /// the full-level seed, dB.
    pub fn level_cmd_db(&self) -> f64 {
        let reference: f64 = self.reference[self.lo..self.hi].iter().sum();
        let drive: f64 = self.drive[self.lo..self.hi].iter().sum();
        if reference > 0.0 && drive > 0.0 {
            10.0 * (drive / reference).log10()
        } else {
            20.0 * self.level_scale.log10()
        }
    }

    fn check(&self, m: &Measurement) -> Result<(), String> {
        let n = self.target.len();
        if m.measured_psd.len() != n
            || m.h_mag2.len() != n
            || m.coherence.len() != n
            || (!m.notched_bins.is_empty() && m.notched_bins.len() != n)
        {
            return Err(format!("the loop inputs are not on the {n}-line grid"));
        }
        Ok(())
    }

    fn level_power(&self) -> f64 {
        self.level_scale * self.level_scale
    }

    /// The starting drive, from the system-identification measurement (its
    /// `measured_psd` is not used).
    pub fn seed(&mut self, m: &Measurement) -> Result<Report, String> {
        self.check(m)?;
        let peak = m.h_mag2[self.lo..self.hi]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        if peak <= 0.0 || peak.is_nan() {
            return Err("the plant estimate is zero across the band".into());
        }
        let floor = peak * H_FLOOR_FRACTION;
        let p = self.level_power();
        let mut untrusted = Vec::new();
        for k in self.lo..self.hi {
            self.reference[k] = self.target[k] / m.h_mag2[k].max(floor);
            self.drive[k] = self.reference[k] * p;
            if m.coherence[k] < self.params.coherence_floor {
                untrusted.push(k);
            }
        }
        Ok(Report {
            untrusted,
            ..Report::default()
        })
    }

    /// One control loop: correct each trusted, unheld in-band line toward the
    /// target at the scheduled level.
    pub fn iterate(&mut self, m: &Measurement) -> Result<Report, String> {
        self.check(m)?;
        let p = self.level_power();
        let mut untrusted = Vec::new();
        let mut clamped = Vec::new();
        for k in self.lo..self.hi {
            if m.coherence[k] < self.params.coherence_floor {
                untrusted.push(k);
                continue;
            }
            if m.notched_bins.get(k).copied().unwrap_or(false) {
                continue;
            }
            let target = (self.target[k] * p).max(TINY);
            let measured = m.measured_psd[k].max(TINY);
            let mut step_db = self.params.gain * 10.0 * (target / measured).log10();
            if step_db > self.params.max_step_db {
                step_db = self.params.max_step_db;
                clamped.push(k);
            } else if step_db < -self.params.max_step_db {
                step_db = -self.params.max_step_db;
                clamped.push(k);
            }
            self.drive[k] *= 10f64.powf(step_db / 10.0);
        }
        let mut sq = 0.0;
        for k in self.lo..self.hi {
            let e = 10.0 * (m.measured_psd[k].max(TINY) / (self.target[k] * p).max(TINY)).log10();
            sq += e * e;
        }
        let inband_err_db = (sq / (self.hi - self.lo) as f64).sqrt();
        let inband_rms = (m.measured_psd[self.lo..self.hi].iter().sum::<f64>() * m.df_hz)
            .max(0.0)
            .sqrt();
        Ok(Report {
            untrusted,
            clamped,
            inband_err_db: Some(inband_err_db),
            inband_rms: Some(inband_rms),
        })
    }

    /// The scheduled level moved to `level_db` (dB re the full target): scale
    /// the whole drive by the power ratio of the new level to the old.
    pub fn retarget(&mut self, level_db: f64) -> Result<Report, String> {
        let scale = 10f64.powf(level_db / 20.0);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(format!("a level of {level_db} dB is not a usable level"));
        }
        let ratio = (scale / self.level_scale).powi(2);
        for d in &mut self.drive[self.lo..self.hi] {
            *d *= ratio;
        }
        self.level_scale = scale;
        Ok(Report::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn law() -> RatioLaw {
        RatioLaw::new(
            vec![0.0, 1.0, 1.0, 1.0, 0.0],
            (1, 4),
            1.0,
            RatioParams::default(),
        )
        .unwrap()
    }

    fn m<'a>(measured: &'a [f64], h: &'a [f64], coh: &'a [f64]) -> Measurement<'a> {
        Measurement {
            measured_psd: measured,
            h_mag2: h,
            coherence: coh,
            notched_bins: &[],
            df_hz: 1.0,
        }
    }

    #[test]
    fn the_seed_inverts_the_plant_in_band_only() {
        let mut l = law();
        let h = [9.0, 2.0, 4.0, 0.5, 9.0];
        let coh = [1.0, 1.0, 0.1, 1.0, 1.0];
        let r = l.seed(&m(&[0.0; 5], &h, &coh)).unwrap();
        assert_eq!(l.drive(), &[0.0, 0.5, 0.25, 2.0, 0.0]);
        assert_eq!(r.untrusted, vec![2]);
        assert!(l.level_cmd_db().abs() < 1e-12);
    }

    #[test]
    fn a_loop_moves_by_a_fraction_of_the_error_within_the_step() {
        let mut l = law();
        let ones = [1.0; 5];
        l.seed(&m(&[0.0; 5], &ones, &ones)).unwrap();
        // Line 1 reads 1 dB low: +0.5 dB. Line 2 reads 10 dB high: -5 dB asked,
        // held to -2 dB. Line 3 on target: unchanged.
        let measured = [0.0, 10f64.powf(-0.1), 10.0, 1.0, 0.0];
        let r = l.iterate(&m(&measured, &ones, &ones)).unwrap();
        assert!((l.drive()[1] - 10f64.powf(0.05)).abs() < 1e-12);
        assert!((l.drive()[2] - 10f64.powf(-0.2)).abs() < 1e-12);
        assert_eq!(l.drive()[3], 1.0);
        assert_eq!(r.clamped, vec![2]);
    }

    #[test]
    fn a_level_change_scales_the_whole_drive() {
        let mut l = law();
        let ones = [1.0; 5];
        l.seed(&m(&[0.0; 5], &ones, &ones)).unwrap();
        l.retarget(-6.0).unwrap();
        assert!((l.level_cmd_db() + 6.0).abs() < 1e-12);
        assert!(l.retarget(f64::NAN).is_err());
    }

    #[test]
    fn parameters_are_checked_against_their_declaration() {
        let mut p = RatioParams::default();
        assert_eq!(p.gain, 0.5);
        assert!(p.set("gain", 1.5).is_err());
        assert!(p.set("surprise", 0.1).is_err());
        assert_eq!(p, RatioParams::default(), "a refused value changes nothing");
        p.set("max_step_db", 3.0).unwrap();
        assert_eq!(p.max_step_db, 3.0);
    }
}
