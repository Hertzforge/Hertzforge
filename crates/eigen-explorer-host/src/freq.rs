//! The windowed zero-crossing frequency MEASUREMENT.
//!
//! It was already device-neutral - it consumes `f64` samples at a known rate and answers
//! in Hz - and both halves of the Explorer module need it: the delegated path corrects its
//! open-loop nudge estimate with it, and the HOST path
//! ([`crate::service::ExplorerHostService`]) uses it as the measured cross-check against
//! the frequency it commanded. One definition, so a "commanded vs measured" comparison
//! can never be two different measurements wearing one name.
//! The delegated Explorer service re-exports it at its original path.

use std::time::Duration;

/// The bench_probe-parity WINDOWED frequency measurement: a zero-crossing accumulator
/// fed one telemetry chunk at a time, emitting a measurement only when enough signal
/// has been observed (`bench_probe codrain`'s `RsAccum.ch1_hz`, the method that read
/// the CORRECT frequency on hardware during the proven 2026-07-15 sweep).
///
/// ## Why windowed, and why zero-crossing
///
/// * A single decoded chunk is one 64 KB block - under ~14 ms of signal. At the band
///   low edge (5 Hz) that is 1/14th of a cycle: unmeasurable. The window accumulates
///   ACROSS chunks until it has real cycles to count.
/// * Chunks are internally contiguous at the device rate but chunk-to-chunk gaps exist
///   (the proven `decode_every_n_reads = 8` drain decodes every 8th block). Crossings
///   are counted WITHIN chunks only (the `prev` sample resets per chunk), and the time
///   base is the ACCUMULATED FRAME COUNT / fs - so the decimated drain does NOT scale
///   the measured Hz: both the crossing count and the duration cover the same decoded
///   subset. (This is why bench_probe measured correct Hz under the same decimation.)
/// * An UNBROKEN stream fed a piece at a time (the host path's acquisition blocks) is the
///   other case, and it goes through [`WindowedFreqMeasure::push_contiguous`] instead: the
///   interval between two pieces is a sample interval like any other, and leaving it
///   untested while counting its time reads low by up to a crossing per piece.
/// * Zero-crossing over >= `min_crossings` observed crossings is unbiased for a tone
///   and cheap; no FFT bin quantization, no peak-picking on noise.
///
/// ## Starvation + maturity (deterministic: SIGNAL time, not wall time)
///
/// The window matures on accumulated SIGNAL duration (`frames / fs`, default 0.5 s)
/// AND a minimum crossing count (so the +/-1 crossing quantization is bounded to a few
/// percent). A low tone keeps accumulating until it has enough crossings; a window
/// that reaches 5x the nominal duration without them is discarded (noise-floor / DC
/// signal - NO measurement is the honest answer, never garbage). Under the proven
/// every-8th-block drain the decoded signal accumulates slower than wall time, so the
/// cadence stretches accordingly - the VALUE stays correct.
///
/// ## Amplitude gate (2026-07-16): a crossing count is not evidence of a TONE
///
/// The crossing count alone cannot tell a tone from noise. NOISE crosses zero too, and
/// when the drive sits at (or is clamped near) zero the counter happily accumulates its
/// `min_crossings` from the noise floor and matures a number - an IN-BAND number, which
/// the estimator's band gate cannot reject, because noise is broadband and part of it
/// always lands in the band. That is what walked the operator's frequency estimate around
/// while he touched nothing (2026-07-16: measured 56.9 -> 39.9 -> 35.0 Hz at a drive
/// frozen near zero, dragging the estimate 45.9 -> 44.4 -> 42.0).
///
/// So the window ALSO tracks the signal's RMS and refuses to mature below
/// [`WindowedFreqMeasure::min_rms`]. This is ADDITIVE to the estimator's band gate (Bug
/// 2/3, 2026-07-15), not a replacement: the band gate rejects out-of-band artifacts, this
/// rejects in-band noise. Both must pass.
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
    /// The TIME the crossing count is divided by, in sample intervals: every frame for
    /// [`Self::push_chunk`] (its historical time base), and exactly the intervals that were
    /// tested for a crossing for [`Self::push_contiguous`] - one fewer than its samples
    /// whenever a push starts a fresh stream, because that first sample has no interval
    /// before it. Maturity and the RMS gate stay on `frames`.
    span_frames: u64,
    /// Sum of squares over the accumulated window, for the RMS amplitude gate. The
    /// telemetry is AC data about zero (the same assumption `push_chunk`'s crossing count
    /// makes), so this is the AC RMS without a mean removal that a sub-cycle chunk would
    /// corrupt.
    sumsq: f64,
    /// The window's samples (2026-09-07), so a matured window is measured by a Hann-windowed
    /// FFT peak with parabolic refinement - noise-robust - and the crossing count is only
    /// the fallback. On the bench a 0.24 mV tone at the 20 Hz start "measured" 300-350 Hz
    /// by crossings (noise crossings), and the estimate, the sweep guard and the survey
    /// line all followed that number. Bounded at [`Self::MAX_SAMPLES`].
    samples: Vec<f64>,
    /// Measure a matured window by the FFT peak (contiguous full-decode streams). Off by
    /// default: the crossing count is the right instrument for the bursty every-8th
    /// decode the tests below model, where a stitched buffer has no honest spectrum.
    use_fft: bool,
    /// The last nonzero sample of the previous [`Self::push_contiguous`], so the interval
    /// BETWEEN two pushes of one unbroken stream is tested for a crossing like every other
    /// interval. `None` after a reset, a [`Self::push_chunk`] (which promises nothing about
    /// what follows it) or a [`Self::break_continuity`].
    carry: Option<f64>,
}

impl WindowedFreqMeasure {
    /// The default maturity window of accumulated SIGNAL (bench_probe integrated ~1 s
    /// drain windows; 0.5 s of decoded signal bounds the crossing quantization to
    /// ~2 % at 30 Hz while keeping the correction cadence responsive).
    pub const DEFAULT_WINDOW: Duration = Duration::from_millis(500);
    /// Minimum observed crossings for a valid measurement (6 full cycles: bounds the
    /// +/-1 crossing quantization to ~8 % worst-case at the band low edge).
    pub const MIN_CROSSINGS: u64 = 12;
    /// Most samples retained for the FFT measurement (one second at 65.536 kHz; the
    /// default window is half that).
    pub const MAX_SAMPLES: usize = 65_536;
    /// A spectral peak must stand this far above the median bin to count as a tone.
    pub const MIN_PEAK_OVER_MEDIAN: f64 = 8.0;

    /// A measurer for signal at `fs_hz` maturing after `window` of accumulated signal.
    ///
    /// The amplitude gate starts CLOSED (`min_rms = INFINITY`): nothing matures until the
    /// caller supplies a threshold measured from the channel's real noise floor via
    /// [`Self::set_min_rms`]. The first cut of this shipped a guessed 1e-3 default, which
    /// silently encoded one bench's cabling and front-end gain into a number that had to be
    /// right in two different unit systems at once. Starting closed means a caller that
    /// forgets to measure gets NO measurement (the honest, fail-closed outcome) rather than
    /// a measurement validated against a fiction.
    pub fn new(fs_hz: f64, window: Duration) -> Self {
        Self {
            fs_hz,
            window,
            min_crossings: Self::MIN_CROSSINGS,
            min_rms: f64::INFINITY,
            samples: Vec::new(),
            use_fft: false,
            crossings: 0,
            frames: 0,
            span_frames: 0,
            sumsq: 0.0,
            carry: None,
        }
    }

    /// Set the amplitude gate: the smallest window RMS that counts as a TONE rather than the
    /// channel's noise floor, in the channel's own units. Derived by the caller from the
    /// MEASURED floor (the noise-floor probe in [`crate::floor`]), so it carries no
    /// assumption about units, cabling or gain. `0.0` disables the gate (tests only);
    /// `INFINITY` keeps it closed.
    /// Measure matured windows by the Hann-FFT peak (parabolic refined) instead of the
    /// crossing count. For CONTIGUOUS streams only (the full deduplicated decode).
    pub fn with_fft(mut self, on: bool) -> Self {
        self.use_fft = on;
        self
    }

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

    /// Feed one CONTIGUOUS chunk of the measured channel. Counts strict sign changes
    /// about ZERO, exactly like bench_probe's `ChannelStats` (the telemetry is AC data
    /// around zero; a chunk-local mean removal would MANUFACTURE crossings on a
    /// sub-cycle chunk, whose partial sine arc has a large mean). Never carries `prev`
    /// across chunks (chunk gaps under the decimated drain would otherwise add a
    /// phase-random crossing per boundary). A fully DC-offset channel accumulates no
    /// crossings and yields NO measurement - the honest outcome.
    pub fn push_chunk(&mut self, signal: &[f64]) {
        if signal.len() < 2 {
            return;
        }
        let mut prev = signal[0];
        self.sumsq += prev * prev;
        for &cur in &signal[1..] {
            if (prev < 0.0 && cur > 0.0) || (prev > 0.0 && cur < 0.0) {
                self.crossings += 1;
            }
            if cur != 0.0 {
                prev = cur;
            }
            self.sumsq += cur * cur;
        }
        self.frames += signal.len() as u64;
        self.span_frames += signal.len() as u64;
        self.carry = None;
        self.keep_samples(signal);
    }

    /// Feed the next piece of an UNBROKEN stream: the first sample of `signal` is the one
    /// that immediately followed the last sample of the previous `push_contiguous`.
    ///
    /// # Why [`Self::push_chunk`] is wrong for such a stream
    ///
    /// `push_chunk` counts crossings within a chunk only, because its caller's chunks have
    /// GAPS between them. The time base still counts every frame, so each chunk boundary is
    /// one interval of time whose crossing is never counted. On a contiguous stream fed a
    /// block at a time that is a bias LOW, and not a small one: averaged over phase it is
    /// one interval per block (-0.78 % at a 128-frame block), but for a tone with a whole
    /// or half number of cycles per block the boundary sits at the SAME phase every block,
    /// so a boundary that holds a crossing holds one in every block.
    /// MEASURED, 2026-11-01, the host Explorer on a wire at the chassis's 8533.33 Hz /
    /// 128-frame geometry: steady tones at 533.33 Hz and 1066.67 Hz read 6.25 % and 3.13 %
    /// low, every window (WHICH whole-cycle tones lose is set by where the boundary falls
    /// in their phase), and 777 Hz, unrelated to the block, -0.83 %.
    ///
    /// The cure is two halves, and both are needed. The interval between two pushes is
    /// TESTED (the carried sample); and where the stream starts afresh - the first push of
    /// a window, or the first after [`Self::break_continuity`] - the first sample has no
    /// interval before it, so it adds no TIME either. Breaking the stream at a gap without
    /// the second half reads LOW by one interval per gap (measured: -4.5 Hz at 777 Hz with
    /// a gap every 256 frames); carrying across a gap without the first reads a
    /// phase-random extra crossing per gap.
    ///
    /// Same counting rule as `push_chunk` otherwise (strict sign changes about zero, an
    /// exact zero does not replace `prev`). Call [`Self::break_continuity`] where the
    /// stream has a gap, or the interval across the gap is tested as if it were one.
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
        self.keep_samples(signal);
    }

    /// The stream has a GAP before the next push: do not test the interval across it.
    /// Keeps the accumulated window - the crossings and the frames counted so far still
    /// describe the same samples - and drops only the carried sample.
    pub fn break_continuity(&mut self) {
        self.carry = None;
    }

    /// Retain `signal` for the FFT measurement, up to [`Self::MAX_SAMPLES`].
    fn keep_samples(&mut self, signal: &[f64]) {
        if self.use_fft && self.samples.len() < Self::MAX_SAMPLES {
            let room = Self::MAX_SAMPLES - self.samples.len();
            self.samples
                .extend_from_slice(&signal[..signal.len().min(room)]);
        }
    }

    /// Take a matured measurement (Hz), or `None` if the window is still accumulating.
    /// A window that reaches 5x the nominal signal duration without `min_crossings`
    /// is discarded (noise-floor / DC signal). Taking (or discarding) resets the
    /// accumulation.
    ///
    /// A matured window whose RMS is below the amplitude gate is DISCARDED, not returned:
    /// there is no tone to measure, and a crossing count taken off the noise floor is a
    /// number without a referent. NO measurement is the honest answer.
    pub fn take(&mut self) -> Option<f64> {
        let seconds = self.frames as f64 / self.fs_hz;
        if seconds < self.window.as_secs_f64() {
            return None;
        }
        // Amplitude gate BEFORE the crossing test: a noise-floor window that happens to
        // clear `min_crossings` must not mature. Discard it outright - waiting longer only
        // accumulates more noise, never a tone.
        // NB `<` (not `!(>=)`): a NaN rms must FAIL the gate, and NaN < x is false, so the
        // explicit non-finite check carries it.
        if self.min_rms > 0.0 && (!self.rms().is_finite() || self.rms() < self.min_rms) {
            self.reset();
            return None;
        }
        // The FFT peak is the measurement; the crossing count is the fallback for a
        // window the spectrum cannot call (no peak clear of the median).
        if self.use_fft {
            if let Some(hz) = fft_peak_hz(&self.samples, self.fs_hz, Self::MIN_PEAK_OVER_MEDIAN) {
                self.reset();
                return Some(hz);
            }
        }
        if self.crossings < self.min_crossings {
            if seconds >= 5.0 * self.window.as_secs_f64().max(0.1) {
                self.reset();
            }
            return None;
        }
        // The crossings are divided by the time they were COUNTED over, which is the span,
        // not the sample count (see `span_frames`; for `push_chunk` the two are equal).
        let span_seconds = self.span_frames as f64 / self.fs_hz;
        let hz = if span_seconds > 0.0 {
            Some(self.crossings as f64 / 2.0 / span_seconds)
        } else {
            None
        };
        self.reset();
        hz
    }

    /// Discard the accumulation (run stop / link change), and the carried sample with it:
    /// the next window starts a fresh stream, whose first sample adds no time.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.crossings = 0;
        self.frames = 0;
        self.span_frames = 0;
        self.sumsq = 0.0;
        self.carry = None;
    }
}

/// Hann-windowed FFT peak of `signal` (mean removed) with parabolic sub-bin refinement,
/// or `None` when the peak does not stand `min_peak_over_median` times above the median
/// bin (no tone: the spectrum is flat noise) or the window is too short to resolve.
pub fn fft_peak_hz(signal: &[f64], fs_hz: f64, min_peak_over_median: f64) -> Option<f64> {
    let n = signal.len();
    if n < 64 || fs_hz <= 0.0 {
        return None;
    }
    let mean = signal.iter().sum::<f64>() / n as f64;
    let nfft = eigen_dsp::fft::next_pow2(n);
    let w = eigen_dsp::window::hann(n);
    let mut buf: Vec<eigen_dsp::fft::Cplx> = Vec::with_capacity(nfft);
    for i in 0..n {
        buf.push(((signal[i] - mean) * w[i], 0.0));
    }
    buf.resize(nfft, (0.0, 0.0));
    eigen_dsp::fft::fft_in_place(&mut buf);
    let nout = nfft / 2 + 1;
    let mags: Vec<f64> = buf
        .iter()
        .take(nout)
        .map(|&c| eigen_dsp::fft::mag(c))
        .collect();
    let (mut peak_k, mut peak_mag) = (0usize, 0.0f64);
    for (k, &m) in mags.iter().enumerate().skip(1) {
        if m > peak_mag {
            peak_mag = m;
            peak_k = k;
        }
    }
    if peak_k == 0 || peak_mag <= 0.0 {
        return None;
    }
    // Tone test: the peak against the median bin (the noise floor of a flat spectrum).
    let mut sorted: Vec<f64> = mags[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    if median > 0.0 && peak_mag < min_peak_over_median * median {
        return None;
    }
    let df = fs_hz / nfft as f64;
    let k = peak_k;
    let hz = if k >= 1 && k + 1 < nout {
        let a = mags[k - 1].max(1e-30).ln();
        let b = mags[k].max(1e-30).ln();
        let c = mags[k + 1].max(1e-30).ln();
        let denom = a - 2.0 * b + c;
        let delta = if denom.abs() > 1e-30 {
            0.5 * (a - c) / denom
        } else {
            0.0
        };
        (k as f64 + delta.clamp(-0.5, 0.5)) * df
    } else {
        k as f64 * df
    };
    Some(hz)
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
        m.push_chunk(&tone(fs, 250.0, 8_000, 1.0));
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
        m.push_chunk(&tone(fs, 250.0, 8_000, 1.0e-3));
        assert_eq!(m.take(), None);
    }

    /// The bench case of 2026-09-07: a tone at the noise floor. Crossings over-read it by
    /// an order of magnitude; the FFT peak calls it.
    #[test]
    fn a_noisy_low_tone_measures_by_the_fft_peak_not_the_crossings() {
        let fs = 65_536.0;
        let mut m = WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW)
            .with_min_rms(0.0)
            .with_fft(true);
        let mut seed = 12345u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as f64 / u64::MAX as f64) - 0.5
        };
        let sig: Vec<f64> = (0..40_000)
            .map(|k| 0.02 * (TAU * 20.0 * k as f64 / fs).sin() + 0.05 * rng())
            .collect();
        m.push_chunk(&sig);
        let hz = m.take().expect("a measurement matures");
        assert!(
            (hz - 20.0).abs() < 1.0,
            "fft peak should call 20 Hz, got {hz}"
        );
    }

    /// THE BLOCK-BOUNDARY CROSSING. A tone with exactly 1.5 cycles per 64-frame block, its
    /// phase placed so a zero crossing falls between the last sample of every block and the
    /// first of the next, fed a block at a time. `push_chunk` never tests that interval and
    /// reads 2/3 of the tone (two crossings counted per block of three); `push_contiguous`
    /// reads it exactly.
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

        let mut chunked =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.0);
        let mut contiguous =
            WindowedFreqMeasure::new(fs, WindowedFreqMeasure::DEFAULT_WINDOW).with_min_rms(0.0);
        for b in sig.chunks(block) {
            chunked.push_chunk(b);
            contiguous.push_contiguous(b);
        }
        let lossy = chunked.take().expect("a second of tone matures");
        let exact = contiguous.take().expect("a second of tone matures");
        assert!(
            (lossy / f - 2.0 / 3.0).abs() < 0.01,
            "push_chunk should drop the boundary crossing of every block: read {lossy} Hz \
             against {f} Hz"
        );
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

    #[test]
    fn a_flat_noise_window_yields_no_fft_tone() {
        let mut seed = 777u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as f64 / u64::MAX as f64) - 0.5
        };
        let sig: Vec<f64> = (0..8192).map(|_| rng()).collect();
        assert!(fft_peak_hz(&sig, 8192.0, WindowedFreqMeasure::MIN_PEAK_OVER_MEDIAN).is_none());
    }
}
