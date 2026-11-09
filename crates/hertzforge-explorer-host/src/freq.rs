//! The windowed zero-crossing frequency MEASUREMENT: the cross-check of the frequency
//! the service commands against the frequency its control channel carries
//! ([`crate::service::ExplorerHostService`]).

use std::time::Duration;

/// A windowed frequency measurement: a zero-crossing count over an unbroken stream fed a
/// block at a time, emitting a measurement only once enough signal has been observed.
///
/// ## Why windowed, and why zero-crossing
///
/// * One acquisition block is too short to count cycles at the band's low edge, so the
///   window accumulates ACROSS blocks until it holds real cycles.
/// * The stream is unbroken between blocks, so [`WindowedFreqMeasure::push_contiguous`]
///   tests the interval between two pushes like any other; leaving it untested while
///   counting its time reads low by up to a crossing per block.
/// * Zero-crossing over at least `min_crossings` observed crossings is unbiased for a tone
///   and cheap; no FFT bin quantization, no peak-picking on noise.
///
/// ## Maturity, in SIGNAL time
///
/// The window matures on accumulated SIGNAL duration (`frames / fs`, default 0.5 s) AND a
/// minimum crossing count, so the +/-1 crossing quantization is bounded to a few percent.
/// A low tone keeps accumulating until it has enough crossings; a window that reaches 5x
/// the nominal duration without them is discarded (noise floor or DC: no measurement is
/// the honest answer).
///
/// ## Amplitude gate: a crossing count is not evidence of a TONE
///
/// Noise crosses zero too, and a drive at or near zero would let the counter mature an
/// in-band number from the noise floor. So the window also tracks the signal's RMS and
/// refuses to mature below [`WindowedFreqMeasure::set_min_rms`]'s threshold.
#[derive(Debug, Clone)]
pub struct WindowedFreqMeasure {
    fs_hz: f64,
    /// Minimum accumulated SIGNAL duration before a measurement can mature.
    window: Duration,
    min_crossings: u64,
    /// Minimum accumulated signal RMS before a measurement can mature (the amplitude
    /// gate). 0.0 disables the gate.
    min_rms: f64,
    crossings: u64,
    frames: u64,
    /// The TIME the crossing count is divided by, in sample intervals: exactly the
    /// intervals that were tested for a crossing, one fewer than the samples whenever a
    /// push starts a fresh stream, because that first sample has no interval before it.
    /// Maturity and the RMS gate stay on `frames`.
    span_frames: u64,
    /// Sum of squares over the accumulated window, for the RMS amplitude gate (the AC
    /// RMS about zero, as the crossing count assumes).
    sumsq: f64,
    /// The last nonzero sample of the previous push, so the interval BETWEEN two pushes of
    /// one unbroken stream is tested for a crossing like every other interval. `None` after
    /// a reset or a [`Self::break_continuity`].
    carry: Option<f64>,
}

impl WindowedFreqMeasure {
    /// The default maturity window of accumulated SIGNAL: 0.5 s bounds the crossing
    /// quantization to about 2 % at 30 Hz while keeping the cross-check responsive.
    pub const DEFAULT_WINDOW: Duration = Duration::from_millis(500);
    /// Minimum observed crossings for a valid measurement (6 full cycles: bounds the
    /// +/-1 crossing quantization to about 8 % worst-case at the band's low edge).
    pub const MIN_CROSSINGS: u64 = 12;

    /// A measurer for signal at `fs_hz` maturing after `window` of accumulated signal.
    ///
    /// The amplitude gate starts CLOSED (`min_rms = INFINITY`): nothing matures until the
    /// caller supplies a threshold measured from the channel's real noise floor via
    /// [`Self::set_min_rms`], so a caller that forgets to measure gets no measurement
    /// rather than one validated against a guessed number.
    pub fn new(fs_hz: f64, window: Duration) -> Self {
        Self {
            fs_hz,
            window,
            min_crossings: Self::MIN_CROSSINGS,
            min_rms: f64::INFINITY,
            crossings: 0,
            frames: 0,
            span_frames: 0,
            sumsq: 0.0,
            carry: None,
        }
    }

    /// Set the amplitude gate: the smallest window RMS that counts as a TONE rather than the
    /// channel's noise floor, in the channel's own units. Derived by the caller from the
    /// MEASURED floor (the noise-floor probe in [`crate::floor`]). `0.0` disables the gate;
    /// `INFINITY` keeps it closed.
    pub fn set_min_rms(&mut self, min_rms: f64) {
        self.min_rms = min_rms.max(0.0);
    }

    /// Builder form of [`Self::set_min_rms`].
    pub fn with_min_rms(mut self, min_rms: f64) -> Self {
        self.set_min_rms(min_rms);
        self
    }

    /// The sample rate this measurer denominates its answer with.
    pub fn sample_rate_hz(&self) -> f64 {
        self.fs_hz
    }

    /// The RMS of the currently accumulated window (0.0 when empty). The quantity the
    /// amplitude gate tests.
    pub fn rms(&self) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        (self.sumsq / self.frames as f64).max(0.0).sqrt()
    }

    /// Feed the next piece of an UNBROKEN stream: the first sample of `signal` is the one
    /// that immediately followed the last sample of the previous `push_contiguous`.
    ///
    /// Two halves, and both are needed. The interval between two pushes is TESTED (the
    /// carried sample); and where the stream starts afresh (the first push of a window, or
    /// the first after [`Self::break_continuity`]) the first sample has no interval before
    /// it, so it adds no TIME either. Breaking the stream at a gap without the second half
    /// reads LOW by one interval per gap; carrying across a gap without the first reads a
    /// phase-random extra crossing per gap.
    ///
    /// Counts strict sign changes about zero; an exact zero does not replace `prev`. Call
    /// [`Self::break_continuity`] where the stream has a gap.
    pub fn push_contiguous(&mut self, signal: &[f64]) {
        let Some((&first, rest)) = signal.split_first() else {
            return;
        };
        let (mut prev, tail) = match self.carry {
            Some(p) => (p, signal),
            None => {
                self.sumsq += first * first;
                (first, rest)
            }
        };
        // One interval of time per sample interval TESTED: `tail.len()` of them, whether
        // the first is the one across from the carried sample or not.
        self.span_frames += tail.len() as u64;
        for &cur in tail {
            if (prev < 0.0 && cur > 0.0) || (prev > 0.0 && cur < 0.0) {
                self.crossings += 1;
            }
            if cur != 0.0 {
                prev = cur;
            }
            self.sumsq += cur * cur;
        }
        self.frames += signal.len() as u64;
        self.carry = Some(prev);
    }

    /// The stream has a GAP before the next push: do not test the interval across it.
    /// Keeps the accumulated window and drops only the carried sample.
    pub fn break_continuity(&mut self) {
        self.carry = None;
    }

    /// Take a matured measurement (Hz), or `None` if the window is still accumulating.
    /// A window that reaches 5x the nominal signal duration without `min_crossings`
    /// is discarded (noise floor or DC). Taking (or discarding) resets the accumulation.
    ///
    /// A matured window whose RMS is below the amplitude gate is DISCARDED, not returned:
    /// a crossing count taken off the noise floor is a number without a referent.
    pub fn take(&mut self) -> Option<f64> {
        let seconds = self.frames as f64 / self.fs_hz;
        if seconds < self.window.as_secs_f64() {
            return None;
        }
        // Amplitude gate BEFORE the crossing test: a noise-floor window that happens to
        // clear `min_crossings` must not mature. A NaN rms must FAIL the gate.
        if self.min_rms > 0.0 && (!self.rms().is_finite() || self.rms() < self.min_rms) {
            self.reset();
            return None;
        }
        if self.crossings < self.min_crossings {
            if seconds >= 5.0 * self.window.as_secs_f64().max(0.1) {
                self.reset();
            }
            return None;
        }
        // The crossings are divided by the time they were COUNTED over: the span.
        let span_seconds = self.span_frames as f64 / self.fs_hz;
        let hz = if span_seconds > 0.0 {
            Some(self.crossings as f64 / 2.0 / span_seconds)
        } else {
            None
        };
        self.reset();
        hz
    }

    /// Discard the accumulation, and the carried sample with it: the next window starts a
    /// fresh stream, whose first sample adds no time.
    pub fn reset(&mut self) {
        self.crossings = 0;
        self.frames = 0;
        self.span_frames = 0;
        self.sumsq = 0.0;
        self.carry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn tone(fs: f64, f: f64, n: usize, amp: f64) -> Vec<f64> {
        (0..n)
            .map(|k| amp * (TAU * f * k as f64 / fs).sin())
            .collect()
    }

    #[test]
    fn a_matured_window_reads_the_tone() {
        let fs = 8_000.0;
        let mut m =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.0);
        m.push_contiguous(&tone(fs, 250.0, 8_000, 1.0));
        let hz = m
            .take()
            .expect("a full second of a 250 Hz tone must mature");
        assert!((hz - 250.0).abs() < 2.0, "measured {hz} Hz");
    }

    /// The move must not have dropped the amplitude gate: a window whose RMS is under
    /// the floor is DISCARDED, not answered.
    #[test]
    fn the_amplitude_gate_still_refuses_a_noise_floor_window() {
        let fs = 8_000.0;
        let mut m =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.1);
        m.push_contiguous(&tone(fs, 250.0, 8_000, 1.0e-3));
        assert_eq!(m.take(), None);
    }

    /// THE BLOCK-BOUNDARY CROSSING. A tone with exactly 1.5 cycles per 64-frame block, its
    /// phase placed so a zero crossing falls between the last sample of every block and the
    /// first of the next, fed a block at a time: `push_contiguous` counts that crossing and
    /// reads the tone exactly.
    #[test]
    fn a_contiguous_stream_counts_the_crossing_between_two_pushes() {
        let fs = 8_192.0;
        let block = 64usize;
        let f = 1.5 * fs / block as f64; // 192 Hz: 1.5 cycles, 3 crossings per block
                                         // Zero crossings at `k = -0.5 + m * fs / (2 f)`, i.e. every 21.33 frames, and the
                                         // third of every block lands at `block - 0.5`: BETWEEN two blocks.
        let sig: Vec<f64> = (0..fs as usize)
            .map(|k| (TAU * f * (k as f64 + 0.5) / fs).sin())
            .collect();
        // Sanity: the boundary interval really does hold a sign change.
        assert!(
            sig[block - 1] * sig[block] < 0.0,
            "fixture: no crossing at the boundary"
        );

        let mut contiguous =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.0);
        for b in sig.chunks(block) {
            contiguous.push_contiguous(b);
        }
        let exact = contiguous.take().expect("a second of tone matures");
        // Within ONE crossing of a one-second window (0.5 Hz): the crossing quantization,
        // which no counting rule can beat - the window's last crossing falls half a sample
        // past its last sample.
        assert!(
            (exact - f).abs() <= 0.5 + 1e-9,
            "push_contiguous read {exact} Hz against {f} Hz"
        );
    }

    /// A gap breaks the stream: the interval ACROSS it is not a sample interval and must not
    /// be tested. Two pushes of the same half-cycle, with a gap between them that a carried
    /// sample would read as a sign change, count no crossing at all.
    #[test]
    fn break_continuity_stops_a_gap_counting_as_a_crossing() {
        let fs = 1_000.0;
        let mut m = WindowedFreqMeasure::new(fs, Duration::from_millis(1)).with_min_rms(0.0);
        let mut carried = m.clone();
        m.push_contiguous(&[1.0, 1.0, 1.0]);
        carried.push_contiguous(&[1.0, 1.0, 1.0]);
        m.break_continuity();
        m.push_contiguous(&[-1.0, -1.0, -1.0]);
        carried.push_contiguous(&[-1.0, -1.0, -1.0]);
        assert_eq!(m.crossings, 0, "the gap was counted as a crossing");
        assert_eq!(
            carried.crossings, 1,
            "an unbroken stream counts the boundary"
        );
        assert_eq!(m.frames, 6);
        // Time: the broken stream TESTED 2 + 2 intervals, the unbroken one 2 + 3.
        assert_eq!(m.span_frames, 4, "a fresh start's first sample added time");
        assert_eq!(carried.span_frames, 5);
    }
}
