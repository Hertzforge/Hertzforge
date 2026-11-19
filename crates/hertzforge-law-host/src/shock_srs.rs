//! SRS shock law files: the `shock-srs` world.
//!
//! An SRS law is called between pulses: once after system identification
//! (`seed`) and once after each fired, measured and judged pulse (`iterate`).
//! It returns one multiplicative gain per synthesis component and up to
//! [`MAX_SRS_VALUES`] declared values. The host checks what it is handed and
//! what the law returns ([`validate_srs_setup`], [`validate_srs_inputs`],
//! [`validate_srs_outputs`]); every protection of the drive (the per-pulse
//! gain cap, the limit channels' cap, the drive ceiling and abort, the
//! predicted kinematics) belongs to the controller.
//!
//! The types here mirror the world field for field and depend on no
//! controller crate, so the controller's adapter is plain copies.

use crate::bindings::hertzforge::control_law::types as wit_types;
use crate::convert;
use crate::error::{LawError, SrsOutputRule};
use crate::host::{LawHost, LoopCall};
use crate::sandbox::{HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{DiagnosticValue, GuestOutput, LawIdentity, Manifest, Param};
use crate::validate;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::ResourceAny;
use wasmtime::Engine;

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "shock-srs",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

use bindings::hertzforge::control_law::srs_types as wit;
pub(crate) use bindings::{ShockSrs, ShockSrsPre};

/// The most values an SRS law may report on one call.
pub const MAX_SRS_VALUES: usize = 8;

/// What an SRS law is built for: fixed for the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SrsSetup<'a> {
    /// The analysis band centres, Hz, ascending.
    pub band_hz: &'a [f64],
    /// The synthesis components' frequencies, Hz, in table order.
    pub component_hz: &'a [f64],
    /// The full-level target SRS on the band grid, EU.
    pub target: &'a [f64],
    /// The quality factor every SRS is computed at.
    pub q: f64,
    /// `|H|` at each component, EU per volt, floored by the core; `None` when
    /// the run identified nothing.
    pub component_gain: Option<&'a [f64]>,
}

/// What an SRS law sees on one call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SrsInputs<'a> {
    /// Pulses fired so far: 0 at the seed.
    pub pulse_index: u64,
    /// The target at the scale the last pulse was commanded at, EU.
    pub target: &'a [f64],
    /// The last pulse's measured control SRS, EU (zeros at the seed).
    pub measured: &'a [f64],
    /// The scale the last pulse was commanded at, linear.
    pub commanded_scale: f64,
}

/// One call's result, checked by the host: one finite gain above zero per
/// component, and at most [`MAX_SRS_VALUES`] declared, finite values, each
/// once.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsOutputs {
    /// One multiplicative gain per component, in table order.
    pub gains: Vec<f64>,
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

/// Check an SRS setup: a band grid that is positive and ascending, components
/// with positive frequencies, a target per band that is finite and not
/// negative, a usable Q, and (when given) one positive identified gain per
/// component.
pub fn validate_srs_setup(s: &SrsSetup<'_>) -> Result<(), LawError> {
    if s.band_hz.is_empty() {
        return Err(setup_err("the band grid is empty"));
    }
    if s.component_hz.is_empty() {
        return Err(setup_err("the component table is empty"));
    }
    finite("band_hz", s.band_hz).map_err(setup_err)?;
    if s.band_hz[0] <= 0.0 || s.band_hz.windows(2).any(|w| w[1] <= w[0]) {
        return Err(setup_err("the band centres are not positive and ascending"));
    }
    finite("component_hz", s.component_hz).map_err(setup_err)?;
    if s.component_hz.iter().any(|&f| f <= 0.0) {
        return Err(setup_err("a component frequency is not above zero"));
    }
    if s.target.len() != s.band_hz.len() {
        return Err(setup_err(format!(
            "the target has {} bands; the grid has {}",
            s.target.len(),
            s.band_hz.len()
        )));
    }
    finite("target", s.target).map_err(setup_err)?;
    if s.target.iter().any(|&t| t < 0.0) {
        return Err(setup_err("the target is negative at a band"));
    }
    if !(s.q.is_finite() && s.q > 0.5) {
        return Err(setup_err(format!(
            "the quality factor {} is not usable",
            s.q
        )));
    }
    if let Some(h) = s.component_gain {
        if h.len() != s.component_hz.len() {
            return Err(setup_err(format!(
                "{} identified gains for {} components",
                h.len(),
                s.component_hz.len()
            )));
        }
        if let Some(i) = h.iter().position(|&g| !(g.is_finite() && g > 0.0)) {
            return Err(setup_err(format!(
                "the identified gain at component {i} is not a positive number"
            )));
        }
    }
    Ok(())
}

/// Check one call's inputs against the band grid: a target and a measured
/// SRS per band, finite and not negative, and a positive finite scale.
pub fn validate_srs_inputs(i: &SrsInputs<'_>, bands: usize) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    if i.target.len() != bands || i.measured.len() != bands {
        return bad(format!(
            "the target has {} and the measured SRS {} bands; the grid has {bands}",
            i.target.len(),
            i.measured.len()
        ));
    }
    finite("target", i.target).map_err(LawError::InputsInvalid)?;
    finite("measured", i.measured).map_err(LawError::InputsInvalid)?;
    if i.target.iter().chain(i.measured).any(|&v| v < 0.0) {
        return bad("an SRS is negative at a band".into());
    }
    if !(i.commanded_scale.is_finite() && i.commanded_scale > 0.0) {
        return bad(format!(
            "the commanded scale {} is not a positive number",
            i.commanded_scale
        ));
    }
    Ok(())
}

/// Check one call's output: one gain per component, finite and above zero;
/// at most [`MAX_SRS_VALUES`] values, each declared in `manifest`, reported
/// once and finite.
pub fn validate_srs_outputs(
    o: &SrsOutputs,
    components: usize,
    manifest: &Manifest,
) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadSrsOutput { rule, detail });
    if o.gains.len() != components {
        return bad(
            SrsOutputRule::WrongLength,
            format!(
                "{} gains returned; the table has {components} components",
                o.gains.len()
            ),
        );
    }
    if let Err(e) = finite("gains", &o.gains) {
        return bad(SrsOutputRule::NonFinite, e);
    }
    if let Some(j) = o.gains.iter().position(|&g| g <= 0.0) {
        return bad(
            SrsOutputRule::NotPositive,
            format!("gains[{j}] is {}; a gain must be above zero", o.gains[j]),
        );
    }
    if o.values.len() > MAX_SRS_VALUES {
        return bad(
            SrsOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_SRS_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                SrsOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                SrsOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                SrsOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

fn setup_wit(s: &SrsSetup<'_>) -> wit::SrsSetup {
    wit::SrsSetup {
        band_hz: s.band_hz.to_vec(),
        component_hz: s.component_hz.to_vec(),
        target: s.target.to_vec(),
        q: s.q,
        component_gain: s.component_gain.map(<[f64]>::to_vec),
    }
}

fn inputs_wit(i: &SrsInputs<'_>) -> wit::SrsInputs {
    wit::SrsInputs {
        pulse_index: i.pulse_index,
        target: i.target.to_vec(),
        measured: i.measured.to_vec(),
        commanded_scale: i.commanded_scale,
    }
}

fn outputs_from_wit(o: wit::SrsOutputs) -> SrsOutputs {
    SrsOutputs {
        gains: o.gains,
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

/// A compiled SRS law with a checked manifest. Cheap to instantiate as often
/// as needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedSrsLaw {
    identity: LawIdentity,
    manifest: Manifest,
    pre: ShockSrsPre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedSrsLaw {
    /// Read the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: ShockSrsPre<HostState>,
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
        let manifest = convert::manifest(manifest);
        validate::validate_manifest(&manifest)?;
        if manifest.diagnostics.len() > MAX_SRS_VALUES {
            return Err(LawError::ManifestInvalid(format!(
                "{} values declared; an SRS law reports at most {MAX_SRS_VALUES}",
                manifest.diagnostics.len()
            )));
        }
        Ok(LoadedSrsLaw {
            identity: LawIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                author: manifest.author.clone(),
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
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Check `supplied` parameter values against the manifest and complete
    /// them with defaults (one control channel).
    pub fn resolve_params(&self, supplied: &[Param]) -> Result<Vec<Param>, LawError> {
        validate::resolve_params(&self.manifest, supplied, 1)
    }

    /// Build the law for a run: check the setup and the parameter values,
    /// instantiate the law in a fresh sandbox and call its constructor, all
    /// within `budget`.
    pub fn instantiate(
        &self,
        setup: &SrsSetup<'_>,
        params: &[Param],
        budget: Duration,
    ) -> Result<SrsLawInstance, LawError> {
        validate_srs_setup(setup)?;
        let params = self.resolve_params(params)?;
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
            .hertzforge_control_law_shock_srs_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(SrsLawInstance {
            identity: self.identity.clone(),
            manifest: self.manifest.clone(),
            params,
            bands: setup.band_hz.len(),
            components: setup.component_hz.len(),
            sandbox: sb,
            bindings,
            law,
            ticker,
            poisoned: None,
        })
    }
}

/// One SRS law built for one run, with its own sandbox. Every call takes a
/// wall-clock budget; a call that traps, runs past its budget or exceeds the
/// memory cap leaves the instance unusable ([`LawError::Poisoned`] from then
/// on).
pub struct SrsLawInstance {
    identity: LawIdentity,
    manifest: Manifest,
    params: Vec<Param>,
    bands: usize,
    components: usize,
    sandbox: Sandbox,
    bindings: ShockSrs,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl SrsLawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The complete parameter values the law was built with, in manifest
    /// order (for the run record).
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Bands on the grid the law was built for.
    pub fn bands(&self) -> usize {
        self.bands
    }

    /// Components in the table the law was built for.
    pub fn components(&self) -> usize {
        self.components
    }

    /// Whether an earlier failure left this instance unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// The kept tail of what the law wrote to stdout and stderr.
    pub fn guest_output(&self) -> GuestOutput {
        self.sandbox.output()
    }

    /// The starting gains, before any pulse has fired.
    pub fn seed(
        &mut self,
        inputs: &SrsInputs<'_>,
        budget: Duration,
    ) -> Result<SrsOutputs, LawError> {
        self.call(LoopCall::Seed, inputs, budget)
    }

    /// The next gains, after a pulse was fired, measured and judged.
    pub fn iterate(
        &mut self,
        inputs: &SrsInputs<'_>,
        budget: Duration,
    ) -> Result<SrsOutputs, LawError> {
        self.call(LoopCall::Iterate, inputs, budget)
    }

    fn call(
        &mut self,
        which: LoopCall,
        inputs: &SrsInputs<'_>,
        budget: Duration,
    ) -> Result<SrsOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_srs_inputs(inputs, self.bands)?;
        let wit_inputs = inputs_wit(inputs);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_shock_srs_law().law();
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
        let out = outputs_from_wit(out);
        validate_srs_outputs(&out, self.components, &self.manifest)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiagnosticSpec;

    fn manifest() -> Manifest {
        Manifest {
            name: "t".into(),
            version: "1".into(),
            author: String::new(),
            summary: String::new(),
            params: Vec::new(),
            diagnostics: vec![DiagnosticSpec {
                key: "worst_band_error_db".into(),
                label: "w".into(),
                unit: String::new(),
                help: String::new(),
            }],
        }
    }

    fn rule(r: Result<(), LawError>) -> Option<SrsOutputRule> {
        match r {
            Err(LawError::BadSrsOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn each_output_rule_is_named() {
        let m = manifest();
        let good = SrsOutputs {
            gains: vec![1.0; 4],
            values: vec![DiagnosticValue {
                key: "worst_band_error_db".into(),
                value: -1.0,
            }],
        };
        assert_eq!(rule(validate_srs_outputs(&good, 4, &m)), None);
        let mut o = good.clone();
        o.gains.pop();
        assert_eq!(
            rule(validate_srs_outputs(&o, 4, &m)),
            Some(SrsOutputRule::WrongLength)
        );
        let mut o = good.clone();
        o.gains[1] = f64::NAN;
        assert_eq!(
            rule(validate_srs_outputs(&o, 4, &m)),
            Some(SrsOutputRule::NonFinite)
        );
        for g in [0.0, -0.5] {
            let mut o = good.clone();
            o.gains[2] = g;
            assert_eq!(
                rule(validate_srs_outputs(&o, 4, &m)),
                Some(SrsOutputRule::NotPositive)
            );
        }
        let mut o = good.clone();
        o.values[0].key = "other".into();
        assert_eq!(
            rule(validate_srs_outputs(&o, 4, &m)),
            Some(SrsOutputRule::UndeclaredValue)
        );
        let mut o = good.clone();
        o.values = (0..9)
            .map(|_| DiagnosticValue {
                key: "worst_band_error_db".into(),
                value: 1.0,
            })
            .collect();
        assert_eq!(
            rule(validate_srs_outputs(&o, 4, &m)),
            Some(SrsOutputRule::TooManyValues)
        );
        let mut o = good;
        o.values[0].value = f64::INFINITY;
        assert_eq!(
            rule(validate_srs_outputs(&o, 4, &m)),
            Some(SrsOutputRule::NonFinite)
        );
    }

    #[test]
    fn the_setup_and_inputs_are_checked_against_the_grids() {
        let bands = [100.0, 200.0, 400.0];
        let comps = [100.0, 400.0];
        let target = [1.0; 3];
        let good = SrsSetup {
            band_hz: &bands,
            component_hz: &comps,
            target: &target,
            q: 10.0,
            component_gain: Some(&[1.0, 2.0]),
        };
        validate_srs_setup(&good).unwrap();
        validate_srs_setup(&SrsSetup {
            component_gain: None,
            ..good
        })
        .unwrap();
        let down = [400.0, 200.0, 100.0];
        for bad in [
            SrsSetup {
                band_hz: &down,
                ..good
            },
            SrsSetup {
                target: &[1.0; 2],
                ..good
            },
            SrsSetup { q: 0.4, ..good },
            SrsSetup {
                component_gain: Some(&[1.0]),
                ..good
            },
            SrsSetup {
                component_gain: Some(&[1.0, 0.0]),
                ..good
            },
            SrsSetup {
                component_hz: &[],
                ..good
            },
        ] {
            assert!(validate_srs_setup(&bad).is_err(), "{bad:?}");
        }
        let zeros = [0.0; 3];
        let seed = SrsInputs {
            pulse_index: 0,
            target: &target,
            measured: &zeros,
            commanded_scale: 0.25,
        };
        validate_srs_inputs(&seed, 3).unwrap();
        assert!(validate_srs_inputs(&seed, 4).is_err());
        assert!(validate_srs_inputs(
            &SrsInputs {
                commanded_scale: 0.0,
                ..seed
            },
            3
        )
        .is_err());
        assert!(validate_srs_inputs(
            &SrsInputs {
                measured: &[f64::NAN, 0.0, 0.0],
                ..seed
            },
            3
        )
        .is_err());
    }
}
