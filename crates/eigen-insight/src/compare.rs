//! The compare engine: the compare dispatch table, executed - with the
//! nine-field CompareResult and honest method disclosure.
//!
//! One choice is DELIBERATE and stated: the density-or-power class {Psd,
//! Autopower, PowerSpectrum, Cpsd magnitude} takes CONSERVATIVE REBIN as one
//! rule; everything amplitude-like takes point interpolation in the space
//! matching its definition.

use eigen_data::{units, Abscissa, FunctionKind, FunctionRecord, GridSpacing, Ordinate};
use eigen_dsp::{db, regrid};
use serde::{Deserialize, Serialize};

/// Which rung of the hierarchy produced the comparison (recompute > rebin >
/// interpolate) - ALWAYS disclosed (the anti-opaque-chain differentiator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegridMethod {
    /// Grids already identical - no resampling at all.
    ExactGrids,
    /// Integral-preserving conservative rebin (density class).
    ConservativeRebin,
    /// Point interpolation (log-log, linear, or complex mag/phase).
    Interpolation,
    /// Downgraded from rebin because a grid is irregular (disclosed).
    InterpolationIrregularDowngrade,
    /// Reference profile evaluated analytically onto the partner grid.
    ProfileEvaluation,
}

impl RegridMethod {
    pub fn label(self) -> &'static str {
        match self {
            RegridMethod::ExactGrids => "exact grids (no resampling)",
            RegridMethod::ConservativeRebin => "conservative rebin (integral-preserving)",
            RegridMethod::Interpolation => "point interpolation",
            RegridMethod::InterpolationIrregularDowngrade => {
                "point interpolation (rebin refused: irregular grid)"
            }
            RegridMethod::ProfileEvaluation => "profile breakpoint evaluation",
        }
    }
}

/// Verdict against a reference/tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOutcome {
    Pass,
    Fail,
    /// No tolerance was in play - overlay only.
    NoVerdict,
}

/// The nine-field compare report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareResult {
    /// 1. Verdict.
    pub outcome: CompareOutcome,
    /// 2. Worst margin in dB (negative = exceedance) against the tolerance,
    ///    None when NoVerdict.
    pub margin_db: Option<f64>,
    /// 3. Frequency (abscissa value) of the worst line.
    pub worst_at: Option<f64>,
    /// 4. Method rung actually used.
    pub method: RegridMethod,
    /// 5. Grid disclosure ("2.5 Hz vs 4.0 Hz onto 512-line intersection").
    pub grid_note: String,
    /// 6. Warnings (unit conversions, phase-unwrap flags, coherence averages).
    pub warnings: Vec<String>,
    /// 7. Contiguous valid-region count (span intersection can fragment).
    pub regions: usize,
    /// 8. Resolution-bias note when df differs materially (coarse grid lost
    ///    information no interpolation can recover).
    pub resolution_note: Option<String>,
    /// 9. Extrapolation mask summary: bins excluded as outside-span.
    pub excluded_bins: usize,
    /// The shared comparison grid.
    pub grid: Vec<f64>,
    /// Aligned operand values on `grid` (NaN where masked).
    pub a: Vec<f64>,
    pub b: Vec<f64>,
    /// Error trace in dB (a vs b), NaN where either is masked/non-positive.
    pub err_db: Vec<f64>,
}

fn kinds_comparable(a: FunctionKind, b: FunctionKind) -> Result<(), String> {
    use FunctionKind::*;
    if a == Correlation || b == Correlation {
        return Err(
            "correlations overlay by eye; compare verdicts are for spectra and profiles".into(),
        );
    }
    if a == b && a != Unknown {
        return Ok(());
    }
    let profile_ok = matches!((a, b), (Profile, _) | (_, Profile));
    if profile_ok && a != Unknown && b != Unknown {
        return Ok(());
    }
    if a == Unknown || b == Unknown {
        return Err("kind is Unknown - assign a kind in the inspector before comparing".into());
    }
    Err(format!(
        "kinds differ ({} vs {}) - Insight refuses cross-kind comparison",
        a.label(),
        b.label()
    ))
}

/// Convert `b`'s ordinate into `a`'s unit when quantities match
/// (convert-then-proceed; different quantities refuse).
fn reconcile_units(
    a: &FunctionRecord,
    b: &FunctionRecord,
    warnings: &mut Vec<String>,
) -> Result<Vec<f64>, String> {
    let Ordinate::Real(bv) = &b.ordinate else {
        return Err("complex operands go through the FRF path".into());
    };
    if a.unit_ordinate.label == b.unit_ordinate.label {
        return Ok(bv.clone());
    }
    // Probe convertibility and get the amplitude scale factor.
    let (scale, offset) = {
        let mut probe = [0.0f64, 1.0];
        units::convert_slice(&mut probe, &b.unit_ordinate, &a.unit_ordinate)
            .map_err(|e| format!("unit mismatch: {e}"))?
    };
    if offset != 0.0 {
        return Err("offset units cannot convert on squared quantities".into());
    }
    let factor = if a.kind.rebin_class() {
        // A DENSITY carries the squared unit (g^2/Hz): amplitude factor squared.
        scale * scale
    } else {
        scale
    };
    let v: Vec<f64> = bv.iter().map(|x| x * factor).collect();
    warnings.push(format!(
        "'{}' converted from {} to {} for comparison{}",
        b.name,
        b.unit_ordinate.label,
        a.unit_ordinate.label,
        if a.kind.rebin_class() {
            " (density: amplitude factor squared)"
        } else {
            ""
        }
    ));
    Ok(v)
}

fn grids_equal(a: &Abscissa, b: &Abscissa) -> bool {
    let (xa, xb) = (a.values(), b.values());
    xa.len() == xb.len()
        && xa
            .iter()
            .zip(&xb)
            .all(|(p, q)| (p - q).abs() <= p.abs().max(1e-12) * 1e-9)
}

fn count_regions(mask: &[bool]) -> usize {
    let mut n = 0usize;
    let mut inside = false;
    for &m in mask {
        if m && !inside {
            n += 1;
        }
        inside = m;
    }
    n
}

/// Overlay two real functions of the same (or profile-compatible) kind onto a
/// shared grid, per the dispatch table. `a` is the primary (its grid and unit
/// win when a choice exists); tolerance checking is `check_against_tolerance`.
pub fn align_pair(a: &FunctionRecord, b: &FunctionRecord) -> Result<CompareResult, String> {
    use FunctionKind::*;
    kinds_comparable(a.kind, b.kind)?;
    let mut warnings = Vec::new();

    // Profile partner: evaluate breakpoints onto the measured grid.
    if b.kind == Profile || a.kind == Profile {
        let (meas, prof, swapped) = if b.kind == Profile {
            (a, b, false)
        } else {
            (b, a, true)
        };
        let r = align_profile(meas, prof, &mut warnings, swapped)?;
        return Ok(r);
    }

    if a.ordinate.is_complex() || b.ordinate.is_complex() {
        return align_complex(a, b, warnings);
    }

    let bv = reconcile_units(a, b, &mut warnings)?;
    let Ordinate::Real(av) = &a.ordinate else {
        unreachable!()
    };
    let (xa, xb) = (a.abscissa.values(), b.abscissa.values());

    // Same-fs / overlapping-span law for time histories.
    if a.kind == TimeHistory {
        let (sa, sb) = (a.abscissa.spacing(), b.abscissa.spacing());
        if sa != GridSpacing::Linear || sb != GridSpacing::Linear {
            return Err("time histories must be evenly sampled".into());
        }
        let (da, db_) = (xa[1] - xa[0], xb[1] - xb[0]);
        if ((da - db_) / da).abs() > 1e-9 {
            return Err(format!(
                "sample rates differ ({:.6} vs {:.6} s) - resample one first (Transform -> Resample), then compare",
                da, db_
            ));
        }
    }

    let span = regrid::span_intersection((xa[0], xa[xa.len() - 1]), (xb[0], xb[xb.len() - 1]))
        .ok_or("no overlapping span - nothing to compare")?;

    // Destination grid = a's lines inside the intersection.
    let grid: Vec<f64> = xa
        .iter()
        .copied()
        .filter(|x| *x >= span.0 && *x <= span.1)
        .collect();
    if grid.len() < 2 {
        return Err("fewer than 2 shared lines in the overlapping span".into());
    }
    let a_on: Vec<f64> = xa
        .iter()
        .zip(av)
        .filter(|(x, _)| **x >= span.0 && **x <= span.1)
        .map(|(_, y)| *y)
        .collect();

    let exact = grids_equal(&a.abscissa, &b.abscissa);
    let (b_on, mask, method) = if exact {
        (bv.clone(), vec![true; grid.len()], RegridMethod::ExactGrids)
    } else if a.kind.rebin_class() {
        // Conservative rebin of b onto a's bins.
        let b_spacing = b.abscissa.spacing();
        match (b.abscissa.edges(), dst_edges_for(&a.abscissa, &grid)) {
            (Some(src_edges), Some(dst_edges)) if b_spacing != GridSpacing::Irregular => {
                let r = regrid::rebin_conservative(&xb, &bv, &src_edges, &dst_edges);
                (r.y, r.mask, RegridMethod::ConservativeRebin)
            }
            _ => {
                warnings.push(
                    "conservative rebin refused (irregular grid) - using point interpolation; band totals may shift"
                        .into(),
                );
                let r = regrid::interp_loglog(&xb, &bv, &grid);
                (r.y, r.mask, RegridMethod::InterpolationIrregularDowngrade)
            }
        }
    } else if a.kind == Coherence {
        let r = regrid::interp_clamped01(&xb, &bv, &grid);
        (r.y, r.mask, RegridMethod::Interpolation)
    } else if a.kind == TimeHistory {
        let r = regrid::interp_linear(&xb, &bv, &grid);
        (r.y, r.mask, RegridMethod::Interpolation)
    } else {
        // LinearSpectrum / Srs / BandSpectrum-alike: log-log point interp.
        if a.kind == Srs {
            if let (Some(qa), Some(qb)) = (srs_q_of(a), srs_q_of(b)) {
                if (qa - qb).abs() > 1e-6 {
                    return Err(format!("SRS Q differs ({qa} vs {qb}) - not comparable"));
                }
            }
        }
        if a.kind == BandSpectrum {
            return Err("band spectra compare only on identical band schemes".into());
        }
        let r = regrid::interp_loglog(&xb, &bv, &grid);
        (r.y, r.mask, RegridMethod::Interpolation)
    };

    // Resolution-bias note when df differs materially.
    let resolution_note = df_note(&xa, &xb);
    let excluded = mask.iter().filter(|m| !**m).count()
        + (xa.len() - grid.len())
        + xb.iter().filter(|x| **x < span.0 || **x > span.1).count();
    let err_db: Vec<f64> = a_on
        .iter()
        .zip(&b_on)
        .zip(&mask)
        .map(|((pa, pb), m)| {
            if !*m || !pa.is_finite() || !pb.is_finite() || *pa <= 0.0 || *pb <= 0.0 {
                f64::NAN
            } else if a.kind.rebin_class() {
                db::to_db_power(*pa, *pb)
            } else {
                db::to_db_amplitude(*pa, *pb)
            }
        })
        .collect();
    let grid_note = format!(
        "{} lines on the intersection {:.4}..{:.4} {}; {}",
        grid.len(),
        span.0,
        span.1,
        a.unit_abscissa.label,
        method.label()
    );
    Ok(CompareResult {
        outcome: CompareOutcome::NoVerdict,
        margin_db: None,
        worst_at: None,
        method,
        grid_note,
        warnings,
        regions: count_regions(&mask),
        resolution_note,
        excluded_bins: excluded,
        grid,
        a: a_on,
        b: b_on,
        err_db,
    })
}

fn dst_edges_for(_a: &Abscissa, grid: &[f64]) -> Option<Vec<f64>> {
    // Edges for the intersection sub-grid: the sub-grid inherits the parent's
    // uniform step or ratio, so the standard edge rule applies directly.
    // Irregular sub-grids return None and the caller downgrades.
    Abscissa::Explicit(grid.to_vec()).edges()
}

fn df_note(xa: &[f64], xb: &[f64]) -> Option<String> {
    if xa.len() < 2 || xb.len() < 2 {
        return None;
    }
    let da = (xa[xa.len() - 1] - xa[0]) / (xa.len() - 1) as f64;
    let db_ = (xb[xb.len() - 1] - xb[0]) / (xb.len() - 1) as f64;
    let ratio = (da / db_).max(db_ / da);
    (ratio > 1.5).then(|| {
        format!(
            "grids differ in resolution ({:.4} vs {:.4} mean spacing): the coarser measurement physically lost narrowband detail; no interpolation recovers it",
            da, db_
        )
    })
}

fn srs_q_of(r: &FunctionRecord) -> Option<f64> {
    // Q rides provenance params when Insight computed the SRS; imported SRS
    // without Q metadata compares with a warning-free pass (unknowable).
    if let eigen_data::Provenance::Derived {
        op, params_json, ..
    } = &r.provenance
    {
        if op == "srs" {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(params_json) {
                return v.get("q").and_then(|q| q.as_f64());
            }
        }
    }
    None
}

fn align_profile(
    meas: &FunctionRecord,
    prof: &FunctionRecord,
    warnings: &mut Vec<String>,
    swapped: bool,
) -> Result<CompareResult, String> {
    let Ordinate::Real(pv) = &prof.ordinate else {
        return Err("profile must be real-valued".into());
    };
    let px = prof.abscissa.values();
    if px.len() < 2 {
        return Err("profile needs at least 2 breakpoints".into());
    }
    let Ordinate::Real(mv) = &meas.ordinate else {
        return Err("cannot check a complex function against a profile".into());
    };
    let mx = meas.abscissa.values();
    let span = regrid::span_intersection((mx[0], mx[mx.len() - 1]), (px[0], px[px.len() - 1]))
        .ok_or("profile and measurement do not overlap")?;
    let grid: Vec<f64> = mx
        .iter()
        .copied()
        .filter(|x| *x >= span.0 && *x <= span.1)
        .collect();
    let m_on: Vec<f64> = mx
        .iter()
        .zip(mv)
        .filter(|(x, _)| **x >= span.0 && **x <= span.1)
        .map(|(_, y)| *y)
        .collect();
    // Breakpoint evaluation = log-log point interpolation on the breakpoints
    // (exactly how the spec defines the line between breakpoints).
    let p_on = regrid::interp_loglog(&px, pv, &grid);
    if swapped {
        warnings.push("profile given as primary operand; evaluated onto the measured grid".into());
    }
    let power = meas.kind.rebin_class();
    let err_db: Vec<f64> = m_on
        .iter()
        .zip(&p_on.y)
        .zip(&p_on.mask)
        .map(|((m, p), ok)| {
            if !*ok || *m <= 0.0 || *p <= 0.0 {
                f64::NAN
            } else if power {
                db::to_db_power(*m, *p)
            } else {
                db::to_db_amplitude(*m, *p)
            }
        })
        .collect();
    let excluded = p_on.mask.iter().filter(|m| !**m).count() + (mx.len() - grid.len());
    Ok(CompareResult {
        outcome: CompareOutcome::NoVerdict,
        margin_db: None,
        worst_at: None,
        method: RegridMethod::ProfileEvaluation,
        grid_note: format!(
            "profile evaluated at {} measured lines on {:.4}..{:.4} {}",
            grid.len(),
            span.0,
            span.1,
            meas.unit_abscissa.label
        ),
        warnings: warnings.clone(),
        regions: count_regions(&p_on.mask),
        resolution_note: None,
        excluded_bins: excluded,
        grid,
        a: m_on,
        b: p_on.y,
        err_db,
    })
}

fn align_complex(
    a: &FunctionRecord,
    b: &FunctionRecord,
    mut warnings: Vec<String>,
) -> Result<CompareResult, String> {
    let (Ordinate::Complex(_), Ordinate::Complex(bv)) = (&a.ordinate, &b.ordinate) else {
        return Err("FRF comparison needs both operands complex".into());
    };
    let (xa, xb) = (a.abscissa.values(), b.abscissa.values());
    let span = regrid::span_intersection((xa[0], xa[xa.len() - 1]), (xb[0], xb[xb.len() - 1]))
        .ok_or("no overlapping span")?;
    let grid: Vec<f64> = xa
        .iter()
        .copied()
        .filter(|x| *x >= span.0 && *x <= span.1)
        .collect();
    let r = regrid::interp_complex(&xb, bv, &grid);
    if !r.unwrap_flags.is_empty() {
        warnings.push(format!(
            "phase steps over 90 degrees at {} destination lines - the source grid undersamples the phase there",
            r.unwrap_flags.len()
        ));
    }
    let Ordinate::Complex(av) = &a.ordinate else {
        unreachable!()
    };
    let a_on: Vec<f64> = xa
        .iter()
        .zip(av)
        .filter(|(x, _)| **x >= span.0 && **x <= span.1)
        .map(|(_, (re, im))| (re * re + im * im).sqrt())
        .collect();
    let b_mag: Vec<f64> =
        r.y.iter()
            .map(|(re, im)| (re * re + im * im).sqrt())
            .collect();
    let err_db: Vec<f64> = a_on
        .iter()
        .zip(&b_mag)
        .zip(&r.mask)
        .map(|((m, p), ok)| {
            if !*ok || *m <= 0.0 || *p <= 0.0 {
                f64::NAN
            } else {
                db::to_db_amplitude(*m, *p)
            }
        })
        .collect();
    let excluded = r.mask.iter().filter(|m| !**m).count();
    Ok(CompareResult {
        outcome: CompareOutcome::NoVerdict,
        margin_db: None,
        worst_at: None,
        method: RegridMethod::Interpolation,
        grid_note: format!(
            "complex magnitude/phase interpolation, {} lines on {:.4}..{:.4} {}",
            grid.len(),
            span.0,
            span.1,
            a.unit_abscissa.label
        ),
        warnings,
        regions: count_regions(&r.mask),
        resolution_note: df_note(&xa, &xb),
        excluded_bins: excluded,
        grid,
        a: a_on,
        b: b_mag,
        err_db,
    })
}

/// Tolerance check: measured vs a hi (and optionally lo) limit trace, both
/// already aligned via `align_pair` against the SAME measured primary.
/// Margin = min over valid lines of (limit_hi - meas) and (meas - limit_lo)
/// in dB; negative margin = exceedance = Fail.
pub fn verdict_against(
    meas_vs_hi: &CompareResult,
    meas_vs_lo: Option<&CompareResult>,
) -> (CompareOutcome, Option<f64>, Option<f64>) {
    let mut worst: Option<(f64, f64)> = None; // (margin_db, at)
    for (i, e) in meas_vs_hi.err_db.iter().enumerate() {
        if e.is_nan() {
            continue;
        }
        // err_db = meas vs limit: positive means meas ABOVE hi-limit -> margin
        // is -err.
        let margin = -*e;
        if worst.map(|(m, _)| margin < m).unwrap_or(true) {
            worst = Some((margin, meas_vs_hi.grid[i]));
        }
    }
    if let Some(lo) = meas_vs_lo {
        for (i, e) in lo.err_db.iter().enumerate() {
            if e.is_nan() {
                continue;
            }
            // meas vs lo-limit: negative err means meas BELOW lo -> margin = err.
            let margin = *e;
            if worst.map(|(m, _)| margin < m).unwrap_or(true) {
                worst = Some((margin, lo.grid[i]));
            }
        }
    }
    match worst {
        None => (CompareOutcome::NoVerdict, None, None),
        Some((m, at)) => (
            if m >= 0.0 {
                CompareOutcome::Pass
            } else {
                CompareOutcome::Fail
            },
            Some(m),
            Some(at),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::mk;
    use eigen_data::{Abscissa, FunctionKind};

    fn even(start: f64, step: f64, n: usize) -> Abscissa {
        Abscissa::Even { start, step, n }
    }

    #[test]
    fn cross_kind_refuses_and_unknown_refuses() {
        let a = mk(FunctionKind::Psd, even(0.0, 2.0, 8), vec![1.0; 8], "a");
        let b = mk(
            FunctionKind::LinearSpectrum,
            even(0.0, 2.0, 8),
            vec![1.0; 8],
            "b",
        );
        assert!(align_pair(&a, &b).unwrap_err().contains("kinds differ"));
        let u = mk(FunctionKind::Unknown, even(0.0, 2.0, 8), vec![1.0; 8], "u");
        let p = mk(FunctionKind::Psd, even(0.0, 2.0, 8), vec![1.0; 8], "p");
        assert!(align_pair(&p, &u).unwrap_err().contains("assign a kind"));
    }

    #[test]
    fn psd_different_df_takes_conservative_rebin_and_preserves_grms() {
        // Flat PSD at two different df on the same span: rebin onto the finer
        // grid must reproduce the same band level (integral preserved).
        let a = mk(
            FunctionKind::Psd,
            even(0.0, 2.0, 251),
            vec![0.04; 251],
            "fine",
        );
        let b = mk(
            FunctionKind::Psd,
            even(0.0, 10.0, 51),
            vec![0.04; 51],
            "coarse",
        );
        let r = align_pair(&a, &b).unwrap();
        assert_eq!(r.method, RegridMethod::ConservativeRebin);
        for (i, v) in r.b.iter().enumerate() {
            if r.err_db[i].is_nan() {
                continue;
            }
            assert!((v - 0.04).abs() < 1e-12, "line {i}: {v}");
        }
        assert!(
            r.resolution_note.is_some(),
            "5x df difference must be disclosed"
        );
    }

    #[test]
    fn time_histories_refuse_cross_rate() {
        let a = mk(
            FunctionKind::TimeHistory,
            even(0.0, 1.0 / 2048.0, 64),
            vec![0.0; 64],
            "a",
        );
        let b = mk(
            FunctionKind::TimeHistory,
            even(0.0, 1.0 / 4096.0, 64),
            vec![0.0; 64],
            "b",
        );
        let e = align_pair(&a, &b).unwrap_err();
        assert!(
            e.contains("resampler is v1.1")
                || e.contains("Sample rates differ")
                || e.contains("sample rates differ")
        );
    }

    #[test]
    fn profile_check_verdicts_pass_and_fail() {
        // Measured flat 0.04; profile limit at 0.08 (hi) -> pass with ~3 dB.
        let meas = mk(
            FunctionKind::Psd,
            even(20.0, 5.0, 100),
            vec![0.04; 100],
            "meas",
        );
        let mut hi = mk(
            FunctionKind::Profile,
            Abscissa::Explicit(vec![20.0, 2000.0]),
            vec![0.08, 0.08],
            "limit-hi",
        );
        hi.role = Some("limit-hi".into());
        let r = align_pair(&meas, &hi).unwrap();
        assert_eq!(r.method, RegridMethod::ProfileEvaluation);
        let (out, margin, at) = verdict_against(&r, None);
        assert_eq!(out, CompareOutcome::Pass);
        assert!((margin.unwrap() - 3.0103).abs() < 0.01, "{margin:?}");
        assert!(at.is_some());
        // Now a measured spike above the limit -> Fail, worst line located.
        let mut v = vec![0.04; 100];
        v[40] = 0.2;
        let meas2 = mk(FunctionKind::Psd, even(20.0, 5.0, 100), v, "meas2");
        let r2 = align_pair(&meas2, &hi).unwrap();
        let (out2, margin2, at2) = verdict_against(&r2, None);
        assert_eq!(out2, CompareOutcome::Fail);
        assert!(margin2.unwrap() < 0.0);
        assert!((at2.unwrap() - (20.0 + 5.0 * 40.0)).abs() < 1e-9);
    }

    #[test]
    fn unit_conversion_squares_for_densities() {
        // a in g^2/Hz-style (label g), b labeled m/s^2: density converts by
        // the amplitude factor SQUARED.
        let a = mk(FunctionKind::Psd, even(10.0, 2.0, 4), vec![1.0; 4], "a");
        let mut b = mk(FunctionKind::Psd, even(10.0, 2.0, 4), vec![1.0; 4], "b");
        b.unit_ordinate = eigen_data::units::unit("m/s^2").unwrap();
        let r = align_pair(&a, &b).unwrap();
        let g0 = eigen_data::units::G0;
        // 1 (m/s^2)^2/Hz = 1/g0^2 g^2/Hz
        assert!((r.b[1] - 1.0 / (g0 * g0)).abs() < 1e-9, "{}", r.b[1]);
        assert!(r.warnings.iter().any(|w| w.contains("converted")));
    }

    #[test]
    fn frf_complex_alignment_flags_and_magnitude() {
        let n = 64;
        let xs: Vec<f64> = (1..=n).map(|i| 5.0 * i as f64).collect();
        let h: Vec<(f64, f64)> = xs.iter().map(|f| (1.0 / f, 0.0)).collect();
        let mut a = mk(
            FunctionKind::Frf,
            Abscissa::Explicit(xs.clone()),
            vec![],
            "a",
        );
        a.ordinate = Ordinate::Complex(h.clone());
        let xs2: Vec<f64> = (1..=(n / 2)).map(|i| 10.0 * i as f64).collect();
        let h2: Vec<(f64, f64)> = xs2.iter().map(|f| (1.0 / f, 0.0)).collect();
        let mut b = mk(FunctionKind::Frf, Abscissa::Explicit(xs2), vec![], "b");
        b.ordinate = Ordinate::Complex(h2);
        let r = align_pair(&a, &b).unwrap();
        // Identical plants on different grids: err ~ 0 dB where valid.
        for e in r.err_db.iter().filter(|e| !e.is_nan()) {
            assert!(e.abs() < 0.02, "{e}");
        }
    }

    #[test]
    fn exact_grids_short_circuit() {
        let a = mk(FunctionKind::Psd, even(0.0, 2.0, 16), vec![0.5; 16], "a");
        let b = mk(FunctionKind::Psd, even(0.0, 2.0, 16), vec![0.25; 16], "b");
        let r = align_pair(&a, &b).unwrap();
        assert_eq!(r.method, RegridMethod::ExactGrids);
        for e in r.err_db.iter() {
            assert!(
                (e - 3.0103).abs() < 0.001,
                "0.5 vs 0.25 power = +3 dB, got {e}"
            );
        }
    }
}
