//! Every field of the classical shock configuration on the wire reaches the
//! controller's `ClassicalTestConfig` and its `ClassicalControl`, and each refusal
//! comes back by name with the field it is about.
//!
//! Each case sets one wire field to a value no default has and reads it back from
//! the built configuration, so a field the conversion drops fails its own case.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_dsp::shock::CompensationShape;
use hertzforge_shock_common::{DriveMonitorKind, LadderAdvance, ShockRunMode, Spacing};
use hertzforge_shock_host::{
    ClassicalShape, ClassicalTestConfig, Polarity, ToleranceAbort, ToleranceStandard,
};
use sds_control::shock::config::{self, Advance, CheckContext, LawFacts, WireShock};
use sds_control::table;
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden_config() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/request_shock_configure.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["params"]["config"].clone()
}

fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = ChannelRole::Control;
    let mut c2 = table::new_row(2);
    c2.role = ChannelRole::Monitor;
    c2.units = Units::Custom("A".into());
    let mut c3 = table::new_row(3);
    c3.role = ChannelRole::Drive;
    c3.units = Units::VoltageV;
    let mut c4 = table::new_row(4);
    c4.role = ChannelRole::Monitor;
    t.channels = vec![c1, c2, c3, c4];
    t
}

fn law(id: &str) -> Option<LawFacts> {
    match id {
        "fd_correct" => Some(sds_control::laws::builtin_classical_facts()),
        "pinv_closed" => Some(LawFacts {
            id: id.into(),
            name: "pseudo-inverse, closed loop".into(),
            version: "1.0.0".into(),
            sha256: String::new(),
            builtin: true,
            world: Err("multiple-output Random"),
            params: Vec::new(),
        }),
        _ => None,
    }
}

fn set(v: &mut Value, path: &str, value: Value) {
    let mut at = v;
    let parts: Vec<&str> = path.split('.').collect();
    for p in &parts[..parts.len() - 1] {
        at = &mut at[*p];
    }
    at[parts[parts.len() - 1]] = value;
}

fn parse_with(path: &str, value: Value) -> WireShock {
    let mut c = golden_config();
    set(&mut c, path, value);
    config::parse(&c).unwrap_or_else(|e| panic!("{path}: {}", e.message))
}

/// Build and check: the controller's configuration when the configuration is
/// valid.
fn built(w: &WireShock) -> ClassicalTestConfig {
    let t = table();
    let ctx = CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    };
    let (r, d) = w.check(&ctx);
    assert!(r.errors.is_empty() && d.is_some(), "{:?}", r.errors);
    let cfg = w.build();
    cfg.validate().unwrap();
    cfg
}

type Read = fn(&ClassicalTestConfig) -> Value;

fn c(cfg: &ClassicalTestConfig) -> &hertzforge_shock_host::ClassicalControl {
    cfg.control.as_ref().expect("the wire always sets control")
}

#[test]
fn every_wire_field_reaches_the_controllers_configuration() {
    let cases: Vec<(&str, Value, Read, Value)> = vec![
        (
            "pulse.shape",
            json!("haversine"),
            |k| json!(k.spec.shape == ClassicalShape::Haversine),
            json!(true),
        ),
        (
            "pulse.amplitude_g",
            json!(17.5),
            |k| json!(k.spec.amplitude_g),
            json!(17.5),
        ),
        (
            "pulse.duration_ms",
            json!(6.5),
            |k| json!(k.spec.duration_ms),
            json!(6.5),
        ),
        (
            "pulse.polarity",
            json!("negative"),
            |k| json!(c(k).polarity == Polarity::Negative),
            json!(true),
        ),
        (
            "pulse.compensation.shape",
            json!("rounded_rectangular"),
            |k| json!(k.spec.compensation.shape == CompensationShape::RoundedRectangular),
            json!(true),
        ),
        (
            "pulse.compensation.pre_frac",
            json!(5.5),
            |k| json!(k.spec.compensation.pre_frac),
            json!(5.5),
        ),
        (
            "pulse.compensation.symmetric",
            json!(false),
            |k| json!(k.spec.compensation.symmetric),
            json!(false),
        ),
        (
            "standard.name",
            json!("iec_60068_2_27"),
            |k| json!(k.standard == ToleranceStandard::Iec60068_2_27),
            json!(true),
        ),
        (
            "standard.tolerance_abort",
            json!("every_level"),
            |k| {
                json!([
                    c(k).tolerance_abort == ToleranceAbort::EveryLevel,
                    k.abort_on_tolerance_failure
                ])
            },
            json!([true, true]),
        ),
        (
            "standard.overshoot_abort",
            json!(0.25),
            |k| json!(c(k).overshoot_abort),
            json!(0.25),
        ),
        (
            "acquisition.fs_hz",
            json!(40960),
            |k| json!(k.fs_hz),
            json!(40960.0),
        ),
        (
            "acquisition.srs.f_lo_hz",
            json!(30),
            |k| json!(k.srs_f_lo_hz),
            json!(30.0),
        ),
        (
            "acquisition.srs.f_hi_hz",
            json!(1500),
            |k| json!(k.srs_f_hi_hz),
            json!(1500.0),
        ),
        (
            "acquisition.srs.bands_per_octave",
            json!(6),
            |k| json!(k.srs_bands_per_octave),
            json!(6.0),
        ),
        (
            "acquisition.srs.band_tol_db",
            json!(2.5),
            |k| json!(k.srs_band_tol_db),
            json!(2.5),
        ),
        (
            "acquisition.pvss",
            json!(true),
            |k| json!(k.compute_pvss),
            json!(true),
        ),
        (
            "schedule.mode",
            json!("manual"),
            |k| json!(k.schedule.mode == ShockRunMode::Manual),
            json!(true),
        ),
        (
            "schedule.levels_db",
            json!([-12, -6, -4, -2, 0]),
            |k| json!(k.schedule.table_db),
            json!([-12.0, -6.0, -4.0, -2.0, 0.0]),
        ),
        (
            "schedule.pulses",
            json!([2]),
            |k| json!(k.schedule.pulses_per_level),
            json!([2]),
        ),
        (
            "schedule.start_db",
            json!(-9),
            |k| json!(k.schedule.start_db),
            json!(-9.0),
        ),
        (
            "spacing.cooling_s",
            json!(1.75),
            |k| json!(k.rate_model.cooling_floor_s),
            json!(1.75),
        ),
        (
            "spacing.settling_s",
            json!(0.75),
            |k| json!(k.rate_model.settling_floor_s),
            json!(0.75),
        ),
        (
            "spacing.compute_s",
            json!(0.4),
            |k| json!(k.rate_model.compute_floor_s),
            json!(0.4),
        ),
        (
            "identification.drive_vrms",
            json!(0.07),
            |k| json!(c(k).sysid.drive_rms),
            json!(0.07),
        ),
        (
            "identification.duration_s",
            json!(3),
            |k| json!(c(k).sysid.duration_s),
            json!(3.0),
        ),
        (
            "identification.nfft",
            json!(4096),
            |k| json!(c(k).sysid.nfft),
            json!(4096),
        ),
        (
            "identification.band_hz",
            json!([10, 4000]),
            |k| json!([c(k).sysid.f_lo_hz, c(k).sysid.f_hi_hz]),
            json!([10.0, 4000.0]),
        ),
        (
            "identification.min_mean_coherence",
            json!(0.85),
            |k| json!(c(k).sysid.min_mean_coherence),
            json!(0.85),
        ),
        (
            "identification.review",
            json!(false),
            |k| json!(c(k).review_hold),
            json!(false),
        ),
        (
            "control.band_hz",
            json!([8, 4500]),
            |k| json!([c(k).band_lo_hz, c(k).band_hi_hz]),
            json!([8.0, 4500.0]),
        ),
        (
            "control.coherence_floor",
            json!(0.85),
            |k| json!(c(k).inverse.coherence_floor),
            json!(0.85),
        ),
        (
            "control.tikhonov_floor",
            json!(0.08),
            |k| json!(c(k).inverse.tikhonov_beta_floor),
            json!(0.08),
        ),
        (
            "control.ref_floor_db",
            json!(40),
            |k| json!(c(k).ref_floor_db),
            json!(40.0),
        ),
        (
            "control.min_band_energy",
            json!(0.85),
            |k| json!(c(k).min_band_energy),
            json!(0.85),
        ),
        (
            "control.law_budget",
            json!(0.2),
            |k| json!(c(k).guards.budget_fraction),
            json!(0.2),
        ),
        (
            "control.law_misses",
            json!(4),
            |k| json!(c(k).guards.max_misses),
            json!(4),
        ),
        (
            "control.max_pulses",
            json!(50),
            |k| json!(c(k).guards.max_pulses),
            json!(50),
        ),
        (
            "control.max_peak_rise_db",
            json!(8),
            |k| json!(c(k).max_peak_rise_db),
            json!(8.0),
        ),
        (
            "control.max_bin_rise_db",
            json!(9),
            |k| json!(c(k).max_bin_rise_db),
            json!(9.0),
        ),
        (
            "control.first_drive_ratio",
            json!(1.5),
            |k| json!(c(k).first_drive_ratio),
            json!(1.5),
        ),
        (
            "control.divergence_pulses",
            json!(4),
            |k| json!(c(k).divergence_pulses),
            json!(4),
        ),
        (
            "limits.drive_vpk_ceiling",
            json!(0.6),
            |k| json!(c(k).drive_vpk_ceiling),
            json!(0.6),
        ),
        (
            "limits.drive_vpk_abort",
            json!(0.9),
            |k| json!(c(k).drive_vpk_abort),
            json!(0.9),
        ),
        (
            "limits.peak_accel_g",
            json!(25),
            |k| json!(k.drive_limit.peak_accel_g),
            json!(25.0),
        ),
        (
            "limits.monitors",
            json!([{"channel": 4, "unit": "g", "peak_ceiling": 30, "kind": "predictive"}]),
            |k| {
                let m = &k.drive_limit.monitors[0];
                json!([
                    m.channel,
                    m.eu,
                    m.peak_ceiling,
                    m.kind == DriveMonitorKind::Predictive
                ])
            },
            json!(["ch4", "g", 30.0, true]),
        ),
        (
            "feasibility.moving_mass_lb.test_article",
            json!(20),
            |k| json!(k.moving_mass.test_article_lb),
            json!(20.0),
        ),
        (
            "feasibility.residual_velocity_ips",
            json!(0.2),
            |k| json!(k.residual_velocity_tol_ips),
            json!(0.2),
        ),
        (
            "feasibility.residual_disp_in",
            json!(0.002),
            |k| json!(k.residual_disp_tol_in),
            json!(0.002),
        ),
        (
            "feasibility.shaker.name",
            json!("Second shaker"),
            |k| json!(k.shaker.as_ref().map(|s| s.name.clone())),
            json!("Second shaker"),
        ),
        ("train.count", json!(4), |k| json!(k.train.count), json!(4)),
        (
            "train.spacing_s",
            json!(7.5),
            |k| json!(matches!(k.train.spacing, Spacing::Delay(s) if s == 7.5)),
            json!(true),
        ),
        (
            "gunfire.rounds_per_min",
            json!(20),
            |k| json!(k.gunfire.map(|g| g.rounds_per_min)),
            json!(20.0),
        ),
        (
            "gunfire.rounds",
            json!(5),
            |k| json!(k.gunfire.map(|g| g.rounds)),
            json!(5),
        ),
        (
            "capture.auto_start",
            json!(true),
            |k| json!(k.capture.auto_start),
            json!(true),
        ),
        (
            "capture.run_name",
            json!("bracket_x"),
            |k| json!(k.capture.run_name),
            json!("bracket_x"),
        ),
        (
            "metadata",
            json!([{"key": "lot", "label": "Lot", "value": "A7"}]),
            |k| json!([k.metadata[0].key, k.metadata[0].value]),
            json!(["lot", "A7"]),
        ),
    ];
    for (path, value, read, want) in cases {
        let w = parse_with(path, value);
        let got = read(&built(&w));
        assert_eq!(
            got, want,
            "{path} did not reach the controller's configuration"
        );
    }
}

#[test]
fn the_ladders_advance_rule_and_per_level_limit_and_the_law_reach_the_run() {
    let w = parse_with("schedule.advance", json!("count"));
    assert_eq!(w.advance, Advance::Count);
    assert_eq!(w.build().schedule.advance, LadderAdvance::Count);
    let w = parse_with("schedule.advance", json!("on_pass"));
    assert_eq!(w.build().schedule.advance, LadderAdvance::OnPass);
    let w = parse_with("schedule.max_pulses_per_level", json!(7));
    assert_eq!(w.max_pulses_per_level, 7);
    assert_eq!(w.build().schedule.max_pulses_per_level, 7);
    // The wire's defaults: advance on count, five pulses a level.
    let mut c = golden_config();
    c["schedule"].as_object_mut().unwrap().remove("advance");
    c["schedule"]
        .as_object_mut()
        .unwrap()
        .remove("max_pulses_per_level");
    let w = config::parse(&c).unwrap();
    assert_eq!((w.advance, w.max_pulses_per_level), (Advance::Count, 5));
    // The law and its values, as a catalog choice.
    let w = parse_with(
        "control.law",
        json!({"id": "fd_correct", "params": {"beta": 0.25, "initial_backoff_db": 3}}),
    );
    let choice = w.law_choice(&law("fd_correct").unwrap());
    assert_eq!(choice.id, "fd_correct");
    let keys: Vec<&str> = choice.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["beta", "initial_backoff_db"]);
}

#[test]
fn the_wires_defaults_are_the_contracts() {
    let w = config::parse(&json!({
        "pulse": {"shape": "half_sine", "amplitude_g": 1, "duration_ms": 11},
        "schedule": {"mode": "auto", "levels_db": [-6, 0], "pulses": [1]},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    }))
    .unwrap();
    let k = built(&w);
    // Rectangular shoulders 4 times the duration, not the library's half-sine 1.5.
    assert_eq!(k.spec.compensation.shape, CompensationShape::Rectangular);
    assert_eq!(k.spec.compensation.pre_frac, 4.0);
    assert_eq!(k.fs_hz, 51_200.0);
    assert_eq!(k.schedule.start_db, -6.0, "the first level");
    let c = c(&k);
    assert_eq!((c.band_lo_hz, c.band_hi_hz), (5.0, 5000.0));
    assert_eq!(c.tolerance_abort, ToleranceAbort::FullLevel);
    assert!(!k.abort_on_tolerance_failure);
    assert!(c.review_hold);
    assert_eq!(w.law.id, "fd_correct");
    // At a lower rate the band's default follows it.
    let mut v = json!({
        "pulse": {"shape": "half_sine", "amplitude_g": 1, "duration_ms": 11},
        "acquisition": {"fs_hz": 8192},
        "schedule": {"mode": "auto", "levels_db": [0], "pulses": [1]},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    });
    let w = config::parse(&v).unwrap();
    assert_eq!(w.band_hz.1, 0.4 * 8192.0);
    v["acquisition"]["fs_hz"] = json!(51200);
    assert_eq!(config::parse(&v).unwrap().band_hz.1, 5000.0);
}

fn errors_of(c: &Value) -> Vec<(String, String)> {
    let w = config::parse(c).unwrap();
    let t = table();
    let ctx = CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    };
    let (r, _) = w.check(&ctx);
    r.errors
        .iter()
        .map(|i| {
            let j = i.to_json();
            (
                j["reason"].as_str().unwrap().to_owned(),
                j["field"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[test]
fn each_content_error_is_refused_by_name_at_its_field() {
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "pulse.amplitude_g",
            json!(0),
            "out_of_range",
            "pulse.amplitude_g",
        ),
        (
            "pulse.duration_ms",
            json!(2000),
            "out_of_range",
            "pulse.duration_ms",
        ),
        (
            "pulse.compensation.pre_frac",
            json!(0),
            "out_of_range",
            "pulse.compensation.pre_frac",
        ),
        (
            "standard.overshoot_abort",
            json!(3),
            "out_of_range",
            "standard.overshoot_abort",
        ),
        (
            "acquisition.fs_hz",
            json!(500),
            "out_of_range",
            "acquisition.fs_hz",
        ),
        (
            "acquisition.srs.f_hi_hz",
            json!(30000),
            "band_above_grid",
            "acquisition.srs.f_hi_hz",
        ),
        (
            "schedule.levels_db",
            json!([]),
            "levels_empty",
            "schedule.levels_db",
        ),
        (
            "schedule.levels_db",
            json!([-6, -12, 0]),
            "levels_not_ascending",
            "schedule.levels_db[1]",
        ),
        (
            "schedule.levels_db",
            json!([-80, 0]),
            "out_of_range",
            "schedule.levels_db[0]",
        ),
        (
            "schedule.pulses",
            json!([1, 1]),
            "pulses_length_mismatch",
            "schedule.pulses",
        ),
        (
            "schedule.pulses",
            json!([0]),
            "out_of_range",
            "schedule.pulses[0]",
        ),
        (
            "schedule.start_db",
            json!(3),
            "out_of_range",
            "schedule.start_db",
        ),
        (
            "schedule.max_pulses_per_level",
            json!(0),
            "out_of_range",
            "schedule.max_pulses_per_level",
        ),
        (
            "spacing.cooling_s",
            json!(-1),
            "out_of_range",
            "spacing.cooling_s",
        ),
        (
            "identification.drive_vrms",
            json!(0.6),
            "out_of_range",
            "identification.drive_vrms",
        ),
        (
            "identification.nfft",
            json!(3000),
            "out_of_range",
            "identification.nfft",
        ),
        (
            "identification.duration_s",
            json!(0.05),
            "out_of_range",
            "identification.duration_s",
        ),
        (
            "identification.band_hz",
            json!([5, 30000]),
            "band_above_grid",
            "identification.band_hz",
        ),
        (
            "identification.min_mean_coherence",
            json!(0),
            "out_of_range",
            "identification.min_mean_coherence",
        ),
        (
            "control.band_hz",
            json!([0, 4000]),
            "out_of_range",
            "control.band_hz",
        ),
        (
            "control.tikhonov_floor",
            json!(1),
            "out_of_range",
            "control.tikhonov_floor",
        ),
        (
            "control.law_budget",
            json!(1.5),
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
            "control.max_bin_rise_db",
            json!(6.5),
            "out_of_range",
            "control.max_bin_rise_db",
        ),
        (
            "control.first_drive_ratio",
            json!(0.5),
            "out_of_range",
            "control.first_drive_ratio",
        ),
        (
            "control.law",
            json!({"id": "nope"}),
            "law_unknown",
            "control.law.id",
        ),
        (
            "control.law",
            json!({"id": "pinv_closed"}),
            "law_wrong_world",
            "control.law.id",
        ),
        (
            "control.law",
            json!({"id": "fd_correct", "params": {"gain": 1}}),
            "law_param_unknown",
            "control.law.params.gain",
        ),
        (
            "control.law",
            json!({"id": "fd_correct", "params": {"beta": 2}}),
            "law_param_invalid",
            "control.law.params.beta",
        ),
        (
            "limits.drive_vpk_ceiling",
            json!(12),
            "out_of_range",
            "limits.drive_vpk_ceiling",
        ),
        (
            "limits.drive_vpk_abort",
            json!(0.4),
            "out_of_range",
            "limits.drive_vpk_abort",
        ),
        (
            "limits.peak_accel_g",
            json!(0),
            "out_of_range",
            "limits.peak_accel_g",
        ),
        (
            "limits.monitors",
            json!([{"channel": 9, "unit": "A", "peak_ceiling": 40, "kind": "between_pulse"}]),
            "limit_unknown_channel",
            "limits.monitors[0].channel",
        ),
        (
            "feasibility.residual_disp_in",
            json!(0),
            "out_of_range",
            "feasibility.residual_disp_in",
        ),
        (
            "train.spacing_s",
            json!(0.5),
            "spacing_below_floor",
            "train.spacing_s",
        ),
        ("train.count", json!(0), "out_of_range", "train.count"),
        (
            "gunfire.rounds_per_min",
            json!(600),
            "spacing_below_floor",
            "gunfire.rounds_per_min",
        ),
        ("gunfire.rounds", json!(0), "out_of_range", "gunfire.rounds"),
    ];
    for (path, value, reason, field) in cases {
        let mut c = golden_config();
        set(&mut c, path, value.clone());
        let errors = errors_of(&c);
        assert!(
            errors.iter().any(|(r, f)| r == reason && f == field),
            "{path} = {value}: wanted {reason} at {field}, got {errors:?}"
        );
    }
    // A pulse longer than the controller timeout cannot be safe-stopped behind.
    let mut c = golden_config();
    set(&mut c, "pulse.duration_ms", json!(900));
    set(&mut c, "pulse.compensation.pre_frac", json!(20));
    let w = config::parse(&c).unwrap();
    let t = table();
    let (r, _) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(
        r.errors.iter().any(|i| i.reason == "record_too_long"),
        "{:?}",
        r.errors
    );
}

#[test]
fn the_table_must_have_a_control_and_a_drive() {
    let w = config::parse(&golden_config()).unwrap();
    let mut t = table();
    t.channels.retain(|c| c.role != ChannelRole::Drive);
    let (r, _) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(r.errors.iter().any(|i| i.reason == "no_drive_channel"));
    let mut t = table();
    t.channels.retain(|c| c.role != ChannelRole::Control);
    let (r, _) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(r.errors.iter().any(|i| i.reason == "no_control_channel"));
}

#[test]
fn a_composite_out_of_its_quiet_window_is_a_warning_not_an_error() {
    let mut c = golden_config();
    set(
        &mut c,
        "pulse.compensation",
        json!({"shape": "half_sine", "pre_frac": 1.5, "post_frac": 1.5, "symmetric": true}),
    );
    let w = config::parse(&c).unwrap();
    let t = table();
    let (r, d) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.warnings[0].reason, "quiet_window_exceeded");
    assert!(!d.unwrap().quiet_window.inside());
}

#[test]
fn each_shape_error_is_bad_params_naming_the_field() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("pulse.shape", json!("square"), "config.pulse.shape"),
        (
            "pulse.amplitude_g",
            json!("big"),
            "config.pulse.amplitude_g",
        ),
        ("pulse.rise_frac", json!(0.1), "config.pulse.rise_frac"),
        ("pulse.polarity", json!("up"), "config.pulse.polarity"),
        (
            "pulse.compensation.shape",
            json!("cosine"),
            "config.pulse.compensation.shape",
        ),
        (
            "standard.name",
            json!("mil_std_810"),
            "config.standard.name",
        ),
        (
            "standard.tolerance_abort",
            json!("never"),
            "config.standard.tolerance_abort",
        ),
        ("schedule.mode", json!("hold"), "config.schedule.mode"),
        (
            "schedule.levels_db",
            json!("all"),
            "config.schedule.levels_db",
        ),
        ("schedule.pulses", json!([1.5]), "config.schedule.pulses[0]"),
        (
            "schedule.advance",
            json!("sometimes"),
            "config.schedule.advance",
        ),
        (
            "identification.band_hz",
            json!([5]),
            "config.identification.band_hz",
        ),
        (
            "identification.review",
            json!("yes"),
            "config.identification.review",
        ),
        (
            "control.law",
            json!({"params": {}}),
            "config.control.law.id",
        ),
        (
            "limits.drive_vpk_ceiling",
            Value::Null,
            "config.limits.drive_vpk_ceiling",
        ),
        (
            "limits.monitors",
            json!([{"channel": 2, "unit": "A", "peak_ceiling": 40, "kind": "after"}]),
            "config.limits.monitors[0].kind",
        ),
        (
            "feasibility.moving_mass_lb.armature",
            json!(-1),
            "config.feasibility.moving_mass_lb.armature",
        ),
        ("train.count", Value::Null, "config.train.count"),
        (
            "metadata",
            json!([{"key": "a"}]),
            "config.metadata[0].label",
        ),
    ];
    for (path, value, param) in cases {
        let mut c = golden_config();
        set(&mut c, path, value.clone());
        let e = config::parse(&c).map(|_| ()).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"), "{path}");
        assert_eq!(
            e.detail.as_deref().map(|d| d["param"].clone()),
            Some(json!(param)),
            "{path} = {value}: {}",
            e.message
        );
    }
    // A shaker record that does not parse is its own refusal.
    let mut c = golden_config();
    set(&mut c, "feasibility.shaker", json!({"name": 3}));
    let e = config::parse(&c).map(|_| ()).unwrap_err();
    assert_eq!(e.reason, Some("shaker_record_invalid"));
}

#[test]
fn mimo_shock_classical_refuses_a_second_drive_row() {
    // Single-output shock plays one drive record. With two enabled drive rows a station opens
    // both as outputs and the one record would be split across them as interleaved frames.
    let w = config::parse(&golden_config()).unwrap();
    let mut t = table();
    let mut second = table::new_row(5);
    second.role = ChannelRole::Drive;
    second.units = Units::VoltageV;
    t.channels.push(second);
    let (r, _) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(
        r.errors.iter().any(|i| i.reason == "extra_drive_channels"),
        "{:?}",
        r.errors
    );
    // A disabled second drive row is not refused.
    t.channels.last_mut().unwrap().enabled = false;
    let (r, _) = w.check(&CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
    });
    assert!(
        !r.errors.iter().any(|i| i.reason == "extra_drive_channels"),
        "{:?}",
        r.errors
    );
}
