//! Single-output Sine law files: the `sine-single` world.
//!
//! A Sine law is called once per control tick, on the controller's real-time
//! thread: `engage` for the starting drive after identification, `tick` for
//! every tick after it, and `adjust` when the operator changes a live
//! parameter. It returns one drive amplitude, its own fault judgement and up
//! to [`MAX_SINE_VALUES`] declared values. The host checks what it is handed
//! and what the law returns ([`validate_sine_setup`], [`validate_sine_inputs`],
//! [`validate_sine_outputs`]); every protection of the drive (the change and
//! rise caps, the first-drive bound, the notch, the drive ceiling and abort)
//! belongs to the controller.
//!
//! The deadline on a call is the caller's budget: the controller passes its
//! own per-call budget (a fraction of the tick), and the host's epoch ticker
//! stops a call within about one tick (1 ms by default) of it.
//!
//! The types here mirror the world field for field and depend on no
//! controller crate, so the controller's adapter is plain copies.

use crate::bindings::hertzforge::control_law::types as wit_types;
use crate::convert;
use crate::error::{LawError, SineOutputRule};
use crate::host::LawHost;
use crate::sandbox::{HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{DiagnosticValue, GuestOutput, LawIdentity, Manifest, Param, ParamKind};
use crate::validate;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::ResourceAny;
use wasmtime::Engine;

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "sine-single",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

use bindings::hertzforge::control_law::sine_types as wit;
pub(crate) use bindings::{SineSingle, SineSinglePre};

/// The most values a Sine law may report on one call.
pub const MAX_SINE_VALUES: usize = 8;

/// Why the core holds the drive this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SineHold {
    /// Nothing holds it.
    Free,
    /// A limit channel holds the control point down.
    Notch,
    /// A resonance search maps the plant at a fixed drive.
    Probe,
}

/// The identification, on its own grid, as the law sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SineIdentification<'a> {
    /// The grid, Hz, ascending.
    pub frequency_hz: &'a [f64],
    /// `|H|`, control units pk per drive unit pk.
    pub gain: &'a [f64],
    /// Coherence, 0 to 1.
    pub coherence: &'a [f64],
    /// Whether the core trusts the estimate there.
    pub trusted: &'a [bool],
    /// The full-level target at each grid frequency.
    pub target: &'a [f64],
}

/// What a Sine law is built for: fixed for the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SineSetup<'a> {
    /// Control channels in the composite.
    pub control_channels: usize,
    /// The control tick, seconds.
    pub tick_s: f64,
    /// The run band, Hz.
    pub band_hz: (f64, f64),
    /// The drive ceiling, drive units peak.
    pub drive_ceiling: f64,
    /// The identification.
    pub identification: SineIdentification<'a>,
}

/// What a Sine law sees on one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SineInputs<'a> {
    pub tick_index: u64,
    pub dt_s: f64,
    pub frequency_hz: f64,
    pub sweep_rate_hz_per_s: f64,
    pub level_db: f64,
    pub target: f64,
    pub measured: f64,
    pub channel_amplitude: &'a [f64],
    pub channel_phase_deg: &'a [f64],
    pub drive: f64,
    pub prediction_db: Option<f64>,
    pub hold: SineHold,
    pub notch_ceiling: Option<f64>,
    pub settling: bool,
}

/// One call's result, checked by the host: a finite drive at least zero and
/// at most [`MAX_SINE_VALUES`] declared, finite values, each once.
#[derive(Debug, Clone, PartialEq)]
pub struct SineOutputs {
    /// The tone's amplitude, drive units peak.
    pub drive: f64,
    /// The law's own fault judgement.
    pub faulted: bool,
    /// The law's declared values.
    pub values: Vec<DiagnosticValue>,
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

/// Check a Sine setup: 1 to 64 control channels, a positive tick and
/// ceiling, a positive band, an identification whose lists agree in length
/// with a positive ascending grid, and finite values in them.
pub fn validate_sine_setup(s: &SineSetup<'_>) -> Result<(), LawError> {
    if s.control_channels == 0 || s.control_channels > validate::MAX_CONTROL_CHANNELS {
        return Err(setup_err(format!(
            "{} control channels; 1 to {} are allowed",
            s.control_channels,
            validate::MAX_CONTROL_CHANNELS
        )));
    }
    if !(s.tick_s.is_finite() && s.tick_s > 0.0) {
        return Err(setup_err(format!(
            "the tick {} s is not positive",
            s.tick_s
        )));
    }
    if !(s.drive_ceiling.is_finite() && s.drive_ceiling > 0.0) {
        return Err(setup_err(format!(
            "the drive ceiling {} is not positive",
            s.drive_ceiling
        )));
    }
    let (lo, hi) = s.band_hz;
    if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi >= lo) {
        return Err(setup_err(format!("the band ({lo}, {hi}) Hz is not usable")));
    }
    let id = &s.identification;
    let n = id.frequency_hz.len();
    if id.gain.len() != n
        || id.coherence.len() != n
        || id.trusted.len() != n
        || id.target.len() != n
    {
        return Err(setup_err(format!(
            "the identification's lists disagree: {n} frequencies, {} gains, {} coherences, \
             {} trust flags, {} targets",
            id.gain.len(),
            id.coherence.len(),
            id.trusted.len(),
            id.target.len()
        )));
    }
    finite("frequency_hz", id.frequency_hz).map_err(setup_err)?;
    if id.frequency_hz.first().is_some_and(|&f| f <= 0.0)
        || id.frequency_hz.windows(2).any(|w| w[1] <= w[0])
    {
        return Err(setup_err(
            "the identification's grid is not positive and ascending",
        ));
    }
    finite("gain", id.gain).map_err(setup_err)?;
    finite("coherence", id.coherence).map_err(setup_err)?;
    finite("target", id.target).map_err(setup_err)?;
    Ok(())
}

/// Check one tick's inputs: one amplitude and one phase per control channel,
/// every number finite, the drive and the target not negative.
pub fn validate_sine_inputs(i: &SineInputs<'_>, channels: usize) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    if i.channel_amplitude.len() != channels || i.channel_phase_deg.len() != channels {
        return bad(format!(
            "{} amplitudes and {} phases for {channels} control channels",
            i.channel_amplitude.len(),
            i.channel_phase_deg.len()
        ));
    }
    let scalars = [
        ("dt_s", i.dt_s),
        ("frequency_hz", i.frequency_hz),
        ("sweep_rate_hz_per_s", i.sweep_rate_hz_per_s),
        ("level_db", i.level_db),
        ("target", i.target),
        ("measured", i.measured),
        ("drive", i.drive),
    ];
    if let Some((name, v)) = scalars.iter().find(|(_, v)| !v.is_finite()) {
        return bad(format!("{name} is {v}, not a finite number"));
    }
    finite("channel_amplitude", i.channel_amplitude).map_err(LawError::InputsInvalid)?;
    finite("channel_phase_deg", i.channel_phase_deg).map_err(LawError::InputsInvalid)?;
    if i.drive < 0.0 || i.target < 0.0 || i.measured < 0.0 {
        return bad("the drive, the target and the measured amplitude are not negative".into());
    }
    if i.prediction_db.is_some_and(|p| !p.is_finite())
        || i.notch_ceiling.is_some_and(|c| !c.is_finite())
    {
        return bad("the prediction and the notch ceiling are finite when given".into());
    }
    Ok(())
}

/// Check one call's output: a finite drive at least zero; at most
/// [`MAX_SINE_VALUES`] values, each declared in `manifest`, reported once and
/// finite.
pub fn validate_sine_outputs(o: &SineOutputs, manifest: &Manifest) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadSineOutput { rule, detail });
    if !o.drive.is_finite() {
        return bad(
            SineOutputRule::NonFinite,
            format!("the drive {} is not a finite number", o.drive),
        );
    }
    if o.drive < 0.0 {
        return bad(
            SineOutputRule::Negative,
            format!("the drive {} is below zero", o.drive),
        );
    }
    if o.values.len() > MAX_SINE_VALUES {
        return bad(
            SineOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_SINE_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                SineOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                SineOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                SineOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

/// Check the live keys a Sine manifest declares: each a declared number
/// parameter, at most once.
pub fn validate_live(manifest: &Manifest, live: &[String]) -> Result<(), LawError> {
    let mut seen = HashSet::new();
    for key in live {
        let Some(spec) = manifest.params.iter().find(|p| &p.key == key) else {
            return Err(LawError::ManifestInvalid(format!(
                "live parameter `{key}` is not declared"
            )));
        };
        if !matches!(spec.kind, ParamKind::Number(_)) {
            return Err(LawError::ManifestInvalid(format!(
                "live parameter `{key}` is not a number parameter"
            )));
        }
        if !seen.insert(key.as_str()) {
            return Err(LawError::ManifestInvalid(format!(
                "live parameter `{key}` is listed twice"
            )));
        }
    }
    Ok(())
}

fn hold_wit(h: SineHold) -> wit::Hold {
    match h {
        SineHold::Free => wit::Hold::Free,
        SineHold::Notch => wit::Hold::Notch,
        SineHold::Probe => wit::Hold::Probe,
    }
}

fn setup_wit(s: &SineSetup<'_>) -> wit::SineSetup {
    let id = &s.identification;
    wit::SineSetup {
        control_channels: s.control_channels as u32,
        tick_s: s.tick_s,
        band_lo_hz: s.band_hz.0,
        band_hi_hz: s.band_hz.1,
        drive_ceiling: s.drive_ceiling,
        identification: wit::Identification {
            frequency_hz: id.frequency_hz.to_vec(),
            gain: id.gain.to_vec(),
            coherence: id.coherence.to_vec(),
            trusted: id.trusted.to_vec(),
            target: id.target.to_vec(),
        },
    }
}

fn inputs_wit(i: &SineInputs<'_>) -> wit::TickInputs {
    wit::TickInputs {
        tick_index: i.tick_index,
        dt_s: i.dt_s,
        frequency_hz: i.frequency_hz,
        sweep_rate_hz_per_s: i.sweep_rate_hz_per_s,
        level_db: i.level_db,
        target: i.target,
        measured: i.measured,
        channel_amplitude: i.channel_amplitude.to_vec(),
        channel_phase_deg: i.channel_phase_deg.to_vec(),
        drive: i.drive,
        prediction_db: i.prediction_db,
        hold: hold_wit(i.hold),
        notch_ceiling: i.notch_ceiling,
        settling: i.settling,
    }
}

fn outputs_from_wit(o: wit::TickOutputs) -> SineOutputs {
    SineOutputs {
        drive: o.drive,
        faulted: o.faulted,
        values: o
            .values
            .into_iter()
            .map(|v: wit_types::DiagnosticValue| DiagnosticValue {
                key: v.key,
                value: v.value,
            })
            .collect(),
    }
}

/// A compiled Sine law with a checked manifest and live list. Cheap to
/// instantiate as often as needed; each instance has its own memory and
/// state.
#[derive(Clone)]
pub struct LoadedSineLaw {
    identity: LawIdentity,
    manifest: Manifest,
    live: Vec<String>,
    pre: SineSinglePre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedSineLaw {
    /// Read the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: SineSinglePre<HostState>,
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
        let manifest = convert::manifest(m.law);
        validate::validate_manifest(&manifest)?;
        if manifest.diagnostics.len() > MAX_SINE_VALUES {
            return Err(LawError::ManifestInvalid(format!(
                "{} values declared; a Sine law reports at most {MAX_SINE_VALUES}",
                manifest.diagnostics.len()
            )));
        }
        validate_live(&manifest, &m.live)?;
        Ok(LoadedSineLaw {
            identity: LawIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                author: manifest.author.clone(),
                sha256,
            },
            manifest,
            live: m.live,
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
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The number parameters the operator may change while the test runs.
    pub fn live(&self) -> &[String] {
        &self.live
    }

    /// Check `supplied` parameter values against the manifest and complete
    /// them with defaults, for `control_channels` channels.
    pub fn resolve_params(
        &self,
        supplied: &[Param],
        control_channels: usize,
    ) -> Result<Vec<Param>, LawError> {
        validate::resolve_params(&self.manifest, supplied, control_channels)
    }

    /// Build the law for a run: check the setup and the parameter values,
    /// instantiate the law in a fresh sandbox and call its constructor, all
    /// within `budget`.
    pub fn instantiate(
        &self,
        setup: &SineSetup<'_>,
        params: &[Param],
        budget: Duration,
    ) -> Result<SineLawInstance, LawError> {
        validate_sine_setup(setup)?;
        let params = self.resolve_params(params, setup.control_channels)?;
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
            .hertzforge_control_law_sine_single_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(SineLawInstance {
            identity: self.identity.clone(),
            manifest: self.manifest.clone(),
            live: self.live.clone(),
            params,
            channels: setup.control_channels,
            sandbox: sb,
            bindings,
            law,
            ticker,
            poisoned: None,
        })
    }
}

/// Which call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SineCall {
    Engage,
    Tick,
}

/// One Sine law built for one run, with its own sandbox. Every call takes a
/// wall-clock budget; a call that traps, runs past its budget or exceeds the
/// memory cap leaves the instance unusable ([`LawError::Poisoned`] from then
/// on).
pub struct SineLawInstance {
    identity: LawIdentity,
    manifest: Manifest,
    live: Vec<String>,
    params: Vec<Param>,
    channels: usize,
    sandbox: Sandbox,
    bindings: SineSingle,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl SineLawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The live parameters.
    pub fn live(&self) -> &[String] {
        &self.live
    }

    /// The complete parameter values the law was built with, in manifest
    /// order.
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Whether an earlier failure left this instance unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// The kept tail of what the law wrote to stdout and stderr.
    pub fn guest_output(&self) -> GuestOutput {
        self.sandbox.output()
    }

    /// The starting drive.
    pub fn engage(
        &mut self,
        inputs: &SineInputs<'_>,
        budget: Duration,
    ) -> Result<SineOutputs, LawError> {
        self.call(SineCall::Engage, inputs, budget)
    }

    /// One control tick.
    pub fn tick(
        &mut self,
        inputs: &SineInputs<'_>,
        budget: Duration,
    ) -> Result<SineOutputs, LawError> {
        self.call(SineCall::Tick, inputs, budget)
    }

    /// The operator changed a live parameter: `p`'s key must be declared live
    /// and its value within its range (checked here too).
    pub fn adjust(&mut self, p: &Param, budget: Duration) -> Result<(), LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        if !self.live.contains(&p.key) {
            return Err(LawError::ParamInvalid {
                key: p.key.clone(),
                reason: "not a live parameter of this law".into(),
            });
        }
        validate::resolve_params(&self.manifest, std::slice::from_ref(p), self.channels)?;
        let wit_param = convert::params(std::slice::from_ref(p))
            .into_iter()
            .next()
            .ok_or_else(|| LawError::Host("the parameter did not convert".into()))?;
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_sine_single_law().law();
        match guest.call_adjust(&mut self.sandbox.store, self.law, &wit_param) {
            Ok(r) => r.map_err(LawError::Law),
            Err(e) => {
                let err = self.sandbox.classify(&e);
                self.poisoned = Some(err.to_string());
                Err(err)
            }
        }
    }

    fn call(
        &mut self,
        which: SineCall,
        inputs: &SineInputs<'_>,
        budget: Duration,
    ) -> Result<SineOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_sine_inputs(inputs, self.channels)?;
        let wit_inputs = inputs_wit(inputs);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_sine_single_law().law();
        let r = match which {
            SineCall::Engage => guest.call_engage(&mut self.sandbox.store, self.law, &wit_inputs),
            SineCall::Tick => guest.call_tick(&mut self.sandbox.store, self.law, &wit_inputs),
        };
        let out = match r {
            Ok(out) => out.map_err(LawError::Law)?,
            Err(e) => {
                let err = self.sandbox.classify(&e);
                self.poisoned = Some(err.to_string());
                return Err(err);
            }
        };
        let out = outputs_from_wit(out);
        validate_sine_outputs(&out, &self.manifest)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DiagnosticSpec, NumberSpec, ParamSpec};

    fn manifest() -> Manifest {
        Manifest {
            name: "t".into(),
            version: "1".into(),
            author: String::new(),
            summary: String::new(),
            params: vec![
                ParamSpec {
                    key: "gain".into(),
                    label: "g".into(),
                    help: String::new(),
                    kind: ParamKind::Number(NumberSpec {
                        unit: String::new(),
                        min: 0.0,
                        max: 1.0,
                        step: 0.1,
                        default: 0.5,
                    }),
                },
                ParamSpec {
                    key: "on".into(),
                    label: "o".into(),
                    help: String::new(),
                    kind: ParamKind::Toggle(true),
                },
            ],
            diagnostics: vec![DiagnosticSpec {
                key: "error_db".into(),
                label: "e".into(),
                unit: "dB".into(),
                help: String::new(),
            }],
        }
    }

    fn rule(r: Result<(), LawError>) -> Option<SineOutputRule> {
        match r {
            Err(LawError::BadSineOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn each_output_rule_is_named() {
        let m = manifest();
        let good = SineOutputs {
            drive: 0.1,
            faulted: false,
            values: vec![DiagnosticValue {
                key: "error_db".into(),
                value: -1.0,
            }],
        };
        assert_eq!(rule(validate_sine_outputs(&good, &m)), None);
        for (drive, want) in [
            (f64::NAN, SineOutputRule::NonFinite),
            (f64::INFINITY, SineOutputRule::NonFinite),
            (-0.1, SineOutputRule::Negative),
        ] {
            let o = SineOutputs {
                drive,
                ..good.clone()
            };
            assert_eq!(rule(validate_sine_outputs(&o, &m)), Some(want));
        }
        let mut o = good.clone();
        o.values[0].key = "other".into();
        assert_eq!(
            rule(validate_sine_outputs(&o, &m)),
            Some(SineOutputRule::UndeclaredValue)
        );
        let mut o = good.clone();
        o.values = vec![o.values[0].clone(); 9];
        assert_eq!(
            rule(validate_sine_outputs(&o, &m)),
            Some(SineOutputRule::TooManyValues)
        );
        let mut o = good;
        o.values[0].value = f64::NAN;
        assert_eq!(
            rule(validate_sine_outputs(&o, &m)),
            Some(SineOutputRule::NonFinite)
        );
        assert_eq!(
            [
                SineOutputRule::NonFinite,
                SineOutputRule::Negative,
                SineOutputRule::UndeclaredValue,
                SineOutputRule::TooManyValues
            ]
            .map(SineOutputRule::as_str),
            [
                "non_finite",
                "negative",
                "undeclared_value",
                "too_many_values"
            ]
        );
    }

    #[test]
    fn live_keys_are_declared_number_parameters_once() {
        let m = manifest();
        assert!(validate_live(&m, &["gain".into()]).is_ok());
        assert!(validate_live(&m, &["on".into()]).is_err());
        assert!(validate_live(&m, &["nope".into()]).is_err());
        assert!(validate_live(&m, &["gain".into(), "gain".into()]).is_err());
    }

    #[test]
    fn the_setup_and_inputs_are_checked() {
        let f = [100.0, 200.0];
        let id = SineIdentification {
            frequency_hz: &f,
            gain: &[1.0, 2.0],
            coherence: &[0.9, 0.9],
            trusted: &[true, true],
            target: &[1.0, 1.0],
        };
        let good = SineSetup {
            control_channels: 1,
            tick_s: 0.01,
            band_hz: (100.0, 200.0),
            drive_ceiling: 1.0,
            identification: id,
        };
        validate_sine_setup(&good).unwrap();
        for bad in [
            SineSetup {
                control_channels: 0,
                ..good
            },
            SineSetup {
                tick_s: 0.0,
                ..good
            },
            SineSetup {
                identification: SineIdentification { gain: &[1.0], ..id },
                ..good
            },
            SineSetup {
                identification: SineIdentification {
                    frequency_hz: &[200.0, 100.0],
                    ..id
                },
                ..good
            },
        ] {
            assert!(validate_sine_setup(&bad).is_err(), "{bad:?}");
        }
        let i = SineInputs {
            tick_index: 1,
            dt_s: 0.01,
            frequency_hz: 150.0,
            sweep_rate_hz_per_s: 0.0,
            level_db: 0.0,
            target: 1.0,
            measured: 0.9,
            channel_amplitude: &[0.9],
            channel_phase_deg: &[-10.0],
            drive: 0.2,
            prediction_db: Some(-6.0),
            hold: SineHold::Free,
            notch_ceiling: None,
            settling: false,
        };
        validate_sine_inputs(&i, 1).unwrap();
        assert!(validate_sine_inputs(&i, 2).is_err());
        assert!(validate_sine_inputs(
            &SineInputs {
                measured: f64::NAN,
                ..i
            },
            1
        )
        .is_err());
    }
}
