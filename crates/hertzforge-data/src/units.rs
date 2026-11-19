//! Pragmatic unit model (design stolen with pride from the ASAM ODS
//! AoUnit factor+offset scheme, research report 11).
//!
//! A unit converts to SI as `si = value * si_factor + si_offset`. Two units are
//! convertible iff they share a `Quantity`. This is deliberately NOT a full
//! dimensional-analysis system - it is the vibration lab's working set, exact
//! where exactness matters (g, in/s, mil), extensible via `Unit::custom`.

use serde::{Deserialize, Serialize};

/// Standard gravity, m/s^2 (exact by definition).
pub const G0: f64 = 9.80665;
/// Inch in meters (exact by definition).
pub const INCH_M: f64 = 0.0254;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quantity {
    Acceleration,
    Velocity,
    Displacement,
    Force,
    Voltage,
    Pressure,
    Frequency,
    Time,
    Dimensionless,
    Unknown,
}

/// A unit with its conversion to the SI base of its quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    pub label: String,
    pub quantity: Quantity,
    pub si_factor: f64,
    pub si_offset: f64,
}

impl Unit {
    pub fn custom(label: &str, quantity: Quantity, si_factor: f64) -> Unit {
        Unit {
            label: label.into(),
            quantity,
            si_factor,
            si_offset: 0.0,
        }
    }

    pub fn dimensionless() -> Unit {
        Unit {
            label: String::new(),
            quantity: Quantity::Dimensionless,
            si_factor: 1.0,
            si_offset: 0.0,
        }
    }

    pub fn unknown(label: &str) -> Unit {
        Unit {
            label: label.into(),
            quantity: Quantity::Unknown,
            si_factor: 1.0,
            si_offset: 0.0,
        }
    }

    pub fn to_si(&self, v: f64) -> f64 {
        v * self.si_factor + self.si_offset
    }

    pub fn from_si(&self, si: f64) -> f64 {
        (si - self.si_offset) / self.si_factor
    }
}

/// The catalog. Lookup is case-sensitive on the canonical label plus a few
/// widespread aliases; importers should normalize before falling back to
/// `Unit::unknown` (which preserves the source's label verbatim).
pub fn unit(label: &str) -> Option<Unit> {
    use Quantity::*;
    let u = |label: &str, q, f| Unit {
        label: label.into(),
        quantity: q,
        si_factor: f,
        si_offset: 0.0,
    };
    Some(match label {
        "g" | "G" => u("g", Acceleration, G0),
        "m/s^2" | "m/s2" | "m/s²" => u("m/s^2", Acceleration, 1.0),
        "in/s" | "ips" => u("in/s", Velocity, INCH_M),
        "mm/s" => u("mm/s", Velocity, 1e-3),
        "m/s" => u("m/s", Velocity, 1.0),
        "mil" => u("mil", Displacement, INCH_M / 1000.0),
        "in" => u("in", Displacement, INCH_M),
        "mm" => u("mm", Displacement, 1e-3),
        "um" | "µm" => u("um", Displacement, 1e-6),
        "m" => u("m", Displacement, 1.0),
        "N" => u("N", Force, 1.0),
        "lbf" => u("lbf", Force, 4.448_221_615_260_5),
        "V" => u("V", Voltage, 1.0),
        "mV" => u("mV", Voltage, 1e-3),
        "Pa" => u("Pa", Pressure, 1.0),
        "kPa" => u("kPa", Pressure, 1e3),
        "psi" => u("psi", Pressure, 6_894.757_293_168),
        "Hz" => u("Hz", Frequency, 1.0),
        "s" | "sec" => u("s", Time, 1.0),
        "ms" => u("ms", Time, 1e-3),
        "" => Unit::dimensionless(),
        _ => return None,
    })
}

/// Density label for a PSD-class ordinate ("g^2/Hz" for base unit "g").
/// Purely presentational; the stored unit stays the base unit and the KIND
/// carries the /Hz semantics (single source of truth).
pub fn density_label(base: &Unit) -> String {
    if base.label.is_empty() {
        "1/Hz".into()
    } else {
        format!("{}^2/Hz", base.label)
    }
}

/// Convert a value between two units of the SAME quantity.
pub fn convert(v: f64, from: &Unit, to: &Unit) -> Result<f64, String> {
    if from.quantity != to.quantity || from.quantity == Quantity::Unknown {
        return Err(format!(
            "cannot convert '{}' ({:?}) to '{}' ({:?})",
            from.label, from.quantity, to.label, to.quantity
        ));
    }
    Ok(to.from_si(from.to_si(v)))
}

/// Convert a slice in place between two units of the same quantity, returning
/// the scale/offset applied so callers can log it in provenance.
pub fn convert_slice(vs: &mut [f64], from: &Unit, to: &Unit) -> Result<(f64, f64), String> {
    // Validate once, then apply the composed affine map.
    convert(0.0, from, to)?;
    let scale = from.si_factor / to.si_factor;
    let offset = (from.si_offset - to.si_offset) / to.si_factor;
    for v in vs.iter_mut() {
        *v = *v * scale + offset;
    }
    Ok((scale, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_round_trips_through_ms2() {
        let g = unit("g").unwrap();
        let ms2 = unit("m/s^2").unwrap();
        let v = convert(2.0, &g, &ms2).unwrap();
        assert!((v - 2.0 * G0).abs() < 1e-12);
        let back = convert(v, &ms2, &g).unwrap();
        assert!((back - 2.0).abs() < 1e-12);
    }

    #[test]
    fn mil_is_a_thousandth_inch() {
        let mil = unit("mil").unwrap();
        let inch = unit("in").unwrap();
        assert!((convert(1000.0, &mil, &inch).unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn cross_quantity_refuses() {
        let g = unit("g").unwrap();
        let v = unit("V").unwrap();
        assert!(convert(1.0, &g, &v).is_err());
    }

    #[test]
    fn unknown_refuses_even_against_itself() {
        let a = Unit::unknown("blobs");
        assert!(convert(1.0, &a, &a).is_err());
    }

    #[test]
    fn slice_conversion_matches_scalar() {
        let ips = unit("in/s").unwrap();
        let mms = unit("mm/s").unwrap();
        let mut v = [1.0, 2.5, -3.0];
        convert_slice(&mut v, &ips, &mms).unwrap();
        for (got, x) in v.iter().zip([1.0, 2.5, -3.0]) {
            assert!((got - convert(x, &ips, &mms).unwrap()).abs() < 1e-12);
        }
    }

    #[test]
    fn every_catalog_alias_resolves_to_same_quantity() {
        for (a, b) in [("g", "G"), ("in/s", "ips"), ("m/s^2", "m/s2"), ("s", "sec")] {
            assert_eq!(unit(a).unwrap().quantity, unit(b).unwrap().quantity);
            assert_eq!(unit(a).unwrap().si_factor, unit(b).unwrap().si_factor);
        }
    }
}
