//! `TwrHost` against an HERTZFORGE-SIM station.
//!
//! Every row here is about the HOST's obligations - the parks, the barriers, the
//! teardown, the upload gate and the fidelity fence - and none of them is about the
//! control law, which `hertzforge-twr`'s own `sim_e2e` gate owns. A daemon test that
//! re-asserted convergence would be measuring the service through a second, slower
//! lens; a daemon test that asserted a park is measuring something only the daemon can
//! get wrong.

use std::time::{Duration, Instant};

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};

use super::*;

// =====================================================================================
// Fixtures
// =====================================================================================

const FS: f64 = 8192.0;
const N: usize = 2048;

/// One control input and one drive output - the table `build_twr_station` requires.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    t.channels.push(ctrl);
    let mut drive = Channel::new(2);
    drive.role = ChannelRole::Drive;
    drive.units = Units::VoltageV;
    t.channels.push(drive);
    t
}

/// A decaying-sine transient: a plausible TWR reference, generated rather than shipped
/// so the tests have no data dependency they can silently lose.
fn reference_bytes() -> (Vec<u8>, String) {
    let mut bytes = Vec::with_capacity(N * 4);
    for i in 0..N {
        let t = i as f64 / FS;
        let v = ((-18.0 * t).exp() * (std::f64::consts::TAU * 137.0 * t).sin()) as f32;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let mut h = digest::Sha256::new();
    h.update(&bytes);
    let sha = h.finish_hex();
    (bytes, sha)
}

fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn host() -> TwrHost {
    let (tx, _) = tokio::sync::broadcast::channel(64);
    TwrHost::new(BackendKind::Sim, Vec::new(), tx, 30.0)
}

/// Stage the fixture record and answer its sha.
fn stage(h: &TwrHost) -> String {
    let (bytes, sha) = reference_bytes();
    h.begin_reference("t.csv".into(), FS, "g".into(), N as u64, sha.clone())
        .expect("begin");
    for (seq, chunk) in bytes.chunks(512 * 4).enumerate() {
        h.reference_chunk(seq as u32, &b64(chunk)).expect("chunk");
    }
    h.commit_reference().expect("commit");
    sha
}

/// A profile that runs QUICKLY on the Sim wire: a short sysID and a one-rung ladder.
///
/// Short on purpose - the control law's own gate lives in `hertzforge-twr` and re-proving it
/// here would only make this file slow. What the shortness must NOT do is change the
/// SHAPE under test, so the ladder, the parks and the schedule are all real.
fn profile(sha: &str) -> TwrProfile {
    TwrProfile {
        schema_version: hertzforge_protocol::PROFILE_SCHEMA,
        reference_sha: sha.into(),
        prep: hertzforge_protocol::TwrPrep {
            auto_resample: true,
            detrend: Some(false),
            highpass_hz: None,
            scale: 1.0,
            taper_frac: None,
        },
        section: None,
        pad_pre: 128,
        pad_post: 256,
        sysid: hertzforge_protocol::TwrSysIdSpec {
            excitation: hertzforge_protocol::TwrExcitation::Random,
            drive_rms: 0.05,
            duration_s: 0.25,
            nfft: 512,
            f_lo_hz: 30.0,
            f_hi_hz: 1500.0,
            min_mean_coherence: 0.8,
        },
        inverse: hertzforge_protocol::TwrInverseSpec {
            tikhonov_beta_floor: 0.05,
            coherence_floor: 0.9,
        },
        law: hertzforge_protocol::TwrLawSpec {
            beta: 0.5,
            max_db_per_iter: 6.0,
            ref_floor_db: 50.0,
            f_lo_hz: 30.0,
            f_hi_hz: 1500.0,
            initial_drive_backoff_db: 0.0,
            min_masked_energy_frac: 0.5,
        },
        schedule: hertzforge_protocol::TwrLevelSchedule {
            mode: hertzforge_protocol::TwrRunMode::Manual,
            table_db: vec![-6.0, 0.0],
            gate_rms_error_pct: 10.0,
            max_iterations_per_level: 4,
            on_gate_fail: hertzforge_protocol::TwrGateFailPolicy::Hold,
        },
        run: hertzforge_protocol::TwrRunSchedule::Once,
        adapt_during_playback: false,
        limits: hertzforge_protocol::TwrLimitsSpec {
            envelope_alarm_frac: 0.25,
            envelope_abort_frac: 5.0,
            envelope_debounce_samples: 8,
            response_rms_limit: None,
            drive_clip: None,
            drive_rms_limit: None,
            max_drive_vpk: Some(hertzforge_protocol::DEFAULT_TWR_MAX_DRIVE_VPK),
            divergence_n: 3,
            divergence_tol_frac: 0.10,
            feasibility: hertzforge_protocol::TwrFeasibilityCeilings::default(),
        },
        kinematics_hp_hz: 5.0,
        review_after_sysid: true,
        metadata: Vec::new(),
    }
}

/// Park until `pred` holds, or fail loudly with the state it got stuck in.
fn wait_until(h: &TwrHost, what: &str, pred: impl Fn(&TwrHost) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if pred(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut s = hertzforge_protocol::StatusReport::idle();
    h.fill_status(&mut s);
    panic!("timed out waiting for {what}; state = {:?}", s.module_state);
}

fn module_state(h: &TwrHost) -> String {
    let mut s = hertzforge_protocol::StatusReport::idle();
    h.fill_status(&mut s);
    s.module_state.unwrap_or_default()
}

// =====================================================================================
// The parks
// =====================================================================================

/// **THE POST-SYSID REVIEW PARK, default ON**: a PASSING system ID holds before the
/// first iterate unit, the park is LIVE, and `ProceedAfterReview` releases it.
///
/// Four properties in one row because they are one behaviour and asserting them apart
/// would let three of them pass against a park that never happened:
///
/// * the run reaches `awaiting_review` and STAYS there (no unit runs on its own);
/// * the token is NAMESPACED with its reason, per `IPC_CONTRACT.md`'s v5 table;
/// * `is_live()` is TRUE at the park - the SRS hold obligation, which is what stops a
///   second module binding the chassis while an operator studies the FRF;
/// * `ProceedAfterReview` releases it and the run leaves the park.
///
/// RED-MUTATION: set `review_armed` to `false` unconditionally in `set_profile` and the
/// run walks past the park; the `awaiting_review` wait times out.
#[test]
fn a_passing_sysid_parks_at_the_review_hold_and_proceed_releases_it() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.start().expect("start");

    wait_until(&h, "the review park", |h| h.at_review_hold());
    let token = module_state(&h);
    assert!(
        token.starts_with("awaiting_review: "),
        "the token must be NAMESPACED with its reason: {token}"
    );
    assert!(token.contains("coherence"), "{token}");
    assert!(h.is_live(), "a park is a PAUSED run holding a claimed core");

    // IT STAYS. A park that a later unit walks past is not a park.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        h.at_review_hold(),
        "the park must hold until it is released"
    );

    h.proceed_after_review().expect("release");
    wait_until(&h, "the park to release", |h| !h.at_review_hold());
}

/// The park is BYPASSABLE per profile, and the bypass is the profile's own flag.
#[test]
fn review_after_sysid_false_walks_straight_past_the_park() {
    let h = host();
    let sha = stage(&h);
    let mut p = profile(&sha);
    p.review_after_sysid = false;
    h.set_profile(p, table(), "test").expect("configure");
    h.start().expect("start");

    wait_until(&h, "the sysID to finish", |h| {
        !matches!(module_state(h).as_str(), "priming" | "sysid")
    });
    assert!(
        !h.at_review_hold(),
        "the profile turned the park off: {}",
        module_state(&h)
    );
}

/// **A VANISHED CONTROLLER LEAVES THE REVIEW PARK STANDING; THE OPERATOR'S STOP ENDS
/// IT.** The SRS review-hold obligation, applied here.
///
/// The row most likely to be got wrong in a way nothing else notices: a safe-stop that
/// tore the park down would discard a measured system ID because a network went quiet,
/// and the operator would find an idle core with no sign of why.
///
/// RED-MUTATION: delete the `survives_vanish` early return in `safe_stop` and the first
/// assertion reds.
#[test]
fn safe_stop_leaves_the_review_park_standing_and_operator_stop_ends_it() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.start().expect("start");
    wait_until(&h, "the review park", |h| h.at_review_hold());

    h.safe_stop("controller vanished");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        h.at_review_hold(),
        "a VANISH must leave the park standing - the plant is already parked and the \
         system ID is measured"
    );
    assert!(h.is_live(), "and the core stays CLAIMED");

    h.operator_stop("operator stop");
    wait_until(&h, "the operator stop to end the park", |h| {
        !h.at_review_hold()
    });
}

/// **THE STATUS MUST NOT KEEP REPORTING A PARK THE RUN HAS LEFT.**
///
/// The published snapshot DECORATES the phase with its reason; it does not override it.
/// The two are written by different things at different times - the cell at unit
/// boundaries, the phase whenever anything moves the run - and they go out of step in
/// exactly one situation, which is the one that matters most: the pump has EXITED at a
/// park, so nothing rewrites the cell, and a stop that only moves the phase would leave
/// the wire reporting a hold that is over.
///
/// FOUND BY RED-MUTATION, not by reasoning (2026-08-09): with `safe_stop`'s park
/// early-return removed, a vanished controller stopped a run parked at the review hold
/// and the daemon went on reporting `awaiting_review: system ID passed ...` for the
/// remaining 20 seconds of the probe's window. The probe row PASSED, because it was
/// asserting on the string the daemon was still serving.
///
/// RED-MUTATION: drop the `snap.phase == phase` guard in `fill_status` and this reds -
/// the status keeps serving the park's decorated token after an operator stop.
#[test]
fn the_status_stops_reporting_the_park_the_moment_the_run_leaves_it() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.start().expect("start");
    wait_until(&h, "the review park", |h| h.at_review_hold());
    assert!(
        module_state(&h).starts_with("awaiting_review: "),
        "the park's own token carries its reason"
    );

    // The OPERATOR's stop ends the park. The pump has already exited here, so nothing
    // will rewrite the published cell - which is precisely the gap under test.
    h.operator_stop("operator stop");
    let token = module_state(&h);
    assert!(
        !token.starts_with("awaiting_review"),
        "the wire must not report a hold the run has left: {token}"
    );
    // ...and what it reports instead is the phase's own plain token, which is less
    // detail and no lie.
    assert!(
        token == "stopping" || token.starts_with("stopped"),
        "expected the stop phase, got {token}"
    );
}

/// **THE COHERENCE HOLD HAS NO RELEASE VERB**, and the refusal says so by name.
///
/// Exercised against the HOST's barrier rather than by forcing a real gate failure: the
/// service's own hold is proven in `hertzforge-twr`, and what the daemon has to get right is
/// that `ProceedAfterReview` refuses from it instead of respawning the pump. The phase
/// mirror is the input to that decision, so the test drives the phase.
///
/// RED-MUTATION: route `Phase::CoherenceHold` to the release path and this reds - which
/// is the state where one click bypasses a safety refusal.
#[test]
fn proceed_after_review_refuses_from_the_coherence_hold_by_name() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.set_phase(Phase::CoherenceHold);

    match h.proceed_after_review() {
        Err(ProtocolError::Refused { message }) => {
            assert!(message.contains("coherence hold"), "{message}");
            assert!(
                message.contains("RESET"),
                "the refusal must name what DOES release it: {message}"
            );
            assert!(
                message.contains("safety refusal"),
                "and say why it has no release verb: {message}"
            );
        }
        other => panic!("the coherence hold must refuse by name: {other:?}"),
    }
    // ...and the level verbs cannot reach past it either (the second barrier).
    assert!(h.iterate_once().is_err());
}

// =====================================================================================
// The upload
// =====================================================================================

/// The upload's not-live gate serves the CONTRACT string verbatim (the wire probe
/// asserts it).
#[test]
fn an_upload_during_a_live_run_is_refused_with_the_contract_string() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.start().expect("start");
    wait_until(&h, "the run to go live", |h| h.is_live());

    for r in [
        h.begin_reference("x".into(), FS, "g".into(), 4, sha.clone()),
        h.reference_chunk(0, "AACAPw=="),
        h.commit_reference().map(|_| ()),
    ] {
        match r {
            Err(ProtocolError::Refused { message }) => {
                assert_eq!(
                    message, UPLOAD_NOT_WHILE_LIVE,
                    "the not-live refusal is a two-file contract with the wire probe"
                );
            }
            other => panic!("an upload during a live run: {other:?}"),
        }
    }
}

/// A DOWNWARD resample is refused AT UPLOAD TIME, quoting both rates.
///
/// The operator learns immediately rather than minutes later at arm, and the refusal
/// says WHY - linear interpolation has no anti-alias filter, so converging on a
/// downsampled target is converging on an aliased spectrum.
#[test]
fn a_reference_above_the_station_rate_is_refused_at_upload_time() {
    let h = host();
    // The Sim station is built AT the record's rate, so the only way to be above it is
    // to be above what a chunk of THIS core could ever program. On HERTZFORGE-SIM the build
    // rate follows the record, so the refusal is exercised through the NI branch.
    let (tx, _) = tokio::sync::broadcast::channel(64);
    let ni = TwrHost::new(BackendKind::Ni, Vec::new(), tx, 30.0);
    let too_fast = hertzforge_station::NI_BUILD_RATE_HZ * 2.0;
    match ni.begin_reference(
        "fast.csv".into(),
        too_fast,
        "g".into(),
        16,
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
    ) {
        Err(ProtocolError::Refused { message }) => {
            assert!(message.contains(&format!("{too_fast}")), "{message}");
            assert!(message.contains("DOWNWARD resample"), "{message}");
            assert!(message.contains("anti-alias"), "{message}");
        }
        other => panic!("a too-fast record must be refused at upload: {other:?}"),
    }
    // ANTI-VACUITY: the same record at a rate the station can program is accepted, so
    // the refusal is about the RATE and not about the verb.
    assert!(h
        .begin_reference(
            "ok.csv".into(),
            FS,
            "g".into(),
            16,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into()
        )
        .is_ok());
}

/// **A PROFILE NAMING A DIFFERENT RECORD THAN THE ONE STAGED IS REFUSED, quoting BOTH
/// digests.**
///
/// The half that `core.rs`'s unstaged row cannot reach, and the more dangerous of the
/// two: a core with SOMETHING staged is a core that could plausibly just run it. A
/// profile is never run against whatever happens to be loaded - a run whose reference
/// silently changed between the review and the arm is exactly what keying by digest
/// makes impossible.
///
/// RED-MUTATION: drop the `if r.sha256 == profile.reference_sha` guard and this reds on
/// the acceptance.
#[test]
fn a_profile_naming_a_different_record_than_the_staged_one_is_refused() {
    let h = host();
    let staged_sha = stage(&h);
    let mut p = profile(&staged_sha);
    p.reference_sha = "0".repeat(64);
    match h.set_profile(p, table(), "test") {
        Err(ProtocolError::Refused { message }) => {
            assert!(message.contains("000000000000"), "the NAMED sha: {message}");
            assert!(
                message.contains(&staged_sha[..12]),
                "and the STAGED one: {message}"
            );
            assert!(
                message.contains("never run against whatever happens to be loaded"),
                "{message}"
            );
        }
        other => panic!("a mismatched sha must be refused: {other:?}"),
    }
    assert!(!h.is_configured(), "and nothing is left configured");

    // ANTI-VACUITY: the SAME profile with the staged sha configures, so the refusal is
    // about the digest and not about the profile.
    assert!(h.set_profile(profile(&staged_sha), table(), "test").is_ok());
}

/// `ReferenceInfo` answers the staged manifest with the CORE-computed sha, and gains the
/// APPLIED prep only once a profile exists.
#[test]
fn reference_info_carries_the_manifest_and_then_the_applied_prep() {
    let h = host();
    let sha = stage(&h);

    let doc = h.reference_info_doc().expect("a record is staged");
    assert_eq!(
        doc.schema_version,
        hertzforge_protocol::TWR_REFERENCE_SCHEMA
    );
    assert!(doc.json.contains(&sha), "{}", doc.json);
    assert!(doc.json.contains("\"applied\":null"), "{}", doc.json);

    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    let doc = h.reference_info_doc().expect("still staged");
    assert!(doc.json.contains("prepared_samples"), "{}", doc.json);
    assert!(doc.json.contains("prepared_fs_hz"), "{}", doc.json);
    assert!(!doc.json.contains("\"applied\":null"), "{}", doc.json);
}

// =====================================================================================
// Teardown
// =====================================================================================

/// **`Reset` RELEASES THE CORE AND DISCARDS THE RECORD; `release_idle` KEEPS IT.**
///
/// Two verbs, two different meanings, and the difference is the whole reason both exist:
/// Reset is "I am done with this module" and the idle release is "someone else configured
/// something". An operator who staged a 60 s field trace and then opened the Sine panel
/// must not have to send it again.
///
/// RED-MUTATION: clear `staged` in `teardown` instead of in `reset` and the second half
/// reds.
#[test]
fn reset_discards_the_staged_record_and_release_idle_keeps_it() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    assert!(h.is_configured());

    assert!(h.release_idle(), "a configured-but-idle run is releasable");
    assert!(!h.is_configured(), "the core is released");
    assert!(
        h.reference_info_doc().is_ok(),
        "the STAGED RECORD survives an idle release - the record is not the run"
    );

    // ...and a fresh configure works from the surviving record, with no re-upload.
    h.set_profile(profile(&sha), table(), "test")
        .expect("reconfigure");
    h.reset().expect("reset");
    assert!(!h.is_configured());
    assert!(
        h.reference_info_doc().is_err(),
        "Reset means done: the record goes with the run"
    );
}

/// A configured-but-LIVE run is NOT releasable by `release_idle`, which is the guard that
/// stops a parked measurement being dropped by someone else's `SetProfile`.
#[test]
fn release_idle_refuses_a_live_run() {
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    h.start().expect("start");
    wait_until(&h, "the run to go live", |h| h.is_live());
    assert!(!h.release_idle(), "a LIVE run is never released silently");
}

// =====================================================================================
// The chassis-ledger guard, the phase table and the fidelity fence
// =====================================================================================

/// **THE NI-REAL LEDGER GUARD is compiled in and - since 2026-08-09 - OPEN.**
///
/// The chassis gate is recorded, so the pin is the post-gate state: the ledger date is
/// `Some` - a `None` reappearing here is a rollback or a bad merge deleting hardware
/// evidence, which is exactly what this assert catches. The refusal STRING stays
/// pinned: the wire probe's rollback-detection row asserts its absence on NI-REAL, so
/// it remains a two-file contract.
///
/// The SIM half is the anti-vacuity, unchanged: an HERTZFORGE-SIM station starts either way,
/// so the guard is about the station class and not a blanket refusal of `Start`.
#[test]
fn the_chassis_ledger_is_recorded_and_its_refusal_string_is_pinned() {
    assert_eq!(
        TWR_CHASSIS_LEDGER_DATE,
        Some("2026-08-09"),
        "the chassis-gate ledger date is compiled evidence; if this went None or \
         changed, hardware evidence was rolled back - re-run the chassis gate before \
         touching this"
    );
    assert!(NO_CHASSIS_LEDGER.contains("NI-REAL"));
    assert!(NO_CHASSIS_LEDGER.contains("this build records no chassis gate"));
    assert!(NO_CHASSIS_LEDGER.contains("HERTZFORGE-SIM and NI-SIM stations are unaffected"));

    // ANTI-VACUITY: the guard does not refuse a Sim run.
    let h = host();
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    assert!(
        h.start().is_ok(),
        "HERTZFORGE-SIM is unaffected by the guard"
    );
}

/// **EVERY PARK IS LIVE** - the SRS hold obligation, stated as a table so a new phase
/// has to be classified rather than defaulting into "idle".
#[test]
fn every_park_holds_the_core() {
    for p in [
        Phase::AwaitingReview,
        Phase::CoherenceHold,
        Phase::FeasibilityBlocked,
        Phase::Holding,
    ] {
        assert!(
            p.is_live(),
            "{p:?} is a PAUSED run holding a claimed core, not an idle one"
        );
        assert_eq!(
            p.run_state(),
            hertzforge_protocol::RunState::Armed,
            "{p:?} maps to Armed plus a token - NO new RunState variant (IPC_CONTRACT v5)"
        );
    }
    // ANTI-VACUITY: the terminal phases are NOT live, so `is_live` is a classification
    // and not a constant.
    for p in [Phase::Idle, Phase::Stopped, Phase::Complete, Phase::Aborted] {
        assert!(!p.is_live(), "{p:?}");
    }
    // ...and only the two that hold a MEASUREMENT survive a vanish.
    assert!(Phase::AwaitingReview.survives_vanish());
    assert!(Phase::CoherenceHold.survives_vanish());
    assert!(!Phase::FeasibilityBlocked.survives_vanish());
    assert!(!Phase::Playing.survives_vanish());
}

/// **THE REMOTE-CAPTURE OBLIGATION, TWR half.** The wire profile has no capture block and
/// the translation must not inherit whatever the core default is.
///
/// RED-MUTATION: set `auto_start: true` and this reds - the state where a remote
/// configure opens a disk writer on the core host that the operator cannot see, size,
/// stop or fetch.
#[test]
fn the_twr_translation_forces_capture_off() {
    let (bytes, sha) = reference_bytes();
    let staged = StagedReference {
        name: "t.csv".into(),
        fs_hz: FS,
        eu: "g".into(),
        sha256: sha.clone(),
        samples: bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64)
            .collect(),
    };
    let cfg = translate_twr(&profile(&sha), &staged).expect("translate");
    assert!(
        !cfg.capture.auto_start,
        "the daemon must force the TWR capture off"
    );
    // ...and every operator input crosses VERBATIM rather than being defaulted
    // daemon-side (the wire fidelity fence, spot-checked on the fields most likely to
    // be quietly replaced by a default).
    assert_eq!(cfg.sysid.nfft, 512);
    assert_eq!(cfg.law.f_hi_hz, 1500.0);
    assert_eq!(cfg.schedule.table_db, vec![-6.0, 0.0]);
    assert_eq!(cfg.limits.max_drive_vpk, Some(5.0));
    assert!(cfg.review_after_sysid);
}

/// **A CLEARED DRIVE CEILING IS REFUSED**, and the refusal comes from the service's own
/// validation rather than from a daemon-side substitution.
///
/// The ceiling is the AUTHORIZATION, so its absence is not "unlimited", it is
/// "unauthorized". A daemon that helpfully filled in 5.0 V here would turn a refusal the
/// operator was owed into a run.
#[test]
fn a_cleared_drive_ceiling_is_refused_at_configure() {
    let h = host();
    let sha = stage(&h);
    let mut p = profile(&sha);
    p.limits.max_drive_vpk = None;
    match h.set_profile(p, table(), "test") {
        Err(ProtocolError::InvalidArgument { message }) => {
            assert!(message.contains("max_drive_vpk"), "{message}");
        }
        other => panic!("a cleared ceiling must be refused: {other:?}"),
    }
    assert!(!h.is_configured(), "and nothing is left configured");
}

/// The station refusals are TWR's own sentences, not the shock host's.
///
/// The text reaches the operator, and "a shock run plays a synthesized pulse" would be a
/// sentence about a module they are not running.
#[test]
fn the_station_refusals_name_what_a_twr_run_needs() {
    let mut no_control = ChannelTable::new();
    let mut a = Channel::new(1);
    a.role = ChannelRole::Monitor;
    no_control.channels.push(a);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    no_control.channels.push(d);
    match build_twr_station(BackendKind::Sim, &[], &no_control, FS) {
        Err(ProtocolError::Refused { message }) => {
            assert!(message.contains("CONTROL ACCELEROMETER"), "{message}");
            assert!(!message.contains("shock"), "{message}");
        }
        other => panic!("no Control row: {other:?}"),
    }

    let mut two_drive = table();
    let mut d2 = Channel::new(3);
    d2.role = ChannelRole::Drive;
    two_drive.channels.push(d2);
    match build_twr_station(BackendKind::Sim, &[], &two_drive, FS) {
        Err(ProtocolError::Refused { message }) => {
            assert!(message.contains("2 enabled Drive rows"), "{message}");
            assert!(message.contains("half rate"), "{message}");
        }
        other => panic!("two Drive rows: {other:?}"),
    }
}

/// **THE HISTORY MIRROR is answerable without the service mutex, and it says what it
/// carries.**
///
/// An EMPTY history is a measurement ("nothing has iterated"), which is a different fact
/// from "this core cannot answer" - and only the second is a refusal.
#[test]
fn history_answers_an_empty_table_before_anything_iterates() {
    let h = host();
    assert!(
        h.history_doc().is_err(),
        "an unconfigured core cannot answer, and says so"
    );
    let sha = stage(&h);
    h.set_profile(profile(&sha), table(), "test")
        .expect("configure");
    let doc = h.history_doc().expect("configured");
    assert_eq!(
        doc.schema_version,
        hertzforge_protocol::TWR_REFERENCE_SCHEMA
    );
    assert!(doc.json.contains("\"rows\":[]"), "{}", doc.json);
}
