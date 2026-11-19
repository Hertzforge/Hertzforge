//! The laws' per-line work spread over threads gives exactly the results of the
//! single-threaded path: every drive, every diagnostic, bit for bit, for each
//! built-in law on plants with more controls than drives, as many, and fewer,
//! with the FRF held and with it changing every loop.

mod common;

use common::*;
use hertzforge_mimo_laws::{with_threads, BuiltinMimo};
use hertzforge_sdm::law::MimoLoopOutputs;

fn run(law: BuiltinMimo, s: &Synthetic, threads: usize, loops: usize) -> Vec<MimoLoopOutputs> {
    with_threads(threads, || {
        let params = law.spec().resolve(&[]).expect("defaults");
        let mut l = law.build(&s.setup, &params).expect("built");
        closed_loop(l.as_mut(), s, loops).outputs
    })
}

fn same(a: &[MimoLoopOutputs], b: &[MimoLoopOutputs], what: &str) {
    assert_eq!(a.len(), b.len());
    for (l, (x, y)) in a.iter().zip(b).enumerate() {
        let (dx, dy) = (x.drive.data(), y.drive.data());
        assert!(
            dx.len() == dy.len()
                && dx
                    .iter()
                    .zip(dy)
                    .all(|(p, q)| p.re.to_bits() == q.re.to_bits()
                        && p.im.to_bits() == q.im.to_bits()),
            "{what}: loop {l}'s drive differs between one thread and several"
        );
        assert_eq!(x.diagnostics.len(), y.diagnostics.len(), "{what}");
        for (p, q) in x.diagnostics.iter().zip(&y.diagnostics) {
            assert_eq!(p.key, q.key);
            assert_eq!(
                format!("{:?}", p.data),
                format!("{:?}", q.data),
                "{what}: {}",
                p.key
            );
        }
    }
}

#[test]
fn every_law_gives_the_same_bits_on_one_thread_and_on_several() {
    // 256 lines, 192 in band: well over the threshold at which lines are split.
    for (nc, nd) in [(12, 4), (6, 6), (4, 8)] {
        let s = Synthetic::random(0x7E57 + nc as u64, nc, nd, 256);
        for law in BuiltinMimo::ALL {
            let one = run(law, &s, 1, 4);
            let many = run(law, &s, 8, 4);
            same(&one, &many, &format!("{} at {nc} x {nd}", law.id()));
        }
    }
}

#[test]
fn an_frf_changing_every_loop_gives_the_same_bits_too() {
    // A changed plant estimate re-forms each line's coordinates (the threaded
    // refresh), carrying the drive in force into them.
    let mut s = Synthetic::random(0xF00D, 12, 6, 256);
    s.frf = perturbed(&s.plant, 9, 0.05);
    let law = BuiltinMimo::PseudoInverse;
    let mut runs = Vec::new();
    for threads in [1, 8] {
        runs.push(with_threads(threads, || {
            let params = law.spec().resolve(&[]).expect("defaults");
            let mut l = law.build(&s.setup, &params).expect("built");
            let mut out = Vec::new();
            let mut s2 = Synthetic::random(0xF00D, 12, 6, 256);
            s2.frf = s.frf.clone();
            for step in 0..3u64 {
                // A new estimate each pass: the law's coordinates are formed again.
                s2.frf = perturbed(&s.plant, 20 + step, 0.05);
                out.extend(closed_loop(l.as_mut(), &s2, 1).outputs);
            }
            out
        }));
    }
    same(&runs[0], &runs[1], "an FRF changing every pass");
}
