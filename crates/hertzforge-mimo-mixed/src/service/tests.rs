//! The step's drive protections, on the cost probe's step: the `drive_vpk`
//! abort and the clamp at each drive's own peak.

use super::*;
use hertzforge_engine::ProcessStep;

const FS: f64 = 16384.0;
const BLOCK: usize = 1024;

fn probe() -> StepCostProbe {
    let law = hertzforge_mimo_sine::laws::builtin("matrix").expect("matrix");
    StepCostProbe::new(FS, 4, 4, 0, 2, law).expect("probe")
}

/// `blocks` blocks with each input reading the drive of its index a block
/// late; every drive frame played.
fn play(pr: &mut StepCostProbe, blocks: usize) -> Vec<f32> {
    let (n_in, m) = (pr.n_in, pr.step.m);
    let mut input = vec![0.0f32; BLOCK * n_in];
    let mut out = Vec::new();
    let mut all = Vec::new();
    for _ in 0..blocks {
        pr.feed(BLOCK);
        pr.step.process(&input, BLOCK, &mut out);
        for f in 0..BLOCK {
            for ch in 0..n_in {
                input[f * n_in + ch] = out[f * m + ch % m];
            }
        }
        all.extend_from_slice(&out);
    }
    all
}

/// Requirement: a drive frame at or past a drive's abort level latches the
/// `drive_vpk` trip naming that drive, and the drive is at zero within the
/// abort ramp (`ABORT_RAMP_S`) of the trip and stays there.
#[test]
fn mimo_mixed_drive_vpk_trips_and_ramps_to_zero() {
    let mut pr = probe();
    let m = pr.step.m;
    pr.step.abort_v = vec![1e9; m];
    pr.step.abort_v[2] = 0.02;
    let out = play(&mut pr, 32);
    let trip = pr.step.trip.clone().expect("tripped");
    println!("trip: {trip:?}");
    assert!(
        matches!(trip, StepTrip::DriveVpk { drive: 2, .. }),
        "{trip:?}"
    );
    let first = out
        .chunks(m)
        .position(|f| f[2].abs() as f64 >= 0.02)
        .expect("a frame at the abort level");
    let ramp = (ABORT_RAMP_S * FS).ceil() as usize + 1;
    let after = &out[(first + ramp) * m..];
    let worst = after.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    println!("frame {first} at the abort level; largest after the ramp {worst:e}");
    assert_eq!(worst, 0.0);
    assert!(pr.step.stopped());
}

/// Requirement: with the abort out of the way, no drive frame is ever past
/// its drive's own peak (0.01 V on drive 1, unbounded on the others), the
/// clamp is counted, and the other drives are untouched by it. Drive 1's
/// peak is lowered after four blocks, so the drives in force are over it
/// until the ceiling rule next decides (a peak stated from the start is
/// held by the rule and never reaches the clamp).
#[test]
fn mimo_mixed_the_clamp_holds_each_drive_at_its_own_peak() {
    let mut pr = probe();
    let m = pr.step.m;
    pr.step.abort_v = vec![1e9; m];
    pr.step.peak_v = vec![1e9; m];
    let mut out = play(&mut pr, 4);
    pr.step.peak_v[1] = 0.01;
    out.clear();
    out.extend(play(&mut pr, 16));
    let worst: Vec<f32> = (0..m)
        .map(|j| out.chunks(m).fold(0.0f32, |a, f| a.max(f[j].abs())))
        .collect();
    println!(
        "largest frame per drive {worst:?}; clamped {}",
        pr.step.clamped
    );
    assert!(worst[1] as f64 <= 0.01 + 1e-9, "{worst:?}");
    assert!(pr.step.clamped > 0);
    assert!(
        worst[0] as f64 > 0.01 && worst[2] as f64 > 0.01,
        "{worst:?}"
    );
    assert!(pr.step.trip.is_none());
}

/// `frames` frames in one call, each input reading the drive of its index
/// as `play` does; the largest frame per drive.
fn play_frames(pr: &mut StepCostProbe, frames: usize, input: &mut Vec<f32>) -> Vec<f64> {
    let (n_in, m) = (pr.n_in, pr.step.m);
    input.resize(frames * n_in, 0.0);
    let mut out = Vec::new();
    pr.feed(frames);
    pr.step.process(&input[..frames * n_in], frames, &mut out);
    for f in 0..frames {
        for ch in 0..n_in {
            input[f * n_in + ch] = out[f * m + ch % m];
        }
    }
    (0..m)
        .map(|j| out.chunks(m).fold(0.0f64, |a, f| a.max(f[j].abs() as f64)))
        .collect()
}

/// Requirement: the clamp at a drive's peak is a backstop under its abort.
/// A level the ceiling rule did not foresee (set straight into the step,
/// 0.5 dB up, as no service path can) plays a drive past its peak: with the
/// abort at the default (`peak_v` plus 1 dB, `MimoDrive::abort_level_vpk`)
/// the frames are clamped at the peak and the run goes on; with the abort
/// at the peak itself (the old wiring) the same frames end the run.
#[test]
fn mimo_mixed_the_clamp_is_a_backstop_under_the_peak_abort() {
    let run = |abort_over_peak: f64| {
        let mut pr = probe();
        let m = pr.step.m;
        let mut input = Vec::new();
        let mut worst = vec![0.0f64; m];
        for _ in 0..16 {
            let w = play_frames(&mut pr, BLOCK, &mut input);
            for j in 0..m {
                worst[j] = worst[j].max(w[j]);
            }
        }
        // The rule binds: every drive's ceiling at 70 % of its free peak.
        let drive = crate::config::MimoDrive {
            peak_v: 1.0,
            ..crate::config::MimoDrive::new(1, 1.0)
        };
        // The rule settles onto the new ceilings (its first ticks after the
        // change are clamped), then the abort is set and the run checked.
        for j in 0..m {
            pr.step.peak_v[j] = 0.7 * worst[j];
        }
        for _ in 0..16 {
            play_frames(&mut pr, BLOCK, &mut input);
        }
        pr.step.clamped = 0;
        for j in 0..m {
            pr.step.abort_v[j] = if abort_over_peak > 0.0 {
                0.7 * worst[j] * drive.abort_level_vpk()
            } else {
                0.7 * worst[j]
            };
        }
        for _ in 0..16 {
            play_frames(&mut pr, BLOCK, &mut input);
        }
        let clamped_before = pr.step.clamped;
        let tripped_before = pr.step.trip.clone();
        pr.step.level = Ramp::at(10f64.powf(0.5 / 20.0));
        let mut over = 0.0f64;
        for _ in 0..4 {
            let w = play_frames(&mut pr, BLOCK, &mut input);
            for j in 0..m {
                over = over.max(w[j] / pr.step.peak_v[j]);
            }
        }
        println!(
            "abort over peak {abort_over_peak}: clamped {clamped_before} then {}, trip before \
             {tripped_before:?} after {:?}, largest frame {over:.4} of the peak",
            pr.step.clamped, pr.step.trip
        );
        (
            clamped_before,
            tripped_before,
            pr.step.clamped,
            pr.step.trip.clone(),
            over,
        )
    };
    let (c0, t0, c1, t1, over) = run(1.0);
    assert_eq!(
        (c0, t0),
        (0, None),
        "the rule held the budget before the step"
    );
    assert!(c1 > 0, "the unforeseen level reached the clamp");
    assert!(over <= 1.0 + 1e-6, "{over}");
    assert!(t1.is_none(), "clamped, not an abort: {t1:?}");
    let (_, _, _, t_old, _) = run(0.0);
    assert!(
        matches!(t_old, Some(StepTrip::DriveVpk { .. })),
        "with the abort at the peak the same frames abort: {t_old:?}"
    );
}

/// Requirement: a level step asked between two control ticks starts at the
/// next tick, so the tick's look-ahead sees it: with the ceiling rule bound
/// (every drive's ceiling at 70 % of its free peak), a +6 dB step over two
/// ticks asked one frame after a tick plays no frame past any drive's peak.
#[test]
fn mimo_mixed_a_level_step_between_ticks_stays_inside_the_budget() {
    let mut pr = probe();
    let m = pr.step.m;
    let mut input = Vec::new();
    let mut worst = vec![0.0f64; m];
    for _ in 0..16 {
        let w = play_frames(&mut pr, BLOCK, &mut input);
        for j in 0..m {
            worst[j] = worst[j].max(w[j]);
        }
    }
    // The rule settles onto the new ceilings (its first ticks after the
    // change are clamped; the abort is out of the way), then is checked.
    for j in 0..m {
        pr.step.peak_v[j] = 0.7 * worst[j];
    }
    for _ in 0..16 {
        play_frames(&mut pr, BLOCK, &mut input);
    }
    pr.step.clamped = 0;
    for _ in 0..16 {
        play_frames(&mut pr, BLOCK, &mut input);
    }
    assert_eq!(pr.step.clamped, 0, "the rule held the budget");
    // One frame past a tick.
    let tf = pr.step.tick_frames;
    let to = (tf - pr.step.frame % tf + 1) as usize;
    play_frames(&mut pr, to, &mut input);
    assert_eq!(pr.step.frame % tf, 1);
    pr.step.go_level(2.0, 2.0 * tf as f64);
    let mut over = 0.0f64;
    for _ in 0..8 {
        let w = play_frames(&mut pr, BLOCK, &mut input);
        for j in 0..m {
            over = over.max(w[j] / pr.step.peak_v[j]);
        }
    }
    println!(
        "level {:.3}; clamped {}; largest frame {over:.4} of the peak; trip {:?}",
        pr.step.level.value(),
        pr.step.clamped,
        pr.step.trip
    );
    assert!((pr.step.level.value() - 2.0).abs() < 1e-9);
    assert_eq!(pr.step.clamped, 0, "a frame past the peak");
    assert!(pr.step.trip.is_none());
}

/// Requirement: a tone switched on while the ceiling rule binds plays no
/// frame past any drive's peak. Tone 1 is switched off and forgotten (never
/// engaged, as a tone whose part has not started), the ceilings are set at
/// 70 % of the free peaks with tone 0 alone, and tone 1 is switched on.
#[test]
fn mimo_mixed_a_tone_switched_on_under_a_bound_rule_stays_inside_the_budget() {
    let mut pr = probe();
    let m = pr.step.m;
    let mut input = Vec::new();
    let mut worst = vec![0.0f64; m];
    for _ in 0..16 {
        let w = play_frames(&mut pr, BLOCK, &mut input);
        for j in 0..m {
            worst[j] = worst[j].max(w[j]);
        }
    }
    let part = pr.step.tones[1].part;
    pr.step.clock.set_enabled(part, false);
    for _ in 0..4 {
        play_frames(&mut pr, BLOCK, &mut input);
    }
    pr.step.tones[1].engaged = false;
    for j in 0..m {
        pr.step.peak_v[j] = 0.7 * worst[j];
    }
    for _ in 0..16 {
        play_frames(&mut pr, BLOCK, &mut input);
    }
    pr.step.clamped = 0;
    for _ in 0..16 {
        play_frames(&mut pr, BLOCK, &mut input);
    }
    assert_eq!(pr.step.clamped, 0, "the rule held the budget");
    pr.step.clock.set_enabled(part, true);
    let mut over = 0.0f64;
    for _ in 0..8 {
        let w = play_frames(&mut pr, BLOCK, &mut input);
        for j in 0..m {
            over = over.max(w[j] / pr.step.peak_v[j]);
        }
    }
    println!(
        "tone 1 part level {:.3}; scales {:?}; clamped {}; largest frame {over:.4} of the peak",
        pr.step.bank.part_level(1),
        pr.step.tones.iter().map(|t| t.scale).collect::<Vec<_>>(),
        pr.step.clamped
    );
    assert!(pr.step.bank.part_level(1) > 0.999, "switched on");
    assert_eq!(pr.step.clamped, 0, "a frame past the peak");
    assert!(pr.step.trip.is_none());
}
