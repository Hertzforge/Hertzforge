//! The closed loop shared by the pseudo-inverse, shape-constrained and
//! buzz-test laws: Smallwood's correction of a factor of the reference.
//!
//! Source: D. O. Smallwood, "Multiple Shaker Random Vibration Control: An
//! Update", Proc. IEST 1999 (the random correction algorithm, its appendix and
//! its safeguards); the factorization with zeros after C. J. Dodds and J. D.
//! Robson, "Partial Coherence in Multivariate Random Processes", Journal of
//! Sound and Vibration 42 (1975).
//!
//! # The loop
//!
//! Per line, the reference is factored `G_ref = R R^H` with `R` lower
//! triangular (a zero pivot leaves that source's column zero). The loop keeps a
//! factor `Z`, starting at `R`, and the drive is `G_uu = A Z Z^H A^H` with `A`
//! the plant's inverse, computed as the Gram matrix of `A Z`, so it is positive
//! semi-definite by construction. Each loop the error `E = G_ref - G_measured`
//! corrects `Z`:
//!
//! - **First form** (amplitude and phase): the lower-triangular `Delta` solving
//!   the linearization `Delta Z^H + Z Delta^H = E`, column by column:
//!   `Delta_jj = Re[E_jj - sum_{i<j} (Delta_ji conj(Z_ji) + Z_ji conj(Delta_ji))] / (2 Z_jj)`,
//!   `Delta_kj = [E_kj - sum_{i<j} (Delta_ki conj(Z_ji) + Z_ki conj(Delta_ji)) - Z_kj Delta_jj] / Z_jj`.
//! - **Second form** (amplitude only): for a cross term `Z_kj` whose channel
//!   `k` is above its reference (`E_kk < 0`) when the first form would grow
//!   it, `Delta_kj = Z_kj E_kk / (2 G_ref,kk)`, or `-Z_kj / 2` when
//!   `|E_kk| >= G_ref,kk / 2` (the brief's first-order reading of the
//!   appendix). Autospectra take priority over cross terms, in channel order,
//!   which the column order of the recursion gives.
//! - **Update** `Z_new = Z + e Delta` with the convergence factor `e`.
//!
//! # The coordinates
//!
//! When the plant carries as many independent sources as there are controls
//! (a square or under-determined plant of full rank), the loop runs in the
//! controls' own coordinates and channel order, exactly as the source states
//! it, with `A = H^+`.
//!
//! When it carries fewer, `q`, (more controls than drives, or directions the
//! regularization or the shape constraint drops), the response lies in a
//! `q`-dimensional subspace and only that part of the error can be corrected.
//! Run in the controls' coordinates, the recursion then leaves part of the
//! error unmatched and the plant's projection mixes it back in, and the loop
//! winds up (measured: a reachable 6 x 4 reference came within 0.5 dB by loop
//! 4 and drifted to 2.5 dB by loop 30). The source's rank rule ("the number of
//! nonzero rows and columns in Z should not exceed the rank") answers this by
//! keeping only the first channels, which makes the loop's gain on them a
//! sub-block of the plant's projection. Here instead the loop runs in the
//! coordinates of that subspace: with the weighted plant `W H = U S V^H` and
//! the inverse's filter factors `f`, `T = U_q^H W` reduces a control-space
//! matrix to `q` by `q` and `A = V_q diag(f_q)` maps the reduced sources to the
//! drives (`A T = H^+`). The reduced loop is square and full rank, converges at
//! the full gain, and settles at the least-squares response `P G_ref P^H`, the
//! response the open-loop pseudo-inverse gives on an exact plant. In both
//! cases the seed, `Z = R`, gives exactly `G_uu = H^+ G_ref H^+^H`.
//!
//! When the plant estimate changes (the FRF updated during the run) the
//! coordinates change with it; the factor is carried over by factoring the
//! current drive in the new coordinates, `A^+ G_uu A^+^H`, so the correction
//! made so far is kept.
//!
//! # Safeguards from the source, as implemented here
//!
//! - **Diagonal clamp.** No diagonal of `Z` moves by more than the per-loop
//!   cap: by default `+3.5 / -6.0 dB` (about +50% and -50%), the limit
//!   implicit in the source's 50% clamp. The source clamps each diagonal step
//!   alone, relative to `max(Z_11, Z_jj)`. Here the bound is relative to
//!   `Z_jj` itself, so it holds on every channel, and the full correction is
//!   solved first and a clamped column has all its first-form terms scaled by
//!   the diagonal's own ratio. Solved against the clamped diagonal, the column's
//!   cross terms made up for it and overshot under a tight cap (measured: a
//!   10 dB deficit under a +1 / -2 dB cap was still 3 dB off after 30 loops,
//!   falling and rising in turn); scaled per column, a uniform error is
//!   corrected uniformly. Second-form terms are bounded already and are applied
//!   as they are (scaled with the rest, they stalled a line 10 dB too high).
//!   Clamped columns are counted.
//! - **Floor.** A diagonal never goes below `1e-4` times the square root of the
//!   line's largest reference PSD; when one would, the rest of its column
//!   shrinks with it (the source's guard against a collapsing diagonal).
//! - **Divisor.** The source divides by `max(Z_11, Z_jj)` so a small `Z_jj`
//!   cannot blow the correction up, and its appendix then adds a rescaling for
//!   the slow, stuck-high behaviour that divisor causes on quiet channels. Here
//!   a small `Z_jj` is already held by the clamp and the floor, so the exact
//!   divisor `Z_jj` of the linearization is used (never below the floor).
//! - **Rank.** No more independent sources than the plant carries: the
//!   reduced coordinates have exactly that many.
//! - **Drive cap.** After the update the step is halved, per line, until no
//!   drive's PSD moves by more than the per-loop cap from the law's previous
//!   drive, so the law never asks for more than the cap (the controller applies
//!   its own cap regardless).
//!
//! # Safeguards added here, against wind-up
//!
//! The source's safeguards bound a diagonal's step and the drive's step, but
//! not where the factor goes over many loops. A source the plant does not
//! answer in, a direction the plant estimate holds and the structure does not
//! (an estimate identified through noise keeps its weak singular values on the
//! noise floor, above any fixed truncation), keeps a positive error loop after
//! loop, and the first form adds `E_kj / Z_jj` to its row each time: the
//! cross terms are clamped by nothing, and the second form acts only on a
//! channel above its reference. Measured on a 48 by 12 lightly damped plate
//! identified through noise: the drive rose from 8e4 to 1e8 lbf^2 in 45
//! loops, nearly all of it in rows whose target has no pivot of its own and
//! in sources whose response was below 1 % of what they commanded, while the
//! open-loop drive gave 1.22 dB. Two rules stop it, both arithmetic only:
//!
//! - **Answer hold.** A source whose measured response is below `ANSWER_MIN`
//!   (-15 dB) of what its row commands, `(T G_yy T^H)_kk < ANSWER_MIN (Z Z^H)_kk`,
//!   does not answer its drive, and no element of its row may grow that loop
//!   (each may still shrink). A law does not raise the drive where the
//!   response does not follow it. This is the measured form of the source's
//!   rank rule: a direction the plant does not carry gets no more drive.
//! - **Command bound.** No row commands more than `COMMAND_MAX` (+10 dB) of
//!   its source's target, `(Z Z^H)_kk <= COMMAND_MAX G_t,kk` (or more than it
//!   commanded the loop before, where a factor carried over an FRF update is
//!   already above it). It bounds the drive at about ten times the seed's
//!   whatever the response does, and leaves 10 dB for the plant's error in
//!   any one direction.
//!
//! On the plate the two rules hold the drive at 2.5e5 lbf^2 and the band error
//! at 1.46 dB; the bound does most of the work there, and the hold alone at
//! -10 dB gave 1.49 dB at 3.4e5. A higher hold threshold was not taken: the
//! loop must still climb to a plant 10 dB less sensitive than its estimate.

use crate::support::{
    band_hermitian, gram_packed, inverse_diagnostics, largest_change_db, matmul_adj_into,
    matmul_into, over_line_chunks, row_power, Inverse, InverseCache, InverseRule, ZERO,
};
use hertzforge_sdm::law::{LawError, MimoSetup};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// A pivot below this fraction of its channel's reference PSD is zero: the
/// channel is fully coherent with the ones before it.
const PIVOT_REL: f64 = 1e-10;
/// The floor on a factor diagonal, relative to the square root of the line's
/// largest reference PSD (the source's default tolerance).
const FLOOR_TOL: f64 = 1e-4;
/// How many times the drive-cap search halves the step before holding the
/// line's factor where it was.
const HALVINGS: usize = 30;
/// A source answers its drive when its measured response, in the loop's
/// coordinates, is at least this fraction of what its row of the factor
/// commands (-15 dB). On an exact plant the two are equal; a plant 10 dB less
/// sensitive than its estimate still answers, and the loop climbs to it.
const ANSWER_MIN: f64 = 0.03;
/// No source commands more than this multiple of its target (+10 dB).
const COMMAND_MAX: f64 = 10.0;

/// The closed loop's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LoopSettings {
    /// The convergence factor `e`, 0 to 1.
    pub convergence: f64,
    /// Largest rise per loop, dB (of a PSD; half of it applies to a factor
    /// element).
    pub cap_up_db: f64,
    /// Largest fall per loop, dB.
    pub cap_down_db: f64,
    /// Singular values above this fraction of the largest count toward the
    /// rank.
    pub rank_tol: f64,
}

/// One loop's result.
pub(crate) struct Step {
    pub drive: HermitianStack,
    pub condition: Vec<f64>,
    pub kept: Vec<f64>,
    pub correction_db: f64,
    pub clamped: usize,
}

/// The coordinates of one line: `q` sources, `A` (drives by `q`), `T` (`q` by
/// controls, absent in the controls' own coordinates) and `A^+`.
#[derive(Clone, Debug)]
struct LineMap {
    q: usize,
    a: Vec<C64>,
    t: Option<Vec<C64>>,
    a_pinv: Vec<C64>,
}

impl LineMap {
    fn new(inv: &Inverse, i: usize, rank_tol: f64) -> Self {
        let (nc, nd, k) = (inv.nc, inv.nd, inv.k);
        let q = inv.sources(i, rank_tol);
        let (u, v) = (inv.u.line(i), inv.v.line(i));
        let f = &inv.f[i * k..(i + 1) * k];
        if q == nc {
            // The controls' own coordinates: A = H^+ (drives by controls), of
            // full column rank, and A^+ = W^-1 U diag(1/f) V^H.
            let mut a_pinv = vec![ZERO; nc * nd];
            for c in 0..nc {
                let wc = inv.weight(i, c);
                for d in 0..nd {
                    let mut x = ZERO;
                    for j in 0..q {
                        x += u[c * k + j] * v[d * k + j].conj() / f[j];
                    }
                    a_pinv[c * nd + d] = x / wc;
                }
            }
            LineMap {
                q,
                a: inv.hp.line(i).to_vec(),
                t: None,
                a_pinv,
            }
        } else {
            let mut a = vec![ZERO; nd * q];
            let mut t = vec![ZERO; q * nc];
            let mut a_pinv = vec![ZERO; q * nd];
            for j in 0..q {
                for d in 0..nd {
                    a[d * q + j] = v[d * k + j] * f[j];
                    a_pinv[j * nd + d] = v[d * k + j].conj() / f[j];
                }
                for c in 0..nc {
                    t[j * nc + c] = u[c * k + j].conj() * inv.weight(i, c);
                }
            }
            LineMap {
                q,
                a,
                t: Some(t),
                a_pinv,
            }
        }
    }

    /// `T G T^H` (or `G` itself) of a control-space line, `q` by `q`, into `out`.
    fn reduce(&self, g: &HermitianStack, line: usize, nc: usize, out: &mut [C64]) {
        let mut full = vec![ZERO; nc * nc];
        for r in 0..nc {
            for c in 0..nc {
                full[r * nc + c] = g.get(line, r, c);
            }
        }
        match &self.t {
            None => out[..nc * nc].copy_from_slice(&full),
            Some(t) => {
                let q = self.q;
                let mut tg = vec![ZERO; q * nc];
                matmul_into(t, &full, q, nc, nc, &mut tg);
                matmul_adj_into(&tg, t, q, nc, q, &mut out[..q * q]);
            }
        }
    }
}

/// Cholesky with zeros of one `n` by `n` matrix `g` (row-major): `R` lower
/// triangular with `G = R R^H`; a pivot at or below [`PIVOT_REL`] of its
/// channel's PSD (or a channel with no PSD) leaves that column zero.
fn factor_with_zeros(g: &[C64], n: usize, r: &mut [C64], pivot: &mut [bool]) {
    r[..n * n].fill(ZERO);
    for j in 0..n {
        let gjj = g[j * n + j].re;
        let d = gjj - (0..j).map(|i| r[j * n + i].norm_sqr()).sum::<f64>();
        let live = gjj > 0.0 && d > PIVOT_REL * gjj;
        pivot[j] = live;
        if !live {
            continue;
        }
        let rjj = d.sqrt();
        r[j * n + j] = C64::new(rjj, 0.0);
        for k in (j + 1)..n {
            let mut s = g[k * n + j];
            for i in 0..j {
                s -= r[k * n + i] * r[j * n + i].conj();
            }
            r[k * n + j] = s / rjj;
        }
    }
}

/// One line's slots of the target, its factor, `Z` and the pivots.
type LineSlots<'a> = (&'a mut [C64], &'a mut [C64], &'a mut [C64], &'a mut [bool]);

/// The factor loop for one run.
pub(crate) struct FactorLoop {
    lines: usize,
    nc: usize,
    nd: usize,
    band: (usize, usize),
    /// The target on the in-band lines, in the controls' coordinates.
    target: HermitianStack,
    rule: InverseRule,
    settings: LoopSettings,
    cache: InverseCache,
    /// The inverse generation the coordinates below were built for (0: none).
    generation: u64,
    maps: Vec<LineMap>,
    /// Per in-band line, `nc * nc` slots each, the top-left `q` by `q` used:
    /// the target in the line's coordinates, its factor `R`, and `Z`.
    gt: Vec<C64>,
    r: Vec<C64>,
    z: Vec<C64>,
    /// Which columns of `R` have a non-zero pivot (`nc` slots per line).
    pivot: Vec<bool>,
    last: Option<HermitianStack>,
}

impl FactorLoop {
    /// The loop driving toward `target` (controls by controls, the whole grid)
    /// for `setup`, with the inverse formed by `rule`.
    pub fn new(
        setup: &MimoSetup,
        target: &HermitianStack,
        rule: InverseRule,
        settings: LoopSettings,
    ) -> Result<Self, LawError> {
        setup.check()?;
        let (nc, band) = (setup.controls, setup.band);
        let nb = band.1 - band.0;
        Ok(FactorLoop {
            lines: setup.lines,
            nc,
            nd: setup.drives,
            band,
            target: band_hermitian(target, band),
            rule,
            settings,
            cache: InverseCache::default(),
            generation: 0,
            maps: Vec::new(),
            gt: vec![ZERO; nb * nc * nc],
            r: vec![ZERO; nb * nc * nc],
            z: vec![ZERO; nb * nc * nc],
            pivot: vec![false; nb * nc],
            last: None,
        })
    }

    /// Bring the coordinates up to the plant estimate `frf`. On a change, the
    /// target is reduced and factored again, and `Z` restarts at `R` or, when
    /// `carry` and a drive is in force, is the factor of that drive in the new
    /// coordinates.
    fn refresh(&mut self, frf: &MatrixStack, carry: bool) -> Result<(), LawError> {
        let (nc, nd) = (self.nc, self.nd);
        self.cache.get(frf, self.band, &self.target, self.rule)?;
        let generation = self.cache.generation();
        if generation == self.generation {
            return Ok(());
        }
        let inv = self.cache.current().expect("formed above");
        let tol = self.settings.rank_tol;
        let nb = self.band.1 - self.band.0;
        // Each line's coordinates, its target and factor, and `Z`, over threads:
        // the lines are independent.
        let mut maps: Vec<Option<LineMap>> = vec![None; nb];
        over_line_chunks(&mut maps, 1, nb, |first, chunk| {
            for (k, m) in chunk.iter_mut().enumerate() {
                *m = Some(LineMap::new(inv, first + k, tol));
            }
        });
        self.maps = maps.into_iter().map(|m| m.expect("every line")).collect();
        let per = nc * nc;
        let (maps, target, last, band) = (&self.maps, &self.target, &self.last, self.band);
        // Each line's slots of the four per-line arrays, chunked together.
        let mut slots: Vec<LineSlots<'_>> = self
            .gt
            .chunks_mut(per)
            .zip(self.r.chunks_mut(per))
            .zip(self.z.chunks_mut(per))
            .zip(self.pivot.chunks_mut(nc))
            .map(|(((g, r), z), p)| (g, r, z, p))
            .collect();
        over_line_chunks(&mut slots, 1, nb, |first, chunk| {
            let mut c = vec![ZERO; nc * nc];
            let mut tmp = vec![ZERO; nc * nd];
            let mut piv = vec![false; nc];
            let mut d = vec![ZERO; nd * nd];
            for (k, (gt, r, z, pivot)) in chunk.iter_mut().enumerate() {
                let i = first + k;
                let map = &maps[i];
                let q = map.q;
                map.reduce(target, i, nc, gt);
                factor_with_zeros(gt, q, r, &mut pivot[..q]);
                for p in &mut pivot[q..] {
                    *p = false;
                }
                match (last, carry) {
                    (Some(last), true) => {
                        // A^+ G_uu A^+^H, factored.
                        for row in 0..nd {
                            for col in 0..nd {
                                d[row * nd + col] = last.get(band.0 + i, row, col);
                            }
                        }
                        matmul_into(&map.a_pinv, &d, q, nd, nd, &mut tmp[..q * nd]);
                        matmul_adj_into(&tmp[..q * nd], &map.a_pinv, q, nd, q, &mut c[..q * q]);
                        factor_with_zeros(&c, q, z, &mut piv);
                    }
                    _ => z[..q * q].copy_from_slice(&r[..q * q]),
                }
                // Only the target's sources are live.
                for j in 0..q {
                    if !pivot[j] {
                        for k in j..q {
                            z[k * q + j] = ZERO;
                        }
                    }
                }
            }
        });
        self.generation = generation;
        Ok(())
    }

    /// The drive, condition numbers and kept counts for the current factor.
    fn drive(
        &mut self,
        frf: &MatrixStack,
    ) -> Result<(HermitianStack, Vec<f64>, Vec<f64>), LawError> {
        let (nc, nd) = (self.nc, self.nd);
        let inv = self.cache.get(frf, self.band, &self.target, self.rule)?;
        let per = HermitianStack::packed_len(nd);
        let mut data = vec![ZERO; self.lines * per];
        let (lo, hi) = self.band;
        let (maps, zs) = (&self.maps, &self.z);
        over_line_chunks(
            &mut data[lo * per..hi * per],
            per,
            hi - lo,
            |first, chunk| {
                let mut b = vec![ZERO; nd * nc];
                for (k, out) in chunk.chunks_mut(per).enumerate() {
                    let i = first + k;
                    let map = &maps[i];
                    let q = map.q;
                    let z = &zs[i * nc * nc..i * nc * nc + q * q];
                    matmul_into(&map.a, z, nd, q, q, &mut b[..nd * q]);
                    gram_packed(&b[..nd * q], nd, q, out);
                }
            },
        );
        let (cond, kept) = inverse_diagnostics(inv, self.lines, self.band);
        let drive = HermitianStack::from_packed(self.lines, nd, data)?;
        Ok((drive, cond, kept))
    }

    /// The first drive: `Z = R`, so `G_uu = H^+ G_ref H^+^H`.
    pub fn seed(&mut self, frf: &MatrixStack) -> Result<Step, LawError> {
        self.generation = 0;
        self.last = None;
        self.refresh(frf, false)?;
        let (drive, condition, kept) = self.drive(frf)?;
        self.last = Some(drive.clone());
        Ok(Step {
            drive,
            condition,
            kept,
            correction_db: 0.0,
            clamped: 0,
        })
    }

    /// One loop: correct the factor from the full-level `response`, limit the
    /// step to the cap, and return the new drive.
    pub fn iterate(
        &mut self,
        frf: &MatrixStack,
        response: &HermitianStack,
    ) -> Result<Step, LawError> {
        let Some(last) = self.last.clone() else {
            return Err(LawError("iterate was called before seed".into()));
        };
        self.refresh(frf, true)?;
        let (nc, nd) = (self.nc, self.nd);
        let s = self.settings;
        let e = s.convergence;
        let up = 10f64.powf(s.cap_up_db / 20.0);
        let down = 10f64.powf(-s.cap_down_db / 20.0);
        let psd_up = up * up * (1.0 + 1e-12);
        let psd_down = down * down * (1.0 - 1e-12);
        let nb = self.band.1 - self.band.0;
        let band0 = self.band.0;
        let (maps, gts, pivots) = (&self.maps, &self.gt, &self.pivot);
        // Each line's correction is independent of every other line's, so the
        // lines are corrected over threads; each line's arithmetic is unchanged.
        let counts = over_line_chunks(&mut self.z, nc * nc, nb, |first, chunk| {
            let mut clamped = 0usize;
            let mut measured = vec![ZERO; nc * nc];
            let mut err = vec![ZERO; nc * nc];
            let mut delta = vec![ZERO; nc * nc];
            let mut second = vec![false; nc * nc];
            let mut znew = vec![ZERO; nc * nc];
            let mut ztry = vec![ZERO; nc * nc];
            let mut b = vec![ZERO; nd * nc];
            let mut power = vec![0.0; nd];
            for (k, zline) in chunk.chunks_mut(nc * nc).enumerate() {
                let i = first + k;
                let line = band0 + i;
                let map = &maps[i];
                let q = map.q;
                let slot = i * nc * nc;
                let gt = &gts[slot..slot + q * q];
                let pivot = &pivots[i * nc..i * nc + q];
                let zl = &mut zline[..q * q];
                let gmax = (0..q).map(|k| gt[k * q + k].re).fold(0.0f64, f64::max);
                if gmax <= 0.0 || q == 0 {
                    continue;
                }
                let minz = FLOOR_TOL * gmax.sqrt();
                map.reduce(response, line, nc, &mut measured);
                for idx in 0..q * q {
                    err[idx] = gt[idx] - measured[idx];
                }

                // The correction, column by column.
                delta[..q * q].fill(ZERO);
                second[..q * q].fill(false);
                for j in 0..q {
                    if !pivot[j] {
                        continue;
                    }
                    let zjj = zl[j * q + j].re;
                    let div = zjj.max(minz);
                    let mut sjj = err[j * q + j].re;
                    for t in 0..j {
                        sjj -= 2.0 * (delta[j * q + t] * zl[j * q + t].conj()).re;
                    }
                    let djj = sjj / (2.0 * div);
                    delta[j * q + j] = C64::new(djj, 0.0);
                    for k in (j + 1)..q {
                        let mut t = err[k * q + j];
                        for m in 0..j {
                            t -= delta[k * q + m] * zl[j * q + m].conj()
                                + zl[k * q + m] * delta[j * q + m].conj();
                        }
                        t -= zl[k * q + j] * djj;
                        let mut dkj = t / div;
                        let ekk = err[k * q + k].re;
                        let zkj = zl[k * q + j];
                        if ekk < 0.0 && (zkj + dkj * e).norm() > zkj.norm() {
                            let gkk = gt[k * q + k].re;
                            dkj = if gkk <= 0.0 || -ekk >= 0.5 * gkk {
                                -zkj * 0.5
                            } else {
                                zkj * (ekk / (2.0 * gkk))
                            };
                            second[k * q + j] = true;
                        }
                        delta[k * q + j] = dkj;
                    }
                }

                // The diagonal clamp, per column: a column whose diagonal would move
                // by more than the cap has its first-form terms scaled down until it
                // does not. Second-form terms are already bounded (at most half the
                // element, toward zero) and are applied as they are.
                for j in 0..q {
                    if !pivot[j] {
                        continue;
                    }
                    let zjj = zl[j * q + j].re;
                    let step = e * delta[j * q + j].re;
                    let (lo, hi) = (zjj * (down - 1.0), zjj * (up - 1.0));
                    if step > hi || step < lo {
                        clamped += 1;
                        let c = if step > hi { hi / step } else { lo / step };
                        for k in j..q {
                            if !second[k * q + j] {
                                delta[k * q + j] *= c;
                            }
                        }
                    }
                }

                // The answer hold: a source whose measured response is below
                // `ANSWER_MIN` of what its row of `Z` commands does not answer its
                // drive, and no element of its row may grow this loop. Without it the
                // first form raises such a row every loop by `E_kj / Z_jj`, with
                // nothing to stop it: the diagonal clamp bounds only diagonals, and
                // the second form acts only on a channel above its reference.
                for k in 0..q {
                    let commanded: f64 = (0..=k).map(|t| zl[k * q + t].norm_sqr()).sum();
                    if commanded <= 0.0 || measured[k * q + k].re >= ANSWER_MIN * commanded {
                        continue;
                    }
                    for t in 0..=k {
                        let z0 = zl[k * q + t];
                        if (z0 + delta[k * q + t] * e).norm() > z0.norm() {
                            delta[k * q + t] = ZERO;
                        }
                    }
                }

                // The update, with the floor.
                for idx in 0..q * q {
                    znew[idx] = zl[idx] + delta[idx] * e;
                }
                // The command bound: no row of `Z` commands more than
                // `COMMAND_MAX` times its source's target, `(Z Z^H)_kk <= COMMAND_MAX
                // G_t,kk`, or more than it commanded last loop where that was already
                // above it (a factor carried over an FRF update).
                for k in 0..q {
                    let old: f64 = (0..=k).map(|t| zl[k * q + t].norm_sqr()).sum();
                    let new: f64 = (0..=k).map(|t| znew[k * q + t].norm_sqr()).sum();
                    let limit = (COMMAND_MAX * gt[k * q + k].re).max(old);
                    if new > limit {
                        let c = (limit / new).sqrt();
                        for t in 0..=k {
                            znew[k * q + t] *= c;
                        }
                    }
                }
                for j in 0..q {
                    if !pivot[j] {
                        continue;
                    }
                    let d = znew[j * q + j].re;
                    if d < minz {
                        let old = zl[j * q + j].re;
                        let f = if old > 0.0 {
                            (d.max(0.0) / old).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        for k in (j + 1)..q {
                            znew[k * q + j] = zl[k * q + j] * f;
                        }
                        znew[j * q + j] = C64::new(minz, 0.0);
                    }
                }

                // The drive cap: halve the step until every drive PSD is inside it.
                let old = last.diagonal(line);
                let top = old.iter().copied().fold(0.0f64, f64::max);
                let floor = 1e-6 * top;
                let mut alpha = 1.0;
                let mut accepted = false;
                for _ in 0..=HALVINGS {
                    for idx in 0..q * q {
                        ztry[idx] = zl[idx] + (znew[idx] - zl[idx]) * alpha;
                    }
                    if top <= 0.0 {
                        accepted = true;
                        break;
                    }
                    matmul_into(&map.a, &ztry[..q * q], nd, q, q, &mut b[..nd * q]);
                    row_power(&b[..nd * q], nd, q, &mut power);
                    let inside = power.iter().zip(&old).all(|(p, o)| {
                        let ratio = p / o.max(floor);
                        ratio <= psd_up && ratio >= psd_down
                    });
                    if inside {
                        accepted = true;
                        break;
                    }
                    alpha *= 0.5;
                }
                if accepted {
                    zl.copy_from_slice(&ztry[..q * q]);
                }
            }
            clamped
        });
        let clamped: usize = counts.into_iter().sum();
        let (drive, condition, kept) = self.drive(frf)?;
        let correction_db = largest_change_db(&last, &drive, self.band);
        self.last = Some(drive.clone());
        Ok(Step {
            drive,
            condition,
            kept,
            correction_db,
            clamped,
        })
    }
}

impl Step {
    /// The loop's outputs: the drive and the diagnostics `condition`, `kept`,
    /// `correction_db` and, when `clamped` is declared, `clamped`.
    pub fn outputs(self, with_clamped: bool) -> hertzforge_sdm::law::MimoLoopOutputs {
        use hertzforge_sdm::law::{DiagnosticData, DiagnosticValue};
        let value = |key: &str, data| DiagnosticValue {
            key: key.into(),
            data,
        };
        let mut diagnostics = vec![
            value("condition", DiagnosticData::PerLine(self.condition)),
            value("kept", DiagnosticData::PerLine(self.kept)),
            value("correction_db", DiagnosticData::Scalar(self.correction_db)),
        ];
        if with_clamped {
            diagnostics.push(value(
                "clamped",
                DiagnosticData::Scalar(self.clamped as f64),
            ));
        }
        hertzforge_sdm::law::MimoLoopOutputs {
            drive: self.drive,
            diagnostics,
        }
    }
}
