//! `SineWireProfile` -> `eigen_sine::SineTestConfig`: **the wire fidelity fence.**
//!
//! # The silent-default class, and why this file is shaped the way it is
//!
//! The silent-default class is where a UI-authored SAFETY FEATURE - a notch table,
//! a control weight, a coherence floor - was edited on screen and silently
//! DEFAULTED on the remote path. The operator authored a ceiling and got no
//! ceiling, with no indication anywhere that the two ends disagreed. Random's
//! translation was a long list of `if let Some(v) = ... { cfg.x = v }` arms, and a
//! field nobody wrote an arm for simply kept the core's default.
//!
//! The rule that keeps Sine out of that hole is not a test. It is a SHAPE:
//!
//! 1. [`config_from_wire`] destructures [`SineWireProfile`] EXHAUSTIVELY - no
//!    `..`, no wildcard binding. A field added to the wire type and not consumed
//!    here is a COMPILE ERROR in this function.
//! 2. It builds [`SineTestConfig`] with a COMPLETE struct literal - again no `..`.
//!    A field added to the config type and not supplied here is a COMPILE ERROR at
//!    the literal.
//! 3. [`wire_from_config`] (tests) does the mirror: it destructures the CONFIG
//!    exhaustively and builds the WIRE type with a complete literal, so the
//!    round-trip test cannot be satisfied by a converter that quietly drops a
//!    field on the way back.
//!
//! Those three together mean a field added on EITHER side stops this crate
//! compiling until somebody decides what it means over the seam. That is the
//! difference between a fence and a promise.
//!
//! # The one asymmetry, named
//!
//! The wire carries a single optional [`FeasibilityInputs`]; the config carries
//! `shaker: Option<ShakerRecord>` and `moving_mass: MovingMass` as two fields.
//! One wire field feeds two config fields, and the inverse is only faithful when
//! the config's `moving_mass` is the default whenever `shaker` is `None` - which
//! is what "no shaker selected" means (feasibility is skipped and the kinematic
//! monitor is disabled, so a mass stack with nothing to compare it against is not
//! carried). [`wire_from_config`] states that, and the round-trip corpus covers
//! both arms.
//!
//! A `shaker_json` that does not parse is a REFUSAL, never a run with feasibility
//! silently skipped - the Random host's rule, and the reason this returns a
//! `Result` rather than a config.

use eigen_protocol::{ProtocolError, SineWireProfile};
use eigen_sine::{
    service::config::{
        CaptureConfig, CaptureRule, ColaConfig, ColaWaveform, ControlChannel, ControlSet,
        LimitProfile, MetaField, SineMode, SineTestConfig,
    },
    BandwidthMode, BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan,
    FeedforwardConfig, LadderConfig, NotchGovernorConfig, ProfileRow, Quantity, RstdConfig,
    RstdDwellMode, RungFor, SineProfile, StepSineConfig, StepSpacing, SweepDirection, SweepRate,
    SweepTarget,
};

/// Translate a v5 wire profile into the core's test configuration.
///
/// **Exhaustive on both sides.** See the module docs - the destructuring pattern
/// and the struct literal below are the fidelity fence, and neither may grow a `..`.
pub fn config_from_wire(w: SineWireProfile) -> Result<SineTestConfig, ProtocolError> {
    // ---- THE FENCE, half one: every wire field is BOUND ------------------------
    let SineWireProfile {
        // Checked by `Command::check_schemas` before any handler runs; it is the
        // envelope's business, not the configuration's.
        schema_version: _,
        profile,
        mode,
        control,
        limit_profiles,
        blend_w,
        compression,
        tracking_bw,
        feedforward,
        notch,
        ladder,
        sysid_level_db,
        coherence_floor,
        drive_vpk_ceiling,
        drive_vpk_abort,
        fault_debounce_ticks,
        kinematic_monitor,
        capture,
        cola,
        feasibility,
        control_tick_frames,
        fs_hz,
        review_hold,
        metadata,
    } = w;

    // ---- feasibility: ONE wire field, TWO config fields ------------------------
    let (shaker, moving_mass) = match feasibility {
        None => (None, eigen_shaker_db::MovingMass::default()),
        Some(f) => {
            let rec = serde_json::from_str::<eigen_shaker_db::ShakerRecord>(&f.shaker_json)
                .map_err(|e| ProtocolError::InvalidArgument {
                    message: format!(
                        "feasibility.shaker_json did not parse as a shaker record ({e}); \
                         refusing rather than running with the shaker-limit check silently \
                         skipped"
                    ),
                })?;
            tracing::info!(
                target: "eigen::backendd",
                shaker = %rec.name,
                "sine feasibility inputs adopted from the wire"
            );
            (
                Some(rec),
                eigen_shaker_db::MovingMass {
                    armature_lb: f.armature_lb,
                    slip_table_lb: f.slip_table_lb,
                    drive_adapter_lb: f.drive_adapter_lb,
                    fixturing_lb: f.fixturing_lb,
                    test_article_lb: f.test_article_lb,
                },
            )
        }
    };

    // ---- THE FENCE, half two: every config field is SUPPLIED -------------------
    let cfg = SineTestConfig {
        profile: control_profile(profile),
        mode: mode_from_wire(mode),
        control: ControlSet {
            channels: control
                .channels
                .into_iter()
                .map(|c| ControlChannel {
                    index: c.index,
                    weight: c.weight,
                })
                .collect(),
            mode: composite_from_wire(control.mode),
        },
        limit_profiles: limit_profiles
            .into_iter()
            .map(|lp| LimitProfile {
                channel_index: lp.channel_index,
                profile: control_profile(lp.profile),
                shared_name: lp.shared_name,
            })
            .collect(),
        blend_w,
        compression: match compression {
            eigen_protocol::SineCompression::Slow => CompressionPreset::Slow,
            eigen_protocol::SineCompression::Fast => CompressionPreset::Fast,
            eigen_protocol::SineCompression::Custom {
                ki,
                max_rate_db_per_s,
            } => CompressionPreset::Custom {
                ki,
                max_rate_db_per_s,
            },
        },
        tracking_bw: match tracking_bw {
            eigen_protocol::SineBandwidthMode::FixedHz(hz) => BandwidthMode::FixedHz(hz),
            eigen_protocol::SineBandwidthMode::Proportional { pct, max_hz } => {
                BandwidthMode::Proportional { pct, max_hz }
            }
        },
        feedforward: FeedforwardConfig {
            coherence_floor: feedforward.coherence_floor,
            h_floor_frac: feedforward.h_floor_frac,
        },
        notch: NotchGovernorConfig {
            drive_floor: notch.drive_floor,
            smoothing_ticks: notch.smoothing_ticks,
            release_ramp_db_per_s: notch.release_ramp_db_per_s,
        },
        ladder: LadderConfig {
            sysid_level_db: ladder.sysid_level_db,
            start_db: ladder.start_db,
            table_db: ladder.table_db,
            per_rung: rung_from_wire(ladder.per_rung),
            full_for: rung_from_wire(ladder.full_for),
        },
        sysid_level_db,
        coherence_floor,
        drive_vpk_ceiling,
        drive_vpk_abort,
        fault_debounce_ticks,
        kinematic_monitor,
        capture: CaptureConfig {
            rule: match capture.rule {
                eigen_protocol::SineCaptureRule::Off => CaptureRule::Off,
                eigen_protocol::SineCaptureRule::EntireTest => CaptureRule::EntireTest,
                eigen_protocol::SineCaptureRule::Manual => CaptureRule::Manual,
                eigen_protocol::SineCaptureRule::AutoFirstSeconds { n_s } => {
                    CaptureRule::AutoFirstSeconds { n_s }
                }
            },
            level_settle_s: capture.level_settle_s,
            periodic_save_s: capture.periodic_save_s,
        },
        cola: ColaConfig {
            enabled: cola.enabled,
            volts_pk: cola.volts_pk,
            waveform: match cola.waveform {
                eigen_protocol::SineColaWaveform::Sine => ColaWaveform::Sine,
                eigen_protocol::SineColaWaveform::Square => ColaWaveform::Square,
                eigen_protocol::SineColaWaveform::DcRef => ColaWaveform::DcRef,
            },
        },
        shaker,
        moving_mass,
        control_tick_frames,
        fs_hz,
        review_hold,
        metadata: metadata
            .into_iter()
            .map(|m| MetaField {
                key: m.key,
                value: m.value,
            })
            .collect(),
    };

    // The core's own validator is the authority on every numeric rule (row
    // ordering, band validity, the drive-guard ordering, the blend range). One
    // validator, the same one the local app answers to - a second copy here would
    // be a second opinion, and the two would eventually disagree.
    cfg.validate().map_err(|e| ProtocolError::InvalidArgument {
        message: e.to_string(),
    })?;
    Ok(cfg)
}

fn control_profile(p: eigen_protocol::SineControlProfile) -> SineProfile {
    SineProfile {
        label: p.label,
        rows: p
            .rows
            .into_iter()
            .map(|r| ProfileRow {
                freq_hz: r.freq_hz,
                quantity: match r.quantity {
                    eigen_protocol::SineQuantity::AccelGPk => Quantity::AccelGPk,
                    eigen_protocol::SineQuantity::VelocityIpsPk => Quantity::VelocityIpsPk,
                    eigen_protocol::SineQuantity::DisplacementInPp => Quantity::DisplacementInPp,
                },
                value: r.value,
                alarm_db: r.alarm_db,
                abort_db: r.abort_db,
            })
            .collect(),
        global_alarm_db: p.global_alarm_db,
        global_abort_db: p.global_abort_db,
    }
}

fn composite_from_wire(m: eigen_protocol::SineCompositeMode) -> CompositeMode {
    match m {
        eigen_protocol::SineCompositeMode::WeightedAvg => CompositeMode::WeightedAvg,
        eigen_protocol::SineCompositeMode::Min => CompositeMode::Min,
        eigen_protocol::SineCompositeMode::Max => CompositeMode::Max,
    }
}

fn rate_from_wire(r: eigen_protocol::SineSweepRate) -> SweepRate {
    match r {
        eigen_protocol::SineSweepRate::LogOctPerMin(v) => SweepRate::LogOctPerMin(v),
        eigen_protocol::SineSweepRate::LinearHzPerSec(v) => SweepRate::LinearHzPerSec(v),
    }
}

fn direction_from_wire(d: eigen_protocol::SweepDirection) -> SweepDirection {
    match d {
        eigen_protocol::SweepDirection::Up => SweepDirection::Up,
        eigen_protocol::SweepDirection::Down => SweepDirection::Down,
    }
}

fn dwell_from_wire(d: eigen_protocol::SineDwellFor) -> DwellFor {
    match d {
        eigen_protocol::SineDwellFor::Seconds(s) => DwellFor::Seconds(s),
        eigen_protocol::SineDwellFor::Cycles(c) => DwellFor::Cycles(c),
    }
}

fn rung_from_wire(r: eigen_protocol::SineRungFor) -> RungFor {
    match r {
        eigen_protocol::SineRungFor::Sweeps(n) => RungFor::Sweeps(n),
        eigen_protocol::SineRungFor::Seconds(s) => RungFor::Seconds(s),
    }
}

fn mode_from_wire(m: eigen_protocol::SineModeSpec) -> SineMode {
    match m {
        eigen_protocol::SineModeSpec::Sweep(s) => SineMode::Sweep(BidirSweepConfig {
            band: s.band_hz,
            rate: rate_from_wire(s.rate),
            start_hz: s.start_hz,
            initial_direction: direction_from_wire(s.initial_direction),
            target: match s.target {
                eigen_protocol::SineSweepTarget::Sweeps(n) => SweepTarget::Sweeps(n),
                eigen_protocol::SineSweepTarget::DurationS(d) => SweepTarget::DurationS(d),
                eigen_protocol::SineSweepTarget::WhicheverFirst { sweeps, duration_s } => {
                    SweepTarget::WhicheverFirst { sweeps, duration_s }
                }
            },
        }),
        eigen_protocol::SineModeSpec::Dwell(d) => SineMode::Dwell(DwellPlan {
            freq_hz: d.freq_hz,
            for_: dwell_from_wire(d.dwell_for),
        }),
        eigen_protocol::SineModeSpec::StepSine(s) => SineMode::StepSine(StepSineConfig {
            band: s.band_hz,
            steps_per_octave: s.steps_per_octave,
            spacing: match s.spacing {
                eigen_protocol::SineStepSpacing::Log => StepSpacing::Log,
                eigen_protocol::SineStepSpacing::Linear => StepSpacing::Linear,
            },
            dwell: s.dwell.map(dwell_from_wire),
        }),
        eigen_protocol::SineModeSpec::Rstd(r) => SineMode::Rstd(RstdConfig {
            band: r.band_hz,
            level_db: r.level_db,
            rate: rate_from_wire(r.rate),
            tracking_bw_hz: r.tracking_bw_hz,
            settle_k: r.settle_k,
            dwell_mode: match r.dwell_mode {
                eigen_protocol::SineRstdDwellMode::Fixed => RstdDwellMode::Fixed,
                eigen_protocol::SineRstdDwellMode::PhaseTracked => RstdDwellMode::PhaseTracked,
            },
            dwell_for: dwell_from_wire(r.dwell_for),
        }),
    }
}

// =====================================================================================
// The reverse direction - the round-trip fence's other half
// =====================================================================================

#[cfg(test)]
mod roundtrip {
    use super::*;
    use eigen_protocol::PROFILE_SCHEMA;

    /// `SineTestConfig` -> `SineWireProfile`.
    ///
    /// TEST-ONLY, and that is the honest scope: nothing in production sends a
    /// config UP this seam (the daemon receives profiles, it does not author
    /// them). It exists so the round-trip below can fail - a fidelity test with
    /// only one direction implemented can only ever test that direction against
    /// itself.
    ///
    /// **Exhaustive on the config side**, which is the half [`config_from_wire`]
    /// cannot provide: there, the config is BUILT (a literal catches a new config
    /// field), and here it is TAKEN APART (a pattern catches the same field, and
    /// forces a decision about how it crosses).
    fn wire_from_config(c: SineTestConfig) -> SineWireProfile {
        let SineTestConfig {
            profile,
            mode,
            control,
            limit_profiles,
            blend_w,
            compression,
            tracking_bw,
            feedforward,
            notch,
            ladder,
            sysid_level_db,
            coherence_floor,
            drive_vpk_ceiling,
            drive_vpk_abort,
            fault_debounce_ticks,
            kinematic_monitor,
            capture,
            cola,
            shaker,
            moving_mass,
            control_tick_frames,
            fs_hz,
            review_hold,
            metadata,
        } = c;
        SineWireProfile {
            schema_version: PROFILE_SCHEMA,
            profile: wire_profile(profile),
            mode: wire_mode(mode),
            control: eigen_protocol::SineControlSet {
                channels: control
                    .channels
                    .into_iter()
                    .map(|c| eigen_protocol::SineControlChannel {
                        index: c.index,
                        weight: c.weight,
                    })
                    .collect(),
                mode: match control.mode {
                    CompositeMode::WeightedAvg => eigen_protocol::SineCompositeMode::WeightedAvg,
                    CompositeMode::Min => eigen_protocol::SineCompositeMode::Min,
                    CompositeMode::Max => eigen_protocol::SineCompositeMode::Max,
                },
            },
            limit_profiles: limit_profiles
                .into_iter()
                .map(|lp| eigen_protocol::SineLimitProfile {
                    channel_index: lp.channel_index,
                    profile: wire_profile(lp.profile),
                    shared_name: lp.shared_name,
                })
                .collect(),
            blend_w,
            compression: match compression {
                CompressionPreset::Slow => eigen_protocol::SineCompression::Slow,
                CompressionPreset::Fast => eigen_protocol::SineCompression::Fast,
                CompressionPreset::Custom {
                    ki,
                    max_rate_db_per_s,
                } => eigen_protocol::SineCompression::Custom {
                    ki,
                    max_rate_db_per_s,
                },
            },
            tracking_bw: match tracking_bw {
                BandwidthMode::FixedHz(hz) => eigen_protocol::SineBandwidthMode::FixedHz(hz),
                BandwidthMode::Proportional { pct, max_hz } => {
                    eigen_protocol::SineBandwidthMode::Proportional { pct, max_hz }
                }
            },
            feedforward: eigen_protocol::SineFeedforwardConfig {
                coherence_floor: feedforward.coherence_floor,
                h_floor_frac: feedforward.h_floor_frac,
            },
            notch: eigen_protocol::SineNotchConfig {
                drive_floor: notch.drive_floor,
                smoothing_ticks: notch.smoothing_ticks,
                release_ramp_db_per_s: notch.release_ramp_db_per_s,
            },
            ladder: eigen_protocol::SineLadderConfig {
                sysid_level_db: ladder.sysid_level_db,
                start_db: ladder.start_db,
                table_db: ladder.table_db,
                per_rung: wire_rung(ladder.per_rung),
                full_for: wire_rung(ladder.full_for),
            },
            sysid_level_db,
            coherence_floor,
            drive_vpk_ceiling,
            drive_vpk_abort,
            fault_debounce_ticks,
            kinematic_monitor,
            capture: eigen_protocol::SineCaptureConfig {
                rule: match capture.rule {
                    CaptureRule::Off => eigen_protocol::SineCaptureRule::Off,
                    CaptureRule::EntireTest => eigen_protocol::SineCaptureRule::EntireTest,
                    CaptureRule::Manual => eigen_protocol::SineCaptureRule::Manual,
                    CaptureRule::AutoFirstSeconds { n_s } => {
                        eigen_protocol::SineCaptureRule::AutoFirstSeconds { n_s }
                    }
                },
                level_settle_s: capture.level_settle_s,
                periodic_save_s: capture.periodic_save_s,
            },
            cola: eigen_protocol::SineColaConfig {
                enabled: cola.enabled,
                volts_pk: cola.volts_pk,
                waveform: match cola.waveform {
                    ColaWaveform::Sine => eigen_protocol::SineColaWaveform::Sine,
                    ColaWaveform::Square => eigen_protocol::SineColaWaveform::Square,
                    ColaWaveform::DcRef => eigen_protocol::SineColaWaveform::DcRef,
                },
            },
            // The asymmetry, stated: TWO config fields become ONE optional wire
            // field. With no shaker there is nothing to compare a mass stack
            // against - feasibility is skipped and the kinematic monitor is off -
            // so the stack does not cross, and the round trip is faithful only
            // when it was the default. See the module docs.
            feasibility: shaker.map(|rec| eigen_protocol::FeasibilityInputs {
                shaker_json: serde_json::to_string(&rec).expect("a shaker record serializes"),
                armature_lb: moving_mass.armature_lb,
                slip_table_lb: moving_mass.slip_table_lb,
                drive_adapter_lb: moving_mass.drive_adapter_lb,
                fixturing_lb: moving_mass.fixturing_lb,
                test_article_lb: moving_mass.test_article_lb,
            }),
            control_tick_frames,
            fs_hz,
            review_hold,
            metadata: metadata
                .into_iter()
                .map(|m| eigen_protocol::SineMetaField {
                    key: m.key,
                    value: m.value,
                })
                .collect(),
        }
    }

    fn wire_profile(p: SineProfile) -> eigen_protocol::SineControlProfile {
        eigen_protocol::SineControlProfile {
            label: p.label,
            rows: p
                .rows
                .into_iter()
                .map(|r| eigen_protocol::SineProfileRow {
                    freq_hz: r.freq_hz,
                    quantity: match r.quantity {
                        Quantity::AccelGPk => eigen_protocol::SineQuantity::AccelGPk,
                        Quantity::VelocityIpsPk => eigen_protocol::SineQuantity::VelocityIpsPk,
                        Quantity::DisplacementInPp => {
                            eigen_protocol::SineQuantity::DisplacementInPp
                        }
                    },
                    value: r.value,
                    alarm_db: r.alarm_db,
                    abort_db: r.abort_db,
                })
                .collect(),
            global_alarm_db: p.global_alarm_db,
            global_abort_db: p.global_abort_db,
        }
    }

    fn wire_rung(r: RungFor) -> eigen_protocol::SineRungFor {
        match r {
            RungFor::Sweeps(n) => eigen_protocol::SineRungFor::Sweeps(n),
            RungFor::Seconds(s) => eigen_protocol::SineRungFor::Seconds(s),
        }
    }

    fn wire_rate(r: SweepRate) -> eigen_protocol::SineSweepRate {
        match r {
            SweepRate::LogOctPerMin(v) => eigen_protocol::SineSweepRate::LogOctPerMin(v),
            SweepRate::LinearHzPerSec(v) => eigen_protocol::SineSweepRate::LinearHzPerSec(v),
        }
    }

    fn wire_dwell(d: DwellFor) -> eigen_protocol::SineDwellFor {
        match d {
            DwellFor::Seconds(s) => eigen_protocol::SineDwellFor::Seconds(s),
            DwellFor::Cycles(c) => eigen_protocol::SineDwellFor::Cycles(c),
        }
    }

    fn wire_mode(m: SineMode) -> eigen_protocol::SineModeSpec {
        match m {
            SineMode::Sweep(s) => {
                eigen_protocol::SineModeSpec::Sweep(eigen_protocol::SineSweepSpec {
                    band_hz: s.band,
                    rate: wire_rate(s.rate),
                    start_hz: s.start_hz,
                    initial_direction: match s.initial_direction {
                        SweepDirection::Up => eigen_protocol::SweepDirection::Up,
                        SweepDirection::Down => eigen_protocol::SweepDirection::Down,
                    },
                    target: match s.target {
                        SweepTarget::Sweeps(n) => eigen_protocol::SineSweepTarget::Sweeps(n),
                        SweepTarget::DurationS(d) => eigen_protocol::SineSweepTarget::DurationS(d),
                        SweepTarget::WhicheverFirst { sweeps, duration_s } => {
                            eigen_protocol::SineSweepTarget::WhicheverFirst { sweeps, duration_s }
                        }
                    },
                })
            }
            SineMode::Dwell(d) => {
                eigen_protocol::SineModeSpec::Dwell(eigen_protocol::SineDwellPlan {
                    freq_hz: d.freq_hz,
                    dwell_for: wire_dwell(d.for_),
                })
            }
            SineMode::StepSine(s) => {
                eigen_protocol::SineModeSpec::StepSine(eigen_protocol::SineStepSpec {
                    band_hz: s.band,
                    steps_per_octave: s.steps_per_octave,
                    spacing: match s.spacing {
                        StepSpacing::Log => eigen_protocol::SineStepSpacing::Log,
                        StepSpacing::Linear => eigen_protocol::SineStepSpacing::Linear,
                    },
                    dwell: s.dwell.map(wire_dwell),
                })
            }
            SineMode::Rstd(r) => eigen_protocol::SineModeSpec::Rstd(eigen_protocol::SineRstdSpec {
                band_hz: r.band,
                level_db: r.level_db,
                rate: wire_rate(r.rate),
                tracking_bw_hz: r.tracking_bw_hz,
                settle_k: r.settle_k,
                dwell_mode: match r.dwell_mode {
                    RstdDwellMode::Fixed => eigen_protocol::SineRstdDwellMode::Fixed,
                    RstdDwellMode::PhaseTracked => eigen_protocol::SineRstdDwellMode::PhaseTracked,
                },
                dwell_for: wire_dwell(r.dwell_for),
            }),
        }
    }

    /// A FULLY POPULATED config - every optional set, every nested enum on a
    /// non-default arm - so a serializer or a converter that drops a field has
    /// something to drop.
    fn populated() -> SineTestConfig {
        SineTestConfig {
            profile: SineProfile {
                label: "round trip".into(),
                rows: vec![
                    ProfileRow {
                        freq_hz: 20.0,
                        quantity: Quantity::DisplacementInPp,
                        value: 0.5,
                        alarm_db: Some((1.0, 2.0)),
                        abort_db: Some((3.0, 4.0)),
                    },
                    ProfileRow {
                        freq_hz: 200.0,
                        quantity: Quantity::VelocityIpsPk,
                        value: 20.0,
                        alarm_db: None,
                        abort_db: None,
                    },
                    ProfileRow {
                        freq_hz: 2000.0,
                        quantity: Quantity::AccelGPk,
                        value: 5.0,
                        alarm_db: None,
                        abort_db: None,
                    },
                ],
                global_alarm_db: (1.5, 1.75),
                global_abort_db: (3.0, 3.25),
            },
            mode: SineMode::Sweep(BidirSweepConfig {
                band: (20.0, 2000.0),
                rate: SweepRate::LogOctPerMin(2.5),
                start_hz: 25.0,
                initial_direction: SweepDirection::Down,
                target: SweepTarget::WhicheverFirst {
                    sweeps: 4,
                    duration_s: 600.0,
                },
            }),
            control: ControlSet {
                channels: vec![
                    ControlChannel {
                        index: 1,
                        weight: 0.75,
                    },
                    ControlChannel {
                        index: 2,
                        weight: 0.25,
                    },
                ],
                mode: CompositeMode::Min,
            },
            limit_profiles: vec![LimitProfile {
                channel_index: 3,
                profile: SineProfile {
                    label: "notch".into(),
                    rows: vec![
                        ProfileRow {
                            freq_hz: 100.0,
                            quantity: Quantity::AccelGPk,
                            value: 2.0,
                            alarm_db: None,
                            abort_db: None,
                        },
                        ProfileRow {
                            freq_hz: 500.0,
                            quantity: Quantity::AccelGPk,
                            value: 1.0,
                            alarm_db: None,
                            abort_db: None,
                        },
                    ],
                    global_alarm_db: (2.0, 2.0),
                    global_abort_db: (4.0, 4.0),
                },
                shared_name: Some("fixture ceiling".into()),
            }],
            blend_w: 0.6,
            compression: CompressionPreset::Custom {
                ki: 3.5,
                max_rate_db_per_s: 33.0,
            },
            tracking_bw: BandwidthMode::Proportional {
                pct: 7.5,
                max_hz: 45.0,
            },
            feedforward: FeedforwardConfig {
                coherence_floor: 0.62,
                h_floor_frac: 0.07,
            },
            notch: NotchGovernorConfig {
                drive_floor: 0.02,
                smoothing_ticks: 4.5,
                release_ramp_db_per_s: 18.0,
            },
            ladder: LadderConfig {
                sysid_level_db: -22.0,
                start_db: -15.0,
                table_db: vec![-15.0, -9.0, -3.0, 0.0],
                per_rung: RungFor::Seconds(12.0),
                full_for: RungFor::Sweeps(3),
            },
            sysid_level_db: -21.0,
            coherence_floor: 0.55,
            drive_vpk_ceiling: 0.8,
            drive_vpk_abort: 1.6,
            fault_debounce_ticks: 7,
            kinematic_monitor: true,
            capture: CaptureConfig {
                rule: CaptureRule::AutoFirstSeconds { n_s: 4.5 },
                level_settle_s: 1.25,
                periodic_save_s: Some(30.0),
            },
            cola: ColaConfig {
                enabled: true,
                volts_pk: 0.35,
                waveform: ColaWaveform::Square,
            },
            shaker: None,
            moving_mass: eigen_shaker_db::MovingMass::default(),
            control_tick_frames: 96,
            fs_hz: None,
            review_hold: false,
            metadata: vec![
                MetaField {
                    key: "operator".into(),
                    value: "kc".into(),
                },
                MetaField {
                    key: "article".into(),
                    value: "fixture only".into(),
                },
            ],
        }
    }

    /// **THE FIDELITY FENCE'S RUNTIME HALF**: `SineTestConfig -> wire -> SineTestConfig`
    /// is the IDENTITY.
    ///
    /// This crate is the only one that depends on both types, which is why the
    /// test lives here rather than in `eigen-protocol` (which cannot see the
    /// config) or `eigen-sine` (which cannot see the wire).
    ///
    /// RED-MUTATION: drop any single field from either converter - say
    /// `kinematic_monitor`, the flag that decides whether the analytic velocity /
    /// displacement monitor runs at all - and this reds on the equality. That is
    /// exactly the silent-default shape: a safety feature authored on one end and
    /// defaulted on the other.
    #[test]
    fn a_config_round_trips_through_the_wire_unchanged() {
        let before = populated();
        let wire = wire_from_config(before.clone());
        let after = config_from_wire(wire).expect("the round trip must configure");
        assert_eq!(before, after, "the wire dropped or altered a field");
    }

    /// The SERDE leg, on the same corpus: the wire type must survive JSON, not
    /// just a struct-to-struct copy. `deny_unknown_fields` plus a
    /// `skip_serializing_if` that skipped something load-bearing would pass the
    /// test above and fail here.
    #[test]
    fn the_wire_profile_survives_json() {
        let wire = wire_from_config(populated());
        let json = serde_json::to_string(&wire).expect("serialize");
        let back: SineWireProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(wire, back);
        let cfg = config_from_wire(back).expect("configure");
        assert_eq!(cfg, populated());
    }

    /// The FEASIBILITY arm: one wire field, two config fields, and a shaker that
    /// really does come back.
    ///
    /// Separated from the corpus above because the two arms of that asymmetry
    /// cannot both be exercised by one value, and a round trip that only ever ran
    /// the `None` arm would say nothing about the shaker at all - which is the
    /// field whose absence silently disables the shaker-limit check.
    #[test]
    fn the_feasibility_inputs_round_trip_with_the_shaker() {
        let mut cfg = populated();
        cfg.shaker = Some(eigen_shaker_db::ShakerRecord::default());
        cfg.moving_mass = eigen_shaker_db::MovingMass {
            armature_lb: 120.0,
            slip_table_lb: 300.0,
            drive_adapter_lb: 15.0,
            fixturing_lb: 40.0,
            test_article_lb: 25.0,
        };
        let wire = wire_from_config(cfg.clone());
        assert!(
            wire.feasibility.is_some(),
            "a config WITH a shaker must carry feasibility inputs"
        );
        let back = config_from_wire(wire).expect("configure");
        assert_eq!(cfg, back);
    }

    /// **THE THIRD LEG OF THE GOLDEN-FIXTURE FENCE**: the exact bytes the Mac
    /// front end sends CONFIGURE on this daemon.
    ///
    /// `crates/eigen-protocol/golden/sine_wire_profile.golden.json` already fences
    /// two halves against each other - `eigen-protocol`'s `sine_golden.rs` asserts
    /// a maximal Rust profile serializes TO it, and `wireProfile.test.ts` asserts
    /// the TypeScript mapper produces it. Both of those stop at the WIRE TYPE.
    ///
    /// This is the leg neither can cover: the wire type is not what a run is built
    /// from, `SineTestConfig` is, and the converter between them is where a field
    /// can still be dropped after both fixture tests are green. Reading the same
    /// artifact means the assertion is about the bytes an operator's panel actually
    /// emits, not about a literal written in this file to match it.
    ///
    /// # `shaker_json` IS OPAQUE TO BOTH EXISTING FENCES, and this found it
    ///
    /// The fixture's shaker is the literal string `{"name":"fixture shaker"}`. That
    /// is a legal `FeasibilityInputs` - the field is a STRING on the wire, carried
    /// as its own serialized form precisely so this crate's dependency floor stays
    /// at serde - so both existing fixture tests pass it through untouched and
    /// neither ever parses it. This test is the first thing that does, and it does
    /// not parse: `ShakerRecord` requires `manufacturer`, `model` and `notes`.
    ///
    /// **Production is NOT affected, and that was checked rather than assumed.**
    /// `wireProfile.ts`'s `feasibility()` builds the string with
    /// `JSON.stringify(shaker)` from `ipc/types.ts`'s `ShakerRecord`, which declares
    /// all fourteen fields, so a real record from the shaker DB serializes complete.
    /// The fixture's value is a deliberate test-data shortcut, and it is the Rust
    /// golden's literal too (`sine_golden.rs`), so it is shared by both sides on
    /// purpose and is not one side's mistake.
    ///
    /// So the test does both halves: it SUBSTITUTES a real serialized record to
    /// prove the shape configures, and it asserts the fixture's own placeholder is
    /// REFUSED - which is the correct answer and, on UI-shaped bytes, the strongest
    /// version of the never-skip-feasibility-silently pin.
    ///
    /// RED-MUTATION: drop any arm from `config_from_wire` and this reds on the
    /// converted value or on the validator.
    #[test]
    fn the_uis_golden_wire_profile_configures_on_this_daemon() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../eigen-protocol/golden/sine_wire_profile.golden.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("the golden fixture must be readable at {path:?}: {e}"));
        let mut wire: SineWireProfile = serde_json::from_str(&text)
            .expect("the fixture must decode as a SineWireProfile (deny_unknown_fields)");

        // HALF ONE: the fixture's placeholder shaker is REFUSED, by name.
        match config_from_wire(wire.clone()) {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("shaker record"), "{message}");
                assert!(message.contains("silently skipped"), "{message}");
            }
            other => panic!(
                "the fixture's `{{\"name\":\"fixture shaker\"}}` is not a ShakerRecord and \
                 must be refused rather than run with the shaker-limit check skipped: \
                 {other:?}"
            ),
        }

        // HALF TWO: with a REAL record - what the UI's own mapper produces from
        // `ipc/types.ts`'s ShakerRecord - the UI's maximal shape configures.
        if let Some(f) = wire.feasibility.as_mut() {
            f.shaker_json =
                serde_json::to_string(&eigen_shaker_db::ShakerRecord::new("fixture shaker"))
                    .expect("a shaker record serializes");
        }
        let cfg = config_from_wire(wire.clone())
            .expect("the UI's own maximal profile must CONFIGURE, not merely decode");

        // Spot-check the fields whose loss is SILENT and whose meaning is a safety
        // decision - the silent-default shape. A `deny_unknown_fields` decode proves the
        // keys were present; these prove they arrived at the config with their values.
        assert_eq!(cfg.drive_vpk_ceiling, wire.drive_vpk_ceiling);
        assert_eq!(cfg.drive_vpk_abort, wire.drive_vpk_abort);
        assert_eq!(cfg.fault_debounce_ticks, wire.fault_debounce_ticks);
        assert_eq!(cfg.kinematic_monitor, wire.kinematic_monitor);
        assert_eq!(cfg.review_hold, wire.review_hold);
        assert_eq!(cfg.coherence_floor, wire.coherence_floor);
        assert_eq!(cfg.limit_profiles.len(), wire.limit_profiles.len());
        assert!(
            cfg.shaker.is_some(),
            "the fixture carries feasibility inputs; a run built without the shaker \
             would skip the shaker-limit check silently"
        );

        // And it survives the trip BACK, so the fixture is inside the round-trip
        // corpus rather than beside it.
        let again = config_from_wire(wire_from_config(cfg.clone())).expect("re-configure");
        assert_eq!(cfg, again);
    }

    /// A `shaker_json` that does not parse is a REFUSAL, never a run with the
    /// shaker-limit check silently skipped. The Random host's rule, and the exact
    /// failure the fidelity fence exists to end.
    #[test]
    fn an_unparseable_shaker_is_refused_rather_than_skipped() {
        let mut wire = wire_from_config(populated());
        wire.feasibility = Some(eigen_protocol::FeasibilityInputs {
            shaker_json: "{not a shaker}".into(),
            armature_lb: 1.0,
            slip_table_lb: 2.0,
            drive_adapter_lb: 3.0,
            fixturing_lb: 4.0,
            test_article_lb: 5.0,
        });
        match config_from_wire(wire) {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("shaker record"), "{message}");
                assert!(message.contains("silently skipped"), "{message}");
            }
            other => panic!("a bad shaker payload must be refused: {other:?}"),
        }
    }
}
