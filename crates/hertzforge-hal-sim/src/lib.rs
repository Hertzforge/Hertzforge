//! `hertzforge-hal-sim` - the zero-hardware test plant (Layer 3).
//!
//! `SimBackend` implements [`hertzforge_hal::Backend`] as a `HostClosesLoop` device whose
//! "hardware" is a synthetic mechanical plant: the shaker + fixture modeled as a small
//! bank of single-DOF resonators. `write_drive` pushes the drive signal into the plant;
//! `read_input` pulls the plant's acceleration response back out. That closes the loop
//! entirely in software so Hertzforge's control math (COLA / Sine / MIMO) can be developed and
//! validated against a realistic frequency response BEFORE any real hardware exists (the
//! zero-hardware path).
//!
//! ## What the plant models
//!
//! - **Resonance.** Each DOF is a 2nd-order IIR resonator ([`Resonator`]) tuned to a
//!   resonance `f_n` and quality factor `Q`, discretized at the backend sample rate. A
//!   drive at `f_n` is amplified by roughly `Q`; off-resonance drive produces a much
//!   smaller response, with the correct 2nd-order phase roll (0 -> -90 deg at resonance
//!   -> -180 deg well above it).
//! - **Multiple modes.** Several resonators sum, so a swept sine shows a peak at each
//!   `f_n`. Default is a single mode; [`SimConfig::resonances`] takes as many as wanted.
//! - **Noise floor.** A small additive Gaussian noise term from a deterministic xorshift
//!   PRNG ([`Xorshift64`]) seeded from config, so tests are reproducible.
//! - **Transport delay.** An optional integer-sample delay line ([`DelayLine`]) between
//!   drive and response, mirroring real DAC->amp->shaker->accel->ADC latency.
//!
//! ## Output-buffer model
//!
//! `write_drive` buffers the FULL per-frame drive stream into a bounded FIFO of
//! [`SimConfig::output_buffer_frames`] frames, and `read_input` consumes exactly one
//! drive frame per generated sample frame. [`SimBackend::output_space`] reports the real
//! remaining headroom, so the engine's top-up delivery policy - offer what fits, RETAIN
//! what the device refused - is exercised in CI against this backend rather than only
//! against future NI hardware.
//!
//! This REPLACED a single-value latch: `write_drive` used to keep only the LAST frame it
//! was handed and `read_input` applied that one value across every frame of the block it
//! generated. At one frame per call the two models agree exactly; at `frames_per_tick > 1`
//! the latch decimated the drive by the block size (the plant saw 1 of every N drive
//! samples, zero-order held), which is why every module pinned the engine to
//! `frames_per_tick = 1`. Recorded as a deferred refinement in the `hertzforge-demo` crate
//! docs; retired here.
//!
//! ## Clock posture
//!
//! Per the audit posture, [`SimBackend::caps`] reports `ClockDomain::Unknown` by default:
//! a backend must not claim a shared clock it has not proven by a bench loopback, and the
//! engine refuses closed-loop modes on an `Unknown` clock. A simulator, however, IS
//! shared-clock by construction (input and output advance on the same synthetic sample
//! index), so [`SimBackend::with_shared_clock`] lets a closed-loop test opt into
//! `ClockDomain::Shared` explicitly.

#![allow(dead_code)]

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use std::collections::VecDeque;
use std::f64::consts::TAU;

/// Default raw sample rate (that of the first delegated backend); the sim mirrors it so
/// downstream timebase math matches hardware.
pub const DEFAULT_SAMPLE_RATE_HZ: f64 = 65_536.0;

/// Default depth of the simulated output (drive) buffer, in FRAMES.
///
/// Deep enough that a normal rate-matched engine run never sees backpressure (the
/// engine tops up ~one block per tick and `read_input` drains the same amount), and
/// bounded so a test can shrink it and PROVE the retain-and-re-offer path.
pub const DEFAULT_OUTPUT_BUFFER_FRAMES: usize = 4096;

/// Default single mode: a mid-band fixture resonance with a moderately sharp Q.
pub const DEFAULT_RESONANCE_HZ: f64 = 250.0;
pub const DEFAULT_Q: f64 = 20.0;

/// Default PRNG seed for the noise floor (deterministic tests).
pub const DEFAULT_SEED: u64 = 0xE16E_5148_0BAD_5EED;

// ---------------------------------------------------------------------------------------
// Deterministic PRNG (pure std, no external rng crate).
// ---------------------------------------------------------------------------------------

/// A tiny xorshift64* PRNG. Deterministic from its seed, which is exactly what the tests
/// need (reproducible noise floor). Not cryptographic; a simulator noise source only.
#[derive(Debug, Clone)]
pub struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    /// Seed the generator. A zero seed is remapped (xorshift cannot leave state 0).
    pub fn new(seed: u64) -> Self {
        Xorshift64 {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// Next raw 64-bit value (xorshift64*).
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn next_unit(&mut self) -> f64 {
        // Take the top 53 bits for a clean f64 mantissa.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard-normal sample via Box-Muller (one of the pair; the other is discarded,
    /// which is fine for a noise floor).
    fn next_gaussian(&mut self) -> f64 {
        // Guard u1 away from 0 so ln is finite.
        let u1 = self.next_unit().max(1e-12);
        let u2 = self.next_unit();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

// ---------------------------------------------------------------------------------------
// Transport delay line.
// ---------------------------------------------------------------------------------------

/// A fixed integer-sample delay line (ring buffer). `delay == 0` is a pass-through.
///
/// Implemented as a read-old-slot-then-overwrite ring of exactly `delay_samples` slots:
/// with `pos` cycling `0..delay`, the slot read on step `k` was written on step
/// `k - delay`, so the output is the input delayed by exactly `delay_samples`.
#[derive(Debug, Clone)]
struct DelayLine {
    buf: Vec<f64>,
    pos: usize,
}

impl DelayLine {
    fn new(delay_samples: usize) -> Self {
        DelayLine {
            buf: vec![0.0; delay_samples],
            pos: 0,
        }
    }

    /// Push `x`, return the sample delayed by `delay_samples`.
    fn process(&mut self, x: f64) -> f64 {
        let n = self.buf.len();
        if n == 0 {
            return x; // zero delay: pass-through
        }
        // The slot we are about to overwrite currently holds the sample written exactly
        // `n` steps ago. Read it, then overwrite with the new sample and advance.
        let out = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % n;
        out
    }

    fn reset(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.pos = 0;
    }
}

// ---------------------------------------------------------------------------------------
// Single-DOF resonator (2nd-order IIR).
// ---------------------------------------------------------------------------------------

/// A single-degree-of-freedom resonator discretized to a 2nd-order IIR. Models the
/// transfer from drive to acceleration response of one fixture mode: a peak of ~`Q` at
/// `f_n` with the matching 2nd-order phase, and much less gain away from `f_n`.
///
/// The coefficients are the RBJ audio-EQ "bandpass with constant peak gain Q" biquad:
/// the on-resonance magnitude is `Q`. That gives exactly the "amplifies by ~Q at
/// resonance, small off resonance" behavior the plant needs, and it is unconditionally
/// stable for `Q >= 0.5`.
#[derive(Debug, Clone)]
pub struct Resonator {
    // Feed-forward / feed-back biquad coefficients (normalized so a0 = 1).
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    // Direct-form-II transposed state.
    z1: f64,
    z2: f64,
    f_n: f64,
    q: f64,
}

impl Resonator {
    /// Build a resonator for resonance `f_n` (Hz), quality `q`, at sample rate `fs_hz`.
    pub fn new(f_n: f64, q: f64, fs_hz: f64) -> Self {
        let q = q.max(0.5); // below 0.5 it is no longer a resonance; clamp for stability
        let w0 = TAU * f_n / fs_hz;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        // RBJ bandpass, constant 0 dB peak gain form, then scaled by Q so the peak is Q.
        let b0 = q * alpha;
        let b1 = 0.0;
        let b2 = -q * alpha;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Resonator {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
            f_n,
            q,
        }
    }

    /// Process one input sample (Direct Form II Transposed).
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

// ---------------------------------------------------------------------------------------
// Plant: a bank of resonators + broadband feedthrough + noise + transport delay.
// ---------------------------------------------------------------------------------------

/// The synthetic plant: sum of resonator responses to the drive, plus a small broadband
/// feedthrough term (so off-resonance drive still produces *some* response), plus additive
/// Gaussian noise, plus an optional transport delay.
#[derive(Debug, Clone)]
struct Plant {
    resonators: Vec<Resonator>,
    delay: DelayLine,
    rng: Xorshift64,
    noise_level: f64,
    feedthrough: f64,
    // Last drive value seen (for emergency ramp-down / hold).
    last_drive: f64,
}

impl Plant {
    fn from_config(cfg: &SimConfig) -> Self {
        let resonators = cfg
            .resonances
            .iter()
            .map(|&(f_n, q)| Resonator::new(f_n, q, cfg.sample_rate_hz))
            .collect();
        Plant {
            resonators,
            delay: DelayLine::new(cfg.transport_delay_samples),
            rng: Xorshift64::new(cfg.seed),
            noise_level: cfg.noise_level,
            feedthrough: cfg.feedthrough,
            last_drive: 0.0,
        }
    }

    /// Advance the plant by one sample given a drive input, returning the acceleration
    /// response (in the plant's arbitrary "g" unit).
    #[inline]
    fn step(&mut self, drive: f64) -> f64 {
        self.last_drive = drive;
        let delayed = self.delay.process(drive);
        let mut resonant = 0.0;
        for r in &mut self.resonators {
            resonant += r.process(delayed);
        }
        // Broadband feedthrough keeps off-resonance response non-zero and finite.
        let response = resonant + self.feedthrough * delayed;
        let noise = self.noise_level * self.rng.next_gaussian();
        response + noise
    }

    fn reset(&mut self) {
        self.resonators.iter_mut().for_each(Resonator::reset);
        self.delay.reset();
        self.last_drive = 0.0;
    }
}

// ---------------------------------------------------------------------------------------
// Configuration + builder.
// ---------------------------------------------------------------------------------------

/// Builder-style configuration for [`SimBackend`]. Every plant knob is here so tests can
/// construct a deterministic, well-characterized plant.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Sample rate in Hz. Default [`DEFAULT_SAMPLE_RATE_HZ`].
    pub sample_rate_hz: f64,
    /// One `(f_n, Q)` pair per fixture mode. Default: a single mode.
    pub resonances: Vec<(f64, f64)>,
    /// Additive Gaussian noise standard deviation (plant "g" units).
    pub noise_level: f64,
    /// Broadband drive->response feedthrough gain (keeps off-resonance response finite).
    pub feedthrough: f64,
    /// Transport delay in whole samples (DAC->amp->shaker->ADC latency).
    pub transport_delay_samples: usize,
    /// PRNG seed for the noise floor (determinism).
    pub seed: u64,
    /// Max input channels the backend advertises.
    pub max_input_channels: u32,
    /// Max output (drive) channels the backend advertises.
    pub max_output_channels: u32,
    /// If true, `caps().clock` is `Shared` (a sim is shared-clock by construction). If
    /// false (default), it is `Unknown` per the audit posture.
    pub shared_clock: bool,
    /// Depth of the simulated output (drive) FIFO, in frames. See the module docs
    /// for the buffering model. Default [`DEFAULT_OUTPUT_BUFFER_FRAMES`].
    pub output_buffer_frames: usize,
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            resonances: vec![(DEFAULT_RESONANCE_HZ, DEFAULT_Q)],
            noise_level: 1e-4,
            feedthrough: 1e-3,
            transport_delay_samples: 0,
            seed: DEFAULT_SEED,
            max_input_channels: 8,
            max_output_channels: 2,
            shared_clock: false,
            output_buffer_frames: DEFAULT_OUTPUT_BUFFER_FRAMES,
        }
    }
}

impl SimConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample_rate_hz(mut self, hz: f64) -> Self {
        self.sample_rate_hz = hz;
        self
    }

    /// Replace the resonance bank with a single `(f_n, Q)` mode.
    pub fn resonance(mut self, f_n: f64, q: f64) -> Self {
        self.resonances = vec![(f_n, q)];
        self
    }

    /// Replace the resonance bank with an explicit list of `(f_n, Q)` modes.
    pub fn resonances(mut self, modes: Vec<(f64, f64)>) -> Self {
        self.resonances = modes;
        self
    }

    pub fn noise_level(mut self, sigma: f64) -> Self {
        self.noise_level = sigma;
        self
    }

    pub fn feedthrough(mut self, g: f64) -> Self {
        self.feedthrough = g;
        self
    }

    pub fn transport_delay_samples(mut self, n: usize) -> Self {
        self.transport_delay_samples = n;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn max_input_channels(mut self, n: u32) -> Self {
        self.max_input_channels = n;
        self
    }

    pub fn max_output_channels(mut self, n: u32) -> Self {
        self.max_output_channels = n;
        self
    }

    pub fn shared_clock(mut self, yes: bool) -> Self {
        self.shared_clock = yes;
        self
    }

    /// Depth of the simulated output FIFO, in frames (minimum 1). Shrink it to
    /// exercise a caller's backpressure / retain-and-re-offer path.
    pub fn output_buffer_frames(mut self, n: usize) -> Self {
        self.output_buffer_frames = n.max(1);
        self
    }

    pub fn build(self) -> SimBackend {
        SimBackend::from_config(self)
    }
}

// ---------------------------------------------------------------------------------------
// Backend state machine.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open,
    Configured,
    Armed,
    Running,
    Stopped,
}

/// Which plant tap each configured input channel reads. Control/Monitor/Reference input
/// channels each map to the plant response; drive channels feed the plant input.
#[derive(Debug, Clone)]
struct ChannelMap {
    /// One entry per input (measurement) channel, in table order.
    input_channels: Vec<InputChannel>,
    /// The `input_channels` roles, pre-extracted in the same order. `read_input`
    /// needs the roles without holding an immutable borrow of `map` across the
    /// mutable plant calls; building this once at configure keeps that hot path
    /// ALLOCATION-FREE (it used to collect a fresh `Vec<ChannelRole>` on every
    /// single call, which at 400 channels is an allocation per acquired block).
    roles: Vec<ChannelRole>,
    /// Number of drive (output) channels.
    n_drive: usize,
}

#[derive(Debug, Clone)]
struct InputChannel {
    table_index: u32,
    role: ChannelRole,
}

// ---------------------------------------------------------------------------------------
// The backend.
// ---------------------------------------------------------------------------------------

/// A zero-hardware [`Backend`] whose "device" is a synthetic mechanical plant.
///
/// Construct with [`SimBackend::new`] (defaults), [`SimBackend::with_shared_clock`], or a
/// [`SimConfig`] builder (`SimConfig::new()...build()`).
#[derive(Debug, Clone)]
pub struct SimBackend {
    cfg: SimConfig,
    plant: Plant,
    state: State,
    map: Option<ChannelMap>,
    /// The drive frame CURRENTLY applied to the plant: the last frame `read_input`
    /// consumed out of [`SimBackend::out_fifo`]. When the FIFO runs dry this value
    /// is held (a zero-order hold, the benign device behavior; the engine's
    /// lookahead/underrun contract is what keeps the FIFO from running dry).
    pending_drive: Vec<f64>,
    /// The output FIFO: interleaved drive samples, `n_drive` per frame, capacity
    /// `cfg.output_buffer_frames` FRAMES. Written by `write_drive`, drained one
    /// frame per generated sample frame by `read_input`.
    out_fifo: VecDeque<f64>,
    /// Monotonic sample counter, advanced by `read_input`.
    sample_counter: u64,
    epoch: u32,
    /// Stop / emergency-stop ramp state. When `Some(g)`, the drive is scaled by `g` and
    /// `g` decays toward 0 each sample (a safe ramp-down that never issues firmware ops).
    ramp: Option<f64>,
    faulted: bool,
}

impl Default for SimBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SimBackend {
    /// Default plant: single 250 Hz / Q=20 mode at the default raw rate, clock `Unknown`.
    pub fn new() -> Self {
        Self::from_config(SimConfig::default())
    }

    /// Default plant but advertising `ClockDomain::Shared` (a simulator is shared-clock by
    /// construction), so closed-loop tests can run without a bench loopback proof.
    pub fn with_shared_clock() -> Self {
        Self::from_config(SimConfig::default().shared_clock(true))
    }

    /// Build from an explicit [`SimConfig`].
    pub fn from_config(cfg: SimConfig) -> Self {
        let plant = Plant::from_config(&cfg);
        SimBackend {
            cfg,
            plant,
            state: State::Closed,
            map: None,
            pending_drive: Vec::new(),
            out_fifo: VecDeque::new(),
            sample_counter: 0,
            epoch: 0,
            ramp: None,
            faulted: false,
        }
    }

    /// Number of measurement (input) channels currently configured.
    pub fn n_input_channels(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.input_channels.len())
    }

    /// Number of drive (output) channels currently configured.
    pub fn n_drive_channels(&self) -> usize {
        self.map.as_ref().map_or(0, |m| m.n_drive)
    }

    /// Drive FRAMES currently buffered in the output FIFO (test observability).
    pub fn output_buffered_frames(&self) -> usize {
        let n = self.map.as_ref().map_or(0, |m| m.n_drive);
        self.out_fifo.len().checked_div(n).unwrap_or(0)
    }

    /// Depth of the output FIFO in frames (the configured capacity).
    pub fn output_buffer_capacity_frames(&self) -> usize {
        self.cfg.output_buffer_frames.max(1)
    }

    /// The effective drive scaling from any active stop/emergency ramp (1.0 when none).
    fn drive_scale(&self) -> f64 {
        self.ramp.unwrap_or(1.0)
    }

    /// Advance the ramp one step; returns the scale to apply THIS sample. Once it decays
    /// below a small floor, the drive is pinned to zero.
    fn step_ramp(&mut self) -> f64 {
        match self.ramp {
            None => 1.0,
            Some(g) => {
                let scale = g;
                // Geometric decay toward zero. At 65 kHz a 0.99/sample factor is roughly a
                // 1.5 ms time constant: fast enough for a safe ramp-down, slow enough not
                // to slam the shaker with a step discontinuity.
                let next = g * 0.99;
                self.ramp = Some(if next < 1e-4 { 0.0 } else { next });
                scale
            }
        }
    }
}

impl Backend for SimBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: if self.cfg.shared_clock {
                ClockDomain::Shared
            } else {
                ClockDomain::Unknown
            },
            max_input_channels: self.cfg.max_input_channels,
            max_output_channels: self.cfg.max_output_channels,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.cfg.sample_rate_hz,
            // Irrelevant here and reported false rather than true: this is a
            // HostClosesLoop plant, so nothing is ever DELEGATED to it and the
            // resolver never reads this field for a host-loop backend. False is
            // the "claims nothing" posture.
            delegated_response_observable: false,
            // One implicit group: a single in-process plant. The scalar `clock`
            // above is the whole answer.
            clock_groups: vec![],
        }
    }

    fn open(&mut self) -> Result<(), HalError> {
        if self.state != State::Closed {
            return Err(HalError::Config(format!(
                "open() requires Closed state, was {:?}",
                self.state
            )));
        }
        self.state = State::Open;
        self.faulted = false;
        Ok(())
    }

    fn close(&mut self) -> Result<(), HalError> {
        // Close is always safe: reset the plant, drop the mapping, return to Closed.
        self.plant.reset();
        self.ramp = None;
        self.map = None;
        self.pending_drive.clear();
        self.out_fifo.clear();
        self.state = State::Closed;
        Ok(())
    }

    /// Apply the table and the acquisition request.
    ///
    /// The Sim NEVER coerces: it is software, so it can run at exactly the rate
    /// asked for, and `actual_rate_hz == acq.requested_rate_hz` always. A
    /// positive finite request is ADOPTED into the plant config (the plant is
    /// rebuilt below either way, so this costs nothing); a non-positive or
    /// non-finite request is refused rather than silently reinterpreted.
    ///
    /// `group_delay_frames` is `None`: an ideal plant has no acquisition-path
    /// delay to characterize, and `None` is "not characterized", which is the
    /// honest answer - reporting `Some(0)` would let a consumer treat a
    /// simulated station as having a PROVEN zero-delay path.
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        if self.state == State::Closed {
            return Err(HalError::NotConnected);
        }
        if !(acq.requested_rate_hz.is_finite() && acq.requested_rate_hz > 0.0) {
            return Err(HalError::Config(format!(
                "requested rate {} Hz is not a positive finite rate",
                acq.requested_rate_hz
            )));
        }
        if acq.block_frames == 0 {
            return Err(HalError::Config("block_frames must be at least 1".into()));
        }
        self.cfg.sample_rate_hz = acq.requested_rate_hz;
        // The table must at least be structurally sound.
        table
            .validate()
            .map_err(|e| HalError::Config(format!("channel table invalid: {e}")))?;

        // Map roles: Control/Monitor/Reference are measurement inputs; Drive is an output.
        let mut input_channels = Vec::new();
        let mut n_drive = 0usize;
        for ch in table.channels.iter().filter(|c| c.enabled) {
            match ch.role {
                ChannelRole::Drive => n_drive += 1,
                ChannelRole::Control | ChannelRole::Monitor | ChannelRole::Reference => {
                    input_channels.push(InputChannel {
                        table_index: ch.index,
                        role: ch.role,
                    });
                }
            }
        }

        if input_channels.is_empty() {
            return Err(HalError::Config(
                "no enabled input (control/monitor/reference) channels".into(),
            ));
        }
        if input_channels.len() as u32 > self.cfg.max_input_channels {
            return Err(HalError::Config(format!(
                "configured {} input channels exceeds max {}",
                input_channels.len(),
                self.cfg.max_input_channels
            )));
        }
        // A HostClosesLoop plant needs at least one drive output. If the table declares
        // none, synthesize a single implicit drive channel so the plant stays drivable.
        if n_drive == 0 {
            n_drive = 1;
        }
        if n_drive as u32 > self.cfg.max_output_channels {
            return Err(HalError::Config(format!(
                "configured {n_drive} drive channels exceeds max {}",
                self.cfg.max_output_channels
            )));
        }

        // Rebuild the plant fresh from config so re-configure is idempotent.
        self.plant = Plant::from_config(&self.cfg);
        self.pending_drive = vec![0.0; n_drive];
        // Preallocate the whole FIFO so nothing allocates on the hot path.
        self.out_fifo = VecDeque::with_capacity(self.cfg.output_buffer_frames.max(1) * n_drive);
        let roles = input_channels.iter().map(|c| c.role).collect();
        self.map = Some(ChannelMap {
            input_channels,
            roles,
            n_drive,
        });
        self.state = State::Configured;
        Ok(ConfiguredActual {
            actual_rate_hz: self.cfg.sample_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }

    fn arm(&mut self) -> Result<(), HalError> {
        match self.state {
            State::Configured | State::Stopped => {
                if self.map.is_none() {
                    return Err(HalError::Config("arm() before configure()".into()));
                }
                self.state = State::Armed;
                Ok(())
            }
            other => Err(HalError::Config(format!(
                "arm() requires Configured/Stopped, was {other:?}"
            ))),
        }
    }

    fn start(&mut self) -> Result<(), HalError> {
        match self.state {
            State::Armed => {
                self.ramp = None; // clear any prior ramp
                self.state = State::Running;
                Ok(())
            }
            other => Err(HalError::Config(format!(
                "start() requires Armed, was {other:?}"
            ))),
        }
    }

    fn stop(&mut self) -> Result<(), HalError> {
        // Stop from any live state; begin a graceful drive ramp-down.
        if matches!(self.state, State::Running | State::Armed) {
            self.ramp = Some(self.drive_scale());
            self.state = State::Stopped;
        }
        Ok(())
    }

    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let map = self.map.as_ref().ok_or(HalError::NotConnected)?;
        let n_ch = map.input_channels.len();
        if n_ch == 0 {
            // No input channels mapped: this stream can never produce a frame. The
            // old `Ok(0)` meant exactly this (a clean nothing-more-coming), so it is
            // `Ended`, not `NoDataYet` - configure() also refuses zero-input tables,
            // making this unreachable through the normal lifecycle.
            return Ok(InputRead::Ended);
        }
        if out.len() < n_ch {
            // The caller's buffer cannot hold even one frame - a caller bug, not a
            // stream condition. The old return conflated this with end-of-stream;
            // the Sim plant never runs dry (it synthesizes on demand), so say so.
            return Err(HalError::Config(format!(
                "read_input buffer holds {} samples, smaller than one {n_ch}-channel frame",
                out.len()
            )));
        }
        // Live plant response only while running/stopping; otherwise noise floor only.
        let running = matches!(self.state, State::Running | State::Stopped);
        let frames = out.len() / n_ch;
        let n_drive = map.n_drive;

        // Take the role vector out of `map` for the duration of the frame loop so the
        // plant/ramp/FIFO mutations below are not fighting an immutable borrow. It goes
        // straight back afterwards; nothing allocates (the Vec was built at configure).
        let mut roles = std::mem::take(&mut self.map.as_mut().expect("checked above").roles);

        for f in 0..frames {
            // CONSUME one drive frame per generated sample frame. An empty FIFO holds
            // the last consumed frame (zero-order hold) rather than cliffing to zero:
            // the host under-supplying drive is the engine's underrun contract to
            // police, and a DC step at the plant would be the worse failure.
            if self.out_fifo.len() >= n_drive && n_drive > 0 {
                for c in 0..n_drive {
                    self.pending_drive[c] = self.out_fifo.pop_front().unwrap_or(0.0);
                }
            }
            // One combined drive value feeds the (single-input) plant this frame.
            let drive_cmd = self.pending_drive.first().copied().unwrap_or(0.0);
            let scale = self.step_ramp();
            let drive = if running { drive_cmd * scale } else { 0.0 };
            let response = self.plant.step(drive);
            for (c, role) in roles.iter().enumerate() {
                // Reference channels read the commanded drive (a MIMO reference is the
                // commanded reference, not the plant response); everything else reads the
                // measured plant response. Deliberate, simple convention.
                let sample = match role {
                    ChannelRole::Reference => drive,
                    _ => response,
                };
                out[f * n_ch + c] = sample as f32;
            }
        }
        std::mem::swap(
            &mut self.map.as_mut().expect("checked above").roles,
            &mut roles,
        );
        self.sample_counter += frames as u64;
        // The Sim plant synthesizes on demand: every poll fills the whole buffer, so
        // `NoDataYet`/`Ended` never occur on a live configured stream.
        Ok(InputRead::Frames(frames))
    }

    /// Buffer as many whole drive frames as the output FIFO has room for, and report
    /// how many were ACCEPTED (`0..=offered`). Nothing is decimated and nothing is
    /// silently dropped: the frames the FIFO could not take are the caller's to
    /// retain and re-offer (the engine's top-up policy).
    ///
    /// A sub-frame write accepts nothing and buffers nothing - a partial frame has no
    /// meaning at the plant, and half-applying one would inject an out-of-band drive
    /// value. No engine caller produces one (the engine offers whole frames).
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let map = self.map.as_ref().ok_or(HalError::NotConnected)?;
        if map.n_drive == 0 {
            return Err(HalError::Unsupported("no drive channels configured"));
        }
        if drive.is_empty() {
            return Ok(0);
        }
        let n = map.n_drive;
        let cap_frames = self.cfg.output_buffer_frames.max(1);
        let offered = drive.len() / n;
        let free = cap_frames.saturating_sub(self.out_fifo.len() / n);
        let accepted = offered.min(free);
        for s in &drive[..accepted * n] {
            self.out_fifo.push_back(*s as f64);
        }
        Ok(accepted)
    }

    /// Real remaining headroom in the output FIFO, in frames. `Some`, never `None`:
    /// this backend HAS an independent output buffer now, so the engine takes its
    /// top-up path against the Sim in CI (that is the point of the model).
    fn output_space(&mut self) -> Option<usize> {
        let n = self.map.as_ref().map_or(0, |m| m.n_drive);
        if n == 0 {
            return None; // no drive path at all: nothing to top up
        }
        Some(
            self.cfg
                .output_buffer_frames
                .max(1)
                .saturating_sub(self.out_fifo.len() / n),
        )
    }

    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.state != State::Closed,
            faulted: self.faulted,
            message: format!("{:?}", self.state),
            sample_counter: self.sample_counter,
            epoch: self.epoch,
        }
    }

    fn emergency_stop(&mut self) -> Result<(), HalError> {
        // Begin a hard ramp from the current scale toward zero and latch Stopped. There is
        // no firmware here, but the "never issue flash/firmware opcodes" contract still
        // holds: this only touches internal drive state. Subsequent reads produce a
        // decaying-then-zero drive into the plant.
        self.ramp = Some(self.drive_scale());
        if matches!(self.state, State::Running | State::Armed) {
            self.state = State::Stopped;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Tests (all hardware-free).
// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};

    /// A minimal single-control-channel table with one drive output.
    fn table_1ctrl_1drive() -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        t.channels.push(ctrl);
        t.channels.push(drv);
        t
    }

    /// The acquisition request that leaves the plant exactly as it was built:
    /// its OWN rate (the Sim adopts the request, so passing its own rate is the
    /// no-op) and the one-frame-per-block lockstep cadence the Sim runs at.
    fn same_rate(b: &SimBackend) -> AcquisitionConfig {
        AcquisitionConfig::new(b.caps().sample_rate_hz, 1)
    }

    /// Bring a backend fully up to Running against `table`.
    fn run_backend(mut b: SimBackend, table: &ChannelTable) -> SimBackend {
        b.open().unwrap();
        let acq = same_rate(&b);
        b.configure(table, &acq).unwrap();
        b.arm().unwrap();
        b.start().unwrap();
        b
    }

    /// Drive the plant with a fixed-frequency sine for `n` samples and return the RMS of
    /// the control-channel response (channel 0 in the interleaved buffer).
    fn steady_state_rms(b: &mut SimBackend, freq_hz: f64, amp: f64, n: usize) -> f64 {
        let fs = b.caps().sample_rate_hz;
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch];
        // Warm-up: let the resonator reach steady state (discard the transient).
        let warm = (fs / freq_hz * 50.0) as usize;
        let mut phase = 0.0f64;
        let dphi = TAU * freq_hz / fs;
        for _ in 0..warm {
            let s = amp * phase.sin();
            phase += dphi;
            b.write_drive(&[s as f32]).unwrap();
            b.read_input(&mut buf).unwrap();
        }
        // Measure.
        let mut acc = 0.0f64;
        for _ in 0..n {
            let s = amp * phase.sin();
            phase += dphi;
            b.write_drive(&[s as f32]).unwrap();
            b.read_input(&mut buf).unwrap();
            let v = buf[0] as f64;
            acc += v * v;
        }
        (acc / n as f64).sqrt()
    }

    #[test]
    fn caps_defaults_are_the_audit_posture() {
        let b = SimBackend::new();
        let caps = b.caps();
        assert_eq!(caps.loop_owner, LoopOwner::HostClosesLoop);
        assert!(caps.native_modes.is_empty());
        // Default sim does NOT claim a shared clock.
        assert_eq!(caps.clock, ClockDomain::Unknown);
        assert_eq!(caps.sample_rate_hz, DEFAULT_SAMPLE_RATE_HZ);
        assert!(caps.output_buffer.is_some());
    }

    #[test]
    fn shared_clock_opt_in_flips_clock_domain() {
        assert_eq!(
            SimBackend::with_shared_clock().caps().clock,
            ClockDomain::Shared
        );
        assert_eq!(
            SimConfig::new().shared_clock(true).build().caps().clock,
            ClockDomain::Shared
        );
    }

    #[test]
    fn lifecycle_transitions_are_sane() {
        let table = table_1ctrl_1drive();
        let mut b = SimBackend::new();
        let acq = same_rate(&b);
        // Cannot configure before open.
        assert!(matches!(
            b.configure(&table, &acq),
            Err(HalError::NotConnected)
        ));
        b.open().unwrap();
        // Cannot re-open.
        assert!(b.open().is_err());
        b.configure(&table, &acq).unwrap();
        assert_eq!(b.n_input_channels(), 1);
        assert_eq!(b.n_drive_channels(), 1);
        // Cannot start before arm.
        assert!(b.start().is_err());
        b.arm().unwrap();
        b.start().unwrap();
        b.stop().unwrap();
        // Re-arm from stopped is allowed.
        b.arm().unwrap();
        b.close().unwrap();
        assert!(!b.poll_status().connected);
    }

    #[test]
    fn configure_maps_roles_to_inputs_and_drives() {
        // 2 control + 3 monitor + 1 reference (needs control) + 2 drive.
        let mut t = ChannelTable::new();
        for i in 1..=2 {
            let mut c = Channel::new(i);
            c.role = ChannelRole::Control;
            t.channels.push(c);
        }
        for i in 3..=5 {
            let mut c = Channel::new(i);
            c.role = ChannelRole::Monitor;
            t.channels.push(c);
        }
        let mut r = Channel::new(6);
        r.role = ChannelRole::Reference;
        t.channels.push(r);
        for i in 7..=8 {
            let mut c = Channel::new(i);
            c.role = ChannelRole::Drive;
            t.channels.push(c);
        }
        let mut b = SimBackend::new();
        b.open().unwrap();
        let acq = same_rate(&b);
        b.configure(&t, &acq).unwrap();
        // 2 + 3 + 1 = 6 input channels, 2 drive channels.
        assert_eq!(b.n_input_channels(), 6);
        assert_eq!(b.n_drive_channels(), 2);
    }

    #[test]
    fn resonance_amplifies_by_about_q_and_off_resonance_is_much_smaller() {
        let f_n = 250.0;
        let q = 20.0;
        let cfg = SimConfig::new()
            .resonance(f_n, q)
            .noise_level(0.0) // isolate the transfer function
            .feedthrough(1e-3)
            .seed(42);
        let table = table_1ctrl_1drive();

        // On resonance.
        let mut b_on = run_backend(cfg.clone().build(), &table);
        let rms_on = steady_state_rms(&mut b_on, f_n, 1.0, 8192);

        // Off resonance: a decade below.
        let mut b_off = run_backend(cfg.clone().build(), &table);
        let rms_off = steady_state_rms(&mut b_off, f_n / 10.0, 1.0, 8192);

        // On-resonance RMS of a unit-amplitude sine is ~Q/sqrt(2). Allow a generous band
        // for discretization; the point is the peak is on the order of Q, not unity.
        let expected_on = q / 2f64.sqrt();
        assert!(
            rms_on > expected_on * 0.5 && rms_on < expected_on * 1.6,
            "on-resonance rms {rms_on:.3} not within [~{expected_on:.2}]"
        );
        // Off-resonance response is far smaller than on-resonance.
        assert!(
            rms_on / rms_off > 5.0,
            "resonance gain ratio too small: on={rms_on:.4} off={rms_off:.4}"
        );
    }

    #[test]
    fn swept_sine_peaks_at_each_mode() {
        let modes = vec![(150.0, 25.0), (600.0, 25.0)];
        let cfg = SimConfig::new()
            .resonances(modes.clone())
            .noise_level(0.0)
            .feedthrough(5e-4)
            .seed(7);
        let table = table_1ctrl_1drive();

        // Sample the response magnitude at each mode and at an in-between valley.
        let probe = |hz: f64| {
            let mut b = run_backend(cfg.clone().build(), &table);
            steady_state_rms(&mut b, hz, 1.0, 4096)
        };
        let at_mode1 = probe(150.0);
        let at_mode2 = probe(600.0);
        let at_valley = probe(350.0); // between the two peaks

        assert!(
            at_mode1 / at_valley > 4.0,
            "mode1 not a peak: {at_mode1:.3} vs valley {at_valley:.3}"
        );
        assert!(
            at_mode2 / at_valley > 4.0,
            "mode2 not a peak: {at_mode2:.3} vs valley {at_valley:.3}"
        );
    }

    #[test]
    fn noise_floor_is_present_and_bounded() {
        let sigma = 0.01;
        let cfg = SimConfig::new()
            .noise_level(sigma)
            .feedthrough(0.0)
            .seed(123);
        let table = table_1ctrl_1drive();
        let mut b = run_backend(cfg.build(), &table);
        // Zero drive: output should be pure noise. Collect samples.
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch];
        let mut samples = Vec::new();
        for _ in 0..20_000 {
            b.write_drive(&[0.0]).unwrap();
            b.read_input(&mut buf).unwrap();
            samples.push(buf[0] as f64);
        }
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
        let std = var.sqrt();
        // Noise present.
        assert!(std > 0.0, "no noise floor at all");
        // And bounded near the configured sigma (within a factor of ~2 each way).
        assert!(
            std > sigma * 0.5 && std < sigma * 2.0,
            "noise std {std:.5} not near configured sigma {sigma}"
        );
        // No individual sample is absurdly large (bounded floor).
        let max_abs = samples.iter().map(|x| x.abs()).fold(0.0, f64::max);
        assert!(max_abs < sigma * 10.0, "noise sample {max_abs} unbounded");
    }

    #[test]
    fn prng_is_deterministic_for_a_given_seed() {
        let cfg = || {
            SimConfig::new()
                .noise_level(0.02)
                .feedthrough(0.0)
                .seed(999)
        };
        let table = table_1ctrl_1drive();
        let collect = || {
            let mut b = run_backend(cfg().build(), &table);
            let n_ch = b.n_input_channels();
            let mut buf = vec![0.0f32; n_ch];
            let mut out = Vec::new();
            for _ in 0..500 {
                b.write_drive(&[0.0]).unwrap();
                b.read_input(&mut buf).unwrap();
                out.push(buf[0]);
            }
            out
        };
        assert_eq!(
            collect(),
            collect(),
            "same seed must give same noise stream"
        );
    }

    #[test]
    fn poll_status_counter_is_strictly_monotonic_across_reads() {
        let table = table_1ctrl_1drive();
        let mut b = run_backend(SimBackend::new(), &table);
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch * 64]; // 64 frames per read
        let mut last = b.poll_status().sample_counter;
        for _ in 0..50 {
            b.write_drive(&[0.0]).unwrap();
            let frames = b.read_input(&mut buf).unwrap();
            assert_eq!(frames, InputRead::Frames(64));
            let now = b.poll_status().sample_counter;
            assert!(now > last, "counter not monotonic: {now} !> {last}");
            last = now;
        }
    }

    #[test]
    fn transport_delay_shifts_the_response() {
        // A pure impulse should appear delayed by exactly `transport_delay_samples`.
        let delay = 16usize;
        let cfg = SimConfig::new()
            .resonance(250.0, 5.0)
            .noise_level(0.0)
            .feedthrough(1.0) // pass-through so the impulse is visible immediately
            .transport_delay_samples(delay)
            .seed(1);
        let table = table_1ctrl_1drive();
        let mut b = run_backend(cfg.build(), &table);
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch];

        // First sample: impulse of amplitude 1.0.
        let mut trace = Vec::new();
        for i in 0..(delay + 8) {
            let d = if i == 0 { 1.0f32 } else { 0.0f32 };
            b.write_drive(&[d]).unwrap();
            b.read_input(&mut buf).unwrap();
            trace.push(buf[0] as f64);
        }
        // Feedthrough of the impulse should land at index == delay (not index 0).
        assert!(
            trace[0].abs() < 1e-9,
            "impulse leaked with zero delay: {}",
            trace[0]
        );
        assert!(
            trace[delay].abs() > 0.1,
            "no feedthrough impulse at the delay index: {}",
            trace[delay]
        );
    }

    #[test]
    fn emergency_stop_drives_output_toward_zero() {
        let f_n = 250.0;
        let cfg = SimConfig::new()
            .resonance(f_n, 20.0)
            .noise_level(0.0)
            .seed(5);
        let table = table_1ctrl_1drive();
        let mut b = run_backend(cfg.build(), &table);
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch];

        // Drive a steady on-resonance sine to build up energy.
        let fs = b.caps().sample_rate_hz;
        let dphi = TAU * f_n / fs;
        let mut phase = 0.0f64;
        for _ in 0..8192 {
            let s = phase.sin();
            phase += dphi;
            b.write_drive(&[s as f32]).unwrap();
            b.read_input(&mut buf).unwrap();
        }
        // Trigger E-stop, then keep pushing the SAME drive command; the internal ramp must
        // pull the delivered drive (and hence the response) toward zero. Let the ramp and
        // the resonator ringdown settle first, then measure the SETTLED tail.
        b.emergency_stop().unwrap();
        // Let the drive ramp complete AND the Q=20 mode ring down. The ringdown time
        // constant is ~Q/(pi*f_n) ~= 25 ms (~1670 samples); allow many time constants.
        let settle_n = 40_000;
        for _ in 0..settle_n {
            let s = phase.sin();
            phase += dphi;
            b.write_drive(&[s as f32]).unwrap();
            b.read_input(&mut buf).unwrap();
        }
        let mut tail_rms = 0.0f64;
        let tail_n = 8192;
        for _ in 0..tail_n {
            let s = phase.sin();
            phase += dphi;
            b.write_drive(&[s as f32]).unwrap();
            b.read_input(&mut buf).unwrap();
            let v = buf[0] as f64;
            tail_rms += v * v;
        }
        tail_rms = (tail_rms / tail_n as f64).sqrt();
        // After the ramp completes and the mode rings down, the response is essentially 0.
        assert!(
            tail_rms < 0.01,
            "emergency_stop did not drive output to zero (settled tail rms {tail_rms:.5})"
        );
    }

    #[test]
    fn closed_loop_proportional_control_converges_to_target() {
        // A minimal HostClosesLoop control test: drive amplitude adjusted toward the error
        // between a target response level and the measured control-channel envelope. With
        // a stable plant and a small gain, the response amplitude converges to the target.
        let f_n = 300.0;
        let cfg = SimConfig::new()
            .resonance(f_n, 15.0)
            .noise_level(1e-4)
            .feedthrough(1e-3)
            .shared_clock(true) // closed loop: a sim is shared-clock by construction
            .seed(2024);
        let table = table_1ctrl_1drive();
        let mut b = run_backend(cfg.build(), &table);
        assert_eq!(b.caps().clock, ClockDomain::Shared);

        let fs = b.caps().sample_rate_hz;
        let dphi = TAU * f_n / fs;
        let mut phase = 0.0f64;
        let n_ch = b.n_input_channels();
        let mut buf = vec![0.0f32; n_ch];

        let target = 5.0f64; // target response amplitude (arbitrary g)
        let mut drive_amp = 0.05f64; // start well below what is needed
        let mut env = 0.0f64; // one-pole envelope of |response|
        let alpha = 0.001; // envelope smoothing
        let ki = 0.02; // gain on the amplitude command

        let mut est_amp_final = 0.0;
        for block in 0..400 {
            for _ in 0..256 {
                let s = drive_amp * phase.sin();
                phase += dphi;
                b.write_drive(&[s as f32]).unwrap();
                b.read_input(&mut buf).unwrap();
                let v = (buf[0] as f64).abs();
                env += alpha * (v - env);
            }
            // |sine| envelope relates to amplitude by 2/pi; convert to an RMS-comparable
            // "amplitude" estimate.
            let est_amp = env * std::f64::consts::FRAC_PI_2 * 2f64.sqrt();
            let err = target - est_amp;
            drive_amp += ki * err * (drive_amp.abs() + 0.1) / (est_amp.abs() + 0.1);
            drive_amp = drive_amp.clamp(0.0, 100.0);
            if block > 300 {
                est_amp_final = est_amp;
            }
        }
        // After convergence the estimated response amplitude should be near the target.
        assert!(
            (est_amp_final - target).abs() / target < 0.25,
            "closed loop did not converge: est {est_amp_final:.3} vs target {target}"
        );
    }

    #[test]
    fn write_drive_requires_configuration() {
        let mut b = SimBackend::new();
        // Not configured yet.
        assert!(matches!(b.write_drive(&[0.0]), Err(HalError::NotConnected)));
    }

    /// The test that encodes the bug. This test used to be called
    /// `multichannel_drive_takes_the_last_frame` and asserted only
    /// `frames == Frames(1)` - a claim so weak it passed both before and after the
    /// latch was removed, while its NAME kept asserting the decimating behavior as
    /// if it were the contract. It was never a safety assertion, so it is rewritten
    /// to the new truth rather than weakened: every written frame is buffered and
    /// consumed IN ORDER, one per generated sample frame.
    #[test]
    fn multichannel_drive_buffers_every_frame_in_order() {
        // 1 reference input (reads the COMMANDED drive) + 2 drive outputs.
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        t.channels.push(ctrl);
        let mut r = Channel::new(2);
        r.role = ChannelRole::Reference;
        t.channels.push(r);
        for i in 3..=4 {
            let mut d = Channel::new(i);
            d.role = ChannelRole::Drive;
            t.channels.push(d);
        }
        let mut b = run_backend(SimBackend::new(), &t);
        assert_eq!(b.n_drive_channels(), 2);
        assert_eq!(b.n_input_channels(), 2);

        // Three frames of width 2. Under the retired latch the plant would have seen
        // only 0.75 (the last frame's channel 0), three times.
        let n = b.write_drive(&[0.25, 0.0, 0.5, 0.0, 0.75, 0.0]).unwrap();
        assert_eq!(n, 3, "all three offered frames accepted");
        assert_eq!(b.output_buffered_frames(), 3);

        // Read them back one frame at a time; the REFERENCE channel (slot 1) reads
        // the commanded drive, so it is a direct view of what reached the plant.
        let mut buf = vec![0.0f32; 2];
        let mut seen = Vec::new();
        for _ in 0..3 {
            assert_eq!(b.read_input(&mut buf).unwrap(), InputRead::Frames(1));
            seen.push(buf[1]);
        }
        assert_eq!(seen, vec![0.25, 0.5, 0.75], "FIFO order, nothing decimated");
        assert_eq!(b.output_buffered_frames(), 0);
    }

    /// A BLOCK write followed by a block read must stream every drive sample into
    /// the plant. This is the property the engine's `frames_per_tick > 1` path and
    /// the direct-backend block players both depend on.
    #[test]
    fn block_write_then_block_read_streams_every_drive_sample() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        t.channels.push(ctrl);
        let mut r = Channel::new(2);
        r.role = ChannelRole::Reference;
        t.channels.push(r);
        let mut d = Channel::new(3);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
        let mut b = run_backend(SimBackend::new(), &t);

        let block: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        assert_eq!(b.write_drive(&block).unwrap(), 64);
        let mut buf = vec![0.0f32; 2 * 64];
        assert_eq!(b.read_input(&mut buf).unwrap(), InputRead::Frames(64));
        let commanded: Vec<f32> = (0..64).map(|f| buf[f * 2 + 1]).collect();
        assert_eq!(
            commanded, block,
            "every drive sample of the block reached the plant, in order"
        );
    }

    /// The bounded output buffer: `write_drive` accepts only what fits and reports
    /// it, `output_space` reports the real headroom, and reading frees room. This is
    /// what makes the engine's retain-and-re-offer top-up path CI-exercised.
    #[test]
    fn output_buffer_is_bounded_and_reports_real_space() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        t.channels.push(ctrl);
        let mut d = Channel::new(2);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
        let mut b = run_backend(SimConfig::new().output_buffer_frames(6).build(), &t);

        assert_eq!(b.output_space(), Some(6));
        // Offer 10 frames into a 6-frame buffer: 6 accepted, 4 refused (the caller
        // retains them - nothing is silently dropped here).
        let offered: Vec<f32> = (0..10).map(|i| i as f32).collect();
        assert_eq!(b.write_drive(&offered).unwrap(), 6);
        assert_eq!(b.output_space(), Some(0));
        assert_eq!(b.write_drive(&offered[6..]).unwrap(), 0, "buffer is full");

        // Draining two frames opens exactly two frames of room.
        let mut buf = vec![0.0f32; 2];
        assert_eq!(b.read_input(&mut buf).unwrap(), InputRead::Frames(2));
        assert_eq!(b.output_space(), Some(2));
        assert_eq!(b.write_drive(&offered[6..]).unwrap(), 2);
    }

    /// An empty FIFO HOLDS the last consumed drive frame rather than cliffing to
    /// zero. A DC step at the plant is the worse failure; keeping the host supplied
    /// is the engine's lookahead/underrun contract, not the plant's job.
    #[test]
    fn empty_output_fifo_holds_the_last_drive_rather_than_stepping_to_zero() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        t.channels.push(ctrl);
        let mut r = Channel::new(2);
        r.role = ChannelRole::Reference;
        t.channels.push(r);
        let mut d = Channel::new(3);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
        let mut b = run_backend(SimBackend::new(), &t);

        assert_eq!(b.write_drive(&[0.9]).unwrap(), 1);
        let mut buf = vec![0.0f32; 2 * 4]; // ask for 4 frames with only 1 queued
        assert_eq!(b.read_input(&mut buf).unwrap(), InputRead::Frames(4));
        let commanded: Vec<f32> = (0..4).map(|f| buf[f * 2 + 1]).collect();
        assert_eq!(commanded, vec![0.9, 0.9, 0.9, 0.9], "held, not cliffed");
    }

    /// A sub-frame write buffers NOTHING and says so. Half a frame has no meaning at
    /// the plant, and the old code's "take the first sample into channel 0" injected
    /// an out-of-band drive value on a path no engine caller reaches.
    #[test]
    fn sub_frame_write_accepts_nothing() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        t.channels.push(ctrl);
        for i in 2..=3 {
            let mut d = Channel::new(i);
            d.role = ChannelRole::Drive;
            t.channels.push(d);
        }
        let mut b = run_backend(SimBackend::new(), &t);
        assert_eq!(b.write_drive(&[0.5]).unwrap(), 0, "half a frame of two");
        assert_eq!(b.output_buffered_frames(), 0);
    }
}
