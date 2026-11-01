//! The train CLOCK seam: what makes inter-pulse spacing real time instead of
//! bookkeeping.
//!
//! # Why this exists
//!
//! Until this seam landed, a pulse train's "spacing" was a number ADDED to a
//! `f64` field (`ClassicalShockService::run_clock_s`) after each pulse. Nothing
//! waited. On the Sim plant that was invisible; on the chassis it means a
//! 5-pulse gunfire burst at a 2.0 s rate fires five pulses back to back, as fast
//! as the fire path returns, and the rate model's cooling / settling / compute
//! floors - the entire point of the rate model - are enforced against a clock that
//! only ever agrees with itself.
//!
//! The test that "proved" the spacing was the same shape: it asserted that
//! consecutive `at_run_time_s` values differed by `realized_spacing_s`, on a
//! field whose only mutation is `run_clock_s += spacing_s`. That is
//! `(x + s) - x == s` - arithmetic, not evidence: a result that cannot fail is
//! not evidence.
//!
//! # The seam
//!
//! The service is SYNCHRONOUS - `fire_train_spec` blocks its caller for the
//! whole burst - so the clock has to be injectable or the spacing test costs its
//! own spacing in wall time (a 3-pulse 2.0 s train would add 4 s to the suite).
//! [`TrainPacer`] is that injection point: two methods, no cancellation, no
//! knowledge of pulses. [`RealPacer`] is what ships; [`VirtualPacer`] is what
//! the tests and the bench instruments drive, and it advances its clock without
//! sleeping so a wall-time assertion runs instantly.
//!
//! Cancellation deliberately does NOT live here. The service polls its
//! `should_continue` flag around these calls, so the pacer stays a clock and the
//! two concerns can be read separately.

use std::time::{Duration, Instant};

/// A monotonic clock the train loop can be paced against.
///
/// Only DIFFERENCES between [`now_s`](TrainPacer::now_s) values are meaningful;
/// the epoch belongs to the implementation. `Send` because the service is on its
/// way to being hosted off the caller's thread and a clock that
/// blocked that would have to be replaced immediately.
pub trait TrainPacer: Send {
    /// Seconds since this pacer's own epoch. Monotonic and non-decreasing.
    fn now_s(&mut self) -> f64;

    /// Block until [`now_s`](TrainPacer::now_s) would return at least
    /// `deadline_s`.
    ///
    /// A deadline ALREADY IN THE PAST returns immediately: an overrun is not a
    /// debt to be repaid by a shorter next gap. A pulse that took longer than
    /// its own spacing has already violated the rate model, and the honest
    /// response is to report the realized timeline, not to compress the
    /// following gaps trying to catch up.
    fn sleep_until(&mut self, deadline_s: f64);

    /// The timestamp recorded for the START of a pulse.
    ///
    /// Separate from [`now_s`](TrainPacer::now_s) for ONE reason: a virtual
    /// clock has to charge itself the wall time the fire that follows would have
    /// consumed, and it cannot do that from `now_s`, which the inter-pulse wait
    /// calls repeatedly. `RealPacer` cannot tell the two apart and does not try -
    /// the default body is `now_s()`.
    fn pulse_start_s(&mut self) -> f64 {
        self.now_s()
    }
}

/// The shipping pacer: `Instant` + `thread::sleep`.
#[derive(Debug)]
pub struct RealPacer {
    epoch: Instant,
}

impl RealPacer {
    pub fn new() -> Self {
        RealPacer {
            epoch: Instant::now(),
        }
    }
}

impl Default for RealPacer {
    fn default() -> Self {
        RealPacer::new()
    }
}

impl TrainPacer for RealPacer {
    fn now_s(&mut self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn sleep_until(&mut self, deadline_s: f64) {
        let remaining = deadline_s - self.epoch.elapsed().as_secs_f64();
        if remaining > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(remaining));
        }
    }
}

/// A pacer whose clock is a variable: `sleep_until` JUMPS to the deadline
/// instead of waiting for it.
///
/// This is the test and instrument double. It is deliberately public rather than
/// `#[cfg(test)]`: the same object makes an integration test's wall-time
/// assertion instant AND lets a bench instrument replay a train's timeline
/// without holding a chassis for the duration.
///
/// `fire_cost_s` is what makes it useful as a MODEL and not just a stub - each
/// [`pulse_start_s`](TrainPacer::pulse_start_s) charges that much to the clock
/// afterwards, standing in for the wall time a real fire consumes. Set it below
/// the spacing and the loop must sleep; set it above and it must NOT (the
/// overrun case above).
#[derive(Debug, Default)]
pub struct VirtualPacer {
    now: f64,
    fire_cost_s: f64,
    /// Every `sleep_until` deadline, in order - the record of what the loop
    /// asked the clock to wait for.
    pub waits: Vec<f64>,
}

impl VirtualPacer {
    /// A pacer starting at t = 0 where each modelled fire costs `fire_cost_s`.
    pub fn new(fire_cost_s: f64) -> Self {
        VirtualPacer {
            now: 0.0,
            fire_cost_s,
            waits: Vec::new(),
        }
    }
}

impl TrainPacer for VirtualPacer {
    /// FREE: reading the clock costs nothing, so the chunked inter-pulse wait
    /// can poll it as often as it likes without distorting the timeline.
    fn now_s(&mut self) -> f64 {
        self.now
    }

    fn sleep_until(&mut self, deadline_s: f64) {
        self.waits.push(deadline_s);
        if deadline_s > self.now {
            self.now = deadline_s;
        }
    }

    fn pulse_start_s(&mut self) -> f64 {
        let t = self.now;
        self.now += self.fire_cost_s;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_pacer_actually_waits() {
        let mut p = RealPacer::new();
        let t0 = p.now_s();
        p.sleep_until(t0 + 0.020);
        let t1 = p.now_s();
        assert!(
            t1 - t0 >= 0.020,
            "sleep_until must not return early: {t1} - {t0}"
        );
    }

    /// A deadline in the past is not a debt: `sleep_until` returns at once and
    /// the clock is NOT rewound.
    #[test]
    fn a_past_deadline_returns_immediately_and_does_not_rewind() {
        let mut p = RealPacer::new();
        std::thread::sleep(Duration::from_millis(5));
        let before = p.now_s();
        p.sleep_until(0.0);
        assert!(p.now_s() >= before, "the clock went backwards");

        let mut v = VirtualPacer::new(0.0);
        v.sleep_until(1.0);
        assert_eq!(v.now, 1.0);
        v.sleep_until(0.5);
        assert_eq!(v.now, 1.0, "a past deadline must not rewind the clock");
    }

    #[test]
    fn the_virtual_pacer_charges_the_fire_cost_only_to_pulse_starts() {
        let mut v = VirtualPacer::new(0.1);
        assert_eq!(v.pulse_start_s(), 0.0);
        // Reading the clock is free, however many times the wait polls it.
        assert!((v.now_s() - 0.1).abs() < 1e-12);
        assert!((v.now_s() - 0.1).abs() < 1e-12);
        assert!((v.pulse_start_s() - 0.1).abs() < 1e-12);
        assert!((v.now_s() - 0.2).abs() < 1e-12);
    }

    /// `RealPacer` does not distinguish the two clocks - the default body is
    /// what it uses, and the timestamps move only with real time.
    #[test]
    fn the_real_pacer_treats_pulse_start_as_an_ordinary_read() {
        let mut p = RealPacer::new();
        let a = p.pulse_start_s();
        let b = p.now_s();
        assert!(b >= a && b - a < 0.010, "no synthetic cost: {a} -> {b}");
    }
}
