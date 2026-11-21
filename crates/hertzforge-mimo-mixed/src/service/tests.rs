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
/// clamp is counted, and the other drives are untouched by it.
#[test]
fn mimo_mixed_the_clamp_holds_each_drive_at_its_own_peak() {
    let mut pr = probe();
    let m = pr.step.m;
    pr.step.abort_v = vec![1e9; m];
    pr.step.peak_v = vec![1e9; m];
    pr.step.peak_v[1] = 0.01;
    let out = play(&mut pr, 16);
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
