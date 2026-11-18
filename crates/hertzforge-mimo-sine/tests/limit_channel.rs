//! The limit channel on `sine-base-article-6x4`: the backstop notch holds the
//! article at its bound through its resonance and releases after it; and
//! `sine-near-singular-4x4` survives a sweep through its near-singular
//! frequency.

mod common;

use common::*;
use hertzforge_hal_sim::plants::{SineStructure, SINE_NEAR_SINGULAR_HZ};
use hertzforge_mimo_sine::records::{EventKind, Outcome};
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{BidirSweepConfig, SweepDirection, SweepTarget};
use hertzforge_sine::SweepRate;

fn sweep(band: (f64, f64), rate: f64) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band,
        rate: SweepRate::LogOctPerMin(rate),
        start_hz: band.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

/// The bound on each article response, g: the article reads about four times
/// the base at its 230 Hz resonance, so a 2 g bound binds there at full level.
const BOUND_G: f64 = 2.0;

#[test]
fn mimo_sine_the_backstop_notch_holds_the_article_at_its_bound_and_releases() {
    let s = SineStructure::BaseArticle6x4;
    let mut st = setup(s, sweep((150.0, 350.0), 2.0), "matrix");
    st.limits = (0..2)
        .map(|j| LimitChannel {
            channel: 5 + j,
            profile: flat(BOUND_G, 1.5, 3.0),
        })
        .collect();
    let r = run(engaged(s, st, law("matrix"), NOISE), 60.0);
    assert_eq!(
        r.outcome,
        Some(Outcome::Completed),
        "{:?}",
        r.events.iter().map(|e| e.kind).collect::<Vec<_>>()
    );
    let worst = r
        .ticks
        .iter()
        .flat_map(|t| t.limit[..2].iter().map(|l| l.norm()).collect::<Vec<_>>())
        .fold(0.0f64, f64::max);
    let deepest = r.ticks.iter().map(|t| t.notch_depth).fold(1.0f64, f64::min);
    println!(
        "article peak {worst:.3} g against a {BOUND_G} g bound; deepest notch {:.1} dB",
        20.0 * deepest.log10()
    );
    // Held at the bound: no more than 1 dB over it at any update.
    assert!(20.0 * (worst / BOUND_G).log10() < 1.0, "{worst}");
    assert!(deepest < 0.7, "the notch bound: {deepest}");
    assert!(r
        .events
        .iter()
        .any(|e| matches!(e.kind, EventKind::NotchIn { .. })));
    assert!(r.events.iter().any(|e| e.kind == EventKind::NotchOut));
    // Released: the last part of the sweep runs at full level, unnotched.
    let last = r
        .ticks
        .iter()
        .rev()
        .find(|t| t.frequency_hz < 340.0 && !t.settling)
        .unwrap();
    assert_eq!(last.notch_scale, 1.0);
    assert_eq!(last.notch_depth, 1.0);
    // The controls were judged against the notched reference and stayed in
    // their bands.
    let worst_err = r
        .ticks
        .iter()
        .filter(|t| !t.settling)
        .flat_map(|t| t.errors_db().to_vec())
        .fold(0.0f64, |a, e| a.max(e.abs()));
    println!("worst control error against the judged reference {worst_err:.2} dB");
}

#[test]
fn mimo_sine_the_near_singular_structure_survives_a_sweep() {
    let s = SineStructure::NearSingular4x4;
    let st = setup(s, sweep((300.0, 480.0), 1.0), "matrix");
    let r = run(engaged(s, st, law("matrix"), NOISE), 60.0);
    let core = r.service.core().unwrap();
    if let Some(e) = r
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Stopping { .. }))
    {
        println!("stopped at {:.1} Hz: {:?}", e.frequency_hz, e.kind);
    }
    println!(
        "near-singular: outcome {:?}, largest sample {:.3} V, over ceiling {} updates",
        r.outcome,
        core.largest_sample_v(),
        core.over_ceiling_count()
    );
    // It completes or ends on the engine's own stop; no drive over its ceiling.
    let ended = r.outcome.expect("the run ended");
    assert!(
        ended == Outcome::Completed
            || matches!(
                ended,
                Outcome::ControlAbort { .. } | Outcome::Divergence | Outcome::LawFault
            ),
        "{ended:?}"
    );
    assert!(core.largest_sample_v() <= 2.0 + 1e-6);
    // The sweep reached the frequencies where the response is near singular.
    assert!(r
        .ticks
        .iter()
        .any(|t| (t.frequency_hz - SINE_NEAR_SINGULAR_HZ).abs() < 10.0));
}
