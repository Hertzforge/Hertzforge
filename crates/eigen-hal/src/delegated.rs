//! Delegated control: the device-neutral vocabulary for a backend whose DEVICE runs
//! the loop (`LoopOwner::DeviceClosesLoop`).
//!
//! # What is here and what is not
//!
//! The original design asked for device verbs on the HAL. The bench history moved that
//! line: the choreography (what bytes to send, and when) is mode-specific, so it stays
//! in each private delegate crate. What can be shared is the VOCABULARY and the
//! LIFECYCLE, and that is all this module holds: [`DelegatedControl`] (start, level,
//! telemetry, loop state, ramp-down, hold, re-arm, emergency stop), the tagged
//! [`LevelCommand`], the device-reported [`DeviceLoopState`], and a [`TelemetryChunk`]
//! whose channel count and rate are data rather than a crate constant.
//!
//! Nothing here sends a byte. Module ports (for example a Random delegate port) are
//! written against this trait in the module crates; implementors live in private
//! backend crates and convert their own wire types at that boundary.
//!
//! # The evidence gate
//!
//! A [`DelegatedControl`] is created only by a factory that has passed the evidence
//! gate: the backend's `Caps.native_modes` declares the mode, and
//! `eigen_module_abi::resolve_loop` admits the (module, backend) pair. This module
//! does not and cannot enforce that; it is the factory's job, and the trait's
//! [`DelegatedControl::caps`] is the claim the factory checks.
//!
//! # The test double
//!
//! [`testing::ScriptedDelegate`] implements the trait for tests that drive routing
//! through a port without a device. It exists only under `cfg(test)` or the
//! `test-support` feature, and the sim-leak lint
//! (`crates/eigen-hal-sim/tests/no_sim_leak.rs`) refuses both production code that
//! names it and any production `[dependencies]` entry that enables that feature.

use std::time::Duration;

use crate::{Caps, HalError};

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// A device that runs its own control loop, driven through its lifecycle.
///
/// Timing (pacing between blocks, GO-to-run-begin gaps) is the implementor's: the
/// trait says WHAT happens, never how long the device needs.
pub trait DelegatedControl: Send {
    /// The evidence claim this device was admitted on. Unchanged by the lifecycle.
    fn caps(&self) -> Caps;

    /// Begin the device's run (GO, then whatever gated run-begin the device needs).
    fn start(&mut self) -> Result<StartReport, HalError>;

    /// Ask the running device to change level or frequency.
    fn level(&mut self, cmd: LevelCommand) -> Result<LevelAck, HalError>;

    /// The next block of device telemetry, or `Ok(None)` when none arrived within
    /// `timeout`. `Ok(None)` is a steady-state answer, not an end of run.
    fn recv_telemetry(&mut self, timeout: Duration) -> Result<Option<TelemetryChunk>, HalError>;

    /// The device's own state, distinct from the sample stream.
    fn loop_state(&self) -> DeviceLoopState;

    /// Graceful stop: ramp the device's drive down and leave the run.
    fn ramp_down(&mut self) -> Result<(), HalError>;

    /// Stop driving but stay loaded, so [`DelegatedControl::rearm`] can resume.
    /// Not every device can; the default refuses by name.
    fn hold(&mut self) -> Result<(), HalError> {
        Err(HalError::Unsupported("hold"))
    }

    /// Re-enter the run from a hold. The default refuses by name.
    fn rearm(&mut self) -> Result<(), HalError> {
        Err(HalError::Unsupported("rearm"))
    }

    /// Stop now, from any state. Implementors must not refuse this on lifecycle
    /// grounds; an error here means the stop itself could not be delivered.
    fn emergency_stop(&mut self) -> Result<(), HalError>;
}

/// Which quantity a [`LevelCommand`] moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Frequency,
    Level,
}

/// The sense of an increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// The tagged level command: increments and absolute targets are different
/// commands, never one number whose meaning depends on the device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelCommand {
    /// `count` increments in `direction`. `coarse` selects the device's larger step
    /// where it has one. Used by devices that accept only relative steps.
    Nudge {
        axis: Axis,
        direction: Direction,
        coarse: bool,
        count: u32,
    },
    /// An absolute value the device applies directly (for example a linear scale
    /// factor).
    Setpoint { axis: Axis, value: f64 },
    /// A target in dB that the implementor reaches by its own paced sequence.
    StepTo { axis: Axis, target_db: f64 },
}

/// What a [`DelegatedControl::level`] call did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelAck {
    /// Commands actually sent to the device (a refused or truncated step says so).
    pub sent: u32,
    /// The host's estimate of the resulting value, when it has one.
    pub estimated: Option<f64>,
    /// The value the device itself reported, when it reports one. `None` means not
    /// reported, never zero.
    pub device_reported: Option<f64>,
}

/// The device's run phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePhase {
    Idle,
    Loading,
    Armed,
    Running,
    Held,
    Faulted,
}

/// A fault the device reported, decoded by the implementor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFault {
    pub code: u32,
    pub label: String,
}

/// The device's state, distinct from the sample stream.
///
/// Every `Option` field means "not reported" when `None`. A device that does not
/// report its frequency leaves `device_frequency_hz` at `None`; it is never `0.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceLoopState {
    pub phase: DevicePhase,
    pub device_frequency_hz: Option<f64>,
    pub device_level: Option<f64>,
    pub fault: Option<DeviceFault>,
    pub sweep_position: Option<f64>,
    /// The phase text an operator is shown.
    pub narrative: String,
}

impl DeviceLoopState {
    /// Idle with nothing reported.
    pub fn idle() -> Self {
        Self {
            phase: DevicePhase::Idle,
            device_frequency_hz: None,
            device_level: None,
            fault: None,
            sweep_position: None,
            narrative: "idle".to_string(),
        }
    }
}

/// What [`DelegatedControl::start`] reached.
#[derive(Debug, Clone, PartialEq)]
pub struct StartReport {
    pub phase: DevicePhase,
    pub narrative: String,
}

/// One block of device telemetry, interleaved by frame.
///
/// Channel count and sample rate travel with the data. Build it with
/// [`TelemetryChunk::new`], which checks that the samples divide into whole frames.
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryChunk {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub frames: usize,
    /// The device's block sequence number.
    pub seq: u64,
    /// False when the device reported a gap before this block.
    pub contiguous: bool,
    pub rate_hz: f64,
}

impl TelemetryChunk {
    /// A checked chunk: `channels > 0`, `samples.len()` a whole number of frames, and
    /// a finite positive rate. `frames` is derived, so it cannot disagree with the
    /// samples (the frames-versus-samples confusion was a real data-loss bug).
    pub fn new(
        samples: Vec<f32>,
        channels: usize,
        seq: u64,
        contiguous: bool,
        rate_hz: f64,
    ) -> Result<Self, HalError> {
        if channels == 0 {
            return Err(HalError::Config(
                "telemetry chunk with zero channels".into(),
            ));
        }
        if !samples.len().is_multiple_of(channels) {
            return Err(HalError::Config(format!(
                "telemetry chunk of {} samples is not a whole number of {channels}-channel frames",
                samples.len()
            )));
        }
        if !(rate_hz.is_finite() && rate_hz > 0.0) {
            return Err(HalError::Config(format!(
                "telemetry chunk rate {rate_hz} Hz is not a finite positive rate"
            )));
        }
        let frames = samples.len() / channels;
        Ok(Self {
            samples,
            channels,
            frames,
            seq,
            contiguous,
            rate_hz,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_telemetry_chunk_derives_frames_and_refuses_a_partial_frame() {
        let c = TelemetryChunk::new(vec![0.0; 12], 4, 7, true, 5120.0).unwrap();
        assert_eq!((c.channels, c.frames, c.seq), (4, 3, 7));
        assert!(matches!(
            TelemetryChunk::new(vec![0.0; 10], 4, 0, true, 5120.0),
            Err(HalError::Config(_))
        ));
        assert!(matches!(
            TelemetryChunk::new(vec![0.0; 4], 0, 0, true, 5120.0),
            Err(HalError::Config(_))
        ));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                TelemetryChunk::new(vec![0.0; 4], 2, 0, true, bad).is_err(),
                "rate {bad} accepted"
            );
        }
    }

    #[test]
    fn an_idle_state_reports_nothing_and_never_a_zero_frequency() {
        let s = DeviceLoopState::idle();
        assert_eq!(s.phase, DevicePhase::Idle);
        assert_eq!(s.device_frequency_hz, None);
        assert_eq!(s.device_level, None);
        assert!(s.fault.is_none());
    }

    /// A minimal implementor that does not override `hold` / `rearm`: the defaults
    /// refuse by name instead of pretending to hold.
    struct Minimal;
    impl DelegatedControl for Minimal {
        fn caps(&self) -> Caps {
            testing::delegated_caps(&[])
        }
        fn start(&mut self) -> Result<StartReport, HalError> {
            Err(HalError::NotConnected)
        }
        fn level(&mut self, _cmd: LevelCommand) -> Result<LevelAck, HalError> {
            Err(HalError::NotConnected)
        }
        fn recv_telemetry(&mut self, _t: Duration) -> Result<Option<TelemetryChunk>, HalError> {
            Ok(None)
        }
        fn loop_state(&self) -> DeviceLoopState {
            DeviceLoopState::idle()
        }
        fn ramp_down(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    #[test]
    fn hold_and_rearm_default_to_a_named_refusal() {
        let mut m = Minimal;
        assert!(matches!(m.hold(), Err(HalError::Unsupported("hold"))));
        assert!(matches!(m.rearm(), Err(HalError::Unsupported("rearm"))));
    }

    #[test]
    fn the_trait_is_object_safe_and_sendable() {
        fn takes(_: Box<dyn DelegatedControl>) {}
        takes(Box::new(Minimal));
        takes(Box::new(testing::ScriptedDelegate::new(
            testing::delegated_caps(&[]),
        )));
    }
}
