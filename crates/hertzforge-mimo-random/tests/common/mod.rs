//! Shared fixtures: the simulator's cross-coupled plants through the real engine,
//! a reachable reference built from a plant's exact FRF, and the run loop the
//! acceptance tests measure.
#![allow(dead_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::Backend;
use hertzforge_hal_sim::plants::NamedPlant;
use hertzforge_hal_sim::SimConfig;
use hertzforge_mimo_random::{DriveSpec, MimoConfig, MimoRandomService, MimoState, ReferenceSpec};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::{LevelSchedule, RunMode};
use hertzforge_random::LoopConfig;
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::path::PathBuf;

/// A deterministic generator for test data.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn sym(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

/// The grid the normal-suite tests run on.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub fs_hz: f64,
    pub nfft: usize,
    pub band_hz: (f64, f64),
}

impl Grid {
    pub fn standard() -> Self {
        Grid {
            fs_hz: 8192.0,
            nfft: 4096,
            band_hz: (160.0, 1600.0),
        }
    }
    /// The design's full scale: 1,600 band lines at 2 Hz (200 Hz to 3.4 kHz).
    pub fn full_scale() -> Self {
        Grid {
            fs_hz: 16384.0,
            nfft: 8192,
            band_hz: (200.0, 3398.0),
        }
    }
    pub fn lines(&self) -> usize {
        self.nfft / 2 + 1
    }
    pub fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }
    pub fn band(&self) -> (usize, usize) {
        let df = self.df();
        (
            (self.band_hz.0 / df).round() as usize,
            (self.band_hz.1 / df).round() as usize + 1,
        )
    }
    pub fn loop_config(&self) -> LoopConfig {
        let mut c = LoopConfig::demo_default();
        c.fs_hz = self.fs_hz;
        c.nfft = self.nfft;
        c.band_lo_hz = self.band_hz.0;
        c.band_hi_hz = self.band_hz.1;
        c.block_frames = 256;
        c
    }
}

/// Inputs on channels `1..=n_in` (all controls unless `monitors` say otherwise),
/// drives on `101..`.
pub fn table(n_in: usize, n_d: usize, controls: &[u32]) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n_in {
        let mut c = Channel::new(1 + i as u32);
        c.role = if controls.contains(&(1 + i as u32)) {
            ChannelRole::Control
        } else {
            ChannelRole::Monitor
        };
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

/// The simulator running `plant`.
pub fn backend(plant: &NamedPlant, grid: &Grid, seed: u64, noise: f64) -> Box<dyn Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(grid.fs_hz)
            .plant(plant.model.clone())
            .noise_level(noise)
            .shared_clock(true)
            .seed(seed)
            .build(),
    )
}

/// The plant's exact FRF on the grid.
pub fn exact_frf(plant: &NamedPlant, grid: &Grid) -> MatrixStack {
    plant
        .model
        .analytic_frf(grid.lines(), grid.df(), grid.fs_hz)
}

/// A random positive definite drive SDM on the band: unit-ish diagonal at
/// `psd_v2_hz`, partial coherence between drives.
pub fn random_drive_sdm(n_d: usize, grid: &Grid, psd_v2_hz: f64, seed: u64) -> HermitianStack {
    let (lo, hi) = grid.band();
    let mut rng = Rng::new(seed);
    // One fixed mixing matrix A (n x n): G = s (0.6 I + 0.4 A A^H / n).
    let mut a = vec![C64::new(0.0, 0.0); n_d * n_d];
    for v in a.iter_mut() {
        *v = C64::new(rng.sym(), rng.sym());
    }
    let mut g = HermitianStack::zeros(grid.lines(), n_d);
    for line in lo..hi {
        for i in 0..n_d {
            for j in i..n_d {
                let mut s = C64::new(0.0, 0.0);
                for k in 0..n_d {
                    s += a[i * n_d + k] * a[j * n_d + k].conj();
                }
                let mut v = s * (0.4 / n_d as f64);
                if i == j {
                    v += C64::new(0.6, 0.0);
                }
                g.set(line, i, j, v * psd_v2_hz);
            }
        }
    }
    g
}

/// `H G H^H` on the band (zero outside).
pub fn response_of(h: &MatrixStack, g: &HermitianStack, grid: &Grid) -> HermitianStack {
    let (lo, hi) = grid.band();
    let mut out = HermitianStack::zeros(h.lines(), h.rows());
    for line in lo..hi {
        for i in 0..h.rows() {
            for j in i..h.rows() {
                let mut acc = C64::new(0.0, 0.0);
                for a in 0..h.cols() {
                    for b in 0..h.cols() {
                        acc += h.get(line, i, a) * g.get(line, a, b) * h.get(line, j, b).conj();
                    }
                }
                out.set(line, i, j, acc);
            }
        }
    }
    out
}

/// Alarm at +/- `alarm_db`, abort at +/- `abort_db` about the reference's PSDs.
pub fn tolerances(r: &HermitianStack, grid: &Grid, alarm_db: f64, abort_db: f64) -> Tolerances {
    let (lo, hi) = grid.band();
    let n = r.n();
    let mut t = Tolerances::none(r.lines(), n);
    for line in lo..hi {
        for i in 0..n {
            let p = r.get(line, i, i).re;
            let k = line * n + i;
            t.alarm_upper[k] = p * 10f64.powf(alarm_db / 10.0);
            t.alarm_lower[k] = p * 10f64.powf(-alarm_db / 10.0);
            t.abort_upper[k] = p * 10f64.powf(abort_db / 10.0);
            t.abort_lower[k] = p * 10f64.powf(-abort_db / 10.0);
        }
    }
    t
}

/// A reference spec from a reference SDM.
pub fn reference(r: HermitianStack, grid: &Grid) -> ReferenceSpec {
    let tol = tolerances(&r, grid, 3.0, 6.0);
    ReferenceSpec {
        sdm: r,
        tolerances: tol,
        band: grid.band(),
        df_hz: grid.df(),
    }
}

/// Drives with a ceiling of `ceiling` V rms, identification up to 1 V.
pub fn drives(n_d: usize, ceiling: f64) -> Vec<DriveSpec> {
    (0..n_d)
        .map(|k| {
            let mut d = DriveSpec::new(101 + k as u32, ceiling);
            d.pretest_max_vrms = 1.0f64.min(ceiling);
            d
        })
        .collect()
}

/// A quick ladder: -6, -3, 0 dB, no dwell.
pub fn quick_schedule() -> LevelSchedule {
    LevelSchedule {
        mode: RunMode::Auto,
        start_db: -6.0,
        table_db: vec![-6.0, -3.0, 0.0],
        dwell_s: vec![0.0],
    }
}

/// A scratch data root for one test.
pub fn data_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "hertzforge-mimo-random-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A standard physical test on `plant`: every input a control, a reachable
/// reference from the exact FRF and a random drive SDM of `drive_vrms` per drive.
pub fn physical_config(
    plant: &NamedPlant,
    grid: &Grid,
    drive_vrms: f64,
) -> (MimoConfig, ChannelTable) {
    let (n_in, n_d) = plant.model.dims().expect("a sized plant");
    let h = exact_frf(plant, grid);
    let (lo, hi) = grid.band();
    let bw = (hi - lo) as f64 * grid.df();
    let x = random_drive_sdm(n_d, grid, drive_vrms * drive_vrms / bw, 0xD21E);
    let r = response_of(&h, &x, grid);
    let controls: Vec<u32> = (1..=n_in as u32).collect();
    let mut cfg = MimoConfig::new(controls.clone(), drives(n_d, 5.0), reference(r, grid));
    cfg.schedule = quick_schedule();
    cfg.records.enabled = false;
    cfg.estimation.memory_loops = 8.0;
    // A debug build of the linear algebra is many times slower than release; the
    // law's time budget is the design's in release (A5 measures it there) and the
    // whole loop in a debug test run.
    if cfg!(debug_assertions) {
        cfg.law_guards.budget_fraction = 1.0;
    }
    (cfg, table(n_in, n_d, &controls))
}

/// Build the service.
pub fn service(
    cfg: MimoConfig,
    table: ChannelTable,
    plant: &NamedPlant,
    grid: &Grid,
    tag: &str,
) -> MimoRandomService {
    MimoRandomService::new(
        cfg,
        table,
        grid.loop_config(),
        backend(plant, grid, 0x51_4D_00, 1e-4),
        StationClass::HertzforgeSim,
        data_root(tag),
    )
    .expect("the service builds")
}

/// What a convergence run measured.
#[derive(Debug, Clone)]
pub struct Convergence {
    /// Loops after reaching full level until every control PSD was within the bar
    /// on every band line (`None`: not within `max_loops`).
    pub loops_to_converge: Option<u64>,
    /// The worst line error over every control at the end, dB.
    pub final_worst_db: f64,
    /// The mean in-band rms error over the controls at the end, dB.
    pub final_rms_db: f64,
    /// The mean coherence error over every pair at the end (|gamma^2 measured -
    /// gamma^2 reference| averaged over pairs and band lines).
    pub mean_coherence_error: f64,
    pub loops: u64,
    pub state: MimoState,
}

/// The mean |coherence error| over every pair and band line.
pub fn coherence_error(m: &HermitianStack, r: &HermitianStack, band: (usize, usize)) -> f64 {
    let n = m.n();
    let coh = |g: &HermitianStack, l: usize, i: usize, j: usize| {
        let d = g.get(l, i, i).re * g.get(l, j, j).re;
        if d > 0.0 {
            g.get(l, i, j).norm_sqr() / d
        } else {
            0.0
        }
    };
    let (mut s, mut c) = (0.0, 0usize);
    for l in band.0..band.1 {
        for i in 0..n {
            for j in i + 1..n {
                s += (coh(m, l, i, j) - coh(r, l, i, j)).abs();
                c += 1;
            }
        }
    }
    if c == 0 {
        0.0
    } else {
        s / c as f64
    }
}

/// Run to full level and on, until `bar_db` is met or `max_loops` at full level.
pub fn converge(svc: &mut MimoRandomService, bar_db: f64, max_loops: u64) -> Convergence {
    let st = match svc.run_auto(0) {
        Ok(s) => s,
        Err(e) => panic!(
            "the run did not engage: {e}; gate {:?}; ident {:?}",
            svc.gate(),
            svc.identification().map(|s| (
                s.level_db,
                s.frames,
                s.steps,
                s.lag_frames,
                s.drive_vrms.clone()
            ))
        ),
    };
    assert_eq!(st, MimoState::Engaged, "{:?}", svc.log().events());
    let mut at_full = None;
    let mut first = None;
    while svc.state().is_running() {
        svc.loop_tick().expect("a loop");
        if svc.state() == MimoState::FullLevel && at_full.is_none() {
            at_full = Some(svc.loop_count());
        }
        if let Some(f) = at_full {
            let worst = svc
                .worst_error_db()
                .iter()
                .fold(0.0f64, |a, b| a.max(b.abs()));
            if first.is_none() && worst <= bar_db && svc.loop_count() > f {
                first = Some(svc.loop_count() - f);
            }
            if svc.loop_count() - f >= max_loops || first.is_some() {
                break;
            }
        }
        if svc.loop_count() > max_loops + 40 {
            break;
        }
    }
    let worst = svc
        .worst_error_db()
        .iter()
        .fold(0.0f64, |a, b| a.max(b.abs()));
    let rms = svc.error_db().iter().sum::<f64>() / svc.error_db().len().max(1) as f64;
    let coh = svc
        .response_full_level()
        .map(|m| coherence_error(m, svc.reference(), svc.band()))
        .unwrap_or(f64::NAN);
    Convergence {
        loops_to_converge: first,
        final_worst_db: worst,
        final_rms_db: rms,
        mean_coherence_error: coh,
        loops: svc.loop_count(),
        state: svc.state(),
    }
}
