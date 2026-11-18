//! What a MIMO Sine law sees on one update ([`TickInputs`]) and returns
//! ([`TickOutputs`]), and the checks the core makes of the answer.
//!
//! Mirrors `sine-mimo-inputs` and `sine-mimo-outputs` of
//! `wit/control-law/sine-mimo.wit`. Complex amplitudes follow the world's
//! convention: drive `i` plays `Re{ D_i exp(j phi(t)) }`, and a channel's
//! tracked amplitude is its response's phase minus the tone's, so a linear
//! structure in steady state gives `C = H D`.
//!
//! The inputs borrow the core's buffers and the outputs are a buffer the core
//! owns, sized once for the run and reset before each call, so a native law
//! allocates nothing per update.

use hertzforge_sdm::C64;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Why the core holds the drive this update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Nothing holds it: the law's drive is used, within the core's guards.
    Free,
    /// A limit channel holds the drive down: the core judges the controls
    /// against the reference times `notch_scale` and scales the whole drive
    /// vector to keep the limit channels at their bounds.
    Notch,
    /// The core granted the law's request to hold the sweep.
    Held,
    /// The core plays a fixed drive to measure the structure; the law's output
    /// is not used.
    Probe,
    /// The settle window after engage: the tracking filters are still rising,
    /// so a correction would act on readings that have not settled.
    Engage,
}

impl Hold {
    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Hold::Free => "free",
            Hold::Notch => "notch",
            Hold::Held => "held",
            Hold::Probe => "probe",
            Hold::Engage => "engage",
        }
    }
}

/// What a law may ask of the sweep. Recorded always; honoured only when the
/// run's setup allows requests, within its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "factor", rename_all = "snake_case")]
pub enum Request {
    /// Keep the frequency where it is for this update.
    Hold,
    /// Sweep at this share of the scheduled rate (above 0, at most 1).
    Slow(f64),
}

/// Everything a MIMO Sine law sees on one update.
#[derive(Debug, Clone, Copy)]
pub struct TickInputs<'a> {
    /// Updates since the law was built: 0 at `engage`.
    pub index: u64,
    /// Seconds since the last call.
    pub dt_s: f64,
    /// The tone's frequency this update, Hz.
    pub frequency_hz: f64,
    /// The sweep rate, Hz per second, signed: 0 at a dwell, at a step or
    /// while held.
    pub sweep_hz_per_s: f64,
    /// The same rate in octaves per minute, signed.
    pub sweep_oct_per_min: f64,
    /// The scheduled level as a fraction of full level.
    pub level: f64,
    /// Every control's reference at this frequency and level, EU pk.
    pub reference: &'a [C64],
    /// Every control's tracked amplitude, EU pk.
    pub controls: &'a [C64],
    /// Every limit channel's tracked amplitude, in its units, peak.
    pub limits: &'a [C64],
    /// Every limit channel's bound at this frequency; `None` where it has
    /// none.
    pub limit_bounds: &'a [Option<f64>],
    /// The drive in force, volts peak, as played after every guard. Zero on
    /// every drive at `engage`.
    pub drive_in_force: &'a [C64],
    pub hold: Hold,
    /// The backstop notch's depth, 0 to 1 (1 when it is out): the share of
    /// the reference the controls are judged against while it is in. A law
    /// aims for the reference times it, so it never pushes against the notch.
    pub notch_scale: f64,
    /// Whether the core's settle window is open.
    pub settling: bool,
}

/// One update's answer: a buffer the core sizes for the run and resets before
/// each call ([`TickOutputs::reset`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TickOutputs {
    /// The drive, volts peak: one complex amplitude per drive.
    pub drive: Vec<C64>,
    /// The law's own judgement that it is faulted. Shown and recorded; the
    /// core never relies on it.
    pub faulted: bool,
    /// A request to hold or slow the sweep.
    pub request: Option<Request>,
    /// Whether the law returns a notched reference this update.
    pub notched: bool,
    /// The notched reference, one per control, used when `notched` is set:
    /// lower than the reference, at this frequency and level.
    pub notched_reference: Vec<C64>,
}

impl TickOutputs {
    /// A buffer for `drives` drives and `controls` controls, zero throughout.
    pub fn new(drives: usize, controls: usize) -> Self {
        TickOutputs {
            drive: vec![C64::new(0.0, 0.0); drives],
            faulted: false,
            request: None,
            notched: false,
            notched_reference: vec![C64::new(0.0, 0.0); controls],
        }
    }

    /// Clear everything but the sizes, before a call.
    pub fn reset(&mut self) {
        self.drive.fill(C64::new(0.0, 0.0));
        self.faulted = false;
        self.request = None;
        self.notched = false;
        self.notched_reference.fill(C64::new(0.0, 0.0));
    }
}

/// Which rule a refused answer broke. Each is a miss with the drive in force
/// held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRule {
    /// Not one complex amplitude per drive.
    WrongDriveCount,
    /// A drive, a value or a request's factor that is not a finite number.
    NonFinite,
    /// More values than the most a law may report.
    TooManyValues,
    /// A value under a key the manifest does not declare.
    UndeclaredValue,
    /// A slow request whose factor is not above 0 and at most 1.
    BadRequest,
    /// A notched reference that is not one value per control.
    WrongReferenceCount,
}

impl OutputRule {
    /// The stable wire and record name.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputRule::WrongDriveCount => "wrong_drive_count",
            OutputRule::NonFinite => "non_finite",
            OutputRule::TooManyValues => "too_many_values",
            OutputRule::UndeclaredValue => "undeclared_value",
            OutputRule::BadRequest => "bad_request",
            OutputRule::WrongReferenceCount => "wrong_reference_count",
        }
    }
}

impl fmt::Display for OutputRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OutputRule::WrongDriveCount => "the drive was not one value per drive",
            OutputRule::NonFinite => "the answer held a value that is not a finite number",
            OutputRule::TooManyValues => "the law reported more values than allowed",
            OutputRule::UndeclaredValue => "the law reported a value its manifest does not declare",
            OutputRule::BadRequest => "a slow request's factor was not above 0 and at most 1",
            OutputRule::WrongReferenceCount => {
                "the notched reference was not one value per control"
            }
        })
    }
}

/// Why a law call was not used. Every one is a miss.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawFault {
    /// The law reported that it could not compute an answer (or could not be
    /// built).
    Failed(String),
    /// The law panicked (a native law) or trapped (a law file).
    Crashed(String),
    /// The call took longer than its budget.
    OverBudget {
        /// How long the call took, seconds.
        took_s: f64,
        /// The budget, seconds.
        budget_s: f64,
    },
    /// A law file asked for more memory than its cap.
    OverMemory,
    /// The core refused the answer.
    BadOutput(OutputRule),
}

impl LawFault {
    /// The stable wire and record name: `error`, `panicked`, `over_budget`,
    /// `over_memory`, or the broken [`OutputRule`]'s name.
    pub fn reason(&self) -> &'static str {
        match self {
            LawFault::Failed(_) => "error",
            LawFault::Crashed(_) => "panicked",
            LawFault::OverBudget { .. } => "over_budget",
            LawFault::OverMemory => "over_memory",
            LawFault::BadOutput(r) => r.as_str(),
        }
    }
}

impl fmt::Display for LawFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LawFault::Failed(m) => write!(f, "the law failed: {m}"),
            LawFault::Crashed(m) => write!(f, "the law crashed: {m}"),
            LawFault::OverBudget { took_s, budget_s } => write!(
                f,
                "the call took {:.3} ms, over its {:.3} ms budget",
                took_s * 1e3,
                budget_s * 1e3
            ),
            LawFault::OverMemory => f.write_str("the law asked for more memory than its cap"),
            LawFault::BadOutput(r) => write!(f, "the answer was refused: {r}"),
        }
    }
}

impl std::error::Error for LawFault {}

fn finite(c: &C64) -> bool {
    c.re.is_finite() && c.im.is_finite()
}

/// Check an answer for a run of `drives` drives and `controls` controls: one
/// finite drive per drive, a slow request's factor above 0 and at most 1, and
/// a notched reference, when given, of one finite value per control. The
/// first rule broken is named. The values are checked as they are reported
/// (the native law's value buffer refuses an undeclared key; the law host
/// names too many or undeclared values of a law file).
pub fn check_outputs(out: &TickOutputs, drives: usize, controls: usize) -> Result<(), OutputRule> {
    if out.drive.len() != drives {
        return Err(OutputRule::WrongDriveCount);
    }
    if !out.drive.iter().all(finite) {
        return Err(OutputRule::NonFinite);
    }
    match out.request {
        Some(Request::Slow(f)) if !f.is_finite() => return Err(OutputRule::NonFinite),
        Some(Request::Slow(f)) if !(f > 0.0 && f <= 1.0) => return Err(OutputRule::BadRequest),
        _ => {}
    }
    if out.notched {
        if out.notched_reference.len() != controls {
            return Err(OutputRule::WrongReferenceCount);
        }
        if !out.notched_reference.iter().all(finite) {
            return Err(OutputRule::NonFinite);
        }
    }
    Ok(())
}
