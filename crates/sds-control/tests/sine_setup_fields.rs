//! Every Sine wire field reaches the controller's `SineTestConfig` (or the run that
//! carries it to the controller), and every content problem is refused by name at
//! its field. Each case changes one field of the golden configuration to a value it
//! does not already hold, builds, and reads the controller's field back.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_sine::{
    BandwidthMode, ColaWaveform, CompositeMode, CompressionPreset, DwellFor, Quantity,
    RstdDwellMode, RungFor, SineMode, SineTestConfig, StepSpacing, SweepDirection, SweepRate,
    SweepTarget,
};
use sds_control::sine::config::{self, CheckContext, WireSine};
use sds_control::table;
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/request_sine_configure.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["params"]["config"].clone()
}

fn table(cola: bool) -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = ChannelRole::Control;
    let mut c2 = table::new_row(2);
    c2.role = ChannelRole::Control;
    c2.enabled = true;
    let mut c3 = table::new_row(3);
    c3.role = ChannelRole::Monitor;
    let mut c4 = table::new_row(4);
    c4.role = ChannelRole::Drive;
    c4.units = Units::VoltageV;
    let mut rows = vec![c1, c2, c3, c4];
    if cola {
        let mut c5 = table::new_row(5);
        c5.role = ChannelRole::Drive;
        c5.units = Units::VoltageV;
        rows.push(c5);
    }
    t.channels = rows;
    t
}

fn wire(c: &Value) -> WireSine {
    config::parse(c).unwrap_or_else(|e| panic!("{e:?}"))
}

fn build(c: &Value) -> SineTestConfig {
    let w = wire(c);
    let facts = sds_control::laws::builtin_sine_facts();
    w.build(&table(w.cola.enabled), &facts)
}

fn set(c: &mut Value, path: &str, v: Value) {
    let mut node = c;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        let (key, index) = match p.split_once('[') {
            Some((k, rest)) => (
                k,
                Some(rest.trim_end_matches(']').parse::<usize>().unwrap()),
            ),
            None => (*p, None),
        };
        let last = i + 1 == parts.len();
        if last && index.is_none() {
            node[key] = v;
            return;
        }
        node = &mut node[key];
        if let Some(i) = index {
            if last {
                node[i] = v;
                return;
            }
            node = &mut node[i];
        }
    }
}

type Check = fn(&SineTestConfig, &WireSine) -> bool;

#[test]
fn every_wire_field_reaches_the_controllers_configuration() {
    let cases: Vec<(&str, Value, Check)> = vec![
        ("title", json!("Another"), |_, w| w.title == "Another"),
        ("profile.label", json!("relabelled"), |c, _| {
            c.profile.label == "relabelled"
        }),
        ("profile.rows[1].value", json!(1.5), |c, _| {
            c.profile.rows[1].value == 1.5
        }),
        ("profile.rows[1].freq_hz", json!(50), |c, _| {
            c.profile.rows[1].freq_hz == 50.0
        }),
        ("profile.rows[0].quantity", json!("velocity_ips"), |c, _| {
            c.profile.rows[0].quantity == Quantity::VelocityIpsPk
        }),
        ("profile.rows[1].alarm_db", json!([0.5, 0.75]), |c, _| {
            c.profile.rows[1].alarm_db == Some((0.5, 0.75))
        }),
        ("profile.rows[1].abort_db", json!([2.5, 2.25]), |c, _| {
            c.profile.rows[1].abort_db == Some((2.5, 2.25))
        }),
        ("profile.alarm_db", json!([1.25, 1.75]), |c, _| {
            c.profile.global_alarm_db == (1.25, 1.75)
        }),
        ("profile.abort_db", json!([3.5, 2.75]), |c, _| {
            c.profile.global_abort_db == (3.5, 2.75)
        }),
        (
            "test.band_hz",
            json!([25, 1500]),
            |c, _| matches!(&c.mode, SineMode::Sweep(s) if s.band == (25.0, 1500.0)),
        ),
        (
            "test.rate",
            json!({"law": "linear", "hz_per_s": 3}),
            |c, _| matches!(&c.mode, SineMode::Sweep(s) if s.rate == SweepRate::LinearHzPerSec(3.0)),
        ),
        (
            "test.start_hz",
            json!(100),
            |c, _| matches!(&c.mode, SineMode::Sweep(s) if s.start_hz == 100.0),
        ),
        (
            "test.direction",
            json!("down"),
            |c, _| matches!(&c.mode, SineMode::Sweep(s) if s.initial_direction == SweepDirection::Down),
        ),
        (
            "test.end",
            json!({"sweeps": 6, "duration_s": 5000}),
            |c, _| {
                matches!(&c.mode, SineMode::Sweep(s) if s.target
                == SweepTarget::WhicheverFirst { sweeps: 6, duration_s: 5000.0 })
            },
        ),
        ("acquisition.fs_hz", json!(25600), |c, _| {
            c.fs_hz == Some(25_600.0) && c.control_tick_frames == 256
        }),
        ("acquisition.tick_ms", json!(20), |c, _| {
            c.control_tick_s == Some(0.02) && c.control_tick_frames == 328
        }),
        ("schedule.mode", json!("manual"), |_, w| {
            w.mode == hertzforge_sine::RunMode::Manual
        }),
        ("schedule.levels_db", json!([-9, -3, 0]), |c, _| {
            c.ladder.table_db == vec![-9.0, -3.0, 0.0]
        }),
        ("schedule.start_db", json!(-6), |c, _| {
            c.ladder.start_db == -6.0
        }),
        ("schedule.per_level", json!({"seconds": 7}), |c, _| {
            c.ladder.per_rung == RungFor::Seconds(7.0)
        }),
        ("schedule.full_level", json!({"sweeps": 3}), |c, _| {
            c.ladder.full_for == RungFor::Sweeps(3)
        }),
        ("identification.level_db", json!(-24), |c, _| {
            c.sysid_level_db == -24.0 && c.ladder.sysid_level_db == -24.0
        }),
        ("identification.gate_coherence", json!(0.7), |c, _| {
            c.coherence_floor == 0.7
        }),
        ("identification.trust_coherence", json!(0.6), |c, _| {
            c.feedforward.coherence_floor == 0.6
        }),
        ("identification.trust_h_floor_frac", json!(0.1), |c, _| {
            c.feedforward.h_floor_frac == 0.1
        }),
        // The controller always holds at review; the run thread holds or proceeds.
        ("identification.review", json!(false), |c, w| {
            c.review_hold && !w.review
        }),
        ("control.composite", json!("max"), |c, _| {
            c.control.mode == CompositeMode::Max
        }),
        (
            "control.weights",
            json!([{"channel": 2, "weight": 3}]),
            |c, _| {
                c.control
                    .channels
                    .iter()
                    .any(|ch| ch.index == 2 && ch.weight == 3.0)
                    && c.control
                        .channels
                        .iter()
                        .any(|ch| ch.index == 1 && ch.weight == 1.0)
            },
        ),
        (
            "control.tracking",
            json!({"kind": "fixed", "hz": 12}),
            |c, _| c.tracking_bw == BandwidthMode::FixedHz(12.0),
        ),
        ("control.law.params.blend_w", json!(0.25), |c, _| {
            c.blend_w == 0.25
        }),
        ("control.law.params.compression", json!("fast"), |c, _| {
            c.compression == CompressionPreset::Fast
        }),
        (
            "control.law.params",
            json!({"compression": "custom", "ki": 4.5, "max_rate_db_per_s": 33}),
            |c, _| {
                c.compression
                    == CompressionPreset::Custom {
                        ki: 4.5,
                        max_rate_db_per_s: 33.0,
                    }
            },
        ),
        // A law parameter the configuration does not carry reaches the law through
        // its choice.
        ("control.law.params.kp", json!(0.45), |_, w| {
            let facts = sds_control::laws::builtin_sine_facts();
            w.law_choice(&facts)
                .params
                .iter()
                .any(|p| p.key == "kp" && p.value == hertzforge_law_host::ParamValue::Number(0.45))
        }),
        ("control.law_budget", json!(0.5), |c, _| {
            c.law_guards.as_ref().unwrap().budget_fraction == 0.5
        }),
        ("control.law_misses", json!(5), |c, _| {
            c.law_guards.as_ref().unwrap().max_misses == 5
        }),
        ("control.max_step_db", json!(2), |c, _| {
            c.law_guards.as_ref().unwrap().max_step_db == 2.0
        }),
        ("control.max_rise_db_per_s", json!(120), |c, _| {
            c.law_guards.as_ref().unwrap().max_rise_db_per_s == 120.0
        }),
        ("control.divergence_s", json!(2.5), |c, _| {
            c.law_guards.as_ref().unwrap().divergence_s == 2.5
        }),
        ("control.fault_debounce_ticks", json!(80), |c, _| {
            c.fault_debounce_ticks == 80
        }),
        ("limits.drive_vpk_ceiling", json!(0.4), |c, _| {
            c.drive_vpk_ceiling == 0.4
        }),
        ("limits.drive_vpk_abort", json!(0.9), |c, _| {
            c.drive_vpk_abort == 0.9
        }),
        ("limits.kinematic_monitor", json!(false), |c, _| {
            !c.kinematic_monitor
        }),
        ("limits.channels[0].channel", json!(2), |c, _| {
            c.limit_profiles[0].channel_index == 2
        }),
        (
            "limits.channels[0].shared_name",
            json!("fixture"),
            |c, _| c.limit_profiles[0].shared_name.as_deref() == Some("fixture"),
        ),
        (
            "limits.channels[0].profile.rows[0].value",
            json!(4),
            |c, _| c.limit_profiles[0].profile.rows[0].value == 4.0,
        ),
        ("limits.notch.drive_floor", json!(0.02), |c, _| {
            c.notch.drive_floor == 0.02
        }),
        ("limits.notch.smoothing_ticks", json!(5), |c, _| {
            c.notch.smoothing_ticks == 5.0
        }),
        ("limits.notch.release_ramp_db_per_s", json!(10), |c, _| {
            c.notch.release_ramp_db_per_s == 10.0
        }),
        (
            "cola",
            json!({"enabled": true, "volts_pk": 0.2, "waveform": "dc"}),
            |c, _| {
                c.cola.enabled && c.cola.volts_pk == 0.2 && c.cola.waveform == ColaWaveform::DcRef
            },
        ),
        (
            "feasibility.moving_mass_lb.test_article",
            json!(20),
            |c, _| c.moving_mass.test_article_lb == 20.0,
        ),
        ("feasibility.shaker.name", json!("Other shaker"), |c, _| {
            c.shaker.as_ref().is_some_and(|s| s.name == "Other shaker")
        }),
        ("feasibility", Value::Null, |c, _| c.shaker.is_none()),
        // The run's record: the operator's words and whether the streams are kept.
        ("operator", json!("B. Operator"), |_, w| {
            w.operator == "B. Operator"
        }),
        ("article", json!("Bracket, serial 8"), |_, w| {
            w.article == "Bracket, serial 8"
        }),
        ("notes", json!("second article"), |_, w| {
            w.notes == "second article"
        }),
        (
            "specification",
            json!({"document": "Procedure 12", "clause": "4.2"}),
            |_, w| w.specification == Some(("Procedure 12".to_owned(), "4.2".to_owned())),
        ),
        ("record", json!({"streams": false}), |_, w| {
            !w.record_streams
        }),
        ("metadata[0].value", json!("8"), |c, _| {
            c.metadata[0].value == "8"
        }),
    ];
    let base = golden();
    let before = build(&base);
    let base_wire = wire(&base);
    for (path, value, check) in &cases {
        assert!(
            !check(&before, &base_wire),
            "{path}: the golden configuration already holds the value the case sets"
        );
        let mut c = base.clone();
        set(&mut c, path, value.clone());
        let built = build(&c);
        assert!(
            check(&built, &wire(&c)),
            "{path} = {value} did not reach the controller"
        );
    }
}

#[test]
fn the_other_tests_reach_the_controllers_schedulers() {
    let mut c = golden();
    c["test"] = json!({"kind": "dwell", "freq_hz": 250, "end": {"cycles": 5000}});
    c["schedule"]["per_level"] = json!({"seconds": 5});
    c["schedule"]["full_level"] = json!({"seconds": 5});
    assert!(matches!(build(&c).mode, SineMode::Dwell(d)
        if d.freq_hz == 250.0 && d.for_ == DwellFor::Cycles(5000)));
    c["test"] = json!({"kind": "step", "band_hz": [20, 2000], "steps_per_octave": 3,
                       "spacing": "linear", "dwell": {"seconds": 2}});
    assert!(matches!(build(&c).mode, SineMode::StepSine(s)
        if s.steps_per_octave == 3.0 && s.spacing == StepSpacing::Linear
            && s.dwell == Some(DwellFor::Seconds(2.0))));
    c["test"] = json!({"kind": "rstd", "band_hz": [20, 2000], "level_db": -12,
                       "rate": {"law": "log", "oct_per_min": 1}, "tracking_bw_hz": 2,
                       "settle_k": 7, "dwell_mode": "phase_tracked",
                       "dwell": {"seconds": 60}});
    assert!(matches!(build(&c).mode, SineMode::Rstd(r)
        if r.level_db == -12.0 && r.tracking_bw_hz == 2.0 && r.settle_k == 7.0
            && r.dwell_mode == RstdDwellMode::PhaseTracked
            && r.dwell_for == DwellFor::Seconds(60.0)));
}

fn refusals(c: &Value, outputs: u32) -> Vec<(String, String)> {
    let w = wire(c);
    let t = table(false);
    let law = |id: &str| sds_control::laws::sine_lookup(None, id);
    let ctx = CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
        station_outputs: outputs,
    };
    let (r, derived) = w.check(&ctx);
    if !r.errors.is_empty() {
        assert!(derived.is_none());
    }
    r.errors
        .iter()
        .map(|i| {
            let at = match &i.at {
                table::At::Field(f) => f.clone(),
                table::At::Row(n) => format!("row {n}"),
                table::At::Channel(c) => format!("channel {c:?}"),
            };
            (i.reason.to_owned(), at)
        })
        .collect()
}

#[test]
fn each_refusal_is_named_at_its_field() {
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "profile.rows",
            json!([{"freq_hz": 20, "quantity": "accel_g", "value": 1}]),
            "profile_too_short",
            "profile.rows",
        ),
        (
            "profile.rows[1].freq_hz",
            json!(10),
            "frequency_not_ascending",
            "row 1",
        ),
        (
            "profile.rows[1].value",
            json!(0),
            "value_not_positive",
            "row 1",
        ),
        (
            "test.band_hz",
            json!([10, 2000]),
            "profile_not_covering",
            "test.band_hz",
        ),
        (
            "test.band_hz",
            json!([20, 8000]),
            "band_above_grid",
            "test.band_hz",
        ),
        (
            "test.band_hz",
            json!([200, 100]),
            "out_of_range",
            "test.band_hz",
        ),
        (
            "test.rate.oct_per_min",
            json!(0),
            "out_of_range",
            "test.rate.oct_per_min",
        ),
        (
            "test.start_hz",
            json!(5000),
            "out_of_range",
            "test.start_hz",
        ),
        (
            "test.end",
            json!({"sweeps": 1}),
            "end_before_full_level",
            "test.end",
        ),
        (
            "acquisition.fs_hz",
            json!(500),
            "out_of_range",
            "acquisition.fs_hz",
        ),
        (
            "acquisition.tick_ms",
            json!(1),
            "tick_out_of_range",
            "acquisition.tick_ms",
        ),
        (
            "schedule.levels_db",
            json!([]),
            "levels_empty",
            "schedule.levels_db",
        ),
        (
            "schedule.levels_db",
            json!([0, -6]),
            "levels_not_ascending",
            "schedule.levels_db",
        ),
        (
            "schedule.start_db",
            json!(-9),
            "start_not_a_level",
            "schedule.start_db",
        ),
        (
            "identification.level_db",
            json!(3),
            "out_of_range",
            "identification.level_db",
        ),
        (
            "identification.gate_coherence",
            json!(1.5),
            "out_of_range",
            "identification.gate_coherence",
        ),
        (
            "control.weights",
            json!([{"channel": 3, "weight": 1}]),
            "weight_unknown_channel",
            "control.weights[0].channel",
        ),
        (
            "control.weights",
            json!([{"channel": 1, "weight": 0}]),
            "out_of_range",
            "control.weights[0].weight",
        ),
        (
            "control.tracking",
            json!({"kind": "fixed", "hz": 0.001}),
            "tracking_lag",
            "control.tracking",
        ),
        (
            "control.law.id",
            json!("no_such_law"),
            "law_unknown",
            "control.law.id",
        ),
        (
            "control.law.params.zeta",
            json!(1),
            "law_param_unknown",
            "control.law.params.zeta",
        ),
        (
            "control.law.params.blend_w",
            json!(3),
            "law_param_invalid",
            "control.law.params.blend_w",
        ),
        (
            "control.law_budget",
            json!(0),
            "out_of_range",
            "control.law_budget",
        ),
        (
            "control.law_misses",
            json!(0),
            "out_of_range",
            "control.law_misses",
        ),
        (
            "control.max_step_db",
            json!(0.5),
            "out_of_range",
            "control.max_step_db",
        ),
        (
            "control.max_rise_db_per_s",
            json!(0),
            "out_of_range",
            "control.max_rise_db_per_s",
        ),
        (
            "control.divergence_s",
            json!(61),
            "out_of_range",
            "control.divergence_s",
        ),
        (
            "control.fault_debounce_ticks",
            json!(1000000),
            "out_of_range",
            "control.fault_debounce_ticks",
        ),
        (
            "limits.drive_vpk_ceiling",
            json!(11),
            "out_of_range",
            "limits.drive_vpk_ceiling",
        ),
        (
            "limits.drive_vpk_abort",
            json!(0.4),
            "abort_below_ceiling",
            "limits.drive_vpk_abort",
        ),
        (
            "limits.channels[0].channel",
            json!(1),
            "limit_on_control_channel",
            "limits.channels[0].channel",
        ),
        (
            "limits.channels[0].channel",
            json!(9),
            "limit_unknown_channel",
            "limits.channels[0].channel",
        ),
        (
            "limits.channels[0].profile.rows[1].freq_hz",
            json!(1000),
            "profile_not_covering",
            "limits.channels[0].profile",
        ),
        (
            "limits.notch.drive_floor",
            json!(1),
            "out_of_range",
            "limits.notch.drive_floor",
        ),
        (
            "cola",
            json!({"enabled": true}),
            "cola_needs_two_outputs",
            "cola.enabled",
        ),
    ];
    assert!(
        refusals(&golden(), 1).is_empty(),
        "{:?}",
        refusals(&golden(), 1)
    );
    for (path, value, reason, at) in &cases {
        let mut c = golden();
        set(&mut c, path, value.clone());
        let got = refusals(&c, 1);
        assert!(
            got.iter().any(|(r, f)| r == reason && f == at),
            "{path} = {value}: expected {reason} at {at}, got {got:?}"
        );
    }
    // A wrong shape is bad_params naming the field.
    for (path, value, param) in [
        ("test.kind", json!("random"), "config.test.kind"),
        (
            "profile.rows[0].quantity",
            json!("g"),
            "config.profile.rows[0].quantity",
        ),
        ("schedule.mode", json!("sometimes"), "config.schedule.mode"),
        (
            "control.tracking.kind",
            json!("adaptive"),
            "config.control.tracking.kind",
        ),
        ("cola.waveform", json!("triangle"), "config.cola.waveform"),
        // The module's own capture is gone: every run is recorded.
        ("capture", json!({"rule": "off"}), "config.capture"),
    ] {
        let mut c = golden();
        set(&mut c, path, value);
        let e = config::parse(&c).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"), "{path}");
        assert_eq!(e.detail.as_ref().unwrap()["param"], param, "{path}");
    }
    let mut c = golden();
    c["feasibility"]["shaker"] = json!({"name": 3});
    assert_eq!(
        config::parse(&c).unwrap_err().reason,
        Some("shaker_record_invalid")
    );
}

#[test]
fn the_tick_converts_at_the_rate_and_a_law_file_needs_eight_ms() {
    let mut c = golden();
    c["acquisition"]["tick_ms"] = json!(5);
    assert!(refusals(&c, 1).is_empty());
    let w = wire(&c);
    let t = table(false);
    let file = |_: &str| {
        let mut f = sds_control::laws::builtin_sine_facts();
        f.builtin = false;
        f.id = "a".repeat(64);
        f.sha256 = "a".repeat(64);
        Some(f)
    };
    let ctx = CheckContext {
        table: &t,
        law: &file,
        timeout_s: 30.0,
        station_outputs: 1,
    };
    let (r, _) = w.check(&ctx);
    assert!(
        r.errors
            .iter()
            .any(|i| i.reason == "tick_out_of_range" && i.message.contains("law file")),
        "{:?}",
        r.errors
    );
    assert_eq!(config::tick_frames(16_384.0, 10.0), 164);
    assert_eq!(config::tick_frames(17_066.666_666_7, 10.0), 171);
}

#[test]
fn a_one_level_dwell_needs_no_time_at_the_levels_before_full_level() {
    // The time at each level before the last is the default `{"sweeps": 1}`; with one level it
    // is never used, so a dwell that gives only its full-level time configures.
    let mut c = golden();
    c["test"] = json!({"kind": "dwell", "freq_hz": 100, "end": {"seconds": 30}});
    c["schedule"] = json!({"mode": "auto", "levels_db": [0], "start_db": 0,
                           "full_level": {"seconds": 30}});
    assert!(refusals(&c, 1).is_empty(), "{:?}", refusals(&c, 1));
    // Starting at the last level of several leaves the levels before it unused too.
    c["schedule"] = json!({"mode": "auto", "levels_db": [-6, 0], "start_db": 0,
                           "full_level": {"seconds": 30}});
    assert!(refusals(&c, 1).is_empty(), "{:?}", refusals(&c, 1));
    // With a level still to come, the time there is needed and a sweep count is refused.
    c["schedule"] = json!({"mode": "auto", "levels_db": [-6, 0], "start_db": -6,
                           "full_level": {"seconds": 30}});
    assert!(refusals(&c, 1)
        .iter()
        .any(|(r, f)| r == "out_of_range" && f == "schedule.per_level.sweeps"));
}

/// The built-in named without its compression runs at the compression its
/// manifest declares as the default, as its law does: the configuration's
/// compression sets the settle window, so the two must agree.
#[test]
fn the_built_in_named_without_its_compression_takes_its_declared_default() {
    let mut c = golden();
    c["control"]["law"]["params"]
        .as_object_mut()
        .unwrap()
        .remove("compression");
    let w = wire(&c);
    let mut facts = sds_control::laws::builtin_sine_facts();
    for (name, want) in [
        ("fast", CompressionPreset::Fast),
        ("slow", CompressionPreset::Slow),
    ] {
        for p in facts.params.iter_mut().filter(|p| p.key == "compression") {
            if let hertzforge_law_host::ParamKind::Choice(spec) = &mut p.kind {
                spec.default = name.into();
            }
        }
        assert_eq!(
            w.build(&table(w.cola.enabled), &facts).compression,
            want,
            "the manifest declares {name}"
        );
    }
}
