//! The drive across segment seams on a station that keeps real time.
//!
//! A DAQ card's output is a non-regenerating buffer drained by the sample clock.
//! Between two segments nothing feeds it, so the host's work at the seam (the
//! estimate, the level search, the next record's synthesis) must finish before the
//! buffer runs dry. If it does not, the generation stops for good: the buffer then
//! reports no free space, accepts nothing, and the next segment ends on the
//! engine's stuck-output fault. On the 4 by 4 loopback bench that fault ended
//! identification at the seam after the level search's first step.
//!
//! The station here models exactly that on top of the simulator: inputs arrive on
//! the wall clock at the sample rate, the output buffer holds what a DAQ card's
//! buffer holds at the bench's rate and block (two blocks of lookahead plus one,
//! 768 frames at 12 800 Hz and a 256-frame block, 60 ms), and a buffer that ever
//! runs dry stops generating and refuses every later write. A stop that finds the
//! buffer already dry counts too: until that stop the card held its last sample.
//!
//! Identification ends with nothing to feed the output until the loop engages:
//! the matrices are solved, the gate may hold for the operator, and the law's
//! first solve may take seconds. The second test holds at the gate and gives the
//! law a first solve of half a second, eight times the buffer: the station must
//! be stopped (its controlled ramp to zero) for all of it, never left to run dry.
//!
//! Release builds only (a debug build's seams measure the build, not the module):
//! `cargo test --release -p hertzforge-mimo-random --test output_seams`. Listed in
//! `scripts/rust_timing_tests.txt`.
mod common;

use common::*;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    OutputBufferContract, UnderrunPolicy,
};
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_hal_sim::SimConfig;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::{MimoRandomService, MimoState};
use hertzforge_module_abi::StationClass;
use hertzforge_random_laws::{ChoiceError, LawBudgets, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The bench's rate and block.
const FS_HZ: f64 = 12_800.0;
const BLOCK: usize = 256;
/// The NI backend's default lookahead, in blocks; its buffer is one block more.
const LOOKAHEAD_BLOCKS: usize = 2;

/// What the station saw.
#[derive(Debug, Default, Clone)]
struct Seen {
    /// The longest time between two accepted writes while generating, ms.
    longest_gap_ms: f64,
    /// When the output ran dry and stopped, if it did: the frames it had generated
    /// and the ms since the last accepted write. Kept across a stop.
    ran_dry: Option<(u64, f64)>,
    /// Whether the station is generating: started and not stopped since.
    generating: bool,
}

/// The simulator behind a real-time, non-regenerating output buffer.
struct RealTimeStation {
    inner: Box<dyn Backend>,
    n_in: usize,
    n_out: usize,
    buffer_frames: u64,
    started: Option<Instant>,
    written: u64,
    read: u64,
    last_write: Option<Instant>,
    dry: bool,
    seen: Arc<Mutex<Seen>>,
}

impl RealTimeStation {
    fn new(inner: Box<dyn Backend>, n_in: usize, n_out: usize, seen: Arc<Mutex<Seen>>) -> Self {
        RealTimeStation {
            inner,
            n_in,
            n_out,
            buffer_frames: (BLOCK * (LOOKAHEAD_BLOCKS + 1)) as u64,
            started: None,
            written: 0,
            read: 0,
            last_write: None,
            dry: false,
            seen,
        }
    }

    fn elapsed_frames(&self, now: Instant) -> u64 {
        self.started
            .map(|t| (now.duration_since(t).as_secs_f64() * FS_HZ) as u64)
            .unwrap_or(0)
    }

    /// A clock that has run past everything written has run the buffer dry: the
    /// generation stops there and never resumes.
    fn check(&mut self, now: Instant) {
        if self.started.is_some() && !self.dry && self.elapsed_frames(now) > self.written {
            self.dry = true;
            let since = self
                .last_write
                .map(|t| now.duration_since(t).as_secs_f64() * 1e3)
                .unwrap_or(f64::NAN);
            self.seen
                .lock()
                .unwrap()
                .ran_dry
                .get_or_insert((self.written, since));
        }
    }

    fn queued(&self, now: Instant) -> u64 {
        self.written - self.elapsed_frames(now).min(self.written)
    }

    fn reset(&mut self) {
        self.started = None;
        self.written = 0;
        self.read = 0;
        self.last_write = None;
        self.dry = false;
    }
}

impl Backend for RealTimeStation {
    fn caps(&self) -> Caps {
        let mut c = self.inner.caps();
        c.output_buffer = Some(OutputBufferContract {
            min_lookahead_frames: (BLOCK * LOOKAHEAD_BLOCKS) as u32,
            underrun_policy: UnderrunPolicy::RampDownThenAbort,
        });
        c
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
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
        self.inner.start()?;
        self.started = Some(Instant::now());
        self.read = 0;
        self.seen.lock().unwrap().generating = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        // A stop that comes after the buffer ran dry is not a controlled stop:
        // the output sat on its last sample until now.
        self.check(Instant::now());
        self.seen.lock().unwrap().generating = false;
        self.reset();
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        if let Some(t0) = self.started {
            // The block is there when the clock has acquired it.
            let frames = (out.len() / self.n_in.max(1)).min(BLOCK) as u64;
            let due = t0 + Duration::from_secs_f64((self.read + frames) as f64 / FS_HZ);
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            self.read += n as u64;
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let now = Instant::now();
        self.check(now);
        if self.dry {
            return Ok(0);
        }
        let space = self.buffer_frames - self.queued(now).min(self.buffer_frames);
        let n = (drive.len() / self.n_out).min(space as usize);
        if n == 0 {
            return Ok(0);
        }
        let took = self.inner.write_drive(&drive[..n * self.n_out])?;
        assert_eq!(took, n, "the simulator's queue is sized never to refuse");
        if self.started.is_some() {
            if let Some(last) = self.last_write {
                let gap = now.duration_since(last).as_secs_f64() * 1e3;
                let mut s = self.seen.lock().unwrap();
                s.longest_gap_ms = s.longest_gap_ms.max(gap);
            }
        }
        self.last_write = Some(now);
        self.written += n as u64;
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        let now = Instant::now();
        self.check(now);
        if self.dry {
            return Some(0);
        }
        Some((self.buffer_frames - self.queued(now).min(self.buffer_frames)) as usize)
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.reset();
        self.inner.emergency_stop()
    }
}

/// The bench's 4 by 4 test: 2048 points, 16 frames a loop at 50% overlap.
fn bench_service(seen: Arc<Mutex<Seen>>) -> MimoRandomService {
    let grid = Grid {
        fs_hz: FS_HZ,
        nfft: 2048,
        band_hz: (160.0, 1600.0),
    };
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.estimation.frames_per_loop = 16;
    cfg.estimation.overlap = 0.5;
    cfg.identification.noise_frames = 16;
    cfg.identification.frames = 32;
    let sim = SimConfig::new()
        .sample_rate_hz(FS_HZ)
        .plant(p.model.clone())
        .noise_level(1e-4)
        .shared_clock(true)
        .seed(0x51_4D_00)
        // Deep enough that the simulator never refuses what the modelled buffer took.
        .output_buffer_frames(1 << 16)
        .build();
    let station = RealTimeStation::new(Box::new(sim), 4, 4, seen);
    let mut loop_cfg = grid.loop_config();
    loop_cfg.block_frames = BLOCK;
    MimoRandomService::new(
        cfg,
        table,
        loop_cfg,
        Box::new(station),
        StationClass::HertzforgeSim,
        data_root("output-seams"),
    )
    .expect("the service builds")
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn identification_keeps_a_real_time_output_fed_across_every_seam() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut svc = bench_service(seen.clone());
    svc.start().expect("feasibility and arm");
    svc.run_noise_floor().expect("the noise floor");
    assert_eq!(svc.state(), MimoState::SysId, "{:?}", svc.trip());
    let r = svc.run_identification();
    let s = seen.lock().unwrap().clone();
    let coverage_ms = (BLOCK * (LOOKAHEAD_BLOCKS + 1)) as f64 / FS_HZ * 1e3;
    eprintln!(
        "longest host gap between accepted drive writes {:.1} ms against a {coverage_ms:.0} ms \
         buffer; ran dry (frames generated, ms since the last write): {:?}",
        s.longest_gap_ms, s.ran_dry
    );
    assert!(
        r.is_ok() && svc.trip().is_none() && svc.state() == MimoState::Gate,
        "identification ended {:?} in {:?} with trip {:?}; the output ran dry (frames \
         generated, ms since the last write): {:?}",
        r,
        svc.state(),
        svc.trip(),
        s.ran_dry
    );
    assert!(s.ran_dry.is_none(), "{:?}", s.ran_dry);
}

/// The default law, with a first solve that takes `first_solve` of real time.
struct SlowFirstSolve {
    inner: Box<dyn MimoControlLaw + Send>,
    first_solve: Duration,
    seen: Arc<Mutex<Seen>>,
    /// Whether the station was generating when the first solve began.
    generating_at_seed: Arc<Mutex<Option<bool>>>,
}

impl MimoControlLaw for SlowFirstSolve {
    fn needs(&self) -> &[MimoInput] {
        self.inner.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.inner.shapes()
    }
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        *self.generating_at_seed.lock().unwrap() = Some(self.seen.lock().unwrap().generating);
        std::thread::sleep(self.first_solve);
        self.inner.seed(inputs)
    }
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.inner.iterate(inputs)
    }
}

struct SlowFirstSolveFactory {
    inner: Arc<dyn MimoLawFactory>,
    first_solve: Duration,
    seen: Arc<Mutex<Seen>>,
    generating_at_seed: Arc<Mutex<Option<bool>>>,
}

impl MimoLawFactory for SlowFirstSolveFactory {
    fn identity(&self) -> LawIdentity {
        self.inner.identity()
    }
    fn params(&self) -> &[Param] {
        self.inner.params()
    }
    fn build(
        &self,
        setup: &MimoSetup,
        budgets: LawBudgets,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(SlowFirstSolve {
            inner: self.inner.build(setup, budgets)?,
            first_solve: self.first_solve,
            seen: self.seen.clone(),
            generating_at_seed: self.generating_at_seed.clone(),
        }))
    }
}

/// Mutation: remove the stop at the end of `run_identification`
/// (`drive_discontinuity`). The station is then still generating when the first
/// solve begins, and the stop at engage finds its buffer long dry.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn the_output_is_stopped_from_the_end_of_identification_until_the_loop_engages() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let generating_at_seed = Arc::new(Mutex::new(None));
    let svc = bench_service(seen.clone());
    let inner = hertzforge_mimo_random::law::builtin_factory(&svc.config().law, 4).unwrap();
    let mut svc = svc.with_law_factory(Arc::new(SlowFirstSolveFactory {
        inner,
        first_solve: Duration::from_millis(500),
        seen: seen.clone(),
        generating_at_seed: generating_at_seed.clone(),
    }));
    svc.start().expect("feasibility and arm");
    svc.run_noise_floor().expect("the noise floor");
    svc.run_identification().expect("identification");
    assert_eq!(svc.state(), MimoState::Gate, "{:?}", svc.trip());
    assert!(
        !seen.lock().unwrap().generating,
        "the station is stopped when identification returns"
    );
    // The operator reads the gate.
    std::thread::sleep(Duration::from_millis(200));
    svc.proceed().expect("the first solve and the prediction");
    assert_eq!(svc.state(), MimoState::Predict);
    svc.proceed().expect("engage");
    // The first control segment starts the station cold.
    svc.loop_tick().expect("the first loop");
    let s = seen.lock().unwrap().clone();
    assert_eq!(
        *generating_at_seed.lock().unwrap(),
        Some(false),
        "the first solve ran with the station stopped"
    );
    assert!(
        s.ran_dry.is_none(),
        "the output ran dry (frames generated, ms since the last write): {:?}",
        s.ran_dry
    );
    assert!(
        svc.trip().is_none() && svc.state().is_running(),
        "the loop ended {:?} with trip {:?}",
        svc.state(),
        svc.trip()
    );
}
