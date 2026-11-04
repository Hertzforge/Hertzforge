//! The breakpoint builder: a PSD breakpoint table per control, and a coherence
//! and phase per pair.
//!
//! This is the single-output profile editor generalized. Each control's PSD and its
//! alarm and abort lines come from a [`BreakpointTable`] resolved onto the grid by
//! `hertzforge-breakpoints` (log-log straight segments, zero outside the table,
//! per-row dB bands or the builder's defaults). Each pair `(i, j)` has a squared
//! coherence `gamma2_ij` (0 to 1) and a phase `theta_ij` in degrees, each a
//! constant or a [`PairValue::Table`]. A pair not given takes the builder's
//! [`UnspecifiedPairs`] choice: independent (coherence 0) or fully coherent
//! (coherence 1), with zero phase.
//!
//! Per line the matrix is
//!
//! ```text
//! G_ij = sqrt(G_ii G_jj) sqrt(gamma2_ij) exp(i theta_ij),   G_ji = conj(G_ij)
//! ```
//!
//! in the `E[X X^H]` convention, so a positive `theta_01` means channel 1 lags
//! channel 0 by that angle.
//!
//! Positive semi-definiteness is not automatic for three or more channels:
//! pairwise coherences at or below 1 are necessary, not sufficient, since the
//! coherences and phases must also be mutually consistent (with unit coherence,
//! `theta_01 + theta_12` must equal `theta_02`). The build ends in
//! [`Reference::validate`], which names the lines that fail; the repair offered is
//! the eigenvalue clamp followed by a rescale to the entered PSDs, and its report
//! shows how each pair's coherence moved.
//!
//! Sources: D. O. Smallwood, "Multiple Shaker Random Vibration Control: An Update",
//! Proc. IEST 1999 (autospectra, coherence and phase as the specification form);
//! N. Hunter, K. Cross and G. Nelson, "The Cross Spectrum in Multiple Input
//! Multiple Response Vibration Testing", IMAC proceedings (the definiteness
//! condition and its repair).

use crate::{
    band_of, Method, Provenance, PsdRepairReport, Reference, ReferenceError, ReferenceParts,
    RepairPolicy, Tolerances, C64,
};
use hertzforge_breakpoints::{BreakpointTable, Envelope};
use hertzforge_sdm::HermitianStack;
use serde::{Deserialize, Serialize};

/// The control grid: line `k` at `k * df_hz`, `lines` lines from 0 Hz.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub df_hz: f64,
    pub lines: usize,
}

impl Grid {
    /// A grid, refused unless the spacing is positive and there is a line.
    pub fn new(df_hz: f64, lines: usize) -> Result<Grid, ReferenceError> {
        if !(df_hz.is_finite() && df_hz > 0.0) || lines == 0 {
            return Err(ReferenceError::Input(format!(
                "a grid needs a positive line spacing and at least one line, got {df_hz} Hz and \
                 {lines} lines"
            )));
        }
        Ok(Grid { df_hz, lines })
    }

    /// The frequency of line `k`.
    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.df_hz
    }
}

/// A pair quantity: a constant, or `(frequency in Hz, value)` points joined by
/// straight lines in frequency and held at the end values beyond the first and
/// last point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PairValue {
    Constant(f64),
    Table(Vec<(f64, f64)>),
}

impl PairValue {
    fn check(&self, what: &str) -> Result<(), ReferenceError> {
        match self {
            PairValue::Constant(v) if v.is_finite() => Ok(()),
            PairValue::Constant(v) => Err(ReferenceError::Input(format!(
                "{what} is {v}; give a finite number"
            ))),
            PairValue::Table(points) => {
                if points.is_empty() {
                    return Err(ReferenceError::Input(format!(
                        "{what} has an empty table; give at least one point or a constant"
                    )));
                }
                for (k, &(f, v)) in points.iter().enumerate() {
                    if !(f.is_finite() && v.is_finite()) {
                        return Err(ReferenceError::Input(format!(
                            "{what}, point {}: frequency and value must be numbers",
                            k + 1
                        )));
                    }
                    if k > 0 && f <= points[k - 1].0 {
                        return Err(ReferenceError::Input(format!(
                            "{what}, point {}: frequency {f} Hz is not above the point before; \
                             list the points in increasing frequency",
                            k + 1
                        )));
                    }
                }
                Ok(())
            }
        }
    }

    /// The value at frequency `f`.
    pub fn at(&self, f: f64) -> f64 {
        match self {
            PairValue::Constant(v) => *v,
            PairValue::Table(p) => {
                if f <= p[0].0 {
                    return p[0].1;
                }
                let last = p[p.len() - 1];
                if f >= last.0 {
                    return last.1;
                }
                let k = p.partition_point(|&(x, _)| x <= f);
                let (f0, v0) = p[k - 1];
                let (f1, v1) = p[k];
                v0 + (v1 - v0) * (f - f0) / (f1 - f0)
            }
        }
    }

    fn values(&self) -> Vec<f64> {
        match self {
            PairValue::Constant(v) => vec![*v],
            PairValue::Table(p) => p.iter().map(|&(_, v)| v).collect(),
        }
    }
}

/// One pair's coherence and phase. `(row, col)` and `(col, row)` name the same
/// pair; the phase is always that of `G_row,col`, so a pair given with
/// `row > col` has its phase negated when it is stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairSpec {
    pub row: usize,
    pub col: usize,
    /// Squared coherence, 0 to 1.
    pub coherence: PairValue,
    /// Phase of `G_row,col` in degrees.
    pub phase_deg: PairValue,
}

/// What a pair that is not given becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnspecifiedPairs {
    /// Coherence 0: the controls are independent.
    Independent,
    /// Coherence 1 and zero phase: the controls move together.
    FullyCoherent,
}

/// The default alarm band, (below, above) in dB.
pub const DEFAULT_ALARM_DB: (f64, f64) = (3.0, 3.0);
/// The default abort band, (below, above) in dB.
pub const DEFAULT_ABORT_DB: (f64, f64) = (6.0, 6.0);

/// One pair's stored coherence, phase and phase sign.
type PairEntry = Option<(PairValue, PairValue, f64)>;

/// The breakpoint builder's input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreakpointReference {
    pub grid: Grid,
    /// One per control (or degree of freedom).
    pub labels: Vec<String>,
    pub unit: String,
    /// One PSD table per control, in the order of `labels`.
    pub psd: Vec<BreakpointTable>,
    pub pairs: Vec<PairSpec>,
    pub unspecified: UnspecifiedPairs,
    /// Alarm band (below, above) in dB where a table row gives none.
    pub alarm_db: (f64, f64),
    /// Abort band (below, above) in dB where a table row gives none.
    pub abort_db: (f64, f64),
}

impl BreakpointReference {
    /// Independent controls with the default bands, unit `g`.
    pub fn new(grid: Grid, labels: Vec<String>, psd: Vec<BreakpointTable>) -> Self {
        BreakpointReference {
            grid,
            labels,
            unit: "g".into(),
            psd,
            pairs: Vec::new(),
            unspecified: UnspecifiedPairs::Independent,
            alarm_db: DEFAULT_ALARM_DB,
            abort_db: DEFAULT_ABORT_DB,
        }
    }

    /// Add a pair (builder style).
    pub fn pair(
        mut self,
        row: usize,
        col: usize,
        coherence: PairValue,
        phase_deg: PairValue,
    ) -> Self {
        self.pairs.push(PairSpec {
            row,
            col,
            coherence,
            phase_deg,
        });
        self
    }

    /// The pair table per `(row, col)` with `row < col`, phase sign normalized.
    fn pair_table(&self, n: usize) -> Result<Vec<PairEntry>, ReferenceError> {
        let mut table = vec![None; n * n];
        for (k, p) in self.pairs.iter().enumerate() {
            let what = format!("pair {} ({}, {})", k + 1, p.row, p.col);
            if p.row >= n || p.col >= n || p.row == p.col {
                return Err(ReferenceError::Input(format!(
                    "{what} does not name two different controls among the {n}; use indices 0 to \
                     {}",
                    n.saturating_sub(1)
                )));
            }
            p.coherence.check(&format!("{what} coherence"))?;
            p.phase_deg.check(&format!("{what} phase"))?;
            if let Some(v) = p
                .coherence
                .values()
                .into_iter()
                .find(|v| !(0.0..=1.0).contains(v))
            {
                return Err(ReferenceError::Input(format!(
                    "{what}: coherence {v} is outside 0 to 1; coherence here is squared \
                     coherence, between 0 and 1"
                )));
            }
            let (i, j, sign) = if p.row < p.col {
                (p.row, p.col, 1.0)
            } else {
                (p.col, p.row, -1.0)
            };
            let slot = &mut table[i * n + j];
            if slot.is_some() {
                return Err(ReferenceError::Input(format!(
                    "{what} is given twice ({} and {} name one pair); keep one entry",
                    self.labels[i], self.labels[j]
                )));
            }
            *slot = Some((p.coherence.clone(), p.phase_deg.clone(), sign));
        }
        Ok(table)
    }

    /// Build, validate and (with `repair`) repair the reference.
    pub fn build(
        &self,
        repair: Option<RepairPolicy>,
    ) -> Result<(Reference, Option<PsdRepairReport>), ReferenceError> {
        let n = self.labels.len();
        let Grid { df_hz, lines } = Grid::new(self.grid.df_hz, self.grid.lines)?;
        if n == 0 || self.psd.len() != n {
            return Err(ReferenceError::Input(format!(
                "{} labels and {} PSD tables; give one table per control",
                n,
                self.psd.len()
            )));
        }
        for (name, (below, above)) in [("alarm", self.alarm_db), ("abort", self.abort_db)] {
            if !(below.is_finite() && above.is_finite() && below >= 0.0 && above >= 0.0) {
                return Err(ReferenceError::Input(format!(
                    "the {name} band ({below}, {above}) dB must be two non-negative numbers of \
                     dB below and above the reference"
                )));
            }
        }
        let pairs = self.pair_table(n)?;
        // Half a line past the last line, so the resolver's floor lands on it.
        let f_max = (lines as f64 - 0.5) * df_hz;
        let mut sdm = HermitianStack::zeros(lines, n);
        let mut tolerances = Tolerances::none(lines, n);
        for (i, table) in self.psd.iter().enumerate() {
            let env =
                Envelope::from_table_on_grid(table, df_hz, f_max, self.alarm_db, self.abort_db)
                    .map_err(|e| {
                        ReferenceError::Input(format!(
                            "the PSD table of {}: {e}; correct the table",
                            self.labels[i]
                        ))
                    })?;
            if env.reference.psd.len() != lines {
                return Err(ReferenceError::Input(format!(
                    "the PSD table of {} resolved onto {} lines, not the grid's {lines}",
                    self.labels[i],
                    env.reference.psd.len()
                )));
            }
            for (k, &g) in env.reference.psd.iter().enumerate() {
                sdm.set(k, i, i, C64::new(g, 0.0));
                if g > 0.0 {
                    let x = k * n + i;
                    tolerances.alarm_upper[x] = env.alarm_hi[k];
                    tolerances.alarm_lower[x] = env.alarm_lo[k];
                    tolerances.abort_upper[x] = env.abort_hi[k];
                    tolerances.abort_lower[x] = env.abort_lo[k];
                }
            }
        }
        let default = match self.unspecified {
            UnspecifiedPairs::Independent => 0.0,
            UnspecifiedPairs::FullyCoherent => 1.0,
        };
        for k in 0..lines {
            let f = k as f64 * df_hz;
            let d = sdm.diagonal(k);
            for i in 0..n {
                for j in i + 1..n {
                    if d[i] <= 0.0 || d[j] <= 0.0 {
                        continue;
                    }
                    let (gamma2, theta_deg) = match &pairs[i * n + j] {
                        Some((coh, ph, sign)) => (coh.at(f), sign * ph.at(f)),
                        None => (default, 0.0),
                    };
                    let mag = (d[i] * d[j]).sqrt() * gamma2.sqrt();
                    sdm.set(k, i, j, C64::from_polar(mag, theta_deg.to_radians()));
                }
            }
        }
        let band = band_of(&sdm).ok_or_else(|| {
            ReferenceError::Input(format!(
                "every PSD table is zero on this grid (0 to {:.4} Hz); check the tables' \
                 frequencies against the grid",
                (lines - 1) as f64 * df_hz
            ))
        })?;
        let mut provenance = Provenance::new(
            Method::Breakpoints,
            format!(
                "{n} PSD breakpoint tables and {} pair entries",
                self.pairs.len()
            ),
        );
        provenance.assumptions.push(match self.unspecified {
            UnspecifiedPairs::Independent => {
                "pairs not given are independent (coherence 0)".to_string()
            }
            UnspecifiedPairs::FullyCoherent => {
                "pairs not given are fully coherent with zero phase".to_string()
            }
        });
        Reference::from_parts(
            ReferenceParts {
                sdm,
                df_hz,
                band,
                labels: self.labels.clone(),
                unit: self.unit.clone(),
                tolerances,
                response_transformation: None,
                drive_transformation: None,
                provenance,
            },
            repair,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_breakpoints::BreakpointRow;

    pub(crate) fn flat(level: f64, lo: f64, hi: f64) -> BreakpointTable {
        let mut t = BreakpointTable::new("flat");
        t.push(BreakpointRow::psd(lo, level));
        t.push(BreakpointRow::psd(hi, level));
        t
    }

    fn three() -> BreakpointReference {
        BreakpointReference::new(
            Grid::new(1.0, 256).unwrap(),
            vec!["a".into(), "b".into(), "c".into()],
            vec![
                flat(0.01, 20.0, 200.0),
                flat(0.02, 20.0, 200.0),
                flat(0.04, 20.0, 200.0),
            ],
        )
    }

    #[test]
    fn independent_controls_build_a_diagonal_reference_with_tolerances() {
        let (r, rep) = three().build(None).unwrap();
        assert!(rep.is_none());
        assert_eq!(r.band, (20, 201));
        assert!((r.sdm.get(100, 1, 1).re - 0.02).abs() < 1e-15);
        assert_eq!(r.sdm.get(100, 0, 1), C64::new(0.0, 0.0));
        let k = 100 * 3 + 2;
        assert!((r.tolerances.alarm_upper[k] / 0.04 - 10f64.powf(0.3)).abs() < 1e-12);
        assert!((r.tolerances.abort_lower[k] / 0.04 - 10f64.powf(-0.6)).abs() < 1e-12);
        assert!(r.tolerances.alarm_upper[10 * 3].is_nan());
        assert_eq!(r.provenance.method, Method::Breakpoints);
    }

    #[test]
    fn coherence_and_phase_form_the_cross_terms_with_the_documented_sign() {
        let b = three()
            .pair(
                0,
                1,
                PairValue::Constant(0.64),
                PairValue::Table(vec![(20.0, 0.0), (200.0, 90.0)]),
            )
            // Given as (2, 1): the phase is that of G_21, so G_12 carries its negative.
            .pair(2, 1, PairValue::Constant(0.25), PairValue::Constant(40.0));
        let (r, _) = b.build(None).unwrap();
        let g01 = r.sdm.get(110, 0, 1);
        assert!((g01.norm() - 0.8 * (0.01f64 * 0.02).sqrt()).abs() < 1e-15);
        assert!((g01.arg().to_degrees() - 45.0).abs() < 1e-9);
        assert!((r.sdm.get(110, 1, 0).arg().to_degrees() + 45.0).abs() < 1e-9);
        assert!((r.sdm.get(110, 1, 2).arg().to_degrees() + 40.0).abs() < 1e-9);
        assert!((r.sdm.get(110, 2, 1).arg().to_degrees() - 40.0).abs() < 1e-9);
        assert!((crate::coherence(&r.sdm, 110, 1, 2) - 0.25).abs() < 1e-12);
        // Held at the end value beyond the table.
        assert!((r.sdm.get(200, 0, 1).arg().to_degrees() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn fully_coherent_defaults_give_a_rank_one_reference_that_validates() {
        let mut b = three();
        b.unspecified = UnspecifiedPairs::FullyCoherent;
        let (r, _) = b.build(None).unwrap();
        assert!((crate::coherence(&r.sdm, 50, 0, 2) - 1.0).abs() < 1e-12);
        assert!(r.validate().unwrap().min_relative_eigenvalue > -1e-12);
    }

    #[test]
    fn mutually_inconsistent_coherences_are_named_and_the_repair_reports_coherence_changes() {
        // Each pair alone is realizable (coherence 0.9), but with phases 0, 0 and 180
        // degrees the three cannot hold together: theta_01 + theta_12 != theta_02.
        let b = three()
            .pair(0, 1, PairValue::Constant(0.9), PairValue::Constant(0.0))
            .pair(1, 2, PairValue::Constant(0.9), PairValue::Constant(0.0))
            .pair(0, 2, PairValue::Constant(0.9), PairValue::Constant(180.0));
        match b.build(None) {
            Err(ReferenceError::NotPositiveSemiDefinite { violations }) => {
                assert_eq!(violations.len(), 181);
                assert_eq!(violations[0].line, 20);
                assert!(violations[0].smallest_eigenvalue < 0.0);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            b.build(Some(RepairPolicy::STANDARD)),
            Err(ReferenceError::RepairRefused { line: 20, .. })
        ));
        let (r, rep) = b.build(Some(RepairPolicy::any_depth())).unwrap();
        let rep = rep.unwrap();
        assert_eq!(rep.summary.lines_repaired, 181);
        let line = &rep.lines[0];
        assert_eq!(line.pairs.len(), 3);
        for p in &line.pairs {
            assert!((p.before - 0.9).abs() < 1e-12, "{p:?}");
            assert!(p.after < 0.9, "{p:?}");
        }
        // The PSDs entered are kept exactly.
        assert!((r.sdm.get(100, 2, 2).re - 0.04).abs() < 1e-15);
        assert!(r.provenance.repair.is_some());
        r.validate().unwrap();
    }

    #[test]
    fn bad_pair_entries_are_refused() {
        let e = three()
            .pair(0, 1, PairValue::Constant(1.2), PairValue::Constant(0.0))
            .build(None)
            .unwrap_err();
        assert!(e.to_string().contains("outside 0 to 1"), "{e}");
        let e = three()
            .pair(0, 1, PairValue::Constant(0.5), PairValue::Constant(0.0))
            .pair(1, 0, PairValue::Constant(0.5), PairValue::Constant(0.0))
            .build(None)
            .unwrap_err();
        assert!(e.to_string().contains("given twice"), "{e}");
        let e = three()
            .pair(0, 3, PairValue::Constant(0.5), PairValue::Constant(0.0))
            .build(None)
            .unwrap_err();
        assert!(e.to_string().contains("two different controls"), "{e}");
        let e = three()
            .pair(
                0,
                1,
                PairValue::Constant(0.5),
                PairValue::Table(vec![(50.0, 0.0), (40.0, 1.0)]),
            )
            .build(None)
            .unwrap_err();
        assert!(e.to_string().contains("increasing frequency"), "{e}");
    }

    #[test]
    fn a_control_without_level_across_the_band_is_named() {
        let mut b = three();
        b.psd[1] = flat(0.02, 50.0, 200.0);
        match b.build(None) {
            Err(ReferenceError::ZeroDiagonalInBand { line, channel, .. }) => {
                assert_eq!((line, channel.as_str()), (20, "b"));
            }
            other => panic!("{other:?}"),
        }
    }
}
