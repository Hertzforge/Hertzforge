//! What a law sees each update and what it returns.
//!
//! Conventions: a drive's amplitude `D_i` plays `d_i(t) = Re{D_i exp(j phi(t))}`
//! (volts peak); a channel's tracked amplitude `C_c` reads
//! `y_c(t) = Re{C_c exp(j phi(t))}`, so a linear structure in steady state gives
//! `C = H D` with `H` the identification's response over drive.

use crate::C64;

/// The controller's state of the drive, as the law is told it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hold {
    /// Nothing holds the loop.
    #[default]
    Free,
    /// The controller's backstop notch scaled the last drive.
    Notch,
    /// A hold the law asked for was granted.
    Held,
    /// The controller is probing.
    Probe,
    /// The engage settle window: the tracking filters are still rising, so a
    /// correction would act on readings that have not settled.
    Engage,
}

/// What a law may ask of the schedule. Recorded always; honoured only when
/// the controller allows it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Request {
    /// Nothing asked.
    #[default]
    None,
    /// Hold the frequency.
    Hold,
    /// Slow the sweep by a factor in (0, 1].
    Slow(f64),
}

/// One update's inputs. Slices are the law's counts long.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// Update index, 0 at engage.
    pub index: u64,
    /// Time since the last call, s.
    pub dt_s: f64,
    /// The tone's frequency now, Hz.
    pub f_hz: f64,
    /// Signed sweep rate, Hz/s (0 at a dwell or a hold).
    pub sweep_hz_s: f64,
    /// The same in octaves a minute.
    pub sweep_oct_min: f64,
    /// Scheduled level, fraction of full level.
    pub level: f64,
    /// Reference at this frequency and level, one per control.
    pub reference: &'a [C64],
    /// Tracked control amplitudes (zeros at engage).
    pub controls: &'a [C64],
    /// Tracked limit amplitudes.
    pub limits: &'a [C64],
    /// Limit bounds here.
    pub limit_bounds: &'a [f64],
    /// The drive in force, as played after every guard.
    pub drive_in_force: &'a [C64],
    /// The controller's hold state.
    pub hold: Hold,
    /// The backstop notch's depth: the share of the reference the controller
    /// judges the controls against while a limit channel holds the drive down
    /// (1 when no notch is in). A law aims for `reference x notch_scale`
    /// ([`aimed_reference`]) so it does not push against the notch, and its
    /// drive follows the notch out as the controller releases it.
    pub notch_scale: f64,
    /// Whether a settle window is open.
    pub settle_open: bool,
}

/// One update's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Outputs {
    /// One complex amplitude per drive, volts peak.
    pub drive: Vec<C64>,
    /// The law's own judgement that something is wrong (shown, never relied on).
    pub fault: bool,
    /// The declared diagnostics, in declaration order.
    pub values: Vec<f64>,
    /// A request to the schedule.
    pub request: Request,
}

impl Outputs {
    /// Outputs sized for `drives` and `values` declared diagnostics.
    pub fn new(drives: usize, values: usize) -> Outputs {
        Outputs {
            drive: vec![C64::new(0.0, 0.0); drives],
            fault: false,
            values: vec![0.0; values],
            request: Request::None,
        }
    }
}

/// A law's two steps. Every law in this crate implements it.
pub trait SineLaw {
    /// The first drive, after identification.
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String>;
    /// Every drive after it.
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String>;
    /// The names of the diagnostics it declares, in order.
    fn value_names(&self) -> &'static [&'static str];
}

/// The reference a law aims for this update: the reference times the notch
/// scale (see [`Inputs::notch_scale`]); the reference itself when no notch is
/// in.
pub fn aimed_reference(x: &Inputs) -> Vec<C64> {
    let s = if x.notch_scale.is_finite() {
        x.notch_scale.clamp(0.0, 1.0)
    } else {
        1.0
    };
    x.reference.iter().map(|r| r * s).collect()
}

/// The largest `|dB(C) - dB(R)|` over the controls.
pub fn worst_error_db(controls: &[C64], reference: &[C64]) -> f64 {
    controls
        .iter()
        .zip(reference)
        .map(|(c, r)| (20.0 * (c.norm().max(1e-300) / r.norm().max(1e-300)).log10()).abs())
        .fold(0.0, f64::max)
}
