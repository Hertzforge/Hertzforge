//! The per-frame and per-tick calls allocate nothing in their steady state:
//! the frame transform, the matrix floor estimator's and the band
//! estimate's frame updates, the tone bank's drive and tracking frames and
//! its per-tick setters, the headroom and both ceiling rules.
//!
//! Every allocation made on this thread while the counter is on is counted.

mod common;

use common::{c, Rng};
use hertzforge_mimo_mixed::band::MovingFrameEstimate;
use hertzforge_mimo_mixed::budget::{headroom, CeilingRule, CommonScale, InTurn};
use hertzforge_mimo_mixed::estimate::{FrameSpectra, MatrixFloorEstimator};
use hertzforge_mimo_mixed::tone::{ToneBank, ToneLock};
use hertzforge_mimo_random::config::EstimationWindow;
use hertzforge_sdm::C64;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// The allocations `f` makes on this thread.
fn counted(f: impl FnOnce()) -> usize {
    let before = ALLOCS.load(Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCS.load(Ordering::Relaxed) - before
}

#[test]
fn mimo_mixed_frame_estimators_do_not_allocate() {
    let (n, nfft, fs) = (3, 256, 256.0);
    let lines = nfft / 2 + 1;
    let mut fsp = FrameSpectra::new(nfft, fs, n, lines, EstimationWindow::Hann);
    let mut est = MatrixFloorEstimator::new(n, lines, 4.0, 64);
    let mut band = MovingFrameEstimate::new(n, 3, 16);
    let mut rng = Rng::new(1);
    let frames: Vec<Vec<f64>> = (0..n)
        .map(|_| (0..nfft).map(|_| rng.normal()).collect())
        .collect();
    let mut excluded = vec![false; lines];
    excluded[40] = true;
    // Warm: the first loop and first frames.
    for _ in 0..4 {
        fsp.transform(|ch| &frames[ch][..]);
        est.add_frame(fsp.spectra(), &excluded);
        band.add_frame(fsp.spectra(), 41.0, 1.0, &excluded);
    }
    est.end_loop();
    let allocs = counted(|| {
        for f in 0..200 {
            fsp.transform(|ch| &frames[ch][..]);
            est.add_frame(fsp.spectra(), &excluded);
            band.add_frame(fsp.spectra(), 30.0 + 0.1 * f as f64, 1.0, &excluded);
        }
    });
    assert_eq!(allocs, 0, "the frame estimators allocated {allocs} times");
}

#[test]
fn mimo_mixed_tone_bank_frames_do_not_allocate() {
    let fs = 2048.0;
    let (m, ch) = (2, 3);
    let mut bank = ToneBank::new(
        fs,
        &[100.0, 200.0, 333.0],
        &[
            ToneLock::Free,
            ToneLock::Follow {
                leader: 0,
                ratio: 2.0,
            },
            ToneLock::Free,
        ],
        m,
        ch,
        0.5,
        2,
    )
    .unwrap();
    let d = [c(0.3, 0.1), c(-0.2, 0.4)];
    for k in 0..3 {
        bank.set_part_level(k, 1.0, 100.0);
        bank.set_drive(k, &d, 0);
    }
    let mut out = [0.0; 2];
    let mut res = [0.0; 3];
    let input = [0.1, -0.2, 0.05];
    bank.drive_frame(1.0, &mut out);
    bank.track_frame(&input, &mut res);
    let allocs = counted(|| {
        for s in 0..4096 {
            if s % 32 == 0 {
                // The per-tick calls: a sweep's next frequency, the law's
                // new drive vector with its ramp, a part's level.
                bank.set_frequency(2, 333.0 + s as f64 * 1e-3);
                bank.set_drive(1, &d, 16);
                bank.set_part_level(2, 0.5, 64.0);
            }
            out = [0.0; 2];
            bank.drive_frame(1.0, &mut out);
            bank.track_frame(&input, &mut res);
        }
    });
    assert_eq!(allocs, 0, "the tone bank allocated {allocs} times");
}

#[test]
fn mimo_mixed_ceiling_rules_do_not_allocate() {
    let drives: Vec<C64> = vec![
        c(3.0, 0.0),
        c(1.0, 0.0),
        c(2.0, 0.0),
        c(1.0, 0.0),
        c(1.0, 0.0),
        c(4.0, 0.0),
    ];
    let mut room = [0.0; 2];
    let mut scales = [0.0; 3];
    let mut limiting = [false; 3];
    let mut common = CommonScale;
    let mut turn = InTurn;
    let allocs = counted(|| {
        for i in 0..1000 {
            headroom(0.5 + i as f64 * 1e-4, &[10.0, 10.0], &[5.0, 4.0], &mut room);
            common.apply(&drives, 3, &room, &mut scales, &mut limiting);
            turn.apply(&drives, 3, &room, &mut scales, &mut limiting);
        }
    });
    assert_eq!(allocs, 0, "the ceiling rules allocated {allocs} times");
}
