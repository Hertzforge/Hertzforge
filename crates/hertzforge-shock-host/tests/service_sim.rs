//! Sim end-to-end acceptance for `ClassicalShockService` on Path B: the
//! service closes the loop on the hertzforge-hal-sim plant through the
//! `BackendPulseFirer` seam.
//!
//! The fixtures ((g) added with the wall-clock train pacing):
//! (a) the full AUTO ladder fires the scheduled pulses, each recorded with its
//!     SRS, and a clean pulse passes tolerance at every level;
//! (b) a tolerance-violating pulse (wrong amplitude via plant gain) trips the
//!     abort;
//! (c) the feasibility gate hard-blocks an overweight article + the
//!     override-with-ack path arms anyway;
//! (d) a pulse train fires `count` pulses at the planned spacing;
//! (e) gunfire spacing is `60 / rate` (Procedure IV) and a rate below the
//!     rate-model floor is REFUSED;
//! (f) the DriveLimit scales a pulse that would over-g (predictive);
//! (g) the DEFAULT pacer is the real clock and the service actually blocks on
//!     it - the control for (d) and (e), which inject a virtual clock.

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shaker_db::{MovingMass, RatingValue, ShakerRecord};
use hertzforge_shock_common::{RateModel, ShockLevelSchedule, TrainError, TrainSpec};
use hertzforge_shock_host::service::config::GunfireSpec;
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::service::report::{EventKind, PulseOrigin};
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig, RunState, ServiceError, TripKind,
};

const FS: f64 = 5120.0;

/// A clean unity plant: pure feedthrough, no modes, no noise (the sim "shaker"
/// reproduces the drive on its inputs) - the sim_path_b.rs convention.
fn clean_backend(feedthrough: f64) -> SimBackend {
    SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .noise_level(0.0)
        .feedthrough(feedthrough)
        .shared_clock(true)
        .build()
}

/// The sim_path_b geometry: 20 g / 11 ms half-sine with long/low rectangular
/// shoulders (~-8% of peak) that fit INSIDE the MIL surround band.
fn spec_20g_11ms() -> ClassicalPulseSpec {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    spec
}

fn service(
    cfg: ClassicalTestConfig,
    feedthrough: f64,
    tag: &str,
) -> ClassicalShockService<BackendPulseFirer<SimBackend>> {
    // Fixture table (deprecated outside tests).
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let firer = BackendPulseFirer::new(
        clean_backend(feedthrough),
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join(format!(
            "hertzforge-shock-host-{tag}-{}",
            std::process::id()
        ))),
    )
    .unwrap()
}

/// A big shaker the 20 g pulse easily fits: 100k lbf shock force, generous
/// velocity/stroke ratings.
fn big_shaker() -> ShakerRecord {
    let mut s = ShakerRecord::new("big");
    s.force_shock = Some(RatingValue::peak(100_000.0));
    s.max_velocity_ips = Some(RatingValue::peak(400.0));
    s.max_displacement_in_pp = Some(RatingValue::peak(3.0));
    s
}

// ---------------------------------------------------------------------------
// (a) full AUTO ladder on the Sim plant
// ---------------------------------------------------------------------------

#[test]
fn a_full_auto_ladder_records_every_pulse_with_srs_and_passes_tolerance() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.shaker = Some(big_shaker());
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 50.0,
        ..MovingMass::default()
    };
    cfg.capture.auto_start = true;

    let mut s = service(cfg, 1.0, "a");
    // The AUTO ladder really waits since 2026-08-08: five rungs at the conservative
    // 1.0 s floor is 4 s of wall time. `h_the_auto_ladder_paces_on_the_real_clock`
    // is the control that proves the shipping default is not this double.
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    let feas = s.assess_feasibility().unwrap();
    assert!(!feas.exceeded(), "{feas:?}");
    s.arm().unwrap();
    assert!(s.status().capturing, "capture auto-started at arm");

    let end = s.run_auto(100).unwrap();
    assert_eq!(end, RunState::Stopped);

    let report = s.report_data();
    // Default AUTO ladder: [-12, -9, -6, -3, 0] dB, one pulse per level.
    assert_eq!(report.pulses.len(), 5);
    let levels: Vec<f64> = report.pulses.iter().map(|r| r.scheduled_level_db).collect();
    assert_eq!(levels, vec![-12.0, -9.0, -6.0, -3.0, 0.0]);

    // Every pulse recorded in the store with a computed SRS on the run grid.
    assert_eq!(s.pulse_store().len(), 5);
    let n_bands = s.srs_freqs().len();
    assert!(n_bands > 0);
    for rec in s.pulse_store().records() {
        assert_eq!(rec.srs.len(), n_bands);
        assert!(rec.srs.iter().all(|v| v.is_finite() && *v > 0.0));
    }

    // Clean plant: all three tolerance checks pass at every level, and every SRS
    // band matches the commanded drive (unity plant fidelity).
    for row in &report.pulses {
        assert!(row.measured);
        assert_eq!(row.tolerance_overall, Some(true), "{row:?}");
        assert_eq!(row.envelope_pass, Some(true));
        assert_eq!(row.velocity_change_pass, Some(true));
        assert_eq!(row.substitution_pass, Some(true));
        assert_eq!(row.srs_bands_pass, row.srs_bands_total, "{row:?}");
    }
    assert_eq!(report.tolerance.checked, 5);
    assert_eq!(report.tolerance.passed, 5);

    // The measured peaks walk the ladder: 5.02 -> 20 g (unity plant).
    let peaks: Vec<f64> = report.pulses.iter().map(|r| r.peak_g.unwrap()).collect();
    for (peak, db) in peaks.iter().zip(levels.iter()) {
        let expect = 20.0 * 10f64.powf(db / 20.0);
        assert!(
            (peak - expect).abs() < 0.05,
            "peak {peak} at {db} dB (expect ~{expect})"
        );
    }

    // Feasibility digest + honesty markers travel in the report.
    let digest = report.feasibility.as_ref().unwrap();
    assert_eq!(digest.shaker_name, "big");
    assert!(digest.override_ack.is_none());
    assert!(report.honesty.trains.contains("F8"));
    assert!(report.honesty.path.contains("Path B"));
    assert!(report.capabilities.host_compensation, "Path B owns comp");

    // The capture sidecar was flushed at stop (5 pulse records written).
    assert!(s.log().events().iter().any(
        |e| matches!(e.kind, EventKind::CaptureStop { chunks_written, .. } if chunks_written >= 1)
    ));
}

// ---------------------------------------------------------------------------
// (b) tolerance abort on a wrong-amplitude pulse
// ---------------------------------------------------------------------------

#[test]
fn b_wrong_amplitude_pulse_trips_the_tolerance_abort() {
    // A 1.3x plant gain: the measured pulse is +30% in AMPLITUDE (peak), so the
    // envelope deviation is well outside the MIL +/-15% band. The velocity-change
    // error is a SMALLER +22.1%, and here is why: error_frac is the dV over the
    // STANDARD's measurement window (0.4T before the pulse to 0.1T after), and
    // that window straddles the negative compensation shoulders of the
    // host-compensated composite (rectangular, -7.8% of peak, 4T long each). The
    // gain scales the WHOLE composite, so the shoulder area inside the window
    // (also x1.3) partly offsets the main-pulse velocity area. Nominal dV is the
    // main pulse alone (no shoulders), so measured/nominal is NOT the raw 1.30
    // plant ratio: it is +22.1%, not +30%. (The naive +30% assumption ignored
    // both the compensation shoulders and the window geometry.) Either way it is
    // far outside the +/-10% dV band, so the abort is correct.
    let cfg = ClassicalTestConfig::new(spec_20g_11ms());
    let mut s = service(cfg, 1.3, "b");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    let end = s.run_auto(100).unwrap();
    assert_eq!(end, RunState::Aborted);

    let trip = s.trip().expect("trip recorded");
    match &trip.kind {
        TripKind::Tolerance {
            pulse,
            envelope_pass,
            velocity_change_pass,
            error_frac,
            ..
        } => {
            assert_eq!(*pulse, 0, "aborts on the first bad pulse");
            assert!(!envelope_pass);
            assert!(!velocity_change_pass);
            // dV error over the standard's window for the 1.3x-gained compensated
            // composite is +22.1% (see the header comment for the shoulder/window
            // reasoning), not the naive +30% plant-gain ratio.
            assert!((*error_frac - 0.221).abs() < 0.01, "dV error ~+22%");
        }
        k => panic!("wrong trip kind: {k:?}"),
    }
    assert_eq!(s.report_data().tolerance.passed, 0);
    assert!(s.status().abort_active);
    // Only the one pulse fired; the ladder never advanced.
    assert_eq!(s.report_data().pulses.len(), 1);
}

// ---------------------------------------------------------------------------
// (c) feasibility hard-block on an overweight article + override
// ---------------------------------------------------------------------------

#[test]
fn c_feasibility_hard_blocks_overweight_article_and_override_arms() {
    // force_shock / m_total = 5000 lbf / 500 lb = 10 g ceiling < 20 g demand.
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    let mut shaker = ShakerRecord::new("small");
    shaker.force_shock = Some(RatingValue::peak(5000.0));
    shaker.max_velocity_ips = Some(RatingValue::peak(400.0));
    shaker.max_displacement_in_pp = Some(RatingValue::peak(3.0));
    cfg.shaker = Some(shaker);
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 400.0,
        ..MovingMass::default()
    };

    let mut s = service(cfg, 1.0, "c");
    let report = s.assess_feasibility().unwrap();
    assert!(report.exceeded(), "{report:?}");
    assert_eq!(s.state(), RunState::FeasibilityBlocked);

    // Arming is HARD-BLOCKED.
    assert!(matches!(s.arm(), Err(ServiceError::BadState(_))));
    // An empty ack is refused; a real ack arms with the override recorded.
    assert!(matches!(
        s.arm_with_override(""),
        Err(ServiceError::BadState(_))
    ));
    s.arm_with_override("operator accepts 2x force exceedance")
        .unwrap();
    assert_eq!(s.state(), RunState::Armed);
    let digest = s.feasibility().unwrap();
    assert_eq!(
        digest.override_ack.as_deref(),
        Some("operator accepts 2x force exceedance")
    );
    assert!(s
        .log()
        .events()
        .iter()
        .any(|e| matches!(e.kind, EventKind::OverrideAck { .. })));

    // The overridden run fires (one pulse to show the path is live).
    assert!(s.fire_next_pulse().unwrap());
    s.stop().unwrap();
    assert_eq!(s.state(), RunState::Stopped);

    // Counter-case: the same article on the big shaker passes cleanly.
    let mut cfg2 = ClassicalTestConfig::new(spec_20g_11ms());
    cfg2.shaker = Some(big_shaker());
    cfg2.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 400.0,
        ..MovingMass::default()
    };
    let mut s2 = service(cfg2, 1.0, "c2");
    assert!(!s2.assess_feasibility().unwrap().exceeded());
    s2.arm().unwrap();
    s2.stop().unwrap();
}

// ---------------------------------------------------------------------------
// (d) pulse train at the planned spacing
// ---------------------------------------------------------------------------

#[test]
fn d_pulse_train_fires_count_at_the_planned_spacing() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    // Faster floors than the conservative default so MaxRate is visible.
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    cfg.train = TrainSpec::max_rate(4);

    let mut s = service(cfg, 1.0, "d");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    // WALL-clock pacing on a VIRTUAL clock: the spacing assertion below is real
    // (it reads the timeline the loop actually waited out) but costs no suite
    // time. `g_the_default_pacer_is_the_real_clock` is what proves the shipping
    // default is not this double.
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));

    let plan = s.fire_train().unwrap();
    assert_eq!(plan.count, 4);
    // MaxRate realizes exactly the HostDac floor (0.5 s), never below.
    assert!((plan.realized_spacing_s - 0.5).abs() < 1e-12);
    assert!((plan.floor_s - 0.5).abs() < 1e-12);
    // The trigger semantic is a DELEGATED-path unknown and this is Path B: no marker
    // (2026-08-08).
    assert!(!plan.hardware_blocked);

    let report = s.report_data();
    assert_eq!(report.pulses.len(), 4);
    // All four pulses fired at the pinned MANUAL level, spaced by the plan.
    for row in &report.pulses {
        assert_eq!(row.origin, PulseOrigin::Train);
        assert_eq!(row.scheduled_level_db, -12.0);
        assert_eq!(row.tolerance_overall, Some(true));
    }
    // THE SPACING EVIDENCE is `wall_times_s`, not `at_run_time_s`: the latter is
    // `run_clock_s += spacing_s`, so asserting its deltas equal the spacing is
    // `(x + s) - x == s` and holds even when nothing waits (it cannot fail).
    let t = &report.trains[0].wall_times_s;
    assert_eq!(t.len(), 4);
    for i in 1..4 {
        assert!(
            ((t[i] - t[i - 1]) - 0.5).abs() < 1e-9,
            "wall spacing {} at {i}: {t:?}",
            t[i] - t[i - 1]
        );
    }
    assert_eq!(report.trains.len(), 1);
    assert_eq!(report.trains[0].pulse_numbers, vec![0, 1, 2, 3]);
    // A requested delay below the floor is REFUSED.
    let err = s.fire_train_with(TrainSpec::delayed(3, 0.1)).unwrap_err();
    assert!(matches!(
        err,
        ServiceError::Train(TrainError::SpacingBelowFloor { .. })
    ));
    s.stop().unwrap();
}

// ---------------------------------------------------------------------------
// (e) gunfire spacing = 60 / rate (Procedure IV)
// ---------------------------------------------------------------------------

#[test]
fn e_gunfire_spacing_is_sixty_over_rate_and_floor_refuses_fast_rates() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    // 30 rounds/min -> 2.0 s spacing (>= the 0.5 s floor).
    cfg.gunfire = Some(GunfireSpec {
        rounds_per_min: 30.0,
        rounds: 3,
    });

    let mut s = service(cfg, 1.0, "e");
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    let plan = s.fire_gunfire_burst().unwrap();
    assert_eq!(plan.count, 3);
    assert!(
        (plan.realized_spacing_s - 60.0 / 30.0).abs() < 1e-12,
        "gunfire spacing = 60/rate"
    );
    let report = s.report_data();
    assert_eq!(report.pulses.len(), 3);
    assert!(report
        .pulses
        .iter()
        .all(|r| r.origin == PulseOrigin::Gunfire));
    // WALL time, for the reason stated in fixture (d).
    let t = &report.trains[0].wall_times_s;
    assert_eq!(t.len(), 3);
    for i in 1..3 {
        assert!(
            ((t[i] - t[i - 1]) - 2.0).abs() < 1e-9,
            "wall spacing {} at {i}: {t:?}",
            t[i] - t[i - 1]
        );
    }
    assert_eq!(report.trains.len(), 1);
    assert_eq!(report.trains[0].gunfire_rounds_per_min, Some(30.0));
    assert!(
        !report.trains[0].plan.hardware_blocked,
        "hardware-blocked marker is delegated-only; this is a host burst"
    );

    // 600 rounds/min -> 0.1 s spacing < the 0.5 s floor: REFUSED (the
    // rate model refuses rates below the floors; the pulse count is unchanged).
    let mut s2 = {
        let mut cfg2 = ClassicalTestConfig::new(spec_20g_11ms());
        cfg2.schedule = ShockLevelSchedule::manual_default();
        cfg2.rate_model = RateModel {
            cooling_floor_s: 0.5,
            settling_floor_s: 0.2,
            compute_floor_s: 0.1,
            device_recompute_floor_s: 20.0,
            read_cadence_ok: true,
        };
        cfg2.gunfire = Some(GunfireSpec {
            rounds_per_min: 600.0,
            rounds: 3,
        });
        service(cfg2, 1.0, "e2")
    };
    s2.assess_feasibility().unwrap();
    s2.arm().unwrap();
    let err = s2.fire_gunfire_burst().unwrap_err();
    assert!(matches!(
        err,
        ServiceError::Train(TrainError::SpacingBelowFloor { .. })
    ));
    assert_eq!(s2.report_data().pulses.len(), 0);
    assert!(s2
        .log()
        .events()
        .iter()
        .any(|e| matches!(e.kind, EventKind::TrainRefused { .. })));
    s2.stop().unwrap();
}

// ---------------------------------------------------------------------------
// (f) DriveLimit scales a pulse that would over-g
// ---------------------------------------------------------------------------

#[test]
fn f_drive_limit_scales_the_over_g_pulse_before_firing() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    // 15 g predictive ceiling: the -3 dB rung (14.2 g) fits, the 0 dB rung
    // (20 g) does not and is scaled to exactly the ceiling before firing.
    cfg.drive_limit.peak_accel_g = Some(15.0);

    let mut s = service(cfg, 1.0, "f");
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    let end = s.run_auto(100).unwrap();
    assert_eq!(end, RunState::Stopped, "scaled, not aborted");

    let report = s.report_data();
    assert_eq!(report.pulses.len(), 5);
    // The first four rungs fire unscaled.
    for row in &report.pulses[..4] {
        assert_eq!(row.drive_limit_scale, 1.0, "{row:?}");
    }
    // The full-level pulse is scaled to the ceiling and still passes tolerance
    // against its COMMANDED (scaled) amplitude.
    let last = report.pulses.last().unwrap();
    assert_eq!(last.scheduled_level_db, 0.0);
    assert!(last.drive_limit_scale < 1.0);
    assert!(
        (last.peak_g.unwrap() - 15.0).abs() < 1e-6,
        "fired peak == ceiling, got {:?}",
        last.peak_g
    );
    assert!((last.commanded_amplitude_g - 15.0).abs() < 0.01);
    assert_eq!(last.tolerance_overall, Some(true));
    assert!(report
        .drive_limit_actions
        .iter()
        .any(|a| a.phase == "pre-fire" && a.scale < 1.0));
    assert!(s
        .log()
        .events()
        .iter()
        .any(|e| matches!(e.kind, EventKind::DriveLimitAction { .. })));
}

/// (h) THE AUTO LADDER PACES ON THE REAL CLOCK TOO.
///
/// The ladder was NOT in the first wall-clock-pacing scope: `fire_next_pulse` added
/// the rate-model floor to `run_clock_s` - a `f64` field - and returned, so on
/// the chassis `run_auto` fired every rung back to back with the armature getting
/// none of the cooling / settling / compute time the floors reserve, fastest at
/// the highest level. Fixture (g) is the same control for trains; this is the one
/// that would have caught the ladder.
///
/// Two ladder pulses at a 0.5 s floor, no pacer injected, timed by an `Instant`
/// OUTSIDE the service. Costs the suite ~0.5 s, charged once.
///
/// RED-MUTATION: delete the `wait_until` call in `fire_next_pulse_cancellable`
/// and this reds - the second pulse returns in microseconds.
#[test]
fn h_the_auto_ladder_paces_on_the_real_clock() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    let mut s = service(cfg, 1.0, "h");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    // The FIRST rung does not wait (no previous pulse to be spaced from).
    let t0 = std::time::Instant::now();
    assert!(s.fire_next_pulse().unwrap());
    let first = t0.elapsed().as_secs_f64();
    assert!(
        first < 0.4,
        "the first rung waited {first:.4} s; it must not"
    );

    // The SECOND does.
    let t1 = std::time::Instant::now();
    assert!(s.fire_next_pulse().unwrap());
    let second = t1.elapsed().as_secs_f64();
    assert!(
        second >= 0.5,
        "the second ladder rung fired {second:.4} s after the first, inside the 0.5 s          rate-model floor - the AUTO ladder is not paced and the floor bounds nothing"
    );
    assert!(second < 2.0, "the wait overshot badly: {second:.4} s");

    // And the per-pulse wall timestamps agree with the outside observer.
    let w: Vec<f64> = s
        .report_data()
        .pulses
        .iter()
        .map(|r| r.at_wall_time_s)
        .collect();
    assert_eq!(w.len(), 2);
    assert!(
        ((w[1] - w[0]) - 0.5).abs() < 0.100,
        "pacer timeline {w:?} vs the 0.5 s floor"
    );
    s.stop().unwrap();
}

/// (g) THE DEFAULT PACER IS THE REAL CLOCK, and the service really blocks on it.
///
/// Every other train fixture here injects `VirtualPacer` so the suite stays
/// fast, which means none of them can tell a shipping build that waits from one
/// that does not. This row is the control: no injection, a 2-pulse train at the
/// 0.5 s HostDac floor, and an `Instant` OUTSIDE the service asserting that the
/// call took at least one spacing.
///
/// It costs the suite ~0.5 s. That is the price of the claim, and it is charged
/// once rather than by every fixture.
///
/// RED-MUTATION: delete the `wait_until` call in `fire_train_spec` and this
/// reds - the burst returns in microseconds.
#[test]
fn g_the_default_pacer_is_the_real_clock() {
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    let mut s = service(cfg, 1.0, "g");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    let t0 = std::time::Instant::now();
    let plan = s.fire_train_with(TrainSpec::max_rate(2)).unwrap();
    let elapsed = t0.elapsed().as_secs_f64();

    assert_eq!(plan.count, 2);
    assert!((plan.realized_spacing_s - 0.5).abs() < 1e-12);
    assert!(
        elapsed >= 0.5,
        "a 2-pulse train at a 0.5 s floor took {elapsed:.4} s of WALL time - \
         the service did not wait, so the rate model bounds nothing"
    );
    // Sanity on the other side: the loop waits ONE spacing, not several.
    assert!(elapsed < 2.0, "the wait overshot badly: {elapsed:.4} s");

    // And the pacer's own timeline agrees with the outside observer.
    let w = &s.report_data().trains[0].wall_times_s;
    assert_eq!(w.len(), 2);
    assert!(
        ((w[1] - w[0]) - 0.5).abs() < 0.100,
        "pacer timeline {:?} vs the 0.5 s plan",
        w
    );
}
