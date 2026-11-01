//! Service acceptance - `RandomService` end-to-end against the SimBackend plant,
//! mirroring the adversarial-fixture discipline.
//!
//! Each test drives the real [`hertzforge_random::RandomService`] through its state machine
//! against an HERTZFORGE-SIM plant; the service closes the loop through the REAL
//! `hertzforge_engine::Engine` (frames_per_tick = 1, DSP amortized between segments), so the
//! whole run path (ring, timebase, watchdog, output contract, capture sink) is
//! exercised, not a toy integrator.
//!
//! ## Mechanical change when the backend moved out
//!
//! `RandomService::new` used to take one `PlantConfig` carrying BOTH the loop knobs
//! and the Sim plant description, and built the `SimBackend` itself. It now takes the
//! neutral `LoopConfig` plus a finished `Box<dyn Backend>` + its `StationClass`. The
//! two halves live together in [`DemoPlant`] here and `build_service` re-joins them,
//! so every test body below is UNCHANGED - only the constructor's name moved.
//!
//! - (a) Full AUTO ladder on the decoded demo profile: feasibility passes, sysID ->
//!   coherence gate passes -> -12 dB engage -> ladder to full -> converges within
//!   +/-1.5 dB band mean at full -> duration -> clean stop; history + report complete.
//! - (b) Coherence gate HOLD: noise-swamped sysID parks at CoherenceHold, never engages.
//! - (c) Response limit notch: a limit profile straddling a band; the limited channel
//!   stays under its limit, no oscillation across 30+ loops.
//! - (d) Envelope abort: an un-fillable anti-resonance drives the measured PSD below the
//!   abort floor -> Aborted with an envelope trip recorded.
//! - (e) Kinematic abort: a tight accel_g on a monitor channel trips -> immediate stop.
//! - (f) AutoFirstSeconds capture: a ladder run produces one capture file per level entry
//!   with the debounce semantics; sidecars carry level metadata.
//! - (g) Feasibility hard-block: an overweight article refuses arming; the override path
//!   logs the ack.
//! - (h) MANUAL mode stepping: +/- 1 dB and jump-to-table-level honored.
//! - (i) Drive-Vrms abort: a low drive-abort threshold stops the run when
//!   the synthesized drive Vrms breaches it.
//! - (j) Divergence abort (divergence caller contract): a plant whose response
//!   collapses to zero starves the loop so the trim clamps at the max for
//!   `divergence_loops` consecutive loops -> Aborted with a Divergence trip.

use hertzforge_breakpoints::{grms, BreakpointRow, BreakpointTable, LineSpectrum};
use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable, Units};
use hertzforge_random::service::config::{
    CaptureRules, CaptureTrigger, LevelSchedule, LimitKind, LimitProfile, RandomTestConfig, RunMode,
};
use hertzforge_random::service::report::RunState;
use hertzforge_random::service::RandomService;
use hertzforge_random::LoopConfig;
use hertzforge_shaker_db::{MovingMass, RatingValue, ShakerRecord};

// ===========================================================================
// Shared builders
// ===========================================================================

/// The hertzforge-demo fixture: the neutral loop config PLUS the HERTZFORGE-SIM plant it was
/// validated against. These were once one struct (`PlantConfig`); the plant half
/// belongs to whoever builds the backend, which for a test is the test.
#[derive(Clone)]
struct DemoPlant {
    cfg: LoopConfig,
    resonances: Vec<(f64, f64)>,
    noise_level: f64,
    feedthrough: f64,
    plant_seed: u64,
    input_channels: u32,
}

impl DemoPlant {
    /// The line grid + band helpers, delegated so the test bodies read unchanged.
    fn df(&self) -> f64 {
        self.cfg.df()
    }
    fn nbins(&self) -> usize {
        self.cfg.nbins()
    }
    fn bin_of(&self, hz: f64) -> usize {
        self.cfg.bin_of(hz)
    }
    fn band_bins(&self) -> (usize, usize) {
        self.cfg.band_bins()
    }

    /// Build the HERTZFORGE-SIM backend. The numbers are the former
    /// `PlantConfig::demo_default()` values, transcribed so the acceptance gates measure
    /// the same plant they were validated against.
    fn backend(&self) -> Box<dyn hertzforge_hal::Backend> {
        Box::new(
            hertzforge_hal_sim::SimConfig::new()
                .sample_rate_hz(self.cfg.fs_hz)
                .resonances(self.resonances.clone())
                .noise_level(self.noise_level)
                .feedthrough(self.feedthrough)
                .shared_clock(true)
                .seed(self.plant_seed)
                .max_input_channels(self.input_channels.max(1))
                .max_output_channels(1)
                .build(),
        )
    }
}

/// The hertzforge-demo default plant (two in-band modes, healthy feedthrough).
fn demo_plant() -> DemoPlant {
    DemoPlant {
        cfg: LoopConfig::demo_default(),
        resonances: vec![(200.0, 2.5), (550.0, 2.5)],
        noise_level: 2e-4,
        feedthrough: 0.35,
        plant_seed: 0x5A5A_1234,
        input_channels: 1,
    }
}

/// The former `RandomService::new(cfg, table, plant, data_root)` shape, re-joined
/// from the two halves. HERTZFORGE-SIM by construction - the plant IS the Sim.
fn build_service(
    cfg: RandomTestConfig,
    table: ChannelTable,
    plant: DemoPlant,
    data_root: Option<std::path::PathBuf>,
) -> Result<RandomService, hertzforge_random::service::ServiceError> {
    let backend = plant.backend();
    let svc = RandomService::new(
        cfg,
        table,
        plant.cfg,
        backend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        data_root,
    );
    // The post-sysID review hold defaults ON (the expensive-article case). Every
    // acceptance test below except (l) exercises the DRIVING path and would
    // otherwise park at the hold, so the helper opts out and (l) sets the
    // policy explicitly in both directions.
    svc.map(|mut s| {
        s.set_review_after_sysid(false);
        // The 2026-08-03 SOFT START engages below the lowest scheduled level
        // and ramps up. These acceptance tests are about ladder, dwell, notch
        // and review semantics - they assert the level the run ENGAGES at - so
        // they opt out and the soft start has its own dedicated test below.
        s.set_soft_start_db(0.0);
        s
    })
}

/// A single-control-channel table on channel 1 (accel g), drive on channel 2, with a
/// generous control abort so the healthy loop runs.
fn siso_table(control_abort_g: f64) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort.accel_g = Some(control_abort_g);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

/// The decoded demo profile (20/0.01, 80/0.04, 350/0.04, 2000/0.007), the (a) gate
/// profile.
fn demo_profile() -> BreakpointTable {
    let mut bt = BreakpointTable::new("demo-profile");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(80.0, 0.04));
    bt.push(BreakpointRow::psd(350.0, 0.04));
    bt.push(BreakpointRow::psd(2000.0, 0.007));
    bt
}

/// A flat in-band-ish profile (anchors bracketing the interior band) at `level` g^2/Hz.
fn flat_profile(level: f64) -> BreakpointTable {
    let mut bt = BreakpointTable::new("flat");
    bt.push(BreakpointRow::psd(20.0, level));
    bt.push(BreakpointRow::psd(2000.0, level));
    bt
}

/// A generous shaker record that passes feasibility on the demo profile with a light
/// mass stack.
fn generous_shaker() -> ShakerRecord {
    let mut s = ShakerRecord::new("generous-test-shaker");
    s.manufacturer = "test".into();
    s.model = "big".into();
    s.force_random = Some(RatingValue::rms(50_000.0));
    s.max_velocity_ips = Some(RatingValue::peak(100.0));
    s.max_displacement_in_pp = Some(RatingValue::peak(2.0));
    s.max_accel_bare_g = Some(RatingValue::peak(1000.0));
    s.armature_mass_lb = Some(50.0);
    s
}

/// A light moving-mass stack.
fn light_mass() -> MovingMass {
    MovingMass {
        armature_lb: 50.0,
        slip_table_lb: 0.0,
        drive_adapter_lb: 0.0,
        fixturing_lb: 5.0,
        test_article_lb: 10.0,
    }
}

/// The in-band mean level error (dB) of the final composite vs the target, over the
/// band, from the report's final composite PSD + target.
fn final_band_mean_db(svc: &RandomService) -> f64 {
    let plant = demo_plant();
    let (lo, hi) = plant.band_bins();
    let report = svc.report_data();
    let comp = report.final_composite_psd.expect("final composite present");
    let tgt = &report.target_psd;
    let mean_meas: f64 = comp.psd[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
    let mean_tgt: f64 = tgt.psd[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
    10.0 * (mean_meas.max(1e-30) / mean_tgt.max(1e-30)).log10()
}

// ===========================================================================
// (a) Full AUTO ladder + convergence + clean stop
// ===========================================================================

#[test]
fn acceptance_a_full_auto_ladder_converges_and_stops() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::auto_default();
    cfg.schedule.dwell_s = vec![1.0]; // short dwell so the ladder walks in a few ticks
    cfg.full_level_duration_s = 3.0;
    cfg.drive_ceiling_vrms = 1_000.0; // generous: the loop, not the ceiling, sets drive
    cfg.shaker = Some(generous_shaker());
    cfg.moving_mass = light_mass();
    // A wide envelope so the ladder transients never trip the abort during convergence.
    cfg.alarm_db = (6.0, 6.0);
    cfg.abort_db = (12.0, 12.0);

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-a")),
    )
    .expect("service builds");

    let final_state = svc.run_auto(3, 400).expect("auto run");

    // Feasibility passed (not blocked).
    let fz = svc.feasibility().expect("feasibility assessed");
    assert!(
        !fz.report.exceeded(),
        "feasibility should pass the generous shaker"
    );

    // The gate passed and the run reached full level then stopped cleanly.
    println!(
        "(a) final state {:?}, coherence {:.3}, trip {:?}",
        final_state,
        svc.band_coherence(),
        svc.trip()
    );
    assert_eq!(
        final_state,
        RunState::Stopped,
        "AUTO run should end Stopped"
    );

    // Level history walked the full table (start -12 up to full 0 dB).
    let report = svc.report_data();
    let levels: Vec<f64> = report.level_history.iter().map(|h| h.level_db).collect();
    assert!(
        levels.first().copied().unwrap_or(1.0) <= -12.0 + 1e-9,
        "ladder started at -12 dB, got {levels:?}"
    );
    assert!(
        levels.iter().any(|&l| (l - 0.0).abs() < 1e-9),
        "ladder reached full (0 dB), got {levels:?}"
    );

    // Converged within +/-1.5 dB band mean at full level (the demo-parity gate).
    let mean = final_band_mean_db(&svc);
    println!("(a) band-mean at full: {mean:.2} dB; level history {levels:?}");
    assert!(
        mean.abs() <= 1.5,
        "band-mean {mean} dB not within +/-1.5 dB of target"
    );

    // Report is complete: settings, channels, event log, final spectra, honesty.
    assert!(!report.channels.is_empty());
    assert!(report.final_composite_psd.is_some());
    assert!(report.final_drive_psd.is_some());
    assert!(!report.event_log.events().is_empty());
    assert!(report.honesty.path.contains("Path B"));
    assert!(report.trip.is_none(), "clean AUTO run has no trip");

    // Report round-trips through JSON (serde-serializable).
    let json = serde_json::to_string(&report).expect("report serializes");
    assert!(json.contains("\"honesty\""));
    let _back: hertzforge_random::ReportData =
        serde_json::from_str(&json).expect("report round-trips");
}

// ===========================================================================
// (b) Coherence gate HOLD
// ===========================================================================

#[test]
fn acceptance_b_coherence_gate_holds_on_noise_swamped_sysid() {
    // A plant with a huge noise floor swamps the low-level sysID, so the drive->response
    // coherence is far below the floor. The gate must HOLD and never engage.
    let mut plant = demo_plant();
    plant.noise_level = 5.0; // enormous relative to the -20 dB sysID drive
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.coherence_floor = 0.5;

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-b")),
    )
    .expect("service builds");

    let state = svc.run_auto(3, 400).expect("auto run");
    println!(
        "(b) band coherence {:.3}, state {:?}",
        svc.band_coherence(),
        state
    );
    assert_eq!(
        state,
        RunState::CoherenceHold,
        "gate should HOLD on noise-swamped sysID"
    );
    assert!(
        svc.band_coherence() < 0.5,
        "band coherence {} should be below the floor",
        svc.band_coherence()
    );
    // The report records the gate hold event; the loop never engaged.
    let report = svc.report_data();
    let held = report.event_log.events().iter().any(|e| {
        matches!(
            &e.kind,
            hertzforge_random::service::report::EventKind::CoherenceGate { passed: false, .. }
        )
    });
    assert!(held, "coherence gate hold event recorded");
    // No level history beyond nothing (never engaged).
    assert!(
        report.level_history.is_empty(),
        "never engaged, so no level history"
    );
}

// ===========================================================================
// (c) Response limit notch (no oscillation across 30+ loops)
// ===========================================================================

#[test]
fn acceptance_c_response_limit_notch_holds_no_oscillation() {
    let plant = demo_plant();
    let nbins = plant.nbins();
    let table = siso_table(500.0);

    // Flat target the whole band; a response-limit profile caps a SUB-band well below
    // what the flat target needs, so the notch must engage there.
    let mut cfg = RandomTestConfig::siso(flat_profile(1.0e-2), 1);
    // MANUAL, park at full so we can run many loops at a fixed level.
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.alarm_db = (6.0, 6.0);
    cfg.abort_db = (24.0, 24.0); // wide: the notch, not the envelope, is under test

    // Response ceiling: cap the measured RESPONSE in [300,450] Hz an order of magnitude
    // below the flat target. Below that band and above it, no limit (+inf).
    let lo = plant.bin_of(300.0);
    let hi = plant.bin_of(450.0);
    let mut ceiling = vec![f64::INFINITY; nbins];
    for c in ceiling[lo..hi].iter_mut() {
        *c = 1.0e-3; // 10 dB below the 1e-2 target response
    }
    cfg.limit_profiles = vec![LimitProfile {
        channel_index: 1,
        kind: LimitKind::Response,
        resp_ceiling: ceiling,
    }];

    let mut svc = build_service(
        cfg,
        table,
        plant.clone(),
        Some(std::env::temp_dir().join("hertzforge-r4-c")),
    )
    .expect("service builds");

    svc.assess_feasibility()
        .expect("feasibility (no shaker -> not evaluated)");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(
        svc.coherence_gate().expect("gate"),
        "clean plant should pass the gate"
    );
    // Jump to full level (0 dB) and run 40 loops.
    svc.jump_to_table_level(4).expect("jump to full");

    let mut notch_counts = Vec::new();
    for _ in 0..40 {
        if !svc.loop_tick().expect("tick") {
            break;
        }
        notch_counts.push(svc.status().notch_active_lines);
    }

    // The notch engaged and held stable across the back half (no on/off oscillation).
    let back: Vec<usize> = notch_counts[notch_counts.len().saturating_sub(20)..].to_vec();
    assert!(!back.is_empty(), "ran enough loops");
    let stable = back.iter().all(|&c| c == back[0]);
    assert!(
        stable,
        "notch line count oscillated across the back half: {back:?}"
    );
    assert!(
        back[0] >= (hi - lo) / 2,
        "notch did not hold the ceiling band ({})",
        back[0]
    );

    // The limited channel's final measured RESPONSE in the notch band stays under the
    // limit (the notch actually protected it).
    let report = svc.report_data();
    let comp = report.final_composite_psd.expect("final composite");
    let over: usize = comp.psd[lo..hi].iter().filter(|&&p| p > 1.5e-3).count();
    println!(
        "(c) notch back-half {}, bins over limit {}/{}",
        back[0],
        over,
        hi - lo
    );
    assert!(
        over <= (hi - lo) / 5,
        "limited channel exceeded its limit in {over} of {} bins",
        hi - lo
    );
    // No divergence abort from the perpetual notch clamp.
    assert!(
        report.trip.is_none(),
        "notch fight raised a spurious abort: {:?}",
        report.trip
    );
    assert_ne!(svc.state(), RunState::Aborted);
}

// ===========================================================================
// (d) Envelope abort
// ===========================================================================

#[test]
fn acceptance_d_envelope_abort_on_unfillable_antiresonance() {
    // A plant with a deep anti-resonance the flat target cannot reach: the measured PSD
    // is stuck FAR below the reference in the null, breaching the abort LOW band -> the
    // envelope engine aborts with a trip recorded. (This is the "drop the plant gain"
    // scenario: a band the drive physically cannot fill.)
    let mut plant = demo_plant();
    plant.resonances = vec![(200.0, 8.0), (550.0, 8.0)];
    plant.feedthrough = 5e-3; // deep inter-mode null around ~375 Hz
    plant.cfg.band_lo_hz = 120.0;
    plant.cfg.band_hi_hz = 640.0;
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(flat_profile(1.0e-2), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    // A tight LOW abort band so the un-fillable null breaches abort_lo.
    cfg.alarm_db = (3.0, 12.0);
    cfg.abort_db = (6.0, 24.0);
    // Do NOT notch (we want the envelope, not a notch, to catch it) and give the loop
    // plenty of divergence headroom so the ENVELOPE is the trip, not divergence.
    cfg.divergence_loops = 1_000;
    // The narrow null breaches only a few lines, and the default line budget
    // (max_lines_out = 10, added 2026-08-03 for per-line Welch scatter at small
    // nAvg) would ride it on the alarm indicator instead. This test is about
    // the envelope CATCHING an unfillable band, so it runs with the strictest
    // budget - which is also the knob's = 1 regression coverage.
    cfg.max_lines_out = 1;

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-d")),
    )
    .expect("service builds");

    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    svc.jump_to_table_level(4).expect("jump to full"); // full level so the abort_lo bites

    let mut aborted = false;
    for _ in 0..60 {
        if !svc.loop_tick().expect("tick") {
            aborted = true;
            break;
        }
    }
    println!("(d) final state {:?}, trip {:?}", svc.state(), svc.trip());
    assert!(aborted, "envelope abort should have stopped the run");
    assert_eq!(svc.state(), RunState::Aborted);
    let trip = svc.trip().expect("trip recorded");
    assert!(
        matches!(trip.kind, hertzforge_random::TripKind::Envelope { .. }),
        "expected an envelope trip, got {:?}",
        trip.kind
    );
}

// ===========================================================================
// (e) Kinematic abort (tight accel_g on a monitor channel)
// ===========================================================================

#[test]
fn acceptance_e_kinematic_abort_on_tight_monitor_limit() {
    // A single-input-channel table (control) carrying a TIGHT tri-field accel_g limit
    // that the loop's own converged response (~2.7 gRMS for the flat 1e-2 target) blows
    // straight past. The reactive kinematic monitor reads that channel's raw telemetry
    // and trips -> immediate Aborted with a Kinematic trip.
    //
    // NOTE on the single input channel: the shared Sim engine loop resolves `n_frames`
    // from `read_input`'s frame return by dividing by the interleave width, which is
    // only frame-exact at a 1-wide interleave (the hertzforge-demo / adversarial-fixture
    // constraint). So the Sim acceptance runs a single input channel and puts the tight
    // limit on the control channel itself. The KinematicMonitor is channel-generic (it
    // monitors any enabled non-drive channel with a limit); a real multi-channel monitor
    // is a hardware-backend concern, not a control-math one.
    let plant = demo_plant();
    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort = AbortLimits {
        accel_g: Some(0.05), // tight: the converged in-band response blows past this
        ..Default::default()
    };
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    table.channels.push(ctrl);
    table.channels.push(drv);

    let mut cfg = RandomTestConfig::siso(flat_profile(1.0e-2), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (48.0, 48.0); // effectively disable the envelope; kinematic is under test

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-e")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    println!(
        "(e) coherence {:.3}, full_target_grms {:.4}",
        svc.band_coherence(),
        svc.full_target_grms()
    );
    assert!(svc.coherence_gate().expect("gate"));
    svc.jump_to_table_level(4).expect("jump to full");

    let mut aborted = false;
    for _ in 0..60 {
        if !svc.loop_tick().expect("tick") {
            aborted = true;
            break;
        }
    }
    println!("(e) final state {:?}, trip {:?}", svc.state(), svc.trip());
    assert!(aborted, "kinematic abort should stop the run");
    assert_eq!(svc.state(), RunState::Aborted);
    let trip = svc.trip().expect("trip recorded");
    match &trip.kind {
        hertzforge_random::TripKind::Kinematic {
            channel_index,
            axis,
            ..
        } => {
            assert_eq!(
                *channel_index, 1,
                "the control channel (index 1) should trip"
            );
            assert_eq!(axis, "Accel", "the accel axis should trip");
        }
        other => panic!("expected a kinematic trip, got {other:?}"),
    }
}

// ===========================================================================
// (f) AutoFirstSeconds capture per level entry + debounce
// ===========================================================================

#[test]
fn acceptance_f_auto_first_seconds_capture_per_level() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::auto_default();
    cfg.schedule.dwell_s = vec![2.0];
    cfg.full_level_duration_s = 3.0;
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.alarm_db = (6.0, 6.0);
    cfg.abort_db = (18.0, 18.0);
    cfg.shaker = Some(generous_shaker());
    cfg.moving_mass = light_mass();
    // AUTO first-1.5s capture per level, debounced 0.2 s after each level entry.
    cfg.capture = CaptureRules {
        trigger: CaptureTrigger::AutoFirstSeconds { n_s: 1.5 },
        level_settle_s: 0.2,
        periodic_save_s: None,
    };

    let data_root = std::env::temp_dir().join(format!("hertzforge-r4-f-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_root);
    let mut svc =
        build_service(cfg, table, plant, Some(data_root.clone())).expect("service builds");

    let state = svc.run_auto(3, 400).expect("auto run");
    assert_eq!(state, RunState::Stopped);

    // One capture directory per level entry (each named auto_level_...). The five-rung
    // ladder + full-level re-entry produce several level entries; assert we got at least
    // one capture dir per distinct level in the history.
    let report = svc.report_data();
    let n_levels = report.level_history.len();
    // The data root only exists once an AUTO capture has STARTED: the capture
    // preflight runs before any directory is made, and the AUTO window drops a
    // refused start. So a missing root here means "no capture ever started" - on
    // Windows that was the free-space preflight refusing - never a path-style fault.
    let dirs: Vec<_> = std::fs::read_dir(&data_root)
        .expect("data root exists (absent = no AUTO capture ever started; check the preflight)")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("auto_level_"))
        .collect();
    println!(
        "(f) {} level entries, {} capture dirs",
        n_levels,
        dirs.len()
    );
    assert!(!dirs.is_empty(), "AUTO capture produced no per-level files");
    // Each capture dir has the binary + the sidecar; the sidecar carries the run config
    // JSON (which encodes the schedule + level context as metadata).
    for d in &dirs {
        let p = d.path();
        assert!(
            p.join("telemetry.f32").exists(),
            "capture binary missing in {p:?}"
        );
        let sidecar = p.join("capture.json");
        assert!(sidecar.exists(), "capture sidecar missing in {p:?}");
        let text = std::fs::read_to_string(&sidecar).unwrap();
        assert!(
            text.contains("run_config_json"),
            "sidecar missing run_config_json in {p:?}"
        );
    }
    // The capture-start events name the AutoFirstSeconds trigger.
    let started = report.event_log.events().iter().any(|e| {
        matches!(&e.kind, hertzforge_random::service::report::EventKind::CaptureStart { trigger, .. } if trigger.contains("AutoFirstSeconds"))
    });
    assert!(started, "no AutoFirstSeconds capture-start events logged");

    let _ = std::fs::remove_dir_all(&data_root);
}

// ===========================================================================
// (g) Feasibility hard-block + override path
// ===========================================================================

#[test]
fn acceptance_g_feasibility_hard_block_and_override() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    // A tiny-force shaker + a very heavy article: the force demand exceeds the rating.
    let mut shaker = ShakerRecord::new("tiny-shaker");
    shaker.force_random = Some(RatingValue::rms(100.0)); // lbf, tiny
    shaker.armature_mass_lb = Some(50.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.shaker = Some(shaker);
    cfg.moving_mass = MovingMass {
        armature_lb: 50.0,
        slip_table_lb: 0.0,
        drive_adapter_lb: 0.0,
        fixturing_lb: 0.0,
        test_article_lb: 5_000.0, // overweight article -> huge force demand
    };

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-g")),
    )
    .expect("service builds");

    let report = svc.assess_feasibility().expect("feasibility runs");
    assert!(
        report.exceeded(),
        "overweight article should exceed the tiny force rating"
    );
    assert_eq!(svc.state(), RunState::FeasibilityBlocked);

    // Arming WITHOUT an override is refused.
    let refused = svc.arm();
    assert!(
        refused.is_err(),
        "arm() must be refused when feasibility Exceeded"
    );
    assert_eq!(svc.state(), RunState::FeasibilityBlocked);

    // The override path logs the ack and arms.
    svc.arm_with_override("Engineering waiver 2026-07-04: fixture derate, run at reduced level.")
        .expect("override arms");
    assert_eq!(svc.state(), RunState::SysId);
    let fz = svc.feasibility().expect("feasibility digest");
    assert!(fz.override_ack.as_deref().unwrap_or("").contains("waiver"));
    // The event log carries the override ack verbatim.
    let logged = svc.report_data().event_log.events().iter().any(|e| {
        matches!(&e.kind, hertzforge_random::service::report::EventKind::OverrideAck { ack } if ack.contains("waiver"))
    });
    assert!(logged, "override ack not logged");
}

// ===========================================================================
// (h) MANUAL stepping honors +/- 1 dB + jump-to-table-level
// ===========================================================================

#[test]
fn acceptance_h_manual_stepping_and_jump() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -12.0,
        table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        dwell_s: vec![1.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-h")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    assert_eq!(svc.state(), RunState::Engaged);
    assert!(
        (svc.schedule_level_db() - (-12.0)).abs() < 1e-9,
        "engages at -12 dB"
    );

    // Run a couple of ticks at engage.
    for _ in 0..2 {
        svc.loop_tick().expect("tick");
    }

    // +1 dB step.
    let lvl = svc.step_level(1.0).expect("step +1");
    assert!((lvl - (-11.0)).abs() < 1e-9, "step +1 dB gave {lvl}");
    svc.loop_tick().expect("tick");

    // -1 dB step back.
    let lvl = svc.step_level(-1.0).expect("step -1");
    assert!((lvl - (-12.0)).abs() < 1e-9, "step -1 dB gave {lvl}");

    // Step below the table floor clamps at -12.
    let lvl = svc.step_level(-5.0).expect("step -5 clamps");
    assert!(
        (lvl - (-12.0)).abs() < 1e-9,
        "step below floor clamps at -12, got {lvl}"
    );

    // Jump to table level 2 (-6 dB).
    let lvl = svc.jump_to_table_level(2).expect("jump 2");
    assert!((lvl - (-6.0)).abs() < 1e-9, "jump-to-level 2 gave {lvl}");
    assert_eq!(
        svc.state(),
        RunState::Engaged,
        "-6 dB is below full -> Engaged"
    );

    // Jump to the full rung (index 4 = 0 dB) -> FullLevel.
    let lvl = svc.jump_to_table_level(4).expect("jump 4");
    assert!((lvl - 0.0).abs() < 1e-9, "jump-to-full gave {lvl}");
    assert_eq!(svc.state(), RunState::FullLevel, "full rung -> FullLevel");

    // The level history recorded each manual change (auto = false).
    let report = svc.report_data();
    let manual_entries = report.level_history.iter().filter(|h| !h.auto).count();
    assert!(
        manual_entries >= 5,
        "manual level changes not all recorded ({manual_entries})"
    );
    // A sanity check that pacing works: a few loops at full stay under the (wide) abort.
    for _ in 0..5 {
        if !svc.loop_tick().expect("tick") {
            break;
        }
    }
    assert_ne!(
        svc.state(),
        RunState::Aborted,
        "healthy manual run should not abort"
    );

    // Sanity that the grms helper + report line up (no NaN).
    let _ = grms(&LineSpectrum {
        f0_hz: 0.0,
        df_hz: demo_plant().df(),
        psd: report.target_psd.psd.clone(),
    });
}

// ===========================================================================
// (i) Drive-Vrms abort
// ===========================================================================

#[test]
fn acceptance_i_drive_vrms_abort() {
    // A drive-Vrms ABORT threshold set BELOW the drive the engage level synthesizes:
    // the drive clamp ceiling is high (so the clamp does not mask the breach), but the
    // abort threshold is tiny, so the first loop's realized drive Vrms breaches it ->
    // Aborted with a DriveVrms trip (real on Path B / Sim).
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0; // high: not the clamp under test
    cfg.drive_abort_vrms = Some(1.0e-4); // tiny: the engage drive Vrms blows past this
    cfg.abort_db = (48.0, 48.0); // disable the envelope
    cfg.divergence_loops = 1_000; // do not let divergence pre-empt the drive abort

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-i")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));

    let mut aborted = false;
    for _ in 0..10 {
        if !svc.loop_tick().expect("tick") {
            aborted = true;
            break;
        }
    }
    println!("(i) final state {:?}, trip {:?}", svc.state(), svc.trip());
    assert!(aborted, "drive-Vrms abort should stop the run");
    assert_eq!(svc.state(), RunState::Aborted);
    let trip = svc.trip().expect("trip recorded");
    assert!(
        matches!(trip.kind, hertzforge_random::TripKind::DriveVrms { .. }),
        "expected a DriveVrms trip, got {:?}",
        trip.kind
    );
}

// ===========================================================================
// (j) Divergence abort (divergence caller contract)
// ===========================================================================

#[test]
fn acceptance_j_divergence_abort_caller_contract() {
    // The divergence abort is the SERVICE's caller-side contract: the
    // law reports per-bin divergent bins, the service counts consecutive loops with a
    // non-empty divergent set and aborts at `divergence_loops`. Force it with a tiny
    // per-iteration correction clamp against a shaped target the loop cannot converge
    // fast enough: many trusted bins clamp at the max EVERY loop, so their consecutive-
    // clamp streaks reach `divergence_n` and stay divergent for `divergence_loops`
    // consecutive loops -> Aborted with a Divergence trip.
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (48.0, 48.0); // disable the envelope
                                 // Tiny per-iter clamp + short streak/loop thresholds: the loop cannot flatten the
                                 // shaped profile within the clamp, so trusted bins clamp every loop.
    cfg.hinv.max_correction_db_per_iter = 0.02;
    cfg.hinv.divergence_n = 2;
    cfg.divergence_loops = 3;

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-j")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    svc.jump_to_table_level(4).expect("jump to full");

    let mut aborted = false;
    for _ in 0..40 {
        if !svc.loop_tick().expect("tick") {
            aborted = true;
            break;
        }
    }
    println!("(j) final state {:?}, trip {:?}", svc.state(), svc.trip());
    assert!(aborted, "divergence abort should stop the run");
    assert_eq!(svc.state(), RunState::Aborted);
    let trip = svc.trip().expect("trip recorded");
    assert!(
        matches!(trip.kind, hertzforge_random::TripKind::Divergence { .. }),
        "expected a Divergence trip, got {:?}",
        trip.kind
    );
}

// ===========================================================================
// (k) bidirectional LIVE auto/manual takeover is level-neutral
// ===========================================================================

/// The operator's spec (2026-08-02): "when i start a test in auto, i should be
/// able to switch to manual while the test is running ... the operator needs to
/// be able to interrupt auto to take over, and then switch back to auto if
/// desired." Auto -> Manual freezes the schedule where it is (no level change,
/// dwell stops mattering); Manual -> Auto re-engages from the operator's
/// CURRENT level (nearest rung at-or-above, dwell restarted), never from the
/// top, and never with a level jump at the transition itself.
#[test]
fn acceptance_k_live_mode_takeover_is_level_neutral() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: -12.0,
        table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        dwell_s: vec![1.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);
    cfg.full_level_duration_s = 60.0;

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-k")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    assert_eq!(svc.state(), RunState::Engaged);

    // Let the AUTO ladder advance once (to -9) so the takeover happens mid-ladder.
    let mut guard = 0;
    while (svc.schedule_level_db() - (-9.0)).abs() > 1e-9 {
        svc.loop_tick().expect("tick");
        guard += 1;
        assert!(guard < 100, "AUTO never reached -9 dB");
    }

    // --- AUTO -> MANUAL: level-neutral, dwell frozen -------------------------
    svc.set_run_mode(RunMode::Manual).expect("auto -> manual");
    assert_eq!(svc.status().run_mode, RunMode::Manual);
    assert!(
        (svc.schedule_level_db() - (-9.0)).abs() < 1e-9,
        "takeover moved the level: {}",
        svc.schedule_level_db()
    );
    // Tick far beyond the 1 s dwell: the ladder must NOT advance.
    for _ in 0..12 {
        svc.loop_tick().expect("tick");
    }
    assert!(
        (svc.schedule_level_db() - (-9.0)).abs() < 1e-9,
        "MANUAL hold advanced anyway: {}",
        svc.schedule_level_db()
    );
    assert_eq!(svc.state(), RunState::Engaged);

    // The operator drives from here: +1 dB lands BETWEEN rungs (-8).
    let lvl = svc.step_level(1.0).expect("manual step");
    assert!((lvl - (-8.0)).abs() < 1e-9, "step gave {lvl}");

    // --- MANUAL -> AUTO between rungs: neutral now, TO the rung above next ---
    svc.set_run_mode(RunMode::Auto).expect("manual -> auto");
    assert_eq!(svc.status().run_mode, RunMode::Auto);
    assert!(
        (svc.schedule_level_db() - (-8.0)).abs() < 1e-9,
        "re-engage jumped the level: {}",
        svc.schedule_level_db()
    );
    // The dwell restarts at the takeover, runs at -8, then advances TO -6 (the
    // nearest rung above), NOT to -3.
    let mut first_advance = None;
    for _ in 0..40 {
        svc.loop_tick().expect("tick");
        if (svc.schedule_level_db() - (-8.0)).abs() > 1e-9 {
            first_advance = Some(svc.schedule_level_db());
            break;
        }
    }
    assert_eq!(
        first_advance,
        Some(-6.0),
        "resume must advance to the NEAREST rung above"
    );

    // The ladder then proceeds normally to full.
    let mut guard = 0;
    while svc.state() != RunState::FullLevel {
        svc.loop_tick().expect("tick");
        guard += 1;
        assert!(guard < 200, "ladder never reached FullLevel");
    }
    assert!((svc.schedule_level_db() - 0.0).abs() < 1e-9);

    // --- takeover AT FULL LEVEL, then re-engage from a mid-ladder level ------
    svc.set_run_mode(RunMode::Manual).expect("takeover at full");
    assert_eq!(svc.state(), RunState::FullLevel, "takeover changed state");
    let lvl = svc.step_level(-3.0).expect("step down");
    assert!((lvl - (-3.0)).abs() < 1e-9);
    assert_eq!(svc.state(), RunState::Engaged, "below full -> Engaged");
    svc.set_run_mode(RunMode::Auto)
        .expect("re-engage on a rung");
    assert!(
        (svc.schedule_level_db() - (-3.0)).abs() < 1e-9,
        "on-rung re-engage moved the level"
    );
    // On a rung: normal advance to the NEXT rung (0 dB) after the dwell.
    let mut guard = 0;
    while svc.state() != RunState::FullLevel {
        svc.loop_tick().expect("tick");
        guard += 1;
        assert!(guard < 100, "on-rung resume never advanced to full");
    }

    // The event log recorded both takeover directions.
    let notes: Vec<String> = svc
        .report_data()
        .event_log
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            hertzforge_random::service::report::EventKind::Note { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notes.iter().any(|t| t.contains("auto -> manual")),
        "takeover not logged: {notes:?}"
    );
    assert!(
        notes.iter().any(|t| t.contains("manual -> auto")),
        "re-engage not logged: {notes:?}"
    );
}

/// The between-rungs resume dwells at the OPERATOR's level under the rung
/// BELOW's dwell - not the target rung's. Bit on real hardware 2026-08-02: the
/// UI's schedule convention gives the FULL rung a zero dwell (its hold is the
/// full-level duration), so a resume from between -3 and 0 dB consulted
/// dwell_for(full) = 0 and jumped to full after ONE tick.
#[test]
fn acceptance_k2_resume_dwell_uses_the_rung_below() {
    let plant = demo_plant();
    let table = siso_table(500.0);
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -12.0,
        table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        // Per-rung dwells, ZERO on the full rung (the UI's wire convention).
        dwell_s: vec![2.0, 2.0, 2.0, 2.0, 0.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);
    cfg.full_level_duration_s = 60.0;

    let mut svc = build_service(
        cfg,
        table,
        plant,
        Some(std::env::temp_dir().join("hertzforge-r4-k2")),
    )
    .expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));

    // Drive manually up BETWEEN the -3 and 0 rungs.
    svc.step_level(10.0).expect("to -2");
    assert!((svc.schedule_level_db() - (-2.0)).abs() < 1e-9);

    // Re-engage AUTO: the dwell governing the wait at -2 must be the BELOW
    // rung's (-3 dB, 2.0 s), not the full rung's zero.
    svc.set_run_mode(RunMode::Auto).expect("re-engage");
    // Tick ONCE: with the old (target-rung) dwell of 0 s this already advanced
    // to full. It must NOT move yet.
    svc.loop_tick().expect("tick");
    assert!(
        (svc.schedule_level_db() - (-2.0)).abs() < 1e-9,
        "resume advanced on the first tick (used the target rung's zero dwell): {}",
        svc.schedule_level_db()
    );
    // It advances to the rung above (0 dB) only after the below-rung dwell.
    let mut ticks = 1;
    while (svc.schedule_level_db() - (-2.0)).abs() < 1e-9 {
        svc.loop_tick().expect("tick");
        ticks += 1;
        assert!(ticks < 60, "never advanced");
    }
    assert!(
        (svc.schedule_level_db() - 0.0).abs() < 1e-9,
        "advance went to {} not the rung above",
        svc.schedule_level_db()
    );
}

// ===========================================================================
// (l) the post-sysID review hold, and its bypass
// ===========================================================================

/// Operator: "i think we need to pause the test after the system id/frf startup
/// to allow the user to look at the data and decide if they should proceed ...
/// make this able to be bypassed (still doing auto safety checks) with a
/// toggle." The hold is a deliberate stop-and-look AFTER the safety gates have
/// passed - nothing drives until Proceed - and a bypassed run must reach
/// Engaged exactly as before.
#[test]
fn acceptance_l_review_hold_and_bypass() {
    let mk = |review: bool| {
        let plant = demo_plant();
        let table = siso_table(500.0);
        let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
        cfg.schedule = LevelSchedule {
            mode: RunMode::Manual,
            start_db: -12.0,
            table_db: vec![-12.0, -6.0, 0.0],
            dwell_s: vec![1.0],
        };
        cfg.drive_ceiling_vrms = 1_000.0;
        cfg.abort_db = (24.0, 24.0);
        let mut svc = build_service(
            cfg,
            table,
            plant,
            Some(std::env::temp_dir().join("hertzforge-r4-l")),
        )
        .expect("service builds");
        svc.set_review_after_sysid(review);
        svc.assess_feasibility().expect("feasibility");
        svc.arm().expect("arm");
        svc.run_sysid(3).expect("sysid");
        svc
    };

    // ---- BYPASSED: unchanged behaviour, straight to Engaged ----------------
    let mut fast = mk(false);
    assert!(fast.coherence_gate().expect("gate"), "bypass engages");
    assert_eq!(fast.state(), RunState::Engaged);

    // ---- HOLDING: the gate PASSES but nothing drives ----------------------
    let mut held = mk(true);
    let engaged = held.coherence_gate().expect("gate");
    assert!(!engaged, "a held run has not engaged");
    assert_eq!(held.state(), RunState::AwaitingReview);

    // The hold is not a fault: no trip, and the coherence that passed is intact.
    assert!(held.trip().is_none(), "review hold is not a trip");
    assert!(held.status().band_coherence > 0.0);

    // Driving verbs are REFUSED while held - the whole point is that nothing
    // moves until a human says so.
    assert!(
        held.loop_tick().is_err(),
        "must not tick while awaiting review"
    );
    assert!(
        held.step_level(1.0).is_err(),
        "must not step level while awaiting review"
    );

    // ---- PROCEED releases it, from the schedule's start level --------------
    held.proceed_after_review().expect("proceed");
    assert_eq!(held.state(), RunState::Engaged);
    assert!(
        (held.schedule_level_db() - (-12.0)).abs() < 1e-9,
        "proceeding engages at the START level, not wherever sysID left the drive"
    );
    held.loop_tick().expect("ticks once engaged");

    // Proceeding twice is a REFUSAL, not a silent no-op: it means the two ends
    // disagree about what state the run is in.
    assert!(held.proceed_after_review().is_err());
}

/// A loop fault is TERMINAL on the wire: `fault_from_loop` moves the
/// run to Aborted, and a dead loop can never keep reporting `engaged`.
///
/// Before this existed, the daemon's loop thread logged the fault and exited
/// while `module_state` stayed `engaged` for the rest of the session - the run
/// drove nothing, streamed nothing, and reported itself healthy, which is how
/// the 2026-08-02 dead-stream run reached the operator as a mystery instead of
/// a fault banner.
#[test]
fn t_loop_fault_publishes_aborted_not_engaged() {
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -12.0,
        table_db: vec![-12.0],
        dwell_s: vec![600.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);
    let mut svc =
        build_service(cfg, siso_table(500.0), demo_plant(), None).expect("service builds");
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    assert_eq!(svc.state(), RunState::Engaged);

    // The daemon loop thread hits an engine/backend error and reports it.
    svc.fault_from_loop("drive output stalled: test injection");

    assert_eq!(svc.state(), RunState::Aborted, "the fault is terminal");
    assert!(
        !svc.status().state.is_running(),
        "a dead loop must not report running"
    );
    // And the loop cannot be ticked back to life.
    assert!(svc.loop_tick().is_err(), "no ticks after a terminal fault");
}

// ===========================================================================
// (n) SOFT START: engage below the lowest scheduled level and ramp up, with
//     envelope enforcement relaxed until the start level is reached.
// ===========================================================================

/// The operator's ask (2026-08-03): "It should walk up with a true soft-start
/// say -9dB below whatever the lowest level in run schedule is. Abort and Alarm
/// enforcement should probably be reduced some until it reaches start level as
/// it can take big tests to settle a bit particularly on slip tables."
#[test]
fn acceptance_n_soft_start_walks_up_with_relaxed_enforcement() {
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Manual,
        start_db: -6.0,
        table_db: vec![-12.0, -6.0, 0.0],
        dwell_s: vec![600.0],
    };
    cfg.drive_ceiling_vrms = 1_000.0;
    cfg.abort_db = (24.0, 24.0);

    let mut svc =
        build_service(cfg, siso_table(500.0), demo_plant(), None).expect("service builds");
    // build_service opts out; this test is ABOUT the soft start.
    svc.set_soft_start_db(9.0);
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"));
    assert_eq!(svc.state(), RunState::Engaged);

    // Engaged 9 dB below the LOWEST rung (-12), not at the start level (-6).
    assert!(
        (svc.schedule_level_db() - (-21.0)).abs() < 1e-9,
        "soft start engages at lowest-9 dB, got {}",
        svc.schedule_level_db()
    );

    // It CLIMBS, monotonically, and never overshoots the start level.
    let mut prev = svc.schedule_level_db();
    let mut arrived = false;
    for _ in 0..400 {
        if !svc.loop_tick().expect("tick") {
            break;
        }
        let now = svc.schedule_level_db();
        assert!(now >= prev - 1e-9, "the soft start must not go backwards");
        assert!(now <= -6.0 + 1e-9, "must not overshoot the start level");
        prev = now;
        if (now - (-6.0)).abs() < 1e-9 {
            arrived = true;
            break;
        }
    }
    assert!(
        arrived,
        "soft start never reached the start level (at {prev})"
    );
}
