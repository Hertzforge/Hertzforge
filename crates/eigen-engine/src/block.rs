//! Block-size policy: ONE place that answers "how many frames per acquisition block,
//! and how deep is the raw ring".
//!
//! The block size is not a free parameter. It is REQUESTED through
//! [`eigen_hal::AcquisitionConfig::block_frames`], the backend answers with what
//! it will actually deliver in [`eigen_hal::ConfiguredActual::block_frames`], and
//! [`crate::EngineConfig::with_block`] derives the engine's `frames_per_tick` and
//! `ring_frames` from THAT answer. Nothing downstream invents a block size, and
//! nothing hard-codes one that the hardware never agreed to.
//!
//! ## Why the old hard-coded `frames_per_tick = 1` is gone
//!
//! Every module pinned the engine at one frame per tick with a comment citing the
//! `SimBackend` drive latch: `write_drive` kept only the last frame of a block and
//! `read_input` applied that one value across the whole block, so at N frames per
//! tick the plant saw 1 of every N drive samples. That was a real constraint and
//! the workaround was correct. A later change removed the latch (the Sim now buffers
//! the full per-frame stream), so the reason no longer exists and the comments
//! that cite it are false. What survives is a DIFFERENT, still-true reason to keep
//! the EIGEN-SIM plant path at one frame per tick, and it is named explicitly by
//! [`SIM_NATURAL_BLOCK_FRAMES`].

use eigen_hal::ConfiguredActual;

// The ACQUISITION-block half of the policy lives in `eigen-hal`, next to
// `AcquisitionConfig::block_frames`, because a block is a property of an
// acquisition and not of the engine - the direct-backend players (TWR / SRS /
// shock) request one with no `Engine` in the picture. Re-exported
// here so this module stays the single place an engine caller reads.
pub use eigen_hal::{
    default_block_frames, MAX_BLOCK_FRAMES, MIN_BLOCK_FRAMES, SIM_NATURAL_BLOCK_FRAMES,
    TARGET_BLOCK_SECONDS,
};

/// Raw-ring depth as a multiple of the block: the watchdog must be able to fall
/// this many blocks behind the acquisition before a frame is dropped.
pub const RING_BLOCKS: usize = 8;

/// Floor on the derived ring depth, so a tiny block still gets a usable tap.
pub const MIN_RING_FRAMES: usize = 256;

/// The jitter fraction that DISABLES the scheduling-health period-jitter guard.
///
/// A deterministic single-threaded test harness runs the whole engine loop as
/// fast as the CPU allows and then stops for the control DSP between segments, so
/// the wall-clock interval between ticks bears no relation to the sample cadence
/// and the jitter check flags every segment boundary. Harnesses set the guard to
/// this value EXPLICITLY, by name, so the disabling is visible at the call site
/// and can be grepped for. A hardware path must never use it: on real hardware
/// the tick cadence IS the sample cadence, and period jitter is the symptom the
/// guard exists to catch (a thermal throttle or an MMCSS budget overrun after
/// `arm()` already passed its latency gate).
pub const TEST_DISABLE_JITTER_GUARD: f64 = 1e12;

/// The per-tick COMPUTE budget a deterministic Sim harness runs under.
///
/// The engine's default budget is the TICK PERIOD, which is right on hardware
/// (processing must finish before the next block's data is due) and wrong for a
/// harness: at the EIGEN-SIM one-frame block the period is ~122 us, so a single OS
/// hiccup inside a `process` call that genuinely takes nanoseconds trips
/// `ComputeOverrun` and aborts the run. That was NOT a control fault, and every
/// Sim module grew a retry loop to paper over it - retries that later had to be
/// unwound. One second is far above any real per-tick cost on these fixtures and
/// far below anything that could hide a genuine compute problem.
///
/// Like [`TEST_DISABLE_JITTER_GUARD`], harnesses set this EXPLICITLY, by name, so
/// the relaxation is visible at the call site and greppable. A HARDWARE path must
/// leave `EngineConfig::compute_budget` at `None`.
pub const TEST_RELAX_COMPUTE_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

/// Raw-ring depth for a given block: at least [`RING_BLOCKS`] blocks, never below
/// [`MIN_RING_FRAMES`].
///
/// The engine ENFORCES this floor rather than trusting the caller: see
/// `Engine::new`, which auto-raises a short `ring_frames` and records that it did.
/// Auto-raise rather than refuse, because the cost of being wrong is asymmetric -
/// a ring one block too shallow drops raw frames the abort watchdog needed to
/// see, and refusing to construct an engine over it would take a live safety
/// monitor offline to punish a configuration mistake.
pub fn ring_frames_for(frames_per_tick: usize) -> usize {
    (frames_per_tick.max(1) * RING_BLOCKS).max(MIN_RING_FRAMES)
}

/// The block the engine will actually run, from what the backend configured.
/// `0` is coerced to 1: a zero-frame block is a backend contract violation, and
/// dividing by it downstream is worse than running one frame at a time.
pub fn frames_per_tick_from(actual: &ConfiguredActual) -> usize {
    actual.block_frames.max(1)
}

/// The longest a watchdog debounce is ever allowed to be, in SECONDS.
///
/// A ceiling, not a target. Whatever a module asks for, the sustain converts to
/// at most this much continuous over-limit signal before `emergency_stop`. 5 ms
/// is well under any mechanical response time of interest and far under the
/// physical abort latency the arm gate already bounds separately.
pub const MAX_WATCHDOG_SUSTAIN_SECONDS: f64 = 0.005;

/// Convert a watchdog debounce expressed in TIME into the frame count
/// `EngineConfig::watchdog_sustain` wants, at a given sample rate.
///
/// ## Why this function exists
///
/// `watchdog_sustain` is counted in RAW FRAMES: `ChannelLimiter::check_axis`
/// trips once a channel has been over its limit for that many CONSECUTIVE
/// frames. A frame count is therefore a TIME only relative to a sample rate, and
/// every per-module value in this repo (random 64, sine 16, recorder 4) was
/// chosen against the EIGEN-SIM plant's 65 536 Hz.
///
/// Carried onto hardware unchanged, the same number means something else. At
/// 51.2 kHz it is 1.28x longer; at a 1 kHz configuration - which nothing
/// forbids, since the rate is whatever the driver programmed - random's 64
/// frames is **64 ms** of continuous over-limit before the e-stop, a 65x silent
/// stretch of a safety debounce. That is precisely the failure pattern of
/// borrowing one mode's constant for another: the number never changed, so
/// nothing looked wrong.
///
/// Modules on a HARDWARE station state the debounce in seconds and call this;
/// EIGEN-SIM keeps its frame constants exactly as they were, because changing
/// Sim behaviour is not what this conversion is for.
///
/// Never returns 0 (that would disable the debounce and make one stray sample
/// an e-stop) and never exceeds [`MAX_WATCHDOG_SUSTAIN_SECONDS`]. A nonsense
/// rate collapses to 1 - spike-sensitive, the safe direction.
pub fn watchdog_sustain_frames(seconds: f64, fs_hz: f64) -> u32 {
    if !(fs_hz.is_finite() && fs_hz > 0.0 && seconds.is_finite()) || seconds <= 0.0 {
        return 1;
    }
    let capped = seconds.min(MAX_WATCHDOG_SUSTAIN_SECONDS);
    let frames = (capped * fs_hz).round();
    if !frames.is_finite() || frames < 1.0 {
        return 1;
    }
    frames.min(u32::MAX as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_block_is_about_twenty_milliseconds_in_the_plan_band() {
        // The 51.2 kHz delta-sigma class, target band 512-2048 frames.
        let b = default_block_frames(51_200.0);
        assert_eq!(b, 1024);
        assert!((512..=2048).contains(&b));
        let seconds = b as f64 / 51_200.0;
        assert!(
            (seconds - TARGET_BLOCK_SECONDS).abs() < 0.005,
            "{seconds} s is not near the {TARGET_BLOCK_SECONDS} s target"
        );
        // The 65,536 Hz default raw rate lands in the same band.
        assert!((512..=2048).contains(&default_block_frames(65_536.0)));
    }

    #[test]
    fn default_block_is_clamped_and_never_degenerate() {
        assert_eq!(default_block_frames(1000.0), MIN_BLOCK_FRAMES);
        assert_eq!(default_block_frames(10_000_000.0), MAX_BLOCK_FRAMES);
        // A nonsense rate must not produce a zero (or NaN-derived) block.
        assert_eq!(default_block_frames(0.0), MIN_BLOCK_FRAMES);
        assert_eq!(default_block_frames(f64::NAN), MIN_BLOCK_FRAMES);
        assert_eq!(default_block_frames(-1.0), MIN_BLOCK_FRAMES);
    }

    #[test]
    fn ring_depth_is_always_at_least_eight_blocks() {
        for fpt in [1usize, 8, 64, 512, 1024, 2048, 4096] {
            let r = ring_frames_for(fpt);
            assert!(
                r >= RING_BLOCKS * fpt,
                "ring {r} is under {RING_BLOCKS} x {fpt}"
            );
            assert!(r >= MIN_RING_FRAMES);
        }
        // A zero block cannot produce a zero ring (ring::channel panics on 0).
        assert!(ring_frames_for(0) >= MIN_RING_FRAMES);
    }

    /// THE core hazard: a frame-count debounce silently stretches when the
    /// rate drops. Stating it in seconds is what stops that.
    ///
    /// RED-MUTATION: return `(seconds * fs_hz).round() as u32` with no cap and no
    /// floor and the ceiling + degenerate-rate assertions red.
    #[test]
    fn a_time_stated_sustain_holds_its_meaning_across_rates() {
        // 1 ms, the random module's Sim-era 64 frames at 65 536 Hz.
        let ms = 0.001;
        assert_eq!(watchdog_sustain_frames(ms, 65_536.0), 66);
        assert_eq!(watchdog_sustain_frames(ms, 51_200.0), 51);
        assert_eq!(watchdog_sustain_frames(ms, 1_000.0), 1);
        // ...and at every one of those rates it is the SAME 1 ms, which the
        // frame-count constant was not:
        for fs in [65_536.0, 51_200.0, 25_600.0, 8_192.0] {
            let f = watchdog_sustain_frames(ms, fs) as f64;
            assert!(
                (f / fs - ms).abs() <= 1.0 / fs,
                "at {fs} Hz, {f} frames is {} s, not {ms}",
                f / fs
            );
        }
        // The frame count carried unchanged is what this replaces: 64 frames at
        // 1 kHz would have been 64 ms - 64x the intent.
        assert!(64.0 / 1_000.0 > 10.0 * ms);
    }

    #[test]
    fn a_sustain_is_never_zero_and_never_unbounded() {
        // Zero would make one stray sample an e-stop.
        assert_eq!(watchdog_sustain_frames(0.0, 51_200.0), 1);
        assert_eq!(watchdog_sustain_frames(-1.0, 51_200.0), 1);
        assert_eq!(watchdog_sustain_frames(1e-9, 51_200.0), 1);
        assert_eq!(watchdog_sustain_frames(f64::NAN, 51_200.0), 1);
        // A degenerate rate collapses to spike-sensitive, the SAFE direction.
        assert_eq!(watchdog_sustain_frames(0.001, 0.0), 1);
        assert_eq!(watchdog_sustain_frames(0.001, -1.0), 1);
        assert_eq!(watchdog_sustain_frames(0.001, f64::NAN), 1);
        // And the ceiling holds however long a caller asks for.
        let capped = watchdog_sustain_frames(10.0, 51_200.0);
        assert_eq!(
            capped,
            (MAX_WATCHDOG_SUSTAIN_SECONDS * 51_200.0).round() as u32
        );
        assert!((capped as f64) / 51_200.0 <= MAX_WATCHDOG_SUSTAIN_SECONDS + 1e-9);
    }

    #[test]
    fn frames_per_tick_never_comes_back_zero() {
        let actual = ConfiguredActual {
            actual_rate_hz: 51_200.0,
            block_frames: 0,
            group_delay_frames: None,
        };
        assert_eq!(frames_per_tick_from(&actual), 1);
    }
}
