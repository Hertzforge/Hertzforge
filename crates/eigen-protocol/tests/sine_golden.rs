//! THE GOLDEN-FIXTURE FENCE for [`SineWireProfile`].
//!
//! # What this fences
//!
//! The Mac front end builds the sine wire profile in TypeScript
//! (`ui/src/lib/sine/wireProfile.ts`). TypeScript cannot see this crate's types, so
//! the two halves of the seam are free to drift - and the drift is SILENT in the
//! direction that matters most: a field the UI stops sending is a field
//! `#[serde(default)]` fills in with something the operator never authored. That is
//! the class where a UI-authored safety feature was edited on screen and
//! silently defaulted on the remote path.
//!
//! The fence is ONE artifact shared by both sides:
//!
//! ```text
//!   crates/eigen-protocol/golden/sine_wire_profile.golden.json
//!            ^                                     ^
//!            |                                     |
//!   this test: build a maximal profile      wireProfile.test.ts: map a maximal
//!   from the RUST types, serialize,         store config through
//!   assert it IS the fixture                toSineWireProfile(), assert it IS
//!                                           the same fixture
//! ```
//!
//! A field added on either side reds one of the two:
//!
//! - **Added in Rust** - the maximal literal below has no `..`, so it stops
//!   compiling; once the author fills it in, the serialization gains a key the
//!   fixture lacks and [`the_maximal_rust_profile_is_the_golden_fixture`] reds. If
//!   they regenerate the fixture without touching the mapper, the TypeScript test
//!   reds instead.
//! - **Added in TypeScript** - `deny_unknown_fields` makes
//!   [`the_golden_fixture_round_trips_under_deny_unknown_fields`] red the moment the
//!   fixture is regenerated from the mapper, naming the field.
//! - **Dropped on either side** - the missing key is a deserialize error (required
//!   fields) or a `Value` inequality (defaulted fields). Both red.
//!
//! # Regenerating the fixture
//!
//! Deliberately NOT automatic. There is no `WRITE_GOLDEN=1` escape hatch, because the
//! first thing a red fence teaches is "regenerate and move on", which deletes the
//! fence. To regenerate after an INTENDED protocol change:
//!
//! ```text
//! cargo test -p eigen-protocol --test sine_golden -- --ignored --nocapture print_golden
//! ```
//!
//! and paste the printed document over the fixture. Then run the TypeScript side and
//! fix the mapper, which is the work the fence exists to force.
//!
//! # What the fixture is and is NOT
//!
//! It is MAXIMAL IN FIELDS: every one of the 24 top-level fields and every field of
//! every nested struct is present and non-default, including the three
//! `skip_serializing_if = "Option::is_none"` optionals (`feasibility`, `fs_hz`, and
//! the per-row band overrides) which would otherwise vanish from the document and be
//! fenced by nothing.
//!
//! It is NOT maximal in VARIANTS: `mode`, `compression`, `tracking_bw`, the sweep
//! target, the dwell/rung durations, the capture rule and the COLA waveform each pick
//! one arm. Variants are fenced by the round-trip corpus in `tests/v5.rs` and by the
//! TypeScript mapper's own per-variant tests; this fixture fences the FIELD SET,
//! which is the axis the silently-defaulted hole lives on.

use eigen_protocol::{
    FeasibilityInputs, SineBandwidthMode, SineCaptureConfig, SineCaptureRule, SineColaConfig,
    SineColaWaveform, SineCompositeMode, SineCompression, SineControlChannel, SineControlProfile,
    SineControlSet, SineFeedforwardConfig, SineLadderConfig, SineLimitProfile, SineMetaField,
    SineModeSpec, SineNotchConfig, SineProfileRow, SineQuantity, SineRungFor, SineSweepRate,
    SineSweepSpec, SineSweepTarget, SineWireProfile, SweepDirection,
};

/// The shared artifact, read at COMPILE time from this crate's `golden/` directory.
///
/// ONE file that both sides read by path - `wireProfile.test.ts` imports the same
/// document - rather than two copies: two copies of a fixture whose whole job is to
/// be one document is the drift this test exists to stop. It lives in this crate so
/// the protocol crate builds and tests without the UI tree.
const GOLDEN: &str = include_str!("../golden/sine_wire_profile.golden.json");

/// The maximal profile, built from the Rust types with NO `..` anywhere.
///
/// The missing `..` is half the fence: a field added to any struct below stops this
/// function compiling, which is a louder failure than a test that goes red.
fn maximal() -> SineWireProfile {
    SineWireProfile {
        schema_version: 1,
        profile: SineControlProfile {
            label: "maximal control profile".to_string(),
            rows: vec![
                // Row 0 carries BOTH per-row band overrides, so the two
                // `skip_serializing_if` optionals are in the document.
                SineProfileRow {
                    freq_hz: 5.0,
                    quantity: SineQuantity::DisplacementInPp,
                    value: 0.5,
                    alarm_db: Some((1.5, 1.5)),
                    abort_db: Some((3.0, 3.0)),
                },
                // Row 1 carries NEITHER, so the omitted form is in the document too -
                // a mapper that always emits the keys reds here.
                SineProfileRow {
                    freq_hz: 60.0,
                    quantity: SineQuantity::VelocityIpsPk,
                    value: 12.0,
                    alarm_db: None,
                    abort_db: None,
                },
                SineProfileRow {
                    freq_hz: 2000.0,
                    quantity: SineQuantity::AccelGPk,
                    value: 5.0,
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
                duration_s: 120.0,
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
        limit_profiles: vec![SineLimitProfile {
            channel_index: 3,
            profile: SineControlProfile {
                label: "fixture ceiling".to_string(),
                rows: vec![SineProfileRow {
                    freq_hz: 250.0,
                    quantity: SineQuantity::AccelGPk,
                    value: 2.0,
                    alarm_db: None,
                    abort_db: None,
                }],
                global_alarm_db: (1.5, 1.5),
                global_abort_db: (3.0, 3.0),
            },
            shared_name: Some("fixture ceiling".to_string()),
        }],
        blend_w: 0.5,
        compression: SineCompression::Custom {
            ki: 2.0,
            max_rate_db_per_s: 20.0,
        },
        tracking_bw: SineBandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
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
            full_for: SineRungFor::Sweeps(2),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.5,
        drive_vpk_ceiling: 10.0,
        drive_vpk_abort: 20.0,
        fault_debounce_ticks: 5,
        kinematic_monitor: true,
        capture: SineCaptureConfig {
            rule: SineCaptureRule::AutoFirstSeconds { n_s: 30.0 },
            level_settle_s: 1.0,
            periodic_save_s: Some(60.0),
        },
        cola: SineColaConfig {
            enabled: true,
            volts_pk: 1.0,
            waveform: SineColaWaveform::Sine,
        },
        feasibility: Some(FeasibilityInputs {
            shaker_json: "{\"name\":\"fixture shaker\"}".to_string(),
            armature_lb: 120.0,
            slip_table_lb: 300.0,
            drive_adapter_lb: 15.0,
            fixturing_lb: 40.0,
            test_article_lb: 25.0,
        }),
        control_tick_frames: 64,
        // SET, not None: `fs_hz` is `skip_serializing_if = "Option::is_none"`, so an
        // absent one would vanish from the document and be fenced by nothing - the
        // same reason `feasibility` and the per-row band overrides are populated.
        fs_hz: Some(51200.0),
        review_hold: true,
        metadata: vec![
            SineMetaField {
                key: "operator".to_string(),
                value: "Test Operator".to_string(),
            },
            SineMetaField {
                key: "test_article".to_string(),
                value: "fixture".to_string(),
            },
        ],
    }
}

/// The fixture is a legal `SineWireProfile` and re-serializes to itself.
///
/// `deny_unknown_fields` is the half of the fence that catches a TypeScript-side
/// ADDITION: the moment the fixture is regenerated from a mapper that emits a key
/// this crate does not know, the decode fails and NAMES the field.
///
/// The re-serialization half catches a Rust-side addition of a `#[serde(default)]`
/// field: the decode still succeeds (that is what a default is for), but the value
/// that comes back out carries a key the fixture never had.
#[test]
fn the_golden_fixture_round_trips_under_deny_unknown_fields() {
    let decoded: SineWireProfile =
        serde_json::from_str(GOLDEN).expect("the golden fixture must decode as a SineWireProfile");
    let reserialized = serde_json::to_value(&decoded).unwrap();
    let as_written: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(
        reserialized, as_written,
        "the fixture and its round trip differ: a field was added or dropped on the \
         Rust side. Regenerate with `--ignored --nocapture print_golden` ONLY after \
         teaching ui/src/lib/sine/wireProfile.ts to send it."
    );
}

/// The maximal profile this crate can build IS the fixture.
///
/// This is the direction that catches a field the Rust side gained and the mapper did
/// not: `maximal()` has no `..`, so the author is forced to fill the new field in,
/// and the moment they do this assertion reds against the un-regenerated fixture.
#[test]
fn the_maximal_rust_profile_is_the_golden_fixture() {
    let built = serde_json::to_value(maximal()).unwrap();
    let fixture: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(
        built, fixture,
        "the maximal Rust profile no longer matches the shared golden fixture"
    );
}

/// Decoding the fixture yields exactly the value `maximal()` builds.
///
/// Not redundant with the two above: they compare JSON documents, and this compares
/// the DECODED values. A field whose serde representation is lossy (an enum renamed
/// so two variants collide, a tuple flattened) passes a document comparison and fails
/// here.
#[test]
fn the_fixture_decodes_to_the_maximal_value() {
    let decoded: SineWireProfile = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(decoded, maximal());
}

/// A DEMONSTRATION that the fence is live: the fixture with one extra key is refused
/// by name.
///
/// A result that cannot fail is not evidence. The three tests above all
/// pass on a fixture nobody is checking against anything; this one proves the
/// mechanism they rely on actually rejects a mismatch.
#[test]
fn an_unknown_field_in_the_document_is_refused_by_name() {
    let mut doc: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    doc.as_object_mut()
        .unwrap()
        .insert("sysid_averages".to_string(), serde_json::json!(8));
    let err = serde_json::from_value::<SineWireProfile>(doc).unwrap_err();
    assert!(
        err.to_string().contains("sysid_averages"),
        "the refusal must name the field; got: {err}"
    );
}

/// A DEMONSTRATION of the other direction: a field DROPPED from the document is
/// refused (required) rather than silently defaulted.
///
/// `blend_w` is the drop that matters most in the silently-defaulted sense - it is the operator's
/// feedforward trust weight, and a run that silently used 0.0 instead of what was on
/// screen would be a different test.
#[test]
fn a_dropped_required_field_is_refused_rather_than_defaulted() {
    let mut doc: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    doc.as_object_mut().unwrap().remove("blend_w");
    let err = serde_json::from_value::<SineWireProfile>(doc).unwrap_err();
    assert!(
        err.to_string().contains("blend_w"),
        "the refusal must name the missing field; got: {err}"
    );
}

/// Print the canonical fixture. Run only when regenerating - see the module docs.
#[test]
#[ignore = "regeneration helper; prints the fixture, asserts nothing"]
fn print_golden() {
    println!("{}", serde_json::to_string_pretty(&maximal()).unwrap());
}
