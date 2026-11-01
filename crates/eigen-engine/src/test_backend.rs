//! In-crate MOCK backend implementing `eigen_hal::Backend`, hardware-free.
//!
//! This is a TEST double (compiled only under `cfg(test)`). It deliberately does NOT
//! depend on `eigen-hal-sim`: the engine's own tests must not co-confirm against the
//! real Sim plant. The mock emits scripted synthetic input frames and records the drive
//! frames pushed back, and can inject the four fault classes the safety tests need:
//!
//! * an over-limit spike on a channel (script a frame value above a limit),
//! * a counter reset (bump `epoch`, reset `sample_counter`),
//! * a counter gap (advance `sample_counter` by more than the frames delivered),
//! * (drive underrun is exercised at the `OutputDelivery` level, not here).
//!
//! The mock is a `HostClosesLoop` backend with `clock = Unknown` (the honest default).
//!
//! This is a test utility whose API is intentionally broader than any single test uses
//! (it is `#[path]`-included by the integration test as well), so unused-item lints are
//! silenced module-wide.
#![allow(dead_code)]

use eigen_channel_table::ChannelTable;
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};

/// One scripted input block plus the counter/epoch the device reports after it.
#[derive(Debug, Clone)]
pub struct ScriptedBlock {
    /// Interleaved f32 frames: `n_frames * n_channels`.
    pub samples: Vec<f32>,
    pub n_frames: usize,
    /// The `sample_counter` the device reports after this block is read.
    pub counter_after: u64,
    /// The `epoch` the device reports after this block (bump to inject a reset).
    pub epoch_after: u32,
}

pub struct MockBackend {
    n_channels: usize,
    /// Width of one DRIVE frame (the engine's `out_channels`). Defaults to
    /// `n_channels`; tests with asymmetric in/out shapes set it explicitly so the
    /// accepted-frames return of `write_drive` counts in the right unit.
    drive_channels: usize,
    sample_rate_hz: f64,
    script: Vec<ScriptedBlock>,
    next_block: usize,
    status: BackendStatus,
    opened: bool,
    armed: bool,
    started: bool,
    estop_count: u32,
    /// Drive frames captured from `write_drive` (interleaved), for happy-path assertions.
    pub captured_drive: Vec<f32>,
    faulted: bool,
    /// When `Some(cap)`, the mock has an INDEPENDENT output buffer of `cap` frames:
    /// `output_space()` reports real headroom, `write_drive` accepts only what fits,
    /// and `drain_output(n)` plays `n` frames out of it. When `None` (the default) the
    /// mock is a lockstep backend with no space semantics.
    out_capacity: Option<usize>,
    /// Frames currently sitting in the simulated output buffer.
    out_filled: usize,
    /// Frames played out of the simulated output buffer on every `read_input`.
    out_drain_per_read: usize,
    /// When true, `write_drive` answers `Ok(0)` however much space it reports.
    refuse_writes: bool,
    /// `write_drive` calls seen (top-up policy observability: the top-up path issues
    /// ONE call per tick, the retired lockstep issued one per frame).
    pub write_calls: usize,
}

impl MockBackend {
    /// A bare mock with no script (used by unit tests that only need `emergency_stop`).
    pub fn new(n_channels: usize) -> Self {
        MockBackend {
            n_channels,
            drive_channels: n_channels,
            sample_rate_hz: 1000.0,
            script: Vec::new(),
            next_block: 0,
            status: BackendStatus {
                connected: true,
                faulted: false,
                message: String::new(),
                sample_counter: 0,
                epoch: 0,
            },
            opened: false,
            armed: false,
            started: false,
            estop_count: 0,
            captured_drive: Vec::new(),
            faulted: false,
            out_capacity: None,
            out_filled: 0,
            out_drain_per_read: 0,
            refuse_writes: false,
            write_calls: 0,
        }
    }

    /// Give the mock an independent output buffer of `cap` frames, so
    /// `output_space()` reports `Some(..)` and the engine takes its TOP-UP path.
    /// `drain_per_read` frames are played out of it on every `read_input` (a
    /// DMA-clocked AO buffer draining at the sample clock); `0` models a device
    /// whose output has stopped consuming.
    pub fn with_output_buffer(mut self, cap: usize, drain_per_read: usize) -> Self {
        self.out_capacity = Some(cap);
        self.out_drain_per_read = drain_per_read;
        self
    }

    /// Start with the simulated output buffer already FULL, so `output_space()`
    /// answers `Some(0)` from the very first tick.
    ///
    /// This is the NI shape, not a contrived one. `NiBackend::output_space`
    /// answers `Some(0)` whenever `query_write_space()` itself fails - a
    /// deliberate asymmetry with `write_drive`, which on the same query failure
    /// offers the whole block and lets the write be the authority. In that state
    /// the engine's `offer` is 0, so `write_drive` is NEVER CALLED, and the only
    /// guard left is the staging-depth cap.
    pub fn with_output_full(mut self) -> Self {
        self.out_filled = self.out_capacity.unwrap_or(0);
        self
    }

    /// Refuse every write regardless of reported space: `write_drive` returns
    /// `Ok(0)` while `output_space()` still says there is room.
    ///
    /// Also the NI shape. `NiBackend::write_drive` treats a DAQmx timeout as
    /// honest backpressure (`is_timeout(status) -> Ok(written)`), so a device
    /// whose AO has stopped consuming returns `Ok(0)` indefinitely with no
    /// error - the exact starvation signature, and the one a `Result`-shaped
    /// caller is least likely to be watching for.
    pub fn refusing_all_writes(mut self) -> Self {
        self.refuse_writes = true;
        self
    }

    /// Play `n` frames out of the simulated output buffer (frees that much space).
    pub fn drain_output(&mut self, n: usize) {
        self.out_filled = self.out_filled.saturating_sub(n);
    }

    /// Frames currently buffered device-side.
    pub fn output_filled(&self) -> usize {
        self.out_filled
    }

    pub fn with_rate(mut self, hz: f64) -> Self {
        self.sample_rate_hz = hz;
        self
    }

    /// Set the drive-frame width for runs whose `out_channels != n_channels`.
    pub fn with_drive_channels(mut self, n: usize) -> Self {
        self.drive_channels = n;
        self
    }

    /// Load a scripted sequence of input blocks.
    pub fn with_script(mut self, script: Vec<ScriptedBlock>) -> Self {
        self.script = script;
        self
    }

    /// Number of times `emergency_stop()` has been called (safety-test observability).
    pub fn estop_count(&self) -> u32 {
        self.estop_count
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Whether the script is exhausted.
    pub fn script_done(&self) -> bool {
        self.next_block >= self.script.len()
    }

    /// Helper to build a flat block of `n_frames` constant-per-channel samples.
    pub fn const_block(
        value_per_channel: &[f32],
        n_frames: usize,
        counter_after: u64,
        epoch_after: u32,
    ) -> ScriptedBlock {
        let mut samples = Vec::with_capacity(n_frames * value_per_channel.len());
        for _ in 0..n_frames {
            samples.extend_from_slice(value_per_channel);
        }
        ScriptedBlock {
            samples,
            n_frames,
            counter_after,
            epoch_after,
        }
    }
}

impl Backend for MockBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Unknown,
            max_input_channels: self.n_channels as u32,
            max_output_channels: self.n_channels as u32,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.sample_rate_hz,
            // A host-loop mock delegates nothing and has
            // one implicit clock group.
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }

    fn open(&mut self) -> Result<(), HalError> {
        self.opened = true;
        Ok(())
    }

    fn close(&mut self) -> Result<(), HalError> {
        self.opened = false;
        Ok(())
    }

    fn configure(
        &mut self,
        _table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        // The mock never coerces: it delivers exactly what it was asked for.
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }

    fn arm(&mut self) -> Result<(), HalError> {
        if !self.opened {
            return Err(HalError::NotConnected);
        }
        self.armed = true;
        Ok(())
    }

    fn start(&mut self) -> Result<(), HalError> {
        if !self.armed {
            return Err(HalError::Config("not armed".into()));
        }
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), HalError> {
        self.started = false;
        Ok(())
    }

    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        if self.faulted {
            return Err(HalError::Io("faulted".into()));
        }
        // A buffered output drains at the sample clock, in step with acquisition.
        self.out_filled = self.out_filled.saturating_sub(self.out_drain_per_read);
        if self.next_block >= self.script.len() {
            return Ok(InputRead::Ended); // script exhausted: clean end of stream
        }
        let block = &self.script[self.next_block];
        let n = block.samples.len().min(out.len());
        out[..n].copy_from_slice(&block.samples[..n]);
        let frames = block.n_frames.min(n / self.n_channels.max(1));
        self.status.sample_counter = block.counter_after;
        self.status.epoch = block.epoch_after;
        self.next_block += 1;
        Ok(InputRead::Frames(frames))
    }

    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.write_calls += 1;
        let n_ch = self.drive_channels.max(1);
        let offered = drive.len() / n_ch;
        match self.out_capacity {
            // Lockstep mock: accept every offered frame, counted in DRIVE frames.
            None => {
                self.captured_drive.extend_from_slice(drive);
                Ok(offered)
            }
            // Buffered mock: accept only what fits, and say so.
            Some(cap) => {
                let accepted = if self.refuse_writes {
                    0
                } else {
                    offered.min(cap.saturating_sub(self.out_filled))
                };
                self.captured_drive
                    .extend_from_slice(&drive[..accepted * n_ch]);
                self.out_filled += accepted;
                Ok(accepted)
            }
        }
    }

    fn output_space(&mut self) -> Option<usize> {
        self.out_capacity.map(|c| c.saturating_sub(self.out_filled))
    }

    fn poll_status(&mut self) -> BackendStatus {
        self.status.clone()
    }

    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.estop_count += 1;
        self.started = false;
        self.status.faulted = true;
        Ok(())
    }
}
