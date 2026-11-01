//! Abscissa (independent axis) semantics: grids, spacing detection, and the
//! bin-edge rule that makes conservative rebinning honest.

use serde::{Deserialize, Serialize};

/// Relative tolerance for spacing detection.
const SPACING_REL_TOL: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GridSpacing {
    Linear,
    Log,
    Irregular,
}

/// A function's independent axis. `Even` is linear by construction; log and
/// irregular grids arrive as `Explicit` and are classified by
/// `detect_spacing` - which makes the log edge branch REACHABLE (an earlier
/// design's was dead code because no log abscissa existed in the type system).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Abscissa {
    Even { start: f64, step: f64, n: usize },
    Explicit(Vec<f64>),
}

impl Abscissa {
    pub fn len(&self) -> usize {
        match self {
            Abscissa::Even { n, .. } => *n,
            Abscissa::Explicit(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn first(&self) -> Option<f64> {
        match self {
            Abscissa::Even { start, n, .. } => (*n > 0).then_some(*start),
            Abscissa::Explicit(v) => v.first().copied(),
        }
    }

    pub fn last(&self) -> Option<f64> {
        match self {
            Abscissa::Even { start, step, n } => {
                (*n > 0).then(|| start + step * ((*n as f64) - 1.0))
            }
            Abscissa::Explicit(v) => v.last().copied(),
        }
    }

    /// Materialize the axis values. `x[i] = start + i*step` for `Even` - computed
    /// per index, never accumulated (accumulation grows a floating-point tail that
    /// changes the point count downstream).
    pub fn values(&self) -> Vec<f64> {
        match self {
            Abscissa::Even { start, step, n } => (0..*n).map(|i| start + step * i as f64).collect(),
            Abscissa::Explicit(v) => v.clone(),
        }
    }

    pub fn spacing(&self) -> GridSpacing {
        match self {
            Abscissa::Even { .. } => GridSpacing::Linear,
            Abscissa::Explicit(v) => detect_spacing(v),
        }
    }

    /// Bin edges for integral-preserving rebinning. Returns `None` for
    /// `Irregular` spacing - conservative rebin REFUSES irregular grids and
    /// the caller downgrades to point interpolation with a method-line
    /// disclosure.
    ///
    /// Rules:
    /// - Linear: arithmetic midpoints; end bins extend half a step outward.
    ///   DC convention: when the grid starts EXACTLY at 0, the first edge
    ///   is 0 (bin 0 spans [0, step/2]) - a spectrum's DC bin has no negative
    ///   frequency to own.
    /// - Log: geometric midpoints sqrt(x_i * x_{i+1}); end bins extend by the
    ///   grid ratio symmetrically (first edge x0/sqrt(r), last xn*sqrt(r)).
    /// - n == 1: a single point has no defensible width - `None`.
    pub fn edges(&self) -> Option<Vec<f64>> {
        let x = self.values();
        if x.len() < 2 {
            return None;
        }
        match self.spacing() {
            GridSpacing::Linear => {
                let step = x[1] - x[0];
                let mut e = Vec::with_capacity(x.len() + 1);
                let first = x[0] - step / 2.0;
                e.push(if x[0] == 0.0 { 0.0 } else { first });
                for w in x.windows(2) {
                    e.push((w[0] + w[1]) / 2.0);
                }
                e.push(x[x.len() - 1] + step / 2.0);
                Some(e)
            }
            GridSpacing::Log => {
                let r = x[1] / x[0];
                let sr = r.sqrt();
                let mut e = Vec::with_capacity(x.len() + 1);
                e.push(x[0] / sr);
                for w in x.windows(2) {
                    e.push((w[0] * w[1]).sqrt());
                }
                e.push(x[x.len() - 1] * sr);
                Some(e)
            }
            GridSpacing::Irregular => None,
        }
    }
}

/// Classify an explicit axis. Linear when successive differences agree to
/// `SPACING_REL_TOL` (relative to the mean difference); else Log when all
/// values are positive and successive RATIOS agree to the tolerance; else
/// Irregular. Two points default to Linear (ratio evidence is ambiguous).
pub fn detect_spacing(x: &[f64]) -> GridSpacing {
    if x.len() < 3 {
        return GridSpacing::Linear;
    }
    let diffs: Vec<f64> = x.windows(2).map(|w| w[1] - w[0]).collect();
    let mean_d = diffs.iter().sum::<f64>() / diffs.len() as f64;
    if mean_d != 0.0
        && diffs
            .iter()
            .all(|d| ((d - mean_d) / mean_d).abs() <= SPACING_REL_TOL)
    {
        return GridSpacing::Linear;
    }
    if x[0] > 0.0 {
        let ratios: Vec<f64> = x.windows(2).map(|w| w[1] / w[0]).collect();
        let mean_r = ratios.iter().sum::<f64>() / ratios.len() as f64;
        if mean_r > 0.0
            && ratios
                .iter()
                .all(|r| ((r - mean_r) / mean_r).abs() <= SPACING_REL_TOL)
        {
            return GridSpacing::Log;
        }
    }
    GridSpacing::Irregular
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_values_no_accumulation_tail() {
        // 25.0 + 101 * 0.1: the classic count-off-by-one trap.
        let a = Abscissa::Even {
            start: 25.0,
            step: 0.1,
            n: 101,
        };
        let v = a.values();
        assert_eq!(v.len(), 101);
        assert!((v[100] - 35.0).abs() < 1e-9);
    }

    #[test]
    fn detects_linear_log_irregular() {
        let lin: Vec<f64> = (0..64).map(|i| 2.5 * i as f64).collect();
        assert_eq!(detect_spacing(&lin), GridSpacing::Linear);
        let log: Vec<f64> = (0..48).map(|i| 10.0 * 1.06f64.powi(i)).collect();
        assert_eq!(detect_spacing(&log), GridSpacing::Log);
        let irr = vec![1.0, 2.0, 3.5, 9.0, 11.0];
        assert_eq!(detect_spacing(&irr), GridSpacing::Irregular);
    }

    #[test]
    fn linear_edges_widths_sum_to_span_plus_end_halves() {
        let a = Abscissa::Even {
            start: 5.0,
            step: 2.0,
            n: 10,
        };
        let e = a.edges().unwrap();
        assert_eq!(e.len(), 11);
        let total: f64 = e.windows(2).map(|w| w[1] - w[0]).sum();
        assert!((total - 2.0 * 10.0).abs() < 1e-9, "n*step total width");
        for w in e.windows(2) {
            assert!((w[1] - w[0] - 2.0).abs() < 1e-9);
        }
    }

    #[test]
    fn dc_bin_owns_zero_to_half_step() {
        let a = Abscissa::Even {
            start: 0.0,
            step: 4.0,
            n: 8,
        };
        let e = a.edges().unwrap();
        assert_eq!(e[0], 0.0);
        assert!((e[1] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn log_edges_are_geometric_and_reachable() {
        // The reachability gate: a LOG fixture must take the log branch.
        let x: Vec<f64> = (0..24).map(|i| 20.0 * 2f64.powf(i as f64 / 6.0)).collect();
        let a = Abscissa::Explicit(x.clone());
        assert_eq!(a.spacing(), GridSpacing::Log);
        let e = a.edges().unwrap();
        assert_eq!(e.len(), x.len() + 1);
        for (i, xi) in x.iter().enumerate() {
            assert!(e[i] < *xi && *xi < e[i + 1], "bin {i} brackets its center");
            // Geometric midpoint property: e[i+1]/x[i] == x[i]/e[i] within tol.
            let lo = xi / e[i];
            let hi = e[i + 1] / xi;
            assert!((lo - hi).abs() / hi < 1e-9, "bin {i} geometric symmetry");
        }
    }

    #[test]
    fn irregular_refuses_edges() {
        let a = Abscissa::Explicit(vec![1.0, 2.0, 3.5, 9.0]);
        assert_eq!(a.edges(), None);
    }

    #[test]
    fn single_point_refuses_edges() {
        assert_eq!(
            Abscissa::Even {
                start: 1.0,
                step: 1.0,
                n: 1
            }
            .edges(),
            None
        );
    }
}
