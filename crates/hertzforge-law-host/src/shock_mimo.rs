//! MIMO Shock law files: the `shock-mimo` world (`wit/control-law/shock-mimo.wit`).
//!
//! A MIMO Shock law is called between pulses: once after system identification
//! (`seed`, for the ladder's first level) and once after each fired, measured
//! and judged pulse (`iterate`, with the next level already scheduled). It
//! returns every drive's whole record for the next pulse, the delays it expects
//! (optional) and up to [`MAX_SHOCK_MIMO_VALUES`] declared values.
//!
//! The host checks what it hands a law ([`validate_shock_mimo_setup`],
//! [`validate_shock_mimo_inputs`]) and what the law returns
//! ([`validate_shock_mimo_outputs`]): the drive count, each record's length,
//! finite values, the delays' shape and the declared values, each refusal
//! named by a [`ShockMimoOutputRule`]. Every protection of the drive (each
//! drive's ceiling and abort, the growth cap, the predicted response, the
//! divergence watch) belongs to the controller; [`drive_over_ceiling`] names a
//! drive over its ceiling for it, and the host passes such a drive through.
//!
//! The types are the world's own records, as the bindings generate them; the
//! controller's adapter copies them field for field.

use crate::convert;
use crate::error::{LawError, ShockMimoOutputRule};
use crate::host::LawHost;
use crate::sandbox::{HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{GuestOutput, LawIdentity, Manifest, Param};
use crate::validate;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::ResourceAny;
use wasmtime::Engine;

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "shock-mimo",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

pub(crate) use bindings::{ShockMimo, ShockMimoPre};

pub use bindings::hertzforge::control_law::mimo_shock_types::{
    Complex, ComponentShape, ControlVerdict, HoldTiming, MatrixStack, MimoShockInputs,
    MimoShockOutputs, MimoShockSetup, Plant, SrsComponent, SrsTarget, Target, TargetKind, Timing,
    WaveformTarget,
};

/// The most values a MIMO Shock law may report on one call.
pub const MAX_SHOCK_MIMO_VALUES: usize = 8;
/// The most control channels a MIMO Shock law is built for.
pub const MAX_SHOCK_MIMO_CONTROLS: usize = 16;
/// The most drives a MIMO Shock law commands.
pub const MAX_SHOCK_MIMO_DRIVES: usize = 8;
/// The shortest record, samples.
pub const MIN_SHOCK_MIMO_RECORD_LEN: usize = 64;
/// The longest record, samples.
pub const MAX_SHOCK_MIMO_RECORD_LEN: usize = 16384;

/// What a MIMO Shock law file says about itself: the manifest every law has,
/// and the target kinds it accepts (at least one, each once).
#[derive(Debug, Clone, PartialEq)]
pub struct ShockMimoManifest {
    /// Name, version, author, summary, parameters and values.
    pub law: Manifest,
    /// The target kinds the law accepts.
    pub targets: Vec<TargetKind>,
}

impl ShockMimoManifest {
    /// Whether the law accepts a target of `kind`.
    pub fn accepts(&self, kind: TargetKind) -> bool {
        self.targets.contains(&kind)
    }
}

/// The stable name of a target kind: `waveform` or `srs`.
pub fn target_kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Waveform => "waveform",
        TargetKind::Srs => "srs",
    }
}

/// The kind of a setup's target.
pub fn target_kind(t: &Target) -> TargetKind {
    match t {
        Target::Waveform(_) => TargetKind::Waveform,
        Target::Srs(_) => TargetKind::Srs,
    }
}

fn setup_err(msg: impl Into<String>) -> LawError {
    LawError::SetupInvalid(msg.into())
}

fn finite(name: &str, v: &[f64]) -> Result<(), String> {
    match v.iter().position(|x| !x.is_finite()) {
        Some(i) => Err(format!("{name}[{i}] is not a finite number")),
        None => Ok(()),
    }
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

/// Check a setup: the sizes inside the limits, a power-of-two record, a band
/// inside the record's Nyquist frequency, a plant of `record_len / 2 + 1` lines
/// of `controls x drives` with one coherence per control and line and one
/// trusted flag per line, a target of the right lengths, one delay per control
/// in hold timing, one positive ceiling per drive and an ascending ladder.
/// Every number must be finite.
pub fn validate_shock_mimo_setup(s: &MimoShockSetup) -> Result<(), LawError> {
    let (nc, nd, len) = (
        s.controls as usize,
        s.drives as usize,
        s.record_len as usize,
    );
    if nc == 0 || nc > MAX_SHOCK_MIMO_CONTROLS {
        return Err(setup_err(format!(
            "{nc} controls; 1 to {MAX_SHOCK_MIMO_CONTROLS} are accepted"
        )));
    }
    if nd == 0 || nd > MAX_SHOCK_MIMO_DRIVES {
        return Err(setup_err(format!(
            "{nd} drives; 1 to {MAX_SHOCK_MIMO_DRIVES} are accepted"
        )));
    }
    if !len.is_power_of_two()
        || !(MIN_SHOCK_MIMO_RECORD_LEN..=MAX_SHOCK_MIMO_RECORD_LEN).contains(&len)
    {
        return Err(setup_err(format!(
            "a record of {len} samples; a power of two from {MIN_SHOCK_MIMO_RECORD_LEN} to \
             {MAX_SHOCK_MIMO_RECORD_LEN} is needed"
        )));
    }
    if !(s.fs_hz.is_finite() && s.fs_hz > 0.0) {
        return Err(setup_err(format!(
            "the sample rate {} is not usable",
            s.fs_hz
        )));
    }
    if !(s.band_lo_hz.is_finite()
        && s.band_hi_hz.is_finite()
        && 0.0 <= s.band_lo_hz
        && s.band_lo_hz < s.band_hi_hz
        && s.band_hi_hz <= s.fs_hz / 2.0)
    {
        return Err(setup_err(format!(
            "the band {} to {} Hz is not inside 0 to {} Hz",
            s.band_lo_hz,
            s.band_hi_hz,
            s.fs_hz / 2.0
        )));
    }
    let lines = len / 2 + 1;
    let f = &s.plant.frf;
    if (f.lines as usize, f.rows as usize, f.cols as usize) != (lines, nc, nd) {
        return Err(setup_err(format!(
            "the plant is {} lines of {} x {}; {lines} lines of {nc} x {nd} are needed",
            f.lines, f.rows, f.cols
        )));
    }
    want_len("the plant", f.data.len(), lines * nc * nd)?;
    if f.data
        .iter()
        .any(|c| !(c.re.is_finite() && c.im.is_finite()))
    {
        return Err(setup_err("the plant holds a value that is not finite"));
    }
    want_len("the coherence", s.plant.coherence.len(), nc * lines)?;
    if s.plant.coherence.iter().any(|c| !(0.0..=1.0).contains(c)) {
        return Err(setup_err("a coherence is not between 0 and 1"));
    }
    want_len("the trusted flags", s.plant.trusted.len(), lines)?;
    match &s.target {
        Target::Waveform(w) => {
            want_len("the target", w.records.len(), nc * len)?;
            finite("target", &w.records).map_err(setup_err)?;
        }
        Target::Srs(t) => {
            let nf = t.natural_hz.len();
            if nf == 0 {
                return Err(setup_err("the SRS has no natural frequencies"));
            }
            finite("natural_hz", &t.natural_hz).map_err(setup_err)?;
            if t.natural_hz[0] <= 0.0 || t.natural_hz.windows(2).any(|w| w[1] <= w[0]) {
                return Err(setup_err(
                    "the SRS natural frequencies are not positive and ascending",
                ));
            }
            if !(t.q.is_finite() && t.q > 0.5) {
                return Err(setup_err(format!(
                    "the SRS quality factor {} is not usable",
                    t.q
                )));
            }
            want_len("the target SRS", t.points.len(), nc * nf)?;
            finite("points", &t.points).map_err(setup_err)?;
            if t.points.iter().any(|&p| p < 0.0) {
                return Err(setup_err("the target SRS is negative at a point"));
            }
            if !(t.tol_lo_db.is_finite() && t.tol_hi_db.is_finite())
                || t.tol_lo_db > 0.0
                || t.tol_hi_db < 0.0
            {
                return Err(setup_err(
                    "the SRS tolerance must be at or under 0 dB below and at or over 0 dB above",
                ));
            }
            want_len("the synthesized waveforms", t.waveforms.len(), nc * len)?;
            finite("waveforms", &t.waveforms).map_err(setup_err)?;
            if t.components.len() != nc {
                return Err(setup_err(format!(
                    "{} synthesis tables for {nc} controls",
                    t.components.len()
                )));
            }
        }
    }
    match &s.timing {
        Timing::Free(w) if w.is_finite() && *w >= 0.0 => {}
        Timing::Free(w) => return Err(setup_err(format!("the timing window {w} s is not usable"))),
        Timing::Hold(h) => {
            want_len("the held delays", h.delays_s.len(), nc)?;
            finite("delays_s", &h.delays_s).map_err(setup_err)?;
            if !(h.window_s.is_finite() && h.window_s >= 0.0) {
                return Err(setup_err("the hold window is not usable"));
            }
        }
    }
    want_len("the drive ceilings", s.drive_ceiling_v.len(), nd)?;
    if s.drive_ceiling_v
        .iter()
        .any(|v| !(v.is_finite() && *v > 0.0))
    {
        return Err(setup_err("a drive ceiling is not a positive number"));
    }
    if s.ladder_db.is_empty() {
        return Err(setup_err("the level ladder is empty"));
    }
    finite("ladder_db", &s.ladder_db).map_err(setup_err)?;
    if s.ladder_db.windows(2).any(|w| w[1] < w[0]) {
        return Err(setup_err("the level ladder is not ascending"));
    }
    Ok(())
}

/// Check one pulse's inputs against the setup: a pulse index of 1 or more,
/// finite levels, one record per drive and per control, one verdict per
/// control, a measured SRS on the natural frequencies for an SRS target and
/// none for a waveform target, every number finite.
pub fn validate_shock_mimo_inputs(i: &MimoShockInputs, s: &MimoShockSetup) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    let (nc, nd, len) = (
        s.controls as usize,
        s.drives as usize,
        s.record_len as usize,
    );
    if i.pulse_index == 0 {
        return bad("an iteration after no pulse; the first call is the seed".into());
    }
    if !(i.level_db.is_finite() && i.next_level_db.is_finite()) {
        return bad("a level is not finite".into());
    }
    if i.drives.len() != nd * len || i.responses.len() != nc * len {
        return bad(format!(
            "{} drive and {} response samples; {} and {} are needed",
            i.drives.len(),
            i.responses.len(),
            nd * len,
            nc * len
        ));
    }
    finite("drives", &i.drives).map_err(LawError::InputsInvalid)?;
    finite("responses", &i.responses).map_err(LawError::InputsInvalid)?;
    match (&s.target, &i.srs) {
        (Target::Waveform(_), None) => {}
        (Target::Srs(t), Some(m)) if m.len() == nc * t.natural_hz.len() => {
            finite("srs", m).map_err(LawError::InputsInvalid)?;
        }
        _ => {
            return bad(
                "the measured SRS must be present for an SRS target only, one per control per \
                 natural frequency"
                    .into(),
            )
        }
    }
    if i.verdicts.len() != nc {
        return bad(format!("{} verdicts for {nc} controls", i.verdicts.len()));
    }
    if i.verdicts
        .iter()
        .any(|v| !(v.error_db.is_finite() && v.delay_s.is_finite() && v.peak_ratio.is_finite()))
    {
        return bad("a verdict holds a value that is not finite".into());
    }
    Ok(())
}

/// Check a law's outputs: one record of `record_len` per drive, every sample
/// finite, one finite delay per control when delays are given, at most
/// [`MAX_SHOCK_MIMO_VALUES`] values, each declared once and finite. The first
/// rule broken is named.
pub fn validate_shock_mimo_outputs(
    o: &MimoShockOutputs,
    s: &MimoShockSetup,
    manifest: &Manifest,
) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadShockMimoOutput { rule, detail });
    let (nc, nd, len) = (
        s.controls as usize,
        s.drives as usize,
        s.record_len as usize,
    );
    if o.drives.len() != nd * len {
        return if !o.drives.is_empty() && o.drives.len().is_multiple_of(len) {
            bad(
                ShockMimoOutputRule::WrongDriveCount,
                format!(
                    "{} drive records returned; the run has {nd} drives",
                    o.drives.len() / len
                ),
            )
        } else {
            bad(
                ShockMimoOutputRule::WrongLength,
                format!(
                    "{} drive samples returned; {nd} records of {len} are {} samples",
                    o.drives.len(),
                    nd * len
                ),
            )
        };
    }
    if let Some(k) = o.drives.iter().position(|x| !x.is_finite()) {
        return bad(
            ShockMimoOutputRule::NonFinite,
            format!(
                "drive {} sample {} is not a finite number",
                k / len,
                k % len
            ),
        );
    }
    if let Some(d) = &o.delays_s {
        if d.len() != nc || d.iter().any(|x| !x.is_finite()) {
            return bad(
                ShockMimoOutputRule::DelaysShape,
                format!(
                    "{} delays for {nc} controls; one finite delay per control",
                    d.len()
                ),
            );
        }
    }
    if o.values.len() > MAX_SHOCK_MIMO_VALUES {
        return bad(
            ShockMimoOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_SHOCK_MIMO_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                ShockMimoOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                ShockMimoOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                ShockMimoOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

/// A drive whose peak is over its ceiling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CeilingExcess {
    /// The drive, in the setup's drive order.
    pub drive: usize,
    /// Its peak, volts.
    pub peak_v: f64,
    /// Its ceiling, volts.
    pub ceiling_v: f64,
}

impl CeilingExcess {
    /// The stable name: `over_ceiling`.
    pub const RULE: &'static str = "over_ceiling";
}

impl fmt::Display for CeilingExcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "drive {} peaks at {:.4} V, over its {:.4} V ceiling ({})",
            self.drive,
            self.peak_v,
            self.ceiling_v,
            Self::RULE
        )
    }
}

/// The first drive of checked outputs whose peak is over its ceiling. The
/// host does not refuse such a drive: a law's drive is a request, and the
/// controller scales it down or aborts. This names it for the controller and
/// the record.
pub fn drive_over_ceiling(o: &MimoShockOutputs, s: &MimoShockSetup) -> Option<CeilingExcess> {
    let len = s.record_len as usize;
    if len == 0 {
        return None;
    }
    o.drives
        .chunks(len)
        .zip(&s.drive_ceiling_v)
        .enumerate()
        .find_map(|(drive, (rec, &ceiling_v))| {
            let peak_v = rec.iter().fold(0.0f64, |a, x| a.max(x.abs()));
            (peak_v > ceiling_v).then_some(CeilingExcess {
                drive,
                peak_v,
                ceiling_v,
            })
        })
}

/// Check a MIMO Shock manifest: the rules every manifest follows, at most
/// [`MAX_SHOCK_MIMO_VALUES`] values, and at least one target kind, each once.
pub fn validate_shock_mimo_manifest(m: &ShockMimoManifest) -> Result<(), LawError> {
    validate::validate_manifest(&m.law)?;
    if m.law.diagnostics.len() > MAX_SHOCK_MIMO_VALUES {
        return Err(LawError::ManifestInvalid(format!(
            "{} values declared; a MIMO Shock law reports at most {MAX_SHOCK_MIMO_VALUES}",
            m.law.diagnostics.len()
        )));
    }
    if m.targets.is_empty() {
        return Err(LawError::ManifestInvalid(
            "the law accepts no target kind; it must accept waveform, srs or both".into(),
        ));
    }
    let twice = m
        .targets
        .iter()
        .enumerate()
        .any(|(i, k)| m.targets[..i].contains(k));
    if twice {
        return Err(LawError::ManifestInvalid(
            "a target kind is listed twice".into(),
        ));
    }
    Ok(())
}

/// A compiled MIMO Shock law with a checked manifest. Cheap to instantiate as
/// often as needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedShockMimoLaw {
    identity: LawIdentity,
    manifest: ShockMimoManifest,
    pre: ShockMimoPre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedShockMimoLaw {
    /// Read and check the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: ShockMimoPre<HostState>,
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
        let manifest = ShockMimoManifest {
            law: convert::manifest(m.law),
            targets: m.targets,
        };
        validate_shock_mimo_manifest(&manifest)?;
        Ok(LoadedShockMimoLaw {
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
    pub fn manifest(&self) -> &ShockMimoManifest {
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

    /// Build the law for a run: check the setup, that the law accepts its
    /// target kind, and the parameter values; instantiate the law in a fresh
    /// sandbox and call its constructor, all within `budget`.
    pub fn instantiate(
        &self,
        setup: &MimoShockSetup,
        params: &[Param],
        budget: Duration,
    ) -> Result<ShockMimoLawInstance, LawError> {
        validate_shock_mimo_setup(setup)?;
        let kind = target_kind(&setup.target);
        if !self.manifest.accepts(kind) {
            return Err(setup_err(format!(
                "the law accepts {} targets, not {}",
                self.manifest
                    .targets
                    .iter()
                    .map(|k| target_kind_name(*k))
                    .collect::<Vec<_>>()
                    .join(" and "),
                target_kind_name(kind)
            )));
        }
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
            .hertzforge_control_law_shock_mimo_law()
            .law()
            .call_constructor(&mut sb.store, setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(ShockMimoLawInstance {
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

/// One MIMO Shock law built for one run, with its own sandbox. Every call
/// takes a wall-clock budget; a call that traps, runs past its budget or
/// exceeds the memory cap leaves the instance unusable ([`LawError::Poisoned`]
/// from then on).
pub struct ShockMimoLawInstance {
    identity: LawIdentity,
    manifest: ShockMimoManifest,
    params: Vec<Param>,
    setup: MimoShockSetup,
    sandbox: Sandbox,
    bindings: ShockMimo,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

enum Call<'a> {
    Seed(f64),
    Iterate(&'a MimoShockInputs),
}

impl ShockMimoLawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's manifest.
    pub fn manifest(&self) -> &ShockMimoManifest {
        &self.manifest
    }

    /// The complete parameter values the law was built with, in manifest
    /// order (for the run record).
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// The setup the law was built for.
    pub fn setup(&self) -> &MimoShockSetup {
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

    /// The drives of the first pulse, at `level_db`.
    pub fn seed(&mut self, level_db: f64, budget: Duration) -> Result<MimoShockOutputs, LawError> {
        if !level_db.is_finite() {
            return Err(LawError::InputsInvalid(format!(
                "the first level {level_db} dB is not finite"
            )));
        }
        self.call(Call::Seed(level_db), budget)
    }

    /// The drives of the next pulse, after a pulse was fired, measured and
    /// judged.
    pub fn iterate(
        &mut self,
        inputs: &MimoShockInputs,
        budget: Duration,
    ) -> Result<MimoShockOutputs, LawError> {
        validate_shock_mimo_inputs(inputs, &self.setup)?;
        self.call(Call::Iterate(inputs), budget)
    }

    fn call(&mut self, which: Call<'_>, budget: Duration) -> Result<MimoShockOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_shock_mimo_law().law();
        let r = match which {
            Call::Seed(level) => guest.call_seed(&mut self.sandbox.store, self.law, level),
            Call::Iterate(i) => guest.call_iterate(&mut self.sandbox.store, self.law, i),
        };
        let out = match r {
            Ok(out) => out.map_err(LawError::Law)?,
            Err(e) => {
                let err = self.sandbox.classify(&e);
                self.poisoned = Some(err.to_string());
                return Err(err);
            }
        };
        validate_shock_mimo_outputs(&out, &self.setup, &self.manifest.law)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::hertzforge::control_law::types::DiagnosticValue;
    use crate::types::DiagnosticSpec;

    fn manifest() -> Manifest {
        Manifest {
            name: "t".into(),
            version: "1".into(),
            author: String::new(),
            summary: String::new(),
            params: Vec::new(),
            diagnostics: vec![DiagnosticSpec {
                key: "weight".into(),
                label: "w".into(),
                unit: String::new(),
                help: String::new(),
            }],
        }
    }

    fn setup(kind: TargetKind) -> MimoShockSetup {
        let (nc, nd, len) = (2u32, 3u32, 64u32);
        let lines = len / 2 + 1;
        MimoShockSetup {
            fs_hz: 1024.0,
            record_len: len,
            drives: nd,
            controls: nc,
            band_lo_hz: 10.0,
            band_hi_hz: 400.0,
            target: match kind {
                TargetKind::Waveform => Target::Waveform(WaveformTarget {
                    records: vec![0.0; (nc * len) as usize],
                }),
                TargetKind::Srs => Target::Srs(SrsTarget {
                    natural_hz: vec![100.0, 200.0],
                    q: 10.0,
                    points: vec![1.0; 4],
                    tol_lo_db: -1.5,
                    tol_hi_db: 3.0,
                    waveforms: vec![0.0; (nc * len) as usize],
                    components: vec![Vec::new(), Vec::new()],
                }),
            },
            timing: Timing::Free(0.002),
            plant: Plant {
                frf: MatrixStack {
                    lines,
                    rows: nc,
                    cols: nd,
                    data: vec![Complex { re: 1.0, im: 0.0 }; (lines * nc * nd) as usize],
                },
                coherence: vec![1.0; (nc * lines) as usize],
                trusted: vec![true; lines as usize],
            },
            drive_ceiling_v: vec![1.0; 3],
            ladder_db: vec![-6.0, 0.0],
            identification_level_db: -26.0,
        }
    }

    fn rule(r: Result<(), LawError>) -> Option<ShockMimoOutputRule> {
        match r {
            Err(LawError::BadShockMimoOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn mimo_shock_each_output_rule_is_named() {
        let (s, m) = (setup(TargetKind::Waveform), manifest());
        let good = MimoShockOutputs {
            drives: vec![0.1; 3 * 64],
            delays_s: Some(vec![0.0, 0.001]),
            values: vec![DiagnosticValue {
                key: "weight".into(),
                value: 0.5,
            }],
        };
        let check = |o: &MimoShockOutputs| rule(validate_shock_mimo_outputs(o, &s, &m));
        assert_eq!(check(&good), None);
        let mut o = good.clone();
        o.drives.truncate(2 * 64);
        assert_eq!(check(&o), Some(ShockMimoOutputRule::WrongDriveCount));
        let mut o = good.clone();
        o.drives.extend(vec![0.0; 64]);
        assert_eq!(check(&o), Some(ShockMimoOutputRule::WrongDriveCount));
        let mut o = good.clone();
        o.drives.pop();
        assert_eq!(check(&o), Some(ShockMimoOutputRule::WrongLength));
        let mut o = good.clone();
        o.drives.clear();
        assert_eq!(check(&o), Some(ShockMimoOutputRule::WrongLength));
        let mut o = good.clone();
        o.drives[70] = f64::INFINITY;
        assert_eq!(check(&o), Some(ShockMimoOutputRule::NonFinite));
        let mut o = good.clone();
        o.delays_s = Some(vec![0.0]);
        assert_eq!(check(&o), Some(ShockMimoOutputRule::DelaysShape));
        let mut o = good.clone();
        o.delays_s = Some(vec![0.0, f64::NAN]);
        assert_eq!(check(&o), Some(ShockMimoOutputRule::DelaysShape));
        let mut o = good.clone();
        o.values[0].key = "other".into();
        assert_eq!(check(&o), Some(ShockMimoOutputRule::UndeclaredValue));
        let mut o = good.clone();
        o.values.push(o.values[0].clone());
        assert_eq!(check(&o), Some(ShockMimoOutputRule::UndeclaredValue));
        let mut o = good.clone();
        o.values = vec![o.values[0].clone(); 9];
        assert_eq!(check(&o), Some(ShockMimoOutputRule::TooManyValues));
        let mut o = good;
        o.values[0].value = f64::NAN;
        assert_eq!(check(&o), Some(ShockMimoOutputRule::NonFinite));
    }

    #[test]
    fn mimo_shock_a_drive_over_its_ceiling_is_named_not_refused() {
        let s = setup(TargetKind::Waveform);
        let mut o = MimoShockOutputs {
            drives: vec![0.5; 3 * 64],
            delays_s: None,
            values: Vec::new(),
        };
        assert_eq!(drive_over_ceiling(&o, &s), None);
        o.drives[2 * 64 + 5] = -1.25;
        let e = drive_over_ceiling(&o, &s).unwrap();
        assert_eq!((e.drive, e.peak_v, e.ceiling_v), (2, 1.25, 1.0));
        assert!(e.to_string().contains("over_ceiling"));
        assert_eq!(rule(validate_shock_mimo_outputs(&o, &s, &manifest())), None);
    }

    type Break<T> = (&'static str, fn(&mut T));

    #[test]
    fn mimo_shock_the_setup_is_checked() {
        validate_shock_mimo_setup(&setup(TargetKind::Waveform)).unwrap();
        validate_shock_mimo_setup(&setup(TargetKind::Srs)).unwrap();
        let breaks: [Break<MimoShockSetup>; 20] = [
            ("no controls", |s| s.controls = 0),
            ("17 controls", |s| s.controls = 17),
            ("9 drives", |s| s.drives = 9),
            ("record 100", |s| s.record_len = 100),
            ("record 32", |s| s.record_len = 32),
            ("rate", |s| s.fs_hz = f64::NAN),
            ("band over Nyquist", |s| s.band_hi_hz = 600.0),
            ("band reversed", |s| s.band_lo_hz = 500.0),
            ("plant lines", |s| s.plant.frf.lines -= 1),
            ("plant data", |s| {
                s.plant.frf.data.pop();
            }),
            ("plant nan", |s| s.plant.frf.data[3].im = f64::NAN),
            ("coherence", |s| {
                s.plant.coherence.pop();
            }),
            ("coherence range", |s| s.plant.coherence[0] = 1.5),
            ("trusted", |s| {
                s.plant.trusted.pop();
            }),
            ("target", |s| {
                if let Target::Waveform(w) = &mut s.target {
                    w.records.pop();
                }
            }),
            ("hold delays", |s| {
                s.timing = Timing::Hold(HoldTiming {
                    delays_s: vec![0.0],
                    window_s: 0.001,
                })
            }),
            ("ceiling", |s| s.drive_ceiling_v[1] = 0.0),
            ("ceilings", |s| {
                s.drive_ceiling_v.pop();
            }),
            ("ladder", |s| s.ladder_db = vec![0.0, -6.0]),
            ("empty ladder", |s| s.ladder_db.clear()),
        ];
        for (what, f) in breaks {
            let mut s = setup(TargetKind::Waveform);
            f(&mut s);
            assert!(
                matches!(
                    validate_shock_mimo_setup(&s),
                    Err(LawError::SetupInvalid(_))
                ),
                "{what}"
            );
        }
        let srs_breaks: [Break<SrsTarget>; 7] = [
            ("descending", |t| t.natural_hz = vec![200.0, 100.0]),
            ("points", |t| {
                t.points.pop();
            }),
            ("negative", |t| t.points[0] = -1.0),
            ("q", |t| t.q = 0.2),
            ("tolerance", |t| t.tol_lo_db = 1.0),
            ("waveforms", |t| {
                t.waveforms.pop();
            }),
            ("components", |t| {
                t.components.pop();
            }),
        ];
        for (what, f) in srs_breaks {
            let mut s = setup(TargetKind::Srs);
            if let Target::Srs(t) = &mut s.target {
                f(t);
            }
            assert!(validate_shock_mimo_setup(&s).is_err(), "{what}");
        }
    }

    #[test]
    fn mimo_shock_the_inputs_are_checked_against_the_setup() {
        let s = setup(TargetKind::Srs);
        let good = MimoShockInputs {
            pulse_index: 1,
            level_db: -6.0,
            next_level_db: -3.0,
            drives: vec![0.0; 3 * 64],
            responses: vec![0.0; 2 * 64],
            srs: Some(vec![1.0; 4]),
            verdicts: vec![
                ControlVerdict {
                    passed: false,
                    error_db: 2.0,
                    delay_s: 0.0,
                    peak_ratio: 1.0,
                };
                2
            ],
            limited: false,
        };
        validate_shock_mimo_inputs(&good, &s).unwrap();
        let breaks: [Break<MimoShockInputs>; 8] = [
            ("pulse 0", |i| i.pulse_index = 0),
            ("level", |i| i.next_level_db = f64::INFINITY),
            ("drives", |i| {
                i.drives.pop();
            }),
            ("response", |i| i.responses[3] = f64::NAN),
            ("no srs", |i| i.srs = None),
            ("short srs", |i| i.srs = Some(vec![1.0; 3])),
            ("verdicts", |i| {
                i.verdicts.pop();
            }),
            ("verdict nan", |i| i.verdicts[0].error_db = f64::NAN),
        ];
        for (what, f) in breaks {
            let mut i = good.clone();
            f(&mut i);
            assert!(
                matches!(
                    validate_shock_mimo_inputs(&i, &s),
                    Err(LawError::InputsInvalid(_))
                ),
                "{what}"
            );
        }
        let w = setup(TargetKind::Waveform);
        assert!(
            validate_shock_mimo_inputs(&good, &w).is_err(),
            "an SRS for a waveform target"
        );
    }

    #[test]
    fn mimo_shock_a_manifest_must_accept_a_target_kind_once() {
        let mut m = ShockMimoManifest {
            law: manifest(),
            targets: vec![TargetKind::Srs],
        };
        validate_shock_mimo_manifest(&m).unwrap();
        m.targets.clear();
        assert!(matches!(
            validate_shock_mimo_manifest(&m),
            Err(LawError::ManifestInvalid(_))
        ));
        m.targets = vec![TargetKind::Srs, TargetKind::Srs];
        assert!(validate_shock_mimo_manifest(&m).is_err());
        m.targets = vec![TargetKind::Srs, TargetKind::Waveform];
        validate_shock_mimo_manifest(&m).unwrap();
    }
}
