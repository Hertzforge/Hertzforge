//! The profile import takes the wire's own quantity names as column headers:
//! `accel_g`, `velocity_ips` and `disp_pp_in`, beside `freq_hz`.
//!
//! Red first (session log 2026-11-08, Sine core): a file with those headers
//! was refused, though the wire names the quantities so.

use hertzforge_sine::{ProfileRow, Quantity, SineProfile};

#[test]
fn the_wire_quantity_names_are_column_headers() {
    let src = "freq_hz,accel_g,velocity_ips,disp_pp_in\n\
               5,,,0.1\n\
               20,1.0,,\n\
               100,,2.5,\n\
               2000,12,,\n";
    let p = SineProfile::from_csv(src).expect("the wire's names are headers");
    assert_eq!(p.rows.len(), 4);
    assert_eq!(p.rows[0].quantity, Quantity::DisplacementInPp);
    assert_eq!(p.rows[1], ProfileRow::accel(20.0, 1.0));
    assert_eq!(p.rows[2].quantity, Quantity::VelocityIpsPk);
    assert_eq!(p.rows[2].value, 2.5);
    assert_eq!(p.rows[3], ProfileRow::accel(2000.0, 12.0));
}
