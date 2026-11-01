//! Operator slew ceilings for a HOST-SYNTHESIZED drive, denominated PER FRAME.
//!
//! ## Two types, the same name, two crates - and that is deliberate
//!
//! The delegated Explorer pacer's `SlewLimits` carries a third field,
//! `tick_interval`, because a delegated drive is moved by NUDGE BURSTS: the pacer wakes on a
//! wall-clock tick, works out how many increment steps fit in the elapsed interval under
//! the slew rate, and sends that burst. The tick interval is the denominator of that
//! arithmetic, and it is a property of the PACER, not of the signal.
//!
//! A host-synthesized drive has no bursts and no pacer tick. The process step computes a
//! sample every frame, so the natural denominator is the FRAME, and the frame rate is the
//! station's ACTUAL sample rate - which is not the requested one (the NI-9234 coerces a
//! request up its 13.1072 MHz/(256n) ladder, so 8192 Hz becomes 8533.33 Hz). Baking any
//! interval into this type would re-create the class of bug the rate-honesty work exists
//! to remove: a slew computed against a rate the hardware is not running at.
//!
//! So this type carries the operator's INTENT only - octaves per second, dB per second -
//! and every per-frame increment is derived at the consumer from the actual rate it
//! adopted at configure time. Same name, because it is the same operator control; a
//! different type, because the arithmetic underneath it is different and silently sharing
//! one would let a tick-denominated value reach a per-frame consumer.

/// Slew ceilings for a host-synthesized drive. Both limits are the natural rate for their
/// axis: frequency in octaves/second, drive in dB/second (the standard sine-testing safety
/// rate).
///
/// Per-frame increments are NOT stored. Derive them from the station's ACTUAL sample rate
/// with [`octaves_per_frame`](Self::octaves_per_frame) /
/// [`freq_factor_per_frame`](Self::freq_factor_per_frame) and
/// [`db_per_frame`](Self::db_per_frame) / [`drive_factor_per_frame`](Self::drive_factor_per_frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlewLimits {
    /// Max frequency slew, octaves per second (default 1.0).
    pub freq_oct_per_s: f64,
    /// Max drive slew, dB per second (default 6.0).
    pub drive_db_per_s: f64,
}

impl Default for SlewLimits {
    fn default() -> Self {
        Self {
            freq_oct_per_s: 1.0,
            drive_db_per_s: 6.0,
        }
    }
}

impl SlewLimits {
    /// Octaves of frequency travel allowed in ONE frame at `fs_hz`. `fs_hz` must be the
    /// ACTUAL adopted sample rate, never the requested one.
    ///
    /// A non-positive or non-finite rate yields 0.0 - no travel - which is the safe
    /// direction: a frequency that cannot move is inconvenient, one that moves under an
    /// unknown rate is not bounded by anything.
    pub fn octaves_per_frame(&self, fs_hz: f64) -> f64 {
        if fs_hz.is_finite() && fs_hz > 0.0 {
            self.freq_oct_per_s / fs_hz
        } else {
            0.0
        }
    }

    /// The multiplicative frequency factor for ONE frame at `fs_hz`: `2^(oct/frame)`.
    /// Multiply the current frequency by this once per frame to travel at the ceiling.
    /// Returns exactly 1.0 (no travel) when the rate is unusable.
    pub fn freq_factor_per_frame(&self, fs_hz: f64) -> f64 {
        2f64.powf(self.octaves_per_frame(fs_hz))
    }

    /// Decibels of drive travel allowed in ONE frame at `fs_hz`. Same rate contract and
    /// same fail-to-zero behaviour as [`octaves_per_frame`](Self::octaves_per_frame).
    pub fn db_per_frame(&self, fs_hz: f64) -> f64 {
        if fs_hz.is_finite() && fs_hz > 0.0 {
            self.drive_db_per_s / fs_hz
        } else {
            0.0
        }
    }

    /// The multiplicative drive-amplitude factor for ONE frame at `fs_hz`:
    /// `10^(dB/frame / 20)`. Returns exactly 1.0 (no travel) when the rate is unusable.
    ///
    /// NOTE this is a RATIO, so it can never lift a drive off exactly zero. A ramp from
    /// the 0.000 V arm posture starts from a small floor, not from 0 - that is the
    /// consumer's decision and it belongs with the consumer's ceiling, not here.
    pub fn drive_factor_per_frame(&self, fs_hz: f64) -> f64 {
        10f64.powf(self.db_per_frame(fs_hz) / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_one_octave_and_six_db_per_second() {
        let s = SlewLimits::default();
        assert_eq!(s.freq_oct_per_s, 1.0);
        assert_eq!(s.drive_db_per_s, 6.0);
    }

    /// The whole point of the restatement: the per-frame increment tracks the ACTUAL
    /// rate. One second of frames at either rate travels the same OCTAVE, which is what
    /// the operator set.
    #[test]
    fn per_frame_increments_follow_the_actual_rate() {
        let s = SlewLimits::default();
        // The NI-9234 coerces a request of 8192 Hz up to 8533.33 Hz.
        let requested = 8192.0;
        let actual = 13_107_200.0 / (256.0 * 6.0); // 8533.33...
        assert!(s.octaves_per_frame(actual) < s.octaves_per_frame(requested));
        // A second of frames travels one octave at EITHER rate.
        let travel = |fs: f64| s.freq_factor_per_frame(fs).powf(fs);
        assert!((travel(requested) - 2.0).abs() < 1e-9);
        assert!((travel(actual) - 2.0).abs() < 1e-9);
        // Same for the drive: a second of frames is 6 dB.
        let gain_db = |fs: f64| 20.0 * s.drive_factor_per_frame(fs).powf(fs).log10();
        assert!((gain_db(requested) - 6.0).abs() < 1e-9);
        assert!((gain_db(actual) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn an_unusable_rate_yields_no_travel() {
        let s = SlewLimits::default();
        for fs in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(s.octaves_per_frame(fs), 0.0, "fs {fs}");
            assert_eq!(s.db_per_frame(fs), 0.0, "fs {fs}");
            assert_eq!(s.freq_factor_per_frame(fs), 1.0, "fs {fs}");
            assert_eq!(s.drive_factor_per_frame(fs), 1.0, "fs {fs}");
        }
    }
}
