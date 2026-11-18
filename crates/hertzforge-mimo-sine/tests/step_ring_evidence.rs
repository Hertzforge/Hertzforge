//! Evidence, not a requirement: the ring of the last tone on a resonance at a
//! stepped sine's step, on `sine-base-article-6x4` (the article's 230 Hz mode,
//! damping 0.02) with its two article responses limited at 2 g (abort band
//! widened so the run is not stopped), 150 to 350 Hz, six steps an octave,
//! 2 s a step. For each step: the largest limit reading in the first 0.6 s
//! after the step against the reading the step settles to (its last 0.5 s).
//! `cargo test -p hertzforge-mimo-sine --test step_ring_evidence -- --ignored --nocapture`.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::records::EventKind;
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{DwellFor, StepSineConfig, StepSpacing};

#[test]
#[ignore]
fn mimo_sine_step_ring_evidence() {
    let s = SineStructure::BaseArticle6x4;
    let plan = TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
        band: (150.0, 350.0),
        steps_per_octave: 6.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(2.0)),
    }));
    let mut st = setup(s, plan, "matrix");
    st.limits = (0..2)
        .map(|j| LimitChannel {
            channel: 5 + j,
            profile: flat(2.0, 1.5, 20.0),
        })
        .collect();
    let r = run(engaged(s, st, law("matrix"), NOISE), 40.0);
    println!("outcome {:?}", r.outcome);
    // Steps: where the frequency changes between records.
    let mut starts = vec![0usize];
    for k in 1..r.ticks.len() {
        if (r.ticks[k].frequency_hz - r.ticks[k - 1].frequency_hz).abs() > 1.0 {
            starts.push(k);
        }
    }
    starts.push(r.ticks.len());
    println!("step Hz | from Hz | peak limit g (first 0.6 s) | at s after | settled g | peak over settled dB | over 2 g bound dB");
    for w in starts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == 0 {
            continue;
        }
        let t0 = r.ticks[a].time_s;
        let f = r.ticks[b - 1].frequency_hz;
        let from = r.ticks[a - 1].frequency_hz;
        let lim = |t: &hertzforge_mimo_sine::records::TickRecord| {
            t.limit[..2].iter().map(|l| l.norm()).fold(0.0, f64::max)
        };
        let (mut peak, mut at) = (0.0f64, 0.0);
        for t in &r.ticks[a..b] {
            if t.time_s - t0 <= 0.6 && lim(t) > peak {
                peak = lim(t);
                at = t.time_s - t0;
            }
        }
        let t_end = r.ticks[b - 1].time_s;
        let tail: Vec<f64> = r.ticks[a..b]
            .iter()
            .filter(|t| t.time_s > t_end - 0.5)
            .map(lim)
            .collect();
        let settled = tail.iter().sum::<f64>() / tail.len().max(1) as f64;
        println!(
            "{f:.1} | {from:.1} | {peak:.2} | {at:.2} | {settled:.2} | {:+.2} | {:+.2}",
            20.0 * (peak / settled).log10(),
            20.0 * (peak / 2.0).log10()
        );
    }
    let notches = r
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NotchIn { .. }))
        .count();
    println!(
        "run length {:.2} s; notch entries {notches}",
        r.ticks.last().map(|t| t.time_s).unwrap_or(0.0)
    );
}
