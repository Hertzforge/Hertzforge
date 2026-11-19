//! Integration tests for the channel-table I/O surface: the JSON round-trip and
//! the UFF-58 export stub.

use hertzforge_channel_table::{
    AbortLimits, Channel, ChannelRole, ChannelTable, Interface, Polarity, Units,
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
fn json_is_pretty_and_lossless() {
    let t = tricky_table();
    let json = t.to_json().unwrap();
    // Pretty JSON is multi-line and indented.
    assert!(json.contains('\n'), "expected pretty (multi-line) JSON");
    assert!(json.contains("  "), "expected indentation");
    let back = ChannelTable::from_json(&json).unwrap();
    assert_eq!(t, back);
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
