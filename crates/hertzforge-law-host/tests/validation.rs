//! The host's checks on manifests, parameter values, setups, loop inputs and
//! loop outputs, without a WebAssembly component. Each test starts from a
//! value that passes and shows that each single change it names is refused,
//! so a rule that stopped being enforced fails here.

use hertzforge_law_host::validate::{
    resolve_params, validate_inputs, validate_manifest, validate_outputs, validate_setup,
};
use hertzforge_law_host::{
    Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, DiagnosticValue, IntegerSpec, LawError,
    LoopDiagnostics, LoopInputs, LoopOutputs, Manifest, NumberSpec, OutputRule, Param, ParamKind,
    ParamSpec, ParamValue, Setup,
};

fn number(min: f64, max: f64, step: f64, default: f64) -> NumberSpec {
    NumberSpec {
        unit: "dB".into(),
        min,
        max,
        step,
        default,
    }
}

fn spec(key: &str, kind: ParamKind) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: format!("Label of {key}"),
        help: "Help.".into(),
        kind,
    }
}

/// A manifest with one parameter of every kind and two diagnostics.
fn good_manifest() -> Manifest {
    Manifest {
        name: "every kind".into(),
        version: "1.2.3".into(),
        author: "a test".into(),
        summary: "A manifest using every parameter kind.".into(),
        params: vec![
            spec("gain", ParamKind::Number(number(0.0, 1.0, 0.05, 0.5))),
            spec(
                "loops",
                ParamKind::Integer(IntegerSpec {
                    unit: String::new(),
                    min: 1,
                    max: 10,
                    step: 1,
                    default: 3,
                }),
            ),
            spec("smooth", ParamKind::Toggle(true)),
            spec(
                "method",
                ParamKind::Choice(ChoiceSpec {
                    options: vec!["fast".into(), "careful".into()],
                    default: "careful".into(),
                }),
            ),
            spec(
                "weight_curve",
                ParamKind::Curve(CurveSpec {
                    unit: "dB".into(),
                    min: -20.0,
                    max: 20.0,
                    default: vec![
                        Breakpoint {
                            frequency_hz: 20.0,
                            value: 0.0,
                        },
                        Breakpoint {
                            frequency_hz: 2000.0,
                            value: -3.0,
                        },
                    ],
                }),
            ),
            spec(
                "channel_weight",
                ParamKind::PerChannel(number(0.0, 10.0, 0.1, 1.0)),
            ),
        ],
        diagnostics: vec![
            DiagnosticSpec {
                key: "untrusted_lines".into(),
                label: "Untrusted lines".into(),
                unit: "lines".into(),
                help: String::new(),
            },
            DiagnosticSpec {
                key: "condition".into(),
                label: "Condition number".into(),
                unit: String::new(),
                help: String::new(),
            },
        ],
    }
}

fn param_mut<'a>(m: &'a mut Manifest, key: &str) -> &'a mut ParamKind {
    &mut m
        .params
        .iter_mut()
        .find(|p| p.key == key)
        .expect("the parameter is in the good manifest")
        .kind
}

#[test]
fn a_manifest_using_every_kind_passes() {
    assert_eq!(validate_manifest(&good_manifest()), Ok(()));
}

#[test]
fn each_manifest_rule_refuses_its_case() {
    type Mutation = (&'static str, fn(&mut Manifest));
    let cases: Vec<Mutation> = vec![
        ("empty name", |m| m.name = String::new()),
        ("blank version", |m| m.version = "   ".into()),
        ("name too long", |m| m.name = "x".repeat(129)),
        ("control character in summary", |m| {
            m.summary = "a\u{7}b".into()
        }),
        ("upper-case key", |m| m.params[0].key = "Gain".into()),
        ("key with a hyphen", |m| m.params[0].key = "max-step".into()),
        ("key starting with a digit", |m| {
            m.params[0].key = "2gain".into()
        }),
        ("key too long", |m| m.params[0].key = "k".repeat(65)),
        ("duplicate parameter key", |m| {
            m.params[1].key = "gain".into()
        }),
        ("empty label", |m| m.params[0].label = " ".into()),
        ("min above max", |m| {
            *param_mut(m, "gain") = ParamKind::Number(number(2.0, 1.0, 0.1, 1.5))
        }),
        ("default above max", |m| {
            *param_mut(m, "gain") = ParamKind::Number(number(0.0, 1.0, 0.1, 1.5))
        }),
        ("zero step", |m| {
            *param_mut(m, "gain") = ParamKind::Number(number(0.0, 1.0, 0.0, 0.5))
        }),
        ("NaN min", |m| {
            *param_mut(m, "gain") = ParamKind::Number(number(f64::NAN, 1.0, 0.1, 0.5))
        }),
        ("infinite max", |m| {
            *param_mut(m, "gain") = ParamKind::Number(number(0.0, f64::INFINITY, 0.1, 0.5))
        }),
        ("integer step below one", |m| {
            if let ParamKind::Integer(s) = param_mut(m, "loops") {
                s.step = 0;
            }
        }),
        ("integer default below min", |m| {
            if let ParamKind::Integer(s) = param_mut(m, "loops") {
                s.default = 0;
            }
        }),
        ("choice default not an option", |m| {
            if let ParamKind::Choice(s) = param_mut(m, "method") {
                s.default = "slow".into();
            }
        }),
        ("choice option listed twice", |m| {
            if let ParamKind::Choice(s) = param_mut(m, "method") {
                s.options.push("fast".into());
            }
        }),
        ("choice with no options", |m| {
            if let ParamKind::Choice(s) = param_mut(m, "method") {
                s.options.clear();
            }
        }),
        ("curve frequencies not increasing", |m| {
            if let ParamKind::Curve(s) = param_mut(m, "weight_curve") {
                s.default[1].frequency_hz = 20.0;
            }
        }),
        ("curve value out of range", |m| {
            if let ParamKind::Curve(s) = param_mut(m, "weight_curve") {
                s.default[0].value = 30.0;
            }
        }),
        ("curve with no default points", |m| {
            if let ParamKind::Curve(s) = param_mut(m, "weight_curve") {
                s.default.clear();
            }
        }),
        ("negative curve frequency", |m| {
            if let ParamKind::Curve(s) = param_mut(m, "weight_curve") {
                s.default[0].frequency_hz = -1.0;
            }
        }),
        ("per-channel default out of range", |m| {
            *param_mut(m, "channel_weight") = ParamKind::PerChannel(number(0.0, 10.0, 0.1, 11.0))
        }),
        ("duplicate diagnostic key", |m| {
            m.diagnostics[1].key = "untrusted_lines".into()
        }),
        ("bad diagnostic key", |m| {
            m.diagnostics[0].key = "Untrusted Lines".into()
        }),
        ("empty diagnostic label", |m| {
            m.diagnostics[0].label = String::new()
        }),
    ];
    for (name, mutate) in cases {
        let mut m = good_manifest();
        mutate(&mut m);
        let r = validate_manifest(&m);
        assert!(
            matches!(r, Err(LawError::ManifestInvalid(_))),
            "{name}: expected the manifest to be refused, got {r:?}"
        );
    }
}

#[test]
fn parameters_resolve_to_manifest_order_with_defaults() {
    let m = good_manifest();
    let supplied = vec![
        Param::new("method", ParamValue::Choice("fast".into())),
        Param::new("gain", ParamValue::Number(0.25)),
    ];
    let resolved = resolve_params(&m, &supplied, 3).expect("good values resolve");
    let keys: Vec<&str> = resolved.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "gain",
            "loops",
            "smooth",
            "method",
            "weight_curve",
            "channel_weight"
        ]
    );
    assert_eq!(resolved[0].value, ParamValue::Number(0.25));
    assert_eq!(resolved[1].value, ParamValue::Integer(3));
    assert_eq!(resolved[2].value, ParamValue::Toggle(true));
    assert_eq!(resolved[3].value, ParamValue::Choice("fast".into()));
    // A per-channel default is one value per control channel.
    assert_eq!(resolved[5].value, ParamValue::PerChannel(vec![1.0; 3]));
}

#[test]
fn each_bad_parameter_value_is_refused_by_key() {
    let m = good_manifest();
    let curve = |pts: &[(f64, f64)]| {
        ParamValue::Curve(
            pts.iter()
                .map(|&(frequency_hz, value)| Breakpoint {
                    frequency_hz,
                    value,
                })
                .collect(),
        )
    };
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "undeclared key",
            vec![Param::new("speed", ParamValue::Number(1.0))],
            "speed",
        ),
        (
            "supplied twice",
            vec![
                Param::new("gain", ParamValue::Number(0.1)),
                Param::new("gain", ParamValue::Number(0.2)),
            ],
            "gain",
        ),
        (
            "wrong kind",
            vec![Param::new("gain", ParamValue::Integer(1))],
            "gain",
        ),
        (
            "above max",
            vec![Param::new("gain", ParamValue::Number(1.5))],
            "gain",
        ),
        (
            "NaN",
            vec![Param::new("gain", ParamValue::Number(f64::NAN))],
            "gain",
        ),
        (
            "integer below min",
            vec![Param::new("loops", ParamValue::Integer(0))],
            "loops",
        ),
        (
            "choice not an option",
            vec![Param::new("method", ParamValue::Choice("slow".into()))],
            "method",
        ),
        (
            "curve not increasing",
            vec![Param::new(
                "weight_curve",
                curve(&[(100.0, 0.0), (50.0, 0.0)]),
            )],
            "weight_curve",
        ),
        (
            "curve value out of range",
            vec![Param::new("weight_curve", curve(&[(100.0, -25.0)]))],
            "weight_curve",
        ),
        (
            "per-channel of the wrong length",
            vec![Param::new(
                "channel_weight",
                ParamValue::PerChannel(vec![1.0, 1.0]),
            )],
            "channel_weight",
        ),
        (
            "per-channel value out of range",
            vec![Param::new(
                "channel_weight",
                ParamValue::PerChannel(vec![1.0, 12.0, 1.0]),
            )],
            "channel_weight",
        ),
    ];
    for (name, supplied, bad_key) in cases {
        match resolve_params(&m, &supplied, 3) {
            Err(LawError::ParamInvalid { key, .. }) => {
                assert_eq!(key, bad_key, "{name}: refused under the wrong key")
            }
            other => panic!("{name}: expected a refusal of `{bad_key}`, got {other:?}"),
        }
    }
}

const N: usize = 8;

fn grid() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
    (vec![1.0; N], vec![0.5; N], vec![2.0; N], vec![0.9; N])
}

fn inputs<'a>(
    t: &'a [f64],
    m: &'a [f64],
    h: &'a [f64],
    c: &'a [f64],
    notched: &'a [bool],
) -> LoopInputs<'a> {
    LoopInputs {
        target_psd: t,
        measured_psd: m,
        h_mag2: h,
        coherence: c,
        current_level_db: -6.0,
        df_hz: 1.25,
        notched_bins: notched,
        band: (1, 7),
        dof: 16.0,
        loop_index: 3,
    }
}

#[test]
fn setups_off_the_rules_are_refused() {
    let t = vec![1.0; N];
    let good = Setup {
        target_psd: &t,
        df_hz: 1.0,
        band: (1, 7),
        control_channels: 2,
    };
    assert_eq!(validate_setup(&good), Ok(()));
    let neg = {
        let mut v = t.clone();
        v[3] = -1.0;
        v
    };
    let bad = [
        Setup {
            target_psd: &[],
            ..good
        },
        Setup {
            target_psd: &neg,
            ..good
        },
        Setup { df_hz: 0.0, ..good },
        Setup {
            df_hz: f64::NAN,
            ..good
        },
        Setup {
            band: (7, 7),
            ..good
        },
        Setup {
            band: (1, N + 1),
            ..good
        },
        Setup {
            control_channels: 0,
            ..good
        },
    ];
    for (i, s) in bad.iter().enumerate() {
        assert!(
            matches!(validate_setup(s), Err(LawError::SetupInvalid(_))),
            "bad setup {i} passed"
        );
    }
}

#[test]
fn loop_inputs_off_the_grid_are_refused() {
    let (t, m, h, c) = grid();
    assert_eq!(validate_inputs(&inputs(&t, &m, &h, &c, &[]), N), Ok(()));
    assert_eq!(
        validate_inputs(&inputs(&t, &m, &h, &c, &[false; N]), N),
        Ok(())
    );
    let short = vec![0.5; N - 1];
    let mut nan = m.clone();
    nan[2] = f64::NAN;
    let mut negative = h.clone();
    negative[0] = -1.0;
    let cases: Vec<(&str, LoopInputs)> = vec![
        ("ragged measured", inputs(&t, &short, &h, &c, &[])),
        ("NaN measured", inputs(&t, &nan, &h, &c, &[])),
        ("negative plant gain", inputs(&t, &m, &negative, &c, &[])),
        ("ragged notch mask", inputs(&t, &m, &h, &c, &[false; 3])),
        (
            "band off the grid",
            LoopInputs {
                band: (2, N + 1),
                ..inputs(&t, &m, &h, &c, &[])
            },
        ),
        (
            "zero line spacing",
            LoopInputs {
                df_hz: 0.0,
                ..inputs(&t, &m, &h, &c, &[])
            },
        ),
        (
            "infinite level",
            LoopInputs {
                current_level_db: f64::INFINITY,
                ..inputs(&t, &m, &h, &c, &[])
            },
        ),
        (
            "negative degrees of freedom",
            LoopInputs {
                dof: -1.0,
                ..inputs(&t, &m, &h, &c, &[])
            },
        ),
    ];
    for (name, i) in cases {
        assert!(
            matches!(validate_inputs(&i, N), Err(LawError::InputsInvalid(_))),
            "{name}: passed"
        );
    }
}

fn good_outputs() -> LoopOutputs {
    LoopOutputs {
        drive_psd: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0],
        level_cmd_db: -3.0,
        diagnostics: LoopDiagnostics {
            untrusted_bins: vec![1],
            clamped_bins: vec![2, 3],
            divergent_bins: vec![],
            inband_err_db: Some(0.5),
            inband_rms_g: Some(2.0),
            servo_faulted: false,
            values: vec![DiagnosticValue {
                key: "condition".into(),
                value: 12.0,
            }],
        },
    }
}

#[test]
fn each_bad_output_is_refused() {
    let declared = good_manifest().diagnostics;
    assert_eq!(validate_outputs(&good_outputs(), N, &declared), Ok(()));
    type Mutation = (&'static str, OutputRule, fn(&mut LoopOutputs));
    use OutputRule::*;
    let cases: Vec<Mutation> = vec![
        ("drive one line short", WrongLength, |o| {
            o.drive_psd.pop();
        }),
        ("drive one line long", WrongLength, |o| {
            o.drive_psd.push(0.0)
        }),
        ("NaN drive", NonFiniteDrive, |o| o.drive_psd[3] = f64::NAN),
        ("infinite drive", NonFiniteDrive, |o| {
            o.drive_psd[3] = f64::INFINITY
        }),
        ("negative drive", NegativeDrive, |o| o.drive_psd[3] = -1e-9),
        ("non-finite level", NonFiniteLevel, |o| {
            o.level_cmd_db = f64::NAN
        }),
        ("untrusted line off the grid", Diagnostics, |o| {
            o.diagnostics.untrusted_bins.push(N)
        }),
        ("divergent line off the grid", Diagnostics, |o| {
            o.diagnostics.divergent_bins.push(usize::MAX)
        }),
        ("non-finite in-band error", Diagnostics, |o| {
            o.diagnostics.inband_err_db = Some(f64::INFINITY)
        }),
        ("undeclared diagnostic", Diagnostics, |o| {
            o.diagnostics.values.push(DiagnosticValue {
                key: "surprise".into(),
                value: 1.0,
            })
        }),
        ("diagnostic reported twice", Diagnostics, |o| {
            o.diagnostics.values.push(DiagnosticValue {
                key: "condition".into(),
                value: 1.0,
            })
        }),
        ("non-finite diagnostic", Diagnostics, |o| {
            o.diagnostics.values[0].value = f64::NAN
        }),
        // Two rules broken: the first in rule order is named.
        ("negative line and a NaN line", NonFiniteDrive, |o| {
            o.drive_psd[1] = -1.0;
            o.drive_psd[3] = f64::NAN;
        }),
    ];
    for (name, want, mutate) in cases {
        let mut o = good_outputs();
        mutate(&mut o);
        match validate_outputs(&o, N, &declared) {
            Err(LawError::BadOutput { rule, .. }) => assert_eq!(rule, want, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
}
