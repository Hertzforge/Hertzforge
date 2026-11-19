//! Control-core gates: composite golden values, blend-law continuity and
//! trust-boundary behavior, notch-governor adversarial fixtures, drive-guard
//! verdicts, and the compression-preset pairs.

use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_sine::{
    composite_amplitude, tracked_displacement_in_pp, tracked_velocity_ips_pk, BlendConfig,
    BlendLaw, CompositeMode, CompressionPreset, ControlError, DriveGuard, DriveVerdict,
    FeedforwardConfig, FeedforwardTable, LimitChannelTick, NotchGovernor, NotchGovernorConfig,
    G_TO_IN_S2,
};
use std::f64::consts::TAU;

fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: {a} vs {b} (tol {tol}, err {})",
        (a - b).abs()
    );
}

// =============================================================================
// Composite (golden, hand-computed)
// =============================================================================

#[test]
fn composite_weighted_avg_is_normalized_dot_product() {
    // weights [1,2,3] normalize to [1/6, 1/3, 1/2]:
    // avg = 1/6*1 + 1/3*2 + 1/2*3 = 14/6.
    let amps = [1.0, 2.0, 3.0];
    let weights = [1.0, 2.0, 3.0];
    let avg = composite_amplitude(&amps, &weights, CompositeMode::WeightedAvg).unwrap();
    assert_close(avg, 14.0 / 6.0, 1e-12, "weighted avg");
}

#[test]
fn composite_min_max_operate_on_weighted_values() {
    let amps = [1.0, 2.0, 3.0];
    let weights = [1.0, 2.0, 3.0];
    // weighted values: [1/6, 2/3, 3/2]
    let min = composite_amplitude(&amps, &weights, CompositeMode::Min).unwrap();
    let max = composite_amplitude(&amps, &weights, CompositeMode::Max).unwrap();
    assert_close(min, 1.0 / 6.0, 1e-12, "weighted min");
    assert_close(max, 3.0 / 2.0, 1e-12, "weighted max");
}

#[test]
fn composite_equal_weights_reduce_to_plain_stats() {
    let amps = [2.0, 8.0, 5.0];
    let w = [1.0, 1.0, 1.0];
    let avg = composite_amplitude(&amps, &w, CompositeMode::WeightedAvg).unwrap();
    let min = composite_amplitude(&amps, &w, CompositeMode::Min).unwrap();
    let max = composite_amplitude(&amps, &w, CompositeMode::Max).unwrap();
    assert_close(avg, 5.0, 1e-12, "plain avg");
    assert_close(min, 2.0 / 3.0, 1e-12, "plain min (weighted by 1/3)");
    assert_close(max, 8.0 / 3.0, 1e-12, "plain max (weighted by 1/3)");
}

#[test]
fn composite_rejects_bad_inputs() {
    assert!(matches!(
        composite_amplitude(&[], &[], CompositeMode::WeightedAvg),
        Err(ControlError::BadComposite(_))
    ));
    assert!(matches!(
        composite_amplitude(&[1.0], &[1.0, 2.0], CompositeMode::Min),
        Err(ControlError::BadComposite(_))
    ));
    assert!(matches!(
        composite_amplitude(&[1.0, 2.0], &[0.0, 0.0], CompositeMode::Max),
        Err(ControlError::BadComposite(_))
    ));
    assert!(matches!(
        composite_amplitude(&[1.0, 2.0], &[1.0, -3.0], CompositeMode::WeightedAvg),
        Err(ControlError::BadComposite(_))
    ));
}

// =============================================================================
// Feedforward table
// =============================================================================

/// A 10-point log grid 10..=100 Hz with |H| = f/10 so the trusted ff_db is
/// exactly -20*log10(f/10) dB against a unit target (linear in ln f, which
/// makes the table's ln-f interpolation EXACT everywhere on the span).
fn tilt_table(coh: impl Fn(f64) -> f64) -> FeedforwardTable {
    let freqs: Vec<f64> = (0..10).map(|k| 10.0 * 10f64.powf(k as f64 / 9.0)).collect();
    let h: Vec<f64> = freqs.iter().map(|f| f / 10.0).collect();
    let c: Vec<f64> = freqs.iter().map(|&f| coh(f)).collect();
    FeedforwardTable::from_sysid(&freqs, &h, &c, |_| Some(1.0), FeedforwardConfig::default())
        .unwrap()
}

fn ff_expected_db(f: f64) -> f64 {
    -20.0 * (f / 10.0).log10()
}

#[test]
fn feedforward_ff_db_is_target_db_minus_h_db() {
    let t = tilt_table(|_| 1.0);
    // Exact grid point and interpolated points.
    assert_close(t.ff_db(10.0).unwrap(), 0.0, 1e-12, "ff at 10 Hz");
    assert_close(
        t.ff_db(30.0).unwrap(),
        ff_expected_db(30.0),
        1e-9,
        "ff at 30 Hz (interp)",
    );
    assert_close(
        t.ff_db(100.0).unwrap(),
        ff_expected_db(100.0),
        1e-9,
        "ff at 100 Hz",
    );
    // Out of grid.
    assert_eq!(t.ff_db(9.0), None);
    assert_eq!(t.ff_db(101.0), None);
    assert_eq!(t.ff_db(f64::NAN), None);
}

#[test]
fn feedforward_trust_mask_gates_coherence_and_h_floor() {
    // Low coherence above 50 Hz -> those bins untrusted; interpolation with
    // an untrusted bracket end returns None.
    let t = tilt_table(|f| if f < 50.0 { 1.0 } else { 0.2 });
    assert!(t.ff_db(12.0).is_some());
    assert_eq!(t.ff_db(90.0), None, "untrusted bin");
    // Between the last trusted grid point (~46.4 Hz) and the first untrusted
    // one the bracket is mixed -> None.
    assert_eq!(t.ff_db(48.0), None, "mixed-trust bracket");

    // |H| floor: bin 5 orders below the grid peak is untrusted even with
    // perfect coherence (default h_floor_frac = 0.05).
    let freqs = [10.0, 100.0];
    let h = [1.0, 0.001];
    let c = [1.0, 1.0];
    let t2 =
        FeedforwardTable::from_sysid(&freqs, &h, &c, |_| Some(1.0), Default::default()).unwrap();
    assert!(t2.ff_db(10.0).is_some());
    assert_eq!(t2.ff_db(100.0), None, "|H| below floor fraction of peak");
}

#[test]
fn feedforward_target_gap_marks_bins_untrusted() {
    // The profile-target closure returning None (out of profile band) makes
    // the bin untrusted.
    let freqs = [10.0, 20.0, 40.0];
    let h = [1.0, 1.0, 1.0];
    let c = [1.0, 1.0, 1.0];
    let t = FeedforwardTable::from_sysid(
        &freqs,
        &h,
        &c,
        |f| if f < 15.0 { None } else { Some(2.0) },
        Default::default(),
    )
    .unwrap();
    assert_eq!(t.ff_db(10.0), None);
    assert_close(
        t.ff_db(20.0).unwrap(),
        20.0 * 2f64.log10(),
        1e-12,
        "trusted target",
    );
}

#[test]
fn feedforward_rejects_bad_grids() {
    let cfg = FeedforwardConfig::default();
    assert!(FeedforwardTable::from_sysid(&[], &[], &[], |_| Some(1.0), cfg).is_err());
    assert!(
        FeedforwardTable::from_sysid(&[10.0, 10.0], &[1.0, 1.0], &[1.0, 1.0], |_| Some(1.0), cfg)
            .is_err(),
        "non-increasing grid"
    );
    assert!(
        FeedforwardTable::from_sysid(&[10.0, 20.0], &[1.0], &[1.0, 1.0], |_| Some(1.0), cfg)
            .is_err(),
        "ragged grid"
    );
}

// =============================================================================
// Blend law (delta form)
// =============================================================================

#[test]
fn blend_total_is_w_scaled_ff_delta_and_monotone_in_w() {
    let table = tilt_table(|_| 1.0);
    // Fixed sweep path 20 -> 80 Hz in 60 log steps.
    let path: Vec<f64> = (0..=60)
        .map(|i| 20.0 * (80.0f64 / 20.0).powf(i as f64 / 60.0))
        .collect();
    let expected_delta = ff_expected_db(80.0) - ff_expected_db(20.0); // about -12.04 dB

    let mut totals = Vec::new();
    for iw in 0..=10 {
        let w = iw as f64 / 10.0;
        let mut law = BlendLaw::new(BlendConfig {
            w,
            max_step_db_per_tick: 1.0,
        })
        .unwrap();
        let mut total = 0.0;
        for &f in &path {
            let step = law.step(f, 0.0, &table);
            // Continuity: no single tick moves more than the per-tick share.
            assert!(step.abs() <= 1.0, "per-tick step {step} exceeded the clamp");
            total += step;
        }
        assert_close(
            total,
            w * expected_delta,
            1e-9,
            &format!("telescoped total at w={w}"),
        );
        totals.push(total);
    }
    // Monotone mix: larger w carries more (negative) prediction, strictly.
    for pair in totals.windows(2) {
        assert!(
            pair[1] < pair[0],
            "totals not monotone in w: {} then {}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn blend_trust_boundary_emits_no_step_and_reanchors_without_jump() {
    // Bins at/above 60 Hz untrusted.
    let table = tilt_table(|f| if f < 50.0 { 1.0 } else { 0.2 });
    let mut law = BlendLaw::new(BlendConfig {
        w: 1.0,
        max_step_db_per_tick: 5.0,
    })
    .unwrap();

    // Anchor inside the trusted region.
    let first = law.step(20.0, 0.0, &table);
    assert_close(first, 0.0, 1e-12, "first tick anchors, no jump");
    // Normal trusted move produces the ff delta.
    let step = law.step(30.0, 0.0, &table);
    assert_close(
        step,
        ff_expected_db(30.0) - ff_expected_db(20.0),
        1e-9,
        "trusted delta",
    );
    // Crossing INTO the untrusted region: prediction term drops out.
    assert_close(law.step(70.0, 0.0, &table), 0.0, 1e-12, "boundary in");
    assert_close(law.step(90.0, 0.0, &table), 0.0, 1e-12, "inside gap");
    // Crossing BACK into trust: re-anchor tick emits nothing...
    assert_close(law.step(30.0, 0.0, &table), 0.0, 1e-12, "re-anchor");
    // ...and the next trusted move resumes clean deltas from the new anchor.
    let resumed = law.step(20.0, 0.0, &table);
    assert_close(
        resumed,
        ff_expected_db(20.0) - ff_expected_db(30.0),
        1e-9,
        "post-gap delta",
    );
}

#[test]
fn blend_w1_dwell_tracks_plant_drift_through_servo_step() {
    // w = 1 at a FIXED frequency (dwell): the prediction delta is zero every
    // tick, so the whole move must be the servo step - a drifting plant gain
    // is still corrected (the servo is never bypassed).
    let table = tilt_table(|_| 1.0);
    let mut law = BlendLaw::new(BlendConfig {
        w: 1.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();

    let f_dwell = 40.0;
    let dt = 0.01;
    let ki = 2.0; // Slow preset integral gain
    let target = 1.0;
    let mut drive_db: f64 = 0.0;
    let mut gain_db: f64 = 0.0; // plant gain drifts up 0.004 dB per tick
    let mut measured = 0.0;
    for _ in 0..2000 {
        gain_db += 0.004;
        measured = 10f64.powf((drive_db + gain_db) / 20.0);
        let e_db = 20.0 * (target / measured).log10();
        let servo_step = (ki * e_db * dt).clamp(-0.2, 0.2);
        let step = law.step(f_dwell, servo_step, &table);
        assert_close(step, servo_step, 1e-12, "dwell step == servo step");
        drive_db += step;
    }
    let final_err_db = 20.0 * (target / measured).log10();
    assert!(
        final_err_db.abs() < 0.5,
        "w=1 dwell did not track plant drift: residual {final_err_db} dB"
    );
    // The drive moved to cancel the accumulated drift (8 dB), within the lag.
    assert_close(drive_db, -8.0, 0.5, "drive compensation");
}

#[test]
fn blend_rejects_bad_config() {
    assert!(BlendLaw::new(BlendConfig {
        w: 1.5,
        max_step_db_per_tick: 1.0
    })
    .is_err());
    assert!(BlendLaw::new(BlendConfig {
        w: f64::NAN,
        max_step_db_per_tick: 1.0
    })
    .is_err());
    assert!(BlendLaw::new(BlendConfig {
        w: 0.5,
        max_step_db_per_tick: 0.0
    })
    .is_err());
    let mut law = BlendLaw::new(BlendConfig::default()).unwrap();
    assert!(law.set_w(-0.1).is_err());
    assert!(law.set_w(1.0).is_ok());
    assert_eq!(law.w(), 1.0);
}

// =============================================================================
// Notch governor - synthetic 2-channel plant fixtures
// =============================================================================

/// The caller loop the docs demand: previous tick's `binding` decides
/// hold_tick vs update (the scalar hold rule), the governor observes the
/// applied drive, and the applied scale comes from `apply`.
struct NotchFixture {
    servo: AmplitudeServo,
    gov: NotchGovernor,
    applied: f64,
    binding: bool,
}

impl NotchFixture {
    fn new(start_scale: f64) -> Self {
        let (ki, rate) = CompressionPreset::Slow.gains();
        let mut servo = AmplitudeServo::new(1.0, rate).with_gains(0.3, ki);
        servo.set_feedforward_scale(start_scale);
        NotchFixture {
            servo,
            gov: NotchGovernor::new(NotchGovernorConfig::default(), 1).unwrap(),
            applied: start_scale,
            binding: false,
        }
    }

    /// One control tick against gains (g_control, g_limit) and a limit bound.
    /// Returns (control_amp, limit_amp) realized at the applied drive.
    fn tick(&mut self, g_c: f64, g_l: f64, bound: f64, dt: f64) -> (f64, f64) {
        let c_amp = g_c * self.applied;
        let l_amp = g_l * self.applied;
        self.gov
            .observe(
                self.applied,
                &[LimitChannelTick {
                    tracked_amp: l_amp,
                    limit_amp_at_f: bound,
                    sysid_h_mag: None,
                }],
            )
            .unwrap();
        let requested = if self.binding {
            self.servo.hold_tick(c_amp)
        } else {
            self.servo.update(c_amp, dt)
        };
        let (applied, binding) = self.gov.apply(requested);
        self.applied = applied;
        self.binding = binding;
        (g_c * applied, g_l * applied)
    }
}

#[test]
fn notch_limit_fight_holds_bound_without_windup_or_release_overshoot() {
    // Control channel gain 0.8 (wants drive 1.25 for target 1.0); limit
    // channel gain 2.0 with bound 1.0 (allows drive 0.5). The governor must
    // win the fight: limit channel held at/below its bound, applied level
    // steady, and NO post-release overshoot because the caller hold_ticks
    // the servo while binding.
    let dt = 0.01;
    let bound = 1.0;
    let mut fx = NotchFixture::new(0.25);

    // Startup ramp: the servo climbs from a low level until the ceiling
    // engages. The limit channel must never exceed its bound even here.
    let mut ramp_ticks = 0;
    while !fx.binding {
        let (_c, l) = fx.tick(0.8, 2.0, bound, dt);
        if ramp_ticks >= 1 {
            assert!(
                l <= bound * 1.001,
                "limit channel exceeded its bound during ramp: {l}"
            );
        }
        ramp_ticks += 1;
        assert!(ramp_ticks < 200, "governor never engaged");
    }

    // The fight window: 60 binding ticks.
    let mut applied_log = Vec::new();
    for i in 0..60 {
        let (_c, l) = fx.tick(0.8, 2.0, bound, dt);
        assert!(
            l <= bound * 1.001,
            "limit channel exceeded its bound at fight tick {i}: {l}"
        );
        applied_log.push(fx.applied);
    }
    assert!(fx.binding, "governor should be binding during the fight");
    // Applied level spread bounded < 1 dB over the fight window.
    let max = applied_log
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let min = applied_log.iter().copied().fold(f64::INFINITY, f64::min);
    let spread_db = 20.0 * (max / min).log10();
    assert!(
        spread_db < 1.0,
        "applied level spread {spread_db} dB >= 1 dB"
    );

    // Release: the limit-channel resonance passes (gain drops to 0.5); the
    // ceiling lifts to 2.0 and the servo ramps back to target.
    let mut peak_c: f64 = 0.0;
    let mut last_c = 0.0;
    for _ in 0..1500 {
        let (c, _l) = fx.tick(0.8, 0.5, bound, dt);
        peak_c = peak_c.max(c);
        last_c = c;
    }
    let overshoot_db = 20.0 * (peak_c / 1.0).log10();
    assert!(
        overshoot_db < 1.0,
        "post-release overshoot {overshoot_db} dB >= 1 dB (windup?)"
    );
    assert_close(last_c, 1.0, 0.02, "control back on target after release");
    assert!(!fx.binding, "governor released");
}

#[test]
fn notch_anti_resonance_engages_no_notch_and_no_nan() {
    // Limit-channel gain ~0: sensitivity -> 0 => allowed scale -> infinity =>
    // NO notch (the SAFE direction - documented so nobody "fixes"
    // it). Exercise both a tiny gain and an exactly-zero gain.
    for g_l in [1e-6, 0.0] {
        let dt = 0.01;
        let mut fx = NotchFixture::new(0.5);
        for _ in 0..800 {
            let (_c, _l) = fx.tick(0.8, g_l, 1.0, dt);
            assert!(!fx.binding, "anti-resonance must not notch (g_l={g_l})");
            assert!(fx.applied.is_finite(), "applied went non-finite");
            assert!(
                fx.gov.ceiling().is_infinite() || fx.gov.ceiling() > 1e5,
                "ceiling should be effectively unlimited, got {}",
                fx.gov.ceiling()
            );
        }
        // The servo reached its target unimpeded.
        assert_close(0.8 * fx.applied, 1.0, 0.02, "control on target");
    }
}

#[test]
fn notch_sensitivity_fallback_below_drive_floor() {
    // Below the drive floor the measured ratio is untrusted; the sysID |H|
    // fallback sets the sensitivity. With no fallback the channel is skipped
    // (no ceiling).
    let cfg = NotchGovernorConfig::default(); // floor 0.01
    let mut gov = NotchGovernor::new(cfg, 1).unwrap();
    gov.observe(
        0.005, // below floor
        &[LimitChannelTick {
            tracked_amp: 123.0, // garbage measured ratio must NOT be used
            limit_amp_at_f: 1.0,
            sysid_h_mag: Some(4.0),
        }],
    )
    .unwrap();
    assert_close(
        gov.smoothed_sensitivity(0).unwrap(),
        4.0,
        1e-12,
        "sysID-seeded sensitivity",
    );
    assert_close(gov.ceiling(), 0.25, 1e-12, "ceiling from sysID fallback");

    let mut gov2 = NotchGovernor::new(cfg, 1).unwrap();
    gov2.observe(
        0.005,
        &[LimitChannelTick {
            tracked_amp: 123.0,
            limit_amp_at_f: 1.0,
            sysid_h_mag: None,
        }],
    )
    .unwrap();
    assert!(
        gov2.ceiling().is_infinite(),
        "no fallback -> channel skipped -> no ceiling"
    );
    assert_eq!(gov2.smoothed_sensitivity(0), None);
}

#[test]
fn notch_sensitivity_is_one_pole_smoothed() {
    let cfg = NotchGovernorConfig::default(); // smoothing_ticks = 3
    let alpha = 1.0 - (-1.0f64 / 3.0).exp();
    let mut gov = NotchGovernor::new(cfg, 1).unwrap();
    let tick = |amp: f64| LimitChannelTick {
        tracked_amp: amp,
        limit_amp_at_f: 1.0,
        sysid_h_mag: None,
    };
    // Seeded directly on the first sample (not smoothed up from zero).
    gov.observe(1.0, &[tick(1.0)]).unwrap();
    assert_close(gov.smoothed_sensitivity(0).unwrap(), 1.0, 1e-12, "seed");
    // A jump to 3.0 moves only alpha of the way per tick.
    gov.observe(1.0, &[tick(3.0)]).unwrap();
    assert_close(
        gov.smoothed_sensitivity(0).unwrap(),
        1.0 + alpha * 2.0,
        1e-12,
        "one-pole step",
    );
}

#[test]
fn notch_governor_validation_and_shape_errors() {
    assert!(NotchGovernor::new(NotchGovernorConfig::default(), 0).is_err());
    assert!(NotchGovernor::new(
        NotchGovernorConfig {
            drive_floor: 0.0,
            ..Default::default()
        },
        1
    )
    .is_err());
    let mut gov = NotchGovernor::new(NotchGovernorConfig::default(), 2).unwrap();
    assert!(matches!(
        gov.observe(
            1.0,
            &[LimitChannelTick {
                tracked_amp: 1.0,
                limit_amp_at_f: 1.0,
                sysid_h_mag: None
            }]
        ),
        Err(ControlError::BadInputs(_))
    ));
    assert_eq!(
        gov.release_ramp_db_per_s(),
        20.0,
        "default release ramp = Slow compression rate"
    );
}

// =============================================================================
// Drive guard + kinematic helpers
// =============================================================================

#[test]
fn drive_guard_verdicts() {
    let g = DriveGuard::new(5.0, 8.0).unwrap();
    assert_eq!(g.check(3.0), DriveVerdict::Ok);
    assert_eq!(g.check(5.0), DriveVerdict::Ok);
    assert_eq!(g.check(6.0), DriveVerdict::Clamped(5.0));
    assert_eq!(g.check(8.0), DriveVerdict::Abort);
    assert_eq!(g.check(9.0), DriveVerdict::Abort);
    assert_eq!(g.check(f64::NAN), DriveVerdict::Abort);
    assert_eq!(g.check(f64::INFINITY), DriveVerdict::Abort);
}

#[test]
fn drive_guard_rejects_bad_config() {
    assert!(DriveGuard::new(0.0, 1.0).is_err());
    assert!(DriveGuard::new(-1.0, 1.0).is_err());
    assert!(DriveGuard::new(5.0, 4.0).is_err(), "abort below ceiling");
    assert!(DriveGuard::new(5.0, f64::NAN).is_err());
}

#[test]
fn kinematic_helpers_match_shm_closed_form() {
    // 1 g pk at 100 Hz: v = G/(2 pi 100), d_pp = 2 G/(2 pi 100)^2.
    let f = 100.0;
    let v = tracked_velocity_ips_pk(1.0, f);
    let d = tracked_displacement_in_pp(1.0, f);
    assert_close(v, G_TO_IN_S2 / (TAU * f), 1e-12, "velocity");
    assert_close(
        d,
        2.0 * G_TO_IN_S2 / (TAU * f).powi(2),
        1e-12,
        "displacement",
    );
}

// =============================================================================
// Compression presets (PAIRS)
// =============================================================================

#[test]
fn compression_presets_are_ki_rate_pairs() {
    assert_eq!(CompressionPreset::Slow.gains(), (2.0, 20.0));
    assert_eq!(CompressionPreset::Fast.gains(), (6.0, 60.0));
    assert_eq!(
        CompressionPreset::Custom {
            ki: 3.5,
            max_rate_db_per_s: 42.0
        }
        .gains(),
        (3.5, 42.0)
    );
}

#[test]
fn compression_preset_serde_round_trip() {
    let p = CompressionPreset::Custom {
        ki: 3.5,
        max_rate_db_per_s: 42.0,
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: CompressionPreset = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}
