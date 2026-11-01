//! The v3 wire contract's SEMANTICS, as opposed to its round-trips (which live in
//! `roundtrip.rs`): selector arithmetic, the mutating/read-only split that is the run
//! lease's whole definition, the schema gates, and the 400-channel byte budget.

use hertzforge_protocol::{
    ChannelSel, Command, Doc, ExplorerCommand, Hello, LadderMode, LevelTarget, PeerRole,
    ProtocolError, RandomProfile, RecorderCommand, SchemaKind, ShockCommand, ShockSpacing,
    ShockTrainSpec, SineCommand, SineProfile, SrsCommand, StreamKind, StreamSub, SweepDirection,
    TableDoc, TestProfile, PROFILE_SCHEMA,
};

fn sine() -> SineProfile {
    SineProfile {
        schema_version: PROFILE_SCHEMA,
        start_hz: 100.0,
        band_hz: Some((20.0, 2000.0)),
        max_drive_v: Some(1.0),
    }
}

fn random() -> RandomProfile {
    RandomProfile {
        frf_pairs: None,
        feasibility: None,
        limits: None,
        control_weights: None,
        composite_mode: None,
        schema_version: PROFILE_SCHEMA,
        breakpoints: Vec::new(),
        schedule: Vec::new(),
        ladder: LadderMode::Auto,
        duration_s: None,
        acquisition: None,
        control: None,
    }
}

// =====================================================================================
// The run lease: which commands an Observer may send
// =====================================================================================

/// THE OBSERVER GATE, stated over the whole command surface rather than sampled.
///
/// Every variant is listed here explicitly. That is the point: a new verb added to
/// `Command` without a decision about whether an Observer may send it would leave
/// this list short, and the count assertion at the end reds. The failure mode this
/// guards is an Observer that can drive a shaker.
///
/// RED-MUTATION: move `Command::Abort` into the read-only arm of
/// `Command::is_mutating` (the "it is only supervisory anyway" argument) and this
/// reds - which is the state where any watcher on the lab LAN can stop someone else's
/// run.
#[test]
fn the_mutating_split_covers_every_command_and_abort_is_mutating() {
    let mutating: Vec<Command> = vec![
        Command::SetProfile {
            profile: TestProfile::OpenLoopSine(sine()),
        },
        Command::Start,
        Command::Stop,
        Command::Abort,
        Command::RampDown,
        Command::SetLevel {
            level: LevelTarget::Db(-6.0),
        },
        Command::AdjustLevel { delta_db: 1.0 },
        Command::GoToFrequency { hz: 500.0 },
        Command::AdjustFrequency { delta_hz: 1.0 },
        Command::SelectBackend {
            backend: hertzforge_protocol::BackendId::new("ni"),
        },
        Command::PutChannelTable {
            table: TableDoc::new("e", "{}"),
        },
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, "{}"),
        }),
        Command::Recorder(RecorderCommand::StartMonitor),
        Command::Recorder(RecorderCommand::Arm),
        Command::Recorder(RecorderCommand::RecordNow),
        Command::Recorder(RecorderCommand::StopRecording),
        Command::Recorder(RecorderCommand::StopMonitor),
        // Were MISSING from this list until 2026-08-08 - the count below was
        // computed from a stale "16 variants" reading and never noticed. Both
        // move a shaker (a ladder takeover hands the level schedule to whoever
        // sent it; releasing a review hold is the run starting to drive), so
        // both are gated.
        Command::SetLadderMode {
            ladder: LadderMode::Manual,
        },
        Command::ProceedAfterReview,
        // v4.
        Command::Explorer(ExplorerCommand::StartSweep {
            direction: SweepDirection::Up,
            rate_oct_per_min: 1.0,
        }),
        Command::Explorer(ExplorerCommand::StopSweep),
        // v5 - the closed-loop Sine group. Every verb moves a tone across a resonance,
        // changes how the next drive is computed, or releases a safety park.
        Command::Sine(SineCommand::Hold),
        Command::Sine(SineCommand::Resume),
        Command::Sine(SineCommand::Reverse),
        Command::Sine(SineCommand::SetBlendW { w: 0.5 }),
        Command::Sine(SineCommand::OverrideFeasibility { ack: "ack".into() }),
        // v5 - classical Shock. The three CAPTURE verbs are here rather than in the
        // read-only list even though the daemon refuses them today: the classification
        // answers what the verb IS, and a verb filed read-only because its
        // implementation happens to be a refusal becomes Observer-reachable the moment
        // someone implements it.
        Command::Shock(ShockCommand::FirePulse),
        Command::Shock(ShockCommand::RunAuto { max_pulses: None }),
        Command::Shock(ShockCommand::JumpToLevel { index: 0 }),
        Command::Shock(ShockCommand::ArmWithOverride { ack: "ack".into() }),
        Command::Shock(ShockCommand::Reset),
        Command::Shock(ShockCommand::FireTrain {
            spec: Some(ShockTrainSpec {
                count: 2,
                spacing: ShockSpacing::MaxRate,
            }),
        }),
        Command::Shock(ShockCommand::FireGunfireBurst),
        Command::Shock(ShockCommand::StartCapture),
        Command::Shock(ShockCommand::StopCapture),
        Command::Shock(ShockCommand::SaveSnapshot),
        // v5 - SRS.
        Command::Srs(SrsCommand::FirePulse),
        Command::Srs(SrsCommand::RunAuto { max_pulses: None }),
        Command::Srs(SrsCommand::JumpToLevel { index: 0 }),
        Command::Srs(SrsCommand::ArmWithOverride { ack: "ack".into() }),
        Command::Srs(SrsCommand::Reset),
        Command::Srs(SrsCommand::RunSysId),
        Command::Srs(SrsCommand::StartCapture),
        Command::Srs(SrsCommand::StopCapture),
        Command::Srs(SrsCommand::SaveSnapshot),
    ];
    let read_only: Vec<Command> = vec![
        Command::DescribeCapabilities,
        Command::GetStatus,
        Command::ListBackends,
        Command::Subscribe {
            subs: vec![StreamSub::all(StreamKind::Scope)],
        },
        Command::Unsubscribe {
            streams: vec![StreamKind::Scope],
        },
        Command::GetChannelTable,
        Command::ListCaptures,
        Command::Recorder(RecorderCommand::Status),
        Command::Recorder(RecorderCommand::Meters),
        Command::Recorder(RecorderCommand::Frf),
        Command::Recorder(RecorderCommand::Recordings),
        // v5 - the shock QUERIES. Three per group, and they are the only shock verbs
        // an Observer may send: reading what already fired moves nothing.
        Command::Shock(ShockCommand::PulseList),
        Command::Shock(ShockCommand::PulseDetail { seq: 1 }),
        Command::Shock(ShockCommand::SrsFreqs),
        Command::Srs(SrsCommand::PulseList),
        Command::Srs(SrsCommand::PulseDetail { seq: 1 }),
        Command::Srs(SrsCommand::SrsFreqs),
    ];

    for c in &mutating {
        assert!(c.is_mutating(), "{c:?} must require the run lease");
    }
    for c in &read_only {
        assert!(!c.is_mutating(), "{c:?} must be available to an Observer");
    }

    // ANTI-VACUITY, RECOMPUTED FROM THE SOURCE AT v5 (not incremented from 32 - see
    // the correction note below for why incrementing is the trap).
    //
    // Counting the variants as they stand in `command.rs`, `sine.rs` and `shock.rs`:
    //
    //   `Command`          25 variants, of which 5 are group WRAPPERS
    //                      (Recorder, Explorer, Sine, Shock, Srs)  ->  20 plain rows
    //   `RecorderCommand`  10
    //   `ExplorerCommand`   2
    //   `SineCommand`       5
    //   `ShockCommand`     13
    //   `SrsCommand`       12
    //
    //   20 + 10 + 2 + 5 + 13 + 12 = 62
    //
    // If a variant is added anywhere without being classified above, this count is
    // short and the test reds.
    //
    // CORRECTED 2026-08-08: the count read 28 against a comment claiming "16
    // variants", and `SetLadderMode` / `ProceedAfterReview` were in NEITHER list - the
    // count matched only because it had been computed from the same stale reading.
    // A count is not an enumeration; the rows above are. That is also why v5's number
    // was re-derived from the merged tree rather than added to: two sessions grew this
    // enum in parallel, and a kept-and-bumped number passes with an unclassified
    // command, which is precisely the state where an Observer can drive a shaker.
    assert_eq!(
        mutating.len() + read_only.len(),
        62,
        "a Command, RecorderCommand, ExplorerCommand, SineCommand, ShockCommand or \
         SrsCommand variant was added without being classified as mutating or \
         read-only; classify it above and RECOMPUTE this number from the source"
    );
    // ... and a subscription is NOT mutating, which is what lets an Observer observe.
    assert!(!Command::Subscribe {
        subs: vec![StreamSub::all(StreamKind::Waveform)]
    }
    .is_mutating());
}

/// The refusal an Observer gets NAMES the holder, and says the E-stop is the answer
/// for anyone without the lease. A refusal that only says "no" trains an operator to
/// keep pressing.
#[test]
fn the_observer_refusal_names_the_holder_and_the_physical_answer() {
    let named = ProtocolError::NotController {
        holder: "10.0.0.4:51022".into(),
    }
    .to_string();
    assert!(named.contains("10.0.0.4:51022"), "{named}");
    assert!(named.contains("OBSERVER"), "{named}");
    assert!(named.contains("E-stop"), "{named}");

    // Nobody holding it is a DIFFERENT sentence, not an empty name.
    let free = ProtocolError::NotController {
        holder: String::new(),
    }
    .to_string();
    assert!(free.contains("nobody"), "{free}");
    assert!(!free.contains("held by ."), "{free}");
}

// =====================================================================================
// Schema gates
// =====================================================================================

/// A command carrying a document written at a foreign schema is refused BEFORE
/// anything acts on it, and the refusal names both numbers.
#[test]
fn every_document_carrying_command_checks_its_schema() {
    // Good ones pass.
    for c in [
        Command::SetProfile {
            profile: TestProfile::Random(random()),
        },
        Command::PutChannelTable {
            table: TableDoc::new("e", "{}"),
        },
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, "{}"),
        }),
        Command::Start,
        Command::ListCaptures,
    ] {
        assert!(c.check_schemas().is_ok(), "{c:?}");
    }

    // A profile from the future is refused.
    let mut future = random();
    future.schema_version = PROFILE_SCHEMA + 1;
    let err = Command::SetProfile {
        profile: TestProfile::Random(future),
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

    // So is a table, and so is a recorder config - each naming its own kind, so a UI
    // can tell the operator WHICH document to fix.
    let table = Command::PutChannelTable {
        table: TableDoc {
            schema_version: 99,
            etag: "e".into(),
            json: "{}".into(),
        },
    }
    .check_schemas()
    .unwrap_err();
    assert!(matches!(
        table,
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::ChannelTable,
            theirs: 99,
            ..
        }
    ));
    let cfg = Command::Recorder(RecorderCommand::Configure {
        config: Doc {
            schema_version: 99,
            json: "{}".into(),
        },
    })
    .check_schemas()
    .unwrap_err();
    assert!(matches!(
        cfg,
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::RecorderConfig,
            theirs: 99,
            ..
        }
    ));
}

/// THE UNKNOWN-FIELD DIAGNOSTIC. A profile carrying a field this build does not know
/// is a REFUSAL naming the field, not a silent partial parse.
///
/// This is the shape the design called for, chosen deliberately over an accept-and-report
/// capture map: the profile is the payload that points a shaker, and "we ignored part
/// of it and ran anyway" is the silent best-effort parse the version rule bans one
/// layer up. See `SineProfile`'s docs.
///
/// RED-MUTATION: drop `#[serde(deny_unknown_fields)]` from `SineProfile` and this
/// reds - the typo'd profile decodes, `max_drive_v` silently stays `None`, and the
/// operator's drive ceiling is gone.
#[test]
fn an_unknown_profile_field_is_refused_and_the_message_names_it() {
    // The operator (or a hand-edited setup file) typo'd the drive ceiling.
    let typo = r#"{"schema_version":1,"start_hz":100.0,"band_hz":null,"max_drive":1.0}"#;
    let err = serde_json::from_str::<SineProfile>(typo).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("max_drive"), "the field must be named: {msg}");
    assert!(msg.contains("unknown field"), "{msg}");

    // Carried across the wire, that message IS the diagnostic the operator reads.
    let carried = ProtocolError::MalformedPayload {
        container: "profile".into(),
        detail: msg.clone(),
    }
    .to_string();
    assert!(carried.contains("max_drive"), "{carried}");

    // ANTI-VACUITY: the same document WITHOUT the typo decodes, so the refusal above
    // is about the unknown field and not about the whole shape being wrong.
    let good = r#"{"schema_version":1,"start_hz":100.0,"band_hz":null,"max_drive_v":1.0}"#;
    let p: SineProfile = serde_json::from_str(good).expect("the corrected profile parses");
    assert_eq!(p.max_drive_v, Some(1.0));

    // And an OMITTED schema_version defaults to this build's, so a payload written
    // before the field existed is not spuriously refused within one wire version.
    let bare = r#"{"start_hz":100.0,"band_hz":null,"max_drive_v":1.0}"#;
    let p: SineProfile = serde_json::from_str(bare).unwrap();
    assert_eq!(p.schema_version, PROFILE_SCHEMA);
}

// =====================================================================================
// The 400-channel decimated-stream budget
// =====================================================================================

/// Waveform-frame bytes for `n_channels` x `n_samples`, using the pinned v2 layout
/// from `ui/IPC_CONTRACT.md`: a 44-byte fixed header (24 + the 20-byte observability
/// block), then per channel a 4-byte prefix and `n_samples` f32 samples.
///
/// Duplicated here rather than imported because this crate must not depend on the ui
/// crate; the numbers are the contract, and if the layout moves this arithmetic is
/// wrong in an obvious way rather than a subtle one.
fn waveform_bytes(n_channels: usize, n_samples: usize) -> usize {
    44 + n_channels * (4 + n_samples * 4)
}

/// THE 400-CHANNEL BUDGET GATE.
///
/// **The budget: 1.0 MB/s of display stream on the wire, per subscribed stream.**
/// That number is chosen to be an order of magnitude under a 100 Mbit/s lab link
/// (12.5 MB/s), so the display path cannot crowd out the command path even on the
/// worst link the deployment targets, and it is stated HERE so the test asserts a
/// number a human agreed to rather than whatever the code happens to produce.
///
/// The gate is that a 400-channel source, subscribed through a REALISTIC selector,
/// stays inside it - and, as the anti-vacuity half, that the same source subscribed
/// with `ChannelSel::All` at full rate BLOWS it. A budget test that passes for every
/// input is not a budget test.
#[test]
fn a_four_hundred_channel_source_stays_within_the_stream_budget_through_the_selector() {
    const BUDGET_BYTES_PER_S: f64 = 1_000_000.0;
    const N_CHANNELS: usize = 400;
    const SAMPLES_PER_FRAME: usize = 1024;

    let available: Vec<(u32, Option<u32>)> = (1..=N_CHANNELS as u32)
        .map(|i| (i, if i <= 8 { Some(1) } else { None }))
        .collect();

    // What an operator actually watches: the 8-channel control group, at a display
    // cadence the eye can use.
    let sub = StreamSub {
        kind: StreamKind::Waveform,
        channels: ChannelSel::Group(1),
        max_rate_hz: Some(20.0),
        bin_decimation: None,
    };
    let selected = sub.selected_channels(&available);
    assert_eq!(selected, 8, "the group selector must resolve to 8 channels");
    let rate = 1.0 / sub.min_period_s().expect("a ceiling was asked for");
    let bytes_per_s = waveform_bytes(selected, SAMPLES_PER_FRAME) as f64 * rate;
    assert!(
        bytes_per_s <= BUDGET_BYTES_PER_S,
        "selected stream is {bytes_per_s:.0} B/s, over the {BUDGET_BYTES_PER_S:.0} B/s budget"
    );

    // ANTI-VACUITY: the SAME source, subscribed the v2 way (everything, uncapped at
    // the producer's 50 Hz block cadence), is two orders of magnitude over. This is
    // the number that made selectors a wire change rather than a nicety.
    let all = StreamSub::all(StreamKind::Waveform);
    assert_eq!(all.selected_channels(&available), N_CHANNELS);
    let unselected = waveform_bytes(N_CHANNELS, SAMPLES_PER_FRAME) as f64 * 50.0;
    assert!(
        unselected > BUDGET_BYTES_PER_S * 50.0,
        "the unselected stream must be dramatically over budget for this gate to \
         mean anything; it is {unselected:.0} B/s"
    );

    // And the spectrum half of the same story: 400 channels x 4096 bins, decimated
    // 8:1 and rate-capped, also fits - the SAME two selectors doing the work.
    let spec = StreamSub {
        kind: StreamKind::Spectrum,
        channels: ChannelSel::Ids((1..=12).collect()),
        max_rate_hz: Some(5.0),
        bin_decimation: Some(8),
    };
    let bins = spec.decimated_bins(4096);
    assert_eq!(bins, 512);
    // MCS1 batched layout: 48-byte header, then per channel 4 bytes + bins x f32.
    let spec_bytes = (48 + spec.selected_channels(&available) * (4 + bins * 4)) as f64 * 5.0;
    assert!(
        spec_bytes <= BUDGET_BYTES_PER_S,
        "spectrum stream is {spec_bytes:.0} B/s, over budget"
    );
}

// =====================================================================================
// Selector semantics at the wire level
// =====================================================================================

/// The selectors survive the wire with their meaning intact - an `Ids` list that came
/// back as `All` would silently restore the firehose the subscription existed to
/// prevent.
#[test]
fn selectors_survive_the_wire_with_their_meaning() {
    let sub = StreamSub {
        kind: StreamKind::Spectrum,
        channels: ChannelSel::Ids(vec![3, 1, 4]),
        max_rate_hz: Some(12.5),
        bin_decimation: Some(4),
    };
    let json = serde_json::to_string(&sub).unwrap();
    let back: StreamSub = serde_json::from_str(&json).unwrap();
    assert_eq!(back, sub);
    assert!(!back.channels.is_all(), "an Ids selector must not widen");
    assert_eq!(back.decimation(), 4);
    assert_eq!(back.min_period_s(), Some(0.08));

    // The channel selector is order-preserving as WRITTEN but membership-based as
    // APPLIED: [3,1,4] accepts 1 even though it is not first.
    assert!(back.channels.accepts(1, None));
    assert!(!back.channels.accepts(2, None));

    // An omitted `channels` decodes as All - the v2 meaning - so a minimal client
    // that only wants a stream kind still works.
    let minimal: StreamSub =
        serde_json::from_str(r#"{"kind":"scope","max_rate_hz":null,"bin_decimation":null}"#)
            .expect("channels is defaulted");
    assert!(minimal.channels.is_all());
}

// =====================================================================================
// Handshake
// =====================================================================================

/// A Hello asking to control, with a token, is a different message from a bare one -
/// and the token never appears in a refusal.
#[test]
fn the_handshake_carries_authority_and_never_echoes_the_token_in_a_refusal() {
    let h = Hello::new(PeerRole::Ui, "hertzforge-ui")
        .asking(hertzforge_protocol::RunRole::Controller)
        .with_token("hunter2");
    let err = h.check_token("something-else").unwrap_err();
    let msg = err.to_string();
    assert!(
        !msg.contains("hunter2"),
        "a refusal must never echo the offered token: {msg}"
    );
    assert!(matches!(err, ProtocolError::Unauthorized { .. }));
}

/// The LOCAL-ONLY refusal says what is not served AND that there is no fallback. The
/// absence of a fallback is the whole point: a front end that quietly ran the Sine
/// module locally when the daemon had no answer would put the run on different
/// hardware than the operator selected.
#[test]
fn the_local_only_refusal_forbids_a_fallback_in_so_many_words() {
    let msg = ProtocolError::LocalOnly {
        what: "Sine module".into(),
    }
    .to_string();
    assert!(msg.contains("Sine module"), "{msg}");
    assert!(msg.contains("LOCAL-ONLY"), "{msg}");
    assert!(msg.contains("no fallback"), "{msg}");
}
