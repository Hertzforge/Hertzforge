//! Refreshing the model along the operating direction (`Derivations.md` D3).
//!
//! One pulse gives one drive vector `D` and one response vector `Y` a line, so
//! it fixes the plant only along `D`. The refresh moves the model the least
//! amount (Frobenius norm) that makes it reproduce that pulse, blended by `mu`:
//!
//! `Hh+ = Hh + mu (Y - Hh D) D^H / (D^H D)`
//!
//! At `mu = 1`, `Hh+ D = Y` exactly. Lines where the pulse's drive energy is
//! below `floor_rel` times the largest line's are left untouched, so noise on
//! lines the pulse did not drive is never learned.

use crate::{MatrixStack, C64};

/// Refresh `model` in place from a pulse's drive and response spectra (both
/// normalized to the same level). Returns how many lines were refreshed.
pub fn refresh_along_drive(
    model: &mut MatrixStack,
    drives: &[Vec<C64>],
    responses: &[Vec<C64>],
    mu: f64,
    floor_rel: f64,
) -> Result<usize, String> {
    let (lines, n_c, n_d) = model.dims();
    if drives.len() != n_d || drives.iter().any(|d| d.len() != lines) {
        return Err("the drives must be one spectrum per drive on the model's lines".into());
    }
    if responses.len() != n_c || responses.iter().any(|d| d.len() != lines) {
        return Err("the responses must be one spectrum per control on the model's lines".into());
    }
    if !(mu > 0.0 && mu <= 1.0) {
        return Err(format!("the refresh blend {mu} must be in (0, 1]"));
    }
    let energy: Vec<f64> = (0..lines)
        .map(|f| drives.iter().map(|d| d[f].norm_sqr()).sum())
        .collect();
    let peak = energy.iter().fold(0.0f64, |a, &b| a.max(b));
    if peak <= 0.0 {
        return Ok(0);
    }
    let floor = floor_rel.max(0.0) * peak;
    let mut count = 0;
    let mut resid = vec![C64::new(0.0, 0.0); n_c];
    for f in 0..lines {
        let e = energy[f];
        if e <= floor || e <= 0.0 {
            continue;
        }
        let line = model.line_mut(f);
        for (r, out) in resid.iter_mut().enumerate() {
            let mut hd = C64::new(0.0, 0.0);
            for c in 0..n_d {
                hd += line[r * n_d + c] * drives[c][f];
            }
            *out = responses[r][f] - hd;
        }
        for r in 0..n_c {
            let k = resid[r] * (mu / e);
            for c in 0..n_d {
                line[r * n_d + c] += k * drives[c][f].conj();
            }
        }
        count += 1;
    }
    Ok(count)
}
