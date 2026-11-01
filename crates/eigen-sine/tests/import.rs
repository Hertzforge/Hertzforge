//! Import/export hardening tests for `eigen_sine::profile`: every delimiter,
//! header aliasing, the exactly-one-quantity precedence rule, the
//! adversarial fixture, and lossless round-trips.

use eigen_sine::{
    import_with_report, pct_to_db, ProfileError, ProfileRow, ProfileWarning, Quantity, SineProfile,
};

fn mixed_profile() -> SineProfile {
    let mut p = SineProfile::new("mixed");
    p.push(ProfileRow::displacement(5.0, 1.0).with_alarm_db(1.0, 2.0));
    p.push(ProfileRow::accel(15.320388, 12.0).with_abort_db(3.0, 4.0));
    p.push(
        ProfileRow::velocity(100.0, 48.13)
            .with_alarm_db(1.5, 1.5)
            .with_abort_db(3.0, 3.0),
    );
    p.push(ProfileRow::accel(2000.0, 12.0));
    p
}

// --- basic imports -----------------------------------------------------------------

#[test]
fn csv_with_header_basic() {
    let src = "freq,accel\n10,1.0\n2000,12\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[0], ProfileRow::accel(10.0, 1.0));
    assert_eq!(p.rows[1], ProfileRow::accel(2000.0, 12.0));
}

#[test]
fn tsv_with_header_and_mixed_quantities() {
    let src = "freq\tdisp\taccel\tvel\n5\t1.0\t\t\n20\t\t12\t\n100\t\t\t30\n";
    let p = SineProfile::from_tsv(src).unwrap();
    assert_eq!(p.rows[0].quantity, Quantity::DisplacementInPp);
    assert_eq!(p.rows[1].quantity, Quantity::AccelGPk);
    assert_eq!(p.rows[2].quantity, Quantity::VelocityIpsPk);
    assert_eq!(p.rows[2].value, 30.0);
}

#[test]
fn clipboard_auto_detects_semicolon() {
    let src = "freq;g\n10;1\n2000;12\n";
    let p = SineProfile::paste_from_clipboard(src).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[1].value, 12.0);
}

#[test]
fn clipboard_auto_detects_whitespace_headerless() {
    let src = "10 1.5\n2000 12\n";
    let p = SineProfile::paste_from_clipboard(src).unwrap();
    assert_eq!(p.rows[0], ProfileRow::accel(10.0, 1.5));
    assert_eq!(p.rows[1], ProfileRow::accel(2000.0, 12.0));
}

#[test]
fn headerless_two_column_is_freq_accel() {
    let src = "10\t1.0\n2000\t12\n";
    let p = SineProfile::paste_from_clipboard(src).unwrap();
    assert_eq!(p.rows[0].quantity, Quantity::AccelGPk);
    assert_eq!(p.rows[0].freq_hz, 10.0);
}

#[test]
fn headerless_three_columns_is_error() {
    let src = "10\t1.0\t2.0\n";
    match SineProfile::from_tsv(src) {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("two columns"), "{msg}"),
        other => panic!("expected a headerless-shape parse error, got {other:?}"),
    }
}

#[test]
fn header_aliases_resolve_case_insensitively() {
    let src = "HZ,GPK\n10,1\n2000,12\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].quantity, Quantity::AccelGPk);
    let src2 = "Frequency (Hz),Velocity (in/s pk)\n10,5\n100,5\n";
    let p2 = SineProfile::from_csv(src2).unwrap();
    assert_eq!(p2.rows[0].quantity, Quantity::VelocityIpsPk);
    let src3 = "F\tIN_S\n10\t5\n100\t5\n";
    let p3 = SineProfile::from_tsv(src3).unwrap();
    assert_eq!(p3.rows[0].quantity, Quantity::VelocityIpsPk);
    let src4 = "f,inch\n1,0.5\n10,0.5\n";
    let p4 = SineProfile::from_csv(src4).unwrap();
    assert_eq!(p4.rows[0].quantity, Quantity::DisplacementInPp);
}

#[test]
fn comment_lines_and_blanks_are_skipped() {
    let src = "# a comment\n\nfreq,g\n// another\n10,1\n\n2000,12\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows.len(), 2);
}

#[test]
fn exponent_notation_and_whitespace_tolerated() {
    let src = "freq,g\n 1e1 , 1E0 \n2E3,1.2e1\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].freq_hz, 10.0);
    assert_eq!(p.rows[0].value, 1.0);
    assert_eq!(p.rows[1].freq_hz, 2000.0);
    assert_eq!(p.rows[1].value, 12.0);
}

// --- alarm / abort columns -----------------------------------------------------------

#[test]
fn symmetric_and_pair_band_columns() {
    let src = "freq,g,alarm,abort-,abort+\n10,1,1.5,3,4\n2000,12,2,3,3\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].alarm_db, Some((1.5, 1.5)));
    assert_eq!(p.rows[0].abort_db, Some((3.0, 4.0)));
    assert_eq!(p.rows[1].alarm_db, Some((2.0, 2.0)));
}

#[test]
fn alarm_minus_plus_underscore_aliases() {
    let src = "freq,g,alarm_minus,alarm_plus\n10,1,1,2\n2000,12,1,2\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].alarm_db, Some((1.0, 2.0)));
}

#[test]
fn pct_columns_convert_asymmetrically() {
    let src = "freq,g,alarm_pct,abort_pct\n10,1,10,25\n2000,12,10,25\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].alarm_db, Some(pct_to_db(10.0)));
    assert_eq!(p.rows[0].abort_db, Some(pct_to_db(25.0)));
    let (minus, plus) = p.rows[0].alarm_db.unwrap();
    assert!((minus - 0.9151).abs() < 1e-4);
    assert!((plus - 0.8279).abs() < 1e-4);
}

#[test]
fn missing_band_cells_stay_none() {
    let src = "freq,g,alarm\n10,1,\n2000,12,2\n";
    let p = SineProfile::from_csv(src).unwrap();
    assert_eq!(p.rows[0].alarm_db, None);
    assert_eq!(p.rows[1].alarm_db, Some((2.0, 2.0)));
}

// --- the adversarial fixture ----------------------------------------------------

#[test]
fn adversarial_units_crlf_quotes_exponent_mixed_case() {
    // Units column + CRLF line endings + quoted fields (one containing the
    // delimiter) + a 1e3 exponent + mixed-case headers.
    let src = "Freq (Hz),ACCEL (g pk),Units,\"Alarm-\",Alarm+\r\n\
               \"1e1\",\"1.0\",\"g, pk\",1.0,2.0\r\n\
               2E3,12,g pk,1.5,1.5\r\n";
    let (p, report) = import_with_report(src, None).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[0].freq_hz, 10.0);
    assert_eq!(p.rows[0].value, 1.0);
    assert_eq!(p.rows[0].alarm_db, Some((1.0, 2.0)));
    assert_eq!(p.rows[1].freq_hz, 2000.0);
    assert_eq!(p.rows[1].value, 12.0);
    assert!(report.is_ok(), "{report:?}");
    assert!(report
        .warnings
        .contains(&ProfileWarning::UnitsColumnIgnored));
}

// --- error fixtures --------------------------------------------------------------------

#[test]
fn two_quantities_populated_is_row_error_listing_columns() {
    let src = "freq,accel,disp\n10,1,0.5\n";
    match SineProfile::from_csv(src) {
        Err(ProfileError::MultipleQuantities { row, columns }) => {
            assert_eq!(row, 0);
            assert_eq!(
                columns,
                vec!["accel".to_string(), "displacement".to_string()]
            );
        }
        other => panic!("expected MultipleQuantities, got {other:?}"),
    }
}

#[test]
fn non_increasing_freq_lands_in_report_errors() {
    let src = "freq,g\n100,1\n50,2\n";
    let (p, report) = import_with_report(src, None).unwrap();
    assert_eq!(p.rows.len(), 2);
    assert!(!report.is_ok());
    assert!(report
        .errors
        .iter()
        .any(|e| matches!(e, ProfileError::NonMonotonicFreq { row: 1, .. })));
    assert!(p.validate().is_err());
}

#[test]
fn missing_freq_column_is_error() {
    let src = "g,alarm\n1,2\n";
    match SineProfile::from_csv(src) {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("frequency column"), "{msg}"),
        other => panic!("expected a missing-frequency-column error, got {other:?}"),
    }
}

#[test]
fn missing_quantity_column_is_error() {
    let src = "freq,alarm\n10,2\n";
    match SineProfile::from_csv(src) {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("quantity column"), "{msg}"),
        other => panic!("expected a missing-quantity-column error, got {other:?}"),
    }
}

#[test]
fn row_with_no_quantity_value_is_error() {
    let src = "freq,g,vel\n10,1,\n2000,,\n";
    match SineProfile::from_csv(src) {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("no quantity value"), "{msg}"),
        other => panic!("expected a no-quantity-value error, got {other:?}"),
    }
}

#[test]
fn unparseable_cells_are_errors() {
    match SineProfile::from_csv("freq,g\nten,1\n") {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("frequency"), "{msg}"),
        other => panic!("expected a frequency parse error, got {other:?}"),
    }
    match SineProfile::from_csv("freq,g\n10,one\n") {
        Err(ProfileError::Parse(msg)) => assert!(msg.contains("accel"), "{msg}"),
        other => panic!("expected an accel parse error, got {other:?}"),
    }
}

#[test]
fn empty_input_is_error() {
    assert!(matches!(
        SineProfile::from_csv("# only comments\n"),
        Err(ProfileError::Parse(_))
    ));
}

// --- exports / round-trips ---------------------------------------------------------------

#[test]
fn csv_export_reimport_equals_original_rows() {
    let p = mixed_profile();
    let csv = p.to_csv();
    // Canonical header present, one quantity column per row.
    assert!(csv.starts_with(
        "freq_hz,accel_g_pk,vel_ips_pk,disp_in_pp,alarm_minus_db,alarm_plus_db,\
         abort_minus_db,abort_plus_db\n"
    ));
    let back = SineProfile::from_csv(&csv).unwrap();
    assert_eq!(back.rows, p.rows);
}

#[test]
fn tsv_export_reimport_equals_original_rows() {
    let p = mixed_profile();
    let back = SineProfile::from_tsv(&p.to_tsv()).unwrap();
    assert_eq!(back.rows, p.rows);
}

#[test]
fn export_populates_exactly_one_quantity_column_per_row() {
    let p = mixed_profile();
    for line in p.to_csv().lines().skip(1) {
        let cells: Vec<&str> = line.split(',').collect();
        let populated = cells[1..4].iter().filter(|c| !c.is_empty()).count();
        assert_eq!(populated, 1, "line {line:?}");
    }
}

#[test]
fn json_round_trip_is_lossless() {
    let mut p = mixed_profile();
    p.global_alarm_db = (1.0, 2.0);
    p.global_abort_db = (2.5, 3.5);
    let json = p.to_json().unwrap();
    let back = SineProfile::from_json(&json).unwrap();
    assert_eq!(back, p); // label + globals + rows, exactly
}

#[test]
fn json_missing_globals_take_defaults() {
    let json = r#"{ "label": "x", "rows": [] }"#;
    let p = SineProfile::from_json(json).unwrap();
    assert_eq!(p.global_alarm_db, (1.5, 1.5));
    assert_eq!(p.global_abort_db, (3.0, 3.0));
}
