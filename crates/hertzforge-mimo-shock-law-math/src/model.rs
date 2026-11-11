//! The law's inverse of the plant model: the Tikhonov-regularized
//! pseudo-inverse per line, with the regularization relative to the line's
//! largest singular value, and no drive at all on lines outside the mask.

use crate::{MatrixStack, C64};
use hertzforge_sdm::linalg::{pinv, Regularization};

/// `G(f) = (Hh^H Hh + (lambda_rel s_max)^2 I)^-1 Hh^H` on every masked line,
/// zero elsewhere. `Hh` is `n_c x n_d` per line; `G` is `n_d x n_c`.
pub fn regularized_inverse(
    model: &MatrixStack,
    mask: &[bool],
    lambda_rel: f64,
) -> Result<MatrixStack, String> {
    if mask.len() != model.lines() {
        return Err("the mask must have one flag per line".into());
    }
    if !(lambda_rel >= 0.0 && lambda_rel.is_finite()) {
        return Err(format!(
            "regularization {lambda_rel} must be finite and not negative"
        ));
    }
    let (mut g, _) =
        pinv(model, Regularization::Tikhonov { lambda_rel }).map_err(|e| e.to_string())?;
    for (f, &on) in mask.iter().enumerate() {
        if !on {
            g.line_mut(f).fill(C64::new(0.0, 0.0));
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_inverse_is_tikhonov_relative_and_masked() {
        // One line, diagonal 2 x 2 with singular values 4 and 1.
        let mut h = MatrixStack::zeros(2, 2, 2);
        for f in 0..2 {
            h.set(f, 0, 0, C64::new(4.0, 0.0));
            h.set(f, 1, 1, C64::new(1.0, 0.0));
        }
        let g = regularized_inverse(&h, &[true, false], 0.5).unwrap();
        // mu = 0.5 * 4 = 2: factors s / (s^2 + 4).
        assert!((g.get(0, 0, 0).re - 4.0 / 20.0).abs() < 1e-12);
        assert!((g.get(0, 1, 1).re - 1.0 / 5.0).abs() < 1e-12);
        assert!(g.line(1).iter().all(|v| v.norm() == 0.0));
    }
}
