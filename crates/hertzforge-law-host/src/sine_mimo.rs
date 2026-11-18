//! MIMO Sine law files: the `sine-mimo` world (`wit/control-law/sine-mimo.wit`).
//!
//! A MIMO Sine law is called once per control update on the controller's
//! real-time thread: `engage` for the first drive after identification, `tick`
//! for every update after it. It returns one complex amplitude per drive, its
//! own fault judgement, up to [`MAX_SINE_MIMO_VALUES`] declared values, and
//! optionally a request to hold or slow the sweep and a notched reference.
//!
//! This module loads such a file and checks its manifest ([`LoadedSineMimoLaw`],
//! [`validate_sine_mimo_manifest`]): the shapes, the test types and whether
//! the law accepts an amplitude-only reference. Building a law for a run and
//! calling it are not in this version.
//!
//! The types are the world's own records, as the bindings generate them.

use crate::convert;
use crate::error::LawError;
use crate::host::LawHost;
use crate::sandbox::HostState;
use crate::types::{LawIdentity, Manifest, Param};
use crate::validate;

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "sine-mimo",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

pub(crate) use bindings::SineMimoPre;

pub use bindings::hertzforge::control_law::mimo_sine_types::{
    Complex, Hold, Identification, MatrixStack, ProportionalTracking, Request, SineMimoInputs,
    SineMimoOutputs, SineMimoSetup, SineShape, TestType, Tolerance, Tracking,
};

/// The most values a MIMO Sine law may report on one call.
pub const MAX_SINE_MIMO_VALUES: usize = 8;
/// The most control channels a MIMO Sine law is built for.
pub const MAX_SINE_MIMO_CONTROLS: usize = 16;
/// The most drives a MIMO Sine law commands.
pub const MAX_SINE_MIMO_DRIVES: usize = 8;
/// The most limit channels a MIMO Sine run has.
pub const MAX_SINE_MIMO_LIMITS: usize = 8;

/// What a MIMO Sine law file says about itself: the manifest every law has,
/// the shapes and test types it accepts (at least one of each, each once) and
/// whether it accepts an amplitude-only reference.
#[derive(Debug, Clone, PartialEq)]
pub struct SineMimoManifest {
    /// Name, version, author, summary, parameters and values.
    pub law: Manifest,
    /// The shapes the law accepts.
    pub shapes: Vec<SineShape>,
    /// The test types the law accepts.
    pub test_types: Vec<TestType>,
    /// Whether the law accepts an amplitude-only reference.
    pub amplitude_only: bool,
}

impl SineMimoManifest {
    /// Whether the law accepts a test of `shape`.
    pub fn accepts_shape(&self, shape: SineShape) -> bool {
        self.shapes.contains(&shape)
    }

    /// Whether the law accepts a test of `test_type`.
    pub fn accepts_test_type(&self, test_type: TestType) -> bool {
        self.test_types.contains(&test_type)
    }
}

/// The stable name of a shape: `square`, `more_controls_than_drives` or
/// `fewer_controls_than_drives`.
pub fn shape_name(shape: SineShape) -> &'static str {
    match shape {
        SineShape::Square => "square",
        SineShape::MoreControlsThanDrives => "more_controls_than_drives",
        SineShape::FewerControlsThanDrives => "fewer_controls_than_drives",
    }
}

/// The stable name of a test type: `sweep`, `dwell` or `stepped`.
pub fn test_type_name(test_type: TestType) -> &'static str {
    match test_type {
        TestType::Sweep => "sweep",
        TestType::Dwell => "dwell",
        TestType::Stepped => "stepped",
    }
}

fn listed_twice<T: PartialEq>(v: &[T]) -> bool {
    v.iter().enumerate().any(|(i, k)| v[..i].contains(k))
}

/// Check a MIMO Sine manifest: the rules every manifest follows, at most
/// [`MAX_SINE_MIMO_VALUES`] values, at least one shape and one test type, and
/// none listed twice.
pub fn validate_sine_mimo_manifest(m: &SineMimoManifest) -> Result<(), LawError> {
    validate::validate_manifest(&m.law)?;
    let bad = |s: String| Err(LawError::ManifestInvalid(s));
    if m.law.diagnostics.len() > MAX_SINE_MIMO_VALUES {
        return bad(format!(
            "{} values declared; a MIMO Sine law reports at most {MAX_SINE_MIMO_VALUES}",
            m.law.diagnostics.len()
        ));
    }
    if m.shapes.is_empty() {
        return bad("the law accepts no shape; it must accept at least one".into());
    }
    if listed_twice(&m.shapes) {
        return bad("a shape is listed twice".into());
    }
    if m.test_types.is_empty() {
        return bad(
            "the law accepts no test type; it must accept sweep, dwell, stepped or several".into(),
        );
    }
    if listed_twice(&m.test_types) {
        return bad("a test type is listed twice".into());
    }
    Ok(())
}

/// A compiled MIMO Sine law with a checked manifest.
#[derive(Clone)]
pub struct LoadedSineMimoLaw {
    identity: LawIdentity,
    manifest: SineMimoManifest,
    /// The linked law, instantiated once per run when a run builds it.
    #[allow(dead_code)]
    pre: SineMimoPre<HostState>,
}

impl LoadedSineMimoLaw {
    /// Read and check the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: SineMimoPre<HostState>,
    ) -> Result<Self, LawError> {
        let mut sb = host.sandbox();
        let _active = host.ticker.active();
        sb.arm(host.config.describe_budget);
        let bindings = pre
            .instantiate(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let m = bindings
            .call_describe(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let manifest = SineMimoManifest {
            law: convert::manifest(m.law),
            shapes: m.shapes,
            test_types: m.test_types,
            amplitude_only: m.amplitude_only,
        };
        validate_sine_mimo_manifest(&manifest)?;
        Ok(LoadedSineMimoLaw {
            identity: LawIdentity {
                name: manifest.law.name.clone(),
                version: manifest.law.version.clone(),
                author: manifest.law.author.clone(),
                sha256,
            },
            manifest,
            pre,
        })
    }

    /// Name, version, author and file hash.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's checked manifest.
    pub fn manifest(&self) -> &SineMimoManifest {
        &self.manifest
    }

    /// Check `supplied` parameter values against the manifest and complete
    /// them with defaults, for a run of `controls` control channels (a
    /// per-channel parameter holds one value per control).
    pub fn resolve_params(
        &self,
        supplied: &[Param],
        controls: usize,
    ) -> Result<Vec<Param>, LawError> {
        validate::resolve_params(&self.manifest.law, supplied, controls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SineMimoManifest {
        SineMimoManifest {
            law: Manifest {
                name: "t".into(),
                version: "1".into(),
                author: String::new(),
                summary: String::new(),
                params: Vec::new(),
                diagnostics: Vec::new(),
            },
            shapes: vec![SineShape::Square],
            test_types: vec![TestType::Sweep, TestType::Dwell],
            amplitude_only: false,
        }
    }

    fn refused(m: &SineMimoManifest) -> String {
        match validate_sine_mimo_manifest(m) {
            Err(LawError::ManifestInvalid(s)) => s,
            other => panic!("not refused as invalid: {other:?}"),
        }
    }

    #[test]
    fn mimo_sine_a_manifest_names_its_shapes_and_test_types() {
        let m = manifest();
        validate_sine_mimo_manifest(&m).unwrap();
        assert!(m.accepts_shape(SineShape::Square));
        assert!(!m.accepts_shape(SineShape::MoreControlsThanDrives));
        assert!(m.accepts_test_type(TestType::Dwell));
        assert!(!m.accepts_test_type(TestType::Stepped));
        let mut m2 = m.clone();
        m2.shapes.clear();
        assert!(refused(&m2).contains("no shape"));
        let mut m2 = m.clone();
        m2.shapes.push(SineShape::Square);
        assert!(refused(&m2).contains("shape is listed twice"));
        let mut m2 = m.clone();
        m2.test_types.clear();
        assert!(refused(&m2).contains("no test type"));
        let mut m2 = m;
        m2.test_types.push(TestType::Sweep);
        assert!(refused(&m2).contains("test type is listed twice"));
    }

    #[test]
    fn mimo_sine_names_are_stable() {
        assert_eq!(shape_name(SineShape::Square), "square");
        assert_eq!(
            shape_name(SineShape::MoreControlsThanDrives),
            "more_controls_than_drives"
        );
        assert_eq!(
            shape_name(SineShape::FewerControlsThanDrives),
            "fewer_controls_than_drives"
        );
        assert_eq!(test_type_name(TestType::Sweep), "sweep");
        assert_eq!(test_type_name(TestType::Dwell), "dwell");
        assert_eq!(test_type_name(TestType::Stepped), "stepped");
    }
}
