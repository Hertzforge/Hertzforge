//! The v4 wire contract's SEMANTICS: the Explorer profile variant, the Explorer
//! command group, and the two `StatusReport` open-loop readouts.
//!
//! v4 exists because `SineProfile` could not be grown. It is `deny_unknown_fields`, so
//! adding a field to it is NOT additive: a peer built before the field meets a serde
//! field error part-way through a workflow, on the payload that points a shaker. A new
//! VARIANT plus one version bump moves that failure to the connect handshake, where a
//! version mismatch belongs. Everything below is the evidence for that trade.

use hertzforge_protocol::{
    Command, ExplorerCommand, ExplorerProfile, ExplorerSlew, ProtocolError, SchemaKind,
    StatusReport, SweepDirection, SweepSpec, TestMode, TestProfile, PROFILE_SCHEMA,
    PROTOCOL_VERSION,
};

fn explorer() -> ExplorerProfile {
    ExplorerProfile {
        schema_version: PROFILE_SCHEMA,
        fs_hz: 8192.0,
        band_hz: (20.0, 2000.0),
        start_hz: 100.0,
        // THE ARM POSTURE. Zero is a legal, expected value here, not a placeholder.
        amplitude_vpk: 0.0,
        max_drive_vpk: 1.0,
        slew: ExplorerSlew {
            freq_oct_per_s: 1.0,
            drive_db_per_s: 6.0,
        },
        sweep: None,
        display_nfft: 4096,
    }
}

// =====================================================================================
// The version gate is what makes the new variant safe
// =====================================================================================

/// The bump landed. Stated here as well as in the crate's own unit test because THIS
/// file is the one that adds payloads an older peer cannot parse - if the two ever
/// disagree, the payloads shipped without the gate that refuses the peers who would
/// choke on them.
///
/// **Asserted BEHAVIOURALLY as of v5, not as a number.** This is a HISTORICAL pin
/// file: it owns the semantics the v4 payloads shipped with, and what those payloads
/// need is not "the version equals 4" - it is "every pre-v4 peer is refused at the
/// gate", which stays true and stays load-bearing however far the version moves
/// afterwards. Pinning the literal here would red this file on every future bump, for
/// a surface that did not change, which trains the next person to edit the number
/// without reading what it guards. The CURRENT version is pinned exactly once, in
/// `tests/v5.rs`.
#[test]
fn the_new_payloads_ride_a_bumped_version() {
    // A v3 peer - the last one that did not know `TestProfile::Explorer` - is refused
    // at the envelope, naming both versions.
    let mut old = hertzforge_protocol::Envelope::new(1u64, Command::Start);
    old.v = 3;
    assert_eq!(
        old.check_version().unwrap_err(),
        ProtocolError::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs: 3,
        },
        "the v4 Explorer payloads need a gate that refuses every pre-v4 peer"
    );

    // ANTI-VACUITY: this build's own envelopes pass, so the refusal above is about the
    // version and not about `check_version` refusing everything.
    assert!(hertzforge_protocol::Envelope::new(1u64, Command::Start)
        .check_version()
        .is_ok());
}

// =====================================================================================
// The Explorer profile
// =====================================================================================

/// The profile ROUND-TRIPS with every field intact, including a `0.0` amplitude.
///
/// The zero is the assertion that matters. `amplitude_vpk: 0.0` is the Explorer's arm
/// posture - the tone exists, the output is silent, the operator raises it - so a
/// serializer that treated zero as "absent" (a `skip_serializing_if` reflex) would
/// turn the safe posture into a decode failure, or worse into a default.
#[test]
fn the_explorer_profile_round_trips_including_a_zero_amplitude() {
    let p = TestProfile::Explorer(ExplorerProfile {
        sweep: Some(SweepSpec {
            direction: SweepDirection::Down,
            rate_oct_per_min: 0.5,
        }),
        ..explorer()
    });
    let json = serde_json::to_string(&p).expect("encode");
    assert!(json.contains("\"amplitude_vpk\":0.0"), "{json}");
    let back: TestProfile = serde_json::from_str(&json).expect("decode");
    assert_eq!(back, p);
}

/// An UNKNOWN FIELD is a hard, named decode failure - the same posture as
/// `SineProfile` and `RandomProfile`, and the reason a new field on this type will
/// need its own version bump too.
#[test]
fn an_unknown_field_on_the_explorer_profile_is_refused_by_name() {
    let json = r#"{"explorer":{"schema_version":1,"fs_hz":8192.0,"band_hz":[20.0,2000.0],
        "start_hz":100.0,"amplitude_vpk":0.0,"max_drive_vpk":1.0,
        "slew":{"freq_oct_per_s":1.0,"drive_db_per_s":6.0},"display_nfft":4096,
        "amplitude_vrms":0.7}"#;
    let err = serde_json::from_str::<TestProfile>(json).expect_err("must refuse");
    assert!(
        err.to_string().contains("amplitude_vrms"),
        "the offending field must be NAMED: {err}"
    );
}

/// The Explorer runs under the OPEN-LOOP SINE mode.
///
/// It is the same mode as SMART - commanded drive, measured response, nothing
/// correcting per tick. What differs is who SYNTHESIZES, which is the resolver's
/// `DriveOrigin` axis and not a mode. Claiming a new mode here would have forced a
/// `ModeMatrix` row for a cell that already exists.
#[test]
fn the_explorer_profile_is_open_loop_sine_not_a_new_mode() {
    assert_eq!(
        TestProfile::Explorer(explorer()).mode(),
        TestMode::OpenLoopSine
    );
}

/// The profile goes through the SAME schema gate as every other document-carrying
/// command, and the gate is now exhaustive (no `_` arm) so a future verb cannot
/// inherit a silent pass.
#[test]
fn the_explorer_profile_is_schema_gated_like_every_other_document() {
    assert!(Command::SetProfile {
        profile: TestProfile::Explorer(explorer()),
    }
    .check_schemas()
    .is_ok());

    let future = ExplorerProfile {
        schema_version: PROFILE_SCHEMA + 1,
        ..explorer()
    };
    let err = Command::SetProfile {
        profile: TestProfile::Explorer(future),
    }
    .check_schemas()
    .unwrap_err();
    assert!(matches!(
        err,
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::Profile,
            ..
        }
    ));
}

// =====================================================================================
// The Explorer command group
// =====================================================================================

/// Both sweep verbs are MUTATING and both are gated by the frequency action.
///
/// The Observer half is the safety-relevant one: a watcher on the lab LAN that could
/// start a sweep would be a watcher that can move someone else's shaker across a
/// resonance.
#[test]
fn the_sweep_verbs_are_mutating_and_gated_by_the_frequency_action() {
    for e in [
        ExplorerCommand::StartSweep {
            direction: SweepDirection::Up,
            rate_oct_per_min: 2.0,
        },
        ExplorerCommand::StopSweep,
    ] {
        assert!(e.is_mutating(), "{e:?} must require the run lease");
        assert!(Command::Explorer(e).is_mutating());
        assert_eq!(
            Command::Explorer(e).action_kind(),
            Some(hertzforge_protocol::ActionKind::GoToFrequency),
            "a sweep is frequency authority - mapped, not matrixed"
        );
    }
}

/// The command round-trips through the externally-tagged wire form.
#[test]
fn the_sweep_verbs_round_trip() {
    let c = Command::Explorer(ExplorerCommand::StartSweep {
        direction: SweepDirection::Down,
        rate_oct_per_min: 0.25,
    });
    let json = serde_json::to_string(&c).expect("encode");
    assert!(json.contains("\"down\""), "{json}");
    assert_eq!(serde_json::from_str::<Command>(&json).expect("decode"), c);
}

// =====================================================================================
// The status additions
// =====================================================================================

/// The two new readouts are ADDITIVE: absent from an idle report entirely, so an older
/// decoder sees exactly the v3 document it expects.
///
/// `sweep_active` is three-valued, and this pins the distinction the UI depends on:
/// `None` (no sweep-capable module live) must not serialize the same as `Some(false)`
/// (a module is live and parked), because the front end suppresses spectrum averaging
/// on `Some(true)` and must not suppress it for a module that never sweeps.
#[test]
fn the_open_loop_readouts_are_additive_and_sweep_state_is_three_valued() {
    let idle = serde_json::to_string(&StatusReport::idle()).expect("encode");
    assert!(!idle.contains("drive_vpk"), "{idle}");
    assert!(!idle.contains("sweep_active"), "{idle}");

    let parked = StatusReport {
        drive_vpk: Some(0.0),
        sweep_active: Some(false),
        ..StatusReport::idle()
    };
    let json = serde_json::to_string(&parked).expect("encode");
    assert!(json.contains("\"sweep_active\":false"), "{json}");
    assert!(json.contains("\"drive_vpk\":0.0"), "{json}");
    assert_eq!(
        serde_json::from_str::<StatusReport>(&json).expect("decode"),
        parked
    );

    // ANTI-VACUITY: the two states really do encode differently, so the assertions
    // above are not passing on a field that is always omitted.
    assert_ne!(json, idle);
}
