//! Grammar, validation, SHM kinematics, crossovers, and alarm/abort band tests
//! for `eigen_sine::profile`.

use eigen_sine::{
    crossover_hz, pct_to_db, ProfileError, ProfileRow, ProfileWarning, Quantity, SineProfile,
    G_TO_IN_S2,
};
use std::f64::consts::TAU;

/// The Irvine classic: low-frequency 1.0 in pk-pk displacement ramp into a
/// 12 g plateau, boundary snapped to the analytic crossover.
fn irvine_profile() -> SineProfile {
    let mut p = SineProfile::new("irvine");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(20.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    let snapped = p.snap_rows_to_crossovers();
    assert_eq!(snapped, vec![1]);
    p
}

/// The analytic accel/displacement crossover for 12 g / 1.0 in pk-pk:
/// `f = (1/2 pi) * sqrt(a / d0)` with `a = 12 * 386.0886 in/s^2` and
/// `d0 = 0.5 in` (zero-to-peak).
fn irvine_crossover() -> f64 {
    let a_in_s2 = 12.0 * G_TO_IN_S2;
    let d0 = 1.0 / 2.0;
    (a_in_s2 / d0).sqrt() / TAU
}

// --- grammar / defaults ---------------------------------------------------------

#[test]
fn quantity_display_units() {
    assert_eq!(Quantity::AccelGPk.to_string(), "g pk");
    assert_eq!(Quantity::VelocityIpsPk.to_string(), "in/s pk");
    assert_eq!(Quantity::DisplacementInPp.to_string(), "in pk-pk");
}

#[test]
fn default_global_bands() {
    let p = SineProfile::new("x");
    assert_eq!(p.global_alarm_db, (1.5, 1.5));
    assert_eq!(p.global_abort_db, (3.0, 3.0));
    assert_eq!(SineProfile::default().global_alarm_db, (1.5, 1.5));
}

// --- validation: hard errors ------------------------------------------------------

#[test]
fn too_few_rows_is_error() {
    let p = SineProfile::new("x");
    assert_eq!(p.validate(), Err(ProfileError::TooFewRows(0)));
    let mut p1 = SineProfile::new("x");
    p1.push(ProfileRow::accel(10.0, 1.0));
    assert_eq!(p1.validate(), Err(ProfileError::TooFewRows(1)));
}

#[test]
fn non_positive_freq_is_error() {
    for bad in [0.0, -5.0, f64::NAN] {
        let mut p = SineProfile::new("x");
        p.push(ProfileRow::accel(bad, 1.0));
        p.push(ProfileRow::accel(100.0, 1.0));
        assert!(
            matches!(
                p.validate(),
                Err(ProfileError::NonPositiveFreq { row: 0, .. })
            ),
            "freq {bad} should be a NonPositiveFreq error"
        );
    }
}

#[test]
fn non_monotonic_freq_is_error() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(100.0, 1.0));
    p.push(ProfileRow::accel(50.0, 2.0));
    assert!(matches!(
        p.validate(),
        Err(ProfileError::NonMonotonicFreq { row: 1, .. })
    ));
    // Equal frequencies are also non-monotonic.
    let mut p2 = SineProfile::new("x");
    p2.push(ProfileRow::accel(100.0, 1.0));
    p2.push(ProfileRow::accel(100.0, 2.0));
    assert!(matches!(
        p2.validate(),
        Err(ProfileError::NonMonotonicFreq { .. })
    ));
}

#[test]
fn non_positive_value_is_error() {
    for bad in [0.0, -1.0, f64::NAN] {
        let mut p = SineProfile::new("x");
        p.push(ProfileRow::accel(10.0, 1.0));
        p.push(ProfileRow::accel(100.0, bad));
        assert!(
            matches!(
                p.validate(),
                Err(ProfileError::NonPositiveValue { row: 1, .. })
            ),
            "value {bad} should be a NonPositiveValue error"
        );
    }
}

// --- validation: warnings ---------------------------------------------------------

#[test]
fn alarm_wider_than_abort_per_row_warns() {
    let mut p = SineProfile::new("x");
    // Row override: alarm (4, 4) dB is wider than the global abort (3, 3) dB.
    p.push(ProfileRow::accel(10.0, 1.0).with_alarm_db(4.0, 4.0));
    p.push(ProfileRow::accel(100.0, 1.0));
    let report = p.validate_report();
    assert!(report.is_ok());
    assert!(report
        .warnings
        .contains(&ProfileWarning::AlarmWiderThanAbort { row: 0 }));
}

#[test]
fn alarm_wider_on_one_side_only_warns() {
    let mut p = SineProfile::new("x");
    p.push(
        ProfileRow::accel(10.0, 1.0)
            .with_alarm_db(1.0, 5.0)
            .with_abort_db(3.0, 3.0),
    );
    p.push(ProfileRow::accel(100.0, 1.0));
    let report = p.validate_report();
    assert!(report
        .warnings
        .contains(&ProfileWarning::AlarmWiderThanAbort { row: 0 }));
}

#[test]
fn global_alarm_wider_than_abort_warns_once() {
    let mut p = SineProfile::new("x");
    p.global_alarm_db = (6.0, 6.0);
    p.global_abort_db = (3.0, 3.0);
    p.push(ProfileRow::accel(10.0, 1.0));
    p.push(ProfileRow::accel(100.0, 1.0));
    p.push(ProfileRow::accel(1000.0, 1.0));
    let report = p.validate_report();
    let count = report
        .warnings
        .iter()
        .filter(|w| matches!(w, ProfileWarning::GlobalAlarmWiderThanAbort))
        .count();
    assert_eq!(count, 1);
}

#[test]
fn narrow_alarm_no_warning() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 1.0).with_alarm_db(1.0, 1.0));
    p.push(ProfileRow::accel(100.0, 1.0));
    assert!(p.validate_report().warnings.is_empty());
}

#[test]
fn join_discontinuity_warns_above_half_db() {
    // Displacement segment held to 20 Hz where the accel plateau starts: the
    // displacement side demands (2 pi 20)^2 * 0.5 / G = 20.45 g vs the 12 g
    // plateau -> +4.6 dB discontinuity at row 1.
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(20.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    let report = p.validate_report();
    assert!(report.is_ok());
    let Some(ProfileWarning::JoinDiscontinuity { row, delta_db }) = report
        .warnings
        .iter()
        .find(|w| matches!(w, ProfileWarning::JoinDiscontinuity { .. }))
    else {
        panic!("expected a JoinDiscontinuity warning, got {report:?}");
    };
    assert_eq!(*row, 1);
    let expected_db = 20.0 * ((accel_of_disp(1.0, 20.0)) / 12.0).log10();
    assert!((delta_db - expected_db).abs() < 1e-9, "delta {delta_db}");
}

#[test]
fn snapped_profile_has_no_join_discontinuity_warning() {
    let p = irvine_profile();
    let report = p.validate_report();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, ProfileWarning::JoinDiscontinuity { .. })),
        "snapped joins must be continuous: {report:?}"
    );
}

#[test]
fn same_quantity_joins_never_warn_discontinuity() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 1.0));
    p.push(ProfileRow::accel(100.0, 10.0));
    p.push(ProfileRow::accel(1000.0, 2.0));
    assert!(p.validate_report().warnings.is_empty());
}

// --- SHM helper for tests ---------------------------------------------------------

/// Acceleration in g pk demanded by `d_pp` in pk-pk at `f`.
fn accel_of_disp(d_pp: f64, f: f64) -> f64 {
    (TAU * f).powi(2) * (d_pp / 2.0) / G_TO_IN_S2
}

/// Acceleration in g pk demanded by `v` in/s pk at `f`.
fn accel_of_vel(v: f64, f: f64) -> f64 {
    TAU * f * v / G_TO_IN_S2
}

// --- evaluation -------------------------------------------------------------------

#[test]
fn constant_accel_segment_targets() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    for f in [10.0, 55.5, 1000.0, 2000.0] {
        assert_eq!(p.target_accel_g_pk(f), Some(12.0));
        assert_eq!(p.target_in_quantity(f), Some((Quantity::AccelGPk, 12.0)));
    }
}

#[test]
fn velocity_row_converts_via_shm() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::velocity(10.0, 5.0));
    p.push(ProfileRow::velocity(100.0, 5.0));
    let f = 25.0;
    let a = p.target_accel_g_pk(f).unwrap();
    assert!((a - accel_of_vel(5.0, f)).abs() < 1e-12);
    // Round trip: derived velocity equals the entered value.
    assert!((p.velocity_ips_pk(f).unwrap() - 5.0).abs() < 1e-12);
}

#[test]
fn displacement_row_converts_via_shm() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::displacement(15.0, 1.0));
    let f = 10.0;
    let a = p.target_accel_g_pk(f).unwrap();
    assert!((a - accel_of_disp(1.0, f)).abs() < 1e-12);
    assert!((p.displacement_in_pp(f).unwrap() - 1.0).abs() < 1e-12);
}

#[test]
fn same_quantity_rows_interpolate_log_log() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 1.0));
    p.push(ProfileRow::accel(100.0, 10.0));
    // Log-log line from (10, 1) to (100, 10): value(f) = f/10 exactly (n = 1).
    for f in [10.0, 31.622776601683793, 50.0, 100.0] {
        let (q, v) = p.target_in_quantity(f).unwrap();
        assert_eq!(q, Quantity::AccelGPk);
        assert!((v - f / 10.0).abs() < 1e-9, "f={f}, v={v}");
    }
}

#[test]
fn differing_quantities_hold_left_constant() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(20.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    // Everywhere in [5, 20): governing quantity is displacement, constant 1.0.
    for f in [5.0, 10.0, 19.999] {
        assert_eq!(
            p.target_in_quantity(f),
            Some((Quantity::DisplacementInPp, 1.0))
        );
        let a = p.target_accel_g_pk(f).unwrap();
        assert!((a - accel_of_disp(1.0, f)).abs() < 1e-12);
    }
    // At the join, the RIGHT segment governs.
    assert_eq!(p.target_in_quantity(20.0), Some((Quantity::AccelGPk, 12.0)));
}

#[test]
fn out_of_band_is_none() {
    let p = irvine_profile();
    assert_eq!(p.target_accel_g_pk(4.999), None);
    assert_eq!(p.target_accel_g_pk(2000.001), None);
    assert_eq!(p.target_accel_g_pk(f64::NAN), None);
    assert_eq!(p.target_in_quantity(0.0), None);
    assert_eq!(p.velocity_ips_pk(-1.0), None);
    assert_eq!(p.displacement_in_pp(1e9), None);
}

#[test]
fn band_upper_edge_is_in_band() {
    let p = irvine_profile();
    assert_eq!(p.target_accel_g_pk(2000.0), Some(12.0));
}

#[test]
fn derived_kinematics_match_formulas() {
    let p = irvine_profile();
    let f = 1000.0;
    let a = p.target_accel_g_pk(f).unwrap(); // 12 g plateau
    let v = p.velocity_ips_pk(f).unwrap();
    let d = p.displacement_in_pp(f).unwrap();
    assert!((v - a * G_TO_IN_S2 / (TAU * f)).abs() < 1e-12);
    assert!((d - 2.0 * a * G_TO_IN_S2 / (TAU * f).powi(2)).abs() < 1e-12);
}

// --- crossovers -------------------------------------------------------------------

#[test]
fn irvine_accel_disp_crossover_golden() {
    // Irvine: 12 g plateau + 1.0 in pk-pk displacement ramp. With
    // a = 12 * 386.0886 = 4633.06 in/s^2 and d_zero_to_peak = 0.5 in,
    // f_cross = (1/2 pi) * sqrt(a / d0) ~ 15.3 Hz.
    let left = ProfileRow::displacement(5.0, 1.0);
    let right = ProfileRow::accel(2000.0, 12.0);
    let f = crossover_hz(&left, &right).unwrap();
    let expected = irvine_crossover();
    assert!(
        (f - expected).abs() < 0.01,
        "crossover {f} Hz vs analytic {expected} Hz"
    );
    // Absolute anchor (the expected ~15.3 Hz).
    assert!((15.2..15.4).contains(&f), "crossover {f} Hz");
    // Order-independent.
    assert_eq!(crossover_hz(&right, &left), Some(f));
}

#[test]
fn accel_vel_crossover_matches_formula() {
    // f = a / (2 pi v), a in in/s^2.
    let left = ProfileRow::velocity(5.0, 10.0);
    let right = ProfileRow::accel(2000.0, 6.0);
    let f = crossover_hz(&left, &right).unwrap();
    let expected = 6.0 * G_TO_IN_S2 / (TAU * 10.0);
    assert!((f - expected).abs() < 1e-9);
    // At the crossover the two specs demand the same acceleration.
    let a_from_v = accel_of_vel(10.0, f);
    assert!((a_from_v - 6.0).abs() < 1e-9);
}

#[test]
fn vel_disp_crossover_matches_formula() {
    // f = v / (2 pi d0).
    let left = ProfileRow::displacement(1.0, 2.0);
    let right = ProfileRow::velocity(100.0, 30.0);
    let f = crossover_hz(&left, &right).unwrap();
    let expected = 30.0 / (TAU * 1.0); // d0 = 1.0
    assert!((f - expected).abs() < 1e-9);
}

#[test]
fn crossover_same_quantity_or_bad_values_is_none() {
    let a = ProfileRow::accel(10.0, 1.0);
    let b = ProfileRow::accel(100.0, 2.0);
    assert_eq!(crossover_hz(&a, &b), None);
    let zero = ProfileRow::displacement(5.0, 0.0);
    assert_eq!(crossover_hz(&zero, &b), None);
    let nan = ProfileRow::velocity(5.0, f64::NAN);
    assert_eq!(crossover_hz(&nan, &b), None);
}

#[test]
fn snap_moves_boundary_to_crossover() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(20.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    let snapped = p.snap_rows_to_crossovers();
    assert_eq!(snapped, vec![1]);
    assert!((p.rows[1].freq_hz - irvine_crossover()).abs() < 1e-9);
    // Idempotent: a second snap changes nothing.
    assert!(p.snap_rows_to_crossovers().is_empty());
}

#[test]
fn snap_respects_monotonicity() {
    // The 12 g / 1.0 in pk-pk crossover (~15.3 Hz) is ABOVE the following
    // row's 10 Hz, so snapping row 1 would break monotonicity: leave it.
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(8.0, 12.0));
    p.push(ProfileRow::accel(10.0, 12.0));
    let snapped = p.snap_rows_to_crossovers();
    assert!(snapped.is_empty());
    assert_eq!(p.rows[1].freq_hz, 8.0);
}

// --- alarm / abort bands ------------------------------------------------------------

#[test]
fn global_bands_scale_the_accel_target() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 10.0));
    p.push(ProfileRow::accel(100.0, 10.0));
    let (alo, ahi) = p.alarm_bounds(50.0).unwrap();
    assert!((alo - 10.0 * 10f64.powf(-1.5 / 20.0)).abs() < 1e-12);
    assert!((ahi - 10.0 * 10f64.powf(1.5 / 20.0)).abs() < 1e-12);
    let (blo, bhi) = p.abort_bounds(50.0).unwrap();
    assert!((blo - 10.0 * 10f64.powf(-3.0 / 20.0)).abs() < 1e-12);
    assert!((bhi - 10.0 * 10f64.powf(3.0 / 20.0)).abs() < 1e-12);
    // Alarm sits inside abort.
    assert!(blo < alo && ahi < bhi);
}

#[test]
fn per_row_override_governs_its_segment() {
    let mut p = SineProfile::new("x");
    p.push(ProfileRow::accel(10.0, 10.0).with_alarm_db(1.0, 2.0));
    p.push(ProfileRow::accel(100.0, 10.0));
    p.push(ProfileRow::accel(1000.0, 10.0));
    // Segment 0 (LEFT row 0 override).
    let (lo, hi) = p.alarm_bounds(50.0).unwrap();
    assert!((lo - 10.0 * 10f64.powf(-1.0 / 20.0)).abs() < 1e-12);
    assert!((hi - 10.0 * 10f64.powf(2.0 / 20.0)).abs() < 1e-12);
    // Segment 1 (row 1 has no override -> global).
    let (lo2, hi2) = p.alarm_bounds(500.0).unwrap();
    assert!((lo2 - 10.0 * 10f64.powf(-1.5 / 20.0)).abs() < 1e-12);
    assert!((hi2 - 10.0 * 10f64.powf(1.5 / 20.0)).abs() < 1e-12);
    // Abort still global on both segments.
    let (blo, _) = p.abort_bounds(50.0).unwrap();
    assert!((blo - 10.0 * 10f64.powf(-3.0 / 20.0)).abs() < 1e-12);
}

#[test]
fn bounds_out_of_band_none() {
    let p = irvine_profile();
    assert_eq!(p.alarm_bounds(1.0), None);
    assert_eq!(p.abort_bounds(3000.0), None);
}

// --- percent -> dB (asymmetric) --------------------------------------------------

#[test]
fn pct_to_db_mil_preset_golden() {
    // MIL +/-10%: minus = -20*log10(0.9) = 0.9151 dB, plus = 20*log10(1.1)
    // = 0.8279 dB (asymmetric).
    let (minus, plus) = pct_to_db(10.0);
    assert!((minus - 0.9151).abs() < 1e-4, "minus {minus}");
    assert!((plus - 0.8279).abs() < 1e-4, "plus {plus}");
}

#[test]
fn pct_to_db_is_asymmetric_and_guarded() {
    let (minus, plus) = pct_to_db(25.0);
    assert!(minus > plus, "minus side must be wider in dB");
    assert!((minus + 20.0 * 0.75f64.log10()).abs() < 1e-12);
    assert!((plus - 20.0 * 1.25f64.log10()).abs() < 1e-12);
    assert_eq!(pct_to_db(0.0), (0.0, 0.0));
    assert_eq!(pct_to_db(-5.0), (0.0, 0.0));
    assert_eq!(pct_to_db(f64::NAN), (0.0, 0.0));
    let (m100, p100) = pct_to_db(100.0);
    assert!(m100.is_infinite() && p100.is_finite());
}
