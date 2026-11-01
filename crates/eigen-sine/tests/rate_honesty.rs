//! The run is denominated in the rate the DEVICE programmed, and a coercion
//! is VISIBLE.
//!
//! `SineService::sample_rate_hz` used to be read once from `caps()` before
//! anything was configured, and ~8 arm-time consumers derived from it: the
//! sysID FFT sizing, the burst length, the FRF bin spacing, the coherence band
//! indices, the step's `fs_hz`, the settle window, the capture rate, the
//! control-tick bound. On a device that coerces, every one of them was
//! denominated in a rate the hardware never ran at. Random shipped exactly this
//! and put every profile frequency 6.25x high on a shaker.
//!
//! The mock here is the shape of the real defect: the 9234 rides a
//! 13.1072 MHz/(256n) ladder, so 8192 Hz is programmed as 8533.33 Hz. Nothing on
//! the bench can produce that on demand (51 200 is exactly on the ladder and
//! comes back uncoerced), which is why the evidence is a mock and not a chassis
//! row - a gate that can only ask for uncoerced rates cannot falsify the warning.
//!
//! RED-MUTATIONS, each flipped and observed once:
//!
//! * Delete the `self.sample_rate_hz = fs;` adoption in `adopt_configured_rate`
//!   (`src/service/mod.rs`): `the_adopted_rate_is_what_the_device_programmed`
//!   AND `a_control_tick_inside_the_batch_budget_arms` red (7 of 9 stay green).
//!   **Observed red 2026-08-08** - the run stayed denominated in 8192 Hz while
//!   the device clocked 8533.33, and the control-tick bound was then evaluated
//!   against the wrong rate too, which is the same defect showing up twice.
//! * Delete the `EventKind::RateCoerced` push:
//!   `a_coerced_rate_raises_the_typed_warning_with_both_numbers` reds alone.
//!   **Observed red 2026-08-08.**
//! * Delete the `fs <= 0.0` refusal: `a_backend_that_answers_zero_hz_refuses_arm`
//!   reds alone. **Observed red 2026-08-08** - arm SUCCEEDED against a 0 Hz
//!   device and the run went on to size an FFT and a settle window against it.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use eigen_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, EventKind, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RungFor, SineProfile, SineService, SweepDirection, SweepRate,
    SweepTarget,
};

/// What this rig REQUESTS. Off the 9234's ladder on purpose.
const REQUEST_HZ: f64 = 8_192.0;
/// What the ladder programs instead: 13.1072 MHz / (256 * 6).
const COERCED_HZ: f64 = 8_533.333_333_333_334;

// =============================================================================
// A backend that answers a rate of its own choosing
// =============================================================================

/// The minimum honest backend for an ARM-PATH test: it declares a proven Shared
/// clock (so the arm gate is not the thing refusing), and its `configure` answers
/// whatever rate and block the fixture told it to.
///
/// Deliberately NOT `eigen-hal-sim`: that plant answers the request back, which
/// is the one behaviour these tests need it not to have.
struct CoercingBackend {
    /// The rate `configure` answers, regardless of the request.
    answers_hz: f64,
    /// The block `configure` answers, regardless of the request.
    answers_block: usize,
    /// What the pre-configure `caps()` advertises.
    caps_hz: f64,
    max_output_channels: u32,
    /// Requests seen, so a test can assert the SECOND configure pass happened.
    pub seen: Vec<(f64, usize)>,
}

impl CoercingBackend {
    fn new(answers_hz: f64, answers_block: usize) -> Self {
        Self {
            answers_hz,
            answers_block,
            caps_hz: REQUEST_HZ,
            max_output_channels: 2,
            seen: Vec::new(),
        }
    }
}

impl Backend for CoercingBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            // The arm gate's clock precondition, satisfied so that the REFUSALS
            // these tests observe are the rate adoption's and not the gate's.
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: self.max_output_channels,
            // A NI-REAL station that drives an output is refused without one, and
            // that refusal is not what these tests are about.
            abort_latency_bound: Some(std::time::Duration::from_millis(25)),
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.caps_hz,
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
        _table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.seen.push((acq.requested_rate_hz, acq.block_frames));
        Ok(ConfiguredActual {
            actual_rate_hz: self.answers_hz,
            block_frames: self.answers_block,
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
        BackendStatus {
            connected: true,
            faulted: false,
            message: String::new(),
            sample_counter: 0,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

// =============================================================================
// The fixture
// =============================================================================

fn table_1ctrl() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

fn cfg(band: (f64, f64), control_tick_frames: usize) -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(band.0, 1.0));
    profile.push(ProfileRow::accel(band.1, 1.0));
    SineTestConfig {
        profile,
        mode: SineMode::Sweep(BidirSweepConfig {
            band,
            rate: SweepRate::LogOctPerMin(8.0),
            start_hz: band.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 1.0,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Sweeps(1),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

/// Build a service on a backend that answers `answers_hz` / `answers_block`, and
/// try to ARM it. `Ok` carries the armed service.
fn arm_on(
    answers_hz: f64,
    answers_block: usize,
    band: (f64, f64),
    control_tick_frames: usize,
) -> Result<SineService, eigen_sine::ServiceError> {
    let mut svc = SineService::new(
        cfg(band, control_tick_frames),
        table_1ctrl(),
        Box::new(CoercingBackend::new(answers_hz, answers_block)),
        // HARDWARE, so the block derivation is the rate-derived one rather than
        // the Sim's one-frame natural block. The class is what selects it.
        eigen_module_abi::StationClass::NiReal,
        0x51E5_0001,
        None,
    )
    .expect("construction");
    svc.assess_feasibility().expect("no shaker configured");
    svc.arm()?;
    Ok(svc)
}

/// The same rig with the operator's REQUESTED rate set (`SineTestConfig::fs_hz`).
fn arm_on_with_fs(
    fs_hz: Option<f64>,
    answers_hz: f64,
    answers_block: usize,
    band: (f64, f64),
    control_tick_frames: usize,
) -> Result<SineService, eigen_sine::ServiceError> {
    let mut c = cfg(band, control_tick_frames);
    c.fs_hz = fs_hz;
    let mut svc = SineService::new(
        c,
        table_1ctrl(),
        Box::new(CoercingBackend::new(answers_hz, answers_block)),
        eigen_module_abi::StationClass::NiReal,
        0x51E5_0001,
        None,
    )
    .expect("construction");
    svc.assess_feasibility().expect("no shaker configured");
    svc.arm()?;
    Ok(svc)
}

fn coercion_events(svc: &SineService) -> Vec<(f64, f64)> {
    svc.log()
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::RateCoerced {
                requested_hz,
                actual_hz,
            } => Some((*requested_hz, *actual_hz)),
            _ => None,
        })
        .collect()
}

// =============================================================================
// Adoption
// =============================================================================

// =============================================================================
// The REQUESTED rate (acceptance, 2026-08-09)
// =============================================================================

/// **`SineTestConfig::fs_hz` IS WHAT THE ACQUISITION SEAM IS ASKED FOR.**
///
/// The operator reversed the v1 "no rate knob" call at acceptance. A knob that is
/// stored and not asked for is worse than no knob: the panel would show a rate
/// the run never requested.
///
/// The discriminating observation is the coercion warning's `requested_hz`, which is
/// the same value `arm_inner` hands to `AcquisitionConfig::new` - so this reads the
/// number that actually crossed into the driver, not a field echoed back. It cannot
/// pass by accident: the rig's `caps()` advertises `REQUEST_HZ` (8192), so a config
/// whose `fs_hz` was dropped on the floor would report 8192 here.
///
/// RED-MUTATION: replace the `cfg.fs_hz` arm in `SineService::new` with plain
/// `caps.sample_rate_hz` and this reds on `requested 8192`.
#[test]
fn the_requested_rate_from_the_config_is_what_reaches_the_acquisition_seam() {
    const ASKED_HZ: f64 = 32_768.0;
    let svc = arm_on_with_fs(Some(ASKED_HZ), COERCED_HZ, 1024, (60.0, 500.0), 512).expect("arm");

    let events = coercion_events(&svc);
    assert_eq!(events.len(), 1, "exactly one coercion warning: {events:?}");
    let (req, act) = events[0];
    assert!(
        (req - ASKED_HZ).abs() < 1e-9,
        "the seam must have been asked for the OPERATOR's {ASKED_HZ} Hz, not the \
         station's {REQUEST_HZ} Hz; got {req}"
    );
    assert!((act - COERCED_HZ).abs() < 1e-9, "actual {act}");

    // ... and the ANSWER still wins downstream. A rate knob that changed what the
    // run believed it got would be the 6.25x defect with a UI in front of it.
    assert!(
        (svc.sample_rate_hz() - COERCED_HZ).abs() < 1e-9,
        "the armed run is denominated in the device's answer, not the request: {}",
        svc.sample_rate_hz()
    );
}

/// **ABSENT means the station's own rate, unchanged.** Every config written before
/// the field existed decodes to `None`, and this is the assertion that says those
/// runs behave exactly as they did.
///
/// Anti-vacuity against the test above: same rig, same band, the ONLY difference is
/// `fs_hz`, and the requested rate moves with it.
#[test]
fn an_absent_requested_rate_takes_the_stations_own() {
    let svc = arm_on_with_fs(None, COERCED_HZ, 1024, (60.0, 500.0), 512).expect("arm");
    let events = coercion_events(&svc);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        (events[0].0 - REQUEST_HZ).abs() < 1e-9,
        "with no requested rate the seam is asked for the station's {REQUEST_HZ} Hz; \
         got {}",
        events[0].0
    );
}

/// A requested rate that is not a rate is refused BY NAME at validation, before the
/// driver gets a chance to refuse it in its own words.
#[test]
fn a_nonsense_requested_rate_is_refused_by_name() {
    for bad in [0.0, -1.0, f64::NAN] {
        let mut c = cfg((60.0, 500.0), 512);
        c.fs_hz = Some(bad);
        let e = c.validate().expect_err("a rate must be a rate");
        let text = e.to_string();
        assert!(
            text.contains("fs_hz"),
            "the refusal must name the field: {text}"
        );
    }
}

/// RED-MUTATION test (module doc): delete the adoption line.
#[test]
fn the_adopted_rate_is_what_the_device_programmed() {
    let svc = arm_on(COERCED_HZ, 1024, (60.0, 500.0), 512).expect("arm");
    assert!(
        (svc.sample_rate_hz() - COERCED_HZ).abs() < 1e-9,
        "the run must be denominated in {COERCED_HZ}, not the {REQUEST_HZ} it asked for; \
         got {}",
        svc.sample_rate_hz()
    );
    assert_eq!(svc.configured().map(|a| a.actual_rate_hz), Some(COERCED_HZ));
}

/// RED-MUTATION test (module doc): delete the `RateCoerced` push.
#[test]
fn a_coerced_rate_raises_the_typed_warning_with_both_numbers() {
    let svc = arm_on(COERCED_HZ, 1024, (60.0, 500.0), 512).expect("arm");
    let events = coercion_events(&svc);
    assert_eq!(events.len(), 1, "exactly one coercion warning: {events:?}");
    let (req, act) = events[0];
    // BOTH numbers. An "actual" with no "requested" beside it cannot show a
    // coercion at all - that is the whole content of the warning.
    assert!((req - REQUEST_HZ).abs() < 1e-9, "requested {req}");
    assert!((act - COERCED_HZ).abs() < 1e-9, "actual {act}");
}

/// The other half. A warning that fires on every run is not a warning, so an
/// UNCOERCED device must raise none.
#[test]
fn an_uncoerced_rate_raises_no_warning() {
    let svc = arm_on(REQUEST_HZ, 1024, (60.0, 500.0), 512).expect("arm");
    assert!(!svc.rate_was_coerced());
    assert!(coercion_events(&svc).is_empty());
}

/// RED-MUTATION test (module doc): delete the `fs <= 0.0` refusal.
#[test]
fn a_backend_that_answers_zero_hz_refuses_arm() {
    let err = arm_on(0.0, 1024, (60.0, 500.0), 512)
        .err()
        .expect("0 Hz is not a rate this run can be denominated in");
    let text = err.to_string();
    assert!(
        text.contains('0') && text.contains("Refusing to arm"),
        "the refusal must name the answer and say it is refusing, not defaulting: {text}"
    );
    // "not known" is never "the same as what I asked for".
    assert!(
        !text.contains("8192.000 Hz, which is not"),
        "the refusal must not have silently substituted the request: {text}"
    );
}

// =============================================================================
// The two-pass configure
// =============================================================================

/// The block is a DURATION. A block sized against the pre-configure rate is the
/// wrong duration on a device that coerced, and the block period is what the
/// engine's tick cadence, ring depth and scheduling-health budget are built from.
#[test]
fn the_block_is_re_derived_from_the_adopted_rate_in_a_second_pass() {
    let svc = arm_on(COERCED_HZ, 1024, (60.0, 500.0), 512).expect("arm");
    let re_derived =
        svc.log().events().iter().any(
            |e| matches!(&e.kind, EventKind::Note { text } if text.contains("block re-derived")),
        );
    let want = eigen_hal::default_block_frames(COERCED_HZ);
    let asked = eigen_hal::default_block_frames(REQUEST_HZ);
    if want == asked {
        assert!(
            !re_derived,
            "the two rates round to the same block, so a second pass would be a \
             re-commit for nothing"
        );
    } else {
        assert!(
            re_derived,
            "the block for {COERCED_HZ} Hz is {want} frames but the first pass asked for \
             {asked}; the second pass must have run"
        );
    }
}

// =============================================================================
// The Nyquist / band guard
// =============================================================================

/// The HIGH edge CLAMPS, and says so. There is a real test underneath a band
/// whose top is past Nyquist, so narrowing it is right - silently narrowing it is
/// not.
#[test]
fn a_band_top_above_nyquist_is_clamped_and_reported() {
    // 0.45 x 8533.33 = 3840.0 Hz.
    let svc = arm_on(COERCED_HZ, 1024, (60.0, 6_000.0), 512).expect("arm");
    let nyq = 0.45 * COERCED_HZ;
    let clamp = svc
        .log()
        .events()
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::BandClamped { from_hz, to_hz, .. } => Some((*from_hz, *to_hz)),
            _ => None,
        })
        .expect("the clamp must be REPORTED, not silent");
    assert!((clamp.0 - 6_000.0).abs() < 1e-9, "from {}", clamp.0);
    assert!((clamp.1 - nyq).abs() < 1e-6, "to {} vs {nyq}", clamp.1);
}

/// The LOW edge REFUSES. A band whose bottom is above Nyquist has nothing left to
/// run, and narrowing it to a point would be inventing a test.
#[test]
fn a_band_bottom_above_nyquist_refuses_arm() {
    let err = arm_on(COERCED_HZ, 1024, (4_000.0, 6_000.0), 512)
        .err()
        .expect("nothing in this band can be synthesized");
    let text = err.to_string();
    assert!(
        text.contains("0.45"),
        "the refusal must state the rule: {text}"
    );
    assert!(
        text.contains("8533") || text.contains("3840"),
        "the refusal must state the ADOPTED rate or its limit: {text}"
    );
}

// =============================================================================
// The control-tick bound, restated in blocks
// =============================================================================

/// The old bound compared a FRAME count against a BLOCK count and was vacuous on
/// hardware by three orders of magnitude. The restated one is in the units the
/// batch pump uses, and it is checked against the device's answer.
#[test]
fn a_control_tick_longer_than_a_batch_refuses_arm() {
    // 2 ticks x a 64-frame block (MIN_BLOCK_FRAMES) = a 128-frame batch. A
    // 512-frame control tick cannot fire inside one.
    let err = arm_on(COERCED_HZ, eigen_hal::MIN_BLOCK_FRAMES, (60.0, 500.0), 512)
        .err()
        .expect("no control tick would fire in a batch");
    let text = err.to_string();
    assert!(text.contains("control_tick_frames"), "{text}");
    assert!(
        text.contains("per-BATCH"),
        "the bound must state the unit it is now in: {text}"
    );
    assert!(
        text.contains("STOP is not honored"),
        "the refusal must say what breaks, not just that a number is too big: {text}"
    );
}

/// The other half at the same block: a tick that DOES fit arms.
#[test]
fn a_control_tick_inside_the_batch_budget_arms() {
    let svc = arm_on(COERCED_HZ, eigen_hal::MIN_BLOCK_FRAMES, (60.0, 500.0), 128)
        .expect("2 x 64 = a 128-frame batch holds a 128-frame tick");
    assert!((svc.sample_rate_hz() - COERCED_HZ).abs() < 1e-9);
}
