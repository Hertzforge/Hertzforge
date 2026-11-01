//! The station backend FACTORY - the one place ANY front end constructs a backend.
//!
//! # Why this exists
//!
//! Before the factory, six module files each built their own `SimBackend`
//! (`twr.rs`, `shock.rs` twice, `scope.rs` per scope channel, `recorder.rs`,
//! `sine.rs`, `random.rs`). That is not a wiring inconvenience, it is the
//! anti-sim-leak problem in its purest form: the decision "what hardware is this
//! run on" was made independently at six sites, none of them consulted the
//! operator's station selection, and every one of them answered "the Sim" - while
//! the service crates below them hard-coded `StationClass::HertzforgeSim` into their arm
//! gates to match. A run could not have been on hardware, and nothing said so.
//!
//! Construction is now ONE function, [`try_build_station_backend`], which answers
//! three things together because they are one fact:
//!
//! * the `Box<dyn Backend>` itself,
//! * the [`StationClass`] the arm gate is given (the same `NiBackend` is NI-SIM or
//!   NI-REAL depending on what the DRIVER says about the devices behind it, so the
//!   class is never inferrable from the backend's type), and
//! * a [`ProvenanceBasis`] - the raw material for the data-category label the UI
//!   renders. This crate produces the DATA; it builds no UI.
//!
//! # Why it is a CRATE
//!
//! There is a second front end: the headless [`hertzforge-backendd`] daemon that serves a
//! remote Mac UI over `hertzforge-protocol`. A second copy of the construction decision
//! would restore exactly the defect the factory removed - two answers to "what hardware is
//! this run on", drifting apart, each able to substitute a software plant for absent
//! hardware without the other knowing. So the buildable core (station spec ->
//! `StationBackend`) lives here and BOTH front ends consume it.
//!
//! The lift is deliberately PARTIAL and mechanical. What stayed behind in
//! `ui/src-tauri/src/factory.rs` is only what is genuinely Tauri-layer state: reading
//! the operator's LIVE station selection out of the Tauri-hosted registry, merging it
//! into a [`StationSpec`], and `tracing` the result. Nothing about how a backend is
//! built moved with it, and no behavior changed - the tests moved with the code they
//! test.
//!
//! [`hertzforge-backendd`]: https://example.invalid/ "crates/hertzforge-backendd in this repo"
//!
//! # What this crate deliberately does NOT do
//!
//! * **It does not route the Explorer.** The delegated-controller path is
//!   delegated/Sim-only by architecture and the Explorer module opens and claims that
//!   device on its own arm thread. Asking the factory for that station gets a refusal naming
//!   that, not a backend.
//! * **It does not widen delegated caps or invent an NI selection.** [`NiSelection`]
//!   derives a default from enumeration when nothing is stated, and says so.
//! * **It never falls back to the Sim.** Every failure path is a typed refusal.
//!   A factory that quietly substitutes a software plant for absent hardware is
//!   the exact defect the whole phase exists to make impossible.
//! * **It does not log.** `tracing` is not a dependency here; each front end logs
//!   the [`ProvenanceBasis::log_line`] through its own subscriber. That keeps the
//!   extraction from adding a workspace dependency the workspace did not have.

#![forbid(unsafe_code)]

pub mod lock;

use hertzforge_hal::Backend;
use hertzforge_hal::DiscoveredDevice;
use hertzforge_hal_ni::config::{AiAssign, AiInterface, AoAssign, NiConfig};
use hertzforge_hal_ni::timing::TimingFamilyMap;
use hertzforge_hal_ni::{NiBackend, NiError};
use hertzforge_module_abi::StationClass;
use serde::{Deserialize, Serialize};

/// Which registry row a station selection names.
///
/// Moved here from `ui/src-tauri/src/hardware.rs` for one reason: it is the
/// factory's INPUT, so a front end that consumes the factory must be able to name a
/// kind without linking a Tauri app. The wire form (`snake_case`) is unchanged and
/// load-bearing - the TypeScript `BackendKind` union is `"sim" | "ni"`, and `hardware.rs` re-exports this type so no call site moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The HERTZFORGE-SIM physics plant.
    Sim,
    /// The NI-DAQmx station (NI-SIM or NI-REAL - the driver decides which).
    Ni,
}

/// A constructed station: the backend, the class the arm gate is given, and the
/// provenance basis for whatever labels the run.
///
/// The three travel together on purpose. Handing a module a backend without its
/// class is what let six arm gates be told "HERTZFORGE-SIM" about backends they knew
/// nothing about.
pub struct StationBackend {
    pub backend: Box<dyn Backend>,
    pub station: StationClass,
    pub provenance: ProvenanceBasis,
}

impl std::fmt::Debug for StationBackend {
    /// Hand-written because `Box<dyn Backend>` is not `Debug` (the trait is a
    /// hardware contract, not a data type). The provenance IS the useful part -
    /// it is what identifies the backend behind the box.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StationBackend")
            .field("station", &self.station)
            .field("provenance", &self.provenance)
            .finish_non_exhaustive()
    }
}

/// The raw material for a run's data-category label and provenance block.
///
/// DATA, not presentation: the UI renders it (badges, the station panel, the capture
/// sidecar). Nothing here is computed at render time, so what a report says about a
/// run is what the factory recorded when the run was built.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceBasis {
    /// The station class, rendered (`HERTZFORGE-SIM`, `NI-SIM (driver-simulated)`,
    /// `NI-REAL`). `StationClass`'s own `Display`, so the wording has one source.
    pub station_label: String,
    /// Which backend implementation is behind it (`hertzforge-hal-sim`, `hertzforge-hal-ni`).
    pub backend_kind: &'static str,
    /// Backend-opaque device ids the station is bound to. EMPTY for HERTZFORGE-SIM,
    /// because a software plant is not a device and a synthetic id would read as
    /// one.
    pub device_ids: Vec<String>,
    /// Device serial numbers, in the same order as `device_ids`. A device the
    /// driver could not give a serial for contributes an empty string rather than
    /// being dropped, so the two lists stay index-aligned.
    pub serials: Vec<String>,
    /// What the DRIVER says about whether hardware exists behind these devices.
    /// `Some(true)` = every device is driver-simulated, `Some(false)` = every
    /// device is physical, `None` = the question does not apply (HERTZFORGE-SIM: there
    /// is no driver to ask).
    ///
    /// It is All-or-None by construction: a MIXED set is refused at
    /// [`try_build_station_backend`], before a backend exists, so this field can
    /// never be a half-truth about half a rig.
    pub driver_simulated: Option<bool>,
    /// The core build this run was produced by (`hertzforge-buildinfo`'s git sha, with
    /// `-dirty` when the tree had uncommitted tracked changes at build time).
    pub core_build: &'static str,
}

impl ProvenanceBasis {
    /// The one-line log form: the data category, the backend behind it, and the
    /// devices it is bound to.
    ///
    /// This is what puts a run's category in the diagnostic log the moment the
    /// station is built, rather than only wherever the UI eventually renders it. A
    /// bug report from the field then carries the answer to "was this a
    /// measurement" without anyone having to remember.
    pub fn log_line(&self) -> String {
        let devices = if self.device_ids.is_empty() {
            "(no devices)".to_string()
        } else {
            self.device_ids
                .iter()
                .zip(self.serials.iter())
                .map(|(id, sn)| {
                    if sn.is_empty() {
                        id.clone()
                    } else {
                        format!("{id} sn:{sn}")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let simulated = match self.driver_simulated {
            Some(true) => "driver-simulated",
            Some(false) => "physical",
            None => "n/a (no driver)",
        };
        format!(
            "station {} via {} [{}] devices: {} | core {}",
            self.station_label, self.backend_kind, simulated, devices, self.core_build
        )
    }
}

/// The core build sha, as a `&'static str` for [`ProvenanceBasis::core_build`].
///
/// `hertzforge_buildinfo::GIT_SHA` is already `&'static`; the dirty flag is folded in
/// here so a run built from a dirty tree cannot be mistaken for one built from the
/// named commit.
fn core_build() -> &'static str {
    if hertzforge_buildinfo::GIT_DIRTY {
        // Two `&'static str`s cannot be concatenated at runtime, so the dirty case
        // is a distinct literal-shaped value. `Box::leak` on a per-process constant
        // is bounded (one allocation for the life of the process) and keeps the
        // field `&'static` for callers that embed it in `'static` structures.
        static DIRTY: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        DIRTY.get_or_init(|| {
            Box::leak(format!("{}-dirty", hertzforge_buildinfo::GIT_SHA).into_boxed_str())
        })
    } else {
        hertzforge_buildinfo::GIT_SHA
    }
}

// =====================================================================================
// The request
// =====================================================================================

/// The HERTZFORGE-SIM plant to build when the station is the Sim, as backend-neutral
/// data.
///
/// Every module has its own: TWR wants a three-mode resonant plant, the shock
/// modules want pure unity feedthrough, Random wants the demo plant its acceptance
/// gates were validated against, the scope wants a per-channel resonance bank.
/// Those numbers used to live inline at each module's `SimConfig::new()...build()`
/// site; they live at the CALL to this factory now, and the factory is the only
/// thing that turns them into a `SimBackend`.
///
/// Hertzforge-Sim is FIRST CLASS and stays selectable forever: the Sim is where the
/// control-law truth lives, and a hardware station cannot replace it.
#[derive(Debug, Clone)]
pub struct SimPlantSpec {
    pub sample_rate_hz: f64,
    /// `(f_n, Q)` per fixture mode. Empty = a pure-feedthrough unity plant.
    pub resonances: Vec<(f64, f64)>,
    pub noise_level: f64,
    pub feedthrough: f64,
    pub transport_delay_samples: usize,
    pub seed: u64,
    pub max_input_channels: u32,
    pub max_output_channels: u32,
    /// A software plant IS shared-clock by construction (input and output advance
    /// on the same synthetic sample index), and every closed-loop caller states
    /// `true`. It is a field rather than a constant because `SimConfig`'s own
    /// default is the conservative `false` (never claim a clock you have
    /// not proven), and a caller must make the claim deliberately.
    pub shared_clock: bool,
}

impl Default for SimPlantSpec {
    /// Field-for-field `SimConfig::default()`.
    ///
    /// That is deliberate and load-bearing: the six module call sites were written
    /// as `SimConfig::new().<only the fields they cared about>.build()`, so every
    /// field they did NOT name took the Sim's default. Mirroring the defaults here
    /// lets each site keep naming exactly the fields it named before, with
    /// `..SimPlantSpec::default()` standing in for the rest - which is what makes
    /// "this move changes construction, not semantics" checkable by reading the
    /// diff instead of trusting it.
    fn default() -> Self {
        SimPlantSpec {
            sample_rate_hz: hertzforge_hal_sim::DEFAULT_SAMPLE_RATE_HZ,
            resonances: vec![(
                hertzforge_hal_sim::DEFAULT_RESONANCE_HZ,
                hertzforge_hal_sim::DEFAULT_Q,
            )],
            noise_level: 1e-4,
            feedthrough: 1e-3,
            transport_delay_samples: 0,
            seed: hertzforge_hal_sim::DEFAULT_SEED,
            max_input_channels: 8,
            max_output_channels: 2,
            shared_clock: false,
        }
    }
}

/// Which NI devices and physical channels a station binds to.
///
/// The front end fills this from the OPERATOR'S persisted selection: `restrict_devices` is
/// the device set they chose in the Hardware panel, and `inputs`/`outputs` come
/// from the channel table's `physical` column (schema v2) when its rows are
/// bound. Deriving is what happens when neither is stated, and it is still a
/// DEVELOPMENT convenience written down as one - it is not a claim that the
/// first device is the right device.
#[derive(Debug, Clone)]
pub struct NiSelection {
    /// Explicit physical channel names (`cDAQ1Mod1/ai0`). Empty = derive.
    pub inputs: Vec<String>,
    /// Explicit physical drive channels. Empty = derive.
    pub outputs: Vec<String>,
    /// How many AI channels to take when deriving.
    pub derive_inputs: usize,
    /// How many AO channels to take when deriving.
    pub derive_outputs: usize,
    /// IEPE or plain voltage front end for the derived input rows. Replaced per-row
    /// by `Backend::configure` from the channel table's `interface`; this is only
    /// what the task is built with before the first configure.
    pub iepe: bool,
    /// The device ids the OPERATOR bound this station to. Empty = every device
    /// the scan found is fair game (the pre-selection case).
    ///
    /// Non-empty and this is a HARD filter, not a preference: a device outside
    /// the set is invisible to the derivation, and a bound device that is not in
    /// the scan is a refusal ([`StationBuildError::SelectedDevicesAbsent`]).
    /// Falling back to "some other device that happens to be there" is exactly
    /// the substitution the whole phase exists to make impossible.
    pub restrict_devices: Vec<String>,
}

impl Default for NiSelection {
    fn default() -> Self {
        NiSelection {
            inputs: Vec::new(),
            outputs: Vec::new(),
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            restrict_devices: Vec::new(),
        }
    }
}

/// Everything the factory needs beyond the [`BackendKind`].
///
/// One struct with a per-kind half, rather than an enum, because the CALLER does
/// not know which kind it will get - the kind comes from the operator's station
/// selection - and every module must be able to describe itself on every station
/// without branching at the call site.
#[derive(Debug, Clone, Default)]
pub struct StationSpec {
    /// Consulted only when the resolved station is HERTZFORGE-SIM.
    pub sim: SimPlantSpec,
    /// Consulted only when the kind is [`BackendKind::Ni`].
    pub ni: NiSelection,
    /// The rate + block the NI task is BUILT with. `Backend::configure` replaces
    /// both at arm; this is what the tasks exist as until then.
    pub ni_block_frames: usize,
}

// =====================================================================================
// Refusals
// =====================================================================================

/// Why a station could not be built. Typed so a test can assert the CAUSE rather
/// than a substring, and so no arm can be quietly turned into a Sim fallback
/// without a test going red.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StationBuildError {
    /// The NI-DAQmx runtime is not installed on this host. Normal and expected on
    /// a developer Mac.
    NiDriverUnavailable(String),
    /// The scan succeeded and found nothing usable.
    NoNiDevices,
    /// The scan found devices but not enough channels for the request.
    NotEnoughNiChannels {
        want_inputs: usize,
        want_outputs: usize,
        have_inputs: usize,
        have_outputs: usize,
    },
    /// Some selected devices are driver-simulated and some are physical. Refused
    /// HERE, before a backend exists, so `driver_simulated` can never describe half
    /// a rig - and so the operator is told at selection time rather than at
    /// configure (where the backend's own mixed-set refusal would also catch it).
    MixedSimulatedAndReal {
        simulated: Vec<String>,
        real: Vec<String>,
    },
    /// The station is bound to devices the scan does not report. The operator
    /// chose these; something else being present is not a substitute.
    SelectedDevicesAbsent {
        missing: Vec<String>,
        found: Vec<String>,
    },
    /// The bound channels span more than one DAQmx timing family (a cDAQ chassis
    /// and a standalone card, or two chassis). Refused HERE rather than at arm:
    /// the driver's own refusal (-201425) is honest but arrives three screens
    /// after the binding mistake. See `hertzforge_hal_ni::timing`.
    MixedTimingFamilies(String),
    /// The NI backend refused its own construction.
    Ni(String),
}

impl std::fmt::Display for StationBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StationBuildError::NiDriverUnavailable(d) => write!(
                f,
                "the NI-DAQmx runtime is not available on this host, so no NI station can be \
                 built: {d}. There is no fallback: a synthetic plant presented as an NI \
                 station would be indistinguishable from a measurement. Select the Hertzforge-Sim \
                 station to run against the physics plant, or install the NI-DAQmx runtime."
            ),
            StationBuildError::NoNiDevices => write!(
                f,
                "the NI-DAQmx scan completed and reported no devices. Check that the chassis \
                 is powered and connected, then rescan."
            ),
            StationBuildError::NotEnoughNiChannels {
                want_inputs,
                want_outputs,
                have_inputs,
                have_outputs,
            } => write!(
                f,
                "the NI devices found expose {have_inputs} input and {have_outputs} output \
                 channel(s); this station needs {want_inputs} and {want_outputs}. Channels are \
                 never silently shared or reused."
            ),
            StationBuildError::MixedSimulatedAndReal { simulated, real } => write!(
                f,
                "refusing a MIXED device set: {} driver-simulated ({}) and {} physical ({}). \
                 A run spanning both has no honest data category - half of it is a measurement \
                 and half is not - so the set is refused rather than labeled.",
                simulated.len(),
                simulated.join(", "),
                real.len(),
                real.join(", ")
            ),
            StationBuildError::SelectedDevicesAbsent { missing, found } => write!(
                f,
                "previously selected device not present: {}. The scan reports {}. This station \
                 REFUSES to run: the operator bound it to specific devices, and a different \
                 device that happens to be attached is not a substitute for the one that was \
                 selected. Reattach the device, or select a new device set in Hardware.",
                missing.join(", "),
                if found.is_empty() {
                    "no devices".to_string()
                } else {
                    found.join(", ")
                }
            ),
            StationBuildError::MixedTimingFamilies(m) => write!(
                f,
                "refusing a set that cannot share one DAQmx timing method: {m} Rebind so every \
                 channel in the run sits in one family, or select a device set that does."
            ),
            StationBuildError::Ni(m) => write!(f, "the NI backend refused construction: {m}"),
        }
    }
}
// =====================================================================================
// Discovery, for front ends that may not name a backend crate
// =====================================================================================

/// The NI-DAQmx scan, re-exposed.
///
/// `hertzforge-hal-ni` is BANNED outside this crate's whitelist (deny.toml + the sim-leak
/// lint), and that ban is what keeps a second front end from growing its own station
/// construction. But a front end still legitimately needs to SEE the devices - to
/// render a scan list, and to run a preflight before it claims anything. So the scan
/// comes out through the same door the construction does.
///
/// This is a pass-through: no filtering, no sorting, no inference. Devices arrive in
/// the driver's own order, with `simulated` asserted by the driver and never guessed.
pub fn scan_ni() -> Result<Vec<DiscoveredDevice>, StationBuildError> {
    match hertzforge_hal_ni::enumerate() {
        Ok(d) => Ok(d),
        Err(NiError::DriverNotAvailable { tried, detail }) => {
            Err(StationBuildError::NiDriverUnavailable(format!(
                "tried {tried}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("; {detail}")
                }
            )))
        }
        Err(e) => Err(StationBuildError::Ni(e.to_string())),
    }
}

// =====================================================================================
// The factory
// =====================================================================================
/// Build the station backend for `kind`, with the refusal TYPED.
///
/// THE one construction point, for every front end. Each front end wraps this with
/// whatever it owes its own layer - the Tauri app's `factory::build_station_backend`
/// adds a `tracing` line and stringifies the error for the command surface; the
/// daemon logs through its own subscriber and maps the error onto a
/// `hertzforge_protocol::ProtocolError`. Neither wrapper decides anything.
pub fn try_build_station_backend(
    kind: BackendKind,
    spec: &StationSpec,
) -> Result<StationBackend, StationBuildError> {
    match kind {
        BackendKind::Sim => Ok(build_sim(&spec.sim)),
        BackendKind::Ni => build_ni(
            hertzforge_hal_ni::enumerate(),
            &spec.ni,
            spec.ni_block_frames,
        ),
    }
}

// =====================================================================================
// Liveness: how many station backends exist right now
// =====================================================================================

/// Station backends currently ALIVE in this process.
///
/// Incremented when the factory hands one out and decremented when it drops, by
/// [`TrackedBackend`]. This is the signal the sim-rig manager consults before
/// mutating the DAQmx configuration: creating or deleting a simulated device
/// while a task holds a handle to it is how a driver-level fault becomes a run
/// that ends in the middle.
///
/// It counts CONSTRUCTED stations, not armed ones, and that is deliberately
/// conservative: a configured-but-not-started station still holds device
/// handles, and the cost of refusing a rig edit is a second's wait while the
/// cost of allowing one is a corrupted run.
static OPEN_STATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many station backends are alive right now.
pub fn open_station_count() -> usize {
    OPEN_STATIONS.load(std::sync::atomic::Ordering::SeqCst)
}

/// A `Backend` that counts itself.
///
/// Pure delegation plus a `Drop`. It exists because "is a station open" cannot
/// be answered by asking the modules: the backend is MOVED into a service at
/// configure and nothing tells the factory when that service goes away. A
/// counting wrapper answers it automatically and cannot drift, because the
/// answer is a consequence of the value's lifetime rather than a flag someone
/// remembers to clear.
struct TrackedBackend {
    inner: Box<dyn Backend>,
}

impl TrackedBackend {
    fn wrap(inner: Box<dyn Backend>) -> Box<dyn Backend> {
        OPEN_STATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::new(TrackedBackend { inner })
    }
}

impl Drop for TrackedBackend {
    fn drop(&mut self) {
        OPEN_STATIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Backend for TrackedBackend {
    fn caps(&self) -> hertzforge_hal::Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &hertzforge_channel_table::ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.emergency_stop()
    }
}

/// The HERTZFORGE-SIM station. Infallible: a software plant has nothing to fail at
/// construction, and its `configure` is where a bad table is caught.
fn build_sim(plant: &SimPlantSpec) -> StationBackend {
    let backend = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(plant.sample_rate_hz)
        .resonances(plant.resonances.clone())
        .noise_level(plant.noise_level)
        .feedthrough(plant.feedthrough)
        .transport_delay_samples(plant.transport_delay_samples)
        .shared_clock(plant.shared_clock)
        .seed(plant.seed)
        .max_input_channels(plant.max_input_channels.max(1))
        .max_output_channels(plant.max_output_channels.max(1))
        .build();
    StationBackend {
        backend: TrackedBackend::wrap(Box::new(backend)),
        station: StationClass::HertzforgeSim,
        provenance: ProvenanceBasis {
            station_label: StationClass::HertzforgeSim.to_string(),
            backend_kind: "hertzforge-hal-sim",
            // A software plant is not a device. An invented id here would read as
            // one in a report.
            device_ids: Vec::new(),
            serials: Vec::new(),
            // There is no driver to ask, which is a different answer from "no".
            driver_simulated: None,
            core_build: core_build(),
        },
    }
}

/// The NI station, from an ALREADY-PERFORMED scan.
///
/// The scan result is a parameter rather than something this function performs so
/// the whole classification - driver-absent, empty, short on channels, mixed,
/// NI-SIM, NI-REAL - is testable on a host with no NI runtime. A refusal that can
/// only be produced on one machine is not a refusal anyone can check.
fn build_ni(
    scan: Result<Vec<DiscoveredDevice>, NiError>,
    sel: &NiSelection,
    block_frames: usize,
) -> Result<StationBackend, StationBuildError> {
    let all = match scan {
        Ok(d) => d,
        Err(NiError::DriverNotAvailable { tried, detail }) => {
            return Err(StationBuildError::NiDriverUnavailable(format!(
                "tried {tried}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("; {detail}")
                }
            )))
        }
        Err(e) => return Err(StationBuildError::Ni(e.to_string())),
    };
    if all.is_empty() {
        return Err(StationBuildError::NoNiDevices);
    }

    // THE OPERATOR'S DEVICE SET. When one is bound, every device the operator did
    // NOT choose becomes invisible here - and a chosen device the scan does not
    // report is a refusal, not a prompt to use whatever else is attached. This is
    // the arm-time half of the absent-device state the Hardware panel renders;
    // the panel's check is advisory, this one is what stops the run.
    let devices: Vec<DiscoveredDevice> = if sel.restrict_devices.is_empty() {
        all
    } else {
        let missing: Vec<String> = sel
            .restrict_devices
            .iter()
            .filter(|want| !all.iter().any(|d| &&d.id == want))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(StationBuildError::SelectedDevicesAbsent {
                missing,
                found: all.iter().map(|d| d.id.clone()).collect(),
            });
        }
        // Keep the restricted devices AND their parent (chassis) rows. The parent
        // row is the timing-family EVIDENCE: `TimingFamilyMap::from_devices`
        // treats a module whose parent is absent from the list as a standalone
        // island (deliberately - an unproven family must not be invented), so
        // filtering the chassis out here made two sibling cDAQ modules read as
        // two islands and falsely refused MixedTimingFamilies. Found live by the
        // first cross-host wire probe, 2026-07-25.
        let parents: Vec<String> = all
            .iter()
            .filter(|d| sel.restrict_devices.iter().any(|want| want == &d.id))
            .filter_map(|d| d.parent.clone())
            .collect();
        all.into_iter()
            .filter(|d| {
                sel.restrict_devices.iter().any(|want| want == &d.id)
                    || parents.iter().any(|p| p == &d.id)
            })
            .collect()
    };
    if devices.is_empty() {
        return Err(StationBuildError::NoNiDevices);
    }

    // Resolve the physical channels: the channel table's explicit bindings where
    // it has them, derived from the scan where it does not.
    //
    // PER DIRECTION, not all-or-nothing. A table can legitimately bind its input
    // rows while the drive row is still unbound (the operator is halfway through
    // the grid's per-row dropdown), and treating that as "explicit, therefore no
    // drive channel" would build a task with no output and fail one layer down
    // with a message about the wrong thing.
    let (derived_in, derived_out) = derive_channels(
        &devices,
        // Only the UNBOUND direction is derived - and only the unbound direction
        // can fail the channel-count check, so a fully bound table is never
        // refused for a scan that is short of channels it does not use.
        if sel.inputs.is_empty() {
            sel.derive_inputs.max(1)
        } else {
            0
        },
        if sel.outputs.is_empty() {
            sel.derive_outputs
        } else {
            0
        },
    )?;
    let inputs = if sel.inputs.is_empty() {
        derived_in
    } else {
        sel.inputs.clone()
    };
    let outputs = if sel.outputs.is_empty() {
        derived_out
    } else {
        sel.outputs.clone()
    };
    if inputs.is_empty() {
        return Err(StationBuildError::NotEnoughNiChannels {
            want_inputs: sel.derive_inputs.max(1),
            want_outputs: sel.derive_outputs,
            have_inputs: 0,
            have_outputs: outputs.len(),
        });
    }

    // Which devices did the selection actually touch? Only those decide the class
    // and the provenance - a chassis full of modules the run does not read has no
    // bearing on whether THIS run is a measurement.
    let bound: Vec<&DiscoveredDevice> = devices
        .iter()
        .filter(|d| {
            let owns = |p: &String| p.split('/').next().map(|dev| dev == d.id).unwrap_or(false);
            inputs.iter().any(owns) || outputs.iter().any(owns)
        })
        .collect();
    if bound.is_empty() {
        return Err(StationBuildError::NoNiDevices);
    }

    // THE CLASS. Asserted by the driver, never by a name or a human.
    let simulated: Vec<String> = bound
        .iter()
        .filter(|d| d.simulated)
        .map(|d| d.id.clone())
        .collect();
    let real: Vec<String> = bound
        .iter()
        .filter(|d| !d.simulated)
        .map(|d| d.id.clone())
        .collect();
    let station = match (simulated.is_empty(), real.is_empty()) {
        (false, true) => StationClass::NiSimulated,
        (true, false) => StationClass::NiReal,
        _ => return Err(StationBuildError::MixedSimulatedAndReal { simulated, real }),
    };

    let cfg = NiConfig::new(
        inputs
            .iter()
            .enumerate()
            .map(|(i, p)| AiAssign {
                physical: p.clone(),
                interface: if sel.iepe {
                    AiInterface::Iepe
                } else {
                    AiInterface::Voltage
                },
                // Placeholders that ARE validated by the driver: since the
                // commit-at-configure fix, `open()` builds AND COMMITS the tasks,
                // so the driver checks these ranges immediately - the original
                // comment here claimed they were "replaced before a single sample
                // is read" and the 9234 refuted it live (-200077: the +/-10 V
                // placeholder exceeds its +/-5 V input; 2026-07-25, first GUI
                // milestone run). `Backend::configure` still replaces every
                // assignment from the channel table, but the placeholder must be
                // LEGAL ON ANY DEVICE in the meantime: +/-1 V is inside every
                // supported input range (a smaller-than-minimum request is coerced
                // UP by the driver, which is harmless for a placeholder).
                // Non-zero because `NiConfig::validate` rejects a zero
                // sensitivity as a division hazard.
                sensitivity_mv_per_eu: 1000.0,
                max_eu: 1.0,
                table_index: Some(i as u32 + 1),
            })
            .collect(),
        outputs
            .iter()
            .map(|p| AoAssign {
                physical: p.clone(),
                table_index: None,
            })
            .collect(),
    );
    cfg.validate()
        .map_err(|e| StationBuildError::Ni(e.to_string()))?;
    // A set spanning a cDAQ chassis AND a standalone card
    // has no shared timing method, and the driver only says so at commit/start
    // (-201425). `devices` is the scan this station was built from, so the family
    // grouping is a DRIVER fact here, not an inference from device names.
    cfg.validate_timing(&TimingFamilyMap::from_devices(&devices))
        .map_err(|e| StationBuildError::MixedTimingFamilies(e.to_string()))?;

    // The rate is a placeholder too: `configure` states the request and the driver
    // answers with what it programmed (the acquisition seam). The task is built
    // at [`NI_BUILD_RATE_HZ`] so `open()` is legal before the first configure.
    let acq = hertzforge_hal::AcquisitionConfig::new(NI_BUILD_RATE_HZ, block_frames.max(1));

    let device_ids: Vec<String> = bound.iter().map(|d| d.id.clone()).collect();
    let serials: Vec<String> = bound.iter().map(|d| d.serial.clone()).collect();
    Ok(StationBackend {
        backend: TrackedBackend::wrap(Box::new(NiBackend::new(cfg, acq))),
        station,
        provenance: ProvenanceBasis {
            station_label: station.to_string(),
            backend_kind: "hertzforge-hal-ni",
            device_ids,
            serials,
            // All-or-None: the mixed case was refused above.
            driver_simulated: Some(station == StationClass::NiSimulated),
            core_build: core_build(),
        },
    })
}

/// The rate an NI task is BUILT with, before any `configure`.
///
/// 51.2 kHz is the NI-9234 delta-sigma class's native rate, so `open()` finds a
/// legal timing configuration on the dev rig. It is NOT the rate a run uses: the
/// acquisition seam has every module state its request at `configure` and read
/// back what the driver actually programmed. `pub` so the registry row reports
/// the same number rather than a second one written in the UI.
pub const NI_BUILD_RATE_HZ: f64 = 51_200.0;

/// Take the first `want_in` AI and `want_out` AO physical channels the scan
/// reported, in the driver's own device order. `devices` is already restricted to
/// the operator's chosen set, so a derivation can never wander onto a device they
/// did not select. A want of 0 derives nothing and cannot fail.
fn derive_channels(
    devices: &[DiscoveredDevice],
    want_in: usize,
    want_out: usize,
) -> Result<(Vec<String>, Vec<String>), StationBuildError> {
    let all_in: Vec<String> = devices
        .iter()
        .flat_map(|d| d.inputs.iter().map(|c| c.id.clone()))
        .collect();
    let all_out: Vec<String> = devices
        .iter()
        .flat_map(|d| d.outputs.iter().map(|c| c.id.clone()))
        .collect();
    if all_in.len() < want_in || all_out.len() < want_out {
        return Err(StationBuildError::NotEnoughNiChannels {
            want_inputs: want_in,
            want_outputs: want_out,
            have_inputs: all_in.len(),
            have_outputs: all_out.len(),
        });
    }
    Ok((all_in[..want_in].to_vec(), all_out[..want_out].to_vec()))
}

/// The physical channels a channel table BINDS, split by direction.
///
/// Only ENABLED rows contribute: a disabled row is not acquired, and letting it
/// hold a physical channel would size the task for channels nothing reads.
///
/// Returns two EMPTY lists when the table binds nothing, which is the correct
/// answer for a table authored on the Sim station (where there are no physical
/// channels to bind) and for a freshly migrated v1 table. The caller then
/// derives, and the derivation is confined to the operator's device set.
///
/// A PARTIALLY bound table contributes only what it binds. That is deliberate:
/// the alternative - refusing unless every row is bound - would make the grid's
/// per-row dropdown unusable while the operator is halfway through filling it,
/// and the channel-count refusal one layer down already catches a task that
/// cannot be built.
pub fn physical_from_table(
    table: &hertzforge_channel_table::ChannelTable,
) -> (Vec<String>, Vec<String>) {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for ch in table.channels.iter().filter(|c| c.enabled) {
        let Some(p) = ch.physical.as_ref() else {
            continue;
        };
        if ch.role == hertzforge_channel_table::ChannelRole::Drive {
            outputs.push(p.resolved());
        } else {
            inputs.push(p.resolved());
        }
    }
    (inputs, outputs)
}
#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_hal::DiscoveredChannel;

    fn dev(id: &str, serial: &str, simulated: bool, n_in: usize, n_out: usize) -> DiscoveredDevice {
        let mut d = DiscoveredDevice::new(id, "NI-9234");
        d.serial = serial.into();
        d.simulated = simulated;
        d.inputs = (0..n_in)
            .map(|i| DiscoveredChannel::new(format!("{id}/ai{i}"), format!("ai{i}")))
            .collect();
        d.outputs = (0..n_out)
            .map(|i| DiscoveredChannel::new(format!("{id}/ao{i}"), format!("ao{i}")))
            .collect();
        d
    }

    // -------------------------------------------------------------------------
    // The Sim station
    // -------------------------------------------------------------------------

    /// HERTZFORGE-SIM yields the HertzforgeSim class and NO device ids: a software plant is
    /// not a device, and a synthetic id in the provenance block would read as one
    /// in a report.
    ///
    /// RED-MUTATION: give `build_sim`'s provenance a placeholder device id (say
    /// `vec!["hertzforge-sim".into()]`) and this test reds.
    #[test]
    fn a_sim_station_is_hertzforge_sim_with_no_devices() {
        let st = try_build_station_backend(BackendKind::Sim, &StationSpec::default())
            .expect("the Sim station always builds");
        assert_eq!(st.station, StationClass::HertzforgeSim);
        assert!(
            st.provenance.device_ids.is_empty(),
            "a plant is not a device"
        );
        assert!(st.provenance.serials.is_empty());
        assert_eq!(
            st.provenance.driver_simulated, None,
            "there is no driver to ask; None is a different answer from Some(false)"
        );
        assert_eq!(st.provenance.backend_kind, "hertzforge-hal-sim");
        assert_eq!(st.provenance.station_label, "HERTZFORGE-SIM");
    }

    /// The plant spec reaches the backend: what the caller asked for is what caps
    /// reports.
    #[test]
    fn the_sim_plant_spec_reaches_the_backend() {
        let spec = StationSpec {
            sim: SimPlantSpec {
                sample_rate_hz: 51_200.0,
                max_input_channels: 4,
                max_output_channels: 2,
                shared_clock: true,
                ..SimPlantSpec::default()
            },
            ..StationSpec::default()
        };
        let st = try_build_station_backend(BackendKind::Sim, &spec).expect("builds");
        let caps = st.backend.caps();
        assert_eq!(caps.max_input_channels, 4);
        assert_eq!(caps.max_output_channels, 2);
        assert_eq!(caps.sample_rate_hz, 51_200.0);
        assert_eq!(caps.clock, hertzforge_hal::ClockDomain::Shared);
    }

    /// `SimPlantSpec::default()` is field-for-field `SimConfig::default()`, INCLUDING
    /// the conservative `shared_clock: false`.
    ///
    /// That equivalence is what makes the six module call sites' `..Default::default()`
    /// a faithful stand-in for the fields their inline `SimConfig::new()...build()`
    /// chains never named - i.e. it is what makes "this move changes construction, not
    /// semantics" true rather than asserted.
    ///
    /// RED-MUTATION: flip any field of `SimPlantSpec::default()` away from the Sim's
    /// own default (say `shared_clock: true`, which reads as harmless) and this reds.
    #[test]
    fn the_default_plant_spec_is_the_sims_own_default() {
        let via_spec = try_build_station_backend(BackendKind::Sim, &StationSpec::default())
            .expect("builds")
            .backend
            .caps();
        let direct = hertzforge_hal_sim::SimConfig::default().build().caps();
        assert_eq!(
            via_spec, direct,
            "the factory default must be the Sim default"
        );
        assert_eq!(
            direct.clock,
            hertzforge_hal::ClockDomain::Unknown,
            "the Sim's own posture is UNPROVEN until a caller claims otherwise"
        );
    }

    // -------------------------------------------------------------------------
    // Provenance
    // -------------------------------------------------------------------------

    /// Every station carries the core build sha. A provenance block that cannot say
    /// which build produced a run cannot settle an argument about that run later.
    ///
    /// RED-MUTATION: return `""` from `core_build()` and this reds.
    #[test]
    fn the_provenance_basis_carries_the_core_build_sha() {
        let sim = try_build_station_backend(BackendKind::Sim, &StationSpec::default()).unwrap();
        assert!(!sim.provenance.core_build.is_empty());
        assert!(
            sim.provenance
                .core_build
                .starts_with(hertzforge_buildinfo::GIT_SHA),
            "the stamp must be the buildinfo sha ({}), got {}",
            hertzforge_buildinfo::GIT_SHA,
            sim.provenance.core_build
        );
        if hertzforge_buildinfo::GIT_DIRTY {
            assert!(
                sim.provenance.core_build.ends_with("-dirty"),
                "a dirty tree must not be reported as the named commit"
            );
        }

        let ni = build_ni(
            Ok(vec![dev("cDAQ9178Sim1Mod1", "01ABCDEF", true, 4, 2)]),
            &NiSelection::default(),
            1024,
        )
        .expect("a driver-simulated device set builds");
        assert_eq!(ni.provenance.core_build, sim.provenance.core_build);
    }

    // -------------------------------------------------------------------------
    // The NI station: the refusals
    // -------------------------------------------------------------------------

    /// An absent NI-DAQmx runtime is a TYPED refusal, never a Sim.
    ///
    /// RED-MUTATION: make the `NiError::DriverNotAvailable` arm of `build_ni`
    /// return `Ok(build_sim(&SimPlantSpec::default()))` and this test reds twice
    /// over - the `assert!(matches!(...))` fails, and so does the
    /// no-HertzforgeSim-from-an-NI-request assertion below it. Run and reverted.
    #[test]
    fn an_absent_ni_driver_refuses_and_never_yields_a_sim() {
        let err = build_ni(
            Err(NiError::DriverNotAvailable {
                tried: "libnidaqmx.so".into(),
                detail: "cannot open shared object file".into(),
            }),
            &NiSelection::default(),
            1024,
        )
        .map(|st| st.station)
        .expect_err("a missing driver cannot produce a station");
        assert!(
            matches!(err, StationBuildError::NiDriverUnavailable(ref d) if d.contains("libnidaqmx")),
            "the loader diagnosis must survive the hop: {err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("no fallback") || text.contains("There is no fallback"),
            "the refusal must say a fallback does not exist: {text}"
        );
    }

    /// A request for an NI station can NEVER come back as HERTZFORGE-SIM, on any input.
    /// This is the invariant the whole firewall protects, stated once as a test
    /// over every refusal path plus both success paths.
    #[test]
    fn no_ni_request_ever_yields_an_hertzforge_sim_station() {
        let scans: Vec<Result<Vec<DiscoveredDevice>, NiError>> = vec![
            Err(NiError::DriverNotAvailable {
                tried: "libnidaqmx.so".into(),
                detail: String::new(),
            }),
            Ok(vec![]),
            Ok(vec![dev("cDAQ1Mod1", "0A", true, 0, 0)]),
            Ok(vec![dev("cDAQ1Mod1", "0A", true, 4, 2)]),
            Ok(vec![dev("cDAQ1Mod1", "0A", false, 4, 2)]),
            Ok(vec![
                dev("cDAQ1Mod1", "0A", true, 4, 0),
                dev("cDAQ1Mod2", "0B", false, 0, 2),
            ]),
        ];
        for scan in scans {
            if let Ok(st) = build_ni(scan, &NiSelection::default(), 1024) {
                assert_ne!(
                    st.station,
                    StationClass::HertzforgeSim,
                    "an NI request produced an HERTZFORGE-SIM station"
                );
                assert_eq!(st.provenance.backend_kind, "hertzforge-hal-ni");
            }
        }
    }

    /// An empty scan is a refusal, not an empty task.
    #[test]
    fn an_empty_scan_refuses() {
        let err = build_ni(Ok(Vec::new()), &NiSelection::default(), 1024).unwrap_err();
        assert_eq!(err, StationBuildError::NoNiDevices);
    }

    /// Too few channels is a refusal, never a truncated task. (`plan_from_table`
    /// makes the same promise one layer down; this is the selection-time half.)
    #[test]
    fn too_few_channels_refuses_rather_than_truncating() {
        let err = build_ni(
            Ok(vec![dev("cDAQ1Mod1", "0A", true, 1, 0)]),
            &NiSelection {
                derive_inputs: 4,
                derive_outputs: 1,
                ..NiSelection::default()
            },
            1024,
        )
        .unwrap_err();
        assert_eq!(
            err,
            StationBuildError::NotEnoughNiChannels {
                want_inputs: 4,
                want_outputs: 1,
                have_inputs: 1,
                have_outputs: 0,
            }
        );
    }

    /// A set spanning driver-simulated AND physical devices is refused at
    /// SELECTION time, so `driver_simulated` is All-or-None by construction and can
    /// never describe half a rig.
    ///
    /// RED-MUTATION: replace the mixed arm with
    /// `StationClass::NiSimulated` (i.e. "if any device is simulated, call the
    /// whole thing simulated") and this reds.
    #[test]
    fn a_mixed_simulated_and_real_set_is_refused() {
        let err = build_ni(
            Ok(vec![
                dev("cDAQ1Mod1", "0A", true, 4, 0),
                dev("cDAQ1Mod2", "0B", false, 0, 2),
            ]),
            &NiSelection::default(),
            1024,
        )
        .unwrap_err();
        match err {
            StationBuildError::MixedSimulatedAndReal { simulated, real } => {
                assert_eq!(simulated, vec!["cDAQ1Mod1".to_string()]);
                assert_eq!(real, vec!["cDAQ1Mod2".to_string()]);
            }
            other => panic!("expected the mixed-set refusal, got {other:?}"),
        }
    }

    /// MILESTONE FINDING 1, at the factory. The bulk-assign that produced DAQmx
    /// -201425 on the box (rows across the cDAQ chassis AND the PCIe card) is
    /// refused when the station is built, with both families named - not at arm,
    /// three screens later.
    ///
    /// RED-MUTATION: delete the `cfg.validate_timing(...)` call in `build_ni` and
    /// this reds; the station builds and the driver's refusal moves back to arm.
    #[test]
    fn a_set_spanning_two_timing_families_is_refused_at_station_build() {
        let mut chassis = dev("cDAQ1", "0A", true, 0, 0);
        chassis.product = "cDAQ-9178".into();
        let mut ai = dev("cDAQ1Mod1", "0B", true, 4, 0);
        ai.parent = Some("cDAQ1".into());
        let mut ao = dev("cDAQ1Mod2", "0C", true, 0, 2);
        ao.parent = Some("cDAQ1".into());
        let card = dev("PCIe6363", "0D", true, 16, 2);

        let scan = vec![chassis, ai, ao, card];
        let err = build_ni(
            Ok(scan.clone()),
            &NiSelection {
                // Exactly what "Assign in Scan Order" produced on the box.
                inputs: vec!["cDAQ1Mod1/ai0".into(), "PCIe6363/ai0".into()],
                outputs: vec!["cDAQ1Mod2/ao0".into()],
                ..NiSelection::default()
            },
            1024,
        )
        .unwrap_err();
        match &err {
            StationBuildError::MixedTimingFamilies(m) => {
                assert!(m.contains("cDAQ1"), "names the chassis: {m}");
                assert!(m.contains("PCIe6363"), "names the card: {m}");
                assert!(m.contains("-201425"), "cites the driver status: {m}");
            }
            other => panic!("expected the timing-family refusal, got {other:?}"),
        }
        assert!(
            err.to_string().contains("Rebind"),
            "the refusal must say what to do: {err}"
        );

        // ANTI-VACUITY: the same scan, bound entirely inside the chassis, builds.
        // Without this half the test would pass just as well if `build_ni`
        // refused everything.
        let ok = build_ni(
            Ok(scan),
            &NiSelection {
                inputs: vec!["cDAQ1Mod1/ai0".into(), "cDAQ1Mod1/ai1".into()],
                outputs: vec!["cDAQ1Mod2/ao0".into()],
                ..NiSelection::default()
            },
            1024,
        );
        assert!(
            ok.is_ok(),
            "one chassis, AI and AO, must still build: {:?}",
            ok.err()
        );
    }

    /// FOUND BY THE FIRST CROSS-HOST WIRE PROBE (2026-07-25): the daemon's
    /// `restrict_devices` list names the MODULES, not the chassis - and the
    /// restrict filter dropped the chassis row, so `TimingFamilyMap` saw two
    /// parent-less islands and falsely refused MixedTimingFamilies for two
    /// SIBLING modules of one chassis. The filter must retain the parent rows
    /// of retained modules: the chassis row IS the family evidence.
    ///
    /// RED-MUTATION: remove the `parents` retention from the restrict filter in
    /// `build_ni` and this reds with the false MixedTimingFamilies.
    #[test]
    fn restricting_to_sibling_modules_keeps_their_chassis_as_one_family() {
        let mut chassis = dev("cDAQ1", "0A", true, 0, 0);
        chassis.product = "cDAQ-9178".into();
        let mut ai = dev("cDAQ1Mod1", "0B", true, 4, 0);
        ai.parent = Some("cDAQ1".into());
        let mut ao = dev("cDAQ1Mod2", "0C", true, 0, 2);
        ao.parent = Some("cDAQ1".into());
        let card = dev("PCIe6363", "0D", true, 16, 2);

        let ok = build_ni(
            Ok(vec![chassis, ai, ao, card]),
            &NiSelection {
                inputs: vec!["cDAQ1Mod1/ai0".into()],
                outputs: vec!["cDAQ1Mod2/ao0".into()],
                // The daemon's config shape: modules only, chassis unnamed.
                restrict_devices: vec!["cDAQ1Mod1".into(), "cDAQ1Mod2".into()],
                ..NiSelection::default()
            },
            1024,
        );
        assert!(
            ok.is_ok(),
            "sibling modules restricted without their chassis must still be ONE \
             family (the chassis row is retained as evidence): {:?}",
            ok.err()
        );
    }

    // -------------------------------------------------------------------------
    // The NI station: the classification
    // -------------------------------------------------------------------------

    /// The DRIVER's `is_simulated` decides the class, and the provenance says so.
    ///
    /// RED-MUTATION: hard-code `StationClass::NiReal` and the first half reds;
    /// hard-code `NiSimulated` and the second half reds.
    #[test]
    fn the_driver_decides_ni_sim_versus_ni_real() {
        let sim = build_ni(
            Ok(vec![dev("cDAQ9178Sim1Mod1", "01ABCDEF", true, 4, 2)]),
            &NiSelection::default(),
            1024,
        )
        .expect("builds");
        assert_eq!(sim.station, StationClass::NiSimulated);
        assert_eq!(sim.provenance.driver_simulated, Some(true));
        assert_eq!(sim.provenance.station_label, "NI-SIM (driver-simulated)");
        assert_eq!(
            sim.provenance.device_ids,
            vec!["cDAQ9178Sim1Mod1".to_string()]
        );
        assert_eq!(sim.provenance.serials, vec!["01ABCDEF".to_string()]);

        let real = build_ni(
            Ok(vec![dev("cDAQ1Mod1", "01F2E3D4", false, 4, 2)]),
            &NiSelection::default(),
            1024,
        )
        .expect("builds");
        assert_eq!(real.station, StationClass::NiReal);
        assert_eq!(real.provenance.driver_simulated, Some(false));
        assert_eq!(real.provenance.station_label, "NI-REAL");
        assert!(real.station.is_hardware());
    }

    /// Devices the selection did NOT bind contribute nothing to the class or the
    /// provenance. A chassis module the run never reads cannot make a real
    /// measurement look simulated (or the reverse).
    #[test]
    fn only_the_bound_devices_decide_the_class() {
        let st = build_ni(
            Ok(vec![
                dev("cDAQ1Mod1", "0A", false, 4, 2),
                // Simulated, present in the scan, NOT selected.
                dev("cDAQ1Mod9", "0Z", true, 4, 2),
            ]),
            &NiSelection::default(),
            1024,
        )
        .expect("builds");
        assert_eq!(st.station, StationClass::NiReal);
        assert_eq!(st.provenance.device_ids, vec!["cDAQ1Mod1".to_string()]);
    }

    /// A device with no serial contributes an EMPTY string rather than being
    /// dropped, so `device_ids` and `serials` stay index-aligned.
    #[test]
    fn a_serial_less_device_keeps_the_two_lists_aligned() {
        let st = build_ni(
            Ok(vec![dev("cDAQ1Mod1", "", true, 4, 2)]),
            &NiSelection::default(),
            1024,
        )
        .expect("builds");
        assert_eq!(st.provenance.device_ids.len(), st.provenance.serials.len());
        assert_eq!(st.provenance.serials, vec![String::new()]);
    }

    // -------------------------------------------------------------------------
    // The other registry rows
    // -------------------------------------------------------------------------

    /// The table's bindings are what the NI task is built from when they exist.
    #[test]
    fn an_explicitly_bound_table_binds_those_channels_not_derived_ones() {
        let st = build_ni(
            Ok(vec![dev("cDAQ1Mod1", "0A", true, 8, 2)]),
            &NiSelection {
                inputs: vec!["cDAQ1Mod1/ai5".into()],
                outputs: vec!["cDAQ1Mod1/ao1".into()],
                derive_inputs: 4,
                derive_outputs: 1,
                ..NiSelection::default()
            },
            1024,
        )
        .expect("builds");
        assert_eq!(st.provenance.device_ids, vec!["cDAQ1Mod1".to_string()]);
        assert_eq!(st.station, StationClass::NiSimulated);
    }

    /// PARTIAL binding: inputs bound, drive not. The unbound direction derives;
    /// the bound one is honored. Treating "any explicit binding" as "fully
    /// explicit" would build a task with no output while the operator was halfway
    /// through the grid.
    #[test]
    fn a_partially_bound_table_derives_only_the_unbound_direction() {
        let st = build_ni(
            Ok(vec![dev("cDAQ1Mod1", "0A", true, 4, 2)]),
            &NiSelection {
                inputs: vec!["cDAQ1Mod1/ai2".into()],
                outputs: Vec::new(),
                derive_inputs: 1,
                derive_outputs: 1,
                ..NiSelection::default()
            },
            1024,
        )
        .expect("the unbound drive derives");
        assert_eq!(st.station, StationClass::NiSimulated);
    }

    // -------------------------------------------------------------------------
    // The operator's device set is a hard filter
    // -------------------------------------------------------------------------

    /// THE ABSENT-DEVICE REFUSAL at arm. The operator bound this station to a
    /// device; a different device that happens to be attached is not a
    /// substitute, and it is certainly not the Sim.
    ///
    /// RED-MUTATION: delete the `SelectedDevicesAbsent` arm so `restrict_devices`
    /// simply filters to nothing, and this reds (the build would fall through to
    /// `NoNiDevices`, which says the wrong thing, or - with the filter removed
    /// entirely - would SUCCEED on the wrong device). Run and reverted.
    #[test]
    fn a_bound_device_that_is_not_in_the_scan_refuses_and_names_it() {
        let err = build_ni(
            // The scan found a perfectly usable device. It is not the one the
            // operator selected.
            Ok(vec![dev("cDAQ1Mod9", "0Z", true, 4, 2)]),
            &NiSelection {
                restrict_devices: vec!["cDAQ9178Sim1Mod1".into()],
                ..NiSelection::default()
            },
            1024,
        )
        .unwrap_err();
        match &err {
            StationBuildError::SelectedDevicesAbsent { missing, found } => {
                assert_eq!(missing, &vec!["cDAQ9178Sim1Mod1".to_string()]);
                assert_eq!(found, &vec!["cDAQ1Mod9".to_string()]);
            }
            other => panic!("expected SelectedDevicesAbsent, got {other:?}"),
        }
        let text = err.to_string();
        assert!(
            text.contains("previously selected device not present"),
            "{text}"
        );
        assert!(text.contains("REFUSES to run"), "{text}");
        assert!(text.contains("not a substitute"), "{text}");
    }

    /// A restricted set never derives onto a device outside it, even when that
    /// device would satisfy the channel count on its own.
    #[test]
    fn the_derivation_never_wanders_outside_the_operators_device_set() {
        let st = build_ni(
            Ok(vec![
                dev("chosen", "0A", true, 2, 1),
                dev("notChosen", "0B", false, 8, 4),
            ]),
            &NiSelection {
                derive_inputs: 2,
                derive_outputs: 1,
                restrict_devices: vec!["chosen".into()],
                ..NiSelection::default()
            },
            1024,
        )
        .expect("builds on the chosen device");
        assert_eq!(st.provenance.device_ids, vec!["chosen".to_string()]);
        assert_eq!(
            st.station,
            StationClass::NiSimulated,
            "the unchosen REAL device must not have decided the class"
        );

        // And when the chosen device is short of channels it REFUSES rather than
        // borrowing from the one next to it.
        let err = build_ni(
            Ok(vec![
                dev("chosen", "0A", true, 1, 0),
                dev("notChosen", "0B", true, 8, 4),
            ]),
            &NiSelection {
                derive_inputs: 4,
                derive_outputs: 1,
                restrict_devices: vec!["chosen".into()],
                ..NiSelection::default()
            },
            1024,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StationBuildError::NotEnoughNiChannels {
                have_inputs: 1,
                have_outputs: 0,
                ..
            }
        ));
    }

    // -------------------------------------------------------------------------
    // The channel table's physical bindings
    // -------------------------------------------------------------------------

    /// `physical_from_table` reads the table's BOUND rows, split by direction, and
    /// skips disabled rows.
    ///
    /// Moved here with the function. The other half of the old combined test
    /// (that a hosted REQUEST merges these into a `StationSpec`) stayed in
    /// `ui/src-tauri/src/factory.rs`, because merging the operator's live selection
    /// is the half that did not move.
    ///
    /// RED-MUTATION: drop the `.filter(|c| c.enabled)` and the disabled `ai7` row
    /// appears in `inputs`, sizing the task for a channel nothing reads.
    #[test]
    fn the_table_binds_physical_channels_by_direction_and_skips_disabled_rows() {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, PhysicalChannelRef};
        let mut table = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai0"));
        let mut mon = Channel::new(2);
        mon.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai1"));
        // Disabled rows never contribute: nothing acquires them, so holding a
        // physical channel would size the task for a channel nothing reads.
        let mut off = Channel::new(3);
        off.enabled = false;
        off.physical = Some(PhysicalChannelRef::new("cDAQ1Mod1", "ai7"));
        let mut drv = Channel::new(4);
        drv.role = ChannelRole::Drive;
        drv.physical = Some(PhysicalChannelRef::new("cDAQ1Mod2", "ao0"));
        table.channels.extend([ctrl, mon, off, drv]);

        let (inputs, outputs) = physical_from_table(&table);
        assert_eq!(inputs, vec!["cDAQ1Mod1/ai0", "cDAQ1Mod1/ai1"]);
        assert_eq!(outputs, vec!["cDAQ1Mod2/ao0"]);

        // An UNBOUND table contributes nothing, which is the correct answer for a
        // table authored on the Sim station (no physical channels exist to bind).
        let plain = ChannelTable::default_with_channels(2);
        assert_eq!(physical_from_table(&plain), (Vec::new(), Vec::new()));
    }
}
