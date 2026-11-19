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
//! refusal test below reds.
//!
//! The Recorder asks the gate for a capture-only run (`{None, None}`), which every
//! loop owner supports, so a device-loop backend is no longer a refusal it can make.
//! The refusal it still makes for a capture-only request is the rate policy's: the
//! gate is given the rate the station programmed, and a rate that does not exist is
//! refused before anything is derived from it. A gate whose refusal cannot be
//! produced is not evidence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Interface, Units};
use hertzforge_hal::{
    Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError, InputRead, LoopOwner,
};
use hertzforge_recorder::service::{RecorderConfig, RecorderService};
use hertzforge_recorder::trigger::TriggerMode;

/// A station that programs a rate that does not exist: whatever it is asked, it
/// answers 0 Hz. Nothing else about it matters - the gate refuses on the programmed
/// rate, before a single lifecycle call past `configure` reaches the device.
struct NoRateBackend;

impl Backend for NoRateBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
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
            actual_rate_hz: 0.0,
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
        Err(HalError::Unsupported("this station has no output task"))
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
fn recorder_arm_refuses_a_station_that_programs_no_rate_with_the_gates_typed_reason() {
    let mut svc = RecorderService::with_backend(
        cfg(),
        input_table(2),
        NoRateBackend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("refuse"),
    )
    .expect("the config and table are valid; the refusal must come from the gate");
    svc.configure().expect("configure is a state transition");
    let text = svc
        .start_monitor()
        .expect_err("the station programmed 0 Hz; nothing can be recorded at it")
        .to_string();

    assert!(
        text.contains("configured at 0 Hz"),
        "the refusal must be the gate's rate policy, not a generic backend error: {text}"
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
