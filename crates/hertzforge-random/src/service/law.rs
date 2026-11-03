//! The run core's side of the control-law seam: how a law is built, and the clock
//! its calls are timed on.
//!
//! The service holds whatever law a [`LawFactory`] builds as a
//! `Box<dyn ControlLaw + Send>`. The default factory builds the shipped
//! [`HinvLaw`] from the run's `HinvConfig`, exactly as the service did before the
//! seam existed. A test (and, later, a law loaded from a file) injects its own
//! factory with [`crate::RandomService::with_law_factory`].
//!
//! The law proposes; the core disposes. The guards that make any law safe to run
//! (output checks, the per-loop change cap, the core's own divergence count, the
//! time budget, panic containment, the controlled stop) live in the service, not
//! here, and apply to every law the factory can build.

use crate::control::{ControlError, ControlLaw, HinvConfig, HinvLaw};
use std::sync::Arc;
use std::time::Instant;

/// Everything a law factory is given to build a law for one run: the full-level
/// target on the line grid, the control band, the line spacing, the control
/// channel count, the time one law call may take, and the shipped law's knobs (a
/// law that has its own parameters ignores them).
#[derive(Debug, Clone, Copy)]
pub struct LawSetup<'a> {
    /// The FULL (0 dB) shaped target PSD on the line grid, zero outside the band.
    pub target_psd: &'a [f64],
    /// The half-open control band `[lo, hi)` in line-grid bins.
    pub band: (usize, usize),
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// The shipped law's configuration (`RandomTestConfig::hinv`).
    pub hinv: &'a HinvConfig,
    /// Control channels the composite is built from.
    pub control_channels: usize,
    /// The time one law call may take, seconds: `law_guards.budget_fraction` of
    /// one loop's acquisition time. The core measures every call against it; a
    /// law run contained gives its host the same number as the call's deadline,
    /// so the host stops a call that will not return at the moment the core
    /// would stop waiting for it.
    pub call_budget_s: f64,
}

impl LawSetup<'_> {
    /// Number of lines on the grid.
    pub fn nbins(&self) -> usize {
        self.target_psd.len()
    }
}

/// Builds the control law for a run. Called once per run, at system
/// identification, after the station has fixed the line grid.
pub trait LawFactory: Send + Sync {
    /// Build a law for `setup`, or say why it cannot be built (the run then
    /// refuses to engage, as it does for a bad law configuration).
    fn build(&self, setup: &LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError>;
}

impl<F> LawFactory for F
where
    F: Fn(&LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError> + Send + Sync,
{
    fn build(&self, setup: &LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError> {
        self(setup)
    }
}

/// The default factory: the shipped [`HinvLaw`], built from `setup.hinv` on the
/// run's target, band and line spacing.
#[derive(Debug, Clone, Copy, Default)]
pub struct HinvLawFactory;

impl LawFactory for HinvLawFactory {
    fn build(&self, setup: &LawSetup<'_>) -> Result<Box<dyn ControlLaw + Send>, ControlError> {
        let law = HinvLaw::new(
            setup.hinv.clone(),
            setup.target_psd,
            setup.band,
            setup.df_hz,
        )?;
        Ok(Box::new(law))
    }
}

/// The clock a law call is timed on. Monotonic seconds from an arbitrary origin.
///
/// Injectable so a test can make a law "slow" deterministically (the law advances
/// a test clock) instead of resting a pass or a failure on the wall clock.
pub trait LawClock: Send + Sync {
    /// Seconds since this clock's origin. Never decreases.
    fn now_s(&self) -> f64;
}

/// The production clock: [`std::time::Instant`] from the moment it was built.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    pub fn new() -> Self {
        MonotonicClock {
            origin: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl LawClock for MonotonicClock {
    fn now_s(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// The default law factory, shared.
pub(crate) fn default_factory() -> Arc<dyn LawFactory> {
    Arc::new(HinvLawFactory)
}

/// The default law clock, shared.
pub(crate) fn default_clock() -> Arc<dyn LawClock> {
    Arc::new(MonotonicClock::new())
}
