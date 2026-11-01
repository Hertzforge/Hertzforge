//! [`ScriptedDelegate`]: a [`DelegatedControl`] with no device behind it, for tests
//! that drive a delegate port's routing.
//!
//! Compiled only under `cfg(test)` or the `test-support` feature. Never constructible
//! in production: the sim-leak lint refuses production code that names this type and
//! any production `[dependencies]` entry that enables `hertzforge-hal/test-support`.
//!
//! It is a lifecycle state machine and a call log, nothing more. It invents no device
//! behavior: a level command moves the estimate only by the step a test scripted, and
//! the device reports a value only when a test sets one.

use std::collections::VecDeque;
use std::time::Duration;

use super::{
    Axis, DelegatedControl, DeviceFault, DeviceLoopState, DevicePhase, Direction, LevelAck,
    LevelCommand, StartReport, TelemetryChunk,
};
use crate::{Caps, ClockDomain, HalError, LoopOwner, NativeMode};

/// A device-owned-loop `Caps` declaring exactly `native`, for test doubles.
pub fn delegated_caps(native: &[NativeMode]) -> Caps {
    Caps {
        loop_owner: LoopOwner::DeviceClosesLoop,
        native_modes: native.to_vec(),
        clock: ClockDomain::Unknown,
        max_input_channels: 4,
        max_output_channels: 1,
        abort_latency_bound: None,
        output_buffer: None,
        sample_rate_hz: 1000.0,
        delegated_response_observable: false,
        clock_groups: Vec::new(),
    }
}

/// One call the double received, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum DelegatedCall {
    Start,
    Level(LevelCommand),
    RecvTelemetry,
    RampDown,
    Hold,
    Rearm,
    EmergencyStop,
}

/// The scripted double. See the module docs.
pub struct ScriptedDelegate {
    caps: Caps,
    state: DeviceLoopState,
    telemetry: VecDeque<TelemetryChunk>,
    calls: Vec<DelegatedCall>,
    fail_next: Option<String>,
    /// (fine, coarse) step per nudge on the Level axis, in the estimate's unit.
    level_step: Option<(f64, f64)>,
    estimated_level: Option<f64>,
}

impl ScriptedDelegate {
    pub fn new(caps: Caps) -> Self {
        Self {
            caps,
            state: DeviceLoopState::idle(),
            telemetry: VecDeque::new(),
            calls: Vec::new(),
            fail_next: None,
            level_step: None,
            estimated_level: None,
        }
    }

    /// Script the Level-axis nudge step, starting from `start` (fine, coarse).
    pub fn with_level_nudge(mut self, start: f64, fine: f64, coarse: f64) -> Self {
        self.level_step = Some((fine, coarse));
        self.estimated_level = Some(start);
        self
    }

    /// Queue a telemetry block for the next `recv_telemetry`.
    pub fn push_telemetry(&mut self, chunk: TelemetryChunk) {
        self.telemetry.push_back(chunk);
    }

    /// The next lifecycle call (start, level, ramp_down, hold, rearm) fails with
    /// `HalError::Io(msg)` and leaves the device Faulted. `emergency_stop` is not
    /// affected: it is the way out of a fault.
    pub fn fail_next(&mut self, msg: &str) {
        self.fail_next = Some(msg.to_string());
    }

    /// Script what the device reports for its level and frequency.
    pub fn set_device_report(&mut self, level: Option<f64>, frequency_hz: Option<f64>) {
        self.state.device_level = level;
        self.state.device_frequency_hz = frequency_hz;
    }

    /// Every call received, in order.
    pub fn calls(&self) -> &[DelegatedCall] {
        &self.calls
    }

    fn take_failure(&mut self) -> Result<(), HalError> {
        if let Some(msg) = self.fail_next.take() {
            self.state.phase = DevicePhase::Faulted;
            self.state.fault = Some(DeviceFault {
                code: 0,
                label: msg.clone(),
            });
            self.state.narrative = format!("faulted: {msg}");
            return Err(HalError::Io(msg));
        }
        Ok(())
    }

    fn enter(&mut self, phase: DevicePhase, narrative: &str) {
        self.state.phase = phase;
        self.state.narrative = narrative.to_string();
    }
}

impl DelegatedControl for ScriptedDelegate {
    fn caps(&self) -> Caps {
        self.caps.clone()
    }

    fn start(&mut self) -> Result<StartReport, HalError> {
        self.calls.push(DelegatedCall::Start);
        if self.state.phase != DevicePhase::Idle {
            return Err(HalError::NotConnected);
        }
        self.take_failure()?;
        self.enter(DevicePhase::Running, "running");
        Ok(StartReport {
            phase: DevicePhase::Running,
            narrative: self.state.narrative.clone(),
        })
    }

    fn level(&mut self, cmd: LevelCommand) -> Result<LevelAck, HalError> {
        self.calls.push(DelegatedCall::Level(cmd));
        if self.state.phase != DevicePhase::Running {
            return Err(HalError::NotConnected);
        }
        self.take_failure()?;
        let (axis, sent) = match cmd {
            LevelCommand::Nudge {
                axis,
                direction,
                coarse,
                count,
            } => {
                if axis == Axis::Level {
                    if let (Some((fine, big)), Some(est)) = (self.level_step, self.estimated_level)
                    {
                        let step = if coarse { big } else { fine };
                        let sign = if direction == Direction::Up {
                            1.0
                        } else {
                            -1.0
                        };
                        self.estimated_level = Some(est + sign * step * f64::from(count));
                    }
                }
                (axis, count)
            }
            LevelCommand::Setpoint { axis, value } => {
                if axis == Axis::Level {
                    self.estimated_level = Some(value);
                }
                (axis, 1)
            }
            LevelCommand::StepTo { axis, target_db } => {
                if axis == Axis::Level {
                    self.estimated_level = Some(target_db);
                }
                (axis, 1)
            }
        };
        let (estimated, device_reported) = match axis {
            Axis::Level => (self.estimated_level, self.state.device_level),
            Axis::Frequency => (None, self.state.device_frequency_hz),
        };
        Ok(LevelAck {
            sent,
            estimated,
            device_reported,
        })
    }

    fn recv_telemetry(&mut self, _timeout: Duration) -> Result<Option<TelemetryChunk>, HalError> {
        self.calls.push(DelegatedCall::RecvTelemetry);
        match self.state.phase {
            DevicePhase::Running | DevicePhase::Held => Ok(self.telemetry.pop_front()),
            _ => Err(HalError::NotConnected),
        }
    }

    fn loop_state(&self) -> DeviceLoopState {
        self.state.clone()
    }

    fn ramp_down(&mut self) -> Result<(), HalError> {
        self.calls.push(DelegatedCall::RampDown);
        match self.state.phase {
            DevicePhase::Idle => Ok(()),
            DevicePhase::Running | DevicePhase::Held => {
                self.take_failure()?;
                self.enter(DevicePhase::Idle, "idle");
                Ok(())
            }
            _ => Err(HalError::NotConnected),
        }
    }

    fn hold(&mut self) -> Result<(), HalError> {
        self.calls.push(DelegatedCall::Hold);
        if self.state.phase != DevicePhase::Running {
            return Err(HalError::NotConnected);
        }
        self.take_failure()?;
        self.enter(DevicePhase::Held, "held, loaded");
        Ok(())
    }

    fn rearm(&mut self) -> Result<(), HalError> {
        self.calls.push(DelegatedCall::Rearm);
        if self.state.phase != DevicePhase::Held {
            return Err(HalError::NotConnected);
        }
        self.take_failure()?;
        self.enter(DevicePhase::Running, "running");
        Ok(())
    }

    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.calls.push(DelegatedCall::EmergencyStop);
        self.state.fault = None;
        self.enter(DevicePhase::Idle, "stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> ScriptedDelegate {
        let mut d = ScriptedDelegate::new(delegated_caps(&[NativeMode::Random]))
            .with_level_nudge(-20.0, 1.0, 10.0);
        d.start().expect("idle -> running");
        d
    }

    #[test]
    fn the_double_walks_the_lifecycle_and_logs_every_call() {
        let mut d = running();
        assert_eq!(d.loop_state().phase, DevicePhase::Running);
        d.hold().unwrap();
        assert_eq!(d.loop_state().phase, DevicePhase::Held);
        assert!(
            matches!(d.hold(), Err(HalError::NotConnected)),
            "hold from held"
        );
        d.rearm().unwrap();
        d.ramp_down().unwrap();
        assert_eq!(d.loop_state().phase, DevicePhase::Idle);
        assert!(
            matches!(
                d.level(LevelCommand::Setpoint {
                    axis: Axis::Level,
                    value: 0.5
                }),
                Err(HalError::NotConnected)
            ),
            "a level command after the ramp-down is refused"
        );
        assert_eq!(
            d.calls(),
            &[
                DelegatedCall::Start,
                DelegatedCall::Hold,
                DelegatedCall::Hold,
                DelegatedCall::Rearm,
                DelegatedCall::RampDown,
                DelegatedCall::Level(LevelCommand::Setpoint {
                    axis: Axis::Level,
                    value: 0.5
                }),
            ]
        );
    }

    #[test]
    fn a_nudge_moves_the_estimate_only_by_the_scripted_step() {
        let mut d = running();
        let ack = d
            .level(LevelCommand::Nudge {
                axis: Axis::Level,
                direction: Direction::Up,
                coarse: false,
                count: 3,
            })
            .unwrap();
        assert_eq!(ack.sent, 3);
        assert_eq!(ack.estimated, Some(-17.0));
        assert_eq!(ack.device_reported, None, "the device reported nothing");
        d.set_device_report(Some(-17.2), None);
        let ack = d
            .level(LevelCommand::Nudge {
                axis: Axis::Level,
                direction: Direction::Down,
                coarse: true,
                count: 1,
            })
            .unwrap();
        assert_eq!(ack.estimated, Some(-27.0));
        assert_eq!(ack.device_reported, Some(-17.2));
        let f = d
            .level(LevelCommand::Nudge {
                axis: Axis::Frequency,
                direction: Direction::Up,
                coarse: false,
                count: 2,
            })
            .unwrap();
        assert_eq!(f.estimated, None, "the double invents no frequency model");
        assert_eq!(f.device_reported, None);

        // Unscripted: no step, no estimate.
        let mut bare = ScriptedDelegate::new(delegated_caps(&[]));
        bare.start().unwrap();
        let ack = bare
            .level(LevelCommand::Nudge {
                axis: Axis::Level,
                direction: Direction::Up,
                coarse: false,
                count: 1,
            })
            .unwrap();
        assert_eq!(ack.estimated, None);
    }

    #[test]
    fn telemetry_is_served_while_running_and_refused_when_idle() {
        let mut d = ScriptedDelegate::new(delegated_caps(&[]));
        assert!(matches!(
            d.recv_telemetry(Duration::ZERO),
            Err(HalError::NotConnected)
        ));
        d.start().unwrap();
        assert_eq!(d.recv_telemetry(Duration::ZERO).unwrap(), None);
        let chunk = TelemetryChunk::new(vec![1.0, 2.0], 2, 1, true, 1000.0).unwrap();
        d.push_telemetry(chunk.clone());
        assert_eq!(d.recv_telemetry(Duration::ZERO).unwrap(), Some(chunk));
    }

    #[test]
    fn a_scripted_fault_faults_the_device_and_only_the_emergency_stop_clears_it() {
        let mut d = running();
        d.fail_next("link dropped");
        assert!(matches!(
            d.level(LevelCommand::StepTo {
                axis: Axis::Level,
                target_db: -10.0
            }),
            Err(HalError::Io(_))
        ));
        let s = d.loop_state();
        assert_eq!(s.phase, DevicePhase::Faulted);
        assert_eq!(
            s.fault.as_ref().map(|f| f.label.as_str()),
            Some("link dropped")
        );
        assert!(matches!(d.ramp_down(), Err(HalError::NotConnected)));
        assert!(matches!(d.start(), Err(HalError::NotConnected)));
        d.emergency_stop().unwrap();
        let s = d.loop_state();
        assert_eq!(s.phase, DevicePhase::Idle);
        assert!(s.fault.is_none());
        d.start().expect("restartable after the stop");
    }

    #[test]
    fn the_caps_are_the_claim_it_was_built_with() {
        let d = ScriptedDelegate::new(delegated_caps(&[NativeMode::Random]));
        let c = d.caps();
        assert_eq!(c.loop_owner, LoopOwner::DeviceClosesLoop);
        assert_eq!(c.native_modes, vec![NativeMode::Random]);
        assert!(!c.delegated_response_observable);
    }
}
