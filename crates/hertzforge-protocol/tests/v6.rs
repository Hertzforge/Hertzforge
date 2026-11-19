//! **v6 - the TWR wire surface (TWR-on-NI, wire bump #5).**
//!
//! What this file gates, and why each row is here rather than assumed:
//!
//! 1. THE BUMP LANDED and every older peer is refused (the crate unit test says it too;
//!    this file is the one that adds the payloads an older peer cannot parse).
//! 2. `PROFILE_SCHEMA` STAYS 1 for the third bump running, and a stored Random profile
//!    still clears the gate. The anti-vacuity half proves the gate still refuses.
//! 3. THE PROFILE ROUND-TRIPS with every field intact, from a FULLY-POPULATED literal -
//!    a serializer that drops a field reds here, which is half three of the fidelity
//!    fence `twr.rs` documents.
//! 4. THE CLEARED CEILING SURVIVES THE WIRE. `max_drive_vpk: None` is the operator
//!    having cleared their authorization, and it must arrive as `None` rather than as
//!    the 5.0 default - otherwise a refusal the operator was owed becomes a run.
//! 5. EVERY VERB round-trips, and the `is_mutating` / `action_kind` tables are asserted
//!    verb by verb rather than in bulk.
//! 6. `CommitReference` IS AN OBJECT ON THE WIRE, not a bare string. A unit variant
//!    would serialize as `"commit_reference"` and growing a field later would change
//!    the shape from string to object mid-version.
//! 7. `TestMode::Twr` EXISTS, `ModeMatrix` grew the row, and the baseline still CLAIMS
//!    NOTHING - the mirror exception changes where evidence comes from, never whether
//!    it is required first.
//! 8. `SchemaKind::TwrReference` checks exactly, and `Response::Twr(Doc)` round-trips.

use hertzforge_protocol::{
    caps::{ModeMatrix, ModeSupport, UnsupportedReason},
    Command, Doc, LadderMode, ProtocolError, RandomProfile, Response, SchemaKind, TestMode,
    TestProfile, TwrCommand, TwrExcitation, TwrFeasibilityCeilings, TwrGateFailPolicy,
    TwrInverseSpec, TwrLawSpec, TwrLevelSchedule, TwrLimitsSpec, TwrMetaField, TwrPrep, TwrProfile,
    TwrRunMode, TwrRunSchedule, TwrSection, TwrSysIdSpec, DEFAULT_TWR_MAX_DRIVE_VPK,
    PROFILE_SCHEMA, PROTOCOL_VERSION, TWR_REFERENCE_SCHEMA,
};

/// A FULLY-POPULATED profile: every `Option` is `Some`, every collection non-empty.
///
/// Populated on purpose. A fixture built from defaults round-trips happily while a
/// serializer silently drops the fields the defaults left absent, which is the exact
/// hole `#[serde(skip_serializing_if)]` opens if nobody looks.
fn twr() -> TwrProfile {
    TwrProfile {
        schema_version: PROFILE_SCHEMA,
        reference_sha: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
        prep: TwrPrep {
            auto_resample: true,
            detrend: Some(true),
            highpass_hz: Some(5.0),
            scale: 1.0,
            taper_frac: Some(0.02),
        },
        section: Some(TwrSection {
            start: 128,
            len: 4096,
        }),
        pad_pre: 512,
        pad_post: 1024,
        sysid: TwrSysIdSpec {
            excitation: TwrExcitation::LogChirp,
            drive_rms: 0.05,
            duration_s: 4.0,
            nfft: 2048,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            min_mean_coherence: 0.8,
        },
        inverse: TwrInverseSpec {
            tikhonov_beta_floor: 0.05,
            coherence_floor: 0.9,
        },
        law: TwrLawSpec {
            beta: 0.5,
            max_db_per_iter: 6.0,
            ref_floor_db: 50.0,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            initial_drive_backoff_db: 3.0,
            min_masked_energy_frac: 0.9,
        },
        schedule: TwrLevelSchedule {
            mode: TwrRunMode::Manual,
            table_db: vec![-12.0, -6.0, 0.0],
            gate_rms_error_pct: 10.0,
            max_iterations_per_level: 8,
            on_gate_fail: TwrGateFailPolicy::Hold,
        },
        run: TwrRunSchedule::LoopN(3),
        adapt_during_playback: false,
        limits: TwrLimitsSpec {
            envelope_alarm_frac: 0.25,
            envelope_abort_frac: 0.5,
            envelope_debounce_samples: 8,
            response_rms_limit: Some(12.0),
            drive_clip: Some(4.5),
            drive_rms_limit: Some(1.5),
            max_drive_vpk: Some(DEFAULT_TWR_MAX_DRIVE_VPK),
            divergence_n: 3,
            divergence_tol_frac: 0.10,
            feasibility: TwrFeasibilityCeilings {
                peak_accel: Some(50.0),
                peak_velocity_ips: Some(70.0),
                disp_pkpk_in: Some(1.0),
            },
        },
        kinematics_hp_hz: 5.0,
        review_after_sysid: true,
        metadata: vec![TwrMetaField {
            key: "run_number".into(),
            label: "Run Number".into(),
            value: "7".into(),
        }],
    }
}

/// Every verb in the group, in declaration order. The exhaustive matches in `twr.rs`
/// force a new verb to answer `is_mutating` and `action_kind` to compile; this list is
/// what forces it to be round-tripped and tabled here too.
fn every_twr_command() -> Vec<TwrCommand> {
    vec![
        TwrCommand::IterateOnce,
        TwrCommand::BeginPlayback,
        TwrCommand::StepLevel { up: true },
        TwrCommand::JumpLevel { index: 2 },
        TwrCommand::FreezeAdaptation { frozen: true },
        TwrCommand::Reset,
        TwrCommand::BeginReference {
            name: "drop_test_z.csv".into(),
            fs_hz: 8192.0,
            eu: "g".into(),
            total_samples: 1_048_576,
            sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
        },
        TwrCommand::ReferenceChunk {
            seq: 0,
            samples_b64_f32le: "AACAPwAAAEA=".into(),
        },
        TwrCommand::CommitReference {},
        TwrCommand::ReferenceInfo,
        TwrCommand::History,
    ]
}

// =====================================================================================
// 1 + 2: the version gate, and the schema that deliberately did NOT move
// =====================================================================================

/// The bump landed, and EVERY older peer is refused - not just the previous one.
#[test]
fn the_twr_payloads_ride_a_bumped_version() {
    assert_eq!(PROTOCOL_VERSION, 6, "TWR-on-NI, wire bump #5");
    for old in 1..PROTOCOL_VERSION {
        let err = hertzforge_protocol::envelope::check_version(old).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::VersionMismatch {
                ours: PROTOCOL_VERSION,
                theirs: old,
            },
            "a v{old} peer must be refused at the gate, not part-way through a run"
        );
    }
}

/// **`PROFILE_SCHEMA` stays 1 across a THIRD bump, and a stored Random profile proves
/// it.**
///
/// The number is one global exact-match value shared by every `TestProfile` variant, so
/// bumping it to mark the new TWR variant would refuse the deployed application's
/// stored Random and Explorer profiles - documents that did not change - mid-workflow.
///
/// RED-MUTATION: set `PROFILE_SCHEMA = 2` and the first assertion reds, which is
/// exactly the deployed-profile refusal it is here to prevent.
#[test]
fn the_profile_schema_stays_one_and_the_twr_profile_defaults_to_it() {
    assert_eq!(
        PROFILE_SCHEMA, 1,
        "the v6 bump must not move the profile schema"
    );

    // A TWR profile written with no `schema_version` field takes the current one.
    let bare = r#"{"twr":{
        "reference_sha":"abc","prep":{"auto_resample":true,"scale":1.0},
        "pad_pre":0,"pad_post":0,
        "sysid":{"excitation":"random","drive_rms":0.05,"duration_s":4.0,"nfft":2048,
                 "f_lo_hz":20.0,"f_hi_hz":2000.0,"min_mean_coherence":0.8},
        "inverse":{"tikhonov_beta_floor":0.05,"coherence_floor":0.9},
        "law":{"beta":0.5,"max_db_per_iter":6.0,"ref_floor_db":50.0,"f_lo_hz":20.0,
               "f_hi_hz":2000.0,"initial_drive_backoff_db":0.0,
               "min_masked_energy_frac":0.9},
        "schedule":{"mode":"auto","table_db":[0.0],"gate_rms_error_pct":10.0,
                    "max_iterations_per_level":8,"on_gate_fail":"abort"},
        "run":"once","adapt_during_playback":false,
        "limits":{"envelope_alarm_frac":0.25,"envelope_abort_frac":0.5,
                  "envelope_debounce_samples":8,"divergence_n":3,
                  "divergence_tol_frac":0.1,"feasibility":{}},
        "kinematics_hp_hz":5.0}}"#;
    let decoded: TestProfile = serde_json::from_str(bare).expect("the TWR profile must decode");
    assert_eq!(decoded.schema_version(), PROFILE_SCHEMA);
    // The same literal carries no `review_after_sysid`, which is the other absent-field
    // default the v6 surface ships: it arrives HOLDING.
    match &decoded {
        TestProfile::Twr(p) => assert!(
            p.review_after_sysid,
            "a profile that predates the review flag must arrive holding, not running"
        ),
        other => panic!("the literal is a TWR profile: {other:?}"),
    }
    assert!(Command::SetProfile { profile: decoded }
        .check_schemas()
        .is_ok());

    // ANTI-VACUITY: a profile from the FUTURE is still refused, so the pass above is
    // about the number being unchanged and not about the gate having been removed.
    let mut future = twr();
    future.schema_version = PROFILE_SCHEMA + 1;
    assert!(matches!(
        Command::SetProfile {
            profile: TestProfile::Twr(Box::new(future)),
        }
        .check_schemas()
        .unwrap_err(),
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::Profile,
            ..
        }
    ));

    // ... and a v4-era Random document still configures, which is the compatibility row
    // the decision exists for.
    let v4_era = r#"{"random":{"schema_version":1,"breakpoints":[],"schedule":[],
        "ladder":"auto","duration_s":null}}"#;
    let old: TestProfile = serde_json::from_str(v4_era).expect("a v4-era profile must decode");
    assert!(Command::SetProfile { profile: old }.check_schemas().is_ok());
}

// =====================================================================================
// 3 + 4: the profile on the wire
// =====================================================================================

/// The whole profile survives a round trip with every field intact, and it declares its
/// own mode.
#[test]
fn the_twr_profile_round_trips_fully_populated_and_declares_its_mode() {
    let profile = TestProfile::Twr(Box::new(twr()));
    assert_eq!(
        profile.mode(),
        TestMode::Twr,
        "TWR is its OWN mode, not Shock"
    );

    let text = serde_json::to_string(&profile).expect("serialize");
    let back: TestProfile = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, profile, "{text}");

    // The reference SAMPLES are not in here and must never be: the record crosses
    // through the upload group and this profile names it by digest. A profile that grew
    // a sample array would put megabytes on the command path, and these two rows are
    // what notice.
    //
    // Asserted as "no samples ARRAY" plus a size bound rather than "the substring
    // `samples` is absent": `envelope_debounce_samples` is a legitimate scalar with that
    // word in it, and a substring check that reds on a real field is a check that gets
    // deleted rather than fixed.
    assert!(
        !text.contains("\"samples\":["),
        "the profile must not carry a reference sample array: {text}"
    );
    assert!(
        text.len() < 4096,
        "a fully-populated TWR profile is a page of scalars ({} bytes); anything near a \
         record's size means the samples got in",
        text.len()
    );
    assert!(text.contains("reference_sha"), "{text}");

    // `deny_unknown_fields` is the unknown-field diagnostic, and it is on the profile
    // family throughout - a stray field is a NAMED decode failure, never an ignore.
    let stray = text.replace("\"pad_pre\":512", "\"pad_pre\":512,\"pad_middle\":7");
    let err = serde_json::from_str::<TestProfile>(&stray).unwrap_err();
    assert!(err.to_string().contains("pad_middle"), "{err}");
}

/// **A CLEARED drive ceiling crosses as cleared.**
///
/// `max_drive_vpk: None` is the operator having removed their own authorization, and
/// the core refuses to run without one. The field is an `Option` precisely so that
/// state is expressible; a plain `f64` (which is what the shock profiles carry) would
/// turn a cleared ceiling into 5.0 V on the wire and a refusal the operator was owed
/// into a run.
///
/// RED-MUTATION: change the field to `f64` with a `5.0` default and the first assertion
/// cannot even be written.
#[test]
fn a_cleared_drive_ceiling_survives_the_wire_and_an_absent_one_defaults_safe() {
    let mut cleared = twr();
    cleared.limits.max_drive_vpk = None;
    let text = serde_json::to_string(&cleared).unwrap();
    let back: TwrProfile = serde_json::from_str(&text).unwrap();
    assert_eq!(
        back.limits.max_drive_vpk, None,
        "a cleared ceiling must arrive cleared, not defaulted: {text}"
    );

    // An ABSENT field is the other case, and it takes the SAFE value rather than the
    // cleared one: a stored profile written before the field existed gets a ceiling.
    let absent = text.replace("\"max_drive_vpk\":null,", "");
    let defaulted: TwrProfile = serde_json::from_str(&absent).expect("the field is defaulted");
    assert_eq!(
        defaulted.limits.max_drive_vpk,
        Some(DEFAULT_TWR_MAX_DRIVE_VPK),
        "an omitted ceiling defaults to the safe number, not to cleared: {absent}"
    );
}

// =====================================================================================
// 5 + 6: the command group
// =====================================================================================

/// Every verb round-trips inside a `Command`, and none of them carries a versioned
/// document (so all of them clear `check_schemas`).
#[test]
fn every_twr_command_round_trips_and_clears_the_schema_gate() {
    for verb in every_twr_command() {
        let cmd = Command::Twr(verb.clone());
        let text = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back, cmd, "{text}");
        assert!(cmd.check_schemas().is_ok(), "{text}");
    }
}

/// **The per-verb `is_mutating` table, spelled out.**
///
/// The three UPLOAD verbs are MUTATING even though they move no shaker: they write the
/// document the next run replicates, so an Observer that could stage a reference could
/// change what the Controller's next arm plays. The two QUERIES are not, so an Observer
/// can read the manifest and the convergence history - which is most of what an
/// observer is for.
#[test]
fn the_mutating_table_is_per_verb_and_the_uploads_are_gated() {
    for (verb, want) in [
        (TwrCommand::IterateOnce, true),
        (TwrCommand::BeginPlayback, true),
        (TwrCommand::StepLevel { up: false }, true),
        (TwrCommand::JumpLevel { index: 0 }, true),
        (TwrCommand::FreezeAdaptation { frozen: false }, true),
        (TwrCommand::Reset, true),
        (
            TwrCommand::BeginReference {
                name: "r".into(),
                fs_hz: 8192.0,
                eu: "g".into(),
                total_samples: 1,
                sha256: "00".into(),
            },
            true,
        ),
        (
            TwrCommand::ReferenceChunk {
                seq: 0,
                samples_b64_f32le: String::new(),
            },
            true,
        ),
        (TwrCommand::CommitReference {}, true),
        (TwrCommand::ReferenceInfo, false),
        (TwrCommand::History, false),
    ] {
        assert_eq!(verb.is_mutating(), want, "{verb:?}");
        // And the general gate agrees with the group's own answer - the Observer check
        // consults `Command::is_mutating`, so a group whose table the wrapper did not
        // consult would be a hole nothing else notices.
        assert_eq!(Command::Twr(verb.clone()).is_mutating(), want, "{verb:?}");
    }
}

/// **`action_kind` MAPS onto existing rows and never invents one.**
///
/// The `ActionMatrix` has nine rows and gains none at v6 - every literal of it in the
/// tree would otherwise grow a field for a verb with identical support semantics. The
/// two read-only queries answer `None`, which is how they stay unmatrixed.
#[test]
fn action_kinds_map_onto_existing_rows_and_the_queries_claim_none() {
    use hertzforge_protocol::ActionKind;
    assert_eq!(
        TwrCommand::IterateOnce.action_kind(),
        Some(ActionKind::Start)
    );
    assert_eq!(
        TwrCommand::BeginPlayback.action_kind(),
        Some(ActionKind::Start)
    );
    assert_eq!(
        TwrCommand::StepLevel { up: true }.action_kind(),
        Some(ActionKind::SetLevel)
    );
    assert_eq!(
        TwrCommand::JumpLevel { index: 0 }.action_kind(),
        Some(ActionKind::SetLevel)
    );
    assert_eq!(
        TwrCommand::FreezeAdaptation { frozen: true }.action_kind(),
        Some(ActionKind::AdjustLevel)
    );
    assert_eq!(TwrCommand::Reset.action_kind(), Some(ActionKind::Stop));
    assert_eq!(
        TwrCommand::CommitReference {}.action_kind(),
        Some(ActionKind::SetProfile),
        "staging the record a profile names is profile authority"
    );
    assert_eq!(TwrCommand::ReferenceInfo.action_kind(), None);
    assert_eq!(TwrCommand::History.action_kind(), None);

    // The general wrapper forwards, rather than answering for the group.
    assert_eq!(
        Command::Twr(TwrCommand::Reset).action_kind(),
        Some(ActionKind::Stop)
    );
}

/// **`CommitReference` is an OBJECT on the wire, not a bare string.**
///
/// A unit variant serializes as `"commit_reference"`; a struct variant with no fields
/// serializes as `{"commit_reference":{}}`. The two are different wire shapes, and the
/// difference is the whole reason for the empty braces: giving the verb a field later
/// would otherwise change a string into an object mid-version, which is a decode
/// failure on every peer that got past the gate.
///
/// RED-MUTATION: drop the `{}` from the variant declaration and this reds.
#[test]
fn commit_reference_is_an_object_so_a_future_field_is_additive() {
    let text = serde_json::to_string(&Command::Twr(TwrCommand::CommitReference {})).unwrap();
    assert_eq!(text, r#"{"twr":{"commit_reference":{}}}"#, "{text}");
    assert!(
        serde_json::from_str::<Command>(r#"{"twr":"commit_reference"}"#).is_err(),
        "the bare-string form must NOT decode - it is the shape this variant avoids"
    );
}

/// The chunk payload is BASE64 TEXT, not a JSON number array.
///
/// Inbound binary frames are refused by design at this seam, and a million-element JSON
/// float array is roughly twenty times the bytes of the base64 it replaces. This row
/// pins the encoding so a well-meaning "make it readable" change reds.
#[test]
fn reference_chunks_cross_as_base64_text() {
    let text = serde_json::to_string(&TwrCommand::ReferenceChunk {
        seq: 4,
        samples_b64_f32le: "AACAPw==".into(),
    })
    .unwrap();
    assert!(text.contains(r#""samples_b64_f32le":"AACAPw==""#), "{text}");
    assert!(text.contains(r#""seq":4"#), "{text}");
}

// =====================================================================================
// 7 + 8: the capability row and the document kind
// =====================================================================================

/// **`TestMode::Twr` exists, `ModeMatrix` grew the row, and the baseline claims
/// NOTHING.**
///
/// The mirror exception (`hertzforge_hal::NativeMode` has no `Twr` and will not get one)
/// changes where the evidence for a claim comes from - a bench ledger rather than a
/// device's own declaration - and changes nothing about the order: evidence first, then
/// the claim. So the default row is `Unsupported`, exactly like every other.
#[test]
fn the_twr_mode_row_exists_and_the_baseline_still_claims_nothing() {
    assert_eq!(TestMode::ALL.len(), 7);
    assert!(TestMode::ALL.contains(&TestMode::Twr));

    // TWR does NOT close a per-tick loop: its law runs BETWEEN plays. Claiming it here
    // would demand a proven-Shared clock for a mode that does not need one.
    assert!(
        !TestMode::Twr.closes_loop_on_host(),
        "TWR corrects between plays, not per tick"
    );

    let m = ModeMatrix::default();
    assert!(!m.support(TestMode::Twr).is_supported());
    assert!(matches!(
        m.support(TestMode::Twr).reason(),
        Some(UnsupportedReason::NoNativeMode { requested: None, proven }) if proven.is_empty()
    ));

    // `support_mut` reaches the new row, so a constructor iterating `TestMode::ALL`
    // fills it rather than leaving it silently unclaimed while claiming to be
    // exhaustive.
    let mut filled = ModeMatrix::all_unsupported();
    *filled.support_mut(TestMode::Twr) = ModeSupport::hosted();
    assert!(filled.twr.is_supported());
    assert!(filled.support(TestMode::Twr).is_supported());

    // And it survives the wire, which is the point of the row existing at all.
    let text = serde_json::to_string(&filled).unwrap();
    let back: ModeMatrix = serde_json::from_str(&text).unwrap();
    assert_eq!(back, filled, "{text}");
}

/// The TWR document kind checks EXACTLY, and `Response::Twr` round-trips.
#[test]
fn the_twr_document_kind_checks_exactly_and_the_response_round_trips() {
    assert_eq!(TWR_REFERENCE_SCHEMA, 1);
    assert_eq!(SchemaKind::TwrReference.current(), TWR_REFERENCE_SCHEMA);
    assert!(SchemaKind::TwrReference.check(TWR_REFERENCE_SCHEMA).is_ok());
    let err = SchemaKind::TwrReference
        .check(TWR_REFERENCE_SCHEMA + 1)
        .unwrap_err();
    assert!(matches!(
        err,
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::TwrReference,
            ..
        }
    ));

    let doc = Doc::new(
        SchemaKind::TwrReference,
        r#"{"sha256":"9f86","samples":4096,"fs_hz":8533.333}"#,
    );
    assert_eq!(doc.schema_version, TWR_REFERENCE_SCHEMA);
    let resp = Response::Twr(doc);
    let text = serde_json::to_string(&resp).unwrap();
    let back: Response = serde_json::from_str(&text).unwrap();
    assert_eq!(back, resp, "{text}");
}

/// A sanity row on the OTHER profiles: adding a variant must not have moved anyone
/// else's discriminant meaning. Externally-tagged serde is name-keyed rather than
/// index-keyed, so this is cheap insurance rather than a real risk - but the cost of
/// being wrong is every stored profile decoding as the wrong test.
#[test]
fn the_new_variant_did_not_disturb_the_existing_profile_names() {
    let random = TestProfile::Random(RandomProfile {
        schema_version: PROFILE_SCHEMA,
        breakpoints: Vec::new(),
        schedule: Vec::new(),
        ladder: LadderMode::Auto,
        duration_s: None,
        acquisition: None,
        control: None,
        limits: None,
        control_weights: None,
        composite_mode: None,
        feasibility: None,
        frf_pairs: None,
    });
    let text = serde_json::to_string(&random).unwrap();
    assert!(text.starts_with(r#"{"random":"#), "{text}");
    let twr_text = serde_json::to_string(&TestProfile::Twr(Box::new(twr()))).unwrap();
    assert!(twr_text.starts_with(r#"{"twr":"#), "{twr_text}");
}
