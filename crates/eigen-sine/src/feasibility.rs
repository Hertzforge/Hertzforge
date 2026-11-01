//! Deterministic sine feasibility: per-frequency analytic maxima over the
//! profile band compared like-for-like against the `eigen-shaker-db` sine
//! ratings. Crest factor is 1 by definition (a sine's peak IS its amplitude),
//! so unlike the random calculator there is no crest-factor knob. Reuses
//! `eigen-shaker-db` report TYPES only; does NOT call its PSD calculator.

use crate::profile::{
    accel_g_pk_of, displacement_in_pp_from_accel, velocity_ips_pk_from_accel, SineProfile,
};
use eigen_shaker_db::{
    FeasibilityItem, FeasibilityReport, MovingMass, Quantity as DbQuantity, RatingBasis,
    RatingValue, ShakerRecord, Verdict,
};
use std::f64::consts::SQRT_2;

/// A per-quantity worst case over the profile band: the maximum demanded value
/// and the frequency it occurs at.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Extreme {
    value: f64,
    freq_hz: f64,
}

/// Band-wide worst-case sine kinematics.
#[derive(Debug, Clone, Copy, PartialEq)]
struct WorstCase {
    accel_g_pk: Extreme,
    velocity_ips_pk: Extreme,
    displacement_in_pp: Extreme,
}

/// Per-frequency maxima over the profile band, computed ANALYTICALLY from the
/// segment endpoints.
///
/// Why endpoints suffice: within a segment the governing quantity is either
/// constant (differing-quantity neighbors) or a log-log straight line
/// (same-quantity neighbors), i.e. a power law `q(f) = Q * f^k`. The SHM
/// conversions multiply by integer powers of `f` (`a ~ q * f^p` for some fixed
/// `p` per quantity), so the acceleration target on a segment is `a(f) = A *
/// f^m`, and likewise `v(f) ~ a/f = A * f^(m-1)` and `d(f) ~ a/f^2 = A *
/// f^(m-2)` are power laws too. A power law is monotone on an interval, so each
/// of a, v, d attains its segment maximum at a segment endpoint - evaluating
/// the endpoints of every segment and taking the running maximum is exact.
///
/// Interior boundaries are evaluated from BOTH adjoining segments (a
/// differing-quantity join may be discontinuous), so the supremum is not missed.
/// Returns `None` when the profile has fewer than 2 rows.
fn worst_case(profile: &SineProfile) -> Option<WorstCase> {
    if profile.rows.len() < 2 {
        return None;
    }
    let mut a = Extreme {
        value: f64::NEG_INFINITY,
        freq_hz: 0.0,
    };
    let mut v = a;
    let mut d = a;
    for w in profile.rows.windows(2) {
        let (l, r) = (&w[0], &w[1]);
        // Segment endpoint values in the governing (left-row) quantity: the
        // log-log interpolation passes through the row values at the endpoints;
        // a differing-quantity segment holds the left row's constant value.
        let endpoints: [(f64, f64); 2] = if l.quantity == r.quantity {
            [(l.freq_hz, l.value), (r.freq_hz, r.value)]
        } else {
            [(l.freq_hz, l.value), (r.freq_hz, l.value)]
        };
        for (f, val) in endpoints {
            let a_g = accel_g_pk_of(l.quantity, val, f);
            let v_ips = velocity_ips_pk_from_accel(a_g, f);
            let d_pp = displacement_in_pp_from_accel(a_g, f);
            if a_g > a.value {
                a = Extreme {
                    value: a_g,
                    freq_hz: f,
                };
            }
            if v_ips > v.value {
                v = Extreme {
                    value: v_ips,
                    freq_hz: f,
                };
            }
            if d_pp > d.value {
                d = Extreme {
                    value: d_pp,
                    freq_hz: f,
                };
            }
        }
    }
    if a.value.is_finite() && v.value.is_finite() && d.value.is_finite() {
        Some(WorstCase {
            accel_g_pk: a,
            velocity_ips_pk: v,
            displacement_in_pp: d,
        })
    } else {
        None
    }
}

/// Assess a sine profile against a shaker record: deterministic, crest = 1.
/// Items: peak force vs `force_sine`, peak velocity vs `max_velocity_ips`,
/// pk-pk displacement vs `max_displacement_in_pp`, peak acceleration vs
/// `max_accel_bare_g`. Basis handling is like-for-like:
///
/// - `Peak` ratings compare against the peak demand directly.
/// - `SineEquivalent` ratings ALSO compare against the peak demand with a FULL
///   verdict (never capped at Advisory): sine-equivalent is the NATIVE basis
///   for a sine test, a deliberate difference from the random module's rule
///   (where the sine->random equivalence is approximate and stays advisory).
/// - `Rms` ratings compare against `peak / sqrt(2)` (sine RMS).
/// - Displacement pk-pk ratings are a physical stroke envelope and compare
///   directly against the pk-pk demand regardless of stored basis.
///
/// A missing rating yields `Verdict::NotEvaluated`. An invalid profile (fails
/// [`SineProfile::validate`]) yields an empty `NotEvaluated` report whose
/// assumptions carry the validation error.
pub fn assess_sine(
    record: &ShakerRecord,
    moving_mass: &MovingMass,
    profile: &SineProfile,
) -> FeasibilityReport {
    let base_assumptions = vec![
        "deterministic sine (crest factor 1.0)".to_string(),
        "per-frequency analytic maxima at segment endpoints".to_string(),
        "SineEquivalent force basis treated as native (full verdict)".to_string(),
    ];
    if let Err(e) = profile.validate() {
        let mut assumptions = base_assumptions;
        assumptions.push(format!("profile failed validation, nothing assessed: {e}"));
        return FeasibilityReport {
            items: Vec::new(),
            overall: Verdict::NotEvaluated,
            assumptions,
        };
    }
    let Some(wc) = worst_case(profile) else {
        let mut assumptions = base_assumptions;
        assumptions.push("profile has no evaluable segments; nothing assessed".to_string());
        return FeasibilityReport {
            items: Vec::new(),
            overall: Verdict::NotEvaluated,
            assumptions,
        };
    };

    let m_total = moving_mass.total();
    let f_pk = m_total * wc.accel_g_pk.value; // lbf, g-based shortcut
    let mut assumptions = base_assumptions;
    assumptions.push(format!(
        "Total moving mass = {m_total:.2} lb (armature + slip table + drive adapter + \
         fixturing + test article)."
    ));
    assumptions.push(
        "Force demand uses F[lbf] = m[lb] * a[g] (g-based shortcut; weight and mass share \
         the lbm number under standard gravity)."
            .to_string(),
    );
    assumptions.push("Sine RMS = peak / sqrt(2) for Rms-basis ratings.".to_string());
    assumptions.push(format!(
        "Worst-case demands over the band: {:.4} g pk @ {:.3} Hz; {:.4} in/s pk @ {:.3} Hz; \
         {:.4} in pk-pk @ {:.3} Hz -> F_pk = {f_pk:.1} lbf.",
        wc.accel_g_pk.value,
        wc.accel_g_pk.freq_hz,
        wc.velocity_ips_pk.value,
        wc.velocity_ips_pk.freq_hz,
        wc.displacement_in_pp.value,
        wc.displacement_in_pp.freq_hz,
    ));

    let items = vec![
        peak_native_item(DbQuantity::Force, "sine force", f_pk, record.force_sine),
        peak_native_item(
            DbQuantity::Velocity,
            "sine velocity",
            wc.velocity_ips_pk.value,
            record.max_velocity_ips,
        ),
        displacement_item(wc.displacement_in_pp.value, record.max_displacement_in_pp),
        peak_native_item(
            DbQuantity::Acceleration,
            "sine acceleration",
            wc.accel_g_pk.value,
            record.max_accel_bare_g,
        ),
    ];

    let overall = worst_verdict(&items);
    FeasibilityReport {
        items,
        overall,
        assumptions,
    }
}

/// Build an item for a quantity whose sine demand is natively PEAK. Basis
/// handling per the module docs: `Peak` and `SineEquivalent` compare the peak
/// demand with a FULL verdict (sine-equivalent is native here, deliberately
/// unlike the random module); `Rms` compares `peak / sqrt(2)`.
fn peak_native_item(
    quantity: DbQuantity,
    name: &str,
    demand_pk: f64,
    rating: Option<RatingValue>,
) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity,
            label: format!("{name} (peak)"),
            computed_value: demand_pk,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (demand, basis_label) = match r.basis {
                RatingBasis::Rms => (demand_pk / SQRT_2, "RMS"),
                RatingBasis::Peak => (demand_pk, "peak"),
                RatingBasis::SineEquivalent => {
                    (demand_pk, "peak, sine-equivalent basis - native for sine")
                }
            };
            let (ratio, db) = margins(r.value, demand);
            FeasibilityItem {
                quantity,
                label: format!("{name} ({basis_label})"),
                computed_value: demand,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                // Full verdict for EVERY basis, including SineEquivalent.
                verdict: verdict_from_ratio(ratio),
            }
        }
    }
}

/// Displacement item: the pk-pk demand vs the pk-pk stroke rating. The rating
/// is a physical envelope, so it is a hard peak-to-peak bound regardless of the
/// stored `RatingBasis` (mirrors the random calculator's displacement rule).
fn displacement_item(d_pp: f64, rating: Option<RatingValue>) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity: DbQuantity::Displacement,
            label: "sine displacement (pk-pk)".into(),
            computed_value: d_pp,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (ratio, db) = margins(r.value, d_pp);
            FeasibilityItem {
                quantity: DbQuantity::Displacement,
                label: "sine displacement (pk-pk)".into(),
                computed_value: d_pp,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict: verdict_from_ratio(ratio),
            }
        }
    }
}

/// `(ratio, db)` margin from a rating and a demand: `ratio = rating / demand`,
/// `db = 20*log10(ratio)`. A zero demand yields infinite headroom.
fn margins(rating: f64, demand: f64) -> (f64, f64) {
    if demand <= 0.0 {
        return (f64::INFINITY, f64::INFINITY);
    }
    let ratio = rating / demand;
    let db = if ratio > 0.0 {
        20.0 * ratio.log10()
    } else {
        f64::NEG_INFINITY
    };
    (ratio, db)
}

/// Pass if the rating covers the demand (ratio >= 1), else Exceeded.
fn verdict_from_ratio(ratio: f64) -> Verdict {
    if ratio >= 1.0 {
        Verdict::Pass
    } else {
        Verdict::Exceeded
    }
}

/// The overall verdict is the worst HARD verdict among items (mirrors
/// `eigen-shaker-db`): any `Exceeded` fails the report; else any `Pass` passes
/// it; else Advisory / NotEvaluated fall through in that order. (This module
/// never emits Advisory, but the aggregation stays shape-compatible.)
fn worst_verdict(items: &[FeasibilityItem]) -> Verdict {
    if items.iter().any(|i| i.verdict == Verdict::Exceeded) {
        Verdict::Exceeded
    } else if items.iter().any(|i| i.verdict == Verdict::Pass) {
        Verdict::Pass
    } else if items.iter().any(|i| i.verdict == Verdict::Advisory) {
        Verdict::Advisory
    } else {
        Verdict::NotEvaluated
    }
}
