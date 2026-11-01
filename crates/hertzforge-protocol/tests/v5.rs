//! The v5 wire contract's SEMANTICS: three module surfaces landed in ONE bump.
//!
//! v5 carries the closed-loop Sine profile and command group, the classical Shock and
//! SRS profiles and their command groups, nine additive `StatusReport` readouts, the
//! `Response::Shock` document answer, and two new `SchemaKind`s - all in one version
//! increment, because two sessions were building against this seam at the same time
//! and the exact-match gate makes three separate bumps three coordinated redeploys.
//!
//! What this file pins, over and above the round-trips in `roundtrip.rs`:
//!
//! 1. The bump landed, and `PROFILE_SCHEMA` did NOT move with it.
//! 2. A v4-era `schema_version: 1` Random profile still configures on a v5 build -
//!    the compatibility row that makes item 1 a decision rather than an oversight.
//! 3. The new profiles refuse unknown fields BY NAME, like every profile before them.
//! 4. Each new profile declares the right `TestMode`.
//! 5. The three capture verbs are PRESENT (a wire shape), classified MUTATING, and
//!    claim no `ActionKind` row.
//! 6. The status additions are additive and absent from an idle report.
//! 7. The safety defaults (`review_hold`, `review_after_sysid`, `max_drive_vpk`) are
//!    the SAFE values when a stored document omits them.

use hertzforge_protocol::{
    ActionKind, ClassicalShockProfile, Command, Doc, LadderMode, ProtocolError, RandomProfile,
    Response, SchemaKind, ShockCommand, ShockCompensation, ShockCompensationShape, ShockDriveLimit,
    ShockLevelSchedule, ShockPulseShape, ShockPulseSpec, ShockRateModel, ShockRunMode,
    ShockSpacing, ShockToleranceStandard, ShockTrainSpec, SineBandwidthMode, SineCaptureConfig,
    SineCaptureRule, SineColaConfig, SineColaWaveform, SineCommand, SineCompositeMode,
    SineCompression, SineControlChannel, SineControlProfile, SineControlSet, SineDwellFor,
    SineFeedforwardConfig, SineLadderConfig, SineModeSpec, SineNotchConfig, SineProfileRow,
    SineQuantity, SineRungFor, SineSweepRate, SineSweepSpec, SineSweepTarget, SineWireProfile,
    SrsBandFraction, SrsCommand, SrsEngineKind, SrsEnvelopeSpec, SrsFeasibilityTol,
    SrsKinematicAbort, SrsProfile, SrsSynthesisOpts, SrsSysIdExcitation, SrsSysIdSpec, SrsTarget,
    StatusReport, SweepDirection, TestMode, TestProfile, DEFAULT_SHOCK_MAX_DRIVE_VPK,
    PROFILE_SCHEMA, PROTOCOL_VERSION, SHOCK_PULSE_SCHEMA,
};

// =====================================================================================
// Fixtures
// =====================================================================================

/// A minimal but COMPLETE sine profile - every non-optional field populated, so a
/// serializer that dropped one is caught by the round-trip rather than by a `None`
/// that happened to match.
pub fn sine() -> SineWireProfile {
    SineWireProfile {
        schema_version: PROFILE_SCHEMA,
        profile: SineControlProfile {
            label: "acceptance".into(),
            rows: vec![
                SineProfileRow {
                    freq_hz: 5.0,
                    quantity: SineQuantity::DisplacementInPp,
                    value: 0.5,
                    alarm_db: None,
                    abort_db: None,
                },
                SineProfileRow {
                    freq_hz: 60.0,
                    quantity: SineQuantity::AccelGPk,
                    value: 2.0,
                    // A per-row override, so the round-trip exercises the Some arm.
                    alarm_db: Some((1.5, 1.5)),
                    abort_db: Some((3.0, 3.0)),
                },
                SineProfileRow {
                    freq_hz: 2000.0,
                    quantity: SineQuantity::AccelGPk,
                    value: 2.0,
                    alarm_db: None,
                    abort_db: None,
                },
            ],
            global_alarm_db: (1.5, 1.5),
            global_abort_db: (3.0, 3.0),
        },
        mode: SineModeSpec::Sweep(SineSweepSpec {
            band_hz: (5.0, 2000.0),
            rate: SineSweepRate::LogOctPerMin(1.0),
            start_hz: 5.0,
            initial_direction: SweepDirection::Up,
            target: SineSweepTarget::WhicheverFirst {
                sweeps: 2,
                duration_s: 600.0,
            },
        }),
        control: SineControlSet {
            channels: vec![
                SineControlChannel {
                    index: 1,
                    weight: 1.0,
                },
                SineControlChannel {
                    index: 2,
                    weight: 0.5,
                },
            ],
            mode: SineCompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 0.5,
        compression: SineCompression::Custom {
            ki: 4.0,
            max_rate_db_per_s: 40.0,
        },
        tracking_bw: SineBandwidthMode::Proportional {
            pct: 5.0,
            max_hz: 20.0,
        },
        feedforward: SineFeedforwardConfig {
            coherence_floor: 0.5,
            h_floor_frac: 0.05,
        },
        notch: SineNotchConfig {
            drive_floor: 0.01,
            smoothing_ticks: 3.0,
            release_ramp_db_per_s: 20.0,
        },
        ladder: SineLadderConfig {
            sysid_level_db: -20.0,
            start_db: -12.0,
            table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
            per_rung: SineRungFor::Sweeps(1),
            full_for: SineRungFor::Seconds(120.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.5,
        drive_vpk_ceiling: 1.0,
        drive_vpk_abort: 2.0,
        fault_debounce_ticks: 5,
        kinematic_monitor: true,
        capture: SineCaptureConfig {
            rule: SineCaptureRule::AutoFirstSeconds { n_s: 10.0 },
            level_settle_s: 1.0,
            periodic_save_s: Some(30.0),
        },
        cola: SineColaConfig {
            enabled: true,
            volts_pk: 1.0,
            waveform: SineColaWaveform::Square,
        },
        feasibility: None,
        control_tick_frames: 64,
        fs_hz: None,
        review_hold: true,
        metadata: Vec::new(),
    }
}

/// A classical shock profile with every field populated.
pub fn classical() -> ClassicalShockProfile {
    ClassicalShockProfile {
        schema_version: PROFILE_SCHEMA,
        spec: ShockPulseSpec {
            shape: ShockPulseShape::Trapezoid {
                rise_frac: 0.1,
                fall_frac: 0.1,
            },
            amplitude_g: 20.0,
            duration_ms: 11.0,
            compensation: ShockCompensation {
                shape: ShockCompensationShape::HalfSine,
                pre_frac: 1.5,
                post_frac: 1.5,
                symmetric: true,
            },
        },
        fs_hz: 5120.0,
        standard: ShockToleranceStandard::Mil810_516,
        schedule: shock_schedule(),
        train: ShockTrainSpec {
            count: 3,
            spacing: ShockSpacing::Delay(2.0),
        },
        gunfire: None,
        rate_model: ShockRateModel {
            cooling_floor_s: 1.0,
            settling_floor_s: 0.5,
            compute_floor_s: 0.25,
            device_recompute_floor_s: 20.0,
            read_cadence_ok: true,
        },
        drive_limit: ShockDriveLimit {
            peak_accel_g: Some(50.0),
            monitors: Vec::new(),
        },
        feasibility: None,
        residual_velocity_tol_ips: 1.0,
        residual_disp_tol_in: 0.1,
        srs_f_lo_hz: 20.0,
        srs_f_hi_hz: 2000.0,
        srs_bands_per_octave: 3.0,
        srs_band_tol_db: 3.0,
        compute_pvss: true,
        abort_on_tolerance_failure: true,
        max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
        metadata: Vec::new(),
    }
}

/// An SRS profile with every field populated.
pub fn srs() -> SrsProfile {
    SrsProfile {
        schema_version: PROFILE_SCHEMA,
        f_lo_hz: 20.0,
        f_hi_hz: 2000.0,
        fraction: SrsBandFraction::Third,
        target: SrsTarget::Breakpoints {
            points: vec![(20.0, 10.0), (2000.0, 100.0)],
        },
        q: 10.0,
        engine: SrsEngineKind::Wavsyn,
        synthesis: SrsSynthesisOpts {
            max_iters: 50,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        },
        schedule: shock_schedule(),
        limits: None,
        feasibility: None,
        feasibility_tol: SrsFeasibilityTol {
            residual_velocity_ips: 1.0,
            residual_disp_in: 0.1,
        },
        drive_limit: ShockDriveLimit {
            peak_accel_g: None,
            monitors: Vec::new(),
        },
        drive_abort_factor: 1.5,
        kinematic_abort: SrsKinematicAbort {
            accel_g: Some(100.0),
            velocity_ips: None,
            disp_pp_in: None,
        },
        envelope: SrsEnvelopeSpec {
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            per_band: None,
        },
        sysid: Some(SrsSysIdSpec {
            level_db: -20.0,
            excitation: SrsSysIdExcitation::Shaped,
            nfft: 4096,
            h_floor_frac: 0.05,
        }),
        compute_pvss: false,
        ring_out_s: 0.5,
        review_after_sysid: true,
        max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
        metadata: Vec::new(),
    }
}

fn shock_schedule() -> ShockLevelSchedule {
    ShockLevelSchedule {
        mode: ShockRunMode::Auto,
        start_db: -12.0,
        table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        pulses_per_level: vec![1],
    }
}

/// Every variant of every new command group, so a list-based assertion below is an
/// ENUMERATION rather than a sample.
fn every_sine_command() -> Vec<SineCommand> {
    vec![
        SineCommand::Hold,
        SineCommand::Resume,
        SineCommand::Reverse,
        SineCommand::SetBlendW { w: 0.25 },
        SineCommand::OverrideFeasibility {
            ack: "reviewed, proceeding".into(),
        },
    ]
}

fn every_shock_command() -> Vec<ShockCommand> {
    vec![
        ShockCommand::FirePulse,
        ShockCommand::RunAuto {
            max_pulses: Some(10),
        },
        ShockCommand::JumpToLevel { index: 2 },
        ShockCommand::ArmWithOverride {
            ack: "reviewed, proceeding".into(),
        },
        ShockCommand::Reset,
        ShockCommand::PulseList,
        ShockCommand::PulseDetail { seq: 7 },
        ShockCommand::SrsFreqs,
        ShockCommand::FireTrain {
            spec: Some(ShockTrainSpec {
                count: 5,
                spacing: ShockSpacing::MaxRate,
            }),
        },
        ShockCommand::FireGunfireBurst,
        ShockCommand::StartCapture,
        ShockCommand::StopCapture,
        ShockCommand::SaveSnapshot,
    ]
}

fn every_srs_command() -> Vec<SrsCommand> {
    vec![
        SrsCommand::FirePulse,
        SrsCommand::RunAuto { max_pulses: None },
        SrsCommand::JumpToLevel { index: 0 },
        SrsCommand::ArmWithOverride {
            ack: "reviewed, proceeding".into(),
        },
        SrsCommand::Reset,
        SrsCommand::RunSysId,
        SrsCommand::PulseList,
        SrsCommand::PulseDetail { seq: 1 },
        SrsCommand::SrsFreqs,
        SrsCommand::NotchStatus,
        SrsCommand::StartCapture,
        SrsCommand::StopCapture,
        SrsCommand::SaveSnapshot,
    ]
}

// =====================================================================================
// The version gate, and the schema that deliberately did NOT move
// =====================================================================================

/// The bump landed. Stated here as well as in the crate's own unit test because THIS
/// file is the one that adds payloads an older peer cannot parse.
///
/// **The number is 6, not 5, and this row is still about v5** (updated at the TWR bump,
/// wire bump #5). What it asserts is that the payloads in this file ride a version an
/// older peer is refused at, and the constant only ever moves forward - so a v4 peer is
/// refused for the v5 payloads and a v5 peer is refused for the v6 ones. Pinning the
/// literal rather than writing `>= 5` is deliberate: a version that drifts by accident
/// has to red something, and a comparison would let it move silently.
#[test]
fn the_new_payloads_ride_a_bumped_version() {
    assert_eq!(PROTOCOL_VERSION, 6);
}

/// **`PROFILE_SCHEMA` STAYS 1 across the bump, and a v4-era Random profile proves it.**
///
/// This is the compatibility row that makes the decision visible. `PROFILE_SCHEMA` is
/// ONE global exact-match number shared by every `TestProfile` variant, so bumping it
/// to mark the three new v5 variants would have refused the deployed application's
/// stored Random and Explorer profiles - documents that did not change, refused
/// mid-workflow, for a reason that has nothing to do with them.
///
/// RED-MUTATION: set `PROFILE_SCHEMA = 2` and this reds on the second assertion, which
/// is exactly the deployed-profile refusal it is here to prevent.
#[test]
fn the_profile_schema_stays_one_and_a_v4_era_random_profile_still_configures() {
    assert_eq!(
        PROFILE_SCHEMA, 1,
        "the v5 bump must not move the profile schema"
    );

    // The document a v4 front end wrote, verbatim: `schema_version: 1` and none of the
    // fields that were optional then and are optional still.
    let v4_era = r#"{"random":{"schema_version":1,"breakpoints":[],"schedule":[],
        "ladder":"auto","duration_s":null}}"#;
    let decoded: TestProfile = serde_json::from_str(v4_era).expect("a v4-era profile must decode");
    assert_eq!(decoded.schema_version(), PROFILE_SCHEMA);
    assert!(
        Command::SetProfile { profile: decoded }
            .check_schemas()
            .is_ok(),
        "a schema_version:1 Random profile must still pass the schema gate at v5"
    );

    // ANTI-VACUITY: a profile from the FUTURE is still refused, so the pass above is
    // about the number being unchanged and not about the gate having been removed.
    let mut future = RandomProfile {
        frf_pairs: None,
        feasibility: None,
        limits: None,
        control_weights: None,
        composite_mode: None,
        schema_version: PROFILE_SCHEMA + 1,
        breakpoints: Vec::new(),
        schedule: Vec::new(),
        ladder: LadderMode::Auto,
        duration_s: None,
        acquisition: None,
        control: None,
    };
    future.schema_version = PROFILE_SCHEMA + 1;
    assert!(matches!(
        Command::SetProfile {
            profile: TestProfile::Random(future),
        }
        .check_schemas()
        .unwrap_err(),
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::Profile,
            ..
        }
    ));
}

// =====================================================================================
// The three new profiles
// =====================================================================================

/// Each new profile declares the mode it actually runs under.
///
/// The sine one is the load-bearing assertion: closed-loop sine is `TestMode::Sine`,
/// NOT the `OpenLoopSine` the Explorer and SMART share. Something corrects the drive
/// every tick, which is the distinction `TestMode` draws and the one the shared-clock
/// precondition binds to - claiming open-loop here would let a station advertise a
/// per-tick-corrected run it has no proven clock for.
#[test]
fn each_new_profile_declares_its_own_mode() {
    assert_eq!(TestProfile::Sine(Box::new(sine())).mode(), TestMode::Sine);
    assert_eq!(
        TestProfile::ShockClassical(Box::new(classical())).mode(),
        TestMode::Shock
    );
    assert_eq!(TestProfile::Srs(Box::new(srs())).mode(), TestMode::Srs);
}

/// All three round-trip with every field intact, and all three go through the same
/// schema gate as every document-carrying command before them.
#[test]
fn the_new_profiles_round_trip_and_are_schema_gated() {
    for p in [
        TestProfile::Sine(Box::new(sine())),
        TestProfile::ShockClassical(Box::new(classical())),
        TestProfile::Srs(Box::new(srs())),
    ] {
        let json = serde_json::to_string(&p).expect("encode");
        let back: TestProfile = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, p, "the profile lost a field on the wire");
        assert!(Command::SetProfile { profile: p }.check_schemas().is_ok());
    }
}

/// An UNKNOWN FIELD on any of the three is a hard decode failure NAMING the field -
/// the same posture as every profile since v3, and the reason each of these will need
/// its own version bump to grow.
#[test]
fn an_unknown_field_on_each_new_profile_is_refused_by_name() {
    for (label, json) in [
        (
            "sine",
            r#"{"sine":{"schema_version":1,"blend_weight":0.5}}"#,
        ),
        (
            "shock_classical",
            r#"{"shock_classical":{"schema_version":1,"amplitude":20.0}}"#,
        ),
        ("srs", r#"{"srs":{"schema_version":1,"f_low":20.0}}"#),
    ] {
        let err = serde_json::from_str::<TestProfile>(json).expect_err("must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown field"),
            "{label}: the refusal must name the field: {msg}"
        );
    }
}

/// **THE SAFETY DEFAULTS ARE THE SAFE VALUES.**
///
/// A stored profile that omits `review_hold` / `review_after_sysid` gets the HOLD, and
/// one that omits `max_drive_vpk` gets a real ceiling rather than zero or infinity.
/// Serde defaults are the one place where "what a missing field means" is decided, and
/// deciding it wrong here means a document written before the field existed drives
/// straight through the review a human was supposed to do.
///
/// RED-MUTATION: change either `default_*` helper to `false` and this reds.
#[test]
fn the_omitted_safety_fields_default_to_the_safe_value() {
    let no_hold = r#"{"schema_version":1,"f_lo_hz":20.0,"f_hi_hz":2000.0,
        "fraction":"third","target":{"flat":{"accel_g":10.0}},"q":10.0,"engine":"wavsyn",
        "synthesis":{"max_iters":50,"tol_db":1.5,"max_gain_db_per_iter":3.0,"q":10.0},
        "schedule":{"mode":"auto","start_db":-12.0,"table_db":[0.0],"pulses_per_level":[1]},
        "feasibility_tol":{"residual_velocity_ips":1.0,"residual_disp_in":0.1},
        "drive_limit":{},"drive_abort_factor":1.5,"kinematic_abort":{},
        "envelope":{"alarm_db":[3.0,3.0],"abort_db":[6.0,6.0]},
        "compute_pvss":false,"ring_out_s":0.5}"#;
    let p: SrsProfile = serde_json::from_str(no_hold).expect("decode");
    assert!(
        p.review_after_sysid,
        "an omitted review hold must default ON - the safe value"
    );
    assert_eq!(
        p.max_drive_vpk, DEFAULT_SHOCK_MAX_DRIVE_VPK,
        "an omitted drive ceiling must be a real number, never zero or unbounded"
    );

    // The sine hold has the same rule, checked through the whole-profile default path.
    let mut s = sine();
    s.review_hold = true;
    let json = serde_json::to_string(&s).expect("encode");
    let stripped = json.replace(r#""review_hold":true,"#, "");
    assert_ne!(stripped, json, "the field must have been present to strip");
    let back: SineWireProfile = serde_json::from_str(&stripped).expect("decode");
    assert!(back.review_hold, "an omitted sine review hold defaults ON");
}

// =====================================================================================
// The three new command groups
// =====================================================================================

/// Every new verb round-trips through the externally-tagged wire form.
#[test]
fn every_new_command_round_trips() {
    let mut all: Vec<Command> = Vec::new();
    all.extend(every_sine_command().into_iter().map(Command::Sine));
    all.extend(every_shock_command().into_iter().map(Command::Shock));
    all.extend(every_srs_command().into_iter().map(Command::Srs));
    for c in all {
        let json = serde_json::to_string(&c).expect("encode");
        assert_eq!(
            serde_json::from_str::<Command>(&json).expect("decode"),
            c,
            "{json}"
        );
        // And none of them carries a versioned document today, so all pass the gate.
        assert!(c.check_schemas().is_ok(), "{c:?}");
    }
}

/// The SINE verbs are all MUTATING and each maps onto an EXISTING action row.
///
/// The Observer half is the safety-relevant one: a watcher who could send `Hold`
/// could freeze someone else's sweep on a resonance, and one who could send
/// `OverrideFeasibility` could release a run the shaker-limits check refused.
#[test]
fn the_sine_verbs_are_mutating_and_map_onto_existing_action_rows() {
    for s in every_sine_command() {
        assert!(s.is_mutating(), "{s:?} must require the run lease");
        let c = Command::Sine(s.clone());
        assert!(c.is_mutating());
        assert_eq!(c.action_kind(), Some(s.action_kind()));
    }
    assert_eq!(
        SineCommand::Hold.action_kind(),
        ActionKind::GoToFrequency,
        "pausing a traversal is frequency authority"
    );
    assert_eq!(
        SineCommand::SetBlendW { w: 0.5 }.action_kind(),
        ActionKind::AdjustLevel,
        "the blend weight decides how much of the next drive is feedforward"
    );
    assert_eq!(
        SineCommand::OverrideFeasibility { ack: "y".into() }.action_kind(),
        ActionKind::Start,
        "releasing a park is the run starting to drive"
    );
}

/// **THE MUTATING SPLIT OVER BOTH SHOCK GROUPS, stated over every variant.**
///
/// Three verbs per group are read-only (`PulseList`, `PulseDetail`, `SrsFreqs`) and
/// everything else is gated. The judgement call, pinned here so it is not re-litigated
/// silently: the THREE CAPTURE VERBS are MUTATING even though the daemon refuses them
/// today. The classification answers what the verb IS, not whether it currently works
/// - a verb classified read-only because its implementation happens to be a refusal
///   becomes Observer-reachable the moment someone implements it.
#[test]
fn the_shock_mutating_split_covers_every_variant_and_capture_is_mutating() {
    let read_only_shock = [
        ShockCommand::PulseList,
        ShockCommand::PulseDetail { seq: 7 },
        ShockCommand::SrsFreqs,
    ];
    for s in every_shock_command() {
        let expect_mutating = !read_only_shock.contains(&s);
        assert_eq!(
            s.is_mutating(),
            expect_mutating,
            "{s:?} is classified wrong for the Observer gate"
        );
        assert_eq!(Command::Shock(s.clone()).is_mutating(), expect_mutating);
    }
    let read_only_srs = [
        SrsCommand::PulseList,
        SrsCommand::PulseDetail { seq: 1 },
        SrsCommand::SrsFreqs,
        SrsCommand::NotchStatus,
    ];
    for s in every_srs_command() {
        let expect_mutating = !read_only_srs.contains(&s);
        assert_eq!(s.is_mutating(), expect_mutating, "{s:?}");
        assert_eq!(Command::Srs(s.clone()).is_mutating(), expect_mutating);
    }

    // The capture three, named explicitly - this is the assertion the sentence above
    // is about.
    for s in [
        ShockCommand::StartCapture,
        ShockCommand::StopCapture,
        ShockCommand::SaveSnapshot,
    ] {
        assert!(s.is_mutating(), "{s:?} writes files on the core host");
        assert_eq!(
            s.action_kind(),
            None,
            "{s:?} has no ActionMatrix row and must not claim one"
        );
    }
}

/// The read-only shock verbs claim NO action row, and the driving ones are gated like
/// `Start` - a transient producer has no continuous run, so firing IS the run.
#[test]
fn the_shock_action_map_gates_firing_like_start_and_queries_not_at_all() {
    assert_eq!(
        ShockCommand::FirePulse.action_kind(),
        Some(ActionKind::Start)
    );
    assert_eq!(
        ShockCommand::FireTrain { spec: None }.action_kind(),
        Some(ActionKind::Start)
    );
    assert_eq!(SrsCommand::RunSysId.action_kind(), Some(ActionKind::Start));
    assert_eq!(
        ShockCommand::JumpToLevel { index: 1 }.action_kind(),
        Some(ActionKind::SetLevel)
    );
    assert_eq!(ShockCommand::Reset.action_kind(), Some(ActionKind::Stop));
    for c in [
        Command::Shock(ShockCommand::PulseList),
        Command::Shock(ShockCommand::SrsFreqs),
        Command::Srs(SrsCommand::PulseDetail { seq: 3 }),
        // Additive within v5 (2026-08-09). A READ, so it claims no ActionMatrix row -
        // the property that keeps it Observer-reachable.
        Command::Srs(SrsCommand::NotchStatus),
    ] {
        assert_eq!(c.action_kind(), None, "{c:?} is a read-only query");
    }
}

// =====================================================================================
// Status growth and the shock document answer
// =====================================================================================

/// The v5 readouts are ADDITIVE: absent from an idle report entirely, so a v4 decoder
/// handed one sees exactly the document it expects.
///
/// The list below is the authority - COUNT IT, do not carry a number in this sentence
/// that a later field silently falsifies. It grew to TEN when a later sine sweep
/// added `progress`.
///
/// The non-default values are deliberate. `Some(0)` for `train_remaining`, `Some(0.0)`
/// for `thd_pct` and `Some(0.0)` for `progress` are LEGAL, MEANINGFUL values ("no train
/// in flight", "no measurable distortion", "a run that just started") and must not
/// serialize the same as absent - which is what a `skip_serializing_if` reflex on zero
/// would have made them.
#[test]
fn the_v5_status_readouts_are_additive_and_zero_is_not_absent() {
    let idle = serde_json::to_string(&StatusReport::idle()).expect("encode");
    for field in [
        "sweeps_done",
        "thd_pct",
        "progress",
        "hold",
        "notch_active",
        "pulses_fired",
        "pulses_at_level",
        "train_remaining",
        "last_pulse_seq",
        "last_pulse",
    ] {
        assert!(
            !idle.contains(field),
            "{field} must be absent from idle: {idle}"
        );
    }

    let live = StatusReport {
        sweeps_done: Some(3),
        thd_pct: Some(0.0),
        progress: Some(0.0),
        hold: Some("operator hold".into()),
        notch_active: Some(false),
        pulses_fired: Some(12),
        pulses_at_level: Some(2),
        train_remaining: Some(0),
        last_pulse_seq: Some(12),
        last_pulse: Some(Doc::new(SchemaKind::ShockPulse, r#"{"seq":12}"#)),
        ..StatusReport::idle()
    };
    let json = serde_json::to_string(&live).expect("encode");
    assert!(json.contains(r#""train_remaining":0"#), "{json}");
    assert!(json.contains(r#""thd_pct":0.0"#), "{json}");
    assert!(json.contains(r#""progress":0.0"#), "{json}");
    assert!(json.contains(r#""notch_active":false"#), "{json}");
    assert_eq!(
        serde_json::from_str::<StatusReport>(&json).expect("decode"),
        live
    );
    // ANTI-VACUITY: the two documents really are different.
    assert_ne!(json, idle);
}

/// The shock document answer round-trips and its schema kind is checked, not guessed.
///
/// `SchemaKind::ShockPulse` is STAMPING and CHECKING only: it is not carried on the
/// `Doc`, so this asserts the constant is wired to the kind rather than that a tag
/// survives the wire.
#[test]
fn the_shock_document_answer_round_trips_and_checks_its_own_kind() {
    let d = Doc::new(SchemaKind::ShockPulse, r#"{"seq":1,"peak_g":19.8}"#);
    assert_eq!(d.schema_version, SHOCK_PULSE_SCHEMA);
    assert!(d.check(SchemaKind::ShockPulse).is_ok());

    let r = Response::Shock(d);
    let json = serde_json::to_string(&r).expect("encode");
    assert_eq!(serde_json::from_str::<Response>(&json).expect("decode"), r);

    // A document from the future is refused, naming the kind so a UI can say WHICH
    // document to fix.
    let future = Doc {
        schema_version: SHOCK_PULSE_SCHEMA + 1,
        json: "{}".into(),
    };
    assert!(matches!(
        future.check(SchemaKind::ShockPulse).unwrap_err(),
        ProtocolError::UnsupportedSchema {
            kind: SchemaKind::ShockPulse,
            ..
        }
    ));
}

/// A sine mode other than the sweep survives the wire - the enum is four-armed and a
/// corpus that only ever carried `Sweep` would not catch a mis-tagged variant.
#[test]
fn the_other_sine_modes_survive_the_wire() {
    for mode in [
        SineModeSpec::Dwell(hertzforge_protocol::SineDwellPlan {
            freq_hz: 60.0,
            dwell_for: SineDwellFor::Cycles(20_000),
        }),
        SineModeSpec::StepSine(hertzforge_protocol::SineStepSpec {
            band_hz: (20.0, 2000.0),
            steps_per_octave: 12.0,
            spacing: hertzforge_protocol::SineStepSpacing::Log,
            dwell: Some(SineDwellFor::Seconds(60.0)),
        }),
        SineModeSpec::Rstd(hertzforge_protocol::SineRstdSpec {
            band_hz: (20.0, 200.0),
            level_db: -20.0,
            rate: SineSweepRate::LinearHzPerSec(1.0),
            tracking_bw_hz: 5.0,
            settle_k: 5.0,
            dwell_mode: hertzforge_protocol::SineRstdDwellMode::PhaseTracked,
            dwell_for: SineDwellFor::Seconds(300.0),
        }),
    ] {
        let p = TestProfile::Sine(Box::new(SineWireProfile { mode, ..sine() }));
        let json = serde_json::to_string(&p).expect("encode");
        assert_eq!(
            serde_json::from_str::<TestProfile>(&json).expect("decode"),
            p
        );
    }
}
