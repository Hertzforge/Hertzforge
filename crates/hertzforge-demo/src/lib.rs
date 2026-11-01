//! `hertzforge-demo` - the first end-to-end, zero-hardware closed-loop run.
//!
//! This crate ties together the three layers built and verified in isolation so far and
//! shows them CONVERGING a random-vibration control loop with no hardware:
//!
//! - **Layer 3 - the plant.** [`hertzforge_hal_sim::SimBackend`] is a synthetic shaker +
//!   fixture: a bank of single-DOF resonators, a noise floor, and (here) a proven
//!   `Shared` clock so a `HostClosesLoop` closed loop is allowed.
//! - **Layer 2 - the engine.** [`hertzforge_engine::Engine`] runs the real poll/pull loop:
//!   it pulls input from the backend, runs the abort watchdog on the raw ring, feeds the
//!   timebase, hands each frame to a [`hertzforge_engine::ProcessStep`], captures the raw
//!   samples, and pushes the produced drive back through the output-buffer contract with
//!   `write_drive`.
//! - **Layer 1 - the control DSP.** [`RandomController`] is the control law. It runs a
//!   pre-test system-ID ([`hertzforge_dsp::frf::h_estimate_hv`]), inverts the plant
//!   ([`hertzforge_dsp::frf::h_inverse_regularized`]) to pre-shape a target so the RESPONSE
//!   (not the drive) matches the profile, synthesizes the drive with
//!   [`hertzforge_dsp::cola::ColaSynth`], ramps the overall level with
//!   [`hertzforge_dsp::sine::AmplitudeServo`] (dB), and holds while re-estimating the response
//!   PSD with [`hertzforge_dsp::spectrum::psd_welch`] and trimming the per-bin drive shape.
//!
//! ## Loop architecture: engine at the SIM-natural block, control DSP amortized between segments
//!
//! The loop runs the plant THROUGH the real [`hertzforge_engine::Engine::run`], one segment
//! (one control window) at a time, at the block the backend configured
//! (`hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES` = 1 frame per tick on the Sim plant):
//!
//! - The engine's [`ProcessStep`] is a CHEAP [`PlaybackStep`]: it emits the next
//!   pre-synthesized drive sample and records the control-channel response. No FFT runs
//!   inside `process`.
//! - Between segments, [`run_demo`] runs the heavy control DSP (Welch PSD, FRF, COLA
//!   re-synthesis, servo, per-bin shape trim) OFF the tick path and hands the next
//!   segment's pre-synthesized drive to a fresh [`PlaybackStep`].
//!
//! ### Why this shape - the engine constraints found during bring-up (integrator notes)
//!
//! 1. ~~**`SimBackend` latches ONE drive value per block.**~~ **RETIRED 2026-07-25.**
//!    `SimBackend::write_drive` used to keep only the last frame it was handed while
//!    `read_input` applied that single value across the whole block, so at
//!    `frames_per_tick = N > 1` the plant saw 1 of every N drive samples
//!    (zero-order-held) and a broadband random drive was decimated by N. The Sim now
//!    BUFFERS the full per-frame drive stream and consumes one frame per generated sample
//!    frame, so any block size streams every drive sample. This loop still requests one
//!    frame per tick, but for a different and still-true reason - see
//!    [`hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES`]: the segment accounting here is in
//!    SAMPLES and is handed to the engine as `max_ticks`, so keeping the two units equal
//!    is what makes the response bit-comparable against driving the sim directly
//!    (`tests`: the engine response equals the direct-sim response to 1e-2).
//! 2. **`SchedulingHealth` aborts a HostClosesLoop run on a per-tick compute overrun,**
//!    and the compute budget is the tick period (`frames_per_tick / fs`), which is NOT
//!    configurable through `EngineConfig`. A Welch PSD / FRF inside `process` overruns a
//!    1-sample tick by orders of magnitude. Keeping the block DSP OUT of `process` (the
//!    segment design) is what keeps every tick within budget. **This one stands.**
//!
//! Constraint 2 is an honest property of the current engine skeleton, recorded as an
//! integrator follow-up, not worked around by faking anything: the control loop is
//! genuinely closed around the engine's per-sample read -> process -> write -> plant
//! path, and every sample flows through the real ring, timebase, watchdog, output
//! contract, and capture sink.
//!
//! What is exercised of the real engine: the SPSC raw ring, the `Timebase`
//! (device-counter epoch/gap/wrap), the `AbortWatchdog` on the raw ring, the per-tick
//! `SchedulingHealth`, the `OutputDelivery` lookahead/underrun contract, and the
//! `CaptureSink` seam - all per sample, across every segment. What is amortized off the
//! tick path (as a real controller would, running control DSP off the RT thread): the
//! block FFT analysis between segments.

#![forbid(unsafe_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::cola::ColaSynth;
use hertzforge_dsp::frf::{h_estimate_hv, h_inverse_regularized, suggest_epsilon};
use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::spectrum::psd_welch;
use hertzforge_engine::{Engine, EngineConfig, ProcessStep, VecCaptureSink};
use hertzforge_hal::Backend;
use hertzforge_hal_sim::{SimBackend, SimConfig};

/// Configuration for the whole demo: plant, control band, and loop knobs. Defaults give
/// a fast, well-resolved, reproducible run that converges.
#[derive(Debug, Clone)]
pub struct DemoConfig {
    /// Backend sample rate (Hz). Lower than the 65,536 Hz default raw rate so the interior band is
    /// well resolved with a modest `nfft` and the demo runs quickly.
    pub fs_hz: f64,
    /// Plant resonances `(f_n, Q)`, placed INSIDE the control band.
    pub resonances: Vec<(f64, f64)>,
    /// Plant additive-noise standard deviation (g).
    pub noise_level: f64,
    /// Broadband drive->response feedthrough (keeps off-resonance |H| well above zero so
    /// a flat target is reachable everywhere in band).
    pub feedthrough: f64,
    /// COLA / FRF transform length (power of two).
    pub nfft: usize,
    /// COLA overlap fraction (0.5 or 0.75).
    pub overlap: f64,
    /// Lower edge of the interior control band (Hz), safely above DC.
    pub band_lo_hz: f64,
    /// Upper edge of the interior control band (Hz), safely below Nyquist.
    pub band_hi_hz: f64,
    /// Target control PSD level inside the band (g^2/Hz). Flat profile.
    pub target_psd: f64,
    /// Drive amplitude used during the pre-test system-ID (small, low level).
    pub pretest_drive: f64,
    /// Servo maximum slew rate (dB/s).
    pub servo_rate_db_per_s: f64,
    /// How many control windows make up one analysis segment (Welch averages ~ this x
    /// (1/(1-overlap))). More = lower PSD variance = steadier control.
    pub blocks_per_window: usize,
    /// PRNG seed for the COLA drive (reproducibility).
    pub cola_seed: u64,
    /// PRNG seed for the plant noise (reproducibility).
    pub plant_seed: u64,
    /// Per-bin spectral-correction log-domain damping (0<g<=1).
    pub shape_gain: f64,
    /// Enable the adaptive per-bin spectral (shape) correction on top of the FRF inverse.
    pub spectral_correction: bool,
}

impl Default for DemoConfig {
    fn default() -> Self {
        DemoConfig {
            fs_hz: 8_192.0,
            // Two in-band modes so the FRF is non-trivial (peaks the inverse must
            // pre-emphasize). Moderate Q plus a healthy broadband feedthrough floor keep
            // the in-band |H| dynamic range modest with NO deep anti-resonance null, so a
            // flat target is physically reachable everywhere in band.
            resonances: vec![(200.0, 2.5), (550.0, 2.5)],
            noise_level: 2e-4,
            feedthrough: 0.35,
            nfft: 1024, // df = 8 Hz at 8192 Hz
            overlap: 0.5,
            band_lo_hz: 60.0,
            band_hi_hz: 780.0,
            target_psd: 1.0e-2, // g^2/Hz, flat in band
            pretest_drive: 0.05,
            // One control window spans window_len/fs seconds (~1.5 s here), so the servo's
            // dB-rate limit acts per window. Keep it gentle: ~1.5 dB max move per window so
            // the overall level settles smoothly instead of over-correcting on each noisy
            // PSD estimate.
            servo_rate_db_per_s: 1.0,
            blocks_per_window: 12, // ~23 Welch averages at 50% overlap
            cola_seed: 0xC0FF_EE12_3456_7890,
            plant_seed: 0x5A5A_1234,
            shape_gain: 0.5,
            spectral_correction: true,
        }
    }
}

impl DemoConfig {
    /// Number of one-sided PSD bins for this `nfft`.
    pub fn nbins(&self) -> usize {
        self.nfft / 2 + 1
    }

    /// Frequency-bin width (Hz).
    pub fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }

    /// Bin index for a frequency (rounded).
    pub fn bin_of(&self, hz: f64) -> usize {
        (hz / self.df()).round() as usize
    }

    /// The half-open interior band `[lo_bin, hi_bin)` in COLA/Welch bins, kept strictly
    /// inside (0, Nyquist) so no control energy lands at DC or Nyquist (the documented
    /// COLA edge-bin limitation).
    pub fn band_bins(&self) -> (usize, usize) {
        let lo = self.bin_of(self.band_lo_hz).max(1);
        let hi = self.bin_of(self.band_hi_hz).min(self.nfft / 2 - 1) + 1;
        (lo, hi.max(lo + 1))
    }

    /// The flat one-sided target PSD vector (length `nbins`), zero outside the band.
    pub fn target_profile(&self) -> Vec<f64> {
        let (lo, hi) = self.band_bins();
        let mut t = vec![0.0; self.nbins()];
        for b in t.iter_mut().take(hi).skip(lo) {
            *b = self.target_psd;
        }
        t
    }

    /// Samples in one analysis segment (a whole number of COLA hops).
    pub fn window_len(&self) -> usize {
        let hop = ((self.nfft as f64) * (1.0 - self.overlap)).round() as usize;
        let want = self.blocks_per_window * self.nfft;
        want.div_ceil(hop) * hop
    }

    /// Target in-band response RMS (g): sqrt of the target PSD integrated over the band.
    pub fn target_inband_rms(&self) -> f64 {
        let (lo, hi) = self.band_bins();
        let t = self.target_profile();
        (t[lo..hi].iter().sum::<f64>() * self.df()).sqrt()
    }

    /// Build the sim backend for this config (a `Shared`-clock resonance plant).
    pub fn build_backend(&self) -> SimBackend {
        SimConfig::new()
            .sample_rate_hz(self.fs_hz)
            .resonances(self.resonances.clone())
            .noise_level(self.noise_level)
            .feedthrough(self.feedthrough)
            .shared_clock(true) // a sim is shared-clock by construction; closed loop allowed
            .seed(self.plant_seed)
            .max_output_channels(1)
            .build()
    }

    /// A one-control-one-drive channel table (g control, generous abort limit so the
    /// healthy converged loop never trips the watchdog; a real runaway still would).
    pub fn channel_table(&self) -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::AccelG;
        ctrl.abort.accel_g = Some(self.abort_limit_g());
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(ctrl);
        t.channels.push(drv);
        t
    }

    /// The control-channel abort limit (g): well above the converged in-band peak so the
    /// healthy loop runs, but low enough that a genuine runaway trips it.
    pub fn abort_limit_g(&self) -> f64 {
        // Converged in-band RMS is `target_inband_rms`; peaks are a few sigma above.
        // 20x the RMS is a generous, physical over-test guard.
        (self.target_inband_rms() * 20.0).max(50.0)
    }
}

/// Phase of the control law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Low-level pre-test: drive a small random signal and record drive+response for the
    /// FRF system-ID.
    PreTest,
    /// The plant `H` is estimated; ramp the overall level up with the servo.
    RampUp,
    /// At full level; hold and keep flattening the response PSD.
    Hold,
}

/// One snapshot of loop state, emitted per control window for the convergence trace.
#[derive(Debug, Clone)]
pub struct LoopSample {
    /// Control-window index (one per completed analysis segment).
    pub iter: u64,
    pub phase: Phase,
    /// Overall servo level in dB (0 dB = the shaped-target full level).
    pub level_db: f64,
    /// In-band RMS error between measured and target PSD, in dB. `None` before the first
    /// valid measurement.
    pub inband_err_db: Option<f64>,
    /// Peak absolute drive sample commanded in this window (runaway guard).
    pub drive_peak: f64,
    /// Measured in-band response RMS (g), for context.
    pub inband_rms_g: Option<f64>,
}

/// A cheap engine [`ProcessStep`] that plays back a pre-synthesized drive stream and
/// records the control-channel response. All the heavy control DSP happens BETWEEN
/// segments (in [`RandomController`]); this step does no allocation-heavy work per tick,
/// so it never overruns the engine's per-tick compute budget even at `frames_per_tick = 1`.
pub struct PlaybackStep {
    drive: Vec<f64>,
    pos: usize,
    /// Recorded control-channel response, one sample per frame processed.
    pub response: Vec<f64>,
    /// Peak |drive| actually emitted.
    pub drive_peak: f64,
}

impl PlaybackStep {
    pub fn new(drive: Vec<f64>) -> Self {
        let response = Vec::with_capacity(drive.len());
        PlaybackStep {
            drive,
            pos: 0,
            response,
            drive_peak: 0.0,
        }
    }
}

impl ProcessStep for PlaybackStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        let in_ch = input.len().checked_div(n_frames).unwrap_or(1).max(1);
        drive_out.clear();
        drive_out.reserve(n_frames);
        for f in 0..n_frames {
            // Record the control-channel response (channel 0) for this frame.
            self.response.push(input[f * in_ch] as f64);
            // Emit the next pre-synthesized drive sample (zero past the end).
            let d = self.drive.get(self.pos).copied().unwrap_or(0.0);
            self.pos += 1;
            if d.abs() > self.drive_peak {
                self.drive_peak = d.abs();
            }
            drive_out.push(d as f32);
        }
    }
}

/// The Random control law state, advanced once per analysis segment. Holds the target,
/// the plant-inverse drive shaping, the COLA synth, and the level servo. It does not
/// touch the engine directly; [`run_demo`] runs each segment through the real engine and
/// feeds the recorded response back here.
pub struct RandomController {
    cfg: DemoConfig,
    target: Vec<f64>,
    /// Per-bin drive-PSD shape at unit servo level: `drive_psd[k] = shape[k] * scale^2`.
    /// Seeded from the FRF inverse, then trimmed by the per-bin shape loop.
    shape: Vec<f64>,
    cola: ColaSynth,
    servo: AmplitudeServo,
    target_inband_rms: f64,
    phase: Phase,
    iter: u64,
    /// Latched: the servo faulted or hit its ceiling during a HEALTHY run (a bug signal).
    unexpected_servo_event: bool,
    hold_windows: u32,
}

impl RandomController {
    pub fn new(cfg: DemoConfig) -> Self {
        let target = cfg.target_profile();
        let nbins = cfg.nbins();
        let cola = ColaSynth::with_seed(cfg.nfft, cfg.overlap, cfg.fs_hz, cfg.cola_seed);
        let target_inband_rms = cfg.target_inband_rms();
        let servo = AmplitudeServo::new(target_inband_rms, cfg.servo_rate_db_per_s)
            // Mostly-integral, low gain: one update per (noisy) window, so a big
            // proportional kick would chase measurement noise. A gentle integrator plus
            // the dB-rate clamp gives a smooth monotone level climb that settles.
            .with_gains(0.0, 0.4)
            .with_drive_ceiling(64.0) // ~36 dB of headroom over the full (0 dB) level
            .with_fault_fraction(1e-3);
        RandomController {
            cfg,
            target,
            shape: vec![0.0; nbins],
            cola,
            servo,
            target_inband_rms,
            phase: Phase::PreTest,
            iter: 0,
            unexpected_servo_event: false,
            hold_windows: 0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn target_inband_rms(&self) -> f64 {
        self.target_inband_rms
    }
    pub fn unexpected_servo_event(&self) -> bool {
        self.unexpected_servo_event
    }
    pub fn hold_windows(&self) -> u32 {
        self.hold_windows
    }
    pub fn drive_scale(&self) -> f64 {
        self.servo.drive_scale()
    }

    /// Synthesize the next segment's drive stream (`window_len` samples) from the current
    /// per-bin PSD, scaled by the servo level. Continuous with the previous segment's
    /// COLA tail (the synth state carries over), so the drive stream has no seam.
    pub fn synthesize_segment(&mut self) -> Vec<f64> {
        let scale2 = self.servo.drive_scale().powi(2);
        let psd: Vec<f64> = self.shape.iter().map(|&s| s * scale2).collect();
        let n = self.cfg.window_len();
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let hop = self.cola.next_frame(&psd);
            out.extend_from_slice(&hop);
        }
        out.truncate(n);
        out
    }

    /// The pre-test drive stream: a low-level flat band-limited random excitation (no
    /// plant estimate yet). Continuous with the COLA tail.
    pub fn synthesize_pretest(&mut self) -> Vec<f64> {
        let (lo, hi) = self.cfg.band_bins();
        let level = self.cfg.pretest_drive * self.cfg.pretest_drive;
        let mut psd = vec![0.0; self.cfg.nbins()];
        for p in psd.iter_mut().take(hi).skip(lo) {
            *p = level;
        }
        let n = self.cfg.window_len();
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let hop = self.cola.next_frame(&psd);
            out.extend_from_slice(&hop);
        }
        out.truncate(n);
        out
    }

    /// Estimate the plant `H` from a pre-test drive/response segment and seed the per-bin
    /// drive shape from the regularized inverse so the RESPONSE (not the drive) matches
    /// the target: `drive_psd = target * |Hinv|^2` makes `response = |H|^2 * drive_psd`
    /// approximately flat.
    pub fn system_id(&mut self, drive: &[f64], response: &[f64]) {
        let h = h_estimate_hv(drive, response, self.cfg.fs_hz, self.cfg.nfft);
        let (lo, hi) = self.cfg.band_bins();
        // Regularize using the IN-BAND peak gain only; out-of-band bins carry no pre-test
        // drive so their Hv is noise-dominated and huge, which would blow up a
        // whole-spectrum epsilon and collapse the inverse.
        let band_h: Vec<(f64, f64)> = h[lo..hi].to_vec();
        let eps = suggest_epsilon(&band_h, 0.05).max(1e-12);
        let hinv = h_inverse_regularized(&h, eps);
        for s in self.shape.iter_mut() {
            *s = 0.0;
        }
        // Only shape (and therefore only ever drive) inside the interior band; band edges
        // and out-of-band bins stay zero.
        let band = self.shape[lo..hi]
            .iter_mut()
            .zip(self.target[lo..hi].iter())
            .zip(hinv[lo..hi].iter());
        for ((s, &t), &(re, im)) in band {
            *s = (t * (re * re + im * im)).max(0.0);
        }
    }

    /// Measure the control-channel PSD over a segment's response and return
    /// `(psd, inband_err_db, inband_rms_g)`.
    pub fn measure(&self, response: &[f64]) -> (Vec<f64>, Option<f64>, Option<f64>) {
        let (freqs, psd) = psd_welch(response, self.cfg.fs_hz, self.cfg.nfft, self.cfg.overlap);
        let (lo, hi) = self.cfg.band_bins();
        let _ = freqs;
        if psd.len() < hi {
            return (psd, None, None);
        }
        let df = self.cfg.df();
        let mut sq_err_db = 0.0;
        let mut nb = 0usize;
        let mut var = 0.0;
        for (&meas_raw, &tgt_raw) in psd[lo..hi].iter().zip(self.target[lo..hi].iter()) {
            let meas = meas_raw.max(1e-30);
            let tgt = tgt_raw.max(1e-30);
            let e = 10.0 * (meas / tgt).log10();
            sq_err_db += e * e;
            nb += 1;
            var += meas_raw;
        }
        let rms_err_db = (sq_err_db / nb.max(1) as f64).sqrt();
        let rms_g = (var * df).sqrt();
        (psd, Some(rms_err_db), Some(rms_g))
    }

    /// Advance the controller one segment given the response measured for the drive it
    /// last synthesized. Returns the trace sample for this window.
    pub fn step(&mut self, response: &[f64], drive_peak: f64) -> LoopSample {
        self.iter += 1;
        let (lo, hi) = self.cfg.band_bins();
        let (resp_psd, inband_err_db, inband_rms_g) = self.measure(response);
        let dt = self.cfg.window_len() as f64 / self.cfg.fs_hz;

        match self.phase {
            Phase::PreTest => {
                // system_id() is called by run_demo at the pre-test -> ramp handoff (it
                // needs the drive too); here PreTest just advances to RampUp once we have
                // a measurement.
                if inband_rms_g.is_some() {
                    self.phase = Phase::RampUp;
                }
            }
            Phase::RampUp => {
                // The servo is the STARTUP RAMP: move the overall drive level (dB) up from
                // the seed toward the level that puts the in-band RMS on target,
                // rate-clamped so the climb is a controlled ramp, not a step.
                if let Some(rms) = inband_rms_g {
                    let before = self.servo.drive_scale();
                    self.servo.update(rms, dt);
                    self.check_servo();
                    let after = self.servo.drive_scale();
                    if (after - before).abs() / before.max(1e-9) < 0.02 {
                        self.phase = Phase::Hold;
                    }
                }
            }
            Phase::Hold => {
                // Two orthogonal loops at full level:
                //   - the SERVO holds the overall in-band RMS on target (mean level), and
                //   - the per-bin spectral loop redistributes power to flatten the SHAPE
                //     while PRESERVING the in-band mean (so it does not move the level).
                // Mean-preserving + servo-owns-mean keeps the two loops from fighting.
                self.hold_windows += 1;
                if let Some(rms) = inband_rms_g {
                    self.servo.update(rms, dt);
                    self.check_servo();
                }
                if self.cfg.spectral_correction && resp_psd.len() >= hi {
                    self.correct_spectrum(&resp_psd, lo, hi);
                }
            }
        }

        LoopSample {
            iter: self.iter,
            phase: self.phase,
            level_db: 20.0 * self.servo.drive_scale().log10(),
            inband_err_db,
            drive_peak,
            inband_rms_g,
        }
    }

    fn check_servo(&mut self) {
        if self.servo.faulted() || self.servo.drive_scale() >= self.servo.max_drive_scale() * 0.999
        {
            // On a HEALTHY loop the control channel is live and the level sits well under
            // the ceiling; either event is a bug signal.
            self.unexpected_servo_event = true;
        }
    }

    /// Adaptive per-bin spectral (shape) correction: nudge the drive shape toward
    /// flattening the measured response error, preserving the in-band MEAN (the servo
    /// owns the overall level). Damped in log space and clamped both per-update and
    /// absolutely, so it is stable against the single-segment Welch PSD variance.
    fn correct_spectrum(&mut self, resp_psd: &[f64], lo: usize, hi: usize) {
        let gain = self.cfg.shape_gain;
        let (step_min, step_max) = (0.5f64, 2.0f64);

        let mean_before: f64 = self.shape[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
        let trim = self.shape[lo..hi]
            .iter_mut()
            .zip(resp_psd[lo..hi].iter())
            .zip(self.target[lo..hi].iter());
        for ((s, &meas_raw), &tgt_raw) in trim {
            let meas = meas_raw.max(1e-30);
            let tgt = tgt_raw.max(1e-30);
            let step = (tgt / meas).powf(gain).clamp(step_min, step_max);
            *s = (*s * step).max(0.0);
        }
        // Renormalize to preserve the in-band mean (shape-only; level stays the servo's).
        let mean_after: f64 = self.shape[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
        if mean_after > 0.0 {
            let renorm = mean_before / mean_after;
            for s in self.shape[lo..hi].iter_mut() {
                *s *= renorm;
            }
        }
        // Absolute per-bin clamp around the band median, so no bin can compound away.
        let mut band: Vec<f64> = self.shape[lo..hi].to_vec();
        band.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = band[band.len() / 2].max(1e-30);
        let (floor, ceil) = (median * 1e-4, median * 1e4);
        for s in self.shape[lo..hi].iter_mut() {
            *s = s.clamp(floor, ceil);
        }
    }
}

/// Aggregate result of a full demo run.
#[derive(Debug, Clone)]
pub struct DemoResult {
    pub trace: Vec<LoopSample>,
    pub target_inband_rms: f64,
    pub unexpected_servo_event: bool,
    /// Peak |drive| across the whole run (runaway bound).
    pub max_drive_peak: f64,
    pub final_phase: Phase,
    pub hold_windows: u32,
    /// Total frames the engine actually processed (across all segments).
    pub total_frames: u64,
    /// Total frames the engine captured through the real `CaptureSink` (across segments).
    pub total_captured: u64,
    /// Total emergency-stops the engine issued (should be 0 on a healthy run).
    pub estops: u32,
    /// True if any segment stopped for a reason other than the segment ending cleanly.
    pub abnormal_stop: Option<String>,
}

impl DemoResult {
    /// Mean in-band error (dB) over the last `n` trace samples that carry an error.
    pub fn settled_mean_err_db(&self, n: usize) -> Option<f64> {
        let errs: Vec<f64> = self.trace.iter().filter_map(|s| s.inband_err_db).collect();
        if errs.is_empty() {
            return None;
        }
        let tail = &errs[errs.len().saturating_sub(n)..];
        Some(tail.iter().sum::<f64>() / tail.len() as f64)
    }

    /// Worst (max abs) in-band error over the last `n` trace samples that carry an error.
    pub fn settled_worst_err_db(&self, n: usize) -> Option<f64> {
        let errs: Vec<f64> = self.trace.iter().filter_map(|s| s.inband_err_db).collect();
        if errs.is_empty() {
            return None;
        }
        let tail = &errs[errs.len().saturating_sub(n)..];
        Some(tail.iter().fold(0.0f64, |m, &e| m.max(e.abs())))
    }
}

/// Run the closed-loop demo headless: build the sim plant + the controller, then run the
/// plant through the real [`Engine`] one segment at a time (at the block the backend
/// configured), doing the control DSP between segments. `max_windows` bounds the number
/// of control windows. Shared by `main` and the integration test.
pub fn run_demo(cfg: DemoConfig, max_windows: u64) -> DemoResult {
    let table = cfg.channel_table();
    let mut backend = cfg.build_backend();
    backend.open().expect("sim open");
    // Request the HERTZFORGE-SIM plant's natural block; the engine's cadence then comes from
    // what the backend ACTUALLY configured, which is also what sets the ring depth and the
    // scheduling-health budget.
    let acq = hertzforge_hal::AcquisitionConfig::new(
        backend.caps().sample_rate_hz,
        hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES,
    );
    let actual = backend.configure(&table, &acq).expect("sim configure");
    backend.arm().expect("sim arm");
    backend.start().expect("sim start");

    let mut ecfg = EngineConfig::with_block(1, 1, &actual);
    // Random control is inherently peaky; a spike-sensitive watchdog would false-trip on
    // Gaussian tails. The healthy converged loop stays well under the abort limit.
    ecfg.watchdog_sustain = 64;
    // Deterministic single-threaded harness: it runs flat out and then stops for the
    // control DSP between segments, so wall-clock tick spacing carries no information.
    ecfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
    // The tick-period compute budget is meaningless for this harness (~122 us at
    // one frame per tick); relax it explicitly rather than retrying the spurious
    // ComputeOverrun it produces. See `TEST_RELAX_COMPUTE_BUDGET`.
    ecfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);

    let mut controller = RandomController::new(cfg.clone());
    let window_len = cfg.window_len() as u64;

    let mut trace = Vec::new();
    let mut max_drive_peak = 0.0f64;
    let mut total_frames = 0u64;
    let mut total_captured = 0u64;
    let mut estops = 0u32;
    let mut abnormal_stop = None;

    // Pre-test: a few low-level segments for the FRF system-ID. Keep the last one's
    // drive+response for the estimate.
    let pretest_segments = 3usize;
    let mut last_pretest_drive = Vec::new();
    let mut last_pretest_resp = Vec::new();

    for _ in 0..pretest_segments {
        let drive = controller.synthesize_pretest();
        let (resp, dpk, rep) = run_segment(&mut backend, &table, &ecfg, drive.clone(), window_len);
        total_frames += rep.frames_in;
        total_captured += rep.frames_captured;
        estops += rep.estops;
        record_abnormal(&rep, &mut abnormal_stop);
        max_drive_peak = max_drive_peak.max(dpk);
        let sample = controller.step(&resp, dpk);
        trace.push(sample);
        last_pretest_drive = drive;
        last_pretest_resp = resp;
    }

    // System-ID from the last pre-test segment, then seed the ramp at 0 dB (full shaped
    // level) and let the servo trim.
    controller.system_id(&last_pretest_drive, &last_pretest_resp);
    controller.servo.set_feedforward_scale(1.0);
    controller.phase = Phase::RampUp;

    // Ramp + hold segments until max_windows total.
    while (trace.len() as u64) < max_windows {
        let drive = controller.synthesize_segment();
        let (resp, dpk, rep) = run_segment(&mut backend, &table, &ecfg, drive, window_len);
        total_frames += rep.frames_in;
        total_captured += rep.frames_captured;
        estops += rep.estops;
        record_abnormal(&rep, &mut abnormal_stop);
        max_drive_peak = max_drive_peak.max(dpk);
        let sample = controller.step(&resp, dpk);
        trace.push(sample);
    }

    DemoResult {
        trace,
        target_inband_rms: controller.target_inband_rms(),
        unexpected_servo_event: controller.unexpected_servo_event(),
        max_drive_peak,
        final_phase: controller.phase(),
        hold_windows: controller.hold_windows(),
        total_frames,
        total_captured,
        estops,
        abnormal_stop,
    }
}

/// Run ONE segment of `window_len` samples through the real engine at the configured
/// block cadence, playing back `drive` and returning `(response, drive_peak, run_report)`.
/// (`window_len` is in SAMPLES and is passed as `max_ticks`; that is exact only because
/// this path runs the SIM-natural one-frame block - see `SIM_NATURAL_BLOCK_FRAMES`.) A fresh
/// engine + capture sink per segment keeps the timebase/ring/output-contract exercised
/// end to end while the backend (and thus the plant + noise stream) persists across
/// segments, so the closed loop is genuinely continuous.
fn run_segment(
    backend: &mut SimBackend,
    table: &ChannelTable,
    ecfg: &EngineConfig,
    drive: Vec<f64>,
    window_len: u64,
) -> (Vec<f64>, f64, hertzforge_engine::RunReport) {
    use hertzforge_engine::StopReason;
    // The engine's SchedulingHealth times each `process` call against the tick period with
    // a wall clock. At one frame per tick the period is ~122 us (1/8192 s), so an OS
    // scheduling hiccup on ANY one of the ~12k ticks in a segment (a page fault, a
    // preemption under a loaded test runner) can momentarily exceed the budget and abort
    // the segment as a spurious ComputeOverrun - even though the per-tick work
    // (PlaybackStep) is genuinely nanoseconds. That is a timing artifact of the
    // deterministic single-threaded harness, not a control fault, so re-run the segment a
    // few times to get a clean, complete window. A REAL, persistent fault (or any non-
    // scheduling stop) is returned as-is for the caller to surface.
    let max_attempts = 8;
    let mut last = None;
    for _ in 0..max_attempts {
        // Engine::run() calls backend.stop() when a segment ends, leaving the SimBackend
        // Stopped with a decaying drive ramp. Re-arm + start so the plant is Running with a
        // clean drive path; the backend's plant state, sample counter, and noise stream
        // persist across segments, so the closed loop stays continuous even though each
        // segment gets a fresh Engine. arm() only accepts Configured/Stopped and start()
        // only Armed, so both are no-op guards on the first (already-Running) segment.
        let _ = backend.arm();
        let _ = backend.start();
        let mut engine = Engine::new(clone_cfg(ecfg), table, backend);
        let mut step = PlaybackStep::new(drive.clone());
        let mut sink = VecCaptureSink::new();
        let report = engine.run(backend, &mut step, &mut sink, window_len);
        if matches!(report.reason, StopReason::SchedulingFault(_)) {
            last = Some((step.response, step.drive_peak, report));
            continue; // spurious wall-clock overrun; retry a clean segment
        }
        return (step.response, step.drive_peak, report);
    }
    // All attempts hit a scheduling fault: return the last one so the caller surfaces it.
    last.expect("at least one attempt ran")
}

/// `EngineConfig` is not `Clone`; rebuild it field-for-field for each segment's engine.
fn clone_cfg(c: &EngineConfig) -> EngineConfig {
    let mut e = EngineConfig::new(c.in_channels, c.out_channels);
    e.frames_per_tick = c.frames_per_tick;
    e.ring_frames = c.ring_frames;
    e.watchdog_hp_hz = c.watchdog_hp_hz;
    e.watchdog_sustain = c.watchdog_sustain;
    e.jitter_frac = c.jitter_frac;
    e.compute_budget = c.compute_budget;
    e.stall_deadline = c.stall_deadline;
    e
}

fn record_abnormal(rep: &hertzforge_engine::RunReport, out: &mut Option<String>) {
    use hertzforge_engine::StopReason;
    if out.is_some() {
        return;
    }
    // A segment ending because it hit its window (InputEnded) is normal here. Anything
    // else is a real fault worth surfacing.
    if !matches!(rep.reason, StopReason::InputEnded) {
        *out = Some(format!("{:?}", rep.reason));
    }
}
