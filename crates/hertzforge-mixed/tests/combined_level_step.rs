//! The combined test of the shell's example on the control service's simulator
//! plant (modes at 200 and 550 Hz, Q 2.5, feedthrough 0.35) at 8192 Hz with
//! 0.5 Hz lines: a floor, a standing tone, a tone swept at 0.3 oct/min with
//! followers at 2 and 3 times it, a standing band and a swept band, manual
//! levels from -12 dB, and a manual step of +1 dB once every part has
//! settled. Nothing trips; every tone holds within 0.8 dB rms and under its
//! abort after the step (each reading carrying its own scatter).

mod common;
use common::table;
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::{
    parts::{
        BandWidth, Direction, FloorPart, MixedIdentification, MixedLawChoice, MixedLimits,
        NarrowbandPart, Part, PartBody, PartSchedule, PartSweepTarget, PartWindow, SweepLaw,
        ToneAmplitude, TonePart,
    },
    MixedConfig, MixedLawFactories, MixedPhase, MixedService, PartKind,
};
use hertzforge_random::service::config::{ControlSet, LevelSchedule};
use hertzforge_random::RunMode;
use hertzforge_sine::control::CompressionPreset;
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use std::sync::Arc;

fn part(id: &str, body: PartBody) -> Part {
    Part {
        id: id.into(),
        name: id.into(),
        enabled: true,
        window: PartWindow::default(),
        body,
    }
}
fn tone(id: &str, schedule: PartSchedule, g: f64) -> Part {
    part(
        id,
        PartBody::Tone(TonePart {
            schedule,
            amplitude: ToneAmplitude::Constant { g_pk: g },
            alarm_db: (1.5, 1.5),
            abort_db: (3.0, 3.0),
        }),
    )
}
fn band(id: &str, schedule: PartSchedule, width: BandWidth) -> Part {
    part(
        id,
        PartBody::Narrowband(NarrowbandPart {
            schedule,
            level_g2_hz: 5e-4,
            width,
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    )
}
fn fixed(hz: f64) -> PartSchedule {
    PartSchedule::Fixed {
        hz,
        for_s: None,
        for_cycles: None,
    }
}

fn cfg() -> MixedConfig {
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(20.0, 1e-4));
    t.push(BreakpointRow::psd(2000.0, 1e-4));
    let floor = part(
        "floor",
        PartBody::Floor(FloorPart {
            breakpoints: t,
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    );
    let follow = |r: f64| PartSchedule::Follow {
        of: "rotor".into(),
        ratio: r,
    };
    MixedConfig {
        df_hz: 0.5,
        lines: 4000,
        nfft: Some(16384),
        blocks_per_window: 4,
        control_dof: 120.0,
        control: ControlSet::single(0),
        parts: vec![
            floor,
            tone("blade", fixed(120.0), 0.3),
            tone(
                "rotor",
                PartSchedule::Sweep {
                    lo_hz: 400.0,
                    hi_hz: 600.0,
                    start_hz: 400.0,
                    direction: Direction::Up,
                    law: SweepLaw::LogOctPerMin,
                    rate: 0.3,
                    target: PartSweepTarget::Duration { s: f64::INFINITY },
                },
                0.3,
            ),
            tone("rotor2", follow(2.0), 0.1),
            tone("rotor3", follow(3.0), 0.1),
            band("mesh", fixed(250.0), BandWidth::Hz { hz: 20.0 }),
            band(
                "rband",
                PartSchedule::Sweep {
                    lo_hz: 1300.0,
                    hi_hz: 1600.0,
                    start_hz: 1300.0,
                    direction: Direction::Up,
                    law: SweepLaw::LinearHzPerS,
                    rate: 5.0,
                    target: PartSweepTarget::Duration { s: f64::INFINITY },
                },
                BandWidth::Share { share: 0.05 },
            ),
        ],
        exclusion_hz: 2.0,
        tracking_hz: 0.5,
        schedule: LevelSchedule {
            mode: RunMode::Manual,
            start_db: -12.0,
            table_db: vec![-12.0, -6.0, 0.0],
            dwell_s: vec![5.0],
        },
        full_level_duration_s: Some(15.0),
        level_ramp_s: 3.0,
        shutdown_ramp_s: 3.0,
        part_ramp_s: 3.0,
        floor_settle_s: 30.0,
        soft_start_db: 9.0,
        soft_start_relax_db: 6.0,
        shaker: None,
        moving_mass: Default::default(),
        crest_factor: 3.0,
        identification: MixedIdentification {
            sysid_level_db: -20.0,
            pretest_max_vrms: Some(1.0),
            ..MixedIdentification::default()
        },
        limits: MixedLimits {
            drive_vpk_ceiling: 5.0,
            drive_vpk_abort: Some(6.0),
            ..MixedLimits::default()
        },
        capture: Default::default(),
        law: MixedLawChoice::default(),
        law_guards: None,
        control_tick_s: Some(0.01),
        metadata: Vec::new(),
    }
}

#[test]
fn a_manual_level_step_in_the_combined_test_keeps_every_tone_in() {
    let fs = 8192.0;
    // The control service's simulator plant: modes at 200 and 550 Hz, Q 2.5,
    // feedthrough 0.35 (an anti-resonance near 600 Hz), noise 2e-4.
    let sim = SimConfig::new()
        .sample_rate_hz(fs)
        .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
        .feedthrough(0.35)
        .noise_level(2e-4)
        .shared_clock(true)
        .seed(0x5A5A_1234)
        .build();
    let mut laws = MixedLawFactories::builtin();
    // The floor law's settings as the shell's example sends them.
    laws.floor_hinv.damping = 4.0;
    laws.floor_hinv.max_correction_db_per_iter = 2.0;
    laws.floor_hinv.divergence_n = 10;
    laws.tones = Arc::new(ServoBlendFactory {
        params: ServoBlendParams {
            blend_w: 0.5,
            compression: CompressionPreset::Custom {
                ki: 0.5,
                max_rate_db_per_s: 20.0,
            },
            kp: 0.3,
        },
    });
    let mut s = MixedService::new(
        cfg(),
        table(),
        Box::new(sim),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_laws(laws)
    .with_block_frames(128);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    eprintln!(
        "lag {} coherence {:.3}",
        s.sysid_lag_frames(),
        s.band_coherence()
    );
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    let mut t = 0.0;
    let mut stepped = None;
    // Per tone: (id, worst |error|, sum of squares, readings).
    let mut worst: Vec<(String, f64, f64, usize)> = Vec::new();
    while t < 200.0 {
        if !s.run_chunk().unwrap() {
            break;
        }
        t += 0.25;
        let st = s.live_status();
        if st.phase == MixedPhase::Running
            && stepped.is_none()
            && st
                .parts
                .iter()
                .filter(|r| r.kind == PartKind::Tone)
                .all(|r| r.settled)
        {
            s.step_level(1.0).unwrap();
            stepped = Some(t);
        }
        if ((t * 4.0) as u64).is_multiple_of(4) && std::env::var("CL_DEBUG").is_ok() {
            let tones: Vec<String> = st
                .parts
                .iter()
                .filter(|r| r.kind == PartKind::Tone)
                .map(|r| {
                    format!(
                        "{} {:.1}Hz {:?} d{:.3}",
                        r.id,
                        r.freq_hz.unwrap_or(0.0),
                        r.error_db.map(|e| (e * 100.0).round() / 100.0),
                        r.drive
                    )
                })
                .collect();
            eprintln!("t {t:.1} {:?} lvl {:.2} {tones:?}", st.phase, st.level_db);
        }
        if stepped.is_some_and(|ts| t > ts + 12.0) {
            for r in st.parts.iter().filter(|r| r.kind == PartKind::Tone) {
                if let Some(e) = r.error_db {
                    match worst.iter_mut().find(|w| w.0 == r.id) {
                        Some(w) => {
                            w.1 = w.1.max(e.abs());
                            w.2 += e * e;
                            w.3 += 1;
                        }
                        None => worst.push((r.id.clone(), e.abs(), e * e, 1)),
                    }
                }
            }
            if t > stepped.unwrap() + 40.0 {
                break;
            }
        }
    }
    let summary: Vec<String> = worst
        .iter()
        .map(|w| {
            format!(
                "{} rms {:.2} worst {:.2} dB over {}",
                w.0,
                (w.2 / w.3 as f64).sqrt(),
                w.1,
                w.3
            )
        })
        .collect();
    eprintln!(
        "stepped at {stepped:?}; end {:?}; after the step {summary:?}",
        s.end_reason()
    );
    assert!(stepped.is_some());
    assert!(s.trip().is_none(), "{:?}", s.end_reason());
    // A 0.1 g follower on 1e-4 g^2/Hz reads with 0.6 dB of scatter
    // (sqrt(G B_n) / A), so its worst of a hundred readings is near 1.8 dB:
    // each tone is held within 0.8 dB rms and never reaches its 3 dB abort.
    assert_eq!(worst.len(), 4);
    assert!(
        worst
            .iter()
            .all(|w| (w.2 / w.3 as f64).sqrt() < 0.8 && w.1 < 3.0),
        "{summary:?}"
    );
}

/// The identified lag on the service's plant against the pipeline's own: the
/// same station as a flat wire (no modes, feedthrough 1) gives the pipeline's
/// lag alone; the modes' group delay is what the cross-correlation adds.
#[test]
fn the_identified_lag_is_the_pipeline_plus_the_modes_delay() {
    let lag = |modes: Vec<(f64, f64)>, feed: f64| {
        let sim = SimConfig::new()
            .sample_rate_hz(8192.0)
            .resonances(modes)
            .feedthrough(feed)
            .noise_level(2e-4)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .build();
        let mut s = MixedService::new(
            cfg(),
            table(),
            Box::new(sim),
            hertzforge_module_abi::StationClass::HertzforgeSim,
            1,
            None,
        )
        .unwrap()
        .with_block_frames(128);
        s.arm().unwrap();
        s.run_sysid(0).unwrap();
        s.sysid_lag_frames()
    };
    let wire = lag(Vec::new(), 1.0);
    let plant = lag(vec![(200.0, 2.5), (550.0, 2.5)], 0.35);
    eprintln!("lag: flat wire {wire} frames, the service's plant {plant} frames");
    assert!(plant >= wire && plant <= wire + 4, "{wire} {plant}");
}

/// The built-in tone law's servo, as it is (shared with Sine, not changed
/// here): a velocity-form PI whose rate-clamped step is back-calculated into
/// its error history. Measured, kp 0.3, ki 0.5, 20 dB/s at 10 ms ticks, a
/// reading stepped to and held 0.88 dB over target: the clamped 0.2 dB step
/// is applied twice (the back-calculation makes the second step exactly the
/// clamp), then the integral's 0.0044 dB a tick. It does not repeat the clamp
/// on every tick.
#[test]
fn the_servo_repeats_its_clamped_step_while_the_reading_holds() {
    let mut s = hertzforge_dsp::sine::AmplitudeServo::new(1.0, 20.0).with_gains(0.3, 0.5);
    let start = s.drive_scale();
    let read = 10f64.powf(0.88 / 20.0);
    let mut steps = Vec::new();
    let mut last = start;
    for _ in 0..50 {
        let now = s.update(read, 0.01);
        steps.push(20.0 * (now / last).log10());
        last = now;
    }
    eprintln!(
        "servo steps, dB: first {:.3}, tenth {:.3}, fiftieth {:.3}",
        steps[0], steps[9], steps[49]
    );
    assert!(
        (steps[0] + 0.2).abs() < 1e-9 && (steps[1] + 0.2).abs() < 1e-9,
        "{steps:?}"
    );
    assert!(
        steps[2..]
            .iter()
            .all(|d| (d + 0.5 * 0.88 * 0.01).abs() < 1e-9),
        "{steps:?}"
    );
}

/// The mechanism of the fall after a level step, in the built-in tone law's
/// servo alone (shared with Sine; not changed here). Once a step is
/// rate-clamped, the back-calculation leaves the next step at exactly the
/// clamp plus `(kp + ki dt)` times the error's change since, so while the
/// reading keeps moving the same way, by however little, every step is the
/// clamp. Replayed from the combined test without the read slew: the reading
/// handed over at +0.83 dB and a narrow tracker kept it drifting up 0.004 dB
/// a tick for 30 ticks, so the servo took 30 clamped 0.2 dB steps (6 dB in
/// 0.3 s, its full 20 dB/s), and stopped the tick after the reading turned.
#[test]
fn the_servo_takes_its_clamped_step_every_tick_while_its_reading_drifts_the_same_way() {
    let mut s = hertzforge_dsp::sine::AmplitudeServo::new(1.0, 20.0).with_gains(0.3, 0.5);
    let mut last = s.drive_scale();
    let mut steps = Vec::new();
    // On target, then the reading handed over at +0.83 dB, drifting up
    // 0.004 dB a tick for 30 ticks, then easing back down.
    let mut read_db: Vec<f64> = vec![0.0; 5];
    read_db.extend((0..30).map(|i| 0.83 + 0.004 * i as f64));
    read_db.extend((1..=10).map(|i| 0.946 - 0.004 * i as f64));
    for db in &read_db {
        let now = s.update(10f64.powf(db / 20.0), 0.01);
        steps.push(20.0 * (now / last).log10());
        last = now;
    }
    let clamped = steps.iter().filter(|d| (**d + 0.2).abs() < 1e-9).count();
    let fall: f64 = steps[5..35].iter().sum();
    eprintln!(
        "clamped steps {clamped}, fall over the drift {fall:.2} dB, step after it turns {:.4} dB",
        steps[35]
    );
    assert!(steps[..5].iter().all(|d| d.abs() < 1e-9));
    assert!(
        steps[5..35].iter().all(|d| (d + 0.2).abs() < 1e-9),
        "{steps:?}"
    );
    assert_eq!(clamped, 30);
    assert!(steps[35] > -0.2 + 1e-3, "{steps:?}");
}
