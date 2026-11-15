//! Delay tools: the delay of one record against another by cross-correlation,
//! a delay applied to a spectrum as a linear phase, and the least-effort
//! choice of a delay per control before the first pulse.

use crate::spectrum::{irfft, rfft};
use crate::{MatrixStack, C64};

/// The delay `d` (seconds) for which `y(t)` best matches `r(t - d)`: the peak
/// of the circular cross-correlation of the two records zero-padded to twice
/// their length, refined below a sample by a parabola through the peak and its
/// two neighbours. A positive delay means `y` lags `r`.
pub fn estimate_delay(y: &[f64], r: &[f64], fs_hz: f64) -> Result<f64, String> {
    let n = y.len();
    if r.len() != n {
        return Err("the two records must have the same length".into());
    }
    let m = 2 * n;
    let mut yp = y.to_vec();
    yp.resize(m, 0.0);
    let mut rp = r.to_vec();
    rp.resize(m, 0.0);
    let ys = rfft(&yp)?;
    let rs = rfft(&rp)?;
    let cs: Vec<C64> = ys.iter().zip(&rs).map(|(a, b)| a * b.conj()).collect();
    let c = irfft(&cs, m)?;
    let (mut k, mut best) = (0usize, -1.0f64);
    for (i, v) in c.iter().enumerate() {
        if v.abs() > best {
            best = v.abs();
            k = i;
        }
    }
    if best <= 0.0 {
        return Ok(0.0);
    }
    let sign = c[k].signum();
    let ym = c[(k + m - 1) % m] * sign;
    let y0 = c[k] * sign;
    let yp1 = c[(k + 1) % m] * sign;
    let den = ym - 2.0 * y0 + yp1;
    let frac = if den < 0.0 {
        (0.5 * (ym - yp1) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let lag = if k < n { k as f64 } else { k as f64 - m as f64 };
    Ok((lag + frac) / fs_hz)
}

/// `x(f) exp(-j w tau)`: the spectrum of the record delayed by `tau` seconds.
pub fn apply_delay(x: &[C64], omegas: &[f64], tau: f64) -> Vec<C64> {
    x.iter()
        .zip(omegas)
        .map(|(v, w)| v * C64::from_polar(1.0, -w * tau))
        .collect()
}

/// Settings of the least-effort delay search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeastEffort {
    /// Each delay is held inside `+/- bound_s` of control 0's, seconds.
    pub bound_s: f64,
    /// Weight of the drive energy against the unreachable energy (both
    /// normalized to their values at zero delay; see [`least_effort_delays`]).
    pub lambda: f64,
    /// Grid points across the bound in the coarse pass (odd).
    pub grid: usize,
    /// Coordinate-descent sweeps of the coarse pass.
    pub sweeps: usize,
}

impl Default for LeastEffort {
    fn default() -> Self {
        LeastEffort {
            bound_s: 1.0e-3,
            lambda: 0.1,
            grid: 41,
            sweeps: 3,
        }
    }
}

/// The cost [`least_effort_delays`] minimizes, at the delays `tau`:
///
/// `J = sum_f |(I - Hh G) R(tau)|^2 / U0 + lambda sum_f |G R(tau)|^2 / E0`
///
/// with `R_c(f; tau) = R_c(f) exp(-j w tau_c)` and `U0`, `E0` the two sums at
/// zero delay (each replaced by the target energy when it is zero). Masked
/// lines only.
pub struct EffortCost<'a> {
    model: &'a MatrixStack,
    inverse: &'a MatrixStack,
    mask: &'a [bool],
    reference: &'a [Vec<C64>],
    omegas: &'a [f64],
    lambda: f64,
    u0: f64,
    e0: f64,
}

impl<'a> EffortCost<'a> {
    /// The cost for a model `Hh` (`n_c x n_d`), its inverse `G`, the masked
    /// lines and the reference spectra.
    pub fn new(
        model: &'a MatrixStack,
        inverse: &'a MatrixStack,
        mask: &'a [bool],
        reference: &'a [Vec<C64>],
        omegas: &'a [f64],
        lambda: f64,
    ) -> Self {
        let mut c = EffortCost {
            model,
            inverse,
            mask,
            reference,
            omegas,
            lambda,
            u0: 1.0,
            e0: 1.0,
        };
        let (u, e) = c.parts(&vec![0.0; reference.len()]);
        let r_energy: f64 = reference
            .iter()
            .flat_map(|r| {
                r.iter()
                    .zip(mask)
                    .filter(|(_, &m)| m)
                    .map(|(v, _)| v.norm_sqr())
            })
            .sum();
        let fallback = if r_energy > 0.0 { r_energy } else { 1.0 };
        c.u0 = if u > 1e-12 * fallback { u } else { fallback };
        c.e0 = if e > 0.0 { e } else { 1.0 };
        c
    }

    /// The unreachable energy and the drive energy at `tau`, unnormalized.
    pub fn parts(&self, tau: &[f64]) -> (f64, f64) {
        let (lines, n_c, n_d) = self.model.dims();
        let mut rt = vec![C64::new(0.0, 0.0); n_c];
        let mut d = vec![C64::new(0.0, 0.0); n_d];
        let (mut unreach, mut drive) = (0.0, 0.0);
        for f in 0..lines {
            if !self.mask[f] {
                continue;
            }
            for c in 0..n_c {
                rt[c] = self.reference[c][f] * C64::from_polar(1.0, -self.omegas[f] * tau[c]);
            }
            let g = self.inverse.line(f);
            for (i, di) in d.iter_mut().enumerate() {
                let mut acc = C64::new(0.0, 0.0);
                for c in 0..n_c {
                    acc += g[i * n_c + c] * rt[c];
                }
                *di = acc;
                drive += acc.norm_sqr();
            }
            let h = self.model.line(f);
            for c in 0..n_c {
                let mut acc = C64::new(0.0, 0.0);
                for i in 0..n_d {
                    acc += h[c * n_d + i] * d[i];
                }
                unreach += (rt[c] - acc).norm_sqr();
            }
        }
        (unreach, drive)
    }

    /// The normalized cost at `tau`.
    pub fn cost(&self, tau: &[f64]) -> f64 {
        let (u, e) = self.parts(tau);
        u / self.u0 + self.lambda * e / self.e0
    }
}

/// Delays per control (control 0 held at zero) that minimize [`EffortCost`]:
/// coordinate descent over a grid across `+/- bound_s`, then two passes of a
/// grid ten times finer around each delay.
pub fn least_effort_delays(cost: &EffortCost<'_>, settings: LeastEffort) -> Vec<f64> {
    let n_c = cost.reference.len();
    let mut tau = vec![0.0; n_c];
    if n_c < 2 || settings.bound_s <= 0.0 {
        return tau;
    }
    let grid = settings.grid.max(3) | 1;
    let step = 2.0 * settings.bound_s / (grid - 1) as f64;
    let mut best = cost.cost(&tau);
    let search = |tau: &mut Vec<f64>, best: &mut f64, centre_span: Option<f64>, step: f64| {
        for c in 1..n_c {
            let (lo, count) = match centre_span {
                None => (-settings.bound_s, grid),
                Some(span) => (tau[c] - span, (2.0 * span / step).round() as usize + 1),
            };
            let keep = tau[c];
            let mut pick = keep;
            for i in 0..count {
                let t = (lo + i as f64 * step).clamp(-settings.bound_s, settings.bound_s);
                tau[c] = t;
                let j = cost.cost(tau);
                if j < *best {
                    *best = j;
                    pick = t;
                }
            }
            tau[c] = pick;
        }
    };
    for _ in 0..settings.sweeps.max(1) {
        search(&mut tau, &mut best, None, step);
    }
    let mut s = step;
    for _ in 0..2 {
        let fine = s / 10.0;
        search(&mut tau, &mut best, Some(s), fine);
        s = fine;
    }
    tau
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_apply_delay_shifts_a_record() {
        let n = 256;
        let fs = 256.0;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 - 40.0;
                (-t * t / 20.0).exp()
            })
            .collect();
        let w = crate::spectrum::omegas(fs, n);
        let y = irfft(&apply_delay(&rfft(&x).unwrap(), &w, 5.0 / fs), n).unwrap();
        for i in 10..200 {
            assert!((y[i] - x[i - 5]).abs() < 1e-9);
        }
        let d = estimate_delay(&y, &x, fs).unwrap();
        assert!((d - 5.0 / fs).abs() < 1e-3 / fs, "{d}");
    }
}
