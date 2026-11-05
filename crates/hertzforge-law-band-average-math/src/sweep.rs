//! The automatic regularization: the sweep over kappa at the first solve.

use crate::fit::real_dot;
use crate::{median, BandAverage, BandDrive, Kappa, KappaRule};
use hertzforge_sdm::C64;

/// `10^x` for `count` values of `x` evenly spaced from `start` to `stop`, both included.
pub(crate) fn logspace(start: f64, stop: f64, count: usize) -> Vec<f64> {
    let step = (stop - start) / (count - 1) as f64;
    (0..count)
        .map(|i| {
            let x = if i + 1 == count {
                stop
            } else {
                start + i as f64 * step
            };
            10f64.powf(x)
        })
        .collect()
}

/// `f"{x:.{precision}g}"`: the number of significant digits, no trailing zeros.
pub(crate) fn format_g(x: f64, precision: usize) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let exp = format!("{:.*e}", precision.saturating_sub(1), x);
    let (mantissa, power) = exp.split_once('e').expect("formatted with an exponent");
    let power: i32 = power.parse().expect("an integer exponent");
    let strip = |s: &str| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if power < -4 || power >= precision as i32 {
        let sign = if power < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), power.abs())
    } else {
        let decimals = (precision as i32 - 1 - power).max(0) as usize;
        strip(&format!("{x:.decimals$}"))
    }
}

/// `np.gradient` on a non-uniform axis: second order inside, first order at the ends.
fn gradient(f: &[f64], t: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut g = vec![0.0; n];
    g[0] = (f[1] - f[0]) / (t[1] - t[0]);
    g[n - 1] = (f[n - 1] - f[n - 2]) / (t[n - 1] - t[n - 2]);
    for i in 1..n - 1 {
        let (d1, d2) = (t[i] - t[i - 1], t[i + 1] - t[i]);
        let a = -d2 / (d1 * (d1 + d2));
        let b = (d2 - d1) / (d1 * d2);
        let c = d1 / (d2 * (d1 + d2));
        g[i] = a * f[i - 1] + b * f[i] + c * f[i + 1];
    }
    g
}

/// The L-curve corner: greatest curvature of (log residual, log penalty) along
/// `t = log kappa`. Kappa rising moves the curve left then up, a clockwise turn,
/// so the corner is the most negative signed curvature.
fn l_curve_corner(t: &[f64], residual: &[f64], penalty: &[f64]) -> usize {
    let tiny = f64::MIN_POSITIVE;
    let rho: Vec<f64> = residual.iter().map(|x| x.max(tiny).ln()).collect();
    let nu: Vec<f64> = penalty.iter().map(|x| x.max(tiny).ln()).collect();
    let (r1, n1) = (gradient(&rho, t), gradient(&nu, t));
    let (r2, n2) = (gradient(&r1, t), gradient(&n1, t));
    let mut best = 0;
    let mut least = f64::INFINITY;
    for i in 0..t.len() {
        let curvature =
            (r1[i] * n2[i] - n1[i] * r2[i]) / (r1[i] * r1[i] + n1[i] * n1[i]).powf(1.5).max(tiny);
        if curvature < least {
            least = curvature;
            best = i;
        }
    }
    best
}

/// The index of the first smallest value among `candidates`.
fn first_least(candidates: impl Iterator<Item = usize>, value: impl Fn(usize) -> f64) -> usize {
    let mut best = usize::MAX;
    let mut least = f64::INFINITY;
    for i in candidates {
        if best == usize::MAX || value(i) < least {
            best = i;
            least = value(i);
        }
    }
    best
}

impl BandAverage {
    /// Solves every band at each kappa of the sweep, PSD re-fit included, and
    /// records each band's predicted mean |dB| error and drive energy through its
    /// own forward model. Freezes one kappa per band by the configured rule. The
    /// frozen thresholds serve every later solve; the sweep never runs in the loop.
    pub(crate) fn resolve_auto_kappa(&mut self) -> Result<(), String> {
        let wide = self.params.kappa_rule != KappaRule::Knee;
        let grid = if wide {
            logspace(0.0, 5.0, 81)
        } else {
            logspace(1.0, 5.0, 65)
        };
        let n_bands = self.n_bands();
        let d = self.n_drives;
        let n_ctrl = self.n_ctrl;
        let at = |k: usize, b: usize| k * n_bands + b;
        let mut err = vec![f64::NAN; grid.len() * n_bands];
        let mut energy = vec![0.0; grid.len() * n_bands];
        let mut residual = vec![0.0; grid.len() * n_bands];
        let mut penalty = vec![0.0; grid.len() * n_bands];
        let mut solved: Vec<BandDrive> = Vec::with_capacity(grid.len());
        for (k, &kappa) in grid.iter().enumerate() {
            let output = self.solve_bands(Kappa::Single(kappa))?;
            for (b, p) in output.iter().enumerate() {
                if self.factored[b].is_none() {
                    continue;
                }
                let spec = self.spec_row(b);
                let a = &self.a_bands[b][..n_ctrl * d * d];
                let r: Vec<f64> = a.chunks_exact(d * d).map(|row| real_dot(row, p)).collect();
                let (mut sum, mut count) = (0.0, 0usize);
                let mut res = 0.0;
                for (ri, si) in r.iter().zip(spec) {
                    res += (ri - si) * (ri - si);
                    if *si > 0.0 {
                        sum += (10.0 * (ri / si).max(1e-6).log10()).abs();
                        count += 1;
                    }
                }
                if count > 0 {
                    err[at(k, b)] = sum / count as f64;
                }
                let hz: f64 = self.wk[b].iter().sum();
                let tr: f64 = (0..d).map(|i| p[i * d + i].re).sum();
                energy[at(k, b)] = tr * hz;
                residual[at(k, b)] = res;
                penalty[at(k, b)] = p.iter().map(|x: &C64| x.norm_sqr()).sum();
            }
            solved.push(output);
        }
        let scored: Vec<usize> = (0..n_bands)
            .filter(|&b| (0..grid.len()).any(|k| !err[at(k, b)].is_nan()))
            .collect();
        if scored.is_empty() {
            self.cond_number = Some(300.0);
            return Ok(());
        }

        let mut chosen = vec![0usize; n_bands];
        let reduction = self.params.drive_reduction;
        if self.params.kappa_rule == KappaRule::Front {
            let n_b: Vec<f64> = scored
                .iter()
                .map(|&b| self.spec_row(b).iter().filter(|&&s| s > 0.0).count() as f64)
                .collect();
            let n_total: f64 = n_b.iter().sum();
            let weighted = |k: usize, j: usize| {
                let e = err[at(k, scored[j])];
                n_b[j] * if e.is_nan() { f64::INFINITY } else { e }
            };
            // One multiplier for every band: each band then spends drive where it buys
            // the most error, and sweeping it traces the least energy for each mean error.
            let finite: f64 = (0..scored.len())
                .map(|j| weighted(0, j))
                .filter(|x| x.is_finite())
                .sum();
            let last = grid.len() - 1;
            let last_energy: f64 = scored.iter().map(|&b| energy[at(last, b)]).sum();
            let mu_ref = finite / last_energy.max(1e-300);
            let mut picks: Vec<Vec<usize>> = Vec::new();
            let (mut errs, mut energies) = (Vec::new(), Vec::new());
            for scale in logspace(-6.0, 6.0, 241) {
                let mu = mu_ref * scale;
                let pick: Vec<usize> = (0..scored.len())
                    .map(|j| {
                        first_least(0..grid.len(), |k| {
                            weighted(k, j) + mu * energy[at(k, scored[j])]
                        })
                    })
                    .collect();
                errs.push(
                    pick.iter()
                        .enumerate()
                        .map(|(j, &k)| weighted(k, j))
                        .sum::<f64>()
                        / n_total,
                );
                energies.push(
                    pick.iter()
                        .zip(&scored)
                        .map(|(&k, &b)| energy[at(k, b)])
                        .sum::<f64>(),
                );
                picks.push(pick);
            }
            let best_err = errs.iter().copied().fold(f64::INFINITY, f64::min);
            let front_i = first_least(
                (0..errs.len()).filter(|&i| errs[i] <= best_err + 0.1),
                |i| energies[i],
            );
            let mut sel_i = front_i;
            if reduction > 0.0 {
                let allowed = (1.0 - reduction) * energies[front_i];
                let cheaper: Vec<usize> = (0..errs.len())
                    .filter(|&i| energies[i] <= allowed)
                    .collect();
                if !cheaper.is_empty() {
                    sel_i = first_least(cheaper.into_iter(), |i| errs[i]);
                }
            }
            for (j, &b) in scored.iter().enumerate() {
                chosen[b] = picks[sel_i][j];
            }
            if self.params.drive_rms_limit.is_some() {
                // Every front point up to the selection, least drive first. Each is scaled
                // to the limit, and the one with the least predicted error is used.
                let mut below: Vec<usize> = (0..errs.len())
                    .filter(|&i| energies[i] <= energies[sel_i])
                    .collect();
                below.sort_by(|&x, &y| energies[x].total_cmp(&energies[y]));
                let mut front_picks: Vec<&Vec<usize>> = Vec::new();
                let mut pred = Vec::new();
                for &i in &below {
                    if front_picks.last().is_some_and(|last| **last == picks[i]) {
                        continue;
                    }
                    front_picks.push(&picks[i]);
                    let mut out: BandDrive = vec![Vec::new(); n_bands];
                    for (j, &b) in scored.iter().enumerate() {
                        out[b] = solved[picks[i][j]][b].clone();
                    }
                    pred.push(self.predicted_error(&self.limit_scaled(&out).0));
                }
                let front_i = first_least(0..pred.len(), |i| pred[i]);
                self.front_i = front_i;
                for (j, &b) in scored.iter().enumerate() {
                    chosen[b] = front_picks[front_i][j];
                }
                self.front = Some(
                    front_picks
                        .iter()
                        .map(|p| {
                            let mut v = vec![grid[0]; n_bands];
                            for (j, &b) in scored.iter().enumerate() {
                                v[b] = grid[p[j]];
                            }
                            v
                        })
                        .collect(),
                );
            }
        } else {
            let t: Vec<f64> = grid.iter().map(|x| x.ln()).collect();
            for &b in &scored {
                let column = |values: &[f64]| -> Vec<f64> {
                    (0..grid.len()).map(|k| values[at(k, b)]).collect()
                };
                let knee = if self.params.kappa_rule == KappaRule::LCurve {
                    l_curve_corner(&t, &column(&residual), &column(&penalty))
                } else {
                    let e = column(&err);
                    let best = e
                        .iter()
                        .copied()
                        .filter(|x| !x.is_nan())
                        .fold(f64::INFINITY, f64::min);
                    e.iter().position(|&x| x <= best + 0.1).unwrap_or(0)
                };
                chosen[b] = knee;
                if reduction > 0.0 {
                    let allowed = (1.0 - reduction) * energy[at(knee, b)];
                    chosen[b] = (0..=knee)
                        .rev()
                        .find(|&j| energy[at(j, b)] <= allowed)
                        .unwrap_or(0);
                }
            }
        }

        let band_kappa: Vec<f64> = chosen.iter().map(|&k| grid[k]).collect();
        let kb: Vec<f64> = scored.iter().map(|&b| band_kappa[b]).collect();
        let (low, high) = kb
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &x| {
                (l.min(x), h.max(x))
            });
        self.cond_number = Some(high);
        let rule = match self.params.kappa_rule {
            KappaRule::Front => "front",
            KappaRule::Knee => "knee",
            KappaRule::LCurve => "lcurve",
        };
        self.log.push(format!(
            "band_average: condition_number auto per band ({rule}) -> median {:.0}, range {low:.0}..{high:.0} \
             over {} bands (drive_reduction {})",
            median(&kb),
            kb.len(),
            format_g(reduction, 6)
        ));
        self.band_kappa = Some(band_kappa);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logspace_hits_its_ends() {
        let g = logspace(0.0, 5.0, 81);
        assert_eq!(g.len(), 81);
        assert_eq!(g[0], 1.0);
        assert_eq!(g[80], 1e5);
        assert!((g[16] - 10.0).abs() < 1e-12);
    }

    #[test]
    fn format_g_matches_python() {
        assert_eq!(format_g(0.7, 6), "0.7");
        assert_eq!(format_g(0.0, 6), "0");
        assert_eq!(format_g(101.0, 6), "101");
        assert_eq!(format_g(1234.5, 3), "1.23e+03");
        assert_eq!(format_g(12.34, 3), "12.3");
        assert_eq!(format_g(1.5e-7, 3), "1.5e-07");
    }

    #[test]
    fn the_gradient_is_exact_on_a_parabola_on_a_nonuniform_axis() {
        let t = [0.0, 0.5, 1.5, 2.0, 3.0];
        let f: Vec<f64> = t.iter().map(|x| x * x).collect();
        let g = gradient(&f, &t);
        for i in 1..4 {
            assert!((g[i] - 2.0 * t[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn the_corner_of_a_sharp_l_is_found() {
        // Residual falls and then holds; penalty holds and then rises.
        let t: Vec<f64> = (0..41).map(|i| i as f64 * 0.25).collect();
        let residual: Vec<f64> = t
            .iter()
            .map(|x| (1.0 + (-3.0 * (x - 5.0)).exp()).ln().exp())
            .collect();
        let penalty: Vec<f64> = t
            .iter()
            .map(|x| (1.0 + (3.0 * (x - 5.0)).exp()).ln().exp())
            .collect();
        let corner = l_curve_corner(&t, &residual, &penalty);
        assert!((t[corner] - 5.0).abs() < 1.5, "corner at {}", t[corner]);
    }
}
