//! SRS's arm path CONSULTS the shared arm gate
//! (`hertzforge_module_abi::arm_gate`).
//!
//! One of six sibling tests (a `tests/arm_gate_consulted.rs` in each module
//! crate) that together answer the predicate "every arm consults the
//! resolver". Per-crate rather than one cross-module file, because the
//! falsifying mutation is per-module: bypass the helper in ONE module's arm
//! path and exactly that module's test reds, which is what makes the suite tell
//! you WHERE the gate went missing.
//!
//! RED-MUTATION for this file: delete the `hertzforge_module_abi::arm_gate(...)`
//! call from `HalPulsePlant::ensure_open` (in `src/service/plant.rs`) and the
//! refusal test below reds - the plant would open against a controller that
//! generates its own drive and only discover it on the first `write_drive`.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use hertzforge_srs::{HalPulsePlant, PulsePlant};

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
        acq: &AcquisitionConfig,
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

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    t.channels.push(drv);
    t
}

#[test]
fn srs_arm_refuses_a_device_closes_loop_backend_with_the_gates_typed_reason() {
    let mut plant = HalPulsePlant::new(
        DeviceClosesLoopBackend,
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("the table is valid; the refusal must come from the gate, not the table");
    let text = plant
        .ensure_open()
        .expect_err("SRS synthesizes host-side wavelet drive; this backend has no host drive path");
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

/// The other half: a refusal that could not have been an acceptance proves
/// nothing, so the same plant against a host-loop backend must open.
///
/// Note the SRS-specific fact this also pins: SRS asks for `HostBetweenRuns`
/// correction, which does NOT require a proven shared clock (each shot is
/// independent - there is no COLA continuity to preserve). The plant below
/// declares an UNPROVEN clock deliberately, and must still open.
#[test]
fn srs_arm_succeeds_against_a_host_loop_plant_even_on_an_unproven_clock() {
    let backend = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(8192.0)
        .shared_clock(false)
        .build();
    let mut plant = HalPulsePlant::new(
        backend,
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .expect("valid table");
    plant
        .ensure_open()
        .expect("HostBetweenRuns carries no clock precondition");
}
