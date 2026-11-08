//! The clock the time between pulses is kept on.
//!
//! A closed-loop SRS run starts a pulse no sooner than the rate model's time
//! between pulses after the last one started. The service is synchronous: the
//! wait happens inside [`crate::SrsShockService::fire_pulse`], so the clock is
//! injectable or every spacing test would cost its spacing in wall time.
//! [`RealPacer`] is what ships; [`VirtualPacer`] advances its clock without
//! sleeping, so a test or an instrument can read a run's timeline instantly.
//!
//! Cancellation does not live here: the service polls its cancel flag around
//! these calls, so the pacer stays a clock.

use std::time::{Duration, Instant};

/// A monotonic clock pulses are paced against. Only differences between
/// [`now_s`](PulsePacer::now_s) values mean anything.
pub trait PulsePacer: Send {
    /// Seconds since this pacer's own epoch. Never decreases.
    fn now_s(&mut self) -> f64;

    /// Block until [`now_s`](PulsePacer::now_s) would return at least
    /// `deadline_s`. A deadline already past returns at once: an overrun is
    /// not repaid by a shorter next gap.
    fn sleep_until(&mut self, deadline_s: f64);

    /// The timestamp recorded for the start of a pulse, read once the device's
    /// clock is running (`PulsePlant::fire_timed`), so the time between pulses
    /// holds between the device's own starts. A virtual clock charges itself
    /// the time the rest of the fire would take here; a real one just reads the
    /// clock.
    fn pulse_start_s(&mut self) -> f64 {
        self.now_s()
    }
}

/// The shipping pacer: [`Instant`] and [`std::thread::sleep`].
#[derive(Debug)]
pub struct RealPacer {
    epoch: Instant,
}

impl RealPacer {
    /// A pacer whose epoch is now.
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

impl PulsePacer for RealPacer {
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

/// A pacer whose clock is a variable: `sleep_until` jumps to the deadline.
/// Each [`pulse_start_s`](PulsePacer::pulse_start_s) charges `fire_cost_s` to
/// the clock afterwards, standing in for the time a fire takes.
#[derive(Debug, Default)]
pub struct VirtualPacer {
    now: f64,
    fire_cost_s: f64,
    /// Every pulse start, in order.
    pub starts: Vec<f64>,
}

impl VirtualPacer {
    /// A pacer at t = 0 where each fire costs `fire_cost_s`.
    pub fn new(fire_cost_s: f64) -> Self {
        VirtualPacer {
            now: 0.0,
            fire_cost_s,
            starts: Vec::new(),
        }
    }
}

impl PulsePacer for VirtualPacer {
    fn now_s(&mut self) -> f64 {
        self.now
    }

    fn sleep_until(&mut self, deadline_s: f64) {
        if deadline_s > self.now {
            self.now = deadline_s;
        }
    }

    fn pulse_start_s(&mut self) -> f64 {
        let t = self.now;
        self.starts.push(t);
        self.now += self.fire_cost_s;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_pacer_waits_and_a_past_deadline_returns_at_once() {
        let mut p = RealPacer::new();
        let t0 = p.now_s();
        p.sleep_until(t0 + 0.010);
        assert!(p.now_s() - t0 >= 0.010);
        let before = p.now_s();
        p.sleep_until(0.0);
        assert!(p.now_s() >= before);
    }

    #[test]
    fn the_virtual_pacer_jumps_and_charges_only_pulse_starts() {
        let mut v = VirtualPacer::new(0.1);
        assert_eq!(v.pulse_start_s(), 0.0);
        assert!((v.now_s() - 0.1).abs() < 1e-12);
        v.sleep_until(1.0);
        assert_eq!(v.now_s(), 1.0);
        v.sleep_until(0.5);
        assert_eq!(v.now_s(), 1.0, "a past deadline does not rewind");
        assert_eq!(v.pulse_start_s(), 1.0);
        assert_eq!(v.starts, vec![0.0, 1.0]);
    }
}
