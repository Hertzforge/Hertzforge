//! The shaker step on its own, sample by sample, with a source of constant ones so each
//! drive sample is the step's gain: an abort and a failed source zero the very next
//! block; an end or an abort takes no later drive start; the ramps have the shared
//! shape; and no f32 handed to the station is past the ceiling.

use hertzforge_engine::ring::{channel, Producer};
use hertzforge_engine::ProcessStep;
use hertzforge_modal_test::host::{
    stop_ramp_frames, DriveBuild, DrivePhase, ModalStep, StepShared,
};
use hertzforge_modal_test::setup::{
    DriveSet, ModalSetup, Mode, SourceKind, CEILING_MARGIN, STOP_RAMP_DEPTH_DB, STOP_RAMP_S,
};
use hertzforge_modal_test::source::{FeedShared, Source, SourceBuild};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const FS: f64 = 1024.0;
const N: usize = 64;

struct Rig {
    step: ModalStep,
    shared: Arc<StepShared>,
    feed: Arc<FeedShared>,
    source: Producer,
    out: Vec<f32>,
}

fn rig(ramp_frames: u64, ceiling_v: f64, value: f32) -> Rig {
    let (tx, rx) = channel(1 << 16, 1);
    let (cap, _cap_rx) = channel(16, 2);
    let feed = Arc::new(FeedShared::default());
    let b = DriveBuild {
        outputs: 1,
        slots: vec![0],
        block: N,
        ramp_frames,
        ceiling_v,
        stop_ramp_frames: stop_ramp_frames(FS),
    };
    let (step, shared) = ModalStep::new(Some((b, rx, feed.clone())), cap, 0);
    let mut r = Rig {
        step,
        shared,
        feed,
        source: tx,
        out: Vec::new(),
    };
    r.fill(1 << 15, value);
    r
}

impl Rig {
    fn fill(&mut self, n: usize, v: f32) {
        for _ in 0..n {
            assert!(self.source.push(&[v]));
        }
    }

    /// Run `n` frames; the drive written.
    fn run(&mut self, n: usize) -> Vec<f32> {
        let input = vec![0f32; n];
        self.step.process(&input, n, &mut self.out);
        self.out.clone()
    }
}

#[test]
fn modal_test_step_zeroes_the_very_next_block_after_an_abort() {
    let mut r = rig(N as u64, 2.0, 1.0);
    r.shared.start.store(true, Ordering::Release);
    r.run(4 * N);
    assert_eq!(r.shared.phase(), DrivePhase::Driving);
    assert!(r.run(N).iter().all(|v| *v == 1.0));
    r.shared.abort.store(true, Ordering::Release);
    let next = r.run(N);
    assert!(
        next.iter().all(|v| *v == 0.0),
        "the block after the abort: {next:?}"
    );
    assert_eq!(r.shared.phase(), DrivePhase::Zeroed);
}

#[test]
fn modal_test_step_zeroes_the_very_next_block_after_a_source_failure() {
    let mut r = rig(N as u64, 2.0, 1.0);
    r.shared.start.store(true, Ordering::Release);
    r.run(4 * N);
    assert_eq!(r.shared.phase(), DrivePhase::Driving);
    // The source thread failed with its ring still full.
    r.feed.failed.store(true, Ordering::Release);
    let next = r.run(N);
    assert!(
        next.iter().all(|v| *v == 0.0),
        "the block after the source failed: {next:?}"
    );
    assert_eq!(r.shared.phase(), DrivePhase::Zeroed);
}

#[test]
fn modal_test_step_takes_no_drive_start_once_ended_or_aborted() {
    for abort in [false, true] {
        let mut r = rig(N as u64, 2.0, 1.0);
        r.run(N);
        // The end (or the abort) is asked, then a start, in the same block and later.
        r.shared.closed.store(true, Ordering::Release);
        r.shared.stop.store(true, Ordering::Release);
        if abort {
            r.shared.abort.store(true, Ordering::Release);
        }
        r.shared.start.store(true, Ordering::Release);
        let a = r.run(4 * N);
        r.shared.start.store(true, Ordering::Release);
        let b = r.run(4 * N);
        assert!(
            a.iter().chain(&b).all(|v| *v == 0.0),
            "abort {abort}: driven after the end"
        );
        assert!(
            matches!(r.shared.phase(), DrivePhase::Idle | DrivePhase::Zeroed),
            "abort {abort}: {:?}",
            r.shared.phase()
        );
    }
}

#[test]
fn modal_test_step_ramps_up_and_stops_by_the_shared_ramp() {
    let ramp = N as u64 * 2;
    let mut r = rig(ramp, 2.0, 1.0);
    r.run(N / 2);
    r.shared.start.store(true, Ordering::Release);
    let up = r.run(N / 2 + ramp as usize + N);
    // Zero to the next frame's start, then the ramp: k / ramp, one step per sample.
    let first = up.iter().position(|v| *v != 0.0).unwrap();
    assert_eq!(first, N / 2 + 1, "the ramp begins on a frame's start");
    for (k, v) in up[N / 2..N / 2 + ramp as usize].iter().enumerate() {
        assert_eq!(*v, (k as f64 / ramp as f64) as f32, "ramp sample {k}");
    }
    assert!(up[N / 2 + ramp as usize..].iter().all(|v| *v == 1.0));
    r.shared.stop.store(true, Ordering::Release);
    let n = stop_ramp_frames(FS) as usize;
    assert_eq!(n, (STOP_RAMP_S * FS).round() as usize);
    let down = r.run(n + N);
    // The shared ramp: every sample down by the same ratio, the depth in dB over its
    // length, then zero; never a step.
    let ratio = 10f64.powf(-STOP_RAMP_DEPTH_DB / 20.0 / n as f64);
    assert_eq!(down[0], 1.0);
    for k in 1..n {
        let (a, b) = (f64::from(down[k - 1]), f64::from(down[k]));
        assert!(b <= a, "monotone at {k}: {a} then {b}");
        assert!(
            (b / a - ratio).abs() < 1e-6,
            "sample {k}: down by {} not {ratio}",
            b / a
        );
        assert!(a - b <= 1.0 - ratio + 1e-6, "step at {k}: {}", a - b);
    }
    let last = f64::from(down[n - 1]);
    let depth = 10f64.powf(-STOP_RAMP_DEPTH_DB / 20.0);
    assert!(
        (last - depth).abs() < depth * 0.01,
        "{last} at the ramp's end, {depth} asked"
    );
    assert!(down[n..].iter().all(|v| *v == 0.0), "zero after the ramp");
    assert_eq!(r.shared.phase(), DrivePhase::Idle);
}

#[test]
fn modal_test_step_never_hands_over_an_f32_past_the_ceiling() {
    // At a third of full gain a sample of one is 1/3 in f64 but rounds above it as the
    // f32 the station is handed: with the ceiling at 1/3 that sample must trip, not go.
    let ceiling = 1.0 / 3.0;
    assert!(f64::from((1.0f64 / 3.0) as f32) > ceiling);
    let mut r = rig(3, ceiling, 1.0);
    r.shared.start.store(true, Ordering::Release);
    let out = r.run(4 * N);
    let peak = out.iter().fold(0f64, |m, v| m.max(f64::from(v.abs())));
    assert!(peak <= ceiling, "{peak} handed over, ceiling {ceiling}");
    assert!(r.shared.ceiling_trip.load(Ordering::Acquire));
}

const KINDS: [SourceKind; 6] = [
    SourceKind::Random,
    SourceKind::BurstRandom,
    SourceKind::PseudoRandom,
    SourceKind::PeriodicRandom,
    SourceKind::PeriodicChirp,
    SourceKind::BurstChirp,
];

/// The levels a test of `kind` drives at: the first just inside the ceiling by the
/// margin, the others lower (one drive for the kinds a single shaker plays).
fn levels(kind: SourceKind, ceiling: f64) -> Vec<f64> {
    let edge = ceiling * (1.0 - CEILING_MARGIN) / kind.crest();
    match kind {
        SourceKind::Random | SourceKind::BurstRandom | SourceKind::PeriodicRandom => {
            vec![edge, edge * 0.6, edge * 0.31]
        }
        _ => vec![edge],
    }
}

fn source(kind: SourceKind, levels: Vec<f64>, seed: u64) -> Source {
    Source::new(SourceBuild {
        kind,
        fs_hz: FS,
        block: N,
        lo_line: 2,
        hi_line: 20,
        levels,
        burst_pct: 50.0,
        delay_frames: 0,
        seed,
    })
}

#[test]
fn modal_test_no_source_at_an_accepted_level_trips_or_passes_the_ceiling_in_f32() {
    let ceiling = 0.1;
    let blocks = 64;
    for kind in KINDS {
        let lv = levels(kind, ceiling);
        // The setup accepts these levels.
        let mut s = ModalSetup::new(Mode::Shaker, 2048.0, 2048);
        s.shaker.source = kind;
        s.shaker.ceiling_v = ceiling;
        s.shaker.drives = lv
            .iter()
            .enumerate()
            .map(|(k, l)| DriveSet {
                row: 10 + k as u32,
                level_vrms: *l,
            })
            .collect();
        assert!(s.refusals().is_empty(), "{kind:?}: {:?}", s.refusals());
        for seed in 0..200u64 {
            // The source alone, at the old edge (level times crest at the ceiling): no
            // f32 it hands over passes level times crest.
            let at = ceiling / kind.crest();
            let mut src = source(kind, vec![at], seed);
            for _ in 0..blocks {
                for v in &src.next_frame()[0] {
                    let w = f64::from(*v as f32).abs();
                    assert!(w <= at * kind.crest(), "{kind:?} seed {seed}: {w}");
                }
            }
            // Through the step at the accepted levels, ramp and all.
            let d = lv.len();
            let (tx, rx) = channel(blocks * N + 1, d);
            let (cap, _cap_rx) = channel(16, d);
            let mut src = source(kind, lv.clone(), seed);
            for _ in 0..blocks {
                let f = src.next_frame();
                for i in 0..N {
                    let row: Vec<f32> = f.iter().map(|x| x[i] as f32).collect();
                    assert!(tx.push(&row));
                }
            }
            let b = DriveBuild {
                outputs: d,
                slots: (0..d).collect(),
                block: N,
                ramp_frames: N as u64,
                ceiling_v: ceiling,
                stop_ramp_frames: stop_ramp_frames(FS),
            };
            let (mut step, shared) =
                ModalStep::new(Some((b, rx, Arc::new(FeedShared::default()))), cap, 0);
            shared.start.store(true, Ordering::Release);
            let n = blocks * N;
            let mut out = Vec::new();
            step.process(&vec![0f32; n], n, &mut out);
            assert!(
                !shared.ceiling_trip.load(Ordering::Acquire),
                "{kind:?} seed {seed}: tripped at {}",
                f64::from_bits(shared.trip_value.load(Ordering::Acquire))
            );
            assert!(!shared.underrun.load(Ordering::Acquire));
            let peak = out.iter().fold(0f64, |m, v| m.max(f64::from(v.abs())));
            assert!(peak <= ceiling, "{kind:?} seed {seed}: {peak}");
            assert!(
                peak > 0.5 * lv[0],
                "{kind:?} seed {seed}: it drove ({peak})"
            );
        }
    }
}
