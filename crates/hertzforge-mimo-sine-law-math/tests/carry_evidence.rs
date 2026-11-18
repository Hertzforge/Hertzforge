//! Evidence for where `matrix` should carry, ignored by default (minutes in an
//! optimized build): linear 4 by 4, seeds 0 to 4, identification with 2
//! percent line-to-line scatter, envelope dynamics, sweeps 40 to 400 Hz at one
//! and four octaves a minute. (a) the exact model; (b) the split-fit model,
//! carrying only where it is modal; (c) the fit before the split (long chains
//! left out) carried everywhere; (d) carry weight 0.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::law::worst_error_db;
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::modal::{FitLimits, Model, ModelChoice};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, ModalStructure, Played, Schedule,
};
use hertzforge_mimo_sine_law_math::C64;

/// (worst, median) over the resonance windows `f_r (1 +- 2 zeta_r)` of the
/// elastic modes in band, of the worst control error in each, dB.
fn windows(
    st: &ModalStructure,
    skip: usize,
    rows: &[hertzforge_mimo_sine_law_math::reference_structure::Row],
) -> (f64, f64) {
    let ones = vec![C64::new(1.0, 0.0); st.controls];
    let mut per = Vec::new();
    for r in skip..st.f_hz.len() {
        let (f, z) = (st.f_hz[r], st.zeta[r]);
        if !(40.0..=400.0).contains(&f) {
            continue;
        }
        let (lo, hi) = (f * (1.0 - 2.0 * z), f * (1.0 + 2.0 * z));
        let w = rows
            .iter()
            .skip(30)
            .filter(|x| x.f >= lo && x.f <= hi)
            .map(|x| worst_error_db(&x.controls, &ones))
            .fold(0.0, f64::max);
        per.push(w);
    }
    per.sort_by(f64::total_cmp);
    (per[per.len() - 1], per[per.len() / 2])
}

#[test]
#[ignore = "evidence: minutes in an optimized build"]
fn mimo_sine_evidence_where_to_carry() {
    let seeds = load("lin4x4_seeds.json");
    let band = (40.0, 400.0);
    for rate in [1.0, 4.0] {
        eprintln!("sweep 40-400 Hz at {rate} oct/min: worst / median window error, dB");
        let mut better = 0;
        for s in 0..5 {
            let c = &seeds[s.to_string()];
            let st = structure(c);
            let skip = c["n_suspension"].as_u64().unwrap() as usize;
            let exact = st.setup(band, false);
            let noisy = st.setup(band, true);
            let sweep = Schedule::Sweep {
                f0: 40.0,
                f1: 400.0,
                oct_min: rate,
            };
            let run =
                |setup: &hertzforge_mimo_sine_law_math::Setup, model: Model, p: MatrixParams| {
                    let mut law = MatrixLaw::with_model(setup, p, model).unwrap();
                    match run_closed_loop(
                        &mut law,
                        &st,
                        setup,
                        sweep,
                        Played::AsAsked,
                        Dynamics::Envelope,
                        MATRIX_VALUES.len(),
                        1_000_000,
                    ) {
                        Ok(rows) => windows(&st, skip, &rows),
                        Err(_) => (f64::INFINITY, f64::INFINITY),
                    }
                };
            let d = MatrixParams::default();
            let ma =
                Model::build(&exact.identification, band, ModelChoice::Identification).unwrap();
            let mb = Model::build(&noisy.identification, band, ModelChoice::default()).unwrap();
            let whole = FitLimits {
                split: false,
                ..FitLimits::default()
            };
            let mc = Model::build_with(&noisy.identification, band, ModelChoice::default(), whole)
                .unwrap();
            let left_b = mb.fit.candidates_left_out;
            let left_c = mc.fit.candidates_left_out;
            let a = run(&exact, ma, d);
            let b = run(&noisy, mb.clone(), d);
            let cc = run(
                &noisy,
                mc,
                MatrixParams {
                    carry_outside_model: true,
                    ..d
                },
            );
            let dd = run(&noisy, mb, MatrixParams { carry: 0.0, ..d });
            if b.0 < dd.0 {
                better += 1;
            }
            eprintln!(
                "seed {s}: (a) {:.2} / {:.2}  (b) {:.2} / {:.2} [left out {left_b}]  (c) {:.2} / {:.2} [left out {left_c}]  (d) {:.2} / {:.2}",
                a.0, a.1, b.0, b.1, cc.0, cc.1, dd.0, dd.1
            );
        }
        eprintln!("(b) lower worst window error than (d) on {better} of 5 seeds");
    }
}

#[test]
#[ignore = "diagnosis: the split fit's groups on seeds 0 to 4"]
fn mimo_sine_evidence_fit_groups() {
    let seeds = load("lin4x4_seeds.json");
    for s in 0..5 {
        let st = structure(&seeds[s.to_string()]);
        let setup = st.setup((40.0, 400.0), true);
        let t = std::time::Instant::now();
        let m = Model::build(&setup.identification, (40.0, 400.0), ModelChoice::default()).unwrap();
        eprintln!(
            "seed {s}: {:.2} s, kept {}, left out {}, total work {:.2e}, groups {:?}",
            t.elapsed().as_secs_f64(),
            m.modes.modes(),
            m.fit.candidates_left_out,
            m.fit.total_work as f64,
            m.fit
                .groups
                .iter()
                .map(|g| (g.candidates, g.lines, g.work as f64 / 1e9, g.outcome))
                .collect::<Vec<_>>()
        );
    }
}
