//! `hertzforge-shaker-db` - shaker rating database + pre-test feasibility calculator.
//!
//! A JSON-file-backed store of user-imported shaker records (force ratings, velocity
//! / displacement / acceleration limits, armature mass, overturning-moment limit,
//! plus expandable custom fields), recallable by name, one file per shaker under a
//! caller-supplied directory (no hardcoded paths; the suggested default is
//! `<data root>/shakers/`). Records import/export as JSON for sharing.
//!
//! The load-bearing piece is the pre-test [`FeasibilityCalculator::assess`]: given a
//! shaker record, the moving-mass stack, optional test-article geometry, an
//! interpolated reference PSD ([`hertzforge_breakpoints::LineSpectrum`]), and a crest
//! factor, it computes the required force / velocity / displacement / acceleration and
//! compares them LIKE-FOR-LIKE against the rating basis (Rms / Peak / SineEquivalent).
//! Every assumption (crest factor, basis conversions, derations) is listed in the
//! report so the UI and test-report print them.
//!
//! Conventions honored:
//! - Kinematics are RMS (from the profile), peak comparisons apply the crest factor.
//! - Ratings carry a `RatingBasis`; the calculator converts to compare like-for-like.
//! - `SineEquivalent` ratings are approximate: flagged `Advisory`, never a hard fail.
//! - Overturning moment is a STATIC estimate only ("dynamic amplification not
//!   modeled"), evaluated only when geometry is present; absent geometry it is
//!   `NotEvaluated`.
//!
//! Pure library plus filesystem I/O for the store; no panics in library paths.
#![allow(dead_code)]

use hertzforge_breakpoints::{grms, BreakpointTable, LineSpectrum};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Standard gravity, `m/s^2`, re-exported convention shared with `hertzforge-breakpoints`.
pub const G0_M_PER_S2: f64 = hertzforge_breakpoints::G0_M_PER_S2;

// =============================================================================
// Schema
// =============================================================================

/// What a numeric rating value means: is it an RMS number, a peak number, or a
/// sine-equivalent approximation? The feasibility calculator compares LIKE-FOR-LIKE
/// against this basis, so the basis must travel with every rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RatingBasis {
    /// The value is an RMS quantity (compare against the RMS demand).
    Rms,
    /// The value is a peak quantity (compare against `crest_factor * RMS`).
    Peak,
    /// The value is a sine-equivalent rating: an approximate mapping from a sine
    /// force rating to random capability. Compared against peak demand but flagged
    /// `Advisory` (not a hard fail) because the equivalence is approximate.
    SineEquivalent,
}

impl fmt::Display for RatingBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RatingBasis::Rms => write!(f, "RMS"),
            RatingBasis::Peak => write!(f, "peak"),
            RatingBasis::SineEquivalent => write!(f, "sine-equivalent"),
        }
    }
}

/// A rating value paired with its measurement basis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RatingValue {
    pub value: f64,
    pub basis: RatingBasis,
}

impl RatingValue {
    pub fn rms(value: f64) -> Self {
        RatingValue {
            value,
            basis: RatingBasis::Rms,
        }
    }
    pub fn peak(value: f64) -> Self {
        RatingValue {
            value,
            basis: RatingBasis::Peak,
        }
    }
    pub fn sine_equivalent(value: f64) -> Self {
        RatingValue {
            value,
            basis: RatingBasis::SineEquivalent,
        }
    }
}

/// A shaker record. Optional fields are `None` when the manufacturer does not
/// publish that limit; the feasibility calculator marks the corresponding check
/// `NotEvaluated` rather than assuming a value. `custom` carries expandable
/// user-defined key/value fields (e.g. flexure limits, coolant flow) that the UI
/// can round-trip without a schema change.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ShakerRecord {
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub notes: String,
    /// Random force rating, `lbf`.
    pub force_random: Option<RatingValue>,
    /// Sine force rating, `lbf`.
    pub force_sine: Option<RatingValue>,
    /// Shock force rating, `lbf`.
    pub force_shock: Option<RatingValue>,
    /// Maximum velocity, `in/s`.
    pub max_velocity_ips: Option<RatingValue>,
    /// Maximum displacement, peak-to-peak `in`.
    pub max_displacement_in_pp: Option<RatingValue>,
    /// Maximum bare-armature acceleration, `g`.
    pub max_accel_bare_g: Option<RatingValue>,
    /// Armature mass, `lb`.
    pub armature_mass_lb: Option<f64>,
    /// Overturning-moment limit, `lbf*in`.
    pub overturning_moment_limit_lbf_in: Option<f64>,
    /// Expandable user-defined fields.
    pub custom: Vec<(String, String)>,
}

impl ShakerRecord {
    /// A record with only a name set.
    pub fn new(name: impl Into<String>) -> Self {
        ShakerRecord {
            name: name.into(),
            ..Default::default()
        }
    }
}

/// The moving-mass stack, all in `lb`. `total()` is the sum that drives the
/// `F = m * a` demand and the acceleration deration.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MovingMass {
    pub armature_lb: f64,
    pub slip_table_lb: f64,
    pub drive_adapter_lb: f64,
    pub fixturing_lb: f64,
    pub test_article_lb: f64,
}

impl MovingMass {
    /// Total moving mass, `lb`.
    pub fn total(&self) -> f64 {
        self.armature_lb
            + self.slip_table_lb
            + self.drive_adapter_lb
            + self.fixturing_lb
            + self.test_article_lb
    }
}

/// Test-article geometry for the static overturning-moment estimate. Both fields
/// optional; if `cg_offset_in` is absent, overturning is `NotEvaluated`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ArticleGeometry {
    /// Horizontal offset of the article CG from the armature/table axis, `in`
    /// (drives the primary static overturning moment `F_peak * cg_offset`).
    pub cg_offset_in: Option<f64>,
    /// Height of the article CG above the table, `in` (used for a lateral-moment
    /// estimate when present).
    pub cg_height_in: Option<f64>,
}

// =============================================================================
// Feasibility report types
// =============================================================================

/// Verdict for a single feasibility item and for the overall assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Within rating with margin.
    Pass,
    /// A soft concern that does not fail the overall assessment but is surfaced
    /// (e.g. a sine-equivalent force comparison).
    Advisory,
    /// The demand exceeds the rating: a hard failure.
    Exceeded,
    /// The rating (or the geometry needed to evaluate) is absent; not assessed.
    NotEvaluated,
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Pass => write!(f, "Pass"),
            Verdict::Advisory => write!(f, "Advisory"),
            Verdict::Exceeded => write!(f, "Exceeded"),
            Verdict::NotEvaluated => write!(f, "NotEvaluated"),
        }
    }
}

/// The physical quantity a [`FeasibilityItem`] concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantity {
    Force,
    Velocity,
    Displacement,
    Acceleration,
    OverturningMoment,
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quantity::Force => write!(f, "force"),
            Quantity::Velocity => write!(f, "velocity"),
            Quantity::Displacement => write!(f, "displacement"),
            Quantity::Acceleration => write!(f, "acceleration"),
            Quantity::OverturningMoment => write!(f, "overturning moment"),
        }
    }
}

/// One line of the feasibility assessment: the demanded value compared against the
/// rating (with its basis), the margin, and the verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityItem {
    pub quantity: Quantity,
    /// Human label, e.g. "random force (peak)".
    pub label: String,
    /// The computed demand value in the item's engineering unit.
    pub computed_value: f64,
    /// The rating value compared against (in the same unit), when a rating exists.
    pub rating_value: Option<f64>,
    /// The basis the rating was compared on, when a rating exists.
    pub rating_basis: Option<RatingBasis>,
    /// Margin as a linear ratio `rating / demand` (>1 = pass), when both are known.
    pub margin_ratio: Option<f64>,
    /// Margin in dB `10*log10(rating^2 / demand^2)` = `20*log10(rating/demand)`,
    /// when both are known. Positive = headroom.
    pub margin_db: Option<f64>,
    pub verdict: Verdict,
}

/// The full pre-test feasibility report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityReport {
    pub items: Vec<FeasibilityItem>,
    /// Worst hard verdict across the items (Advisory does not fail overall but is
    /// surfaced). `NotEvaluated` items do not set the overall verdict.
    pub overall: Verdict,
    /// Every assumption made (crest factor, basis conversions, derations), listed so
    /// the UI and the printed test report can show them.
    pub assumptions: Vec<String>,
}

impl FeasibilityReport {
    /// True if the overall verdict is a hard exceedance.
    pub fn exceeded(&self) -> bool {
        self.overall == Verdict::Exceeded
    }
    /// True if any item is an Advisory (surfaced but not failing).
    pub fn has_advisory(&self) -> bool {
        self.items.iter().any(|i| i.verdict == Verdict::Advisory)
    }
}

// =============================================================================
// Feasibility calculator
// =============================================================================

/// Pounds-mass to slug conversion divisor: `1 slug = 32.174 lbm` (standard gravity
/// in ft/s^2). Used so `F[lbf] = m[slug] * a[ft/s^2]`; equivalently, with `a`
/// expressed in g, `F[lbf] = m[lbm] * a[g]` directly (the g-based shortcut used
/// here, documented in the assumptions list).
pub const LBM_PER_SLUG: f64 = 32.174_048_556_430_4;

/// Inches per metre (shared convention).
pub const IN_PER_M: f64 = hertzforge_breakpoints::IN_PER_M;

/// The default crest factor (3-sigma) applied to convert RMS demand to peak.
pub const DEFAULT_CREST_FACTOR: f64 = 3.0;

/// The feasibility calculator. Stateless; grouped as a type so the entry point and
/// its helpers read as a unit and the UI can hold one instance.
pub struct FeasibilityCalculator;

impl FeasibilityCalculator {
    /// Assess a proposed random test against a shaker record.
    ///
    /// # Inputs
    /// - `record`: the shaker's ratings.
    /// - `moving_mass`: the mass stack; `moving_mass.total()` is `m_total`.
    /// - `geometry`: optional test-article CG geometry (overturning moment).
    /// - `profile`: the interpolated reference PSD ([`LineSpectrum`], acceleration
    ///   density in `g^2/Hz`) whose gRMS is the RMS acceleration demand.
    /// - `crest_factor`: sigma multiplier for RMS -> peak (default
    ///   [`DEFAULT_CREST_FACTOR`] if the caller passes `<= 0`).
    ///
    /// # Method
    /// - `a_rms = grms(profile)` (g). `F_rms = m_total * a_rms` (lbf, g-based
    ///   shortcut). `F_peak = crest_factor * F_rms`.
    /// - Force is compared against `force_random` LIKE-FOR-LIKE by basis: an `Rms`
    ///   rating compares to `F_rms`; a `Peak` rating to `F_peak`; a `SineEquivalent`
    ///   rating to `F_peak` but the verdict is capped at `Advisory` (approximate).
    /// - Velocity / displacement RMS come from the breakpoint kinematics; peak
    ///   comparisons apply the crest factor; displacement is compared pk-pk
    ///   (`2 * peak`) against `max_displacement_in_pp`.
    /// - Acceleration headroom uses the derated ceiling
    ///   `min(bare-accel rating, force_rating / m_total)`; both `a_rms` and
    ///   `crest * a_rms` are checked.
    /// - Overturning: only when `geometry.cg_offset_in` is present -> static moment
    ///   `F_peak * cg_offset` vs the limit, ALWAYS labeled static-estimate-only; a
    ///   lateral estimate is added when `cg_height_in` is present. Absent geometry
    ///   -> `NotEvaluated`.
    ///
    /// The profile must be given as a [`LineSpectrum`]; to derive the kinematics the
    /// caller also supplies the source [`BreakpointTable`] via
    /// [`FeasibilityCalculator::assess_with_table`]. This entry point derives
    /// velocity / displacement from the line spectrum directly (discrete
    /// integration), which agrees with the analytic table kinematics to within grid
    /// tolerance; use [`FeasibilityCalculator::assess_with_table`] when the table is
    /// on hand for the exact analytic kinematics.
    pub fn assess(
        record: &ShakerRecord,
        moving_mass: MovingMass,
        geometry: Option<ArticleGeometry>,
        profile: &LineSpectrum,
        crest_factor: f64,
    ) -> FeasibilityReport {
        let a_rms = grms(profile);
        let (v_rms_ips, d_rms_in) = line_spectrum_kinematics(profile);
        Self::assess_inner(
            record,
            moving_mass,
            geometry,
            a_rms,
            v_rms_ips,
            d_rms_in,
            crest_factor,
            "line-spectrum discrete integration",
        )
    }

    /// As [`FeasibilityCalculator::assess`] but takes the source breakpoint table so
    /// the velocity / displacement RMS use the exact analytic per-segment integrals
    /// (`hertzforge-breakpoints` kinematics) rather than the discrete grid sums. The
    /// acceleration RMS still uses the interpolated profile's gRMS so the force
    /// demand matches what the control loop will actually realize on the line grid.
    pub fn assess_with_table(
        record: &ShakerRecord,
        moving_mass: MovingMass,
        geometry: Option<ArticleGeometry>,
        table: &BreakpointTable,
        profile: &LineSpectrum,
        crest_factor: f64,
    ) -> Result<FeasibilityReport, hertzforge_breakpoints::BreakpointError> {
        let a_rms = grms(profile);
        let v_rms_ips = table.velocity_rms_ips()?;
        let d_rms_in = table.displacement_rms_mil()? / 1000.0; // mil -> in
        Ok(Self::assess_inner(
            record,
            moving_mass,
            geometry,
            a_rms,
            v_rms_ips,
            d_rms_in,
            crest_factor,
            "analytic per-segment kinematics (hertzforge-breakpoints)",
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assess_inner(
        record: &ShakerRecord,
        moving_mass: MovingMass,
        geometry: Option<ArticleGeometry>,
        a_rms_g: f64,
        v_rms_ips: f64,
        d_rms_in: f64,
        crest_factor: f64,
        kinematics_source: &str,
    ) -> FeasibilityReport {
        let crest = if crest_factor > 0.0 {
            crest_factor
        } else {
            DEFAULT_CREST_FACTOR
        };
        let m_total = moving_mass.total();
        let mut items = Vec::new();
        let mut assumptions = vec![
            format!("Crest factor = {crest:.2} sigma (RMS -> peak)."),
            format!("Total moving mass = {m_total:.2} lb (armature + slip table + drive adapter + fixturing + test article)."),
            format!("Kinematics source: {kinematics_source}."),
            "Force demand uses F[lbf] = m[lb] * a[g] (g-based shortcut; weight and mass share the lbm number under standard gravity).".to_string(),
        ];

        // --- force ---
        let f_rms = m_total * a_rms_g; // lbf
        let f_peak = crest * f_rms;
        assumptions.push(format!(
            "a_rms = {a_rms_g:.4} g -> F_rms = {f_rms:.1} lbf, F_peak = {f_peak:.1} lbf."
        ));
        items.push(force_item(record.force_random, f_rms, f_peak, crest));

        // --- velocity ---
        let v_peak = crest * v_rms_ips;
        items.push(kinematic_item(
            Quantity::Velocity,
            "velocity",
            "in/s",
            record.max_velocity_ips,
            v_rms_ips,
            v_peak,
            crest,
        ));

        // --- displacement (compared pk-pk = 2*peak) ---
        let d_peak = crest * d_rms_in;
        let d_pp = 2.0 * d_peak;
        assumptions.push(format!(
            "d_rms = {:.4} in -> d_peak = {:.4} in -> d_pk-pk = {:.4} in (compared against pk-pk rating).",
            d_rms_in, d_peak, d_pp
        ));
        items.push(displacement_item(record.max_displacement_in_pp, d_pp));

        // --- acceleration headroom (derated by mass) ---
        items.push(accel_item(
            record,
            m_total,
            a_rms_g,
            crest,
            &mut assumptions,
        ));

        // --- overturning moment (static estimate only, geometry-gated) ---
        items.push(overturning_item(
            record.overturning_moment_limit_lbf_in,
            geometry,
            f_peak,
            &mut assumptions,
        ));

        let overall = worst_verdict(&items);
        FeasibilityReport {
            items,
            overall,
            assumptions,
        }
    }
}

// --- item builders (module-private) ------------------------------------------

/// Build the force feasibility item, comparing like-for-like by the rating basis.
fn force_item(
    rating: Option<RatingValue>,
    f_rms: f64,
    f_peak: f64,
    _crest: f64,
) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity: Quantity::Force,
            label: "random force".into(),
            computed_value: f_rms,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (demand, label) = match r.basis {
                RatingBasis::Rms => (f_rms, "random force (RMS)"),
                RatingBasis::Peak => (f_peak, "random force (peak)"),
                RatingBasis::SineEquivalent => (f_peak, "random force (peak vs sine-equivalent)"),
            };
            let (ratio, db) = margins(r.value, demand);
            let base_verdict = verdict_from_ratio(ratio);
            // Sine-equivalent ratings are approximate: cap a would-be Exceeded/Pass
            // at Advisory only when it would otherwise be a hard PASS-or-FAIL claim
            // we cannot stand behind. Sine-equivalent never HARD-fails; a
            // pass is reported as Advisory too so the operator knows the basis is
            // approximate.
            let verdict = match r.basis {
                RatingBasis::SineEquivalent => Verdict::Advisory,
                _ => base_verdict,
            };
            FeasibilityItem {
                quantity: Quantity::Force,
                label: label.into(),
                computed_value: demand,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict,
            }
        }
    }
}

/// Build a velocity feasibility item (peak comparison applies the crest factor).
#[allow(clippy::too_many_arguments)]
fn kinematic_item(
    quantity: Quantity,
    name: &str,
    unit: &str,
    rating: Option<RatingValue>,
    rms: f64,
    peak: f64,
    _crest: f64,
) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity,
            label: format!("{name} ({unit})"),
            computed_value: rms,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (demand, basis_label) = match r.basis {
                RatingBasis::Rms => (rms, "RMS"),
                RatingBasis::Peak => (peak, "peak"),
                RatingBasis::SineEquivalent => (peak, "peak vs sine-equivalent"),
            };
            let (ratio, db) = margins(r.value, demand);
            let verdict = match r.basis {
                RatingBasis::SineEquivalent => Verdict::Advisory,
                _ => verdict_from_ratio(ratio),
            };
            FeasibilityItem {
                quantity,
                label: format!("{name} ({basis_label})"),
                computed_value: demand,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict,
            }
        }
    }
}

/// Displacement item: the pk-pk demand (`2 * peak`) vs the pk-pk rating. The rating
/// is a physical envelope (a stroke limit), so it is treated as a hard peak-to-peak
/// bound regardless of the stored `RatingBasis`.
fn displacement_item(rating: Option<RatingValue>, d_pp: f64) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity: Quantity::Displacement,
            label: "displacement (pk-pk)".into(),
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
                quantity: Quantity::Displacement,
                label: "displacement (pk-pk)".into(),
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

/// Acceleration item: derate the ceiling by mass. The usable acceleration ceiling
/// is `min(bare-armature accel rating, force_random / m_total)` because at a given
/// moving mass the force rating caps the achievable acceleration to `F/m`. Both the
/// RMS and the crest-scaled peak acceleration are checked against that ceiling.
fn accel_item(
    record: &ShakerRecord,
    m_total: f64,
    a_rms_g: f64,
    crest: f64,
    assumptions: &mut Vec<String>,
) -> FeasibilityItem {
    let bare = record.max_accel_bare_g.map(|r| r.value);
    let force_derived = record
        .force_random
        .filter(|_| m_total > 0.0)
        .map(|f| f.value / m_total); // g-based: F[lbf]/m[lb] = a[g]
    let ceiling = match (bare, force_derived) {
        (Some(b), Some(fd)) => Some(b.min(fd)),
        (Some(b), None) => Some(b),
        (None, Some(fd)) => Some(fd),
        (None, None) => None,
    };
    // If the ceiling is set BY the force-derived term (force <= bare, or bare
    // absent) AND that force rating is sine-equivalent, the acceleration ceiling
    // inherits that approximate basis: the verdict is capped at Advisory (never a
    // hard fail), consistent with the treatment of sine-equivalent force ratings.
    let force_is_sine = matches!(
        record.force_random.map(|f| f.basis),
        Some(RatingBasis::SineEquivalent)
    );
    let ceiling_from_force = match (bare, force_derived) {
        (Some(b), Some(fd)) => fd <= b,
        (None, Some(_)) => true,
        _ => false,
    };
    let a_peak = crest * a_rms_g;
    match ceiling {
        None => FeasibilityItem {
            quantity: Quantity::Acceleration,
            label: "acceleration (g)".into(),
            computed_value: a_rms_g,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(c) => {
            assumptions.push(format!(
                "Acceleration ceiling = {c:.2} g = min(bare-armature rating, force_random / m_total); peak demand = {a_peak:.2} g."
            ));
            // Compare the PEAK acceleration (worst case) against the ceiling.
            let (ratio, db) = margins(c, a_peak);
            let verdict = if ceiling_from_force && force_is_sine {
                Verdict::Advisory
            } else {
                verdict_from_ratio(ratio)
            };
            FeasibilityItem {
                quantity: Quantity::Acceleration,
                label: "acceleration (peak vs derated ceiling)".into(),
                computed_value: a_peak,
                rating_value: Some(c),
                rating_basis: Some(RatingBasis::Peak),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict,
            }
        }
    }
}

/// Overturning-moment item: static estimate only, geometry-gated.
fn overturning_item(
    limit: Option<f64>,
    geometry: Option<ArticleGeometry>,
    f_peak: f64,
    assumptions: &mut Vec<String>,
) -> FeasibilityItem {
    let not_evaluated = |reason: &str| FeasibilityItem {
        quantity: Quantity::OverturningMoment,
        label: format!("overturning moment ({reason})"),
        computed_value: 0.0,
        rating_value: limit,
        rating_basis: None,
        margin_ratio: None,
        margin_db: None,
        verdict: Verdict::NotEvaluated,
    };
    let Some(limit) = limit else {
        return not_evaluated("no limit published");
    };
    let Some(geom) = geometry else {
        return not_evaluated("no geometry provided");
    };
    let Some(offset) = geom.cg_offset_in else {
        return not_evaluated("no CG offset provided");
    };
    // Static moment = F_peak * cg_offset. If cg_height is present, add a lateral
    // estimate (a fraction of peak force assumed to act at the CG height); we take
    // the primary offset moment as the compared quantity and note the lateral term.
    let static_moment = f_peak * offset.abs();
    let mut note = format!(
        "Overturning moment = F_peak ({f_peak:.1} lbf) * CG offset ({:.2} in) = {static_moment:.1} lbf*in. STATIC ESTIMATE ONLY (dynamic amplification not modeled).",
        offset.abs()
    );
    let compared = if let Some(h) = geom.cg_height_in {
        let lateral = f_peak * h.abs();
        note.push_str(&format!(
            " Lateral estimate F_peak * CG height ({:.2} in) = {lateral:.1} lbf*in; the larger of the two is compared.",
            h.abs()
        ));
        static_moment.max(lateral)
    } else {
        static_moment
    };
    assumptions.push(note);
    let (ratio, db) = margins(limit, compared);
    FeasibilityItem {
        quantity: Quantity::OverturningMoment,
        label: "overturning moment (static estimate only)".into(),
        computed_value: compared,
        rating_value: Some(limit),
        rating_basis: None,
        margin_ratio: Some(ratio),
        margin_db: Some(db),
        verdict: verdict_from_ratio(ratio),
    }
}

/// `(ratio, db)` margin from a rating and a demand. `ratio = rating / demand`,
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

/// Pass if the rating exceeds the demand (ratio >= 1), else Exceeded.
fn verdict_from_ratio(ratio: f64) -> Verdict {
    if ratio >= 1.0 {
        Verdict::Pass
    } else {
        Verdict::Exceeded
    }
}

/// The overall verdict is the worst HARD verdict among items: any `Exceeded` -> the
/// report is Exceeded; else if all evaluated items pass -> Pass; else NotEvaluated
/// (nothing was assessable). Advisory does NOT fail the overall but is surfaced
/// via [`FeasibilityReport::has_advisory`].
fn worst_verdict(items: &[FeasibilityItem]) -> Verdict {
    if items.iter().any(|i| i.verdict == Verdict::Exceeded) {
        Verdict::Exceeded
    } else if items.iter().any(|i| i.verdict == Verdict::Pass) {
        Verdict::Pass
    } else {
        // no exceedance and no pass: everything was NotEvaluated or Advisory.
        if items.iter().any(|i| i.verdict == Verdict::Advisory) {
            Verdict::Advisory
        } else {
            Verdict::NotEvaluated
        }
    }
}

/// Discrete velocity/displacement RMS from a line spectrum (acceleration density
/// `g^2/Hz`), returned as `(v_rms in/s, d_rms in)`. Uses the same
/// `(g0/(2 pi f))^n` integrands as the analytic `hertzforge-breakpoints` kinematics, summed
/// over the in-band lines. DC (`f = 0`) is skipped (the `1/f` weighting diverges and
/// carries no controllable energy). This mirrors the analytic path within grid
/// tolerance and is used by [`FeasibilityCalculator::assess`] when only the line
/// spectrum is on hand.
fn line_spectrum_kinematics(profile: &LineSpectrum) -> (f64, f64) {
    let kv = (G0_M_PER_S2 / std::f64::consts::TAU).powi(2);
    let kd = (G0_M_PER_S2 / (std::f64::consts::TAU * std::f64::consts::TAU)).powi(2);
    let mut v2 = 0.0;
    let mut d2 = 0.0;
    for (k, &p) in profile.psd.iter().enumerate() {
        if p <= 0.0 {
            continue;
        }
        let f = profile.freq(k);
        if f <= 0.0 {
            continue;
        }
        let f2 = f * f;
        v2 += p / f2 * profile.df_hz;
        d2 += p / (f2 * f2) * profile.df_hz;
    }
    let v_rms_m = (kv * v2).max(0.0).sqrt();
    let d_rms_m = (kd * d2).max(0.0).sqrt();
    (v_rms_m * IN_PER_M, d_rms_m * IN_PER_M)
}

// =============================================================================
// Store: one JSON file per shaker under a caller-supplied directory
// =============================================================================

/// Errors from the JSON-file-backed store.
#[derive(Debug)]
pub enum StoreError {
    /// Filesystem I/O error, with the path it happened on.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// JSON (de)serialization error.
    Json(String),
    /// A record name sanitized to an empty filename (e.g. all-punctuation name).
    EmptyName,
    /// No shaker found with the requested name.
    NotFound(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io { path, source } => {
                write!(f, "I/O error on {}: {source}", path.display())
            }
            StoreError::Json(s) => write!(f, "JSON error: {s}"),
            StoreError::EmptyName => write!(f, "shaker name is empty after filename sanitization"),
            StoreError::NotFound(n) => write!(f, "no shaker named {n:?} in the store"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A JSON-file-backed shaker database rooted at a caller-supplied directory. One
/// file per shaker, named from the sanitized shaker name. Recall-by-name is the UI
/// path; `import_json` / `export_json` move a single record in and out for sharing.
///
/// The directory is created on [`ShakerDb::open`] if it does not exist. There are no
/// hardcoded paths; the suggested default (documented, not enforced) is
/// `<data root>/shakers/`.
pub struct ShakerDb {
    dir: PathBuf,
}

impl ShakerDb {
    /// Open (creating if needed) a store rooted at `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io {
            path: dir.clone(),
            source: e,
        })?;
        Ok(ShakerDb { dir })
    }

    /// The store's root directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The on-disk path a record with `name` would occupy.
    pub fn path_for(&self, name: &str) -> Result<PathBuf, StoreError> {
        let file = sanitize_filename(name);
        if file.is_empty() {
            return Err(StoreError::EmptyName);
        }
        Ok(self.dir.join(format!("{file}.json")))
    }

    /// List the names of all stored shakers, read from each file's `name` field
    /// (not the filename, so a renamed-on-disk file still reports its true name).
    /// Sorted for stable UI presentation.
    pub fn list(&self) -> Result<Vec<String>, StoreError> {
        let mut names = Vec::new();
        let entries = std::fs::read_dir(&self.dir).map_err(|e| StoreError::Io {
            path: self.dir.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| StoreError::Io {
                path: self.dir.clone(),
                source: e,
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|e| StoreError::Io {
                path: path.clone(),
                source: e,
            })?;
            if let Ok(rec) = serde_json::from_str::<ShakerRecord>(&text) {
                names.push(rec.name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Load a shaker by name. Returns [`StoreError::NotFound`] if no matching file.
    pub fn load(&self, name: &str) -> Result<ShakerRecord, StoreError> {
        let path = self.path_for(name)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(name.to_string()))
            }
            Err(e) => return Err(StoreError::Io { path, source: e }),
        };
        serde_json::from_str(&text).map_err(|e| StoreError::Json(e.to_string()))
    }

    /// Save a record, overwriting any existing file for its name. The filename is
    /// derived from `record.name`.
    pub fn save(&self, record: &ShakerRecord) -> Result<PathBuf, StoreError> {
        let path = self.path_for(&record.name)?;
        let json =
            serde_json::to_string_pretty(record).map_err(|e| StoreError::Json(e.to_string()))?;
        std::fs::write(&path, json).map_err(|e| StoreError::Io {
            path: path.clone(),
            source: e,
        })?;
        Ok(path)
    }

    /// Delete a shaker by name. Returns [`StoreError::NotFound`] if absent.
    pub fn delete(&self, name: &str) -> Result<(), StoreError> {
        let path = self.path_for(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(name.to_string()))
            }
            Err(e) => Err(StoreError::Io { path, source: e }),
        }
    }

    /// True if a shaker with `name` exists in the store.
    pub fn contains(&self, name: &str) -> Result<bool, StoreError> {
        Ok(self.path_for(name)?.exists())
    }

    /// Serialize a record to a JSON string for sharing (does not touch the store).
    pub fn export_json(record: &ShakerRecord) -> Result<String, StoreError> {
        serde_json::to_string_pretty(record).map_err(|e| StoreError::Json(e.to_string()))
    }

    /// Parse a shared JSON string into a record and SAVE it into the store.
    pub fn import_json(&self, json: &str) -> Result<ShakerRecord, StoreError> {
        let rec: ShakerRecord =
            serde_json::from_str(json).map_err(|e| StoreError::Json(e.to_string()))?;
        self.save(&rec)?;
        Ok(rec)
    }
}

/// Sanitize a shaker name into a safe filename stem: keep ASCII alphanumerics, map
/// spaces and separators to `_`, drop everything else, and collapse repeated `_`.
/// Case is preserved. An all-punctuation name sanitizes to empty (caught by the
/// store as [`StoreError::EmptyName`]).
fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_us = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if matches!(c, ' ' | '-' | '_' | '.' | '/' | '\\') && !last_us && !out.is_empty() {
            out.push('_');
            last_us = true;
        }
        // all other characters (and separators that would double an underscore or
        // lead the string) are dropped.
    }
    // trim a trailing underscore.
    while out.ends_with('_') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests;
