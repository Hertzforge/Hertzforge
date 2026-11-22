//! Closed-loop evidence for the structure delay the loop-speed rule reads,
//! ignored by default (minutes in an optimized build): `matrix` with
//! `largest` and with `principal`, same seeds, linear 4 by 4 with 2 percent
//! seeded scatter, envelope dynamics. Dwells away from and at a resonance
//! started 6 dB low, a stepped sine, sweeps at one and four octaves a minute.
//! The reference structure is linear, so there is no softening case here.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::law::{worst_error_db, Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::modal::{Model, ModelChoice};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, ModalStructure, Played, Row, Scatter, Schedule, FS_HZ, TICK_SAMPLES,
};
use hertzforge_mimo_sine_law_math::setup::DelayEstimate;
use hertzforge_mimo_sine_law_math::speed::GainRule;
use hertzforge_mimo_sine_law_math::{Setup, C64};

const BAND: (f64, f64) = (40.0, 400.0);

/// A law whose first drive is half of what it asks: 6 dB low.
struct Low(MatrixLaw);

impl SineLaw for Low {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.0.engage(x, out)?;
        out.drive.iter_mut().for_each(|v| *v *= 0.5);
        Ok(())
    }
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.0.tick(x, out)
    }
    fn value_names(&self) -> &'static [&'static str] {
        self.0.value_names()
    }
}

fn params(how: DelayEstimate) -> MatrixParams {
    MatrixParams {
        gain_rule: GainRule::LoopSpeed {
            g_max: 0.5,
            beta: 0.5,
            delay: how,
        },
        ..MatrixParams::default()
    }
}

fn run(
    st: &ModalStructure,
    setup: &Setup,
    model: &Model,
    how: DelayEstimate,
    s: Schedule,
    low: bool,
) -> Option<Vec<Row>> {
    let law = MatrixLaw::with_model(setup, params(how), model.clone()).unwrap();
    let mut law: Box<dyn SineLaw> = if low {
        Box::new(Low(law))
    } else {
        Box::new(law)
    };
    run_closed_loop(
        law.as_mut(),
        st,
        setup,
        s,
        Played::AsAsked,
        Dynamics::Envelope,
        MATRIX_VALUES.len(),
        1_000_000,
    )
    .ok()
}

fn err(r: &Row) -> f64 {
    let ones = vec![C64::new(1.0, 0.0); r.controls.len()];
    worst_error_db(&r.controls, &ones)
}

/// The largest any control is over its reference, dB (0 when none is).
fn over(r: &Row) -> f64 {
    r.controls
        .iter()
        .map(|c| 20.0 * c.norm().max(1e-300).log10())
        .fold(0.0, f64::max)
}

/// Time to within 1 dB (s, None if never), the worst error over the last
/// second, and the largest overshoot after first coming within 1 dB.
fn dwell_metrics(rows: &[Row]) -> (Option<f64>, f64, f64) {
    let dt = TICK_SAMPLES as f64 / FS_HZ;
    let first = rows
        .iter()
        .skip(1)
        .position(|r| err(r) <= 1.0)
        .map(|k| k + 1);
    let last_s = (1.0 / dt) as usize;
    let tail = rows[rows.len().saturating_sub(last_s)..]
        .iter()
        .map(err)
        .fold(0.0, f64::max);
    let ov = first.map_or(0.0, |k| rows[k..].iter().map(over).fold(0.0, f64::max));
    (first.map(|k| k as f64 * dt), tail, ov)
}

/// (worst, median) over the resonance windows of the elastic modes.
fn windows(st: &ModalStructure, skip: usize, rows: &[Row]) -> (f64, f64) {
    let mut per = Vec::new();
    for r in skip..st.f_hz.len() {
        let (f, z) = (st.f_hz[r], st.zeta[r]);
        if !(BAND.0..=BAND.1).contains(&f) {
            continue;
        }
        let (lo, hi) = (f * (1.0 - 2.0 * z), f * (1.0 + 2.0 * z));
        let w = rows
            .iter()
            .skip(30)
            .filter(|x| x.f >= lo && x.f <= hi)
            .map(err)
            .fold(0.0, f64::max);
        per.push(w);
    }
    per.sort_by(f64::total_cmp);
    (per[per.len() - 1], per[per.len() / 2])
}

/// Worst over every step of the error in the second half of the step, and
/// the median of the per-step values.
fn stepped_metrics(rows: &[Row]) -> (f64, f64) {
    let mut per = Vec::new();
    let mut a = 0;
    while a < rows.len() {
        let f = rows[a].f;
        let mut b = a;
        while b < rows.len() && rows[b].f == f {
            b += 1;
        }
        let half = a + (b - a) / 2;
        per.push(rows[half..b].iter().map(err).fold(0.0, f64::max));
        a = b;
    }
    per.sort_by(f64::total_cmp);
    (per[per.len() - 1], per[per.len() / 2])
}

fn show(o: Option<f64>) -> String {
    o.map_or("never".into(), |t| format!("{t:.2}"))
}

#[test]
#[ignore = "evidence: minutes in an optimized build"]
fn mimo_sine_evidence_delay_estimate_closed_loop() {
    let seeds = load("lin4x4_seeds.json");
    let forms = [DelayEstimate::Largest, DelayEstimate::Principal];
    for (kind, s) in (0..2).flat_map(|k| (0..5u64).map(move |s| (k, s))) {
        let c = &seeds[s.to_string()];
        let st = structure(c);
        let skip = c["n_suspension"].as_u64().unwrap() as usize;
        // Kind 0: the fixed 2 percent scatter the earlier evidence used
        // (relative only); kind 1: seeded 2 percent, relative and floor.
        let setup = if kind == 0 {
            st.setup(BAND, true)
        } else {
            st.setup_scattered(BAND, Scatter::of(0.02, 100 + s))
        };
        let model = Model::build(&setup.identification, BAND, ModelChoice::default()).unwrap();
        // Away from resonance: the middle of the widest gap between elastic
        // modes in band. At a resonance: the lightest elastic mode in band.
        let el: Vec<usize> = (skip..st.f_hz.len())
            .filter(|&r| (BAND.0 + 5.0..=BAND.1 - 5.0).contains(&st.f_hz[r]))
            .collect();
        let (mut gap, mut away) = (0.0, 0.0);
        for w in el.windows(2) {
            let (a, b) = (st.f_hz[w[0]], st.f_hz[w[1]]);
            if (b / a).ln() > gap {
                gap = (b / a).ln();
                away = (a * b).sqrt();
            }
        }
        let light = *el
            .iter()
            .min_by(|a, b| st.zeta[**a].total_cmp(&st.zeta[**b]))
            .unwrap();
        let at = st.f_hz[light];
        let kind_name = if kind == 0 { "fixed" } else { "seeded" };
        eprintln!(
            "{kind_name} scatter, seed {s}: away {away:.1} Hz, resonance {at:.1} Hz (zeta {:.4})",
            st.zeta[light]
        );
        for how in forms {
            let name = how.name();
            let mut line = format!("  {name:9}");
            for (f, label) in [(away, "away"), (at, "res")] {
                match run(
                    &st,
                    &setup,
                    &model,
                    how,
                    Schedule::Dwell { f, secs: 6.0 },
                    true,
                ) {
                    Some(rows) => {
                        let (t, tail, ov) = dwell_metrics(&rows);
                        let g = rows.last().unwrap().values[0];
                        line += &format!(
                            " | {label}: to 1 dB {} s, last 1 s {tail:.2} dB, over {ov:.2} dB, gain {g:.4}",
                            show(t)
                        );
                    }
                    None => line += &format!(" | {label}: lost"),
                }
            }
            let stepped = Schedule::Steps {
                f0: BAND.0,
                f1: BAND.1,
                per_octave: 6.0,
                secs: 2.0,
            };
            match run(&st, &setup, &model, how, stepped, false) {
                Some(rows) => {
                    let (w, m) = stepped_metrics(&rows);
                    line += &format!(" | stepped {w:.2} / {m:.2}");
                }
                None => line += " | stepped lost",
            }
            for rate in [1.0, 4.0] {
                let sw = Schedule::Sweep {
                    f0: BAND.0,
                    f1: BAND.1,
                    oct_min: rate,
                };
                match run(&st, &setup, &model, how, sw, false) {
                    Some(rows) => {
                        let (w, m) = windows(&st, skip, &rows);
                        let whole = rows.iter().skip(30).map(err).fold(0.0, f64::max);
                        line += &format!(" | sweep {rate}: {w:.2} / {m:.2} (all {whole:.2})");
                    }
                    None => line += &format!(" | sweep {rate}: lost"),
                }
            }
            eprintln!("{line}");
        }
    }
}
