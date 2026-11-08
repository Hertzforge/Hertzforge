//! The per-line linear algebra of `hertzforge_sdm::linalg`, split over threads.
//!
//! The lines of a stack are independent, so every function here cuts the band's
//! lines into one contiguous chunk per thread (`std::thread::scope`, no new
//! dependency), runs the sequential `linalg` call on each chunk, and joins the
//! results. Lines outside the band are left zero: nothing the loop does reads
//! them. A transformation held on one line is applied to every line, as `linalg`
//! does.

use hertzforge_sdm::linalg::{self, LinalgError, Regularization, RepairReport};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::ops::Range;

/// Threads used for the per-line math: the machine's parallelism, at most 8.
pub fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
}

/// `[lo, hi)` cut into at most `parts` contiguous, non-empty ranges.
pub fn ranges(band: (usize, usize), parts: usize) -> Vec<Range<usize>> {
    let (lo, hi) = band;
    let n = hi.saturating_sub(lo);
    if n == 0 {
        return Vec::new();
    }
    let parts = parts.clamp(1, n);
    let base = n / parts;
    let extra = n % parts;
    let mut out = Vec::with_capacity(parts);
    let mut start = lo;
    for p in 0..parts {
        let len = base + usize::from(p < extra);
        out.push(start..start + len);
        start += len;
    }
    out
}

/// Run `f` on every range of the band, one thread each, results in band order.
pub fn par_map<T, F>(band: (usize, usize), f: F) -> Vec<T>
where
    T: Send,
    F: Fn(Range<usize>) -> T + Sync,
{
    let rs = ranges(band, threads());
    if rs.len() <= 1 {
        return rs.into_iter().map(&f).collect();
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = rs
            .into_iter()
            .map(|r| {
                let f = &f;
                s.spawn(move || f(r))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| match h.join() {
                Ok(v) => v,
                Err(p) => std::panic::resume_unwind(p),
            })
            .collect()
    })
}

/// Lines `r` of a Hermitian stack.
pub fn sub_herm(g: &HermitianStack, r: Range<usize>) -> HermitianStack {
    let per = HermitianStack::packed_len(g.n());
    let data = g.data()[r.start * per..r.end * per].to_vec();
    HermitianStack::from_packed(r.len(), g.n(), data).expect("a sub-range keeps the shape")
}

/// Lines `r` of a matrix stack; a one-line stack (a broadcast transformation) is
/// returned whole.
pub fn sub_mat(m: &MatrixStack, r: Range<usize>) -> MatrixStack {
    if m.lines() == 1 {
        return m.clone();
    }
    let per = m.rows() * m.cols();
    let data = m.data()[r.start * per..r.end * per].to_vec();
    MatrixStack::from_vec(r.len(), m.rows(), m.cols(), data).expect("a sub-range keeps the shape")
}

/// Join chunks of the band back into a stack of `lines` lines, zero outside.
pub fn join_herm(
    lines: usize,
    n: usize,
    band: (usize, usize),
    parts: Vec<HermitianStack>,
) -> HermitianStack {
    let per = HermitianStack::packed_len(n);
    let mut data = vec![C64::new(0.0, 0.0); lines * per];
    let mut at = band.0 * per;
    for p in parts {
        let d = p.data();
        data[at..at + d.len()].copy_from_slice(d);
        at += d.len();
    }
    HermitianStack::from_packed(lines, n, data).expect("the parts fill the band")
}

/// Join chunks of the band back into a matrix stack of `lines` lines.
pub fn join_mat(
    lines: usize,
    rows: usize,
    cols: usize,
    band: (usize, usize),
    parts: Vec<MatrixStack>,
) -> MatrixStack {
    let per = rows * cols;
    let mut data = vec![C64::new(0.0, 0.0); lines * per];
    let mut at = band.0 * per;
    for p in parts {
        let d = p.data();
        data[at..at + d.len()].copy_from_slice(d);
        at += d.len();
    }
    MatrixStack::from_vec(lines, rows, cols, data).expect("the parts fill the band")
}

fn collect<T>(parts: Vec<Result<T, LinalgError>>) -> Result<Vec<T>, LinalgError> {
    parts.into_iter().collect()
}

/// `T G T^H` on the band's lines.
pub fn congruence(
    t: &MatrixStack,
    g: &HermitianStack,
    band: (usize, usize),
) -> Result<HermitianStack, LinalgError> {
    let parts = collect(par_map(band, |r| {
        linalg::congruence(&sub_mat(t, r.clone()), &sub_herm(g, r))
    }))?;
    Ok(join_herm(g.lines(), t.rows(), band, parts))
}

/// `H G_xx H^H` on the band's lines.
pub fn response_from_drive(
    h: &MatrixStack,
    g: &HermitianStack,
    band: (usize, usize),
) -> Result<HermitianStack, LinalgError> {
    let parts = collect(par_map(band, |r| {
        linalg::response_from_drive(&sub_mat(h, r.clone()), &sub_herm(g, r))
    }))?;
    Ok(join_herm(g.lines(), h.rows(), band, parts))
}

/// `T_r H T_d` on the band's lines.
pub fn frf_transform(
    t_resp: &MatrixStack,
    h: &MatrixStack,
    t_drive: Option<&MatrixStack>,
    band: (usize, usize),
) -> Result<MatrixStack, LinalgError> {
    let cols = t_drive.map(|t| t.cols()).unwrap_or(h.cols());
    let parts = collect(par_map(band, |r| {
        let td = t_drive.map(|t| sub_mat(t, r.clone()));
        linalg::frf_transform(&sub_mat(t_resp, r.clone()), &sub_mat(h, r), td.as_ref())
    }))?;
    Ok(join_mat(h.lines(), t_resp.rows(), cols, band, parts))
}

/// The eigen square root on the band's lines.
pub fn sqrt_factor(g: &HermitianStack, band: (usize, usize)) -> Result<MatrixStack, LinalgError> {
    let parts = collect(par_map(band, |r| linalg::sqrt_factor(&sub_herm(g, r))))?;
    Ok(join_mat(g.lines(), g.n(), g.n(), band, parts))
}

/// H1, `G_yx G_xx^+`, on the band's lines.
pub fn h1_frf(
    g_yx: &MatrixStack,
    g_xx: &HermitianStack,
    reg: Regularization,
    band: (usize, usize),
) -> Result<MatrixStack, LinalgError> {
    let parts = collect(par_map(band, |r| {
        linalg::h1_frf(&sub_mat(g_yx, r.clone()), &sub_herm(g_xx, r), reg)
    }))?;
    Ok(join_mat(
        g_yx.lines(),
        g_yx.rows(),
        g_yx.cols(),
        band,
        parts,
    ))
}

/// Multiple coherence on the band's lines, line-major; 0 outside the band.
pub fn multiple_coherence(
    g_yy: &HermitianStack,
    g_yx: &MatrixStack,
    g_xx: &HermitianStack,
    reg: Regularization,
    band: (usize, usize),
) -> Result<Vec<f64>, LinalgError> {
    let n = g_yy.n();
    let parts = collect(par_map(band, |r| {
        linalg::multiple_coherence(
            &sub_herm(g_yy, r.clone()),
            &sub_mat(g_yx, r.clone()),
            &sub_herm(g_xx, r),
            reg,
        )
    }))?;
    let mut out = vec![0.0; g_yy.lines() * n];
    let mut at = band.0 * n;
    for p in parts {
        out[at..at + p.len()].copy_from_slice(&p);
        at += p.len();
    }
    Ok(out)
}

/// Condition numbers on the band's lines; NaN outside the band.
pub fn condition_numbers(h: &MatrixStack, band: (usize, usize)) -> Result<Vec<f64>, LinalgError> {
    let parts = collect(par_map(band, |r| linalg::condition_numbers(&sub_mat(h, r))))?;
    let mut out = vec![f64::NAN; h.lines()];
    let mut at = band.0;
    for p in parts {
        out[at..at + p.len()].copy_from_slice(&p);
        at += p.len();
    }
    Ok(out)
}

/// The singular values per band line, `min(rows, cols)` each, line-major over the
/// whole grid (zero outside the band).
pub fn singular_values(h: &MatrixStack, band: (usize, usize)) -> Result<Vec<f64>, LinalgError> {
    let k = h.rows().min(h.cols());
    let parts = collect(par_map(band, |r| linalg::svd(&sub_mat(h, r)).map(|s| s.s)))?;
    let mut out = vec![0.0; h.lines() * k];
    let mut at = band.0 * k;
    for p in parts {
        out[at..at + p.len()].copy_from_slice(&p);
        at += p.len();
    }
    Ok(out)
}

/// The regularized pseudo-inverse on the band's lines.
pub fn pinv(
    h: &MatrixStack,
    reg: Regularization,
    band: (usize, usize),
) -> Result<MatrixStack, LinalgError> {
    let parts = collect(par_map(band, |r| {
        linalg::pinv(&sub_mat(h, r), reg).map(|(p, _)| p)
    }))?;
    Ok(join_mat(h.lines(), h.cols(), h.rows(), band, parts))
}

/// `P G_ref P^H`, `P = H H^+`, on the band's lines.
pub fn projection_achievable(
    h: &MatrixStack,
    g_ref: &HermitianStack,
    reg: Regularization,
    band: (usize, usize),
) -> Result<HermitianStack, LinalgError> {
    let parts = collect(par_map(band, |r| {
        linalg::projection_achievable(&sub_mat(h, r.clone()), &sub_herm(g_ref, r), reg)
    }))?;
    Ok(join_herm(g_ref.lines(), g_ref.n(), band, parts))
}

/// Positive semi-definite repair of the band's lines, all or nothing: on a
/// rejection nothing is written. The report covers the band's lines, offset to the
/// grid (`clamped` has one entry per grid line).
pub fn psd_repair(
    g: &mut HermitianStack,
    clamp_rel: f64,
    reject_rel: f64,
    band: (usize, usize),
) -> Result<RepairReport, LinalgError> {
    let results = par_map(band, |r| {
        let mut part = sub_herm(g, r.clone());
        linalg::psd_repair(&mut part, clamp_rel, reject_rel).map(|rep| (r, part, rep))
    });
    let mut parts = Vec::with_capacity(results.len());
    for res in results {
        match res {
            Ok(p) => parts.push(p),
            Err(LinalgError::NegativeEigenvalue {
                line,
                eigenvalue,
                lambda_max,
                reject_rel,
            }) => {
                // Name the grid line, not the chunk's.
                let r0 = parts
                    .last()
                    .map(|(r, _, _): &(Range<usize>, HermitianStack, RepairReport)| r.end)
                    .unwrap_or(band.0);
                return Err(LinalgError::NegativeEigenvalue {
                    line: line + r0,
                    eigenvalue,
                    lambda_max,
                    reject_rel,
                });
            }
            Err(e) => return Err(e),
        }
    }
    let mut report = RepairReport {
        clamped: vec![0; g.lines()],
        worst_relative: 0.0,
        worst_line: None,
    };
    let n = g.n();
    let mut chunks = Vec::with_capacity(parts.len());
    for (r, part, rep) in parts {
        for (i, c) in rep.clamped.iter().enumerate() {
            report.clamped[r.start + i] = *c;
        }
        if rep.worst_relative < report.worst_relative {
            report.worst_relative = rep.worst_relative;
            report.worst_line = rep.worst_line.map(|l| l + r.start);
        }
        chunks.push(part);
    }
    let joined = join_herm(g.lines(), n, band, chunks);
    // Keep the out-of-band lines as they were (the caller zeroes them itself).
    let per = HermitianStack::packed_len(n);
    let mut data = g.data().to_vec();
    data[band.0 * per..band.1 * per].copy_from_slice(&joined.data()[band.0 * per..band.1 * per]);
    *g = HermitianStack::from_packed(g.lines(), n, data).expect("same shape");
    Ok(report)
}

/// The smallest numerical rank over the lines of `t` (singular values above
/// `1e-10` of the line's largest).
pub fn min_rank(t: &MatrixStack) -> Result<usize, LinalgError> {
    let s = linalg::svd(t)?;
    let k = s.k();
    let mut worst = usize::MAX;
    for line in 0..t.lines() {
        let sv = &s.s[line * k..(line + 1) * k];
        let smax = sv.first().copied().unwrap_or(0.0);
        let rank = sv.iter().filter(|&&x| x > 1e-10 * smax && x > 0.0).count();
        worst = worst.min(rank);
    }
    Ok(if worst == usize::MAX { 0 } else { worst })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_the_band_once_in_order() {
        let r = ranges((3, 20), 4);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].start, 3);
        assert_eq!(r[3].end, 20);
        for w in r.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert_eq!(ranges((5, 7), 8).len(), 2);
        assert!(ranges((5, 5), 8).is_empty());
    }

    #[test]
    fn split_math_matches_the_sequential_call_and_zeroes_outside_the_band() {
        let (lines, n) = (37, 3);
        let mut g = HermitianStack::zeros(lines, n);
        let mut t = MatrixStack::zeros(1, 2, n);
        for line in 0..lines {
            for i in 0..n {
                g.set(line, i, i, C64::new(1.0 + (line + i) as f64, 0.0));
                for j in i + 1..n {
                    g.set(
                        line,
                        i,
                        j,
                        C64::new(0.1 * j as f64, 0.05 * line as f64 / 37.0),
                    );
                }
            }
        }
        t.set(0, 0, 0, C64::new(1.0, 0.0));
        t.set(0, 1, 2, C64::new(0.5, -0.5));
        let band = (4, 33);
        let par = congruence(&t, &g, band).unwrap();
        let seq = linalg::congruence(&t, &g).unwrap();
        for line in 0..lines {
            for i in 0..2 {
                for j in 0..2 {
                    let want = if line >= band.0 && line < band.1 {
                        seq.get(line, i, j)
                    } else {
                        C64::new(0.0, 0.0)
                    };
                    assert!((par.get(line, i, j) - want).norm() < 1e-12);
                }
            }
        }
    }
}
