//! Evidence, not a requirement: what each law does when the reference moves
//! faster than the loop can see. A 12 dB rise and a 12 dB fall over 1 Hz and
//! over 0.1 Hz in a sweep at one and at four octaves a minute, and a 12 dB
//! level step at a dwell, on `sine-loopback-4x4` and `sine-linear-4x4`.
//! Printed update by update around the step. Run with
//! `cargo test -p hertzforge-mimo-sine --test steep_reference -- --ignored --nocapture`.
//! The tests that are not ignored hold `matrix` to its requirement on the
//! simulator's loopback and linear structures.
//! Optional filters: `STEEP_LAW`, `STEEP_STRUCTURE`, `STEEP_TRACE=1` for the
//! update-by-update print.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::core::Command;
use hertzforge_mimo_sine::records::{EventKind, Outcome};
use hertzforge_mimo_sine::*;
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{
    BidirSweepConfig, LadderConfig, RungFor, SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;

/// A profile at `lo` up to `f1` and `hi` from `f2`, bands as given.
fn stepped_profile(lo: f64, hi: f64, f1: f64, f2: f64, alarm: f64, abort: f64) -> SineProfile {
    let mut p = SineProfile::new("steep");
    p.global_alarm_db = (alarm, alarm);
    p.global_abort_db = (abort, abort);
    p.push(ProfileRow::accel(5.0, lo));
    p.push(ProfileRow::accel(f1, lo));
    p.push(ProfileRow::accel(f2, hi));
    p.push(ProfileRow::accel(2000.0, hi));
    p
}

fn sweep(rate: f64) -> TestPlan {
    sweep_over((40.0, 80.0), rate)
}

fn sweep_over(band: (f64, f64), rate: f64) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band,
        rate: SweepRate::LogOctPerMin(rate),
        start_hz: band.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

fn phased(st: &mut MimoSineSetup, id: &str) {
    if !law(id).manifest().amplitude_only {
        st.phase_specified = true;
        for c in &mut st.controls {
            c.phase = vec![PhasePoint {
                freq_hz: 5.0,
                phase_deg: 0.0,
            }];
        }
    }
}

/// The response the drive in force gives by the identification, control 0..n
/// (the true over-test on a structure the identification matches; the
/// tracked amplitude lags it).
fn predicted(r: &Run, t: &hertzforge_mimo_sine::records::TickRecord) -> Vec<C64> {
    let ls = r.service.law_setup().expect("engaged");
    let id = &ls.identification;
    let (n, m) = (ls.controls, ls.drives);
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    hertzforge_mimo_sine_law_math::setup::interp_stack(
        &id.frequency_hz,
        id.response.data(),
        n * m,
        t.frequency_hz,
        &mut h,
    );
    let d = &t.drive[..m];
    (0..n)
        .map(|i| (0..m).map(|j| h[i * m + j] * d[j]).sum())
        .collect()
}

struct Summary {
    outcome: Option<Outcome>,
    true_over_db: f64,
    /// From the end of the step (60 Hz) to the last update inside the window
    /// with a true error over 2 dB, s (the identification alone leaves about 1 dB).
    recover_s: f64,
    over_db: f64,
    under_db: f64,
    capped: usize,
    over_ceiling: usize,
    alarms: usize,
}

fn summarize(r: &Run, around: (f64, f64)) -> Summary {
    let mut over = f64::NEG_INFINITY;
    let mut under = f64::INFINITY;
    let mut capped = 0;
    let mut ceil = 0;
    let mut true_over = f64::NEG_INFINITY;
    let mut t_end = f64::NAN;
    let mut last_bad = f64::NAN;
    for t in r
        .ticks
        .iter()
        .filter(|t| t.time_s > 0.5 && t.miss.is_none())
    {
        if t.cap_bound {
            capped += 1;
        }
        if t.ceiling_scale < 1.0 {
            ceil += 1;
        }
        let inside = t.frequency_hz >= around.0 && t.frequency_hz <= around.1 || around.0 < 0.0;
        if inside {
            if t_end.is_nan() && t.frequency_hz >= 60.0 {
                t_end = t.time_s;
            }
            for (c, rf) in predicted(r, t).iter().zip(&t.reference) {
                let e = 20.0 * (c.norm().max(1e-300) / rf.norm().max(1e-300)).log10();
                true_over = true_over.max(e);
                if e.abs() > 2.0 {
                    last_bad = t.time_s;
                }
            }
        }
        // The control over its reference, at any time (settling or not).
        for (c, rf) in t.controls().iter().zip(&t.reference) {
            let e = 20.0 * (c.norm().max(1e-300) / rf.norm().max(1e-300)).log10();
            if t.frequency_hz >= around.0 && t.frequency_hz <= around.1 || around.0 < 0.0 {
                over = over.max(e);
                under = under.min(e);
            }
        }
    }
    Summary {
        outcome: r.outcome,
        true_over_db: true_over,
        recover_s: if last_bad.is_nan() {
            0.0
        } else {
            (last_bad - t_end).max(0.0)
        },
        over_db: over,
        under_db: under,
        capped,
        over_ceiling: ceil,
        alarms: r
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Alarm { .. }))
            .count(),
    }
}

fn trace(r: &Run, f_lo: f64, f_hi: f64) {
    if std::env::var("STEEP_TRACE").is_err() {
        return;
    }
    println!("    t_s     f_Hz    |R|      |C|      err_dB  |HD|     |Dask|   |Dforce|  gain     cap ceil set hold");
    for t in r
        .ticks
        .iter()
        .filter(|t| t.frequency_hz >= f_lo && t.frequency_hz <= f_hi)
    {
        let c = t.controls()[0];
        let rf = t.reference[0];
        let hd = predicted(r, t)[0];
        println!(
            "    {:6.3} {:7.3} {:8.5} {:8.5} {:+7.2} {:8.5} {:8.5} {:8.5} {:8.5} {}   {}    {}   {:?}",
            t.time_s,
            t.frequency_hz,
            rf.norm(),
            c.norm(),
            20.0 * (c.norm().max(1e-300) / rf.norm().max(1e-300)).log10(),
            hd.norm(),
            t.drive_asked[0].norm(),
            t.drive[0].norm(),
            t.values[0],
            t.cap_bound as u8,
            (t.ceiling_scale < 1.0) as u8,
            t.settling as u8,
            t.hold,
        );
    }
}

const LAWS: [&str; 5] = ["matrix", "paired", "steepest_descent", "secant", "ratio"];

fn wanted(id: &str, s: SineStructure) -> bool {
    std::env::var("STEEP_LAW").map(|l| l == id).unwrap_or(true)
        && std::env::var("STEEP_STRUCTURE")
            .map(|n| n == s.name())
            .unwrap_or(true)
}

fn params(st: &mut MimoSineSetup) {
    if let Ok(p) = std::env::var("STEEP_PARAMS") {
        let v: serde_json::Value = serde_json::from_str(&p).expect("STEEP_PARAMS is JSON");
        st.law.params = v.as_object().expect("an object").clone();
    }
}

#[test]
#[ignore]
fn mimo_sine_steep_reference_sweep_evidence() {
    println!("law | structure | rate oct/min | step | width Hz | outcome | true over dB | read over dB | read under dB | capped | over ceiling | alarms | recover s");
    for s in [SineStructure::Loopback4x4, SineStructure::Linear4x4] {
        // `STEEP_HI`: the reference's top (the bench driver's B6 row uses 0.1
        // with 1 Hz lines, ceilings 0.8 V and no added noise).
        let hi = std::env::var("STEEP_HI")
            .map(|v| v.parse().expect("a number"))
            .unwrap_or(s.full_level_g());
        let noise = std::env::var("STEEP_NOISE")
            .map(|v| v.parse().expect("a number"))
            .unwrap_or(NOISE);
        let lo = hi / 4.0;
        for id in LAWS {
            if !wanted(id, s) {
                continue;
            }
            if !law(id).manifest().accepts_test_type(TestType::Sweep) {
                continue;
            }
            for rate in [1.0, 4.0] {
                for rise in [true, false] {
                    for width in [1.0, 0.1, 0.0] {
                        if width == 0.0 && !rise {
                            continue;
                        }
                        // Width 0: no step, the baseline (the reference at
                        // `hi` throughout).
                        let (a, b) = if width == 0.0 {
                            (hi, hi)
                        } else if rise {
                            (lo, hi)
                        } else {
                            (hi, lo)
                        };
                        let width = if width == 0.0 { 1.0 } else { width };
                        let mut st = setup(s, sweep(rate), id);
                        for c in &mut st.controls {
                            c.profile = stepped_profile(a, b, 60.0 - width, 60.0, 20.0, 20.0);
                        }
                        if let Ok(v) = std::env::var("STEEP_RES") {
                            st.identification.resolution_hz = v.parse().expect("a number");
                        }
                        if std::env::var("STEEP_HI").is_ok() {
                            for d in &mut st.drives {
                                d.ceiling_vpk = 0.8;
                                d.abort_vpk = 0.95;
                            }
                        }
                        phased(&mut st, id);
                        params(&mut st);
                        let max_s = 60.0 / rate + 5.0;
                        let r = run(engaged(s, st, law(id), noise), max_s);
                        let sm = summarize(&r, (60.0 - width - 0.5, 63.0));
                        println!(
                            "{id} | {} | {rate} | {} | {width} | {:?} | {:+.2} | {:+.2} | {:+.2} | {} | {} | {} | {:.2}",
                            s.name(),
                            if rise { "rise" } else { "fall" },
                            sm.outcome,
                            sm.true_over_db,
                            sm.over_db,
                            sm.under_db,
                            sm.capped,
                            sm.over_ceiling,
                            sm.alarms,
                            sm.recover_s
                        );
                        trace(&r, 60.0 - width - 0.2, 61.5);
                    }
                }
            }
        }
    }
}

#[test]
#[ignore]
fn mimo_sine_steep_reference_dwell_level_step_evidence() {
    for s in [SineStructure::Loopback4x4, SineStructure::Linear4x4] {
        for id in LAWS {
            if !wanted(id, s) {
                continue;
            }
            let mut st = setup(s, dwell(60.0, 30.0), id);
            st.schedule.mode = LevelMode::Manual;
            st.schedule.ladder = LadderConfig {
                sysid_level_db: -20.0,
                start_db: -12.0,
                table_db: vec![-12.0, 0.0],
                per_rung: RungFor::Seconds(1e6),
                full_for: RungFor::Seconds(1e6),
            };
            phased(&mut st, id);
            params(&mut st);
            let mut svc = engaged(s, st, law(id), NOISE);
            let (rt, re) = svc.take_records().unwrap();
            let cmd = svc.commands();
            for _ in 0..(3.0 * FS / 512.0) as usize {
                svc.run_batches(1).unwrap();
            }
            cmd.send(Command::StepLevel(12.0)).unwrap();
            for _ in 0..(3.0 * FS / 512.0) as usize {
                svc.run_batches(1).unwrap();
            }
            let r = Run {
                ticks: rt.try_iter().collect(),
                events: re.try_iter().collect(),
                outcome: None,
                service: svc,
            };
            let sm = summarize(&r, (-1.0, 0.0));
            println!(
                "{id} | {} | dwell level step +12 dB | true over {:+.2} dB | read over {:+.2} dB | under {:+.2} dB | capped {} | alarms {}",
                s.name(),
                sm.true_over_db,
                sm.over_db,
                sm.under_db,
                sm.capped,
                sm.alarms
            );
        }
    }
}

/// A stepped sine across the reference's step, for every law (the laws that
/// do not accept a sweep meet a reference step this way): 56 to 64 Hz in
/// 1 Hz steps, 2 s each, the 12 dB rise between 59 and 60 Hz.
#[test]
#[ignore]
fn mimo_sine_steep_reference_stepped_evidence() {
    println!("law | structure | step | outcome | true over dB | read over dB | read under dB | capped | alarms");
    for s in [SineStructure::Loopback4x4, SineStructure::Linear4x4] {
        let hi = s.full_level_g();
        let lo = hi / 4.0;
        for id in LAWS {
            if !wanted(id, s) {
                continue;
            }
            if !law(id).manifest().accepts_test_type(TestType::Stepped) {
                continue;
            }
            for rise in [true, false] {
                let (a, b) = if rise { (lo, hi) } else { (hi, lo) };
                let plan = TestPlan::Stepped(StepPlan::List {
                    frequencies_hz: (56..=64).map(|f| f as f64).collect(),
                    dwell: hertzforge_sine::scheduler::DwellFor::Seconds(2.0),
                });
                let mut st = setup(s, plan, id);
                for c in &mut st.controls {
                    c.profile = stepped_profile(a, b, 59.0, 60.0, 20.0, 20.0);
                }
                phased(&mut st, id);
                params(&mut st);
                let r = run(engaged(s, st, law(id), NOISE), 25.0);
                let sm = summarize(&r, (58.5, 61.5));
                println!(
                    "{id} | {} | {} | {:?} | {:+.2} | {:+.2} | {:+.2} | {} | {}",
                    s.name(),
                    if rise { "rise" } else { "fall" },
                    sm.outcome,
                    sm.true_over_db,
                    sm.over_db,
                    sm.under_db,
                    sm.capped,
                    sm.alarms
                );
                trace(&r, 58.5, 61.5);
            }
        }
    }
}

/// One sweep of `matrix` from 55 to 65 Hz through a 12 dB step between
/// `60 - width` and 60 Hz on `s`, judged with 20 dB bands so nothing stops
/// it; the summary around the step.
fn matrix_through_a_step(s: SineStructure, rate: f64, rise: bool, width: f64) -> Summary {
    let hi = s.full_level_g();
    let (a, b) = if rise { (hi / 4.0, hi) } else { (hi, hi / 4.0) };
    let mut st = setup(s, sweep_over((55.0, 65.0), rate), "matrix");
    for c in &mut st.controls {
        c.profile = stepped_profile(a, b, 60.0 - width, 60.0, 20.0, 20.0);
    }
    let r = run(engaged(s, st, law("matrix"), NOISE), 40.0);
    summarize(&r, (60.0 - width - 0.5, 63.0))
}

/// Requirement: through a 12 dB rise or fall over 1 Hz or 0.1 Hz at one
/// octave a minute, the response `matrix` drives is never more than 1 dB
/// over the reference beyond what the identification leaves with no step
/// (about 1 dB here: 2 dB in all), and the run completes. Before the fix,
/// on the loopback: 4.1, 18.2, 6.0 dB and an abort.
#[test]
fn mimo_sine_matrix_holds_a_steep_reference_on_the_loopback() {
    for rise in [true, false] {
        for width in [1.0, 0.1] {
            let sm = matrix_through_a_step(SineStructure::Loopback4x4, 1.0, rise, width);
            println!(
                "rise {rise} width {width}: {:?}, true over {:+.2} dB, read over {:+.2} dB",
                sm.outcome, sm.true_over_db, sm.over_db
            );
            assert_eq!(sm.outcome, Some(Outcome::Completed));
            assert!(
                sm.true_over_db < 2.0,
                "rise {rise} width {width}: {}",
                sm.true_over_db
            );
        }
    }
}
