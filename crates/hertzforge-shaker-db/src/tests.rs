//! Tests for `hertzforge-shaker-db`: schema round-trip, store CRUD, and the
//! feasibility calculator across the required cases (realistic pass, mass-driven
//! exceedance, peak-vs-RMS basis flip at the crest boundary, displacement-limited
//! low-frequency profile, overturning NotEvaluated vs StaticEstimateOnly).

use super::*;
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable, LineSpectrum};

/// A ~6 gRMS profile (the decoded demo profile: 20/0.01, 80/0.04, 350/0.04,
/// 2000/0.007) interpolated to a 1 Hz grid.
fn six_grms_profile() -> (BreakpointTable, LineSpectrum) {
    let mut t = BreakpointTable::new("demo");
    t.push(BreakpointRow::psd(20.0, 0.01));
    t.push(BreakpointRow::psd(80.0, 0.04));
    t.push(BreakpointRow::psd(350.0, 0.04));
    t.push(BreakpointRow::psd(2000.0, 0.007));
    let spec = t.interpolate(0.5, 2000.0).unwrap();
    (t, spec)
}

/// A low-frequency, high-displacement profile: 5..30 Hz flat at 0.05 g^2/Hz. Low
/// acceleration RMS but large displacement RMS (the 1/f^4 weighting dominates).
fn low_freq_profile() -> (BreakpointTable, LineSpectrum) {
    let mut t = BreakpointTable::new("lowf");
    t.push(BreakpointRow::psd(5.0, 0.05));
    t.push(BreakpointRow::psd(30.0, 0.05));
    let spec = t.interpolate(0.1, 30.0).unwrap();
    (t, spec)
}

/// A T2000-class record: 20,000 lbf random rating (peak basis by default),
/// generous kinematic limits, 200 lb armature.
fn t2000_class(force_basis: RatingBasis) -> ShakerRecord {
    let mut r = ShakerRecord::new("T2000-class");
    r.manufacturer = "Example Shakers".into();
    r.model = "T2000".into();
    r.force_random = Some(RatingValue {
        value: 20_000.0,
        basis: force_basis,
    });
    r.max_velocity_ips = Some(RatingValue::peak(100.0));
    r.max_displacement_in_pp = Some(RatingValue::peak(2.0));
    r.max_accel_bare_g = Some(RatingValue::peak(100.0));
    r.armature_mass_lb = Some(200.0);
    r.overturning_moment_limit_lbf_in = Some(50_000.0);
    r
}

fn mass_totaling(total_lb: f64) -> MovingMass {
    MovingMass {
        armature_lb: 200.0,
        slip_table_lb: 0.0,
        drive_adapter_lb: 0.0,
        fixturing_lb: 0.0,
        test_article_lb: total_lb - 200.0,
    }
}

// --- schema round-trip ---------------------------------------------------------

#[test]
fn shaker_record_json_round_trip() {
    let mut r = t2000_class(RatingBasis::Peak);
    r.notes = "demo unit".into();
    r.custom.push(("flexure_limit_in".into(), "0.5".into()));
    r.force_sine = Some(RatingValue::sine_equivalent(30_000.0));
    let json = ShakerDb::export_json(&r).unwrap();
    let back: ShakerRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(r, back);
}

#[test]
fn moving_mass_total() {
    let m = MovingMass {
        armature_lb: 200.0,
        slip_table_lb: 50.0,
        drive_adapter_lb: 25.0,
        fixturing_lb: 100.0,
        test_article_lb: 125.0,
    };
    assert_eq!(m.total(), 500.0);
}

// --- store CRUD ----------------------------------------------------------------

/// A unique scratch directory under the session scratchpad for store tests.
fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!(
        "hertzforge-shaker-db-test-{}-{}",
        tag,
        std::process::id()
    ));
    base
}

#[test]
fn store_crud_lifecycle() {
    let dir = scratch_dir("crud");
    let _ = std::fs::remove_dir_all(&dir);
    let db = ShakerDb::open(&dir).unwrap();
    assert!(db.list().unwrap().is_empty());

    let r = t2000_class(RatingBasis::Peak);
    let path = db.save(&r).unwrap();
    assert!(path.exists());
    assert!(db.contains("T2000-class").unwrap());

    let loaded = db.load("T2000-class").unwrap();
    assert_eq!(loaded, r);

    let names = db.list().unwrap();
    assert_eq!(names, vec!["T2000-class".to_string()]);

    db.delete("T2000-class").unwrap();
    assert!(!db.contains("T2000-class").unwrap());
    assert!(matches!(
        db.load("T2000-class"),
        Err(StoreError::NotFound(_))
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_import_export_json() {
    let dir = scratch_dir("impexp");
    let _ = std::fs::remove_dir_all(&dir);
    let db = ShakerDb::open(&dir).unwrap();
    let r = t2000_class(RatingBasis::Rms);
    let json = ShakerDb::export_json(&r).unwrap();
    let imported = db.import_json(&json).unwrap();
    assert_eq!(imported, r);
    assert!(db.contains("T2000-class").unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_sanitizes_filenames() {
    let dir = scratch_dir("sani");
    let _ = std::fs::remove_dir_all(&dir);
    let db = ShakerDb::open(&dir).unwrap();
    let mut r = ShakerRecord::new("T2000 / Slip #3 (west bay)");
    r.force_random = Some(RatingValue::peak(1000.0));
    let path = db.save(&r).unwrap();
    // filename has no slashes, spaces, #, or parens.
    let fname = path.file_name().unwrap().to_str().unwrap();
    assert!(!fname.contains('/') && !fname.contains(' ') && !fname.contains('#'));
    // recall-by-name still works (name field preserved).
    assert_eq!(db.load("T2000 / Slip #3 (west bay)").unwrap().name, r.name);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_empty_name_is_error() {
    let dir = scratch_dir("empty");
    let _ = std::fs::remove_dir_all(&dir);
    let db = ShakerDb::open(&dir).unwrap();
    let r = ShakerRecord::new("!!!"); // sanitizes to empty
    assert!(matches!(db.save(&r), Err(StoreError::EmptyName)));
    let _ = std::fs::remove_dir_all(&dir);
}

// --- feasibility: realistic pass -----------------------------------------------

#[test]
fn feasibility_realistic_case_passes() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    // a_rms ~ 6.06 g; F_rms = 500*6.06 = 3030 lbf; F_peak = 9090 lbf < 20000 -> pass.
    assert_eq!(report.overall, Verdict::Pass, "items: {:#?}", report.items);
    let force = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    assert_eq!(force.verdict, Verdict::Pass);
    assert!(force.margin_ratio.unwrap() > 2.0);
    // assumptions are populated for the report/UI.
    assert!(!report.assumptions.is_empty());
    assert!(report
        .assumptions
        .iter()
        .any(|a| a.contains("Crest factor")));
    // overturning was NOT evaluated (no geometry).
    let ot = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::OverturningMoment)
        .unwrap();
    assert_eq!(ot.verdict, Verdict::NotEvaluated);
}

#[test]
fn feasibility_mass_driven_exceedance() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    // Raise total mass so F_peak = 3 * m * 6.06 > 20000 -> m > ~1100 lb. Use 1500.
    let mass = mass_totaling(1500.0);
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    assert_eq!(
        report.overall,
        Verdict::Exceeded,
        "items: {:#?}",
        report.items
    );
    let force = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    assert_eq!(force.verdict, Verdict::Exceeded);
    assert!(force.margin_ratio.unwrap() < 1.0);
}

#[test]
fn feasibility_peak_vs_rms_basis_flips_verdict_at_crest_boundary() {
    let (_t, spec) = six_grms_profile();
    let a_rms = hertzforge_breakpoints::grms(&spec); // ~6.06
                                                     // Choose a mass so that F_rms < rating < F_peak: rating between m*a and 3*m*a.
                                                     // Pick m = 900 lb -> F_rms ~ 5454, F_peak ~ 16362. A rating of 8000 lbf sits
                                                     // between them: RMS-basis passes (8000 > 5454), peak-basis fails (8000 < 16362).
    let mass = mass_totaling(900.0);
    let rating = 8000.0;
    // sanity on the arithmetic.
    let f_rms = 900.0 * a_rms;
    let f_peak = 3.0 * f_rms;
    assert!(
        f_rms < rating && rating < f_peak,
        "boundary not set up: F_rms {f_rms} rating {rating} F_peak {f_peak}"
    );

    let mut rms_rec = t2000_class(RatingBasis::Rms);
    rms_rec.force_random = Some(RatingValue::rms(rating));
    let rms_report =
        FeasibilityCalculator::assess(&rms_rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let rms_force = rms_report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    assert_eq!(rms_force.verdict, Verdict::Pass, "RMS-basis should pass");

    let mut peak_rec = t2000_class(RatingBasis::Peak);
    peak_rec.force_random = Some(RatingValue::peak(rating));
    let peak_report =
        FeasibilityCalculator::assess(&peak_rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let peak_force = peak_report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    assert_eq!(
        peak_force.verdict,
        Verdict::Exceeded,
        "peak-basis should fail"
    );
}

#[test]
fn feasibility_sine_equivalent_is_advisory_never_hard_fail() {
    let (_t, spec) = six_grms_profile();
    let mut rec = t2000_class(RatingBasis::SineEquivalent);
    // Set the rating absurdly low so a peak/RMS basis would hard-fail.
    rec.force_random = Some(RatingValue::sine_equivalent(100.0));
    let mass = mass_totaling(500.0);
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let force = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    // Sine-equivalent never HARD fails: it is Advisory even when under-rated.
    assert_eq!(force.verdict, Verdict::Advisory);
    // Advisory does not fail the overall: overall is not Exceeded.
    assert_ne!(report.overall, Verdict::Exceeded);
    assert!(report.has_advisory());
}

#[test]
fn feasibility_displacement_limited_low_freq_profile_trips() {
    let (_t, spec) = low_freq_profile();
    let mut rec = t2000_class(RatingBasis::Peak);
    // Ample force/velocity/accel headroom, but a tight displacement stroke limit.
    rec.force_random = Some(RatingValue::peak(50_000.0));
    rec.max_velocity_ips = Some(RatingValue::peak(200.0));
    rec.max_accel_bare_g = Some(RatingValue::peak(200.0));
    rec.max_displacement_in_pp = Some(RatingValue::peak(0.5)); // tight stroke
    let mass = mass_totaling(300.0);
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let disp = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Displacement)
        .unwrap();
    assert_eq!(disp.verdict, Verdict::Exceeded, "disp item: {disp:#?}");
    assert_eq!(report.overall, Verdict::Exceeded);
    // Force should NOT be the limiting item here.
    let force = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::Force)
        .unwrap();
    assert_eq!(force.verdict, Verdict::Pass);
}

// --- overturning moment: geometry gating ---------------------------------------

#[test]
fn feasibility_overturning_not_evaluated_without_geometry() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let ot = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::OverturningMoment)
        .unwrap();
    assert_eq!(ot.verdict, Verdict::NotEvaluated);
    assert!(ot.margin_ratio.is_none());
}

#[test]
fn feasibility_overturning_static_estimate_with_geometry() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    // F_peak ~ 9090 lbf; a 4 in CG offset -> ~36360 lbf*in < 50000 limit -> pass.
    let geom = ArticleGeometry {
        cg_offset_in: Some(4.0),
        cg_height_in: None,
    };
    let report = FeasibilityCalculator::assess(&rec, mass, Some(geom), &spec, DEFAULT_CREST_FACTOR);
    let ot = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::OverturningMoment)
        .unwrap();
    assert!(matches!(ot.verdict, Verdict::Pass | Verdict::Exceeded));
    assert!(ot.label.contains("static estimate only"));
    // the static-estimate assumption text is present.
    assert!(report
        .assumptions
        .iter()
        .any(|a| a.contains("STATIC ESTIMATE ONLY")));
    assert!(ot.margin_ratio.is_some());
}

#[test]
fn feasibility_overturning_exceeded_with_large_offset() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    // A 10 in offset -> F_peak(~9090) * 10 = ~90900 lbf*in > 50000 -> exceeded.
    let geom = ArticleGeometry {
        cg_offset_in: Some(10.0),
        cg_height_in: None,
    };
    let report = FeasibilityCalculator::assess(&rec, mass, Some(geom), &spec, DEFAULT_CREST_FACTOR);
    let ot = report
        .items
        .iter()
        .find(|i| i.quantity == Quantity::OverturningMoment)
        .unwrap();
    assert_eq!(ot.verdict, Verdict::Exceeded);
    assert_eq!(report.overall, Verdict::Exceeded);
}

// --- assess_with_table analytic kinematics agree with the discrete path --------

#[test]
fn assess_with_table_agrees_with_line_spectrum_path() {
    // Displacement (~1/f^4 weighted) is the most grid-sensitive quantity, dominated
    // by the low-frequency band edge; use a fine 0.1 Hz grid so the discrete
    // integration converges onto the analytic per-segment integral to well under 1%.
    let t = {
        let mut t = BreakpointTable::new("demo");
        t.push(BreakpointRow::psd(20.0, 0.01));
        t.push(BreakpointRow::psd(80.0, 0.04));
        t.push(BreakpointRow::psd(350.0, 0.04));
        t.push(BreakpointRow::psd(2000.0, 0.007));
        t
    };
    let spec = t.interpolate(0.1, 2000.0).unwrap();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    let discrete = FeasibilityCalculator::assess(&rec, mass, None, &spec, DEFAULT_CREST_FACTOR);
    let analytic =
        FeasibilityCalculator::assess_with_table(&rec, mass, None, &t, &spec, DEFAULT_CREST_FACTOR)
            .unwrap();
    // velocity + displacement demands should agree within ~1% (grid vs analytic).
    for q in [Quantity::Velocity, Quantity::Displacement] {
        let d = discrete.items.iter().find(|i| i.quantity == q).unwrap();
        let a = analytic.items.iter().find(|i| i.quantity == q).unwrap();
        let rel = (d.computed_value - a.computed_value).abs() / a.computed_value.max(1e-12);
        assert!(
            rel < 0.01,
            "{q} differs by {rel}: discrete {} analytic {}",
            d.computed_value,
            a.computed_value
        );
    }
}

#[test]
fn assess_defaults_crest_factor_when_nonpositive() {
    let (_t, spec) = six_grms_profile();
    let rec = t2000_class(RatingBasis::Peak);
    let mass = mass_totaling(500.0);
    // crest_factor <= 0 falls back to DEFAULT_CREST_FACTOR (3.0).
    let report = FeasibilityCalculator::assess(&rec, mass, None, &spec, 0.0);
    assert!(report
        .assumptions
        .iter()
        .any(|a| a.contains("Crest factor = 3.00")));
}
