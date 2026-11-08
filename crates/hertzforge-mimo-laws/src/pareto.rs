//! The minimum-drive target: on each line, the drive spectral density matrix
//! that trades the autospectrum residuals against the drive power, over the
//! positive semi-definite cone, and the plant's predicted response to it.
//!
//! Sources: K. Coletti, R. B. Davis and R. Schultz, "Pareto-optimal target
//! definition for multi-axis random vibration testing", journal preprint,
//! 2026 (OSTI 3023856): the problem, its special cases and
//! the barrier method; D. O. Smallwood, "Multiple-Input Multiple-Output (MIMO)
//! Linear Systems Extreme Inputs/Outputs", Shock and Vibration 14 (2007), and
//! "Minimum Input Trace for Multiple Input Multiple Output Linear Systems",
//! Journal of the IEST 56 (2013): the minimum input trace, which is the
//! exact-control end of the trade-off. The dual form and the damped Newton
//! method below are this implementation's.
//!
//! # The problem
//!
//! Per line, with `H` the plant (controls by drives), `y` the specified
//! autospectra and `w` their weights, the drive `X` minimizes
//!
//! ```text
//!   sum_k w_k | y_k - (H X H^H)_kk |  +  lambda tr X      over X >= 0
//! ```
//!
//! (Coletti, Davis and Schultz, Eq. 2, with the residual weights added here).
//! `lambda` prices a unit of drive power in residual units: large values buy
//! lower drive with larger residuals, small ones reproduce the autospectra
//! where the plant can, and as `lambda` goes to zero the solution is Smallwood's
//! minimum input trace among the drives that reproduce them exactly (Eq. 4).
//! The target the closed loop then holds is the response the plant predicts
//! for that drive, `H X H^H` (their Eq. 9 for the under-determined case, and
//! the response any feasible target of their Eq. 5 produces in the others).
//!
//! # The dual, and why it is solved instead
//!
//! The dual of the problem above is
//!
//! ```text
//!   maximize  sum_k nu_k y_k   over nu in R^nc
//!   subject to  S(nu) = lambda I - H^H diag(nu) H  >= 0,   |nu_k| <= w_k
//! ```
//!
//! `nc` real variables with a `nd` by `nd` semi-definite constraint and a box,
//! against `nd^2` real variables in the primal. The barrier form adds
//! `mu [log det S + sum_k log(w_k - nu_k) + sum_k log(w_k + nu_k)]`, and at its
//! optimum the primal iterate `X = mu S^-1` is positive definite and has
//! residuals `y_k - (H X H^H)_kk = 2 mu nu_k / (w_k^2 - nu_k^2)`, which vanish
//! as `mu` does unless the bound is active: the usual pairing of an l1 term
//! with its box. Each Newton step costs one Cholesky factorization of `S`, a
//! triangular solve against `H^H`, the Gram matrix `M = H S^-1 H^H` and one
//! `nc` by `nc` real solve: the Hessian of the barrier in `nu` is
//! `-mu |M_kl|^2 - delta_kl mu [1/(w_k - nu_k)^2 + 1/(w_k + nu_k)^2]`.
//!
//! The Newton method is the damped one of self-concordant barriers (step
//! `delta / (1 + decrement)`), which stays inside the domain without a line
//! search. So the solve, from its inputs to the drive, uses only arithmetic
//! and square roots: no logarithm, exponential or library decomposition
//! decides a step, and the native law and the law file, which compile the
//! same code for two targets, take the same steps and reach the same drive.
//! An iteration converged to a tolerance would otherwise differ between the
//! two by about that tolerance whenever one of its inputs differed in the last
//! bit, which is what the parity test caught in a first version that scaled
//! the trade-off by a library pseudo-inverse.

use crate::support::{over_line_chunks, InverseRule, Weighting};
use hertzforge_sdm::law::{LawError, Tolerances};
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// How a line's residuals are weighed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResidualWeight {
    /// Each control's residual in its own units (the source's form): loud
    /// controls count for more.
    Absolute,
    /// Each control's residual relative to its specification: a control is
    /// judged by the fraction of its autospectrum it misses by.
    Relative,
}

/// The trade-off grid of the automatic choice, relative: 1e-6 to 1e6.
pub(crate) const GRID: [f64; 13] = [
    1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6,
];

/// At most this many in-band lines decide the automatic trade-off.
const SAMPLE_LINES: usize = 64;
/// Stage after stage the barrier weight falls by this factor.
const MU_STEP: f64 = 8.0;
/// Stages end when the duality gap is below this fraction of the objective
/// at the current point (the weighted residual plus the trade-off times the
/// drive power). Relative to the objective, not to the response: at a small
/// trade-off the objective is the trade-off term alone, and a gap relative to
/// the response would leave the drive power unresolved, a spread drive in
/// place of the minimum one.
const GAP_REL: f64 = 1e-7;
/// A stage's Newton method ends at this squared Newton decrement (of the
/// barrier function, which is scale-free).
const DECREMENT2: f64 = 1e-12;
/// Newton steps per stage at most.
const STEPS: usize = 80;
/// Stages at most.
const STAGES: usize = 40;
/// Eigenvalues of a line's drive below this fraction of its largest are set
/// to zero, so the target has exactly the rank the solution has. The
/// minimum-drive solution tends to a fully coherent drive, and the barrier
/// leaves its other directions at a small, arbitrary size. The closed loop
/// factors the target, and its rule for a rank-deficient one is Smallwood's:
/// a zero pivot leaves that source's column zero and only the sources the
/// target has are corrected. Directions of a small, arbitrary size defeat
/// that rule: they become sources whose corrections are noise, clamped every
/// loop (measured on a 4 by 4 plant: 50 to 110 clamped diagonals a loop and
/// the error wandering between 0.4 and 3.6 dB), and their small pivots turn
/// last-bit differences between the native law and the law file into visible
/// ones (the factor's sensitivity is the target's condition number times the
/// rounding unit; at a cutoff of 1e-4 the two were 2e-12 apart on a 6 by 4
/// plant, at 1e-3 within the 1e-12 bar). What the cutoff drops is at most this
/// fraction of the drive power, 30 dB below the drive's main direction.
const RANK_CUTOFF_REL: f64 = 1e-3;

/// One line's solution.
#[derive(Clone, Debug)]
pub(crate) struct LineSolution {
    /// The drive, `nd` by `nd`, row-major, Hermitian.
    pub x: Vec<C64>,
    /// `tr X`.
    pub trace: f64,
    /// `sum_k w_k |y_k - (H X H^H)_kk|`.
    pub residual: f64,
    /// `(H X H^H)_kk`, the predicted autospectra.
    pub predicted: Vec<f64>,
}

/// Cholesky `S = L L^H` of a Hermitian matrix given row-major in `s` (`n` by
/// `n`), into `l` (lower, row-major, the rest zero). `None` when `S` is not
/// positive definite.
fn cholesky(s: &[C64], n: usize, l: &mut [C64]) -> Option<()> {
    l.fill(C64::new(0.0, 0.0));
    for j in 0..n {
        let mut d = s[j * n + j].re;
        for k in 0..j {
            d -= l[j * n + k].norm_sqr();
        }
        if d <= 0.0 || !d.is_finite() {
            return None;
        }
        let ljj = d.sqrt();
        l[j * n + j] = C64::new(ljj, 0.0);
        for i in (j + 1)..n {
            let mut v = s[i * n + j];
            for k in 0..j {
                v -= l[i * n + k] * l[j * n + k].conj();
            }
            l[i * n + j] = v / ljj;
        }
    }
    Some(())
}

/// Real Cholesky of a symmetric positive definite `n` by `n` matrix in place
/// (lower triangle), then the solve `A x = b` in place of `b`. `false` when
/// `A` is not positive definite.
fn solve_spd(a: &mut [f64], n: usize, b: &mut [f64]) -> bool {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 || !d.is_finite() {
            return false;
        }
        let ljj = d.sqrt();
        a[j * n + j] = ljj;
        for i in (j + 1)..n {
            let mut v = a[i * n + j];
            for k in 0..j {
                v -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = v / ljj;
        }
    }
    // L y = b
    for i in 0..n {
        let mut v = b[i];
        for k in 0..i {
            v -= a[i * n + k] * b[k];
        }
        b[i] = v / a[i * n + i];
    }
    // L^T x = y
    for i in (0..n).rev() {
        let mut v = b[i];
        for k in (i + 1)..n {
            v -= a[k * n + i] * b[k];
        }
        b[i] = v / a[i * n + i];
    }
    true
}

/// The workspace of one line's solve.
struct Work {
    nc: usize,
    nd: usize,
    /// `S`, `nd` by `nd`.
    s: Vec<C64>,
    /// Its Cholesky factor.
    l: Vec<C64>,
    /// `B = L^-1 H^H`, `nd` by `nc`.
    b: Vec<C64>,
    /// `M = B^H B`, `nc` by `nc`.
    m: Vec<C64>,
    /// The negated Hessian, `nc` by `nc`, real.
    hess: Vec<f64>,
    /// The gradient, then the Newton direction.
    g: Vec<f64>,
}

impl Work {
    fn new(nc: usize, nd: usize) -> Self {
        Work {
            nc,
            nd,
            s: vec![C64::new(0.0, 0.0); nd * nd],
            l: vec![C64::new(0.0, 0.0); nd * nd],
            b: vec![C64::new(0.0, 0.0); nd * nc],
            m: vec![C64::new(0.0, 0.0); nc * nc],
            hess: vec![0.0; nc * nc],
            g: vec![0.0; nc],
        }
    }

    /// `S(nu) = lambda I - H^H diag(nu) H`, its factor, `B` and `M`. `false`
    /// when `S` is not positive definite (the point is outside the domain).
    fn factor(&mut self, h: &[C64], nu: &[f64], lambda: f64) -> bool {
        let (nc, nd) = (self.nc, self.nd);
        for i in 0..nd {
            for j in 0..nd {
                let mut v = C64::new(0.0, 0.0);
                for k in 0..nc {
                    v -= h[k * nd + i].conj() * nu[k] * h[k * nd + j];
                }
                if i == j {
                    v += lambda;
                }
                self.s[i * nd + j] = v;
            }
        }
        if cholesky(&self.s, nd, &mut self.l).is_none() {
            return false;
        }
        // B = L^-1 H^H: column k of B solves L b = h_k^H.
        for k in 0..nc {
            for i in 0..nd {
                let mut v = h[k * nd + i].conj();
                for j in 0..i {
                    v -= self.l[i * nd + j] * self.b[j * nc + k];
                }
                self.b[i * nc + k] = v / self.l[i * nd + i].re;
            }
        }
        // M = B^H B.
        for k in 0..nc {
            for q in k..nc {
                let mut v = C64::new(0.0, 0.0);
                for i in 0..nd {
                    v += self.b[i * nc + k].conj() * self.b[i * nc + q];
                }
                self.m[k * nc + q] = v;
                self.m[q * nc + k] = v.conj();
            }
        }
        true
    }

    /// The Newton direction of the barrier function at `nu` for barrier
    /// weight `mu`, into `g`; returns the squared Newton decrement, or `None`
    /// when the Hessian is not positive definite.
    #[allow(clippy::needless_range_loop)]
    fn newton(&mut self, y: &[f64], w: &[f64], nu: &[f64], mu: f64) -> Option<f64> {
        let nc = self.nc;
        for k in 0..nc {
            let (lo, hi) = (w[k] - nu[k], w[k] + nu[k]);
            self.g[k] = y[k] - mu * self.m[k * nc + k].re - mu / lo + mu / hi;
            for q in 0..nc {
                self.hess[k * nc + q] = mu * self.m[k * nc + q].norm_sqr();
            }
            self.hess[k * nc + k] += mu * (1.0 / (lo * lo) + 1.0 / (hi * hi));
        }
        let g0 = self.g.clone();
        if !solve_spd(&mut self.hess, nc, &mut self.g) {
            return None;
        }
        let mut dec2 = 0.0;
        for k in 0..nc {
            dec2 += g0[k] * self.g[k];
        }
        Some(dec2.max(0.0))
    }

    /// `tr S^-1` from the current factor: the squared norm of `L^-1`.
    #[allow(clippy::needless_range_loop)]
    fn inverse_trace(&self) -> f64 {
        let nd = self.nd;
        let mut total = 0.0;
        // Column j of L^-1 solves L c = e_j; its squared norm is summed.
        let mut c = vec![C64::new(0.0, 0.0); nd];
        for j in 0..nd {
            for i in j..nd {
                let mut v = if i == j {
                    C64::new(1.0, 0.0)
                } else {
                    C64::new(0.0, 0.0)
                };
                for k in j..i {
                    v -= self.l[i * nd + k] * c[k];
                }
                c[i] = v / self.l[i * nd + i].re;
                total += c[i].norm_sqr();
            }
        }
        total
    }

    /// The primal iterate `X = mu S^-1` from the current factor, row-major.
    fn primal(&self, mu: f64, x: &mut [C64]) {
        let nd = self.nd;
        // S^-1 = L^-H L^-1: column j of L^-1 solves L c = e_j.
        let mut inv = vec![C64::new(0.0, 0.0); nd * nd];
        for j in 0..nd {
            for i in j..nd {
                let mut v = if i == j {
                    C64::new(1.0, 0.0)
                } else {
                    C64::new(0.0, 0.0)
                };
                for k in j..i {
                    v -= self.l[i * nd + k] * inv[k * nd + j];
                }
                inv[i * nd + j] = v / self.l[i * nd + i].re;
            }
        }
        for i in 0..nd {
            for j in 0..nd {
                let mut v = C64::new(0.0, 0.0);
                for k in i.max(j)..nd {
                    v += inv[k * nd + i].conj() * inv[k * nd + j];
                }
                x[i * nd + j] = v * mu;
            }
        }
        for i in 0..nd {
            x[i * nd + i] = C64::new(x[i * nd + i].re, 0.0);
        }
    }
}

/// One line: the drive minimizing the weighted residual plus `lambda` times
/// the drive power. `h` is `nc` by `nd` row-major, `y` the specified
/// autospectra, `w` their residual weights (positive), `lambda` the absolute
/// trade-off (positive). `warm` is a dual point to start from.
pub(crate) fn solve_line(
    h: &[C64],
    nc: usize,
    nd: usize,
    y: &[f64],
    w: &[f64],
    lambda: f64,
    warm: Option<&[f64]>,
) -> Result<(LineSolution, Vec<f64>), LawError> {
    if lambda <= 0.0 || !lambda.is_finite() {
        return Err(LawError(format!(
            "the trade-off must be positive, got {lambda}"
        )));
    }
    let mut work = Work::new(nc, nd);
    let r0: f64 = (0..nc).map(|k| w[k] * y[k]).sum();
    let degree = (nd + 2 * nc) as f64;
    let mut nu = vec![0.0; nc];
    if let Some(wm) = warm {
        for k in 0..nc {
            nu[k] = wm[k].clamp(-0.9 * w[k], 0.9 * w[k]);
        }
        if !work.factor(h, &nu, lambda) {
            nu.iter_mut().for_each(|v| *v = 0.0);
        }
    }
    if !work.factor(h, &nu, lambda) {
        return Err(LawError(
            "the trade-off problem has no interior point".into(),
        ));
    }
    let mut mu = (r0 / degree).max(lambda * 1e-12).max(f64::MIN_POSITIVE);
    let mut trial = vec![0.0; nc];
    for _stage in 0..STAGES {
        for _step in 0..STEPS {
            let dec2 = match work.newton(y, w, &nu, mu) {
                Some(d) => d,
                None => break,
            };
            // The decrement of a self-concordant function is scale-free:
            // `dec2 / mu` is its square.
            if dec2 <= DECREMENT2 * mu {
                break;
            }
            // The damped step of a self-concordant barrier, in the units of
            // the barrier itself (the objective over mu).
            let dec = (dec2 / mu).sqrt();
            let t = 1.0 / (1.0 + dec);
            let mut ok = false;
            let mut tt = t;
            for _ in 0..60 {
                for k in 0..nc {
                    trial[k] = nu[k] + tt * work.g[k];
                }
                let inside = (0..nc).all(|k| trial[k].abs() < w[k]);
                if inside && work.factor(h, &trial, lambda) {
                    ok = true;
                    break;
                }
                tt *= 0.5;
            }
            if !ok {
                // The factor of `nu` itself is restored for the primal below.
                work.factor(h, &nu, lambda);
                break;
            }
            nu.copy_from_slice(&trial);
        }
        // The objective at this point: the weighted residuals of `X = mu S^-1`
        // plus the trade-off times its trace.
        let objective = {
            let residual: f64 = (0..nc)
                .map(|k| w[k] * (y[k] - mu * work.m[k * nc + k].re).abs())
                .sum();
            residual + lambda * mu * work.inverse_trace()
        };
        if mu * degree <= GAP_REL * objective.max(f64::MIN_POSITIVE) {
            break;
        }
        mu /= MU_STEP;
    }
    // `work` holds the factor of the final `nu`.
    let mut x = vec![C64::new(0.0, 0.0); nd * nd];
    work.primal(mu, &mut x);
    let trace: f64 = (0..nd).map(|i| x[i * nd + i].re).sum();
    let predicted: Vec<f64> = (0..nc).map(|k| mu * work.m[k * nc + k].re).collect();
    let residual: f64 = (0..nc).map(|k| w[k] * (y[k] - predicted[k]).abs()).sum();
    Ok((
        LineSolution {
            x,
            trace,
            residual,
            predicted,
        },
        nu,
    ))
}

/// The scale of one line: the residual with no drive at all (`sum w_k y_k`)
/// and a drive power, `sum_k y_k / ||h_k||^2`, the least power that can put
/// `y_k` on control `k` alone (by Cauchy-Schwarz, `(h X h^H) <= ||h||^2 tr X`).
/// The absolute trade-off is `lambda_rel * r0 / d0`, so `lambda_rel = 1`
/// prices that drive at the whole response, and the grid means the same on
/// every plant. Only arithmetic: the scale must come out the same on every
/// target, because the solve's decisions depend on it.
#[allow(clippy::needless_range_loop)]
fn scales(h_line: &[C64], nc: usize, nd: usize, y: &[f64], w: &[f64]) -> (f64, f64) {
    let r0: f64 = (0..nc).map(|k| w[k] * y[k]).sum();
    let mut d0 = 0.0;
    for k in 0..nc {
        let mut gain = 0.0;
        for d in 0..nd {
            gain += h_line[k * nd + d].norm_sqr();
        }
        if gain > 0.0 {
            d0 += y[k] / gain;
        }
    }
    (r0, d0)
}

/// The weights of a line's residuals.
fn residual_weights(y: &[f64], weight: ResidualWeight) -> Vec<f64> {
    match weight {
        ResidualWeight::Absolute => vec![1.0; y.len()],
        ResidualWeight::Relative => {
            let ymax = y.iter().copied().fold(0.0f64, f64::max);
            y.iter()
                .map(|&v| 1.0 / v.max(1e-12 * ymax).max(f64::MIN_POSITIVE))
                .collect()
        }
    }
}

/// The minimum-drive target for the whole grid.
pub(crate) struct Target {
    /// The predicted response `H X H^H`, controls by controls, zero outside
    /// the band.
    pub target: HermitianStack,
    /// The relative trade-off used.
    pub tradeoff: f64,
    /// `tr X` per line of the grid (zero outside the band).
    pub drive_power: Vec<f64>,
    /// The predicted over the specified autospectrum per control per line,
    /// line-major: 1 where the target is as specified, below 1 where the
    /// trade-off gave level up (1 outside the band or where unspecified). A
    /// ratio, not decibels: a logarithm is evaluated differently by the two
    /// targets' libraries, and the parity test compares these values.
    pub target_ratio: Vec<f64>,
}

/// How the trade-off is chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tradeoff {
    /// At the knee of drive power against residual over the grid, on a
    /// subsample of the band's lines.
    Auto,
    /// This relative value.
    Fixed(f64),
}

/// The per-line inputs the solve reads: the plant and the pseudo-inverse
/// scale, prepared once.
struct Lines {
    nc: usize,
    nd: usize,
    /// The in-band plant, line-major.
    h: Vec<C64>,
    /// Specified autospectra per in-band line.
    y: Vec<f64>,
    /// Residual weights per in-band line.
    w: Vec<f64>,
    /// `(r0, d0)` per in-band line.
    scale: Vec<(f64, f64)>,
    /// The alarm lines per in-band line per control, as `(lower, upper)`
    /// bounds on the autospectrum (infinite where there is no line).
    alarm: Vec<(f64, f64)>,
}

impl Lines {
    fn new(
        frf: &MatrixStack,
        reference: &HermitianStack,
        tolerances: &Tolerances,
        band: (usize, usize),
        weight: ResidualWeight,
    ) -> Result<Self, LawError> {
        let (lines, nc, nd) = frf.dims();
        if reference.lines() != lines || reference.n() != nc {
            return Err(LawError(format!(
                "the reference is {} lines by {}, the plant {lines} by {nc} by {nd}",
                reference.lines(),
                reference.n()
            )));
        }
        let nb = band.1 - band.0;
        let mut h = Vec::with_capacity(nb * nc * nd);
        let mut y = Vec::with_capacity(nb * nc);
        let mut w = Vec::with_capacity(nb * nc);
        let mut alarm = Vec::with_capacity(nb * nc);
        let bound = |v: Option<&f64>, none: f64| match v {
            Some(&x) if x.is_finite() => x,
            _ => none,
        };
        for line in band.0..band.1 {
            h.extend_from_slice(frf.line(line));
            let d: Vec<f64> = reference
                .diagonal(line)
                .iter()
                .map(|v| v.max(0.0))
                .collect();
            w.extend(residual_weights(&d, weight));
            y.extend(d);
            for k in 0..nc {
                let at = line * nc + k;
                alarm.push((
                    bound(tolerances.alarm_lower.get(at), f64::NEG_INFINITY),
                    bound(tolerances.alarm_upper.get(at), f64::INFINITY),
                ));
            }
        }
        let scale = (0..nb)
            .map(|i| {
                scales(
                    &h[i * nc * nd..(i + 1) * nc * nd],
                    nc,
                    nd,
                    &y[i * nc..(i + 1) * nc],
                    &w[i * nc..(i + 1) * nc],
                )
            })
            .collect();
        Ok(Lines {
            nc,
            nd,
            h,
            y,
            w,
            scale,
            alarm,
        })
    }

    /// The absolute trade-off of in-band line `i` for the relative one.
    fn lambda(&self, i: usize, rel: f64) -> Option<f64> {
        let (r0, d0) = self.scale[i];
        if r0 > 0.0 && d0 > 0.0 {
            Some(rel * r0 / d0)
        } else {
            None
        }
    }

    /// Solve in-band line `i` at relative trade-off `rel`; `None` for a line
    /// with nothing specified or no plant.
    fn solve(
        &self,
        i: usize,
        rel: f64,
        warm: Option<&[f64]>,
    ) -> Result<Option<(LineSolution, Vec<f64>)>, LawError> {
        let (nc, nd) = (self.nc, self.nd);
        let lambda = match self.lambda(i, rel) {
            Some(l) => l,
            None => return Ok(None),
        };
        solve_line(
            &self.h[i * nc * nd..(i + 1) * nc * nd],
            nc,
            nd,
            &self.y[i * nc..(i + 1) * nc],
            &self.w[i * nc..(i + 1) * nc],
            lambda,
            warm,
        )
        .map(Some)
    }
}

/// The automatic trade-off: over the grid, on at most [`SAMPLE_LINES`] lines
/// spread over the band, the drive power and the residual are summed, and the
/// grid point minimizing their product is chosen, leaving out the points whose
/// drive power or residual is above nine tenths of the largest (Coletti, Davis
/// and Schultz, section 3.1, with the product in place of the sum of
/// logarithms), and the points whose target leaves the specification's alarm
/// lines on any sampled control and line: a test designed to alarm is not a
/// choice. With no point left, the smallest trade-off, which reproduces the
/// autospectra wherever the plant can.
fn choose_tradeoff(lines: &Lines) -> Result<f64, LawError> {
    let nb = lines.scale.len();
    let nc = lines.nc;
    let stride = nb.div_ceil(SAMPLE_LINES).max(1);
    let sample: Vec<usize> = (0..nb).step_by(stride).collect();
    let mut inp = vec![0.0; GRID.len()];
    let mut res = vec![0.0; GRID.len()];
    let mut inside = vec![true; GRID.len()];
    for &i in &sample {
        let mut warm: Option<Vec<f64>> = None;
        for (g, &rel) in GRID.iter().enumerate() {
            if let Some((sol, nu)) = lines.solve(i, rel, warm.as_deref())? {
                inp[g] += sol.trace;
                res[g] += sol.residual;
                for k in 0..nc {
                    let (lo, hi) = lines.alarm[i * nc + k];
                    let p = sol.predicted[k];
                    if p < lo || p > hi {
                        inside[g] = false;
                    }
                }
                // A larger trade-off keeps the dual point feasible when it is
                // scaled with it.
                warm = Some(nu);
            }
        }
    }
    let inp_max = inp.iter().copied().fold(0.0f64, f64::max);
    let res_max = res.iter().copied().fold(0.0f64, f64::max);
    let floor = 1e-12 * (inp_max + res_max);
    let mut best: Option<(f64, f64)> = None;
    for g in 0..GRID.len() {
        if !inside[g] || inp[g] > 0.9 * inp_max || res[g] > 0.9 * res_max {
            continue;
        }
        let score = (inp[g] + floor) * (res[g] + floor);
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((GRID[g], score));
        }
    }
    Ok(best.map(|(rel, _)| rel).unwrap_or(GRID[0]))
}

/// The minimum-drive target of `reference` on the plant `frf` over `band`.
pub(crate) fn target(
    frf: &MatrixStack,
    reference: &HermitianStack,
    tolerances: &Tolerances,
    band: (usize, usize),
    weight: ResidualWeight,
    tradeoff: Tradeoff,
) -> Result<Target, LawError> {
    let (grid_lines, nc, nd) = frf.dims();
    let lines = Lines::new(frf, reference, tolerances, band, weight)?;
    let rel = match tradeoff {
        Tradeoff::Fixed(v) => v,
        Tradeoff::Auto => choose_tradeoff(&lines)?,
    };
    let nb = band.1 - band.0;
    // Every in-band line at `rel`, lines in parallel; each line's arithmetic is
    // its own, so the split does not change a result.
    let mut out = vec![C64::new(0.0, 0.0); nb * nd * nd];
    let results: Vec<Result<(), LawError>> =
        over_line_chunks(&mut out, nd * nd, nb, |first, chunk| {
            for (j, x) in chunk.chunks_mut(nd * nd).enumerate() {
                let i = first + j;
                if let Some((sol, _)) = lines.solve(i, rel, None)? {
                    x.copy_from_slice(&sol.x);
                    clean_rank(x, nd, RANK_CUTOFF_REL);
                }
            }
            Ok(())
        });
    for r in results {
        r?;
    }
    let drive = {
        let full = MatrixStack::from_vec(nb, nd, nd, out)
            .map_err(|e| LawError(format!("the drive stack: {e}")))?;
        HermitianStack::from_full(&full).map_err(|e| LawError(format!("the drive: {e}")))?
    };
    let inband = MatrixStack::from_vec(nb, nc, nd, lines.h.clone())
        .map_err(|e| LawError(format!("the in-band plant: {e}")))?;
    let predicted = linalg::response_from_drive(&inband, &drive)
        .map_err(|e| LawError(format!("the predicted response: {e}")))?;
    let per = HermitianStack::packed_len(nc);
    let mut packed = vec![C64::new(0.0, 0.0); grid_lines * per];
    packed[band.0 * per..band.1 * per].copy_from_slice(predicted.data());
    let target = HermitianStack::from_packed(grid_lines, nc, packed)
        .map_err(|e| LawError(format!("the target: {e}")))?;
    let mut drive_power = vec![0.0; grid_lines];
    let mut target_ratio = vec![1.0; grid_lines * nc];
    for i in 0..nb {
        let line = band.0 + i;
        drive_power[line] = drive.trace(i);
        let y = &lines.y[i * nc..(i + 1) * nc];
        for k in 0..nc {
            let p = predicted.get(i, k, k).re;
            if y[k] > 0.0 {
                target_ratio[line * nc + k] = p / y[k];
            }
        }
    }
    Ok(Target {
        target,
        tradeoff: rel,
        drive_power,
        target_ratio,
    })
}

/// Eigenvalues and eigenvectors of a Hermitian `n` by `n` matrix (row-major)
/// by cyclic Jacobi rotations: arithmetic and square roots only, so the same
/// on every target. Returns the eigenvalues and the eigenvectors as the
/// columns of a row-major `n` by `n` matrix.
pub(crate) fn eigh_jacobi(a: &[C64], n: usize) -> (Vec<f64>, Vec<C64>) {
    let mut a = a.to_vec();
    let mut v = vec![C64::new(0.0, 0.0); n * n];
    for i in 0..n {
        v[i * n + i] = C64::new(1.0, 0.0);
    }
    let scale: f64 = a.iter().map(|z| z.norm_sqr()).sum::<f64>().sqrt();
    for _sweep in 0..60 {
        let off: f64 = (0..n)
            .flat_map(|p| ((p + 1)..n).map(move |q| (p, q)))
            .map(|(p, q)| a[p * n + q].norm_sqr())
            .sum::<f64>()
            .sqrt();
        if off <= 1e-15 * scale {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let g = a[p * n + q];
                let mag = g.norm();
                if mag == 0.0 {
                    continue;
                }
                // Take the phase out of a_pq (column q and row q scaled by a
                // unit complex number), then a real rotation zeroes it.
                let u = g / mag;
                let uc = u.conj();
                for k in 0..n {
                    a[k * n + q] *= uc;
                }
                for k in 0..n {
                    a[q * n + k] *= u;
                }
                for k in 0..n {
                    v[k * n + q] *= uc;
                }
                let app = a[p * n + p].re;
                let aqq = a[q * n + q].re;
                let apq = a[p * n + q].re;
                let zeta = (aqq - app) / (2.0 * apq);
                let t = if zeta >= 0.0 {
                    1.0 / (zeta + (1.0 + zeta * zeta).sqrt())
                } else {
                    -1.0 / (-zeta + (1.0 + zeta * zeta).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k * n + p], a[k * n + q]);
                    a[k * n + p] = akp * c - akq * s;
                    a[k * n + q] = akp * s + akq * c;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p * n + k], a[q * n + k]);
                    a[p * n + k] = apk * c - aqk * s;
                    a[q * n + k] = apk * s + aqk * c;
                }
                a[p * n + q] = C64::new(0.0, 0.0);
                a[q * n + p] = C64::new(0.0, 0.0);
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = vkp * c - vkq * s;
                    v[k * n + q] = vkp * s + vkq * c;
                }
            }
        }
    }
    let values = (0..n).map(|i| a[i * n + i].re).collect();
    (values, v)
}

/// Set the eigenvalues of the Hermitian `x` (`n` by `n`, row-major) below
/// `rel` times its largest to zero, in place: the matrix of exactly the rank
/// its solution has.
pub(crate) fn clean_rank(x: &mut [C64], n: usize, rel: f64) {
    let (values, v) = eigh_jacobi(x, n);
    let max = values.iter().copied().fold(0.0f64, f64::max);
    if max <= 0.0 {
        x.fill(C64::new(0.0, 0.0));
        return;
    }
    let kept: Vec<(usize, f64)> = values
        .iter()
        .enumerate()
        .filter(|(_, &e)| e >= rel * max)
        .map(|(j, &e)| (j, e))
        .collect();
    for i in 0..n {
        for k in 0..n {
            let mut sum = C64::new(0.0, 0.0);
            for &(j, e) in &kept {
                sum += v[i * n + j] * e * v[k * n + j].conj();
            }
            x[i * n + k] = sum;
        }
        x[i * n + i] = C64::new(x[i * n + i].re, 0.0);
    }
}

/// The loop's inverse rule for the minimum-drive law: the default law's.
pub(crate) fn loop_rule() -> InverseRule {
    InverseRule::Regularized {
        reg: Regularization::Truncated { rcond: 1e-3 },
        weighting: Weighting::None,
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    /// One control, two drives: the response is `|h|^2 x` along the plant's
    /// direction, so the drive is `y / |h|^2` of power when the trade-off is
    /// below `|h|^2` (per unit weight) and zero above it.
    #[test]
    fn one_control_two_drives_has_the_closed_form() {
        let h = [c(1.0, 0.5), c(-0.5, 2.0)];
        let hh: f64 = h.iter().map(|v| v.norm_sqr()).sum();
        let y = [3.0];
        let w = [1.0];
        for &(lambda, want) in &[(0.1 * hh, 3.0 / hh), (10.0 * hh, 0.0)] {
            let (sol, _) = solve_line(&h, 1, 2, &y, &w, lambda, None).unwrap();
            assert!(
                (sol.trace - want).abs() < 1e-6 * (1.0 + want),
                "lambda {lambda}: trace {} against {want}",
                sol.trace
            );
            if want > 0.0 {
                assert!(
                    (sol.predicted[0] - 3.0).abs() < 1e-6,
                    "{}",
                    sol.predicted[0]
                );
            }
        }
    }

    /// With a tiny trade-off and a square well-conditioned plant the autospectra
    /// are reproduced, and the drive power is below the pseudo-inverse drive's
    /// for the independent reference, which also reproduces them.
    #[test]
    fn the_exact_control_end_needs_no_more_than_the_independent_reference() {
        let h = [c(1.0, 0.0), c(0.3, 0.2), c(-0.2, 0.4), c(0.8, -0.1)];
        let y = [2.0, 1.0];
        let w = [1.0, 1.0];
        let (sol, _) = solve_line(&h, 2, 2, &y, &w, 1e-6, None).unwrap();
        for k in 0..2 {
            assert!(
                (sol.predicted[k] - y[k]).abs() < 1e-4 * y[k],
                "{:?}",
                sol.predicted
            );
        }
        // The pseudo-inverse drive for diag(y): H^-1 diag(y) H^-H.
        let det = h[0] * h[3] - h[1] * h[2];
        let inv = [h[3] / det, -h[1] / det, -h[2] / det, h[0] / det];
        let mut pinv_trace = 0.0;
        for d in 0..2 {
            for k in 0..2 {
                pinv_trace += y[k] * inv[d * 2 + k].norm_sqr();
            }
        }
        assert!(
            sol.trace <= pinv_trace * (1.0 + 1e-6),
            "{} > {pinv_trace}",
            sol.trace
        );
    }

    /// Along the grid the drive power falls and the residual rises: the
    /// Pareto property.
    #[test]
    fn the_trade_off_moves_along_a_pareto_curve() {
        let h = [
            c(1.0, 0.0),
            c(0.3, 0.2),
            c(0.1, -0.4),
            c(-0.2, 0.4),
            c(0.8, -0.1),
            c(0.5, 0.5),
            c(0.2, 0.2),
            c(-0.6, 0.1),
            c(0.9, 0.0),
        ];
        let y = [2.0, 1.0, 0.5];
        let w = [1.0, 1.0, 1.0];
        let mut last: Option<(f64, f64)> = None;
        for &rel in &[1e-3, 1e-2, 1e-1, 1.0, 10.0] {
            let (sol, _) = solve_line(&h, 3, 3, &y, &w, rel, None).unwrap();
            if let Some((t, r)) = last {
                // Each solve is to a relative gap of 1e-7 of its own objective.
                assert!(
                    sol.trace <= t * (1.0 + 1e-6),
                    "trace rose at {rel}: {} > {t}",
                    sol.trace
                );
                // Residuals at the exact-control end are barrier artifacts of
                // about 1e-7 of the response (3.5 here), not a trend.
                assert!(
                    sol.residual >= r - 1e-6 * 3.5,
                    "residual fell at {rel}: {} < {r}",
                    sol.residual
                );
            }
            last = Some((sol.trace, sol.residual));
        }
    }

    /// The Jacobi eigensolver diagonalizes a Hermitian matrix: `A V = V D`
    /// and `V^H V = I`, and the rank cleanup keeps the trace of what it keeps.
    #[test]
    fn the_jacobi_eigensolver_diagonalizes_and_the_cleanup_keeps_the_rank() {
        let n = 5;
        let mut rng = 0x9e3779b97f4a7c15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let b: Vec<C64> = (0..n * n).map(|_| c(next(), next())).collect();
        // A = B B^H + a rank-one part, so it is Hermitian positive definite.
        let mut a = vec![c(0.0, 0.0); n * n];
        for i in 0..n {
            for k in 0..n {
                let mut s = c(0.0, 0.0);
                for j in 0..n {
                    s += b[i * n + j] * b[k * n + j].conj();
                }
                a[i * n + k] = s;
            }
        }
        let (values, v) = eigh_jacobi(&a, n);
        for j in 0..n {
            for i in 0..n {
                let mut av = c(0.0, 0.0);
                for k in 0..n {
                    av += a[i * n + k] * v[k * n + j];
                }
                let d = av - v[i * n + j] * values[j];
                assert!(d.norm() < 1e-12, "A v != lambda v at {i} {j}: {d}");
            }
            for l in 0..n {
                let mut dot = c(0.0, 0.0);
                for k in 0..n {
                    dot += v[k * n + j].conj() * v[k * n + l];
                }
                let want = if j == l { 1.0 } else { 0.0 };
                assert!((dot - want).norm() < 1e-12, "V^H V at {j} {l}: {dot}");
            }
        }
        let trace: f64 = (0..n).map(|i| a[i * n + i].re).sum();
        let sum: f64 = values.iter().sum();
        assert!((trace - sum).abs() < 1e-12 * trace);
        // A rank-one matrix plus a tiny part cleans to rank one.
        let mut r1 = vec![c(0.0, 0.0); n * n];
        let u: Vec<C64> = (0..n).map(|_| c(next(), next())).collect();
        for i in 0..n {
            for k in 0..n {
                r1[i * n + k] = u[i] * u[k].conj() + a[i * n + k] * 1e-6;
            }
        }
        clean_rank(&mut r1, n, 1e-3);
        let (values, _) = eigh_jacobi(&r1, n);
        let max = values.iter().copied().fold(0.0f64, f64::max);
        let kept = values.iter().filter(|&&e| e > 1e-9 * max).count();
        assert_eq!(kept, 1, "{values:?}");
    }

    /// The drive is Hermitian and positive definite.
    #[test]
    fn the_drive_is_hermitian_and_positive() {
        let h = [
            c(1.0, 0.0),
            c(0.3, 0.2),
            c(-0.2, 0.4),
            c(0.8, -0.1),
            c(0.5, 0.1),
            c(0.0, 0.7),
        ];
        let y = [2.0, 1.0];
        let w = [0.5, 1.0];
        let (sol, _) = solve_line(&h, 2, 3, &y, &w, 0.3, None).unwrap();
        let n = 3;
        for i in 0..n {
            assert_eq!(sol.x[i * n + i].im, 0.0);
            assert!(sol.x[i * n + i].re > 0.0);
            for j in 0..n {
                let d = sol.x[i * n + j] - sol.x[j * n + i].conj();
                assert!(d.norm() < 1e-12, "{i} {j}: {d}");
            }
        }
        let mut l = vec![c(0.0, 0.0); n * n];
        assert!(cholesky(&sol.x, n, &mut l).is_some());
    }
}
