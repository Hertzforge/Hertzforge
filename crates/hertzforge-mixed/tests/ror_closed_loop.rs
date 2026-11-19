//! Random-on-random and the union, closed loop on the lagged resonant plant of
//! `common` (300 Hz, Q 8, 40 frames of delay): a fixed band, a swept band and a
//! band following another at a ratio each hold their added level, and in the
//! combined test the composite mean square is the sum of the parts' (V2 in
//! the loop).

mod common;
use common::*;
use hertzforge_mixed::{
    parts::{Direction, PartSchedule, PartSweepTarget, SweepLaw},
    predict, MixedService, PartKind,
};

fn sweep(lo: f64, hi: f64, rate: f64) -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: lo,
        hi_hz: hi,
        start_hz: lo,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate,
        target: PartSweepTarget::Sweeps { n: 4 },
    }
}

/// Each band's error (dB) every second for `secs` after `skip` seconds, by
/// band in parts order.
fn band_errors(s: &mut MixedService, skip: f64, secs: f64) -> Vec<Vec<f64>> {
    run_for(s, skip);
    let mut out: Vec<Vec<f64>> = Vec::new();
    for _ in 0..secs as usize {
        assert!(run_for(s, 1.0), "the run ended: {:?}", s.end_reason());
        let st = s.live_status();
        let bands: Vec<f64> = st
            .parts
            .iter()
            .filter(|r| r.kind == PartKind::Narrowband)
            .map(|r| r.error_db.unwrap_or(f64::NAN))
            .collect();
        if out.is_empty() {
            out = vec![Vec::new(); bands.len()];
        }
        for (o, b) in out.iter_mut().zip(bands) {
            o.push(b);
        }
    }
    out
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// A 40 Hz band of 0.05 g^2/Hz added at 600 Hz (fixed), a 40 Hz band of 0.05
/// swept 450 to 750 Hz at 2 Hz/s, and one following the swept band at 0.6 of
/// its center, all on 0.01 g^2/Hz. Each band's moving-frame level, judged as
/// floor plus band against the reference, holds within 0.5 dB on average over
/// 30 s (its own scatter at the degrees of freedom it reaches is about
/// 0.5 dB), and the floor holds within 0.5 dB.
#[test]
fn fixed_swept_and_following_bands_hold_their_level() {
    let cfg = config(vec![
        floor(0.01),
        band("fixed", fixed(600.0), 0.05, 40.0),
        band("swept", sweep(450.0, 750.0, 2.0), 0.05, 40.0),
        band(
            "follow",
            PartSchedule::Follow {
                of: "swept".into(),
                ratio: 0.6,
            },
            0.05,
            40.0,
        ),
    ]);
    let mut s = engaged(cfg, DELAY);
    let e = band_errors(&mut s, 15.0, 30.0);
    let st = s.live_status();
    for (name, v) in ["fixed", "swept", "follow"].iter().zip(&e) {
        let row = st.parts.iter().find(|r| r.id == *name).unwrap();
        eprintln!(
            "band {name}: mean {:+.3} dB, last {:+.3} dB, dof {:.0}, gain {:+.2} dB",
            mean(v),
            v.last().unwrap(),
            row.dof.unwrap(),
            row.gain_db.unwrap()
        );
        assert!(mean(v).abs() < 0.5, "{name}: {v:?}");
    }
    eprintln!("band floor: {:+.3} dB rms", st.floor.error_db_rms.unwrap());
    assert!(st.floor.error_db_rms.unwrap().abs() < 0.5);
}

/// The union: the floor, a dwelling tone, a swept tone and a swept band. The
/// control composite's mean square, averaged over 20 s, is the sum of the
/// parts' (the prediction's total) within 5 %: V2 in the loop.
#[test]
fn v2_in_the_loop_the_composite_is_the_sum_of_the_parts() {
    let cfg = config(vec![
        floor(0.01),
        tone("t1", fixed(250.0), 2.0),
        tone("t2", sweep(380.0, 520.0, 2.0), 1.5),
        band("b1", sweep(600.0, 900.0, 3.0), 0.05, 40.0),
    ]);
    let want = predict(&cfg).unwrap().total_ms_g2;
    let mut s = engaged(cfg, DELAY);
    run_for(&mut s, 18.0);
    let mut ms = Vec::new();
    for _ in 0..80 {
        assert!(run_for(&mut s, 0.25), "{:?}", s.end_reason());
        ms.push(s.live_status().totals.control_rms_g.unwrap().powi(2));
    }
    let got = mean(&ms);
    let st = s.live_status();
    for r in &st.parts {
        eprintln!("union {}: err {:?} dB", r.id, r.error_db);
    }
    eprintln!("union: composite {got:.3} g^2 against the parts' {want:.3} g^2");
    assert!((got / want - 1.0).abs() < 0.05, "{got} {want}");
}
