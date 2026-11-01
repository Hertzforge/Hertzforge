//! Exhaustive tests for `eigen-breakpoints`. Mirrors the coverage bar of
//! `eigen-channel-table`: grammar errors, mixed tables, slope resolution,
//! the demo/NAVMAT-ish profile gRMS (6.058), interpolation-vs-analytic agreement,
//! kinematics, the frequency-domain envelope, every import format + clipboard, and
//! lossless JSON round-trip.

use super::*;

/// The decoded OEM demo profile: 20/0.01, 80/0.04 (+3.0103 dB/oct), 350/0.04 (flat),
/// 2000/0.007 (-3.0103 dB/oct). Analytic gRMS is 6.058 (verified in the spec).
fn demo_profile() -> BreakpointTable {
    let mut t = BreakpointTable::new("demo");
    t.push(BreakpointRow::psd(20.0, 0.01));
    t.push(BreakpointRow::psd(80.0, 0.04));
    t.push(BreakpointRow::psd(350.0, 0.04));
    t.push(BreakpointRow::psd(2000.0, 0.007));
    t
}

// --- grammar / validation ------------------------------------------------------

#[test]
fn empty_table_is_error() {
    let t = BreakpointTable::new("x");
    assert_eq!(t.validate(), Err(BreakpointError::Empty));
}

#[test]
fn first_row_must_be_anchor() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::slope(20.0, 3.0));
    assert_eq!(t.validate(), Err(BreakpointError::FirstRowNotAnchor));
}

#[test]
fn non_monotonic_freq_is_error() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(80.0, 0.04));
    t.push(BreakpointRow::psd(20.0, 0.01)); // decreasing
    match t.validate() {
        Err(BreakpointError::NonMonotonicFreq { row, .. }) => assert_eq!(row, 1),
        other => panic!("expected NonMonotonicFreq, got {other:?}"),
    }
}

#[test]
fn equal_freq_is_non_monotonic() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(80.0, 0.04));
    t.push(BreakpointRow::psd(80.0, 0.05));
    assert!(matches!(
        t.validate(),
        Err(BreakpointError::NonMonotonicFreq { .. })
    ));
}

#[test]
fn non_positive_freq_is_error() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(0.0, 0.01));
    assert!(matches!(
        t.validate(),
        Err(BreakpointError::NonPositiveFreq { .. })
    ));
}

#[test]
fn negative_psd_is_error() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(20.0, -0.01));
    assert!(matches!(
        t.validate(),
        Err(BreakpointError::NegativePsd { .. })
    ));
}

#[test]
fn slope_only_after_anchor_resolves() {
    // A valid mixed table: anchor then a +3.0103 dB/oct slope row.
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(20.0, 0.01));
    t.push(BreakpointRow::slope(80.0, DB_PER_OCT_PER_DECADE)); // exactly +3.0103 dB/oct
    let anchors = t.resolve().unwrap();
    assert_eq!(anchors.len(), 2);
    // +3.0103 dB/oct over 2 octaves (20->80) doubles per octave -> x4 in level.
    assert!(
        (anchors[1].level_g2_hz - 0.04).abs() < 1e-9,
        "{:?}",
        anchors[1]
    );
}

#[test]
fn slope_derivation_matches_nominal_3db() {
    // Nominal +3 dB/oct (not exactly 3.0103): 2 octaves -> +6 dB -> x 10^0.6.
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow::psd(20.0, 0.01));
    t.push(BreakpointRow::slope(80.0, 3.0));
    let anchors = t.resolve().unwrap();
    let expected = 0.01 * 10f64.powf(6.0 / 10.0);
    assert!((anchors[1].level_g2_hz - expected).abs() < 1e-12);
}

// --- gRMS: analytic + discrete agreement ---------------------------------------

#[test]
fn analytic_grms_matches_6058() {
    let g = demo_profile().grms_of_table().unwrap();
    assert!(
        (g - 6.058).abs() / 6.058 < 0.01,
        "analytic gRMS {g} not within 1% of 6.058"
    );
    // Tighter: our closed form should nail it to many digits.
    assert!((g - 6.0581820929).abs() < 1e-6, "gRMS = {g}");
}

#[test]
fn discrete_grms_agrees_with_analytic_on_fine_grid() {
    let t = demo_profile();
    let analytic = t.grms_of_table().unwrap();
    let spec = t.interpolate(0.5, 2000.0).unwrap();
    let discrete = grms(&spec);
    assert!(
        (discrete - analytic).abs() / analytic < 0.01,
        "discrete {discrete} vs analytic {analytic} exceed 1%"
    );
}

#[test]
fn discrete_grms_converges_as_grid_refines() {
    let t = demo_profile();
    let analytic = t.grms_of_table().unwrap();
    let coarse = grms(&t.interpolate(4.0, 2000.0).unwrap());
    let fine = grms(&t.interpolate(0.25, 2000.0).unwrap());
    assert!(
        (fine - analytic).abs() < (coarse - analytic).abs() + 1e-9,
        "finer grid should be at least as close: coarse {coarse} fine {fine} analytic {analytic}"
    );
}

#[test]
fn flat_profile_grms_is_exact() {
    // A single flat segment: 10..1000 Hz at 0.1 g^2/Hz -> gRMS = sqrt(0.1*990).
    let mut t = BreakpointTable::new("flat");
    t.push(BreakpointRow::psd(10.0, 0.1));
    t.push(BreakpointRow::psd(1000.0, 0.1));
    let g = t.grms_of_table().unwrap();
    assert!((g - (0.1_f64 * 990.0).sqrt()).abs() < 1e-9, "{g}");
}

#[test]
fn singular_slope_segment_uses_log_branch() {
    // A pure -3.0103 dB/oct segment (n = -1): integral = w1*f1*ln(f2/f1).
    let mut t = BreakpointTable::new("sing");
    t.push(BreakpointRow::psd(100.0, 0.04));
    t.push(BreakpointRow::slope(1000.0, -DB_PER_OCT_PER_DECADE));
    let g2 = t.grms_of_table().unwrap().powi(2);
    let expected = 0.04 * 100.0 * (1000.0f64 / 100.0).ln();
    assert!(
        (g2 - expected).abs() / expected < 1e-9,
        "{g2} vs {expected}"
    );
}

// --- interpolation -------------------------------------------------------------

#[test]
fn interpolation_zeros_out_of_band() {
    let t = demo_profile();
    let spec = t.interpolate(1.0, 2500.0).unwrap();
    // below 20 Hz: zero.
    assert_eq!(spec.psd[0], 0.0);
    assert_eq!(spec.psd[10], 0.0); // 10 Hz
                                   // above 2000 Hz: zero.
    assert_eq!(spec.psd[2400], 0.0);
    // in-band flat region ~200 Hz is ~0.04.
    let k = 200;
    assert!((spec.psd[k] - 0.04).abs() < 1e-6, "{}", spec.psd[k]);
}

#[test]
fn interpolation_hits_anchor_values() {
    let t = demo_profile();
    let spec = t.interpolate(1.0, 2000.0).unwrap();
    assert!((spec.psd[20] - 0.01).abs() < 1e-9);
    assert!((spec.psd[80] - 0.04).abs() < 1e-9);
    assert!((spec.psd[350] - 0.04).abs() < 1e-9);
    assert!((spec.psd[2000] - 0.007).abs() < 1e-9);
}

#[test]
fn interpolation_midpoint_on_log_slope() {
    // On the 20->80 Hz +3.0103 dB/oct rise, 40 Hz is one octave above 20 -> x2.
    let t = demo_profile();
    let spec = t.interpolate(1.0, 2000.0).unwrap();
    assert!((spec.psd[40] - 0.02).abs() < 1e-6, "{}", spec.psd[40]);
}

#[test]
fn interpolate_rejects_nonpositive_df() {
    let t = demo_profile();
    assert!(t.interpolate(0.0, 100.0).is_err());
    assert!(t.interpolate(-1.0, 100.0).is_err());
}

// --- kinematics ----------------------------------------------------------------

#[test]
fn kinematics_positive_and_consistent_units() {
    let t = demo_profile();
    let v_ips = t.velocity_rms_ips().unwrap();
    let d_mil = t.displacement_rms_mil().unwrap();
    assert!(v_ips > 0.0 && v_ips.is_finite(), "v_rms = {v_ips}");
    assert!(d_mil > 0.0 && d_mil.is_finite(), "d_rms = {d_mil}");
    // SI accessors are consistent with the imperial ones.
    let v_si = t.velocity_rms_si().unwrap();
    let d_si = t.displacement_rms_si().unwrap();
    assert!((v_si * IN_PER_M - v_ips).abs() < 1e-9);
    assert!((d_si * IN_PER_M * 1000.0 - d_mil).abs() < 1e-9);
}

#[test]
fn kinematics_flat_band_matches_hand_integral() {
    // Flat 0.01 g^2/Hz from 100..200 Hz. v_rms^2 = (g0/2pi)^2 * integral df/f^2
    // = (g0/2pi)^2 * 0.01 * (1/100 - 1/200).
    let mut t = BreakpointTable::new("flat");
    t.push(BreakpointRow::psd(100.0, 0.01));
    t.push(BreakpointRow::psd(200.0, 0.01));
    let v_si = t.velocity_rms_si().unwrap();
    let k = (G0_M_PER_S2 / std::f64::consts::TAU).powi(2);
    let expected = (k * 0.01 * (1.0 / 100.0 - 1.0 / 200.0)).sqrt();
    assert!(
        (v_si - expected).abs() / expected < 1e-9,
        "{v_si} vs {expected}"
    );
}

#[test]
fn low_frequency_profile_has_larger_displacement() {
    // Same PSD level, but a lower band -> displacement RMS should be much larger
    // (displacement integrand ~ 1/f^4).
    let mut lo = BreakpointTable::new("lo");
    lo.push(BreakpointRow::psd(5.0, 0.01));
    lo.push(BreakpointRow::psd(20.0, 0.01));
    let mut hi = BreakpointTable::new("hi");
    hi.push(BreakpointRow::psd(500.0, 0.01));
    hi.push(BreakpointRow::psd(2000.0, 0.01));
    assert!(lo.displacement_rms_mil().unwrap() > 100.0 * hi.displacement_rms_mil().unwrap());
}

// --- alarm / abort frequency-domain envelope -----------------------------------

#[test]
fn envelope_global_bands_hit_and_miss() {
    let t = demo_profile();
    // +/-3 dB alarm, +/-6 dB abort, 1 Hz grid to 2000 Hz.
    let env = Envelope::from_table_on_grid(&t, 1.0, 2000.0, (3.0, 3.0), (6.0, 6.0)).unwrap();
    // Measured = reference exactly -> no breach.
    let report = env.check(&env.reference.clone());
    assert!(!report.alarmed() && !report.aborted());

    // Push one in-band line +4 dB -> alarm (not abort).
    let mut m = env.reference.clone();
    m.psd[200] *= 10f64.powf(4.0 / 10.0);
    let report = env.check(&m);
    assert!(report.alarmed(), "expected alarm");
    assert!(!report.aborted(), "should not abort at +4 dB");
    assert_eq!(report.alarm_bins[0].line, 200);
    assert!((report.worst_alarm_db.unwrap() - 4.0).abs() < 1e-6);

    // Push it +8 dB -> abort.
    let mut m2 = env.reference.clone();
    m2.psd[200] *= 10f64.powf(8.0 / 10.0);
    let report = env.check(&m2);
    assert!(report.aborted(), "expected abort at +8 dB");
    assert!((report.worst_abort_db.unwrap() - 8.0).abs() < 1e-6);
}

#[test]
fn envelope_low_breach_detected() {
    let t = demo_profile();
    let env = Envelope::from_table_on_grid(&t, 1.0, 2000.0, (3.0, 3.0), (6.0, 6.0)).unwrap();
    // Drop a line 8 dB below reference -> abort (below abort_lo).
    let mut m = env.reference.clone();
    m.psd[200] *= 10f64.powf(-8.0 / 10.0);
    let report = env.check(&m);
    assert!(report.aborted());
    assert!(report.worst_abort_db.unwrap() < 0.0);
}

#[test]
fn envelope_per_row_override_beats_global() {
    // Row starting the flat segment (index 1: 80 Hz) gets a tight +/-1 dB abort.
    let mut t = demo_profile();
    t.rows[1].abort_db = Some((1.0, 1.0));
    let env = Envelope::from_table_on_grid(&t, 1.0, 2000.0, (3.0, 3.0), (6.0, 6.0)).unwrap();
    // A +2 dB bump at 200 Hz (inside the 80..350 segment) now aborts (override 1 dB)
    // even though the global abort is 6 dB.
    let mut m = env.reference.clone();
    m.psd[200] *= 10f64.powf(2.0 / 10.0);
    let report = env.check(&m);
    assert!(
        report.aborted(),
        "per-row 1 dB abort override should trip at +2 dB"
    );
    // A +2 dB bump at 40 Hz (the 20..80 segment, global 6 dB) does NOT abort.
    let mut m2 = env.reference.clone();
    m2.psd[40] *= 10f64.powf(2.0 / 10.0);
    let report2 = env.check(&m2);
    assert!(
        !report2.aborted(),
        "global 6 dB segment should not abort at +2 dB"
    );
}

#[test]
fn envelope_grid_mismatch_returns_empty() {
    let t = demo_profile();
    let env = Envelope::from_table_on_grid(&t, 1.0, 2000.0, (3.0, 3.0), (6.0, 6.0)).unwrap();
    let mut wrong = env.reference.clone();
    wrong.df_hz = 2.0;
    let report = env.check(&wrong);
    assert!(report.alarm_bins.is_empty() && report.abort_bins.is_empty());
}

#[test]
fn envelope_out_of_band_never_breaches() {
    let t = demo_profile();
    let env = Envelope::from_table_on_grid(&t, 1.0, 2000.0, (3.0, 3.0), (6.0, 6.0)).unwrap();
    // A huge value at 5 Hz (out of band) must not breach.
    let mut m = env.reference.clone();
    m.psd[5] = 100.0;
    let report = env.check(&m);
    assert!(!report.alarmed() && !report.aborted());
}

// --- imports: TSV / CSV / clipboard / two-column / comments --------------------

#[test]
fn import_tsv_two_column_headered() {
    let src = "freq\tpsd\n20\t0.01\n80\t0.04\n350\t0.04\n2000\t0.007\n";
    let t = BreakpointTable::from_tsv(src).unwrap();
    assert_eq!(t.rows.len(), 4);
    assert!((t.grms_of_table().unwrap() - 6.0581820929).abs() < 1e-6);
}

#[test]
fn import_csv_with_alarm_abort_pairs() {
    let src = "frequency,g2/Hz,alarm-,alarm+,abort-,abort+\n\
               20,0.01,3,3,6,6\n\
               80,0.04,3,3,6,6\n\
               2000,0.007,3,3,6,6\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.rows[0].alarm_db, Some((3.0, 3.0)));
    assert_eq!(t.rows[0].abort_db, Some((6.0, 6.0)));
}

#[test]
fn import_csv_symmetric_single_band() {
    let src = "freq,psd,alarm,abort\n20,0.01,3,6\n2000,0.007,3,6\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows[0].alarm_db, Some((3.0, 3.0)));
    assert_eq!(t.rows[0].abort_db, Some((6.0, 6.0)));
}

#[test]
fn import_clipboard_autodetect_semicolon() {
    let src = "freq;psd\n20;0.01\n2000;0.007";
    let t = BreakpointTable::paste_from_clipboard(src).unwrap();
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].freq_hz, 20.0);
}

#[test]
fn import_clipboard_autodetect_whitespace_two_column() {
    // Bare two-column whitespace, no header -> (freq, psd).
    let src = "20 0.01\n80 0.04\n350 0.04\n2000 0.007";
    let t = BreakpointTable::paste_from_clipboard(src).unwrap();
    assert_eq!(t.rows.len(), 4);
    assert!((t.grms_of_table().unwrap() - 6.0581820929).abs() < 1e-6);
}

#[test]
fn import_clipboard_autodetect_tab() {
    let src = "20\t0.01\n2000\t0.007";
    let t = BreakpointTable::paste_from_clipboard(src).unwrap();
    assert_eq!(t.rows.len(), 2);
}

#[test]
fn import_skips_comment_and_blank_lines() {
    let src = "# NAVMAT-ish demo profile\nfreq,psd\n// leading comment\n20,0.01\n\n2000,0.007\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows.len(), 2);
}

#[test]
fn import_slope_column() {
    // A slope row via the slope column (no PSD on that row).
    let src = "freq,psd,slope\n20,0.01,\n80,,3.0103\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert!(matches!(t.rows[0].level, LevelSpec::Psd(_)));
    assert!(matches!(t.rows[1].level, LevelSpec::SlopeDbPerOct(_)));
    let anchors = t.resolve().unwrap();
    assert!((anchors[1].level_g2_hz - 0.04).abs() < 1e-4);
}

#[test]
fn import_csv_rfc4180_quoted_field() {
    // A quoted field containing a comma must not split.
    let src = "freq,psd,note\n\"20\",\"0.01\",\"low, band\"\n2000,0.007,x\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].freq_hz, 20.0);
}

#[test]
fn import_tolerates_unit_suffix_in_cell() {
    let src = "freq,psd\n20 Hz,0.01 g2/Hz\n2000 Hz,0.007 g2/Hz\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows[0].freq_hz, 20.0);
    assert_eq!(t.rows[1].freq_hz, 2000.0);
}

#[test]
fn import_missing_level_is_error() {
    let src = "freq,note\n20,x\n";
    assert!(matches!(
        BreakpointTable::from_csv(src),
        Err(BreakpointError::Parse(_))
    ));
}

#[test]
fn import_reordered_renamed_headers() {
    // psd before freq, friendly names.
    let src = "Level (g^2/Hz),Frequency\n0.01,20\n0.007,2000\n";
    let t = BreakpointTable::from_csv(src).unwrap();
    assert_eq!(t.rows[0].freq_hz, 20.0);
    assert!(matches!(t.rows[0].level, LevelSpec::Psd(v) if (v - 0.01).abs() < 1e-12));
}

// --- both-column PSD+slope disagreement warning ---------------------------

#[test]
fn both_column_psd_wins_and_flags_disagreement() {
    // Row 2 declares PSD 0.05 but slope 3 dB/oct from 0.01@20 over 2 oct -> 0.0398.
    // PSD wins; disagreement is >0.1 dB -> warning.
    let src = "freq,psd,slope\n20,0.01,\n80,0.05,3.0103\n";
    let (t, report) = import_with_report(src, Some(',')).unwrap();
    // PSD won.
    assert!(matches!(t.rows[1].level, LevelSpec::Psd(v) if (v - 0.05).abs() < 1e-12));
    // disagreement warning present.
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, BreakpointWarning::PsdSlopeDisagreement { row: 1, .. })),
        "warnings: {:?}",
        report.warnings
    );
}

#[test]
fn both_column_agreement_no_warning() {
    // PSD matches the +3.0103 dB/oct slope within 0.1 dB -> no warning.
    let src = "freq,psd,slope\n20,0.01,\n80,0.04,3.0103\n";
    let (_t, report) = import_with_report(src, Some(',')).unwrap();
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| matches!(w, BreakpointWarning::PsdSlopeDisagreement { .. })),
        "unexpected disagreement: {:?}",
        report.warnings
    );
}

#[test]
fn alarm_wider_than_abort_warns() {
    let mut t = BreakpointTable::new("x");
    t.push(BreakpointRow {
        freq_hz: 20.0,
        level: LevelSpec::Psd(0.01),
        alarm_db: Some((6.0, 6.0)), // wider than abort
        abort_db: Some((3.0, 3.0)),
    });
    t.push(BreakpointRow::psd(2000.0, 0.007));
    let report = t.validate_report();
    assert!(report
        .warnings
        .iter()
        .any(|w| matches!(w, BreakpointWarning::AlarmWiderThanAbort(0))));
}

// --- JSON round-trip -----------------------------------------------------------

#[test]
fn json_round_trip_lossless() {
    let mut t = demo_profile();
    t.rows[1].alarm_db = Some((3.0, 4.0));
    t.rows[2].abort_db = Some((6.0, 6.0));
    // include a slope row for full coverage.
    t.push(BreakpointRow::slope(3000.0, -6.0));
    let json = t.to_json().unwrap();
    let back = BreakpointTable::from_json(&json).unwrap();
    assert_eq!(t, back);
}

#[test]
fn json_round_trip_line_spectrum() {
    // A COMPUTED spectrum's bins carry more precision than their shortest decimal
    // string. serde_json's serializer writes the full-precision decimal but its
    // deserializer rounds to the nearest representable f64, so exact `==` on a
    // computed float vector is NOT achievable through serde_json (a dependency
    // behavior, not this crate's). The physically-meaningful invariant is that the
    // grid and every PSD value round-trip to within f64-decimal tolerance; the
    // struct-level LOSSLESS guarantee applies to AUTHORED tables (see
    // `json_round_trip_lossless`, where anchors like 0.01/0.04 are exact).
    let spec = demo_profile().interpolate(2.0, 2000.0).unwrap();
    let json = serde_json::to_string(&spec).unwrap();
    let back: LineSpectrum = serde_json::from_str(&json).unwrap();
    assert_eq!(spec.f0_hz, back.f0_hz);
    assert_eq!(spec.df_hz, back.df_hz);
    assert_eq!(spec.psd.len(), back.psd.len());
    for (a, b) in spec.psd.iter().zip(back.psd.iter()) {
        assert!((a - b).abs() <= 1e-12 * a.abs().max(1e-30), "{a} vs {b}");
    }
}

// --- discrete grms free function edge cases ------------------------------------

#[test]
fn grms_of_empty_spectrum_is_zero() {
    let s = LineSpectrum {
        f0_hz: 0.0,
        df_hz: 1.0,
        psd: vec![],
    };
    assert_eq!(grms(&s), 0.0);
}
