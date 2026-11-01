//! Service state-machine tests over a SCRIPTED plant player (deterministic,
//! misbehavior-injectable). The full-stack acceptance run against the real
//! hertzforge-hal-sim plant lives in `tests/sim_e2e.rs`.

use super::config::TwrConfig;
use super::{Advanced, TwrService, TwrState};
use crate::limits::FeasibilityCeilings;
use crate::player::{ReferencePlayer, TwrPath};
use crate::reference::ReferenceWaveform;
use crate::schedule::{GateFailPolicy, RunSchedule, TwrRunMode};
use crate::TwrError;

const FS: f64 = 4096.0;

/// A linear gain + integer-delay plant with injectable misbehavior. Each play
/// is independent (fresh delay memory), like a flushed backend.
struct ScriptedPlayer {
    gain: f64,
    delay: usize,
    plays: usize,
    /// After this many plays the gain is multiplied by `late_gain_factor`.
    misbehave_after: Option<usize>,
    late_gain_factor: f64,
    /// Additive PER-PLAY-VARYING broadband noise amplitude. It must vary per
    /// play: a repeatable deterministic disturbance is CANCELLABLE by the
    /// correction law (the law would simply bake its negation into the drive -
    /// correct behavior, not a gate failure).
    disturbance: f64,
    armed: bool,
    stopped: bool,
    /// The operator cancel flag, shared with whoever holds the other handle.
    /// A raised flag makes the NEXT `play()` answer `Cancelled` at frame 0,
    /// which is what a real block-boundary cancel looks like to the service.
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// `emergency_stop()` was taken rather than the ramped `stop()`.
    emergency_stopped: bool,
    /// Injectable alignment provenance. `None` = this player makes no claim
    /// (the trait default); `Some(DerivedNoPeak)` scripts the no-peak xcorr.
    lag_provenance: Option<crate::player::LagSource>,
}

impl ScriptedPlayer {
    fn well_behaved(gain: f64, delay: usize) -> Self {
        ScriptedPlayer {
            gain,
            delay,
            plays: 0,
            misbehave_after: None,
            late_gain_factor: 1.0,
            disturbance: 0.0,
            armed: false,
            stopped: false,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            emergency_stopped: false,
            lag_provenance: None,
        }
    }
}

impl ReferencePlayer for ScriptedPlayer {
    fn path(&self) -> TwrPath {
        TwrPath::HostBackend
    }
    fn fs_hz(&self) -> f64 {
        FS
    }
    fn arm(&mut self) -> Result<(), TwrError> {
        self.armed = true;
        Ok(())
    }
    fn play(&mut self, drive: &[f64]) -> Result<Option<Vec<f64>>, TwrError> {
        assert!(self.armed, "play before arm");
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(TwrError::Cancelled {
                frames_done: 0,
                frames_expected: drive.len(),
            });
        }
        self.plays += 1;
        let g = match self.misbehave_after {
            Some(n) if self.plays > n => self.gain * self.late_gain_factor,
            _ => self.gain,
        };
        // Per-play-seeded LCG noise (uniform in [-1, 1]) - varies every play so
        // the correction cannot learn it.
        let mut seed = (self.plays as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x0BAD_5EED);
        let mut next_noise = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 30) as f64) - 1.0
        };
        let out = (0..drive.len())
            .map(|i| {
                let d = if i >= self.delay {
                    drive[i - self.delay]
                } else {
                    0.0
                };
                g * d + self.disturbance * next_noise()
            })
            .collect();
        Ok(Some(out))
    }
    fn lag_provenance(&self) -> Option<crate::player::LagSource> {
        self.lag_provenance
    }
    fn request_cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn stop(&mut self) -> Result<(), TwrError> {
        self.stopped = true;
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), TwrError> {
        self.emergency_stopped = true;
        self.stopped = true;
        Ok(())
    }
}

/// A short band-limited burst reference: hann-windowed 200 Hz tone.
fn burst_reference() -> ReferenceWaveform {
    let n = 400usize;
    let samples: Vec<f64> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            w * (std::f64::consts::TAU * 200.0 * i as f64 / FS).sin()
        })
        .collect();
    ReferenceWaveform::from_samples("burst 200 Hz", FS, samples).unwrap()
}

fn config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(burst_reference());
    c.prep.highpass_hz = None; // the burst has no drift; keep the fixture exact
    c.prep.taper_frac = None;
    c.prep.detrend = None;
    c.pad_pre = 128;
    c.pad_post = 256;
    c.sysid.nfft = 512;
    c.sysid.duration_s = 0.5; // 2048 samples = 4 blocks
    c.sysid.f_lo_hz = 30.0;
    c.sysid.f_hi_hz = 1500.0;
    c.law.f_lo_hz = 30.0;
    c.law.f_hi_hz = 1500.0;
    c.run = RunSchedule::LoopN(2);
    c
}

#[test]
fn auto_run_walks_the_ladder_and_completes() {
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), config()).unwrap();
    assert_eq!(svc.state(), TwrState::Configured);
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Complete, "events: {:?}", svc.events());
    let st = svc.status();
    assert_eq!(st.plays_completed, 2);
    assert!(st.abort_reason.is_none());
    // Every ladder level gated at or under the gate.
    let gate = svc.config().schedule.gate_rms_error_pct;
    for level in [-12.0, -9.0, -6.0, -3.0, 0.0] {
        let last = svc
            .history()
            .records
            .iter()
            .rfind(|r| (r.level_db - level).abs() < 1e-9 && r.iteration > 0)
            .unwrap_or_else(|| panic!("no records at {level} dB"));
        assert!(
            last.metrics.rms_error_pct <= gate,
            "{level} dB ended at {:.2}%",
            last.metrics.rms_error_pct
        );
    }
    // A linear plant with a correct FRF converges essentially at once.
    assert!(svc.status().total_iterations <= 10);
}

#[test]
fn envelope_abort_fires_on_a_plant_gain_step() {
    let mut player = ScriptedPlayer::well_behaved(2.0, 5);
    player.misbehave_after = Some(2); // sysid play + first iteration are honest
    player.late_gain_factor = 3.0; // then the plant gain triples
    let mut svc = TwrService::new(player, config()).unwrap();
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Aborted);
    let reason = svc.status().abort_reason.unwrap();
    assert!(
        reason.contains("envelope") || reason.contains("diverging"),
        "unexpected abort reason: {reason}"
    );
}

#[test]
fn divergence_guard_aborts_on_a_sign_flipped_plant() {
    let mut player = ScriptedPlayer::well_behaved(2.0, 5);
    player.misbehave_after = Some(1); // honest during sysid only
    player.late_gain_factor = -0.8; // sign flip: corrections push the wrong way
    let mut cfg = config();
    // Keep the envelope out of the way so the divergence guard is what trips.
    cfg.limits.envelope_abort_frac = 50.0;
    cfg.limits.envelope_alarm_frac = 25.0;
    cfg.schedule.max_iterations_per_level = 20;
    let mut svc = TwrService::new(player, cfg).unwrap();
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Aborted);
    let reason = svc.status().abort_reason.unwrap();
    assert!(reason.contains("diverging"), "reason: {reason}");
}

#[test]
fn gate_failure_aborts_or_holds_per_policy() {
    // A per-play-varying noise floor the correction cannot cancel keeps the
    // error above an unreachably tight gate.
    let mk_player = || {
        let mut p = ScriptedPlayer::well_behaved(2.0, 5);
        p.disturbance = 0.05;
        p
    };
    let mk_cfg = |policy: GateFailPolicy| {
        let mut c = config();
        c.schedule.gate_rms_error_pct = 0.001;
        c.schedule.max_iterations_per_level = 3;
        c.schedule.on_gate_fail = policy;
        c.sysid.min_mean_coherence = 0.2; // the disturbance costs coherence
        c.limits.envelope_abort_frac = 50.0;
        c.limits.envelope_alarm_frac = 25.0;
        c.limits.divergence_n = 50; // out of the way
        c
    };

    let mut svc = TwrService::new(mk_player(), mk_cfg(GateFailPolicy::Abort)).unwrap();
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Aborted);
    assert!(svc
        .status()
        .abort_reason
        .unwrap()
        .contains("failed to gate"));

    let mut svc = TwrService::new(mk_player(), mk_cfg(GateFailPolicy::Hold)).unwrap();
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Holding);
    // The operator can still act from Holding: jump resets and resumes.
    svc.jump_to_level(0).unwrap();
    assert_eq!(svc.state(), TwrState::Iterating);
}

#[test]
fn feasibility_blocks_and_override_unblocks() {
    let mut cfg = config();
    cfg.limits.feasibility = FeasibilityCeilings {
        peak_accel: Some(1e-6), // absurdly tight: always fails
        ..FeasibilityCeilings::default()
    };
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg).unwrap();
    assert_eq!(svc.state(), TwrState::FeasibilityBlocked);
    assert!(svc.feasibility().blocked);
    assert!(matches!(svc.advance(), Err(TwrError::State(_))));
    assert!(svc.arm_override("").is_err()); // empty ack rejected
    svc.arm_override("operator accepts stroke risk, fixture is bolted")
        .unwrap();
    assert_eq!(svc.state(), TwrState::Configured);
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Complete);
}

#[test]
fn manual_mode_iterates_steps_and_plays_on_command() {
    let mut cfg = config();
    cfg.schedule.mode = TwrRunMode::Manual;
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg).unwrap();
    // Sysid.
    assert!(matches!(svc.advance().unwrap(), Advanced::SysId { .. }));
    assert_eq!(svc.state(), TwrState::Iterating);
    assert_eq!(svc.status().level_db, -12.0);
    // Playback before convergence is refused.
    assert!(svc.begin_playback().is_err());
    // One iteration converges the linear plant at this level.
    svc.advance().unwrap();
    assert!(svc.status().converged_at_level);
    // MANUAL does not auto-advance.
    assert_eq!(svc.status().level_db, -12.0);
    // Trim up two dB, re-converge, then play.
    svc.step_level_up().unwrap();
    svc.step_level_up().unwrap();
    assert_eq!(svc.status().level_db, -10.0);
    svc.advance().unwrap();
    assert!(svc.status().converged_at_level);
    svc.begin_playback().unwrap();
    let end = svc.run_to_completion(50).unwrap();
    assert_eq!(end, TwrState::Complete);
    assert_eq!(svc.status().plays_completed, 2);
}

#[test]
fn stop_is_graceful_and_level_controls_require_a_live_run() {
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), config()).unwrap();
    assert!(svc.step_level_up().is_err()); // not live yet
    svc.advance().unwrap(); // sysid
    svc.advance().unwrap(); // one iteration
    svc.stop().unwrap();
    assert_eq!(svc.state(), TwrState::Stopped);
    assert!(matches!(svc.advance().unwrap(), Advanced::Terminal(_)));
}

/// The operator presses Stop 12 s into a 30 s play. The run must end STOPPED,
/// not ABORTED.
///
/// RED-MUTATION: delete the `TwrError::Cancelled` arm from
/// `TwrService::play_through` and this test reds on the state - the cancel
/// error propagates as a bare `Err`, `self.state` stays `Iterating`,
/// `player_armed` stays true and nothing stops the player. That is exactly the
/// shipped defect: the panel announced `aborted` with a stream-error string,
/// and the next Start refused because the module looked terminal.
#[test]
fn stop_midplay_ends_stopped_not_aborted() {
    let player = ScriptedPlayer::well_behaved(2.0, 5);
    let cancel = std::sync::Arc::clone(&player.cancel);
    let mut svc = TwrService::new(player, config()).unwrap();
    svc.advance().unwrap(); // sysid
    svc.advance().unwrap(); // one iteration
    assert_eq!(svc.state(), TwrState::Iterating);

    // The flag goes up from "another thread" while a play is in flight.
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let err = svc.advance().unwrap_err();
    assert!(
        matches!(err, TwrError::Cancelled { .. }),
        "the unit must unwind on the cancel, not swallow it: {err:?}"
    );

    assert_eq!(svc.state(), TwrState::Stopped, "stop is not an abort");
    assert!(
        svc.status().abort_reason.is_none(),
        "a stop must not write an abort reason: {:?}",
        svc.status().abort_reason
    );
    assert!(svc.player().stopped, "the player was left running");
    assert!(
        !svc.player().emergency_stopped,
        "a graceful stop must not take the emergency path"
    );
    assert!(svc
        .report()
        .events
        .iter()
        .any(|e| e.contains("operator stop: play cancelled")));
}

/// The other half: the service raises the cancel flag BEFORE it does anything
/// else, so a play already in flight unwinds instead of running to the end of
/// the record. Abort additionally takes the immediate teardown.
#[test]
fn stop_and_abort_raise_the_cancel_flag_first() {
    let player = ScriptedPlayer::well_behaved(2.0, 5);
    let cancel = std::sync::Arc::clone(&player.cancel);
    let mut svc = TwrService::new(player, config()).unwrap();
    svc.advance().unwrap(); // sysid
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    svc.stop().unwrap();
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "stop() must raise the flag; a stop that only sets state waits out the record"
    );
    assert!(!svc.player().emergency_stopped, "stop is the ramped path");

    let player = ScriptedPlayer::well_behaved(2.0, 5);
    let cancel = std::sync::Arc::clone(&player.cancel);
    let mut svc = TwrService::new(player, config()).unwrap();
    svc.advance().unwrap();
    svc.abort("operator abort from the panel");
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(svc.state(), TwrState::Aborted);
    assert!(
        svc.player().emergency_stopped,
        "abort takes the 25 ms taper, not stop's 250 ms one"
    );
}

/// A failed identification is a BARRIER, not a retry prompt.
///
/// The count is the assertion: two `advance()` calls after a failed sysID must
/// drive the shaker ONCE. Before `CoherenceHold` existed the failure left the
/// run `Configured` - the state that means "ready to run sysID" - so every
/// subsequent unit re-ran the excitation, and the panel showed no barrier.
///
/// RED-MUTATION: delete `self.state = TwrState::CoherenceHold;` from
/// `do_sysid`'s error arm and the play count goes 1 -> 2.
#[test]
fn a_failed_sysid_holds_and_the_second_advance_does_not_re_drive() {
    let mut player = ScriptedPlayer::well_behaved(2.0, 5);
    player.disturbance = 10.0; // response is mostly uncorrelated noise
    let mut svc = TwrService::new(player, config()).unwrap();

    let err = svc.advance().unwrap_err();
    assert!(matches!(err, TwrError::SysId(_)), "{err:?}");
    assert_eq!(svc.state(), TwrState::CoherenceHold);
    assert_eq!(svc.player().plays, 1);

    let again = svc.advance().unwrap_err();
    assert!(
        matches!(&again, TwrError::State(m) if m.contains("coherence hold")),
        "the refusal must name the hold: {again:?}"
    );
    assert_eq!(
        svc.player().plays,
        1,
        "the second advance re-drove the shaker from a coherence hold"
    );
    assert_eq!(svc.state(), TwrState::CoherenceHold);
}

/// A no-peak lag measurement is the SAME barrier as a failed coherence gate,
/// refused BEFORE the estimate is built. A constant misalignment is a linear
/// phase - Welch coherence cannot see it - so without this gate the run proceeds
/// on the derived fallback and the coherence gate downstream blesses a
/// wrong-phase H (the 2026-07-30 collapse shape, green-lit).
///
/// RED-MUTATION (run 2026-08-09): delete the `lag_provenance()` check from
/// `do_sysid` - the run proceeds past sysid on a perfectly coherent pair and
/// this test fails on the state assert. Restored.
#[test]
fn a_no_peak_lag_holds_before_the_estimate() {
    let mut player = ScriptedPlayer::well_behaved(2.0, 5);
    // The pair itself is perfectly coherent - the barrier must fire on the
    // PROVENANCE, not on estimate quality.
    player.lag_provenance = Some(crate::player::LagSource::DerivedNoPeak);
    let mut svc = TwrService::new(player, config()).unwrap();

    let err = svc.advance().unwrap_err();
    assert!(
        matches!(&err, TwrError::Play(m) if m.contains("no peak")),
        "the refusal must name the no-peak: {err:?}"
    );
    assert_eq!(svc.state(), TwrState::CoherenceHold);
    assert_eq!(svc.player().plays, 1);

    let again = svc.advance().unwrap_err();
    assert!(matches!(&again, TwrError::State(m) if m.contains("coherence hold")));
    assert_eq!(
        svc.player().plays,
        1,
        "a no-peak hold must not re-drive on the next advance"
    );
}

/// The hold's release is RESET and nothing else.
///
/// The barrier that actually fires for the level verbs is `require_live()`,
/// which admits only `Iterating`/`Holding`. `unhold()`'s explicit
/// `== TwrState::Holding` is a SECOND guard behind it and is unreachable from
/// here today - stated rather than claimed, because mutating `unhold` to
/// release `CoherenceHold` too leaves this test GREEN. It is kept because the
/// day a verb reaches `unhold` without `require_live` in front of it, the
/// name-the-state form is the one that does not silently release the wrong
/// park.
#[test]
fn only_reset_releases_the_coherence_hold() {
    let mut player = ScriptedPlayer::well_behaved(2.0, 5);
    player.disturbance = 10.0;
    let mut svc = TwrService::new(player, config()).unwrap();
    svc.advance().unwrap_err();
    assert_eq!(svc.state(), TwrState::CoherenceHold);

    // Every other release path refuses and leaves the hold standing.
    assert!(svc.step_level_up().is_err());
    assert!(svc.step_level_down().is_err());
    assert!(svc.jump_to_level(0).is_err());
    assert!(svc.begin_playback().is_err());
    assert!(svc.arm_override("not a feasibility block").is_err());
    assert_eq!(svc.state(), TwrState::CoherenceHold);

    // Reset does, and it drops what the failed identification produced.
    svc.reset().unwrap();
    assert_eq!(svc.state(), TwrState::Configured);
    assert!(svc.status().sysid_mean_coherence.is_none());
    assert!(svc.history().records.is_empty());
    // ...and the run is runnable again from the top.
    let mut ok = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), config()).unwrap();
    ok.reset().unwrap();
    assert_eq!(ok.run_to_completion(200).unwrap(), TwrState::Complete);
}

/// The ceiling refuses BY NAME and carries both numbers - what was commanded
/// and what was authorized. Never clips: a clipped drive would
/// converge around its own distortion, because the law corrects the realized
/// record by construction.
/// Two rows, and they discriminate different call sites: the ranging
/// excitation is drive and meets the ceiling BEFORE it is played, and so does
/// the realized correction record.
///
/// RED-MUTATION: delete the `check_drive_peak` call from `do_sysid` and row 1
/// reds on `plays == 0` (the excitation goes out); delete it from
/// `play_current` and row 2 reds the same way on the iteration.
#[test]
fn the_drive_ceiling_refuses_by_name_with_both_numbers() {
    // Row 1: a ceiling under the ranging excitation refuses before the shaker
    // moves at all.
    let mut cfg = config();
    cfg.limits.max_drive_vpk = Some(0.001);
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg).unwrap();
    let end = svc.run_to_completion(200).unwrap();
    assert_eq!(end, TwrState::Aborted);
    assert_eq!(
        svc.player().plays,
        0,
        "the refusal must land BEFORE the excitation is played"
    );
    let reason = svc.status().abort_reason.unwrap();
    assert!(
        reason.contains("exceeds the max drive ceiling"),
        "the refusal must be named: {reason}"
    );
    assert!(reason.contains("0.0010"), "names the ceiling: {reason}");
    assert!(
        reason.contains("REFUSED, not clipped"),
        "and says it did not clip: {reason}"
    );

    // Row 2: a ceiling the ranging drive fits under but the REALIZED
    // correction record does not. Manual, so no auto level step can be the
    // thing that refuses instead.
    let mut probe_cfg = config();
    probe_cfg.schedule.mode = TwrRunMode::Manual;
    probe_cfg.sysid.drive_rms = 0.01;
    let mut probe =
        TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), probe_cfg.clone()).unwrap();
    probe.advance().unwrap();
    probe.advance().unwrap();
    let drive_peak = probe
        .drive_time()
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    let ranging_peak = probe_cfg
        .sysid
        .build_excitation(FS)
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    assert!(ranging_peak < drive_peak, "{ranging_peak} vs {drive_peak}");

    let mut cfg2 = probe_cfg;
    cfg2.limits.max_drive_vpk = Some((ranging_peak + drive_peak) / 2.0);
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg2).unwrap();
    svc.advance().unwrap(); // sysid passes the ceiling
    assert_eq!(svc.player().plays, 1);
    let err = svc.advance().unwrap();
    assert_eq!(err, Advanced::Terminal(TwrState::Aborted));
    assert_eq!(
        svc.player().plays,
        1,
        "the correction drive must be refused BEFORE it reaches the player"
    );
    assert!(svc
        .status()
        .abort_reason
        .unwrap()
        .contains("exceeds the max drive ceiling"));
}

/// The walkable-ceiling hole: four +1 dB clicks all used to report
/// success, and the play after them threw away a measured sysID and three
/// converged levels. The check lives INSIDE `scale_drive_db`, so a step that
/// would not fit refuses before anything moves.
///
/// RED-MUTATION: move the ceiling check out of `scale_drive_db` (back to
/// play-time only) and this test reds on `step_level_up().is_err()`.
#[test]
fn a_level_step_at_the_ceiling_refuses_and_leaves_the_drive_unchanged() {
    // MANUAL, because the point is the OPERATOR's step. In AUTO the ladder
    // takes its own +3 dB the moment the level gates, and that step meets the
    // same check - it aborts the run by name, which is the right answer to a
    // different question.
    let manual = || {
        let mut c = config();
        c.schedule.mode = TwrRunMode::Manual;
        // The ranging excitation is drive too, so it meets the same ceiling.
        // Keep it well under the correction drive so the ceiling calibrated
        // below binds the LEVEL STEP and not the sysid pass. (The plant double
        // is noiseless, so a smaller ranging drive costs no coherence.)
        c.sysid.drive_rms = 0.01;
        c
    };
    // Phase 1: learn what this fixture's realized drive peak actually is, so
    // the ceiling below is calibrated rather than guessed.
    let mut probe = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), manual()).unwrap();
    probe.advance().unwrap(); // sysid
    probe.advance().unwrap(); // one iteration
    let peak0 = probe
        .drive_time()
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    assert!(peak0 > 0.0, "the fixture produced no drive");
    let sysid_peak = manual()
        .sysid
        .build_excitation(FS)
        .iter()
        .fold(0.0f64, |m, &v| m.max(v.abs()));
    assert!(
        sysid_peak < peak0,
        "calibration premise: the ranging peak {sysid_peak} must sit under the \
         correction drive peak {peak0}, or the ceiling below binds the wrong play"
    );

    // Phase 2: a ceiling the CURRENT drive fits under, and a +1 dB step
    // (x1.122) does not.
    let mut cfg = manual();
    cfg.limits.max_drive_vpk = Some(peak0 * 1.05);
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg).unwrap();
    svc.advance().unwrap();
    svc.advance().unwrap();
    let level_before = svc.status().level_db;
    let spec_before = svc.drive_spectrum_mag();

    let err = svc.step_level_up().unwrap_err().to_string();
    assert!(err.contains("refused"), "{err}");
    assert!(err.contains("exceeds the max drive ceiling"), "{err}");
    assert!(
        err.contains("The level and the drive are unchanged"),
        "the refusal must say what did NOT happen: {err}"
    );

    assert_eq!(svc.status().level_db, level_before, "the level moved");
    let spec_after = svc.drive_spectrum_mag();
    assert_eq!(spec_before.len(), spec_after.len());
    for (i, (b, a)) in spec_before.iter().zip(spec_after.iter()).enumerate() {
        assert!(
            (b.1 - a.1).abs() < 1e-12,
            "drive spectrum bin {i} moved on a refused step: {} -> {}",
            b.1,
            a.1
        );
    }
    // The run is still live and still usable - a refusal is not an abort.
    assert_eq!(svc.state(), TwrState::Iterating);
    assert!(svc.status().abort_reason.is_none());
    // A step DOWN still works: the ceiling bounds one direction only.
    svc.step_level_down().unwrap();
    assert!(svc.status().level_db < level_before);
}

/// A clipped run must SAY it is clipped.
///
/// Nothing else reports it: the law corrects the realized clipped drive by
/// construction, so the error plateaus rather than rises and the divergence
/// guard never fires. The operator adds drive, it stalls again, and nothing
/// anywhere says the record is being flattened.
///
/// RED-MUTATION: stop assigning `last_clipped_frac` in `play_current` and the
/// non-zero assertion reds while the run still "converges".
#[test]
fn a_clipping_run_reports_a_non_zero_clipped_frac() {
    let mut cfg = config();
    cfg.schedule.mode = TwrRunMode::Manual;
    cfg.sysid.drive_rms = 0.005;
    // A clip well under the correction drive, so the realized record is
    // flattened but the ranging pass is not.
    cfg.limits.drive_clip = Some(0.02);
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), cfg).unwrap();

    svc.advance().unwrap(); // sysid: under the clip
    assert_eq!(
        svc.status().clipped_frac,
        0.0,
        "the ranging drive was not clipped"
    );

    svc.advance().unwrap(); // one correction pass, clipped
    let frac = svc.status().clipped_frac;
    assert!(
        frac > 0.0,
        "a clipped drive record must reach status; got {frac}"
    );
    assert!(frac <= 1.0, "a fraction, not a count: {frac}");
    // It agrees with the convergence table's count, which is the other end of
    // the same fact.
    let rec = svc.history().records.last().unwrap();
    assert!(rec.clipped_samples > 0);
    let n = svc.drive_time().len();
    assert!(
        (frac - rec.clipped_samples as f64 / n as f64).abs() < 1e-12,
        "status {frac} vs table {}/{n}",
        rec.clipped_samples
    );
}

#[test]
fn report_carries_honesty_history_and_metadata() {
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), config()).unwrap();
    svc.run_to_completion(200).unwrap();
    let rep = svc.report();
    assert_eq!(rep.state, TwrState::Complete);
    assert!(rep.honesty.capabilities.adaptation_runs_hostside);
    // The honesty sentence is DERIVED, and the report carries the same one the
    // live banner does. It used to assert `contains("Sim")` against a hardcoded
    // string that claimed a station the run might not be on.
    assert_eq!(rep.honesty.note, rep.status.path_label);
    assert!(
        rep.honesty.note.contains("CHASSIS GATE PENDING"),
        "the live evidence gap must ride the report: {}",
        rep.honesty.note
    );
    assert!(
        rep.honesty.note.contains("an unnamed station"),
        "a scripted double names no station, and the label must say so rather \
         than pick one: {}",
        rep.honesty.note
    );
    assert!(!rep.history.records.is_empty());
    assert!(rep.sysid.as_ref().unwrap().in_band_mean_coherence > 0.9);
    assert_eq!(rep.metadata.len(), 4);
    assert!(!rep.config_json.is_empty());
    // Round-trips as JSON (the UI/report path).
    let j = serde_json::to_string(&rep).unwrap();
    assert!(j.contains("honesty"));
}

#[test]
fn capture_and_snapshot_write_files() {
    let dir = std::env::temp_dir().join(format!(
        "hertzforge_twr_capture_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut svc = TwrService::new(ScriptedPlayer::well_behaved(2.0, 5), config()).unwrap();
    let run_dir = svc.start_capture(&dir).unwrap();
    svc.run_to_completion(200).unwrap();
    svc.stop_capture().unwrap();
    assert!(run_dir.join("telemetry.f32").exists());
    assert!(run_dir.join("capture.json").exists());
    let snap = svc.save_snapshot(&run_dir, vec![]).unwrap();
    assert!(snap.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
