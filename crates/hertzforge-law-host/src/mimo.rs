//! Multiple-output Random law files: [`LoadedMimoLaw`] is one compiled law,
//! [`MimoLawInstance`] one law built for a run, and [`WasmMimoLaw`] runs an
//! instance as the controller's [`MimoControlLaw`].
//!
//! The law is written against the MIMO law boundary of `hertzforge-sdm`; the
//! host carries that boundary's values across the `random-mimo` interface
//! (Hermitian stacks packed, rectangular stacks in full, optional inputs only
//! when the manifest declares them) and checks them both ways. What a law
//! returns is refused, as a failed call naming the rule, when it is not a
//! well-formed drive ([`MimoOutputRule`]): the wrong dimensions, a non-finite
//! value, a line that is not Hermitian within [`HERMITIAN_TOLERANCE`], a drive
//! outside the band, or a diagnostic that is not as declared. A drive that is
//! well formed but not positive semi-definite is passed on: the controller
//! repairs a small negative eigenvalue and refuses a large one, and applies
//! every cap and limit, for every law alike.

use crate::convert;
use crate::error::{LawError, MimoOutputRule};
use crate::host::LawHost;
use crate::mimo_bindings::hertzforge::control_law::mimo_types as wit;
use crate::mimo_bindings::{RandomMimo, RandomMimoPre};
use crate::sandbox::{HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{GuestOutput, LawIdentity, Manifest, Param};
use crate::validate::{self, MAX_CONTROL_CHANNELS};
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs,
    MimoSetup, MimoShape, Tolerances,
};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::ResourceAny;
use wasmtime::Engine;

type WitBand = crate::bindings::hertzforge::control_law::types::Band;

/// The largest relative Hermitian error, `||G - G^H|| / ||G||` (Frobenius,
/// per line), a drive may have. A packed matrix can only break it with an
/// imaginary part on its diagonal.
pub const HERMITIAN_TOLERANCE: f64 = 1e-9;

/// Most drives a multiple-output law may command.
pub const MAX_DRIVES: usize = 1024;

// ---------------------------------------------------------------------------
// Plain types
// ---------------------------------------------------------------------------

/// The layout of a diagnostic's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// One number for the loop.
    Scalar,
    /// One number per line.
    PerLine,
    /// One number per control channel per line, line-major.
    PerControlLine,
    /// One number per drive per line, line-major.
    PerDriveLine,
}

impl DiagnosticKind {
    /// The layout of `data`.
    pub fn of(data: &DiagnosticData) -> Self {
        match data {
            DiagnosticData::Scalar(_) => DiagnosticKind::Scalar,
            DiagnosticData::PerLine(_) => DiagnosticKind::PerLine,
            DiagnosticData::PerControlLine(_) => DiagnosticKind::PerControlLine,
            DiagnosticData::PerDriveLine(_) => DiagnosticKind::PerDriveLine,
        }
    }

    /// How many values this layout holds on a grid of `lines` lines with
    /// `controls` control channels and `drives` drives.
    pub fn len(self, lines: usize, controls: usize, drives: usize) -> usize {
        match self {
            DiagnosticKind::Scalar => 1,
            DiagnosticKind::PerLine => lines,
            DiagnosticKind::PerControlLine => lines * controls,
            DiagnosticKind::PerDriveLine => lines * drives,
        }
    }

    fn name(self) -> &'static str {
        match self {
            DiagnosticKind::Scalar => "scalar",
            DiagnosticKind::PerLine => "per-line",
            DiagnosticKind::PerControlLine => "per-control-line",
            DiagnosticKind::PerDriveLine => "per-drive-line",
        }
    }
}

/// The layout of the diagnostic declared under `key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticLayout {
    /// A key the law's manifest declares.
    pub key: String,
    /// The layout of its values.
    pub kind: DiagnosticKind,
}

/// What a multiple-output law says about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct MimoManifest {
    /// Name, version, author, summary, parameters and diagnostics. A
    /// per-channel parameter holds one value per control channel.
    pub law: Manifest,
    /// The shapes the law supports.
    pub shapes: Vec<MimoShape>,
    /// The optional inputs the law reads.
    pub needs: Vec<MimoInput>,
    /// The layout of every declared diagnostic.
    pub diagnostic_layouts: Vec<DiagnosticLayout>,
}

impl MimoManifest {
    /// The declared layout of the diagnostic `key`.
    pub fn layout(&self, key: &str) -> Option<DiagnosticKind> {
        self.diagnostic_layouts
            .iter()
            .find(|l| l.key == key)
            .map(|l| l.kind)
    }

    /// Whether the law supports a test of `shape`.
    pub fn supports(&self, shape: MimoShape) -> bool {
        self.shapes.contains(&shape)
    }

    /// Whether the law reads the optional input `input`.
    pub fn needs(&self, input: MimoInput) -> bool {
        self.needs.contains(&input)
    }
}

/// The dimensions a law instance was built for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MimoDims {
    /// Lines on the grid.
    pub lines: usize,
    /// Control channels.
    pub controls: usize,
    /// Drives.
    pub drives: usize,
    /// The band `[lo, hi)`.
    pub band: (usize, usize),
    /// Line spacing, Hz.
    pub df_hz: f64,
}

impl MimoDims {
    /// The dimensions of `setup`.
    pub fn of(setup: &MimoSetup) -> Self {
        MimoDims {
            lines: setup.lines,
            controls: setup.controls,
            drives: setup.drives,
            band: setup.band,
            df_hz: setup.df_hz,
        }
    }
}

/// A drive exactly as a law returned it: packed, nothing dropped (the
/// diagonal keeps any imaginary part, so the Hermitian rule can see it).
#[derive(Debug, Clone, PartialEq)]
pub struct PackedDrive {
    /// Lines the law says it holds.
    pub lines: usize,
    /// Matrix order the law says it holds.
    pub n: usize,
    /// The packed values.
    pub data: Vec<C64>,
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

fn manifest_err(msg: impl Into<String>) -> LawError {
    LawError::ManifestInvalid(msg.into())
}

/// Check a multiple-output manifest: the rules every manifest meets, at least
/// one shape and none twice, no need twice, and exactly one layout for each
/// declared diagnostic and none for an undeclared key.
pub fn validate_mimo_manifest(m: &MimoManifest) -> Result<(), LawError> {
    validate::validate_manifest(&m.law)?;
    if m.shapes.is_empty() {
        return Err(manifest_err("the law supports no shape"));
    }
    let mut shapes = HashSet::new();
    for s in &m.shapes {
        if !shapes.insert(*s) {
            return Err(manifest_err(format!("shape {s:?} is listed twice")));
        }
    }
    let mut needs = HashSet::new();
    for n in &m.needs {
        if !needs.insert(*n) {
            return Err(manifest_err(format!("input {n:?} is needed twice")));
        }
    }
    let mut laid_out = HashSet::new();
    for l in &m.diagnostic_layouts {
        if !m.law.diagnostics.iter().any(|d| d.key == l.key) {
            return Err(manifest_err(format!(
                "a layout is given for `{}`, which is not a declared diagnostic",
                l.key
            )));
        }
        if !laid_out.insert(l.key.as_str()) {
            return Err(manifest_err(format!(
                "diagnostic `{}` has two layouts",
                l.key
            )));
        }
    }
    if let Some(d) = m
        .law
        .diagnostics
        .iter()
        .find(|d| !laid_out.contains(d.key.as_str()))
    {
        return Err(manifest_err(format!(
            "diagnostic `{}` has no layout",
            d.key
        )));
    }
    Ok(())
}

fn tolerance_values(name: &str, v: &[f64]) -> Result<(), String> {
    if let Some((k, x)) = v
        .iter()
        .enumerate()
        .find(|(_, x)| !(x.is_nan() || (x.is_finite() && **x >= 0.0)))
    {
        return Err(format!(
            "tolerance {name} value {k} is {x}: a line is a finite, non-negative PSD, or NaN for \
             none"
        ));
    }
    Ok(())
}

fn tolerances(t: &Tolerances, lines: usize, controls: usize) -> Result<(), String> {
    for (name, v) in [
        ("alarm upper", &t.alarm_upper),
        ("alarm lower", &t.alarm_lower),
        ("abort upper", &t.abort_upper),
        ("abort lower", &t.abort_lower),
    ] {
        if v.len() != lines * controls {
            return Err(format!(
                "tolerance {name} has {} values, {lines} lines by {controls} controls need {}",
                v.len(),
                lines * controls
            ));
        }
        tolerance_values(name, v)?;
    }
    Ok(())
}

fn shape_name(s: MimoShape) -> &'static str {
    match s {
        MimoShape::Square => "square",
        MimoShape::MoreControlsThanDrives => "more controls than drives",
        MimoShape::FewerControlsThanDrives => "fewer controls than drives",
    }
}

/// Check a setup for a law with manifest `m`: every dimension consistent, the
/// grid within the interface's 32-bit counts, 1 to
/// [`MAX_CONTROL_CHANNELS`] controls and 1 to [`MAX_DRIVES`] drives, a finite
/// reference, tolerance lines that are PSDs or NaN, and a shape the law
/// supports.
pub fn validate_mimo_setup(s: &MimoSetup, m: &MimoManifest) -> Result<(), LawError> {
    let fail = |msg: String| Err(LawError::SetupInvalid(msg));
    if let Err(e) = s.check() {
        return fail(e.0);
    }
    if u32::try_from(s.lines).is_err() {
        return fail(format!(
            "{} lines is more than the interface carries",
            s.lines
        ));
    }
    if s.controls > MAX_CONTROL_CHANNELS {
        return fail(format!(
            "{} control channels; at most {MAX_CONTROL_CHANNELS} are accepted",
            s.controls
        ));
    }
    if s.drives > MAX_DRIVES {
        return fail(format!(
            "{} drives; at most {MAX_DRIVES} are accepted",
            s.drives
        ));
    }
    if !s.reference.is_finite() {
        return fail("the reference holds a value that is not finite".into());
    }
    if let Err(e) = tolerances(&s.tolerances, s.lines, s.controls) {
        return fail(e);
    }
    let shape = s.shape();
    if !m.supports(shape) {
        let supported: Vec<&str> = m.shapes.iter().map(|s| shape_name(*s)).collect();
        return fail(format!(
            "this law supports {}, not a test with {} controls and {} drives ({})",
            supported.join(", "),
            s.controls,
            s.drives,
            shape_name(shape)
        ));
    }
    Ok(())
}

fn herm(name: &str, h: &HermitianStack, lines: usize, n: usize) -> Result<(), String> {
    if h.lines() != lines || h.n() != n {
        return Err(format!(
            "{name} is {} lines by {}, expected {lines} by {n}",
            h.lines(),
            h.n()
        ));
    }
    if !h.is_finite() {
        return Err(format!("{name} holds a value that is not finite"));
    }
    Ok(())
}

fn rect(name: &str, m: &MatrixStack, lines: usize, rows: usize, cols: usize) -> Result<(), String> {
    if m.dims() != (lines, rows, cols) {
        let (l, r, c) = m.dims();
        return Err(format!(
            "{name} is {l} lines by {r} x {c}, expected {lines} by {rows} x {cols}"
        ));
    }
    if !m.is_finite() {
        return Err(format!("{name} holds a value that is not finite"));
    }
    Ok(())
}

/// Check one loop's inputs against the dimensions `d` the law was built for:
/// every stack the right size and finite, every input in `needs` present, the
/// multiple coherence finite and non-negative, the tolerance lines PSDs or
/// NaN, and the band and line spacing those of the setup. These are the
/// caller's numbers: a failure here is never the law's.
pub fn validate_mimo_inputs(
    i: &MimoLoopInputs<'_>,
    d: &MimoDims,
    needs: &[MimoInput],
) -> Result<(), LawError> {
    let (lines, nc, nd) = (d.lines, d.controls, d.drives);
    let check = || -> Result<(), String> {
        herm("the reference", i.reference, lines, nc)?;
        herm("the response", i.response, lines, nc)?;
        rect("the FRF", i.frf, lines, nc, nd)?;
        tolerances(i.tolerances, lines, nc)?;
        if i.multiple_coherence.len() != lines * nc {
            return Err(format!(
                "the multiple coherence has {} values, expected {}",
                i.multiple_coherence.len(),
                lines * nc
            ));
        }
        if let Some((k, x)) = i
            .multiple_coherence
            .iter()
            .enumerate()
            .find(|(_, x)| !(x.is_finite() && **x >= 0.0))
        {
            return Err(format!(
                "multiple coherence value {k} is {x}, not a finite, non-negative number"
            ));
        }
        if !(i.response_dof.is_finite() && i.response_dof >= 0.0) {
            return Err(format!(
                "degrees of freedom {} is not a finite, non-negative number",
                i.response_dof
            ));
        }
        if i.band != d.band {
            return Err(format!(
                "the band [{}, {}) is not the setup's [{}, {})",
                i.band.0, i.band.1, d.band.0, d.band.1
            ));
        }
        if i.df_hz != d.df_hz {
            return Err(format!(
                "the line spacing {} Hz is not the setup's {} Hz",
                i.df_hz, d.df_hz
            ));
        }
        for need in needs {
            let missing = || format!("{need:?} is declared by the law but was not supplied");
            match need {
                MimoInput::LastDrive => herm(
                    "the last drive",
                    i.last_drive.ok_or_else(missing)?,
                    lines,
                    nd,
                )?,
                MimoInput::ResponseDriveCsd => rect(
                    "the response-drive cross spectrum",
                    i.response_drive_csd.ok_or_else(missing)?,
                    lines,
                    nc,
                    nd,
                )?,
                MimoInput::SysidResponse => herm(
                    "the identification response",
                    i.sysid_response.ok_or_else(missing)?,
                    lines,
                    nc,
                )?,
                MimoInput::SysidDrive => herm(
                    "the identification drive",
                    i.sysid_drive.ok_or_else(missing)?,
                    lines,
                    nd,
                )?,
                MimoInput::SysidNoise => herm(
                    "the identification noise floor",
                    i.sysid_noise.ok_or_else(missing)?,
                    lines,
                    nc,
                )?,
            }
        }
        Ok(())
    };
    check().map_err(LawError::InputsInvalid)
}

/// Check one loop's drive and diagnostics against the dimensions `d` and the
/// manifest's layouts. A failure is [`LawError::BadMimoOutput`] naming the
/// first rule broken, in [`MimoOutputRule`] order. Positive
/// semi-definiteness is not checked: the controller owns it.
pub fn validate_mimo_outputs(
    drive: &PackedDrive,
    diagnostics: &[DiagnosticValue],
    d: &MimoDims,
    layouts: &[DiagnosticLayout],
) -> Result<(), LawError> {
    let bad = |rule: MimoOutputRule, detail: String| Err(LawError::BadMimoOutput { rule, detail });
    let n = d.drives;
    let per = HermitianStack::packed_len(n);
    if drive.lines != d.lines || drive.n != n || drive.data.len() != d.lines * per {
        return bad(
            MimoOutputRule::Dimensions,
            format!(
                "the drive is {} lines of {} x {} with {} packed values; the run needs {} lines \
                 of {n} x {n} ({} values)",
                drive.lines,
                drive.n,
                drive.n,
                drive.data.len(),
                d.lines,
                d.lines * per
            ),
        );
    }
    if let Some((k, v)) = drive
        .data
        .iter()
        .enumerate()
        .find(|(_, v)| !(v.re.is_finite() && v.im.is_finite()))
    {
        return bad(
            MimoOutputRule::NonFinite,
            format!(
                "drive line {} holds {v}, not a finite number",
                k / per.max(1)
            ),
        );
    }
    // The packed line in full (the diagonal keeps its imaginary part), so the
    // stack crate's own measure applies.
    let mut full = MatrixStack::zeros(1, n, n);
    for line in 0..d.lines {
        let packed = &drive.data[line * per..(line + 1) * per];
        for r in 0..n {
            for c in r..n {
                let v = packed[HermitianStack::packed_index(n, r, c)];
                full.set(0, r, c, v);
                if c != r {
                    full.set(0, c, r, v.conj());
                }
            }
        }
        let e = full
            .hermitian_error(0)
            .map_err(|e| LawError::Host(e.to_string()))?;
        if e > HERMITIAN_TOLERANCE {
            return bad(
                MimoOutputRule::NotHermitian,
                format!(
                    "drive line {line} is not Hermitian: relative error {e:.3e} is above \
                     {HERMITIAN_TOLERANCE:e} (a diagonal value is not real)"
                ),
            );
        }
    }
    let (lo, hi) = d.band;
    if let Some(line) = (0..d.lines).filter(|l| *l < lo || *l >= hi).find(|l| {
        drive.data[l * per..(l + 1) * per]
            .iter()
            .any(|v| *v != C64::new(0.0, 0.0))
    }) {
        return bad(
            MimoOutputRule::OutOfBand,
            format!("drive line {line} is outside the band [{lo}, {hi}) and is not zero"),
        );
    }
    let mut seen = HashSet::new();
    for v in diagnostics {
        let diag = |detail: String| bad(MimoOutputRule::Diagnostics, detail);
        let Some(kind) = layouts.iter().find(|l| l.key == v.key).map(|l| l.kind) else {
            return diag(format!(
                "diagnostic `{}` is not declared in the manifest",
                v.key
            ));
        };
        if !seen.insert(v.key.as_str()) {
            return diag(format!("diagnostic `{}` is reported twice", v.key));
        }
        let got = DiagnosticKind::of(&v.data);
        if got != kind {
            return diag(format!(
                "diagnostic `{}` is declared {} but reported {}",
                v.key,
                kind.name(),
                got.name()
            ));
        }
        let values: &[f64] = match &v.data {
            DiagnosticData::Scalar(x) => std::slice::from_ref(x),
            DiagnosticData::PerLine(x)
            | DiagnosticData::PerControlLine(x)
            | DiagnosticData::PerDriveLine(x) => x,
        };
        let want = kind.len(d.lines, d.controls, d.drives);
        if values.len() != want {
            return diag(format!(
                "diagnostic `{}` has {} values, its {} layout needs {want}",
                v.key,
                values.len(),
                kind.name()
            ));
        }
        if let Some((k, x)) = values.iter().enumerate().find(|(_, x)| !x.is_finite()) {
            return diag(format!(
                "diagnostic `{}` value {k} is {x}, not finite",
                v.key
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn shape_from_wit(s: wit::MimoShape) -> MimoShape {
    match s {
        wit::MimoShape::Square => MimoShape::Square,
        wit::MimoShape::MoreControlsThanDrives => MimoShape::MoreControlsThanDrives,
        wit::MimoShape::FewerControlsThanDrives => MimoShape::FewerControlsThanDrives,
    }
}

fn input_from_wit(i: wit::MimoInput) -> MimoInput {
    match i {
        wit::MimoInput::LastDrive => MimoInput::LastDrive,
        wit::MimoInput::ResponseDriveCsd => MimoInput::ResponseDriveCsd,
        wit::MimoInput::SysidResponse => MimoInput::SysidResponse,
        wit::MimoInput::SysidDrive => MimoInput::SysidDrive,
        wit::MimoInput::SysidNoise => MimoInput::SysidNoise,
    }
}

fn kind_from_wit(k: wit::DiagnosticKind) -> DiagnosticKind {
    match k {
        wit::DiagnosticKind::Scalar => DiagnosticKind::Scalar,
        wit::DiagnosticKind::PerLine => DiagnosticKind::PerLine,
        wit::DiagnosticKind::PerControlLine => DiagnosticKind::PerControlLine,
        wit::DiagnosticKind::PerDriveLine => DiagnosticKind::PerDriveLine,
    }
}

/// A manifest as the law returned it, unchecked.
fn manifest_from_wit(m: wit::MimoManifest) -> MimoManifest {
    MimoManifest {
        law: convert::manifest(m.law),
        shapes: m.shapes.into_iter().map(shape_from_wit).collect(),
        needs: m.needs.into_iter().map(input_from_wit).collect(),
        diagnostic_layouts: m
            .diagnostic_layouts
            .into_iter()
            .map(|l| DiagnosticLayout {
                key: l.key,
                kind: kind_from_wit(l.kind),
            })
            .collect(),
    }
}

fn complex_wit(data: &[C64]) -> Vec<wit::Complex> {
    data.iter()
        .map(|c| wit::Complex { re: c.re, im: c.im })
        .collect()
}

// The dimensions below were checked against the interface's 32-bit counts
// (`validate_mimo_setup`, and every input against the setup), so the
// narrowing casts cannot truncate.
fn hermitian_wit(h: &HermitianStack) -> wit::HermitianStack {
    wit::HermitianStack {
        lines: h.lines() as u32,
        n: h.n() as u32,
        data: complex_wit(h.data()),
    }
}

fn matrix_wit(m: &MatrixStack) -> wit::MatrixStack {
    wit::MatrixStack {
        lines: m.lines() as u32,
        rows: m.rows() as u32,
        cols: m.cols() as u32,
        data: complex_wit(m.data()),
    }
}

fn tolerances_wit(t: &Tolerances) -> wit::ToleranceLines {
    wit::ToleranceLines {
        alarm_upper: t.alarm_upper.clone(),
        alarm_lower: t.alarm_lower.clone(),
        abort_upper: t.abort_upper.clone(),
        abort_lower: t.abort_lower.clone(),
    }
}

fn band_wit(b: (usize, usize)) -> WitBand {
    WitBand {
        lo: b.0 as u32,
        hi: b.1 as u32,
    }
}

/// A checked setup (`validate_mimo_setup` passed).
fn setup_wit(s: &MimoSetup) -> wit::MimoSetup {
    wit::MimoSetup {
        controls: s.controls as u32,
        drives: s.drives as u32,
        lines: s.lines as u32,
        df_hz: s.df_hz,
        band: band_wit(s.band),
        reference: hermitian_wit(&s.reference),
        tolerances: tolerances_wit(&s.tolerances),
    }
}

/// Checked inputs (`validate_mimo_inputs` passed). Only the optional inputs in
/// `needs` are carried; anything else the caller supplied stays behind.
fn inputs_wit(i: &MimoLoopInputs<'_>, needs: &[MimoInput]) -> wit::MimoInputs {
    let want = |n: MimoInput| needs.contains(&n);
    wit::MimoInputs {
        reference: hermitian_wit(i.reference),
        tolerances: tolerances_wit(i.tolerances),
        response: hermitian_wit(i.response),
        response_dof: i.response_dof,
        frf: matrix_wit(i.frf),
        multiple_coherence: i.multiple_coherence.to_vec(),
        df_hz: i.df_hz,
        band: band_wit(i.band),
        loop_index: i.loop_index,
        last_drive: i
            .last_drive
            .filter(|_| want(MimoInput::LastDrive))
            .map(hermitian_wit),
        response_drive_csd: i
            .response_drive_csd
            .filter(|_| want(MimoInput::ResponseDriveCsd))
            .map(matrix_wit),
        sysid_response: i
            .sysid_response
            .filter(|_| want(MimoInput::SysidResponse))
            .map(hermitian_wit),
        sysid_drive: i
            .sysid_drive
            .filter(|_| want(MimoInput::SysidDrive))
            .map(hermitian_wit),
        sysid_noise: i
            .sysid_noise
            .filter(|_| want(MimoInput::SysidNoise))
            .map(hermitian_wit),
    }
}

/// Outputs as the law returned them, unchecked.
fn outputs_from_wit(o: wit::MimoOutputs) -> (PackedDrive, Vec<DiagnosticValue>) {
    let drive = PackedDrive {
        lines: o.drive.lines as usize,
        n: o.drive.n as usize,
        data: o
            .drive
            .data
            .into_iter()
            .map(|c| C64::new(c.re, c.im))
            .collect(),
    };
    let diagnostics = o
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
        .collect();
    (drive, diagnostics)
}

// ---------------------------------------------------------------------------
// Loaded law and instance
// ---------------------------------------------------------------------------

/// A compiled multiple-output law with a checked manifest. Cheap to
/// instantiate as often as needed; each instance has its own memory and
/// state.
#[derive(Clone)]
pub struct LoadedMimoLaw {
    identity: LawIdentity,
    manifest: MimoManifest,
    pre: RandomMimoPre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedMimoLaw {
    /// Read the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: RandomMimoPre<HostState>,
    ) -> Result<Self, LawError> {
        let mut sb = host.sandbox();
        let _active = host.ticker.active();
        sb.arm(host.config.describe_budget);
        let bindings = pre
            .instantiate(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let manifest = bindings
            .call_describe(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let manifest = manifest_from_wit(manifest);
        validate_mimo_manifest(&manifest)?;
        Ok(LoadedMimoLaw {
            identity: LawIdentity {
                name: manifest.law.name.clone(),
                version: manifest.law.version.clone(),
                author: manifest.law.author.clone(),
                sha256,
            },
            manifest,
            pre,
            engine: host.engine.clone(),
            memory_limit_bytes: host.config.memory_limit_bytes,
            capture_bytes: host.config.capture_bytes,
            ticker: Arc::clone(&host.ticker),
        })
    }

    /// Name, version, author and file hash.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's checked manifest.
    pub fn manifest(&self) -> &MimoManifest {
        &self.manifest
    }

    /// Check `supplied` parameter values against the manifest and complete
    /// them with defaults (a per-channel parameter has one value per control
    /// channel): the list [`Self::instantiate`] hands the law.
    pub fn resolve_params(
        &self,
        supplied: &[Param],
        controls: usize,
    ) -> Result<Vec<Param>, LawError> {
        validate::resolve_params(&self.manifest.law, supplied, controls)
    }

    /// Build the law for a run: check the setup (including that the law
    /// supports its shape) and the parameter values, instantiate the law in a
    /// fresh sandbox and call its constructor, all within `budget`.
    pub fn instantiate(
        &self,
        setup: &MimoSetup,
        params: &[Param],
        budget: Duration,
    ) -> Result<MimoLawInstance, LawError> {
        validate_mimo_setup(setup, &self.manifest)?;
        let params = self.resolve_params(params, setup.controls)?;
        let wit_setup = setup_wit(setup);
        let wit_params = convert::params(&params);

        let mut sb = Sandbox::new(&self.engine, self.memory_limit_bytes, self.capture_bytes);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        sb.arm(budget);
        let bindings = self
            .pre
            .instantiate(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let law = bindings
            .hertzforge_control_law_random_mimo_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(MimoLawInstance {
            identity: self.identity.clone(),
            manifest: self.manifest.clone(),
            params,
            dims: MimoDims::of(setup),
            sandbox: sb,
            bindings,
            law,
            ticker,
            poisoned: None,
        })
    }
}

#[derive(Clone, Copy)]
enum LoopCall {
    Seed,
    Iterate,
}

/// One multiple-output law built for one run, with its own sandbox. Every
/// call takes a wall-clock budget; a call that traps, runs past its budget or
/// exceeds the memory cap leaves the instance unusable
/// ([`LawError::Poisoned`] from then on).
pub struct MimoLawInstance {
    identity: LawIdentity,
    manifest: MimoManifest,
    params: Vec<Param>,
    dims: MimoDims,
    sandbox: Sandbox,
    bindings: RandomMimo,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl MimoLawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's manifest.
    pub fn manifest(&self) -> &MimoManifest {
        &self.manifest
    }

    /// The complete parameter values the law was built with, in manifest
    /// order (for the run record).
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// The dimensions the law was built for.
    pub fn dims(&self) -> &MimoDims {
        &self.dims
    }

    /// Whether an earlier failure left this instance unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// The kept tail of what the law wrote to stdout and stderr.
    pub fn guest_output(&self) -> GuestOutput {
        self.sandbox.output()
    }

    /// The first loop, from the system-identification measurement.
    pub fn seed(
        &mut self,
        inputs: &MimoLoopInputs<'_>,
        budget: Duration,
    ) -> Result<MimoLoopOutputs, LawError> {
        self.loop_call(LoopCall::Seed, inputs, budget)
    }

    /// One control loop.
    pub fn iterate(
        &mut self,
        inputs: &MimoLoopInputs<'_>,
        budget: Duration,
    ) -> Result<MimoLoopOutputs, LawError> {
        self.loop_call(LoopCall::Iterate, inputs, budget)
    }

    fn loop_call(
        &mut self,
        which: LoopCall,
        inputs: &MimoLoopInputs<'_>,
        budget: Duration,
    ) -> Result<MimoLoopOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_mimo_inputs(inputs, &self.dims, &self.manifest.needs)?;
        let wit_inputs = inputs_wit(inputs, &self.manifest.needs);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_random_mimo_law().law();
        let r = match which {
            LoopCall::Seed => guest.call_seed(&mut self.sandbox.store, self.law, &wit_inputs),
            LoopCall::Iterate => guest.call_iterate(&mut self.sandbox.store, self.law, &wit_inputs),
        };
        let out = match r {
            Ok(out) => out.map_err(LawError::Law)?,
            Err(e) => {
                let err = self.sandbox.classify(&e);
                self.poisoned = Some(err.to_string());
                return Err(err);
            }
        };
        let (drive, diagnostics) = outputs_from_wit(out);
        validate_mimo_outputs(
            &drive,
            &diagnostics,
            &self.dims,
            &self.manifest.diagnostic_layouts,
        )?;
        let drive = HermitianStack::from_packed(drive.lines, drive.n, drive.data)
            .map_err(|e| LawError::Host(e.to_string()))?;
        Ok(MimoLoopOutputs { drive, diagnostics })
    }
}

// ---------------------------------------------------------------------------
// The controller's law trait
// ---------------------------------------------------------------------------

/// A multiple-output law file built for one run, as the controller's
/// [`MimoControlLaw`].
///
/// Every call carries `budget` as its deadline: the run's own call budget, so
/// the host stops a call that will not return when the controller would stop
/// waiting for it. The boundary's error carries the host's message; the typed
/// error behind it is [`Self::last_error`]. A call that traps, runs past its
/// budget or breaches the memory cap leaves the instance unusable, and every
/// later call fails with that same first error, so the controller's misses in
/// a row run out and it takes its controlled stop.
pub struct WasmMimoLaw {
    instance: MimoLawInstance,
    budget: Duration,
    last_error: Option<LawError>,
    poisoned_by: Option<LawError>,
}

impl WasmMimoLaw {
    /// Run `instance` with `budget` as every call's deadline.
    pub fn new(instance: MimoLawInstance, budget: Duration) -> Self {
        WasmMimoLaw {
            instance,
            budget,
            last_error: None,
            poisoned_by: None,
        }
    }

    /// Build `law` for a run with `params` and run it with `budget` as every
    /// call's deadline (the constructor's too).
    pub fn build(
        law: &LoadedMimoLaw,
        setup: &MimoSetup,
        params: &[Param],
        budget: Duration,
    ) -> Result<Self, LawError> {
        Ok(Self::new(law.instantiate(setup, params, budget)?, budget))
    }

    /// The law instance (its identity, manifest, parameters and kept output).
    pub fn instance(&self) -> &MimoLawInstance {
        &self.instance
    }

    /// Each call's deadline.
    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// The host's typed account of the last call, when it failed: a trap, a
    /// deadline, a breached memory cap, a refused output (naming the rule),
    /// the law's own error, or refused inputs. Cleared by a call that
    /// succeeds.
    pub fn last_error(&self) -> Option<&LawError> {
        self.last_error.as_ref()
    }

    fn finish(
        &mut self,
        r: Result<MimoLoopOutputs, LawError>,
    ) -> Result<MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        match r {
            Ok(out) => {
                self.last_error = None;
                Ok(out)
            }
            Err(e) => {
                let e = match (&e, &self.poisoned_by) {
                    (LawError::Poisoned(_), Some(first)) => first.clone(),
                    _ => e,
                };
                if e.poisons() && self.poisoned_by.is_none() {
                    self.poisoned_by = Some(e.clone());
                }
                let msg = e.to_string();
                self.last_error = Some(e);
                Err(hertzforge_sdm::law::LawError(msg))
            }
        }
    }
}

impl MimoControlLaw for WasmMimoLaw {
    fn needs(&self) -> &[MimoInput] {
        &self.instance.manifest.needs
    }

    fn shapes(&self) -> &[MimoShape] {
        &self.instance.manifest.shapes
    }

    fn seed(
        &mut self,
        inputs: &MimoLoopInputs<'_>,
    ) -> Result<MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        let r = self.instance.seed(inputs, self.budget);
        self.finish(r)
    }

    fn iterate(
        &mut self,
        inputs: &MimoLoopInputs<'_>,
    ) -> Result<MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        let r = self.instance.iterate(inputs, self.budget);
        self.finish(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiagnosticSpec;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    fn dims() -> MimoDims {
        MimoDims {
            lines: 4,
            controls: 3,
            drives: 2,
            band: (1, 3),
            df_hz: 2.0,
        }
    }

    /// A good drive: identity-like in band, zero outside.
    fn good_drive() -> PackedDrive {
        let d = dims();
        let mut h = HermitianStack::zeros(d.lines, d.drives);
        for line in d.band.0..d.band.1 {
            h.set(line, 0, 0, c(2.0, 0.0));
            h.set(line, 1, 1, c(1.0, 0.0));
            h.set(line, 0, 1, c(0.5, -0.5));
        }
        PackedDrive {
            lines: d.lines,
            n: d.drives,
            data: h.into_data(),
        }
    }

    fn layouts() -> Vec<DiagnosticLayout> {
        vec![
            DiagnosticLayout {
                key: "cond".into(),
                kind: DiagnosticKind::PerLine,
            },
            DiagnosticLayout {
                key: "kept".into(),
                kind: DiagnosticKind::PerDriveLine,
            },
        ]
    }

    fn rule(r: Result<(), LawError>) -> Option<MimoOutputRule> {
        match r {
            Err(LawError::BadMimoOutput { rule, .. }) => Some(rule),
            Err(e) => panic!("unexpected error {e}"),
            Ok(()) => None,
        }
    }

    #[test]
    fn a_well_formed_drive_passes_every_rule() {
        let diags = vec![
            DiagnosticValue {
                key: "cond".into(),
                data: DiagnosticData::PerLine(vec![1.0; 4]),
            },
            DiagnosticValue {
                key: "kept".into(),
                data: DiagnosticData::PerDriveLine(vec![2.0; 8]),
            },
        ];
        assert_eq!(
            rule(validate_mimo_outputs(
                &good_drive(),
                &diags,
                &dims(),
                &layouts()
            )),
            None
        );
    }

    #[test]
    fn each_broken_rule_is_named() {
        let d = dims();
        let mut short = good_drive();
        short.data.pop();
        assert_eq!(
            rule(validate_mimo_outputs(&short, &[], &d, &[])),
            Some(MimoOutputRule::Dimensions)
        );
        let mut wrong_n = good_drive();
        wrong_n.n = 3;
        assert_eq!(
            rule(validate_mimo_outputs(&wrong_n, &[], &d, &[])),
            Some(MimoOutputRule::Dimensions)
        );
        let mut nan = good_drive();
        nan.data[4] = c(f64::NAN, 0.0);
        assert_eq!(
            rule(validate_mimo_outputs(&nan, &[], &d, &[])),
            Some(MimoOutputRule::NonFinite)
        );
        // Line 1, element (1, 1): the diagonal given an imaginary part.
        let mut skew = good_drive();
        let per = HermitianStack::packed_len(2);
        skew.data[per + HermitianStack::packed_index(2, 1, 1)] = c(1.0, 0.5);
        assert_eq!(
            rule(validate_mimo_outputs(&skew, &[], &d, &[])),
            Some(MimoOutputRule::NotHermitian)
        );
        // Below the tolerance it passes: the rule is relative, not exact.
        let mut tiny = good_drive();
        tiny.data[per + HermitianStack::packed_index(2, 1, 1)] = c(1.0, 1e-12);
        assert_eq!(rule(validate_mimo_outputs(&tiny, &[], &d, &[])), None);
        let mut oob = good_drive();
        oob.data[0] = c(1e-30, 0.0);
        assert_eq!(
            rule(validate_mimo_outputs(&oob, &[], &d, &[])),
            Some(MimoOutputRule::OutOfBand)
        );
    }

    #[test]
    fn diagnostics_must_be_declared_once_in_layout_and_finite() {
        let d = dims();
        let drive = good_drive();
        let v = |key: &str, data: DiagnosticData| DiagnosticValue {
            key: key.into(),
            data,
        };
        for (diags, why) in [
            (
                vec![v("other", DiagnosticData::Scalar(1.0))],
                "not declared",
            ),
            (
                vec![
                    v("cond", DiagnosticData::PerLine(vec![1.0; 4])),
                    v("cond", DiagnosticData::PerLine(vec![1.0; 4])),
                ],
                "twice",
            ),
            (
                vec![v("cond", DiagnosticData::Scalar(1.0))],
                "declared per-line",
            ),
            (
                vec![v("cond", DiagnosticData::PerLine(vec![1.0; 3]))],
                "needs 4",
            ),
            (
                vec![v(
                    "kept",
                    DiagnosticData::PerDriveLine(vec![f64::INFINITY; 8]),
                )],
                "not finite",
            ),
        ] {
            match validate_mimo_outputs(&drive, &diags, &d, &layouts()) {
                Err(LawError::BadMimoOutput {
                    rule: MimoOutputRule::Diagnostics,
                    detail,
                }) => assert!(detail.contains(why), "{detail}"),
                other => panic!("{why}: {other:?}"),
            }
        }
    }

    fn manifest() -> MimoManifest {
        MimoManifest {
            law: Manifest {
                name: "m".into(),
                version: "1".into(),
                author: String::new(),
                summary: String::new(),
                params: Vec::new(),
                diagnostics: vec![DiagnosticSpec {
                    key: "cond".into(),
                    label: "Condition".into(),
                    unit: String::new(),
                    help: String::new(),
                }],
            },
            shapes: vec![MimoShape::Square],
            needs: vec![MimoInput::SysidResponse],
            diagnostic_layouts: vec![DiagnosticLayout {
                key: "cond".into(),
                kind: DiagnosticKind::PerLine,
            }],
        }
    }

    #[test]
    fn manifest_rules_cover_shapes_needs_and_layouts() {
        assert!(validate_mimo_manifest(&manifest()).is_ok());
        let mut m = manifest();
        m.shapes.clear();
        assert!(validate_mimo_manifest(&m).is_err());
        let mut m = manifest();
        m.shapes.push(MimoShape::Square);
        assert!(validate_mimo_manifest(&m).is_err());
        let mut m = manifest();
        m.needs.push(MimoInput::SysidResponse);
        assert!(validate_mimo_manifest(&m).is_err());
        let mut m = manifest();
        m.diagnostic_layouts.clear();
        let e = validate_mimo_manifest(&m).unwrap_err().to_string();
        assert!(e.contains("has no layout"), "{e}");
        let mut m = manifest();
        m.diagnostic_layouts.push(DiagnosticLayout {
            key: "ghost".into(),
            kind: DiagnosticKind::Scalar,
        });
        assert!(validate_mimo_manifest(&m).is_err());
    }

    #[test]
    fn a_setup_of_an_unsupported_shape_is_refused() {
        let lines = 4;
        let setup = |nc: usize, nd: usize| MimoSetup {
            controls: nc,
            drives: nd,
            lines,
            df_hz: 1.0,
            band: (1, 3),
            reference: HermitianStack::zeros(lines, nc),
            tolerances: Tolerances::none(lines, nc),
        };
        assert!(validate_mimo_setup(&setup(2, 2), &manifest()).is_ok());
        match validate_mimo_setup(&setup(3, 2), &manifest()) {
            Err(LawError::SetupInvalid(why)) => {
                assert!(why.contains("more controls than drives"), "{why}")
            }
            other => panic!("{other:?}"),
        }
        let mut bad = setup(2, 2);
        bad.tolerances.alarm_upper[0] = -1.0;
        assert!(validate_mimo_setup(&bad, &manifest()).is_err());
        let mut bad = setup(2, 2);
        bad.reference.set(1, 0, 0, c(f64::INFINITY, 0.0));
        assert!(validate_mimo_setup(&bad, &manifest()).is_err());
    }

    #[test]
    fn inputs_are_checked_against_the_setup_and_the_declared_needs() {
        let d = MimoDims {
            lines: 4,
            controls: 2,
            drives: 2,
            band: (1, 3),
            df_hz: 1.0,
        };
        let reference = HermitianStack::zeros(4, 2);
        let tol = Tolerances::none(4, 2);
        let frf = MatrixStack::zeros(4, 2, 2);
        let coh = vec![0.9; 8];
        let mut i = MimoLoopInputs {
            reference: &reference,
            tolerances: &tol,
            response: &reference,
            response_dof: 32.0,
            frf: &frf,
            multiple_coherence: &coh,
            df_hz: 1.0,
            band: (1, 3),
            loop_index: 0,
            last_drive: None,
            response_drive_csd: None,
            sysid_response: None,
            sysid_drive: None,
            sysid_noise: None,
        };
        assert!(validate_mimo_inputs(&i, &d, &[]).is_ok());
        let e = validate_mimo_inputs(&i, &d, &[MimoInput::SysidResponse]).unwrap_err();
        assert!(matches!(e, LawError::InputsInvalid(_)), "{e}");
        i.sysid_response = Some(&reference);
        assert!(validate_mimo_inputs(&i, &d, &[MimoInput::SysidResponse]).is_ok());
        let bad_coh = vec![f64::NAN; 8];
        i.multiple_coherence = &bad_coh;
        assert!(validate_mimo_inputs(&i, &d, &[]).is_err());
        i.multiple_coherence = &coh;
        i.band = (0, 3);
        assert!(validate_mimo_inputs(&i, &d, &[]).is_err());
    }

    #[test]
    fn only_declared_optional_inputs_are_carried() {
        let h = HermitianStack::zeros(2, 2);
        let m = MatrixStack::zeros(2, 2, 2);
        let tol = Tolerances::none(2, 2);
        let coh = vec![1.0; 4];
        let i = MimoLoopInputs {
            reference: &h,
            tolerances: &tol,
            response: &h,
            response_dof: 1.0,
            frf: &m,
            multiple_coherence: &coh,
            df_hz: 1.0,
            band: (0, 2),
            loop_index: 3,
            last_drive: Some(&h),
            response_drive_csd: Some(&m),
            sysid_response: Some(&h),
            sysid_drive: Some(&h),
            sysid_noise: Some(&h),
        };
        let w = inputs_wit(&i, &[MimoInput::SysidDrive]);
        assert!(w.sysid_drive.is_some());
        assert!(w.last_drive.is_none());
        assert!(w.response_drive_csd.is_none());
        assert!(w.sysid_response.is_none());
        assert!(w.sysid_noise.is_none());
        assert_eq!(w.loop_index, 3);
    }
}
