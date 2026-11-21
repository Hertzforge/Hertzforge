//! The boundary a MIMO control law is written against.
//!
//! The law proposes, the core disposes: a law computes the next drive spectral
//! density matrix from what it is given and nothing else. Every protection (drive
//! ceilings, caps, limiting, envelope, divergence, budgets) belongs to the core.
//!
//! Everything here is referred to full level (0 dB): the reference is the full-level
//! specification, the measured response has been scaled up to its full-level
//! equivalent by the core, and the drive a law returns is the full-level drive. The
//! core applies the level schedule. A law never sees the level.
//!
//! Spaces: `controls` is the control space the reference is written in (physical
//! channels, or virtual degrees of freedom after the response transformation);
//! `drives` is the drive space the law commands (physical outputs, or virtual drives
//! before the drive transformation). The core transforms both ways.

use crate::{HermitianStack, MatrixStack, ShapeError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The control-to-drive shapes a law may support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MimoShape {
    /// As many control channels as drives.
    Square,
    /// More control channels than drives (least squares).
    MoreControlsThanDrives,
    /// Fewer control channels than drives (minimum norm).
    FewerControlsThanDrives,
}

impl MimoShape {
    /// The shape of a test with `controls` control channels and `drives` drives.
    pub fn of(controls: usize, drives: usize) -> MimoShape {
        use std::cmp::Ordering::*;
        match controls.cmp(&drives) {
            Equal => MimoShape::Square,
            Greater => MimoShape::MoreControlsThanDrives,
            Less => MimoShape::FewerControlsThanDrives,
        }
    }
}

/// The optional inputs a law may declare it needs. The core marshals only those,
/// because at full scale each is tens of megabytes per loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MimoInput {
    /// The drive SDM of the last loop as it was played, after the core's limits
    /// (full level), drives by drives.
    LastDrive,
    /// This loop's measured response-to-drive cross spectrum, controls by drives,
    /// so a law can adapt its own plant estimate.
    ResponseDriveCsd,
    /// System identification: the control response SDM, controls by controls.
    SysidResponse,
    /// System identification: the drive SDM, drives by drives.
    SysidDrive,
    /// System identification: the control response SDM with the drives off (the
    /// noise floor), controls by controls.
    SysidNoise,
}

/// Alarm and abort lines per control channel, as power spectral densities on the
/// grid, line-major (`lines * controls` values each). `NaN` means no line there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tolerances {
    pub alarm_upper: Vec<f64>,
    pub alarm_lower: Vec<f64>,
    pub abort_upper: Vec<f64>,
    pub abort_lower: Vec<f64>,
}

impl Tolerances {
    /// No lines at all.
    pub fn none(lines: usize, controls: usize) -> Self {
        let v = vec![f64::NAN; lines * controls];
        Tolerances {
            alarm_upper: v.clone(),
            alarm_lower: v.clone(),
            abort_upper: v.clone(),
            abort_lower: v,
        }
    }

    fn check(&self, lines: usize, controls: usize) -> Result<(), ShapeError> {
        let want = lines * controls;
        for (name, v) in [
            ("alarm_upper", &self.alarm_upper),
            ("alarm_lower", &self.alarm_lower),
            ("abort_upper", &self.abort_upper),
            ("abort_lower", &self.abort_lower),
        ] {
            if v.len() != want {
                return Err(ShapeError(format!(
                    "tolerance {name} has {} values, {lines} lines by {controls} controls need {want}",
                    v.len()
                )));
            }
        }
        Ok(())
    }
}

/// What a law is built for: fixed for the run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MimoSetup {
    /// Control channels (or virtual DOFs) the reference is written in.
    pub controls: usize,
    /// Drives (or virtual drives) the law commands.
    pub drives: usize,
    /// Lines on the grid.
    pub lines: usize,
    /// Line spacing, Hz. Line `k` is at `k * df_hz`.
    pub df_hz: f64,
    /// The half-open controlled band `[lo, hi)` in lines.
    pub band: (usize, usize),
    /// The reference SDM at full level, controls by controls per line.
    pub reference: HermitianStack,
    /// The alarm and abort lines per control.
    pub tolerances: Tolerances,
}

impl MimoSetup {
    /// The shape of this test.
    pub fn shape(&self) -> MimoShape {
        MimoShape::of(self.controls, self.drives)
    }

    /// Every dimension consistent.
    pub fn check(&self) -> Result<(), ShapeError> {
        if self.controls == 0 || self.drives == 0 || self.lines == 0 {
            return Err(ShapeError(
                "controls, drives and lines must all be at least 1".into(),
            ));
        }
        if !(self.df_hz.is_finite() && self.df_hz > 0.0) {
            return Err(ShapeError(format!(
                "df_hz must be positive and finite, got {}",
                self.df_hz
            )));
        }
        let (lo, hi) = self.band;
        if lo >= hi || hi > self.lines {
            return Err(ShapeError(format!(
                "band [{lo}, {hi}) is not inside 0..{}",
                self.lines
            )));
        }
        if self.reference.lines() != self.lines || self.reference.n() != self.controls {
            return Err(ShapeError(format!(
                "reference is {} lines by {}, the setup is {} lines by {} controls",
                self.reference.lines(),
                self.reference.n(),
                self.lines,
                self.controls
            )));
        }
        self.tolerances.check(self.lines, self.controls)
    }
}

/// Everything a law sees on one loop. All stacks are on the setup's grid, in line
/// order, at full level.
#[derive(Clone, Debug)]
pub struct MimoLoopInputs<'a> {
    /// The reference SDM at full level, controls by controls.
    pub reference: &'a HermitianStack,
    /// The alarm and abort lines per control.
    pub tolerances: &'a Tolerances,
    /// The measured control response SDM scaled to full level, controls by
    /// controls. At the seed call this is the identification response.
    pub response: &'a HermitianStack,
    /// Statistical degrees of freedom of `response`.
    pub response_dof: f64,
    /// The plant estimate the core is using (held or updated), controls by drives,
    /// response units per volt.
    pub frf: &'a MatrixStack,
    /// Multiple coherence of each control on all drives, line-major
    /// (`lines * controls` values): the identification's while the plant estimate
    /// is held, the loop's own running estimate while it is updated. With the
    /// update blended in below 1, `frf` is a mix of the two and this is the
    /// loop's alone.
    pub multiple_coherence: &'a [f64],
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// The half-open controlled band `[lo, hi)` in lines.
    pub band: (usize, usize),
    /// 0 for the seed, then 1, 2, ... for each iteration.
    pub loop_index: u64,
    /// Present only when the law declared [`MimoInput::LastDrive`].
    pub last_drive: Option<&'a HermitianStack>,
    /// Present only when the law declared [`MimoInput::ResponseDriveCsd`].
    pub response_drive_csd: Option<&'a MatrixStack>,
    /// Present only when the law declared [`MimoInput::SysidResponse`].
    pub sysid_response: Option<&'a HermitianStack>,
    /// Present only when the law declared [`MimoInput::SysidDrive`].
    pub sysid_drive: Option<&'a HermitianStack>,
    /// Present only when the law declared [`MimoInput::SysidNoise`].
    pub sysid_noise: Option<&'a HermitianStack>,
}

impl MimoLoopInputs<'_> {
    /// Every dimension consistent with `setup`, and every declared input present.
    pub fn check(&self, setup: &MimoSetup, needs: &[MimoInput]) -> Result<(), ShapeError> {
        let (lines, nc, nd) = (setup.lines, setup.controls, setup.drives);
        let herm = |name: &str, s: &HermitianStack, n: usize| -> Result<(), ShapeError> {
            if s.lines() != lines || s.n() != n {
                return Err(ShapeError(format!(
                    "{name} is {} lines by {}, expected {lines} by {n}",
                    s.lines(),
                    s.n()
                )));
            }
            Ok(())
        };
        herm("reference", self.reference, nc)?;
        herm("response", self.response, nc)?;
        self.tolerances.check(lines, nc)?;
        if self.frf.dims() != (lines, nc, nd) {
            return Err(ShapeError(format!(
                "frf is {:?}, expected ({lines}, {nc}, {nd})",
                self.frf.dims()
            )));
        }
        if self.multiple_coherence.len() != lines * nc {
            return Err(ShapeError(format!(
                "multiple coherence has {} values, expected {}",
                self.multiple_coherence.len(),
                lines * nc
            )));
        }
        if self.band != setup.band || self.df_hz != setup.df_hz {
            return Err(ShapeError("band or df_hz differ from the setup".into()));
        }
        for need in needs {
            match need {
                MimoInput::LastDrive => match self.last_drive {
                    Some(s) => herm("last_drive", s, nd)?,
                    None => {
                        return Err(ShapeError(
                            "last_drive was declared but not supplied".into(),
                        ))
                    }
                },
                MimoInput::ResponseDriveCsd => match self.response_drive_csd {
                    Some(m) if m.dims() == (lines, nc, nd) => {}
                    Some(m) => {
                        return Err(ShapeError(format!(
                            "response_drive_csd is {:?}, expected ({lines}, {nc}, {nd})",
                            m.dims()
                        )))
                    }
                    None => {
                        return Err(ShapeError(
                            "response_drive_csd was declared but not supplied".into(),
                        ))
                    }
                },
                MimoInput::SysidResponse => match self.sysid_response {
                    Some(s) => herm("sysid_response", s, nc)?,
                    None => {
                        return Err(ShapeError(
                            "sysid_response was declared but not supplied".into(),
                        ))
                    }
                },
                MimoInput::SysidDrive => match self.sysid_drive {
                    Some(s) => herm("sysid_drive", s, nd)?,
                    None => {
                        return Err(ShapeError(
                            "sysid_drive was declared but not supplied".into(),
                        ))
                    }
                },
                MimoInput::SysidNoise => match self.sysid_noise {
                    Some(s) => herm("sysid_noise", s, nc)?,
                    None => {
                        return Err(ShapeError(
                            "sysid_noise was declared but not supplied".into(),
                        ))
                    }
                },
            }
        }
        Ok(())
    }
}

/// A value a law reports for a loop under a key its manifest declares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DiagnosticData {
    /// One number for the loop.
    Scalar(f64),
    /// One number per line of the grid.
    PerLine(Vec<f64>),
    /// One number per control channel per line, line-major.
    PerControlLine(Vec<f64>),
    /// One number per drive per line, line-major.
    PerDriveLine(Vec<f64>),
}

/// One named diagnostic. Shown and recorded; never relied on for protection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticValue {
    pub key: String,
    pub data: DiagnosticData,
}

/// One loop's result: the next full-level drive SDM, drives by drives per line,
/// which the core checks (finite, Hermitian, positive semi-definite, zero out of
/// band) before anything else happens to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MimoLoopOutputs {
    pub drive: HermitianStack,
    pub diagnostics: Vec<DiagnosticValue>,
}

/// A law that could not compute a sound drive says so instead of guessing. The core
/// holds the previous drive and counts the miss.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LawError(pub String);

impl fmt::Display for LawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LawError {}

impl From<ShapeError> for LawError {
    fn from(e: ShapeError) -> Self {
        LawError(e.0)
    }
}

impl From<crate::linalg::LinalgError> for LawError {
    fn from(e: crate::linalg::LinalgError) -> Self {
        LawError(e.to_string())
    }
}

/// The native MIMO control-law boundary. A law file built against the
/// `random-mimo` world is adapted to this trait by the host; the built-in laws
/// implement it directly and are also built as law files from the same source.
pub trait MimoControlLaw {
    /// The optional inputs this law reads. The core supplies only these.
    fn needs(&self) -> &[MimoInput];

    /// The shapes this law supports. The core refuses a test of another shape.
    fn shapes(&self) -> &[MimoShape];

    /// Called once after system identification, with the identification response
    /// as `response`. Returns the first full-level drive.
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError>;

    /// Called once per control loop with a fresh estimate. Returns the next
    /// full-level drive.
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::C64;

    fn setup(nc: usize, nd: usize, lines: usize) -> MimoSetup {
        let mut reference = HermitianStack::zeros(lines, nc);
        for line in 0..lines {
            for i in 0..nc {
                reference.set(line, i, i, C64::new(1.0, 0.0));
            }
        }
        MimoSetup {
            controls: nc,
            drives: nd,
            lines,
            df_hz: 1.0,
            band: (1, lines - 1),
            reference,
            tolerances: Tolerances::none(lines, nc),
        }
    }

    #[test]
    fn shapes_follow_the_counts() {
        assert_eq!(MimoShape::of(3, 3), MimoShape::Square);
        assert_eq!(MimoShape::of(72, 24), MimoShape::MoreControlsThanDrives);
        assert_eq!(MimoShape::of(4, 6), MimoShape::FewerControlsThanDrives);
        assert_eq!(setup(72, 24, 8).shape(), MimoShape::MoreControlsThanDrives);
    }

    #[test]
    fn a_consistent_setup_passes_and_a_wrong_reference_is_named() {
        assert!(setup(3, 2, 8).check().is_ok());
        let mut s = setup(3, 2, 8);
        s.reference = HermitianStack::zeros(8, 2);
        let e = s.check().unwrap_err();
        assert!(e.0.contains("reference"), "{e}");
        let mut s = setup(3, 2, 8);
        s.band = (4, 9);
        assert!(s.check().is_err());
        let mut s = setup(3, 2, 8);
        s.tolerances.abort_upper.pop();
        assert!(s.check().unwrap_err().0.contains("abort_upper"));
    }

    #[test]
    fn declared_inputs_must_be_present_and_shaped() {
        let s = setup(3, 2, 8);
        let response = HermitianStack::zeros(8, 3);
        let frf = MatrixStack::zeros(8, 3, 2);
        let coh = vec![1.0; 24];
        let mut inputs = MimoLoopInputs {
            reference: &s.reference,
            tolerances: &s.tolerances,
            response: &response,
            response_dof: 64.0,
            frf: &frf,
            multiple_coherence: &coh,
            df_hz: 1.0,
            band: (1, 7),
            loop_index: 0,
            last_drive: None,
            response_drive_csd: None,
            sysid_response: None,
            sysid_drive: None,
            sysid_noise: None,
        };
        assert!(inputs.check(&s, &[]).is_ok());
        let e = inputs.check(&s, &[MimoInput::LastDrive]).unwrap_err();
        assert!(e.0.contains("last_drive"), "{e}");
        let wrong = HermitianStack::zeros(8, 3);
        inputs.last_drive = Some(&wrong);
        assert!(inputs.check(&s, &[MimoInput::LastDrive]).is_err());
        let right = HermitianStack::zeros(8, 2);
        inputs.last_drive = Some(&right);
        assert!(inputs.check(&s, &[MimoInput::LastDrive]).is_ok());
        let bad_frf = MatrixStack::zeros(8, 2, 3);
        inputs.frf = &bad_frf;
        assert!(inputs.check(&s, &[]).unwrap_err().0.contains("frf"));
    }
}
