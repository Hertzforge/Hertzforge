//! Sim end-to-end acceptance for [`SrsShockService`] on Path B: the service closes
//! the SRS iteration on the hertzforge-hal-sim plant through the
//! [`super::plant::HalPulsePlant`] seam, mirroring the hertzforge-random / Explorer
//! service acceptance suites and the crate's own synthesis and notch fixtures.
//!
//! The acceptance cases (all Path B / Sim):
//! (a) a full AUTO run converges the control SRS to the target within tolerance at
//!     full level (asserts the achieved worst per-band error);
//! (b) a limit channel whose (user-supplied) FRF resonates in-band: the arm-time
//!     predictive notch holds the limit-channel PREDICTED SRS under bound while the
//!     control SRS still matches AWAY from the notch (the `limit::PredictiveNotch`
//!     fixture, now through the service);
//! (c) an envelope abort trips on a gross over-level;
//! (d) a feasibility hard-block on an overweight article + the override-with-ack
//!     path;
//! (e) the level ladder walks and per-level SRS improves.
//!
//! # A Sim-plant constraint that shapes case (b)
//!
//! `hertzforge-hal-sim`'s `SimBackend` feeds EVERY input channel (control and monitor
//! alike) the SAME plant response, so a Monitor channel cannot physically resonate
//! differently from the control point through the sim. The predictive notch,
//! however, operates on the limit channel's FRF, and that FRF can be USER-SUPPLIED
//! (config `FrfSpec`) independent of the plant. Case (b) therefore drives the
//! control loop on a flat plant (so the control converges) and gives the limit
//! channel a user-supplied resonant FRF (so the notch has a real in-band bump to
//! defend) - exactly how the crate's own `predictive_notch` fixture builds it.

use super::config::{FrfSpec, LimitChannelSpec, LimitSet, LimitTarget, SrsTestConfig};
use super::plant::{FirePath, HalPulsePlant};
use super::report::{RunState, TripKind};
use super::{ServiceError, SrsShockService};
use crate::iterate::SynthesisOpts;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shaker_db::{MovingMass, RatingValue, ShakerRecord};
use hertzforge_shock_common::ShockLevelSchedule;

const FS: f64 = 20_000.0;

// ---------------------------------------------------------------------------
// Plant + table + service builders
// ---------------------------------------------------------------------------

/// A plant at `FS` (shared clock so the closed loop runs without a bench loopback
/// proof). `modes` is a `(f_n, Q)` resonance bank; empty = pure broadband
/// feedthrough. Noise is zeroed for determinism.
fn plant(modes: Vec<(f64, f64)>, feedthrough: f64) -> SimBackend {
    SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(modes)
        .noise_level(0.0)
        .feedthrough(feedthrough)
        .shared_clock(true)
        .max_input_channels(4)
        .build()
}

/// A minimal SISO table: one Drive channel + one Control channel. The sim maps the
/// Control channel to the plant response.
fn siso_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut drv = Channel::new(1);
    drv.role = ChannelRole::Drive;
    drv.description = "Drive".into();
    let mut ctrl = Channel::new(2);
    ctrl.role = ChannelRole::Control;
    ctrl.description = "Control".into();
    t.channels.push(drv);
    t.channels.push(ctrl);
    t
}

/// The SISO table plus a named Monitor channel. The SimBackend maps the monitor to
/// the SAME plant response as Control (see the module docs); the limit channel's
/// resonant FRF is user-supplied, not measured off this tap.
fn table_with_monitor(monitor_name: &str) -> ChannelTable {
    let mut t = siso_table();
    let mut mon = Channel::new(3);
    mon.role = ChannelRole::Monitor;
    mon.description = monitor_name.into();
    t.channels.push(mon);
    t
}

fn host_service(cfg: SrsTestConfig, table: ChannelTable, backend: SimBackend) -> SrsShockService {
    let host = HalPulsePlant::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    SrsShockService::new(
        cfg,
        table,
        FirePath::Host(Box::new(host)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-sh4b-{}", std::process::id()))),
    )
    .unwrap()
}

/// A big shaker the target easily fits (feasibility passes).
fn big_shaker() -> ShakerRecord {
    let mut s = ShakerRecord::new("big");
    s.force_shock = Some(RatingValue::peak(100_000.0));
    s.max_velocity_ips = Some(RatingValue::peak(400.0));
    s.max_displacement_in_pp = Some(RatingValue::peak(5.0));
    s
}

/// Synthesis knobs with more iterations + a generous per-iter clamp so the offline
/// model converges the flat target.
fn tight_synth() -> SynthesisOpts {
    SynthesisOpts {
        max_iters: 120,
        tol_db: 1.5,
        max_gain_db_per_iter: 6.0,
        q: 10.0,
    }
}

/// A user-supplied resonant limit FRF: a 5x Lorentzian bump at `f0` on a unity
/// floor (the `predictive_notch::bump_frf` fixture shape).
fn bump_frf(f0: f64, half_width: f64) -> FrfSpec {
    let freqs: Vec<f64> = (0..=400).map(|i| i as f64 * (FS / 2.0 / 400.0)).collect();
    let mag: Vec<f64> = freqs
        .iter()
        .map(|&f| 1.0 + 4.0 / (1.0 + ((f - f0) / half_width).powi(2)))
        .collect();
    FrfSpec {
        freqs_hz: freqs,
        mag,
    }
}

/// The worst |err_db| over all bands with a defined error, from a report.
fn worst_abs_err_db(r: &super::report::ReportData) -> f64 {
    r.band_results
        .iter()
        .filter_map(|b| b.err_db)
        .fold(0.0f64, |m, e| m.max(e.abs()))
}

// ===========================================================================
// (a) full AUTO run converges the control SRS to the target
// ===========================================================================

#[test]
fn a_full_auto_converges_control_srs_to_target() {
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    cfg.shaker = Some(big_shaker());
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 50.0,
        ..MovingMass::default()
    };
    // 8 pulses per level so the per-band gain law converges within a level before
    // the ladder advances (1 pulse/level cannot iterate to tolerance).
    cfg.schedule.pulses_per_level = vec![8];
    // BYPASS the review hold: this case asserts CONVERGENCE, and the hold is a
    // deliberate operator stop between the system ID and the first pulse. The hold
    // itself is covered by the `g_*` cases below.
    cfg.review_after_sysid = false;

    // A pure-feedthrough unity plant: the reduced-level system ID measures |H|~=1,
    // the feedforward seed lands the first pulse near target, and the law tightens
    // it band-by-band.
    let mut s = host_service(cfg, siso_table(), plant(vec![], 1.0));

    let end = s.run_auto(200).unwrap();
    assert_eq!(end, RunState::Stopped, "trip: {:?}", s.trip());
    assert!(s.trip().is_none());

    let r = s.report_data();
    // Default AUTO ladder [-12,-9,-6,-3,0] dB x 8 pulses = 40 pulses, every one
    // recorded.
    assert_eq!(r.pulses.len(), 40);
    assert_eq!(s.store().len(), 40);
    assert_eq!(r.pulses.last().unwrap().level_db, 0.0);

    // Every pulse carries a full-band SRS in the store.
    let n_bands = s.band_freqs().len();
    assert!(n_bands > 0);
    for rec in s.store().records() {
        assert_eq!(rec.srs.len(), n_bands);
        assert!(rec.srs.iter().all(|v| v.is_finite() && *v > 0.0));
    }

    // Achieved control SRS at full level: the worst per-band error is well within
    // the synthesis tolerance (measured achieved worst ~= 0.18 dB on this plant;
    // assert a generous <0.5 dB, comfortably inside tol_db = 1.5).
    let worst = worst_abs_err_db(&r);
    assert!(
        worst < 0.5,
        "converged control SRS worst-band error {worst:.3} dB (expected < 0.5)"
    );

    // Honesty + settings travel in the report.
    assert!(r.honesty.path.contains("Path B"));
    assert!(r.feasibility.as_ref().unwrap().override_ack.is_none());
    assert_eq!(r.settings.engine, "wavsyn");
}

// ===========================================================================
// (b) limit channel with an in-band resonant FRF: predictive notch holds the
//     predicted limit SRS under bound; control matches away from the notch
// ===========================================================================

#[test]
fn b_limit_channel_notch_holds_predicted_srs_and_control_matches_elsewhere() {
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    cfg.shaker = Some(big_shaker());
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 50.0,
        ..MovingMass::default()
    };
    // A limit channel with a 5x resonant FRF bump at 500 Hz, bounded at 120 g. At
    // full level the un-notched drive would drive it to ~250 g (50 g target x 5),
    // so the arm-time notch must bind.
    cfg.limits = Some(LimitSet {
        channels: vec![LimitChannelSpec {
            name: "fixture_resp".into(),
            limit: LimitTarget::Flat { accel_g: 120.0 },
            frf: Some(bump_frf(500.0, 150.0)),
        }],
        headroom_db: 1.0,
    });
    // A notch deliberately pulls the control SRS BELOW target at the notched bands
    // (the module docs' honest trade: a limit-held pulse reads low). Widen abort_lo
    // so those bands do not abort; keep abort_hi tight (over-level still aborts).
    cfg.envelope.alarm_db = (30.0, 3.0);
    cfg.envelope.abort_db = (30.0, 6.0);
    cfg.schedule.pulses_per_level = vec![8];
    // BYPASS the review hold (see case (a)): this case asserts the predictive notch.
    cfg.review_after_sysid = false;

    // Flat control plant so the control loop converges; the limit resonance lives
    // entirely in the user-supplied FRF above.
    let mut s = host_service(cfg, table_with_monitor("fixture_resp"), plant(vec![], 1.0));

    let end = s.run_auto(200).unwrap();
    assert_eq!(end, RunState::Stopped, "trip: {:?}", s.trip());

    // The arm-time predictive notch bound the resonant bands.
    let notch = s.arm_notch_report().expect("notch planned at arm");
    assert!(
        !notch.notched_bands.is_empty(),
        "the bump must force a notch"
    );
    assert!(notch.converged, "notch must converge: {notch:?}");
    assert!(
        notch.worst_exceedance_before_db.unwrap() > 2.0,
        "fixture must start in exceedance (got {:?})",
        notch.worst_exceedance_before_db
    );
    assert!(
        notch.worst_exceedance_after_db.unwrap() <= 0.0,
        "notch must clear the exceedance (got {:?})",
        notch.worst_exceedance_after_db
    );
    // The notched bands cluster around the 500 Hz bump.
    for nb in &notch.notched_bands {
        assert!(
            nb.band_freq_hz > 250.0 && nb.band_freq_hz < 1100.0,
            "notched band {} Hz should sit near the 500 Hz bump",
            nb.band_freq_hz
        );
    }

    // The LAST-pulse predicted limit-channel SRS stays under the 120 g bound at
    // every band (achieved worst ~= 106 g on this fixture).
    let preds = s.last_predicted_limit_srs();
    assert_eq!(preds.len(), 1);
    let (name, pred) = &preds[0];
    assert_eq!(name, "fixture_resp");
    let worst_pred = pred.iter().cloned().fold(0.0f64, f64::max);
    assert!(
        worst_pred <= 120.0,
        "predicted limit SRS worst band {worst_pred:.1} g exceeds the 120 g bound"
    );
    assert!(
        worst_pred > 60.0,
        "the limit should actually bind near its bound (got {worst_pred:.1} g)"
    );

    // The control SRS still matches the target AWAY from the notch (bands below
    // 250 Hz and above 900 Hz land within ~1 dB), while the notched bands read
    // deliberately low (the surfaced trade).
    let bands = s.band_freqs().to_vec();
    let r = s.report_data();
    let mut checked_away = 0usize;
    for (b, br) in r.band_results.iter().enumerate() {
        let f = bands[b];
        if !(250.0..=900.0).contains(&f) {
            let err = br.err_db.expect("comparable band");
            assert!(
                err.abs() < 1.0,
                "band {f:.0} Hz (away from the notch) err {err:.3} dB should match within 1 dB"
            );
            checked_away += 1;
        }
    }
    assert!(
        checked_away >= 4,
        "should verify several away-from-notch bands"
    );

    // At least one notched band reads meaningfully low (the notch trade is real).
    let notched_low = r.band_results.iter().enumerate().any(|(b, br)| {
        let f = bands[b];
        (400.0..=650.0).contains(&f) && br.err_db.map(|e| e < -3.0).unwrap_or(false)
    });
    assert!(
        notched_low,
        "a notched band should read low (the limit trade)"
    );

    // The limit report travels with the notch plan + bounds.
    let lr = r.limit_report.expect("limit report present");
    assert!(lr.arm_plan.is_some());
    assert_eq!(lr.bounds_g.len(), 1);
    assert_eq!(lr.headroom_db, 1.0);
}

// ===========================================================================
// (c) envelope abort on a gross over-level
// ===========================================================================

#[test]
fn c_envelope_abort_trips_on_gross_over_level() {
    // No system ID (unity-plant seed) + a high-gain plant (feedthrough 20x): the
    // FIRST -12 dB pulse lands far above the target, breaching the abort
    // envelope's high side.
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    cfg.sysid = None;
    cfg.shaker = Some(big_shaker());

    let mut s = host_service(cfg, siso_table(), plant(vec![], 20.0));
    let end = s.run_auto(200).unwrap();
    assert_eq!(end, RunState::Aborted);

    let trip = s.trip().expect("trip recorded");
    match &trip.kind {
        TripKind::Envelope {
            abort_bands,
            worst_err_db,
            ..
        } => {
            assert!(*abort_bands > 0, "at least one band over the abort band");
            assert!(
                *worst_err_db > 6.0,
                "gross over-level: worst err {worst_err_db:.1} dB well over the +6 dB abort band"
            );
        }
        k => panic!("wrong trip kind: {k:?}"),
    }
    assert_eq!(trip.at_pulse, 0, "aborts on the first over-level pulse");
    assert!(s.status().abort_active);
    // Only the one bad pulse fired; the ladder never advanced.
    assert_eq!(s.report_data().pulses.len(), 1);
}

// ===========================================================================
// (d) feasibility hard-block on an overweight article + override-with-ack
// ===========================================================================

#[test]
fn d_feasibility_hard_blocks_overweight_article_and_override_arms() {
    // force_shock / m_total = 500 lbf / 500 lb ~ 1 g ceiling << the target demand,
    // so the deterministic feasibility (crest = 1.0) is Exceeded.
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    let mut shaker = ShakerRecord::new("small");
    shaker.force_shock = Some(RatingValue::peak(500.0));
    shaker.max_velocity_ips = Some(RatingValue::peak(400.0));
    shaker.max_displacement_in_pp = Some(RatingValue::peak(5.0));
    cfg.shaker = Some(shaker);
    cfg.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 400.0,
        ..MovingMass::default()
    };

    let mut s = host_service(cfg, siso_table(), plant(vec![(500.0, 4.0)], 1.0));
    let report = s.assess_feasibility().unwrap();
    assert!(report.exceeded(), "{report:?}");
    assert_eq!(s.state(), RunState::FeasibilityBlocked);

    // Arming is HARD-BLOCKED.
    assert!(matches!(s.arm(), Err(ServiceError::BadState(_))));
    // An empty ack is refused.
    assert!(matches!(
        s.arm_with_override(""),
        Err(ServiceError::BadState(_))
    ));
    // A real ack arms (Path B with system ID enters SysId) and records the ack.
    s.arm_with_override("operator accepts the force exceedance")
        .unwrap();
    assert_eq!(s.state(), RunState::SysId);
    let digest = s.feasibility().unwrap();
    assert_eq!(
        digest.override_ack.as_deref(),
        Some("operator accepts the force exceedance")
    );
    assert!(s.report_data().feasibility.unwrap().override_ack.is_some());

    // Counter-case: the same article on the big shaker is NOT blocked.
    let mut cfg2 = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg2.synthesis = tight_synth();
    cfg2.shaker = Some(big_shaker());
    cfg2.moving_mass = MovingMass {
        armature_lb: 100.0,
        test_article_lb: 400.0,
        ..MovingMass::default()
    };
    let mut s2 = host_service(cfg2, siso_table(), plant(vec![(500.0, 4.0)], 1.0));
    assert!(!s2.assess_feasibility().unwrap().exceeded());
    s2.arm().unwrap();
    assert_eq!(s2.state(), RunState::SysId);
}

// ===========================================================================
// (e) the level ladder walks and per-level SRS improves
// ===========================================================================

#[test]
fn e_level_ladder_walks_and_per_level_srs_improves() {
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    cfg.schedule = ShockLevelSchedule::auto_default();
    cfg.schedule.pulses_per_level = vec![6];
    cfg.shaker = Some(big_shaker());
    // BYPASS the review hold (see case (a)): this case asserts the ladder walk.
    cfg.review_after_sysid = false;

    // A mild in-band resonance (500 Hz, Q=4): the first pulse at each level is off,
    // and the per-band law tightens it pulse-over-pulse within the level.
    let mut s = host_service(cfg, siso_table(), plant(vec![(500.0, 4.0)], 1.0));

    let end = s.run_auto(200).unwrap();
    assert_eq!(end, RunState::Stopped, "trip: {:?}", s.trip());

    let r = s.report_data();
    // 5 ladder rungs x 6 pulses = 30 pulses.
    assert_eq!(r.pulses.len(), 30);
    let levels: Vec<f64> = r.pulses.iter().map(|p| p.level_db).collect();
    assert_eq!(
        levels,
        vec![
            -12.0, -12.0, -12.0, -12.0, -12.0, -12.0, -9.0, -9.0, -9.0, -9.0, -9.0, -9.0, -6.0,
            -6.0, -6.0, -6.0, -6.0, -6.0, -3.0, -3.0, -3.0, -3.0, -3.0, -3.0, 0.0, 0.0, 0.0, 0.0,
            0.0, 0.0
        ]
    );

    // The peaks walk the ladder up (each rung's peak > the previous rung's).
    let rung_peak = |rung: usize| r.pulses[rung * 6].peak_accel_g;
    for rung in 1..5 {
        assert!(
            rung_peak(rung) > rung_peak(rung - 1),
            "rung {rung} peak {:.2} should exceed rung {} peak {:.2}",
            rung_peak(rung),
            rung - 1,
            rung_peak(rung - 1)
        );
    }

    // Per-level SRS improves: within the FIRST level, the worst-band error drops
    // from the first pulse to the last pulse of that level (the law iterating).
    let first_pulse_err = r.pulses[0].worst_err_db.unwrap().abs();
    let last_of_level_err = r.pulses[5].worst_err_db.unwrap().abs();
    assert!(
        last_of_level_err < first_pulse_err,
        "within the first level the worst error should drop \
         (first {first_pulse_err:.3} dB -> last {last_of_level_err:.3} dB)"
    );

    // The level history records each rung entry.
    let hist_levels: Vec<f64> = r.level_history.iter().map(|h| h.level_db).collect();
    assert_eq!(hist_levels, vec![-12.0, -9.0, -6.0, -3.0, 0.0]);
}

// ===========================================================================
// The scripted plant double (rate, cold-task, cancellation and review-hold cases)
//
// The Sim cases above prove the DSP; these prove the SERVICE's own arithmetic
// and lifecycle, which needs a plant that can be made to do what a real one only
// does on a chassis: coerce the sample rate at bring-up, answer a fire with
// `Cancelled`, and be asked afterwards whether it was actually parked.
// ===========================================================================

use super::plant::{FiredResponses, PlantError, PulsePlant};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// What the service DID to its plant, readable from the test after the service
/// has been dropped (which is the point of the drop-safety case).
#[derive(Default)]
struct PlantSpy {
    opens: AtomicUsize,
    fires: AtomicUsize,
    stops: AtomicUsize,
    emergency_stops: AtomicUsize,
    /// Drive samples the last fire was handed (the synthesized record, no ring-out).
    last_drive_len: AtomicUsize,
    /// Ring-out samples the last fire was handed.
    last_ring_out: AtomicUsize,
    /// Whether the last fire was handed a cancel flag at all.
    last_fire_saw_cancel: AtomicBool,
    /// Cleared by `ensure_open`/`fire`, set by `stop`/`emergency_stop`.
    parked: AtomicBool,
}

impl PlantSpy {
    fn fires(&self) -> usize {
        self.fires.load(Ordering::Relaxed)
    }
    fn stops(&self) -> usize {
        self.stops.load(Ordering::Relaxed)
    }
    fn emergency_stops(&self) -> usize {
        self.emergency_stops.load(Ordering::Relaxed)
    }
    fn parked(&self) -> bool {
        self.parked.load(Ordering::Relaxed)
    }
}

/// A scripted [`PulsePlant`]: an ideal unity plant (the response IS the drive
/// record) whose bring-up COERCES the sample rate, exactly as a cDAQ-9234 rounds
/// a request up its `13.1072 MHz/(256 n)` ladder at configure.
struct MockPlant {
    spy: Arc<PlantSpy>,
    /// Reported before `ensure_open` - the REQUESTED rate.
    fs_hz: f64,
    /// Reported after `ensure_open` - what the "device" configured.
    coerced_fs_hz: f64,
    /// Answer `Cancelled` on this fire index (0-based). `None` = never.
    cancel_on_fire: Option<usize>,
}

impl MockPlant {
    fn new(spy: Arc<PlantSpy>, requested_fs_hz: f64, coerced_fs_hz: f64) -> Self {
        MockPlant {
            spy,
            fs_hz: requested_fs_hz,
            coerced_fs_hz,
            cancel_on_fire: None,
        }
    }
    fn cancel_on_fire(mut self, n: usize) -> Self {
        self.cancel_on_fire = Some(n);
        self
    }
}

impl PulsePlant for MockPlant {
    fn fs_hz(&self) -> f64 {
        self.fs_hz
    }
    fn ensure_open(&mut self) -> Result<(), String> {
        self.spy.opens.fetch_add(1, Ordering::Relaxed);
        // The adoption seam: the rate the caller asked for is NOT the rate that
        // comes back out of configure.
        self.fs_hz = self.coerced_fs_hz;
        self.spy.parked.store(false, Ordering::Relaxed);
        Ok(())
    }
    fn fire(
        &mut self,
        drive_g: &[f64],
        ring_out_samples: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<FiredResponses, PlantError> {
        let n = self.spy.fires.fetch_add(1, Ordering::Relaxed);
        self.spy
            .last_drive_len
            .store(drive_g.len(), Ordering::Relaxed);
        self.spy
            .last_ring_out
            .store(ring_out_samples, Ordering::Relaxed);
        self.spy
            .last_fire_saw_cancel
            .store(cancel.is_some(), Ordering::Relaxed);
        self.spy.parked.store(false, Ordering::Relaxed);

        let flagged = cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false);
        if flagged || self.cancel_on_fire == Some(n) {
            // The plant contract: a cancelled fire leaves the task RUNNING; the
            // SERVICE is what parks it.
            return Err(PlantError::Cancelled {
                frames_done: 0,
                frames_committed: drive_g.len(),
                frames_expected: drive_g.len() + ring_out_samples,
            });
        }
        let mut control: Vec<f64> = drive_g.to_vec();
        control.resize(drive_g.len() + ring_out_samples, 0.0);
        Ok(FiredResponses {
            control_g: control,
            monitors: Vec::new(),
        })
    }
    fn stop(&mut self) -> Result<(), String> {
        self.spy.stops.fetch_add(1, Ordering::Relaxed);
        self.spy.parked.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), String> {
        self.spy.emergency_stops.fetch_add(1, Ordering::Relaxed);
        self.spy.parked.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// A service on the scripted plant. `cfg` is taken as given so each case states
/// exactly the policy it is testing.
fn mock_service(cfg: SrsTestConfig, plant: MockPlant) -> SrsShockService {
    SrsShockService::new(
        cfg,
        siso_table(),
        FirePath::Host(Box::new(plant)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-mock-{}", std::process::id()))),
    )
    .unwrap()
}

/// A config that fires without tripping on the ideal unity plant: one short
/// level, a wide-open envelope (this family of cases is not about tolerance) and
/// no shaker (feasibility NotEvaluated).
fn mock_cfg() -> SrsTestConfig {
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, 50.0);
    cfg.synthesis = tight_synth();
    cfg.schedule.start_db = 0.0;
    cfg.schedule.table_db = vec![0.0];
    cfg.schedule.pulses_per_level = vec![4];
    cfg.envelope.alarm_db = (60.0, 60.0);
    cfg.envelope.abort_db = (60.0, 60.0);
    cfg.compute_pvss = false;
    cfg
}

// ===========================================================================
// (d4) The Send bounds
// ===========================================================================

/// COMPILE-TIME. `SrsShockService` is `Send`, so a host may own it on any thread
/// or move it between them - it does not have to be confined to a dedicated
/// worker, which is what the missing `+ Send` on the two `FirePath` trait objects
/// forced for no reason a type could name.
///
/// RED-MUTATION: drop the `+ Send` from `FirePath::Host(Box<dyn PulsePlant + Send>)`
/// and this case does not compile.
#[test]
fn d4_the_service_and_its_fire_path_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SrsShockService>();
    assert_send::<FirePath>();

    // And a built service really does cross a thread boundary.
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0));
    let handle = std::thread::spawn(move || {
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        s.fire_pulse().unwrap()
    });
    assert!(handle.join().expect("the service ran on another thread"));
    assert_eq!(spy.fires(), 1);
    // The service was dropped on that thread; Drop parked the plant there.
    assert!(spy.parked());
}

// ===========================================================================
// (f) the DEVICE owns the rate - arm adopts it and re-derives at it
// ===========================================================================

/// RED-MUTATION (verified by application): delete the `self.adopt_rate_at_arm()?`
/// call from `arm_inner` and this case reds on the ring-out sample count and on
/// the record length the plant is handed - both stay denominated in the
/// REQUESTED 20 000 Hz while the plant plays at 8 533.33 Hz.
#[test]
fn f_arm_adopts_the_device_rate_and_rederives_the_hz_denominated_set() {
    const REQUESTED: f64 = 20_000.0;
    // A real 9234 ladder value (13.1072 MHz / (256 x 6)).
    const COERCED: f64 = 13_107_200.0 / (256.0 * 6.0);
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None; // the seam under test is arm, not the system ID
    let ring_out_s = cfg.ring_out_s;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), REQUESTED, COERCED));

    // Before arm the service is denominated in what it ASKED for.
    assert_eq!(s.fs_hz(), REQUESTED);
    assert_eq!(
        s.ring_out_samples(),
        (ring_out_s * REQUESTED).round() as usize
    );

    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    // After arm it is denominated in what the DEVICE configured, and the
    // requested rate is still nameable rather than absorbed.
    assert_eq!(s.fs_hz(), COERCED);
    assert_eq!(s.requested_fs_hz(), REQUESTED);
    let want_ring = (ring_out_s * COERCED).round() as usize;
    assert_eq!(s.ring_out_samples(), want_ring);
    assert_ne!(
        want_ring,
        (ring_out_s * REQUESTED).round() as usize,
        "setup invalid: the coercion must actually move the ring-out count"
    );

    // The drive record the plant is handed is synthesized at the ADOPTED rate.
    let want_drive_len = s
        .drive_components()
        .expect("armed")
        .synthesize(COERCED)
        .len();
    assert_ne!(
        want_drive_len,
        s.drive_components().unwrap().synthesize(REQUESTED).len(),
        "setup invalid: the coercion must actually move the record length"
    );
    assert!(s.fire_pulse().unwrap());
    assert_eq!(spy.fires(), 1);
    assert_eq!(
        spy.last_drive_len.load(Ordering::Relaxed),
        want_drive_len,
        "the fired record must be synthesized at the rate the device configured"
    );
    assert_eq!(spy.last_ring_out.load(Ordering::Relaxed), want_ring);
    assert!(
        spy.last_fire_saw_cancel.load(Ordering::Relaxed),
        "every fire is handed the cancel flag"
    );

    // Both authorities travel in the report, and the band grid the run reports is
    // the grid that was validated against the ADOPTED rate.
    let settings = s.report_data().settings;
    assert_eq!(settings.fs_hz, COERCED);
    assert_eq!(settings.requested_fs_hz, REQUESTED);
    assert_eq!(settings.ring_out_samples, want_ring);
    assert_eq!(settings.band_freqs, s.band_freqs().to_vec());
    assert!(settings.band_freqs.iter().all(|f| *f < COERCED / 2.0));
}

/// The rate check that CAN fail. A coercion DOWNWARD can put the analysis band
/// top at or above Nyquist; the construction-time check passed at the REQUESTED
/// rate, so only a re-check at the adopted rate catches it.
///
/// RED-MUTATION (verified by application): delete `self.adopt_rate_at_arm()?`
/// and this arm SUCCEEDS - the run goes on to analyze a 2 000 Hz band top on a
/// 3 000 Hz timebase.
#[test]
fn f2_arm_refuses_when_the_configured_rate_puts_the_band_top_at_nyquist() {
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 3_000.0));
    s.assess_feasibility().unwrap();

    let err = s.arm().expect_err("2000 Hz band top on a 3000 Hz device");
    let msg = err.to_string();
    assert!(msg.contains("Nyquist"), "{msg}");
    assert!(
        msg.contains("3000"),
        "the refusal names the CONFIGURED rate: {msg}"
    );
    assert!(
        msg.contains("20000"),
        "the refusal names the REQUESTED rate: {msg}"
    );
    assert!(!matches!(s.state(), RunState::Armed | RunState::Firing));
}

// ===========================================================================
// (g) the review hold
// ===========================================================================

#[test]
fn g_review_hold_is_entered_after_sysid_parks_the_plant_and_refuses_driving_verbs() {
    let spy = Arc::new(PlantSpy::default());
    let cfg = mock_cfg(); // review_after_sysid defaults TRUE
    assert!(cfg.review_after_sysid, "the hold is the DEFAULT");
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0));

    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.state(), RunState::SysId);
    s.run_sysid().unwrap();

    // HELD, with the hardware parked.
    assert_eq!(s.state(), RunState::AwaitingReview);
    assert!(!RunState::AwaitingReview.is_running());
    assert!(spy.parked(), "entering the hold parks the plant");
    assert!(spy.stops() >= 1);
    let stops_at_hold = spy.stops();

    // Every driving verb refuses BY NAME.
    for msg in [
        s.fire_pulse().unwrap_err().to_string(),
        s.step_up().unwrap_err().to_string(),
        s.step_down().unwrap_err().to_string(),
        s.jump_to_level(0).unwrap_err().to_string(),
    ] {
        assert!(
            msg.contains("HOLDING for operator review") && msg.contains("proceed_after_review"),
            "the refusal must name the hold and the way out: {msg}"
        );
    }
    // Nothing fired while held (the sysID fire is still the only one), and the
    // refusals did not disturb the parked plant.
    assert_eq!(spy.fires(), 1);
    assert_eq!(spy.stops(), stops_at_hold);

    // PROCEED continues into the ordinary armed path.
    s.proceed_after_review().unwrap();
    assert_eq!(s.state(), RunState::Armed);
    assert!(s.fire_pulse().unwrap());
    assert_eq!(spy.fires(), 2);

    // Proceeding twice is a refusal, not a no-op.
    assert!(matches!(
        s.proceed_after_review(),
        Err(ServiceError::BadState(_))
    ));
}

#[test]
fn g2_review_hold_bypassed_when_the_config_clears_it() {
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0));

    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid().unwrap();

    assert_eq!(
        s.state(),
        RunState::Armed,
        "the bypass arms straight through"
    );
    assert!(!spy.parked(), "the bypass does not park");
    assert_eq!(spy.stops(), 0);
    // `proceed_after_review` in a state that cannot proceed is a refusal.
    assert!(matches!(
        s.proceed_after_review(),
        Err(ServiceError::BadState(_))
    ));
}

/// DROP SAFETY. A controller vanishing (or any host dropping the service) during
/// the hold must leave the device parked, not reserved-and-running.
#[test]
fn g3_dropping_the_service_during_the_hold_leaves_the_plant_parked() {
    let spy = Arc::new(PlantSpy::default());
    let mut s = mock_service(
        mock_cfg(),
        MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0),
    );
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid().unwrap();
    assert_eq!(s.state(), RunState::AwaitingReview);
    let stops_at_hold = spy.stops();

    // The disconnect-shaped exit: nobody calls stop(), the service is just gone.
    drop(s);

    assert!(spy.parked(), "the plant is parked after the drop");
    assert_eq!(
        spy.stops(),
        stops_at_hold + 1,
        "Drop parks once more, on top of the hold's park"
    );
    assert_eq!(spy.emergency_stops(), 0, "a drop is not an abort");
}

// ===========================================================================
// (h) cancellation
// ===========================================================================

#[test]
fn h_a_cancelled_sysid_is_a_typed_early_out_that_parks_and_stays_rerunnable() {
    let spy = Arc::new(PlantSpy::default());
    let mut s = mock_service(
        mock_cfg(),
        MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0).cancel_on_fire(0),
    );
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.state(), RunState::SysId);

    let err = s.run_sysid().expect_err("the sysID fire cancels");
    assert!(matches!(err, ServiceError::Cancelled(_)), "{err:?}");
    assert!(
        err.to_string().contains("committed frame"),
        "the early-out carries the committed-queue arithmetic: {err}"
    );

    // NEVER half-armed: no FRF was measured, so the drive is NOT armed and the
    // run sits in the state that says a system ID is owed.
    assert_eq!(s.state(), RunState::SysId);
    assert!(s.drive_components().is_none());
    assert!(spy.parked(), "a cancelled sysID parks the plant");
    assert_eq!(spy.emergency_stops(), 0, "a cancel is not an abort");
    assert!(
        !s.cancel_handle().load(Ordering::Relaxed),
        "the service CONSUMES the cancel request"
    );

    // And the retry runs, because the flag was consumed.
    s.run_sysid().unwrap();
    assert_eq!(s.state(), RunState::AwaitingReview);
}

#[test]
fn h2_a_cancelled_pulse_discards_the_record_and_stops_cleanly() {
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let mut s = mock_service(
        cfg,
        MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0).cancel_on_fire(1),
    );
    s.assess_feasibility().unwrap();
    s.arm().unwrap();

    assert!(s.fire_pulse().unwrap(), "pulse 0 fires normally");
    assert_eq!(s.store().len(), 1);

    assert!(!s.fire_pulse().unwrap(), "pulse 1 is cancelled mid-record");
    assert_eq!(s.state(), RunState::Stopped, "a cancel is a clean stop");
    assert!(s.trip().is_none(), "a cancel writes NO trip record");
    assert_eq!(
        s.store().len(),
        1,
        "the truncated record must NOT be ingested as a measurement"
    );
    assert!(spy.parked());
    assert_eq!(spy.emergency_stops(), 0);
    assert!(!s.cancel_handle().load(Ordering::Relaxed));
}

/// The host-installed handle is the same flag the fire reads.
#[test]
fn h3_a_host_installed_cancel_handle_cancels_the_next_fire() {
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0));
    let host_flag = Arc::new(AtomicBool::new(true));
    s.set_cancel_handle(Arc::clone(&host_flag));
    assert!(
        !host_flag.load(Ordering::Relaxed),
        "installing CLEARS the handle, so a stale set flag cannot pre-cancel"
    );

    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    host_flag.store(true, Ordering::Relaxed);

    assert!(!s.fire_pulse().unwrap());
    assert_eq!(s.state(), RunState::Stopped);
    assert!(spy.parked());
    assert!(
        !host_flag.load(Ordering::Relaxed),
        "consumed on the way out"
    );
}

// ===========================================================================
// (i) the terminal paths reach the hardware
// ===========================================================================

#[test]
fn i_stop_parks_the_plant_and_abort_emergency_stops_it() {
    // Clean stop.
    let spy = Arc::new(PlantSpy::default());
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let mut s = mock_service(cfg, MockPlant::new(Arc::clone(&spy), 20_000.0, 20_000.0));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert!(s.fire_pulse().unwrap());
    s.stop().unwrap();
    assert_eq!(s.state(), RunState::Stopped);
    assert!(spy.parked(), "stop() reaches the hardware");
    assert!(spy.stops() >= 1);
    assert_eq!(spy.emergency_stops(), 0, "a clean stop is not an abort");

    // Abort.
    let spy2 = Arc::new(PlantSpy::default());
    let mut cfg2 = mock_cfg();
    cfg2.review_after_sysid = false;
    cfg2.sysid = None;
    let mut s2 = mock_service(cfg2, MockPlant::new(Arc::clone(&spy2), 20_000.0, 20_000.0));
    s2.assess_feasibility().unwrap();
    s2.arm().unwrap();
    s2.abort_now("operator abort");
    assert_eq!(s2.state(), RunState::Aborted);
    assert!(spy2.parked(), "abort() reaches the hardware");
    assert!(
        spy2.emergency_stops() >= 1,
        "abort takes the EMERGENCY teardown, not the ramped one"
    );
}
