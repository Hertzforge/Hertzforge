//! The windowed zero-crossing frequency MEASUREMENT: the cross-check of the frequency
//! the service commands against the frequency its control channel carries
//! ([`crate::service::ExplorerHostService`]).

use std::time::Duration;

/// A windowed frequency measurement over an unbroken stream fed a block at a time,
/// emitting a measurement only once enough signal has been observed.
///
/// ## What it measures: the time between rising zero crossings
///
/// Each rising crossing is placed BETWEEN its two samples by linear interpolation, and the
/// frequency is the number of whole periods between the first and the last rising crossing
/// of a window, divided by the time between them. A crossing COUNT divided by the window's
/// duration is quantized to one crossing per window, which is 1 Hz on a 0.5 s window at
/// any frequency: 0.1 % at 1 kHz and 2 % at 50 Hz, where the stated cross-check is 0.05 %.
/// Measured on the NI loopback 2026-11-09 (X1, X11): 1010 Hz read 1010.729, 50.6 Hz read
/// 50.008, 43.0 Hz read 42.563. Timing the crossings has no such floor: a clean tone reads
/// to parts per million, and the noise on the chassis (about 120 uV rms against a 0.2 V
/// tone) moves a crossing by microseconds.
///
/// * Rising crossings only, so the answer is a whole number of periods: a DC offset moves
///   every rising crossing by the same time and cancels, where a span from a rising to a
///   falling crossing would carry it (the loopback carries about 0.7 mV).
/// * One acquisition block is too short to hold periods at the band's low edge, so the
///   window accumulates ACROSS blocks until it holds real cycles.
/// * The stream is unbroken between blocks, so [`WindowedFreqMeasure::push_contiguous`]
///   tests the interval between two pushes like any other. Where the stream has a GAP
///   ([`WindowedFreqMeasure::break_continuity`]) the time across it is unknown, so each
///   unbroken run contributes its own periods over its own span, and the runs are summed.
///
/// ## Hysteresis: noise near zero is not a crossing
///
/// Near zero a small tone's slope per sample can be comparable with the noise, and the
/// noise then crosses zero two or three times where the tone crosses once. A rising
/// crossing is COUNTED only once the signal has been below `-h` since the last one and then
/// reaches `+h`; the time recorded is the last upward zero crossing before it reached
/// `+h`. `h` is the amplitude gate ([`WindowedFreqMeasure::set_min_rms`]): a level the
/// floor does not reach, and below any tone the gate lets mature.
///
/// ## Maturity, in SIGNAL time
///
/// The window matures on accumulated SIGNAL duration (`frames / fs`, default 0.5 s) AND a
/// minimum count of sign changes. A low tone keeps accumulating until it has enough; a
/// window that reaches 5x the nominal duration without them is discarded (noise floor or
/// DC: no measurement is the honest answer).
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
    /// gate), and the hysteresis a rising crossing must clear. 0.0 disables both.
    min_rms: f64,
    /// Strict sign changes about zero, both directions: the maturity count.
    crossings: u64,
    frames: u64,
    /// Sum of squares over the accumulated window, for the RMS amplitude gate (the AC
    /// RMS about zero, as the crossing count assumes).
    sumsq: f64,
    /// The current unbroken run: its next sample's position in frames from the run's
    /// first sample, and the last nonzero sample with its position, so the interval
    /// BETWEEN two pushes is tested like every other. `None` after a reset or a
    /// [`Self::break_continuity`].
    carry: Option<Carry>,
    /// The current run's counted rising crossings: the first and last time (in frames
    /// from the run's first sample) and how many.
    run_first_rise: f64,
    run_last_rise: f64,
    run_rises: u64,
    /// The hysteresis state of the current run.
    armed_low: bool,
    candidate: Option<f64>,
    /// Closed runs: whole periods and the frames they span.
    periods: u64,
    period_frames: f64,
}

#[derive(Debug, Clone, Copy)]
struct Carry {
    /// Position, in frames from the run's first sample, of the next sample to arrive.
    next_pos: f64,
    /// The last nonzero sample and its position.
    prev: f64,
    prev_pos: f64,
}

impl WindowedFreqMeasure {
    /// The default maturity window of accumulated SIGNAL: 0.5 s keeps the cross-check
    /// responsive, and holds at least ten periods from 20 Hz up.
    pub const DEFAULT_WINDOW: Duration = Duration::from_millis(500);
    /// Minimum observed sign changes for a valid measurement (6 full cycles).
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
            sumsq: 0.0,
            carry: None,
            run_first_rise: 0.0,
            run_last_rise: 0.0,
            run_rises: 0,
            armed_low: false,
            candidate: None,
            periods: 0,
            period_frames: 0.0,
        }
    }

    /// Set the amplitude gate: the smallest window RMS that counts as a TONE rather than the
    /// channel's noise floor, in the channel's own units, and the hysteresis a rising
    /// crossing must clear. Derived by the caller from the MEASURED floor (the noise-floor
    /// probe in [`crate::floor`]). `0.0` disables the gate; `INFINITY` keeps it closed.
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

    fn hysteresis(&self) -> f64 {
        if self.min_rms.is_finite() {
            self.min_rms
        } else {
            0.0
        }
    }

    /// Feed the next piece of an UNBROKEN stream: the first sample of `signal` is the one
    /// that immediately followed the last sample of the previous `push_contiguous`.
    ///
    /// Counts strict sign changes about zero; an exact zero does not replace `prev`. Call
    /// [`Self::break_continuity`] where the stream has a gap.
    pub fn push_contiguous(&mut self, signal: &[f64]) {
        if signal.is_empty() {
            return;
        }
        let h = self.hysteresis();
        let mut c = match self.carry {
            Some(c) => c,
            None => {
                // A fresh run: its first sample has no interval before it.
                let first = signal[0];
                self.sumsq += first * first;
                self.frames += 1;
                self.armed_low = first < -h;
                self.candidate = None;
                self.run_rises = 0;
                let c = Carry {
                    next_pos: 1.0,
                    prev: first,
                    prev_pos: 0.0,
                };
                self.carry = Some(c);
                return self.push_contiguous(&signal[1..]);
            }
        };
        for &cur in signal {
            let pos = c.next_pos;
            c.next_pos += 1.0;
            self.sumsq += cur * cur;
            self.frames += 1;
            if (c.prev < 0.0 && cur > 0.0) || (c.prev > 0.0 && cur < 0.0) {
                self.crossings += 1;
                if c.prev < 0.0 {
                    // Upward: the zero of the line through the two samples.
                    let t = c.prev_pos + (pos - c.prev_pos) * (c.prev / (c.prev - cur));
                    self.candidate = Some(t);
                }
            }
            if cur < -h {
                self.armed_low = true;
                self.candidate = None;
            } else if cur > h && self.armed_low {
                if let Some(t) = self.candidate.take() {
                    if self.run_rises == 0 {
                        self.run_first_rise = t;
                    }
                    self.run_last_rise = t;
                    self.run_rises += 1;
                }
                self.armed_low = false;
            }
            if cur != 0.0 {
                c.prev = cur;
                c.prev_pos = pos;
            }
        }
        self.carry = Some(c);
    }

    /// Fold the current unbroken run into the window's periods.
    fn close_run(&mut self) {
        if self.run_rises >= 2 {
            self.periods += self.run_rises - 1;
            self.period_frames += self.run_last_rise - self.run_first_rise;
        }
        self.run_rises = 0;
        self.candidate = None;
        self.armed_low = false;
        self.carry = None;
    }

    /// The stream has a GAP before the next push: do not test the interval across it.
    /// Keeps the accumulated window; the run before the gap keeps its periods.
    pub fn break_continuity(&mut self) {
        self.close_run();
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
        // The run in progress is part of this window; the next window starts afresh.
        self.close_run();
        let hz = if self.period_frames > 0.0 && self.periods > 0 {
            Some(self.periods as f64 / self.period_frames * self.fs_hz)
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
        self.sumsq = 0.0;
        self.carry = None;
        self.run_rises = 0;
        self.candidate = None;
        self.armed_low = false;
        self.periods = 0;
        self.period_frames = 0.0;
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
        assert!((hz / 250.0 - 1.0).abs() < 1e-6, "measured {hz} Hz");
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
        assert!(
            (exact / f - 1.0).abs() < 1e-6,
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
    }

    /// Feed `sig` a 256-frame block at a time, as the chassis delivers it, and take the
    /// first matured reading.
    fn read_blocks(fs: f64, sig: &[f64], min_rms: f64) -> f64 {
        let mut m =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(min_rms);
        for b in sig.chunks(256) {
            m.push_contiguous(b);
            if let Some(hz) = m.take() {
                return hz;
            }
        }
        panic!("no reading matured");
    }

    /// THE BENCH FINDING (NI loopback, 2026-11-09): a crossing COUNT over a 0.5 s window is
    /// quantized to 1 Hz at every frequency, so tones off a whole number of crossings per
    /// window read up to 1 Hz off (1010 Hz read 1010.729, 50.6 Hz read 50.008, 43.0 Hz read
    /// 42.563), against a stated 0.05 %. Timed crossings read each of them to 5 ppm.
    #[test]
    fn a_tone_off_a_whole_count_per_window_reads_to_parts_per_million() {
        let fs = 12_800.0;
        for &f in &[
            20.0, 25.9, 43.0, 50.6, 100.0, 514.5, 515.0, 1_010.0, 1_999.7, 5_000.3,
        ] {
            // At the band's top (2.56 samples a period) the straight line between two
            // samples places a crossing off the sine by up to a few hundredths of a sample,
            // and the error at the window's two ends does not cancel: 30 ppm at 5 kHz.
            let tol = if f > 2_000.0 { 1e-4 } else { 5e-6 };
            // An arbitrary starting phase, so no crossing sits on a sample.
            let sig: Vec<f64> = (0..12_800)
                .map(|k| 0.2 * (TAU * f * k as f64 / fs + 0.37).sin())
                .collect();
            let hz = read_blocks(fs, &sig, 0.0);
            assert!(
                (hz / f - 1.0).abs() < tol,
                "{f} Hz read {hz} Hz ({:+.4} %)",
                (hz / f - 1.0) * 100.0
            );
        }
    }

    /// The loopback carries about 0.7 mV of DC. Rising crossings only: every one moves by
    /// the same time and the period is unchanged.
    #[test]
    fn a_dc_offset_does_not_move_the_reading() {
        let fs = 12_800.0;
        for &f in &[20.0, 43.0, 250.0] {
            let sig: Vec<f64> = (0..12_800)
                .map(|k| 0.2 * (TAU * f * k as f64 / fs + 1.1).sin() + 0.000_7)
                .collect();
            let hz = read_blocks(fs, &sig, 0.0);
            assert!((hz / f - 1.0).abs() < 5e-6, "{f} Hz with DC read {hz} Hz");
        }
    }

    /// A small tone in the chassis's noise: 2 mV pk at 250 Hz moves 0.25 mV a sample at
    /// its crossing against 0.12 mV rms of noise, so the noise adds crossings. The
    /// hysteresis (the amplitude gate, three times the floor) counts one per period.
    #[test]
    fn noise_at_a_small_tones_crossing_is_not_counted_as_a_period() {
        let fs = 12_800.0;
        let f = 250.0;
        let sigma = 0.000_12;
        // A deterministic approximately Gaussian noise: the sum of twelve uniforms.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut uniform = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64
        };
        let sig: Vec<f64> = (0..12_800)
            .map(|k| {
                let g: f64 = (0..12).map(|_| uniform()).sum::<f64>() - 6.0;
                0.002 * (TAU * f * k as f64 / fs).sin() + sigma * g
            })
            .collect();
        let hz = read_blocks(fs, &sig, 3.0 * sigma);
        assert!(
            (hz / f - 1.0).abs() < 5e-4,
            "a 2 mV tone in 0.12 mV of noise read {hz} Hz"
        );
    }

    /// Across a gap each unbroken run counts its own periods over its own span.
    #[test]
    fn runs_either_side_of_a_gap_are_summed_not_bridged() {
        let fs = 12_800.0;
        let f = 777.0;
        let sig: Vec<f64> = (0..38_400)
            .map(|k| (TAU * f * k as f64 / fs + 0.2).sin())
            .collect();
        let mut m =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.0);
        // Keep one block in three: two thirds of the stream dropped, a gap before each.
        let mut hz = None;
        for (i, b) in sig.chunks(256).enumerate() {
            if i % 3 != 0 {
                continue;
            }
            m.break_continuity();
            m.push_contiguous(b);
            if let Some(h) = m.take() {
                hz = Some(h);
                break;
            }
        }
        let hz = hz.expect("a reading matures across gaps");
        assert!(
            (hz / f - 1.0).abs() < 1e-5,
            "under gaps {f} Hz read {hz} Hz"
        );
    }
}
