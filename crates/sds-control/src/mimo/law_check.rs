//! The check of a multiple-output law before it drives hardware (schema/PROTOCOL.md,
//! "Control Laws", `laws.check`): the law runs in process on every simulated test
//! article of [`MimoCheckCase::ALL`], through the MIMO Random run service and under
//! the core's own guards, and each run is judged by the rules of
//! `hertzforge-random-laws` ([`judge`]). A law file that passes every case gets a
//! check record against its SHA-256, which is what clears it for hardware
//! ([`hertzforge_random_laws::may_arm_on_hardware`]); the simulator runs any law.
//!
//! Each plant's reference is one the plant can reach with a sane drive: `H X H^H`
//! from the plant's exact FRF and a fixed, partially coherent drive `X` of 0.5 V rms
//! per drive, weighted on every line toward the directions the plant drives well (right
//! singular vectors whose singular value is at least [`KEEP_RATIO`] of the largest keep
//! their power; the rest keep [`WEAK_WEIGHT`] of it). A reference that leans on a weak
//! direction asks for a large drive and is not one a law should be judged by; one with
//! no power at all in the weak directions leaves holes in some controls' references.
//! The alarm and abort lines are +/- 6 and +/- 12 dB about each control's reference: a
//! case fails on the law's behaviour, not on a tight envelope the operator would set for
//! a real test.
//! The identification gate is overridden when it does not pass (the ill-conditioned
//! plant is built to fail it): the check is of the law, and the case says so.

use crate::protocol::{num, ErrorCode, MethodError};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::Backend;
use hertzforge_hal_sim::plants::{NamedPlant, StandardPlant};
use hertzforge_hal_sim::SimConfig;
use hertzforge_mimo_random::report::MimoEventKind;
use hertzforge_mimo_random::{
    DriveSpec, MimoConfig, MimoError, MimoFaultReason, MimoRandomService, MimoState, MimoTripKind,
    ReferenceSpec,
};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::{LevelSchedule, RunMode};
use hertzforge_random::LoopConfig;
use hertzforge_random_laws::{
    judge, CheckCase, MimoCaseMeasure, MimoCheckCase, MimoLawFactory, Requirement, TripClass,
    CONVERGE_DB, MAX_LOOPS_AT_FULL, SETTLE_LOOPS,
};
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{linalg, HermitianStack, C64};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The drive each plant's reference is built from, volts rms per drive.
pub const REFERENCE_DRIVE_VRMS: f64 = 0.5;
/// The reference's drive keeps the plant's directions whose singular value is at
/// least this fraction of the line's largest.
pub const KEEP_RATIO: f64 = 0.5;
/// The power the reference's drive keeps in the other directions (-10 dB), so no
/// control's reference has a hole where only a weak direction reaches it.
pub const WEAK_WEIGHT: f64 = 0.1;
/// The share of the reference drive's power that is coherent between drives.
pub const COHERENT_SHARE: f64 = 0.4;
/// Every drive's ceiling, volts rms (ten times the reference drive).
pub const DRIVE_CEILING_VRMS: f64 = 5.0;
/// Welch frames per loop in the check's runs.
pub const FRAMES_PER_LOOP: usize = 32;
/// The simulator's measurement noise on every case without its own, volts.
pub const FLOOR_NOISE: f64 = 1e-4;
/// How the law host begins a law file's output it refused at the boundary.
const HOST_REFUSAL: &str = "the law's output was refused";

/// How the check runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckOptions {
    /// The law's time budget per call as a fraction of the loop's acquisition
    /// (the run's own default unless a test relaxes it for a debug build).
    pub budget_fraction: f64,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions {
            budget_fraction: MimoConfig::new(Vec::new(), Vec::new(), empty_reference())
                .law_guards
                .budget_fraction,
        }
    }
}

fn empty_reference() -> ReferenceSpec {
    ReferenceSpec {
        sdm: HermitianStack::zeros(2, 1),
        tolerances: Tolerances::none(2, 1),
        band: (0, 1),
        df_hz: 1.0,
    }
}

/// One case's run and verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseResult {
    pub case: MimoCheckCase,
    pub measure: MimoCaseMeasure,
    pub verdict: CheckCase,
}

/// The line grid a run is made on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// Sample rate, Hz.
    pub fs_hz: f64,
    /// Points per Welch frame.
    pub nfft: usize,
    /// The controlled band, Hz.
    pub band_hz: (f64, f64),
}

impl Grid {
    fn of(case: MimoCheckCase) -> Grid {
        match case {
            // The rigid plate's claim is its rigid-body band.
            MimoCheckCase::RigidPlate12x6 => Grid {
                fs_hz: 4096.0,
                nfft: 2048,
                band_hz: (26.0, 150.0),
            },
            // 2 Hz lines: the modal plants' Q 50 modes are resolved above 160 Hz.
            _ => Grid {
                fs_hz: 8192.0,
                nfft: 4096,
                band_hz: (160.0, 1600.0),
            },
        }
    }
    fn lines(&self) -> usize {
        self.nfft / 2 + 1
    }
    fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }
    fn band(&self) -> (usize, usize) {
        let df = self.df();
        (
            (self.band_hz.0 / df).round() as usize,
            (self.band_hz.1 / df).round() as usize + 1,
        )
    }
    fn loop_config(&self) -> LoopConfig {
        let mut c = LoopConfig::demo_default();
        c.fs_hz = self.fs_hz;
        c.nfft = self.nfft;
        c.band_lo_hz = self.band_hz.0;
        c.band_hi_hz = self.band_hz.1;
        c.block_frames = 256;
        c
    }
}

/// A fixed generator for the reference's drive.
struct Rng(u64);

impl Rng {
    fn sym(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        2.0 * ((self.0 >> 11) as f64 / (1u64 << 53) as f64) - 1.0
    }
}

/// A partially coherent drive SDM of `vrms` per drive on the band:
/// `s (0.6 I + 0.4 A A^H / n)` with one fixed complex `A`.
fn reference_drive(n_d: usize, grid: &Grid, vrms: f64) -> HermitianStack {
    let (lo, hi) = grid.band();
    let s = vrms * vrms / ((hi - lo) as f64 * grid.df());
    let mut rng = Rng(0xD21E ^ 0x9E37_79B9_7F4A_7C15);
    let a: Vec<C64> = (0..n_d * n_d)
        .map(|_| C64::new(rng.sym(), rng.sym()))
        .collect();
    let mut g = HermitianStack::zeros(grid.lines(), n_d);
    for line in lo..hi {
        for i in 0..n_d {
            for j in i..n_d {
                let mut acc = C64::new(0.0, 0.0);
                for k in 0..n_d {
                    acc += a[i * n_d + k] * a[j * n_d + k].conj();
                }
                let mut v = acc * (COHERENT_SHARE / n_d as f64);
                if i == j {
                    v += C64::new(1.0 - COHERENT_SHARE, 0.0);
                }
                g.set(line, i, j, v * s);
            }
        }
    }
    g
}

/// `x` weighted on every band line toward the plant's well-driven directions, `P x P`
/// with `P = V diag(w) V^H` over the right singular vectors of `h` (`w` 1 for those
/// [`KEEP_RATIO`] keeps, `sqrt(WEAK_WEIGHT)` for the rest), then scaled back to `x`'s
/// total power.
fn well_driven(
    h: &hertzforge_sdm::MatrixStack,
    x: HermitianStack,
    grid: &Grid,
) -> Result<HermitianStack, String> {
    let svd = linalg::svd(h).map_err(|e| e.to_string())?;
    let (lines, n_d, k) = (h.lines(), h.cols(), svd.k());
    let (lo, hi) = grid.band();
    let mut p = hertzforge_sdm::MatrixStack::zeros(lines, n_d, n_d);
    for line in lo..hi {
        let s = &svd.s[line * k..(line + 1) * k];
        // Weight 1 on a well-driven direction, WEAK_WEIGHT on the rest (as a power).
        let w: Vec<f64> = (0..k)
            .map(|m| {
                if s[m] >= KEEP_RATIO * s[0] {
                    1.0
                } else {
                    WEAK_WEIGHT.sqrt()
                }
            })
            .collect();
        for r in 0..n_d {
            for c in 0..n_d {
                let mut acc = C64::new(0.0, 0.0);
                for (m, wm) in w.iter().enumerate() {
                    acc += svd.v.get(line, r, m) * svd.v.get(line, c, m).conj() * *wm;
                }
                p.set(line, r, c, acc);
            }
        }
    }
    let mut y = linalg::congruence(&p, &x).map_err(|e| e.to_string())?;
    let before: f64 = linalg::trace(&x).iter().sum();
    let after: f64 = linalg::trace(&y).iter().sum();
    if after > 0.0 {
        y.scale_lines(&vec![before / after; lines])
            .map_err(|e| e.to_string())?;
    }
    Ok(y)
}

/// Alarm at +/- 6 dB and abort at +/- 12 dB about each control's reference PSD.
fn tolerances(r: &HermitianStack, band: (usize, usize)) -> Tolerances {
    let n = r.n();
    let mut t = Tolerances::none(r.lines(), n);
    for line in band.0..band.1 {
        for i in 0..n {
            let p = r.get(line, i, i).re;
            let k = line * n + i;
            t.alarm_upper[k] = p * 10f64.powf(0.6);
            t.alarm_lower[k] = p * 10f64.powf(-0.6);
            t.abort_upper[k] = p * 10f64.powf(1.2);
            t.abort_lower[k] = p * 10f64.powf(-1.2);
        }
    }
    t
}

/// Inputs on channels `1..=n_in`, every one a control; drives on `101..`.
fn table(n_in: usize, n_d: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n_in {
        let mut c = Channel::new(1 + i as u32);
        c.role = ChannelRole::Control;
        c.units = Units::AccelG;
        c.range_max = 1.0e6;
        t.channels.push(c);
    }
    for k in 0..n_d {
        let mut d = Channel::new(101 + k as u32);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        t.channels.push(d);
    }
    t
}

fn plant_of(case: MimoCheckCase) -> NamedPlant {
    StandardPlant::from_name(case.plant())
        .expect("every check case names a standard plant")
        .build()
}

/// A run of a law on one plant, as the check makes it: the plant, the grid, the
/// level schedule (dB, the last entry full level) and the measurement noise (dB
/// under the smallest control's response; `None` for the simulator's floor).
#[derive(Debug, Clone)]
pub struct PlantRun {
    pub plant: NamedPlant,
    pub grid: Grid,
    pub levels_db: Vec<f64>,
    pub noise_below_db: Option<f64>,
}

impl PlantRun {
    /// The run a check case makes.
    pub fn of(case: MimoCheckCase) -> PlantRun {
        PlantRun {
            plant: plant_of(case),
            grid: Grid::of(case),
            levels_db: case.levels_db().to_vec(),
            noise_below_db: case.noise_below_db(),
        }
    }
}

/// The simulator's noise for `case`: the floor, or `noise_below_db` under the
/// smallest control's in-band rms (white noise of standard deviation `s` has
/// in-band rms `s sqrt(2 B / fs)`).
fn noise_of(below: Option<f64>, reference: &HermitianStack, grid: &Grid) -> f64 {
    let Some(below) = below else {
        return FLOOR_NOISE;
    };
    let (lo, hi) = grid.band();
    let df = grid.df();
    let smallest = (0..reference.n())
        .map(|c| {
            (lo..hi)
                .map(|l| reference.get(l, c, c).re * df)
                .sum::<f64>()
                .sqrt()
        })
        .fold(f64::INFINITY, f64::min);
    let bw = (hi - lo) as f64 * df;
    smallest * 10f64.powf(-below / 20.0) * (grid.fs_hz / (2.0 * bw)).sqrt()
}

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// A scratch data root for one case (records are off; the service still wants one).
fn scratch_root() -> PathBuf {
    let n = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("hertzforge-law-check-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The service for `run` with the law from `factory`.
fn build(
    run: &PlantRun,
    factory: Arc<dyn MimoLawFactory>,
    opts: &CheckOptions,
    root: &std::path::Path,
) -> Result<MimoRandomService, String> {
    let grid = run.grid;
    let plant = &run.plant;
    let (n_in, n_d) = plant.model.dims().ok_or("the plant has no size")?;
    let h = plant
        .model
        .analytic_frf(grid.lines(), grid.df(), grid.fs_hz);
    let x = well_driven(&h, reference_drive(n_d, &grid, REFERENCE_DRIVE_VRMS), &grid)?;
    let r = linalg::response_from_drive(&h, &x).map_err(|e| e.to_string())?;
    let noise = noise_of(run.noise_below_db, &r, &grid);
    let band = grid.band();
    let reference = ReferenceSpec {
        tolerances: tolerances(&r, band),
        sdm: r,
        band,
        df_hz: grid.df(),
    };
    let drives = (0..n_d)
        .map(|k| {
            let mut d = DriveSpec::new(101 + k as u32, DRIVE_CEILING_VRMS);
            d.pretest_max_vrms = 1.0;
            d
        })
        .collect();
    let mut cfg = MimoConfig::new((1..=n_in as u32).collect(), drives, reference);
    let levels = &run.levels_db;
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: levels.first().copied().unwrap_or(0.0),
        table_db: levels.clone(),
        dwell_s: vec![0.0],
    };
    cfg.records.enabled = false;
    cfg.estimation.frames_per_loop = FRAMES_PER_LOOP;
    cfg.estimation.memory_loops = 8.0;
    cfg.law_guards.budget_fraction = opts.budget_fraction;
    let backend: Box<dyn Backend> = Box::new(
        SimConfig::new()
            .sample_rate_hz(grid.fs_hz)
            .plant(plant.model.clone())
            .noise_level(noise)
            .shared_clock(true)
            .seed(0x51_4D_00)
            .build(),
    );
    MimoRandomService::new(
        cfg,
        table(n_in, n_d),
        grid.loop_config(),
        backend,
        StationClass::HertzforgeSim,
        root,
    )
    .map(|s| s.with_law_factory(factory))
    .map_err(|e| e.to_string())
}

/// Walk the run from idle to engaged, overriding a failed gate or prediction.
fn engage(svc: &mut MimoRandomService, m: &mut MimoCaseMeasure) -> Result<(), MimoError> {
    const ACK: &str =
        "The law check runs on the simulator; the gate is the plant's, not the law's.";
    svc.start()?;
    if svc.state() == MimoState::Feasibility {
        svc.override_with(ACK)?;
    }
    if svc.state() == MimoState::NoiseFloor {
        svc.run_noise_floor()?;
    }
    if svc.state() == MimoState::SysId {
        svc.run_identification()?;
    }
    if svc.state() == MimoState::Gate {
        m.gate_passed = svc.gate().is_some_and(|g| g.passed);
        if !m.gate_passed {
            svc.override_with(ACK)?;
        }
        svc.proceed()?;
    }
    if svc.state() == MimoState::Predict {
        if let Err(MimoError::Blocked(_)) = svc.proceed() {
            svc.override_with(ACK)?;
            svc.proceed()?;
        }
    }
    Ok(())
}

/// Run `case` with the law `factory` builds and measure it.
pub fn run_case(
    case: MimoCheckCase,
    factory: Arc<dyn MimoLawFactory>,
    opts: &CheckOptions,
) -> MimoCaseMeasure {
    run_plant(&PlantRun::of(case), factory, opts)
}

/// Make `run` with the law `factory` builds and measure it, exactly as a check case
/// is made: any plant, such as a bench network's computed FRF, before it is wired.
pub fn run_plant(
    run: &PlantRun,
    factory: Arc<dyn MimoLawFactory>,
    opts: &CheckOptions,
) -> MimoCaseMeasure {
    let root = scratch_root();
    let mut m = MimoCaseMeasure {
        final_error_db: f64::NAN,
        worst_line_db: f64::NAN,
        settle_db: f64::NAN,
        ..MimoCaseMeasure::default()
    };
    let mut svc = match build(run, factory, opts, &root) {
        Ok(s) => s,
        Err(e) => {
            m.setup_error = Some(e);
            let _ = std::fs::remove_dir_all(&root);
            return m;
        }
    };
    let worst = |v: &[f64]| v.iter().fold(0.0f64, |a, b| a.max(b.abs()));
    // Each control's in-band error, every loop at full level.
    let mut history: Vec<Vec<f64>> = Vec::new();
    let outcome = engage(&mut svc, &mut m).and_then(|()| {
        let ladder = run.levels_db.len() as u64;
        let mut at_full: Option<u64> = None;
        while svc.state().is_running() {
            svc.loop_tick()?;
            m.worst_call_ms = m.worst_call_ms.max(svc.timing().law_ms);
            if svc.state() == MimoState::FullLevel && at_full.is_none() {
                at_full = Some(svc.loop_count());
            }
            if let Some(f) = at_full {
                history.push(svc.error_db().to_vec());
                m.loops_at_full = svc.loop_count() - f;
                if m.loops_at_full > 0 && worst(svc.error_db()) <= CONVERGE_DB {
                    m.loops_to_converge = Some(m.loops_at_full);
                    break;
                }
                if m.loops_at_full >= MAX_LOOPS_AT_FULL {
                    break;
                }
            }
            if svc.loop_count() > ladder + MAX_LOOPS_AT_FULL + 20 {
                break;
            }
        }
        Ok(())
    });
    if let Err(e) = outcome {
        m.setup_error = Some(e.to_string());
    }
    m.reached_full_level = svc.loops_at_full_level() > 0 || m.loops_at_full > 0;
    m.final_error_db = worst(svc.error_db());
    m.worst_line_db = worst(svc.worst_error_db());
    m.settle_db = settle_db(&history);
    m.trip = svc.trip().map(|t| t.kind.words());
    m.trip_class = match svc.trip().map(|t| &t.kind) {
        None => TripClass::None,
        Some(MimoTripKind::Divergence { .. } | MimoTripKind::CapDivergence { .. }) => {
            TripClass::CoreJudgement
        }
        Some(_) => TripClass::Other,
    };
    for e in svc.log().events() {
        if let MimoEventKind::LawMiss { reason, detail } = &e.kind {
            let detail = detail.as_str();
            m.misses.push(if detail.is_empty() {
                reason.words().to_string()
            } else {
                format!("{} ({detail})", reason.words())
            });
            // Refused by the core's output checks, or by the law host's (a law file's
            // output is checked at the boundary before the core sees it).
            if matches!(reason, MimoFaultReason::BadOutput(_)) || detail.starts_with(HOST_REFUSAL) {
                m.rejected += 1;
            }
        }
    }
    let stats = svc.law_stats();
    m.calls = stats.calls;
    m.repaired_lines = stats.repaired_lines;
    m.capped_lines = stats.capped_drive_lines;
    m.budget_ms = opts.budget_fraction * svc.loop_config().segment_seconds() * 1e3;
    if svc.state().is_running() {
        svc.stop();
    }
    drop(svc);
    let _ = std::fs::remove_dir_all(&root);
    m
}

/// The largest loop-to-loop change of any control's error over the last
/// [`SETTLE_LOOPS`] loops of `history` (NaN with fewer than that many changes).
fn settle_db(history: &[Vec<f64>]) -> f64 {
    if history.len() < SETTLE_LOOPS + 1 {
        return f64::NAN;
    }
    let tail = &history[history.len() - SETTLE_LOOPS - 1..];
    let mut most = 0.0f64;
    for pair in tail.windows(2) {
        for (a, b) in pair[0].iter().zip(&pair[1]) {
            most = most.max((b - a).abs());
        }
    }
    most
}

/// The law under check: what builds it for a run of `controls` control channels.
pub type LawSource<'a> = &'a (dyn Fn(usize) -> Result<Arc<dyn MimoLawFactory>, String> + Sync);

/// Run every case of the check with the law `law` builds. `progress` hears each
/// case before it runs (`None`) and after (its result).
pub fn run_check(
    law: LawSource<'_>,
    opts: &CheckOptions,
    progress: &mut dyn FnMut(usize, MimoCheckCase, Option<&CaseResult>),
) -> Vec<CaseResult> {
    let mut out = Vec::new();
    for (i, case) in MimoCheckCase::ALL.into_iter().enumerate() {
        progress(i, case, None);
        let controls = StandardPlant::from_name(case.plant())
            .map(|p| p.dims().0)
            .unwrap_or(0);
        let measure = match law(controls) {
            Ok(f) => run_case(case, f, opts),
            Err(e) => MimoCaseMeasure {
                setup_error: Some(e),
                ..MimoCaseMeasure::default()
            },
        };
        let verdict = judge(case, &measure);
        let r = CaseResult {
            case,
            measure,
            verdict,
        };
        progress(i, case, Some(&r));
        out.push(r);
    }
    out
}

/// One case on the wire.
pub fn case_json(r: &CaseResult) -> Value {
    let m = &r.measure;
    json!({
        "name": r.case.name(),
        "plant": r.case.plant(),
        "passed": r.verdict.passed,
        "detail": r.verdict.detail,
        "loops_to_converge": m.loops_to_converge,
        "loops_at_full": m.loops_at_full,
        "final_error_db": num(m.final_error_db),
        "worst_line_db": num(m.worst_line_db),
        "settle_db": num(m.settle_db),
        "requirement": match r.case.requirement() {
            Requirement::Converge => "converge",
            Requirement::Settle => "settle",
            Requirement::Survive => "survive",
        },
        "trip": m.trip,
        "misses": m.misses.len(),
        "rejected": m.rejected,
        "repaired_lines": m.repaired_lines,
        "capped_lines": m.capped_lines,
        "calls": m.calls,
        "worst_call_ms": num(m.worst_call_ms),
        "budget_ms": num(m.budget_ms),
        "gate_passed": m.gate_passed,
    })
}

/// A `laws.check` progress event's data.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: MimoCheckCase,
    r: Option<&CaseResult>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": MimoCheckCase::ALL.len(),
        "case": case.name(),
        "state": if r.is_some() { "done" } else { "running" },
        "result": r.map(case_json),
    })
}

/// The report of a whole check.
pub fn report_json(cases: &[CaseResult], recorded: bool) -> Value {
    let passed = !cases.is_empty() && cases.iter().all(|c| c.verdict.passed);
    json!({
        "passed": passed,
        "recorded": recorded,
        "tool_version": hertzforge_random_laws::MIMO_CHECK_VERSION,
        "cases": cases.iter().map(case_json).collect::<Vec<_>>(),
    })
}

/// The control-channel counts of the check's plants, each once.
pub fn control_counts() -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for c in MimoCheckCase::ALL {
        if let Some(p) = StandardPlant::from_name(c.plant()) {
            let n = p.dims().0;
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

static CHECK_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The one check that may run at a time: held while it runs, released when dropped.
pub struct CheckSlot(());

impl CheckSlot {
    /// Take the slot, or the refusal when a check is already running.
    pub fn take() -> Result<CheckSlot, MethodError> {
        if CHECK_RUNNING.swap(true, Ordering::AcqRel) {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "law_check_running",
                "A law check is already running; wait for its report.",
            ));
        }
        Ok(CheckSlot(()))
    }
}

impl Drop for CheckSlot {
    fn drop(&mut self) {
        CHECK_RUNNING.store(false, Ordering::Release);
    }
}

/// The refusal for an id the catalog does not hold.
pub fn unknown_law(id: &str) -> MethodError {
    MethodError::named(
        ErrorCode::NotFound,
        "law_unknown",
        format!("No law {id} is in the catalog. Import its file first, or check it by path."),
    )
    .with_detail(json!({"id": id}))
}

/// The refusal for parameter values the law does not accept.
pub fn bad_choice(id: &str, why: &str) -> MethodError {
    MethodError::named(
        ErrorCode::BadRequest,
        "law_params_invalid",
        format!(
            "The parameters for law {id} were not accepted ({why}). Correct them and check again."
        ),
    )
    .with_detail(json!({"id": id}))
}

/// The refusal when the check ran but its record could not be written.
pub fn not_recorded(why: &str) -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "law_check_not_recorded",
        format!(
            "The check ran but its record was not written ({why}). Check that the data folder \
             is writable, then check the law again."
        ),
    )
}

/// The refusal for a law of the other test type.
pub fn single_output_refusal(id: &str) -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "law_check_unsupported",
        format!(
            "Law {id} is a single-output Random law; the check before hardware runs \
             multiple-output laws. Choose a MIMO law."
        ),
    )
}

/// The time now, RFC 3339 in UTC, to the second.
pub fn now_rfc3339() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_rfc_3339_utc() {
        let d = now_rfc3339();
        assert_eq!(d.len(), 20, "{d}");
        assert!(d.ends_with('Z') && d.as_bytes()[10] == b'T', "{d}");
        assert!(d.as_str() > "2026-01-01T00:00:00Z");
    }

    #[test]
    fn the_noise_case_sits_30_db_under_the_smallest_control() {
        let case = MimoCheckCase::Noisy4x4;
        let grid = Grid::of(case);
        let p = plant_of(case);
        let h = p.model.analytic_frf(grid.lines(), grid.df(), grid.fs_hz);
        let x = well_driven(&h, reference_drive(4, &grid, REFERENCE_DRIVE_VRMS), &grid).unwrap();
        let r = linalg::response_from_drive(&h, &x).unwrap();
        let s = noise_of(case.noise_below_db(), &r, &grid);
        let (lo, hi) = grid.band();
        let bw = (hi - lo) as f64 * grid.df();
        let in_band = s * (2.0 * bw / grid.fs_hz).sqrt();
        let smallest = (0..4)
            .map(|c| {
                (lo..hi)
                    .map(|l| r.get(l, c, c).re * grid.df())
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(f64::INFINITY, f64::min);
        assert!((20.0 * (smallest / in_band).log10() - 30.0).abs() < 1e-9);
        assert_eq!(noise_of(None, &r, &grid), FLOOR_NOISE);
    }
}
