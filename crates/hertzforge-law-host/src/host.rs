//! [`LawHost`] loads law files; [`LoadedLaw`] is one compiled law;
//! [`LawInstance`] is one law built for a run.

use crate::bindings::{RandomSingle, RandomSinglePre};
use crate::convert;
use crate::error::LawError;
use crate::sandbox::{self, HostState, Sandbox};
use crate::ticker::Ticker;
use crate::types::{
    DiagnosticSpec, GuestOutput, LawIdentity, LoopInputs, LoopOutputs, Manifest, Param, Setup,
};
use crate::validate;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::{Component, Linker, ResourceAny};
use wasmtime::{Config, Engine};

/// How a [`LawHost`] contains the laws it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct HostConfig {
    /// Cap on a law's total linear memory, bytes. Default 256 MiB.
    pub memory_limit_bytes: usize,
    /// How often the deadline is checked while a call runs. A call is
    /// stopped within about one tick of its budget. Default 1 ms.
    pub epoch_tick: Duration,
    /// Budget for instantiating a law and reading its manifest at load.
    /// Default 1 s.
    pub describe_budget: Duration,
    /// How much of each of stdout and stderr is kept (the last bytes).
    /// Default 16 KiB.
    pub capture_bytes: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            memory_limit_bytes: 256 * 1024 * 1024,
            epoch_tick: Duration::from_millis(1),
            describe_budget: Duration::from_secs(1),
            capture_bytes: 16 * 1024,
        }
    }
}

impl HostConfig {
    fn validate(&self) -> Result<(), LawError> {
        // A law needs at least one 64 KiB page of memory to run at all.
        if self.memory_limit_bytes < 64 * 1024 {
            return Err(LawError::Host(format!(
                "a memory cap of {} bytes is below one WebAssembly page",
                self.memory_limit_bytes
            )));
        }
        if self.epoch_tick.is_zero() || self.epoch_tick > Duration::from_millis(100) {
            return Err(LawError::Host(format!(
                "an epoch tick of {:?} is outside (0, 100 ms]",
                self.epoch_tick
            )));
        }
        if self.capture_bytes > 1024 * 1024 {
            return Err(LawError::Host(format!(
                "a capture of {} bytes is over the 1 MiB allowed",
                self.capture_bytes
            )));
        }
        Ok(())
    }
}

fn chain(e: &wasmtime::Error) -> String {
    e.chain()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// The SHA-256 of a law file as 64 lower-case hex digits: the hash
/// [`LawIdentity::sha256`] carries.
pub fn law_file_sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Loads and compiles law files. One per process is enough; it owns the
/// Wasmtime engine, the linker and the deadline ticker thread.
pub struct LawHost {
    engine: Engine,
    linker: Linker<HostState>,
    config: HostConfig,
    ticker: Arc<Ticker>,
}

impl LawHost {
    /// A host with `config`. Fails on a config out of range or when the
    /// ticker thread cannot start.
    pub fn new(config: HostConfig) -> Result<Self, LawError> {
        config.validate()?;
        let mut cfg = Config::new();
        cfg.wasm_component_model(true)
            .epoch_interruption(true)
            // Determinism: NaN bit patterns and relaxed-SIMD results are
            // otherwise allowed to differ between machines.
            .cranelift_nan_canonicalization(true)
            .relaxed_simd_deterministic(true);
        let engine =
            Engine::new(&cfg).map_err(|e| LawError::Host(format!("engine: {}", chain(&e))))?;
        let linker = sandbox::linker(&engine)
            .map_err(|e| LawError::Host(format!("linker: {}", chain(&e))))?;
        let ticker = Arc::new(Ticker::start(engine.clone(), config.epoch_tick)?);
        Ok(LawHost {
            engine,
            linker,
            config,
            ticker,
        })
    }

    /// The configuration this host runs laws under.
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Compile a law file and read its manifest. This is the only place a
    /// law is compiled; instantiating it later for a run compiles nothing.
    ///
    /// Fails with [`LawError::Compile`] when the bytes are not a component,
    /// [`LawError::NotALaw`] when the component is not a `random-single` law
    /// or imports more than WASI 0.2, [`LawError::ManifestInvalid`] when the
    /// manifest breaks a rule, and a trap, deadline or memory error when the
    /// law fails while being read.
    pub fn load(&self, bytes: &[u8]) -> Result<LoadedLaw, LawError> {
        let sha256 = law_file_sha256(bytes);
        let component =
            Component::new(&self.engine, bytes).map_err(|e| LawError::Compile(chain(&e)))?;
        let pre = self
            .linker
            .instantiate_pre(&component)
            .map_err(|e| LawError::NotALaw(chain(&e)))?;
        let pre = RandomSinglePre::new(pre).map_err(|e| LawError::NotALaw(chain(&e)))?;

        let mut sb = Sandbox::new(
            &self.engine,
            self.config.memory_limit_bytes,
            self.config.capture_bytes,
        );
        let _active = self.ticker.active();
        sb.arm(self.config.describe_budget);
        let bindings = pre
            .instantiate(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let manifest = bindings
            .call_describe(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let manifest = convert::manifest(manifest);
        validate::validate_manifest(&manifest)?;
        Ok(LoadedLaw {
            identity: LawIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                author: manifest.author.clone(),
                sha256,
            },
            manifest,
            pre,
            engine: self.engine.clone(),
            config: self.config.clone(),
            ticker: Arc::clone(&self.ticker),
        })
    }
}

/// A compiled law with a checked manifest. Cheap to instantiate as often as
/// needed; each instance has its own memory and state.
#[derive(Clone)]
pub struct LoadedLaw {
    identity: LawIdentity,
    manifest: Manifest,
    pre: RandomSinglePre<HostState>,
    engine: Engine,
    config: HostConfig,
    ticker: Arc<Ticker>,
}

impl LoadedLaw {
    /// Name, version, author and file hash.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The law's checked manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Check `supplied` parameter values against the manifest and complete
    /// them with defaults: the list [`Self::instantiate`] hands the law.
    pub fn resolve_params(
        &self,
        supplied: &[Param],
        control_channels: usize,
    ) -> Result<Vec<Param>, LawError> {
        validate::resolve_params(&self.manifest, supplied, control_channels)
    }

    /// Build the law for a run: check the setup and parameter values,
    /// instantiate the law in a fresh sandbox and call its constructor, all
    /// within `budget`.
    pub fn instantiate(
        &self,
        setup: &Setup<'_>,
        params: &[Param],
        budget: Duration,
    ) -> Result<LawInstance, LawError> {
        validate::validate_setup(setup)?;
        let params = self.resolve_params(params, setup.control_channels)?;
        let wit_setup = convert::setup(setup)?;
        let wit_params = convert::params(&params);

        let mut sb = Sandbox::new(
            &self.engine,
            self.config.memory_limit_bytes,
            self.config.capture_bytes,
        );
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        sb.arm(budget);
        let bindings = self
            .pre
            .instantiate(&mut sb.store)
            .map_err(|e| sb.classify(&e))?;
        let law = bindings
            .hertzforge_control_law_random_single_law()
            .law()
            .call_constructor(&mut sb.store, &wit_setup, &wit_params)
            .map_err(|e| sb.classify(&e))?
            .map_err(LawError::Law)?;
        Ok(LawInstance {
            identity: self.identity.clone(),
            diagnostics: self.manifest.diagnostics.clone(),
            params,
            nbins: setup.target_psd.len(),
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

/// One law built for one run, with its own sandbox. Every call takes a
/// wall-clock budget; a call that traps, runs past its budget or exceeds the
/// memory cap leaves the instance unusable ([`LawError::Poisoned`] from then
/// on), and the caller holds its last good drive.
pub struct LawInstance {
    identity: LawIdentity,
    diagnostics: Vec<DiagnosticSpec>,
    params: Vec<Param>,
    nbins: usize,
    sandbox: Sandbox,
    bindings: RandomSingle,
    law: ResourceAny,
    ticker: Arc<Ticker>,
    poisoned: Option<String>,
}

impl LawInstance {
    /// Which law this is.
    pub fn identity(&self) -> &LawIdentity {
        &self.identity
    }

    /// The complete parameter values the law was built with, in manifest
    /// order (for the run record).
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Lines on the grid the law was built for.
    pub fn nbins(&self) -> usize {
        self.nbins
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
        inputs: &LoopInputs<'_>,
        budget: Duration,
    ) -> Result<LoopOutputs, LawError> {
        self.loop_call(LoopCall::Seed, inputs, budget)
    }

    /// One control loop.
    pub fn iterate(
        &mut self,
        inputs: &LoopInputs<'_>,
        budget: Duration,
    ) -> Result<LoopOutputs, LawError> {
        self.loop_call(LoopCall::Iterate, inputs, budget)
    }

    /// Tell the law the scheduled level moved, and get the drive at the new
    /// level: `level_db` is the new level in dB relative to the full target,
    /// `target_rms` the target's in-band RMS at that level. The output is
    /// checked like a loop's.
    pub fn retarget_level(
        &mut self,
        level_db: f64,
        target_rms: f64,
        budget: Duration,
    ) -> Result<LoopOutputs, LawError> {
        self.check_usable()?;
        if !level_db.is_finite() || !target_rms.is_finite() || target_rms < 0.0 {
            return Err(LawError::InputsInvalid(format!(
                "retarget to {level_db} dB at {target_rms} RMS: the level must be finite and \
                 the RMS finite and non-negative"
            )));
        }
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let r = self
            .bindings
            .hertzforge_control_law_random_single_law()
            .law()
            .call_retarget_level(&mut self.sandbox.store, self.law, level_db, target_rms);
        self.finish(r)
    }

    fn check_usable(&self) -> Result<(), LawError> {
        match &self.poisoned {
            Some(first) => Err(LawError::Poisoned(first.clone())),
            None => Ok(()),
        }
    }

    fn poison(&mut self, e: &wasmtime::Error) -> LawError {
        let err = self.sandbox.classify(e);
        self.poisoned = Some(err.to_string());
        err
    }

    fn loop_call(
        &mut self,
        which: LoopCall,
        inputs: &LoopInputs<'_>,
        budget: Duration,
    ) -> Result<LoopOutputs, LawError> {
        self.check_usable()?;
        validate::validate_inputs(inputs, self.nbins)?;
        let wit_inputs = convert::inputs(inputs)?;
        let ticker = Arc::clone(&self.ticker);
        let _active = ticker.active();
        self.sandbox.arm(budget);
        let guest = self
            .bindings
            .hertzforge_control_law_random_single_law()
            .law();
        let r = match which {
            LoopCall::Seed => guest.call_seed(&mut self.sandbox.store, self.law, &wit_inputs),
            LoopCall::Iterate => guest.call_iterate(&mut self.sandbox.store, self.law, &wit_inputs),
        };
        self.finish(r)
    }

    /// A call's result as the caller sees it: a failure of the call itself
    /// poisons the instance, the law's own error is passed on, and an output is
    /// converted and checked.
    fn finish(
        &mut self,
        r: wasmtime::Result<
            Result<crate::bindings::hertzforge::control_law::types::LoopOutputs, String>,
        >,
    ) -> Result<LoopOutputs, LawError> {
        let out = r.map_err(|e| self.poison(&e))?.map_err(LawError::Law)?;
        let out = convert::outputs(out);
        validate::validate_outputs(&out, self.nbins, &self.diagnostics)?;
        Ok(out)
    }
}
