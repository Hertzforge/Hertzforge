//! The controller's MIMO Random service behind the wire's driver trait.
//!
//! [`ServiceDriver`] implements [`MimoRunDriver`] on
//! [`hertzforge_mimo_random::MimoRandomService`]. The service is a synchronous state
//! machine whose identification and control loops are blocking calls of many engine
//! blocks, so it lives on its own run thread, as single-output Random's does
//! (`crate::random::run`):
//!
//! - The driver's methods return promptly. They answer from what the run thread last
//!   published (the status, an owned snapshot of the display quantities, the report)
//!   and hand commands to the thread over a channel.
//! - The operator's stop and abort reach the station directly, without waiting for a
//!   blocking call to end: a stop while the loop drives starts the drive ramp-down at
//!   the next output block (the service's own ramp, shared); a stop or an abort at any
//!   other time sets the station signal ([`crate::random::backend::Interruptible`]),
//!   which takes the drive down with the station's own stop at the next engine block.
//! - Any failure on the run thread ends in the station's emergency stop: every call
//!   into the service is contained, and the station is wrapped so that dropping it
//!   (after a panic, at the end of the run, or when the service drops the run) takes
//!   the drive to zero.
//!
//! The setup's reference is put on the run's line grid here: an inline reference is
//! interpolated onto the grid's lines inside its own band, and a reference file is
//! read with `hertzforge-reference` after its SHA-256 is checked against the one the
//! setup names.

use super::check::Validation;
use super::driver::{
    Curve, DriveVerdict, Fault, Gates, IdentificationInfo, LawRef, MimoReport, MimoRunDriver,
    MimoStatus, OverrideAck, OverrideReason, PackedBlock, Prediction, QuantitySelector, Series,
    StartContext, StopOutcome,
};
use super::setup::{ControlSpace, MimoSetup, RealMatrix, Reference};
use crate::protocol::{ErrorCode, MethodError};
use crate::random::backend::{Interruptible, ABORT, STOP};
use crate::random::view::{End, State};
use crate::station::{self, NiScanner, StationPlan};
use crate::table::{Item, Report};
use crate::views::Role;
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_law_host::{Breakpoint, Param, ParamKind, ParamSpec, ParamValue};
use hertzforge_mimo_random::config::{Caps as DriveCaps, EstimationWindow, LimitSpec};
use hertzforge_mimo_random::guards::OutputFault;
use hertzforge_mimo_random::quantities::QuantitySnapshot;
use hertzforge_mimo_random::{
    DriveSpec, FrfMode, MimoConfig, MimoError, MimoFaultReason, MimoQuantity, MimoRandomService,
    MimoState, MimoTripKind, MimoTripRecord, ReferenceSpec,
};
use hertzforge_random::service::config::LevelSchedule;
use hertzforge_random::service::ramp::DriveRamp;
use hertzforge_random::LoopConfig;
use hertzforge_random_laws::{
    builtin_mimo_manifest, may_arm_on_hardware, BuiltinMimo, CatalogEntry, LawCatalog, LawChoice,
    MimoLawFactory,
};
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, SimStandardPlant, StationSpec};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a driver call waits for the run thread to answer a command.
const ANSWER_WAIT: Duration = Duration::from_secs(60);

/// The law catalog the control service keeps, shared with the driver so a law file
/// chosen in the setup is resolved through the law host.
pub type SharedCatalog = Arc<Mutex<Option<LawCatalog>>>;

/// What the driver needs from the service that makes it.
#[derive(Clone)]
pub struct AdapterOptions {
    /// The local data directory: the NI hardware lock and the law catalog live here.
    pub data_root: PathBuf,
    /// Where the run's records go (`None`: the controller's local default).
    pub capture_root: Option<PathBuf>,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced.
    pub sim_speed: f64,
    pub scanner: Arc<dyn NiScanner>,
    /// What the hardware lock names as this service's address.
    pub bind: String,
    /// The service's law catalog, for a law file the setup names.
    pub catalog: Option<SharedCatalog>,
    /// A law to run in place of the one the catalog resolves (a law built
    /// elsewhere, such as a law file resolved by the caller). `None` in service.
    pub law_factory: Option<Arc<dyn MimoLawFactory>>,
}

impl AdapterOptions {
    /// Options with no catalog, run unpaced, records under `root`.
    pub fn local(root: &Path, scanner: Arc<dyn NiScanner>) -> AdapterOptions {
        AdapterOptions {
            data_root: root.to_path_buf(),
            capture_root: Some(root.to_path_buf()),
            sim_speed: 0.0,
            scanner,
            bind: "ws://127.0.0.1:0/".into(),
            catalog: None,
            law_factory: None,
        }
    }
}

/// A driver factory that runs every MIMO setup on the controller's service.
pub fn factory(opts: AdapterOptions) -> super::driver::DriverFactory {
    Arc::new(move |_ctx: &StartContext| Box::new(ServiceDriver::new(opts.clone())))
}

// ---- the reference on the run's grid ------------------------------------------------

/// The reference as the run uses it: on the grid's lines, in the control space,
/// with tolerances and band.
#[derive(Debug, Clone)]
pub struct GridReference {
    pub spec: ReferenceSpec,
    /// Where it came from, in words, for the run's log.
    pub source: String,
}

/// The run's line grid: `nfft / 2 + 1` lines at `fs / nfft`.
fn grid_of(setup: &MimoSetup) -> (usize, f64) {
    (setup.estimation.n_pts / 2 + 1, setup.df_hz())
}

fn tolerances_about(sdm: &HermitianStack, band: (usize, usize), setup: &MimoSetup) -> Tolerances {
    let n = sdm.n();
    let mut t = Tolerances::none(sdm.lines(), n);
    for i in 0..n {
        let (alarm, abort) = setup.band_of(i);
        for line in band.0..band.1 {
            let p = sdm.get(line, i, i).re;
            let k = line * n + i;
            t.alarm_upper[k] = p * 10f64.powf(alarm[1] / 10.0);
            t.alarm_lower[k] = p * 10f64.powf(-alarm[0] / 10.0);
            t.abort_upper[k] = p * 10f64.powf(abort[1] / 10.0);
            t.abort_lower[k] = p * 10f64.powf(-abort[0] / 10.0);
        }
    }
    t
}

/// An inline reference on the run's grid: every grid line inside the reference's
/// first and last frequency, each packed element interpolated linearly between the
/// two reference lines around it (a convex combination of two positive
/// semi-definite matrices is one too). Lines outside are zero.
pub fn inline_on_grid(
    setup: &MimoSetup,
    freqs: &[f64],
    n: usize,
    re: &[f64],
    im: &[f64],
) -> Result<GridReference, String> {
    let (lines, df) = grid_of(setup);
    let per = HermitianStack::packed_len(n);
    if freqs.len() < 2 || re.len() != freqs.len() * per || im.len() != re.len() {
        return Err("the inline reference's shape does not match its line count".into());
    }
    let (f0, f1) = (freqs[0], freqs[freqs.len() - 1]);
    let eps = 1e-9 * df;
    let lo = ((f0 - eps) / df).ceil().max(1.0) as usize;
    let hi = (((f1 + eps) / df).floor() as usize + 1).min(lines);
    if hi <= lo + 1 {
        return Err(format!(
            "the reference spans {f0} to {f1} Hz, fewer than two lines of the {df} Hz grid; \
             widen the reference or use more points per frame"
        ));
    }
    let mut sdm = HermitianStack::zeros(lines, n);
    let mut k = 0usize;
    for line in lo..hi {
        let f = line as f64 * df;
        while k + 2 < freqs.len() && freqs[k + 1] < f {
            k += 1;
        }
        let (fa, fb) = (freqs[k], freqs[k + 1]);
        let w = if fb > fa {
            ((f - fa) / (fb - fa)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        for r in 0..n {
            for c in r..n {
                let p = HermitianStack::packed_index(n, r, c);
                let a = C64::new(re[k * per + p], im[k * per + p]);
                let b = C64::new(re[(k + 1) * per + p], im[(k + 1) * per + p]);
                sdm.set(line, r, c, a * (1.0 - w) + b * w);
            }
        }
    }
    let band = (lo, hi);
    let tolerances = tolerances_about(&sdm, band, setup);
    Ok(GridReference {
        spec: ReferenceSpec {
            sdm,
            tolerances,
            band,
            df_hz: df,
        },
        source: format!(
            "inline, {} points from {f0} to {f1} Hz, on {} grid lines",
            freqs.len(),
            hi - lo
        ),
    })
}

/// The folder of a reference path: the folder itself, or the folder holding a
/// `reference.bin` or `reference.json` the path names.
fn reference_dir(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_file() {
        p.parent().map(Path::to_path_buf).unwrap_or(p)
    } else {
        p
    }
}

/// A reference file on the run's grid, its content hash checked against the
/// setup's first.
pub fn file_on_grid(setup: &MimoSetup, path: &str, sha256: &str) -> Result<GridReference, String> {
    let dir = reference_dir(path);
    let (bin, _) = hertzforge_reference::native::paths(&dir);
    let bytes = std::fs::read(&bin).map_err(|e| {
        format!(
            "{} could not be read ({e}); choose the folder that holds the reference file",
            bin.display()
        )
    })?;
    let hash = hertzforge_reference::native::content_hash(&bytes);
    let want = format!("sha256:{}", sha256.to_ascii_lowercase());
    if hash != want {
        return Err(format!(
            "the reference file's SHA-256 is {} but the setup names {}; the file changed \
             after it was chosen, so choose it again",
            hash.trim_start_matches("sha256:"),
            sha256
        ));
    }
    let r = hertzforge_reference::native::read(&dir).map_err(|e| e.to_string())?;
    let (lines, df) = grid_of(setup);
    if (r.df_hz - df).abs() > 1e-9 * df || r.lines() != lines {
        return Err(format!(
            "the reference file has {} lines of {} Hz and the setup's grid has {lines} lines of \
             {df} Hz; build the reference on the setup's grid (sample rate {} Hz, {} points)",
            r.lines(),
            r.df_hz,
            setup.estimation.fs_hz,
            setup.estimation.n_pts
        ));
    }
    if r.n() != setup.controlled() {
        return Err(format!(
            "the reference file is {} by {} and the setup controls {} quantities; choose a \
             reference written for these controls",
            r.n(),
            r.n(),
            setup.controlled()
        ));
    }
    Ok(GridReference {
        spec: ReferenceSpec {
            sdm: r.sdm,
            tolerances: r.tolerances,
            band: r.band,
            df_hz: r.df_hz,
        },
        source: format!("file {} ({})", dir.display(), hash),
    })
}

/// The setup's reference on the run's grid.
pub fn reference_on_grid(setup: &MimoSetup) -> Result<GridReference, String> {
    match &setup.reference {
        Reference::Inline {
            freqs_hz,
            n,
            re,
            im,
        } => inline_on_grid(setup, freqs_hz, *n, re, im),
        Reference::File { path, sha256 } => file_on_grid(setup, path, sha256),
    }
}

// ---- the configuration ---------------------------------------------------------------

fn matrix(m: &RealMatrix) -> Result<MatrixStack, String> {
    MatrixStack::from_vec(
        1,
        m.rows,
        m.cols,
        m.values.iter().map(|v| C64::new(*v, 0.0)).collect(),
    )
    .map_err(|e| e.0)
}

/// The law's parameter values from the setup's JSON, by the kinds its manifest
/// declares. Keys the manifest does not declare were refused by the setup's check.
pub fn params_of(specs: &[ParamSpec], given: &Map<String, Value>) -> Vec<Param> {
    let mut out = Vec::new();
    for s in specs {
        let Some(v) = given.get(&s.key) else { continue };
        let value = match &s.kind {
            ParamKind::Number(_) => v.as_f64().map(ParamValue::Number),
            ParamKind::Integer(_) => v.as_i64().map(ParamValue::Integer),
            ParamKind::Toggle(_) => v.as_bool().map(ParamValue::Toggle),
            ParamKind::Choice(_) => v.as_str().map(|x| ParamValue::Choice(x.to_owned())),
            ParamKind::PerChannel(_) => v
                .as_array()
                .map(|a| ParamValue::PerChannel(a.iter().filter_map(Value::as_f64).collect())),
            ParamKind::Curve(_) => v.as_array().map(|a| {
                ParamValue::Curve(
                    a.iter()
                        .filter_map(|p| match p {
                            Value::Array(xy) if xy.len() == 2 => Some(Breakpoint {
                                frequency_hz: xy[0].as_f64()?,
                                value: xy[1].as_f64()?,
                            }),
                            Value::Object(o) => Some(Breakpoint {
                                frequency_hz: o.get("frequency_hz")?.as_f64()?,
                                value: o.get("value")?.as_f64()?,
                            }),
                            _ => None,
                        })
                        .collect(),
                )
            }),
        };
        if let Some(value) = value {
            out.push(Param::new(s.key.clone(), value));
        }
    }
    out
}

/// The law a setup (or a new choice in the review hold) names: the choice and, for
/// a law file, the factory the catalog resolves.
fn resolve_law(
    id: &str,
    params: &Map<String, Value>,
    controls: usize,
    opts: &AdapterOptions,
) -> Result<ResolvedLaw, String> {
    if let Some(b) = BuiltinMimo::from_id(id) {
        let specs = builtin_mimo_manifest(b).law.params;
        return Ok((LawChoice::new(id, params_of(&specs, params)), None, None));
    }
    let Some(cell) = opts.catalog.as_ref() else {
        return Err(format!(
            "the law `{id}` is not built in and no law catalog is open; import the law file \
             and choose it from the catalog"
        ));
    };
    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        *guard = Some(LawCatalog::open(&opts.data_root).map_err(|e| e.to_string())?);
    }
    let cat = guard.as_ref().ok_or("the law catalog did not open")?;
    let entry = cat
        .entry(id)
        .ok_or_else(|| format!("the law `{id}` is not in the catalog; import it first"))?;
    let choice = LawChoice::new(id, params_of(&entry.manifest.params, params));
    let f = cat
        .resolve_mimo(&choice, controls)
        .map_err(|e| e.to_string())?;
    let file = cat.laws_dir().join(format!("{id}.wasm"));
    Ok((choice, Some(f), file.is_file().then_some(file)))
}

/// The hardware arming gate for control laws: the simulator runs any
/// law; a run on a hardware station arms only with a built-in law or a law file
/// whose check on the simulator passed for that exact file.
pub fn hardware_law_gate(
    kind: &str,
    law_id: &str,
    entry: Option<&CatalogEntry>,
) -> Result<(), MethodError> {
    if kind == "sim" || BuiltinMimo::from_id(law_id).is_some() {
        return Ok(());
    }
    let Some(entry) = entry else {
        return Err(refused(
            "law_not_checked",
            format!(
                "This run is on hardware and the law `{law_id}` is not in the law catalog; import \
                 it, run its check on the simulator, then start again."
            ),
        ));
    };
    may_arm_on_hardware(entry).map_err(|why| {
        refused(
            "law_not_checked",
            format!(
                "This run is on hardware and {why}. Run the law's check on the simulator \
                 and start again, or choose a built-in law."
            ),
        )
    })
}

/// The catalog's entry for `id`, opening the catalog if it is not open yet.
fn catalog_entry(id: &str, opts: &AdapterOptions) -> Option<CatalogEntry> {
    let cell = opts.catalog.as_ref()?;
    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        *guard = LawCatalog::open(&opts.data_root).ok();
    }
    guard.as_ref()?.entry(id)
}

/// A law as a run takes it: the choice, the factory for a law file (none for a
/// built-in) and the law file's place in the catalog.
type ResolvedLaw = (LawChoice, Option<Arc<dyn MimoLawFactory>>, Option<PathBuf>);

/// The controller's configuration of `setup` on `table`.
pub fn config_of(
    setup: &MimoSetup,
    reference: &GridReference,
    law: LawChoice,
) -> Result<(MimoConfig, LoopConfig), String> {
    let mut drives = Vec::with_capacity(setup.drives.len());
    for d in &setup.drives {
        let mut s = DriveSpec::new(d.channel, d.ceiling_vrms);
        s.abort_vrms = d.abort_vrms;
        s.peak_v = d.peak_v;
        s.shaker = d
            .shaker
            .as_ref()
            .map(|v| serde_json::from_value::<ShakerRecord>(v.clone()))
            .transpose()
            .map_err(|e| {
                format!(
                    "the shaker record of channel {} does not parse ({e}); give it in the shaker \
                     database's form or leave it out",
                    d.channel
                )
            })?;
        // The setup carries the shaker's own moving mass as one figure.
        s.moving_mass = MovingMass {
            armature_lb: d.moving_mass_lb,
            ..MovingMass::default()
        };
        drives.push(s);
    }
    let controls = setup.controls.iter().map(|c| c.channel).collect();
    let mut cfg = MimoConfig::new(controls, drives, reference.spec.clone());
    cfg.limits = setup
        .limits
        .iter()
        .map(|l| LimitSpec {
            channel_index: l.channel,
            ceiling_psd: vec![l.ceiling],
            abort_margin_db: l.abort_margin_db,
        })
        .collect();
    cfg.article_mass_lb = setup.article_mass_lb;
    cfg.caps = DriveCaps {
        up_db: setup.control.cap_up_db,
        down_db: setup.control.cap_down_db,
    };
    cfg.soft_start_db = setup.control.soft_start_db;
    cfg.law_guards.budget_fraction = setup.control.law_budget;
    cfg.law_guards.max_misses = setup.control.law_misses;
    if let ControlSpace::Virtual {
        response, drive, ..
    } = &setup.space
    {
        cfg.control_space = hertzforge_mimo_random::ControlSpace::Virtual {
            response: matrix(response)?,
        };
        cfg.drive_transform = drive.as_ref().map(matrix).transpose()?;
    }
    cfg.law = law;
    cfg.frf_mode = if setup.control.frf_update {
        FrfMode::Update {
            blend: setup.control.frf_blend,
        }
    } else {
        FrfMode::Hold
    };
    cfg.estimation.frames_per_loop = setup.estimation.frames_per_loop;
    cfg.estimation.memory_loops = setup.estimation.memory_loops.max(1) as f64;
    cfg.estimation.overlap = setup.estimation.overlap;
    cfg.estimation.window = match setup.estimation.window.as_str() {
        "rectangular" => EstimationWindow::Rectangular,
        _ => EstimationWindow::Hann,
    };
    let n_d = setup.drives.len();
    cfg.identification.below_reference_db = -setup.identification.level_db;
    cfg.identification.frames = setup.identification.frames.max(2 * n_d);
    cfg.identification.noise_frames = setup.identification.noise_frames.max(1);
    cfg.gates.coherence_threshold = setup.identification.coherence_floor;
    cfg.gates.condition_threshold = setup.identification.condition_limit;
    let levels: Vec<f64> = setup.schedule.levels.iter().map(|l| l.0).collect();
    cfg.schedule = LevelSchedule {
        mode: setup.schedule.mode,
        start_db: levels.first().copied().unwrap_or(0.0),
        table_db: levels,
        dwell_s: setup
            .schedule
            .levels
            .iter()
            .map(|l| l.1.unwrap_or(0.0))
            .collect(),
    };
    cfg.level_ramp_s = setup.control.level_ramp_s;
    cfg.shutdown_ramp_s = setup.control.shutdown_ramp_s;
    cfg.full_level_duration_s =
        (setup.schedule.full_level_s > 0.0).then_some(setup.schedule.full_level_s);
    cfg.protections.divergence_loops = setup.control.max_loops_out.max(1);
    let mut lc = LoopConfig::demo_default();
    lc.fs_hz = setup.estimation.fs_hz;
    lc.nfft = setup.estimation.n_pts;
    let (lo, hi) = reference.spec.band;
    lc.band_lo_hz = (lo as f64 * reference.spec.df_hz).max(lc.df());
    lc.band_hi_hz = (hi.saturating_sub(1)) as f64 * reference.spec.df_hz;
    Ok((cfg, lc))
}

// ---- the station the run drives --------------------------------------------------------

/// The simulator's plant for a test of `inputs` measurement rows and `drives` drive
/// rows: the first standard cross-coupled modal plant of that size.
pub fn sim_plant_for(inputs: usize, drives: usize) -> Option<SimStandardPlant> {
    SimStandardPlant::ALL
        .into_iter()
        .find(|p| p.dims() == (inputs, drives))
}

/// The station wrapper the run drives through: paced on the simulator, and the
/// drive to zero when it is dropped (the end of a run, a panic on the run thread, or
/// the service dropping the run). Every call is forwarded explicitly.
struct RunStation {
    inner: Box<dyn Backend>,
    /// `(fs_hz, speed, started, frames)` when the simulator is paced.
    pace: Option<(f64, f64, Instant, Arc<AtomicU64>)>,
    zeroed: bool,
}

impl RunStation {
    fn wrap(inner: Box<dyn Backend>, pace: Option<(f64, f64)>) -> Box<dyn Backend> {
        Box::new(RunStation {
            inner,
            pace: pace.map(|(fs, s)| (fs, s, Instant::now(), Arc::new(AtomicU64::new(0)))),
            zeroed: false,
        })
    }
}

impl Drop for RunStation {
    fn drop(&mut self) {
        if !self.zeroed {
            self.zeroed = true;
            let _ = self.inner.emergency_stop();
        }
    }
}

impl Backend for RunStation {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out);
        if let (Ok(InputRead::Frames(n)), Some((fs, speed, t0, frames))) = (&r, &self.pace) {
            let total = frames.fetch_add(*n as u64, Ordering::Relaxed) + *n as u64;
            if *speed > 0.0 {
                let ahead = total as f64 / fs / speed - t0.elapsed().as_secs_f64();
                if ahead > 0.005 {
                    std::thread::sleep(Duration::from_secs_f64(ahead.min(0.25)));
                }
            }
        }
        r
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.zeroed = true;
        self.inner.emergency_stop()
    }
}

fn refused(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

fn wrong_state(verb: &str, phase: &str) -> MethodError {
    refused(
        "wrong_state",
        format!(
            "{verb} does not apply while the run is {}.",
            phase.replace('_', " ")
        ),
    )
}

// ---- what the thread publishes -------------------------------------------------------------

/// The run thread's published state.
#[derive(Clone)]
struct Published {
    status: MimoStatus,
    snapshot: Option<Arc<QuantitySnapshot>>,
    /// The setup's control positions the run controls (after any excluded at the
    /// gate); `None`: every one.
    kept: Option<Vec<usize>>,
    report: Option<MimoReport>,
    /// The identification set's record folder, once the run has written one.
    ident_dir: Option<PathBuf>,
}

/// How the operator asked the run to end (the first ask wins).
#[derive(Debug, Clone)]
struct EndRequest {
    outcome: &'static str,
    by: String,
    level_db: Option<f64>,
}

struct Shared {
    published: Mutex<Published>,
    signal: Arc<AtomicU8>,
    ramp: Mutex<Option<Arc<DriveRamp>>>,
    started: Mutex<Option<Instant>>,
    end_request: Mutex<Option<EndRequest>>,
    log: Mutex<Vec<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

type Reply<T> = mpsc::Sender<Result<T, MethodError>>;

enum Cmd {
    Start {
        ctx: Box<StartContext>,
        reply: Reply<()>,
    },
    Proceed {
        reply: Reply<()>,
    },
    Override {
        ack: String,
        by: String,
        reason: Option<OverrideReason>,
        reply: Reply<()>,
    },
    Iterate {
        law: Option<(LawRef, Map<String, Value>)>,
        reply: Reply<Prediction>,
    },
    Exclude {
        positions: Vec<usize>,
        ack: String,
        by: String,
        reply: Reply<()>,
    },
    Stop,
    Abort {
        by: String,
    },
}

/// [`MimoRunDriver`] on the controller's MIMO Random service.
pub struct ServiceDriver {
    opts: AdapterOptions,
    setup: Option<Arc<MimoSetup>>,
    law: LawRef,
    shared: Arc<Shared>,
    tx: Option<Sender<Cmd>>,
    /// An identification set to load when the run starts.
    pending_ident: Option<PathBuf>,
}

fn idle_status(law: LawRef) -> MimoStatus {
    MimoStatus {
        state: State::Configured,
        phase: "idle",
        elapsed_s: None,
        level_db: None,
        at_level_s: None,
        remaining_at_level_s: None,
        loop_index: 0,
        alarm: false,
        control_error_db: Vec::new(),
        gates: None,
        caps: Vec::new(),
        drive_rms_v: Vec::new(),
        law,
        faults: Vec::new(),
        override_ack: Vec::new(),
        prediction: None,
        excluded: Vec::new(),
        end: None,
        streaming: false,
    }
}

impl ServiceDriver {
    pub fn new(opts: AdapterOptions) -> ServiceDriver {
        let law = LawRef {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            sha256: String::new(),
        };
        ServiceDriver {
            opts,
            setup: None,
            shared: Arc::new(Shared {
                published: Mutex::new(Published {
                    status: idle_status(law.clone()),
                    snapshot: None,
                    kept: None,
                    report: None,
                    ident_dir: None,
                }),
                signal: Arc::new(AtomicU8::new(0)),
                ramp: Mutex::new(None),
                started: Mutex::new(None),
                end_request: Mutex::new(None),
                log: Mutex::new(Vec::new()),
            }),
            law,
            tx: None,
            pending_ident: None,
        }
    }

    /// The run's log of operator events (who stopped it, a lost controller), oldest
    /// first.
    pub fn log(&self) -> Vec<String> {
        lock(&self.shared.log).clone()
    }

    /// The run's record folder holding its identification set, once identification
    /// has been measured or loaded.
    pub fn identification_dir(&self) -> Option<PathBuf> {
        lock(&self.shared.published).ident_dir.clone()
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd, verb: &str) -> Result<T, MethodError> {
        let Some(tx) = self.tx.as_ref() else {
            let phase = lock(&self.shared.published).status.phase;
            return Err(wrong_state(verb, phase));
        };
        let (rtx, rrx) = mpsc::channel();
        tx.send(make(rtx))
            .map_err(|_| MethodError::internal("the MIMO run thread has ended"))?;
        match rrx.recv_timeout(ANSWER_WAIT) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(MethodError::internal(format!(
                "the MIMO run did not answer within {} s; the request stands",
                ANSWER_WAIT.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(MethodError::internal(
                "the MIMO run ended before it answered",
            )),
        }
    }

    fn request_end(&self, outcome: &'static str, by: &str) {
        let level = {
            let p = lock(&self.shared.published);
            p.status.level_db
        };
        let mut r = lock(&self.shared.end_request);
        if r.is_none() {
            *r = Some(EndRequest {
                outcome,
                by: by.to_owned(),
                level_db: level,
            });
        }
    }
}

impl MimoRunDriver for ServiceDriver {
    fn configure(
        &mut self,
        setup: Arc<MimoSetup>,
        law: Option<LawRef>,
        items: Report,
    ) -> Validation {
        let mut v = super::check::numeric(&setup, items, law.clone());
        // A reference file is read and its hash checked now, so a wrong file is
        // refused with the setup rather than when the run starts.
        if matches!(setup.reference, Reference::File { .. }) {
            v.report
                .warnings
                .retain(|i| i.reason != "reference_file_unread");
            if let Err(e) = reference_on_grid(&setup) {
                v.report.errors.push(Item::field(
                    "reference_file_unread",
                    "setup.reference",
                    format!("The reference file was not used: {e}."),
                ));
            }
        }
        if let Some(l) = law {
            self.law = l;
        }
        lock(&self.shared.published).status.law = self.law.clone();
        self.setup = Some(setup);
        v
    }

    fn start(&mut self, ctx: &StartContext) -> Result<(), MethodError> {
        let Some(setup) = self.setup.clone() else {
            return Err(refused(
                "not_configured",
                "MIMO Random is not set up; send a setup first.",
            ));
        };
        if self.tx.is_some() {
            let phase = lock(&self.shared.published).status.phase;
            return Err(wrong_state("Starting", phase));
        }
        let (tx, rx) = mpsc::channel();
        let actor = Actor {
            opts: self.opts.clone(),
            setup,
            law: self.law.clone(),
            shared: self.shared.clone(),
            svc: None,
            lock: None,
            review: true,
            ended: false,
            override_ack: Vec::new(),
            min_cap: Vec::new(),
            faults: Vec::new(),
            report: None,
            pending_ident: self.pending_ident.clone(),
            kept: None,
        };
        std::thread::Builder::new()
            .name("mimo-random".into())
            .spawn(move || actor.run(rx))
            .map_err(|e| {
                MethodError::internal(format!("the MIMO run thread did not start: {e}"))
            })?;
        self.tx = Some(tx);
        let ctx = Box::new(ctx.clone());
        self.ask(|reply| Cmd::Start { ctx, reply }, "Starting")
    }

    fn proceed(&mut self) -> Result<(), MethodError> {
        self.ask(|reply| Cmd::Proceed { reply }, "Proceeding")
    }

    fn override_gate(
        &mut self,
        ack: &str,
        by: &str,
        reason: Option<OverrideReason>,
    ) -> Result<(), MethodError> {
        let (ack, by) = (ack.to_owned(), by.to_owned());
        self.ask(
            |reply| Cmd::Override {
                ack,
                by,
                reason,
                reply,
            },
            "Overriding",
        )
    }

    fn exclude_controls(
        &mut self,
        positions: &[usize],
        ack: &str,
        by: &str,
    ) -> Result<(), MethodError> {
        let (positions, ack, by) = (positions.to_vec(), ack.to_owned(), by.to_owned());
        self.ask(
            |reply| Cmd::Exclude {
                positions,
                ack,
                by,
                reply,
            },
            "Excluding a control",
        )
    }

    fn iterate_prediction(
        &mut self,
        law: Option<(LawRef, Map<String, Value>)>,
    ) -> Result<Prediction, MethodError> {
        let p = self.ask(|reply| Cmd::Iterate { law, reply }, "A new prediction")?;
        self.law = p.law.clone();
        Ok(p)
    }

    fn stop(&mut self, outcome: StopOutcome, by: &str) {
        self.request_end(outcome.name(), by);
        lock(&self.shared.log).push(format!("{} by {by}", outcome.name()));
        let phase = lock(&self.shared.published).status.phase;
        let ramp = lock(&self.shared.ramp).clone();
        let ramped = matches!(phase, "engaged" | "full_level" | "ramping_down")
            && ramp.is_some_and(|r| r.request_stop().is_some());
        if !ramped && matches!(phase, "sysid_noise" | "sysid" | "engaged" | "full_level") {
            // Inside a blocking call: end it at the next engine block with the
            // station's controlled stop.
            let _ = self.shared.signal.fetch_max(STOP, Ordering::SeqCst);
        }
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(Cmd::Stop);
        }
    }

    fn abort(&mut self, by: &str) {
        self.request_end("operator_abort", by);
        lock(&self.shared.log).push(format!("operator_abort by {by}"));
        if self.tx.is_some() {
            let _ = self.shared.signal.fetch_max(ABORT, Ordering::SeqCst);
        }
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(Cmd::Abort { by: by.to_owned() });
        }
    }

    fn status(&mut self) -> MimoStatus {
        let mut s = lock(&self.shared.published).status.clone();
        if let Some(t0) = *lock(&self.shared.started) {
            if s.end.is_none() {
                s.elapsed_s = Some(t0.elapsed().as_secs_f64());
            }
        }
        let asked = lock(&self.shared.end_request).is_some();
        if s.state.is_live() && asked && matches!(s.phase, "engaged" | "full_level") {
            s.state = State::Stopping;
            s.phase = "ramping_down";
        }
        if matches!(s.phase, "engaged" | "full_level" | "ramping_down") {
            if let Some(r) = lock(&self.shared.ramp).as_ref() {
                let now = r.level_db();
                if now.is_finite() {
                    s.level_db = Some(now.max(-60.0));
                }
            }
        }
        s
    }

    fn state(&mut self) -> State {
        lock(&self.shared.published).status.state
    }

    fn quantity(&mut self, q: &QuantitySelector) -> Option<Series> {
        let (snap, kept) = {
            let p = lock(&self.shared.published);
            (p.snapshot.clone()?, p.kept.clone())
        };
        let setup = self.setup.as_ref()?;
        match kept {
            None => quantity_of(&snap, setup, q),
            Some(kept) => quantity_kept(&snap, setup, q, &kept),
        }
    }

    fn report(&mut self) -> Option<MimoReport> {
        lock(&self.shared.published).report.clone()
    }

    fn controller_lost(&mut self, name: &str) {
        lock(&self.shared.log).push(format!("controller lost: {name}"));
    }

    fn controller_back(&mut self, name: &str) {
        lock(&self.shared.log).push(format!("controller back: {name}"));
    }

    fn load_identification(
        &mut self,
        path: &Path,
        table: &ChannelTable,
    ) -> Result<IdentificationInfo, MethodError> {
        let Some(setup) = self.setup.clone() else {
            return Err(refused(
                "not_configured",
                "MIMO Random is not set up; send a setup first.",
            ));
        };
        if self.tx.is_some() {
            let phase = lock(&self.shared.published).status.phase;
            return Err(wrong_state(
                "Loading an identification set (it is loaded before the run starts)",
                phase,
            ));
        }
        let set = hertzforge_mimo_random::records::read_identification(path).map_err(|e| {
            MethodError::bad_params(
                "path",
                format!("{e}; choose a folder that holds an identification set"),
            )
        })?;
        let reference = reference_on_grid(&setup).map_err(|e| {
            refused(
                "setup_invalid",
                format!("The reference could not be used: {e}."),
            )
        })?;
        let (cfg, lc) = config_of(
            &setup,
            &reference,
            LawChoice::new(setup.law.id.clone(), Vec::new()),
        )
        .map_err(|e| {
            refused(
                "setup_invalid",
                format!("The setup could not be used: {e}."),
            )
        })?;
        let want = hertzforge_mimo_random::setup_hash_of(&cfg, table, lc.nbins(), lc.df());
        if set.setup_hash != want {
            return Err(refused(
                "identification_mismatch",
                format!(
                    "The identification set in {} was measured on another setup (hash {}, this \
                     setup {want}): the channels, their sensitivities, the transformations or \
                     the grid differ. Run the identification again, or restore the setup it was \
                     measured on.",
                    path.display(),
                    set.setup_hash
                ),
            )
            .with_detail(json!({"setup_hash": want, "identification_hash": set.setup_hash})));
        }
        self.pending_ident = Some(path.to_path_buf());
        lock(&self.shared.log).push(format!("identification to load from {}", path.display()));
        Ok(info_of(path, &set))
    }

    fn save_identification(&mut self, path: &Path) -> Result<IdentificationInfo, MethodError> {
        let Some(dir) = self.identification_dir() else {
            let phase = lock(&self.shared.published).status.phase;
            return Err(wrong_state(
                "Saving the identification set (none has been measured in this run yet)",
                phase,
            ));
        };
        hertzforge_mimo_random::records::copy_identification(&dir, path)
            .map_err(|e| MethodError::bad_params("path", format!("The set was not saved: {e}.")))?;
        let set = hertzforge_mimo_random::records::read_identification(path)
            .map_err(|e| MethodError::internal(format!("the saved set does not read back: {e}")))?;
        Ok(info_of(path, &set))
    }
}

fn info_of(path: &Path, set: &hertzforge_mimo_random::IdentificationSet) -> IdentificationInfo {
    IdentificationInfo {
        path: path.display().to_string(),
        setup_hash: set.setup_hash.clone(),
        frames: set.frames,
        level_db: set.level_db,
        drive_vrms: set.drive_vrms.clone(),
    }
}

impl Drop for ServiceDriver {
    fn drop(&mut self) {
        // The service dropped the run: anything still driving goes to zero at the
        // next engine block, and the thread ends when the channel closes.
        if self.tx.is_some()
            && !matches!(
                lock(&self.shared.published).status.state,
                State::Finished | State::Configured
            )
        {
            let _ = self.shared.signal.fetch_max(ABORT, Ordering::SeqCst);
        }
    }
}

// ---- the quantities --------------------------------------------------------------------------

fn lines_of(s: hertzforge_mimo_random::Series, names: &[(&str, &str, Role)]) -> Series {
    let freqs = Arc::new(s.x);
    let curves = s
        .traces
        .into_iter()
        .map(|t| {
            let (name, role) = names
                .iter()
                .find(|(from, _, _)| *from == t.label)
                .map(|(_, to, r)| ((*to).to_owned(), *r))
                .unwrap_or((t.label, Role::Measured));
            Curve { name, role, y: t.y }
        })
        .collect();
    Series::Lines { freqs, curves }
}

fn rows_of(s: hertzforge_mimo_random::Series, rows: &[usize]) -> Option<Series> {
    let all = s.traces;
    let picked = rows
        .iter()
        .map(|i| all.get(*i).map(|t| t.y.clone()))
        .collect::<Option<Vec<_>>>()?;
    Some(Series::Rows {
        freqs: Arc::new(s.x),
        rows: picked,
    })
}

const PAIR_NAMES: &[(&str, &str, Role)] = &[
    ("measured", "Measured", Role::Measured),
    ("reference", "Reference", Role::Reference),
];

/// One wire quantity from a snapshot of the service's.
pub fn quantity_of(
    snap: &QuantitySnapshot,
    setup: &MimoSetup,
    q: &QuantitySelector,
) -> Option<Series> {
    let get = |m: MimoQuantity| snap.quantity(&m);
    match q {
        QuantitySelector::Psd(i) => {
            let reference = get(MimoQuantity::Ref(*i))?;
            let mut out = Vec::new();
            if let Some(m) = get(MimoQuantity::Psd(*i)) {
                out.extend(m.traces.into_iter().map(|t| Curve {
                    name: "Control".into(),
                    role: Role::Measured,
                    y: t.y,
                }));
            }
            let names = [
                ("reference", "Reference", Role::Reference),
                ("alarm upper", "Alarm high", Role::Tolerance),
                ("alarm lower", "Alarm low", Role::Tolerance),
                ("abort upper", "Abort high", Role::Abort),
                ("abort lower", "Abort low", Role::Abort),
            ];
            let Series::Lines { freqs, curves } = lines_of(reference, &names) else {
                return None;
            };
            out.extend(curves);
            Some(Series::Lines { freqs, curves: out })
        }
        QuantitySelector::Coh(i, j) => Some(lines_of(get(MimoQuantity::Coh(*i, *j))?, PAIR_NAMES)),
        QuantitySelector::Phase(i, j) => {
            Some(lines_of(get(MimoQuantity::Phase(*i, *j))?, PAIR_NAMES))
        }
        QuantitySelector::CsdMag(i, j) => {
            Some(lines_of(get(MimoQuantity::CsdMag(*i, *j))?, PAIR_NAMES))
        }
        QuantitySelector::ErrorDb(rows) => rows_of(get(MimoQuantity::ErrorDb)?, rows),
        QuantitySelector::CohError(rows) => rows_of(get(MimoQuantity::CohError)?, rows),
        QuantitySelector::DrivePsd(k) => Some(lines_of(
            get(MimoQuantity::DrivePsd(*k))?,
            &[("drive", "Drive", Role::Drive)],
        )),
        QuantitySelector::DriveCoh(k, l) => Some(lines_of(
            get(MimoQuantity::DriveCoh(*k, *l))?,
            &[("drive", "Coherence", Role::Measured)],
        )),
        QuantitySelector::Frf(i, k) => Some(lines_of(
            get(MimoQuantity::Frf(*i, *k))?,
            &[
                ("magnitude", "Magnitude", Role::Measured),
                ("phase", "Phase", Role::Measured),
            ],
        )),
        QuantitySelector::Sv => {
            let s = get(MimoQuantity::Sv)?;
            let freqs = Arc::new(s.x);
            let curves = s
                .traces
                .into_iter()
                .enumerate()
                .map(|(j, t)| Curve {
                    name: format!("Singular value {}", j + 1),
                    role: Role::Measured,
                    y: t.y,
                })
                .collect();
            Some(Series::Lines { freqs, curves })
        }
        QuantitySelector::Cond => {
            let s = get(MimoQuantity::Cond)?;
            let n = s.x.len();
            let Series::Lines { freqs, mut curves } =
                lines_of(s, &[("condition", "Condition number", Role::Measured)])
            else {
                return None;
            };
            curves.push(Curve {
                name: "Gate".into(),
                role: Role::Limit,
                y: vec![setup.identification.condition_limit; n],
            });
            Some(Series::Lines { freqs, curves })
        }
        QuantitySelector::Mcoh(i) => Some(lines_of(
            get(MimoQuantity::Mcoh(*i))?,
            &[("multiple coherence", "Multiple coherence", Role::Measured)],
        )),
        QuantitySelector::Trace => Some(lines_of(
            get(MimoQuantity::Trace)?,
            &[
                ("measured", "Control", Role::Measured),
                ("reference", "Reference", Role::Reference),
            ],
        )),
        QuantitySelector::PredErrorDb(i) => {
            let s = get(MimoQuantity::PredErrorDb(*i))?;
            let e = s.traces.first()?.y.first().copied()?;
            // The prediction is one rms error per control; it is drawn flat across
            // the band.
            let r = get(MimoQuantity::Ref(*i))?;
            let n = r.x.len();
            Some(Series::Lines {
                freqs: Arc::new(r.x),
                curves: vec![Curve {
                    name: "Predicted rms error".into(),
                    role: Role::Measured,
                    y: vec![e; n],
                }],
            })
        }
        QuantitySelector::LawDiag(key, index) => {
            let s = get(MimoQuantity::LawDiagnostic {
                key: key.clone(),
                index: *index,
            })?;
            if s.edges.is_empty() {
                return Some(Series::Scalar(*s.traces.first()?.y.first()?));
            }
            Some(lines_of(s, &[]))
        }
        QuantitySelector::SdmBlock(idx) => sdm_block(snap, idx),
        QuantitySelector::PredSdmBlock(idx) => achievable_block(snap, idx),
    }
}

/// A quantity on the setup's positions after controls were excluded at the gate:
/// each position is mapped to the run's; an excluded control has no curve of its
/// own (nothing is shown), and its heatmap row is empty (NaN).
fn quantity_kept(
    snap: &QuantitySnapshot,
    setup: &MimoSetup,
    q: &QuantitySelector,
    kept: &[usize],
) -> Option<Series> {
    use QuantitySelector as Q;
    let p = |i: &usize| kept.iter().position(|k| k == i);
    let all = |b: &[usize]| b.iter().map(p).collect::<Option<Vec<_>>>();
    let rows = |rows: &[usize], make: fn(Vec<usize>) -> Q| -> Option<Series> {
        let mapped: Vec<Option<usize>> = rows.iter().map(p).collect();
        let present: Vec<usize> = mapped.iter().flatten().copied().collect();
        let Series::Rows { freqs, rows: got } = quantity_of(snap, setup, &make(present))? else {
            return None;
        };
        let width = freqs.len();
        let mut it = got.into_iter();
        let out = mapped
            .iter()
            .map(|m| match m {
                Some(_) => it.next().unwrap_or_else(|| vec![f64::NAN; width]),
                None => vec![f64::NAN; width],
            })
            .collect();
        Some(Series::Rows { freqs, rows: out })
    };
    let mapped = match q {
        Q::Psd(i) => Q::Psd(p(i)?),
        Q::Coh(i, j) => Q::Coh(p(i)?, p(j)?),
        Q::Phase(i, j) => Q::Phase(p(i)?, p(j)?),
        Q::CsdMag(i, j) => Q::CsdMag(p(i)?, p(j)?),
        Q::Frf(i, k) => Q::Frf(p(i)?, *k),
        Q::Mcoh(i) => Q::Mcoh(p(i)?),
        Q::PredErrorDb(i) => Q::PredErrorDb(p(i)?),
        Q::SdmBlock(b) => Q::SdmBlock(all(b)?),
        Q::PredSdmBlock(b) => Q::PredSdmBlock(all(b)?),
        Q::ErrorDb(r) => return rows(r, Q::ErrorDb),
        Q::CohError(r) => return rows(r, Q::CohError),
        other => other.clone(),
    };
    quantity_of(snap, setup, &mapped)
}

/// A feasibility verdict as the wire names it.
fn verdict_name(v: &str) -> &'static str {
    match v {
        "Pass" => "pass",
        "Advisory" => "advisory",
        "Exceeded" => "exceeded",
        _ => "not_evaluated",
    }
}

/// The packed block `idx` of `g`, on the band's lines, scaled by `p`.
fn packed_of(g: &HermitianStack, idx: &[usize], band: (usize, usize), p: f64) -> PackedBlock {
    let m = idx.len();
    let per = HermitianStack::packed_len(m);
    let (lo, hi) = band;
    let mut b = PackedBlock {
        n: m,
        re: Vec::with_capacity((hi - lo) * per),
        im: Vec::with_capacity((hi - lo) * per),
    };
    for line in lo..hi {
        for (r, i) in idx.iter().enumerate() {
            for j in &idx[r..] {
                let v = g.get(line, *i, *j) * p;
                b.re.push(v.re);
                b.im.push(v.im);
            }
        }
    }
    b
}

/// The achievable reference's block `idx` at full level, with the reference's:
/// what the drives can reach of the reference when controls outnumber them.
fn achievable_block(snap: &QuantitySnapshot, idx: &[usize]) -> Option<Series> {
    let ach = snap.achievable()?;
    let reference = snap.reference();
    if idx.is_empty() || idx.iter().any(|i| *i >= reference.n()) {
        return None;
    }
    let band = snap.band();
    Some(Series::Packed {
        freqs: Arc::new((band.0..band.1).map(|l| l as f64 * snap.df_hz()).collect()),
        measured: packed_of(ach, idx, band, 1.0),
        reference: Some(packed_of(reference, idx, band, 1.0)),
    })
}

fn sdm_block(snap: &QuantitySnapshot, idx: &[usize]) -> Option<Series> {
    let reference = snap.reference();
    let n = reference.n();
    if idx.is_empty() || idx.iter().any(|i| *i >= n) {
        return None;
    }
    let (lo, hi) = snap.band();
    let p = if snap.level_db().is_finite() {
        10f64.powf(snap.level_db() / 10.0)
    } else {
        1.0
    };
    let m = idx.len();
    let per = HermitianStack::packed_len(m);
    let block = |g: &HermitianStack| {
        let mut b = PackedBlock {
            n: m,
            re: Vec::with_capacity((hi - lo) * per),
            im: Vec::with_capacity((hi - lo) * per),
        };
        for line in lo..hi {
            for (r, i) in idx.iter().enumerate() {
                for j in &idx[r..] {
                    let v = g.get(line, *i, *j) * p;
                    b.re.push(v.re);
                    b.im.push(v.im);
                }
            }
        }
        b
    };
    let measured = snap.response_full().map(block).unwrap_or(PackedBlock {
        n: m,
        re: Vec::new(),
        im: Vec::new(),
    });
    Some(Series::Packed {
        freqs: Arc::new((lo..hi).map(|l| l as f64 * snap.df_hz()).collect()),
        measured,
        reference: Some(block(reference)),
    })
}

// ---- the run thread --------------------------------------------------------------------------

struct Actor {
    opts: AdapterOptions,
    setup: Arc<MimoSetup>,
    law: LawRef,
    shared: Arc<Shared>,
    svc: Option<MimoRandomService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    /// Hold for the operator after identification (the setup's `review`).
    review: bool,
    ended: bool,
    override_ack: Vec<OverrideAck>,
    /// Per drive, the smallest cap factor of the run.
    min_cap: Vec<f64>,
    faults: Vec<Fault>,
    report: Option<MimoReport>,
    /// An identification set to load once armed.
    pending_ident: Option<PathBuf>,
    /// The setup's control positions the run still controls, after some were
    /// excluded at the gate (`None`: every one).
    kept: Option<Vec<usize>>,
}

/// A service call that panicked: the run ends with the drive at zero.
struct Panicked;

impl Actor {
    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            if self.has_work() {
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => self.handle(cmd),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return self.abandon(),
                    }
                }
                if self.has_work() {
                    self.step();
                    self.publish();
                }
            } else {
                match rx.recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(_) => return self.abandon(),
                }
            }
        }
    }

    /// The service dropped this run: the drive to zero, everything released.
    fn abandon(&mut self) {
        if self.svc.is_some() {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        self.svc = None;
        self.lock = None;
    }

    /// Call the service, contained. A panic ends the run: the station signal goes to
    /// abort, the service (and with it the station, whose wrapper takes the drive to
    /// zero) is dropped.
    fn call<T>(
        &mut self,
        f: impl FnOnce(&mut MimoRandomService) -> T,
    ) -> Result<Option<T>, Panicked> {
        let Some(svc) = self.svc.as_mut() else {
            return Ok(None);
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(svc))) {
            Ok(v) => Ok(Some(v)),
            Err(_) => {
                self.shared.signal.store(ABORT, Ordering::SeqCst);
                let svc = self.svc.take();
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(svc)));
                self.lock = None;
                self.finish_now(
                    "fault",
                    "The run failed while computing; the drive was taken to zero.".into(),
                    None,
                );
                Err(Panicked)
            }
        }
    }

    fn svc_state(&self) -> Option<MimoState> {
        self.svc.as_ref().map(|s| s.state())
    }

    fn has_work(&self) -> bool {
        if self.ended {
            return false;
        }
        let Some(svc) = self.svc.as_ref() else {
            return false;
        };
        match svc.state() {
            MimoState::NoiseFloor
            | MimoState::SysId
            | MimoState::Engaged
            | MimoState::FullLevel
            | MimoState::Stopped
            | MimoState::Aborted => true,
            MimoState::Gate => svc.gate().is_some_and(|g| g.passed),
            MimoState::Predict => !self.review && !self.prediction_breach(),
            MimoState::Idle | MimoState::Feasibility => false,
        }
    }

    fn prediction_breach(&self) -> bool {
        self.svc
            .as_ref()
            .and_then(|s| s.prediction())
            .is_none_or(|p| !p.breaches.is_empty())
    }

    fn interrupted(&self) -> bool {
        self.shared.signal.load(Ordering::SeqCst) != 0
    }

    /// One step of the run in its present state.
    fn step(&mut self) {
        let Some(state) = self.svc_state() else {
            return;
        };
        let r: Result<Option<Result<(), MimoError>>, Panicked> = match state {
            MimoState::NoiseFloor => self.call(|s| s.run_noise_floor()),
            MimoState::SysId => self.call(|s| s.run_identification()),
            MimoState::Gate | MimoState::Predict => self.call(|s| s.proceed()),
            MimoState::Engaged | MimoState::FullLevel => self.call(|s| s.loop_tick().map(|_| ())),
            MimoState::Stopped | MimoState::Aborted => {
                self.finish_from_service();
                return;
            }
            _ => return,
        };
        let Ok(Some(r)) = r else { return };
        if let Err(e) = r {
            self.after_error(e);
        }
        if self.svc_state().is_some_and(|s| s.is_terminal()) {
            self.finish_from_service();
        }
    }

    /// A service call failed: the operator's stop or abort reached the station, or
    /// the run cannot go on. Either way it ends now, the drive taken down.
    fn after_error(&mut self, e: MimoError) {
        let requested = self.interrupted() || lock(&self.shared.end_request).is_some();
        let words = e.to_string();
        let _ = self.call(|s| {
            if !s.state().is_terminal() {
                s.abort_now(&words);
            }
        });
        if requested {
            return; // finish_from_service reports the operator's request
        }
        match e {
            MimoError::IdentUnreachable(m) => self.finish_now(
                "sysid_level_unreachable",
                format!("Identification could not reach its level: {m}."),
                None,
            ),
            other => self.finish_now("fault", format!("The run stopped: {other}."), None),
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Start { ctx, reply } => {
                let r = self.start(&ctx);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Proceed { reply } => {
                let r = self.proceed_cmd();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Override {
                ack,
                by,
                reason,
                reply,
            } => {
                let r = self.override_cmd(&ack, &by, reason);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Exclude {
                positions,
                ack,
                by,
                reply,
            } => {
                let r = self.exclude_cmd(&positions, &ack, &by);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Iterate { law, reply } => {
                let r = self.iterate_cmd(law);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Stop => {
                let st = self.svc_state();
                match st {
                    Some(MimoState::Engaged | MimoState::FullLevel) => {
                        let _ = self.call(|s| s.request_stop());
                    }
                    Some(s) if !s.is_terminal() => {
                        let _ = self.call(|s| s.stop());
                    }
                    _ => {}
                }
                if self.svc_state().is_some_and(|s| s.is_terminal()) {
                    self.finish_from_service();
                }
                self.publish();
            }
            Cmd::Abort { by } => {
                let _ = self.call(|s| {
                    if !s.state().is_terminal() {
                        s.abort_now(&format!("aborted by {by}"));
                    }
                });
                if self.svc.is_some() {
                    self.finish_from_service();
                }
                self.publish();
            }
        }
    }

    fn start(&mut self, ctx: &StartContext) -> Result<(), MethodError> {
        if self.svc.is_some() || self.ended {
            return Err(wrong_state("Starting", "running"));
        }
        let setup = self.setup.clone();
        let reference = reference_on_grid(&setup).map_err(|e| {
            refused(
                "arm_refused",
                format!("The reference could not be used: {e}."),
            )
        })?;
        let (choice, factory, law_file) = resolve_law(
            &setup.law.id,
            &setup.law.params,
            setup.controlled(),
            &self.opts,
        )
        .map_err(|e| refused("arm_refused", format!("The law could not be used: {e}.")))?;
        // Nothing opens on hardware with a law that has not passed its check.
        let entry = if ctx.kind == "sim" {
            None
        } else {
            catalog_entry(&setup.law.id, &self.opts)
        };
        hardware_law_gate(ctx.kind, &setup.law.id, entry.as_ref())?;
        let (cfg, loop_cfg) = config_of(&setup, &reference, choice)
            .map_err(|e| refused("arm_refused", format!("The setup could not be used: {e}.")))?;
        let table = ctx.table.clone().ok_or_else(|| {
            MethodError::internal("the MIMO run was started without the channel table")
        })?;
        // The station.
        let inputs = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count();
        let drives = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        let fs = setup.estimation.fs_hz;
        let built = if ctx.kind == "sim" {
            let plant = sim_plant_for(inputs, drives).ok_or_else(|| {
                refused(
                    "arm_refused",
                    format!(
                        "The simulator plays a standard plant of 4 by 4, 12 by 6, 24 by 24 or \
                         72 by 24 (inputs by drives), and this table has {inputs} inputs and \
                         {drives} drives; enable that many rows to run on the simulator."
                    ),
                )
            })?;
            let spec = StationSpec {
                sim: SimPlantSpec {
                    sample_rate_hz: fs,
                    noise_level: 1e-4,
                    shared_clock: true,
                    seed: 0x4D49_4D4F,
                    max_input_channels: inputs as u32,
                    max_output_channels: drives as u32,
                    plant: SimPlant::Modal(plant),
                    ..SimPlantSpec::default()
                },
                ..StationSpec::default()
            };
            hertzforge_station::try_build_station_backend(BackendKind::Sim, &spec)
                .map_err(station::build_refusal)?
        } else {
            let (ins, outs) = hertzforge_station::physical_from_table(&table);
            let plan = StationPlan::Ni {
                devices: ctx.ni_devices.clone(),
                inputs: ins,
                outputs: outs,
                fs_hz: fs,
            };
            station::build(&plan, &*self.opts.scanner)?
        };
        let hardware = built.station.is_hardware();
        if hardware {
            self.lock = Some(
                hertzforge_station::lock::acquire(
                    &self.opts.data_root,
                    &self.opts.bind,
                    &built.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => refused(
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    ),
                    hertzforge_station::lock::LockError::Io(m) => {
                        refused("arm_refused", format!("The station refused to arm: {m}"))
                    }
                })?,
            );
        }
        let pace = (!hardware && self.opts.sim_speed > 0.0).then_some((fs, self.opts.sim_speed));
        let backend = RunStation::wrap(
            Interruptible::wrap(built.backend, self.shared.signal.clone()),
            pace,
        );
        let mut loop_cfg = loop_cfg;
        if !hardware {
            loop_cfg.block_frames = 256;
        }
        let records = self
            .opts
            .capture_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root);
        let mut svc = MimoRandomService::new(cfg, table, loop_cfg, backend, built.station, records)
            .map_err(|e| {
                self.lock = None;
                refused("arm_refused", format!("The run could not be built: {e}"))
            })?;
        if let Some(f) = self.opts.law_factory.clone().or(factory) {
            svc = svc.with_law_factory(f);
        }
        if self.opts.law_factory.is_none() {
            svc.set_law_file(law_file);
        }
        self.review = setup.identification.review;
        *lock(&self.shared.ramp) = Some(svc.drive_ramp());
        *lock(&self.shared.started) = Some(Instant::now());
        self.svc = Some(svc);
        let pending = self.pending_ident.clone();
        match self.call(|s| s.start()) {
            Ok(Some(Ok(()))) => {
                lock(&self.shared.log).push(format!("started: {}", reference.source));
                if let Some(dir) = pending {
                    let loaded = self.load_cmd(&dir).and_then(|()| {
                        // Straight on to the prediction when the loaded set passes
                        // its gates, so the start answers at the review hold.
                        let passed = self
                            .svc
                            .as_ref()
                            .and_then(|s| s.gate())
                            .is_some_and(|g| g.passed);
                        match self.call(|s| if passed { s.proceed() } else { Ok(()) }) {
                            Ok(Some(Ok(()))) => Ok(()),
                            Ok(Some(Err(e))) => Err(refused(
                                "arm_refused",
                                format!("The law could not be seeded on the loaded set: {e}."),
                            )),
                            _ => Err(MethodError::internal("the run failed while predicting")),
                        }
                    });
                    if let Err(e) = loaded {
                        let _ = self.call(|s| s.stop());
                        self.svc = None;
                        self.lock = None;
                        *lock(&self.shared.started) = None;
                        *lock(&self.shared.ramp) = None;
                        return Err(e);
                    }
                }
                Ok(())
            }
            Ok(Some(Err(e))) => {
                let words = e.to_string();
                let blocked = matches!(e, MimoError::Blocked(_));
                let rows = self
                    .svc
                    .as_ref()
                    .map(|s| s.feasibility_rows_now())
                    .unwrap_or_default();
                let _ = self.call(|s| s.abort_now("not armed"));
                self.svc = None;
                self.lock = None;
                *lock(&self.shared.started) = None;
                if blocked {
                    let which: Vec<String> = rows
                        .iter()
                        .filter(|r| r.verdict == "Exceeded")
                        .map(|r| format!("channel {} ({})", r.channel_index, r.detail))
                        .collect();
                    return Err(refused(
                        "feasibility_exceeded",
                        format!(
                            "The reference would exceed the shaker on {}; lower the reference, \
                             correct the shaker record, the moving mass or the article mass, or \
                             remove the shaker record to run without the check.",
                            which.join(", ")
                        ),
                    ));
                }
                Err(refused(
                    "arm_refused",
                    format!("The station refused to arm: {words}"),
                ))
            }
            _ => Err(MethodError::internal("the run failed while arming")),
        }
    }

    fn review_phase(&self) -> &'static str {
        self.publish_state().1
    }

    fn proceed_cmd(&mut self) -> Result<(), MethodError> {
        match self.svc_state() {
            Some(MimoState::Predict) => {}
            Some(MimoState::Gate) => {
                return Err(refused(
                    "gate_failed",
                    "Identification failed a gate, so the run holds; override with your reason \
                     to engage anyway, or stop and correct the setup.",
                ))
            }
            _ => return Err(wrong_state("Proceeding", self.review_phase())),
        }
        if self.prediction_breach() {
            return Err(refused(
                "prediction_exceeded",
                "The prediction puts a drive over its ceiling, so the run holds; change the law \
                 or its parameters, or override with your reason (the ceilings still hold the \
                 drive).",
            ));
        }
        self.engage()
    }

    fn engage(&mut self) -> Result<(), MethodError> {
        match self.call(|s| s.proceed()) {
            Ok(Some(Ok(()))) => Ok(()),
            Ok(Some(Err(e))) => Err(refused(
                "wrong_state",
                format!("The run did not engage: {e}."),
            )),
            _ => Err(MethodError::internal("the run failed while engaging")),
        }
    }

    fn override_cmd(
        &mut self,
        ack: &str,
        by: &str,
        reason: Option<OverrideReason>,
    ) -> Result<(), MethodError> {
        let st = self.svc_state();
        let gate_failed = st == Some(MimoState::Gate) && !self.gate_passed();
        let breach = st == Some(MimoState::Predict) && self.prediction_breach();
        let holds = if gate_failed {
            Some(OverrideReason::Gate)
        } else if breach {
            Some(OverrideReason::Prediction)
        } else {
            None
        };
        let Some(holds) = holds else {
            return match st {
                Some(MimoState::Predict) | Some(MimoState::Gate) => Err(refused(
                    "override_not_needed",
                    "Identification passed its gates and the prediction is within every \
                     ceiling; proceed instead.",
                )),
                _ => Err(wrong_state("Overriding", self.review_phase())),
            };
        };
        if let Some(r) = reason {
            if r != holds {
                let what = match holds {
                    OverrideReason::Gate => "a failed identification gate",
                    OverrideReason::Prediction => "a prediction over a drive ceiling",
                };
                return Err(refused(
                    "override_not_needed",
                    format!(
                        "The run holds for {what}, not for what this override names; read the \
                         Predict page and acknowledge what holds the run (reason \"{}\").",
                        holds.name()
                    ),
                ));
            }
        }
        let ack2 = ack.to_owned();
        let r = self.call(move |s| -> Result<(), MimoError> {
            s.override_with(&ack2)?;
            match holds {
                // Past the gate to the prediction; it engages only when the
                // prediction is within every ceiling.
                OverrideReason::Gate => {
                    s.proceed()?;
                    if s.state() == MimoState::Predict
                        && s.prediction().is_some_and(|p| p.breaches.is_empty())
                    {
                        s.proceed()?;
                    }
                }
                OverrideReason::Prediction => s.proceed()?,
            }
            Ok(())
        });
        self.override_ack.push(OverrideAck {
            by: by.to_owned(),
            text: ack.to_owned(),
            reason: holds,
        });
        lock(&self.shared.log).push(format!("override ({}) by {by}: {ack}", holds.name()));
        match r {
            Ok(Some(Ok(()))) => Ok(()),
            Ok(Some(Err(e))) => Err(refused(
                "gate_failed",
                format!("The run did not go on after the override: {e}."),
            )),
            _ => Err(MethodError::internal("the run failed while engaging")),
        }
    }

    /// The setup's control positions, in order, that the run controls now.
    fn kept_positions(&self) -> Vec<usize> {
        self.kept
            .clone()
            .unwrap_or_else(|| (0..self.setup.controlled()).collect())
    }

    /// The setup's positions excluded at the gate.
    fn excluded_positions(&self) -> Vec<usize> {
        let kept = self.kept_positions();
        (0..self.setup.controlled())
            .filter(|i| !kept.contains(i))
            .collect()
    }

    /// A per-control list of the run's, laid out on the setup's positions: an
    /// excluded control's entry is NaN (null on the wire).
    fn on_setup_positions(&self, v: &[f64]) -> Vec<f64> {
        let Some(kept) = self.kept.as_ref() else {
            return v.to_vec();
        };
        let mut out = vec![f64::NAN; self.setup.controlled()];
        for (k, &i) in kept.iter().enumerate() {
            if let (Some(o), Some(x)) = (out.get_mut(i), v.get(k)) {
                *o = *x;
            }
        }
        out
    }

    fn exclude_cmd(&mut self, positions: &[usize], ack: &str, by: &str) -> Result<(), MethodError> {
        let st = self.svc_state();
        if st != Some(MimoState::Gate) || self.gate_passed() {
            return Err(refused(
                "wrong_state",
                "A control is excluded when identification has failed a gate and the run holds \
                 for it; it is not offered now.",
            ));
        }
        let n = self.setup.controlled();
        if let Some(bad) = positions.iter().find(|&&i| i >= n) {
            return Err(MethodError::bad_params(
                "controls",
                format!("there is no control at position {bad}; the setup has {n} controls"),
            ));
        }
        let kept = self.kept_positions();
        let mut svc_pos = Vec::new();
        for &i in positions {
            match kept.iter().position(|&k| k == i) {
                Some(k) => svc_pos.push(k),
                None => {
                    return Err(MethodError::bad_params(
                        "controls",
                        format!("the control at position {i} is already excluded"),
                    ))
                }
            }
        }
        let ack2 = ack.to_owned();
        match self.call(move |s| s.exclude_controls(&svc_pos, &ack2)) {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(e))) => {
                return Err(refused(
                    "wrong_state",
                    format!("The control was not excluded: {e}."),
                ))
            }
            _ => return Err(MethodError::internal("the run failed while excluding")),
        }
        let keep: Vec<usize> = kept
            .into_iter()
            .filter(|i| !positions.contains(i))
            .collect();
        self.kept = Some(keep);
        let labels = self.setup.controlled_labels();
        let names: Vec<String> = positions
            .iter()
            .map(|&i| {
                labels
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("control {i}"))
            })
            .collect();
        lock(&self.shared.log).push(format!(
            "excluded at the gate by {by}: {} ({ack})",
            names.join(", ")
        ));
        Ok(())
    }

    fn gate_passed(&self) -> bool {
        self.svc
            .as_ref()
            .and_then(|s| s.gate())
            .is_some_and(|g| g.passed)
    }

    fn iterate_cmd(
        &mut self,
        law: Option<(LawRef, Map<String, Value>)>,
    ) -> Result<Prediction, MethodError> {
        if self.svc_state() != Some(MimoState::Predict) {
            return Err(wrong_state("A new prediction", self.review_phase()));
        }
        let r = match law {
            Some((lr, params)) => {
                let (choice, factory, law_file) =
                    resolve_law(&lr.id, &params, self.setup.controlled(), &self.opts)
                        .map_err(|e| MethodError::bad_params("law", e))?;
                let r = self.call(move |s| {
                    s.set_law_file(law_file);
                    s.change_law(choice, factory)
                });
                if matches!(r, Ok(Some(Ok(())))) {
                    self.law = lr;
                }
                r
            }
            None => self.call(|s| s.iterate_prediction()),
        };
        match r {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(e))) => return Err(MethodError::bad_params("law", e.to_string())),
            _ => return Err(MethodError::internal("the run failed while predicting")),
        }
        self.prediction()
            .ok_or_else(|| MethodError::internal("the run made no prediction"))
    }

    fn load_cmd(&mut self, dir: &Path) -> Result<(), MethodError> {
        if self.svc_state() != Some(MimoState::NoiseFloor) {
            return Err(refused(
                "wrong_state",
                "An identification set is loaded right after the run starts, before the noise \
                 floor is measured; start the run and load it then.",
            ));
        }
        let set = hertzforge_mimo_random::records::read_identification(dir).map_err(|e| {
            MethodError::bad_params(
                "path",
                format!("{e}; choose a run folder that holds an identification set"),
            )
        })?;
        match self.call(move |s| s.load_identification(set)) {
            Ok(Some(Ok(()))) => {
                lock(&self.shared.log)
                    .push(format!("identification loaded from {}", dir.display()));
                Ok(())
            }
            Ok(Some(Err(e))) => Err(refused(
                "identification_mismatch",
                format!("The identification set was not loaded: {e}."),
            )),
            _ => Err(MethodError::internal("the run failed while loading")),
        }
    }

    fn prediction(&self) -> Option<Prediction> {
        let p = self.svc.as_ref()?.prediction()?;
        Some(Prediction {
            iteration: p.iterations + 1,
            law: self.law.clone(),
            drive_rms_v: p.drive_rms.clone(),
            ceiling_vrms: p.ceilings.clone(),
            error_db: self.on_setup_positions(&p.error_db),
            has_achievable: p.has_achievable,
            unreachable_db: self.on_setup_positions(&p.unreachable_db),
            drive_accel: p.drive_accel_rms.clone(),
            accel_control: {
                let kept = self.kept_positions();
                p.accel_control
                    .iter()
                    .map(|&i| kept.get(i).copied().unwrap_or(i))
                    .collect()
            },
            feasibility: p
                .feasibility
                .iter()
                .map(|r| DriveVerdict {
                    verdict: verdict_name(&r.verdict),
                    message: r.detail.clone(),
                })
                .collect(),
        })
    }

    fn gates(&self) -> Option<Gates> {
        let g = self.svc.as_ref()?.gate()?;
        let coherence_min = g
            .coherence
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min)
            .clamp(0.0, 1.0);
        let condition_max = if g.worst_condition.is_finite() {
            g.worst_condition.max(1.0)
        } else {
            f64::MAX
        };
        Some(Gates {
            coherence_min,
            coherence_floor: g.coherence_threshold,
            condition_max,
            condition_limit: g.condition_threshold.max(1.0),
            coherence: self.on_setup_positions(&g.coherence),
        })
    }

    /// The wire state and phase of the service's state.
    fn publish_state(&self) -> (State, &'static str) {
        let Some(svc) = self.svc.as_ref() else {
            return if self.ended {
                (State::Finished, "stopped")
            } else {
                (State::Configured, "idle")
            };
        };
        match svc.state() {
            MimoState::Idle | MimoState::Feasibility => (State::Armed, "armed"),
            MimoState::NoiseFloor => (State::Running, "sysid_noise"),
            MimoState::SysId => (State::Running, "sysid"),
            MimoState::Gate => {
                if svc.gate().is_some_and(|g| g.passed) {
                    (State::Running, "sysid")
                } else {
                    (State::Review, "gate_failed")
                }
            }
            MimoState::Predict => {
                if self.prediction_breach() {
                    (State::Review, "prediction_exceeded")
                } else if self.review {
                    (State::Review, "awaiting_review")
                } else {
                    (State::Running, "sysid")
                }
            }
            MimoState::Engaged => {
                if svc.stopping() {
                    (State::Stopping, "ramping_down")
                } else {
                    (State::Running, "engaged")
                }
            }
            MimoState::FullLevel => {
                if svc.stopping() {
                    (State::Stopping, "ramping_down")
                } else {
                    (State::Running, "full_level")
                }
            }
            MimoState::Stopped => (State::Stopping, "ramping_down"),
            MimoState::Aborted => (State::Stopping, "ramping_down"),
        }
    }

    fn publish(&mut self) {
        let mut p = lock(&self.shared.published);
        if self.ended {
            if let Some(r) = self.report.clone() {
                p.report = Some(r);
            }
            p.status.law = self.law.clone();
            p.status.override_ack = self.override_ack.clone();
            p.status.faults = self.faults.clone();
            return;
        }
        let Some(svc) = self.svc.as_ref() else {
            p.status.law = self.law.clone();
            return;
        };
        let (state, phase) = self.publish_state();
        let caps = svc.drive_caps();
        if !caps.is_empty() {
            if self.min_cap.len() != caps.len() {
                self.min_cap = caps.clone();
            }
            for (m, c) in self.min_cap.iter_mut().zip(&caps) {
                *m = m.min(*c);
            }
        }
        let error = svc.error_db().to_vec();
        let kept = self.kept_positions();
        let alarm = error.iter().enumerate().any(|(i, e)| {
            let a = self.setup.band_of(kept.get(i).copied().unwrap_or(i)).0;
            e.is_finite() && *e > a[0].min(a[1])
        });
        let level = svc.level_db();
        let driving = matches!(svc.state(), MimoState::Engaged | MimoState::FullLevel);
        let mut faults = self.faults.clone();
        faults.extend(svc.status().faults.into_iter().map(|m| Fault {
            kind: "records",
            message: m,
        }));
        let st = MimoStatus {
            state,
            phase,
            elapsed_s: None,
            level_db: (driving && level.is_finite()).then_some(level),
            at_level_s: driving.then(|| svc.elapsed_at_level_s()),
            remaining_at_level_s: svc.remaining_at_full_level_s(),
            loop_index: svc.loop_count(),
            alarm,
            control_error_db: self.on_setup_positions(
                &error
                    .iter()
                    .map(|e| if e.is_finite() { *e } else { 0.0 })
                    .collect::<Vec<_>>(),
            ),
            gates: self.gates(),
            caps,
            drive_rms_v: svc.drive_rms().to_vec(),
            law: self.law.clone(),
            faults,
            override_ack: self.override_ack.clone(),
            prediction: self.prediction(),
            excluded: self.excluded_positions(),
            end: None,
            streaming: true,
        };
        let ident_dir = svc.identification().and(svc.records_dir());
        drop(p);
        let snapshot = Arc::new(svc.quantity_snapshot());
        let mut p = lock(&self.shared.published);
        p.status = st;
        p.snapshot = Some(snapshot);
        p.kept = self.kept.clone();
        if ident_dir.is_some() {
            p.ident_dir = ident_dir;
        }
    }

    /// The service has ended (stopped or aborted): say how, and release the station.
    fn finish_from_service(&mut self) {
        if self.ended {
            return;
        }
        let Some(svc) = self.svc.as_ref() else {
            return;
        };
        let state = svc.state();
        let trip = svc.trip().cloned();
        let request = lock(&self.shared.end_request).clone();
        let level = svc.level_db();
        let (outcome, message, trip_json, level_db) = match (&trip, &request) {
            (Some(t), _) if matches!(t.kind, MimoTripKind::LawFault { .. }) => (
                "law_fault",
                t.kind.words(),
                Some(trip_json(t, &self.setup)),
                Some(t.at_level_db).filter(|x| x.is_finite()),
            ),
            (_, Some(r)) => (
                r.outcome,
                match r.outcome {
                    "operator_abort" => format!("Aborted by {}.", r.by),
                    "safe_stop" => "Nobody held control for the timeout, so the service stopped \
                                    the run."
                        .to_owned(),
                    _ => format!("Stopped by {}.", r.by),
                },
                None,
                r.level_db,
            ),
            (Some(t), None) => match &t.kind {
                MimoTripKind::Plant { message } => (
                    "fault",
                    format!("The station faulted: {message}; the drive was taken to zero."),
                    None,
                    Some(t.at_level_db).filter(|x| x.is_finite()),
                ),
                k => (
                    "aborted",
                    k.words(),
                    Some(trip_json(t, &self.setup)),
                    Some(t.at_level_db).filter(|x| x.is_finite()),
                ),
            },
            (None, None) => {
                if state == MimoState::Aborted {
                    ("aborted", "The run was aborted.".to_owned(), None, None)
                } else {
                    (
                        "completed",
                        "The run completed its time at full level and stopped.".to_owned(),
                        None,
                        Some(0.0),
                    )
                }
            }
        };
        let _ = level;
        self.finish_now(outcome, message, trip_json.map(|t| (t, level_db)));
    }

    /// End the run now with `outcome`: the report, the final status, the station
    /// released.
    fn finish_now(
        &mut self,
        outcome: &'static str,
        message: String,
        trip: Option<(Value, Option<f64>)>,
    ) {
        if self.ended {
            return;
        }
        // The last state the service reached, published before it goes.
        if self.svc.is_some() {
            self.publish();
        }
        let elapsed = lock(&self.shared.started).map(|t| t.elapsed().as_secs_f64());
        let request_level = lock(&self.shared.end_request)
            .as_ref()
            .and_then(|r| r.level_db);
        let (trip, level) = match trip {
            Some((t, l)) => (Some(t), l),
            None => (None, request_level),
        };
        let end = End {
            outcome,
            message,
            at_s: elapsed,
            level_db: level,
            trip,
        };
        let phase = if matches!(outcome, "operator_abort" | "aborted" | "fault") {
            "aborted"
        } else {
            "stopped"
        };
        let report = self.svc.as_ref().map(|svc| {
            let snap = svc.quantity_snapshot();
            let (lo, hi) = snap.band();
            let df = snap.df_hz();
            let rms = |g: &HermitianStack, i: usize| {
                ((lo..hi).map(|l| g.get(l, i, i).re.max(0.0)).sum::<f64>() * df).sqrt()
            };
            let n = snap.reference().n();
            MimoReport {
                law: self.law.clone(),
                end: Some(end.clone()),
                loops: svc.loop_count(),
                control_rms: self.on_setup_positions(
                    &(0..n)
                        .map(|i| snap.response_full().map_or(0.0, |g| rms(g, i)))
                        .collect::<Vec<_>>(),
                ),
                reference_rms: self.on_setup_positions(
                    &(0..n).map(|i| rms(snap.reference(), i)).collect::<Vec<_>>(),
                ),
                control_error_db: self.on_setup_positions(svc.error_db()),
                drive_rms_v: svc.drive_rms().to_vec(),
                min_cap: self.min_cap.clone(),
                gates: self.gates(),
                override_ack: self.override_ack.clone(),
            }
        });
        // Release: the service (its station wrapper takes the drive to zero), then
        // the hardware lock.
        let svc = self.svc.take();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(svc)));
        self.lock = None;
        *lock(&self.shared.ramp) = None;
        self.ended = true;
        self.report = report.or_else(|| {
            Some(MimoReport {
                law: self.law.clone(),
                end: Some(end.clone()),
                loops: 0,
                control_rms: Vec::new(),
                reference_rms: Vec::new(),
                control_error_db: Vec::new(),
                drive_rms_v: Vec::new(),
                min_cap: Vec::new(),
                gates: None,
                override_ack: self.override_ack.clone(),
            })
        });
        let mut p = lock(&self.shared.published);
        p.status.state = State::Finished;
        p.status.phase = phase;
        p.status.end = Some(end);
        p.status.elapsed_s = elapsed;
        if !matches!(outcome, "completed") {
            p.status.remaining_at_level_s = None;
        }
        p.report = self.report.clone();
        lock(&self.shared.log).push(format!("finished: {outcome}"));
    }
}

/// A trip as the protocol's `trip` object.
fn trip_json(t: &MimoTripRecord, setup: &MimoSetup) -> Value {
    let at_level_db = if t.at_level_db.is_finite() {
        t.at_level_db
    } else {
        setup.identification.level_db
    };
    let base = |kind: &str| -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("kind".into(), json!(kind));
        m.insert("at_level_db".into(), json!(at_level_db));
        m.insert("at_loop".into(), json!(t.at_loop));
        m
    };
    let mut m;
    match &t.kind {
        MimoTripKind::Envelope {
            control,
            abort_lines,
            worst_db,
        } => {
            m = base("envelope");
            m.insert("abort_lines".into(), json!(abort_lines));
            m.insert("worst_db".into(), json!(worst_db));
            m.insert("control".into(), json!(control));
        }
        MimoTripKind::Limit {
            channel_index,
            over_db,
            ..
        } => {
            m = base("envelope");
            m.insert("abort_lines".into(), json!(1));
            m.insert("worst_db".into(), json!(over_db));
            m.insert("channel".into(), json!(channel_index));
        }
        MimoTripKind::Kinematic {
            channel_index,
            axis,
            value,
            limit,
        } => {
            m = base("kinematic");
            let a = axis.to_ascii_lowercase();
            let axis = if a.starts_with("vel") {
                "velocity"
            } else if a.starts_with("disp") {
                "displacement"
            } else {
                "accel"
            };
            m.insert("channel".into(), json!(channel_index));
            m.insert("axis".into(), json!(axis));
            m.insert("value".into(), json!(value));
            m.insert("limit".into(), json!(limit));
        }
        MimoTripKind::DriveVrms {
            drive,
            channel_index,
            realized_vrms,
            limit_vrms,
        } => {
            m = base("drive_vrms");
            m.insert("realized_vrms".into(), json!(realized_vrms));
            m.insert("limit_vrms".into(), json!(limit_vrms));
            m.insert("drive".into(), json!(drive));
            m.insert("channel".into(), json!(channel_index));
        }
        MimoTripKind::Divergence {
            control,
            consecutive_loops,
            ..
        } => {
            m = base("divergence");
            m.insert("consecutive_loops".into(), json!(consecutive_loops));
            m.insert("divergent_bins".into(), json!(0));
            m.insert("control".into(), json!(control));
        }
        MimoTripKind::CapDivergence {
            drive,
            lines,
            consecutive_loops,
        } => {
            m = base("divergence");
            m.insert("consecutive_loops".into(), json!(consecutive_loops));
            m.insert("divergent_bins".into(), json!(lines));
            m.insert("drive".into(), json!(drive));
        }
        MimoTripKind::LawFault {
            reason,
            consecutive,
        } => {
            m = base("law_fault");
            let r = match reason {
                MimoFaultReason::Error => "error",
                MimoFaultReason::Panicked => "panicked",
                MimoFaultReason::OverBudget => "over_budget",
                MimoFaultReason::BadOutput(OutputFault::Dimensions) => "wrong_length",
                MimoFaultReason::BadOutput(OutputFault::NonFinite) => "non_finite",
                MimoFaultReason::BadOutput(OutputFault::NotPositiveSemiDefinite) => "negative",
                MimoFaultReason::BadOutput(_) => "error",
            };
            m.insert("reason".into(), json!(r));
            m.insert("consecutive".into(), json!((*consecutive).max(1)));
        }
        MimoTripKind::Plant { message } => {
            m = base("envelope");
            m.insert("abort_lines".into(), json!(0));
            m.insert("worst_db".into(), json!(0.0));
            m.insert("message".into(), json!(message));
        }
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A law file in the catalog's form, unchecked.
    fn imported_entry() -> CatalogEntry {
        let dir = std::env::temp_dir().join(format!("sds-gate-{}", std::process::id()));
        let cat = LawCatalog::open(&dir).unwrap();
        let mut e = cat.entry("pinv_closed").unwrap();
        e.id = "f".repeat(64);
        e.builtin = false;
        e.identity.sha256 = "f".repeat(64);
        e.check = hertzforge_random_laws::CheckStatus::Unchecked;
        e
    }

    #[test]
    fn hardware_arms_only_with_a_checked_law_and_the_simulator_with_any() {
        let e = imported_entry();
        let id = e.id.clone();
        // The simulator runs any law.
        assert!(hardware_law_gate("sim", &id, Some(&e)).is_ok());
        assert!(hardware_law_gate("sim", &id, None).is_ok());
        // Hardware refuses an unchecked law file, and one the catalog does not hold,
        // saying what to do.
        for entry in [Some(&e), None] {
            let r = hardware_law_gate("ni", &id, entry).unwrap_err();
            assert_eq!(r.reason, Some("law_not_checked"), "{r:?}");
            assert!(
                r.message.contains("run its check") || r.message.contains("check on the simulator"),
                "{}",
                r.message
            );
        }
        // A failed or unreadable check is refused too.
        let mut bad = e.clone();
        bad.check = hertzforge_random_laws::CheckStatus::Unreadable("torn".into());
        assert!(hardware_law_gate("ni", &id, Some(&bad)).is_err());
        // A built-in law is cleared for hardware.
        assert!(hardware_law_gate("ni", "pinv_closed", None).is_ok());
    }

    #[test]
    fn a_sized_table_finds_its_standard_plant() {
        assert_eq!(sim_plant_for(4, 4), Some(SimStandardPlant::Modal4x4));
        assert_eq!(sim_plant_for(12, 6), Some(SimStandardPlant::Modal12x6));
        assert_eq!(sim_plant_for(3, 2), None);
    }

    #[test]
    fn json_parameters_take_the_kinds_the_manifest_declares() {
        let specs = builtin_mimo_manifest(BuiltinMimo::from_id("pinv_closed").unwrap())
            .law
            .params;
        let mut given = Map::new();
        given.insert("convergence".into(), json!(0.25));
        let p = params_of(&specs, &given);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].value, ParamValue::Number(0.25));
    }
}
