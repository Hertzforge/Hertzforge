//! The drive level ramp, advanced per OUTPUT BLOCK (first Random bench run on NI,
//! 2026-11-01, finding 2).
//!
//! The level used to walk once per control loop: the walker took one bounded step
//! per segment, and the step was applied to the NEXT segment's drive. At a 1.28 s
//! loop and a 3 s, 40 dB controlled stop that is a staircase of 17 dB steps, with
//! the whole loop after the request still at the old level: the DMM read 97 mV for
//! 1.3 s after Stop, then 13 mV, then 1.9 mV, then the floor. A -1 dB manual step
//! settled over three loops the same way.
//!
//! Here the level is a SHARED ramp. The control loop still runs at its own cadence
//! and synthesizes each segment's drive at the level the segment starts at; the
//! playback step scales every output block by the ramp's walk from that level, so
//! a level change or a stop moves the drive smoothly within the ramp time. The
//! controlled stop can be requested from another thread ([`DriveRamp::request_stop`])
//! and starts at the next output block instead of the next loop.
//!
//! Thread contract: the run thread owns the walk (the playback step advances it;
//! the service sets targets between segments). Any thread may request the stop.
//! A stop, once requested, is never undone: later targets are ignored.

use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
struct RampState {
    /// The level the drive is at, dB relative to full.
    level_db: f64,
    /// Where the walk is headed.
    target_db: f64,
    /// How fast it walks, dB per second of output.
    rate_db_per_s: f64,
    /// The level the drive was at when the controlled stop was requested.
    stop_from_db: Option<f64>,
}

/// The drive's level and its walk, shared between the control loop, the playback
/// step and a front end that may stop the run from another thread.
#[derive(Debug)]
pub struct DriveRamp {
    state: Mutex<RampState>,
    /// The controlled stop ramps this far below the level it starts from.
    stop_depth_db: f64,
    /// Over this many seconds. `0` = no controlled-stop ramp.
    stop_ramp_s: f64,
}

impl DriveRamp {
    /// A ramp at 0 dB, standing still, whose controlled stop walks `stop_depth_db`
    /// down over `stop_ramp_s` seconds.
    pub fn new(stop_depth_db: f64, stop_ramp_s: f64) -> Self {
        DriveRamp {
            state: Mutex::new(RampState {
                level_db: 0.0,
                target_db: 0.0,
                rate_db_per_s: 0.0,
                stop_from_db: None,
            }),
            stop_depth_db: stop_depth_db.max(0.0),
            stop_ramp_s: if stop_ramp_s.is_finite() {
                stop_ramp_s.max(0.0)
            } else {
                0.0
            },
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RampState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The level the drive is at now, dB relative to full.
    pub fn level_db(&self) -> f64 {
        self.lock().level_db
    }

    /// Where the walk is headed.
    pub fn target_db(&self) -> f64 {
        self.lock().target_db
    }

    /// Whether the level is still walking.
    pub fn moving(&self) -> bool {
        let s = self.lock();
        (s.level_db - s.target_db).abs() > 1e-9
    }

    /// The level the drive was at when the controlled stop was requested, once it
    /// has been.
    pub fn stop_requested(&self) -> Option<f64> {
        self.lock().stop_from_db
    }

    /// Whether this ramp has a controlled-stop ramp at all.
    pub fn stop_ramps(&self) -> bool {
        self.stop_ramp_s > f64::EPSILON && self.stop_depth_db > 0.0
    }

    /// Start the controlled stop at the next output block: walk `stop_depth_db`
    /// down from the level the drive is at now, over the stop ramp time. Returns
    /// the level it started from, or `None` when no stop ramp is configured (the
    /// caller then stops without one). Idempotent: a second request returns the
    /// first one's level and changes nothing.
    pub fn request_stop(&self) -> Option<f64> {
        if !self.stop_ramps() {
            return None;
        }
        let mut s = self.lock();
        if let Some(from) = s.stop_from_db {
            return Some(from);
        }
        s.stop_from_db = Some(s.level_db);
        s.target_db = s.level_db - self.stop_depth_db;
        s.rate_db_per_s = self.stop_depth_db / self.stop_ramp_s;
        Some(s.level_db)
    }

    /// Put the level at `level_db` now, standing still (engage, a change with no
    /// ramp, any non-driving state). Ignored once a stop is requested.
    pub(crate) fn set(&self, level_db: f64) {
        let mut s = self.lock();
        if s.stop_from_db.is_some() {
            return;
        }
        s.level_db = level_db;
        s.target_db = level_db;
        s.rate_db_per_s = 0.0;
    }

    /// Walk to `target_db` at `rate_db_per_s` from wherever the level is. Ignored
    /// once a stop is requested: a stop is never undone by a level change.
    pub(crate) fn ramp_to(&self, target_db: f64, rate_db_per_s: f64) {
        let mut s = self.lock();
        if s.stop_from_db.is_some() {
            return;
        }
        s.target_db = target_db;
        s.rate_db_per_s = rate_db_per_s.max(0.0);
        if s.rate_db_per_s <= 0.0 {
            s.level_db = target_db;
        }
    }

    /// The level after `seconds` more output with no new request.
    pub(crate) fn projected(&self, seconds: f64) -> f64 {
        let s = self.lock();
        walk(s.level_db, s.target_db, s.rate_db_per_s, seconds)
    }

    /// Advance the walk by `seconds` of output. Returns the level before and after.
    pub(crate) fn advance(&self, seconds: f64) -> (f64, f64) {
        let mut s = self.lock();
        let from = s.level_db;
        s.level_db = walk(s.level_db, s.target_db, s.rate_db_per_s, seconds);
        (from, s.level_db)
    }
}

fn walk(level: f64, target: f64, rate: f64, seconds: f64) -> f64 {
    let delta = target - level;
    let step = rate * seconds.max(0.0);
    if delta.abs() <= step || rate <= 0.0 && delta.abs() <= 1e-12 {
        target
    } else if rate <= 0.0 {
        level
    } else {
        level + step.copysign(delta)
    }
}

/// Scale one output block by the ramp's walk over it, relative to `base_db` (the
/// level the block's drive was synthesized at): the gain moves linearly in dB from
/// the level at the block's start to the level at its end, so no block carries a
/// step. Returns the level at the block's end and the sum of the power gains
/// (gain squared) applied over the block's samples.
pub(crate) fn scale_block(
    ramp: &DriveRamp,
    base_db: f64,
    fs_hz: f64,
    block: &mut [f32],
) -> (f64, f64) {
    let n = block.len();
    if n == 0 || fs_hz <= 0.0 {
        return (ramp.level_db(), 0.0);
    }
    let (l0, l1) = ramp.advance(n as f64 / fs_hz);
    if (l0 - base_db).abs() <= 1e-12 && (l1 - base_db).abs() <= 1e-12 {
        return (l1, n as f64);
    }
    if (l1 - l0).abs() <= 1e-12 {
        let g = 10f64.powf((l0 - base_db) / 20.0);
        let gf = g as f32;
        for x in block.iter_mut() {
            *x *= gf;
        }
        return (l1, g * g * n as f64);
    }
    let mut g2 = 0.0;
    for (i, x) in block.iter_mut().enumerate() {
        let l = l0 + (l1 - l0) * (i + 1) as f64 / n as f64;
        let g = 10f64.powf((l - base_db) / 20.0);
        g2 += g * g;
        *x = (f64::from(*x) * g) as f32;
    }
    (l1, g2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_walks_its_depth_over_its_time_from_where_it_was_asked() {
        let r = DriveRamp::new(40.0, 4.0);
        r.set(-6.0);
        assert_eq!(r.request_stop(), Some(-6.0));
        assert_eq!(
            r.request_stop(),
            Some(-6.0),
            "a second request changes nothing"
        );
        assert!((r.target_db() + 46.0).abs() < 1e-12);
        let (a, b) = r.advance(1.0);
        assert_eq!(a, -6.0);
        assert!((b + 16.0).abs() < 1e-9, "10 dB per second: {b}");
        r.advance(10.0);
        assert!((r.level_db() + 46.0).abs() < 1e-12 && !r.moving());
        r.set(0.0);
        r.ramp_to(0.0, 1.0);
        assert!(
            (r.level_db() + 46.0).abs() < 1e-12,
            "a stop is never undone"
        );
    }

    #[test]
    fn no_stop_ramp_means_no_stop_request() {
        let r = DriveRamp::new(40.0, 0.0);
        assert_eq!(r.request_stop(), None);
        assert_eq!(r.stop_requested(), None);
    }

    #[test]
    fn a_block_is_scaled_smoothly_from_its_synthesis_level() {
        let r = DriveRamp::new(40.0, 1.0);
        r.set(0.0);
        r.ramp_to(-20.0, 20.0); // 20 dB per second
        let mut block = vec![1.0f32; 1000];
        let (end, g2) = scale_block(&r, 0.0, 1000.0, &mut block); // one second
        assert!((end + 20.0).abs() < 1e-9);
        // The mean power gain of a 20 dB linear-in-dB fall: (1 - 0.01) / ln(100).
        let want = 0.99 / 100f64.ln();
        assert!(
            (g2 / 1000.0 - want).abs() < 2e-3,
            "{} vs {want}",
            g2 / 1000.0
        );
        assert!((f64::from(block[999]) - 0.1).abs() < 1e-6, "{}", block[999]);
        for w in block.windows(2) {
            assert!(w[1] <= w[0], "monotonic");
            assert!(f64::from(w[0] / w[1]).log10() * 20.0 < 0.03, "no step");
        }
        // Standing still at the synthesis level: untouched.
        let mut same = vec![0.5f32; 10];
        r.set(-20.0);
        scale_block(&r, -20.0, 1000.0, &mut same);
        assert!(same.iter().all(|&x| x == 0.5));
    }
}
