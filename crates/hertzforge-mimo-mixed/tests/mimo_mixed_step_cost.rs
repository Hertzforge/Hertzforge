//! The run's per-frame step: it allocates nothing in its steady state, and
//! its cost at 12 tones on 24 controls, 8 limits and 12 drives (the largest
//! run) against the frame period.

use hertzforge_engine::ProcessStep;
use hertzforge_mimo_mixed::service::StepCostProbe;
use hertzforge_mimo_sine::law::{
    LawDescriptor, LawManifest, LawSetup, LawValues, MimoSineLaw, MimoSineLawFactory,
};
use hertzforge_mimo_sine::tick::{LawFault, TickInputs, TickOutputs};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const FS: f64 = 16384.0;
const BLOCK: usize = 1024;
/// Ten seconds: every tone's tracker settled and its law engaged.
const WARM_BLOCKS: usize = 160;

fn probe(n: usize, m: usize, p: usize, tones: usize) -> StepCostProbe {
    let law = hertzforge_mimo_sine::laws::builtin("matrix").expect("matrix");
    let law: Arc<dyn MimoSineLawFactory> = Arc::new(Uncounted(law));
    StepCostProbe::new(FS, n, m, p, tones, law).expect("probe")
}

/// A tone law whose own calls are not counted: the built-in laws' mathematics
/// allocates per call (it is MIMO Sine's, called on its loop the same way);
/// what is counted is the step around it.
struct Uncounted(Arc<dyn MimoSineLawFactory>);
struct UncountedLaw(Box<dyn MimoSineLaw + Send>);

static LAW_CALLS: AtomicUsize = AtomicUsize::new(0);

fn uncounted<T>(f: impl FnOnce() -> T) -> T {
    LAW_CALLS.fetch_add(1, Ordering::Relaxed);
    let was = COUNTING.with(|c| c.replace(false));
    let r = f();
    COUNTING.with(|c| c.set(was));
    r
}

impl MimoSineLaw for UncountedLaw {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        uncounted(|| self.0.engage(x, o, v))
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        uncounted(|| self.0.tick(x, o, v))
    }
}

impl MimoSineLawFactory for Uncounted {
    fn build(&self, setup: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(UncountedLaw(self.0.build(setup)?)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

/// Run `blocks` blocks through the step, feeding it between blocks; each
/// input channel reads the drive it shares an index with (mod the drives),
/// a block late, so every tone is read and its law has a loop to close.
fn run(
    pr: &mut StepCostProbe,
    blocks: usize,
    input: &mut [f32],
    out: &mut Vec<f32>,
    counted: bool,
) -> usize {
    let mut allocs = 0;
    let (n_in, m) = (pr.n_in, out.capacity() / BLOCK);
    for _ in 0..blocks {
        pr.feed(BLOCK);
        let before = ALLOCS.load(Ordering::Relaxed);
        COUNTING.with(|c| c.set(counted));
        pr.step.process(input, BLOCK, out);
        COUNTING.with(|c| c.set(false));
        allocs += ALLOCS.load(Ordering::Relaxed) - before;
        for f in 0..BLOCK {
            for ch in 0..n_in {
                input[f * n_in + ch] = out[f * m + ch % m];
            }
        }
    }
    allocs
}

/// Requirement: after ten seconds of warm-up, 4 s of frames (every tone's
/// drive and tracking, 400 control ticks with each tone's law called, the
/// ceiling rule and the hand-over) through the step on 4 controls and 4
/// drives with 4 tones allocate nothing outside the law's own calls.
#[test]
fn mimo_mixed_step_does_not_allocate() {
    let mut pr = probe(4, 4, 0, 4);
    let mut input = vec![0.0f32; BLOCK * pr.n_in];
    let mut out = Vec::with_capacity(BLOCK * 4);
    run(&mut pr, WARM_BLOCKS, &mut input, &mut out, false);
    let calls0 = LAW_CALLS.load(Ordering::Relaxed);
    let allocs = run(&mut pr, 64, &mut input, &mut out, true);
    let calls = LAW_CALLS.load(Ordering::Relaxed) - calls0;
    println!(
        "allocations over {} frames: {allocs}; law calls {calls}",
        64 * BLOCK
    );
    assert!(calls > 0, "no law was called");
    assert_eq!(allocs, 0);
}

/// Requirement: the step's mean cost per frame at 12 tones, 24 controls,
/// 8 limits and 12 drives is under the frame period at 16384 Hz (61.0 us).
#[test]
fn mimo_mixed_step_cost_at_twelve_tones_is_under_the_frame_period() {
    let (n, m, p) = (24, 12, 8);
    let mut pr = probe(n, m, p, 12);
    let mut input = vec![0.0f32; BLOCK * pr.n_in];
    let mut out = Vec::with_capacity(BLOCK * m);
    run(&mut pr, WARM_BLOCKS, &mut input, &mut out, false);
    let blocks = 64;
    let calls0 = LAW_CALLS.load(Ordering::Relaxed);
    let t0 = std::time::Instant::now();
    run(&mut pr, blocks, &mut input, &mut out, false);
    let per_frame = t0.elapsed().as_secs_f64() / (blocks * BLOCK) as f64;
    let calls = LAW_CALLS.load(Ordering::Relaxed) - calls0;
    println!(
        "law calls {calls} (12 tones over {} ticks)",
        blocks * BLOCK / 164
    );
    let period = 1.0 / FS;
    println!(
        "step cost {:.2} us a frame against {:.2} us ({:.1} %)",
        per_frame * 1e6,
        period * 1e6,
        100.0 * per_frame / period
    );
    assert!(per_frame < period);
}
