//! Scheduler gates: every edge rule as its own test, exact dwell
//! cycle accrual, analytic step-sine ladders, and the RSTD full pass on a
//! synthetic SDOF (search accuracy, settled-phase capture, drift tracking,
//! and the band-exit latch).

use eigen_dsp::sweep::SweepRate;
use eigen_sine::{
    BidirSweep, BidirSweepConfig, DwellFor, DwellPlan, DwellRun, LadderConfig, LadderPlan,
    RstdConfig, RstdDwellMode, RstdEvent, RstdPlan, RstdState, RungFor, StepSine, StepSineConfig,
    StepSpacing, SweepDirection, SweepState, SweepTarget,
};
use std::f64::consts::{PI, TAU};

fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: {a} vs {b} (tol {tol}, err {})",
        (a - b).abs()
    );
}

/// A 1-octave band swept at 60 oct/min = 1 octave per second: one full pass
/// takes exactly 1 s of test time.
fn one_second_sweep(target: SweepTarget) -> BidirSweep {
    BidirSweep::new(BidirSweepConfig {
        band: (10.0, 20.0),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: 10.0,
        initial_direction: SweepDirection::Up,
        target,
    })
    .unwrap()
}

// =============================================================================
// BidirSweep - the edge rule set
// =============================================================================

#[test]
fn f17_bounce_counts_one_sweep_and_flips() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    for _ in 0..12 {
        s.tick(0.1);
    }
    // 1.2 s: bounced at 20 Hz at t=1.0 and swept 0.2 s back down.
    assert_eq!(s.sweeps_done(), 1);
    let p = s.progress();
    assert_eq!(p.direction, SweepDirection::Down);
    assert_close(p.freq_hz, 20.0 * (-0.2f64).exp2(), 1e-9, "post-bounce f");
    assert_close(p.elapsed_test_s, 1.2, 1e-12, "test clock");
}

#[test]
fn f17_up_plus_down_counts_two_sweeps() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    for _ in 0..22 {
        s.tick(0.1);
    }
    // 2.2 s: edge at 1.0 s (up done) and at 2.0 s (down done) -> 2 sweeps.
    assert_eq!(s.sweeps_done(), 2);
    assert_eq!(s.progress().direction, SweepDirection::Up);
    assert_close(s.freq_hz(), 10.0 * (0.2f64).exp2(), 1e-9, "back up 0.2 oct");
}

#[test]
fn f17_multiple_bounces_within_one_large_tick() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    s.tick(3.5); // 3 full passes + half a pass
    assert_eq!(s.sweeps_done(), 3);
    assert_close(s.elapsed_test_s(), 3.5, 1e-12, "test clock");
    // Passes: up (1s), down (1s), up (1s), then 0.5 s down from 20 Hz.
    assert_close(s.freq_hz(), 20.0 * (-0.5f64).exp2(), 1e-9, "mid 4th pass");
}

#[test]
fn f17_odd_sweep_target_ends_at_far_edge() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(3));
    for _ in 0..50 {
        s.tick(0.1);
    }
    assert!(s.done());
    assert_eq!(s.sweeps_done(), 3);
    // Passes 1 (up), 2 (down), 3 (up): an odd count ends at the FAR edge.
    assert_close(s.freq_hz(), 20.0, 0.0, "ends at far edge");
    assert_eq!(s.progress().state, SweepState::Done);
    // Test time stops AT the stopping point, not at the tick boundary.
    assert_close(s.elapsed_test_s(), 3.0, 1e-9, "clock stops at target");
}

#[test]
fn f17_duration_target_stops_mid_sweep_exactly() {
    let mut s = one_second_sweep(SweepTarget::DurationS(0.5));
    s.tick(0.3);
    assert!(!s.done());
    s.tick(0.3); // only 0.2 s of this tick is test time
    assert!(s.done());
    assert_close(s.elapsed_test_s(), 0.5, 1e-12, "duration met exactly");
    assert_close(s.freq_hz(), 10.0 * (0.5f64).exp2(), 1e-9, "freq at stop");
    assert_eq!(s.sweeps_done(), 0);
}

#[test]
fn f17_whichever_first_sweeps_side() {
    let mut s = one_second_sweep(SweepTarget::WhicheverFirst {
        sweeps: 1,
        duration_s: 100.0,
    });
    for _ in 0..15 {
        s.tick(0.1);
    }
    assert!(s.done(), "sweeps side must stop the run first");
    assert_eq!(s.sweeps_done(), 1);
    assert_close(s.elapsed_test_s(), 1.0, 1e-9, "stopped at the edge");
}

#[test]
fn f17_whichever_first_duration_side() {
    let mut s = one_second_sweep(SweepTarget::WhicheverFirst {
        sweeps: 100,
        duration_s: 0.4,
    });
    for _ in 0..15 {
        s.tick(0.1);
    }
    assert!(s.done(), "duration side must stop the run first");
    assert_eq!(s.sweeps_done(), 0);
    assert_close(s.elapsed_test_s(), 0.4, 1e-12, "duration met");
}

#[test]
fn f17_hold_pauses_test_clock_and_freq() {
    let mut s = one_second_sweep(SweepTarget::DurationS(10.0));
    s.tick(0.3);
    let f_held = s.freq_hz();
    s.hold();
    assert_eq!(s.progress().state, SweepState::Held);
    for _ in 0..50 {
        s.tick(1.0); // 50 s of wall time while held
    }
    assert_close(s.freq_hz(), f_held, 0.0, "freq frozen during hold");
    assert_close(s.elapsed_test_s(), 0.3, 1e-12, "TEST clock paused");
    assert_eq!(s.sweeps_done(), 0);
    s.resume();
    s.tick(0.2);
    assert_close(s.elapsed_test_s(), 0.5, 1e-12, "clock resumes");
}

#[test]
fn f17_reverse_partial_does_not_count() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    s.tick(0.3); // partway up
    s.reverse(); // abandon the up pass
    assert_eq!(s.sweeps_done(), 0, "the flip itself never counts");
    assert_eq!(s.progress().direction, SweepDirection::Down);
    s.tick(0.2); // still descending, no edge yet
    assert_eq!(s.sweeps_done(), 0, "partial still uncounted mid-pass");
    s.tick(0.2); // reaches 10 Hz at 0.3 s after the reverse
                 // Only the band-edge COMPLETION counts.
    assert_eq!(s.sweeps_done(), 1);
    assert_eq!(s.progress().direction, SweepDirection::Up);
}

#[test]
fn f17_jog_clamps_to_band_and_works_while_held() {
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    s.tick(0.3);
    s.hold();
    s.jog(1e6);
    assert_close(s.freq_hz(), 20.0, 0.0, "jog clamped to band top");
    assert_eq!(s.sweeps_done(), 0, "jogging to an edge never counts");
    s.jog(-1e6);
    assert_close(s.freq_hz(), 10.0, 0.0, "jog clamped to band bottom");
    s.jog(5.0);
    assert_close(s.freq_hz(), 15.0, 1e-12, "plain jog while held");
}

#[test]
fn f17_resume_from_jogged_edge_flips_inward_without_counting() {
    // Jog to the LOW edge while the direction still points Down, then resume:
    // the zero-length pass must not count; the sweep flips inward and runs.
    let mut s = one_second_sweep(SweepTarget::Sweeps(10));
    s.tick(1.5); // bounced at 20 (1 sweep), now descending
    assert_eq!(s.sweeps_done(), 1);
    s.hold();
    s.jog(-1e6); // pinned at 10 Hz, direction Down
    s.resume();
    s.tick(0.1);
    assert_eq!(s.sweeps_done(), 1, "zero-length pass at the edge no-counts");
    assert_eq!(s.progress().direction, SweepDirection::Up);
    assert_close(s.freq_hz(), 10.0 * (0.1f64).exp2(), 1e-9, "sweeping again");
}

#[test]
fn bidir_sweep_linear_law_and_edge_start_normalization() {
    // Linear 5 Hz/s over (10, 20) = 2 s per pass; start ON the top edge with
    // direction Up gets flipped inward at construction.
    let mut s = BidirSweep::new(BidirSweepConfig {
        band: (10.0, 20.0),
        rate: SweepRate::LinearHzPerSec(5.0),
        start_hz: 20.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(4),
    })
    .unwrap();
    assert_eq!(s.progress().direction, SweepDirection::Down);
    s.tick(1.0);
    assert_close(s.freq_hz(), 15.0, 1e-12, "linear descent");
    s.tick(1.5);
    assert_eq!(s.sweeps_done(), 1);
    assert_close(s.freq_hz(), 12.5, 1e-12, "bounced back up");
}

#[test]
fn bidir_sweep_config_validation_and_serde() {
    let good = BidirSweepConfig {
        band: (10.0, 20.0),
        rate: SweepRate::LogOctPerMin(1.0),
        start_hz: 10.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(2),
    };
    assert!(BidirSweep::new(BidirSweepConfig {
        band: (20.0, 10.0),
        ..good
    })
    .is_err());
    assert!(BidirSweep::new(BidirSweepConfig {
        band: (0.0, 10.0),
        ..good
    })
    .is_err());
    assert!(BidirSweep::new(BidirSweepConfig {
        start_hz: 30.0,
        ..good
    })
    .is_err());
    assert!(BidirSweep::new(BidirSweepConfig {
        rate: SweepRate::LogOctPerMin(0.0),
        ..good
    })
    .is_err());
    assert!(BidirSweep::new(BidirSweepConfig {
        target: SweepTarget::Sweeps(0),
        ..good
    })
    .is_err());
    assert!(BidirSweep::new(BidirSweepConfig {
        target: SweepTarget::WhicheverFirst {
            sweeps: 1,
            duration_s: 0.0
        },
        ..good
    })
    .is_err());

    // Serde round-trip (the foreign SweepRate rides the remote-derive mirror).
    let json = serde_json::to_string(&good).unwrap();
    let back: BidirSweepConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, good);
}

// =============================================================================
// Dwell - exact cycle accrual + hold semantics
// =============================================================================

#[test]
fn dwell_cycles_accrue_exactly_as_freq_times_dt() {
    let mut d = DwellRun::new(DwellPlan {
        freq_hz: 100.0,
        for_: DwellFor::Cycles(100),
    })
    .unwrap();
    // 100 Hz * 0.25 s = 25 cycles per tick, exact in binary floating point.
    assert!(!d.tick(0.25));
    assert!(!d.tick(0.25));
    assert!(!d.tick(0.25));
    assert_close(d.cycles(), 75.0, 0.0, "exact accrual");
    assert!(d.tick(0.25), "cycle target met exactly on the 4th tick");
    assert_close(d.cycles(), 100.0, 0.0, "exact total");
    assert!(d.done());
}

#[test]
fn dwell_seconds_target() {
    let mut d = DwellRun::new(DwellPlan {
        freq_hz: 42.0,
        for_: DwellFor::Seconds(1.0),
    })
    .unwrap();
    assert!(!d.tick(0.6));
    assert!(d.tick(0.6));
    assert!(d.done());
    assert_close(d.freq_hz(), 42.0, 0.0, "dwell freq");
}

#[test]
fn f17_dwell_hold_pauses_seconds_and_cycles() {
    // A dwell "hold" = output continues, PROGRESS pauses (test clock rule,
    // matching the sweep semantics - documented on DwellRun).
    let mut d = DwellRun::new(DwellPlan {
        freq_hz: 50.0,
        for_: DwellFor::Cycles(1000),
    })
    .unwrap();
    d.tick(1.0);
    assert_close(d.cycles(), 50.0, 0.0, "pre-hold cycles");
    d.hold();
    for _ in 0..10 {
        d.tick(1.0);
    }
    assert_close(d.cycles(), 50.0, 0.0, "cycles paused during hold");
    assert_close(d.elapsed_s(), 1.0, 0.0, "seconds paused during hold");
    d.resume();
    d.tick(1.0);
    assert_close(d.cycles(), 100.0, 0.0, "resumed accrual");
}

#[test]
fn dwell_validation() {
    assert!(DwellRun::new(DwellPlan {
        freq_hz: 0.0,
        for_: DwellFor::Seconds(1.0)
    })
    .is_err());
    assert!(DwellRun::new(DwellPlan {
        freq_hz: 10.0,
        for_: DwellFor::Seconds(0.0)
    })
    .is_err());
    assert!(DwellRun::new(DwellPlan {
        freq_hz: 10.0,
        for_: DwellFor::Cycles(0)
    })
    .is_err());
}

// =============================================================================
// Step sine
// =============================================================================

#[test]
fn step_sine_log_ladder_is_analytic() {
    // 2 octaves at 10 steps/octave: f_k = 10 * 2^(k/10), 21 points, top edge
    // exact.
    let s = StepSine::new(StepSineConfig {
        band: (10.0, 40.0),
        steps_per_octave: 10.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(1.0)),
    })
    .unwrap();
    let ladder = s.ladder();
    assert_eq!(ladder.len(), 21);
    for (k, &f) in ladder.iter().enumerate().take(20) {
        assert_close(
            f,
            10.0 * (k as f64 / 10.0).exp2(),
            1e-9,
            &format!("ladder step {k}"),
        );
    }
    assert_close(ladder[20], 40.0, 0.0, "top edge exact");
}

#[test]
fn step_sine_linear_ladder_spacing() {
    let s = StepSine::new(StepSineConfig {
        band: (10.0, 40.0),
        steps_per_octave: 10.0,
        spacing: StepSpacing::Linear,
        dwell: Some(DwellFor::Seconds(1.0)),
    })
    .unwrap();
    let ladder = s.ladder();
    assert_eq!(ladder.len(), 21, "same step count as the log rule");
    for (i, &f) in ladder.iter().enumerate() {
        assert_close(f, 10.0 + 1.5 * i as f64, 1e-9, &format!("linear step {i}"));
    }
}

#[test]
fn step_sine_default_dwell_is_max_60s_20cycles_per_step() {
    let s = StepSine::new(StepSineConfig {
        band: (0.1, 10.0),
        steps_per_octave: 1.0,
        spacing: StepSpacing::Log,
        dwell: None,
    })
    .unwrap();
    // At 0.1 Hz: 20 cycles = 200 s > 60 s -> cycles side dominates.
    assert_eq!(s.step_dwell(0), Some(DwellFor::Seconds(200.0)));
    // At the top (10 Hz): 20 cycles = 2 s < 60 s -> the minute dominates.
    let last = s.ladder().len() - 1;
    assert_eq!(s.step_dwell(last), Some(DwellFor::Seconds(60.0)));
}

#[test]
fn step_sine_advances_on_dwell_completion_and_holds_per_f17() {
    let mut s = StepSine::new(StepSineConfig {
        band: (10.0, 40.0),
        steps_per_octave: 1.0, // ladder [10, 20, 40]
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Cycles(10)),
    })
    .unwrap();
    assert_eq!(s.ladder().len(), 3);
    assert_close(s.freq_hz(), 10.0, 0.0, "start at the bottom step");
    // 10 cycles at 10 Hz = 1.0 s.
    s.tick(0.5);
    assert_close(s.freq_hz(), 10.0, 0.0, "mid-step");
    // Hold pauses the step clock.
    s.hold();
    s.tick(100.0);
    assert_close(s.progress().step_cycles, 5.0, 0.0, "held step clock");
    s.resume();
    s.tick(0.5);
    assert_close(s.freq_hz(), 20.0, 0.0, "advanced to step 2");
    // 10 cycles at 20 Hz = 0.5 s.
    s.tick(0.5);
    assert_close(s.freq_hz(), 40.0, 0.0, "advanced to step 3");
    // 10 cycles at 40 Hz = 0.25 s -> done, frequency stays at the top.
    s.tick(0.25);
    assert!(s.done());
    assert_close(s.freq_hz(), 40.0, 0.0, "stays at top when done");
    assert_eq!(s.progress().state, SweepState::Done);
}

// =============================================================================
// RSTD - synthetic SDOF fixtures
// =============================================================================

/// Base-excited SDOF transmissibility `H(r) = (1 + i 2 zeta r)/(1 - r^2 +
/// i 2 zeta r)`: returns `(amp, phase)` in the tracking-filter convention
/// (phase ~ -pi/2 at resonance, negative slope through it).
fn sdof(f: f64, fn_hz: f64, q: f64) -> (f64, f64) {
    let zeta = 1.0 / (2.0 * q);
    let r = f / fn_hz;
    let (nr, ni) = (1.0, 2.0 * zeta * r);
    let (dr, di) = (1.0 - r * r, 2.0 * zeta * r);
    let amp = (nr * nr + ni * ni).sqrt() / (dr * dr + di * di).sqrt();
    let mut ph = ni.atan2(nr) - di.atan2(dr);
    if ph > PI {
        ph -= TAU;
    } else if ph <= -PI {
        ph += TAU;
    }
    (amp, ph)
}

fn rstd_config(dwell_mode: RstdDwellMode, dwell_for: DwellFor) -> RstdConfig {
    RstdConfig {
        band: (200.0, 300.0),
        level_db: -20.0,
        rate: SweepRate::LogOctPerMin(1.0),
        tracking_bw_hz: 10.0,
        settle_k: 5.0,
        dwell_mode,
        dwell_for,
    }
}

/// Drive the plan through Search+Settle+Capture against a fixed SDOF;
/// returns (events seen, settled phase fed during settle/capture).
fn run_search_and_capture(plan: &mut RstdPlan, fn_hz: f64, q: f64, dt: f64) -> Vec<RstdEvent> {
    let mut events = Vec::new();
    let mut guard = 0;
    while plan.state() != RstdState::Dwell && plan.state() != RstdState::Failed {
        let (amp, ph) = sdof(plan.freq_hz(), fn_hz, q);
        let (_f, ev) = plan.tick(dt, amp, ph);
        if let Some(e) = ev {
            events.push(e);
        }
        guard += 1;
        assert!(guard < 500_000, "RSTD never reached Dwell");
    }
    events
}

#[test]
fn rstd_full_pass_on_synthetic_sdof() {
    let (fn_hz, q) = (250.0, 25.0);
    let dt = 0.01;
    let mut plan =
        RstdPlan::new(rstd_config(RstdDwellMode::Fixed, DwellFor::Seconds(1.0))).unwrap();
    let events = run_search_and_capture(&mut plan, fn_hz, q, dt);

    // SearchDone with f_peak within 1% and Q within 15% (transmissibility
    // half-power widens the -3 dB band slightly vs the pure SDOF).
    let Some(RstdEvent::SearchDone {
        f_peak,
        q: q_est,
        amp,
    }) = events
        .iter()
        .find(|e| matches!(e, RstdEvent::SearchDone { .. }))
        .copied()
    else {
        panic!("no SearchDone event; got {events:?}");
    };
    assert!(
        (f_peak - fn_hz).abs() / fn_hz < 0.01,
        "f_peak {f_peak} not within 1% of {fn_hz}"
    );
    assert!(
        (q_est - q).abs() / q < 0.15,
        "Q estimate {q_est} not within 15% of {q}"
    );
    assert!(amp > 0.8 * q, "peak amp {amp} implausible for Q={q}");

    // Captured setpoint == the settled phase (exactly what was fed).
    let (_, settled_phase) = sdof(f_peak, fn_hz, q);
    let Some(RstdEvent::SetpointCaptured { phase_rad }) = events
        .iter()
        .find(|e| matches!(e, RstdEvent::SetpointCaptured { .. }))
        .copied()
    else {
        panic!("no SetpointCaptured event; got {events:?}");
    };
    assert_close(phase_rad, settled_phase, 0.0, "captured == settled phase");
    assert_eq!(plan.setpoint_rad(), Some(settled_phase));

    // Fixed dwell: frequency stays put; DwellComplete after 1 s of test time.
    let mut got_complete = false;
    for _ in 0..200 {
        let (amp, ph) = sdof(plan.freq_hz(), fn_hz, q);
        let (f, ev) = plan.tick(dt, amp, ph);
        assert_close(f, f_peak, 0.0, "fixed dwell holds f_peak");
        if ev == Some(RstdEvent::DwellComplete) {
            got_complete = true;
            break;
        }
    }
    assert!(got_complete, "no DwellComplete");
    assert_eq!(plan.state(), RstdState::Complete);
    assert_close(plan.dwell_elapsed_s(), 1.0, 1e-9, "dwell test seconds");
}

#[test]
fn rstd_phase_tracked_dwell_follows_fn_drift() {
    let (fn0, q) = (250.0, 25.0);
    let dt = 0.01;
    let mut plan = RstdPlan::new(rstd_config(
        RstdDwellMode::PhaseTracked,
        DwellFor::Seconds(1e9),
    ))
    .unwrap();
    run_search_and_capture(&mut plan, fn0, q, dt);
    assert_eq!(plan.state(), RstdState::Dwell);

    // Impose an f_n drift 250 -> 240 Hz at 1 Hz/s; the tracker must follow.
    let drift_s = 10.0;
    let n = (drift_s / dt) as usize;
    for i in 0..=n {
        let fn_now = fn0 - (i as f64) * dt; // 1 Hz per second
        let (amp, ph) = sdof(plan.freq_hz(), fn_now, q);
        let (_f, ev) = plan.tick(dt, amp, ph);
        assert!(
            ev.is_none(),
            "unexpected event during in-band drift: {ev:?}"
        );
    }
    assert_close(plan.freq_hz(), 240.0, 0.5, "tracked the drifted resonance");
    assert!(!plan.band_exit_latched());

    // Give the plant a moment at a fixed f_n and require tight lock.
    for _ in 0..200 {
        let (amp, ph) = sdof(plan.freq_hz(), 240.0, q);
        plan.tick(dt, amp, ph);
    }
    assert_close(plan.freq_hz(), 240.0, 0.1, "settled lock on 240 Hz");
}

#[test]
fn rstd_drift_out_of_band_latches_band_exit_and_holds() {
    let (fn0, q) = (250.0, 25.0);
    let dt = 0.01;
    let mut plan = RstdPlan::new(rstd_config(
        RstdDwellMode::PhaseTracked,
        DwellFor::Seconds(1e9),
    ))
    .unwrap();
    run_search_and_capture(&mut plan, fn0, q, dt);

    // Drift the resonance clear below the band's low edge (200 Hz).
    let mut band_exit_events = 0;
    let mut fn_now = fn0;
    for _ in 0..100_000 {
        fn_now = (fn_now - dt).max(150.0); // 1 Hz/s down to 150 Hz
        let (amp, ph) = sdof(plan.freq_hz(), fn_now, q);
        let (_f, ev) = plan.tick(dt, amp, ph);
        if ev == Some(RstdEvent::BandExit) {
            band_exit_events += 1;
        }
        if plan.band_exit_latched() && fn_now <= 150.0 {
            break;
        }
    }
    assert_eq!(band_exit_events, 1, "BandExit latches exactly once");
    assert!(plan.band_exit_latched());
    assert!(plan.held(), "band exit is alarm + HOLD (never silent)");
    assert_close(plan.freq_hz(), 200.0, 0.0, "pinned at the band edge");

    // The alarm hold pauses progress and cannot be resumed away.
    let cycles_before = plan.dwell_cycles();
    plan.resume();
    let (amp, ph) = sdof(plan.freq_hz(), fn_now, q);
    plan.tick(dt, amp, ph);
    assert_close(
        plan.dwell_cycles(),
        cycles_before,
        0.0,
        "no progress while band-exited",
    );
}

#[test]
fn rstd_search_failed_when_peak_not_in_band() {
    // Resonance far above the band: the in-band amplitude rises monotonically
    // to the top edge, the half-power fit is not well-posed, and the plan
    // must fail LOUDLY (SearchFailed), never silently dwell.
    let dt = 0.01;
    let mut plan =
        RstdPlan::new(rstd_config(RstdDwellMode::Fixed, DwellFor::Seconds(1.0))).unwrap();
    let mut events = Vec::new();
    let mut guard = 0;
    while plan.state() == RstdState::Search {
        let (amp, ph) = sdof(plan.freq_hz(), 500.0, 25.0);
        let (_f, ev) = plan.tick(dt, amp, ph);
        if let Some(e) = ev {
            events.push(e);
        }
        guard += 1;
        assert!(guard < 500_000, "search never completed");
    }
    assert_eq!(events, vec![RstdEvent::SearchFailed]);
    assert_eq!(plan.state(), RstdState::Failed);
    assert!(plan.estimate().is_none());
}

#[test]
fn rstd_hold_pauses_search_progress() {
    let dt = 0.01;
    let mut plan =
        RstdPlan::new(rstd_config(RstdDwellMode::Fixed, DwellFor::Seconds(1.0))).unwrap();
    let f0 = plan.freq_hz();
    let (amp, ph) = sdof(f0, 250.0, 25.0);
    plan.tick(dt, amp, ph);
    let f1 = plan.freq_hz();
    assert!(f1 > f0, "search sweeps up");
    plan.hold();
    for _ in 0..100 {
        plan.tick(dt, amp, ph);
    }
    assert_close(plan.freq_hz(), f1, 0.0, "held search does not advance");
    plan.resume();
    plan.tick(dt, amp, ph);
    assert!(plan.freq_hz() > f1, "resumed search advances");
}

// =============================================================================
// Ladder
// =============================================================================

#[test]
fn ladder_defaults_and_sweep_rungs_advance() {
    let mut l = LadderPlan::new(LadderConfig {
        per_rung: RungFor::Sweeps(2),
        full_for: RungFor::Sweeps(1),
        ..Default::default()
    })
    .unwrap();
    assert_close(l.level_db(), -12.0, 0.0, "starts at table[0]");
    l.note_sweep_completed();
    assert_close(l.level_db(), -12.0, 0.0, "one sweep is not enough");
    l.note_sweep_completed();
    assert_close(l.level_db(), -9.0, 0.0, "advanced after 2 sweeps");
    // Climb to full.
    for _ in 0..6 {
        l.note_sweep_completed();
    }
    assert_close(l.level_db(), 0.0, 0.0, "at full level");
    assert!(!l.done());
    l.note_sweep_completed(); // full_for = 1 sweep
    assert!(l.done());
}

#[test]
fn f17_ladder_rung_clock_pauses_during_hold() {
    let mut l = LadderPlan::new(LadderConfig {
        table_db: vec![-6.0, 0.0],
        per_rung: RungFor::Seconds(1.0),
        full_for: RungFor::Seconds(1.0),
        ..Default::default()
    })
    .unwrap();
    l.tick(0.5);
    l.hold();
    for _ in 0..100 {
        l.tick(1.0);
        l.note_sweep_completed(); // also ignored while held
    }
    assert_close(l.level_db(), -6.0, 0.0, "held rung did not advance");
    assert_close(l.progress().rung_elapsed_s, 0.5, 0.0, "rung clock paused");
    l.resume();
    l.tick(0.5);
    assert_close(l.level_db(), 0.0, 0.0, "advanced after resume");
}

#[test]
fn ladder_manual_step_db_clamps_to_table_range() {
    let mut l = LadderPlan::new(LadderConfig::default()).unwrap();
    assert_close(l.step_db(1.0), -11.0, 1e-12, "manual +1 dB between rungs");
    assert_close(l.step_db(-5.0), -12.0, 0.0, "clamped at table min");
    for _ in 0..20 {
        l.step_db(1.0);
    }
    assert_close(l.level_db(), 0.0, 0.0, "clamped at table max");
}

#[test]
fn f17_ladder_jump_does_not_restart_the_sweep() {
    // The ladder never touches the sweep engine: a rung jump mid-sweep leaves
    // the caller's BidirSweep exactly where it was (frequency, direction,
    // counters), and only the RUNG clock resets.
    let mut sweep = one_second_sweep(SweepTarget::Sweeps(10));
    let mut l = LadderPlan::new(LadderConfig::default()).unwrap();
    sweep.tick(1.3); // mid second pass
    let before = sweep.progress();
    l.tick(0.7); // some rung time accrued
    l.jump_to(3).unwrap();
    assert_close(l.level_db(), -3.0, 0.0, "jumped to rung 3");
    assert_close(l.progress().rung_elapsed_s, 0.0, 0.0, "rung clock reset");
    let after = sweep.progress();
    assert_eq!(before.sweeps_done, after.sweeps_done);
    assert_close(after.freq_hz, before.freq_hz, 0.0, "sweep NOT restarted");
    assert_close(
        after.elapsed_test_s,
        before.elapsed_test_s,
        0.0,
        "sweep clock untouched",
    );
    // The sweep continues from where it was, not from the start.
    sweep.tick(0.1);
    assert!(sweep.progress().elapsed_test_s > before.elapsed_test_s);
}

#[test]
fn ladder_jump_to_validates_and_reopens() {
    let mut l = LadderPlan::new(LadderConfig {
        table_db: vec![-6.0, 0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
        ..Default::default()
    })
    .unwrap();
    assert!(l.jump_to(5).is_err());
    l.note_sweep_completed();
    l.note_sweep_completed();
    assert!(l.done());
    l.jump_to(0).unwrap();
    assert!(!l.done(), "jumping re-opens a completed ladder");
    assert_close(l.level_db(), -6.0, 0.0, "back on rung 0");
}

#[test]
fn ladder_validation_and_config_serde() {
    assert!(LadderPlan::new(LadderConfig {
        table_db: vec![],
        ..Default::default()
    })
    .is_err());
    assert!(LadderPlan::new(LadderConfig {
        per_rung: RungFor::Sweeps(0),
        ..Default::default()
    })
    .is_err());
    assert!(LadderPlan::new(LadderConfig {
        full_for: RungFor::Seconds(0.0),
        ..Default::default()
    })
    .is_err());
    let cfg = LadderConfig::default();
    assert_close(cfg.sysid_level_db, -20.0, 0.0, "sysID default");
    assert_close(cfg.start_db, -12.0, 0.0, "start default");
    assert_eq!(cfg.table_db, vec![-12.0, -9.0, -6.0, -3.0, 0.0]);
    let json = serde_json::to_string(&cfg).unwrap();
    let back: LadderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cfg);
}
