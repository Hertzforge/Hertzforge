//! The Random loop is derived from the rate the DEVICE programs, not from the
//! HERTZFORGE-SIM demo default it was configured with.
//!
//! ## What this file is about
//!
//! `LoopConfig::demo_default()` hardcodes 8192 Hz - the HERTZFORGE-SIM plant rate - and the
//! UI's `loop_from_config` starts from it and adjusts only the band edges. The
//! delta-sigma NI 9234 programs 51 200 Hz. On the first real-chassis Random arm
//! (2026-07-30) the two diverged silently: the COLA synthesizer, the analysis grid
//! (`df = fs/nfft`) and every windowed statistic kept running at 8192 while the device
//! played at 51 200, so every profile frequency would have landed 6.25x high on a
//! shaker. An earlier fix made the arm gate see the loop's own rate, which turned the
//! silent 6.25x into a typed refusal - correct, and it left Random unable to arm on NI
//! at all. This file pins the fix: the loop ADOPTS the device's rate and re-derives
//! itself around it.
//!
//! RED-MUTATION for this file: delete the `cfg.adopt_station_actual(...)` call from
//! `PlantLoop::open` and the station-rate tests red with the arm gate's
//! `RateMismatch` - which is exactly the interim state that earlier fix produced.
//!
//! ## What the fixture here can and cannot prove
//!
//! [`CoercingStation`] models the 9234's coercion at the seam that matters: it
//! reports a `ConfiguredActual` whose rate is NOT the one requested. The plant
//! behind it is the Sim, which adopts the requested rate into its own physics, so
//! the double is a faithful model of the CONTRACT (device says X while the caller
//! asked for Y) and NOT of the physics (its samples really do advance at the
//! requested rate). Every assertion below is therefore about the loop's
//! configuration and the refusals - never about convergence, which only the real
//! chassis can settle.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::RandomTestConfig;
use hertzforge_random::service::engine::PlantLoop;
use hertzforge_random::service::report::RunState;
use hertzforge_random::{LoopConfig, RandomService, MIN_CONTROL_LINES};
use std::time::Duration;

/// The rate the 9234 programs on the arrival chassis (gate 2 evidence).
const NI_9234_RATE_HZ: f64 = 51_200.0;
/// The HERTZFORGE-SIM demo rate `LoopConfig::demo_default()` carries.
const DEMO_RATE_HZ: f64 = 8_192.0;

// ===========================================================================
// The double
// ===========================================================================

/// A backend that COERCES the configured rate the way a delta-sigma NI module
/// does, and characterizes an abort-latency bound the way the NI backend does (the
/// Sim reports `None`, which the arm gate refuses on a hardware station).
///
/// Everything else delegates to a real `SimBackend`, so the loop still opens, arms,
/// starts and streams.
struct CoercingStation {
    inner: SimBackend,
    coerced_rate_hz: f64,
    /// What the device says it will deliver per block, independent of the request.
    coerced_block_frames: Option<usize>,
    abort_latency_bound: Option<Duration>,
}

impl CoercingStation {
    /// A station that programs `rate_hz` whatever it is asked for, and carries the
    /// abort-latency bound + shared clock a hardware station needs to pass the gate.
    fn at(rate_hz: f64) -> Self {
        CoercingStation {
            inner: SimConfig::new()
                .sample_rate_hz(DEMO_RATE_HZ)
                .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
                .noise_level(2e-4)
                .feedthrough(0.35)
                .shared_clock(true)
                .seed(0x5A5A_1234)
                .max_input_channels(1)
                .max_output_channels(1)
                .build(),
            coerced_rate_hz: rate_hz,
            coerced_block_frames: None,
            abort_latency_bound: Some(Duration::from_millis(20)),
        }
    }

    fn block_frames(mut self, frames: usize) -> Self {
        self.coerced_block_frames = Some(frames);
        self
    }

    fn boxed(self) -> Box<dyn Backend> {
        Box::new(self)
    }
}

impl Backend for CoercingStation {
    fn caps(&self) -> Caps {
        Caps {
            abort_latency_bound: self.abort_latency_bound,
            sample_rate_hz: self.coerced_rate_hz,
            ..self.inner.caps()
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let actual = self.inner.configure(table, acq)?;
        Ok(ConfiguredActual {
            actual_rate_hz: self.coerced_rate_hz,
            block_frames: self.coerced_block_frames.unwrap_or(actual.block_frames),
            ..actual
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

/// The demo profile, spanning 20-2000 Hz.
fn profile() -> BreakpointTable {
    let mut bt = BreakpointTable::new("demo-profile");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(80.0, 0.04));
    bt.push(BreakpointRow::psd(350.0, 0.04));
    bt.push(BreakpointRow::psd(2000.0, 0.007));
    bt
}

/// The demo loop with its band sized to `profile()` the way `loop_from_config` does.
fn profile_loop() -> LoopConfig {
    let mut cfg = LoopConfig::demo_default();
    cfg.band_lo_hz = 20.0;
    cfg.band_hi_hz = 2000.0;
    cfg
}

// ===========================================================================
// 1. Adoption keeps the profile band in Hz
// ===========================================================================

/// The band edges are Hz-denominated, so adopting a new rate must move the BINS the
/// band lands in and leave the FREQUENCIES alone. The failure this excludes is the
/// mirror of the original defect: re-interpreting 60-780 as bin indices, or holding
/// bins fixed so the band silently becomes 375-4875 Hz at the higher rate.
#[test]
fn adopting_the_actual_rate_keeps_the_profile_band_in_hz() {
    let mut cfg = profile_loop();
    let before = (cfg.band_lo_hz, cfg.band_hi_hz);
    let before_bins = cfg.band_bins();

    let adoption = cfg
        .adopt_station_actual(StationClass::NiReal, &configured(NI_9234_RATE_HZ, 1024))
        .expect("the demo profile survives the 9234's grid");

    assert_eq!(
        (cfg.band_lo_hz, cfg.band_hi_hz),
        before,
        "the band is Hz; adopting a rate must not rewrite its edges"
    );
    assert!(adoption.rate_changed() && adoption.grid_changed());

    // The bins moved because df moved, and they still bracket the same frequencies.
    let (lo, hi) = cfg.band_bins();
    assert_ne!((lo, hi), before_bins, "a 6.25x rate change moves the bins");
    let df = cfg.df();
    assert!(
        (lo as f64 * df - cfg.band_lo_hz).abs() <= df,
        "the low bin must still sit on the band's low edge: bin {lo} at df {df}"
    );
    assert!(
        (hi.saturating_sub(1) as f64 * df - cfg.band_hi_hz).abs() <= df,
        "the high bin must still sit on the band's high edge: bin {hi} at df {df}"
    );
}

// ===========================================================================
// 2. A station at another rate produces a loop at the STATION's rate
// ===========================================================================

/// The headline property. `PlantLoop::open` must hand its caller back a config that
/// describes what the DEVICE will run - 51 200 Hz - not the 8192 Hz the service
/// asked for.
#[test]
fn a_station_at_a_different_rate_produces_a_loop_at_the_station_rate_not_the_demo_default() {
    let mut cfg = profile_loop();
    assert_eq!(
        cfg.fs_hz, DEMO_RATE_HZ,
        "the demo default is where we start"
    );

    let loop_ = PlantLoop::open(
        CoercingStation::at(NI_9234_RATE_HZ).boxed(),
        StationClass::NiReal,
        &mut cfg,
        table(),
    )
    .expect("a 51 200 Hz station arms once the loop adopts its rate");

    assert_eq!(
        cfg.fs_hz, NI_9234_RATE_HZ,
        "the loop must run at the rate the device programmed"
    );
    assert_ne!(
        cfg.fs_hz, DEMO_RATE_HZ,
        "8192 Hz here is the 6.25x defect: drive synthesized for a rate nothing plays at"
    );
    assert_eq!(
        cfg.df(),
        NI_9234_RATE_HZ / cfg.nfft as f64,
        "the analysis grid follows the adopted rate"
    );
    let adoption = loop_.adoption();
    assert_eq!(adoption.before.fs_hz, DEMO_RATE_HZ);
    assert_eq!(adoption.after.fs_hz, NI_9234_RATE_HZ);
}

// ===========================================================================
// 3. The arm gate then passes - and the OTHER half, that it could have failed
// ===========================================================================

/// `PlantLoop::open` returning `Ok` above already proves the gate passed (it is the
/// only refusal on that path). This is the half that proves the pass MEANS
/// something: the same two rates, put to the gate's own rate policy unadopted, are a
/// refusal. Without this, "the gate passes" is a green that cannot fail.
#[test]
fn the_gate_would_have_refused_the_unadopted_rate() {
    let unadopted = hertzforge_module_abi::rate_policy(DEMO_RATE_HZ, NI_9234_RATE_HZ);
    assert!(
        unadopted.is_some(),
        "8192 vs 51 200 must be a refusal - that is the state 9229cc9 left Random in"
    );
    let text = format!("{}", unadopted.expect("refusal"));
    assert!(
        text.contains("never resamples"),
        "the refusal names the no-resample rule: {text}"
    );

    // And adopted, the same pair is accepted - the adoption IS the regeneration the
    // refusal asks the operator to perform, done from a Hz-denominated profile.
    assert!(
        hertzforge_module_abi::rate_policy(NI_9234_RATE_HZ, NI_9234_RATE_HZ).is_none(),
        "adopting the device rate is what makes the gate's statement true"
    );
}

// ===========================================================================
// 4. The SERVICE's grid follows, not just the loop config
// ===========================================================================

/// Adopting the rate inside `PlantLoop::open` is only half the job: `RandomService`
/// interpolated the reference onto the OLD grid at construction, so `arm_inner` must
/// rebuild the grid-denominated state before the synth and the envelopes are built.
/// The failure this excludes is a loop that runs at 51 200 Hz while its target PSD
/// is still binned at 8 Hz per line.
#[test]
fn the_service_grid_follows_the_station_rate_through_arm() {
    let mut cfg = RandomTestConfig::siso(profile(), 1);
    cfg.full_level_duration_s = 0.0;
    let mut svc = RandomService::new(
        cfg,
        table(),
        profile_loop(),
        CoercingStation::at(NI_9234_RATE_HZ).boxed(),
        StationClass::NiReal,
        Some(std::env::temp_dir().join("hertzforge-random-rate-adoption")),
    )
    .expect("the service builds on the demo grid");

    assert_eq!(
        svc.realized_df_hz(),
        DEMO_RATE_HZ / 1024.0,
        "before arm the grid is the one the caller supplied"
    );

    svc.assess_feasibility().expect("no shaker; advisory");
    svc.arm().expect("arming adopts the station rate");

    assert_eq!(svc.loop_config().fs_hz, NI_9234_RATE_HZ);
    assert_eq!(
        svc.realized_df_hz(),
        NI_9234_RATE_HZ / 1024.0,
        "the service's line grid must be rebuilt at the adopted rate"
    );
    assert_eq!(svc.state(), RunState::SysId);

    // The target still carries energy - the reference was re-interpolated onto the
    // new grid rather than left describing bins that no longer mean the same Hz.
    assert!(
        svc.full_target_grms() > 0.0,
        "the reference must be re-interpolated onto the adopted grid"
    );

    // And the run's log says so, so a rate that moved is answerable afterwards.
    let logged = svc.log().events().iter().any(|e| {
        matches!(
            &e.kind,
            hertzforge_random::service::report::EventKind::StationAdopted {
                requested_fs_hz,
                actual_fs_hz,
                ..
            } if *requested_fs_hz == DEMO_RATE_HZ && *actual_fs_hz == NI_9234_RATE_HZ
        )
    });
    assert!(logged, "the adoption must be in the event log verbatim");
}

// ===========================================================================
// 5-7. The refusals - what adoption will NOT do silently
// ===========================================================================

/// Adoption is not unconditional. `band_bins` CLAMPS, so a grid too coarse for the
/// profile would present as a quietly narrower control band rather than an error.
/// A 100-200 Hz profile at the 9234's 50 Hz per line is three lines; that is not a
/// spectrum being controlled, and it refuses.
#[test]
fn adoption_refuses_a_grid_too_coarse_for_the_profile_band() {
    let mut cfg = LoopConfig::demo_default();
    cfg.band_lo_hz = 100.0;
    cfg.band_hi_hz = 200.0;

    let refusal = cfg
        .adopt_station_actual(StationClass::NiReal, &configured(NI_9234_RATE_HZ, 1024))
        .expect_err("three lines cannot carry a 100-200 Hz profile");

    let text = format!("{refusal}");
    assert!(
        text.contains("too coarse"),
        "the refusal must say what is wrong: {text}"
    );
    assert!(
        text.contains(&MIN_CONTROL_LINES.to_string()),
        "the refusal must name the floor it applied: {text}"
    );
    assert_eq!(
        cfg.fs_hz, DEMO_RATE_HZ,
        "a refused adoption must not leave the config half-rewritten"
    );
}

/// The other clamp. A station that programs a rate BELOW the profile's top
/// breakpoint would drive only the part of the band under its Nyquist and say
/// nothing - the same silent-truncation shape, in the opposite direction from the
/// 9234's coercion.
#[test]
fn adoption_refuses_when_the_band_reaches_above_the_adopted_grid() {
    let mut cfg = profile_loop(); // 20-2000 Hz
    let refusal = cfg
        .adopt_station_actual(StationClass::NiReal, &configured(1_000.0, 128))
        .expect_err("a 1 kHz station cannot carry a 2 kHz profile");

    let text = format!("{refusal}");
    assert!(
        text.contains("silently") && text.contains("undriven"),
        "the refusal must name the silent truncation it is preventing: {text}"
    );
    assert_eq!(cfg.fs_hz, DEMO_RATE_HZ, "the config is left untouched");
}

/// A backend that reports a rate no grid can be built from is a refusal, never a
/// fall-through that keeps the Sim default - keeping 8192 Hz because the device
/// answered garbage is the original defect wearing a different hat.
#[test]
fn an_unusable_reported_rate_is_refused_not_ignored() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut cfg = profile_loop();
        let refusal = cfg
            .adopt_station_actual(StationClass::NiReal, &configured(bad, 1024))
            .expect_err("an unusable rate must refuse");
        assert!(
            format!("{refusal}").contains("will not fall back"),
            "the refusal must say it is refusing rather than defaulting"
        );
        assert_eq!(cfg.fs_hz, DEMO_RATE_HZ);
    }
}

// ===========================================================================
// 8-9. Station-keyed behaviour: HERTZFORGE-SIM untouched, hardware guarded
// ===========================================================================

/// The Sim path must come out of adoption bit-for-bit unchanged. Its rate never
/// diverges (the Sim adopts the request), its one-frame block is the deliberate
/// sample-exact segment accounting `SIM_NATURAL_BLOCK_FRAMES` names, and the two
/// harness relaxations are TRUE there. This is the inverse-pin: the fix must not
/// quietly re-tune the station every control-core and service acceptance gate is
/// measured on.
#[test]
fn an_hertzforge_sim_station_is_left_exactly_as_configured() {
    let mut cfg = LoopConfig::demo_default();
    let before = cfg.clone();

    let adoption = cfg
        .adopt_station_actual(
            StationClass::HertzforgeSim,
            &configured(DEMO_RATE_HZ, hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES),
        )
        .expect("the Sim's own rate is always adoptable");

    assert_eq!(cfg.fs_hz, before.fs_hz);
    assert_eq!(
        cfg.block_frames,
        hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES,
        "the Sim's sample-exact segment accounting must survive"
    );
    assert_eq!(
        cfg.jitter_frac,
        hertzforge_engine::TEST_DISABLE_JITTER_GUARD,
        "the harness relaxations are true on HERTZFORGE-SIM"
    );
    assert_eq!(
        cfg.compute_budget,
        Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET)
    );
    assert!(!adoption.rate_changed() && !adoption.guards_restored());
}

/// The relaxations `LoopConfig::demo_default` carries are documented as
/// never-on-hardware: `TEST_DISABLE_JITTER_GUARD` switches the period-jitter check
/// off, and `TEST_RELAX_COMPUTE_BUDGET` widens a per-tick deadline to one second.
/// hertzforge-sine gates both by station; Random did not, so the Sim's values rode onto
/// NI. That cost nothing while the rate divergence kept Random from arming at all -
/// and would have cost the engine's scheduling-health guards the moment this fix
/// let it arm.
#[test]
fn a_hardware_station_drops_the_deterministic_harness_relaxations() {
    let mut cfg = profile_loop();
    assert_eq!(
        cfg.jitter_frac,
        hertzforge_engine::TEST_DISABLE_JITTER_GUARD
    );
    assert!(cfg.compute_budget.is_some());

    let adoption = cfg
        .adopt_station_actual(StationClass::NiReal, &configured(NI_9234_RATE_HZ, 1024))
        .expect("adoptable");

    assert_ne!(
        cfg.jitter_frac,
        hertzforge_engine::TEST_DISABLE_JITTER_GUARD,
        "the period-jitter guard must be live on hardware"
    );
    assert!(
        cfg.compute_budget.is_none(),
        "on hardware the tick period IS the deadline"
    );
    assert!(adoption.guards_restored());
}

/// The block a hardware loop describes comes from what the device ANSWERED, not
/// from what was requested and not from a second derivation that could disagree
/// with the engine's own cadence (`EngineConfig::with_block` reads the same
/// `ConfiguredActual`).
#[test]
fn a_hardware_station_adopts_the_block_the_device_answered() {
    let mut cfg = profile_loop();
    assert_eq!(
        cfg.block_frames,
        hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES
    );

    let _ = PlantLoop::open(
        CoercingStation::at(NI_9234_RATE_HZ)
            .block_frames(1024)
            .boxed(),
        StationClass::NiReal,
        &mut cfg,
        table(),
    )
    .expect("opens");

    assert_eq!(
        cfg.block_frames, 1024,
        "the loop must describe the block the device programmed"
    );
}

/// The request-side half, and the reason restoring the guards is not a separate
/// change. The NI backend ECHOES the requested block, so a Sim one-frame request
/// becomes a one-frame block on the device - a 19.5 us tick at 51 200 Hz, which is
/// also the per-tick compute deadline once `compute_budget` goes to `None`. A loop
/// that armed with both would scheduling-fault immediately. `open` must therefore
/// request a hardware block BEFORE it configures.
///
/// RED-MUTATION: delete the `station.is_hardware()` block-request branch in
/// `PlantLoop::open` and this reds with `block_frames = 1`.
#[test]
fn a_hardware_station_never_requests_the_sim_one_frame_block() {
    let mut cfg = profile_loop();
    assert_eq!(
        cfg.block_frames,
        hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES,
        "demo_default carries the Sim block; this is what the UI hands us"
    );

    // An echoing station, exactly like the NI backend: whatever is requested comes
    // back as the actual, so this test reads the REQUEST through the answer.
    let _ = PlantLoop::open(
        CoercingStation::at(NI_9234_RATE_HZ).boxed(),
        StationClass::NiReal,
        &mut cfg,
        table(),
    )
    .expect("opens");

    assert_eq!(
        cfg.block_frames,
        hertzforge_hal::default_block_frames(DEMO_RATE_HZ),
        "a hardware station must request a hardware block, not the Sim's one frame"
    );
    assert!(
        cfg.block_frames >= hertzforge_hal::MIN_BLOCK_FRAMES,
        "one frame per tick on a DAQ card is never a deliberate choice"
    );

    // The pairing that makes it matter: the guards are live AND the tick has room.
    assert!(cfg.compute_budget.is_none());
    let tick_seconds = cfg.block_frames as f64 / NI_9234_RATE_HZ;
    assert!(
        tick_seconds > 1e-3,
        "the per-tick deadline must not be microseconds: {tick_seconds} s"
    );
}

/// The other side of that rule: a hardware block the caller chose deliberately is
/// honored, not overwritten. Without this, the branch above would be indistinguishable
/// from "hardware always ignores the caller".
#[test]
fn a_deliberate_hardware_block_is_honored() {
    let mut cfg = profile_loop();
    cfg.block_frames = 512;

    let _ = PlantLoop::open(
        CoercingStation::at(NI_9234_RATE_HZ).boxed(),
        StationClass::NiReal,
        &mut cfg,
        table(),
    )
    .expect("opens");

    assert_eq!(cfg.block_frames, 512, "the caller's choice must survive");
}

// ===========================================================================

fn configured(rate_hz: f64, block_frames: usize) -> hertzforge_hal::ConfiguredActual {
    hertzforge_hal::ConfiguredActual {
        actual_rate_hz: rate_hz,
        block_frames,
        group_delay_frames: None,
    }
}
