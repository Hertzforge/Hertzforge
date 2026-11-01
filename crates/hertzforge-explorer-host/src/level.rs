//! Device-to-host level scale fit for the resonance-profile line (2026-09-07).
//!
//! # Why this exists
//!
//! The delegated path used to record ONE profile point per telemetry chunk from a tracking
//! filter that was RESET whenever the demodulation frequency changed and then "settled"
//! inside the chunk. The decoded telemetry is one 64 KB block in EIGHT
//! (`SMART_DECODE_EVERY_N_READS`), so a chunk is ~1.9 ms of signal followed by ~13 ms of
//! nothing: the filter never settled inside a chunk after a frequency move, and every
//! point taken while the tone was moving read LOW. The operator saw the line pause until
//! the dwell was long enough, or under-report. The reference controller draws a smooth line through
//! coarse frequency blocks because it plots the DEVICE's own per-channel broadband RMS
//! (continuous, full rate, computed on the SHARC) times sqrt(2) at the current frequency
//! (the 2026-09-07 recording: CH1 1.36 g rms on the meter panel, 1.92 g-pk on the
//! profile, ratio 1.412).
//!
//! The device RMS is in the device's own unit (volts-like: `readout.rs` measured
//! drive-over-rms = sqrt(2) on the loopback), while the decoded samples - and therefore
//! the spectra the profile is drawn beside - are in the FE plane unit, which depends on
//! the input range. Nobody has pinned that ratio per range, so it is FITTED live:
//! [`ScaleFit`] keeps the running MEDIAN of `host / device` level ratios, taken only when
//! both are above a floor. The host reference is the service's rolling mean-removed RMS
//! of the decoded samples, which on the bursty hardware stream is unbiased except where
//! the tone period is close to a multiple of the burst period (~70 Hz) - the burst then
//! samples the same phase every time. The median rejects those outliers, which is also
//! why the device RMS carries the profile on hardware and the host RMS only calibrates
//! it. Until the first fit lands the scale is 1.0 and the status says so.

use std::collections::VecDeque;

/// Trailing mean-removed RMS of one channel over the last few CYCLES of the tone.
///
/// The level source for links with no device readout (the Sim): the window is sized
/// from the current frequency (`cycles / hz`), so it follows a moving tone with a lag
/// of a few cycles instead of the half-second the service's rolling amplitude window
/// carries. Continuous samples only - on the 1-in-8 hardware decode the device RMS
/// carries the line and this meter is not consulted.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    fs_hz: f64,
    ring: VecDeque<f64>,
    cap: usize,
}

impl LevelMeter {
    /// A meter at `fs_hz` keeping at most `max_window_s` seconds of samples.
    pub fn new(fs_hz: f64, max_window_s: f64) -> Self {
        let cap = ((fs_hz * max_window_s).round() as usize).max(4);
        Self {
            fs_hz,
            ring: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append samples (oldest first); the ring forgets past its capacity.
    pub fn push(&mut self, samples: impl IntoIterator<Item = f64>) {
        for v in samples {
            if self.ring.len() == self.cap {
                self.ring.pop_front();
            }
            self.ring.push_back(v);
        }
    }

    /// Mean-removed RMS over the last `cycles` cycles of a tone at `hz` (at least
    /// `min_samples`, at most the ring), or `None` while the ring holds fewer than
    /// `min_samples`.
    pub fn rms_at(&self, hz: f64, cycles: f64, min_samples: usize) -> Option<f64> {
        let min = min_samples.max(2);
        if self.ring.len() < min {
            return None;
        }
        let want = if hz.is_finite() && hz > 0.0 {
            (cycles.max(0.0) * self.fs_hz / hz).ceil() as usize
        } else {
            self.ring.len()
        };
        let n = want.clamp(min, self.ring.len());
        let start = self.ring.len() - n;
        let mean = self.ring.iter().skip(start).sum::<f64>() / n as f64;
        let ss = self
            .ring
            .iter()
            .skip(start)
            .map(|v| (v - mean) * (v - mean))
            .sum::<f64>();
        Some((ss / n as f64).sqrt())
    }

    /// The peak of a sine with [`Self::rms_at`]'s RMS (`sqrt 2 x rms`).
    pub fn amplitude_pk_at(&self, hz: f64, cycles: f64, min_samples: usize) -> Option<f64> {
        self.rms_at(hz, cycles, min_samples)
            .map(|r| r * std::f64::consts::SQRT_2)
    }

    /// Forget every sample (a run restart).
    pub fn reset(&mut self) {
        self.ring.clear();
    }
}

/// Running median of `reference / device` level ratios.
#[derive(Debug, Clone)]
pub struct ScaleFit {
    ratios: VecDeque<f64>,
    cap: usize,
    min_reference: f64,
    min_device: f64,
    min_count: usize,
}

impl ScaleFit {
    /// Keep the last `cap` ratios; ignore observations where either side is below its
    /// floor (noise-floor readings would fit noise, not a scale). `min_count` ratios are
    /// needed before [`Self::scale`] reports a fit.
    pub fn new(cap: usize, min_reference: f64, min_device: f64, min_count: usize) -> Self {
        Self {
            ratios: VecDeque::with_capacity(cap.max(1)),
            cap: cap.max(1),
            min_reference,
            min_device,
            min_count: min_count.max(1),
        }
    }

    /// Offer one simultaneous pair. Returns `true` if it was accepted into the fit.
    pub fn observe(&mut self, reference: f64, device: f64) -> bool {
        if !(reference.is_finite() && device.is_finite()) {
            return false;
        }
        if reference < self.min_reference || device < self.min_device || device <= 0.0 {
            return false;
        }
        if self.ratios.len() == self.cap {
            self.ratios.pop_front();
        }
        self.ratios.push_back(reference / device);
        true
    }

    /// The fitted `reference / device` scale (median), once `min_count` ratios are in.
    pub fn scale(&self) -> Option<f64> {
        if self.ratios.len() < self.min_count {
            return None;
        }
        let mut v: Vec<f64> = self.ratios.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = v.len() / 2;
        Some(if v.len().is_multiple_of(2) {
            0.5 * (v[mid - 1] + v[mid])
        } else {
            v[mid]
        })
    }

    /// How many ratios the fit currently holds.
    pub fn count(&self) -> usize {
        self.ratios.len()
    }

    /// Forget every ratio (a run restart or a range change).
    pub fn reset(&mut self) {
        self.ratios.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn cycle_window_rms_of_a_sine_is_amplitude_over_root_two() {
        let fs = 8192.0;
        let mut m = LevelMeter::new(fs, 0.5);
        let sig = (0..4096).map(|k| 0.3 + 2.0 * (TAU * 250.0 * k as f64 / fs).sin());
        m.push(sig);
        let pk = m.amplitude_pk_at(250.0, 3.0, 16).expect("ring filled");
        assert!((pk - 2.0).abs() < 0.06, "pk {pk} (DC removed, 3 cycles)");
    }

    #[test]
    fn a_level_change_is_followed_within_the_cycle_window() {
        let fs = 8192.0;
        let mut m = LevelMeter::new(fs, 0.5);
        m.push((0..4096).map(|k| 5.0 * (TAU * 250.0 * k as f64 / fs).sin()));
        // The tone drops to 1.0 for four cycles; a 3-cycle window must read ~1.0, not the
        // half-second average the rolling amplitude window would carry.
        let four_cycles = (4.0 * fs / 250.0) as usize;
        m.push((0..four_cycles).map(|k| 1.0 * (TAU * 250.0 * k as f64 / fs).sin()));
        let pk = m.amplitude_pk_at(250.0, 3.0, 16).unwrap();
        assert!((pk - 1.0).abs() < 0.05, "pk {pk}");
    }

    #[test]
    fn a_short_ring_reports_nothing_and_an_unknown_tone_uses_the_whole_ring() {
        let mut m = LevelMeter::new(1000.0, 0.01);
        assert!(m.rms_at(50.0, 2.0, 4).is_none());
        m.push([1.0, -1.0, 1.0, -1.0]);
        assert!((m.rms_at(f64::NAN, 2.0, 4).unwrap() - 1.0).abs() < 1e-12);
        m.reset();
        assert!(m.rms_at(50.0, 2.0, 4).is_none());
    }

    #[test]
    fn scale_fit_is_the_median_and_ignores_floor_readings() {
        let mut f = ScaleFit::new(16, 0.01, 0.01, 3);
        assert!(f.scale().is_none());
        assert!(!f.observe(0.001, 0.5), "reference at the floor is refused");
        assert!(!f.observe(0.5, 0.0), "device at zero is refused");
        assert!(!f.observe(f64::NAN, 1.0), "non-finite is refused");
        for (r, d) in [(2.0, 1.0), (2.1, 1.0), (1.9, 1.0), (40.0, 1.0)] {
            assert!(f.observe(r, d));
        }
        let s = f.scale().unwrap();
        assert!(
            (s - 2.05).abs() < 1e-9,
            "median of 1.9 2.0 2.1 40 = 2.05, got {s}"
        );
        assert_eq!(f.count(), 4);
        f.reset();
        assert!(f.scale().is_none());
    }

    #[test]
    fn the_fit_forgets_beyond_its_capacity() {
        let mut f = ScaleFit::new(3, 0.0, 0.0, 1);
        for r in [10.0, 10.0, 10.0, 1.0, 1.0, 1.0] {
            f.observe(r, 1.0);
        }
        assert_eq!(f.count(), 3);
        assert!((f.scale().unwrap() - 1.0).abs() < 1e-12);
    }
}
