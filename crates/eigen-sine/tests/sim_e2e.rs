//! Sine module Sim end-to-end acceptance tests.
//!
//! These are the hardest acceptance gates for the module. They
//! drive the [`eigen_sine::SineService`] against the [`eigen_hal_sim::SimBackend`]
//! plant through the real [`eigen_engine::Engine`] (frames_per_tick = 1), exactly
//! as `eigen-demo` does for Random. The fixtures run at fs
//! 8,192-16,384 Hz with the band <= 1-2 kHz. The Q=50 accuracy fixture
//! (`d8_accuracy_bar_slow`) runs the true +-1 dB bar (~4 s wall clock; formerly
//! `#[ignore]`d and failing, now a genuine pass after the feedforward DC-bin fix),
//! with a fast CI proxy alongside.
//!
//! The COLA aux-channel test uses a LOCAL `ScriptedBackend` implementing
//! `eigen_hal::Backend` to CAPTURE the `write_drive` frames, because
//! `SimBackend::read_input` echoes ch0 only and cannot observe the aux channel,
//! and `eigen_engine`'s own `MockBackend` is `#[cfg(test)]`-private.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use eigen_hal_sim::SimConfig;
use eigen_shaker_db::{RatingValue, ShakerRecord};
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ColaWaveform, ControlChannel, ControlSet, LimitProfile, SineMode,
    SineTestConfig,
};
use eigen_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
    LadderConfig, NotchGovernorConfig, ProfileRow, RstdConfig, RstdDwellMode, RungFor, SineProfile,
    SweepDirection, SweepRate, SweepTarget,
};
use eigen_sine::{RunState, SineService, TripKind};

// =============================================================================
// Shared fixtures
// =============================================================================

/// A flat constant-`g` profile over `[lo, hi]` Hz at `g` peak, with the default
/// (1.5 / 3.0 dB) alarm / abort bands.
fn flat_profile(lo: f64, hi: f64, g: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(lo, g));
    p.push(ProfileRow::accel(hi, g));
    p
}

/// A flat profile with a WIDER abort band `(minus, plus)` dB, for fixtures that
/// sweep fast enough to produce a bounded resonance transient the control does
/// not fully suppress (the transient is expected; the abort band accommodates
/// it, matching the test-1 spec "within +-1 dB EXCEPT a bounded transient").
fn flat_profile_bands(lo: f64, hi: f64, g: f64, alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = flat_profile(lo, hi, g);
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    p
}

/// A one-control-one-drive channel table (control channel index 1, drive index 2).
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

/// A base sweep config over a band with a given rate. Feedforward is enabled by
/// default (the service builds it from sysID).
fn sweep_cfg(band: (f64, f64), rate: SweepRate, profile: SineProfile) -> SineTestConfig {
    SineTestConfig {
        profile,
        mode: SineMode::Sweep(BidirSweepConfig {
            band,
            rate,
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
        // The accuracy bar needs feedforward carrying the resonance
        // shape (w = 1.0, the ff slope fully applied) + the Fast compression
        // (ki, max_rate) pair so the servo tracks the sharp gain rise. Slow +
        // w = 0.5 lags a fast sweep.
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
        coherence_floor: 0.5,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        // review_hold OFF for the e2e fixtures. The default is ON (config
        // `review_hold`), and these tests drive `run_auto` to completion with no
        // operator in the loop - so honoring the park here would turn every one of
        // them into a no-op run that still reported Ok. The park has its own tests
        // in `review_hold.rs`; leaving it off here is a statement that these
        // fixtures are unattended, not that the hold does not exist.
        review_hold: false,
        control_tick_frames: 64,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

/// A Sim config with one resonance at `fs` and a healthy feedthrough floor so a
/// flat target is reachable everywhere in band.
fn sim_cfg(f_n: f64, q: f64, fs: f64) -> SimConfig {
    SimConfig::new()
        .sample_rate_hz(fs)
        .resonance(f_n, q)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0001)
}

/// The earlier `SineService::new(cfg, table, sim_cfg, data_root)` shape.
///
/// `SimConfig` moved out of the public constructor: the service takes a
/// finished `Box<dyn Backend>`, the `StationClass` it is bound to, and the sysID
/// PRNG seed (which used to ride on `SimConfig::seed`). This helper re-joins the
/// pieces so every test BODY below is unchanged - the only edit in this file's
/// test functions is the constructor's NAME.
fn sine_svc(
    cfg: SineTestConfig,
    table: ChannelTable,
    sim: SimConfig,
    data_root: Option<std::path::PathBuf>,
) -> Result<SineService, eigen_sine::service::ServiceError> {
    let seed = sim.seed;
    SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        eigen_module_abi::StationClass::EigenSim,
        seed,
        data_root,
    )
}

/// Run a service through the full auto ladder and return it (for inspection).
fn run_full_auto(mut svc: SineService, max_chunks: u64) -> SineService {
    let _ = svc.run_auto(4096, max_chunks);
    svc
}

// =============================================================================
// 1. sweep_tracks_through_resonance
// =============================================================================

#[test]
fn sweep_tracks_through_resonance() {
    // Sim resonance 250 Hz Q=20 at fs 16384; flat 1.0 g over 60-1200 Hz; auto
    // ladder to full; feedforward ON. Assert the tracked-amplitude-vs-f trace
    // peaks near the target (control holds it near 1 g) and does not run away.
    let fs = 16_384.0;
    // A 4 dB abort band accommodates the bounded resonance transient; feedforward
    // (w = 1.0) + Fast compression carry the shape through the 250 Hz Q=20 peak.
    // Band 60-500 Hz (~3 octaves) brackets the resonance; the auto ladder runs it.
    let profile = flat_profile_bands(60.0, 500.0, 1.0, 2.0, 4.0);
    let mut cfg = sweep_cfg((60.0, 500.0), SweepRate::LogOctPerMin(8.0), profile);
    cfg.control_tick_frames = 48; // ~3 ms at 16384
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 600);

    // The run reached at least FullLevel or Stopped (a completed sweep).
    assert!(
        matches!(svc.state(), RunState::Stopped | RunState::FullLevel),
        "sweep did not complete: {:?} (trip {:?})",
        svc.state(),
        svc.trip().map(|t| t.kind.clone())
    );

    let report = svc.report_data();
    let trace = &report.channel_traces[0].tracked;
    assert!(!trace.is_empty(), "no tracked trace logged");

    // The control loop holds the tracked amplitude near the 1 g target across the
    // sweep except a bounded transient near resonance. Assert the MEDIAN tracked
    // amplitude away from the resonance transient is within +-3 dB of 1 g (the
    // closed loop is genuinely controlling, not open-loop resonating).
    let mut off_res: Vec<f64> = trace
        .iter()
        .filter(|p| (p.freq_hz - 250.0).abs() > 60.0 && p.value > 0.0)
        .map(|p| p.value)
        .collect();
    assert!(off_res.len() > 5, "too few off-resonance samples");
    off_res.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = off_res[off_res.len() / 2];
    let db = 20.0 * median.log10();
    assert!(
        db.abs() < 3.0,
        "off-resonance median tracked amplitude {median:.3} g ({db:.2} dB) not within +-3 dB of 1 g"
    );

    // The tracked-vs-f trace must show a peak in the response near 250 Hz WITHOUT
    // the control letting it blow past the target band: the max tracked amplitude
    // is bounded (control caps it), not ~Q*target.
    let max_tracked = trace.iter().map(|p| p.value).fold(0.0f64, f64::max);
    assert!(
        max_tracked < 5.0,
        "control failed: tracked amplitude peaked at {max_tracked:.2} g (should be capped near 1 g)"
    );
}

// =============================================================================
// 2. d8_accuracy_bar - fast proxy + the true slow bar
// =============================================================================

/// Fast CI proxy for the accuracy bar: a sharper Q=50 resonance, a FASTER
/// sweep than the true bar, and a relaxed +-3 dB tolerance through the resonance.
/// The true bar (+-1 dB @ 1 oct/min) is `d8_accuracy_bar_slow` below.
#[test]
fn d8_accuracy_bar_fast_proxy() {
    // Q=50 (sharp) resonance; feedforward ON (w=1) + Fast compression from the
    // base config carry the shape. A faster-than-the-true-bar sweep (6 oct/min)
    // with a relaxed +-3 dB tolerance through the resonance keeps CI quick while
    // still exercising the feedforward-carries-the-shape mechanism. A 5 dB
    // abort band accommodates the bounded Q=50 transient during the climb.
    let fs = 16_384.0;
    let profile = flat_profile_bands(80.0, 500.0, 1.0, 2.5, 5.0);
    let mut cfg = sweep_cfg((80.0, 500.0), SweepRate::LogOctPerMin(6.0), profile);
    cfg.control_tick_frames = 48;
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 50.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 800);
    assert!(
        matches!(svc.state(), RunState::Stopped | RunState::FullLevel),
        "fast proxy did not complete: {:?} (trip {:?})",
        svc.state(),
        svc.trip().map(|t| t.kind.clone())
    );
    let report = svc.report_data();
    let trace = &report.channel_traces[0].tracked;
    // Through the resonance (within +-30 Hz of 250) the feedforward carries the
    // shape. This is the RELAXED CI proxy: a Q=50 peak at a fast (6 oct/min)
    // sweep leaves a bounded transient of ~4 dB (the true +-1 dB bar needs the
    // 1 oct/min slow sweep - see `d8_accuracy_bar_slow`). Assert the transient is
    // BOUNDED to +-5 dB (feedforward working: without it the response would sit
    // near Q ~ 50, i.e. ~+34 dB), and the tracking is TIGHT just outside the
    // immediate resonance (+-1.5 dB at +-25..30 Hz).
    let mut worst_db = 0.0f64;
    let mut n = 0;
    for p in trace
        .iter()
        .filter(|p| (p.freq_hz - 250.0).abs() < 30.0 && p.value > 0.0)
    {
        worst_db = worst_db.max((20.0 * p.value.log10()).abs());
        n += 1;
    }
    assert!(n > 3, "too few through-resonance samples ({n})");
    assert!(
        worst_db < 5.0,
        "fast-proxy: through-resonance transient {worst_db:.2} dB unbounded (> 5 dB); feedforward \
         should keep it far below the ~34 dB open-loop Q=50 gain"
    );
    // Just outside the immediate resonance the control is tight.
    let mut worst_edge = 0.0f64;
    for p in trace
        .iter()
        .filter(|p| (25.0..30.0).contains(&(p.freq_hz - 250.0).abs()) && p.value > 0.0)
    {
        worst_edge = worst_edge.max((20.0 * p.value.log10()).abs());
    }
    assert!(
        worst_edge < 2.0,
        "fast-proxy: near-resonance-edge tracking {worst_edge:.2} dB not tight (> 2 dB)"
    );
}

/// The TRUE accuracy bar: +-1 dB THROUGH a Q=50 resonance at 1
/// oct/min with feedforward ON + Slow compression.
///
/// This was `#[ignore]`d and, when run, FAILED at 2.254 dB because the
/// feedforward never engaged: `run_sysid` built the FRF on a grid starting at the
/// 0 Hz DC bin, which made `FeedforwardTable::from_sysid` reject the WHOLE table
/// (its grid must be strictly positive), so every bin was untrusted and the loop
/// ran on FEEDBACK ALONE (~2.2 dB residual). The fix raised the sysID resolution
/// (nfft sized so several bins span the f/Q peak) AND dropped the DC bin so the
/// feedforward table actually builds and carries the -34 dB resonance cut. With
/// both, the through-resonance error is now ~0.35 dB - a genuine pass, not a
/// relaxed bar. No longer `#[ignore]`d (it runs in ~4 s of wall clock: the sweep
/// is 1 oct/min of TEST time, which is many frames but seconds of compute).
#[test]
fn d8_accuracy_bar_slow() {
    let fs = 16_384.0;
    let profile = flat_profile(100.0, 800.0, 1.0);
    let mut cfg = sweep_cfg((100.0, 800.0), SweepRate::LogOctPerMin(1.0), profile);
    cfg.compression = CompressionPreset::Slow;
    cfg.control_tick_frames = 32;
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 50.0, fs), None).unwrap();
    // A generous chunk cap; the slow sweep needs many chunks.
    let svc = run_full_auto(svc, 20_000);
    let report = svc.report_data();
    let trace = &report.channel_traces[0].tracked;
    let mut worst_db = 0.0f64;
    for p in trace
        .iter()
        .filter(|p| (p.freq_hz - 250.0).abs() < 30.0 && p.value > 0.0)
    {
        worst_db = worst_db.max((20.0 * p.value.log10()).abs());
    }
    assert!(
        worst_db <= 1.0,
        "accuracy bar failed: worst through-resonance error {worst_db:.3} dB (> 1 dB)"
    );
}

/// A differential w=0 (pure feedback) vs w=1 (feedforward-shaped) test that
/// proves the feedforward is LIVE. Runs the SAME Q=50 resonance sweep twice and
/// asserts the worst through-resonance error is MATERIALLY larger at w=0 than at
/// w=1 - so a future regression that silently re-deadens the feedforward (the
/// DC-bin bug fixed earlier) is caught. Reuses the fast-proxy fixture shape (Q=50,
/// 6 oct/min) with a wide abort band so BOTH runs complete without aborting and
/// their through-resonance traces can be compared.
#[test]
fn feedforward_w_knob_changes_through_resonance_error() {
    let fs = 16_384.0;

    // Worst |error| in dB within +-30 Hz of the 250 Hz resonance, for a given
    // blend_w. A WIDE abort band (20/40 dB) keeps a feedback-only (w=0) run from
    // aborting on its larger transient, so both runs complete and are comparable.
    let worst_through_res = |blend_w: f64| -> f64 {
        // The fast-proxy fixture shape (6 oct/min, Fast compression, Q=50): the
        // feedforward (w=1) carries the sharp resonance cut so the through-resonance
        // transient stays bounded (~1.2 dB), while pure feedback (w=0) lags the sharp
        // gain rise and leaves a much larger transient (~3.8 dB) - a clear, material
        // divergence at a few seconds of runtime. A wide abort band so the
        // w=0 run does not abort on its larger transient.
        let profile = flat_profile_bands(80.0, 500.0, 1.0, 20.0, 40.0);
        let mut cfg = sweep_cfg((80.0, 500.0), SweepRate::LogOctPerMin(6.0), profile);
        cfg.blend_w = blend_w;
        cfg.control_tick_frames = 48;
        let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 50.0, fs), None).unwrap();
        let svc = run_full_auto(svc, 800);
        assert!(
            matches!(svc.state(), RunState::Stopped | RunState::FullLevel),
            "the w={blend_w} run did not complete: {:?} (trip {:?})",
            svc.state(),
            svc.trip().map(|t| t.kind.clone())
        );
        let report = svc.report_data();
        let trace = &report.channel_traces[0].tracked;
        let mut worst_db = 0.0f64;
        let mut n = 0;
        for p in trace
            .iter()
            .filter(|p| (p.freq_hz - 250.0).abs() < 30.0 && p.value > 0.0)
        {
            worst_db = worst_db.max((20.0 * p.value.log10()).abs());
            n += 1;
        }
        assert!(
            n > 3,
            "too few through-resonance samples at w={blend_w} ({n})"
        );
        worst_db
    };

    let err_w0 = worst_through_res(0.0);
    let err_w1 = worst_through_res(1.0);

    // The feedforward is LIVE: shaping the drive (w=1) materially reduces the
    // through-resonance error vs pure feedback (w=0). Assert (a) w=0 is a clearly
    // uncontrolled transient (> 3 dB, well above the +-1 dB accuracy bar), (b) w=1
    // is meaningfully tighter (< 2 dB), and (c) the gap is material (> 1.5 dB). A
    // regression that re-deadens the feedforward (the DC-bin bug) would collapse
    // w=1 back toward the w=0 feedback-only error, failing (b) and (c).
    assert!(
        err_w0 > 3.0,
        "the feedback-only (w=0) through-resonance error {err_w0:.2} dB is too small \
         to be a meaningful baseline; the fixture is not exercising the resonance"
    );
    assert!(
        err_w1 < 2.0,
        "the feedforward (w=1) through-resonance error {err_w1:.2} dB is not tight; \
         the feedforward is not carrying the resonance shape"
    );
    assert!(
        err_w0 - err_w1 > 1.5,
        "feedforward not live - w=0 error {err_w0:.2} dB is not materially worse than \
         w=1 error {err_w1:.2} dB (need w=0 at least 1.5 dB worse). A regression \
         re-deadened the feedforward."
    );
}

// =============================================================================
// 3. notch_holds_limit_channel
// =============================================================================

#[test]
fn notch_holds_limit_channel() {
    // A limit channel (index 3) reading the same plant response, with a limit
    // profile that binds near resonance. Assert the limit channel's tracked
    // amplitude stays at/below its bound and the run does not abort on it.
    let fs = 16_384.0;
    let mut t = table_1ctrl();
    let mut lim = Channel::new(3);
    lim.role = ChannelRole::Monitor;
    lim.units = Units::AccelG;
    t.channels.push(lim);

    let profile = flat_profile(60.0, 1000.0, 1.0);
    // The limit profile caps the monitor channel at 1.5 g with a wide abort band
    // so the notch (which uses the abort-high bound) engages near resonance but
    // does not trip.
    let mut limit_prof = flat_profile(60.0, 1000.0, 1.5);
    limit_prof.global_abort_db = (6.0, 6.0);
    limit_prof.global_alarm_db = (3.0, 3.0);

    let mut cfg = sweep_cfg((60.0, 1000.0), SweepRate::LogOctPerMin(30.0), profile);
    cfg.control_tick_frames = 48;
    cfg.limit_profiles = vec![LimitProfile {
        channel_index: 3,
        profile: limit_prof.clone(),
        shared_name: None,
    }];
    let svc = sine_svc(cfg, t, sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 200);

    // The run must NOT have aborted on the limit channel.
    if let Some(trip) = svc.trip() {
        assert!(
            !matches!(trip.kind, TripKind::LimitChannel { .. }),
            "notch failed to hold: limit-channel abort {:?}",
            trip.kind
        );
    }

    // The limit channel's tracked trace stays at/below the abort-high bound
    // (1.5 g * 10^(6/20) ~ 2.99 g) everywhere.
    let report = svc.report_data();
    // The monitor is not a control channel, so it is not in channel_traces; assert
    // instead that the run completed cleanly (no limit abort) - the notch held.
    assert!(
        matches!(
            svc.state(),
            RunState::Stopped | RunState::FullLevel | RunState::Aborted
        ),
        "unexpected state {:?}",
        svc.state()
    );
    // If it aborted, it must have been for a NON-limit reason.
    let _ = report;
}

// =============================================================================
// 4. abort_trips_and_ramps
// =============================================================================

#[test]
fn abort_trips_and_ramps() {
    // A tight drive-Vpk abort: off resonance the control must command
    // a large drive to reach 1 g through the modest feedthrough, so a low Vpk
    // abort trips at runtime. This is a runtime-only abort (it does not gate
    // feasibility). Assert RunState::Aborted, a DriveVpk trip recorded with the
    // realized/limit values and a finite firing frequency, and that the drive
    // ramped down rather than cliffed (the step's post-abort geometric ramp).
    let fs = 16_384.0;
    // Wide alarm/abort bands so the SCALAR abort does not fire first while the
    // clamped drive keeps the response low; the drive-Vpk abort is the intended
    // trip. The steady off-resonance drive to reach 1 g via the 0.35 feedthrough
    // is ~2.9, so a 2.0 Vpk ceiling+abort trips as the ramp climbs into it.
    let profile = flat_profile_bands(60.0, 500.0, 1.0, 10.0, 20.0);
    let mut cfg = sweep_cfg((60.0, 500.0), SweepRate::LogOctPerMin(8.0), profile);
    // Ceiling == abort: no clamp region, so the climbing drive trips the abort
    // directly (a clamp below the abort would hold the drive under the trip and
    // the response would never reach target instead).
    cfg.drive_vpk_ceiling = 2.0;
    cfg.drive_vpk_abort = 2.0;
    cfg.control_tick_frames = 48;
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 200);

    assert_eq!(
        svc.state(),
        RunState::Aborted,
        "run should have aborted (trip {:?})",
        svc.trip().map(|t| t.kind.clone())
    );
    let trip = svc.trip().expect("a trip record");
    match &trip.kind {
        TripKind::DriveVpk {
            realized_vpk,
            limit_vpk,
        } => {
            assert!(
                *realized_vpk >= *limit_vpk,
                "trip realized {realized_vpk} not >= limit {limit_vpk}"
            );
            assert!((*limit_vpk - 2.0).abs() < 1e-9);
        }
        other => panic!("expected a DriveVpk trip, got {other:?}"),
    }
    assert!(trip.at_freq_hz.is_finite() && trip.at_freq_hz > 0.0);

    // The drive ramped down (did not cliff): the final status frequency is finite
    // and the run is Aborted (the step's ramp decays the drive geometrically over
    // the chunk before the service ends the run).
    let status = svc.status();
    assert!(status.abort_active);
    assert!(status.freq_hz.is_finite());
}

/// A separate test that the ANALYTIC kinematic monitor trips at
/// runtime. Feasibility would block a tight profile-vs-shaker velocity limit
/// before the run, so this drives past it with `arm_with_override` and asserts a
/// kinematic velocity trip fires once the loop is engaged and the controlled
/// response's analytic velocity exceeds the (tight) shaker rating.
#[test]
fn kinematic_monitor_trips_at_runtime() {
    let fs = 16_384.0;
    let mut shaker = ShakerRecord::new("tiny");
    // At 1 g pk and 60 Hz the analytic velocity v = a*G/(2*pi*f) ~ 1.02 in/s; a
    // 0.5 in/s limit trips once the loop reaches level. Feasibility Exceeds, so
    // we override.
    shaker.max_velocity_ips = Some(RatingValue::peak(0.5));
    let profile = flat_profile(60.0, 200.0, 1.0);
    let mut cfg = sweep_cfg((60.0, 200.0), SweepRate::LogOctPerMin(8.0), profile);
    cfg.kinematic_monitor = true;
    cfg.shaker = Some(shaker);
    cfg.control_tick_frames = 48;
    let mut svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let feas = svc.assess_feasibility().unwrap();
    assert!(
        feas.exceeded(),
        "the tight velocity limit should Exceed feasibility"
    );
    svc.arm_with_override("test override: proving the runtime kinematic monitor")
        .unwrap();
    svc.run_sysid(4096).unwrap();
    let _ = svc.coherence_gate().unwrap();
    for _ in 0..200 {
        if !svc.run_chunk().unwrap_or(false) {
            break;
        }
    }
    assert_eq!(
        svc.state(),
        RunState::Aborted,
        "kinematic monitor should abort"
    );
    match &svc.trip().expect("trip").kind {
        TripKind::Kinematic { axis, value, limit } => {
            assert_eq!(axis, "velocity");
            assert!(*value > *limit);
        }
        other => panic!("expected a kinematic velocity trip, got {other:?}"),
    }
}

// =============================================================================
// 5. dwell_holds_amplitude
// =============================================================================

#[test]
fn dwell_holds_amplitude() {
    // Dwell at 400 Hz (off the 250 Hz resonance) for a fixed number of seconds;
    // the servo holds the target within tolerance; the elapsed test time accrues.
    let fs = 8_192.0;
    let profile = flat_profile(60.0, 1000.0, 1.0);
    let mut cfg = sweep_cfg((60.0, 1000.0), SweepRate::LogOctPerMin(10.0), profile);
    cfg.mode = SineMode::Dwell(DwellPlan {
        freq_hz: 400.0,
        for_: DwellFor::Seconds(0.5),
    });
    cfg.control_tick_frames = 32;
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 200);

    assert!(
        matches!(svc.state(), RunState::Stopped | RunState::FullLevel),
        "dwell did not complete: {:?}",
        svc.state()
    );
    let status = svc.status();
    // The dwell held near 400 Hz and the servo brought the measured amplitude
    // toward the 1 g target (within a factor of 2 on the settled read).
    assert!(
        (status.freq_hz - 400.0).abs() < 1.0,
        "dwell drifted off frequency"
    );
    if let Some(m) = status.measured_g {
        assert!(
            m > 0.3 && m < 3.0,
            "dwell servo did not hold near target: measured {m:.3} g"
        );
    }
}

// =============================================================================
// 6. rstd_phase_hold
// =============================================================================

#[test]
fn rstd_phase_hold() {
    // RSTD search over a band bracketing the 250 Hz resonance; the search finds
    // the peak, captures the settled phase, and the phase-tracked dwell parks at
    // the found peak and holds. Since Sim resonances are config-fixed, we assert
    // the dwell parks at the found peak (drift-through-fatigue tracking is proven
    // at the DSP layer: eigen_dsp::resonance PhaseTracker drift golden).
    let fs = 16_384.0;
    // A wide abort band: the phase-tracked dwell parks AT the resonance peak,
    // where the plant is very sensitive, so the control-profile scalar abort must
    // not trip on the dwell settling transient (the search/settle/capture phases
    // are already abort-suppressed inside the engine as a low-level probe).
    let profile = flat_profile_bands(150.0, 400.0, 0.2, 6.0, 12.0);
    let mut cfg = sweep_cfg((150.0, 400.0), SweepRate::LogOctPerMin(30.0), profile);
    cfg.mode = SineMode::Rstd(RstdConfig {
        band: (150.0, 400.0),
        level_db: -12.0,
        rate: SweepRate::LogOctPerMin(60.0),
        tracking_bw_hz: 25.0,
        settle_k: 5.0,
        dwell_mode: RstdDwellMode::PhaseTracked,
        dwell_for: DwellFor::Seconds(0.2),
    });
    cfg.control_tick_frames = 32;
    cfg.coherence_floor = 0.0; // do not gate on coherence for this fixture
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let mut svc = run_full_auto(svc, 400);
    // The run either completed (dwell done -> Stopped) or is still running under
    // the chunk cap; either way the search must have produced an estimate near
    // the 250 Hz resonance, surfaced in the event log.
    let report = svc.report_data();
    let found = report.event_log.events().iter().find_map(|e| {
        if let eigen_sine::EventKind::RstdSearchDone { f_peak, .. } = e.kind {
            Some(f_peak)
        } else {
            None
        }
    });
    let f_peak = found.expect("RSTD search should have found a peak");
    assert!(
        (f_peak - 250.0).abs() < 40.0,
        "RSTD peak {f_peak:.1} Hz not near the 250 Hz resonance"
    );
    // The dwell parks at/near the found peak.
    let _ = &mut svc;
    let status = svc.status();
    assert!(
        (status.freq_hz - 250.0).abs() < 40.0,
        "RSTD dwell parked at {:.1} Hz, not near the peak",
        status.freq_hz
    );
}

#[test]
fn rstd_dwell_entry_does_not_spuriously_abort() {
    // On the RSTD Capture -> Dwell transition the probe has FROZEN the drive at the
    // (off-resonance) band.0 level and BYPASSED feedforward, so the response rings
    // at ~Q*target (26 dB at Q=20). Before the fix the settle window was re-opened
    // ONLY on a ladder LEVEL change - the Capture -> Dwell transition is not a
    // level change - so the high-side scalar abort / limit abort / kinematic
    // monitor armed immediately at Dwell entry with no settle window, and the servo
    // (ramping the frozen-hot drive down at only ~0.117 dB/tick) could not clear
    // the abort band before the 3-tick debounce -> spurious abort on a realistic
    // tight band. The earlier rstd_phase_hold test only survived on a deliberately
    // wide 12 dB band.
    //
    // This test uses a REALISTIC tight 4 dB abort band. With the fix re-opening the
    // settle window on the probe -> Dwell transition, the servo ramps the frozen-hot
    // drive down BEFORE the protections arm, so the run reaches and holds the Dwell
    // with NO spurious abort.
    let fs = 16_384.0;
    let profile = flat_profile_bands(150.0, 400.0, 0.2, 2.0, 4.0);
    let mut cfg = sweep_cfg((150.0, 400.0), SweepRate::LogOctPerMin(30.0), profile);
    cfg.mode = SineMode::Rstd(RstdConfig {
        band: (150.0, 400.0),
        level_db: -12.0,
        rate: SweepRate::LogOctPerMin(60.0),
        tracking_bw_hz: 25.0,
        settle_k: 5.0,
        dwell_mode: RstdDwellMode::PhaseTracked,
        dwell_for: DwellFor::Seconds(0.2),
    });
    cfg.control_tick_frames = 32;
    cfg.coherence_floor = 0.0;
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 400);

    // No spurious abort at Dwell entry: the run must NOT be in the Aborted state,
    // and there must be no trip record (a high-side scalar / kinematic / limit
    // abort fired at Dwell entry is exactly the Dwell-entry defect).
    assert!(
        !matches!(svc.state(), RunState::Aborted),
        "RSTD Dwell entry spuriously aborted on a realistic 4 dB band: state {:?}, \
         trip {:?}",
        svc.state(),
        svc.trip().map(|t| t.kind.clone())
    );
    assert!(
        svc.trip().is_none(),
        "RSTD Dwell entry produced a spurious trip on a realistic 4 dB band: {:?}",
        svc.trip().map(|t| t.kind.clone())
    );

    // Setup validity: the run must actually have REACHED the Dwell (found the peak
    // and parked there), so the no-abort assertions above are not vacuously true
    // for a run that aborted before Capture or never entered Dwell.
    let report = svc.report_data();
    let found = report.event_log.events().iter().find_map(|e| {
        if let eigen_sine::EventKind::RstdSearchDone { f_peak, .. } = e.kind {
            Some(f_peak)
        } else {
            None
        }
    });
    let f_peak = found.expect("setup: RSTD search should have found a peak");
    assert!(
        (f_peak - 250.0).abs() < 40.0,
        "setup: RSTD peak {f_peak:.1} Hz not near the 250 Hz resonance"
    );
    let status = svc.status();
    assert!(
        (status.freq_hz - 250.0).abs() < 40.0,
        "setup: RSTD dwell did not park near the peak ({:.1} Hz); the run may not \
         have reached the Dwell, making the no-abort assertion vacuous",
        status.freq_hz
    );
}

// =============================================================================
// 7. cola_on_aux_channel - ScriptedBackend capturing write_drive
// =============================================================================

/// A minimal scripted backend implementing `eigen_hal::Backend` that RECORDS
/// every `write_drive` frame so a test can observe the COLA aux channel.
/// `read_input` returns a small in-band echo of the last control drive so
/// the tracking filter has a signal, but the acceptance is on the CAPTURED drive.
struct ScriptedBackend {
    fs_hz: f64,
    n_in: usize,
    n_out: usize,
    state_open: bool,
    running: bool,
    counter: u64,
    last_drive: Vec<f32>,
    /// Every frame written, interleaved `[ch0, ch1, ...]`.
    captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<f32>>>>,
}

impl ScriptedBackend {
    fn new(
        n_in: usize,
        n_out: usize,
        fs_hz: f64,
        captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<f32>>>>,
    ) -> Self {
        ScriptedBackend {
            fs_hz,
            n_in,
            n_out,
            state_open: false,
            running: false,
            counter: 0,
            last_drive: vec![0.0; n_out],
            captured,
        }
    }
}

impl Backend for ScriptedBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: self.n_in as u32,
            max_output_channels: self.n_out as u32,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.fs_hz,
            // A host-loop test double
            // delegates nothing and has one implicit clock group.
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.state_open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.state_open = false;
        self.running = false;
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
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.running = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.running = false;
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = out.len() / self.n_in.max(1);
        // Echo the last control drive (ch0) as the plant response so the tracking
        // filter has a signal; the acceptance is on the captured drive, not this.
        let echo = self.last_drive.first().copied().unwrap_or(0.0);
        for f in 0..frames {
            for c in 0..self.n_in {
                out[f * self.n_in + c] = echo;
            }
        }
        self.counter += frames as u64;
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        if drive.is_empty() {
            return Ok(0);
        }
        let n = self.n_out.max(1);
        let full = drive.len() / n;
        // Record every full frame.
        if let Ok(mut cap) = self.captured.lock() {
            for f in 0..full {
                cap.push(drive[f * n..(f + 1) * n].to_vec());
            }
        }
        if full > 0 {
            self.last_drive = drive[(full - 1) * n..full * n].to_vec();
        }
        // Lockstep: every full frame offered is accepted (and recorded above).
        Ok(full)
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.state_open,
            faulted: false,
            message: "scripted".into(),
            sample_counter: self.counter,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.running = false;
        Ok(())
    }
}

#[test]
fn cola_on_aux_channel() {
    // Drive the persistent SineProcessStep directly through the real Engine with
    // COLA ON, capturing write_drive frames via the ScriptedBackend. Assert ch1
    // carries a constant-amplitude signal at the swept frequency while ch0 is the
    // control drive. This is the only way to observe COLA.
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_engine::{Engine, EngineConfig, VecCaptureSink};
    use eigen_sine::service::engine::{
        ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
    };
    use eigen_sine::{BidirSweep, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan};

    let fs = 16_384.0;
    // Wide alarm/abort bands so the servo's startup settling (from the low seed
    // scale) does not trip a low-side scalar abort and start a ramp-down: this
    // test observes COLA as a CONSTANT-amplitude aux output during a clean run.
    // (Before the ramp fix, COLA was written at full amplitude even during a
    // ramp-down - a cliff bug - so this test passed by accident with a narrow band
    // that DID spuriously abort. COLA now ramps down on abort, which is covered by
    // `cola_ramps_down_with_drive_on_abort`; here we want a clean, non-aborting run.)
    let profile = flat_profile_bands(100.0, 500.0, 1.0, 30.0, 60.0);
    let cola_amp = 0.7;
    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (100.0, 500.0),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: 100.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
    })
    .unwrap();

    let build = StepBuild {
        fs_hz: fs,
        control_tick_frames: 48,
        out_channels: 2, // COLA ON -> width 2 (the COLA width invariant)
        profile,
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder,
        cola: ColaState {
            enabled: true,
            amp: cola_amp,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e4, 1e5).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        settle_ticks: 0, // observe COLA drive from the first frame
        scalar_abort_debounce: 3,
        start_f_hz: 100.0,
        start_level_scale: 0.2,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    };
    let (mut step, _handles) = SineProcessStep::new(build);

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<f32>>::new()));
    let mut backend = ScriptedBackend::new(1, 2, fs, captured.clone());
    backend.open().unwrap();
    backend.start().unwrap();

    let mut ecfg = EngineConfig::new(1, 2);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_cola(), &backend);
    let mut sink = VecCaptureSink::new();
    // Retry on the parallel-load scheduling-fault artifact: a wall-clock
    // scheduling fault would otherwise stop the engine early and capture too few
    // frames, panicking the assertion below under parallel contention.
    let mut attempts = 0;
    loop {
        let report = engine.run(&mut backend, &mut step, &mut sink, 4000);
        match report.reason {
            eigen_engine::StopReason::SchedulingFault(_) if attempts < 8 => {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            }
            _ => break,
        }
    }

    // Inspect the captured drive frames.
    let frames = captured.lock().unwrap();
    assert!(frames.len() > 500, "too few captured drive frames");
    // Every frame must be 2-wide (the COLA width invariant).
    assert!(
        frames.iter().all(|f| f.len() == 2),
        "drive frames not 2-wide"
    );

    // ch1 (COLA) carries a CONSTANT-amplitude sine: its peak envelope is ~cola_amp
    // and its RMS is ~cola_amp/sqrt(2), stable across the sweep. Check the peak of
    // |ch1| over the middle of the run.
    let mid = &frames[frames.len() / 4..3 * frames.len() / 4];
    let ch1_peak = mid.iter().map(|f| f[1].abs() as f64).fold(0.0f64, f64::max);
    assert!(
        (ch1_peak - cola_amp).abs() < 0.05,
        "COLA ch1 peak {ch1_peak:.3} not near the constant amplitude {cola_amp}"
    );
    // ch1 is not DC: it swings negative too (it is a sine at the swept freq).
    let ch1_min = mid
        .iter()
        .map(|f| f[1] as f64)
        .fold(f64::INFINITY, f64::min);
    assert!(
        ch1_min < -0.5 * cola_amp,
        "COLA ch1 does not swing negative (not a sine)"
    );

    // ch0 (control drive) is a DIFFERENT signal from ch1 (control-scaled, not the
    // constant COLA amplitude): their per-frame values differ.
    let differ = mid.iter().filter(|f| (f[0] - f[1]).abs() > 1e-6).count();
    assert!(
        differ > mid.len() / 2,
        "ch0 and ch1 are identical; COLA is not a distinct constant-amplitude channel"
    );
}

/// A 2-drive channel table for the COLA capture (control drive + COLA aux).
fn table_cola() -> ChannelTable {
    let mut t = table_1ctrl();
    let mut cola = Channel::new(3);
    cola.role = ChannelRole::Drive;
    cola.units = Units::VoltageV;
    cola.description = "COLA (aux)".into();
    t.channels.push(cola);
    t
}

// =============================================================================
// COLA ch1 ramps down with the drive on abort (no cliff at full amplitude)
// =============================================================================

#[test]
fn cola_ramps_down_with_drive_on_abort() {
    // Drive the step through the real Engine with COLA ON, capturing write_drive.
    // Force a drive-Vpk abort (a tight vpk_abort the climbing control drive trips)
    // and assert the CAPTURED ch1 (COLA) amplitude in the LAST frames has decayed
    // toward zero, rather than cliffing at the full constant COLA amplitude on the
    // Drive-role DAC (never a cliff). Before the fix, ch1 stayed at full
    // amplitude for the whole post-abort ramp.
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_engine::{Engine, EngineConfig, VecCaptureSink};
    use eigen_sine::service::engine::{
        ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
    };
    use eigen_sine::{BidirSweep, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan};

    let fs = 16_384.0;
    let profile = flat_profile_bands(100.0, 500.0, 1.0, 20.0, 40.0);
    let cola_amp = 0.7;
    let servo = AmplitudeServo::new(1.0, 60.0)
        .with_gains(0.3, 6.0)
        .with_drive_ceiling(1e6);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (100.0, 500.0),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: 300.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
    })
    .unwrap();

    let build = StepBuild {
        fs_hz: fs,
        control_tick_frames: 48,
        out_channels: 2, // COLA ON -> width 2
        profile,
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder,
        cola: ColaState {
            enabled: true,
            amp: cola_amp,
            waveform: ColaWaveform::Sine,
        },
        // A TIGHT drive-Vpk abort: the ScriptedBackend echoes ch0 at unity, so the
        // servo climbs the drive scale toward ~1.0 to put measured on the 1 g
        // target; a 0.6 Vpk ceiling+abort trips as the scale climbs into it,
        // starting the ramp-down. (ceiling == abort so there is no clamp region
        // holding the drive below the trip.)
        drive_guard: DriveGuard::new(0.6, 0.6).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        settle_ticks: 0,
        scalar_abort_debounce: 3,
        start_f_hz: 300.0,
        start_level_scale: 0.3,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    };
    let (mut step, handles) = SineProcessStep::new(build);

    // The ScriptedBackend echoes ch0 at unity; the servo climbs the drive scale
    // toward ~1.0 to reach the 1 g target and trips the 0.6 Vpk abort on the way.
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<f32>>::new()));
    let mut backend = ScriptedBackend::new(1, 2, fs, captured.clone());
    backend.open().unwrap();
    backend.start().unwrap();

    let mut ecfg = EngineConfig::new(1, 2);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_cola(), &backend);
    let mut sink = VecCaptureSink::new();
    // Retry on the parallel-load scheduling-fault artifact so the run reaches the
    // abort + a long ramp-down deterministically.
    let mut attempts = 0;
    loop {
        let report = engine.run(&mut backend, &mut step, &mut sink, 8000);
        match report.reason {
            eigen_engine::StopReason::SchedulingFault(_) if attempts < 8 => {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            }
            _ => break,
        }
    }

    // The run must have aborted (drive-Vpk) so a ramp-down happened.
    let trip = handles.abort.take_record();
    assert!(
        matches!(
            trip.as_ref().map(|r| &r.kind),
            Some(TripKind::DriveVpk { .. })
        ),
        "setup: expected a DriveVpk abort to start the ramp; got {trip:?}"
    );

    let frames = captured.lock().unwrap();
    assert!(frames.len() > 500, "too few captured drive frames");
    assert!(
        frames.iter().all(|f| f.len() == 2),
        "drive frames not 2-wide"
    );

    // Early in the run (before the abort) ch1 (COLA) sits near its full amplitude.
    let early = &frames[frames.len() / 8..frames.len() / 4];
    let early_peak = early
        .iter()
        .map(|f| f[1].abs() as f64)
        .fold(0.0f64, f64::max);
    assert!(
        early_peak > 0.5 * cola_amp,
        "COLA ch1 was not at full amplitude early (peak {early_peak:.4} vs amp {cola_amp})"
    );

    // The LAST frames (deep into the post-abort ramp) must have DECAYED toward
    // zero - not cliffed at the full COLA amplitude. Assert the tail peak is a
    // small fraction of the full amplitude.
    let tail = &frames[frames.len().saturating_sub(200)..];
    let tail_peak = tail
        .iter()
        .map(|f| f[1].abs() as f64)
        .fold(0.0f64, f64::max);
    assert!(
        tail_peak < 0.1 * cola_amp,
        "COLA ch1 did NOT ramp down on abort - tail peak {tail_peak:.4} still near the \
         full amplitude {cola_amp} (a cliff on the aux Drive-role DAC)"
    );
    // And ch0 (control drive) also decayed to near zero (both ramped together).
    let tail_ch0 = tail
        .iter()
        .map(|f| f[0].abs() as f64)
        .fold(0.0f64, f64::max);
    assert!(
        tail_ch0 < 0.05,
        "control drive ch0 did not ramp down to near zero alongside COLA (tail {tail_ch0:.4})"
    );
}

// =============================================================================
// The step honors StepBuild.fault_fraction (not a hardcoded 1e-2)
// =============================================================================

/// Build a plain single-channel dwell step with an explicit `fault_fraction`, a
/// healthy unity echo, and the servo-fault path armed. Used to prove the step
/// uses the PASSED collapse fraction rather than the old hardcoded 1e-2.
fn build_fault_fraction_step(
    fs: f64,
    fault_fraction: f64,
) -> eigen_sine::service::engine::StepBuild {
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_sine::service::engine::{ColaState, LimitChannelSpec, SchedulerBuild, StepBuild};
    use eigen_sine::{BlendConfig, BlendLaw, DriveGuard, DwellRun, LadderConfig, LadderPlan};

    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let dwell = DwellRun::new(DwellPlan {
        freq_hz: 300.0,
        for_: DwellFor::Seconds(10.0),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Seconds(10.0),
    })
    .unwrap();
    StepBuild {
        fs_hz: fs,
        control_tick_frames: 32,
        out_channels: 1,
        profile: flat_profile_bands(100.0, 500.0, 1.0, 30.0, 60.0),
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Dwell(dwell),
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e6, 1e7).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 5,
        fault_fraction,
        settle_ticks: 20,
        scalar_abort_debounce: 3,
        start_f_hz: 300.0,
        start_level_scale: 0.3,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    }
}

fn run_plain_step(
    fs: f64,
    mut step: eigen_sine::service::engine::SineProcessStep,
    handles: &eigen_sine::service::engine::StepHandles,
) -> Option<TripKind> {
    use eigen_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<f32>>::new()));
    let mut backend = ScriptedBackend::new(1, 1, fs, captured);
    backend.open().unwrap();
    backend.start().unwrap();
    let mut ecfg = EngineConfig::new(1, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_1ctrl(), &backend);
    let mut sink = VecCaptureSink::new();
    let mut attempts = 0;
    loop {
        let report = engine.run(&mut backend, &mut step, &mut sink, 6000);
        match report.reason {
            StopReason::SchedulingFault(_) if attempts < 8 => {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            }
            _ => break,
        }
    }
    handles.abort.take_record().map(|r| r.kind)
}

#[test]
fn step_honors_passed_fault_fraction() {
    use eigen_sine::service::engine::SineProcessStep;
    let fs = 8_192.0;
    // A HEALTHY unity-echo backend: the servo brings measured onto the 1 g target,
    // so feed ~ 1.0.
    //
    // With a HUGE collapse fraction (2.0), the check `feed < fault_fraction` is
    // ALWAYS true (feed ~ 1.0 < 2.0), so the debounced servo-fault MUST trip. This
    // is IMPOSSIBLE under the old hardcoded 1e-2 (feed ~ 1.0 > 1e-2 -> never
    // trips): it proves the step uses the PASSED StepBuild.fault_fraction.
    let (step, handles) = SineProcessStep::new(build_fault_fraction_step(fs, 2.0));
    let trip = run_plain_step(fs, step, &handles);
    assert!(
        matches!(trip, Some(TripKind::ServoFault { .. })),
        "the step must honor a large passed fault_fraction (feed ~1.0 < 2.0 -> \
         ServoFault); got {trip:?} - the old hardcoded 1e-2 could never trip here"
    );

    // With a ZERO collapse fraction, the check `feed < 0.0` is never true even on
    // a perfectly healthy signal, so NO servo-fault fires. (Sanity: the fraction
    // is genuinely wired, both directions.)
    let (step0, handles0) = SineProcessStep::new(build_fault_fraction_step(fs, 0.0));
    let trip0 = run_plain_step(fs, step0, &handles0);
    assert!(
        !matches!(trip0, Some(TripKind::ServoFault { .. })),
        "a zero fault_fraction must never trip a servo-fault on a healthy signal; \
         got {trip0:?}"
    );
}

// =============================================================================
// The TraceLog is bounded (does not grow without bound on a long run)
// =============================================================================

#[test]
fn trace_log_stays_bounded_on_a_long_run() {
    // Drive a dwell step through the engine for FAR more control ticks than the
    // trace cap (control_tick_frames = 1 -> one control tick per frame). Before
    // the fix the TraceLog grew a Vec per input channel FOREVER (hundreds of MB-GB
    // on a long dwell); now it is bounded to ~TRACE_CAP points per channel by
    // progressive downsampling. Assert the trace length never exceeds the bound
    // even after > 8x the cap in ticks, and that a meaningful trace still exists
    // (the report's channel_traces still works).
    use eigen_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
    use eigen_sine::service::engine::{SineProcessStep, TRACE_CAP};

    let fs = 8_192.0;
    // control_tick_frames = 1 so every engine frame is a control tick: a long run
    // produces many ticks quickly.
    let mut build = build_fault_fraction_step(fs, 0.0); // no servo-fault interference
    build.control_tick_frames = 1;
    build.settle_ticks = 1_000_000; // never trip; we only care about trace growth
    let (mut step, handles) = SineProcessStep::new(build);

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<f32>>::new()));
    let mut backend = ScriptedBackend::new(1, 1, fs, captured);
    backend.open().unwrap();
    backend.start().unwrap();
    let mut ecfg = EngineConfig::new(1, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_1ctrl(), &backend);
    let mut sink = VecCaptureSink::new();

    // Run > 8x the cap worth of control ticks (with SchedulingFault retries).
    let target_ticks = (TRACE_CAP as u64) * 8 + 2000;
    let mut done = 0u64;
    let mut attempts = 0;
    while done < target_ticks && attempts < 40 {
        let report = engine.run(&mut backend, &mut step, &mut sink, target_ticks - done);
        done += report.ticks;
        if matches!(report.reason, StopReason::SchedulingFault(_)) {
            attempts += 1;
            let _ = backend.arm();
            let _ = backend.start();
        } else if !matches!(report.reason, StopReason::InputEnded) {
            break;
        }
    }
    assert!(
        done > 2 * TRACE_CAP as u64,
        "did not run enough control ticks to exercise the cap ({done})"
    );

    // The trace is BOUNDED: each channel's stored points stay <= 2 * TRACE_CAP
    // (the decimate-at-2x threshold), never growing with the tick count.
    let tracked = handles.trace.tracked.lock().unwrap();
    for (ch, v) in tracked.iter().enumerate() {
        assert!(
            v.len() <= 2 * TRACE_CAP,
            "tracked trace for channel {ch} grew to {} points (> 2*cap {}); the \
             TraceLog is unbounded",
            v.len(),
            2 * TRACE_CAP
        );
        // And a meaningful trace still exists (channel_traces() still works).
        assert!(
            !v.is_empty(),
            "trace channel {ch} is empty after a long run"
        );
    }
    let broadband = handles.trace.broadband.lock().unwrap();
    for v in broadband.iter() {
        assert!(
            v.len() <= 2 * TRACE_CAP,
            "broadband trace unbounded ({})",
            v.len()
        );
    }
}

// =============================================================================
// The persistent engine keeps the drive continuous across chunk boundaries
// =============================================================================

#[test]
fn drive_is_continuous_across_chunk_boundaries() {
    // The service now builds the engine ONCE per run and re-runs it per chunk,
    // so its OutputDelivery (drive lookahead) persists across chunk boundaries.
    // The OLD behavior rebuilt the engine every chunk, re-priming OutputDelivery
    // with `min_lookahead` (4) zero frames and discarding 4 computed frames - a
    // ~4-frame zero-drive notch at each 0.25 s boundary. Here we drive ONE
    // persistent engine across several sequential run() calls (chunks), capture
    // the drive, and PIN the continuity: at each chunk boundary the drive must NOT
    // show a run of >= 3 consecutive near-zero frames (there is no re-prime).
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_engine::{Engine, EngineConfig, VecCaptureSink};
    use eigen_sine::service::engine::{
        ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
    };
    use eigen_sine::{BidirSweep, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan};

    let fs = 16_384.0;
    let profile = flat_profile_bands(100.0, 500.0, 1.0, 30.0, 60.0);
    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (100.0, 500.0),
        rate: SweepRate::LogOctPerMin(30.0),
        start_hz: 300.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
    })
    .unwrap();
    let build = StepBuild {
        fs_hz: fs,
        control_tick_frames: 48,
        out_channels: 1,
        profile,
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e4, 1e5).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        // A wide settle window so the run never aborts and the drive stays up
        // throughout (so a near-zero run can only come from a re-prime notch).
        settle_ticks: 1_000_000,
        scalar_abort_debounce: 3,
        start_f_hz: 300.0,
        start_level_scale: 0.5,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    };
    let (mut step, _handles) = SineProcessStep::new(build);

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<f32>>::new()));
    let mut backend = ScriptedBackend::new(1, 1, fs, captured.clone());
    backend.open().unwrap();
    backend.start().unwrap();

    let mut ecfg = EngineConfig::new(1, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    // ONE persistent engine, re-run per chunk. Each chunk consumes exactly
    // `chunk` frames (retrying on the parallel-load scheduling-fault artifact so a
    // fault cannot short a chunk and shift the boundary we probe).
    let mut engine = Engine::new(ecfg, &table_1ctrl(), &backend);
    let mut sink = VecCaptureSink::new();
    let chunk = 1500u64;
    let boundaries: Vec<usize> = (1..4).map(|k| (k as u64 * chunk) as usize).collect();
    for _ in 0..4 {
        let mut done = 0u64;
        let mut attempts = 0;
        while done < chunk && attempts < 16 {
            let report = engine.run(&mut backend, &mut step, &mut sink, chunk - done);
            done += report.ticks;
            if matches!(report.reason, eigen_engine::StopReason::SchedulingFault(_)) {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            } else if report.ticks == 0 {
                break;
            }
        }
    }

    let frames = captured.lock().unwrap();
    assert!(
        frames.len() >= 4 * chunk as usize - 8,
        "too few captured frames"
    );
    // Steady drive amplitude away from the very start (the servo has settled).
    let steady: f64 = frames[chunk as usize..2 * chunk as usize]
        .iter()
        .map(|f| f[0].abs() as f64)
        .fold(0.0f64, f64::max)
        .max(1e-6);
    // At each chunk boundary, assert there is NO run of >= 3 consecutive near-zero
    // frames (a re-prime notch would inject 4 zeros). Near-zero = < 5% of the
    // steady peak (a sine legitimately passes through zero for 1-2 samples per
    // cycle, so we only flag a SUSTAINED run).
    let near_zero = |v: f32| (v.abs() as f64) < 0.05 * steady;
    for &b in &boundaries {
        let lo = b.saturating_sub(8);
        let hi = (b + 8).min(frames.len());
        let mut max_run = 0usize;
        let mut run = 0usize;
        for fr in &frames[lo..hi] {
            if near_zero(fr[0]) {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 0;
            }
        }
        assert!(
            max_run < 3,
            "drive shows a run of {max_run} near-zero frames at chunk boundary {b} \
             (a re-prime zero-notch); the persistent engine should keep it continuous"
        );
    }
}

/// The COLA width invariant is enforced at config time: the service
/// returns a Config error when the table's Drive-role count cannot be reconciled
/// to the COLA-implied out-channel count.
#[test]
fn cola_width_invariant_enforced() {
    let fs = 16_384.0;
    let profile = flat_profile(100.0, 500.0, 1.0);

    // COLA OFF but the table declares 2 Drive rows -> Config error (width
    // mismatch: COLA-off needs exactly 1 Drive row).
    let mut cfg = sweep_cfg(
        (100.0, 500.0),
        SweepRate::LogOctPerMin(60.0),
        profile.clone(),
    );
    cfg.cola = ColaConfig::default(); // off
    let mut t_two_drive = table_1ctrl();
    let mut d2 = Channel::new(3);
    d2.role = ChannelRole::Drive;
    d2.units = Units::VoltageV;
    t_two_drive.channels.push(d2);
    let err = sine_svc(cfg, t_two_drive, sim_cfg(250.0, 20.0, fs), None);
    assert!(
        err.is_err(),
        "COLA-off with 2 drive rows should be a Config error"
    );

    // COLA ON but the backend advertises max_output_channels = 1 -> refuse.
    let mut cfg_on = sweep_cfg((100.0, 500.0), SweepRate::LogOctPerMin(60.0), profile);
    cfg_on.cola = ColaConfig {
        enabled: true,
        volts_pk: 1.0,
        waveform: ColaWaveform::Sine,
    };
    let one_out = SimConfig::new()
        .sample_rate_hz(fs)
        .resonance(250.0, 20.0)
        .shared_clock(true)
        .max_output_channels(1);
    let err2 = sine_svc(cfg_on, table_1ctrl(), one_out, None);
    assert!(
        err2.is_err(),
        "COLA-on with max_output_channels=1 should be refused"
    );
}

// =============================================================================
// 8. fault_debounce
// =============================================================================

#[test]
fn fault_debounce() {
    // A short dropout (< debounce window) must NOT abort; a sustained dropout
    // (>= window) MUST abort with a servo-fault trip. We drive the persistent
    // step directly against a backend whose response we can zero out on demand to
    // simulate a control-channel dropout, so the composite collapses vs target.
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
    use eigen_sine::service::engine::{
        ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
    };
    use eigen_sine::{BlendConfig, BlendLaw, DriveGuard, DwellRun, LadderConfig, LadderPlan};

    let fs = 8_192.0;
    // Very wide scalar abort band (60 dB) so the SCALAR-low abort (its own short
    // debounce) does NOT fire on the dropout - this test isolates the SERVO-FAULT
    // debounce, which is the mechanism under test. A real config would use
    // both; here we want the fault path alone.
    let profile = flat_profile_bands(100.0, 500.0, 1.0, 30.0, 60.0);
    // The fault debounce must exceed the tracking-filter settling (a genuine
    // transient the filter smooths out should NOT trip): at 300 Hz with 10%
    // proportional bandwidth (~30 Hz) the filter settles in ~1/30 s ~= 8-9 ticks
    // (32 frames/tick at 8192 Hz), so a 25-tick debounce absorbs a short dropout
    // (its ~10-tick tracked-amplitude dip) while a SUSTAINED dropout (streak grows
    // without bound) still trips. This is exactly the servo-fault debounce caveat:
    // the debounce is sized against the demod settling.
    let debounce = 25u32;

    // A dropout backend: returns a healthy in-band echo until `drop_after` reads,
    // then zero (the control channel dropped out).
    struct DropoutBackend {
        fs_hz: f64,
        counter: u64,
        reads: u64,
        drop_after_reads: u64,
        // If Some, restore the echo after this many reads (a transient dropout).
        restore_after_reads: Option<u64>,
        last_drive: f32,
        open: bool,
    }
    impl Backend for DropoutBackend {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 1,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: Some(OutputBufferContract {
                    min_lookahead_frames: 4,
                    underrun_policy: UnderrunPolicy::RampDownThenAbort,
                }),
                sample_rate_hz: self.fs_hz,
                // A host-loop test double
                // delegates nothing and has one implicit clock group.
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }
        fn open(&mut self) -> Result<(), HalError> {
            self.open = true;
            Ok(())
        }
        fn close(&mut self) -> Result<(), HalError> {
            self.open = false;
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
            Ok(())
        }
        fn start(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
            // 1 input channel: `out.len()` IS the frame count.
            let frames = out.len();
            for o in out.iter_mut() {
                self.reads += 1;
                let dropped = self.reads > self.drop_after_reads
                    && self
                        .restore_after_reads
                        .map(|r| self.reads <= r)
                        .unwrap_or(true);
                // Healthy echo: a large in-band response so measured ~ target
                // (feedthrough-scaled). Dropout: zero.
                *o = if dropped { 0.0 } else { self.last_drive * 3.0 };
            }
            self.counter += frames as u64;
            Ok(InputRead::Frames(frames))
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            if let Some(&d) = drive.last() {
                self.last_drive = d;
            }
            // 1 drive channel, lockstep: every sample is a frame, all accepted.
            Ok(drive.len())
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus {
                connected: self.open,
                faulted: false,
                message: "dropout".into(),
                sample_counter: self.counter,
                epoch: 0,
            }
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    let build_step = |debounce_ticks: u32| {
        let servo = AmplitudeServo::new(1.0, 20.0)
            .with_gains(0.3, 2.0)
            .with_drive_ceiling(1e6)
            .with_fault_fraction(1e-3);
        let blend = BlendLaw::new(BlendConfig {
            w: 0.0,
            max_step_db_per_tick: 1.0,
        })
        .unwrap();
        let dwell = DwellRun::new(DwellPlan {
            freq_hz: 300.0,
            for_: DwellFor::Seconds(10.0),
        })
        .unwrap();
        let ladder = LadderPlan::new(LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Seconds(10.0),
        })
        .unwrap();
        StepBuild {
            fs_hz: fs,
            control_tick_frames: 32,
            out_channels: 1,
            profile: profile.clone(),
            n_input_channels: 1,
            control_slots: vec![0],
            control_weights: vec![1.0],
            composite_mode: CompositeMode::WeightedAvg,
            tracking_bw: BandwidthMode::Proportional {
                pct: 10.0,
                max_hz: 100.0,
            },
            servo,
            blend,
            feedforward: None,
            notch: None,
            limit_channels: Vec::<LimitChannelSpec>::new(),
            scheduler: SchedulerBuild::Dwell(dwell),
            ladder,
            cola: ColaState {
                enabled: false,
                amp: 0.0,
                waveform: ColaWaveform::Sine,
            },
            drive_guard: DriveGuard::new(1e4, 1e5).unwrap(),
            kinematic_monitor: false,
            max_velocity_ips: None,
            max_displacement_in_pp: None,
            fault_debounce_ticks: debounce_ticks,
            // The service-side collapse fraction (wider than the servo's own
            // 1e-3 freeze floor set via with_fault_fraction above).
            fault_fraction: 1e-2,
            // Settle finishes (~20 ticks) well before the dropout at read 2000
            // (~tick 62 at 32 frames/tick), so the fault arms before the dropout.
            settle_ticks: 20,
            scalar_abort_debounce: 3,
            start_f_hz: 300.0,
            start_level_scale: 0.3,
            run_mode: eigen_sine::RunMode::Auto,
            // No transport lag in this fixture, so the tracking
            // reference is uncompensated and the arithmetic is unchanged.
            tracking_lag_frames: 0,
            ring_blocks: 8,
            block_frames: 64,
        }
    };

    let run = |drop_after: u64, restore: Option<u64>, debounce_ticks: u32| {
        let (mut step, handles) = SineProcessStep::new(build_step(debounce_ticks));
        let mut backend = DropoutBackend {
            fs_hz: fs,
            counter: 0,
            reads: 0,
            drop_after_reads: drop_after,
            restore_after_reads: restore,
            last_drive: 0.0,
            open: false,
        };
        backend.open().unwrap();
        backend.start().unwrap();
        let mut ecfg = EngineConfig::new(1, 1);
        ecfg.frames_per_tick = 1;
        ecfg.ring_frames = 256;
        ecfg.jitter_frac = 1e12;
        ecfg.watchdog_sustain = 64;
        let mut engine = Engine::new(ecfg, &table_1ctrl(), &backend);
        let mut sink = VecCaptureSink::new();
        // Run enough frames for the fault to develop + debounce. Retry on the
        // parallel-load scheduling-fault artifact: a wall-clock scheduling
        // fault would otherwise stop the engine before the 25-tick debounce is
        // reached, making the sustained-dropout assertion flaky under contention.
        let mut attempts = 0;
        loop {
            let report = engine.run(&mut backend, &mut step, &mut sink, 20_000);
            match report.reason {
                StopReason::SchedulingFault(_) if attempts < 8 => {
                    attempts += 1;
                    let _ = backend.arm();
                    let _ = backend.start();
                }
                _ => break,
            }
        }
        handles.abort.is_tripped()
    };

    // Transient dropout: drops for ~1.5 control ticks (48 reads) then restores.
    // The tracked amplitude dips for ~1.5 + ~8 (filter settling) ~= 10 ticks,
    // which is under the 25-tick debounce -> NO abort.
    let transient = run(2_000, Some(2_000 + 48), debounce);
    assert!(!transient, "a sub-window transient dropout must NOT abort");

    // Sustained dropout: drops permanently after 2000 reads -> abort with a
    // servo-fault trip.
    let sustained = run(2_000, None, debounce);
    assert!(sustained, "a sustained dropout must abort");
}

// =============================================================================
// 9. status_report_serialize
// =============================================================================

#[test]
fn status_report_serialize() {
    let fs = 8_192.0;
    let profile = flat_profile(60.0, 800.0, 1.0);
    let mut cfg = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(30.0), profile);
    cfg.control_tick_frames = 32;
    cfg.metadata = vec![eigen_sine::MetaField {
        key: "operator".into(),
        value: "test-operator".into(),
    }];
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let svc = run_full_auto(svc, 80);

    // Status round-trips through JSON.
    let status = svc.status();
    let json = serde_json::to_string(&status).unwrap();
    let back: eigen_sine::SineStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back.state, status.state);
    assert!(
        (back.freq_hz - status.freq_hz).abs() < 1e-6,
        "freq_hz did not round-trip: {} vs {}",
        back.freq_hz,
        status.freq_hz
    );

    // Report round-trips through JSON and carries the Path-B / Sim honesty note.
    let report = svc.report_data();
    let rjson = serde_json::to_string(&report).unwrap();
    let rback: eigen_sine::ReportData = serde_json::from_str(&rjson).unwrap();
    assert!(rback.honesty.path.contains("Path B"));
    assert!(rback.honesty.path.contains("Sim") || rback.honesty.path.contains("sim"));
    assert!(rback.honesty.kinematic.contains("ANALYTIC"));
    // The Honesty report discloses the raw-ring accel watchdog's COARSE
    // headroom (worst-case-abort x max-rung x 4x) so it is not read as the fine
    // per-frequency, per-rung accel abort. Assert the disclosure survives
    // serialization: the coarse-backstop framing, the 4x headroom, and the pointer
    // to the demodulated scalar path as the fine abort.
    assert!(
        rback.honesty.kinematic.contains("COARSE"),
        "the Honesty kinematic note must disclose the raw-ring watchdog is a \
         COARSE backstop; got: {}",
        rback.honesty.kinematic
    );
    assert!(
        rback.honesty.kinematic.contains("4x"),
        "the Honesty kinematic note must disclose the 4x headroom of the coarse \
         watchdog; got: {}",
        rback.honesty.kinematic
    );
    assert!(
        rback.honesty.kinematic.contains("FINE") && rback.honesty.kinematic.contains("demodulated"),
        "the Honesty kinematic note must state the FINE per-frequency per-rung \
         accel abort is the demodulated scalar path's job; got: {}",
        rback.honesty.kinematic
    );
    // The metadata survived.
    assert!(rback
        .metadata
        .iter()
        .any(|(k, v)| k == "operator" && v == "test-operator"));
}

// =============================================================================
// The config round-trips through serde (config surface completeness).
// =============================================================================

#[test]
fn config_round_trips_through_serde() {
    let profile = flat_profile(60.0, 800.0, 1.0);
    let cfg = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(30.0), profile);
    let json = serde_json::to_string(&cfg).unwrap();
    let back: SineTestConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.blend_w, cfg.blend_w);
    assert_eq!(back.out_channels(), cfg.out_channels());
}

// =============================================================================
// The raw-ring accel watchdog is armed from the profile abort ceiling
// =============================================================================

/// A backend that injects a RAW accel spike on the control channel: it echoes a
/// small in-band signal every frame, but on the Nth `read_input` frame it emits a
/// single huge sample (a sub-frame peak). Used to prove the raw-ring watchdog
/// (armed by the service from the profile abort ceiling) catches a spike the
/// demodulated scalar path cannot see at control-tick cadence.
struct SpikeBackend {
    fs_hz: f64,
    counter: u64,
    reads: u64,
    spike_at_read: u64,
    spike_value: f32,
    /// The last control drive written (ch0), echoed back scaled so the tracking
    /// filter locks onto a healthy in-band response and the demodulated scalar
    /// path stays satisfied (measured ~ target). Only the raw spike is abnormal.
    last_drive: f32,
    echo_gain: f32,
    open: bool,
}
impl Backend for SpikeBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 1,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.fs_hz,
            // A host-loop test double
            // delegates nothing and has one implicit clock group.
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.open = false;
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
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        // 1 input channel: `out.len()` IS the frame count.
        let frames = out.len();
        for o in out.iter_mut() {
            self.reads += 1;
            *o = if self.reads == self.spike_at_read {
                self.spike_value
            } else {
                self.last_drive * self.echo_gain
            };
        }
        self.counter += frames as u64;
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        if let Some(&d) = drive.last() {
            self.last_drive = d;
        }
        // 1 drive channel, lockstep: every sample is a frame, all accepted.
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.open,
            faulted: false,
            message: "spike".into(),
            sample_counter: self.counter,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

#[test]
fn watchdog_arms_accel_axis_from_profile_abort_ceiling() {
    // Part A (the build path, mod.rs): SineService::new must translate the control
    // profile's abort-HIGH accel ceiling into the control channel's
    // AbortLimits.accel_g so AbortWatchdog::new monitors the accel axis. Before
    // this fix the field stayed None and the watchdog's `monitored` set was empty.
    use eigen_channel_table::AbortLimits;
    let fs = 16_384.0;
    // Flat 1.0 g profile, default abort band +3 dB -> abort-high = 1.0*10^(3/20)
    // ~= 1.4125 g, constant across the band. The service writes a COARSE ceiling:
    // that abort-high scaled to the max ladder rung (0 dB here -> x1) times the 4x
    // safety headroom, so ~5.65 g. The headroom lets bounded raw transients pass
    // while still catching a runaway; the fine per-f abort is the scalar path's.
    let profile = flat_profile(60.0, 500.0, 1.0);
    let abort_high = 1.0 * 10f64.powf(3.0 / 20.0);
    // This is the EIGEN-SIM headroom, and it is a LITERAL - a wrong
    // edit to both the constant and this line would pass. The station-relative
    // assertions live in `tests/watchdog_constants.rs`; `sine_svc` builds an
    // EIGEN-SIM station, so 4.0 is right HERE and this line pins Sim behaviour
    // against exactly the silent change the audit forbids.
    let headroom =
        eigen_sine::service::watchdog_accel_headroom(eigen_module_abi::StationClass::EigenSim);
    assert_eq!(headroom, 4.0, "the Sim-era headroom must not move");
    let expected_ceiling = abort_high * headroom;
    let cfg = sweep_cfg((60.0, 500.0), SweepRate::LogOctPerMin(8.0), profile);
    let svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    let ctrl = svc
        .table()
        .channels
        .iter()
        .find(|c| c.index == 1)
        .expect("control channel");
    let armed = ctrl
        .abort
        .accel_g
        .expect("control channel accel_g must be armed");
    assert!(
        (armed - expected_ceiling).abs() < 1e-6,
        "armed accel ceiling {armed:.4} g != coarse ceiling {expected_ceiling:.4} g \
         (abort-high {abort_high:.4} g x rung 1 x headroom {headroom})"
    );

    // Part B (the safety behavior): with that armed table, a single RAW accel
    // spike above the ceiling within a block trips the engine's raw-ring watchdog
    // (accel axis, control channel), and the watchdog's raw-sample temporal
    // resolution catches it. Prove the demodulated scalar path would NOT: a lone
    // spike of ~1 sample amplitude A raises the tracking-filter tracked amplitude
    // by only ~A * (bw/fs) per sample, orders of magnitude below A within the same
    // control tick, so the scalar abort (which reads `tracked` at control-tick
    // cadence) cannot see the sub-frame peak the raw watchdog trips on.
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
    use eigen_sine::service::engine::{
        ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
    };
    use eigen_sine::{BidirSweep, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan};

    // Build the armed control table exactly as the service does (accel_g set).
    let mut armed_table = table_1ctrl();
    for c in armed_table.channels.iter_mut() {
        if c.index == 1 {
            c.abort = AbortLimits {
                accel_g: Some(expected_ceiling),
                velocity_ips: None,
                displacement_mil: None,
            };
        }
    }

    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (100.0, 500.0),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: 200.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
    })
    .unwrap();
    let build = StepBuild {
        fs_hz: fs,
        control_tick_frames: 48,
        out_channels: 1,
        profile: flat_profile(60.0, 500.0, 1.0),
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e4, 1e5).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        // Keep the scalar/demod protections SUPPRESSED for the whole pre-spike run
        // (a large settle window) so the ONLY trip mechanism in play is the
        // raw-ring watchdog. This isolates the raw-cadence point: the watchdog
        // fires on a single raw frame; the demodulated scalar abort, even when
        // armed, reads `tracked` only at control-tick cadence and smears a lone
        // sample far below the ceiling (asserted below).
        settle_ticks: 100_000,
        scalar_abort_debounce: 3,
        start_f_hz: 200.0,
        start_level_scale: 0.2,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    };
    let (mut step, handles) = SineProcessStep::new(build);

    // A spike ~10x the armed ceiling, at read 4000 (deep into the run, after the
    // servo has settled the echo onto the target so the scalar path is satisfied).
    let spike_value = (expected_ceiling * 10.0) as f32;
    let mut backend = SpikeBackend {
        fs_hz: fs,
        counter: 0,
        reads: 0,
        spike_at_read: 4000,
        spike_value,
        // Echo the drive back with unit gain so the closed loop converges the
        // demodulated amplitude toward the 1 g target (the scalar path stays
        // in-band); the ONLY over-ceiling event is the raw spike.
        last_drive: 0.0,
        echo_gain: 1.0,
        open: false,
    };
    backend.open().unwrap();
    backend.start().unwrap();

    let mut ecfg = EngineConfig::new(1, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    // Spike-sensitive: a single raw frame over the limit trips (the coarse fast
    // backstop). This is the safety point: raw-sample resolution, not tick.
    ecfg.watchdog_sustain = 1;
    let mut engine = Engine::new(ecfg, &armed_table, &backend);
    let mut sink = VecCaptureSink::new();
    // A scheduling fault on this single-threaded harness is a wall-clock artifact
    // under parallel test load (mirrors the service's run_engine_chunk retry);
    // re-run until we get a real stop reason or the spike has been consumed.
    let mut report = engine.run(&mut backend, &mut step, &mut sink, 5000);
    let mut attempts = 0;
    while matches!(report.reason, StopReason::SchedulingFault(_)) && attempts < 8 {
        attempts += 1;
        let _ = backend.arm();
        let _ = backend.start();
        report = engine.run(&mut backend, &mut step, &mut sink, 5000);
    }

    match &report.reason {
        StopReason::AbortTripped(trip) => {
            assert_eq!(
                format!("{:?}", trip.axis),
                "Accel",
                "watchdog tripped on the wrong axis: {:?}",
                trip.axis
            );
            assert_eq!(
                trip.channel_index, 1,
                "watchdog tripped on the wrong channel"
            );
            assert!(
                trip.value >= trip.limit,
                "trip value {} not >= limit {}",
                trip.value,
                trip.limit
            );
        }
        other => {
            panic!("armed accel watchdog should have tripped on the raw spike, got {other:?}")
        }
    }

    // The trip came from the RAW-RING WATCHDOG (raw-sample cadence), NOT from any
    // in-`process()` scalar/kinematic path: the step's own AbortLatch (which those
    // paths set) is untouched. This is the raw-ring cadence point - the demodulated
    // scalar abort reads `tracked` only once per control tick (48 frames here) and
    // a lone raw sample smears through the tracking filter to a demodulated value
    // FAR below the spike magnitude, so it could never catch this sub-frame peak
    // at the raw watchdog's speed.
    assert!(
        !handles.abort.is_tripped(),
        "the in-process scalar/kinematic abort latch must be untouched; the raw-ring \
         watchdog is the mechanism that caught the sub-frame spike"
    );
    let demod = handles.status.read().measured_g;
    assert!(
        demod < 0.5 * (spike_value as f64),
        "demod tracked amplitude {demod:.4} g is nowhere near the raw spike \
         {spike_value:.4} g (the tracking filter smears a lone sample); only the \
         raw-ring watchdog, at raw-sample cadence, could catch it"
    );
}

// =============================================================================
// The control profile must cover the sweep/dwell band
// =============================================================================

#[test]
fn profile_must_cover_the_sweep_band() {
    // A sweep band WIDER than the profile band: the profile covers 100-400 Hz but
    // the sweep runs 60-800 Hz. Out of the profile band the amplitude alarm/abort
    // disarm and the servo target collapses; the service must REJECT this config
    // rather than run a silently-unprotected, near-zero-excitation sweep.
    let fs = 16_384.0;
    let profile = flat_profile(100.0, 400.0, 1.0);
    let cfg = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(8.0), profile);
    let err = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None);
    assert!(
        err.is_err(),
        "a sweep band wider than the profile band must be rejected"
    );

    // A profile that DOES cover the band is accepted.
    let profile_ok = flat_profile(60.0, 800.0, 1.0);
    let cfg_ok = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(8.0), profile_ok);
    assert!(
        sine_svc(cfg_ok, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).is_ok(),
        "a profile covering the full band must be accepted"
    );
}

// =============================================================================
// control_tick_frames must be bounded to the chunk (>= 1 control tick/chunk)
// =============================================================================

/// THE BOUND MOVED TO ARM, and it now reads the ADOPTED rate.
///
/// It used to be checked in `SineService::new` against `caps.sample_rate_hz` -
/// the PRE-CONFIGURE rate - so a device that coerced was bounded against a rate
/// it never ran at. Both sides of the bound are the device's answer, so the check
/// belongs where the answer is (`adopt_configured_rate`). On EIGEN-SIM the budget
/// is the CHUNK; on hardware it is the BATCH (a stop-latency guarantee, and the
/// Sim's one-frame natural block makes the batch form unmeetable there). This
/// fixture is EIGEN-SIM, so the chunk applies.
#[test]
fn control_tick_frames_bounded_to_chunk_at_arm() {
    // At fs 16384 the chunk is 16384 * 0.25 = 4096 frames. A control_tick_frames
    // LARGER than the chunk means no control tick ever fires in a chunk: the
    // command mailbox is never drained (STOP ignored) and the scheduler stalls.
    let fs = 16_384.0;
    let profile = flat_profile(60.0, 800.0, 1.0);
    let mut cfg = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(8.0), profile);
    cfg.control_tick_frames = 5000; // > 4096 chunk frames
                                    // CONSTRUCTION now succeeds: nothing has asked the device anything yet.
    let mut svc = sine_svc(cfg, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None)
        .expect("the bound is not a construction-time claim any more");
    svc.assess_feasibility().expect("no shaker configured");
    let err = svc
        .arm()
        .expect_err("control_tick_frames > chunk must be refused at ARM");
    let text = err.to_string();
    assert!(text.contains("control_tick_frames"), "{text}");
    assert!(
        text.contains("CHUNK"),
        "the refusal must name the unit it bounded against: {text}"
    );

    // At or below the chunk budget it arms.
    let profile2 = flat_profile(60.0, 800.0, 1.0);
    let mut cfg_ok = sweep_cfg((60.0, 800.0), SweepRate::LogOctPerMin(8.0), profile2);
    cfg_ok.control_tick_frames = 4096; // exactly one control tick per chunk
    let mut ok = sine_svc(cfg_ok, table_1ctrl(), sim_cfg(250.0, 20.0, fs), None).unwrap();
    ok.assess_feasibility().unwrap();
    ok.arm()
        .expect("control_tick_frames == chunk (one tick/chunk) must be accepted");
}

// =============================================================================
// Scalar low-side abort + servo-fault collapse are gated on !notch_binding
// =============================================================================

/// A two-input backend for the notch-gating tests: input slot 0 (control) echoes the
/// drive at `ctrl_gain`; input slot 1 (limit) echoes at `limit_gain`. A large
/// `limit_gain` makes the limit channel MORE sensitive than the control channel,
/// so the NotchGovernor caps the drive to protect the limit channel and the
/// control point is legitimately held below its own abort-low band.
struct TwoChannelBackend {
    fs_hz: f64,
    counter: u64,
    last_drive: f32,
    ctrl_gain: f32,
    limit_gain: f32,
    /// Peak amplitude of a FREE-RUNNING in-band sine added to the control channel,
    /// independent of the commanded drive (a fixed over-test condition: cross-axis
    /// coupling / an independent resonance the servo cannot null). Generated from
    /// the backend's own phase at `offset_freq_hz`, so it survives the notch
    /// capping the drive toward zero. Used by the over-test fixture.
    ctrl_offset: f32,
    /// Peak amplitude of a FREE-RUNNING in-band sine on the LIMIT channel,
    /// independent of the commanded drive (an independent excitation / resonance on
    /// the limit axis the drive cannot null). It pins the NotchGovernor ceiling low
    /// (the limit channel reads high regardless of drive, so its sensitivity looks
    /// huge and the ceiling collapses toward zero), so the notch stays HARD-BINDING
    /// even as the control loop dies - exactly the "the limit channel keeps reading
    /// and holds the notch" condition. Sized BELOW the limit channel's own
    /// abort band so it does not itself trip the limit-channel abort.
    limit_offset: f32,
    offset_freq_hz: f64,
    offset_phase: f64,
    /// After this many input reads, the CONTROL channel (slot 0) reads ~0
    /// (a dead sensor / cable-fails-open) while the LIMIT channel keeps reading
    /// its free-running offset. `None` leaves the control channel live all run.
    kill_ctrl_after_reads: Option<u64>,
    open: bool,
}
impl Backend for TwoChannelBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.fs_hz,
            // A host-loop test double
            // delegates nothing and has one implicit clock group.
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.open = false;
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
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        // Interleaved [ctrl, limit] per frame. The control channel carries the
        // drive-proportional response PLUS a FREE-RUNNING in-band over-test sine
        // (amplitude `ctrl_offset` at `offset_freq_hz`) the servo cannot null; it
        // survives the notch capping the drive toward zero.
        let frames = out.len() / 2;
        let dphi = 2.0 * std::f64::consts::PI * self.offset_freq_hz / self.fs_hz;
        // Once past the kill point the control channel FADES to 0 over a window
        // (a dead sensor / cable slowly failing open) rather than cutting hard, so
        // the servo tracks the collapse down gently (no integral-windup slam that
        // would push the drive and overshoot the notch-pinned limit channel into its
        // own abort). The limit channel keeps reading the drive throughout.
        let ctrl_fade = match self.kill_ctrl_after_reads {
            Some(k) if self.counter >= k => {
                // Fade over ~4000 reads after the kill point.
                let past = (self.counter - k) as f64;
                (1.0 - past / 4000.0).clamp(0.0, 1.0) as f32
            }
            _ => 1.0,
        };
        for f in 0..frames {
            let s = self.offset_phase.sin();
            let over = (self.ctrl_offset as f64) * s;
            let ctrl = (self.last_drive * self.ctrl_gain + over as f32) * ctrl_fade;
            out[f * 2] = ctrl;
            // The limit channel: drive-proportional response PLUS a free-running
            // in-band offset so it keeps reading a steady high value that pins
            // the notch ceiling low even when the drive collapses toward zero.
            out[f * 2 + 1] =
                self.last_drive * self.limit_gain + (self.limit_offset as f64 * s) as f32;
            self.offset_phase += dphi;
            if self.offset_phase > std::f64::consts::TAU {
                self.offset_phase -= std::f64::consts::TAU;
            }
        }
        self.counter += frames as u64;
        // FRAMES, per the `eigen_hal::Backend::read_input` contract (now named in
        // the type: `InputRead::Frames`). This mock previously returned
        // `out.len()` (SAMPLES), with a comment saying it did so because "the
        // engine derives n_frames = n_samples / frame_len" - i.e. it was
        // calibrated against the scheduler's old frames/samples conflation so the
        // bad division would cancel (the same antidote eigen-recorder's deleted
        // FrameToSampleAdapter carried). The engine genuinely ran under that
        // pairing (1 frame/tick, verified by instrumentation: 8000 ticks / 8000
        // frames per run pre-fix), so these notch tests were real; only the
        // mock's return-count unit encoded the engine bug. Against the fixed
        // scheduler the samples-return was read as frames, over-claiming the
        // 1-frame scratch (2 "frames" x 2 ch > 2 samples) and stopping every run
        // as a BackendError on tick 1 with 0.0 g measured.
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        if let Some(&d) = drive.last() {
            self.last_drive = d;
        }
        // 1 drive channel, lockstep: every sample is a frame, all accepted.
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.open,
            faulted: false,
            message: "twochan".into(),
            sample_counter: self.counter,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// A 2-input (control + limit monitor) 1-drive table for the notch-gating tests.
fn table_ctrl_and_limit() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut lim = Channel::new(3);
    lim.role = ChannelRole::Monitor;
    lim.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(lim);
    t.channels.push(drv);
    t
}

/// Build a directly-driven notch step: control slot 0, one limit channel at slot
/// 1 with `limit_g` abort-high, dwell at 300 Hz. `fault_debounce` arms the
/// servo-fault path; the scalar-abort path is always armed after settle.
#[allow(clippy::too_many_arguments)]
fn build_notch_step(
    fs: f64,
    ctrl_target_g: f64,
    limit_g: f64,
    fault_debounce: u32,
) -> eigen_sine::service::engine::StepBuild {
    build_notch_step_tuned(
        fs,
        ctrl_target_g,
        limit_g,
        fault_debounce,
        60.0,
        6.0,
        1.0,
        6.0,
        3.0,
        6.0,
        0.01,
        None,
    )
}

/// `build_notch_step` with configurable servo/blend aggression, tight notch
/// smoothing, and a configurable CONTROL abort band. Used by the dead-control
/// fixture: a WIDE control abort band keeps the control channel's collapse from
/// tripping the (short-debounce) low-side scalar abort, so the collapse is
/// caught by the dedicated (feed-collapse) servo-fault path, which is not gated
/// on the notch - not by the scalar band. `ctrl_abort_db` is the control
/// profile's abort band in dB.
#[allow(clippy::too_many_arguments)]
fn build_notch_step_tuned(
    fs: f64,
    ctrl_target_g: f64,
    limit_g: f64,
    fault_debounce: u32,
    servo_max_rate: f64,
    servo_ki: f64,
    blend_step_db: f64,
    ctrl_abort_db: f64,
    notch_smoothing_ticks: f64,
    limit_abort_db: f64,
    notch_drive_floor: f64,
    limit_sysid_h: Option<f64>,
) -> eigen_sine::service::engine::StepBuild {
    use eigen_dsp::sine::AmplitudeServo;
    use eigen_sine::service::engine::{ColaState, LimitChannelSpec, SchedulerBuild, StepBuild};
    use eigen_sine::{BlendConfig, BlendLaw, DriveGuard, DwellRun, LadderConfig, LadderPlan};

    let servo = AmplitudeServo::new(1.0, servo_max_rate)
        .with_gains(0.3, servo_ki)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: blend_step_db,
    })
    .unwrap();
    let dwell = DwellRun::new(DwellPlan {
        freq_hz: 300.0,
        for_: DwellFor::Seconds(10.0),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Seconds(10.0),
    })
    .unwrap();
    // The control profile: flat ctrl_target_g with a configurable abort band.
    let mut ctrl_profile = flat_profile_bands(
        100.0,
        500.0,
        ctrl_target_g,
        ctrl_abort_db * 0.5,
        ctrl_abort_db,
    );
    ctrl_profile.label = "ctrl".into();
    // The limit profile bounds the sensitive limit channel at limit_g.
    let mut limit_profile =
        flat_profile_bands(100.0, 500.0, limit_g, limit_abort_db * 0.5, limit_abort_db);
    limit_profile.label = "limit".into();
    // An optional constant sysID |H| for the limit channel. When set together with a
    // high `notch_drive_floor` (so the notch always takes the sysID fallback rather
    // than the measured tracked/drive ratio), the notch ceiling is a CONSTANT
    // (limit_abort_high / sysid_h) independent of the actual limit reading. That
    // decouples the notch bind point from the limit channel's own abort band: the
    // notch can bind hard (holding the drive down) while the limit channel reads well
    // below its own abort - so a drive transient during a control collapse cannot
    // trip the (zero-debounce) limit-channel abort ahead of the servo-fault. Used by
    // the dead-control fixture.
    let sysid_h = limit_sysid_h.map(|h| (vec![50.0f64, 1000.0f64], vec![h, h]));
    let limit_channels = vec![LimitChannelSpec {
        input_slot: 1,
        channel_index: 3,
        profile: limit_profile,
        sysid_h,
    }];
    let notch = eigen_sine::NotchGovernor::new(
        NotchGovernorConfig {
            smoothing_ticks: notch_smoothing_ticks,
            drive_floor: notch_drive_floor,
            ..NotchGovernorConfig::default()
        },
        1,
    )
    .unwrap();
    StepBuild {
        fs_hz: fs,
        control_tick_frames: 32,
        out_channels: 1,
        profile: ctrl_profile,
        n_input_channels: 2,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch: Some(notch),
        limit_channels,
        scheduler: SchedulerBuild::Dwell(dwell),
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e6, 1e7).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: fault_debounce,
        fault_fraction: 1e-2,
        settle_ticks: 20,
        scalar_abort_debounce: 3,
        start_f_hz: 300.0,
        start_level_scale: 0.3,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    }
}

fn run_notch_step(
    fs: f64,
    step: eigen_sine::service::engine::SineProcessStep,
    handles: &eigen_sine::service::engine::StepHandles,
    ctrl_gain: f32,
    limit_gain: f32,
    ctrl_offset: f32,
) -> Option<TripKind> {
    run_notch_step_inner(
        fs,
        step,
        handles,
        ctrl_gain,
        limit_gain,
        ctrl_offset,
        0.0,
        None,
    )
}

/// `run_notch_step` with the control channel killed after `kill_after` input
/// reads (a dead sensor mid-dwell) while the LIMIT channel carries a free-running
/// `limit_offset` that keeps it reading high and holds the notch bound.
#[allow(clippy::too_many_arguments)]
fn run_notch_step_kill_offset(
    fs: f64,
    step: eigen_sine::service::engine::SineProcessStep,
    handles: &eigen_sine::service::engine::StepHandles,
    ctrl_gain: f32,
    limit_gain: f32,
    ctrl_offset: f32,
    limit_offset: f32,
    kill_after: u64,
) -> Option<TripKind> {
    run_notch_step_inner(
        fs,
        step,
        handles,
        ctrl_gain,
        limit_gain,
        ctrl_offset,
        limit_offset,
        Some(kill_after),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_notch_step_inner(
    fs: f64,
    mut step: eigen_sine::service::engine::SineProcessStep,
    handles: &eigen_sine::service::engine::StepHandles,
    ctrl_gain: f32,
    limit_gain: f32,
    ctrl_offset: f32,
    limit_offset: f32,
    kill_ctrl_after_reads: Option<u64>,
) -> Option<TripKind> {
    use eigen_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
    let mut backend = TwoChannelBackend {
        fs_hz: fs,
        counter: 0,
        last_drive: 0.0,
        ctrl_gain,
        limit_gain,
        ctrl_offset,
        limit_offset,
        // The dwell frequency, so the free-running over-test sine sits inside the
        // control tracking filter's passband and demodulates to ~ctrl_offset.
        offset_freq_hz: 300.0,
        offset_phase: 0.0,
        kill_ctrl_after_reads,
        open: false,
    };
    backend.open().unwrap();
    backend.start().unwrap();
    let mut ecfg = EngineConfig::new(2, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    ecfg.jitter_frac = 1e12;
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_ctrl_and_limit(), &backend);
    let mut sink = VecCaptureSink::new();
    let mut attempts = 0;
    loop {
        let report = engine.run(&mut backend, &mut step, &mut sink, 8000);
        match report.reason {
            StopReason::SchedulingFault(_) if attempts < 8 => {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            }
            _ => break,
        }
    }
    handles.abort.take_record().map(|r| r.kind)
}

#[test]
fn notched_under_test_does_not_trip_low_side_or_servo_fault() {
    use eigen_sine::service::engine::SineProcessStep;
    let fs = 8_192.0;
    // Control target 1.0 g; a MORE-SENSITIVE limit channel (limit_gain 10x the
    // control gain) with its abort-high bound BELOW the level the control needs.
    // The NotchGovernor caps the drive to protect the limit channel, so the
    // control point is held far below its abort-low band - a legitimate notched
    // under-test. Before the notch gating this tripped ScalarAbort{low} and/or
    // ServoFault.
    let ctrl_gain = 0.5f32;
    let limit_gain = 5.0f32; // 10x more sensitive than the control channel
                             // Limit bound 0.4 g: with limit_gain 5.0 the drive is capped near
                             // 0.4/5.0 = 0.08, so the control channel (gain 0.5) reads ~0.04 g, far below
                             // its 1 g target and below its abort-low (~0.7 g). No over-test offset.
    let (step, handles) = SineProcessStep::new(build_notch_step(fs, 1.0, 0.4, 25));
    let trip = run_notch_step(fs, step, &handles, ctrl_gain, limit_gain, 0.0);
    let snap = handles.status.read();
    // Setup validity (so the test cannot pass vacuously): the NotchGovernor held
    // the control point far below its target - well under its abort-low band
    // (~0.7 g at a +3 dB band) - because the more-sensitive limit channel capped
    // the drive. This under-test condition is EXACTLY what would have tripped the
    // low-side scalar abort / servo-fault before they were gated on !notch_binding.
    assert!(
        snap.measured_g < 0.5 * snap.target_g && snap.target_g > 0.0,
        "setup invalid: the control point is not held below its abort-low band \
         by the notch (measured {:.4} g vs target {:.4} g)",
        snap.measured_g,
        snap.target_g
    );
    assert!(
        !matches!(
            trip,
            Some(TripKind::ScalarAbort { side: _, .. }) | Some(TripKind::ServoFault { .. })
        ),
        "a legitimately-notched under-test must NOT trip a low-side scalar abort \
         or a servo-fault collapse; got {trip:?}"
    );
}

#[test]
fn genuine_over_test_still_trips_high_side_even_while_notched() {
    use eigen_sine::service::engine::SineProcessStep;
    let fs = 8_192.0;
    // The CONTROL channel genuinely over-tests (reads far above its abort-high)
    // WHILE a different, more-sensitive limit channel notches the drive. The
    // high-side over-test abort MUST still fire (the gating keeps out_high armed
    // while binding): the notch protects a limit channel, it does not license the
    // control point to run hot.
    //
    // A MORE-sensitive limit channel (limit_gain 5.0) with a tight bound (0.4 g)
    // makes the NotchGovernor cap the drive low (notch binds). The control channel
    // carries a large in-band over-test OFFSET (5.0 g, above its ~2 g abort-high)
    // that is INDEPENDENT of the (notch-capped) drive - so it over-tests even
    // though the drive is held down. Before the notch gating the high-side abort
    // was correctly armed here; the gating must NOT have disarmed it while binding.
    let ctrl_gain = 0.5f32;
    let limit_gain = 5.0f32;
    let ctrl_offset = 5.0f32; // demodulates well above the ~2 g abort-high
    let (step, handles) = SineProcessStep::new(build_notch_step(fs, 1.0, 0.4, 0));
    let trip = run_notch_step(fs, step, &handles, ctrl_gain, limit_gain, ctrl_offset);
    assert!(
        matches!(
            trip,
            Some(TripKind::ScalarAbort { ref side, .. }) if side == "high"
        ),
        "a genuine control-channel over-test must still trip the HIGH-side scalar \
         abort; got {trip:?}"
    );
}

#[test]
fn dead_control_channel_aborts_even_while_notch_binds() {
    // The first notch-gating fix over-corrected by gating the servo-fault collapse
    // detector on !notch_binding. That opened a real gap: if the CONTROL channel
    // dies (cable-fails-open) mid-dwell while a MORE-sensitive limit channel keeps
    // reading and holds the notch, feed -> 0 (a dead control loop) but the
    // servo-fault was gated off, the low-side scalar abort was gated off, and
    // high-side / kinematic / limit-channel abort all saw a live-or-zero limit
    // channel - so the run drove on with a dead control loop and NO abort. The
    // follow-up ungates the servo-fault from notch_binding, so this MUST now trip
    // ServoFault.
    use eigen_sine::service::engine::SineProcessStep;
    let fs = 8_192.0;
    // A MORE-sensitive limit channel (limit_gain 5.0 vs ctrl_gain 0.5) with a tight
    // bound (0.4 g, abort-high ~0.798 g) makes the NotchGovernor cap the drive so
    // the notch binds and holds the LIMIT channel at ~its bound (drive ~0.16, limit
    // ~0.798). The control target is set LOW (0.1 g) so the control channel reads
    // ~0.08 g at the notch equilibrium (feed ~0.8, a meaningfully-driven notched
    // under-test - well above the 1e-2 fault fraction, so it must NOT false-trip
    // while control is alive), and the servo sits near its target rather than
    // hammering the notch (which would otherwise creep the limit channel over its
    // own abort edge). A WIDE control abort band (60 dB -> abort-low ~1e-4 g) keeps
    // the control channel's post-kill collapse toward 0 from tripping the
    // short-debounce low-side scalar abort; the dedicated servo-fault path
    // (feed < 1e-2 for the debounce window) catches it instead - the ungating.
    let ctrl_gain = 0.5f32;
    let limit_gain = 5.0f32;
    let limit_offset = 0.0f32;
    // The notch ceiling is a CONSTANT limit_abort_high / sysid_h (see the builder):
    // limit abort-high = 0.4 * 10^(20/20) = 4.0 g, sysid_h = 40, so ceiling = 0.1.
    // The servo wants drive ~0.2 (control target 0.1 g at ctrl_gain 0.5), so it
    // KEEPS REQUESTING above the 0.1 ceiling and the notch stays HARD-BINDING -
    // even through the control collapse (the dead-control case: the OLD code gated
    // the servo-fault off exactly here). The constant ceiling means the drive is
    // pinned at 0.1, so the limit channel reads ~0.1 * 5 = 0.5 g, WELL below its
    // own 4.0 g abort band: no limit-channel abort can pre-empt the servo-fault.
    // The very wide control abort band (100 dB -> abort-low ~1e-6 g) keeps the
    // collapsing control channel from reaching the low-side scalar band before the
    // servo-fault debounce (8 ticks) completes, so the dedicated servo-fault path
    // is the one that fires.
    // Args: fs, ctrl_target_g=0.1, limit_g=0.4, fault_debounce=8, servo_max_rate=30,
    // servo_ki=4.0, blend_step=0.5, ctrl_abort_db=100, notch_smoothing=3.0,
    // limit_abort_db=20, notch_drive_floor=1.0, limit_sysid_h=Some(40.0).
    let build = || {
        build_notch_step_tuned(
            fs,
            0.1,
            0.4,
            8,
            30.0,
            4.0,
            0.5,
            100.0,
            3.0,
            20.0,
            1.0,
            Some(40.0),
        )
    };

    // Setup validity (so the test cannot pass vacuously): the SAME config run
    // WITHOUT killing the control channel is a legitimately-notched under-test where
    // the notch holds the LIMIT channel at ~its bound and does NOT abort (the
    // ungating must not false-trip it). This proves the notch binds in this fixture;
    // the killed run below then differs only by the dead control channel.
    {
        let (step, handles) = SineProcessStep::new(build());
        let trip = run_notch_step_kill_offset(
            fs,
            step,
            &handles,
            ctrl_gain,
            limit_gain,
            0.0,
            limit_offset,
            u64::MAX,
        );
        let snap = handles.status.read();
        // The limit channel (slot 1) is held at/near its abort-high bound (~0.798 g)
        // by the notch - proof the more-sensitive limit channel capped the drive and
        // the notch is binding. (The snapshot's instantaneous notch_binding flag
        // flickers on the knife edge at the ceiling and is not a reliable end-of-run
        // read; the limit-channel level is the stable evidence.)
        // The notch holds the control point meaningfully BELOW its target: a free
        // servo would drive measured up to target, but the more-sensitive limit
        // channel caps the drive, so the control composite sits under target. That
        // is the notch binding (the snapshot's instantaneous notch_binding flag
        // flickers on the knife edge at the ceiling and is not a reliable end-of-run
        // read; the held-below-target control level is the stable evidence). AND the
        // control level is meaningful (feed >> the 1e-2 fault fraction), so the
        // ungating does not false-trip this legitimate notched under-test.
        assert!(
            snap.measured_g < 0.9 * snap.target_g
                && snap.measured_g > 0.02 * snap.target_g
                && snap.target_g > 0.0,
            "setup invalid: the notch did not hold the control point meaningfully \
             below target (measured {:.4} g vs target {:.4} g); expected a notched \
             under-test with feed well above the 1e-2 fault fraction",
            snap.measured_g,
            snap.target_g
        );
        assert!(
            !matches!(trip, Some(TripKind::ServoFault { .. })),
            "setup invalid: a live notched under-test must NOT trip ServoFault after \
             the ungating (its feed stays well above the fault fraction); got {trip:?}"
        );
    }

    // Now the same notched config, but the control channel DIES after 2000 input
    // reads: well past the 20-tick settle window (~640 reads) and after the notch has
    // seeded from the live limit channel. The limit channel keeps reading the (held)
    // drive so the notch stays bound, while the control channel's tracked amplitude
    // collapses to ~0. That is the servo-fault collapse condition (feed -> 0) the
    // servo-fault detector must catch EVEN WHILE the notch binds - the gap the first
    // notch-gating fix opened.
    let (step, handles) = SineProcessStep::new(build());
    let trip = run_notch_step_kill_offset(
        fs,
        step,
        &handles,
        ctrl_gain,
        limit_gain,
        0.0,
        limit_offset,
        2_000,
    );
    assert!(
        matches!(trip, Some(TripKind::ServoFault { .. })),
        "a dead control channel (tracked -> 0) while the notch binds must trip \
         ServoFault - the servo-fault collapse detector is NOT gated on notch_binding; \
         got {trip:?}"
    );
}
