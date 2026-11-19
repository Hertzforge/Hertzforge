//! Deterministic sine feasibility tests: pass/exceed fixtures, the
//! displacement-limited low-frequency case, and rating-basis handling.

use hertzforge_shaker_db::{
    MovingMass, Quantity as DbQuantity, RatingValue, ShakerRecord, Verdict,
};
use hertzforge_sine::{assess_sine, ProfileRow, SineProfile, G_TO_IN_S2};
use std::f64::consts::{SQRT_2, TAU};

/// Irvine classic: 1.0 in pk-pk displacement ramp into a 12 g plateau to
/// 2 kHz, boundary snapped to the analytic crossover (~15.3 Hz).
fn irvine_profile() -> SineProfile {
    let mut p = SineProfile::new("irvine");
    p.push(ProfileRow::displacement(5.0, 1.0));
    p.push(ProfileRow::accel(20.0, 12.0));
    p.push(ProfileRow::accel(2000.0, 12.0));
    assert_eq!(p.snap_rows_to_crossovers(), vec![1]);
    p
}

/// 100 lb moving-mass stack.
fn mass_100lb() -> MovingMass {
    MovingMass {
        armature_lb: 50.0,
        slip_table_lb: 0.0,
        drive_adapter_lb: 10.0,
        fixturing_lb: 20.0,
        test_article_lb: 20.0,
    }
}

fn item(
    report: &hertzforge_shaker_db::FeasibilityReport,
    q: DbQuantity,
) -> &hertzforge_shaker_db::FeasibilityItem {
    report
        .items
        .iter()
        .find(|i| i.quantity == q)
        .unwrap_or_else(|| panic!("no {q:?} item in {report:?}"))
}

/// Worst-case band kinematics of the Irvine fixture, computed analytically:
/// the maxima all sit at the displacement/accel crossover or the plateau.
fn irvine_worst() -> (f64, f64, f64) {
    let f_cross = (12.0 * G_TO_IN_S2 / 0.5).sqrt() / TAU;
    let a_pk = 12.0; // g, on the plateau
    let v_pk = 12.0 * G_TO_IN_S2 / (TAU * f_cross); // in/s, at the crossover
    let d_pp = 1.0; // in, on the displacement ramp
    (a_pk, v_pk, d_pp)
}

// --- pass / exceed fixtures ------------------------------------------------------

#[test]
fn big_shaker_passes() {
    let record = ShakerRecord {
        name: "big".into(),
        force_sine: Some(RatingValue::peak(40_000.0)),
        max_velocity_ips: Some(RatingValue::peak(100.0)),
        max_displacement_in_pp: Some(RatingValue::peak(3.0)),
        max_accel_bare_g: Some(RatingValue::peak(100.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &irvine_profile());
    assert_eq!(report.overall, Verdict::Pass, "{report:?}");
    for i in &report.items {
        assert_eq!(i.verdict, Verdict::Pass, "{i:?}");
    }
    // The three pinned assumption strings are present.
    for s in [
        "deterministic sine (crest factor 1.0)",
        "per-frequency analytic maxima at segment endpoints",
        "SineEquivalent force basis treated as native (full verdict)",
    ] {
        assert!(
            report.assumptions.iter().any(|a| a == s),
            "missing assumption {s:?} in {:?}",
            report.assumptions
        );
    }
    // Force demand = m_total * a_pk = 100 lb * 12 g = 1200 lbf.
    let force = item(&report, DbQuantity::Force);
    assert!((force.computed_value - 1200.0).abs() < 1e-9);
    assert!((force.margin_ratio.unwrap() - 40_000.0 / 1200.0).abs() < 1e-9);
}

#[test]
fn small_shaker_exceeded() {
    let record = ShakerRecord {
        name: "small".into(),
        force_sine: Some(RatingValue::peak(500.0)),
        max_velocity_ips: Some(RatingValue::peak(30.0)),
        max_displacement_in_pp: Some(RatingValue::peak(0.5)),
        max_accel_bare_g: Some(RatingValue::peak(100.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &irvine_profile());
    assert_eq!(report.overall, Verdict::Exceeded);
    assert!(report.exceeded());
    assert_eq!(item(&report, DbQuantity::Force).verdict, Verdict::Exceeded);
    assert_eq!(
        item(&report, DbQuantity::Velocity).verdict,
        Verdict::Exceeded
    );
    assert_eq!(
        item(&report, DbQuantity::Displacement).verdict,
        Verdict::Exceeded
    );
    assert_eq!(
        item(&report, DbQuantity::Acceleration).verdict,
        Verdict::Pass
    );
}

#[test]
fn velocity_maximum_is_at_the_crossover() {
    let record = ShakerRecord {
        name: "v".into(),
        max_velocity_ips: Some(RatingValue::peak(100.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &irvine_profile());
    let (_, v_pk, _) = irvine_worst();
    let vel = item(&report, DbQuantity::Velocity);
    assert!(
        (vel.computed_value - v_pk).abs() < 1e-9,
        "velocity demand {} vs analytic {v_pk}",
        vel.computed_value
    );
    // ~48 in/s for the 12 g / 1.0 in pk-pk classic.
    assert!((48.0..48.3).contains(&vel.computed_value));
}

#[test]
fn displacement_limited_low_frequency_profile_exceeds_small_stroke() {
    // 0.8 in pk-pk from 2 to 10 Hz: displacement-limited everywhere.
    let mut p = SineProfile::new("lowf");
    p.push(ProfileRow::displacement(2.0, 0.8));
    p.push(ProfileRow::displacement(10.0, 0.8));
    let record = ShakerRecord {
        name: "short-stroke".into(),
        force_sine: Some(RatingValue::peak(10_000.0)),
        max_velocity_ips: Some(RatingValue::peak(100.0)),
        max_displacement_in_pp: Some(RatingValue::peak(0.5)),
        max_accel_bare_g: Some(RatingValue::peak(100.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &p);
    assert_eq!(report.overall, Verdict::Exceeded);
    let disp = item(&report, DbQuantity::Displacement);
    assert_eq!(disp.verdict, Verdict::Exceeded);
    assert!((disp.computed_value - 0.8).abs() < 1e-12);
    assert!((disp.margin_ratio.unwrap() - 0.5 / 0.8).abs() < 1e-12);
    // Accel demand peaks at the upper band edge: (2 pi 10)^2 * 0.4 / G ~ 4.09 g.
    let a_expected = (TAU * 10.0).powi(2) * 0.4 / G_TO_IN_S2;
    let acc = item(&report, DbQuantity::Acceleration);
    assert!((acc.computed_value - a_expected).abs() < 1e-9);
    assert_eq!(acc.verdict, Verdict::Pass);
}

// --- basis handling ------------------------------------------------------------------

#[test]
fn rms_rating_compares_peak_over_sqrt2() {
    // F_pk = 1200 lbf -> F_rms = 848.5 lbf. An RMS rating of 900 lbf passes
    // even though a naive peak comparison (900 < 1200) would fail.
    let record = ShakerRecord {
        name: "rms".into(),
        force_sine: Some(RatingValue::rms(900.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &irvine_profile());
    let force = item(&report, DbQuantity::Force);
    assert_eq!(force.verdict, Verdict::Pass);
    assert!((force.computed_value - 1200.0 / SQRT_2).abs() < 1e-9);
    assert!(force.label.contains("RMS"), "{}", force.label);
    // And an RMS rating below the RMS demand still fails.
    let record2 = ShakerRecord {
        name: "rms2".into(),
        force_sine: Some(RatingValue::rms(800.0)),
        ..Default::default()
    };
    let report2 = assess_sine(&record2, &mass_100lb(), &irvine_profile());
    assert_eq!(item(&report2, DbQuantity::Force).verdict, Verdict::Exceeded);
}

#[test]
fn sine_equivalent_basis_gets_full_verdict_not_advisory() {
    // SineEquivalent is the NATIVE basis for sine (deliberately unlike the
    // random module, where it is capped at Advisory).
    let fail = ShakerRecord {
        name: "se-fail".into(),
        force_sine: Some(RatingValue::sine_equivalent(1000.0)),
        ..Default::default()
    };
    let report = assess_sine(&fail, &mass_100lb(), &irvine_profile());
    assert_eq!(item(&report, DbQuantity::Force).verdict, Verdict::Exceeded);
    assert!(!report.has_advisory());

    let pass = ShakerRecord {
        name: "se-pass".into(),
        force_sine: Some(RatingValue::sine_equivalent(2000.0)),
        ..Default::default()
    };
    let report2 = assess_sine(&pass, &mass_100lb(), &irvine_profile());
    let force = item(&report2, DbQuantity::Force);
    assert_eq!(force.verdict, Verdict::Pass);
    assert!((force.computed_value - 1200.0).abs() < 1e-9);
}

#[test]
fn missing_ratings_are_not_evaluated() {
    let record = ShakerRecord::new("bare");
    let report = assess_sine(&record, &mass_100lb(), &irvine_profile());
    assert_eq!(report.items.len(), 4);
    for i in &report.items {
        assert_eq!(i.verdict, Verdict::NotEvaluated, "{i:?}");
        assert_eq!(i.rating_value, None);
    }
    assert_eq!(report.overall, Verdict::NotEvaluated);
}

#[test]
fn invalid_profile_yields_not_evaluated_report() {
    let mut p = SineProfile::new("bad");
    p.push(ProfileRow::accel(10.0, 1.0)); // only one row
    let record = ShakerRecord {
        name: "any".into(),
        force_sine: Some(RatingValue::peak(1000.0)),
        ..Default::default()
    };
    let report = assess_sine(&record, &mass_100lb(), &p);
    assert_eq!(report.overall, Verdict::NotEvaluated);
    assert!(report.items.is_empty());
    assert!(report
        .assumptions
        .iter()
        .any(|a| a.contains("failed validation")));
}
