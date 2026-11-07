//! THE MULTIPLE-OUTPUT TIMING GATE: 24 drives and 96 inputs on a cross-coupled plant.
//!
//! The 400-channel gate (`scale_400ch.rs`) proves the input side scales but drives one
//! output through a trivial step. This one runs the real [`Engine`] at the width of a
//! full multiple-output test, against a simulator plant in which every drive reaches
//! every input (a seeded 96-input by 24-drive modal plant, 40 modes from 20 Hz to
//! 2 kHz), with a process step that does the spectral-matrix work such a test does on
//! every block:
//!
//! - synthesizes 24 independent drive streams (the drive width the engine delivers);
//! - transforms the block of each of the 72 control inputs and of each of the 24
//!   drives it sent the block before (Hann window, block-length FFT, 513 lines);
//! - adds the block into a packed 72 x 72 response spectral matrix and a 72 x 24
//!   response-drive cross spectrum on every line: 4,356 complex multiply-adds per line,
//!   2.2 million per block.
//!
//! It runs at 51.2 kHz with the rate-derived block (1,024 frames, a 20 ms tick) under
//! the engine's DEFAULT scheduling guard (compute budget = the tick period, the period
//! jitter check armed), so a step that cannot keep up ends the run with a scheduling
//! fault, and asserts the same run rules as the 400-channel gate: a clean end, no ring
//! overrun with limits armed on all 96 inputs, one watchdog poll per tick, every frame
//! at full width, no net memory growth and a steady allocation rate between two
//! identical halves, and a wall-clock budget. It prints the per-tick cost: the process
//! step's mean and worst, the engine's worst compute against its budget, and the whole
//! tick (simulator plant included) against the tick period.
//!
//! RELEASE ONLY. Its verdict depends on the wall clock against a real-time budget, and
//! an unoptimized build of this step is several times the tick period, so in a debug
//! build it is ignored (the timing pass counts it as such) rather than loosened. Run it
//! with `cargo test --release -p hertzforge-engine --test scale_mimo`, or the whole
//! timing pass with `scripts/rust_tests.sh timing --release`. The simulator runs flat
//! out, so this measures cost per tick, not real-time pacing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
use hertzforge_dsp::fft::{self, Cplx};
use hertzforge_engine::{
    block, scheduler::ProcessStep, BlockMeta, CaptureError, CaptureSink, Engine, EngineConfig,
    StopReason,
};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::plants::{random_modal, RandomModalSpec};
use hertzforge_hal_sim::SimConfig;

// ---------------------------------------------------------------------------------
// A counting allocator: live bytes = allocated - freed.
// ---------------------------------------------------------------------------------

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicUsize = AtomicUsize::new(0);
static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn live_bytes() -> isize {
    ALLOCATED.load(Ordering::Relaxed) as isize - FREED.load(Ordering::Relaxed) as isize
}

struct CountingSink {
    frames: u64,
    samples: u64,
}

impl CaptureSink for CountingSink {
    fn write_block(&mut self, samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError> {
        self.frames += meta.n_frames;
        self.samples += samples.len() as u64;
        Ok(())
    }
    fn finalize(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}

const N_CONTROL: usize = 72;
const N_MONITOR: usize = 24;
const N_IN: usize = N_CONTROL + N_MONITOR;
const N_DRIVE: usize = 24;

/// 72 control and 24 monitor inputs, every one with an acceleration limit (so the
/// watchdog checks all 96 per frame), then 24 drives.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..N_IN {
        let mut c = Channel::new(i as u32 + 1);
        c.role = if i < N_CONTROL {
            ChannelRole::Control
        } else {
            ChannelRole::Monitor
        };
        c.abort = AbortLimits {
            accel_g: Some(1.0e6),
            velocity_ips: None,
            displacement_mil: None,
        };
        t.channels.push(c);
    }
    for j in 0..N_DRIVE {
        let mut d = Channel::new((N_IN + j) as u32 + 1);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
    }
    t
}

/// The per-block spectral-matrix step (see the module docs). Everything is allocated
/// up front; `process` allocates nothing.
struct MatrixStep {
    nfft: usize,
    lines: usize,
    rng: u64,
    window: Vec<f64>,
    /// The drive sent last block, per drive, `N_DRIVE x nfft`.
    sent: Vec<f32>,
    scratch: Vec<Cplx>,
    /// Transforms by line: `y[k * N_CONTROL + a]`, `x[k * N_DRIVE + j]`.
    y: Vec<Cplx>,
    x: Vec<Cplx>,
    /// Packed upper triangle per line, and the full cross spectrum per line.
    gyy: Vec<Cplx>,
    gyx: Vec<Cplx>,
    blocks: u64,
    busy: Duration,
    worst: Duration,
}

impl MatrixStep {
    fn new(nfft: usize) -> Self {
        let lines = nfft / 2 + 1;
        MatrixStep {
            nfft,
            lines,
            rng: 0x5CA1_E0F7_1A7E_0001,
            window: hertzforge_dsp::window::hann(nfft),
            sent: vec![0.0; N_DRIVE * nfft],
            scratch: vec![(0.0, 0.0); nfft],
            y: vec![(0.0, 0.0); lines * N_CONTROL],
            x: vec![(0.0, 0.0); lines * N_DRIVE],
            gyy: vec![(0.0, 0.0); lines * N_CONTROL * (N_CONTROL + 1) / 2],
            gyx: vec![(0.0, 0.0); lines * N_CONTROL * N_DRIVE],
            blocks: 0,
            busy: Duration::ZERO,
            worst: Duration::ZERO,
        }
    }

    #[inline]
    fn uniform(&mut self) -> f32 {
        let mut s = self.rng;
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        self.rng = s;
        let u = (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
        (u - 0.5) as f32
    }

    fn transform(&mut self, samples: impl Fn(usize) -> f64, dst_col: usize, width: usize, x: bool) {
        for (n, s) in self.scratch.iter_mut().enumerate() {
            *s = (self.window[n] * samples(n), 0.0);
        }
        fft::fft_in_place(&mut self.scratch);
        let dst = if x { &mut self.x } else { &mut self.y };
        for k in 0..self.lines {
            dst[k * width + dst_col] = self.scratch[k];
        }
    }
}

impl ProcessStep for MatrixStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        let t0 = Instant::now();
        // The drive: 24 independent streams, interleaved.
        drive_out.clear();
        for _ in 0..n_frames * N_DRIVE {
            let v = 0.1 * self.uniform();
            drive_out.push(v);
        }
        if n_frames == self.nfft {
            // Transform every control input and every drive sent the block before.
            for a in 0..N_CONTROL {
                self.transform(|n| input[n * N_IN + a] as f64, a, N_CONTROL, false);
            }
            for j in 0..N_DRIVE {
                let sent = std::mem::take(&mut self.sent);
                let nfft = self.nfft;
                self.transform(|n| sent[j * nfft + n] as f64, j, N_DRIVE, true);
                self.sent = sent;
            }
            for f in 0..n_frames {
                for j in 0..N_DRIVE {
                    self.sent[j * self.nfft + f] = drive_out[f * N_DRIVE + j];
                }
            }
            // Accumulate G_yy (packed upper triangle) and G_yx on every line.
            let tri = N_CONTROL * (N_CONTROL + 1) / 2;
            for k in 0..self.lines {
                let y = &self.y[k * N_CONTROL..(k + 1) * N_CONTROL];
                let x = &self.x[k * N_DRIVE..(k + 1) * N_DRIVE];
                let g = &mut self.gyy[k * tri..(k + 1) * tri];
                let mut o = 0;
                for a in 0..N_CONTROL {
                    let (ar, ai) = y[a];
                    for &(br, bi) in &y[a..] {
                        g[o].0 += ar * br + ai * bi;
                        g[o].1 += ai * br - ar * bi;
                        o += 1;
                    }
                }
                let c = &mut self.gyx[k * N_CONTROL * N_DRIVE..(k + 1) * N_CONTROL * N_DRIVE];
                for (a, &(ar, ai)) in y.iter().enumerate() {
                    for (cell, &(br, bi)) in c[a * N_DRIVE..(a + 1) * N_DRIVE].iter_mut().zip(x) {
                        cell.0 += ar * br + ai * bi;
                        cell.1 += ai * br - ar * bi;
                    }
                }
            }
            self.blocks += 1;
        }
        let dt = t0.elapsed();
        self.busy += dt;
        self.worst = self.worst.max(dt);
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-mode timing gate: cargo test --release -p hertzforge-engine --test scale_mimo"
)]
fn mimo_24_drives_96_inputs_run_clean_on_a_modal_plant_with_a_matrix_step() {
    const FS: f64 = 51_200.0;
    const TICKS: u64 = 240;

    let block = block::default_block_frames(FS);
    assert_eq!(block, 1024, "the 51.2 kHz class block");
    let tick_period = Duration::from_secs_f64(block as f64 / FS);

    let plant = random_modal(&RandomModalSpec {
        n_inputs: N_IN,
        n_drives: N_DRIVE,
        n_modes: 40,
        seed: 0x5CA1_E96D_2400_0001,
        ..RandomModalSpec::default()
    });
    let mut backend = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(plant.model)
        .noise_level(1e-4)
        .shared_clock(true)
        .output_buffer_frames(block * 4)
        .build();
    let table = table();
    backend.open().expect("sim open");
    let actual = backend
        .configure(&table, &AcquisitionConfig::new(FS, block))
        .expect("sim configure");
    assert_eq!(backend.n_input_channels(), N_IN);
    assert_eq!(backend.n_drive_channels(), N_DRIVE);

    // The DEFAULT scheduling guard: nothing relaxed.
    let cfg = EngineConfig::with_block(N_IN, N_DRIVE, &actual);
    assert_eq!(cfg.frames_per_tick, block);
    let mut engine = Engine::new(cfg, &table, &backend);
    engine
        .prefill_output(&mut backend)
        .expect("device prefill before start");
    backend.arm().expect("sim arm");
    backend.start().expect("sim start");

    let mut step = MatrixStep::new(block);
    let mut sink = CountingSink {
        frames: 0,
        samples: 0,
    };

    let started = Instant::now();
    let half = TICKS / 2;
    let r1 = engine.run(&mut backend, &mut step, &mut sink, half);
    assert_eq!(r1.reason, StopReason::InputEnded, "first half ran clean");
    let live_after_first = live_bytes();
    let calls_after_first = ALLOC_CALLS.load(Ordering::Relaxed);
    backend.arm().expect("re-arm");
    backend.start().expect("re-start");
    let r2 = engine.run(&mut backend, &mut step, &mut sink, half);
    assert_eq!(r2.reason, StopReason::InputEnded, "second half ran clean");
    let live_after_second = live_bytes();
    let calls_after_second = ALLOC_CALLS.load(Ordering::Relaxed);
    let elapsed = started.elapsed();

    let health = engine.health();
    let ticks = r1.ticks + r2.ticks;
    let mean_step = step.busy / step.blocks.max(1) as u32;
    let wall_per_tick = elapsed / ticks.max(1) as u32;
    println!(
        "24 drives x 96 inputs at {FS} Hz, block {block} (tick period {:.2} ms), {ticks} ticks:",
        tick_period.as_secs_f64() * 1e3
    );
    println!(
        "  matrix step: mean {:.3} ms, worst {:.3} ms ({:.1} % and {:.1} % of the tick period)",
        mean_step.as_secs_f64() * 1e3,
        step.worst.as_secs_f64() * 1e3,
        100.0 * mean_step.as_secs_f64() / tick_period.as_secs_f64(),
        100.0 * step.worst.as_secs_f64() / tick_period.as_secs_f64()
    );
    println!(
        "  engine worst compute {:.3} ms against a budget of {:.3} ms",
        health.worst_compute.as_secs_f64() * 1e3,
        health.compute_budget.as_secs_f64() * 1e3
    );
    println!(
        "  whole tick, simulator plant included: {:.3} ms ({:.1} % of the tick period); {:.2} s for the run",
        wall_per_tick.as_secs_f64() * 1e3,
        100.0 * wall_per_tick.as_secs_f64() / tick_period.as_secs_f64(),
        elapsed.as_secs_f64()
    );

    assert_eq!(ticks, TICKS);
    assert_eq!(step.blocks, TICKS, "every tick ran the matrix step");
    assert_eq!(r1.ring_overruns + r2.ring_overruns, 0);
    assert!(health.limits_armed, "all 96 inputs carry limits");
    assert_eq!(health.watchdog_polls, ticks, "one watchdog poll per tick");
    assert_eq!(health.health_faults, 0, "no scheduling-health fault");
    let frames = r1.frames_in + r2.frames_in;
    assert_eq!(frames, TICKS * block as u64);
    assert_eq!(sink.frames, frames);
    assert_eq!(
        sink.samples,
        frames * N_IN as u64,
        "every frame at full width"
    );
    assert_eq!(r1.estops + r2.estops, 0, "no aborts on a healthy plant");
    assert!(
        health.worst_compute < health.compute_budget,
        "worst compute {:?} reached the budget {:?}",
        health.worst_compute,
        health.compute_budget
    );

    let growth = live_after_second - live_after_first;
    assert!(
        growth.abs() < 64 * 1024,
        "live memory grew {growth} bytes across an identical second half"
    );
    let calls_first = calls_after_first;
    let calls_second = calls_after_second - calls_after_first;
    assert!(
        calls_second * 2 >= calls_first && calls_first * 2 >= calls_second,
        "allocation rate is not steady between identical halves \
         ({calls_first} then {calls_second} calls)"
    );
    assert!(
        elapsed.as_secs() < 60,
        "the multiple-output gate took {elapsed:?}, past its 60 s budget"
    );
}
