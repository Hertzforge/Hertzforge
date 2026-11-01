//! The Recorder's arm path CONSULTS the shared arm gate
//! (`hertzforge_module_abi::arm_gate`).
//!
//! One of six sibling tests (a `tests/arm_gate_consulted.rs` in each module
//! crate) that together answer the predicate "every arm consults the
//! resolver". Per-crate rather than one cross-module file, because the
//! falsifying mutation is per-module: bypass the helper in ONE module's arm
//! path and exactly that module's test reds, which is what makes the suite tell
//! you WHERE the gate went missing.
//!
//! RED-MUTATION for this file: delete the `hertzforge_module_abi::arm_gate(...)` call
//! from `RecorderService::bring_backend_up` (in `src/service/mod.rs`) and the
//! refusal test below reds. So does changing the request back to
//! `Resolution::capture_only()` - see the note there: this Recorder DOES have a
//! drive path (the injected synthetic drive channel, fed zeros by
//! `SilentDrive`), so `{None, None}` would let it arm against a device-loop
//! backend and then fault on tick 1 when `write_drive` came back `Unsupported`.
//! A gate whose refusal cannot be produced is not evidence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Interface, Units};
use hertzforge_hal::{
    Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError, InputRead, LoopOwner,
};
use hertzforge_recorder::service::{RecorderConfig, RecorderService};
use hertzforge_recorder::trigger::TriggerMode;

/// A `DeviceClosesLoop` mock backend: it generates its own drive and exposes no
/// per-sample host output path, exactly like a delegated controller. Nothing else about it
/// matters - the arm gate refuses on `caps()` alone, before a single lifecycle
/// call past `configure` reaches the device.
///
/// Deliberately synthetic rather than a real delegated backend: the
/// point is to prove THIS module's arm path consults the gate, not to depend on
/// what a particular controller happens to prove today. (`native_modes` is an
/// evidence claim; a test that leaned on it would tempt someone to widen it.)
struct DeviceClosesLoopBackend;

impl Backend for DeviceClosesLoopBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::DeviceClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Unknown,
            max_input_channels: 8,
            max_output_channels: 0,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 65_536.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        panic!("arm() must never be reached: the gate refuses this backend first")
    }
    fn start(&mut self) -> Result<(), HalError> {
        panic!("start() must never be reached: the gate refuses this backend first")
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
        Ok(InputRead::NoDataYet)
    }
    fn write_drive(&mut self, _d: &[f32]) -> Result<usize, HalError> {
        Err(HalError::Unsupported("the device generates drive"))
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

fn temp_root(tag: &str) -> PathBuf {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "hertzforge_recorder_gate_{tag}_{}_{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn input_table(n: u32) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 1..=n {
        let mut c = Channel::new(i);
        c.role = ChannelRole::Monitor;
        c.enabled = true;
        c.units = Units::AccelG;
        c.interface = Interface::SingleEnded;
        c.range_max = 100.0;
        t.channels.push(c);
    }
    t
}

fn cfg() -> RecorderConfig {
    RecorderConfig {
        acq: hertzforge_recorder::AcquisitionConfig {
            native_rate_hz: 8_192.0,
            block_size: 256,
            analysis_decimation: 1,
            ..Default::default()
        },
        trigger_mode: TriggerMode::Manual,
        ..Default::default()
    }
}

#[test]
fn recorder_arm_refuses_a_device_closes_loop_backend_with_the_gates_typed_reason() {
    let mut svc = RecorderService::with_backend(
        cfg(),
        input_table(2),
        DeviceClosesLoopBackend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("refuse"),
    )
    .expect("the config and table are valid; the refusal must come from the gate");
    svc.configure().expect("configure is a state transition");
    let text = svc
        .start_monitor()
        .expect_err(
            "the Recorder's engine writes a synthetic drive every tick; this backend has no \
             host drive path",
        )
        .to_string();

    assert!(
        text.contains("stream drive samples"),
        "the refusal must be the gate's, not a generic backend error: {text}"
    );
    assert!(
        text.contains("generates its own drive"),
        "the refusal must say WHY: {text}"
    );
    assert!(text.contains("arm refused"), "{text}");
}

/// The other half. A refusal that could not have been an acceptance proves
/// nothing about the gate, so the same service on the Sim plant must come up.
#[test]
fn recorder_arm_succeeds_against_a_host_loop_plant() {
    let mut svc = RecorderService::with_backend(
        cfg(),
        input_table(2),
        Box::new(
            hertzforge_hal_sim::SimConfig::new()
                .max_input_channels(2)
                .build(),
        ) as Box<dyn Backend>,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("accept"),
    )
    .expect("valid config");
    svc.configure().expect("configure");
    svc.start_monitor()
        .expect("a host-loop plant satisfies the gate");
}
