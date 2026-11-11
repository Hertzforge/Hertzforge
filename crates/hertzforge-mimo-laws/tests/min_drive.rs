//! The minimum-drive law on synthetic plants with the closed loop simulated
//! here (see `laws.rs` for the harness).
//!
//! - At the exact-control end of the trade-off, on a square plant, the law's
//!   seed drive reproduces the specified autospectra and needs no more power
//!   than the default law's seed drive, which reproduces the whole reference.
//! - Along the trade-off the drive power falls.
//! - With the automatic trade-off the loop converges on the law's own target
//!   on a square plant; with more controls than drives the target is reachable
//!   on the estimate the law was handed, not on the true plant, so the loop's
//!   least-squares fit keeps the plant error.

mod common;

use common::*;
use hertzforge_mimo_laws::{BuiltinMimoLaw, MinimumDriveLaw, ParamVal, PseudoInverseLaw};
use hertzforge_sdm::law::DiagnosticData;

const LINES: usize = 64;
const BAND: (usize, usize) = (8, 56);

fn fixed(v: f64) -> Vec<(&'static str, ParamVal)> {
    vec![
        ("tradeoff", ParamVal::Choice("fixed".into())),
        ("tradeoff_value", ParamVal::Number(v)),
    ]
}

/// A plant with a reachable reference and a 5 percent error in the estimate
/// the law is handed.
fn synthetic(seed: u64, nc: usize, nd: usize) -> Synthetic {
    let plant = well_conditioned(seed, nc, nd, LINES);
    let reference = reachable_reference(&plant, seed, BAND);
    let frf = perturbed(&plant, seed, 0.05);
    run_on(seed, plant, frf, reference, BAND, 0.01)
}

fn per_control_line(run: &Run, l: usize, key: &str) -> Vec<f64> {
    match run.outputs[l]
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(DiagnosticData::PerControlLine(v)) => v.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

#[test]
fn at_the_exact_control_end_the_seed_needs_no_more_than_the_default_laws() {
    let s = synthetic(31, 4, 4);
    let mut law = MinimumDriveLaw::with_params(&s.setup, &fixed(1e-6)).expect("law");
    let mut pinv = PseudoInverseLaw::with_params(&s.setup, &[]).expect("default law");
    let a = closed_loop(&mut law, &s, 0);
    let b = closed_loop(&mut pinv, &s, 0);
    let ratio = per_control_line(&a, 0, "target_ratio");
    for line in BAND.0..BAND.1 {
        let (ta, tb) = (a.last().trace(line), b.last().trace(line));
        assert!(ta <= tb * (1.0 + 1e-6), "line {line}: {ta} against {tb}");
        for c in 0..4 {
            let r = ratio[line * 4 + c];
            assert!(
                (r - 1.0).abs() < 2.5e-3,
                "line {line} control {c}: target ratio {r}"
            );
        }
    }
}

#[test]
fn a_larger_trade_off_lowers_the_drive() {
    let s = synthetic(32, 6, 4);
    let mut last: Option<f64> = None;
    for v in [1e-6, 1e-2, 1.0, 1e2] {
        let mut law = MinimumDriveLaw::with_params(&s.setup, &fixed(v)).expect("law");
        let run = closed_loop(&mut law, &s, 0);
        let power: f64 = run.per_line(0, "drive_power").iter().sum();
        if let Some(p) = last {
            // Each solve is to a relative gap of 1e-7 of its own objective.
            assert!(
                power <= p * (1.0 + 1e-6),
                "trade-off {v}: {power} above {p}"
            );
        }
        last = Some(power);
    }
}

/// The worst `|10 log10(response / target)|` over the controls and in-band
/// lines of a run, with the target's autospectra from the specification and
/// the ratio the law reports.
fn worst_db_from_target(s: &Synthetic, run: &Run, nc: usize) -> f64 {
    let ratio = per_control_line(run, 0, "target_ratio");
    let y = congruence(&s.plant, run.last());
    let mut worst: f64 = 0.0;
    for line in BAND.0..BAND.1 {
        for c in 0..nc {
            let target = s.setup.reference.get(line, c, c).re * ratio[line * nc + c];
            let got = y.get(line, c, c).re;
            worst = worst.max((10.0 * (got / target).log10()).abs());
        }
    }
    worst
}

#[test]
fn with_the_automatic_trade_off_the_loop_converges_on_its_own_target() {
    let s = synthetic(33, 4, 4);
    let mut law = MinimumDriveLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut law, &s, 60);
    let rel = run.scalar(0, "tradeoff");
    assert!((1e-6..=1e6).contains(&rel), "trade-off {rel}");
    // A nearly coherent target starts the loop's weak directions at the floor,
    // so it takes longer than a random reference: the worst error at loops 30
    // and 60.
    let at30 = {
        let mut law = MinimumDriveLaw::with_params(&s.setup, &[]).expect("law");
        worst_db_from_target(&s, &closed_loop(&mut law, &s, 30), 4)
    };
    let worst = worst_db_from_target(&s, &run, 4);
    assert!(
        worst < 0.5,
        "worst {worst} dB from the target at loop 60 ({at30} at 30)"
    );
    for (l, d) in run.drives().enumerate() {
        let w = worst_eigen_ratio(d);
        assert!(w >= -1e-9, "loop {l}: eigenvalue ratio {w}");
    }
}

#[test]
fn with_more_controls_than_drives_the_loop_keeps_only_the_plants_error() {
    // The target is the estimate's response to the optimal drive; the true
    // plant is 5 percent off it, and with six controls on four drives the
    // loop's least-squares fit cannot remove that. Measured: 0.62 dB.
    let s = synthetic(33, 6, 4);
    let mut law = MinimumDriveLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut law, &s, 30);
    let worst = worst_db_from_target(&s, &run, 6);
    assert!(worst < 1.0, "worst {worst} dB from the target");
}

#[test]
#[ignore = "diagnostic print"]
fn print_convergence_trajectory() {
    for &(nc, nd) in &[(4usize, 4usize), (6, 4)] {
        let s = synthetic(33, nc, nd);
        for params in [vec![], fixed(1e-6), fixed(1.0), fixed(1e2)] {
            let mut law = MinimumDriveLaw::with_params(&s.setup, &params).expect("law");
            let run = closed_loop(&mut law, &s, 60);
            let rel = run.scalar(0, "tradeoff");
            let ratio = per_control_line(&run, 0, "target_ratio");
            let (rmin, rmax) = ratio[BAND.0 * nc..BAND.1 * nc]
                .iter()
                .fold((f64::MAX, 0.0f64), |(a, b), &v| (a.min(v), b.max(v)));
            let mut traj = Vec::new();
            for l in [0usize, 2, 5, 10, 20, 30, 45, 60] {
                let y = congruence(&s.plant, &run.outputs[l].drive);
                let mut worst: f64 = 0.0;
                for line in BAND.0..BAND.1 {
                    for c in 0..nc {
                        let target = s.setup.reference.get(line, c, c).re * ratio[line * nc + c];
                        worst = worst.max((10.0 * (y.get(line, c, c).re / target).log10()).abs());
                    }
                }
                traj.push(format!("{l}:{worst:.2}"));
            }
            // The seed drive's eigenvalues on one in-band line, and the loop's
            // clamping and correction size over the run.
            let line = 20;
            let one = {
                let d = &run.outputs[0].drive;
                let mut g = hertzforge_sdm::HermitianStack::zeros(1, nd);
                for r in 0..nd {
                    for c in r..nd {
                        g.set(0, r, c, d.get(line, r, c));
                    }
                }
                hertzforge_sdm::linalg::eigh(&g).expect("eigh").values
            };
            let emax = one.iter().copied().fold(0.0f64, f64::max);
            let eigs: Vec<String> = one.iter().map(|v| format!("{:.1e}", v / emax)).collect();
            let clamped: Vec<String> = [1usize, 5, 10, 20, 30, 45, 60]
                .iter()
                .map(|&l| {
                    format!(
                        "{}/{:.1}",
                        run.scalar(l, "clamped"),
                        run.scalar(l, "correction_db")
                    )
                })
                .collect();
            println!(
                "{nc}x{nd} tradeoff {rel:.0e}: ratio {rmin:.3}..{rmax:.3}, seed drive eigs {} , worst dB by loop {} , clamped/correction {}",
                eigs.join(" "),
                traj.join(" "),
                clamped.join(" ")
            );
        }
    }
}

/// The mean, over the in-band lines and controls, of the target's
/// autospectrum over the specified one, each counted at most at 1.
fn mean_kept(run: &Run, nc: usize) -> f64 {
    let ratio = per_control_line(run, 0, "target_ratio");
    let (mut sum, mut n) = (0.0, 0.0);
    for line in BAND.0..BAND.1 {
        for c in 0..nc {
            sum += ratio[line * nc + c].min(1.0);
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn with_no_alarm_lines_the_automatic_trade_off_keeps_the_autospectra() {
    // 24 controls on 6 drives with singular values spread over 300 to 1, a
    // reference the plant cannot reach, and no alarm lines: the reduced form of
    // a 48 x 12 lightly damped plate, where the knee alone kept 15 % of the
    // specification. With the default largest shortfall of 3 dB the target
    // keeps at least half of each autospectrum on average.
    let (nc, nd) = (24, 6);
    let plant = plant_with_singular_values(41, nc, nd, LINES, &[1.0, 0.3, 0.1, 0.03, 0.01, 3e-3]);
    let mut rng = Rng::new(41);
    let mut reference = hertzforge_sdm::HermitianStack::zeros(LINES, nc);
    for line in BAND.0..BAND.1 {
        let g = random_psd(&mut rng, nc, 1e-2);
        for r in 0..nc {
            for c in r..nc {
                reference.set(line, r, c, g[r * nc + c]);
            }
        }
    }
    let s = run_on(41, plant.clone(), plant, reference, BAND, 0.01);
    let mut law = MinimumDriveLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut law, &s, 0);
    let kept = mean_kept(&run, nc);
    let tradeoff = run.scalar(0, "tradeoff");
    println!("automatic trade-off {tradeoff:e}: the target keeps {kept:.3} of the autospectra");
    assert!(kept >= 0.5, "trade-off {tradeoff:e} keeps {kept:.3}");
    // The bound is the parameter's: at 20 dB it lets the knee give up more.
    let mut loose =
        MinimumDriveLaw::with_params(&s.setup, &[("shortfall_db", ParamVal::Number(20.0))])
            .expect("law");
    let loose = closed_loop(&mut loose, &s, 0);
    println!(
        "at 20 dB: trade-off {:e}, keeps {:.3}",
        loose.scalar(0, "tradeoff"),
        mean_kept(&loose, nc)
    );
}
