//! What the floor law is shown on a paused line: the fill rule (design D7,
//! theory 2.6).
//!
//! The frozen `random-mimo` world has no mask, so every line the law is given
//! must hold a Hermitian, positive semi-definite response. Two rules, by id:
//!
//! - `no_error` (default): the reference at the level, so the law sees no
//!   error and its drive on that line stays where it was.
//! - `model_fill`: the neighbours' error carried through the model as a
//!   congruence, log-Euclidean between the nearest clean neighbours either
//!   side. With `n <= m` (C3): `G_pred = Hhat S Hhat^H`,
//!   `X = inv(G_pred) # G_meas` at each neighbour, `X` interpolated,
//!   `G_fill = X G_pred X`. With `n > m` (C4): `S_eff = Z G_meas Z^H`,
//!   `X_d = inv(S) # S_eff` at each neighbour, interpolated,
//!   `G_fill = Hhat X_d S X_d Hhat^H`. A line falls back to `no_error` where
//!   a neighbour is missing, the gap is wider than `max_gap_lines`, or a
//!   matrix to be inverted is worse conditioned than `max_condition`.

use crate::mat::{inv_geometric_mean, log_euclidean, CMat, Z_REGULARIZATION};
use hertzforge_mixed::LawPick;
use hertzforge_sdm::{HermitianStack, MatrixStack};

/// What a fill rule is given each loop.
#[derive(Debug, Clone, Copy)]
pub struct FillInput<'a> {
    /// The measured estimate on every line, `n` by `n`.
    pub measured: &'a HermitianStack,
    /// Whether each line is clean (read this loop).
    pub clean: &'a [bool],
    /// The reference at the level in force, `n` by `n`.
    pub reference: &'a HermitianStack,
    /// The model `Hhat`, `n` by `m`, every line.
    pub model: &'a MatrixStack,
    /// The drive as played over the estimate, `m` by `m`, every line.
    pub drive: &'a HermitianStack,
    /// The controlled band `[lo, hi)`, lines.
    pub band: (usize, usize),
}

/// What a fill did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillReport {
    /// Lines given the model's fill.
    pub model_lines: usize,
    /// Lines given the reference (the rule's own choice, or the fallback).
    pub reference_lines: usize,
    /// Of those, lines that fell back from the model's fill.
    pub fallback_lines: usize,
}

/// A fill rule (design D7): what a paused line shows the floor law.
pub trait FillRule: Send {
    /// The rule's id.
    fn id(&self) -> &'static str;
    /// Write the law's response into `out` (`n` by `n`, every line): the
    /// measurement on clean lines, the fill on every other line of the band;
    /// lines outside the band are copied from the measurement.
    fn fill(&mut self, input: &FillInput<'_>, out: &mut HermitianStack) -> FillReport;
}

/// `no_error`: the reference at the level on every paused line.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoError;

impl NoError {
    /// The rule's id.
    pub const ID: &'static str = "no_error";
}

fn copy_line(from: &HermitianStack, l: usize, to: &mut HermitianStack) {
    let n = from.n();
    for r in 0..n {
        for c in r..n {
            to.set(l, r, c, from.get(l, r, c));
        }
    }
}

impl FillRule for NoError {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn fill(&mut self, input: &FillInput<'_>, out: &mut HermitianStack) -> FillReport {
        let mut rep = FillReport::default();
        for l in 0..input.measured.lines() {
            let in_band = l >= input.band.0 && l < input.band.1;
            if in_band && !input.clean.get(l).copied().unwrap_or(false) {
                copy_line(input.reference, l, out);
                rep.reference_lines += 1;
            } else {
                copy_line(input.measured, l, out);
            }
        }
        rep
    }
}

/// Default largest condition number a fill will invert.
pub const DEFAULT_MAX_CONDITION: f64 = 1e8;

/// `model_fill`: the model-carried fill (C3 for `n <= m`, C4 for `n > m`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFill {
    /// The widest gap filled, lines (0: any). Wider gaps fall back.
    pub max_gap_lines: usize,
    /// The largest condition number inverted (`max_condition`).
    pub max_condition: f64,
}

impl Default for ModelFill {
    fn default() -> Self {
        ModelFill {
            max_gap_lines: 0,
            max_condition: DEFAULT_MAX_CONDITION,
        }
    }
}

impl ModelFill {
    /// The rule's id.
    pub const ID: &'static str = "model_fill";

    /// The correction at a clean line: `X` (C3) or `X_d` (C4), with the
    /// matrices it is applied to. `None` where ill-conditioned.
    fn correction(&self, input: &FillInput<'_>, l: usize) -> Option<CMat> {
        let n = input.measured.n();
        let m = input.drive.n();
        let h = CMat::from_stack(input.model, l);
        let s = CMat::from_hermitian(input.drive, l);
        let g = CMat::from_hermitian(input.measured, l);
        let (a, b) = if n <= m {
            (s.congruence(&h), g)
        } else {
            let z = h.pinv(Z_REGULARIZATION).ok()?;
            (s, g.congruence(&z))
        };
        if !(a.condition().ok()? <= self.max_condition && b.condition().ok()? <= self.max_condition)
        {
            return None;
        }
        let x = inv_geometric_mean(&a, &b).ok()?;
        x.is_finite().then_some(x)
    }

    /// Apply an interpolated correction on paused line `l`.
    fn apply(&self, input: &FillInput<'_>, l: usize, x: &CMat) -> Option<CMat> {
        let n = input.measured.n();
        let m = input.drive.n();
        let h = CMat::from_stack(input.model, l);
        let s = CMat::from_hermitian(input.drive, l);
        let out = if n <= m {
            let pred = s.congruence(&h);
            if pred.condition().ok()? > self.max_condition {
                return None;
            }
            pred.congruence(x)
        } else {
            if s.condition().ok()? > self.max_condition {
                return None;
            }
            s.congruence(x).congruence(&h)
        };
        out.is_finite().then_some(out)
    }
}

impl FillRule for ModelFill {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn fill(&mut self, input: &FillInput<'_>, out: &mut HermitianStack) -> FillReport {
        let mut rep = FillReport::default();
        let lines = input.measured.lines();
        let (lo, hi) = (input.band.0, input.band.1.min(lines));
        let clean = |l: usize| input.clean.get(l).copied().unwrap_or(false);
        for l in 0..lines {
            if !(l >= lo && l < hi) || clean(l) {
                copy_line(input.measured, l, out);
            }
        }
        let mut l = lo;
        while l < hi {
            if clean(l) {
                l += 1;
                continue;
            }
            // The gap [l, e).
            let mut e = l;
            while e < hi && !clean(e) {
                e += 1;
            }
            let a = (l > lo).then(|| l - 1);
            let b = (e < hi).then_some(e);
            let gap_ok = self.max_gap_lines == 0 || e - l <= self.max_gap_lines;
            let xa = a.filter(|_| gap_ok).and_then(|a| self.correction(input, a));
            let xb = b.filter(|_| gap_ok).and_then(|b| self.correction(input, b));
            let logs = match (&xa, &xb) {
                (Some(xa), Some(xb)) => Some((xa, xb)),
                _ => None,
            };
            for k in l..e {
                let filled = logs.and_then(|(xa, xb)| {
                    let (a, b) = (a.unwrap(), b.unwrap());
                    let t = (k - a) as f64 / (b - a) as f64;
                    let x = log_euclidean(xa, xb, t).ok()?;
                    self.apply(input, k, &x)
                });
                match filled {
                    Some(g) => {
                        g.store_hermitian(out, k);
                        rep.model_lines += 1;
                    }
                    None => {
                        copy_line(input.reference, k, out);
                        rep.reference_lines += 1;
                        rep.fallback_lines += 1;
                    }
                }
            }
            l = e;
        }
        rep
    }
}

/// Build a fill rule from `{id, params}` (`no_error`; `model_fill` with
/// `max_gap_lines` and `max_condition`). An unknown id or parameter is
/// refused with plain words; the configuration's validation names an
/// unknown id `unknown_fill_rule` before this is reached.
pub fn fill_rule(pick: &LawPick) -> Result<Box<dyn FillRule>, String> {
    match pick.id.as_str() {
        NoError::ID => {
            if let Some(k) = pick.params.keys().next() {
                return Err(format!("no_error takes no parameter ({k})"));
            }
            Ok(Box::new(NoError))
        }
        ModelFill::ID => {
            let mut r = ModelFill::default();
            for (k, v) in &pick.params {
                let x = v
                    .as_f64()
                    .ok_or_else(|| format!("fill parameter {k} is a number"))?;
                match k.as_str() {
                    "max_gap_lines" if x >= 0.0 && x.fract() == 0.0 => r.max_gap_lines = x as usize,
                    "max_condition" if x.is_finite() && x >= 1.0 => r.max_condition = x,
                    "max_gap_lines" | "max_condition" => {
                        return Err(format!("fill parameter {k} out of range"))
                    }
                    _ => return Err(format!("unknown fill parameter {k}")),
                }
            }
            Ok(Box::new(r))
        }
        other => Err(format!("unknown fill rule {other:?}")),
    }
}
