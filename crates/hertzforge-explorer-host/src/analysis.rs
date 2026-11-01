//! Analysis pipeline.
//!
//! Three products, all fed from decoded telemetry:
//!
//! 1. **Spectra** ([`SpectrumEngine`]): a windowed FFT (Hann; configurable size, default
//!    4096) per enabled channel over decimated telemetry, streamed as [`SpectrumFrame`]s
//!    at ~5-10 frames/s per channel. Peak-hold is UI-side (the live FFT stays indicative
//!    during a continuous sweep); this crate only streams live spectra. The frame layout
//!    matches the TS `SpectrumFrame` binary contract (`ui/src/lib/ipc/frame.ts`) so the
//!    encoder is a straight pack.
//! 2. **Resonance profile** ([`ProfileTracer`]): a [`sine::TrackingFilter`] amplitude vs
//!    instantaneous estimated-frequency, accumulated as `(freq, amplitude)` points during
//!    sweeps into a [`ProfileTrace`] per channel. This is the CALIBRATED resonance map
//!    (the primary resonance map, not the smeared FFT peak-hold).
//! 3. **Derived velocity/displacement** ([`DerivedTracer`]): integrate acceleration with
//!    a high-pass (matching [`hertzforge_dsp::integrate`] / the abort integrator), exposing a
//!    settling flag for N high-pass time constants so a UI does not plot the transient as
//!    real motion. Derived spectra are tagged [`SpectrumKind::Vel`] /
//!    [`SpectrumKind::Disp`].
//!
//! ## THE FFT IS SYNCHRONOUS IN THE CALLER'S THREAD - KEEP IT OFF AN RT TICK
//!
//! Nothing here spawns a thread or defers work. [`ChannelSpectrum::push`] transforms
//! IN LINE the moment a window fills, so the cost lands on whichever thread pushed the
//! sample that completed it. That was harmless on the delegated path, where samples arrive
//! on a polling loop with no deadline. It is NOT harmless on a host-closes-loop
//! backend: the measured transform is ~24 ms against a ~15 ms block period, so a
//! process step that fed this engine from the engine tick would overrun its own
//! deadline every time a window closed.
//!
//! The rule for any real-time consumer: the RT tick writes samples into a bounded ring
//! and returns; a PUMP thread drains the ring and runs [`SpectrumEngine`] /
//! [`DerivedTracer`] / [`ProfileTracer`] there. This module stays synchronous on
//! purpose - the scheduling decision belongs to the consumer, which is the only layer
//! that knows what its deadline is.

use hertzforge_dsp::sine::TrackingFilter;
use hertzforge_dsp::{fft, window};
use hertzforge_engine::limits::ChannelIntegrator;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// The derived domain a spectrum is expressed in. Matches the TS `SpectrumKind`
/// (`accel` / `vel` / `disp`); the integer codes are pinned by the TS binary contract
/// (`ui/src/lib/ipc/frame.ts`: 0 accel, 1 vel, 2 disp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumKind {
    Accel,
    Vel,
    Disp,
}

impl SpectrumKind {
    /// The wire code used by the TS binary `SpectrumFrame` contract.
    pub fn code(self) -> u32 {
        match self {
            SpectrumKind::Accel => 0,
            SpectrumKind::Vel => 1,
            SpectrumKind::Disp => 2,
        }
    }
}

/// One channel's linear-magnitude spectrum. Field names / semantics match the TS
/// `SpectrumFrame` (`ui/src/lib/ipc/types.ts`): `mags[b]` is the magnitude of the bin
/// centred at `f0_hz + b*df_hz`, in the EU implied by `kind`. `seq` is a per-channel
/// monotonic sequence number for drop/dupe detection.
///
/// The Slice-5 IPC encoder packs this into the pinned little-endian layout
/// (`ch:u32, n_bins:u32, f0_hz:f32, df_hz:f32, kind:u32, seq:u32, mags:f32[n_bins]`);
/// this struct is that layout's Rust source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectrumFrame {
    /// 1-based channel index (matches the channel-table `index`).
    pub ch: u32,
    /// Frequency of bin 0, Hz.
    pub f0_hz: f64,
    /// Bin spacing, Hz.
    pub df_hz: f64,
    /// Linear magnitude per bin, EU for `kind`.
    pub mags: Vec<f32>,
    /// Which derived domain this spectrum is in.
    pub kind: SpectrumKind,
    /// Monotonic per-channel sequence number.
    pub seq: u64,
}

/// A single-channel windowed-FFT spectrum engine. Accumulates decimated samples for one
/// channel until it has a full `nfft`-length window, then emits a linear-magnitude
/// [`SpectrumFrame`]. Amplitude-corrected for the Hann window so a pure tone reads its
/// true amplitude.
#[derive(Debug, Clone)]
pub struct ChannelSpectrum {
    ch: u32,
    nfft: usize,
    fs_hz: f64,
    win: Vec<f64>,
    /// Coherent-gain reciprocal so a tone's magnitude is its amplitude, not the
    /// window-attenuated value.
    amp_correction: f64,
    acc: Vec<f64>,
    seq: u64,
}

impl ChannelSpectrum {
    /// Build a spectrum engine for channel `ch` at sample rate `fs_hz` (the DECIMATED
    /// rate the samples arrive at) with FFT size `nfft` (must be a power of two).
    pub fn new(ch: u32, fs_hz: f64, nfft: usize) -> Self {
        assert!(
            nfft >= 2 && nfft.is_power_of_two(),
            "ChannelSpectrum: nfft must be a power of two >= 2 (got {nfft})"
        );
        let win = window::hann(nfft);
        let amp_correction = window::amplitude_correction(&win);
        Self {
            ch,
            nfft,
            fs_hz,
            win,
            amp_correction,
            acc: Vec::with_capacity(nfft),
            seq: 0,
        }
    }

    /// Push decimated samples for this channel. Emits a [`SpectrumFrame`] each time a
    /// full window fills (returning the most recent one this call produced, or `None` if
    /// no full window completed). `kind` tags the domain (Accel for raw, Vel/Disp for
    /// derived).
    pub fn push(&mut self, samples: &[f64], kind: SpectrumKind) -> Option<SpectrumFrame> {
        let mut out = None;
        for &s in samples {
            self.acc.push(s);
            if self.acc.len() == self.nfft {
                out = Some(self.transform(kind));
                self.acc.clear();
            }
        }
        out
    }

    /// Transform the current full window into a spectrum frame. `single_sided`
    /// magnitude, amplitude-corrected, interior bins scaled x2 for the one-sided fold
    /// (DC and Nyquist not doubled), so a tone reads its physical amplitude.
    fn transform(&mut self, kind: SpectrumKind) -> SpectrumFrame {
        let n = self.nfft;
        let mut buf: Vec<fft::Cplx> = (0..n).map(|i| (self.acc[i] * self.win[i], 0.0)).collect();
        fft::fft_in_place(&mut buf);
        let nout = n / 2 + 1;
        let scale = self.amp_correction / n as f64;
        let mags: Vec<f32> = (0..nout)
            .map(|k| {
                let mut m = fft::mag(buf[k]) * scale;
                let is_nyquist = k == n / 2;
                if k != 0 && !is_nyquist {
                    m *= 2.0; // one-sided fold
                }
                m as f32
            })
            .collect();
        self.seq += 1;
        SpectrumFrame {
            ch: self.ch,
            f0_hz: 0.0,
            df_hz: self.fs_hz / n as f64,
            mags,
            kind,
            seq: self.seq,
        }
    }

    /// The FFT size.
    pub fn nfft(&self) -> usize {
        self.nfft
    }
}

/// The multi-channel spectrum engine: one [`ChannelSpectrum`] per enabled channel,
/// driven from interleaved decimated telemetry. Streams live [`SpectrumFrame`]s (peak-
/// hold is UI-side).
pub struct SpectrumEngine {
    engines: Vec<ChannelSpectrum>,
    n_channels: usize,
}

impl SpectrumEngine {
    /// Build an engine for `channel_indices` (1-based, in interleave order) at the
    /// decimated `fs_hz` with FFT size `nfft`.
    pub fn new(channel_indices: &[u32], fs_hz: f64, nfft: usize) -> Self {
        let engines = channel_indices
            .iter()
            .map(|&ch| ChannelSpectrum::new(ch, fs_hz, nfft))
            .collect();
        Self {
            engines,
            n_channels: channel_indices.len(),
        }
    }

    /// The number of channels.
    pub fn n_channels(&self) -> usize {
        self.n_channels
    }

    /// Push one interleaved decimated chunk (`[c0f0, c1f0, ..., c0f1, ...]`,
    /// `n_channels` per frame). Returns any spectrum frames that completed this call
    /// (0..=n_channels of them). `kind` tags the domain.
    pub fn push_interleaved(&mut self, samples: &[f64], kind: SpectrumKind) -> Vec<SpectrumFrame> {
        if self.n_channels == 0 {
            return Vec::new();
        }
        let frames = samples.len() / self.n_channels;
        // De-interleave into per-channel buffers (small; done per chunk).
        let mut per_ch: Vec<Vec<f64>> = vec![Vec::with_capacity(frames); self.n_channels];
        for f in 0..frames {
            let base = f * self.n_channels;
            for (c, buf) in per_ch.iter_mut().enumerate() {
                buf.push(samples[base + c]);
            }
        }
        let mut out = Vec::new();
        for (engine, buf) in self.engines.iter_mut().zip(per_ch.iter()) {
            if let Some(frame) = engine.push(buf, kind) {
                out.push(frame);
            }
        }
        out
    }
}

/// One `(frequency, amplitude)` point in a resonance profile trace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfilePoint {
    /// Instantaneous estimated frequency at this sample (Hz).
    pub freq_hz: f64,
    /// Tracking-filter amplitude at that frequency (EU).
    pub amplitude: f64,
}

/// A resonance profile trace for one channel: amplitude vs instantaneous frequency,
/// accumulated during sweeps. This is the CALIBRATED resonance map;
/// its peak is the resonance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileTrace {
    /// 1-based channel index.
    pub ch: u32,
    /// The accumulated `(freq, amplitude)` points, in acquisition order.
    pub points: Vec<ProfilePoint>,
}

impl ProfileTrace {
    /// The `(freq_hz, amplitude)` of the peak amplitude point, or `None` if empty. The
    /// peak frequency is the mapped resonance.
    pub fn peak(&self) -> Option<ProfilePoint> {
        self.points
            .iter()
            .copied()
            .max_by(|a, b| a.amplitude.partial_cmp(&b.amplitude).unwrap())
    }
}

/// Seconds of the FRESHEST signal one profile level is measured over.
///
/// The operator-facing number in the capture rule (see [`ProfileTracer::observe`]). Short
/// enough that a hand-clicked dwell of a few hundred milliseconds produces a settled
/// reading, long enough that the level is measured over whole cycles of any tone in a
/// survey band rather than off a handful of samples.
pub const PROFILE_WINDOW_SECONDS: f64 = 0.100;

/// Cycles of the NEW tone skipped after its response ARRIVES, as the transient guard.
const RETUNE_GUARD_CYCLES: f64 = 3.0;

/// Acquisition blocks skipped after a retune arrives, as the other half of the transient
/// guard.
const RETUNE_GUARD_BLOCKS: usize = 2;

/// The longest window the level measurement will hold, in seconds. Only reached by a tone
/// so low that [`MIN_MEASURE_CYCLES`] of it is longer than [`PROFILE_WINDOW_SECONDS`]; it
/// bounds the buffer rather than shaping the measurement.
const MAX_WINDOW_SECONDS: f64 = 2.0;

/// Whole cycles of the tone the window must hold before a level is recorded at all.
///
/// The measurement is a mean-removed RMS over a whole number of cycles, which is EXACT for
/// a steady tone the moment it has one - but only in the limit of clean arithmetic. Three
/// cycles is the floor at which the estimate stops being dominated by where the window
/// happened to start, and it is what makes "record nothing" rather than "record a low
/// number" the answer for a dwell too short to measure (the same honesty rule the
/// traversal wipe established). A tone so low that three cycles will not fit in
/// [`MAX_WINDOW_SECONDS`] measures over whatever whole cycles the window can hold.
const MIN_MEASURE_CYCLES: f64 = 3.0;

/// A tone whose drive has been GENERATED but whose response has not reached the input yet.
///
/// The Explorer's drive is written ahead of the clock by the output lookahead contract, so
/// a retune commanded now is still `arrive_in` input frames away from being what the
/// acquisition is measuring. See [`ProfileTracer::observe_pipelined`].
#[derive(Debug, Clone, Copy)]
struct PendingTone {
    /// The commanded frequency.
    hz: f64,
    /// Input frames still to be consumed before this tone's response arrives.
    arrive_in: usize,
    /// Transient guard applied AFTER arrival, in input frames.
    settle: usize,
}

/// A per-channel resonance-profile tracer: amplitude-vs-frequency points accumulated as
/// the tone is swept or walked. Feed it the raw acceleration samples for one channel plus
/// the frequency they were acquired at.
///
/// Two entry points, for two different shapes of caller - see [`Self::observe`] (the
/// host-closes-loop service: a level per analysis pass, overwritten as the dwell
/// continues) and [`Self::push_run`] (the delegated path: one settled tracking-filter reading
/// per telemetry chunk).
pub struct ProfileTracer {
    filter: TrackingFilter,
    decimate: usize,
    counter: usize,
    trace: ProfileTrace,
    /// Sample rate the samples arrive at, Hz.
    fs_hz: f64,
    /// The acquisition block length in frames, for the transient guard's block half.
    block_frames: usize,
    /// The frequency the previous run locked to. A run at the SAME frequency continues
    /// what the previous one started rather than restarting it.
    last_run_hz: f64,
    /// Samples of the post-arrival transient still to be skipped ([`Self::observe`]).
    guard_remaining: usize,
    /// The freshest samples at the current tone, oldest first ([`Self::observe`]).
    window: VecDeque<f64>,
    /// The window's length in samples, re-derived whenever the tone moves.
    window_cap: usize,
    /// The tone the [`Self::window`] samples actually BELONG to - the one whose response
    /// is arriving at the input now, which is not the commanded frequency while a retune
    /// is still in the output pipeline. NaN before anything has arrived.
    current_hz: f64,
    /// Commanded retunes whose response has not reached the input yet, oldest first.
    /// Bounded by (pipeline / analysis-pass length), a handful of entries.
    pending: VecDeque<PendingTone>,
}

impl ProfileTracer {
    /// Build a tracer for channel `ch` at sample rate `fs_hz`, with tracking-filter
    /// bandwidth `bandwidth_hz` and a `decimate` factor (record one profile point every
    /// `decimate` samples). A bandwidth a few times the sweep rate at the filter is the
    /// standard choice (see [`sine::TrackingFilter`]).
    ///
    /// `bandwidth_hz` and `decimate` shape [`Self::push`] / [`Self::push_run`] only;
    /// [`Self::observe`] uses neither.
    pub fn new(ch: u32, fs_hz: f64, bandwidth_hz: f64, decimate: usize) -> Self {
        let mut t = Self {
            filter: TrackingFilter::new(fs_hz, bandwidth_hz),
            decimate: decimate.max(1),
            counter: 0,
            trace: ProfileTrace {
                ch,
                points: Vec::new(),
            },
            fs_hz,
            block_frames: 1,
            last_run_hz: f64::NAN,
            guard_remaining: 0,
            window: VecDeque::new(),
            window_cap: 1,
            current_hz: f64::NAN,
            pending: VecDeque::new(),
        };
        t.window_cap = t.window_capacity(f64::NAN);
        t
    }

    /// Declare the acquisition block length, in frames. One half of [`Self::observe`]'s
    /// transient guard is expressed in BLOCKS, because a guard shorter than the cadence
    /// the samples arrive in cannot skip anything; the default (1 frame) makes the cycle
    /// half the whole guard, which is right for a caller with no block structure.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = frames.max(1);
        self
    }

    /// Push one acceleration sample at the current instantaneous estimated frequency.
    /// Records a `(freq, amplitude)` point every `decimate` samples.
    ///
    /// NOTE: this per-sample form is exact only when `inst_freq_hz` varies SMOOTHLY per
    /// sample (a true continuous sweep). When telemetry arrives in chunks at a single
    /// measured frequency, prefer [`ProfileTracer::push_run`], which settles the filter
    /// over the whole chunk before recording so the tracking-filter startup transient
    /// does not spuriously peak the profile at the sweep's start frequency.
    pub fn push(&mut self, sample: f64, inst_freq_hz: f64) {
        let (amp, _phase) = self.filter.update(sample, inst_freq_hz);
        self.counter += 1;
        if self.counter >= self.decimate {
            self.counter = 0;
            self.trace.points.push(ProfilePoint {
                freq_hz: inst_freq_hz,
                amplitude: amp,
            });
        }
    }

    /// Record an externally MEASURED `(freq, amplitude)` point: the device-RMS survey path
    /// (2026-09-07), where the level is the box's own broadband RMS x sqrt 2 rather than
    /// this tracer's filter. Bounded: past 200 k points the oldest half is dropped.
    pub fn record_point(&mut self, freq_hz: f64, amplitude: f64) {
        const CAP: usize = 200_000;
        if self.trace.points.len() >= CAP {
            self.trace.points.drain(..CAP / 2);
        }
        self.trace.points.push(ProfilePoint { freq_hz, amplitude });
    }

    /// Process a whole run of samples at ONE measured frequency (a telemetry chunk is a
    /// single tone), then record ONE `(freq, settled-amplitude)` point. The filter locks
    /// on the chunk's tone as it processes the run; the amplitude recorded is the SETTLED
    /// value from the tail of the run, which rejects the two-pole low-pass startup
    /// overshoot that would otherwise spuriously peak the profile at the start frequency.
    /// This is the robust chunked-telemetry entry point for the delegated path
    /// (the delegated Explorer service), where a chunk is long against the filter's settling
    /// time. The HOST-closes-loop service does NOT use it: its analysis pass is ~30 ms,
    /// which is shorter than the filter settles, and an operator clicking the tone along
    /// at a few hertz never waits for it. That path uses [`Self::observe`].
    pub fn push_run(&mut self, samples: &[f64], inst_freq_hz: f64) {
        if samples.is_empty() {
            return;
        }
        // A telemetry chunk is a SINGLE tone at `inst_freq_hz`. Re-lock the filter to
        // this chunk's frequency from a clean phase so cross-chunk phase discontinuities
        // (the frequency jumps between chunks during a stepped/fast sweep) cannot corrupt
        // the demodulation, then settle within the chunk. A long chunk is many settling
        // times (chunk ~62 ms at 4096/65536; filter time-constant ~2/(2*pi*bw) ~= 6 ms at
        // bw 50 Hz), so the tail amplitude is the true locked amplitude.
        //
        // THE RESET IS CONDITIONAL, and the condition is the reason it exists: it guards
        // against a frequency JUMP between chunks. A chunk at the same frequency as the
        // last one has no discontinuity to guard against, and resetting there throws away
        // a filter that had already settled. That is not hypothetical - on a station whose
        // natural acquisition block is short (HERTZFORGE-SIM's is ONE FRAME) every chunk was
        // shorter than the filter's settling time, so every point recorded the startup
        // transient instead of the amplitude: a dwell read ~0 while the drive was live.
        // Continuing the filter across chunks at a held tone is what makes a DWELL - the
        // manual-walk case the peak profile is for - measure the tone rather than the
        // filter's own startup.
        if (inst_freq_hz - self.last_run_hz).abs() > f64::EPSILON {
            self.filter.reset();
        }
        self.last_run_hz = inst_freq_hz;
        let mut amp = 0.0f64;
        // The last quarter of the run, and at least the final sample: `len - len/4` alone
        // is `len` for `len < 4`, which selects NOTHING and records a hard zero.
        let settle_tail = samples.len().saturating_sub((samples.len() / 4).max(1));
        for (i, &s) in samples.iter().enumerate() {
            let (a, _phase) = self.filter.update(s, inst_freq_hz);
            if i >= settle_tail {
                amp = amp.max(a);
            }
        }
        self.trace.points.push(ProfilePoint {
            freq_hz: inst_freq_hz,
            amplitude: amp,
        });
    }

    /// Record the level at the tone the acquisition is CURRENTLY carrying, overwriting the
    /// point already held for that frequency. The Explorer/SMART survey capture.
    ///
    /// The zero-pipeline form of [`Self::observe_pipelined`]: correct for a caller whose
    /// drive reaches the input immediately, and the shape every unit test in this module
    /// uses. **The host-closes-loop service must NOT call this** - its drive is written
    /// ahead of the clock by the output lookahead contract, which is the whole subject of
    /// the pipelined form.
    pub fn observe(&mut self, samples: &[f64], inst_freq_hz: f64, sweeping: bool) -> bool {
        self.observe_pipelined(samples, inst_freq_hz, 0, sweeping)
    }

    /// Record the survey level, with the capture aligned to when each commanded tone
    /// actually REACHES the input rather than to when it was commanded.
    ///
    /// `pipeline_frames` is how many further input frames still carry the PREVIOUS tone at
    /// the instant of a retune - the engine's output queue, the device's output FIFO, and
    /// the acquisition path's own group delay, all of which the service computes as
    /// `(frames_generated - frames_in) + group_delay` (see
    /// `ExplorerHostService::pipeline_frames`). Pass `0` and this is exactly
    /// [`Self::observe`].
    ///
    /// # The rule, stated once
    ///
    /// 1. **A retune is queued at its ARRIVAL, not at its command.** The commanded tone
    ///    goes into a pending list `pipeline_frames` of input ahead; until the input
    ///    reaches that boundary the samples still belong to the tone before it, and they
    ///    are measured as such.
    /// 2. **FINALIZE ON DEPARTURE.** The outgoing tone keeps taking samples right up to
    ///    that boundary - PAST the operator's click, because those samples are still its
    ///    response - and the last of them writes its point. So a dwell is measured over
    ///    its whole length however soon the next click comes.
    /// 3. **A SHORT transient guard follows arrival**: `max(2 acquisition blocks, 3 cycles
    ///    of the new tone)` input frames are skipped and the window emptied. That is the
    ///    plant's own settling at the new tone, and nothing else - the pipeline is rule 1's
    ///    job, not this one's.
    /// 4. **Every pass past the guard records**, the level being the RMS of the freshest
    ///    [`PROFILE_WINDOW_SECONDS`] (snapped down to whole cycles, window mean removed)
    ///    times `sqrt(2)`. A window holding fewer than [`MIN_MEASURE_CYCLES`] whole cycles
    ///    records NOTHING rather than a low number.
    /// 5. **The point for a frequency is OVERWRITTEN, not appended.** A longer dwell keeps
    ///    refining the same point.
    ///
    /// `sweeping` says the tone is under a COMMANDED continuous travel. A swept tone moves
    /// every pass by construction, so treating its motion as a retune would suppress the
    /// whole swept survey; the window then spans the recent travel, which is what a swept
    /// measurement is. A tone that moves while NOT sweeping was RETUNED - by an operator
    /// click, or by the slew walking toward a new target - and that is rule 1's case.
    ///
    /// **KNOWN, and NOT fixed here:** a SWEPT survey is still recorded against the
    /// COMMANDED frequency, so its curve is shifted by the pipeline in frequency (about
    /// 0.12 octaves at 0.5 oct/s against the chassis's 240 ms). Same root cause, different
    /// correction - the swept case needs the frequency the tone HAD `pipeline_frames` ago,
    /// which is a history this type does not keep. Left alone deliberately rather than
    /// half-corrected.
    ///
    /// Returns whether a point was recorded this call.
    ///
    /// # Why not the tracking filter here
    ///
    /// [`Self::push_run`]'s settled tail-max is a tracking-filter reading, and a tracking
    /// filter cannot answer faster than it settles: at the bandwidths a survey uses that
    /// is a couple of hundred milliseconds, which is the whole of a hand-clicked dwell.
    /// MEASURED against the operator's workflow (2026-08-08): with clicks arriving ~3/s
    /// the profile never reached the plant's level before the tone moved on, and the
    /// remembered curve read low everywhere. A short-window RMS has no settling time of
    /// its own - it is exact for a steady tone the moment the window is full - and it
    /// trades the filter's out-of-band rejection for that. The trade is the point: the
    /// operator is reading a resonance survey off the curve, and a curve that lags the
    /// clicks is not one.
    ///
    /// # Why the pipeline term had to be added on top of that
    ///
    /// MEASURED 2026-08-08 (`tests/pipeline_alignment.rs`), against a two-mode plant
    /// behind a 250 ms pipeline: a 5 clicks/s walk recorded, at each stop, the level of a
    /// stop it had already left - 275 Hz read 0.429 V where a paused walk read 1.504 V,
    /// and 0.429 V is what the paused walk read at 190 Hz. The curve was the plant's own
    /// response shifted back by the pipeline. **A flat jumper cannot see this**: a
    /// mean-removed RMS of a sine is its amplitude whatever its frequency, so the same
    /// walk on a wire came out within 0.6 % at every click rate. The plant has to have
    /// structure before the misattribution has anything to show (a result that cannot
    /// fail is not evidence).
    ///
    /// # The threshold, stated because the defect has one
    ///
    /// The point for a frequency is overwritten every pass, so a contaminated early window
    /// is flushed by later clean ones. The last level written before the operator clicks
    /// away is wrong only while `dwell < pipeline + measurement window`. On the NI chassis
    /// that is 64.6 + 100 = **165 ms, i.e. above about 6 clicks/s**; below that rate the
    /// old rule already came out right, which is exactly why the curve looked correct
    /// wherever the operator paused.
    pub fn observe_pipelined(
        &mut self,
        samples: &[f64],
        inst_freq_hz: f64,
        pipeline_frames: usize,
        sweeping: bool,
    ) -> bool {
        // NaN-safe "moved". The FIRST observation must queue a tone too, and its delta
        // against the NaN seed is NaN, which compares false against everything - so the
        // finiteness is stated rather than left to how `!` and NaN happen to interact.
        let delta = (inst_freq_hz - self.last_run_hz).abs();
        let moved = !(delta.is_finite() && delta <= f64::EPSILON);
        let live = inst_freq_hz.is_finite() && inst_freq_hz > 0.0;
        if moved {
            self.last_run_hz = inst_freq_hz;
            if live && sweeping {
                // A swept tone's motion is not a retune: it moves every pass by
                // construction and there is no transient to skip. Anything the operator
                // left pending by clicking and then sweeping is superseded by the sweep.
                self.pending.clear();
                self.current_hz = inst_freq_hz;
                self.window_cap = self.window_capacity(inst_freq_hz);
            } else if live {
                self.pending.push_back(PendingTone {
                    hz: inst_freq_hz,
                    // The retune landed SOMEWHERE inside the drive this pass generated, so
                    // the true arrival is between `pipeline_frames` and
                    // `pipeline_frames + samples.len()` from here. Anchoring at the near
                    // end keeps the OUTGOING tone's window clean (rule 2 never reaches
                    // past it), and the settle guard - which is at least one acquisition
                    // block, i.e. at least half a pass - absorbs the ambiguity on the
                    // incoming side.
                    arrive_in: pipeline_frames,
                    settle: self.guard_samples(inst_freq_hz),
                });
            }
        }
        if !live {
            // A parked oscillator (0 Hz at 0 V) is not a survey tone. Hold nothing: the
            // samples belong to no frequency, and keeping them would let them leak into
            // the first window after the tone comes back. Anything still in the pipeline
            // was drive that is no longer being played.
            self.window.clear();
            self.pending.clear();
            self.current_hz = f64::NAN;
            self.guard_remaining = 0;
            return false;
        }

        let mut recorded = false;
        let mut rest = samples;
        loop {
            let boundary = self
                .pending
                .front()
                .map(|p| p.arrive_in)
                .unwrap_or(usize::MAX);
            if boundary > rest.len() {
                recorded |= self.absorb(rest);
                let consumed = rest.len();
                for q in self.pending.iter_mut() {
                    q.arrive_in -= consumed;
                }
                break;
            }
            // THE DEPARTURE. The outgoing tone owns `boundary` more frames - samples that
            // arrived after the operator clicked away and are still its response - and
            // this absorb is its finalisation: `absorb` ends by writing the point from
            // whatever window it leaves behind, so the last thing recorded for a tone is
            // the freshest CLEAN window it ever had. An extra `record_current()` here
            // would be exactly that call again; it was written, MEASURED to be redundant
            // by deleting it (2026-08-08, no test moved), and removed rather than left as
            // a line a reader would take for load-bearing.
            recorded |= self.absorb(&rest[..boundary]);
            let arrived = self
                .pending
                .pop_front()
                .expect("the boundary was read off the front");
            for q in self.pending.iter_mut() {
                q.arrive_in -= boundary;
            }
            self.current_hz = arrived.hz;
            self.window.clear();
            self.window_cap = self.window_capacity(arrived.hz);
            self.guard_remaining = arrived.settle;
            rest = &rest[boundary..];
        }
        recorded
    }

    /// Take `chunk` into the current tone's window, spending the settle guard first, and
    /// record the level if the window now qualifies.
    fn absorb(&mut self, chunk: &[f64]) -> bool {
        if !(self.current_hz.is_finite() && self.current_hz > 0.0) {
            // Nothing has ARRIVED yet: these samples are the response to drive from before
            // this run's first commanded tone, and they belong to no survey frequency.
            return false;
        }
        if chunk.is_empty() {
            // Not a no-op: a pass that drained nothing still refreshes the point from the
            // window already held, which is what keeps a dwell's value live between
            // acquisitions.
            return self.record_current();
        }
        let skipped = self.guard_remaining.min(chunk.len());
        self.guard_remaining -= skipped;
        self.window.extend(chunk[skipped..].iter().copied());
        while self.window.len() > self.window_cap {
            self.window.pop_front();
        }
        if self.guard_remaining > 0 {
            return false;
        }
        self.record_current()
    }

    /// Write the current tone's level from the window as it stands, overwriting the point
    /// already held for that frequency. Returns whether a point was recorded.
    fn record_current(&mut self) -> bool {
        if !(self.current_hz.is_finite() && self.current_hz > 0.0) || self.guard_remaining > 0 {
            return false;
        }
        let f = self.current_hz;
        let Some(amp) = self.window_amplitude(f) else {
            return false;
        };
        match self.trace.points.last_mut() {
            Some(p) if (p.freq_hz - f).abs() <= f64::EPSILON => p.amplitude = amp,
            _ => self.trace.points.push(ProfilePoint {
                freq_hz: f,
                amplitude: amp,
            }),
        }
        true
    }

    /// The transient guard applied once a retune to `freq_hz` has ARRIVED, in samples.
    fn guard_samples(&self, freq_hz: f64) -> usize {
        let blocks = RETUNE_GUARD_BLOCKS * self.block_frames.max(1);
        let cycles = if freq_hz.is_finite() && freq_hz > 0.0 && self.fs_hz.is_finite() {
            (RETUNE_GUARD_CYCLES * self.fs_hz / freq_hz).ceil().max(0.0) as usize
        } else {
            0
        };
        blocks.max(cycles)
    }

    /// The measurement window for a tone at `freq_hz`, in samples: [`PROFILE_WINDOW_SECONDS`],
    /// widened so a tone too low to fit [`MIN_MEASURE_CYCLES`] cycles in that time still
    /// has somewhere to put them, and bounded by [`MAX_WINDOW_SECONDS`].
    fn window_capacity(&self, freq_hz: f64) -> usize {
        if !(self.fs_hz.is_finite() && self.fs_hz > 0.0) {
            return 1;
        }
        let nominal = (PROFILE_WINDOW_SECONDS * self.fs_hz).round().max(1.0);
        let min_cycles = if freq_hz.is_finite() && freq_hz > 0.0 {
            (MIN_MEASURE_CYCLES * self.fs_hz / freq_hz).ceil()
        } else {
            0.0
        };
        let cap = (MAX_WINDOW_SECONDS * self.fs_hz).max(1.0);
        nominal.max(min_cycles).min(cap) as usize
    }

    /// The tone amplitude implied by the current window: mean-removed RMS over a WHOLE
    /// number of cycles, times `sqrt(2)`. `None` while the window holds too little to
    /// measure.
    ///
    /// Whole cycles because the RMS of a fractional cycle carries a ripple that depends on
    /// where the window happened to start - at 20 Hz in a 100 ms window that is two cycles
    /// and several percent, which would read as a wobble in the survey curve that no plant
    /// produced. The mean is removed because a DC offset on the control accelerometer is
    /// not part of the response at the tone.
    fn window_amplitude(&self, freq_hz: f64) -> Option<f64> {
        let n = self.window.len();
        if n == 0 {
            return None;
        }
        let cycle = self.fs_hz / freq_hz;
        // TOO LITTLE TO MEASURE is answered with nothing, never with the number a partial
        // cycle happens to produce - which is arbitrarily low and would land in the
        // operator's saved curve as a measured value.
        let min_samples = if cycle.is_finite() && cycle >= 1.0 {
            ((MIN_MEASURE_CYCLES * cycle).ceil() as usize).min(self.window_cap)
        } else {
            1
        };
        if n < min_samples {
            return None;
        }
        let m = if cycle.is_finite() && cycle >= 1.0 && (n as f64) >= cycle {
            let whole = ((n as f64 / cycle).floor() * cycle).round() as usize;
            whole.clamp(1, n)
        } else {
            n
        };
        let start = n - m;
        let mut sum = 0.0f64;
        for i in start..n {
            sum += self.window[i];
        }
        let mean = sum / m as f64;
        let mut sq = 0.0f64;
        for i in start..n {
            let d = self.window[i] - mean;
            sq += d * d;
        }
        let amp = (sq / m as f64).sqrt() * std::f64::consts::SQRT_2;
        amp.is_finite().then_some(amp)
    }

    /// The accumulated profile trace (borrowed).
    pub fn trace(&self) -> &ProfileTrace {
        &self.trace
    }

    /// Take the accumulated profile trace, leaving an empty one behind.
    pub fn take_trace(&mut self) -> ProfileTrace {
        std::mem::take(&mut self.trace)
    }

    /// Drain the accumulated POINTS, leaving the trace's channel identity in place.
    ///
    /// [`Self::take_trace`] swaps in `ProfileTrace::default()`, whose `ch` is 0. That is
    /// fine for the one-shot end-of-run report it was written for and wrong for a caller
    /// that drains REPEATEDLY: every take after the first would answer for "channel 0".
    /// A streaming consumer (the daemon's peak-profile raster) wants the points since its
    /// last read and nothing else, so it gets its own accessor rather than a second
    /// meaning bolted onto that one.
    ///
    /// Draining is also what BOUNDS this vector. [`Self::observe`] overwrites the point it
    /// holds for the CURRENT tone, so a dwell adds nothing, but every visited frequency
    /// adds one - and `push_run` appends one point per chunk unconditionally. A survey
    /// that is never read grows either way. The raster is the thing that remembers; the
    /// tracer only has to carry what has not been rastered yet.
    pub fn take_points(&mut self) -> Vec<ProfilePoint> {
        std::mem::take(&mut self.trace.points)
    }
}

/// A per-channel derived velocity/displacement tracer. Integrates acceleration with a
/// high-pass (matching the abort integrator / [`hertzforge_dsp::integrate`]) and exposes a
/// SETTLING flag for the first N high-pass time constants so a UI does not plot the
/// startup transient as real motion.
pub struct DerivedTracer {
    integ: ChannelIntegrator,
    settle_remaining: u64,
    settle_frames: u64,
    frames_seen: u64,
    last_vel_ips: f64,
    last_disp_mil: f64,
}

impl DerivedTracer {
    /// Build a derived tracer at `fs_hz` with high-pass corner `hp_hz`. The settle window
    /// spans `settle_hp_constants` high-pass time constants (a typical value is 5).
    pub fn new(fs_hz: f64, hp_hz: f64, settle_hp_constants: f64) -> Self {
        let tc_frames = if hp_hz > 0.0 {
            (fs_hz / (std::f64::consts::TAU * hp_hz)).max(0.0)
        } else {
            0.0
        };
        let settle_frames = (settle_hp_constants * tc_frames).round().max(0.0) as u64;
        Self {
            integ: ChannelIntegrator::new(fs_hz, hp_hz),
            settle_remaining: settle_frames,
            settle_frames,
            frames_seen: 0,
            last_vel_ips: 0.0,
            last_disp_mil: 0.0,
        }
    }

    /// Push one acceleration sample (g). Returns `(velocity_ips, displacement_mil)`.
    pub fn push(&mut self, accel_g: f64) -> (f64, f64) {
        self.frames_seen += 1;
        if self.settle_remaining > 0 {
            self.settle_remaining -= 1;
        }
        let (vel, disp) = self.integ.push(accel_g);
        self.last_vel_ips = vel;
        self.last_disp_mil = disp;
        (vel, disp)
    }

    /// True while the integrator settle window is still open.
    pub fn settling(&self) -> bool {
        self.settle_remaining > 0
    }

    /// The settle window length in frames.
    pub fn settle_frames(&self) -> u64 {
        self.settle_frames
    }

    /// The most recent derived velocity (in/s) and displacement (mil).
    pub fn last(&self) -> (f64, f64) {
        (self.last_vel_ips, self.last_disp_mil)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn spectrum_tone_lands_in_the_right_bin_at_its_amplitude() {
        let fs = 5120.0;
        let nfft = 1024;
        let mut spec = ChannelSpectrum::new(1, fs, nfft);
        // A pure 200 Hz tone of amplitude 2.0 -> a peak in the bin nearest 200 Hz whose
        // magnitude is ~2.0 (Hann-amplitude-corrected).
        let freq = 200.0;
        let amp = 2.0;
        let mut frame = None;
        for k in 0..nfft {
            let s = amp * (TAU * freq * k as f64 / fs).sin();
            if let Some(f) = spec.push(&[s], SpectrumKind::Accel) {
                frame = Some(f);
            }
        }
        let frame = frame.expect("a full window produced a frame");
        assert_eq!(frame.kind, SpectrumKind::Accel);
        let df = frame.df_hz;
        let peak_bin = (freq / df).round() as usize;
        // Peak bin magnitude ~ amplitude.
        let m = frame.mags[peak_bin];
        assert!(
            (m - amp as f32).abs() < 0.3,
            "peak bin mag {m}, want ~{amp}"
        );
        // The peak is genuinely at that bin (dominant).
        let (argmax, _) = frame
            .mags
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert!((argmax as i64 - peak_bin as i64).abs() <= 1);
    }

    #[test]
    fn spectrum_engine_streams_per_channel_frames() {
        let fs = 5120.0;
        let nfft = 256;
        let mut eng = SpectrumEngine::new(&[1, 2], fs, nfft);
        assert_eq!(eng.n_channels(), 2);
        // Feed exactly one window's worth of interleaved samples (2 channels).
        let mut inter = Vec::new();
        for k in 0..nfft {
            let a = (TAU * 100.0 * k as f64 / fs).sin();
            let b = (TAU * 300.0 * k as f64 / fs).sin();
            inter.push(a);
            inter.push(b);
        }
        let frames = eng.push_interleaved(&inter, SpectrumKind::Accel);
        assert_eq!(frames.len(), 2, "one frame per channel per full window");
        assert_eq!(frames[0].ch, 1);
        assert_eq!(frames[1].ch, 2);
    }

    #[test]
    fn profile_trace_peaks_near_the_tracked_resonance() {
        // Simulate a sweep across a resonance: amplitude peaks when the swept frequency
        // matches a synthetic resonance at 250 Hz. The tracker follows amplitude vs the
        // instantaneous frequency, so the profile peak should land near 250 Hz.
        let fs = 65536.0;
        let mut tracer = ProfileTracer::new(1, fs, 50.0, 64);
        let f_res = 250.0;
        let q = 15.0;
        // Sweep 100 -> 500 Hz slowly; response amplitude ~ resonance magnitude.
        let n = 400_000usize;
        let mut phase = 0.0f64;
        for k in 0..n {
            let frac = k as f64 / n as f64;
            let f = 100.0 + 400.0 * frac;
            // 2nd-order resonance magnitude at f (normalized).
            let r = f / f_res;
            let mag = 1.0 / ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt();
            phase += TAU * f / fs;
            let s = mag * phase.sin();
            tracer.push(s, f);
        }
        let peak = tracer.trace().peak().expect("profile has points");
        assert!(
            (peak.freq_hz - f_res).abs() < 30.0,
            "profile peak at {} Hz, want ~{f_res}",
            peak.freq_hz
        );
    }

    /// The shipped chassis: the 9234 coerces a requested 8192 Hz to 8533.33, and the
    /// engine's block there is ~256 frames (~30 ms).
    const SIM_FS: f64 = 13_107_200.0 / (256.0 * 6.0);
    const SIM_BLOCK: usize = 256;

    /// A second-order resonance's amplitude at `f`: the plant the click sequence walks.
    fn plant_amplitude(f: f64) -> f64 {
        let (f_res, q) = (250.0, 15.0);
        let r = f / f_res;
        1.0 / ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt()
    }

    /// A source that generates the plant's response continuously, with the amplitude
    /// approaching its new steady value over a couple of cycles after a retune - so a
    /// capture that ignored the transient guard would read the ramp, not the plant.
    struct Plant {
        phase: f64,
        amp: f64,
    }

    impl Plant {
        fn new() -> Self {
            Plant {
                phase: 0.0,
                amp: 0.0,
            }
        }
        /// `n` samples at `f`, appended to `out`.
        fn run(&mut self, f: f64, n: usize, out: &mut Vec<f64>) {
            let target = plant_amplitude(f);
            // One time constant = two cycles of the tone.
            let alpha = 1.0 - (-1.0f64 / (2.0 * SIM_FS / f)).exp();
            out.clear();
            for _ in 0..n {
                self.amp += alpha * (target - self.amp);
                self.phase += TAU * f / SIM_FS;
                out.push(self.amp * self.phase.sin());
            }
        }
    }

    /// THE ACCEPTANCE CASE (operator, 2026-08-08: "the reference controller creates a smooth
    /// curve with rapid clicks"). Three retunes a second - a ~333 ms dwell each - and EVERY visited
    /// frequency must come out of the survey with a level, at the plant's steady
    /// amplitude for that frequency.
    ///
    /// RED-MUTATION: restore the settled-tail-max capture (one tracking-filter reading per
    /// pass, filter reset on every retune) and this reds on the VALUES - a 5 Hz-bandwidth
    /// filter is still climbing when the dwell ends, so every point reads low.
    #[test]
    fn a_three_click_per_second_walk_lands_every_frequency_at_the_plant_level() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let mut plant = Plant::new();
        let mut run = Vec::new();
        // Two ticks per analysis pass is the service's batch, so a pass is 2 blocks.
        let pass_frames = 2 * SIM_BLOCK;
        let passes_per_dwell = ((0.333 * SIM_FS) / pass_frames as f64).round() as usize;
        assert!(passes_per_dwell >= 4, "a 333 ms dwell is several passes");

        let clicks = [80.0, 150.0, 220.0, 250.0, 300.0, 500.0, 900.0];
        for &f in clicks.iter() {
            for _ in 0..passes_per_dwell {
                plant.run(f, pass_frames, &mut run);
                tracer.observe(&run, f, false);
            }
        }

        let points = &tracer.trace().points;
        assert_eq!(
            points.len(),
            clicks.len(),
            "one point per VISITED FREQUENCY - a dwell overwrites its own point"
        );
        for (p, &f) in points.iter().zip(clicks.iter()) {
            assert!(
                (p.freq_hz - f).abs() < 1e-9,
                "point {p:?} is not at the visited {f} Hz"
            );
            let want = plant_amplitude(f);
            let err = (p.amplitude - want).abs() / want;
            assert!(
                err < 0.03,
                "{f} Hz read {} , plant is {want} ({:.1}% off)",
                p.amplitude,
                err * 100.0
            );
        }
    }

    /// The same walk with the SLEW modelled: an operator click sets a target and the tone
    /// TRAVELS to it at the frequency slew, so a pass mid-travel carries a tone that was
    /// at a different frequency for most of it.
    ///
    /// Two claims, and the first is the one the guard exists for: **nothing is recorded
    /// while the tone is travelling** (a level attributed to a frequency the tone was only
    /// passing through is a measurement nobody took, and the raster would paint it), and
    /// every frequency the tone LANDED on gets its level.
    ///
    /// FLOOR, stated rather than hidden: a landed tone needs the guard plus one analysis
    /// pass of dwell before it can produce a point. A coarse click at a low frequency slew
    /// can spend most of a fast click cadence travelling and leave less than that - which
    /// is a slew setting, not a capture rule (see the Freq slew help text).
    #[test]
    fn a_slewed_walk_records_where_the_tone_landed_and_nowhere_between() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let pass_frames = 2 * SIM_BLOCK;
        let passes_per_click = ((0.333 * SIM_FS) / pass_frames as f64).round() as usize;
        // 1 oct/s, the shipped default, applied per frame exactly as the step does it.
        let per_frame = 2f64.powf(1.0 / SIM_FS);
        let targets: Vec<f64> = (0..6).map(|i| 200.0 + 10.0 * (i + 1) as f64).collect();

        let mut f = 200.0;
        let mut phase = 0.0f64;
        let mut run = Vec::with_capacity(pass_frames);
        for &target in targets.iter() {
            for _ in 0..passes_per_click {
                run.clear();
                for _ in 0..pass_frames {
                    f = (f * per_frame).min(target);
                    phase += TAU * f / SIM_FS;
                    run.push(plant_amplitude(f) * phase.sin());
                }
                tracer.observe(&run, f, false);
            }
        }

        let points = tracer.trace().points.clone();
        assert!(!points.is_empty());
        for p in points.iter() {
            assert!(
                targets.iter().any(|t| (t - p.freq_hz).abs() < 1e-9),
                "{p:?} is at a frequency the tone only travelled through"
            );
            let want = plant_amplitude(p.freq_hz);
            let err = (p.amplitude - want).abs() / want;
            assert!(
                err < 0.05,
                "{p:?} vs plant {want} ({:.1}% off)",
                err * 100.0
            );
        }
        for t in targets.iter() {
            assert!(
                points.iter().any(|p| (p.freq_hz - t).abs() < 1e-9),
                "no point at the landed frequency {t} Hz"
            );
        }
    }

    /// The guard is SHORT, and it is a skip rather than a wait: the FIRST pass after a
    /// retune is inside it (2 blocks is exactly one pass), and the very next one records.
    #[test]
    fn the_transient_guard_is_one_pass_then_every_pass_records() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let mut plant = Plant::new();
        let mut run = Vec::new();
        let pass_frames = 2 * SIM_BLOCK;
        plant.run(300.0, pass_frames, &mut run);
        assert!(
            !tracer.observe(&run, 300.0, false),
            "the first pass after a retune is the transient guard"
        );
        assert!(tracer.trace().points.is_empty());
        for i in 0..4 {
            plant.run(300.0, pass_frames, &mut run);
            assert!(
                tracer.observe(&run, 300.0, false),
                "pass {} past the guard must record",
                i + 2
            );
        }
        assert_eq!(tracer.trace().points.len(), 1, "the same point, refined");
    }

    /// A LONGER DWELL KEEPS REFINING the same point. The distinguishing case is a level
    /// that CHANGES while the tone is held (the operator raising the drive during a
    /// dwell), because that is where a capture-once-per-dwell rule and a
    /// record-every-pass-and-overwrite rule give different answers.
    ///
    /// RED-MUTATION: record only on the first post-guard pass (or max-hold the point) and
    /// this reds still reading the OLD level after the drive was raised.
    #[test]
    fn a_dwell_follows_a_level_change_without_a_retune() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let pass_frames = 2 * SIM_BLOCK;
        let f = 250.0;
        let mut phase = 0.0f64;
        let mut pass = |tracer: &mut ProfileTracer, amp: f64| {
            let run: Vec<f64> = (0..pass_frames)
                .map(|_| {
                    let s = amp * phase.sin();
                    phase += TAU * f / SIM_FS;
                    s
                })
                .collect();
            tracer.observe(&run, f, false);
        };
        for _ in 0..4 {
            pass(&mut tracer, 1.0);
        }
        let held = tracer.trace().points[0].amplitude;
        assert!((held - 1.0).abs() < 0.03, "the held level reads {held}");
        // The operator raises the drive. No retune, so no guard - the window simply
        // refills, and ~100 ms later the point is the new level.
        for _ in 0..4 {
            pass(&mut tracer, 2.0);
        }
        assert_eq!(
            tracer.trace().points.len(),
            1,
            "still ONE point at this tone"
        );
        let after = tracer.trace().points[0].amplitude;
        assert!(
            (after - 2.0).abs() < 0.03,
            "the point followed the level change to {after}, want 2.0"
        );
    }

    /// A SWEPT tone moves every pass by construction. Its motion must NOT arm the guard,
    /// or a swept survey would record nothing at all.
    ///
    /// RED-MUTATION: drop the `sweeping` argument's effect (arm the guard on any move) and
    /// this reds with zero points.
    #[test]
    fn a_swept_tone_keeps_recording_while_it_travels() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let mut plant = Plant::new();
        let mut run = Vec::new();
        let pass_frames = 2 * SIM_BLOCK;
        // 1 oct/min from 100 Hz: the tone advances a little every pass.
        let mut f = 100.0;
        let per_pass = 2f64.powf((pass_frames as f64 / SIM_FS) / 60.0);
        let mut recorded = 0;
        for _ in 0..40 {
            plant.run(f, pass_frames, &mut run);
            if tracer.observe(&run, f, true) {
                recorded += 1;
            }
            f *= per_pass;
        }
        assert!(
            recorded >= 38,
            "a sweep records on nearly every pass, got {recorded}/40"
        );
    }

    /// Walk `plan` (one `(frequency, amplitude)` per click) through a tracer, with a
    /// PIPELINE of `pipe` input frames between each click and the samples that answer it,
    /// dwelling `dwell_passes` analysis passes on each stop. Returns the tracer.
    ///
    /// This is the stream a pipelined station really hands the tracer: the COMMANDED
    /// frequency changes at the click, the SAMPLES change over `pipe` frames later. Each
    /// stop is given its own amplitude, so a window that took the wrong tone says so in
    /// the value rather than only in the frequency.
    ///
    /// **The dwell has to be SHORT for this to test anything**, and that is the finding
    /// the bench instrumentation produced: the point for a frequency is overwritten on
    /// every pass, so a contaminated early window is flushed by later clean ones and a
    /// LONG dwell comes out right whatever the alignment rule is. The defect survives only
    /// when the tone DEPARTS while the window still straddles the click, i.e. while
    /// `dwell < pipeline + window`. A test that dwelt longer would pass with the pipeline
    /// term deleted - which the first cut of these tests did, on 2026-08-08.
    fn pipelined_walk(plan: &[(f64, f64)], pipe: usize, dwell_passes: usize) -> ProfileTracer {
        let pass_frames = 2 * SIM_BLOCK;
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let mut phase = 0.0f64;
        let mut arriving: VecDeque<(usize, f64, f64)> = VecDeque::new();
        let (mut cur_f, mut cur_a) = plan[0];
        let mut run = Vec::with_capacity(pass_frames);
        let mut frame = 0usize;
        let mut commanded = plan[0].0;
        // The first stop's tone is already on the wire when the walk starts, so only the
        // clicks AFTER it are pipelined. Every stop then gets the same treatment.
        for (i, &(f, a)) in plan.iter().enumerate() {
            commanded = f;
            if i > 0 {
                arriving.push_back((frame + pipe, f, a));
            }
            for _ in 0..dwell_passes {
                run.clear();
                for _ in 0..pass_frames {
                    while arriving
                        .front()
                        .map(|&(at, _, _)| at <= frame)
                        .unwrap_or(false)
                    {
                        let (_, nf, na) = arriving.pop_front().expect("checked");
                        cur_f = nf;
                        cur_a = na;
                    }
                    run.push(cur_a * phase.sin());
                    phase += TAU * cur_f / SIM_FS;
                    frame += 1;
                }
                tracer.observe_pipelined(&run, commanded, pipe, false);
            }
        }
        // The walk ends and the operator sits on the last stop: its own dwell, which is
        // the departure every earlier stop already got from the click that followed it.
        for _ in 0..(dwell_passes + 2 * pipe.div_ceil(pass_frames) + 2) {
            run.clear();
            for _ in 0..pass_frames {
                while arriving
                    .front()
                    .map(|&(at, _, _)| at <= frame)
                    .unwrap_or(false)
                {
                    let (_, nf, na) = arriving.pop_front().expect("checked");
                    cur_f = nf;
                    cur_a = na;
                }
                run.push(cur_a * phase.sin());
                phase += TAU * cur_f / SIM_FS;
                frame += 1;
            }
            tracer.observe_pipelined(&run, commanded, pipe, false);
        }
        tracer
    }

    /// The walk this file's two pipeline properties are asserted over. Distinct
    /// amplitudes, none a multiple of another, so a borrowed level cannot pass as its
    /// neighbour's.
    const WALK: [(f64, f64); 5] = [
        (150.0, 1.0),
        (330.0, 4.0),
        (520.0, 0.6),
        (700.0, 2.5),
        (880.0, 1.7),
    ];
    /// Two analysis passes of pipeline, which is the chassis's own order (240 ms against a
    /// 120 ms pass).
    const WALK_PIPE: usize = 4 * SIM_BLOCK;
    /// Three passes of dwell = 1536 frames, which is under `WALK_PIPE (1024) + the 853
    /// sample window` - the band in which the alignment rule is falsifiable. See
    /// [`pipelined_walk`].
    const WALK_DWELL_PASSES: usize = 3;

    /// **THE PIPELINE RULE.** A retune is measured from when its tone REACHES THE INPUT,
    /// not from when it was commanded.
    ///
    /// RED-MUTATION, RUN 2026-08-08: replace `arrive_in: pipeline_frames` with
    /// `arrive_in: 0` in `observe_pipelined` (which is exactly what aligning to the
    /// COMMAND means) and every stop past the first records the amplitude of the stop
    /// BEFORE it - the operator's defect, reproduced in eleven lines.
    #[test]
    fn a_retune_is_measured_from_when_its_tone_arrives_not_when_it_was_commanded() {
        let tracer = pipelined_walk(&WALK, WALK_PIPE, WALK_DWELL_PASSES);
        let points = tracer.trace().points.clone();
        for &(f, a) in WALK.iter() {
            let p = points
                .iter()
                .find(|p| (p.freq_hz - f).abs() < 1e-9)
                .unwrap_or_else(|| panic!("no point at {f} Hz: {points:?}"));
            assert!(
                (p.amplitude - a).abs() / a < 0.05,
                "{f} Hz recorded {} against {a} - a window aligned to the COMMAND measures \
                 the tone it displaced",
                p.amplitude
            );
        }
    }

    /// **FINALIZE ON DEPARTURE.** The samples that arrive AFTER the operator clicks away
    /// are still the departing tone's response, and they are what its point is written
    /// from - so every stop of a fast walk gets a full-dwell measurement rather than
    /// nothing.
    ///
    /// The dwell here is shorter than the pipeline plus the guard, so a rule that stopped
    /// measuring a tone at the moment the operator clicked away would have nothing left to
    /// record for ANY stop but the last. Every stop having a point is the claim.
    ///
    /// RED-MUTATION, RUN 2026-08-08: apply the OBVIOUS half-fix - keep the guard anchored
    /// to the command and just make it longer (`arrive_in: 0`, `settle: pipeline_frames +
    /// guard_samples(..)`) - and this walk records ONE point, at the last stop. That is
    /// what the pipeline term costs without the departure half, and it is why the fix is
    /// two halves rather than a bigger number in the guard.
    #[test]
    fn every_stop_of_a_fast_walk_keeps_a_point() {
        let tracer = pipelined_walk(&WALK, WALK_PIPE, WALK_DWELL_PASSES);
        let points = tracer.trace().points.clone();
        for &(f, _) in WALK.iter() {
            assert!(
                points.iter().any(|p| (p.freq_hz - f).abs() < 1e-9),
                "no point at {f} Hz after a {WALK_DWELL_PASSES}-pass dwell behind a \
                 {WALK_PIPE}-frame pipeline: {points:?}"
            );
        }
        // And NOTHING is recorded at a frequency the walk never stopped at.
        for p in points.iter() {
            assert!(
                WALK.iter().any(|&(f, _)| (f - p.freq_hz).abs() < 1e-9),
                "{p:?} is at a frequency the walk never stopped at"
            );
        }
    }

    /// A dwell too short to hold [`MIN_MEASURE_CYCLES`] whole cycles records NOTHING. The
    /// alternative is the number a partial cycle happens to produce, which is arbitrarily
    /// low and would land in the operator's saved curve as a measurement he took.
    ///
    /// Same honesty rule as the traversal wipe: absent beats wrong.
    #[test]
    fn a_window_too_short_to_measure_records_nothing_rather_than_a_low_value() {
        // 30 Hz: one cycle is 284 samples at this rate, so three cycles is 853 - more than
        // the 512-frame pass. The guard eats the first pass, the second gives 512, and
        // that is still short of three cycles.
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let pass_frames = 2 * SIM_BLOCK;
        let f = 30.0;
        let mut phase = 0.0f64;
        let mut pass = |t: &mut ProfileTracer| {
            let run: Vec<f64> = (0..pass_frames)
                .map(|_| {
                    let s = phase.sin();
                    phase += TAU * f / SIM_FS;
                    s
                })
                .collect();
            t.observe(&run, f, false)
        };
        assert!(!pass(&mut tracer), "the first pass is the transient guard");
        assert!(
            !pass(&mut tracer),
            "512 samples is under three cycles of a 30 Hz tone; a partial-cycle RMS is not \
             a level"
        );
        assert!(tracer.trace().points.is_empty());
        // Given enough of the tone it DOES record, at the right value - so the refusal
        // above is a floor and not a permanent silence.
        while !pass(&mut tracer) {}
        let p = tracer
            .trace()
            .points
            .last()
            .copied()
            .expect("a point once measurable");
        assert!(
            (p.amplitude - 1.0).abs() < 0.05,
            "the 30 Hz tone reads {} once three cycles are held",
            p.amplitude
        );
    }

    /// A PARKED oscillator (0 Hz, drive at zero) is not a survey tone and contributes
    /// nothing - neither a point nor samples that could leak into the next window.
    #[test]
    fn a_parked_oscillator_records_nothing() {
        let mut tracer = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let quiet = vec![0.0f64; 2 * SIM_BLOCK];
        assert!(!tracer.observe(&quiet, 0.0, false));
        assert!(!tracer.observe(&quiet, f64::NAN, false));
        assert!(tracer.trace().points.is_empty());
    }

    /// The level is a mean-removed RMS, so a DC offset on the control accelerometer does
    /// not inflate the survey curve.
    #[test]
    fn a_dc_offset_does_not_inflate_the_level() {
        let f = 200.0;
        let mut plain = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let mut offset = ProfileTracer::new(1, SIM_FS, 50.0, 1).with_block_frames(SIM_BLOCK);
        let n = 2 * SIM_BLOCK;
        let mut phase = 0.0f64;
        let mut a = Vec::with_capacity(n);
        for _ in 0..(6 * n) {
            a.push(2.0 * phase.sin());
            phase += TAU * f / SIM_FS;
            if a.len() == n {
                plain.observe(&a, f, false);
                let biased: Vec<f64> = a.iter().map(|v| v + 3.0).collect();
                offset.observe(&biased, f, false);
                a.clear();
            }
        }
        let p = plain.trace().points.last().unwrap().amplitude;
        let o = offset.trace().points.last().unwrap().amplitude;
        assert!((p - 2.0).abs() < 0.05, "a 2.0 tone reads {p}");
        assert!(
            (o - p).abs() < 0.05,
            "the DC-biased copy reads {o}, want {p}"
        );
    }

    #[test]
    fn derived_tracer_settles_then_reports_motion() {
        let fs = 65536.0;
        let mut d = DerivedTracer::new(fs, 5.0, 5.0);
        assert!(d.settling(), "settle window open at start");
        assert!(d.settle_frames() > 0);
        // Push a steady 1 g @ 100 Hz sine well past the settle window.
        let n = (d.settle_frames() + 20_000) as usize;
        let mut phase = 0.0f64;
        for _ in 0..n {
            let s = (phase).sin();
            phase += TAU * 100.0 / fs;
            d.push(s);
        }
        assert!(!d.settling(), "settle window closed after N HP constants");
        let (vel, _disp) = d.last();
        // 1 g @ 100 Hz -> velocity amplitude ~ (386.09)/(2*pi*100) ~= 0.61 in/s.
        assert!(vel.abs() > 0.0, "derived velocity is nonzero after settle");
    }
}
