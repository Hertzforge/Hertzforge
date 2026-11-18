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
//! the law accepts an amplitude-only reference. It builds the law for a run
//! ([`LoadedSineMimoLaw::instantiate`]) and calls it
//! ([`SineMimoLawInstance::engage`], [`SineMimoLawInstance::tick`]), each call
//! under a wall-clock budget and the memory cap. It checks what it hands a
//! law ([`validate_sine_mimo_setup`], [`validate_sine_mimo_inputs`]) and what
//! the law returns ([`validate_sine_mimo_outputs`]), each refusal named by a
//! [`SineMimoOutputRule`]. Every protection of the drive (the change, rise and
//! rotation caps, the drive ceilings, the backstop notch, the aborts) belongs
//! to the controller; a drive over its ceiling is passed through.
//!
//! The types are the world's own records, as the bindings generate them.

use crate::convert;
use crate::error::{LawError, SineMimoOutputRule};
use crate::host::LawHost;
use crate::sandbox::{HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{GuestOutput, LawIdentity, Manifest, Param};
use crate::validate;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::ResourceAny;
use wasmtime::Engine;

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "sine-mimo",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

pub(crate) use bindings::{SineMimo, SineMimoPre};

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

fn setup_err(msg: impl Into<String>) -> LawError {
    LawError::SetupInvalid(msg.into())
}

fn finite_c(c: &Complex) -> bool {
    c.re.is_finite() && c.im.is_finite()
}

fn want_len(name: &str, got: usize, want: usize) -> Result<(), LawError> {
    if got == want {
        Ok(())
    } else {
        Err(setup_err(format!(
            "{name} has {got} values; {want} are needed"
        )))
    }
}

fn stack_ok(
    name: &str,
    m: &MatrixStack,
    lines: usize,
    rows: usize,
    cols: usize,
) -> Result<(), LawError> {
    if (m.lines as usize, m.rows as usize, m.cols as usize) != (lines, rows, cols) {
        return Err(setup_err(format!(
            "{name} is {} lines of {} x {}; {lines} lines of {rows} x {cols} are needed",
            m.lines, m.rows, m.cols
        )));
    }
    want_len(name, m.data.len(), lines * rows * cols)?;
    if !m.data.iter().all(finite_c) {
        return Err(setup_err(format!(
            "{name} holds a value that is not finite"
        )));
    }
    Ok(())
}

/// The shape of `controls` by `drives`.
pub fn shape_of(controls: u32, drives: u32) -> SineShape {
    match controls.cmp(&drives) {
        std::cmp::Ordering::Equal => SineShape::Square,
        std::cmp::Ordering::Greater => SineShape::MoreControlsThanDrives,
        std::cmp::Ordering::Less => SineShape::FewerControlsThanDrives,
    }
}

/// Check a setup: the counts inside the limits and the shape theirs, a usable
/// rate, tick and band, one positive ceiling per drive, an ascending grid of
/// at least one line, an identification of `controls x drives` (and
/// `limits x drives`, when there are limit channels) on every line with one
/// coherence per channel and line between 0 and 1 and one trusted flag per
/// line, a reference of `controls x lines`, `limits x lines` limit bounds
/// above 0 where present, four tolerance lists of `controls x lines` at or
/// above 0 dB, and a usable tracking rule. Every number must be finite but
/// the identification's level, which is not finite when it was not measured.
pub fn validate_sine_mimo_setup(s: &SineMimoSetup) -> Result<(), LawError> {
    let (nc, nd, nl) = (s.controls as usize, s.drives as usize, s.limits as usize);
    if nc == 0 || nc > MAX_SINE_MIMO_CONTROLS {
        return Err(setup_err(format!(
            "{nc} controls; 1 to {MAX_SINE_MIMO_CONTROLS} are accepted"
        )));
    }
    if nd == 0 || nd > MAX_SINE_MIMO_DRIVES {
        return Err(setup_err(format!(
            "{nd} drives; 1 to {MAX_SINE_MIMO_DRIVES} are accepted"
        )));
    }
    if nl > MAX_SINE_MIMO_LIMITS {
        return Err(setup_err(format!(
            "{nl} limit channels; 0 to {MAX_SINE_MIMO_LIMITS} are accepted"
        )));
    }
    if s.shape != shape_of(s.controls, s.drives) {
        return Err(setup_err(format!(
            "the shape is {}; {nc} controls and {nd} drives are {}",
            shape_name(s.shape),
            shape_name(shape_of(s.controls, s.drives))
        )));
    }
    if !(s.fs_hz.is_finite() && s.fs_hz > 0.0 && s.tick_s.is_finite() && s.tick_s > 0.0) {
        return Err(setup_err("the sample rate or the tick is not usable"));
    }
    if !(s.band_lo_hz.is_finite()
        && s.band_hi_hz.is_finite()
        && 0.0 < s.band_lo_hz
        && s.band_lo_hz <= s.band_hi_hz
        && s.band_hi_hz < s.fs_hz / 2.0)
    {
        return Err(setup_err(format!(
            "the band {} to {} Hz is not inside 0 to {} Hz",
            s.band_lo_hz,
            s.band_hi_hz,
            s.fs_hz / 2.0
        )));
    }
    want_len("the drive ceilings", s.drive_ceiling_v.len(), nd)?;
    if !s.drive_ceiling_v.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(setup_err("a drive ceiling is not a positive number"));
    }
    let id = &s.identification;
    let lines = id.frequency_hz.len();
    if lines == 0 {
        return Err(setup_err("the identification has no line"));
    }
    if !id.frequency_hz.iter().all(|f| f.is_finite() && *f >= 0.0)
        || id.frequency_hz.windows(2).any(|w| w[1] <= w[0])
    {
        return Err(setup_err("the identification's grid is not ascending"));
    }
    stack_ok("the response", &id.response, lines, nc, nd)?;
    if nl > 0 {
        stack_ok("the limit response", &id.limit_response, lines, nl, nd)?;
    }
    want_len("the coherence", id.coherence.len(), nc * lines)?;
    want_len("the limit coherence", id.limit_coherence.len(), nl * lines)?;
    if !id
        .coherence
        .iter()
        .chain(&id.limit_coherence)
        .all(|c| (0.0..=1.0).contains(c))
    {
        return Err(setup_err("a coherence is not between 0 and 1"));
    }
    want_len("the trusted flags", id.trusted.len(), lines)?;
    want_len("the reference", s.reference.len(), nc * lines)?;
    if !s.reference.iter().all(finite_c) {
        return Err(setup_err("the reference holds a value that is not finite"));
    }
    want_len("the limit bounds", s.limit_bounds.len(), nl * lines)?;
    if !s
        .limit_bounds
        .iter()
        .flatten()
        .all(|b| b.is_finite() && *b > 0.0)
    {
        return Err(setup_err("a limit bound is not a number above 0"));
    }
    let t = &s.tolerance;
    for (name, v) in [
        ("the alarm band below", &t.alarm_low_db),
        ("the alarm band above", &t.alarm_high_db),
        ("the abort band below", &t.abort_low_db),
        ("the abort band above", &t.abort_high_db),
    ] {
        want_len(name, v.len(), nc * lines)?;
        if !v.iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err(setup_err(format!(
                "{name} holds a value that is not a positive number of dB"
            )));
        }
    }
    match s.tracking {
        Tracking::Proportional(p)
            if p.share.is_finite()
                && p.share > 0.0
                && p.share <= 1.0
                && p.max_hz.is_finite()
                && p.max_hz > 0.0 => {}
        Tracking::Fixed(hz) if hz.is_finite() && hz > 0.0 => {}
        _ => return Err(setup_err("the tracking rule is not usable")),
    }
    if !(s.tracker_delay_factor.is_finite() && s.tracker_delay_factor > 0.0) {
        return Err(setup_err(
            "the tracker's delay factor is not a number above 0",
        ));
    }
    if !(s.transport_delay_s.is_finite() && s.transport_delay_s >= 0.0) {
        return Err(setup_err("the transport delay is not a time of 0 or more"));
    }
    Ok(())
}

/// Check one update's inputs against the setup: a finite time step at or
/// above 0, a finite frequency above 0, finite sweep rates, a level at or
/// above 0, one reference and one tracked amplitude per control, one tracked
/// amplitude and one bound per limit channel (a bound above 0 where
/// present), one drive in force per drive, a notch scale from 0 to 1, every
/// number finite.
pub fn validate_sine_mimo_inputs(i: &SineMimoInputs, s: &SineMimoSetup) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    let (nc, nd, nl) = (s.controls as usize, s.drives as usize, s.limits as usize);
    if !(i.dt_s.is_finite() && i.dt_s >= 0.0) {
        return bad(format!("the time step {} s is not usable", i.dt_s));
    }
    if !(i.frequency_hz.is_finite() && i.frequency_hz > 0.0) {
        return bad(format!("the frequency {} Hz is not usable", i.frequency_hz));
    }
    if !(i.sweep_hz_per_s.is_finite() && i.sweep_oct_per_min.is_finite()) {
        return bad("a sweep rate is not finite".into());
    }
    if !(i.level.is_finite() && i.level >= 0.0) {
        return bad(format!("the level {} is not usable", i.level));
    }
    if !(i.notch_scale.is_finite() && (0.0..=1.0).contains(&i.notch_scale)) {
        return bad(format!("the notch scale {} is not 0 to 1", i.notch_scale));
    }
    for (name, got, want) in [
        ("references", i.reference.len(), nc),
        ("tracked controls", i.controls.len(), nc),
        ("tracked limit channels", i.limits.len(), nl),
        ("limit bounds", i.limit_bounds.len(), nl),
        ("drives in force", i.drive_in_force.len(), nd),
    ] {
        if got != want {
            return bad(format!("{got} {name}; {want} are needed"));
        }
    }
    if !i
        .reference
        .iter()
        .chain(&i.controls)
        .chain(&i.limits)
        .chain(&i.drive_in_force)
        .all(finite_c)
    {
        return bad("an amplitude is not finite".into());
    }
    if !i
        .limit_bounds
        .iter()
        .flatten()
        .all(|b| b.is_finite() && *b > 0.0)
    {
        return bad("a limit bound is not a number above 0".into());
    }
    Ok(())
}

/// Check a law's outputs: one finite drive per drive, a slow request's factor
/// finite, above 0 and at most 1, a notched reference (when given) of one
/// finite value per control, at most [`MAX_SINE_MIMO_VALUES`] values, each
/// declared once and finite. The first rule broken is named. A drive over its
/// ceiling is not refused: the controller scales the whole vector.
pub fn validate_sine_mimo_outputs(
    o: &SineMimoOutputs,
    s: &SineMimoSetup,
    manifest: &Manifest,
) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadSineMimoOutput { rule, detail });
    let (nc, nd) = (s.controls as usize, s.drives as usize);
    if o.drive.len() != nd {
        return bad(
            SineMimoOutputRule::WrongDriveCount,
            format!("{} drives returned; the run has {nd}", o.drive.len()),
        );
    }
    if let Some(k) = o.drive.iter().position(|c| !finite_c(c)) {
        return bad(
            SineMimoOutputRule::NonFinite,
            format!("drive {k} is not a finite number"),
        );
    }
    if let Some(Request::Slow(f)) = o.request {
        if !f.is_finite() {
            return bad(
                SineMimoOutputRule::NonFinite,
                "the slow request's factor is not a finite number".into(),
            );
        }
        if !(f > 0.0 && f <= 1.0) {
            return bad(
                SineMimoOutputRule::BadRequest,
                format!("a slow request's factor of {f}; above 0 and at most 1 is needed"),
            );
        }
    }
    if let Some(r) = &o.notched_reference {
        if r.len() != nc {
            return bad(
                SineMimoOutputRule::WrongReferenceCount,
                format!(
                    "a notched reference of {} values for {nc} controls",
                    r.len()
                ),
            );
        }
        if let Some(k) = r.iter().position(|c| !finite_c(c)) {
            return bad(
                SineMimoOutputRule::NonFinite,
                format!("the notched reference of control {k} is not a finite number"),
            );
        }
    }
    if o.values.len() > MAX_SINE_MIMO_VALUES {
        return bad(
            SineMimoOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_SINE_MIMO_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                SineMimoOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                SineMimoOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                SineMimoOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

/// A compiled MIMO Sine law with a checked manifest. Cheap to instantiate as
/// often as needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedSineMimoLaw {
    identity: LawIdentity,
    manifest: SineMimoManifest,
    pre: SineMimoPre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
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

    /// Whether the law accepts a run of this setup's shape, test type and
    /// reference; the refusal names what it accepts.
    pub fn accepts(&self, setup: &SineMimoSetup) -> Result<(), LawError> {
        let m = &self.manifest;
        if !m.accepts_shape(setup.shape) {
            return Err(setup_err(format!(
                "the law accepts {}, not {}",
                m.shapes
                    .iter()
                    .map(|s| shape_name(*s))
                    .collect::<Vec<_>>()
                    .join(" and "),
                shape_name(setup.shape)
            )));
        }
        if !m.accepts_test_type(setup.test_type) {
            return Err(setup_err(format!(
                "the law accepts {}, not {}",
                m.test_types
                    .iter()
                    .map(|t| test_type_name(*t))
                    .collect::<Vec<_>>()
                    .join(" and "),
                test_type_name(setup.test_type)
            )));
        }
        if !setup.phase_specified && !m.amplitude_only {
            return Err(setup_err(
                "the law needs the references' phases; this run gives amplitudes only",
            ));
        }
        Ok(())
    }

    /// Build the law for a run: check the setup, that the law accepts its
    /// shape, test type and reference, and the parameter values; instantiate
    /// the law in a fresh sandbox and call its constructor, all within
    /// `budget`. A law may fit a model here, so the controller gives this call
    /// a longer budget than an update.
    pub fn instantiate(
        &self,
        setup: &SineMimoSetup,
        params: &[Param],
        budget: Duration,
    ) -> Result<SineMimoLawInstance, LawError> {
        validate_sine_mimo_setup(setup)?;
        self.accepts(setup)?;
        let params = self.resolve_params(params, setup.controls as usize)?;
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
            .hertzforge_control_law_sine_mimo_law()
            .law()
            .call_constructor(&mut sb.store, setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(SineMimoLawInstance {
            identity: self.identity.clone(),
            manifest: self.manifest.clone(),
            params,
            setup: setup.clone(),
            sandbox: sb,
            bindings,
            law,
            ticker,
            poisoned: None,
        })
    }
}

/// One MIMO Sine law built for one run, with its own sandbox. Every call takes
/// a wall-clock budget; a call that traps, runs past its budget or exceeds the
/// memory cap leaves the instance unusable ([`LawError::Poisoned`] from then
/// on). A refused answer or the law's own error does not.
pub struct SineMimoLawInstance {
    identity: LawIdentity,
    manifest: SineMimoManifest,
    params: Vec<Param>,
    setup: SineMimoSetup,
    sandbox: Sandbox,
    bindings: SineMimo,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

#[derive(Clone, Copy)]
enum Call {
    Engage,
    Tick,
}

impl SineMimoLawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's manifest.
    pub fn manifest(&self) -> &SineMimoManifest {
        &self.manifest
    }

    /// The complete parameter values the law was built with, in manifest
    /// order (for the run record).
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// The setup the law was built for.
    pub fn setup(&self) -> &SineMimoSetup {
        &self.setup
    }

    /// Whether an earlier failure left this instance unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// The kept tail of what the law wrote to stdout and stderr.
    pub fn guest_output(&self) -> GuestOutput {
        self.sandbox.output()
    }

    /// The first update after identification: the first drive.
    pub fn engage(
        &mut self,
        inputs: &SineMimoInputs,
        budget: Duration,
    ) -> Result<SineMimoOutputs, LawError> {
        self.call(Call::Engage, inputs, budget)
    }

    /// One control update: the drive.
    pub fn tick(
        &mut self,
        inputs: &SineMimoInputs,
        budget: Duration,
    ) -> Result<SineMimoOutputs, LawError> {
        self.call(Call::Tick, inputs, budget)
    }

    fn call(
        &mut self,
        which: Call,
        inputs: &SineMimoInputs,
        budget: Duration,
    ) -> Result<SineMimoOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_sine_mimo_inputs(inputs, &self.setup)?;
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_sine_mimo_law().law();
        let r = match which {
            Call::Engage => guest.call_engage(&mut self.sandbox.store, self.law, inputs),
            Call::Tick => guest.call_tick(&mut self.sandbox.store, self.law, inputs),
        };
        let out = match r {
            Ok(out) => out.map_err(LawError::Law)?,
            Err(e) => {
                let err = self.sandbox.classify(&e);
                self.poisoned = Some(err.to_string());
                return Err(err);
            }
        };
        validate_sine_mimo_outputs(&out, &self.setup, &self.manifest.law)?;
        Ok(out)
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

    /// Two controls, three drives, one limit channel, on three lines.
    pub(crate) fn setup() -> SineMimoSetup {
        let (nc, nd, nl, lines) = (2u32, 3u32, 1u32, 3u32);
        let one = Complex { re: 1.0, im: 0.0 };
        let stack = |rows: u32| MatrixStack {
            lines,
            rows,
            cols: nd,
            data: vec![one; (lines * rows * nd) as usize],
        };
        let per = |n: u32, v: f64| vec![v; (n * lines) as usize];
        SineMimoSetup {
            controls: nc,
            drives: nd,
            limits: nl,
            fs_hz: 10240.0,
            tick_s: 0.01,
            band_lo_hz: 20.0,
            band_hi_hz: 200.0,
            test_type: TestType::Sweep,
            shape: SineShape::FewerControlsThanDrives,
            drive_ceiling_v: vec![1.0, 2.0, 3.0],
            identification: Identification {
                frequency_hz: vec![20.0, 100.0, 200.0],
                response: stack(nc),
                limit_response: stack(nl),
                coherence: per(nc, 0.9),
                limit_coherence: per(nl, 0.9),
                trusted: vec![true; lines as usize],
            },
            identification_level_db: f64::NAN,
            reference: vec![one; (nc * lines) as usize],
            phase_specified: true,
            limit_bounds: vec![Some(5.0), None, Some(4.0)],
            tolerance: Tolerance {
                alarm_low_db: per(nc, 3.0),
                alarm_high_db: per(nc, 3.0),
                abort_low_db: per(nc, 6.0),
                abort_high_db: per(nc, 6.0),
            },
            tracking: Tracking::Proportional(ProportionalTracking {
                share: 0.1,
                max_hz: 20.0,
            }),
            tracker_delay_factor: 1.0 / std::f64::consts::PI,
            transport_delay_s: 0.0,
        }
    }

    pub(crate) fn inputs(s: &SineMimoSetup) -> SineMimoInputs {
        let z = Complex { re: 0.1, im: 0.0 };
        SineMimoInputs {
            index: 1,
            dt_s: 0.01,
            frequency_hz: 100.0,
            sweep_hz_per_s: 1.0,
            sweep_oct_per_min: 0.8,
            level: 0.5,
            reference: vec![z; s.controls as usize],
            controls: vec![z; s.controls as usize],
            limits: vec![z; s.limits as usize],
            limit_bounds: vec![Some(5.0); s.limits as usize],
            drive_in_force: vec![z; s.drives as usize],
            hold: Hold::Free,
            notch_scale: 1.0,
            settling: false,
        }
    }

    fn law_manifest() -> Manifest {
        Manifest {
            diagnostics: vec![crate::types::DiagnosticSpec {
                key: "gain".into(),
                label: "g".into(),
                unit: String::new(),
                help: String::new(),
            }],
            ..manifest().law
        }
    }

    fn rule(r: Result<(), LawError>) -> Option<SineMimoOutputRule> {
        match r {
            Err(LawError::BadSineMimoOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    type Break<T> = (&'static str, fn(&mut T));

    #[test]
    fn mimo_sine_each_output_rule_is_named() {
        use crate::bindings::hertzforge::control_law::types::DiagnosticValue;
        let (s, m) = (setup(), law_manifest());
        let c = Complex { re: 0.5, im: -0.5 };
        let good = SineMimoOutputs {
            drive: vec![c; 3],
            faulted: false,
            values: vec![DiagnosticValue {
                key: "gain".into(),
                value: 0.3,
            }],
            request: Some(Request::Slow(0.5)),
            notched_reference: Some(vec![c; 2]),
        };
        let check = |o: &SineMimoOutputs| rule(validate_sine_mimo_outputs(o, &s, &m));
        assert_eq!(check(&good), None);
        let cases: [(Break<SineMimoOutputs>, SineMimoOutputRule); 12] = [
            (
                ("short", |o| {
                    o.drive.pop();
                }),
                SineMimoOutputRule::WrongDriveCount,
            ),
            (
                ("long", |o| o.drive.push(Complex { re: 0.0, im: 0.0 })),
                SineMimoOutputRule::WrongDriveCount,
            ),
            (
                ("nan drive", |o| o.drive[1].im = f64::NAN),
                SineMimoOutputRule::NonFinite,
            ),
            (
                ("infinite drive", |o| o.drive[2].re = f64::INFINITY),
                SineMimoOutputRule::NonFinite,
            ),
            (
                ("slow 0", |o| o.request = Some(Request::Slow(0.0))),
                SineMimoOutputRule::BadRequest,
            ),
            (
                ("slow 2", |o| o.request = Some(Request::Slow(2.0))),
                SineMimoOutputRule::BadRequest,
            ),
            (
                ("slow nan", |o| o.request = Some(Request::Slow(f64::NAN))),
                SineMimoOutputRule::NonFinite,
            ),
            (
                ("notch short", |o| o.notched_reference = Some(Vec::new())),
                SineMimoOutputRule::WrongReferenceCount,
            ),
            (
                ("notch nan", |o| {
                    o.notched_reference = Some(vec![
                        Complex {
                            re: f64::NAN,
                            im: 0.0
                        };
                        2
                    ])
                }),
                SineMimoOutputRule::NonFinite,
            ),
            (
                ("too many", |o| o.values = vec![o.values[0].clone(); 9]),
                SineMimoOutputRule::TooManyValues,
            ),
            (
                ("undeclared", |o| o.values[0].key = "other".into()),
                SineMimoOutputRule::UndeclaredValue,
            ),
            (
                ("twice", |o| o.values.push(o.values[0].clone())),
                SineMimoOutputRule::UndeclaredValue,
            ),
        ];
        for ((what, f), want) in cases {
            let mut o = good.clone();
            f(&mut o);
            assert_eq!(check(&o), Some(want), "{what}");
        }
        let mut o = good.clone();
        o.values[0].value = f64::NAN;
        assert_eq!(check(&o), Some(SineMimoOutputRule::NonFinite));
        // A drive over its ceiling, a hold request and no notch are all answers.
        let mut o = good;
        o.drive[0] = Complex { re: 50.0, im: 0.0 };
        o.request = Some(Request::Hold);
        o.notched_reference = None;
        assert_eq!(check(&o), None);
        assert_eq!(
            [
                SineMimoOutputRule::WrongDriveCount,
                SineMimoOutputRule::NonFinite,
                SineMimoOutputRule::TooManyValues,
                SineMimoOutputRule::UndeclaredValue,
                SineMimoOutputRule::BadRequest,
                SineMimoOutputRule::WrongReferenceCount,
            ]
            .map(SineMimoOutputRule::as_str),
            [
                "wrong_drive_count",
                "non_finite",
                "too_many_values",
                "undeclared_value",
                "bad_request",
                "wrong_reference_count"
            ]
        );
    }

    #[test]
    fn mimo_sine_the_setup_is_checked() {
        validate_sine_mimo_setup(&setup()).unwrap();
        let breaks: [Break<SineMimoSetup>; 21] = [
            ("no controls", |s| s.controls = 0),
            ("17 controls", |s| s.controls = 17),
            ("9 drives", |s| s.drives = 9),
            ("9 limits", |s| s.limits = 9),
            ("shape", |s| s.shape = SineShape::Square),
            ("tick", |s| s.tick_s = 0.0),
            ("band over Nyquist", |s| s.band_hi_hz = 6000.0),
            ("ceiling", |s| s.drive_ceiling_v[1] = 0.0),
            ("grid", |s| s.identification.frequency_hz.swap(0, 1)),
            ("response", |s| s.identification.response.lines = 2),
            ("response nan", |s| {
                s.identification.response.data[4].re = f64::NAN
            }),
            ("limit response", |s| {
                s.identification.limit_response.rows = 2
            }),
            ("coherence", |s| s.identification.coherence[0] = 1.5),
            ("trusted", |s| {
                s.identification.trusted.pop();
            }),
            ("reference", |s| {
                s.reference.pop();
            }),
            ("bound", |s| s.limit_bounds[0] = Some(0.0)),
            ("tolerance", |s| s.tolerance.abort_high_db[0] = -1.0),
            ("tracking", |s| {
                s.tracking = Tracking::Proportional(ProportionalTracking {
                    share: 10.0,
                    max_hz: 20.0,
                })
            }),
            ("tracker delay factor", |s| s.tracker_delay_factor = 0.0),
            ("transport delay", |s| s.transport_delay_s = -0.01),
            ("transport delay", |s| s.transport_delay_s = f64::NAN),
        ];
        for (what, f) in breaks {
            let mut s = setup();
            f(&mut s);
            assert!(
                matches!(validate_sine_mimo_setup(&s), Err(LawError::SetupInvalid(_))),
                "{what}"
            );
        }
    }

    #[test]
    fn mimo_sine_the_inputs_are_checked_against_the_setup() {
        let s = setup();
        validate_sine_mimo_inputs(&inputs(&s), &s).unwrap();
        let breaks: [Break<SineMimoInputs>; 9] = [
            ("dt", |i| i.dt_s = -1.0),
            ("frequency", |i| i.frequency_hz = 0.0),
            ("sweep", |i| i.sweep_oct_per_min = f64::NAN),
            ("level", |i| i.level = -0.5),
            ("notch", |i| i.notch_scale = 1.5),
            ("references", |i| {
                i.reference.pop();
            }),
            ("drives", |i| {
                i.drive_in_force.pop();
            }),
            ("limit bounds", |i| i.limit_bounds = vec![Some(-1.0)]),
            ("control nan", |i| i.controls[0].im = f64::NAN),
        ];
        for (what, f) in breaks {
            let mut i = inputs(&s);
            f(&mut i);
            assert!(
                matches!(
                    validate_sine_mimo_inputs(&i, &s),
                    Err(LawError::InputsInvalid(_))
                ),
                "{what}"
            );
        }
    }
}
