//! The configuration: every refusal by name, defaults, and serde round trips.

mod common;

use common::*;
use hertzforge_mimo_mixed::band::BandStatement;
use hertzforge_mimo_mixed::config::*;
use hertzforge_mimo_mixed::{budget, fill};
use hertzforge_mixed::{LawPick, PartSchedule, ToneAmplitude};

fn name(cfg: &MimoMixedConfig) -> &'static str {
    match cfg.validate(Some(10_240.0)) {
        Ok(()) => "ok",
        Err(e) => {
            // Every refusal reads in plain words.
            assert!(!e.to_string().is_empty());
            e.name()
        }
    }
}

fn base(parts: Vec<MimoPart>) -> MimoMixedConfig {
    config(2, 2, 0.5, parts)
}

#[test]
fn mimo_mixed_a_plain_configuration_validates() {
    let cfg = base(vec![
        floor("f"),
        tone("t1", fixed(100.0), 1.0),
        band("b1", fixed(400.0), 20.0, added(0.05, 0.0)),
    ]);
    assert_eq!(name(&cfg), "ok");
    assert_eq!(cfg.law.floor.id, "pinv_closed");
    assert_eq!(cfg.law.tones.id, "matrix");
    assert_eq!(cfg.law.bands.id, "auto");
    assert_eq!(cfg.law.ceiling_rule.id, "common_scale");
    assert_eq!(cfg.law.fill_rule.id, "no_error");
    assert_eq!(
        (MAX_CONTROLS, MAX_DRIVES, MAX_LIMITS, MAX_TONES, MAX_BANDS),
        (16, 8, 8, 12, 12)
    );
}

#[test]
fn mimo_mixed_single_output_refusals_keep_their_names() {
    let f = || floor("f");
    let cases: Vec<(&str, Vec<MimoPart>)> = vec![
        ("no_floor", vec![tone("t", fixed(100.0), 1.0)]),
        ("more_than_one_floor", vec![f(), floor("g")]),
        (
            "unknown_part_kind",
            vec![
                f(),
                MimoPart {
                    id: "p".into(),
                    name: String::new(),
                    enabled: true,
                    window: Default::default(),
                    body: MimoPartBody::Unknown {
                        kind: "pulse_train".into(),
                        fields: Default::default(),
                    },
                },
            ],
        ),
        (
            "too_many_tones",
            std::iter::once(f())
                .chain(
                    (0..13).map(|i| tone(&format!("t{i}"), fixed(100.0 + 100.0 * i as f64), 1.0)),
                )
                .collect(),
        ),
        (
            "too_many_bands",
            std::iter::once(f())
                .chain((0..13).map(|i| {
                    band(
                        &format!("b{i}"),
                        fixed(100.0 + 100.0 * i as f64),
                        10.0,
                        added(0.01, 0.0),
                    )
                }))
                .collect(),
        ),
        ("duplicate_part_id", vec![f(), tone("f", fixed(100.0), 1.0)]),
        ("part_out_of_band", vec![f(), tone("t", fixed(3000.0), 1.0)]),
        (
            "parts_too_close",
            vec![
                f(),
                tone("a", fixed(100.0), 1.0),
                tone("b", fixed(100.5), 1.0),
            ],
        ),
        (
            "follow_unknown_part",
            vec![
                f(),
                tone(
                    "a",
                    PartSchedule::Follow {
                        of: "x".into(),
                        ratio: 2.0,
                    },
                    1.0,
                ),
            ],
        ),
        (
            "follow_self",
            vec![
                f(),
                tone(
                    "a",
                    PartSchedule::Follow {
                        of: "a".into(),
                        ratio: 2.0,
                    },
                    1.0,
                ),
            ],
        ),
        (
            "follow_loop",
            vec![
                f(),
                tone(
                    "a",
                    PartSchedule::Follow {
                        of: "b".into(),
                        ratio: 2.0,
                    },
                    1.0,
                ),
                tone(
                    "b",
                    PartSchedule::Follow {
                        of: "a".into(),
                        ratio: 2.0,
                    },
                    1.0,
                ),
            ],
        ),
        (
            "follow_no_frequency",
            vec![
                f(),
                tone(
                    "a",
                    PartSchedule::Follow {
                        of: "f".into(),
                        ratio: 2.0,
                    },
                    1.0,
                ),
            ],
        ),
        (
            "segments_not_built",
            vec![
                f(),
                tone(
                    "a",
                    PartSchedule::Segments {
                        segments: Vec::new(),
                    },
                    1.0,
                ),
            ],
        ),
        ("bad_part", vec![f(), tone("a", fixed(100.0), 0.0)]),
    ];
    for (want, parts) in cases {
        assert_eq!(name(&base(parts)), want, "case {want}");
    }
    let mut cfg = config(17, 2, 0.0, vec![floor("f")]);
    cfg.reference = reference(2048, 1.0, (20, 2001), 0.01, &coherence(17, 0.0));
    assert_eq!(name(&cfg), "bad_config");
    let mut cfg = base(vec![floor("f")]);
    cfg.tracking_hz = 0.0;
    assert_eq!(name(&cfg), "bad_config");
}

#[test]
fn mimo_mixed_refuses_a_per_control_list_of_the_wrong_length() {
    let mut t = tone("t", fixed(100.0), 1.0);
    if let MimoPartBody::Tone(tp) = &mut t.body {
        tp.amplitude = PerControl::Each(vec![ToneAmplitude::Constant { g_pk: 1.0 }; 3]);
    }
    assert_eq!(name(&base(vec![floor("f"), t])), "wrong_control_count");
    let mut t = tone("t", fixed(100.0), 1.0);
    if let MimoPartBody::Tone(tp) = &mut t.body {
        tp.phases_stated = true;
        tp.phase_deg = vec![0.0];
    }
    assert_eq!(name(&base(vec![floor("f"), t])), "wrong_control_count");
    let b = band(
        "b",
        fixed(400.0),
        20.0,
        BandStatement::Added {
            level_g2_hz: PerControl::Each(vec![0.1]),
            coherence: 0.0,
        },
    );
    assert_eq!(name(&base(vec![floor("f"), b])), "wrong_control_count");
    let mut f = floor("f");
    if let MimoPartBody::Floor(fp) = &mut f.body {
        fp.alarm_db = PerControl::Each(vec![(3.0, 3.0); 5]);
    }
    assert_eq!(name(&base(vec![f])), "wrong_control_count");
}

#[test]
fn mimo_mixed_refuses_bands_by_name() {
    // Unequal raises on a coherent floor (MV9's band): not added random.
    let raise = BandStatement::Raise {
        db: PerControl::Each(vec![10.0 * 11f64.log10(), 0.0]),
    };
    let cfg = config(
        2,
        2,
        0.8,
        vec![floor("f"), band("b", fixed(400.0), 20.0, raise.clone())],
    );
    assert_eq!(name(&cfg), "band_not_additive");
    // The same raise on an incoherent floor is added random.
    let cfg = config(
        2,
        2,
        0.0,
        vec![floor("f"), band("b", fixed(400.0), 20.0, raise)],
    );
    assert_eq!(name(&cfg), "ok");
    // Equal raises on a coherent floor are added random.
    let equal = BandStatement::Raise {
        db: PerControl::All(6.0),
    };
    let cfg = config(
        2,
        2,
        0.8,
        vec![floor("f"), band("b", fixed(400.0), 20.0, equal)],
    );
    assert_eq!(name(&cfg), "ok");
    let matrix = BandStatement::Matrix {
        values: serde_json::Value::Null,
    };
    let cfg = base(vec![floor("f"), band("b", fixed(400.0), 20.0, matrix)]);
    assert_eq!(name(&cfg), "band_matrix_not_built");
}

#[test]
fn mimo_mixed_refuses_unknown_rules_and_the_model_update() {
    let mut cfg = base(vec![floor("f")]);
    cfg.law.bands = LawPick::new("matrix_band");
    assert_eq!(name(&cfg), "unknown_band_law");
    let mut cfg = base(vec![floor("f")]);
    cfg.law.ceiling_rule = LawPick::new("allocate");
    assert_eq!(name(&cfg), "unknown_ceiling_rule");
    let mut cfg = base(vec![floor("f")]);
    cfg.law.fill_rule = LawPick::new("hold");
    assert_eq!(name(&cfg), "unknown_fill_rule");
    let mut cfg = base(vec![floor("f")]);
    cfg.frf_mode = hertzforge_mimo_random::config::FrfMode::Update { blend: 0.1 };
    assert_eq!(name(&cfg), "frf_update_not_built");
    // The builders refuse by the same names.
    assert_eq!(
        budget::ceiling_rule(&LawPick::new("allocate"))
            .err()
            .unwrap()
            .name(),
        "unknown_ceiling_rule"
    );
    assert_eq!(
        hertzforge_mimo_mixed::band::band_law(&LawPick::new("x"))
            .err()
            .unwrap()
            .name(),
        "unknown_band_law"
    );
    assert!(fill::fill_rule(&LawPick::new("hold")).is_err());
    // Every built id builds.
    for id in CEILING_RULE_IDS {
        assert_eq!(budget::ceiling_rule(&LawPick::new(id)).unwrap().id(), id);
    }
    for id in FILL_RULE_IDS {
        assert_eq!(fill::fill_rule(&LawPick::new(id)).unwrap().id(), id);
    }
    for id in ["scalar_servo", "per_control"] {
        assert_eq!(
            hertzforge_mimo_mixed::band::band_law(&LawPick::new(id))
                .unwrap()
                .id(),
            id
        );
    }
}

#[test]
fn mimo_mixed_engine_refusals_have_their_names() {
    let e = MimoMixedConfigError::LawRefusesTone {
        part: "t".into(),
        law: "ratio".into(),
        test_type: TEST_TYPE_ID.into(),
    };
    assert_eq!(e.name(), "law_refuses_tone");
    assert_eq!(e.part(), Some("t"));
    assert!(e.to_string().contains("mimo_mixed"));
}

#[test]
fn mimo_mixed_forecast_refuses_a_budget_that_cannot_fit() {
    // H = I: a 1 g tone needs 1 V on each drive; random peak 1.5 V; ceiling 6 V.
    let h = vec![
        vec![c(1.0, 0.0), c(0.0, 0.0)],
        vec![c(0.0, 0.0), c(1.0, 0.0)],
    ];
    let model = static_model(2048, &h);
    let cfg = base(vec![floor("f"), tone("t", fixed(100.0), 1.0)]);
    let ok = budget::forecast(&cfg, &model, &[1.5, 1.5], 1.0, 10.0).unwrap();
    assert!((ok.need_v - 2.5).abs() < 1e-9, "{ok:?}");
    let cfg = base(vec![floor("f"), tone("t", fixed(100.0), 5.0)]);
    let e = budget::forecast(&cfg, &model, &[1.5, 1.5], 1.0, 10.0).unwrap_err();
    assert_eq!(e.name(), "budget_cannot_fit");
    match e {
        MimoMixedConfigError::BudgetCannotFit {
            time_s,
            drive,
            need_v,
            ceiling_v,
        } => {
            assert_eq!((time_s, drive), (0.0, 0));
            assert!((need_v - 6.5).abs() < 1e-9 && (ceiling_v - 6.0).abs() < 1e-12);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn mimo_mixed_configuration_round_trips_through_json() {
    let mut t = tone("t1", fixed(100.0), 1.0);
    if let MimoPartBody::Tone(tp) = &mut t.body {
        tp.phases_stated = true;
        tp.phase_deg = vec![0.0, 90.0];
        tp.amplitude = PerControl::Each(vec![
            ToneAmplitude::Constant { g_pk: 1.0 },
            ToneAmplitude::Constant { g_pk: 0.5 },
        ]);
    }
    let cfg = base(vec![
        floor("f"),
        t,
        band(
            "b1",
            fixed(400.0),
            20.0,
            BandStatement::Raise {
                db: PerControl::All(6.0),
            },
        ),
        band("b2", fixed(800.0), 20.0, added(0.05, 0.5)),
    ]);
    // A reference's tolerance lines are `NaN` where there is none, and JSON
    // has no `NaN` (it writes `null`, which does not read back as a number):
    // the round trip is taken with finite lines over the whole grid.
    let mut cfg = cfg;
    let mut tol = cfg.floor_tolerances();
    for v in tol
        .alarm_upper
        .iter_mut()
        .chain(tol.alarm_lower.iter_mut())
        .chain(tol.abort_upper.iter_mut())
        .chain(tol.abort_lower.iter_mut())
    {
        if v.is_nan() {
            *v = 0.0;
        }
    }
    cfg.reference.tolerances = tol;
    let s = serde_json::to_string(&cfg).unwrap();
    let back: MimoMixedConfig = serde_json::from_str(&s).unwrap();
    assert_eq!(back, cfg);
    // The wire shapes: tagged parts and statements, untagged per-control.
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["parts"][1]["body"]["kind"], "tone");
    assert_eq!(v["parts"][1]["body"]["amplitude"][1]["g_pk"], 0.5);
    assert_eq!(v["parts"][2]["body"]["statement"]["kind"], "raise");
    assert_eq!(v["parts"][2]["body"]["statement"]["db"], 6.0);
    assert_eq!(v["parts"][0]["body"]["alarm_db"][0], 3.0);
    // Defaults fill a sparse part.
    let p: MimoPart = serde_json::from_value(serde_json::json!({
        "id": "b",
        "body": {"kind": "narrowband", "schedule": {"kind": "fixed", "hz": 300.0},
                 "width": {"kind": "hz", "hz": 10.0},
                 "statement": {"kind": "added", "level_g2_hz": [0.1, 0.2]}}
    }))
    .unwrap();
    match &p.body {
        MimoPartBody::Narrowband(b) => {
            assert_eq!(
                b.statement,
                BandStatement::Added {
                    level_g2_hz: PerControl::Each(vec![0.1, 0.2]),
                    coherence: 0.0
                }
            );
            assert_eq!(b.alarm_db, PerControl::All(DEFAULT_BAND_ALARM_DB));
        }
        other => panic!("{other:?}"),
    }
    // A kind this program does not build reads, round-trips and is refused.
    let u: MimoPart = serde_json::from_value(serde_json::json!({
        "id": "u", "body": {"kind": "pulse_train", "period_s": 1.0}
    }))
    .unwrap();
    assert_eq!(u.kind(), "pulse_train");
    let again: MimoPart = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
    assert_eq!(again, u);
    // A drive converts to MIMO Random's and back.
    let d = MimoDrive::new(3, 1.5);
    assert_eq!(MimoDrive::from_spec(&d.to_spec()), d);
}
