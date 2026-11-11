use super::tests_support::*;
use super::*;

fn refusal(c: &MixedConfig) -> &'static str {
    match c.validate(Some(8192.0)) {
        Ok(()) => "accepted",
        Err(e) => e.name(),
    }
}

const TWO_SWEEPS: PartSweepTarget = PartSweepTarget::Sweeps { n: 2 };

// ---- good configurations ---------------------------------------------------

#[test]
fn sine_on_random_passes() {
    let c = config(vec![
        floor("floor"),
        tone("t1", fixed(100.0)),
        tone("t2", sweep(400.0, 800.0, 450.0, TWO_SWEEPS)),
        tone("t3", follow("t1", 3.0)),
    ]);
    assert_eq!(refusal(&c), "accepted");
}

#[test]
fn random_on_random_passes() {
    let c = config(vec![
        floor("floor"),
        band("b1", fixed(150.0), BandWidth::Hz { hz: 10.0 }),
        band(
            "b2",
            sweep(300.0, 600.0, 300.0, PartSweepTarget::Duration { s: 120.0 }),
            BandWidth::Share { share: 0.1 },
        ),
        band("b3", follow("b2", 2.0), BandWidth::Hz { hz: 20.0 }),
    ]);
    assert_eq!(refusal(&c), "accepted");
}

#[test]
fn sine_and_random_on_random_passes() {
    let c = config(vec![
        floor("floor"),
        tone("t1", fixed(80.0)),
        tone("t2", sweep(500.0, 900.0, 500.0, TWO_SWEEPS)),
        tone("t2h", follow("t2", 2.0)),
        band("b1", fixed(300.0), BandWidth::Hz { hz: 20.0 }),
        band(
            "b2",
            sweep(1000.0, 1500.0, 1200.0, TWO_SWEEPS),
            BandWidth::Hz { hz: 30.0 },
        ),
    ]);
    assert_eq!(refusal(&c), "accepted");
}

// ---- one test per refusal --------------------------------------------------

#[test]
fn no_floor_is_refused() {
    let c = config(vec![tone("t1", fixed(100.0))]);
    assert_eq!(refusal(&c), "no_floor");
}

#[test]
fn a_second_floor_is_refused() {
    let c = config(vec![floor("f1"), floor("f2")]);
    assert_eq!(refusal(&c), "more_than_one_floor");
}

#[test]
fn an_unknown_and_a_reserved_kind_are_refused_by_name_and_keep_their_fields() {
    for kind in ["laser", RESERVED_PART_KINDS[0]] {
        let body: PartBody =
            serde_json::from_value(serde_json::json!({"kind": kind, "rate_hz": 7.5})).unwrap();
        let mut c = config(vec![floor("floor")]);
        c.parts.push(Part {
            id: "x".into(),
            name: String::new(),
            enabled: true,
            window: PartWindow::default(),
            body: body.clone(),
        });
        let e = c.validate(None).unwrap_err();
        assert_eq!(e.name(), "unknown_part_kind");
        assert_eq!(
            e,
            ConfigError::UnknownPartKind {
                part: "x".into(),
                kind: kind.into()
            }
        );
        // It round-trips, so a record holding it still reads.
        let back: PartBody = serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
        assert_eq!(back, body);
        assert_eq!(serde_json::to_value(&body).unwrap()["rate_hz"], 7.5);
    }
}

#[test]
fn too_many_tones_and_bands_are_refused() {
    let mut parts = vec![floor("floor")];
    for k in 0..=MAX_TONES {
        parts.push(tone(&format!("t{k}"), fixed(100.0 + 10.0 * k as f64)));
    }
    assert_eq!(refusal(&config(parts)), "too_many_tones");
    let mut parts = vec![floor("floor")];
    for k in 0..=MAX_BANDS {
        parts.push(band(
            &format!("b{k}"),
            fixed(100.0 + 50.0 * k as f64),
            BandWidth::Hz { hz: 5.0 },
        ));
    }
    assert_eq!(refusal(&config(parts)), "too_many_bands");
    // At the limit both pass.
    let mut parts = vec![floor("floor")];
    for k in 0..MAX_TONES {
        parts.push(tone(&format!("t{k}"), fixed(100.0 + 10.0 * k as f64)));
    }
    assert_eq!(refusal(&config(parts)), "accepted");
}

#[test]
fn a_repeated_or_empty_id_is_refused() {
    let c = config(vec![
        floor("floor"),
        tone("a", fixed(100.0)),
        tone("a", fixed(300.0)),
    ]);
    assert_eq!(refusal(&c), "duplicate_part_id");
    let c = config(vec![floor("floor"), tone(" ", fixed(100.0))]);
    assert_eq!(refusal(&c), "duplicate_part_id");
}

#[test]
fn a_part_out_of_band_anywhere_in_its_motion_is_refused() {
    // A dwell outside the floor's 20 to 2000 Hz.
    let c = config(vec![floor("floor"), tone("t", fixed(10.0))]);
    assert_eq!(refusal(&c), "part_out_of_band");
    // A sweep that starts inside and reaches outside.
    let c = config(vec![
        floor("floor"),
        tone("t", sweep(1000.0, 2500.0, 1000.0, TWO_SWEEPS)),
    ]);
    assert_eq!(refusal(&c), "part_out_of_band");
    // A band whose center stays inside but whose edge leaves.
    let c = config(vec![
        floor("floor"),
        band("b", fixed(1990.0), BandWidth::Hz { hz: 40.0 }),
    ]);
    assert_eq!(refusal(&c), "part_out_of_band");
    // A swept band by share: the edge at the top of the sweep leaves.
    let c = config(vec![
        floor("floor"),
        band(
            "b",
            sweep(500.0, 1950.0, 500.0, TWO_SWEEPS),
            BandWidth::Share { share: 0.1 },
        ),
    ]);
    assert_eq!(refusal(&c), "part_out_of_band");
    // A follower whose ratio takes it out at the top of its root's sweep only.
    let c = config(vec![
        floor("floor"),
        tone("root", sweep(100.0, 900.0, 100.0, TWO_SWEEPS)),
        tone("h3", follow("root", 3.0)),
    ]);
    match c.validate(None) {
        Err(ConfigError::PartOutOfBand { part, reaches, .. }) => {
            assert_eq!(part, "h3");
            assert!((reaches.1 - 2700.0).abs() < 1e-9, "{reaches:?}");
        }
        other => panic!("{other:?}"),
    }
    // Above the rate's half.
    let c = config(vec![floor("floor"), tone("t", fixed(100.0))]);
    assert_eq!(
        c.validate(Some(3000.0)).unwrap_err().name(),
        "part_out_of_band"
    );
    assert!(c.validate(Some(8192.0)).is_ok());
}

#[test]
fn a_one_way_sweep_is_judged_only_where_it_goes() {
    // One pass up from 1500 Hz: it never sees the 1000 Hz end, so a band edge
    // there is never reached... and a tone at 1100 Hz is never met.
    let one_up = PartSweepTarget::Sweeps { n: 1 };
    let c = config(vec![
        floor("floor"),
        tone("s", sweep(1000.0, 1900.0, 1500.0, one_up)),
        tone("t", fixed(1100.0)),
    ]);
    assert_eq!(refusal(&c), "accepted");
    // Both ways it is.
    let c = config(vec![
        floor("floor"),
        tone("s", sweep(1000.0, 1900.0, 1500.0, TWO_SWEEPS)),
        tone("t", fixed(1100.0)),
    ]);
    assert_eq!(refusal(&c), "parts_too_close");
    // A duration that ends the sweep at 1700 Hz (20 s at 10 Hz/s) keeps clear of
    // a tone at 1800 Hz.
    let c = config(vec![
        floor("floor"),
        tone(
            "s",
            sweep(
                1000.0,
                1900.0,
                1500.0,
                PartSweepTarget::Duration { s: 20.0 },
            ),
        ),
        tone("t", fixed(1800.0)),
    ]);
    assert_eq!(refusal(&c), "accepted");
}

#[test]
fn tones_too_close_are_refused_and_harmonics_and_apart_windows_pass() {
    // Two dwells 0.6 Hz apart against 2 x 0.5 Hz.
    let c = config(vec![
        floor("floor"),
        tone("a", fixed(100.0)),
        tone("b", fixed(100.6)),
    ]);
    match c.validate(None) {
        Err(ConfigError::PartsTooClose {
            closest_hz,
            required_hz,
            ..
        }) => {
            assert!((closest_hz - 0.6).abs() < 1e-9);
            assert!((required_hz - 1.0).abs() < 1e-12);
        }
        other => panic!("{other:?}"),
    }
    // A follower at a ratio that keeps it apart at the root's lowest.
    let c = config(vec![
        floor("floor"),
        tone("root", sweep(50.0, 500.0, 50.0, TWO_SWEEPS)),
        tone("h2", follow("root", 2.0)),
    ]);
    assert_eq!(refusal(&c), "accepted");
    // One so near 1 that it is within a width at the root's lowest (50 Hz x
    // 0.01 = 0.5 Hz).
    let c = config(vec![
        floor("floor"),
        tone("root", sweep(50.0, 500.0, 50.0, TWO_SWEEPS)),
        tone("near", follow("root", 1.01)),
    ]);
    assert_eq!(refusal(&c), "parts_too_close");
    // Two followers of one root, ratios 2 and 2.01: closest 0.5 Hz at 50 Hz.
    let c = config(vec![
        floor("floor"),
        tone("root", sweep(50.0, 500.0, 50.0, TWO_SWEEPS)),
        tone("a", follow("root", 2.0)),
        tone("b", follow("root", 2.01)),
    ]);
    assert_eq!(refusal(&c), "parts_too_close");
    // Two independent sweeps whose spans overlap can cross.
    let c = config(vec![
        floor("floor"),
        tone("a", sweep(100.0, 300.0, 100.0, TWO_SWEEPS)),
        tone("b", sweep(250.0, 400.0, 400.0, TWO_SWEEPS)),
    ]);
    assert_eq!(refusal(&c), "parts_too_close");
    // The same two tones in windows that never meet.
    let mut c = config(vec![
        floor("floor"),
        tone("a", fixed(100.0)),
        tone("b", fixed(100.2)),
    ]);
    c.parts[1].window = PartWindow {
        on_s: 0.0,
        off_s: Some(10.0),
    };
    c.parts[2].window = PartWindow {
        on_s: 10.0,
        off_s: None,
    };
    assert_eq!(refusal(&c), "accepted");
    // A wider tracker needs more room.
    let mut c = config(vec![
        floor("floor"),
        tone("a", fixed(100.0)),
        tone("b", fixed(103.0)),
    ]);
    assert_eq!(refusal(&c), "accepted");
    c.tracking_hz = 2.0;
    assert_eq!(refusal(&c), "parts_too_close");
}

#[test]
fn follows_of_no_part_itself_the_floor_or_a_loop_are_refused() {
    let c = config(vec![floor("floor"), tone("a", follow("nobody", 2.0))]);
    assert_eq!(refusal(&c), "follow_unknown_part");
    let c = config(vec![floor("floor"), tone("a", follow("a", 2.0))]);
    assert_eq!(refusal(&c), "follow_self");
    let c = config(vec![floor("floor"), tone("a", follow("floor", 2.0))]);
    assert_eq!(refusal(&c), "follow_no_frequency");
    let c = config(vec![
        floor("floor"),
        tone("a", follow("c", 2.0)),
        tone("b", follow("a", 2.0)),
        tone("c", follow("b", 2.0)),
    ]);
    match c.validate(None) {
        Err(ConfigError::FollowLoop { parts }) => {
            let mut p = parts.clone();
            p.sort();
            assert_eq!(p, vec!["a", "b", "c"]);
        }
        other => panic!("{other:?}"),
    }
    // A chain resolves root first whatever the list order.
    let c = config(vec![
        floor("floor"),
        tone("c", follow("b", 2.0)),
        tone("b", follow("a", 2.0)),
        tone("a", fixed(100.0)),
    ]);
    let order = c.follow_order().unwrap();
    let pos = |id: &str| order.iter().position(|&i| c.parts[i].id == id).unwrap();
    assert!(pos("a") < pos("b") && pos("b") < pos("c"));
    let m = c.motions().unwrap();
    assert_eq!(m[1].unwrap().lo_hz, 400.0);
    assert_eq!(m[1].unwrap().factor, 4.0);
    assert_eq!(m[1].unwrap().root, 3);
}

#[test]
fn a_schedule_in_segments_reads_and_is_refused_by_name() {
    let s: PartSchedule = serde_json::from_value(serde_json::json!({
        "kind": "segments",
        "segments": [{"kind": "sweep"}, {"kind": "dwell", "s": 10}]
    }))
    .unwrap();
    let c = config(vec![floor("floor"), tone("t", s)]);
    assert_eq!(refusal(&c), "segments_not_built");
}

#[test]
fn a_bad_value_of_a_part_or_around_it_is_refused_naming_the_field() {
    let mut t = tone("t", fixed(100.0));
    if let PartBody::Tone(tp) = &mut t.body {
        tp.amplitude = ToneAmplitude::Constant { g_pk: 0.0 };
    }
    let e = config(vec![floor("floor"), t]).validate(None).unwrap_err();
    assert_eq!((e.name(), e.part()), ("bad_part", Some("t")));
    let c = config(vec![floor("floor"), tone("t", follow("floor", 0.0))]);
    // A ratio of zero is the part's own value, judged before the follow.
    assert_eq!(c.validate(None).unwrap_err().name(), "bad_part");
    let mut c = config(vec![floor("floor"), tone("t", fixed(100.0))]);
    c.parts[1].window = PartWindow {
        on_s: 5.0,
        off_s: Some(5.0),
    };
    assert_eq!(refusal(&c), "bad_part");
    let mut c = config(vec![floor("floor")]);
    c.parts[0].enabled = false;
    assert_eq!(refusal(&c), "bad_part");
    let mut c = config(vec![floor("floor")]);
    c.tracking_hz = 0.0;
    assert_eq!(refusal(&c), "bad_config");
    let mut c = config(vec![floor("floor")]);
    c.limits.drive_vpk_abort = Some(1.0);
    assert_eq!(refusal(&c), "bad_config");
    // A profile that does not reach the whole sweep.
    let mut t = tone("t", sweep(100.0, 400.0, 100.0, TWO_SWEEPS));
    if let PartBody::Tone(tp) = &mut t.body {
        tp.amplitude = ToneAmplitude::Profile {
            rows: vec![ProfileRow::accel(100.0, 1.0), ProfileRow::accel(300.0, 1.0)],
        };
    }
    let e = config(vec![floor("floor"), t]).validate(None).unwrap_err();
    assert_eq!((e.name(), e.part()), ("bad_part", Some("t")));
}

// ---- serde ------------------------------------------------------------------

#[test]
fn the_parts_list_reads_from_its_tagged_form_with_defaults_and_round_trips() {
    let j = serde_json::json!({
        "df_hz": 1.0,
        "lines": 2048,
        "control": {"channels": [{"channel_index": 0, "weight": 1.0}], "mode": "WeightedAvg"},
        "schedule": {"mode": "Auto", "start_db": -12.0, "table_db": [-12.0, -6.0, 0.0], "dwell_s": [3.0]},
        "parts": [
            {"id": "floor", "body": {"kind": "floor", "breakpoints": {"label": "", "rows": [
                {"freq_hz": 20.0, "level": {"Psd": 0.01}, "alarm_db": null, "abort_db": null},
                {"freq_hz": 2000.0, "level": {"Psd": 0.01}, "alarm_db": null, "abort_db": null}
            ]}}},
            {"id": "t1", "name": "Rotor", "body": {"kind": "tone",
                "schedule": {"kind": "fixed", "hz": 100.0, "for_s": 30.0},
                "amplitude": {"kind": "constant", "g_pk": 2.0}}},
            {"id": "t2", "window": {"on_s": 5.0, "off_s": 50.0}, "body": {"kind": "tone",
                "schedule": {"kind": "sweep", "lo_hz": 200.0, "hi_hz": 400.0, "start_hz": 200.0,
                    "direction": "up", "law": "log_oct_per_min", "rate": 1.0,
                    "target": {"kind": "whichever_first", "sweeps": 4, "duration_s": 600.0}},
                "amplitude": {"kind": "constant", "g_pk": 1.0}}},
            {"id": "h2", "body": {"kind": "tone",
                "schedule": {"kind": "follow", "of": "t1", "ratio": 5.0},
                "amplitude": {"kind": "constant", "g_pk": 0.5}}},
            {"id": "b1", "enabled": false, "body": {"kind": "narrowband",
                "schedule": {"kind": "sweep", "lo_hz": 500.0, "hi_hz": 800.0, "start_hz": 800.0,
                    "direction": "down", "law": "linear_hz_per_s", "rate": 2.0,
                    "target": {"kind": "sweeps", "n": 2}},
                "level_g2_hz": 0.1, "width": {"kind": "share", "share": 0.05}}},
            {"id": "b2", "body": {"kind": "narrowband",
                "schedule": {"kind": "fixed", "hz": 1200.0},
                "level_g2_hz": 0.2, "width": {"kind": "hz", "hz": 25.0}}}
        ]
    });
    let c: MixedConfig = serde_json::from_value(j).unwrap();
    assert_eq!(c.exclusion_hz, DEFAULT_EXCLUSION_HZ);
    assert_eq!(c.tracking_hz, DEFAULT_TRACKING_HZ);
    assert_eq!(c.law, MixedLawChoice::default());
    assert_eq!(c.law.floor.id, "hinv");
    assert_eq!(c.law.tones.id, "servo_blend");
    assert!(c.parts[0].enabled && !c.parts[4].enabled);
    match &c.parts[1].body {
        PartBody::Tone(t) => {
            assert_eq!(t.alarm_db, DEFAULT_TONE_ALARM_DB);
            assert_eq!(t.abort_db, DEFAULT_TONE_ABORT_DB);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(c.validate(Some(8192.0)), Ok(()));
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["parts"][1]["body"]["kind"], "tone");
    assert_eq!(v["parts"][1]["body"]["schedule"]["kind"], "fixed");
    assert_eq!(v["parts"][2]["body"]["schedule"]["law"], "log_oct_per_min");
    assert_eq!(
        v["parts"][2]["body"]["schedule"]["target"]["kind"],
        "whichever_first"
    );
    assert_eq!(v["parts"][4]["body"]["width"]["kind"], "share");
    // Options left out are not written.
    assert!(v["parts"][3]["window"].get("off_s").is_none());
    assert!(v["parts"][5]["body"]["schedule"].get("for_s").is_none());
    assert!(v.get("law_guards").is_none());
    assert!(v.get("control_tick_s").is_none());
    let back: MixedConfig = serde_json::from_value(v).unwrap();
    assert_eq!(back, c);
}

#[test]
fn sweep_span_is_exact_for_counts_durations_and_edge_starts() {
    let cfg = |start, dir: Direction, target| {
        sweep(100.0, 200.0, start, target)
            .sweep_config()
            .map(|mut c| {
                c.initial_direction = dir.into();
                c
            })
            .unwrap()
    };
    assert_eq!(
        sweep_span(&cfg(150.0, Direction::Up, PartSweepTarget::Sweeps { n: 1 })),
        (150.0, 200.0)
    );
    assert_eq!(
        sweep_span(&cfg(
            150.0,
            Direction::Down,
            PartSweepTarget::Sweeps { n: 1 }
        )),
        (100.0, 150.0)
    );
    assert_eq!(
        sweep_span(&cfg(150.0, Direction::Up, PartSweepTarget::Sweeps { n: 2 })),
        (100.0, 200.0)
    );
    // From the top edge pointing up: flips inward, one pass covers it all.
    assert_eq!(
        sweep_span(&cfg(200.0, Direction::Up, PartSweepTarget::Sweeps { n: 1 })),
        (100.0, 200.0)
    );
    // 3 s at 10 Hz/s from 150 up.
    let (lo, hi) = sweep_span(&cfg(
        150.0,
        Direction::Up,
        PartSweepTarget::Duration { s: 3.0 },
    ));
    assert_eq!(lo, 150.0);
    assert!((hi - 180.0).abs() < 1e-9);
    // 8 s: 5 s to the top, 3 s back down to 170.
    let (lo, hi) = sweep_span(&cfg(
        150.0,
        Direction::Up,
        PartSweepTarget::Duration { s: 8.0 },
    ));
    assert_eq!((lo, hi), (150.0, 200.0));
    // Whichever first: the duration ends it before the second pass.
    let (lo, hi) = sweep_span(&cfg(
        150.0,
        Direction::Up,
        PartSweepTarget::WhicheverFirst {
            sweeps: 3,
            duration_s: 12.0,
        },
    ));
    assert!((lo - 130.0).abs() < 1e-9, "{lo}");
    assert_eq!(hi, 200.0);
}
