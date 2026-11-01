//! THE GOLDEN-FIXTURE FENCE for [`TwrProfile`] - the Rust half the TS fence
//! declared owed (`ui/src/lib/twr/wireProfile.test.ts` header). Same contract as
//! `sine_golden.rs`: ONE artifact shared by both sides,
//! `crates/hertzforge-protocol/golden/twr_wire_profile.golden.json`, so a field added,
//! dropped or renamed on either side of the seam reds a test that names it -
//! never a silently-defaulted authorization.
//!
//! Two mechanisms instead of sine's maximal literal, same forcing property:
//!
//! - **Round-trip equality**: the fixture deserializes under
//!   `deny_unknown_fields` (a TS-side extra reds naming the field) and
//!   re-serializes to EXACTLY the fixture `Value` (a Rust-side representation
//!   change reds on the diff).
//! - **No-`..` destructures**: every profile struct is exhaustively destructured
//!   below, so a field ADDED in Rust stops this file compiling — the author is
//!   forced here, and the fixture + the TS mapper move in the same change.
//!
//! Regeneration is deliberately manual (no escape hatch): after an INTENDED
//! change, print with
//! `cargo test -p hertzforge-protocol --test twr_golden -- --ignored --nocapture print_golden`
//! and paste over the fixture, then fix the TS mapper — which is the work the
//! fence exists to force.

use hertzforge_protocol::twr::{
    TwrFeasibilityCeilings, TwrInverseSpec, TwrLawSpec, TwrLevelSchedule, TwrLimitsSpec,
    TwrMetaField, TwrPrep, TwrProfile, TwrRunSchedule, TwrSection, TwrSysIdSpec,
};

// In this crate, so the protocol crate builds without the UI
// tree; `wireProfile.test.ts` imports the same file.
const GOLDEN: &str = include_str!("../golden/twr_wire_profile.golden.json");

fn parsed() -> TwrProfile {
    serde_json::from_str(GOLDEN).expect(
        "the golden fixture must deserialize under deny_unknown_fields; a failure here \
         names a TS-side field the Rust profile does not carry",
    )
}

/// The fixture IS the wire document: deserialize → serialize reproduces it
/// byte-for-byte at the Value level. A Rust-side serialization change (rename,
/// representation, a default that starts skipping) reds here with the diff.
#[test]
fn the_golden_fixture_round_trips_under_deny_unknown_fields() {
    let profile = parsed();
    let reserialized = serde_json::to_value(&profile).expect("serialize");
    let fixture: serde_json::Value = serde_json::from_str(GOLDEN).expect("fixture is JSON");
    assert_eq!(
        reserialized, fixture,
        "the Rust serialization of the parsed fixture is not the fixture - one side \
         of the seam moved; regenerate deliberately (see the module docs), never adjust \
         the assertion"
    );
}

/// A field added to ANY profile struct breaks this compile (no `..` anywhere),
/// which is the same forcing function as sine's maximal literal: the author must
/// come here, and the fixture + the TS mapper ride the same change.
#[test]
fn every_profile_field_is_fenced_exhaustively() {
    let TwrProfile {
        schema_version,
        reference_sha,
        prep,
        section,
        pad_pre,
        pad_post,
        sysid,
        inverse,
        law,
        schedule,
        run,
        adapt_during_playback,
        limits,
        kinematics_hp_hz,
        review_after_sysid,
        metadata,
    } = parsed();

    assert_eq!(schema_version, hertzforge_protocol::PROFILE_SCHEMA);
    assert!(!reference_sha.is_empty());
    assert!(kinematics_hp_hz > 0.0);
    // The fixture is maximal-in-fields: the skip_serializing_if optionals must be
    // PRESENT or they are fenced by nothing.
    let TwrSection { start, len } = section.expect(
        "the fixture must exercise `section` (skip_serializing_if) or the optional \
         is unfenced",
    );
    assert!(len > start || start == 0);
    assert!(pad_pre > 0 && pad_post > 0, "fixture pads are non-zero");
    // Presence is the fence for the two booleans; their VALUES are free to be
    // defaults (the fixture is maximal in FIELDS, not in values - sine's rule).
    let _ = (adapt_during_playback, review_after_sysid);

    let TwrPrep {
        auto_resample: _,
        detrend: _,
        highpass_hz: _,
        scale: _,
        taper_frac: _,
    } = prep;
    let TwrSysIdSpec {
        excitation: _,
        drive_rms: _,
        duration_s: _,
        nfft: _,
        f_lo_hz: _,
        f_hi_hz: _,
        min_mean_coherence: _,
    } = sysid;
    let TwrInverseSpec {
        tikhonov_beta_floor: _,
        coherence_floor: _,
    } = inverse;
    let TwrLawSpec {
        beta: _,
        max_db_per_iter: _,
        ref_floor_db: _,
        f_lo_hz: _,
        f_hi_hz: _,
        initial_drive_backoff_db: _,
        min_masked_energy_frac: _,
    } = law;
    let TwrLevelSchedule {
        mode: _,
        table_db: _,
        gate_rms_error_pct: _,
        max_iterations_per_level: _,
        on_gate_fail: _,
    } = schedule;
    match run {
        TwrRunSchedule::Once | TwrRunSchedule::LoopN { .. } | TwrRunSchedule::DurationS { .. } => {}
    }
    let TwrLimitsSpec {
        envelope_alarm_frac: _,
        envelope_abort_frac: _,
        envelope_debounce_samples: _,
        response_rms_limit: _,
        drive_clip: _,
        drive_rms_limit: _,
        max_drive_vpk,
        divergence_n: _,
        divergence_tol_frac: _,
        feasibility,
    } = limits;
    assert!(
        max_drive_vpk.is_some(),
        "the fixture must exercise the drive-ceiling authorization"
    );
    let TwrFeasibilityCeilings {
        peak_accel,
        peak_velocity_ips: _,
        disp_pkpk_in: _,
    } = feasibility;
    assert!(
        peak_accel.is_some(),
        "the fixture must exercise the per-ceiling optionals or they are unfenced"
    );
    for TwrMetaField {
        key: _,
        label: _,
        value: _,
    } in metadata
    {
        // presence is the assertion; fields fenced by the destructure
    }
}

/// Manual regeneration helper (see the module docs — no automatic overwrite).
#[test]
#[ignore = "manual regeneration helper, run with --ignored --nocapture"]
fn print_golden() {
    let profile = parsed();
    println!(
        "{}",
        serde_json::to_string_pretty(&profile).expect("serialize")
    );
}
