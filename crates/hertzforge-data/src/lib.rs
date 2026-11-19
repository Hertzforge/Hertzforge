//! `hertzforge-data` - the Insight function data model.
//!
//! DOF-addressed function records with units, provenance, and grid semantics.
//! Layer rule: this crate knows what the numbers MEAN
//! (quantity, unit, kind, recipe) but performs no signal processing - kernels
//! live in `hertzforge-dsp`, policy in `hertzforge-insight`.
//!
//! No geometry. Module B (the modal package) owns geometry when it exists.

pub mod abscissa;
pub mod dof;
pub mod provenance;
pub mod units;

pub use abscissa::{detect_spacing, Abscissa, GridSpacing};
pub use dof::{Direction, DofRef};
pub use provenance::Provenance;
pub use units::{Quantity, Unit};

use serde::{Deserialize, Serialize};

/// Stable identity of a function within a session. Monotonic, never reused
/// (reuse corrupts downstream id-keyed state; Insight avoids the class entirely
/// by never recycling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FnId(pub u64);

/// Absolute time anchor: epoch seconds + fraction.
/// Optional on every record so drift correction is never precluded; `None`
/// means "the source carried no absolute time", not "t = 0".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AbsTime {
    pub epoch_s: i64,
    /// Fractional second in [0, 1).
    pub frac: f64,
}

/// What a function IS. Drives the compare dispatch table and the
/// dB/axis policy. `Unknown` refuses regrid/compare until the preview sheet
/// assigns a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionKind {
    TimeHistory,
    LinearSpectrum,
    PowerSpectrum,
    Autopower,
    Psd,
    Cpsd,
    Frf,
    Coherence,
    Srs,
    BandSpectrum,
    Profile,
    /// Lag-domain cross-correlation (abscissa: lag seconds, may be negative).
    Correlation,
    Unknown,
}

impl FunctionKind {
    /// The density-or-power class that takes CONSERVATIVE REBIN when regridding.
    /// Everything else that is comparable takes point interpolation.
    pub fn rebin_class(self) -> bool {
        matches!(
            self,
            FunctionKind::Psd
                | FunctionKind::Autopower
                | FunctionKind::PowerSpectrum
                | FunctionKind::Cpsd
        )
    }

    /// Frequency-domain kinds (abscissa is Hz).
    pub fn is_frequency_domain(self) -> bool {
        !matches!(
            self,
            FunctionKind::TimeHistory | FunctionKind::Correlation | FunctionKind::Unknown
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            FunctionKind::TimeHistory => "Time History",
            FunctionKind::LinearSpectrum => "Linear Spectrum",
            FunctionKind::PowerSpectrum => "Power Spectrum",
            FunctionKind::Autopower => "Autopower",
            FunctionKind::Psd => "PSD",
            FunctionKind::Cpsd => "CPSD",
            FunctionKind::Frf => "FRF",
            FunctionKind::Coherence => "Coherence",
            FunctionKind::Srs => "SRS",
            FunctionKind::BandSpectrum => "Band Spectrum",
            FunctionKind::Profile => "Profile",
            FunctionKind::Correlation => "Correlation",
            FunctionKind::Unknown => "Unknown",
        }
    }
}

/// Ordinate storage. Complex is (re, im) pairs; magnitude/phase are derived
/// views, never stored (single source of truth).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Ordinate {
    Real(Vec<f64>),
    Complex(Vec<(f64, f64)>),
}

impl Ordinate {
    pub fn len(&self) -> usize {
        match self {
            Ordinate::Real(v) => v.len(),
            Ordinate::Complex(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_complex(&self) -> bool {
        matches!(self, Ordinate::Complex(_))
    }

    /// Magnitude view (identity for Real - abs is NOT applied, real data may be
    /// legitimately negative, e.g. a time history).
    pub fn magnitude(&self) -> Vec<f64> {
        match self {
            Ordinate::Real(v) => v.clone(),
            Ordinate::Complex(v) => v
                .iter()
                .map(|(re, im)| (re * re + im * im).sqrt())
                .collect(),
        }
    }
}

/// Where a record came from (session bookkeeping; provenance carries the HOW).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Absolute path of the originating file, or a symbolic source like
    /// "session" for purely derived records.
    pub path: String,
    /// Format short-name ("uff", "csv", "wav", "hertzforge-capture", "hertzforge-snapshot",
    /// "derived").
    pub format: String,
    /// Unix seconds at import time (informational; not the data's t0).
    pub imported_at: i64,
}

/// The function record - Insight's atom.
///
/// NAMES-NOT-IDS is structural: display and export surfaces use `name` (and
/// `role`); `FnId` never leaves the process boundary except inside Insight's
/// own IPC, and never appears in any written file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionRecord {
    pub id: FnId,
    pub name: String,
    /// Optional semantic role tag ("reference", "tolerance-hi", "control", ...).
    pub role: Option<String>,
    pub response: Option<DofRef>,
    pub reference: Option<DofRef>,
    pub kind: FunctionKind,
    pub abscissa: Abscissa,
    pub ordinate: Ordinate,
    pub unit_ordinate: Unit,
    pub unit_abscissa: Unit,
    /// Absolute time anchor of sample 0, when the source carried one.
    pub t0: Option<AbsTime>,
    pub provenance: Provenance,
    pub source: SourceRef,
    pub comments: Vec<String>,
}

impl FunctionRecord {
    /// Length coherence: ordinate sample count must match the abscissa.
    pub fn validate(&self) -> Result<(), String> {
        let (na, no) = (self.abscissa.len(), self.ordinate.len());
        if na != no {
            return Err(format!(
                "'{}': abscissa has {} points but ordinate has {}",
                self.name, na, no
            ));
        }
        if let Abscissa::Explicit(x) = &self.abscissa {
            // Positive failure condition with explicit NaN guards, so a NaN point
            // still fails the strictly-increasing check (clippy neg_cmp_op_on_partial_ord).
            if x.windows(2)
                .any(|w| w[1] <= w[0] || w[0].is_nan() || w[1].is_nan())
            {
                return Err(format!(
                    "'{}': abscissa is not strictly increasing",
                    self.name
                ));
            }
        }
        if self.name.trim().is_empty() {
            return Err("record has an empty name".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: FunctionKind, n: usize) -> FunctionRecord {
        FunctionRecord {
            id: FnId(1),
            name: "T1 control".into(),
            role: None,
            response: None,
            reference: None,
            kind,
            abscissa: Abscissa::Even {
                start: 0.0,
                step: 1.0,
                n,
            },
            ordinate: Ordinate::Real(vec![1.0; n]),
            unit_ordinate: units::unit("g").unwrap(),
            unit_abscissa: units::unit("Hz").unwrap(),
            t0: None,
            provenance: Provenance::Imported {
                format: "test".into(),
                path: "mem".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "mem".into(),
                format: "test".into(),
                imported_at: 0,
            },
            comments: vec![],
        }
    }

    #[test]
    fn validate_catches_length_mismatch() {
        let mut r = rec(FunctionKind::Psd, 8);
        r.ordinate = Ordinate::Real(vec![1.0; 7]);
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_catches_non_monotonic_explicit() {
        let mut r = rec(FunctionKind::Srs, 3);
        r.abscissa = Abscissa::Explicit(vec![10.0, 20.0, 15.0]);
        r.ordinate = Ordinate::Real(vec![1.0; 3]);
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_catches_a_nan_explicit_point() {
        let mut r = rec(FunctionKind::Srs, 3);
        r.ordinate = Ordinate::Real(vec![1.0; 3]);
        // First, interior and last position: each window side's NaN term is exercised.
        for x in [
            vec![f64::NAN, 20.0, 30.0],
            vec![10.0, f64::NAN, 30.0],
            vec![10.0, 20.0, f64::NAN],
        ] {
            r.abscissa = Abscissa::Explicit(x);
            assert!(r.validate().is_err());
        }
    }

    #[test]
    fn rebin_class_matches_plan_dispatch_row() {
        for k in [
            FunctionKind::Psd,
            FunctionKind::Autopower,
            FunctionKind::PowerSpectrum,
            FunctionKind::Cpsd,
        ] {
            assert!(k.rebin_class(), "{k:?} must take conservative rebin");
        }
        for k in [
            FunctionKind::LinearSpectrum,
            FunctionKind::Frf,
            FunctionKind::Coherence,
            FunctionKind::Srs,
            FunctionKind::TimeHistory,
            FunctionKind::Profile,
            FunctionKind::Correlation,
        ] {
            assert!(!k.rebin_class(), "{k:?} must NOT take conservative rebin");
        }
    }

    #[test]
    fn serde_round_trip() {
        let r = rec(FunctionKind::Frf, 4);
        let j = serde_json::to_string(&r).unwrap();
        let back: FunctionRecord = serde_json::from_str(&j).unwrap();
        assert_eq!(r, back);
    }
}
