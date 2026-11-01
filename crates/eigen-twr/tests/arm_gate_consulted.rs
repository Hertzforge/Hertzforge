//! TWR's arm path CONSULTS the shared arm gate (`eigen_module_abi::arm_gate`).
//!
//! One of six sibling tests (a `tests/arm_gate_consulted.rs` in each module
//! crate) that together answer the predicate "every arm consults the
//! resolver". Per-crate rather than one cross-module file, because the
//! falsifying mutation is per-module: bypass the helper in ONE module's arm
//! path and exactly that module's test reds, which is what makes the suite tell
//! you WHERE the gate went missing.
//!
//! RED-MUTATION for this file: delete the `eigen_module_abi::arm_gate(...)` call
//! from `BackendReferencePlayer::arm` (in `src/player.rs`) and both tests below
//! red - the first because the player would happily arm a controller that
//! generates its own drive, the second because the refusal text would no longer
//! be the gate's.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable};
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use eigen_twr::player::ReferencePlayer;
use eigen_twr::BackendReferencePlayer;

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
fn twr_arm_refuses_a_device_closes_loop_backend_with_the_gates_typed_reason() {
    let mut player = BackendReferencePlayer::new(
        DeviceClosesLoopBackend,
        table(),
        eigen_module_abi::StationClass::EigenSim,
    )
    .expect("the table is valid; the refusal must come from the gate, not the table");
    let err = player
        .arm()
        .expect_err("TWR streams host-synthesized drive; this backend has no host drive path");
    let text = err.to_string();

    // The gate's `NoHostDrivePath` text, reaching the service error verbatim
    // (surfacing it as a UI panel is later work; today it must at least survive).
    assert!(
        text.contains("stream drive samples"),
        "the refusal must be the gate's, not a generic backend error: {text}"
    );
    assert!(
        text.contains("generates its own drive"),
        "the refusal must say WHY: {text}"
    );
    assert!(
        text.contains("arm refused"),
        "the arm path must label it as a refusal: {text}"
    );
}

/// A backend that COERCES the requested rate, like the 9234 does (8192 asked,
/// 8533.33 delivered up its 13.1072 MHz/(256n) ladder).
struct CoercingBackend;

/// What `caps()` advertises and what `configure()` answers - deliberately
/// different, because that difference is the only thing the rate gate can catch.
const ASKED_HZ: f64 = 8192.0;
const COERCED_HZ: f64 = 8533.33;

impl Backend for CoercingBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: ASKED_HZ,
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
            actual_rate_hz: COERCED_HZ,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        panic!("arm() must never be reached: the rate gate refuses first")
    }
    fn start(&mut self) -> Result<(), HalError> {
        panic!("start() must never be reached: the rate gate refuses first")
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
        Ok(InputRead::NoDataYet)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        Ok(d.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// THE SERVICE states the reference rate, always - so the gate is never
/// vacuous on a real run.
///
/// `arm_gate` skips its rate check entirely when the profile rate is `None`,
/// and the only callers of `with_reference_rate_hz` in the whole tree were two
/// unit tests. On the chassis that made the rate arm green by construction:
/// the sweep built the player without stating a rate, the player then REQUESTED
/// the backend's own rate and compared the device to itself, and a reference at
/// a rate the 9234 will not program would have played at the wrong speed with
/// every frequency shifted and nothing refusing.
///
/// RED-MUTATION: delete `player.set_reference_rate_hz(r.fs_hz)` from
/// `TwrService::new` (or make the player's impl a no-op) and this test reds -
/// the run arms happily against a device programmed 341 Hz away from the record
/// it is about to play.
#[test]
fn the_service_always_states_the_reference_rate_so_the_gate_can_red() {
    let reference =
        eigen_twr::ReferenceWaveform::from_samples("t", ASKED_HZ, vec![0.1; 8192]).unwrap();
    let mut cfg = eigen_twr::TwrConfig::for_reference(reference);
    cfg.sysid.duration_s = 1.0;
    cfg.sysid.nfft = 512;

    let player = BackendReferencePlayer::new(
        CoercingBackend,
        table(),
        eigen_module_abi::StationClass::EigenSim,
    )
    .expect("valid table");
    let mut svc = eigen_twr::TwrService::new(player, cfg).expect("config is valid at 8192 Hz");

    let text = svc
        .advance()
        .expect_err("the device programs a rate the prepared record is not at")
        .to_string();
    assert!(text.contains("arm refused"), "{text}");
    assert!(
        text.contains("8192"),
        "must quote the record's rate: {text}"
    );
    assert!(
        text.contains("8533"),
        "must quote the device's rate: {text}"
    );
}

/// The other half. A refusal that could not have been an acceptance proves
/// nothing about the gate, so the same player against a host-loop plant must
/// arm.
#[test]
fn twr_arm_succeeds_against_a_host_loop_plant() {
    let backend = eigen_hal_sim::SimConfig::new()
        .sample_rate_hz(8192.0)
        .shared_clock(true)
        .build();
    let mut player =
        BackendReferencePlayer::new(backend, table(), eigen_module_abi::StationClass::EigenSim)
            .expect("valid table");
    player.arm().expect("a host-loop plant satisfies the gate");
}

/// The rate policy reaching a real module: a reference record generated at a
/// rate the hardware is not configured for is REFUSED, with the regeneration
/// offer, rather than played at the wrong speed.
///
/// RED-MUTATION: pass `None` instead of `self.reference_rate_hz` in
/// `BackendReferencePlayer::arm` and this test reds.
#[test]
fn twr_arm_refuses_a_reference_record_generated_at_a_different_rate() {
    let backend = eigen_hal_sim::SimConfig::new()
        .sample_rate_hz(8192.0)
        .shared_clock(true)
        .build();
    // 8200 Hz vs 8192 Hz: ~0.1% apart, inside the regeneration band.
    let mut player =
        BackendReferencePlayer::new(backend, table(), eigen_module_abi::StationClass::EigenSim)
            .expect("valid table")
            .with_reference_rate_hz(8200.0);
    let text = player.arm().expect_err("rate mismatch").to_string();
    assert!(text.contains("8200"), "{text}");
    assert!(text.contains("8192"), "{text}");
    assert!(
        text.contains("Regenerate the profile"),
        "the <=1% band must OFFER regeneration: {text}"
    );
    assert!(
        text.contains("does not resample"),
        "and must say Eigen will not resample: {text}"
    );

    // ...and the exact same record at the configured rate arms.
    let backend = eigen_hal_sim::SimConfig::new()
        .sample_rate_hz(8192.0)
        .shared_clock(true)
        .build();
    let mut ok =
        BackendReferencePlayer::new(backend, table(), eigen_module_abi::StationClass::EigenSim)
            .expect("valid table")
            .with_reference_rate_hz(8192.0);
    ok.arm().expect("matching rates arm");
}
