//! Evidence for how the identification's group delay is read, ignored by
//! default: linear 4 by 4, seeds 0 to 4, identifications with 0.5, 2 and 5
//! percent seeded scatter (relative and floor, see `Scatter`), every form
//! against the structure's own delay read from the exact response.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::reference_structure::{ModalStructure, Scatter};
use hertzforge_mimo_sine_law_math::setup::{GroupDelay, TrackingRule};

const BAND: (f64, f64) = (40.0, 400.0);

fn tracking() -> TrackingRule {
    TrackingRule::Proportional {
        pct: 10.0,
        max_hz: 20.0,
    }
}

/// The median of the smoothed delay within `f +- span`.
fn median_at(gd: &GroupDelay, f: f64, span: f64) -> f64 {
    let (g, y) = gd.lines();
    let mut v: Vec<f64> = g
        .iter()
        .zip(y)
        .filter(|(x, _)| (**x - f).abs() <= span)
        .map(|(_, y)| *y)
        .collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

const FORMS: [&str; 6] = [
    "largest",
    "weighted",
    "principal",
    "half",
    "weighted4",
    "directions",
];

/// The present form on the exact response, then each form on the scattered
/// identification.
fn forms(st: &ModalStructure, sc: Scatter) -> (GroupDelay, Vec<GroupDelay>) {
    let ex = st.identification(BAND, 0.25, false);
    let id = st.identification_scattered(BAND, 0.25, sc);
    (
        GroupDelay::of(&ex),
        FORMS
            .iter()
            .map(|h| GroupDelay::of_candidate(&id, h).unwrap())
            .collect(),
    )
}

fn span(f: f64) -> f64 {
    2.0 * tracking().bandwidth_hz(f)
}

#[test]
#[ignore = "evidence: prints the item 1 and 2 tables"]
fn mimo_sine_evidence_delay_estimate_against_truth() {
    let seeds = load("lin4x4_seeds.json");
    let nf = FORMS.len() + 1; // + median window on the present form
    for pct in [0.5, 2.0, 5.0] {
        eprintln!("== scatter {pct} percent; ms; true is the present form on the exact response");
        let mut ratios = vec![Vec::new(); nf];
        let mut res_true = vec![f64::INFINITY; nf];
        let mut res_q = vec![f64::INFINITY; nf];
        let mut res_q_hi = vec![0.0f64; nf];
        let mut res_q_where = vec![(0u64, 0.0f64); nf];
        for s in 0..5u64 {
            let c = &seeds[s.to_string()];
            let st = structure(c);
            let skip = c["n_suspension"].as_u64().unwrap() as usize;
            for rep in 0..3u64 {
                let (ex, fm) = forms(&st, Scatter::of(pct / 100.0, 1000 * s + rep));
                let read = |f: f64| -> Vec<f64> {
                    let mut v: Vec<f64> = fm.iter().map(|g| g.at(f, span(f))).collect();
                    v.push(median_at(&fm[0], f, span(f)));
                    v
                };
                let mut f = 45.0;
                while f <= 395.0 {
                    let t = ex.at(f, span(f));
                    for (i, e) in read(f).iter().enumerate() {
                        ratios[i].push(e / t);
                    }
                    f += 1.0;
                }
                for r in skip..st.f_hz.len() {
                    let fr = st.f_hz[r];
                    if !(45.0..=395.0).contains(&fr) {
                        continue;
                    }
                    let t = ex.at(fr, span(fr));
                    let q = 1.0 / (st.zeta[r] * 2.0 * std::f64::consts::PI * fr);
                    for (i, e) in read(fr).iter().enumerate() {
                        res_true[i] = res_true[i].min(e / t);
                        if e / q < res_q[i] {
                            res_q[i] = e / q;
                            res_q_where[i] = (s, fr);
                        }
                        res_q_hi[i] = res_q_hi[i].max(e / q);
                    }
                }
                if rep == 0 && (s == 0 || pct == 2.0) {
                    eprintln!("seed {s}: f Hz | true | largest weighted principal half weighted4 directions median");
                    let mut pts: Vec<(f64, &str)> = Vec::new();
                    let el: Vec<f64> = st.f_hz[skip..]
                        .iter()
                        .copied()
                        .filter(|f| (45.0..=395.0).contains(f))
                        .collect();
                    for (i, f) in el.iter().enumerate() {
                        pts.push((*f, "res"));
                        if i + 1 < el.len() {
                            pts.push(((f * el[i + 1]).sqrt(), "between"));
                        }
                    }
                    pts.push((100.0, "fixed"));
                    pts.push((350.0, "fixed"));
                    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                    for (f, kind) in pts {
                        let v: Vec<String> =
                            read(f).iter().map(|x| format!("{:6.1}", x * 1e3)).collect();
                        eprintln!(
                            "  {f:6.1} {kind:7} | {:6.1} | {}",
                            ex.at(f, span(f)) * 1e3,
                            v.join(" ")
                        );
                    }
                }
            }
        }
        let names = [
            "largest",
            "weighted",
            "principal",
            "half",
            "weighted4",
            "directions",
            "median",
        ];
        for i in 0..nf {
            let r = &mut ratios[i];
            r.sort_by(f64::total_cmp);
            let pick = |q: f64| r[((r.len() - 1) as f64 * q) as usize];
            eprintln!(
                "  {:10}: est/true over band: least {:.2}, p10 {:.2}, median {:.2}, p90 {:.2}, most {:.1}; at resonances least est/true {:.2}; est/(1/zeta w) least {:.2} (seed {} {:.1} Hz), most {:.2}",
                names[i],
                pick(0.0),
                pick(0.1),
                pick(0.5),
                pick(0.9),
                pick(1.0),
                res_true[i],
                res_q[i],
                res_q_where[i].0,
                res_q_where[i].1,
                res_q_hi[i]
            );
        }
    }
}
