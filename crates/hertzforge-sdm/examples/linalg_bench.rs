//! Times the per-line linear algebra over a 1,600-line stack at the MIMO design's
//! largest shapes (72 controls, 24 drives). Run it optimized:
//!
//! ```text
//! cargo run -p hertzforge-sdm --release --example linalg_bench
//! ```
//!
//! Each row is milliseconds for the whole stack on one thread, the minimum and the
//! median of several runs.

use hertzforge_sdm::linalg::{
    cholesky_factor, congruence, eigh, h1_frf, multiple_coherence, pinv, projection_achievable,
    psd_repair, response_from_drive, sqrt_factor, Regularization, PSD_CLAMP_REL, PSD_REJECT_REL,
};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::hint::black_box;
use std::time::Instant;

const LINES: usize = 1600;
const RUNS: usize = 5;

/// SplitMix64, so the data are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }

    fn cplx(&mut self) -> C64 {
        C64::new(self.uniform(), self.uniform())
    }
}

fn random_stack(rng: &mut Rng, rows: usize, cols: usize) -> MatrixStack {
    let data = (0..LINES * rows * cols).map(|_| rng.cplx()).collect();
    MatrixStack::from_vec(LINES, rows, cols, data).unwrap()
}

/// `A A^H` per line with `A` random `n x n`: positive definite.
fn random_psd(rng: &mut Rng, n: usize) -> HermitianStack {
    let a = random_stack(rng, n, n);
    let mut eye = HermitianStack::zeros(LINES, n);
    for line in 0..LINES {
        for i in 0..n {
            eye.set(line, i, i, C64::new(1.0, 0.0));
        }
    }
    congruence(&a, &eye).unwrap()
}

fn time<T>(name: &str, mut f: impl FnMut() -> T) -> f64 {
    let mut ms: Vec<f64> = (0..RUNS)
        .map(|_| {
            let t = Instant::now();
            black_box(f());
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    println!("{name:<58} {:>9.1} {:>9.1}", ms[0], ms[RUNS / 2]);
    ms[RUNS / 2]
}

fn main() {
    let mut rng = Rng(2026);
    let g72 = random_psd(&mut rng, 72);
    let g24 = random_psd(&mut rng, 24);
    let h = random_stack(&mut rng, 72, 24);
    let t6 = random_stack(&mut rng, 6, 72);
    let gyx = random_stack(&mut rng, 72, 24);
    let (hp, _) = pinv(&h, Regularization::None).unwrap();
    let mut g24_repair = g24.clone();

    println!("{LINES} lines, {RUNS} runs each, one thread; milliseconds per full stack");
    println!("{:<58} {:>9} {:>9}", "operation", "min", "median");
    println!("-- required");
    time("eigh, 72 x 72", || eigh(&g72).unwrap());
    time("pinv, 72 x 24, no regularization", || {
        pinv(&h, Regularization::None).unwrap()
    });
    time("sqrt_factor, 24 x 24", || sqrt_factor(&g24).unwrap());
    time("congruence, 6 x 72 transformation of 72 x 72", || {
        congruence(&t6, &g72).unwrap()
    });
    time("multiple_coherence, 72 controls x 24 drives", || {
        multiple_coherence(&g72, &gyx, &g24, Regularization::None).unwrap()
    });
    println!("-- the other pieces of one control loop");
    time("pinv, 72 x 24, truncated rcond 1e-3", || {
        pinv(&h, Regularization::Truncated { rcond: 1e-3 }).unwrap()
    });
    time(
        "congruence, 24 x 72 H+ of 72 x 72 reference (drive)",
        || congruence(&hp, &g72).unwrap(),
    );
    time("psd_repair, 24 x 24", || {
        psd_repair(&mut g24_repair, PSD_CLAMP_REL, PSD_REJECT_REL).unwrap()
    });
    time("cholesky_factor, 24 x 24", || {
        cholesky_factor(&g24).unwrap()
    });
    time("response_from_drive, 72 x 24 with 24 x 24", || {
        response_from_drive(&h, &g24).unwrap()
    });
    time("h1_frf, 72 x 24", || {
        h1_frf(&gyx, &g24, Regularization::None).unwrap()
    });
    time("projection_achievable, 72 x 24 with 72 x 72", || {
        projection_achievable(&h, &g72, Regularization::None).unwrap()
    });

    // Lines are independent, so a native caller can split a stack across threads.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let h_parts = split_matrix(&h, threads);
    let hp_parts = split_matrix(&hp, threads);
    let g72_parts = split_hermitian(&g72, threads);
    println!("-- the stack split across {threads} threads");
    time("pinv, 72 x 24, no regularization", || {
        on_threads(&h_parts, |p| pinv(p, Regularization::None).unwrap())
    });
    time("eigh, 72 x 72", || {
        on_threads(&g72_parts, |g| eigh(g).unwrap())
    });
    let pairs: Vec<(&MatrixStack, &HermitianStack)> = hp_parts.iter().zip(&g72_parts).collect();
    time(
        "drive: congruence 24 x 72 of 72 x 72, repair, sqrt_factor",
        || {
            on_threads(&pairs, |(t, g)| {
                let mut d = congruence(t, g).unwrap();
                psd_repair(&mut d, PSD_CLAMP_REL, PSD_REJECT_REL).unwrap();
                sqrt_factor(&d).unwrap()
            })
        },
    );
}

fn chunks(lines: usize, parts: usize) -> Vec<(usize, usize)> {
    let per = lines.div_ceil(parts);
    (0..parts)
        .map(|p| (p * per, ((p + 1) * per).min(lines)))
        .filter(|(a, b)| a < b)
        .collect()
}

fn split_matrix(m: &MatrixStack, parts: usize) -> Vec<MatrixStack> {
    let (lines, r, c) = m.dims();
    chunks(lines, parts)
        .into_iter()
        .map(|(a, b)| {
            let data = m.data()[a * r * c..b * r * c].to_vec();
            MatrixStack::from_vec(b - a, r, c, data).unwrap()
        })
        .collect()
}

fn split_hermitian(g: &HermitianStack, parts: usize) -> Vec<HermitianStack> {
    let per = HermitianStack::packed_len(g.n());
    chunks(g.lines(), parts)
        .into_iter()
        .map(|(a, b)| {
            let data = g.data()[a * per..b * per].to_vec();
            HermitianStack::from_packed(b - a, g.n(), data).unwrap()
        })
        .collect()
}

fn on_threads<P: Sync, T: Send>(parts: &[P], f: impl Fn(&P) -> T + Sync) -> Vec<T> {
    std::thread::scope(|s| {
        let f = &f;
        let jobs: Vec<_> = parts.iter().map(|p| s.spawn(move || f(p))).collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    })
}
