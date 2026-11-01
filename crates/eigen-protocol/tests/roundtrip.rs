//! Serde round-trips for every envelope type, plus the two wire-level properties the
//! seam promises: packed display frames cross bit-for-bit, and the version field is
//! decodable ahead of the payload.

use std::time::Duration;

use eigen_protocol::{
    ActionSupport, ActiveIntent, BackendId, BackendInfo, BackendTier, Basis, CapabilityDescriptor,
    CaptureInfo, ChannelSel, ClockDomain, Command, CommandEnvelope, Completion, Doc,
    EngineeringUnit, Envelope, Event, EventEnvelope, FrameBytes, Hello, IntentId, IntentOutcome,
    LadderMode, LeaseAnswer, LeaseId, LevelTarget, LinkState, LoopOwner, ModeSupport,
    ObserverReason, PeerRole, Progress, ProtocolError, PsdBreakpoint, PsdLevel, RandomProfile,
    RecorderCommand, Response, ResponseEnvelope, RunRole, RunState, ScheduleEntry, SchemaKind,
    StatusReport, StreamKind, StreamSub, TableDoc, TestProfile, VersionHeader, PROFILE_SCHEMA,
    PROTOCOL_VERSION,
};

fn roundtrip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let json = serde_json::to_string(value).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

// =====================================================================================
// v5 profile fixtures
// =====================================================================================
//
// Deliberately NOT the same shapes as `v5.rs`'s fixtures. That file populates the
// common arms (Sweep mode, Breakpoints target, empty limit/monitor/metadata lists);
// these populate the OTHER ones - Dwell, a Flat SRS target, a present gunfire spec, a
// non-empty monitor list, a shared limit profile, real feasibility inputs and real
// metadata. Two corpora covering disjoint arms is what makes "every type round-trips"
// a statement about the enum rather than about one path through it.

fn sine_wire_profile() -> eigen_protocol::SineWireProfile {
    use eigen_protocol::*;
    let table = SineControlProfile {
        label: "limit".into(),
        rows: vec![
            SineProfileRow {
                freq_hz: 10.0,
                quantity: SineQuantity::VelocityIpsPk,
                value: 20.0,
                alarm_db: None,
                abort_db: None,
            },
            SineProfileRow {
                freq_hz: 500.0,
                quantity: SineQuantity::AccelGPk,
                value: 5.0,
                alarm_db: Some((2.0, 2.0)),
                abort_db: Some((4.0, 4.0)),
            },
        ],
        global_alarm_db: (1.5, 1.5),
        global_abort_db: (3.0, 3.0),
    };
    SineWireProfile {
        schema_version: PROFILE_SCHEMA,
        profile: table.clone(),
        // The DWELL arm, and a CYCLES duration - the two the v5.rs fixture does not
        // carry.
        mode: SineModeSpec::Dwell(SineDwellPlan {
            freq_hz: 57.5,
            dwell_for: SineDwellFor::Cycles(20_000),
        }),
        control: SineControlSet {
            channels: vec![SineControlChannel {
                index: 3,
                weight: 1.0,
            }],
            mode: SineCompositeMode::Max,
        },
        // A SHARED limit table: the `shared_name` Some arm, and the by-value sharing
        // rule (the resolved profile travels, not a registry reference).
        limit_profiles: vec![SineLimitProfile {
            channel_index: 4,
            profile: table,
            shared_name: Some("fixture-limit".into()),
        }],
        blend_w: 1.0,
        compression: SineCompression::Slow,
        tracking_bw: SineBandwidthMode::FixedHz(10.0),
        feedforward: SineFeedforwardConfig {
            coherence_floor: 0.6,
            h_floor_frac: 0.02,
        },
        notch: SineNotchConfig {
            drive_floor: 0.02,
            smoothing_ticks: 5.0,
            release_ramp_db_per_s: 12.0,
        },
        ladder: SineLadderConfig {
            sysid_level_db: -20.0,
            start_db: -6.0,
            table_db: vec![-6.0, 0.0],
            per_rung: SineRungFor::Seconds(30.0),
            full_for: SineRungFor::Sweeps(2),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.9,
        drive_vpk_ceiling: 0.5,
        drive_vpk_abort: 0.75,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: SineCaptureConfig {
            rule: SineCaptureRule::EntireTest,
            level_settle_s: 2.0,
            periodic_save_s: None,
        },
        cola: SineColaConfig {
            enabled: false,
            volts_pk: 1.0,
            waveform: SineColaWaveform::DcRef,
        },
        // The feasibility Some arm, shared with Random - one type, one meaning.
        feasibility: Some(FeasibilityInputs {
            shaker_json: r#"{"model":"T2000"}"#.into(),
            armature_lb: 120.0,
            slip_table_lb: 400.0,
            drive_adapter_lb: 25.0,
            fixturing_lb: 60.0,
            test_article_lb: 15.0,
        }),
        control_tick_frames: 128,
        fs_hz: None,
        review_hold: false,
        metadata: vec![SineMetaField {
            key: "run_number".into(),
            value: "7".into(),
        }],
    }
}

fn classical_shock_profile() -> eigen_protocol::ClassicalShockProfile {
    use eigen_protocol::*;
    ClassicalShockProfile {
        schema_version: PROFILE_SCHEMA,
        spec: ShockPulseSpec {
            // The simple shape arm, against v5.rs's Trapezoid.
            shape: ShockPulseShape::HalfSine,
            amplitude_g: 20.0,
            duration_ms: 11.0,
            compensation: ShockCompensation {
                shape: ShockCompensationShape::RoundedRectangular,
                pre_frac: 1.25,
                post_frac: 1.75,
                symmetric: false,
            },
        },
        fs_hz: 5120.0,
        standard: ShockToleranceStandard::Iec60068_2_27,
        schedule: ShockLevelSchedule {
            mode: ShockRunMode::Manual,
            start_db: -6.0,
            table_db: vec![-6.0, -3.0, 0.0],
            // A PER-LEVEL pulse count, not the single-value shorthand.
            pulses_per_level: vec![3, 3, 5],
        },
        train: ShockTrainSpec {
            count: 10,
            spacing: ShockSpacing::MaxRate,
        },
        // The gunfire Some arm.
        gunfire: Some(ShockGunfireSpec {
            rounds_per_min: 600.0,
            rounds: 12,
        }),
        rate_model: ShockRateModel {
            cooling_floor_s: 1.0,
            settling_floor_s: 0.5,
            compute_floor_s: 0.25,
            device_recompute_floor_s: 20.0,
            // The FALSE arm: a model that refuses every spacing rather than answering
            // a plausible-looking number.
            read_cadence_ok: false,
        },
        drive_limit: ShockDriveLimit {
            peak_accel_g: Some(40.0),
            monitors: vec![
                ShockDriveMonitor {
                    channel: "amp-volts".into(),
                    eu: "V".into(),
                    peak_ceiling: 8.0,
                    kind: ShockMonitorKind::Predictive,
                },
                ShockDriveMonitor {
                    channel: "table-accel".into(),
                    eu: "g".into(),
                    peak_ceiling: 55.0,
                    kind: ShockMonitorKind::BetweenPulse,
                },
            ],
        },
        feasibility: Some(FeasibilityInputs {
            shaker_json: r#"{"model":"T4000"}"#.into(),
            armature_lb: 200.0,
            slip_table_lb: 0.0,
            drive_adapter_lb: 10.0,
            fixturing_lb: 30.0,
            test_article_lb: 5.0,
        }),
        residual_velocity_tol_ips: 1.0,
        residual_disp_tol_in: 0.1,
        srs_f_lo_hz: 10.0,
        srs_f_hi_hz: 4000.0,
        srs_bands_per_octave: 6.0,
        srs_band_tol_db: 6.0,
        compute_pvss: true,
        abort_on_tolerance_failure: false,
        max_drive_vpk: 3.0,
        metadata: vec![ShockMetaField {
            key: "operator".into(),
            label: "Operator".into(),
            value: "Test Operator".into(),
        }],
    }
}

fn srs_profile() -> eigen_protocol::SrsProfile {
    use eigen_protocol::*;
    SrsProfile {
        schema_version: PROFILE_SCHEMA,
        f_lo_hz: 100.0,
        f_hi_hz: 10_000.0,
        fraction: SrsBandFraction::Arbitrary(4.5),
        // The FLAT arm, against v5.rs's Breakpoints.
        target: SrsTarget::Flat { accel_g: 500.0 },
        q: 10.0,
        engine: SrsEngineKind::Seds,
        synthesis: SrsSynthesisOpts {
            max_iters: 25,
            tol_db: 1.0,
            max_gain_db_per_iter: 2.0,
            q: 10.0,
        },
        schedule: ShockLevelSchedule {
            mode: ShockRunMode::Auto,
            start_db: -12.0,
            table_db: vec![-12.0, 0.0],
            pulses_per_level: vec![2],
        },
        // The limits Some arm, with both LimitTarget shapes and an FRF present on one.
        limits: Some(SrsLimitSet {
            channels: vec![
                SrsLimitChannel {
                    name: "fixture-corner".into(),
                    limit: SrsLimitTarget::Flat { accel_g: 300.0 },
                    frf: Some(SrsFrfSpec {
                        freqs_hz: vec![100.0, 1000.0, 10_000.0],
                        mag: vec![1.0, 2.5, 0.4],
                    }),
                },
                SrsLimitChannel {
                    name: "article-top".into(),
                    limit: SrsLimitTarget::PerBand {
                        bounds_g: vec![250.0, 300.0, 350.0],
                    },
                    frf: None,
                },
            ],
            headroom_db: 3.0,
        }),
        feasibility: None,
        feasibility_tol: SrsFeasibilityTol {
            residual_velocity_ips: 2.0,
            residual_disp_in: 0.25,
        },
        drive_limit: ShockDriveLimit {
            peak_accel_g: None,
            monitors: Vec::new(),
        },
        drive_abort_factor: 1.25,
        kinematic_abort: SrsKinematicAbort {
            accel_g: Some(800.0),
            velocity_ips: Some(100.0),
            disp_pp_in: Some(1.0),
        },
        // The PER-BAND envelope override, which the uniform values are then ignored
        // in favour of.
        envelope: SrsEnvelopeSpec {
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            per_band: Some(vec![SrsToleranceBand {
                alarm_lo_db: 2.0,
                alarm_hi_db: 3.0,
                abort_lo_db: 4.0,
                abort_hi_db: 6.0,
            }]),
        },
        // The sysID NONE arm: skipped, unity-plant seed, no predictive limiting.
        sysid: None,
        compute_pvss: true,
        ring_out_s: 1.0,
        review_after_sysid: false,
        max_drive_vpk: 10.0,
        metadata: Vec::new(),
    }
}

/// Command envelope round-trip, exercising the intent-shaped payloads (profile with
/// breakpoints, absolute + relative setpoints, subscriptions).
#[test]
fn command_envelope_roundtrips() {
    let profile = TestProfile::Random(RandomProfile {
        frf_pairs: None,
        feasibility: None,
        limits: None,
        control_weights: None,
        composite_mode: None,
        schema_version: PROFILE_SCHEMA,
        breakpoints: vec![
            PsdBreakpoint {
                freq_hz: 20.0,
                level: PsdLevel::Psd(0.01),
                alarm_db: Some((-3.0, 3.0)),
                abort_db: Some((-6.0, 6.0)),
            },
            PsdBreakpoint {
                freq_hz: 2000.0,
                level: PsdLevel::SlopeDbPerOct(-6.0),
                alarm_db: None,
                abort_db: None,
            },
        ],
        schedule: vec![
            ScheduleEntry {
                db: -12.0,
                dwell_s: Some(30.0),
            },
            ScheduleEntry {
                db: 0.0,
                dwell_s: None,
            },
        ],
        ladder: LadderMode::Auto,
        duration_s: Some(120.0),
        acquisition: None,
        control: None,
    });

    let commands = vec![
        Command::SetProfile { profile },
        Command::Start,
        Command::SetLevel {
            level: LevelTarget::Db(-6.0),
        },
        Command::SetLevel {
            level: LevelTarget::DriveVolts(0.5),
        },
        Command::AdjustLevel { delta_db: 1.0 },
        Command::GoToFrequency { hz: 500.0 },
        Command::AdjustFrequency { delta_hz: -25.0 },
        Command::RampDown,
        Command::Stop,
        Command::Abort,
        // v3 selectors: the v2-equivalent, an id list with a rate + bin ceiling, and
        // a control-group selection.
        Command::Subscribe {
            subs: vec![
                StreamSub::all(StreamKind::Scope),
                StreamSub {
                    kind: StreamKind::Spectrum,
                    channels: ChannelSel::Ids(vec![1, 2, 5]),
                    max_rate_hz: Some(10.0),
                    bin_decimation: Some(4),
                },
                StreamSub {
                    kind: StreamKind::Waveform,
                    channels: ChannelSel::Group(2),
                    max_rate_hz: None,
                    bin_decimation: None,
                },
            ],
        },
        Command::SelectBackend {
            backend: BackendId::new("delegated-device"),
        },
        Command::DescribeCapabilities,
        // v3 verbs.
        Command::GetChannelTable,
        Command::PutChannelTable {
            table: TableDoc::new("etag-7", r#"{"schemaVersion":2,"channels":[]}"#),
        },
        Command::ListCaptures,
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, r#"{"acq":{}}"#),
        }),
        Command::Recorder(RecorderCommand::StartMonitor),
        Command::Recorder(RecorderCommand::Status),
        // v5 verbs. The whole PROFILE corpus for the three new variants lives in
        // `v5.rs` (it needs fixtures); what belongs here is the envelope-level proof
        // that each group survives the externally-tagged form, including the two
        // String-carrying variants - a `Copy` derive slipped onto either group would
        // stop compiling here rather than at a call site nobody runs.
        Command::Sine(eigen_protocol::SineCommand::Hold),
        Command::Sine(eigen_protocol::SineCommand::SetBlendW { w: 0.25 }),
        Command::Sine(eigen_protocol::SineCommand::OverrideFeasibility {
            ack: "reviewed the feasibility report; proceeding".into(),
        }),
        Command::Shock(eigen_protocol::ShockCommand::FirePulse),
        Command::Shock(eigen_protocol::ShockCommand::PulseDetail { seq: 42 }),
        Command::Shock(eigen_protocol::ShockCommand::FireTrain {
            spec: Some(eigen_protocol::ShockTrainSpec {
                count: 4,
                spacing: eigen_protocol::ShockSpacing::Delay(2.5),
            }),
        }),
        Command::Shock(eigen_protocol::ShockCommand::StartCapture),
        Command::Srs(eigen_protocol::SrsCommand::RunSysId),
        Command::Srs(eigen_protocol::SrsCommand::SrsFreqs),
        // The three new PROFILES ride `SetProfile`, so the envelope carries them too.
        Command::SetProfile {
            profile: TestProfile::Sine(Box::new(sine_wire_profile())),
        },
        Command::SetProfile {
            profile: TestProfile::ShockClassical(Box::new(classical_shock_profile())),
        },
        Command::SetProfile {
            profile: TestProfile::Srs(Box::new(srs_profile())),
        },
    ];
    for (i, cmd) in commands.into_iter().enumerate() {
        let env = CommandEnvelope::new(i as u64, cmd);
        assert_eq!(env.v, PROTOCOL_VERSION);
        assert_eq!(roundtrip(&env), env);
    }
}

/// Response envelope round-trip, including the full capability descriptor (with a
/// characterized abort bound and explicit per-mode/per-action claims) and a status
/// report with in-flight intents.
#[test]
fn response_envelope_roundtrips() {
    // An honestly-shaped delegated-device descriptor: device-owned loop, ONE delegated native
    // mode, paced (converging) level/frequency actions.
    let mut caps = CapabilityDescriptor::claim_nothing(BackendId::new("delegated-device"));
    caps.loop_owner = LoopOwner::DeviceClosesLoop;
    caps.clock = ClockDomain::Independent;
    caps.sample_rate_hz = 65_536.0;
    caps.max_input_channels = 8;
    caps.max_output_channels = 1;
    caps.abort_latency_bound = Some(Duration::from_millis(150));
    // v2 two-axis shape: the device replays its native mode AND corrects.
    caps.modes.open_loop_sine = ModeSupport::delegated(eigen_protocol::TestMode::OpenLoopSine);
    caps.actions.start = ActionSupport::Instant;
    caps.actions.stop = ActionSupport::Instant;
    caps.actions.abort = ActionSupport::Instant;
    caps.actions.set_level = ActionSupport::Converging;
    caps.actions.go_to_frequency = ActionSupport::Converging;
    caps.streams = vec![StreamKind::Scope, StreamKind::Spectrum];
    assert!(
        caps.inconsistencies().is_empty(),
        "test caps must be honest"
    );

    let status = StatusReport {
        backend: Some(BackendId::new("delegated-device")),
        run: RunState::Running,
        level_db: Some(-6.0),
        frequency_hz: Some(482.5),
        measured_grms: None,
        alarm_active: false,
        abort_active: false,
        elapsed_s: Some(12.5),
        remaining_s: None,
        target_grms: Some(4.4),
        band_coherence: Some(0.99),
        module_state: Some("engaged".into()),
        ladder_mode: Some(eigen_protocol::LadderMode::Auto),
        // The request/actual PAIR, and they differ: a round trip that carried
        // one number could not catch a serializer that dropped the other.
        requested_fs_hz: Some(8192.0),
        actual_fs_hz: Some(8533.333333333334),
        // v4: the OPEN-LOOP readouts, carried BESIDE `level_db` rather than folded
        // into it - a round trip that shared one field could not catch a serializer
        // that rendered volts under a dB label. `sweep_active` is deliberately
        // `Some(false)` here and not `None`: the three-valued shape (sweeping /
        // parked / not applicable) is the contract, and a test that only ever
        // carried `None` would not exercise it.
        drive_vpk: Some(0.25),
        sweep_active: Some(false),
        // The display ring's producer-side loss. NON-ZERO here on purpose:
        // a round trip carrying 0 would pass against a serializer that dropped the field,
        // because `Option::is_none` skipping and "decoded as the default" are the same
        // bytes at zero.
        display_blocks_dropped: Some(17),
        // v5. `train_remaining` and `thd_pct` are deliberately ZERO-valued: both are
        // legal, meaningful values ("no train in flight", "no measurable distortion")
        // and a round trip that only ever carried non-zero would pass against a
        // `skip_serializing_if` reflex that treated zero as absent.
        sweeps_done: Some(4),
        thd_pct: Some(0.0),
        // Schedule progress. ZERO-valued for the same reason as the two above: 0.0 is a
        // legal reading (a run that just started) and the field must survive the trip as
        // `Some(0.0)`, not vanish into a `skip_serializing_if` that reads zero as absent.
        progress: Some(0.0),
        hold: Some("operator hold at 482.5 Hz".into()),
        notch_active: Some(true),
        // v6, the closed-loop sine pair. They DIFFER on purpose and neither is the
        // other's rounding: a round trip carrying one number, or two equal ones, would
        // pass against a serializer that wrote the target into both slots - which is
        // the exact shape of the mistake worth catching, since target-vs-measured is
        // the whole point of the pair.
        target_g: Some(5.0),
        measured_g: Some(4.87),
        pulses_fired: Some(9),
        pulses_at_level: Some(3),
        train_remaining: Some(0),
        last_pulse_seq: Some(9),
        last_pulse: Some(Doc::new(
            SchemaKind::ShockPulse,
            r#"{"seq":9,"peak_g":19.7,"pass":true}"#,
        )),
        active_intents: vec![ActiveIntent {
            intent: IntentId(7),
            action: eigen_protocol::ActionKind::GoToFrequency,
        }],
    };

    let responses = vec![
        Response::Accepted {
            completion: Completion::Immediate,
        },
        Response::Accepted {
            completion: Completion::Converging {
                intent: IntentId(7),
            },
        },
        Response::Capabilities(Box::new(caps.clone())),
        Response::Status(Box::new(status)),
        Response::Backends(vec![BackendInfo {
            id: BackendId::new("delegated-device"),
            label: "Delegated device".into(),
            vendor: "Example vendor".into(),
            tier: BackendTier::Available,
            selected: true,
            link: LinkState::Detected,
            caps: Some(caps),
        }]),
        Response::Rejected {
            error: ProtocolError::UnsupportedMode {
                mode: eigen_protocol::TestMode::Random,
            },
        },
        // v3 responses.
        Response::ChannelTable(TableDoc::new("etag-9", r#"{"schemaVersion":2}"#)),
        Response::Captures(vec![CaptureInfo {
            name: "rec_0001".into(),
            frames: 512_000,
            duration_s: 10.0,
            n_channels: 4,
            bytes: 8_192_000,
            started_unix: 1_753_400_000,
            complete: false,
            station_label: "NI-SIM (driver-simulated)".into(),
            run_id: "18446744073709551615".into(),
        }]),
        Response::Recorder(Doc::new(SchemaKind::RecorderConfig, r#"{"state":"idle"}"#)),
        // v5: the shock document answer, at each of the two new schema kinds.
        Response::Shock(Doc::new(
            SchemaKind::ShockPulse,
            r#"{"seq":9,"peak_g":19.7}"#,
        )),
        Response::Shock(Doc::new(
            SchemaKind::ShockSrsGrid,
            r#"{"freqs_hz":[20.0,25.2,31.7]}"#,
        )),
        // Every v3 error shape survives the wire - a refusal the UI cannot decode is
        // a refusal the operator never sees.
        Response::Rejected {
            error: ProtocolError::NotController {
                holder: "10.0.0.4:51022".into(),
            },
        },
        Response::Rejected {
            error: ProtocolError::StaleUpload {
                current_etag: "b".into(),
                provided_etag: "a".into(),
            },
        },
        Response::Rejected {
            error: ProtocolError::UnsupportedSchema {
                kind: SchemaKind::ChannelTable,
                ours: 2,
                theirs: 3,
            },
        },
        Response::Rejected {
            error: ProtocolError::MalformedPayload {
                container: "profile".into(),
                detail: "unknown field `dwell_sec`, expected one of ...".into(),
            },
        },
        Response::Rejected {
            error: ProtocolError::LocalOnly {
                what: "Sine module".into(),
            },
        },
        Response::Rejected {
            error: ProtocolError::Unauthorized {
                detail: "the shared token does not match".into(),
            },
        },
    ];
    for (i, resp) in responses.into_iter().enumerate() {
        let env = ResponseEnvelope::new(i as u64, resp);
        assert_eq!(roundtrip(&env), env);
    }
}

/// Event envelope round-trip: progress (both bases), every settle outcome shape,
/// state change, capability change.
#[test]
fn event_envelope_roundtrips() {
    let events = vec![
        Event::IntentProgress {
            intent: IntentId(3),
            progress: Progress {
                current: 250.0,
                target: 500.0,
                unit: EngineeringUnit::Hertz,
                basis: Basis::Measured,
                fraction: Some(0.5),
            },
        },
        Event::IntentProgress {
            intent: IntentId(4),
            progress: Progress {
                current: 0.31,
                target: 0.5,
                unit: EngineeringUnit::Volts,
                basis: Basis::Estimated, // drive is never telemetered: estimate only
                fraction: None,
            },
        },
        Event::IntentSettled {
            intent: IntentId(3),
            outcome: IntentOutcome::Completed,
        },
        Event::IntentSettled {
            intent: IntentId(4),
            outcome: IntentOutcome::Clamped {
                at: 0.45,
                reason: "operator drive ceiling".into(),
            },
        },
        Event::IntentSettled {
            intent: IntentId(5),
            outcome: IntentOutcome::Failed {
                error: ProtocolError::Disconnected,
            },
        },
        Event::StateChanged {
            status: Box::new(StatusReport::idle()),
        },
        // v3 events.
        Event::RunProvenance {
            provenance: Doc::new(
                SchemaKind::Provenance,
                r#"{"runId":"7","stationLabel":"NI-SIM (driver-simulated)"}"#,
            ),
        },
        Event::LeaseChanged {
            role: RunRole::Observer,
            lease: LeaseAnswer::Observer {
                reason: ObserverReason::Downgraded,
                controller: Some("10.0.0.4:51022".into()),
            },
        },
        Event::LeaseChanged {
            role: RunRole::Controller,
            lease: LeaseAnswer::Controller {
                lease: LeaseId(3),
                timeout_s: 30.0,
            },
        },
        Event::SafeStopping {
            reason: "controller heartbeat lost for 30 s; ramping down".into(),
        },
    ];
    for (i, ev) in events.into_iter().enumerate() {
        let env = EventEnvelope::new(i as u64, ev);
        assert_eq!(roundtrip(&env), env);
    }
}

/// THE opaque-frame property: a hand-packed little-endian frame (packed exactly the
/// way `ui/src-tauri/src/frame.rs` packs - explicit `to_le_bytes` writes, no
/// `#[repr(C)]`) crosses the envelope BIT-FOR-BIT. The protocol neither re-encodes
/// nor inspects it; producer packer and TS decoder stay the two ends of the byte
/// contract.
#[test]
fn packed_frames_cross_bit_for_bit() {
    // Pack a miniature scope-frame-shaped payload with the same byte-writer
    // discipline as the real packer (this test must not depend on the ui crate).
    let mut packed = Vec::new();
    packed.extend_from_slice(&1u32.to_le_bytes()); // n_channels
    packed.extend_from_slice(&2u32.to_le_bytes()); // n_cols
    packed.extend_from_slice(&1.5f64.to_le_bytes()); // t_end_s
    packed.extend_from_slice(&0.25f32.to_le_bytes()); // dt_s
    packed.extend_from_slice(&0f32.to_le_bytes()); // _pad
    packed.extend_from_slice(&0x0102u16.to_le_bytes()); // channel_id
    packed.extend_from_slice(&0u16.to_le_bytes()); // _pad
    packed.extend_from_slice(&0.5f32.to_le_bytes()); // rms
    packed.extend_from_slice(&2.0f32.to_le_bytes()); // peak
    for v in [-1.0f32, -2.0, 1.0, 2.0] {
        packed.extend_from_slice(&v.to_le_bytes()); // mins then maxs
    }
    let original = packed.clone();

    let env = EventEnvelope::new(
        99,
        Event::Frame {
            stream: StreamKind::Scope,
            bytes: FrameBytes::new(packed),
        },
    );
    let back = roundtrip(&env);
    let Event::Frame { stream, bytes } = back.body else {
        panic!("frame event came back as a different variant");
    };
    assert_eq!(stream, StreamKind::Scope);
    // Bit-for-bit: the exact bytes the packer emitted, untouched.
    assert_eq!(bytes.as_slice(), original.as_slice());
    // Spot-check the little-endian layout survived (not merely length): the u16
    // channel id 0x0102 at offset 24 is still 02 01 on the wire.
    assert_eq!(&bytes.as_slice()[24..26], &[0x02, 0x01]);
    assert_eq!(&bytes.as_slice()[0..4], &[1, 0, 0, 0]);
}

/// The version field is decodable ahead of the payload: a peer can probe `v` from
/// any envelope (or Hello) without understanding the rest, which is what makes
/// fail-early possible across versions.
#[test]
fn version_is_probeable_before_the_payload() {
    let env = CommandEnvelope::new(1, Command::Start);
    let json = serde_json::to_string(&env).unwrap();
    let probe: VersionHeader = serde_json::from_str(&json).unwrap();
    assert_eq!(probe.v, PROTOCOL_VERSION);

    let hello = Hello::new(PeerRole::Core, "eigen-core 0.0.0 (test)");
    let probe: VersionHeader =
        serde_json::from_str(&serde_json::to_string(&hello).unwrap()).unwrap();
    assert_eq!(probe.v, PROTOCOL_VERSION);
    assert_eq!(roundtrip(&hello), hello);
}

/// A generic envelope stamps the build's version; a doctored one fails the check
/// with both numbers named.
#[test]
fn envelope_version_check_is_exact_match() {
    let good = Envelope::new(5, Command::Stop);
    assert!(good.check_version().is_ok());

    let mut bad = good;
    bad.v = PROTOCOL_VERSION + 1;
    assert_eq!(
        bad.check_version().unwrap_err(),
        ProtocolError::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs: PROTOCOL_VERSION + 1,
        }
    );
}
