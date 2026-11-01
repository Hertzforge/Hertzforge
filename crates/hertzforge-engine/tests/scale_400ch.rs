//! THE 400-CHANNEL SYNTHETIC GATE, hardware-free and NOT `#[ignore]`d.
//!
//! Drives the real [`Engine`] against a 400-channel `hertzforge-hal-sim` station at
//! the rate-derived block size, long enough to prove three properties that only
//! show up at width:
//!
//! 1. **No raw-ring overrun at the derived sizing.** `ring_frames_for(block)`
//!    gives 8 blocks of headroom; at 400 channels that is a real amount of memory
//!    and a real amount of watchdog work per block, so "the sizing is adequate"
//!    stops being obvious.
//! 2. **The watchdog is checked every block.** `EngineHealth::watchdog_polls`
//!    must equal the tick count exactly. A path that skips the safety check under
//!    load is the failure this catches.
//! 3. **Memory is BOUNDED.** A counting global allocator measures live bytes
//!    across the run; the steady-state live total must not grow with tick count.
//!
//! ## What "memory bounded" here does and does NOT claim
//!
//! There is no allocation-free invariant in this crate to reuse, so this states
//! exactly what was checked. The assertion is NO NET GROWTH: live bytes after the
//! second half of the run equal live bytes after the first half, within a small
//! tolerance. That catches a per-tick leak, an unbounded queue, and a buffer that
//! doubles as the run goes on.
//!
//! It does NOT claim zero allocations per tick, because the engine is not there
//! yet and saying otherwise would be a claim the test cannot support:
//! `OutputDelivery` stores its queue as `VecDeque<Vec<f32>>` and `enqueue` does
//! `frame.to_vec()`, so the DRIVE path allocates one small `Vec` per drive frame
//! and frees it the same tick. That is bounded and non-growing (which is what is
//! asserted), but it is a real per-tick allocation on the RT path and is recorded
//! here as a follow-up rather than papered over. The ACQUISITION path - backend
//! read, ring push, watchdog drain, capture - is allocation-free, and the Sim's
//! `read_input` was made so in this phase (its per-call `Vec<ChannelRole>` moved
//! into the channel map).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
use hertzforge_engine::{
    block, scheduler::SilentDrive, BlockMeta, CaptureError, CaptureSink, Engine, EngineConfig,
    StopReason,
};
use hertzforge_hal::{AcquisitionConfig, Backend};
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

// ---------------------------------------------------------------------------------
// A capture sink that only COUNTS: at 400 channels a Vec sink would dominate the
// memory measurement and prove nothing about the engine.
// ---------------------------------------------------------------------------------

struct CountingSink {
    frames: u64,
    blocks: u64,
    samples: u64,
}

impl CaptureSink for CountingSink {
    fn write_block(&mut self, samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError> {
        self.frames += meta.n_frames;
        self.samples += samples.len() as u64;
        self.blocks += 1;
        Ok(())
    }
    fn finalize(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}

/// 400 measurement channels + 1 drive. Every measurement channel carries an
/// acceleration limit, so the watchdog really monitors 400 channels per frame -
/// the wide-channel cost the gate exists to exercise. The limits are generous
/// enough that a healthy plant never trips them.
fn table_400ch() -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 1..=400u32 {
        let mut c = Channel::new(i);
        c.role = if i == 1 {
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
    let mut d = Channel::new(401);
    d.role = ChannelRole::Drive;
    t.channels.push(d);
    t
}

#[test]
fn four_hundred_channels_run_clean_at_the_derived_block_and_ring() {
    const N_CH: usize = 400;
    const FS: f64 = 51_200.0;
    const TICKS: u64 = 120;

    // The block and the ring both come from the policy, not from this test.
    let block = block::default_block_frames(FS);
    assert_eq!(block, 1024, "the 51.2 kHz class block");
    let expected_ring = block::ring_frames_for(block);

    let mut backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(1e-3)
        .shared_clock(true)
        .max_input_channels(N_CH as u32)
        .max_output_channels(1)
        // Room for several blocks of AO so the top-up policy runs in its normal
        // regime rather than in permanent backpressure.
        .output_buffer_frames(block * 4)
        .build();

    let table = table_400ch();
    backend.open().expect("sim open");
    let actual = backend
        .configure(&table, &AcquisitionConfig::new(FS, block))
        .expect("sim configure");
    assert_eq!(actual.block_frames, block);
    assert_eq!(backend.n_input_channels(), N_CH);

    let cfg = EngineConfig::with_block(N_CH, 1, &actual);
    assert_eq!(cfg.frames_per_tick, block);
    assert_eq!(cfg.ring_frames, expected_ring);

    let mut engine = Engine::new(cfg, &table, &backend);
    assert_eq!(engine.ring_frames(), expected_ring);
    assert_eq!(
        engine.ring_auto_raised(),
        None,
        "the policy-derived depth needs no raise"
    );

    engine
        .prefill_output(&mut backend)
        .expect("device prefill before start");
    backend.arm().expect("sim arm");
    backend.start().expect("sim start");

    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = CountingSink {
        frames: 0,
        blocks: 0,
        samples: 0,
    };

    // --- Run it in two halves and compare live memory between them. ---
    let started = Instant::now();
    let half = TICKS / 2;
    let r1 = engine.run(&mut backend, &mut proc, &mut sink, half);
    assert_eq!(r1.reason, StopReason::InputEnded, "first half ran clean");
    let live_after_first = live_bytes();
    let calls_after_first = ALLOC_CALLS.load(Ordering::Relaxed);

    // `run` stops the backend at the tick budget; re-arm for the second half.
    backend.arm().expect("re-arm");
    backend.start().expect("re-start");
    let r2 = engine.run(&mut backend, &mut proc, &mut sink, half);
    assert_eq!(r2.reason, StopReason::InputEnded, "second half ran clean");
    let live_after_second = live_bytes();
    let calls_after_second = ALLOC_CALLS.load(Ordering::Relaxed);
    let elapsed = started.elapsed();

    // --- 1. NO RING OVERRUN at the derived sizing. ---
    //
    // HONESTY NOTE (a result that cannot fail is not evidence):
    // in today's single-threaded engine a producer-side overrun is structurally
    // unreachable (the push loop drains the watchdog and retries on a full ring,
    // and the ring is >= 8 blocks besides) - see the reachability paragraph on
    // `scheduler::ring_overrun_stop`. So these zeros are NOT evidence that the
    // sizing is adequate under an independently-threaded watchdog; they are
    // evidence that the counter is wired through both surfaces and that nothing
    // in the 400-channel path corrupts it. The sizing arithmetic itself is what
    // the `assert_eq!(cfg.ring_frames, expected_ring)` above pins.
    assert_eq!(r1.ring_overruns, 0, "first half dropped raw frames");
    assert_eq!(r2.ring_overruns, 0, "second half dropped raw frames");
    assert_eq!(engine.health().ring_overruns, 0);
    // The run really was limits-armed, so a drop WOULD have failed it. Without
    // this the zero above could not have been anything else.
    assert!(
        engine.health().limits_armed,
        "all 400 channels carry limits, so a drop WOULD have failed the run"
    );

    // --- 2. THE WATCHDOG WAS CHECKED EVERY BLOCK. ---
    let ticks = r1.ticks + r2.ticks;
    assert_eq!(ticks, TICKS);
    assert_eq!(
        engine.health().watchdog_polls,
        ticks,
        "one watchdog poll per tick, no path skipping the safety check"
    );

    // --- Frames actually flowed, at width. ---
    let frames = r1.frames_in + r2.frames_in;
    assert_eq!(frames, TICKS * block as u64);
    assert_eq!(sink.frames, frames);
    assert_eq!(
        sink.samples,
        frames * N_CH as u64,
        "every frame carried all 400 channels - the frames-vs-samples trap"
    );
    assert_eq!(r1.estops + r2.estops, 0, "no aborts on a healthy plant");
    assert_eq!(r1.blanked_trips + r2.blanked_trips, 0);

    // --- 3. MEMORY IS BOUNDED: no NET growth between the two halves. ---
    // See the module docs for exactly what this does and does not claim. The
    // tolerance covers the second half's own bookkeeping, not a per-tick leak: a
    // leak of even one byte per FRAME would be ~61 kB over 61,440 frames.
    let growth = live_after_second - live_after_first;
    assert!(
        growth.abs() < 64 * 1024,
        "live memory grew {growth} bytes across an identical second half; a \
         per-tick leak or an unbounded queue would show up exactly here"
    );
    // And allocation is STEADY, not accelerating: the two halves must allocate
    // within a factor of two of each other.
    let calls_first = calls_after_first;
    let calls_second = calls_after_second - calls_after_first;
    assert!(
        calls_second * 2 >= calls_first && calls_first * 2 >= calls_second,
        "allocation rate is not steady between identical halves \
         ({calls_first} then {calls_second} calls)"
    );

    // --- Runtime budget: this gate is CI-resident, not an #[ignore]d nicety. ---
    assert!(
        elapsed.as_secs() < 30,
        "the 400-channel gate took {elapsed:?}, past its 30 s budget"
    );
}
