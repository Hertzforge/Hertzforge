//! Classical shock law files: the `shock-classical` world.
//!
//! A classical law is called between pulses: once after system
//! identification (`seed`) and once after each fired, measured and judged
//! pulse (`iterate`). It returns the next full-level drive record in volts and
//! up to [`MAX_CLASSICAL_VALUES`] declared values. The host checks what it is
//! handed and what the law returns ([`validate_classical_setup`],
//! [`validate_classical_inputs`], [`validate_classical_outputs`]); every
//! protection of the drive (the change caps, the first-drive bound, the drive
//! ceiling and abort, the at-rest taper) belongs to the controller.
//!
//! The types here mirror the world field for field and depend on no
//! controller crate, so the controller's adapter is plain copies.

use crate::bindings::hertzforge::control_law::types as wit_types;
use crate::convert;
use crate::error::{ClassicalOutputRule, LawError};
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
        world: "shock-classical",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

use bindings::hertzforge::control_law::classical_types as wit;
pub(crate) use bindings::{ShockClassical, ShockClassicalPre};

/// The most values a classical law may report on one call.
pub const MAX_CLASSICAL_VALUES: usize = 8;

/// The identified plant on the grid's bins (`record_len / 2 + 1` each).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicalPlant<'a> {
    /// `H[k]` as `(re, im)`, control response per drive volt.
    pub frf: &'a [(f64, f64)],
    /// Ordinary coherence, 0 to 1.
    pub coherence: &'a [f64],
    /// The core's regularized inverse.
    pub inverse: &'a [(f64, f64)],
    /// The bins the core trusts.
    pub trusted: &'a [bool],
    /// The Tikhonov epsilon behind `inverse`.
    pub epsilon: f64,
}

/// What a classical law is built for: fixed for the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicalSetup<'a> {
    /// The rate, Hz.
    pub fs_hz: f64,
    /// Samples in every record.
    pub record_len: usize,
    /// The correction band, Hz.
    pub band_hz: (f64, f64),
    /// The full-level reference, EU, `record_len` samples.
    pub reference: &'a [f64],
    /// The main pulse's first sample and length within `reference`.
    pub main_start: usize,
    /// The main pulse's length.
    pub main_len: usize,
    /// The identified plant.
    pub plant: ClassicalPlant<'a>,
    /// The drive ceiling referred to full level, volts peak.
    pub drive_ceiling: f64,
}

/// The core's judgement of the last pulse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicalVerdict {
    /// Inside the standard's tolerance at its commanded level.
    pub passed: bool,
    /// RMS error over the judged window, percent.
    pub error_pct: f64,
    /// Measured peak over commanded peak.
    pub peak_ratio: f64,
    /// Measured velocity change over the nominal.
    pub velocity_change_ratio: f64,
}

/// What a classical law sees on one call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicalInputs<'a> {
    /// Pulses fired so far: 0 at the seed.
    pub pulse_index: u64,
    /// The last response referred to full level, EU (empty at the seed).
    pub response: &'a [f64],
    /// The last drive as played referred to full level, volts (empty at the
    /// seed).
    pub drive: &'a [f64],
    /// The core's verdict on the last pulse (`None` at the seed).
    pub judged: Option<ClassicalVerdict>,
    /// The core scaled the last pulse down.
    pub limited: bool,
}

/// One call's result, checked by the host: `record_len` finite samples and at
/// most [`MAX_CLASSICAL_VALUES`] declared, finite values, each once.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassicalOutputs {
    /// The next full-level drive, volts.
    pub drive: Vec<f64>,
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

/// Check a classical setup: lengths agree with the grid, every number is
/// finite, the band and the main pulse lie on the grid, coherence is in 0 to 1.
pub fn validate_classical_setup(s: &ClassicalSetup<'_>) -> Result<(), LawError> {
    let n = s.record_len;
    if n < 2 {
        return Err(setup_err(format!("a record of {n} samples is too short")));
    }
    let bins = n / 2 + 1;
    if !(s.fs_hz.is_finite() && s.fs_hz > 0.0) {
        return Err(setup_err(format!("the rate {} Hz is not usable", s.fs_hz)));
    }
    let (lo, hi) = s.band_hz;
    if !(lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi > lo && hi <= s.fs_hz / 2.0) {
        return Err(setup_err(format!(
            "the band [{lo}, {hi}] Hz is not on the grid"
        )));
    }
    if s.reference.len() != n {
        return Err(setup_err(format!(
            "the reference has {} samples; the record has {n}",
            s.reference.len()
        )));
    }
    if s.main_len == 0 || s.main_start + s.main_len > n {
        return Err(setup_err("the main pulse is not inside the record"));
    }
    let p = &s.plant;
    for (name, len) in [
        ("frf", p.frf.len()),
        ("coherence", p.coherence.len()),
        ("inverse", p.inverse.len()),
        ("trusted", p.trusted.len()),
    ] {
        if len != bins {
            return Err(setup_err(format!(
                "the plant's {name} has {len} bins; the record has {bins}"
            )));
        }
    }
    finite("reference", s.reference).map_err(setup_err)?;
    let flat = |c: &[(f64, f64)]| -> Vec<f64> { c.iter().flat_map(|&(a, b)| [a, b]).collect() };
    finite("frf", &flat(p.frf)).map_err(setup_err)?;
    finite("inverse", &flat(p.inverse)).map_err(setup_err)?;
    if let Some(i) = p
        .coherence
        .iter()
        .position(|c| !(c.is_finite() && (0.0..=1.0).contains(c)))
    {
        return Err(setup_err(format!("coherence[{i}] is not between 0 and 1")));
    }
    if !(p.epsilon.is_finite() && p.epsilon >= 0.0) {
        return Err(setup_err(
            "the plant's epsilon is not a non-negative number",
        ));
    }
    if !(s.drive_ceiling.is_finite() && s.drive_ceiling > 0.0) {
        return Err(setup_err("the drive ceiling is not a positive number"));
    }
    Ok(())
}

/// Check one call's inputs against the record length: at the seed the
/// response and drive are empty and there is no verdict; after a pulse they
/// are `record_len` finite samples and there is one.
pub fn validate_classical_inputs(i: &ClassicalInputs<'_>, n: usize) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    let seed = i.pulse_index == 0 && i.response.is_empty() && i.drive.is_empty();
    if seed {
        if i.judged.is_some() {
            return bad("the seed carries no verdict".into());
        }
        return Ok(());
    }
    if i.response.len() != n || i.drive.len() != n {
        return bad(format!(
            "the response has {} and the drive {} samples; the record has {n}",
            i.response.len(),
            i.drive.len()
        ));
    }
    finite("response", i.response).map_err(LawError::InputsInvalid)?;
    finite("drive", i.drive).map_err(LawError::InputsInvalid)?;
    match i.judged {
        None => bad("a call after a pulse carries its verdict".into()),
        Some(v)
            if ![v.error_pct, v.peak_ratio, v.velocity_change_ratio]
                .iter()
                .all(|x| x.is_finite()) =>
        {
            bad("the verdict holds a value that is not a finite number".into())
        }
        Some(_) => Ok(()),
    }
}

/// Check one call's output: `n` finite drive samples; at most
/// [`MAX_CLASSICAL_VALUES`] values, each declared in `manifest`, reported
/// once and finite.
pub fn validate_classical_outputs(
    o: &ClassicalOutputs,
    n: usize,
    manifest: &Manifest,
) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadClassicalOutput { rule, detail });
    if o.drive.len() != n {
        return bad(
            ClassicalOutputRule::WrongLength,
            format!(
                "the drive has {} samples; the record has {n}",
                o.drive.len()
            ),
        );
    }
    if let Err(e) = finite("drive", &o.drive) {
        return bad(ClassicalOutputRule::NonFinite, e);
    }
    if o.values.len() > MAX_CLASSICAL_VALUES {
        return bad(
            ClassicalOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_CLASSICAL_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                ClassicalOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                ClassicalOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                ClassicalOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

fn setup_wit(s: &ClassicalSetup<'_>) -> wit::ClassicalSetup {
    let c = |v: &[(f64, f64)]| -> Vec<wit::Complex> {
        v.iter().map(|&(re, im)| wit::Complex { re, im }).collect()
    };
    wit::ClassicalSetup {
        fs_hz: s.fs_hz,
        record_len: s.record_len as u32,
        band_lo_hz: s.band_hz.0,
        band_hi_hz: s.band_hz.1,
        reference: s.reference.to_vec(),
        main_start: s.main_start as u32,
        main_len: s.main_len as u32,
        plant: wit::Plant {
            frf: c(s.plant.frf),
            coherence: s.plant.coherence.to_vec(),
            inverse: c(s.plant.inverse),
            trusted: s.plant.trusted.to_vec(),
            epsilon: s.plant.epsilon,
        },
        drive_ceiling: s.drive_ceiling,
    }
}

fn inputs_wit(i: &ClassicalInputs<'_>) -> wit::PulseInputs {
    wit::PulseInputs {
        pulse_index: i.pulse_index,
        response: i.response.to_vec(),
        drive: i.drive.to_vec(),
        judged: i.judged.map(|v| wit::Verdict {
            passed: v.passed,
            error_pct: v.error_pct,
            peak_ratio: v.peak_ratio,
            velocity_change_ratio: v.velocity_change_ratio,
        }),
        limited: i.limited,
    }
}

fn outputs_from_wit(o: wit::PulseOutputs) -> ClassicalOutputs {
    ClassicalOutputs {
        drive: o.drive,
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

/// A compiled classical law with a checked manifest. Cheap to instantiate as
/// often as needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedClassicalLaw {
    identity: LawIdentity,
    manifest: Manifest,
    pre: ShockClassicalPre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedClassicalLaw {
    /// Read the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: ShockClassicalPre<HostState>,
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
        if manifest.diagnostics.len() > MAX_CLASSICAL_VALUES {
            return Err(LawError::ManifestInvalid(format!(
                "{} values declared; a classical law reports at most {MAX_CLASSICAL_VALUES}",
                manifest.diagnostics.len()
            )));
        }
        Ok(LoadedClassicalLaw {
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
        setup: &ClassicalSetup<'_>,
        params: &[Param],
        budget: Duration,
    ) -> Result<ClassicalLawInstance, LawError> {
        validate_classical_setup(setup)?;
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
            .hertzforge_control_law_shock_classical_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(ClassicalLawInstance {
            identity: self.identity.clone(),
            manifest: self.manifest.clone(),
            params,
            record_len: setup.record_len,
            sandbox: sb,
            bindings,
            law,
            ticker,
            poisoned: None,
        })
    }
}

/// One classical law built for one run, with its own sandbox. Every call
/// takes a wall-clock budget; a call that traps, runs past its budget or
/// exceeds the memory cap leaves the instance unusable ([`LawError::Poisoned`]
/// from then on).
pub struct ClassicalLawInstance {
    identity: LawIdentity,
    manifest: Manifest,
    params: Vec<Param>,
    record_len: usize,
    sandbox: Sandbox,
    bindings: ShockClassical,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl ClassicalLawInstance {
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

    /// Samples in every record the law was built for.
    pub fn record_len(&self) -> usize {
        self.record_len
    }

    /// Whether an earlier failure left this instance unusable.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_some()
    }

    /// The kept tail of what the law wrote to stdout and stderr.
    pub fn guest_output(&self) -> GuestOutput {
        self.sandbox.output()
    }

    /// The first drive, before any pulse has fired.
    pub fn seed(
        &mut self,
        inputs: &ClassicalInputs<'_>,
        budget: Duration,
    ) -> Result<ClassicalOutputs, LawError> {
        self.call(LoopCall::Seed, inputs, budget)
    }

    /// The next drive, after a pulse was fired, measured and judged.
    pub fn iterate(
        &mut self,
        inputs: &ClassicalInputs<'_>,
        budget: Duration,
    ) -> Result<ClassicalOutputs, LawError> {
        self.call(LoopCall::Iterate, inputs, budget)
    }

    fn call(
        &mut self,
        which: LoopCall,
        inputs: &ClassicalInputs<'_>,
        budget: Duration,
    ) -> Result<ClassicalOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_classical_inputs(inputs, self.record_len)?;
        let wit_inputs = inputs_wit(inputs);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self
            .bindings
            .hertzforge_control_law_shock_classical_law()
            .law();
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
        validate_classical_outputs(&out, self.record_len, &self.manifest)?;
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
                key: "corrected_bins".into(),
                label: "c".into(),
                unit: String::new(),
                help: String::new(),
            }],
        }
    }

    fn rule(r: Result<(), LawError>) -> Option<ClassicalOutputRule> {
        match r {
            Err(LawError::BadClassicalOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn each_output_rule_is_named() {
        let m = manifest();
        let good = ClassicalOutputs {
            drive: vec![0.0; 8],
            values: vec![DiagnosticValue {
                key: "corrected_bins".into(),
                value: 3.0,
            }],
        };
        assert_eq!(rule(validate_classical_outputs(&good, 8, &m)), None);
        let mut o = good.clone();
        o.drive.pop();
        assert_eq!(
            rule(validate_classical_outputs(&o, 8, &m)),
            Some(ClassicalOutputRule::WrongLength)
        );
        let mut o = good.clone();
        o.drive[2] = f64::INFINITY;
        assert_eq!(
            rule(validate_classical_outputs(&o, 8, &m)),
            Some(ClassicalOutputRule::NonFinite)
        );
        let mut o = good.clone();
        o.values[0].key = "other".into();
        assert_eq!(
            rule(validate_classical_outputs(&o, 8, &m)),
            Some(ClassicalOutputRule::UndeclaredValue)
        );
        let mut o = good.clone();
        o.values = (0..9)
            .map(|_| DiagnosticValue {
                key: "corrected_bins".into(),
                value: 1.0,
            })
            .collect();
        assert_eq!(
            rule(validate_classical_outputs(&o, 8, &m)),
            Some(ClassicalOutputRule::TooManyValues)
        );
        let mut o = good;
        o.values[0].value = f64::NAN;
        assert_eq!(
            rule(validate_classical_outputs(&o, 8, &m)),
            Some(ClassicalOutputRule::NonFinite)
        );
    }

    #[test]
    fn the_setup_and_inputs_are_checked_against_the_grid() {
        let reference = vec![0.0; 8];
        let frf = vec![(1.0, 0.0); 5];
        let coh = vec![1.0; 5];
        let trusted = vec![true; 5];
        let good = ClassicalSetup {
            fs_hz: 1000.0,
            record_len: 8,
            band_hz: (10.0, 400.0),
            reference: &reference,
            main_start: 2,
            main_len: 3,
            plant: ClassicalPlant {
                frf: &frf,
                coherence: &coh,
                inverse: &frf,
                trusted: &trusted,
                epsilon: 0.01,
            },
            drive_ceiling: 0.5,
        };
        validate_classical_setup(&good).unwrap();
        let short = vec![0.0; 7];
        assert!(validate_classical_setup(&ClassicalSetup {
            reference: &short,
            ..good
        })
        .is_err());
        assert!(validate_classical_setup(&ClassicalSetup {
            main_start: 6,
            ..good
        })
        .is_err());
        let bad_coh = vec![1.5; 5];
        assert!(validate_classical_setup(&ClassicalSetup {
            plant: ClassicalPlant {
                coherence: &bad_coh,
                ..good.plant
            },
            ..good
        })
        .is_err());
        let seed = ClassicalInputs {
            pulse_index: 0,
            response: &[],
            drive: &[],
            judged: None,
            limited: false,
        };
        validate_classical_inputs(&seed, 8).unwrap();
        let v = ClassicalVerdict {
            passed: true,
            error_pct: 1.0,
            peak_ratio: 1.0,
            velocity_change_ratio: 1.0,
        };
        let after = ClassicalInputs {
            pulse_index: 1,
            response: &reference,
            drive: &reference,
            judged: Some(v),
            limited: false,
        };
        validate_classical_inputs(&after, 8).unwrap();
        assert!(validate_classical_inputs(
            &ClassicalInputs {
                judged: None,
                ..after
            },
            8
        )
        .is_err());
        assert!(validate_classical_inputs(&after, 16).is_err());
    }
}
