//! THE ARM-GATE CONSULT TEST.
//!
//! # What this proves, and what it deliberately refuses to prove
//!
//! It proves that `ExplorerHostService::arm` really calls
//! [`eigen_module_abi::arm_gate`], with the arguments the design specifies, on a station
//! where the gate can REFUSE. Every lever below is a REAL falsifying one: each mock
//! makes exactly one of the gate's three checks fail, and the corresponding refusal is
//! asserted BY VARIANT, not by substring.
//!
//! What it refuses to do is assert the happy path on EIGEN-SIM and call that evidence.
//! A Sim station exempts itself from the abort-latency rule and reports a host loop
//! owner and an exact rate, so the gate cannot fail there - and a result that cannot
//! fail is not evidence. That is why each test below has to construct a
//! backend whose caps make one specific check fail.
//!
//! # The red mutations this file pins
//!
//! 1. `Resolution::host_open_loop()` -> anything with a `DeviceReplay` drive: the
//!    `NoHostDrivePath` test stops discriminating.
//! 2. Deleting the `arm_gate` call entirely: all three tests red.
//! 3. **`Some(self.fs_hz)` -> `None`** (the named mutation):
//!    `an_unusable_reported_rate_is_refused` reds, because the rate policy is the only
//!    layer that owns a zero/NaN configured rate and `None` skips it.
//!
//! Mutation 3 was RUN TWICE, 2026-08-08, and the two runs are worth recording because
//! the first one found a defect rather than merely confirming the test.
//!
//! * **First run.** With `None` in place of `Some(self.fs_hz)`, arm() got PAST the gate
//!   and PANICKED three crates down, inside `SweepOscillator::new`'s `fs_hz must be
//!   positive` assertion - the module's construction taken out by a device answering 0.
//!   That is the wire-fed-hazard shape (a hazardous path reachable from a wire-fed value),
//!   so `ExplorerProcessStep::new` was made to REFUSE
//!   ([`eigen_explorer_host::step::StepBuildError`]) as defence in depth.
//! * **Second run**, after that fix: `an_unusable_reported_rate_is_refused` reds cleanly
//!   with `expected a RateMismatch for a reported rate of 0, got Config(...)` - the
//!   refusal now comes from the step-build backstop instead of the gate, which is
//!   exactly the discrimination this test is for. The other three tests still pass (they
//!   exercise different checks).
//!
//! `Some(self.fs_hz)` was then restored, and both layers are live.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_explorer_host::service::{ExplorerHostConfig, ExplorerHostService, ServiceError};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_module_abi::{StationClass, UnsupportedReason};

/// A minimal backend whose CAPS and reported rate are the test's levers. It never
/// acquires anything: `arm()` refuses before a single frame is read in every case here.
struct CapsMock {
    caps: Caps,
    reported_rate_hz: f64,
}

impl CapsMock {
    /// Caps that PASS every gate check, as the baseline each test then breaks one field
    /// of. A host-loop backend, a proven-shared clock (irrelevant to an open-loop
    /// request, present so the tests differ in exactly one thing), and a characterized
    /// abort-latency bound.
    fn good_caps() -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 8,
            max_output_channels: 2,
            abort_latency_bound: Some(std::time::Duration::from_millis(5)),
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: 8_192.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }

    fn new(caps: Caps, reported_rate_hz: f64) -> Self {
        Self {
            caps,
            reported_rate_hz,
        }
    }
}

impl Backend for CapsMock {
    fn caps(&self) -> Caps {
        self.caps.clone()
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: self.reported_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
        Ok(InputRead::NoDataYet)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// One control input + one drive row - the same shape the chassis gate uses.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 10.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

fn cfg() -> ExplorerHostConfig {
    ExplorerHostConfig {
        fs_hz: 8_192.0,
        band_hz: (20.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 1.0,
        ..Default::default()
    }
}

fn arm_against(caps: Caps, rate: f64, station: StationClass) -> Result<(), ServiceError> {
    let backend = Box::new(CapsMock::new(caps, rate));
    let mut svc = ExplorerHostService::new(cfg(), table(), backend, station)
        .expect("the config and table are valid; the REFUSAL under test is the arm gate's");
    svc.arm()
}

/// LEVER 1 - the loop-resolution check. A backend that closes its own loop has no host
/// drive path, so `{HostSynthesized, None}` cannot be honored. This is the check that
/// would silently pass if the request were ever changed to a delegated one.
#[test]
fn a_device_closes_loop_backend_is_refused_with_no_host_drive_path() {
    let mut caps = CapsMock::good_caps();
    caps.loop_owner = LoopOwner::DeviceClosesLoop;
    let err = arm_against(caps, 8_192.0, StationClass::NiReal)
        .expect_err("arm() must refuse a device-closes-loop backend");
    assert!(
        matches!(
            err,
            ServiceError::ArmRefused(UnsupportedReason::NoHostDrivePath { .. })
        ),
        "expected NoHostDrivePath, got {err:?}"
    );
}

/// LEVER 2 - the abort-latency rule. A HARDWARE station that DRIVES an output needs an
/// abort-latency bound to exist. `EigenSim` is exempt, which is precisely why this test
/// has to name a hardware class: run it on `EigenSim` and it cannot fail.
#[test]
fn a_hardware_station_with_no_abort_latency_bound_is_refused() {
    let mut caps = CapsMock::good_caps();
    caps.abort_latency_bound = None;
    let err = arm_against(caps, 8_192.0, StationClass::NiSimulated)
        .expect_err("arm() must refuse a hardware station with no abort-latency bound");
    assert!(
        matches!(
            err,
            ServiceError::ArmRefused(UnsupportedReason::NoAbortLatencyBound {
                station: StationClass::NiSimulated
            })
        ),
        "expected NoAbortLatencyBound for NI-SIM, got {err:?}"
    );

    // The CONTROL for lever 2: the same caps on EIGEN-SIM arm fine, which is what makes
    // the refusal above a statement about hardware rather than about these caps.
    let mut caps = CapsMock::good_caps();
    caps.abort_latency_bound = None;
    arm_against(caps, 8_192.0, StationClass::EigenSim)
        .expect("EIGEN-SIM is exempt from the abort-latency rule - there is nothing to abort");
}

/// LEVER 3 - THE `Some(fs)` MUTATION TARGET. A backend that reports a rate nothing can
/// be denominated in must not arm.
///
/// This is the case `adopt_actual` deliberately does NOT claim, so that exactly one
/// layer owns it and passing `Some(self.fs_hz)` to the gate is a check that can actually
/// fail. Replace it with `None` and the rate policy is skipped, the service arms against
/// a station running at "0 Hz", and every derived quantity downstream - the slew
/// increments, the display bin spacing, the settle windows - is denominated in a
/// fiction.
#[test]
fn an_unusable_reported_rate_is_refused() {
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let err = arm_against(CapsMock::good_caps(), rate, StationClass::NiReal)
            .err()
            .unwrap_or_else(|| panic!("arm() must refuse a station that programmed {rate} Hz"));
        assert!(
            matches!(
                err,
                ServiceError::ArmRefused(UnsupportedReason::RateMismatch { .. })
            ),
            "expected a RateMismatch for a reported rate of {rate}, got {err:?}"
        );
    }

    // The CONTROL: the same path with a USABLE rate arms. Without this row the test
    // above could pass because arm() refuses everything, which would prove nothing
    // about the rate check at all.
    arm_against(CapsMock::good_caps(), 8_192.0, StationClass::NiReal)
        .expect("a usable rate on otherwise-good caps must arm");
}

/// The gate is consulted BEFORE the device is armed and started.
///
/// Ordering matters as much as presence: a gate consulted after `backend.arm()` /
/// `backend.start()` is a gate that refuses a run which is already clocked. The mock
/// records the lifecycle calls it received, and a refused arm must show NONE of them.
#[test]
fn a_refused_arm_never_reached_the_device_lifecycle() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct RecordingMock {
        inner: CapsMock,
        armed: Arc<AtomicU32>,
        started: Arc<AtomicU32>,
    }
    impl Backend for RecordingMock {
        fn caps(&self) -> Caps {
            self.inner.caps()
        }
        fn open(&mut self) -> Result<(), HalError> {
            self.inner.open()
        }
        fn close(&mut self) -> Result<(), HalError> {
            self.inner.close()
        }
        fn configure(
            &mut self,
            t: &ChannelTable,
            a: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            self.inner.configure(t, a)
        }
        fn arm(&mut self) -> Result<(), HalError> {
            self.armed.fetch_add(1, Ordering::Relaxed);
            self.inner.arm()
        }
        fn start(&mut self) -> Result<(), HalError> {
            self.started.fetch_add(1, Ordering::Relaxed);
            self.inner.start()
        }
        fn stop(&mut self) -> Result<(), HalError> {
            self.inner.stop()
        }
        fn read_input(&mut self, o: &mut [f32]) -> Result<InputRead, HalError> {
            self.inner.read_input(o)
        }
        fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
            self.inner.write_drive(d)
        }
        fn poll_status(&mut self) -> BackendStatus {
            self.inner.poll_status()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            self.inner.emergency_stop()
        }
    }

    let armed = Arc::new(AtomicU32::new(0));
    let started = Arc::new(AtomicU32::new(0));
    let mut caps = CapsMock::good_caps();
    caps.abort_latency_bound = None;
    let backend = Box::new(RecordingMock {
        inner: CapsMock::new(caps, 8_192.0),
        armed: armed.clone(),
        started: started.clone(),
    });
    let mut svc = ExplorerHostService::new(cfg(), table(), backend, StationClass::NiReal).unwrap();
    assert!(svc.arm().is_err());
    assert_eq!(
        armed.load(Ordering::Relaxed),
        0,
        "the device was ARMED behind a refused gate"
    );
    assert_eq!(
        started.load(Ordering::Relaxed),
        0,
        "the device CLOCK was started behind a refused gate"
    );
}
