//! Frequency-axis regridding - the kernel under Insight's overlay/compare
//! headline.
//!
//! Three tools, one law:
//! - POINT interpolation (linear or log-log) for amplitude-like functions.
//! - CONSERVATIVE REBIN (integral-preserving) for density/power functions.
//! - NEVER extrapolate: every output carries a validity mask; outside-span
//!   points are `NaN` + `false`, and verdicts must exclude them (policy layer).
//!
//! The within-segment model for rebinning is the log-log power law
//! `y = a * f^n` fitted between adjacent bin centers, with the analytic
//! integral and the guarded `n = -1` logarithmic branch - the most common
//! roll-off in vibration specs sits exactly there (-3.01 dB/octave), so the
//! guard is not theoretical.

use crate::fft;

/// Overlapping span of two closed intervals; `None` when disjoint.
pub fn span_intersection(a: (f64, f64), b: (f64, f64)) -> Option<(f64, f64)> {
    let lo = a.0.max(b.0);
    let hi = a.1.min(b.1);
    (hi > lo).then_some((lo, hi))
}

/// Result of a regrid: values on the destination axis + validity mask.
/// `mask[i] == false` means "outside the source span - excluded from any
/// verdict, rendered as a gap".
#[derive(Debug, Clone, PartialEq)]
pub struct Regridded {
    pub y: Vec<f64>,
    pub mask: Vec<bool>,
}

fn interp_core(x_src: &[f64], y_src: &[f64], x_dst: &[f64], log_x: bool, log_y: bool) -> Regridded {
    assert_eq!(x_src.len(), y_src.len(), "interp: src length mismatch");
    let n = x_dst.len();
    let mut y = vec![f64::NAN; n];
    let mut mask = vec![false; n];
    if x_src.len() < 2 {
        return Regridded { y, mask };
    }
    let (lo, hi) = (x_src[0], x_src[x_src.len() - 1]);
    let mut j = 0usize;
    for (i, &xd) in x_dst.iter().enumerate() {
        if xd < lo || xd > hi {
            continue; // never extrapolate
        }
        while j + 2 < x_src.len() && x_src[j + 1] < xd {
            j += 1;
        }
        let (x0, x1) = (x_src[j], x_src[j + 1]);
        let (y0, y1) = (y_src[j], y_src[j + 1]);
        let t = if log_x && x0 > 0.0 && x1 > 0.0 && xd > 0.0 {
            (xd / x0).ln() / (x1 / x0).ln()
        } else {
            (xd - x0) / (x1 - x0)
        };
        y[i] = if log_y && y0 > 0.0 && y1 > 0.0 {
            (y0.ln() + t * (y1 / y0).ln()).exp()
        } else {
            // Log-y undefined at or below zero: fall back to linear-in-y for
            // this segment (honest degradation, not a refusal - a spectrum
            // with a true zero has no log representation).
            y0 + t * (y1 - y0)
        };
        mask[i] = true;
    }
    Regridded { y, mask }
}

/// Linear-linear point interpolation.
pub fn interp_linear(x_src: &[f64], y_src: &[f64], x_dst: &[f64]) -> Regridded {
    interp_core(x_src, y_src, x_dst, false, false)
}

/// Log-log point interpolation - the rule for amplitude spectra and profiles
/// (matches how specs are DEFINED: straight lines on log-log paper; cannot go
/// negative). Falls back per-segment to linear-y when an endpoint is <= 0.
pub fn interp_loglog(x_src: &[f64], y_src: &[f64], x_dst: &[f64]) -> Regridded {
    interp_core(x_src, y_src, x_dst, true, true)
}

/// Linear interpolation clamped to [0, 1] - the coherence rule.
pub fn interp_clamped01(x_src: &[f64], y_src: &[f64], x_dst: &[f64]) -> Regridded {
    let mut r = interp_core(x_src, y_src, x_dst, false, false);
    for v in r.y.iter_mut() {
        if v.is_finite() {
            *v = v.clamp(0.0, 1.0);
        }
    }
    r
}

/// Integral of the power-law segment through `(f1, y1)` and `(f2, y2)`,
/// evaluated over `[lo, hi]` (which must lie within `[f1, f2]`, all > 0,
/// y > 0). Exponent `n = ln(y2/y1)/ln(f2/f1)`; `|n+1| < 1e-9` takes the
/// logarithmic branch. Degenerate inputs (non-positive f or y, or |n| absurd)
/// fall back to the trapezoid on the log-interpolated endpoints.
pub(crate) fn powerlaw_segment_integral(
    f1: f64,
    y1: f64,
    f2: f64,
    y2: f64,
    lo: f64,
    hi: f64,
) -> f64 {
    if hi <= lo {
        return 0.0;
    }
    let usable = f1 > 0.0 && f2 > 0.0 && y1 > 0.0 && y2 > 0.0 && f2 != f1;
    if usable {
        let n = (y2 / y1).ln() / (f2 / f1).ln();
        if n.is_finite() && n.abs() < 50.0 {
            let a = y1 / f1.powf(n);
            return if (n + 1.0).abs() < 1e-9 {
                a * (hi / lo).ln()
            } else {
                a * (hi.powf(n + 1.0) - lo.powf(n + 1.0)) / (n + 1.0)
            };
        }
    }
    // Fallback: linear segment integral (trapezoid) over [lo, hi].
    let t_lo = (lo - f1) / (f2 - f1);
    let t_hi = (hi - f1) / (f2 - f1);
    let ylo = y1 + t_lo * (y2 - y1);
    let yhi = y1 + t_hi * (y2 - y1);
    (ylo + yhi) / 2.0 * (hi - lo)
}

/// Integral of the piecewise source model over `[lo, hi]`, where the model is:
/// power-law segments between adjacent bin CENTERS, constant extension across
/// the outer half-bins (edges[0]..centers[0] and centers[last]..edges[last]).
/// `lo/hi` must lie within `[edges[0], edges[last]]`.
pub(crate) fn model_integral(centers: &[f64], y: &[f64], edges: &[f64], lo: f64, hi: f64) -> f64 {
    debug_assert_eq!(centers.len(), y.len());
    debug_assert_eq!(edges.len(), centers.len() + 1);
    if hi <= lo || centers.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    let first_c = centers[0];
    let last_c = centers[centers.len() - 1];
    // Leading constant half-bin.
    let a = lo.max(edges[0]);
    let b = hi.min(first_c);
    if b > a {
        total += y[0] * (b - a);
    }
    // Power-law interior segments.
    for k in 0..centers.len().saturating_sub(1) {
        let (f1, f2) = (centers[k], centers[k + 1]);
        let a = lo.max(f1);
        let b = hi.min(f2);
        if b > a {
            total += powerlaw_segment_integral(f1, y[k], f2, y[k + 1], a, b);
        }
    }
    // Trailing constant half-bin.
    let a = lo.max(last_c);
    let b = hi.min(edges[edges.len() - 1]);
    if b > a {
        total += y[y.len() - 1] * (b - a);
    }
    total
}

/// Conservative (integral-preserving) rebin of a DENSITY function.
///
/// `src_centers`/`src_y` with `src_edges` (len+1) describe the source;
/// `dst_edges` (len m+1) the destination bins. Output bin value = model
/// integral over the bin / bin width, so `sum(y_i * w_i)` is preserved
/// wherever the destination tiles the source (the I2 gate pins this at 1e-12).
/// Destination bins extending outside the source edge span are masked false.
pub fn rebin_conservative(
    src_centers: &[f64],
    src_y: &[f64],
    src_edges: &[f64],
    dst_edges: &[f64],
) -> Regridded {
    assert_eq!(src_centers.len(), src_y.len(), "rebin: src length mismatch");
    assert_eq!(
        src_edges.len(),
        src_centers.len() + 1,
        "rebin: src edges length"
    );
    let m = dst_edges.len().saturating_sub(1);
    let mut y = vec![f64::NAN; m];
    let mut mask = vec![false; m];
    if src_centers.is_empty() {
        return Regridded { y, mask };
    }
    let (span_lo, span_hi) = (src_edges[0], src_edges[src_edges.len() - 1]);
    for i in 0..m {
        let (lo, hi) = (dst_edges[i], dst_edges[i + 1]);
        if lo < span_lo - 1e-12 || hi > span_hi + 1e-12 || hi <= lo {
            continue;
        }
        let integral = model_integral(
            src_centers,
            src_y,
            src_edges,
            lo.max(span_lo),
            hi.min(span_hi),
        );
        y[i] = integral / (hi - lo);
        mask[i] = true;
    }
    Regridded { y, mask }
}

/// Unwrap a phase sequence (radians) in place; returns indices where the
/// WRAPPED step exceeded pi/2 (90 degrees) - the mandated disclosure flags
/// (a >90-degree-per-point phase step means the grid undersamples the phase
/// and interpolation is suspect).
pub fn phase_unwrap(phase: &mut [f64]) -> Vec<usize> {
    use std::f64::consts::PI;
    let mut flags = Vec::new();
    let mut offset = 0.0;
    for i in 1..phase.len() {
        let raw_prev = phase[i - 1] - offset;
        let raw = phase[i];
        let mut d = raw - raw_prev;
        // Wrap the step into (-pi, pi]; the offset absorbs the removed turns so
        // unwrapped[i] = unwrapped[i-1] + wrapped step.
        while d > PI {
            d -= 2.0 * PI;
            offset -= 2.0 * PI;
        }
        while d < -PI {
            d += 2.0 * PI;
            offset += 2.0 * PI;
        }
        if d.abs() > PI / 2.0 {
            flags.push(i);
        }
        phase[i] = raw + offset;
    }
    flags
}

/// Complex regrid result.
#[derive(Debug, Clone)]
pub struct RegriddedComplex {
    pub y: Vec<(f64, f64)>,
    pub mask: Vec<bool>,
    /// Destination indices whose source phase neighborhood carried a
    /// > 90-degree step (interpolation there is disclosed, not trusted).
    pub unwrap_flags: Vec<usize>,
}

/// Complex point interpolation via magnitude (log-log) + unwrapped phase
/// (linear in log-f) - the FRF rule.
pub fn interp_complex(x_src: &[f64], y_src: &[(f64, f64)], x_dst: &[f64]) -> RegriddedComplex {
    assert_eq!(
        x_src.len(),
        y_src.len(),
        "interp_complex: src length mismatch"
    );
    let mag: Vec<f64> = y_src.iter().map(|c| fft::mag(*c)).collect();
    let mut phase: Vec<f64> = y_src.iter().map(|(re, im)| im.atan2(*re)).collect();
    let src_flags = phase_unwrap(&mut phase);
    let m = interp_loglog(x_src, &mag, x_dst);
    let p = interp_core(x_src, &phase, x_dst, true, false);
    let mut y = vec![(f64::NAN, f64::NAN); x_dst.len()];
    let mut mask = vec![false; x_dst.len()];
    for i in 0..x_dst.len() {
        if m.mask[i] && p.mask[i] {
            y[i] = (m.y[i] * p.y[i].cos(), m.y[i] * p.y[i].sin());
            mask[i] = true;
        }
    }
    // Map source flag indices onto destination indices that fall inside the
    // flagged source segment.
    let mut unwrap_flags = Vec::new();
    for &si in &src_flags {
        let (f_lo, f_hi) = (x_src[si - 1], x_src[si]);
        for (di, &xd) in x_dst.iter().enumerate() {
            if xd >= f_lo && xd <= f_hi {
                unwrap_flags.push(di);
            }
        }
    }
    unwrap_flags.dedup();
    RegriddedComplex {
        y,
        mask,
        unwrap_flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin_edges(start: f64, step: f64, n: usize) -> (Vec<f64>, Vec<f64>) {
        let centers: Vec<f64> = (0..n).map(|i| start + step * i as f64).collect();
        let mut edges = Vec::with_capacity(n + 1);
        let first = centers[0] - step / 2.0;
        edges.push(if centers[0] == 0.0 { 0.0 } else { first });
        for w in centers.windows(2) {
            edges.push((w[0] + w[1]) / 2.0);
        }
        edges.push(centers[n - 1] + step / 2.0);
        (centers, edges)
    }

    #[test]
    fn never_extrapolates() {
        let x = [10.0, 20.0, 30.0];
        let y = [1.0, 2.0, 3.0];
        let r = interp_linear(&x, &y, &[5.0, 15.0, 35.0]);
        assert!(!r.mask[0] && r.y[0].is_nan());
        assert!(r.mask[1]);
        assert!(!r.mask[2] && r.y[2].is_nan());
    }

    #[test]
    fn loglog_is_exact_on_a_power_law() {
        // y = 3 f^-0.7 sampled coarsely; log-log interpolation must reproduce
        // the law exactly at intermediate points.
        let x: Vec<f64> = (0..8).map(|i| 20.0 * 2f64.powi(i)).collect();
        let y: Vec<f64> = x.iter().map(|f| 3.0 * f.powf(-0.7)).collect();
        let xd = [30.0, 100.0, 999.0];
        let r = interp_loglog(&x, &y, &xd);
        for (i, f) in xd.iter().enumerate() {
            assert!(r.mask[i]);
            assert!((r.y[i] - 3.0 * f.powf(-0.7)).abs() / r.y[i] < 1e-12);
        }
    }

    #[test]
    fn rebin_preserves_total_integral_linear_grid() {
        // The I2 gate, linear fixture: destination exactly tiles the source.
        let (c, e) = lin_edges(0.0, 2.5, 128);
        let y: Vec<f64> = c
            .iter()
            .map(|f| 0.04 + 1e-4 * f + (f / 80.0).sin().abs() * 0.01)
            .collect();
        let src_total: f64 = model_integral(&c, &y, &e, e[0], e[e.len() - 1]);
        // Coarser destination that EXACTLY tiles the source span (integral
        // preservation is only defined over the same support).
        let m = 32;
        let (lo, hi) = (e[0], e[e.len() - 1]);
        let dstep = (hi - lo) / m as f64;
        let de: Vec<f64> = (0..=m).map(|i| lo + dstep * i as f64).collect();
        let r = rebin_conservative(&c, &y, &e, &de);
        let dst_total: f64 = (0..m).map(|i| r.y[i] * (de[i + 1] - de[i])).sum();
        assert!(r.mask.iter().all(|&m| m));
        assert!(
            ((dst_total - src_total) / src_total).abs() < 1e-12,
            "integral drift: src {src_total} dst {dst_total}"
        );
    }

    #[test]
    fn rebin_preserves_total_integral_log_grid() {
        // The gate: LOG fixture through the geometric-edge path.
        let n = 48;
        let centers: Vec<f64> = (0..n).map(|i| 20.0 * 2f64.powf(i as f64 / 6.0)).collect();
        let r_ratio = (centers[1] / centers[0]).sqrt();
        let mut edges = Vec::with_capacity(n + 1);
        edges.push(centers[0] / r_ratio);
        for w in centers.windows(2) {
            edges.push((w[0] * w[1]).sqrt());
        }
        edges.push(centers[n - 1] * r_ratio);
        let y: Vec<f64> = centers.iter().map(|f| 0.9 * f.powf(-1.0)).collect(); // n = -1!
        let src_total = model_integral(&centers, &y, &edges, edges[0], edges[n]);
        // Destination: linear grid inside the log span.
        let lo = edges[0];
        let hi = edges[n];
        let m = 40;
        let dstep = (hi - lo) / m as f64;
        let de: Vec<f64> = (0..=m).map(|i| lo + dstep * i as f64).collect();
        let rb = rebin_conservative(&centers, &y, &edges, &de);
        let dst_total: f64 = (0..m).map(|i| rb.y[i] * dstep).sum();
        assert!(rb.mask.iter().all(|&x| x));
        assert!(((dst_total - src_total) / src_total).abs() < 1e-12);
        // And the n = -1 branch really ran: analytic check on one interior segment.
        let seg =
            powerlaw_segment_integral(centers[6], y[6], centers[7], y[7], centers[6], centers[7]);
        let analytic = 0.9 * (centers[7] / centers[6]).ln();
        assert!((seg - analytic).abs() / analytic < 1e-12);
    }

    #[test]
    fn rebin_masks_outside_source_span() {
        let (c, e) = lin_edges(10.0, 2.0, 16);
        let y = vec![1.0; 16];
        let de = vec![0.0, 5.0, 12.0, 20.0, 100.0];
        let r = rebin_conservative(&c, &y, &e, &de);
        assert!(!r.mask[0]); // 0-5 entirely below span start (9.0)
        assert!(!r.mask[1]); // 5-12 straddles the span edge -> refused
        assert!(r.mask[2]);
        assert!(!r.mask[3]); // extends past span end
    }

    #[test]
    fn phase_unwrap_removes_turns_and_flags_only_big_steps() {
        use std::f64::consts::PI;
        // A wrapped -1.4pi raw jump is REALLY a +0.6pi step: unwrap must undo
        // the turn (no residual > pi step) and flag it (> pi/2 = undersampled).
        let mut p = vec![
            0.0,
            0.3,
            0.3 + 0.6 * PI - 2.0 * PI,
            0.35 + 0.6 * PI - 2.0 * PI,
        ];
        let flags = phase_unwrap(&mut p);
        assert!(
            p.windows(2).all(|w| (w[1] - w[0]).abs() < PI),
            "no residual wrap: {p:?}"
        );
        assert!((p[2] - (0.3 + 0.6 * PI)).abs() < 1e-12, "turn restored");
        assert_eq!(flags, vec![2], "only the >90-degree step is flagged");
        // Small steps never flag.
        let mut q = vec![0.0, 0.2, 0.4, 0.6];
        assert!(phase_unwrap(&mut q).is_empty());
    }

    #[test]
    fn complex_interp_preserves_a_pure_delay_phase_slope() {
        // H = e^{-j w tau}: |H| = 1, phase linear in f. Interpolation on a log
        // grid must keep magnitude 1 and phase on the line.
        let tau = 1.5e-3;
        let x: Vec<f64> = (1..40).map(|i| 5.0 * i as f64).collect();
        let h: Vec<(f64, f64)> = x
            .iter()
            .map(|f| {
                let w = 2.0 * std::f64::consts::PI * f;
                ((w * tau).cos(), -(w * tau).sin())
            })
            .collect();
        let xd = [12.5, 77.5, 150.1];
        let r = interp_complex(&x, &h, &xd);
        for (i, f) in xd.iter().enumerate() {
            assert!(r.mask[i]);
            let mag = (r.y[i].0.powi(2) + r.y[i].1.powi(2)).sqrt();
            assert!((mag - 1.0).abs() < 1e-6, "unit magnitude");
            let want = -2.0 * std::f64::consts::PI * f * tau;
            let got = r.y[i].1.atan2(r.y[i].0);
            // Compare modulo 2pi. Tolerance: phase interpolates linearly in
            // LOG-f (the FRF rule) while a pure delay is linear in f, so a
            // curvature error appears across each segment, largest where the
            // grid is relatively coarsest (the low end: ~2.4e-3 rad at 12.5 Hz
            // on this 5 Hz grid). 5e-3 rad (~0.3 degrees) bounds the honest
            // worst case; tightening this means densifying the source grid,
            // not changing the rule.
            let d = (got - want).rem_euclid(2.0 * std::f64::consts::PI);
            let d = d.min(2.0 * std::f64::consts::PI - d);
            assert!(d < 5e-3, "phase on the delay line at {f} Hz (d = {d})");
        }
    }

    #[test]
    fn span_intersection_basics() {
        assert_eq!(
            span_intersection((0.0, 10.0), (5.0, 20.0)),
            Some((5.0, 10.0))
        );
        assert_eq!(span_intersection((0.0, 4.0), (5.0, 20.0)), None);
    }
}
