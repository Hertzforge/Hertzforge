//! Deterministic-feasibility gate tests + the placeholder
//! double-sided compensation helper.

use crate::feasibility::{
    naive_double_sided_compensation, pulse_feasibility, CompensationError, FeasibilityTolerances,
};
use eigen_dsp::shock::{pulse, pulse_kinematics};
use eigen_shaker_db::{MovingMass, Quantity, RatingBasis, RatingValue, ShakerRecord, Verdict};

const FS: f64 = 5120.0;

/// A big shaker: high shock force, generous velocity and long stroke. The 20 g/11 ms
/// compensated pulse passes here.
fn big_shaker() -> ShakerRecord {
    let mut r = ShakerRecord::new("Big-T5000-class");
    r.force_shock = Some(RatingValue::peak(60_000.0)); // lbf
    r.max_velocity_ips = Some(RatingValue::peak(100.0)); // in/s
    r.max_displacement_in_pp = Some(RatingValue::peak(4.0)); // in pk-pk (long stroke)
    r.max_accel_bare_g = Some(RatingValue::peak(100.0)); // g
    r
}

/// A small-stroke shaker: adequate force/velocity but a SHORT stroke that the
/// compensated pulse's shoulders violate (displacement hard-block).
fn small_stroke_shaker() -> ShakerRecord {
    let mut r = ShakerRecord::new("Small-stroke");
    r.force_shock = Some(RatingValue::peak(60_000.0));
    r.max_velocity_ips = Some(RatingValue::peak(100.0));
    r.max_displacement_in_pp = Some(RatingValue::peak(0.1)); // very short stroke
    r.max_accel_bare_g = Some(RatingValue::peak(100.0));
    r
}

fn small_mass() -> MovingMass {
    MovingMass {
        armature_lb: 200.0,
        ..Default::default()
    }
}

/// Build the canonical compensated pulse: a 20 g / 11 ms half-sine with
/// double-sided rectangular compensation. Verify (here) that the compensation zeroed
/// the residuals so the fixtures rest on a properly-compensated pulse.
fn compensated_20g_11ms() -> Vec<f64> {
    let main = pulse::half_sine(20.0, 11.0, FS);
    naive_double_sided_compensation(&main, 0.5, 0.5, 0.5).expect("compensation solves")
}

// ---------------------------------------------------------------------------
// Compensation helper (the placeholder) - the FIXTURE it rests on.
// ---------------------------------------------------------------------------

#[test]
fn double_sided_compensation_zeros_net_velocity_and_displacement() {
    let comp = compensated_20g_11ms();
    let kin = pulse_kinematics(&comp, FS);
    // Both residuals must be ~0 (the whole point of compensation).
    assert!(
        kin.residual_velocity_ips.abs() < 1e-6,
        "residual velocity ~0, got {}",
        kin.residual_velocity_ips
    );
    assert!(
        kin.residual_disp_in.abs() < 1e-8,
        "residual displacement ~0, got {}",
        kin.residual_disp_in
    );
}

#[test]
fn compensation_rejects_degenerate_and_bad_inputs() {
    let main = pulse::half_sine(20.0, 11.0, FS);
    // Empty pulse.
    assert!(matches!(
        naive_double_sided_compensation(&[], 0.5, 0.5, 0.5),
        Err(CompensationError::EmptyPulse)
    ));
    // Bad amplitude fraction.
    assert!(matches!(
        naive_double_sided_compensation(&main, 0.5, 0.5, 0.0),
        Err(CompensationError::BadAmplitudeFraction(_))
    ));
    assert!(matches!(
        naive_double_sided_compensation(&main, 0.5, 0.5, 1.5),
        Err(CompensationError::BadAmplitudeFraction(_))
    ));
    // Both shoulders zero -> cannot compensate.
    assert!(matches!(
        naive_double_sided_compensation(&main, 0.0, 0.0, 0.5),
        Err(CompensationError::Degenerate(_))
    ));
    // Only one shoulder -> singular (cannot zero both conditions).
    assert!(matches!(
        naive_double_sided_compensation(&main, 0.5, 0.0, 0.5),
        Err(CompensationError::Degenerate(_))
    ));
}

// ---------------------------------------------------------------------------
// Feasibility fixtures.
// ---------------------------------------------------------------------------

#[test]
fn compensated_pulse_passes_on_big_shaker() {
    let comp = compensated_20g_11ms();
    let report = pulse_feasibility(
        &big_shaker(),
        &small_mass(),
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    assert_eq!(
        report.overall,
        Verdict::Pass,
        "big shaker passes; report: {report:#?}"
    );
    assert!(!report.exceeded());
    // Every evaluated item passes.
    for item in &report.items {
        assert!(
            matches!(
                item.verdict,
                Verdict::Pass | Verdict::NotEvaluated | Verdict::Advisory
            ),
            "item {} unexpectedly {:?}",
            item.label,
            item.verdict
        );
    }
    // The compensation-quality gate passed (residuals ~0).
    let comp_item = report
        .items
        .iter()
        .find(|i| i.label.starts_with("compensation quality"))
        .expect("compensation-quality item present");
    assert_eq!(comp_item.verdict, Verdict::Pass);
}

#[test]
fn compensated_pulse_hard_blocks_on_small_stroke_shaker_displacement() {
    let comp = compensated_20g_11ms();
    let kin = pulse_kinematics(&comp, FS);
    // Sanity: the compensated pulse's stroke exceeds the small shaker's 0.1 in.
    assert!(
        kin.pk_pk_disp_in > 0.1,
        "compensated stroke {} in must exceed the small stroke limit",
        kin.pk_pk_disp_in
    );
    let report = pulse_feasibility(
        &small_stroke_shaker(),
        &small_mass(),
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    assert_eq!(
        report.overall,
        Verdict::Exceeded,
        "small-stroke shaker hard-blocks"
    );
    // The displacement item is the one that exceeded.
    let disp = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Displacement)
        .unwrap();
    assert_eq!(disp.verdict, Verdict::Exceeded);
}

#[test]
fn uncompensated_pulse_fails_compensation_quality_gate_f5() {
    // An UNcompensated half-sine leaves residual velocity = 2AT/pi (nonzero); the
    // compensation-quality gate must flag it Exceeded (a pre-arm block), even on
    // the big shaker where force/velocity/stroke would otherwise pass.
    let uncompensated = pulse::half_sine(20.0, 11.0, FS);
    let kin = pulse_kinematics(&uncompensated, FS);
    let g_to_in_s2 = 386.0886;
    let expected_resid_v = g_to_in_s2 * 20.0 * 2.0 * 0.011 / std::f64::consts::PI;
    // 3% band absorbs the discrete-pulse (N-1 intervals) vs continuous-T gap.
    assert!(
        (kin.residual_velocity_ips - expected_resid_v).abs() / expected_resid_v < 0.03,
        "uncompensated residual velocity = 2AT/pi ~ {expected_resid_v}, got {}",
        kin.residual_velocity_ips
    );
    let report = pulse_feasibility(
        &big_shaker(),
        &small_mass(),
        &uncompensated,
        FS,
        FeasibilityTolerances::default(),
    );
    assert_eq!(
        report.overall,
        Verdict::Exceeded,
        "uncompensated pulse is pre-arm blocked"
    );
    let comp_item = report
        .items
        .iter()
        .find(|i| i.label.starts_with("compensation quality"))
        .unwrap();
    assert_eq!(comp_item.verdict, Verdict::Exceeded);
}

#[test]
fn force_shock_is_consumed_and_basis_flip_changes_the_verdict() {
    // A pulse whose peak-accel demand sits BETWEEN a Peak rating (passes) and the same
    // number treated as an Rms rating. The basis flip changes the verdict:
    //   - force_shock as Peak: F_peak/m compared directly -> Pass.
    //   - force_shock as Rms:  approximate basis for a deterministic pulse -> Advisory
    //     (never a hard Pass we can stand behind), demonstrating force_shock is
    //     consumed by its RatingBasis.
    let comp = compensated_20g_11ms();
    let kin = pulse_kinematics(&comp, FS);
    let mass = small_mass();
    let m_total = mass.total();
    // Set force_shock so F/m is comfortably above the peak accel (a clear pass on the
    // Peak basis). Use a generous stroke + velocity so ONLY the force item's basis
    // matters.
    let force_val = (kin.peak_accel_g * 2.0) * m_total; // ceiling = 2x demand
    let mut base = ShakerRecord::new("basis-flip");
    base.max_velocity_ips = Some(RatingValue::peak(1000.0));
    base.max_displacement_in_pp = Some(RatingValue::peak(100.0));
    // No bare-accel rating, so the force-derived ceiling is the binding one.

    let mut peak_rec = base.clone();
    peak_rec.force_shock = Some(RatingValue::peak(force_val));
    let peak_report = pulse_feasibility(
        &peak_rec,
        &mass,
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    let peak_accel_item = peak_report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Acceleration && i.label.contains("peak acceleration"))
        .unwrap();
    assert_eq!(
        peak_accel_item.verdict,
        Verdict::Pass,
        "Peak-basis force -> Pass"
    );

    let mut rms_rec = base;
    rms_rec.force_shock = Some(RatingValue::rms(force_val));
    let rms_report =
        pulse_feasibility(&rms_rec, &mass, &comp, FS, FeasibilityTolerances::default());
    let rms_accel_item = rms_report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Acceleration && i.label.contains("peak acceleration"))
        .unwrap();
    assert_eq!(
        rms_accel_item.verdict,
        Verdict::Advisory,
        "Rms-basis force on a deterministic pulse -> Advisory (basis flip changed the verdict)"
    );
    assert_eq!(rms_accel_item.rating_basis, Some(RatingBasis::Peak)); // ceiling reported as a peak bound
}

#[test]
fn no_force_shock_rating_marks_accel_not_evaluated_when_no_bare_rating() {
    // If neither force_shock nor bare-accel is published, the accel ceiling is
    // NotEvaluated (no assumption invented).
    let comp = compensated_20g_11ms();
    let mut r = ShakerRecord::new("bare-record");
    r.max_velocity_ips = Some(RatingValue::peak(1000.0));
    r.max_displacement_in_pp = Some(RatingValue::peak(100.0));
    let report = pulse_feasibility(
        &r,
        &small_mass(),
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    let accel = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Acceleration && i.label.contains("peak acceleration"))
        .unwrap();
    assert_eq!(accel.verdict, Verdict::NotEvaluated);
    // But the compensation-quality gate still runs (it does not need a rating).
    assert!(report
        .items
        .iter()
        .any(|i| i.label.starts_with("compensation quality")));
}

#[test]
fn assumptions_list_documents_crest_units_and_static_ratings() {
    let comp = compensated_20g_11ms();
    let report = pulse_feasibility(
        &big_shaker(),
        &small_mass(),
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    let joined = report.assumptions.join("\n");
    assert!(
        joined.contains("crest factor = 1.00"),
        "crest 1.0 documented"
    );
    assert!(
        joined.contains("in / g / lbf") || joined.contains("F11"),
        "units documented"
    );
    assert!(
        joined.contains("Static ratings only"),
        "static-only documented"
    );
    assert!(
        joined.contains("no high-pass") || joined.contains("F5"),
        "pure integrator documented"
    );
}

#[test]
fn report_round_trips_through_json() {
    let comp = compensated_20g_11ms();
    let report = pulse_feasibility(
        &big_shaker(),
        &small_mass(),
        &comp,
        FS,
        FeasibilityTolerances::default(),
    );
    let js = serde_json::to_string(&report).unwrap();
    let back: eigen_shaker_db::FeasibilityReport = serde_json::from_str(&js).unwrap();
    // serde_json's f64 parse can land an ULP off the serialized value in this build, so
    // assert SEMANTIC round-trip (structure + verdicts + approximate margins) rather
    // than bit-exact struct equality.
    assert_eq!(back.overall, report.overall);
    assert_eq!(back.items.len(), report.items.len());
    for (b, r) in back.items.iter().zip(report.items.iter()) {
        assert_eq!(b.quantity, r.quantity);
        assert_eq!(b.label, r.label);
        assert_eq!(b.verdict, r.verdict);
        assert_eq!(b.rating_basis, r.rating_basis);
        assert!(
            (b.computed_value - r.computed_value).abs() <= 1e-9 * r.computed_value.abs().max(1.0)
        );
    }
    assert_eq!(back.assumptions, report.assumptions);
}
