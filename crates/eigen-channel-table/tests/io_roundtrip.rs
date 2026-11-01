//! Integration tests for the channel-table I/O surface: CSV/TSV/JSON round-trips,
//! RFC-4180 quoting, the UFF-58 export stub, and a realistic messy spreadsheet paste.

use eigen_channel_table::{
    AbortLimits, Channel, ChannelRole, ChannelTable, Interface, Polarity, ReferenceSpec, Units,
};

/// A table that exercises the awkward corners: a comma and a quote in a description,
/// IEPE with bias, a custom unit, negative polarity, an abort trio, and a control group.
fn tricky_table() -> ChannelTable {
    let mut t = ChannelTable::new();

    let mut c1 = Channel::new(1);
    c1.role = ChannelRole::Control;
    c1.description = "Airbags, primary".into(); // embedded comma
    c1.sensitivity_mv_per_eu = 1000.0;
    c1.range_max = 5.0;
    c1.control_group = Some(0);
    c1.abort = AbortLimits {
        accel_g: Some(5.0),
        velocity_ips: Some(10000.0),
        displacement_mil: Some(10000.0),
    };
    t.channels.push(c1);

    let mut c2 = Channel::new(2);
    c2.interface = Interface::Iepe;
    c2.iepe_bias_v = Some(11.0);
    c2.units = Units::Custom("psi \"gauge\"".into()); // embedded quotes
    c2.sensitivity_mv_per_eu = 10.25;
    c2.polarity = Polarity::Negative;
    c2.control_group = Some(0);
    c2.description = "Trunions".into();
    t.channels.push(c2);

    t
}

#[test]
fn csv_round_trips() {
    let t = tricky_table();
    let csv = t.to_csv();
    let back = ChannelTable::from_csv(&csv).unwrap();
    assert_eq!(t.channels, back.channels);
}

#[test]
fn csv_quotes_embedded_comma_and_quote() {
    let t = tricky_table();
    let csv = t.to_csv();
    // The comma-containing description must be wrapped in quotes.
    assert!(
        csv.contains("\"Airbags, primary\""),
        "description with comma not quoted:\n{csv}"
    );
    // The embedded double-quote in the custom unit must be doubled.
    assert!(
        csv.contains("\"\"gauge\"\""),
        "embedded quote not doubled:\n{csv}"
    );
    // Header present.
    assert!(csv.starts_with("idx,en,save"));
}

#[test]
fn csv_accepts_spreadsheet_export_without_header() {
    // Simulate a paste/export with no header, LF endings, quoted comma field.
    let raw = "1,1,1,0,SE,,g,100,100,+,control,,,,,\"Loc, A\"\n2,1,1,0,IEPE,2.5,g,10,50,-,monitor,,,,,Plain";
    let t = ChannelTable::from_csv(raw).unwrap();
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.channels[0].description, "Loc, A");
    assert_eq!(t.channels[0].role, ChannelRole::Control);
    assert_eq!(t.channels[1].interface, Interface::Iepe);
    assert_eq!(t.channels[1].polarity, Polarity::Negative);
}

#[test]
fn tsv_and_csv_agree_on_channels() {
    let t = tricky_table();
    let from_tsv = ChannelTable::from_tsv(&t.to_tsv()).unwrap();
    let from_csv = ChannelTable::from_csv(&t.to_csv()).unwrap();
    assert_eq!(from_tsv.channels, from_csv.channels);
}

#[test]
fn json_is_pretty_and_lossless() {
    let mut t = tricky_table();
    t.reference = Some(ReferenceSpec {
        n_reference: 0,
        notes: "identity only".into(),
    });
    let json = t.to_json().unwrap();
    // Pretty JSON is multi-line and indented.
    assert!(json.contains('\n'), "expected pretty (multi-line) JSON");
    assert!(json.contains("  "), "expected indentation");
    let back = ChannelTable::from_json(&json).unwrap();
    assert_eq!(t, back);
}

#[test]
fn json_survives_full_cross_format_chain() {
    // TSV -> table -> CSV -> table -> JSON -> table, channels stable throughout.
    let t = tricky_table();
    let a = ChannelTable::from_tsv(&t.to_tsv()).unwrap();
    let b = ChannelTable::from_csv(&a.to_csv()).unwrap();
    let c = ChannelTable::from_json(&b.to_json().unwrap()).unwrap();
    assert_eq!(t.channels, c.channels);
}

#[test]
fn realistic_messy_spreadsheet_paste() {
    // What a user actually pastes out of Excel: a friendly header row with unit
    // suffixes and a `#`, columns REORDERED vs. the schema, mixed boolean spellings,
    // CRLF endings, an unmapped "Serial" column, and a trailing blank line.
    let pasted =
        "Ch #\tDescription\tSensitivity (mV/g)\tRange (g)\tCoupling\tEnabled\tRole\tSerial\r\n\
1\tAirbags\t1000.0\t5\tSE\tYes\tControl\tSN-100\r\n\
2\tTrunions\t10.25\t100\tIEPE\tYes\tMonitor\tSN-101\r\n\
3\tVertical\t10.27\t100\tIEPE\tYes\tMonitor\tSN-102\r\n\
\r\n";
    let t = ChannelTable::paste_from_clipboard_tsv(pasted).unwrap();
    assert_eq!(t.channels.len(), 3);

    assert_eq!(t.channels[0].index, 1);
    assert_eq!(t.channels[0].description, "Airbags");
    assert_eq!(t.channels[0].sensitivity_mv_per_eu, 1000.0);
    assert_eq!(t.channels[0].range_max, 5.0);
    assert_eq!(t.channels[0].interface, Interface::SingleEnded);
    assert_eq!(t.channels[0].role, ChannelRole::Control);
    assert!(t.channels[0].enabled);

    assert_eq!(t.channels[1].sensitivity_mv_per_eu, 10.25);
    assert_eq!(t.channels[1].interface, Interface::Iepe);
    assert_eq!(t.channels[2].description, "Vertical");

    // The pasted table is a valid control setup (one control, no dangling references).
    assert!(
        t.validate().is_ok(),
        "messy paste should validate: {:?}",
        t.validate()
    );
}

#[test]
fn renamed_comma_paste_goes_through_clipboard_csv() {
    // Renamed/reordered comma-delimited headers, with a quoted embedded comma, parse
    // through the comma-aware clipboard paste path (not the fixed-16 `from_csv`).
    let raw = "Channel,Enabled,Units,Sensitivity,Range,Role,Description\n\
1,yes,g,100,50,control,\"Loc, A\"";
    let t = ChannelTable::paste_from_clipboard_csv(raw).unwrap();
    assert_eq!(t.channels.len(), 1);
    assert_eq!(t.channels[0].index, 1);
    assert_eq!(t.channels[0].sensitivity_mv_per_eu, 100.0);
    assert_eq!(t.channels[0].role, ChannelRole::Control);
    assert_eq!(t.channels[0].description, "Loc, A");
}

#[test]
fn uff58_stub_structure_and_todo() {
    let t = tricky_table();
    let uff = t.to_uff58_stub();
    // One dataset per enabled non-drive channel, delimited by "    -1" and "    58".
    assert_eq!(
        uff.matches("    58").count(),
        2,
        "expected 2 dataset-58 blocks"
    );
    assert!(uff.contains("    -1"), "missing UFF dataset delimiter");
    // The stub must loudly flag that ordinate data is not implemented.
    assert!(
        uff.contains("TODO"),
        "UFF stub must carry a TODO marker:\n{uff}"
    );
    // Channel descriptions are carried into the identification records.
    assert!(uff.contains("Airbags, primary"));
    assert!(uff.contains("Trunions"));
}

#[test]
fn uff58_stub_skips_drive_and_disabled() {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.description = "Ctrl".into();
    let mut drive = Channel::new(2);
    drive.role = ChannelRole::Drive;
    drive.description = "DriveOut".into();
    let mut off = Channel::new(3);
    off.enabled = false;
    off.description = "Disabled".into();
    t.channels.push(ctrl);
    t.channels.push(drive);
    t.channels.push(off);

    let uff = t.to_uff58_stub();
    assert_eq!(
        uff.matches("    58").count(),
        1,
        "only the control channel exports"
    );
    assert!(uff.contains("Ctrl"));
    assert!(!uff.contains("DriveOut"));
    assert!(!uff.contains("Disabled"));
}
