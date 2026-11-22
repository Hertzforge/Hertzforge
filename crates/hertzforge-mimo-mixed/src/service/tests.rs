//! The step's drive protections, on the cost probe's step: the `drive_vpk`
//! abort, the clamp at each drive's own peak, the ceiling rule's budget
//! across ramps and switch-ons, and the tone guards (the cap at zero drive,
//! the bounded carry, the watch behind the ceiling).

use super::step::{LIMITED_DIVERGENCE_FACTOR, SHOWN_MAX_DELAYS, UNREADABLE_ABORT_DELAYS};
use super::*;
use hertzforge_engine::ProcessStep;
use hertzforge_mimo_sine::law::LawValues;
use hertzforge_mimo_sine::tick::{LawFault, TickInputs, TickOutputs};

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

/// A tone law that asks twice what it asked last (twice the drive in force
/// where there is one), every tick: a law running away.
struct Greedy {
    last: Vec<C64>,
}

impl hertzforge_mimo_sine::law::MimoSineLaw for Greedy {
    fn name(&self) -> &str {
        "greedy"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.tick(x, o, v)
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        _v: &mut LawValues,
    ) -> Result<(), LawFault> {
        let zero = x.drive_in_force.iter().all(|d| d.norm() == 0.0);
        self.last
            .resize(x.drive_in_force.len(), C64::new(1e-3, 0.0));
        for ((d, f), l) in o.drive.iter_mut().zip(x.drive_in_force).zip(&mut self.last) {
            *d = if zero { *l * 2.0 } else { *f * 2.0 };
            *l = *d;
        }
        Ok(())
    }
}

struct GreedyFactory(Arc<dyn MimoSineLawFactory>);

impl MimoSineLawFactory for GreedyFactory {
    fn build(
        &self,
        _setup: &hertzforge_mimo_sine::law::LawSetup,
    ) -> Result<Box<dyn hertzforge_mimo_sine::law::MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(Greedy { last: Vec::new() }))
    }
    fn describe(&self) -> hertzforge_mimo_sine::law::LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> hertzforge_mimo_sine::law::LawManifest {
        self.0.manifest()
    }
}

/// Requirement: a tone the ceiling rule holds at zero keeps its law's ask
/// under the rise cap, against its own last ask: with every drive's peak
/// at a microvolt (no room, every tone scaled to zero) a law that doubles
/// its ask every tick grows its ask by at most the cap's rise per tick
/// (`max_step_db`, or `max_rise_db_per_s` over a tick), where uncapped it
/// would not be capped at all (measured: 11.0 dB in one tick with the cap
/// taken against the zero drive in force).
#[test]
fn mimo_mixed_a_tone_held_at_zero_by_the_ceiling_rises_no_faster_than_its_cap() {
    let law = Arc::new(GreedyFactory(
        hertzforge_mimo_sine::laws::builtin("matrix").expect("matrix"),
    ));
    let mut pr = StepCostProbe::new(FS, 4, 4, 0, 2, law).expect("probe");
    let m = pr.step.m;
    pr.step.abort_v = vec![1e9; m];
    pr.step.peak_v = vec![1e9; m];
    let mut input = Vec::new();
    let tf = pr.step.tick_frames as usize;
    pr.step.rand_ramp = Ramp::at(1.0);
    for _ in 0..2000 {
        if pr.step.tones.iter().all(|t| t.engaged) {
            break;
        }
        play_frames(&mut pr, tf, &mut input);
    }
    assert!(pr.step.tones[0].engaged, "the tone engaged");
    // The ring kept four ticks ahead, so the rule sees the random part's
    // peak (0.01 V) over every drive's peak: no room for any tone.
    pr.feed(4 * tf);
    pr.step.peak_v = vec![1e-6; m];
    let norm = |a: &[C64]| a.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt();
    let g = pr.step.guards.clone();
    let up_db = g.max_step_db.min(g.max_rise_db_per_s * pr.step.tick_s);
    let mut prev = norm(&pr.step.tones[0].ask);
    let mut worst_db = f64::NEG_INFINITY;
    let mut zeroed = 0;
    for _ in 0..40 {
        play_frames(&mut pr, tf, &mut input);
        let a = norm(&pr.step.tones[0].ask);
        if pr.step.tones[0].scale == 0.0 && prev > 0.0 {
            zeroed += 1;
            worst_db = worst_db.max(20.0 * (a / prev).log10());
        }
        prev = a;
    }
    println!(
        "{zeroed} ticks at zero; largest rise of the ask {worst_db:.3} dB a tick (cap {up_db:.3} dB)"
    );
    assert!(zeroed >= 30, "the rule held the tone at zero");
    assert!(worst_db <= up_db + 1e-9, "{worst_db} dB");
}

/// The probe with `law`, the abort and peaks out of the way and the random
/// part in, played tick by tick until every tone has engaged.
fn engaged_probe(law: Arc<dyn MimoSineLawFactory>, input: &mut Vec<f32>) -> StepCostProbe {
    let mut pr = StepCostProbe::new(FS, 4, 4, 0, 2, law).expect("probe");
    let m = pr.step.m;
    pr.step.abort_v = vec![1e9; m];
    pr.step.peak_v = vec![1e9; m];
    pr.step.rand_ramp = Ramp::at(1.0);
    let tf = pr.step.tick_frames as usize;
    for _ in 0..2000 {
        if pr.step.tones.iter().all(|t| t.engaged) {
            break;
        }
        play_frames(&mut pr, tf, input);
    }
    assert!(pr.step.tones.iter().all(|t| t.engaged), "the tones engaged");
    pr
}

/// Requirement: the "told on target" carry is bounded. A tone whose reading
/// goes unusable with its ramps done (the floor under it raised past any
/// reading) is handed to its law, told on target, for `SHOWN_MAX_DELAYS`
/// tracker delays, then holds its last ask with its law no longer called,
/// and ends the run `tone_unreadable` at `UNREADABLE_ABORT_DELAYS`.
#[test]
fn mimo_mixed_a_tone_unread_is_carried_for_a_bounded_time_then_held_then_ends() {
    let mut input = Vec::new();
    let law = hertzforge_mimo_sine::laws::builtin("matrix").expect("matrix");
    let mut pr = engaged_probe(law, &mut input);
    let tf = pr.step.tick_frames as usize;
    let delay = hertzforge_dsp::tone::TRACKER_DELAY_FACTOR / pr.step.bank.bn_hz();
    pr.step.floor_diag.fill(1e30);
    let t0 = pr.step.frame as f64 / FS;
    let mut last_call_s = None;
    let mut calls = pr.step.tones[0].calls;
    let ask0 = pr.step.tones[0].ask.clone();
    let mut held_ask = None;
    let limit_s = (UNREADABLE_ABORT_DELAYS + 4.0) * delay;
    while pr.step.trip.is_none() && pr.step.frame as f64 / FS - t0 < limit_s {
        play_frames(&mut pr, tf, &mut input);
        let t = pr.step.frame as f64 / FS - t0;
        let tc = &pr.step.tones[0];
        assert!(!tc.usable, "unreadable");
        if tc.calls != calls {
            calls = tc.calls;
            last_call_s = Some(t);
        } else if last_call_s.is_some() && held_ask.is_none() {
            held_ask = Some(tc.ask.clone());
        }
        if let Some(h) = held_ask.as_ref() {
            assert_eq!(h, &tc.ask, "held, its ask stands");
        }
    }
    let end_s = pr.step.frame as f64 / FS - t0;
    println!(
        "tracker delay {delay:.3} s: law last called {last_call_s:?} s in, trip {:?} at {end_s:.2} s",
        pr.step.trip
    );
    let last = last_call_s.expect("carried at first");
    assert!(
        last <= SHOWN_MAX_DELAYS * delay + 2.0 * pr.step.tick_s,
        "{last} s"
    );
    assert!(
        last >= SHOWN_MAX_DELAYS * delay - 2.0 * pr.step.tick_s,
        "{last} s"
    );
    assert!(held_ask.is_some_and(|h| h.len() == ask0.len()));
    assert!(
        matches!(pr.step.trip, Some(StepTrip::ToneUnreadable { .. })),
        "{:?}",
        pr.step.trip
    );
    assert!(
        (end_s - UNREADABLE_ABORT_DELAYS * delay).abs() <= 2.0 * pr.step.tick_s,
        "{end_s}"
    );
}

/// Requirement: a tone law that runs away while the shared ceiling holds
/// its tone (every drive's peak at a microvolt, the tone at zero, its caps
/// binding every tick) is counted by the limited divergence watch and ends
/// the run `divergence` at `LIMITED_DIVERGENCE_FACTOR` times the
/// divergence time.
#[test]
fn mimo_mixed_a_law_running_away_behind_the_ceiling_ends_in_divergence() {
    let mut input = Vec::new();
    let law = Arc::new(GreedyFactory(
        hertzforge_mimo_sine::laws::builtin("matrix").expect("matrix"),
    ));
    let mut pr = engaged_probe(law, &mut input);
    let tf = pr.step.tick_frames as usize;
    let m = pr.step.m;
    pr.feed(4 * tf);
    pr.step.peak_v = vec![1e-6; m];
    let want_s = LIMITED_DIVERGENCE_FACTOR * pr.step.guards.divergence_s;
    let t0 = pr.step.frame as f64 / FS;
    while pr.step.trip.is_none() && pr.step.frame as f64 / FS - t0 < 2.0 * want_s {
        play_frames(&mut pr, tf, &mut input);
    }
    let t = pr.step.frame as f64 / FS - t0;
    println!("trip {:?} at {t:.2} s (watch {want_s} s)", pr.step.trip);
    assert!(
        matches!(pr.step.trip, Some(StepTrip::ToneDivergence { .. })),
        "{:?}",
        pr.step.trip
    );
    assert!(t >= want_s - 2.0 * pr.step.tick_s, "{t} s");
    assert!(t <= want_s + 0.5, "{t} s");
}

/// Requirement: a drive frame that is not a number (here one random frame
/// on drive 1) is never played: the frame plays 0, the step trips naming
/// the drive, and the drive is at zero within the abort ramp. Both of the
/// old comparisons were false for a NaN, so it was played and never seen.
#[test]
fn mimo_mixed_a_drive_frame_not_finite_trips_and_plays_zero() {
    let mut pr = probe();
    let m = pr.step.m;
    let _ = play(&mut pr, 4);
    assert!(pr.step.trip.is_none(), "{:?}", pr.step.trip);
    pr.step.ring.clear();
    let mut bad = vec![0.01; BLOCK * m];
    bad[100 * m + 1] = f64::NAN;
    pr.step.ring.push(&bad);
    let input = vec![0.0f32; BLOCK * pr.n_in];
    let mut out = Vec::new();
    pr.step.process(&input, BLOCK, &mut out);
    let mut all = out.clone();
    all.extend(play(&mut pr, 8));
    let not_finite = all.iter().filter(|v| !v.is_finite()).count();
    let trip = pr.step.trip.clone();
    println!("trip: {trip:?}; frames not finite played: {not_finite}");
    assert_eq!(not_finite, 0, "a frame not finite was played");
    assert!(
        matches!(trip, Some(StepTrip::DriveNotFinite { drive: 1 })),
        "{trip:?}"
    );
    let ramp = (ABORT_RAMP_S * FS).ceil() as usize + 1;
    let worst = all[(100 + ramp) * m..]
        .iter()
        .fold(0.0f32, |a, v| a.max(v.abs()));
    println!("largest frame after the abort ramp {worst:e}");
    assert_eq!(worst, 0.0);
    assert!(pr.step.stopped());
}

/// Requirement: a frame the random part's ring has none for plays no random
/// part and is recorded at gain 0 (so the floor's estimate does not read it
/// as a steady segment); a dry spell under a control tick of frames goes
/// on, and the run trips `random_underrun` once the dry frames reach a
/// tick in all.
#[test]
fn mimo_mixed_a_dry_ring_is_recorded_at_gain_zero_then_trips() {
    let mut pr = probe();
    let m = pr.step.m;
    let tick = pr.step.tick_frames as usize;
    let _ = play(&mut pr, 2);
    assert!(pr.step.trip.is_none(), "{:?}", pr.step.trip);
    let input = vec![0.0f32; BLOCK * pr.n_in];
    let mut out = Vec::new();
    // A spell of tick / 2 dry frames.
    let short = tick / 2;
    pr.step.ring.clear();
    pr.step.records.clear();
    pr.step.ring.push(&vec![0.01; (BLOCK - short) * m]);
    pr.step.process(&input, BLOCK, &mut out);
    let gains = pr.step.records.gain.clone();
    let dry_gain = gains[BLOCK - short..]
        .iter()
        .fold(0.0f64, |a, g| a.max(g.abs()));
    println!(
        "{short} dry frames: underruns {}, largest gain recorded on them {dry_gain}, trip {:?}",
        pr.step.underruns, pr.step.trip
    );
    assert_eq!(pr.step.underruns as usize, short);
    assert!(gains[..BLOCK - short].iter().all(|g| *g > 0.0));
    assert_eq!(dry_gain, 0.0, "a dry frame recorded at the random gain");
    assert!(pr.step.trip.is_none(), "{:?}", pr.step.trip);
    // The rest of a tick dry.
    pr.step.ring.clear();
    pr.step.records.clear();
    pr.step.process(&input, BLOCK, &mut out);
    let trip = pr.step.trip.clone();
    println!(
        "then a dry block: underruns {}, trip {trip:?}",
        pr.step.underruns
    );
    match trip {
        Some(StepTrip::RandomUnderrun { frames }) => assert_eq!(frames as usize, tick),
        other => panic!("no underrun trip: {other:?}"),
    }
}

/// Requirement: the kinematic monitor judges every frame of a block, a
/// block longer than its scratch too (in pieces of the scratch). A block of
/// 5000 frames, past the probe's 4096-frame scratch, with channel 1 at 1 g
/// against its 0.5 g abort from frame 4500 on (past the scratch) trips
/// `Kinematic` naming channel 1; such a block used to be skipped whole, so
/// nothing tripped.
#[test]
fn mimo_mixed_the_kinematic_monitor_judges_a_block_past_its_scratch() {
    let mut pr = probe();
    let n_in = pr.n_in;
    let mut t = hertzforge_channel_table::ChannelTable::default_with_channels(n_in);
    t.channels[0].abort.accel_g = Some(0.5);
    pr.step.kin = Some(hertzforge_random::service::engine::KinematicMonitor::new(
        &t, FS, 5.0, 1, 5.0,
    ));
    // The probe's step is built for blocks of up to 4096 frames.
    let (frames, scratch) = (5000, 4096);
    assert!(frames > scratch);
    let mut input = vec![0.0f32; frames * n_in];
    for f in 4500..frames {
        input[f * n_in] = 1.0;
    }
    let mut out = Vec::new();
    pr.feed(frames);
    pr.step.process(&input, frames, &mut out);
    let trip = pr.step.trip.clone();
    println!("{frames} frames, scratch {scratch} frames: trip {trip:?}");
    assert!(
        matches!(
            trip,
            Some(StepTrip::Kinematic {
                channel_index: 1,
                ..
            })
        ),
        "{trip:?}"
    );
}

/// Requirement: a band law's drive the service cannot play is named, not
/// dropped: one of the wrong shape, one not finite and one not positive
/// semi-definite each give the reason (the band loop then ends the run
/// `law_fault` naming the band); a usable drive gives none.
#[test]
fn mimo_mixed_a_band_drive_that_cannot_play_is_named() {
    use super::floor::band_output_fault;
    let (offs, m) = (5, 2);
    let mut good = HermitianStack::zeros(offs, m);
    for o in 0..offs {
        good.set(o, 0, 0, C64::new(1e-3, 0.0));
        good.set(o, 1, 1, C64::new(1e-3, 0.0));
    }
    assert_eq!(band_output_fault(&good, offs, m), None);
    let shape = band_output_fault(&HermitianStack::zeros(offs - 1, m), offs, m);
    let mut nan = good.clone();
    nan.set(2, 0, 0, C64::new(f64::NAN, 0.0));
    let nan = band_output_fault(&nan, offs, m);
    let mut neg = good.clone();
    neg.set(3, 1, 1, C64::new(-1e-3, 0.0));
    let neg = band_output_fault(&neg, offs, m);
    println!("shape: {shape:?}\nnot finite: {nan:?}\nnot semi-definite: {neg:?}");
    assert!(shape.is_some_and(|w| w.contains("4 offsets")));
    assert!(nan.is_some_and(|w| w.contains("non-finite")));
    assert!(neg.is_some_and(|w| w.contains("semi-definite")));
}

/// Requirement: a tone is judged at the line it is at, not where it
/// started: once the line differs from the one its judging was worked out
/// at (here the record of that line is moved off, as a tone that moved
/// leaves it), the judging is worked out again from the reference and the
/// floor at the tone's line. The floor raised from 1e-9 to 1e-5 under the
/// tone triples the reading's predicted scatter.
#[test]
fn mimo_mixed_a_tone_is_judged_at_the_line_it_is_at() {
    let mut pr = probe();
    let _ = play(&mut pr, 64);
    let line = pr.step.tones[0].judged_line;
    let before = pr.step.tones[0].judging[0];
    assert!(pr.step.trip.is_none(), "{:?}", pr.step.trip);
    pr.step.floor_diag.fill(1e-5);
    pr.step.tones[0].judged_line = usize::MAX;
    let _ = play(&mut pr, 4);
    let tc = &pr.step.tones[0];
    let want = tone_judging(
        tc.reference[line].norm(),
        1e-5,
        (3.0, 3.0),
        (60.0, 60.0),
        pr.step.bank.bn_hz(),
    );
    println!(
        "line {line}: judged at {} now; before {before:?}\nnow {:?}",
        tc.judged_line, tc.judging[0]
    );
    assert_eq!(tc.judged_line, line);
    assert_eq!(tc.judging[0], want);
    assert!(
        want.sigma_db > 2.0 * before.sigma_db,
        "{want:?} against {before:?}"
    );
}

#[test]
fn numbers_in_a_log_line_are_the_same_on_every_platform() {
    // two platforms' last bits of one value
    let here = super::listed(&[0.9261958540267572, 0.930899086473891], 3);
    let there = super::listed(&[0.9261958540267573, 0.9308990864738907], 3);
    assert_eq!(here, "[0.926, 0.931]");
    assert_eq!(here, there);
}
