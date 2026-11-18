//! The run service: a MIMO Sine run on a station, from arm to the drive at
//! zero.
//!
//! The service is handed the station's backend and channel table, the setup
//! and the law's factory. [`MimoSineService::arm`] opens and configures the
//! station; [`MimoSineService::identify`] plays the identification through the
//! station's engine and parks the station; [`MimoSineService::engage`] builds
//! the law (off the real-time path) and the core and brings the station back;
//! [`MimoSineService::run_batches`] runs the engine in bounded batches of
//! ticks, each under `catch_unwind`, as single-shaker Sine does. A panic on
//! the run thread issues the station's emergency stop at once and ends the
//! run; there is no resume. An operator stop and every trip walk the drive
//! down on the core's stop ramp, and the tasks are stopped only once every
//! drive is at zero and the station has played out what it was given.
//!
//! Nothing here touches a file. Records go to the caller through the record
//! channels ([`MimoSineService::take_records`]), sent from the loop with a call
//! that never waits.

use crate::core::{reference_at, Command, CoreParts, CoreState, MimoSineCore, ShakerRating, Slots};
use crate::ident::{self, Estimate};
use crate::law::{
    check_law_accepts, LawIdentification, LawSetup, LawTolerance, MimoSineLawFactory,
};
use crate::records::{Event, Outcome, RecordSink, TickRecord};
use crate::setup::{MimoSineSetup, SetupError, TestPlan};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_engine::{Engine, EngineConfig, ProcessStep, StopReason, VecCaptureSink};
use hertzforge_hal::{AcquisitionConfig, Backend, ConfiguredActual};
use hertzforge_mimo_sine_law_math::modal::{FitLimits, ModalForm};
use hertzforge_mimo_sine_law_math::readings::{sweep_rate_warning, SweepRateReading};
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::C64;
use hertzforge_sine::law::{LawClock, MonotonicClock};
use hertzforge_sine::SweepRate;
use std::fmt;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;

/// Engine ticks per bounded batch: single-shaker Sine's.
pub const TICKS_PER_BATCH: u64 = 2;

/// Frames of zero played after the identification so the station has played
/// out everything it was given before it is parked, in blocks.
const DRAIN_BLOCKS: usize = 16;

/// Batches of zeros run after the drive reaches zero, before the station's
/// tasks are stopped.
const DRAIN_BATCHES: u32 = 8;

/// The block the simulator station is asked for, frames.
pub const SIM_BLOCK_FRAMES: usize = 256;

/// Why the service refused or failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// The setup, or the law's acceptance of it, was refused.
    Setup(SetupError),
    /// The channel table does not hold a channel the setup names, in its role.
    Table(String),
    /// The station refused or failed.
    Station(String),
    /// Called in the wrong state.
    State(String),
    /// The identification could not be made.
    Identification(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Setup(e) => write!(f, "the setup was refused: {e}"),
            ServiceError::Table(m) => write!(f, "the channel table does not fit the setup: {m}"),
            ServiceError::Station(m) => write!(f, "the station: {m}"),
            ServiceError::State(m) => write!(f, "{m}"),
            ServiceError::Identification(m) => write!(f, "the identification: {m}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// What the service is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServiceState {
    /// Built, nothing opened.
    Idle,
    /// The station is configured and running, at zero drive.
    Armed,
    /// The identification is done and passed its gate; the station is parked.
    Identified,
    /// The identification passed and waits for the operator's review.
    AwaitingReview,
    /// The identification's coherence was under the gate; nothing more runs.
    CoherenceHeld,
    /// The loop runs.
    Running,
    /// The run is over.
    Ended(Outcome),
}

/// Everything besides the setup a run is built with.
pub struct EngineOptions {
    /// The station's class: what the run's records label it, and the engine's
    /// timing budgets.
    pub station: StationClass,
    /// The block asked of the station, frames; `None` takes the station's
    /// natural block.
    pub block_frames: Option<usize>,
    /// The identification's random seed.
    pub seed: u64,
    /// The shakers' ratings for the kinematic check (empty: none rated).
    pub ratings: Vec<ShakerRating>,
    /// The clock law calls are timed against; `None` is the monotonic clock.
    pub clock: Option<Arc<dyn LawClock>>,
    /// Update records the caller's channel holds before one is dropped.
    pub record_capacity: usize,
    /// Events the caller's channel holds before one is dropped.
    pub event_capacity: usize,
}

impl EngineOptions {
    /// Options for a station of `station`'s class, every other choice the
    /// default.
    pub fn new(station: StationClass) -> Self {
        EngineOptions {
            station,
            block_frames: None,
            seed: 1,
            ratings: Vec::new(),
            clock: None,
            record_capacity: 1 << 16,
            event_capacity: 4096,
        }
    }
}

/// What the identification found, for the screen, the record and the law.
#[derive(Debug, Clone)]
pub struct IdentificationReport {
    /// The identification on its grid, as the law is given it.
    pub identification: LawIdentification,
    /// The drive-to-response lag taken out, samples.
    pub lag_frames: usize,
    /// The level it was taken at, dB against full level, stated by response:
    /// the largest measured peak at a control over the largest peak of the
    /// full-level reference. `None` when the reference has no amplitude.
    pub level_db: Option<f64>,
    /// Each drive's rms during the measured shot, volts.
    pub drive_rms_v: Vec<f64>,
    /// Each control's mean coherence over the test's band.
    pub band_coherence: Vec<f64>,
    /// Whether every control's band coherence reached the gate.
    pub gate_passed: bool,
    /// The record length and the realizations averaged.
    pub record_len: usize,
    pub realizations: usize,
    /// The line spacing, Hz.
    pub line_spacing_hz: f64,
    /// How long the identification played on the station, s: the probe
    /// and every realization, each two records.
    pub duration_s: f64,
    /// The resonances found in the identification (a sweep only).
    pub modes_found: usize,
    /// The sweep-rate warning for each resonance (a sweep only): the
    /// resonance's own time constants the sweep spends inside its bandwidth,
    /// flagged under two.
    pub sweep_rate_warning: Vec<SweepRateReading>,
}

/// The identification's step: plays its rows, records every input frame.
struct IdentStep {
    rows: Vec<f32>,
    frames: usize,
    pos: usize,
    out_ch: usize,
    in_ch: usize,
    recorded: Vec<f32>,
    rec: usize,
    extra: usize,
}

impl IdentStep {
    fn new(
        rows: &[Vec<f64>],
        drive_slots: &[usize],
        out_ch: usize,
        in_ch: usize,
        extra: usize,
    ) -> Self {
        let frames = rows.first().map(|r| r.len()).unwrap_or(0);
        let mut inter = vec![0.0f32; frames * out_ch];
        for (r, &s) in rows.iter().zip(drive_slots) {
            for (k, v) in r.iter().enumerate() {
                inter[k * out_ch + s] = *v as f32;
            }
        }
        IdentStep {
            rows: inter,
            frames,
            pos: 0,
            out_ch,
            in_ch,
            recorded: vec![0.0; frames * in_ch],
            rec: 0,
            extra,
        }
    }

    fn done(&self) -> bool {
        self.rec >= self.frames && self.pos >= self.frames + self.extra
    }

    /// Input slot `s` as a row.
    fn row(&self, s: usize) -> Vec<f64> {
        (0..self.frames)
            .map(|k| self.recorded[k * self.in_ch + s] as f64)
            .collect()
    }
}

impl ProcessStep for IdentStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        drive_out.clear();
        drive_out.resize(n_frames * self.out_ch, 0.0);
        for f in 0..n_frames {
            if self.pos < self.frames {
                let a = self.pos * self.out_ch;
                drive_out[f * self.out_ch..(f + 1) * self.out_ch]
                    .copy_from_slice(&self.rows[a..a + self.out_ch]);
            }
            self.pos += 1;
            if self.rec < self.frames {
                let a = self.rec * self.in_ch;
                self.recorded[a..a + self.in_ch]
                    .copy_from_slice(&input[f * self.in_ch..(f + 1) * self.in_ch]);
                self.rec += 1;
            }
        }
    }
}

/// The core as the engine's step.
struct CoreStep<'a> {
    core: &'a mut MimoSineCore,
    in_ch: usize,
    out_ch: usize,
}

impl ProcessStep for CoreStep<'_> {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        drive_out.clear();
        drive_out.resize(n_frames * self.out_ch, 0.0);
        for f in 0..n_frames {
            self.core.sample(
                &input[f * self.in_ch..(f + 1) * self.in_ch],
                &mut drive_out[f * self.out_ch..(f + 1) * self.out_ch],
            );
        }
    }
}

/// The station frame positions of the setup's channels: inputs are the
/// table's enabled non-drive channels in table order, drives its enabled
/// drive channels in table order (single-shaker Sine's rule). A setup channel
/// is a table row's `index`.
pub fn slots_for(setup: &MimoSineSetup, table: &ChannelTable) -> Result<Slots, ServiceError> {
    let inputs: Vec<u32> = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .map(|c| c.index)
        .collect();
    let drives: Vec<u32> = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .map(|c| c.index)
        .collect();
    let find = |list: &[u32], ch: usize, what: &str| {
        list.iter()
            .position(|&i| i as usize == ch)
            .ok_or_else(|| ServiceError::Table(format!("channel {ch} is not an enabled {what}")))
    };
    Ok(Slots {
        in_channels: inputs.len(),
        out_channels: drives.len(),
        controls: setup
            .controls
            .iter()
            .map(|c| find(&inputs, c.channel, "input"))
            .collect::<Result<_, _>>()?,
        limits: setup
            .limits
            .iter()
            .map(|c| find(&inputs, c.channel, "input"))
            .collect::<Result<_, _>>()?,
        drives: setup
            .drives
            .iter()
            .map(|c| find(&drives, c.channel, "drive"))
            .collect::<Result<_, _>>()?,
    })
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    hertzforge_sine::law::guard::panic_text(p)
}

/// A MIMO Sine run on a station.
pub struct MimoSineService {
    setup: MimoSineSetup,
    table: ChannelTable,
    backend: Box<dyn Backend>,
    factory: Arc<dyn MimoSineLawFactory>,
    opts: EngineOptions,
    state: ServiceState,
    slots: Option<Slots>,
    actual: Option<ConfiguredActual>,
    engine: Option<Engine>,
    ident: Option<IdentificationReport>,
    law_setup: Option<LawSetup>,
    core: Option<MimoSineCore>,
    commands_tx: SyncSender<Command>,
    commands_rx: Option<Receiver<Command>>,
    sink: RecordSink,
    receivers: Option<(Receiver<TickRecord>, Receiver<Event>)>,
    parked: bool,
    drained: u32,
    station_note: Option<String>,
}

impl MimoSineService {
    /// A run of `setup` on `backend` with `table`, under the law `factory`
    /// builds. The setup is checked, and the law must accept its shape, its
    /// test type and its reference; a refusal names its rule.
    pub fn new(
        setup: MimoSineSetup,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        factory: Arc<dyn MimoSineLawFactory>,
        opts: EngineOptions,
    ) -> Result<Self, ServiceError> {
        setup.validate().map_err(ServiceError::Setup)?;
        check_law_accepts(
            &factory.manifest(),
            setup.shape(),
            setup.test_type(),
            setup.phase_specified,
        )
        .map_err(ServiceError::Setup)?;
        let slots = slots_for(&setup, &table)?;
        for r in &opts.ratings {
            if r.control >= setup.controls.len() {
                return Err(ServiceError::Table(format!(
                    "a shaker rating names control {}, and the run has {}",
                    r.control,
                    setup.controls.len()
                )));
            }
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let (sink, ticks, events) = RecordSink::channels(opts.record_capacity, opts.event_capacity);
        Ok(MimoSineService {
            setup,
            table,
            backend,
            factory,
            opts,
            state: ServiceState::Idle,
            slots: Some(slots),
            actual: None,
            engine: None,
            ident: None,
            law_setup: None,
            core: None,
            commands_tx: tx,
            commands_rx: Some(rx),
            sink,
            receivers: Some((ticks, events)),
            parked: false,
            drained: 0,
            station_note: None,
        })
    }

    /// The service's state.
    pub fn state(&self) -> ServiceState {
        self.state
    }

    /// The run's setup.
    pub fn setup(&self) -> &MimoSineSetup {
        &self.setup
    }

    /// The rate the station runs at, Hz (after arm).
    pub fn sample_rate_hz(&self) -> Option<f64> {
        self.actual.map(|a| a.actual_rate_hz)
    }

    /// The identification's report (after identify).
    pub fn identification(&self) -> Option<&IdentificationReport> {
        self.ident.as_ref()
    }

    /// What the law was built with (after engage).
    pub fn law_setup(&self) -> Option<&LawSetup> {
        self.law_setup.as_ref()
    }

    /// The core (after engage), for its live state.
    pub fn core(&self) -> Option<&MimoSineCore> {
        self.core.as_ref()
    }

    /// The station's last word, when it stopped the run.
    pub fn station_note(&self) -> Option<&str> {
        self.station_note.as_deref()
    }

    /// The mailbox the loop reads every update: stop, hold, resume, manual
    /// frequency, manual level. Clone it to command from another thread.
    pub fn commands(&self) -> SyncSender<Command> {
        self.commands_tx.clone()
    }

    /// The record channels, once: every update's record and the timeline.
    pub fn take_records(&mut self) -> Option<(Receiver<TickRecord>, Receiver<Event>)> {
        self.receivers.take()
    }

    /// Records and events dropped because the caller did not take them in
    /// time.
    pub fn records_dropped(&self) -> u64 {
        self.sink.dropped()
    }

    /// Open and configure the station and start it at zero drive.
    pub fn arm(&mut self) -> Result<(), ServiceError> {
        if self.state != ServiceState::Idle {
            return Err(ServiceError::State(
                "arm from a state other than idle".into(),
            ));
        }
        let st = |e: hertzforge_hal::HalError| ServiceError::Station(e.to_string());
        self.backend.open().map_err(st)?;
        let block = self
            .opts
            .block_frames
            .unwrap_or(if self.opts.station.is_hardware() {
                hertzforge_engine::default_block_frames(self.setup.fs_hz)
            } else {
                SIM_BLOCK_FRAMES
            });
        let acq = AcquisitionConfig::new(self.setup.fs_hz, block);
        let actual = self.backend.configure(&self.table, &acq).map_err(st)?;
        if actual.actual_rate_hz.is_nan() || actual.actual_rate_hz <= 0.0 {
            return Err(ServiceError::Station(
                "the station answered a rate of zero".into(),
            ));
        }
        let band = self.setup.test.band_hz();
        if band.1 >= 0.45 * actual.actual_rate_hz {
            return Err(ServiceError::Station(format!(
                "the station runs at {} Hz; the test reaches {} Hz",
                actual.actual_rate_hz, band.1
            )));
        }
        self.actual = Some(actual);
        self.backend.arm().map_err(st)?;
        self.backend.start().map_err(st)?;
        self.state = ServiceState::Armed;
        Ok(())
    }

    fn fresh_engine(&mut self) -> Result<(), ServiceError> {
        let slots = self.slots.as_ref().expect("built with slots");
        let actual = self
            .actual
            .ok_or_else(|| ServiceError::State("the station is not armed".into()))?;
        let mut cfg = EngineConfig::with_block(slots.in_channels, slots.out_channels, &actual);
        if !self.opts.station.is_hardware() {
            cfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
            cfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
        }
        self.engine = Some(Engine::new(cfg, &self.table, self.backend.as_ref()));
        Ok(())
    }

    /// Run `step` through the engine until `done` says so, in bounded batches
    /// under `catch_unwind`. Returns the station's stop, if it stopped.
    fn pump<S: ProcessStep>(
        &mut self,
        step: &mut S,
        done: impl Fn(&S) -> bool,
    ) -> Result<(), ServiceError> {
        let mut sink = VecCaptureSink::new();
        while !done(step) {
            let engine = self.engine.as_mut().expect("engine built");
            let backend = self.backend.as_mut();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.run_ticks(backend, step, &mut sink, TICKS_PER_BATCH)
            }));
            let report = match caught {
                Ok(r) => r,
                Err(p) => {
                    let _ = self.backend.emergency_stop();
                    self.state = ServiceState::Ended(Outcome::RunThreadFailure);
                    return Err(ServiceError::Station(format!(
                        "the run thread failed ({}); the station was stopped at once",
                        panic_message(&*p)
                    )));
                }
            };
            sink = VecCaptureSink::new();
            if report.reason != StopReason::InputEnded {
                self.state = ServiceState::Ended(Outcome::Station);
                let _ = self.backend.emergency_stop();
                return Err(ServiceError::Station(format!("{:?}", report.reason)));
            }
        }
        Ok(())
    }

    fn play_ident(
        &mut self,
        rows: &[Vec<f64>],
        extra: usize,
    ) -> Result<Vec<Vec<f64>>, ServiceError> {
        let slots = self.slots.clone().expect("built with slots");
        let mut step = IdentStep::new(
            rows,
            &slots.drives,
            slots.out_channels,
            slots.in_channels,
            extra,
        );
        self.pump(&mut step, IdentStep::done)?;
        Ok(slots
            .controls
            .iter()
            .chain(&slots.limits)
            .map(|&s| step.row(s))
            .collect())
    }

    fn reference_peak(&self, grid: &[f64]) -> f64 {
        let mut r = vec![C64::new(0.0, 0.0); self.setup.controls.len()];
        let band = self.setup.test.band_hz();
        let mut peak = 0.0f64;
        for &f in grid
            .iter()
            .filter(|f| **f >= band.0 && **f <= band.1)
            .chain([band.0, band.1].iter())
        {
            reference_at(&self.setup, f, &mut r);
            peak = r.iter().map(|v| v.norm()).fold(peak, f64::max);
        }
        peak
    }

    /// Identify the structure: a probe at a hundredth of each ceiling, then
    /// the measured shot scaled so the largest control peak sits at the
    /// identification's level under the reference's, both played through the
    /// station's engine; then the station is parked and the estimate made.
    /// A gate under the setup's coherence holds the run.
    pub fn identify(&mut self) -> Result<&IdentificationReport, ServiceError> {
        if self.state != ServiceState::Armed {
            return Err(ServiceError::State("identify before arm".into()));
        }
        let fs = self.actual.expect("armed").actual_rate_hz;
        let n = ident::record_len_for(fs, self.setup.identification.resolution_hz);
        let gband = ident::grid_band(self.setup.test.band_hz(), fs);
        let band = ident::band_lines(fs, n, gband.0, gband.1);
        let m = self.setup.drives.len();
        let nc = self.setup.controls.len();
        let ceil: Vec<f64> = self.setup.drives.iter().map(|d| d.ceiling_vpk).collect();
        let realizations = self
            .setup
            .identification
            .frames
            .map(|f| f as usize)
            .unwrap_or_else(|| crate::setup::IdentificationSettings::default_frames(m))
            .max(m);
        let idf = |e: String| ServiceError::Identification(e);
        self.fresh_engine()?;
        // The probe.
        let probe_rms: Vec<f64> = ceil
            .iter()
            .map(|c| c * ident::PROBE_RMS_OF_CEILING)
            .collect();
        let probe = ident::excitation(n, &band, &probe_rms, &ceil, 1, self.opts.seed ^ 0x9e37)
            .map_err(idf)?;
        let resp = self.play_ident(&probe, 0)?;
        let peaks = ident::measured_peaks(&resp[..nc], n, 1);
        let grid_f: Vec<f64> = band
            .iter()
            .enumerate()
            .filter_map(|(k, on)| on.then_some(k as f64 * fs / n as f64))
            .collect();
        let rpk = self.reference_peak(&grid_f);
        let probe_peak = peaks.iter().copied().fold(0.0, f64::max);
        let want = 10f64.powf(self.setup.identification.level_db / 20.0) * rpk;
        let s = if probe_peak > 0.0 && rpk > 0.0 {
            want / probe_peak
        } else {
            1.0
        };
        let rms: Vec<f64> = probe_rms.iter().map(|r| r * s).collect();
        // The measured shot, then zeros until the station has played it all.
        let rows =
            ident::excitation(n, &band, &rms, &ceil, realizations, self.opts.seed).map_err(idf)?;
        let block = self.actual.expect("armed").block_frames.max(1);
        let resp = self.play_ident(&rows, DRAIN_BLOCKS * block)?;
        self.park();
        let lag = ident::measure_lag(&rows, &resp, n);
        let est: Estimate =
            ident::estimate(&rows, &resp, n, realizations, fs, &band, lag).map_err(idf)?;
        let id = ident::law_identification(&est, nc, self.setup.identification.trust_coherence);
        let peaks = ident::measured_peaks(&resp[..nc], n, realizations);
        let level_db = (rpk > 0.0)
            .then(|| 20.0 * (peaks.iter().copied().fold(0.0, f64::max).max(1e-300) / rpk).log10());
        let drive_rms_v = rows
            .iter()
            .map(|r| (r[n..2 * n].iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt())
            .collect();
        let band_coherence = ident::band_coherence(&id, nc, self.setup.test.band_hz());
        let gate_passed = band_coherence
            .iter()
            .all(|c| *c >= self.setup.identification.gate_coherence);
        // The sweep-rate warning, for each resonance the identification shows.
        let (modes_found, sweep_rate_warning) = match &self.setup.test {
            TestPlan::Sweep(c) => sweep_warning(&id, c.band, c.rate),
            _ => (0, Vec::new()),
        };
        self.ident = Some(IdentificationReport {
            identification: id,
            lag_frames: lag,
            level_db,
            drive_rms_v,
            band_coherence,
            gate_passed,
            record_len: n,
            realizations,
            line_spacing_hz: fs / n as f64,
            duration_s: (2 * (1 + realizations) * n) as f64 / fs,
            modes_found,
            sweep_rate_warning,
        });
        self.state = if !gate_passed {
            ServiceState::CoherenceHeld
        } else if self.setup.identification.review {
            ServiceState::AwaitingReview
        } else {
            ServiceState::Identified
        };
        Ok(self.ident.as_ref().expect("just set"))
    }

    /// The operator has reviewed the identification: the run may engage.
    pub fn proceed_after_review(&mut self) -> Result<(), ServiceError> {
        if self.state != ServiceState::AwaitingReview {
            return Err(ServiceError::State("nothing waits for review".into()));
        }
        self.state = ServiceState::Identified;
        Ok(())
    }

    fn park(&mut self) {
        let _ = self.backend.stop();
        self.parked = true;
    }

    /// What the law is built with: the identification, the reference and the
    /// tolerances on its grid, the limit bounds, the level it was taken at.
    pub fn make_law_setup(&self) -> Result<LawSetup, ServiceError> {
        let rep = self
            .ident
            .as_ref()
            .ok_or_else(|| ServiceError::State("no identification".into()))?;
        let id = &rep.identification;
        let grid = &id.frequency_hz;
        let lines = grid.len();
        let (nc, nd, nl) = (
            self.setup.controls.len(),
            self.setup.drives.len(),
            self.setup.limits.len(),
        );
        let mut reference = vec![C64::new(0.0, 0.0); nc * lines];
        let mut r = vec![C64::new(0.0, 0.0); nc];
        let mut tol = LawTolerance {
            alarm_low_db: vec![0.0; nc * lines],
            alarm_high_db: vec![0.0; nc * lines],
            abort_low_db: vec![0.0; nc * lines],
            abort_high_db: vec![0.0; nc * lines],
        };
        let db = |a: f64, b: f64| (20.0 * (a / b).log10()).abs();
        for (k, &f) in grid.iter().enumerate() {
            reference_at(&self.setup, f, &mut r);
            for c in 0..nc {
                reference[c * lines + k] = r[c];
                let p = &self.setup.controls[c].profile;
                let t = p.target_accel_g_pk(f).unwrap_or(1.0);
                let al = p.alarm_bounds(f).unwrap_or((t, t));
                let ab = p.abort_bounds(f).unwrap_or((t, t));
                tol.alarm_low_db[c * lines + k] = db(t, al.0.max(1e-300));
                tol.alarm_high_db[c * lines + k] = db(al.1, t);
                tol.abort_low_db[c * lines + k] = db(t, ab.0.max(1e-300));
                tol.abort_high_db[c * lines + k] = db(ab.1, t);
            }
        }
        let mut limit_bounds = vec![None; nl * lines];
        for (j, l) in self.setup.limits.iter().enumerate() {
            for (k, &f) in grid.iter().enumerate() {
                limit_bounds[j * lines + k] = l.profile.target_accel_g_pk(f);
            }
        }
        let fs = self.actual.expect("armed").actual_rate_hz;
        let band = self.setup.test.band_hz();
        let s = LawSetup {
            controls: nc,
            drives: nd,
            limits: nl,
            fs_hz: fs,
            tick_s: (self.setup.tick_s * fs).round().max(1.0) / fs,
            band_lo_hz: band.0,
            band_hi_hz: band.1,
            test_type: self.setup.test_type(),
            shape: self.setup.shape(),
            drive_ceiling_v: self.setup.drives.iter().map(|d| d.ceiling_vpk).collect(),
            identification: id.clone(),
            identification_level_db: rep.level_db,
            reference,
            phase_specified: self.setup.phase_specified,
            limit_bounds,
            tolerance: tol,
            tracking: self.setup.tracking,
            // The trackers' own delay over their bandwidth (four one-pole
            // sections; measured in the tests).
            tracker_delay_factor: hertzforge_dsp::tone::TRACKER_DELAY_FACTOR,
            // What the identification does not show: it had the
            // drive-to-response lag taken out, and the trackers take the same
            // lag out of their reference phase, but a drive the law answers
            // still reaches its reading that many samples later. The lag
            // counts the station's buffering (the engine's output queue, the
            // device) and the structure's own transport; between a law's
            // answer and the drive there is only the ramp over the next tick,
            // which the loop-speed rule counts as its tick.
            transport_delay_s: rep.lag_frames as f64 / fs,
        };
        s.validate()
            .map_err(|e| ServiceError::Identification(e.to_string()))?;
        Ok(s)
    }

    /// Build the law (off the real-time path; `matrix` fits its model here)
    /// and the core, and bring the station back. A law that cannot be built
    /// ends the run `law_fault` before anything is driven.
    pub fn engage(&mut self) -> Result<(), ServiceError> {
        if self.state != ServiceState::Identified {
            return Err(ServiceError::State(
                "engage needs an identification that passed (and was reviewed)".into(),
            ));
        }
        let law_setup = self.make_law_setup()?;
        let law = match self.factory.build(&law_setup) {
            Ok(l) => l,
            Err(f) => {
                self.state = ServiceState::Ended(Outcome::LawFault);
                return Err(ServiceError::State(format!(
                    "the law could not be built: {f}"
                )));
            }
        };
        let rep = self.ident.as_ref().expect("identified");
        let fs = self.actual.expect("armed").actual_rate_hz;
        let parts = CoreParts {
            slots: self.slots.clone().expect("built with slots"),
            identification: rep.identification.clone(),
            lag_frames: rep.lag_frames,
            law,
            law_values: self.factory.describe().diagnostics.len(),
            clock: self
                .opts
                .clock
                .clone()
                .unwrap_or_else(|| Arc::new(MonotonicClock::new())),
            ratings: self.opts.ratings.clone(),
            records: self.sink.clone(),
            commands: self.commands_rx.take(),
        };
        let core = MimoSineCore::new(self.setup.clone(), fs, parts)
            .map_err(ServiceError::Identification)?;
        self.law_setup = Some(law_setup);
        self.core = Some(core);
        if self.parked {
            // Back to the state the identification started from: closed,
            // opened, configured as before and started, so the station's own
            // buffer holds nothing from before and the drive-to-response lag
            // is the one the identification measured.
            let st = |e: hertzforge_hal::HalError| ServiceError::Station(e.to_string());
            let block = self.actual.expect("armed").block_frames;
            self.backend.close().map_err(st)?;
            self.backend.open().map_err(st)?;
            let acq = AcquisitionConfig::new(self.setup.fs_hz, block);
            let again = self.backend.configure(&self.table, &acq).map_err(st)?;
            if again.actual_rate_hz != self.actual.expect("armed").actual_rate_hz {
                return Err(ServiceError::Station(
                    "the station came back at another rate".into(),
                ));
            }
            self.backend.arm().map_err(st)?;
            self.backend.start().map_err(st)?;
            self.parked = false;
        }
        self.fresh_engine()?;
        self.state = ServiceState::Running;
        Ok(())
    }

    /// Run up to `batches` bounded batches. Returns `true` while the run goes
    /// on. When the core's drive has reached zero after a stop or a trip, the
    /// station is drained and stopped and the run ends with its outcome.
    pub fn run_batches(&mut self, batches: u32) -> Result<bool, ServiceError> {
        if self.state != ServiceState::Running {
            return Ok(false);
        }
        let slots = self.slots.clone().expect("built with slots");
        for _ in 0..batches {
            let mut core = self.core.take().expect("engaged");
            let ended = match core.state() {
                CoreState::Ended(o) => Some(o),
                _ => None,
            };
            let mut step = CoreStep {
                core: &mut core,
                in_ch: slots.in_channels,
                out_ch: slots.out_channels,
            };
            let mut sink = VecCaptureSink::new();
            let engine = self.engine.as_mut().expect("engine built");
            let backend = self.backend.as_mut();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.run_ticks(backend, &mut step, &mut sink, TICKS_PER_BATCH)
            }));
            self.core = Some(core);
            match caught {
                Err(p) => {
                    let _ = self.backend.emergency_stop();
                    self.state = ServiceState::Ended(Outcome::RunThreadFailure);
                    return Err(ServiceError::Station(format!(
                        "the run thread failed ({}); the station was stopped at once",
                        panic_message(&*p)
                    )));
                }
                Ok(r) if r.reason != StopReason::InputEnded => {
                    // The engine already stopped the station where its rule says so.
                    let _ = self.backend.emergency_stop();
                    self.station_note = Some(format!("{:?}", r.reason));
                    self.state = ServiceState::Ended(Outcome::Station);
                    return Ok(false);
                }
                Ok(_) => {}
            }
            if let Some(o) = ended {
                // The drive is at zero: the batch just run played zeros on top
                // of what the station still held. Play out the rest, then stop.
                let drained = self.drained;
                self.drained += 1;
                if drained >= DRAIN_BATCHES {
                    self.end(o);
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn end(&mut self, outcome: Outcome) {
        // The drive is at zero: let the station play out what it holds, then
        // stop its tasks (the controlled stop).
        let _ = self.backend.stop();
        self.state = ServiceState::Ended(outcome);
    }

    /// Run until the run ends or `max_batches` have run; the outcome, if it
    /// ended.
    pub fn run_to_end(&mut self, max_batches: u64) -> Result<Option<Outcome>, ServiceError> {
        let mut k = 0;
        while k < max_batches && self.run_batches(1)? {
            k += 1;
        }
        Ok(match self.state {
            ServiceState::Ended(o) => Some(o),
            _ => None,
        })
    }

    /// Ask for the controlled stop.
    pub fn request_stop(&self) {
        let _ = self.commands_tx.try_send(Command::Stop);
    }
}

/// The resonances in an identification over `band` and the sweep-rate
/// warning for each at the sweep's rate.
pub fn sweep_warning(
    id: &LawIdentification,
    band: (f64, f64),
    rate: SweepRate,
) -> (usize, Vec<SweepRateReading>) {
    let mid = hertzforge_mimo_sine_law_math::setup::Identification {
        grid_hz: id.frequency_hz.clone(),
        response: id.response.clone(),
        limit_response: id.limit_response.clone(),
        coherence: id.coherence.clone(),
        limit_coherence: id.limit_coherence.clone(),
        trusted: id.trusted.clone(),
    };
    let Ok((modes, _)) = ModalForm::fit_bounded(&mid, band, FitLimits::default()) else {
        return (0, Vec::new());
    };
    let n = modes.modes();
    let readings = match rate {
        SweepRate::LogOctPerMin(r) => sweep_rate_warning(&modes, r),
        SweepRate::LinearHzPerSec(r) => (0..n)
            .flat_map(|k| {
                let one = ModalForm {
                    f_hz: vec![modes.f_hz[k]],
                    zeta: vec![modes.zeta[k]],
                    residues: Vec::new(),
                };
                let oct = r / (modes.f_hz[k] * std::f64::consts::LN_2) * 60.0;
                sweep_rate_warning(&one, oct)
            })
            .collect(),
    };
    (n, readings)
}
