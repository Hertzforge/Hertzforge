//! Runtime scheduling-health check.
//!
//! OS priority requests (macOS QoS, Windows MMCSS/REALTIME) are requests that can be
//! denied or throttled AFTER `arm()` passed its bench-measured latency gate: a thermal
//! throttle, a battery transition, or an MMCSS budget overrun degrades scheduling at
//! run time. This module timestamps the processing step and flags two distinct faults:
//!
//! * **Compute-budget overrun:** a single tick's processing took longer than the
//!   per-tick compute budget the run was armed on. If the processing step cannot keep
//!   up with the sample cadence, drive frames arrive late.
//! * **Period jitter:** the wall-clock interval between successive ticks strays beyond
//!   the armed tick period by more than an allowed jitter fraction, i.e. the RT thread
//!   is not being scheduled on cadence even if each tick's own compute is cheap.
//!
//! Both are reported as [`HealthStatus`]; the engine's policy (alarm in Delegate,
//! ramp-down in HostClosesLoop) lives in the scheduler, not here. Timing uses
//! `std::time::{Duration, Instant}` (monotonic) so it is host-clock-independent from the
//! sample timebase and needs no external crate.

use std::time::{Duration, Instant};

/// The armed budget the health monitor checks against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HealthBudget {
    /// Worst-case allowed processing time for a single tick.
    pub compute_budget: Duration,
    /// Nominal wall-clock period between ticks (frames_per_tick / sample_rate).
    pub tick_period: Duration,
    /// Allowed fractional slack on the period before jitter is a fault, e.g. 0.5 means
    /// a tick may arrive up to 1.5x the nominal period late before flagging.
    pub jitter_frac: f64,
}

impl HealthBudget {
    /// Derive a budget from the sample rate and frames-per-tick. The compute budget
    /// defaults to the full tick period (processing must finish before the next tick's
    /// data is due); callers may tighten it.
    pub fn from_rate(sample_rate_hz: f64, frames_per_tick: u32, jitter_frac: f64) -> Self {
        let period_s = if sample_rate_hz > 0.0 {
            frames_per_tick as f64 / sample_rate_hz
        } else {
            0.0
        };
        let period = Duration::from_secs_f64(period_s.max(0.0));
        HealthBudget {
            compute_budget: period,
            tick_period: period,
            jitter_frac: jitter_frac.max(0.0),
        }
    }

    pub fn with_compute_budget(mut self, budget: Duration) -> Self {
        self.compute_budget = budget;
        self
    }
}

/// What the health monitor found for one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Ok,
    /// The processing step exceeded its per-tick compute budget.
    ComputeOverrun,
    /// The wakeup cadence drifted beyond the allowed jitter (starved scheduling).
    PeriodJitter,
    /// Both faults on the same tick.
    OverrunAndJitter,
}

impl HealthStatus {
    pub fn is_fault(self) -> bool {
        !matches!(self, HealthStatus::Ok)
    }
}

/// Rolling scheduling-health monitor. Call [`SchedulingHealth::tick_start`] just before
/// the processing step and [`SchedulingHealth::tick_end`] right after; the returned
/// [`HealthStatus`] captures overrun and jitter for that tick.
pub struct SchedulingHealth {
    budget: HealthBudget,
    last_start: Option<Instant>,
    cur_start: Option<Instant>,
    faults: u64,
    ticks: u64,
    worst_compute: Duration,
}

impl SchedulingHealth {
    pub fn new(budget: HealthBudget) -> Self {
        SchedulingHealth {
            budget,
            last_start: None,
            cur_start: None,
            faults: 0,
            ticks: 0,
            worst_compute: Duration::ZERO,
        }
    }

    pub fn budget(&self) -> HealthBudget {
        self.budget
    }

    pub fn fault_count(&self) -> u64 {
        self.faults
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn worst_compute(&self) -> Duration {
        self.worst_compute
    }

    /// Mark the start of a tick's processing.
    pub fn tick_start(&mut self) {
        self.cur_start = Some(Instant::now());
    }

    /// Declare that the loop deliberately WAITED between processing ticks - an
    /// input-idle tick (the backend's read timed out with no frames ready).
    ///
    /// The wall-clock gap an idle wait creates is a property of the INPUT
    /// stream, not of thread scheduling, so the next processing tick must not
    /// read it as `PeriodJitter`. Clearing the baseline makes the next tick a
    /// fresh first tick, exactly like the run's own first.
    ///
    /// Why this exists (found in a 2026-08-08 review): the NI read
    /// timeout is 2.0 block periods (`READ_TIMEOUT_BLOCKS`) while the hardware
    /// jitter allowance is 1.5 periods (`jitter_frac` 0.5), so a single DAQmx
    /// read timeout made the FOLLOWING tick a guaranteed `PeriodJitter` fault
    /// -> `emergency_stop` on any hardware host-loop run. A latent abort in
    /// every module; Random never soaked long enough to hit it because its
    /// per-segment engine rebuild reset the baseline as a side effect.
    pub fn note_idle_wait(&mut self) {
        self.last_start = None;
    }

    /// Mark the end of a tick's processing and evaluate health for this tick.
    pub fn tick_end(&mut self) -> HealthStatus {
        let now = Instant::now();
        let start = self.cur_start.take().unwrap_or(now);
        let compute = now.saturating_duration_since(start);
        self.evaluate(start, compute)
    }

    /// Evaluate with an explicitly supplied start instant and compute duration. Exposed
    /// so tests (and a real thread that measures its own spans) can drive the monitor
    /// deterministically without sleeping.
    pub fn evaluate(&mut self, tick_start: Instant, compute: Duration) -> HealthStatus {
        self.ticks += 1;
        if compute > self.worst_compute {
            self.worst_compute = compute;
        }

        let overrun = compute > self.budget.compute_budget;

        let jitter = if let Some(prev) = self.last_start {
            let period = tick_start.saturating_duration_since(prev);
            let allowed = self
                .budget
                .tick_period
                .mul_f64(1.0 + self.budget.jitter_frac);
            // Only flag lateness (period too long => starved). An early tick is benign.
            period > allowed && !self.budget.tick_period.is_zero()
        } else {
            false
        };
        self.last_start = Some(tick_start);

        let status = match (overrun, jitter) {
            (false, false) => HealthStatus::Ok,
            (true, false) => HealthStatus::ComputeOverrun,
            (false, true) => HealthStatus::PeriodJitter,
            (true, true) => HealthStatus::OverrunAndJitter,
        };
        if status.is_fault() {
            self.faults += 1;
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_rate_computes_period() {
        // 64 frames at 64 kHz = 1 ms period.
        let b = HealthBudget::from_rate(64_000.0, 64, 0.5);
        assert_eq!(b.tick_period, Duration::from_micros(1000));
        assert_eq!(b.compute_budget, Duration::from_micros(1000));
    }

    #[test]
    fn within_budget_is_ok() {
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5); // 100 ms period
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        assert_eq!(h.evaluate(t0, Duration::from_millis(10)), HealthStatus::Ok);
        // Next tick on cadence (100 ms later) and cheap compute.
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(h.evaluate(t1, Duration::from_millis(10)), HealthStatus::Ok);
        assert_eq!(h.fault_count(), 0);
    }

    #[test]
    fn compute_overrun_flagged() {
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5); // 100 ms budget
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        assert_eq!(
            h.evaluate(t0, Duration::from_millis(150)),
            HealthStatus::ComputeOverrun
        );
        assert_eq!(h.fault_count(), 1);
        assert_eq!(h.worst_compute(), Duration::from_millis(150));
    }

    #[test]
    fn period_jitter_flagged_when_starved() {
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5); // 100 ms period, 1.5x allowed
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        h.evaluate(t0, Duration::from_millis(5));
        // Next tick arrives 200 ms later (>150 ms allowed): starved scheduling.
        let t1 = t0 + Duration::from_millis(200);
        assert_eq!(
            h.evaluate(t1, Duration::from_millis(5)),
            HealthStatus::PeriodJitter
        );
    }

    #[test]
    fn both_faults_on_one_tick() {
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5);
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        h.evaluate(t0, Duration::from_millis(5));
        let t1 = t0 + Duration::from_millis(300);
        assert_eq!(
            h.evaluate(t1, Duration::from_millis(200)),
            HealthStatus::OverrunAndJitter
        );
    }

    #[test]
    fn idle_wait_resets_the_period_baseline() {
        // A driver read timeout is a legitimate wait, not starved scheduling.
        // With the idle wait DECLARED, a 200 ms gap on a 100 ms/1.5x budget is
        // benign; the red half below is the same gap UNDECLARED, which must
        // still fault - deleting the note_idle_wait() call site turns the
        // green half red, so this pair is its own mutation check.
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5);
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        h.evaluate(t0, Duration::from_millis(5));
        h.note_idle_wait();
        let t1 = t0 + Duration::from_millis(200);
        assert_eq!(h.evaluate(t1, Duration::from_millis(5)), HealthStatus::Ok);

        // UNDECLARED: the identical gap is still a fault.
        let t2 = t1 + Duration::from_millis(200);
        assert_eq!(
            h.evaluate(t2, Duration::from_millis(5)),
            HealthStatus::PeriodJitter
        );
    }

    #[test]
    fn early_tick_is_not_jitter() {
        let budget = HealthBudget::from_rate(1000.0, 100, 0.5);
        let mut h = SchedulingHealth::new(budget);
        let t0 = Instant::now();
        h.evaluate(t0, Duration::from_millis(5));
        // Arrives EARLY (50 ms): benign.
        let t1 = t0 + Duration::from_millis(50);
        assert_eq!(h.evaluate(t1, Duration::from_millis(5)), HealthStatus::Ok);
    }
}
