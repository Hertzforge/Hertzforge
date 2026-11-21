//! Multiple-output Random: the [`RandomMimoLaw`] trait, the manifest builder,
//! and the conversions between what the `random-mimo` interface carries and
//! the spectral density matrix stacks of `hertzforge-sdm`.
//!
//! A multiple-output law is written against `hertzforge-sdm`'s types: the
//! setup ([`MimoSetup`]), each loop's inputs ([`MimoLoopInputs`]) and its
//! result ([`MimoLoopOutputs`]), with Hermitian matrices as
//! [`HermitianStack`] and rectangular ones as [`MatrixStack`]. These are the
//! controller's own boundary (`hertzforge_sdm::law`), so a law written once runs
//! natively inside the controller and, through [`crate::export_mimo_law!`], as a
//! law file.
//!
//! ```ignore
//! use hertzforge_law_sdk::mimo::*;
//! use hertzforge_law_sdk::{export_mimo_law, Params, RandomMimoLaw};
//!
//! struct Hold { drive: HermitianStack }
//!
//! impl RandomMimoLaw for Hold {
//!     fn describe() -> MimoManifest {
//!         MimoManifestBuilder::new("hold", "0.1.0")
//!             .summary("Holds the first drive.")
//!             .shapes(&[MimoShape::Square])
//!             .build()
//!     }
//!     fn new(setup: MimoSetup, params: Params) -> Result<Self, String> { /* ... */ }
//!     fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> { /* ... */ }
//!     fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> { /* ... */ }
//! }
//!
//! export_mimo_law!(Hold);
//! ```
//!
//! # Conventions
//!
//! A spectral density matrix is `G = lim (1/T) E[X X^H]` per line, so a
//! response `Y = H X` has `G_yy = H G_xx H^H` (the conjugate of the Bendat and
//! Piersol definition). Hermitian stacks travel packed (upper triangle,
//! row-major per line), rectangular stacks in full (row-major per line). The
//! law works at full level: the reference is the full-level specification,
//! the response has been referred to full level, and the drive returned is the
//! full-level drive. The controller owns the level, and every protection.

use crate::__bindings::hertzforge::control_law::types::{
    Band, DiagnosticSpec, Manifest, ParamSpec,
};
use crate::__mimo_bindings::exports::hertzforge::control_law::random_mimo_law as wit_law;
use crate::__mimo_bindings::hertzforge::control_law::mimo_types as wit_types;
use crate::{Param, Params};
use core::cell::RefCell;

pub use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape, Tolerances,
};
pub use hertzforge_sdm::{Complex, HermitianStack, MatrixStack, ShapeError, C64};

/// Per-line linear algebra on the stacks (eigen and singular value
/// decompositions, the regularized pseudo-inverse, congruences, PSD repair):
/// `hertzforge-sdm`'s, the same the controller uses.
pub use hertzforge_sdm::linalg;

/// The interface's own MIMO types, as generated from the WIT. A law rarely
/// needs them: the conversions in this module turn them into
/// `hertzforge-sdm`'s types and back.
pub mod wit {
    pub use crate::__mimo_bindings::hertzforge::control_law::mimo_types::{
        Complex, DiagnosticData, DiagnosticKind, DiagnosticLayout, HermitianStack, MatrixStack,
        MimoDiagnostic, MimoInput, MimoInputs, MimoManifest, MimoOutputs, MimoSetup, MimoShape,
        ToleranceLines,
    };
}

pub use wit_types::{DiagnosticKind, DiagnosticLayout, MimoManifest};

/// A multiple-output Random control law.
///
/// The host builds one value per run with [`Self::new`], calls
/// [`Self::seed`] once after system identification (with the identification
/// response as the response), then [`Self::iterate`] once per control loop.
/// Calls never overlap. Everything is at full level; the controller applies
/// the level schedule and every protection to the drive the law returns.
pub trait RandomMimoLaw: Sized + 'static {
    /// The law's name, version, parameters, diagnostics, supported shapes and
    /// needed inputs. Read once, when the file is loaded; it must not depend
    /// on anything but the code. Build it with [`MimoManifestBuilder`].
    fn describe() -> MimoManifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest (a per-channel parameter has one
    /// value per control channel). An error refuses the run.
    fn new(setup: MimoSetup, params: Params) -> Result<Self, String>;

    /// Return the first full-level drive from the system-identification
    /// measurement (`inputs.response` is the identification response).
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String>;

    /// Return the next full-level drive from the latest measurement.
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String>;
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// Builds a [`MimoManifest`]: the manifest every law declares (name, version,
/// author, summary, parameters, diagnostics) plus the shapes the law supports,
/// the optional inputs it needs and the layout of each diagnostic.
///
/// Parameters are declared exactly as a single-output law declares them, as
/// [`ParamSpec`] values; the form is drawn from them.
#[derive(Debug, Clone)]
pub struct MimoManifestBuilder {
    law: Manifest,
    shapes: Vec<wit::MimoShape>,
    needs: Vec<wit::MimoInput>,
    layouts: Vec<DiagnosticLayout>,
}

impl MimoManifestBuilder {
    /// A manifest named `name` at `version`, supporting no shape yet.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        MimoManifestBuilder {
            law: Manifest {
                name: name.into(),
                version: version.into(),
                author: String::new(),
                summary: String::new(),
                params: Vec::new(),
                diagnostics: Vec::new(),
            },
            shapes: Vec::new(),
            needs: Vec::new(),
            layouts: Vec::new(),
        }
    }

    /// Who wrote the law.
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.law.author = author.into();
        self
    }

    /// One paragraph on what the law does.
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.law.summary = summary.into();
        self
    }

    /// Declare a parameter.
    pub fn param(mut self, spec: ParamSpec) -> Self {
        self.law.params.push(spec);
        self
    }

    /// Declare several parameters, in order.
    pub fn params(mut self, specs: impl IntoIterator<Item = ParamSpec>) -> Self {
        self.law.params.extend(specs);
        self
    }

    /// Declare a diagnostic and the layout of its values.
    pub fn diagnostic(
        mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        unit: impl Into<String>,
        help: impl Into<String>,
        kind: DiagnosticKind,
    ) -> Self {
        let key = key.into();
        self.law.diagnostics.push(DiagnosticSpec {
            key: key.clone(),
            label: label.into(),
            unit: unit.into(),
            help: help.into(),
        });
        self.layouts.push(DiagnosticLayout { key, kind });
        self
    }

    /// The shapes the law supports (added to any already given).
    pub fn shapes(mut self, shapes: &[MimoShape]) -> Self {
        for s in shapes {
            let w = shape_to_wit(*s);
            if !self.shapes.contains(&w) {
                self.shapes.push(w);
            }
        }
        self
    }

    /// The optional inputs the law reads (added to any already given).
    pub fn needs(mut self, needs: &[MimoInput]) -> Self {
        for n in needs {
            let w = input_to_wit(*n);
            if !self.needs.contains(&w) {
                self.needs.push(w);
            }
        }
        self
    }

    /// The manifest.
    pub fn build(self) -> MimoManifest {
        MimoManifest {
            law: self.law,
            shapes: self.shapes,
            needs: self.needs,
            diagnostic_layouts: self.layouts,
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

/// A shape in the interface's terms.
pub fn shape_to_wit(s: MimoShape) -> wit::MimoShape {
    match s {
        MimoShape::Square => wit::MimoShape::Square,
        MimoShape::MoreControlsThanDrives => wit::MimoShape::MoreControlsThanDrives,
        MimoShape::FewerControlsThanDrives => wit::MimoShape::FewerControlsThanDrives,
    }
}

/// A shape from the interface's terms.
pub fn shape_from_wit(s: wit::MimoShape) -> MimoShape {
    match s {
        wit::MimoShape::Square => MimoShape::Square,
        wit::MimoShape::MoreControlsThanDrives => MimoShape::MoreControlsThanDrives,
        wit::MimoShape::FewerControlsThanDrives => MimoShape::FewerControlsThanDrives,
    }
}

/// An optional input in the interface's terms.
pub fn input_to_wit(i: MimoInput) -> wit::MimoInput {
    match i {
        MimoInput::LastDrive => wit::MimoInput::LastDrive,
        MimoInput::ResponseDriveCsd => wit::MimoInput::ResponseDriveCsd,
        MimoInput::SysidResponse => wit::MimoInput::SysidResponse,
        MimoInput::SysidDrive => wit::MimoInput::SysidDrive,
        MimoInput::SysidNoise => wit::MimoInput::SysidNoise,
    }
}

/// An optional input from the interface's terms.
pub fn input_from_wit(i: wit::MimoInput) -> MimoInput {
    match i {
        wit::MimoInput::LastDrive => MimoInput::LastDrive,
        wit::MimoInput::ResponseDriveCsd => MimoInput::ResponseDriveCsd,
        wit::MimoInput::SysidResponse => MimoInput::SysidResponse,
        wit::MimoInput::SysidDrive => MimoInput::SysidDrive,
        wit::MimoInput::SysidNoise => MimoInput::SysidNoise,
    }
}

fn complex_list(data: Vec<wit::Complex>) -> Vec<C64> {
    data.into_iter().map(|c| C64::new(c.re, c.im)).collect()
}

fn complex_wit(data: &[C64]) -> Vec<wit::Complex> {
    data.iter()
        .map(|c| wit::Complex { re: c.re, im: c.im })
        .collect()
}

fn shape_err(e: ShapeError) -> String {
    e.0
}

/// Unpack a packed Hermitian stack from the interface. Fails on a length
/// that does not match its dimensions. A diagonal's imaginary part is
/// dropped.
pub fn hermitian_from_wit(h: wit::HermitianStack) -> Result<HermitianStack, String> {
    HermitianStack::from_packed(h.lines as usize, h.n as usize, complex_list(h.data))
        .map_err(shape_err)
}

/// Pack a Hermitian stack for the interface.
pub fn hermitian_to_wit(h: &HermitianStack) -> wit::HermitianStack {
    wit::HermitianStack {
        lines: h.lines() as u32,
        n: h.n() as u32,
        data: complex_wit(h.data()),
    }
}

/// Unpack a full matrix stack from the interface. Fails on a length that does
/// not match its dimensions.
pub fn matrix_from_wit(m: wit::MatrixStack) -> Result<MatrixStack, String> {
    MatrixStack::from_vec(
        m.lines as usize,
        m.rows as usize,
        m.cols as usize,
        complex_list(m.data),
    )
    .map_err(shape_err)
}

/// A full matrix stack for the interface.
pub fn matrix_to_wit(m: &MatrixStack) -> wit::MatrixStack {
    wit::MatrixStack {
        lines: m.lines() as u32,
        rows: m.rows() as u32,
        cols: m.cols() as u32,
        data: complex_wit(m.data()),
    }
}

/// The tolerance lines from the interface.
pub fn tolerances_from_wit(t: wit::ToleranceLines) -> Tolerances {
    Tolerances {
        alarm_upper: t.alarm_upper,
        alarm_lower: t.alarm_lower,
        abort_upper: t.abort_upper,
        abort_lower: t.abort_lower,
    }
}

/// The tolerance lines for the interface.
pub fn tolerances_to_wit(t: &Tolerances) -> wit::ToleranceLines {
    wit::ToleranceLines {
        alarm_upper: t.alarm_upper.clone(),
        alarm_lower: t.alarm_lower.clone(),
        abort_upper: t.abort_upper.clone(),
        abort_lower: t.abort_lower.clone(),
    }
}

fn band_from_wit(b: Band) -> (usize, usize) {
    (b.lo as usize, b.hi as usize)
}

fn band_to_wit(b: (usize, usize)) -> Band {
    Band {
        lo: b.0 as u32,
        hi: b.1 as u32,
    }
}

/// The setup from the interface, with every dimension checked.
pub fn setup_from_wit(s: wit::MimoSetup) -> Result<MimoSetup, String> {
    let setup = MimoSetup {
        controls: s.controls as usize,
        drives: s.drives as usize,
        lines: s.lines as usize,
        df_hz: s.df_hz,
        band: band_from_wit(s.band),
        reference: hermitian_from_wit(s.reference)?,
        tolerances: tolerances_from_wit(s.tolerances),
    };
    setup.check().map_err(shape_err)?;
    Ok(setup)
}

/// The setup for the interface. The dimensions must fit the interface's
/// 32-bit counts.
pub fn setup_to_wit(s: &MimoSetup) -> wit::MimoSetup {
    wit::MimoSetup {
        controls: s.controls as u32,
        drives: s.drives as u32,
        lines: s.lines as u32,
        df_hz: s.df_hz,
        band: band_to_wit(s.band),
        reference: hermitian_to_wit(&s.reference),
        tolerances: tolerances_to_wit(&s.tolerances),
    }
}

/// One loop's inputs unpacked from the interface and owned, so a
/// [`MimoLoopInputs`] can borrow them ([`Self::view`]).
#[derive(Debug, Clone)]
pub struct OwnedInputs {
    /// The reference SDM.
    pub reference: HermitianStack,
    /// The tolerance lines.
    pub tolerances: Tolerances,
    /// The measured response SDM at full level.
    pub response: HermitianStack,
    /// Degrees of freedom of `response`.
    pub response_dof: f64,
    /// The plant estimate.
    pub frf: MatrixStack,
    /// Multiple coherence of each control on all drives, line-major: the
    /// identification's while the plant estimate is held, the loop's own while
    /// it is updated.
    pub multiple_coherence: Vec<f64>,
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// The band.
    pub band: (usize, usize),
    /// The loop index.
    pub loop_index: u64,
    /// The last drive as played, when declared.
    pub last_drive: Option<HermitianStack>,
    /// The response-to-drive cross spectrum, when declared.
    pub response_drive_csd: Option<MatrixStack>,
    /// The identification response, when declared.
    pub sysid_response: Option<HermitianStack>,
    /// The identification drive, when declared.
    pub sysid_drive: Option<HermitianStack>,
    /// The identification noise floor, when declared.
    pub sysid_noise: Option<HermitianStack>,
}

impl OwnedInputs {
    /// Unpack `i`. Fails on a stack whose length does not match its
    /// dimensions.
    pub fn from_wit(i: wit::MimoInputs) -> Result<Self, String> {
        Ok(OwnedInputs {
            reference: hermitian_from_wit(i.reference)?,
            tolerances: tolerances_from_wit(i.tolerances),
            response: hermitian_from_wit(i.response)?,
            response_dof: i.response_dof,
            frf: matrix_from_wit(i.frf)?,
            multiple_coherence: i.multiple_coherence,
            df_hz: i.df_hz,
            band: band_from_wit(i.band),
            loop_index: i.loop_index,
            last_drive: i.last_drive.map(hermitian_from_wit).transpose()?,
            response_drive_csd: i.response_drive_csd.map(matrix_from_wit).transpose()?,
            sysid_response: i.sysid_response.map(hermitian_from_wit).transpose()?,
            sysid_drive: i.sysid_drive.map(hermitian_from_wit).transpose()?,
            sysid_noise: i.sysid_noise.map(hermitian_from_wit).transpose()?,
        })
    }

    /// The inputs as the native boundary borrows them.
    pub fn view(&self) -> MimoLoopInputs<'_> {
        MimoLoopInputs {
            reference: &self.reference,
            tolerances: &self.tolerances,
            response: &self.response,
            response_dof: self.response_dof,
            frf: &self.frf,
            multiple_coherence: &self.multiple_coherence,
            df_hz: self.df_hz,
            band: self.band,
            loop_index: self.loop_index,
            last_drive: self.last_drive.as_ref(),
            response_drive_csd: self.response_drive_csd.as_ref(),
            sysid_response: self.sysid_response.as_ref(),
            sysid_drive: self.sysid_drive.as_ref(),
            sysid_noise: self.sysid_noise.as_ref(),
        }
    }
}

/// One loop's inputs packed for the interface. Every optional input present
/// in `i` is passed; a host passes only the declared ones.
pub fn inputs_to_wit(i: &MimoLoopInputs<'_>) -> wit::MimoInputs {
    wit::MimoInputs {
        reference: hermitian_to_wit(i.reference),
        tolerances: tolerances_to_wit(i.tolerances),
        response: hermitian_to_wit(i.response),
        response_dof: i.response_dof,
        frf: matrix_to_wit(i.frf),
        multiple_coherence: i.multiple_coherence.to_vec(),
        df_hz: i.df_hz,
        band: band_to_wit(i.band),
        loop_index: i.loop_index,
        last_drive: i.last_drive.map(hermitian_to_wit),
        response_drive_csd: i.response_drive_csd.map(matrix_to_wit),
        sysid_response: i.sysid_response.map(hermitian_to_wit),
        sysid_drive: i.sysid_drive.map(hermitian_to_wit),
        sysid_noise: i.sysid_noise.map(hermitian_to_wit),
    }
}

/// A loop's result packed for the interface.
pub fn outputs_to_wit(o: MimoLoopOutputs) -> wit::MimoOutputs {
    wit::MimoOutputs {
        drive: hermitian_to_wit(&o.drive),
        diagnostics: o
            .diagnostics
            .into_iter()
            .map(|d| wit::MimoDiagnostic {
                key: d.key,
                data: match d.data {
                    DiagnosticData::Scalar(v) => wit::DiagnosticData::Scalar(v),
                    DiagnosticData::PerLine(v) => wit::DiagnosticData::PerLine(v),
                    DiagnosticData::PerControlLine(v) => wit::DiagnosticData::PerControlLine(v),
                    DiagnosticData::PerDriveLine(v) => wit::DiagnosticData::PerDriveLine(v),
                },
            })
            .collect(),
    }
}

/// A loop's result unpacked from the interface. Fails on a drive whose
/// length does not match its dimensions.
pub fn outputs_from_wit(o: wit::MimoOutputs) -> Result<MimoLoopOutputs, String> {
    Ok(MimoLoopOutputs {
        drive: hermitian_from_wit(o.drive)?,
        diagnostics: o
            .diagnostics
            .into_iter()
            .map(|d| DiagnosticValue {
                key: d.key,
                data: match d.data {
                    wit::DiagnosticData::Scalar(v) => DiagnosticData::Scalar(v),
                    wit::DiagnosticData::PerLine(v) => DiagnosticData::PerLine(v),
                    wit::DiagnosticData::PerControlLine(v) => DiagnosticData::PerControlLine(v),
                    wit::DiagnosticData::PerDriveLine(v) => DiagnosticData::PerDriveLine(v),
                },
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// The exported resource
// ---------------------------------------------------------------------------

/// The resource the interface exports: a [`RandomMimoLaw`] behind a
/// `RefCell`, because the interface hands the law shared references. Calls
/// never overlap, so the borrow never fails; if it did, the call returns an
/// error rather than panicking.
#[doc(hidden)]
pub struct MimoLawResource<L: RandomMimoLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: RandomMimoLaw> MimoLawResource<L> {
    fn call(
        &self,
        inputs: wit::MimoInputs,
        f: impl FnOnce(&mut L, &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String>,
    ) -> Result<wit::MimoOutputs, String> {
        let owned = OwnedInputs::from_wit(inputs)?;
        let mut law = self
            .law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?;
        f(&mut law, &owned.view()).map(outputs_to_wit)
    }
}

impl<L: RandomMimoLaw> wit_law::GuestLaw for MimoLawResource<L> {
    fn new(setup: wit::MimoSetup, params: Vec<Param>) -> Result<Self, String> {
        let setup = setup_from_wit(setup)?;
        L::new(setup, Params(params)).map(|law| MimoLawResource {
            law: RefCell::new(law),
        })
    }

    fn seed(&self, inputs: wit::MimoInputs) -> Result<wit::MimoOutputs, String> {
        self.call(inputs, |law, i| law.seed(i))
    }

    fn iterate(&self, inputs: wit::MimoInputs) -> Result<wit::MimoOutputs, String> {
        self.call(inputs, |law, i| law.iterate(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    #[test]
    fn stacks_round_trip_through_the_interface_types() {
        let mut h = HermitianStack::zeros(2, 3);
        h.set(1, 0, 2, c(0.5, -0.25));
        h.set(1, 2, 2, c(3.0, 0.0));
        let back = hermitian_from_wit(hermitian_to_wit(&h)).unwrap();
        assert_eq!(back, h);
        let mut m = MatrixStack::zeros(2, 3, 2);
        m.set(1, 2, 1, c(1.0, 2.0));
        let back = matrix_from_wit(matrix_to_wit(&m)).unwrap();
        assert_eq!(back, m);
        let short = wit::HermitianStack {
            lines: 2,
            n: 3,
            data: vec![wit::Complex { re: 0.0, im: 0.0 }; 11],
        };
        assert!(hermitian_from_wit(short).is_err());
    }

    #[test]
    fn the_builder_declares_shapes_needs_and_layouts_once_each() {
        let m = MimoManifestBuilder::new("x", "1.0.0")
            .shapes(&[MimoShape::Square, MimoShape::Square])
            .shapes(&[MimoShape::MoreControlsThanDrives])
            .needs(&[MimoInput::SysidResponse, MimoInput::SysidResponse])
            .diagnostic("cond", "Condition", "", "", DiagnosticKind::PerLine)
            .build();
        assert_eq!(
            m.shapes,
            [
                wit::MimoShape::Square,
                wit::MimoShape::MoreControlsThanDrives
            ]
        );
        assert_eq!(m.needs, [wit::MimoInput::SysidResponse]);
        assert_eq!(m.law.diagnostics.len(), 1);
        assert_eq!(m.diagnostic_layouts[0].key, "cond");
        assert_eq!(m.diagnostic_layouts[0].kind, DiagnosticKind::PerLine);
        for s in [
            MimoShape::Square,
            MimoShape::MoreControlsThanDrives,
            MimoShape::FewerControlsThanDrives,
        ] {
            assert_eq!(shape_from_wit(shape_to_wit(s)), s);
        }
        for i in [
            MimoInput::LastDrive,
            MimoInput::ResponseDriveCsd,
            MimoInput::SysidResponse,
            MimoInput::SysidDrive,
            MimoInput::SysidNoise,
        ] {
            assert_eq!(input_from_wit(input_to_wit(i)), i);
        }
    }
}
