//! Time waveform replication law files: the `twr-single` world.
//!
//! A TWR law is called between plays, never while one plays: once at proceed,
//! after the identification (`seed`), and after each judged play the core
//! wants corrected (`iterate`). It returns the next full-level drive record in
//! volts and up to [`MAX_TWR_VALUES`] declared values. The host checks what it
//! is handed and what the law returns ([`validate_twr_setup`],
//! [`validate_twr_inputs`], [`validate_twr_outputs`]); every protection of the
//! drive (the change caps, the predicted peak, the first-drive bound, the drive
//! ceiling, the envelope, the alignment, the at-rest taper) belongs to the
//! controller.
//!
//! The types here mirror the world field for field and depend on no
//! controller crate, so the controller's adapter is plain copies. The plant
//! has the classical world's shape (`classical-types.plant`).

use crate::bindings::hertzforge::control_law::types as wit_types;
use crate::convert;
use crate::error::{LawError, TwrOutputRule};
use crate::host::{LawHost, LoopCall};
use crate::sandbox::{HostState, Sandbox};
use crate::shock_classical::ClassicalPlant;
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
        world: "twr-single",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
            "hertzforge:control-law/classical-types":
                crate::shock_classical::bindings::hertzforge::control_law::classical_types,
        },
    });
}

use crate::shock_classical::bindings::hertzforge::control_law::classical_types as wit_classical;
use bindings::hertzforge::control_law::twr_types as wit;
pub(crate) use bindings::{TwrSingle, TwrSinglePre};

/// The most values a TWR law may report on one call.
pub const MAX_TWR_VALUES: usize = 8;

/// What a TWR law is built for: fixed for the run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwrSetup<'a> {
    /// The rate, Hz.
    pub fs_hz: f64,
    /// Samples in every record.
    pub record_len: usize,
    /// The correction band, Hz.
    pub band_hz: (f64, f64),
    /// The full-level reference on the record grid, EU, `record_len` samples.
    pub reference: &'a [f64],
    /// The replicated section's first sample within `reference`.
    pub section_start: usize,
    /// The replicated section's length.
    pub section_len: usize,
    /// The identified plant (the classical world's shape).
    pub plant: ClassicalPlant<'a>,
    /// The drive ceiling at full level, volts peak.
    pub drive_ceiling: f64,
}

/// The core's judgement of the last play.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwrVerdict {
    /// The play met the convergence gate at its level.
    pub gated: bool,
    /// `100 ||y - r|| / ||r||` over the record.
    pub rms_error_pct: f64,
    /// `100 max|y - r| / max|r|`.
    pub peak_error_pct: f64,
    /// Mean `|20 log10(|Y| / |R|)|` over the trusted bins, dB.
    pub spectral_error_db: f64,
}

/// What the last play was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwrPlayKind {
    /// A correction play on the ladder (also the seed's).
    Iteration,
    /// A scheduled playback play.
    Playback,
}

/// What a TWR law sees on one call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwrInputs<'a> {
    /// Plays judged so far, the identification not counted: 0 at the seed.
    pub play_index: u64,
    /// What the last play was.
    pub kind: TwrPlayKind,
    /// The last response referred to full level, EU (empty at the seed).
    pub response: &'a [f64],
    /// The last drive as played referred to full level, volts (empty at the
    /// seed).
    pub drive: &'a [f64],
    /// The core's verdict on the last play (`None` at the seed).
    pub judged: Option<TwrVerdict>,
    /// The core clipped the last drive.
    pub limited: bool,
}

/// One call's result, checked by the host: `record_len` finite samples and at
/// most [`MAX_TWR_VALUES`] declared, finite values, each once.
#[derive(Debug, Clone, PartialEq)]
pub struct TwrOutputs {
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

/// Check a TWR setup: lengths agree with the grid, every number is finite,
/// the band and the section lie on the grid, coherence is in 0 to 1.
pub fn validate_twr_setup(s: &TwrSetup<'_>) -> Result<(), LawError> {
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
    if s.section_len == 0 || s.section_start + s.section_len > n {
        return Err(setup_err("the section is not inside the record"));
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
/// response and drive are empty and there is no verdict; after a play they are
/// `record_len` finite samples and there is one.
pub fn validate_twr_inputs(i: &TwrInputs<'_>, n: usize) -> Result<(), LawError> {
    let bad = |m: String| Err(LawError::InputsInvalid(m));
    let seed = i.play_index == 0 && i.response.is_empty() && i.drive.is_empty();
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
        None => bad("a call after a play carries its verdict".into()),
        Some(v)
            if ![v.rms_error_pct, v.peak_error_pct, v.spectral_error_db]
                .iter()
                .all(|x| x.is_finite()) =>
        {
            bad("the verdict holds a value that is not a finite number".into())
        }
        Some(_) => Ok(()),
    }
}

/// Check one call's output: `n` finite drive samples; at most
/// [`MAX_TWR_VALUES`] values, each declared in `manifest`, reported once and
/// finite.
pub fn validate_twr_outputs(o: &TwrOutputs, n: usize, manifest: &Manifest) -> Result<(), LawError> {
    let bad = |rule, detail: String| Err(LawError::BadTwrOutput { rule, detail });
    if o.drive.len() != n {
        return bad(
            TwrOutputRule::WrongLength,
            format!(
                "the drive has {} samples; the record has {n}",
                o.drive.len()
            ),
        );
    }
    if let Err(e) = finite("drive", &o.drive) {
        return bad(TwrOutputRule::NonFinite, e);
    }
    if o.values.len() > MAX_TWR_VALUES {
        return bad(
            TwrOutputRule::TooManyValues,
            format!(
                "{} values reported; at most {MAX_TWR_VALUES} are allowed",
                o.values.len()
            ),
        );
    }
    let mut seen = HashSet::new();
    for v in &o.values {
        if !manifest.diagnostics.iter().any(|d| d.key == v.key) {
            return bad(
                TwrOutputRule::UndeclaredValue,
                format!("`{}` is not a value the manifest declares", v.key),
            );
        }
        if !seen.insert(v.key.as_str()) {
            return bad(
                TwrOutputRule::UndeclaredValue,
                format!("`{}` was reported twice", v.key),
            );
        }
        if !v.value.is_finite() {
            return bad(
                TwrOutputRule::NonFinite,
                format!("the value `{}` is not a finite number", v.key),
            );
        }
    }
    Ok(())
}

fn setup_wit(s: &TwrSetup<'_>) -> wit::TwrSetup {
    let c = |v: &[(f64, f64)]| -> Vec<wit_classical::Complex> {
        v.iter()
            .map(|&(re, im)| wit_classical::Complex { re, im })
            .collect()
    };
    wit::TwrSetup {
        fs_hz: s.fs_hz,
        record_len: s.record_len as u32,
        band_lo_hz: s.band_hz.0,
        band_hi_hz: s.band_hz.1,
        reference: s.reference.to_vec(),
        section_start: s.section_start as u32,
        section_len: s.section_len as u32,
        plant: wit_classical::Plant {
            frf: c(s.plant.frf),
            coherence: s.plant.coherence.to_vec(),
            inverse: c(s.plant.inverse),
            trusted: s.plant.trusted.to_vec(),
            epsilon: s.plant.epsilon,
        },
        drive_ceiling: s.drive_ceiling,
    }
}

fn inputs_wit(i: &TwrInputs<'_>) -> wit::PlayInputs {
    wit::PlayInputs {
        play_index: i.play_index,
        kind: match i.kind {
            TwrPlayKind::Iteration => wit::PlayKind::Iteration,
            TwrPlayKind::Playback => wit::PlayKind::Playback,
        },
        response: i.response.to_vec(),
        drive: i.drive.to_vec(),
        judged: i.judged.map(|v| wit::Verdict {
            gated: v.gated,
            rms_error_pct: v.rms_error_pct,
            peak_error_pct: v.peak_error_pct,
            spectral_error_db: v.spectral_error_db,
        }),
        limited: i.limited,
    }
}

fn outputs_from_wit(o: wit::PlayOutputs) -> TwrOutputs {
    TwrOutputs {
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

/// A compiled TWR law with a checked manifest. Cheap to instantiate as often
/// as needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedTwrLaw {
    identity: LawIdentity,
    manifest: Manifest,
    pre: TwrSinglePre<HostState>,
    engine: Engine,
    memory_limit_bytes: usize,
    capture_bytes: usize,
    ticker: Arc<Ticker>,
}

impl LoadedTwrLaw {
    /// Read the manifest of a law linked against `host`'s sandbox.
    pub(crate) fn read(
        host: &LawHost,
        sha256: String,
        pre: TwrSinglePre<HostState>,
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
        if manifest.diagnostics.len() > MAX_TWR_VALUES {
            return Err(LawError::ManifestInvalid(format!(
                "{} values declared; a TWR law reports at most {MAX_TWR_VALUES}",
                manifest.diagnostics.len()
            )));
        }
        Ok(LoadedTwrLaw {
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
        setup: &TwrSetup<'_>,
        params: &[Param],
        budget: Duration,
    ) -> Result<TwrLawInstance, LawError> {
        validate_twr_setup(setup)?;
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
            .hertzforge_control_law_twr_single_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(TwrLawInstance {
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

/// One TWR law built for one run, with its own sandbox. Every call takes a
/// wall-clock budget; a call that traps, runs past its budget or exceeds the
/// memory cap leaves the instance unusable ([`LawError::Poisoned`] from then
/// on).
pub struct TwrLawInstance {
    identity: LawIdentity,
    manifest: Manifest,
    params: Vec<Param>,
    record_len: usize,
    sandbox: Sandbox,
    bindings: TwrSingle,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl TwrLawInstance {
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

    /// The first drive, before any play.
    pub fn seed(
        &mut self,
        inputs: &TwrInputs<'_>,
        budget: Duration,
    ) -> Result<TwrOutputs, LawError> {
        self.call(LoopCall::Seed, inputs, budget)
    }

    /// The next drive, after a play was played, measured and judged.
    pub fn iterate(
        &mut self,
        inputs: &TwrInputs<'_>,
        budget: Duration,
    ) -> Result<TwrOutputs, LawError> {
        self.call(LoopCall::Iterate, inputs, budget)
    }

    fn call(
        &mut self,
        which: LoopCall,
        inputs: &TwrInputs<'_>,
        budget: Duration,
    ) -> Result<TwrOutputs, LawError> {
        if let Some(first) = &self.poisoned {
            return Err(LawError::Poisoned(first.clone()));
        }
        validate_twr_inputs(inputs, self.record_len)?;
        let wit_inputs = inputs_wit(inputs);
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self.bindings.hertzforge_control_law_twr_single_law().law();
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
        validate_twr_outputs(&out, self.record_len, &self.manifest)?;
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

    fn rule(r: Result<(), LawError>) -> Option<TwrOutputRule> {
        match r {
            Err(LawError::BadTwrOutput { rule, .. }) => Some(rule),
            Ok(()) => None,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn each_output_rule_is_named() {
        let m = manifest();
        let good = TwrOutputs {
            drive: vec![0.0; 8],
            values: vec![DiagnosticValue {
                key: "corrected_bins".into(),
                value: 3.0,
            }],
        };
        assert_eq!(rule(validate_twr_outputs(&good, 8, &m)), None);
        let mut o = good.clone();
        o.drive.pop();
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::WrongLength)
        );
        let mut o = good.clone();
        o.drive[2] = f64::INFINITY;
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::NonFinite)
        );
        let mut o = good.clone();
        o.values[0].key = "other".into();
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::UndeclaredValue)
        );
        let mut o = good.clone();
        o.values.push(o.values[0].clone());
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::UndeclaredValue),
            "reported twice"
        );
        let mut o = good.clone();
        o.values = (0..9)
            .map(|_| DiagnosticValue {
                key: "corrected_bins".into(),
                value: 1.0,
            })
            .collect();
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::TooManyValues)
        );
        let mut o = good;
        o.values[0].value = f64::NAN;
        assert_eq!(
            rule(validate_twr_outputs(&o, 8, &m)),
            Some(TwrOutputRule::NonFinite)
        );
    }

    #[test]
    fn the_setup_and_inputs_are_checked_against_the_grid() {
        let reference = vec![0.0; 8];
        let frf = vec![(1.0, 0.0); 5];
        let coh = vec![1.0; 5];
        let trusted = vec![true; 5];
        let good = TwrSetup {
            fs_hz: 1000.0,
            record_len: 8,
            band_hz: (10.0, 400.0),
            reference: &reference,
            section_start: 2,
            section_len: 3,
            plant: ClassicalPlant {
                frf: &frf,
                coherence: &coh,
                inverse: &frf,
                trusted: &trusted,
                epsilon: 0.01,
            },
            drive_ceiling: 0.5,
        };
        validate_twr_setup(&good).unwrap();
        let short = vec![0.0; 7];
        assert!(validate_twr_setup(&TwrSetup {
            reference: &short,
            ..good
        })
        .is_err());
        assert!(validate_twr_setup(&TwrSetup {
            section_start: 6,
            ..good
        })
        .is_err());
        let seed = TwrInputs {
            play_index: 0,
            kind: TwrPlayKind::Iteration,
            response: &[],
            drive: &[],
            judged: None,
            limited: false,
        };
        validate_twr_inputs(&seed, 8).unwrap();
        let v = TwrVerdict {
            gated: false,
            rms_error_pct: 12.0,
            peak_error_pct: 20.0,
            spectral_error_db: 1.0,
        };
        let after = TwrInputs {
            play_index: 1,
            kind: TwrPlayKind::Iteration,
            response: &reference,
            drive: &reference,
            judged: Some(v),
            limited: false,
        };
        validate_twr_inputs(&after, 8).unwrap();
        assert!(validate_twr_inputs(
            &TwrInputs {
                judged: None,
                ..after
            },
            8
        )
        .is_err());
        assert!(validate_twr_inputs(&after, 16).is_err());
    }
}
