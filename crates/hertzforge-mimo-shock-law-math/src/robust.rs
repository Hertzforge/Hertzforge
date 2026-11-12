//! The uncertainty-weighted inverse (`Derivations.md` D16): Tikhonov with a
//! per-line weight equal to how far the model could be from the plant there.
//!
//! `G(f) = (Hh^H Hh + mu(f)^2 I)^-1 Hh^H`,   `mu(f) = max(rho(f), lambda_rel s_max(f))`
//!
//! with `rho(f)` a radius from the model provider (the largest spectral-norm
//! change of the model over a family of mode-frequency drops). For a model
//! error bounded in spectral norm by `rho`, the worst-case least-squares
//! solution is a Tikhonov solution (El Ghaoui and Lebret 1997); taking the
//! weight equal to the radius is the audit's simplification of that map.

use crate::lsq::{hpd_solve, spectral_norm};
use crate::{MatrixStack, C64};

/// The weighted inverse on the masked lines, zero elsewhere.
pub fn weighted_inverse(
    model: &MatrixStack,
    mask: &[bool],
    lambda_rel: f64,
    radius: &[f64],
) -> Result<MatrixStack, String> {
    let (lines, n_c, n_d) = model.dims();
    if mask.len() != lines || radius.len() != lines {
        return Err("the mask and the radius must have one entry per line".into());
    }
    let zero = C64::new(0.0, 0.0);
    let mut g = MatrixStack::zeros(lines, n_d, n_c);
    let mut a = vec![zero; n_d * n_d];
    let mut rhs = vec![zero; n_d * n_c];
    for f in 0..lines {
        if !mask[f] {
            continue;
        }
        let h = model.line(f);
        let smax = spectral_norm(h, n_c, n_d);
        if smax == 0.0 {
            continue;
        }
        let mu = radius[f].max(lambda_rel * smax);
        for i in 0..n_d {
            for j in 0..n_d {
                let mut s = zero;
                for r in 0..n_c {
                    s += h[r * n_d + i].conj() * h[r * n_d + j];
                }
                a[i * n_d + j] = s;
            }
            a[i * n_d + i] += C64::new(mu * mu, 0.0);
            for r in 0..n_c {
                rhs[i * n_c + r] = h[r * n_d + i].conj();
            }
        }
        let x = hpd_solve(&a, n_d, &rhs, n_c)?;
        g.line_mut(f).copy_from_slice(&x);
    }
    Ok(g)
}
