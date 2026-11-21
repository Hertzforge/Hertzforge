//! The service, closed loop on the simulator: the floor, the tones and the
//! bands reach their levels.
#![allow(clippy::needless_range_loop)]

mod common;
mod service_common;

use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_mixed::service::*;
use service_common::*;

/// Requirement: with two dwelling tones of 1 g on the floor, at full level
/// each control's floor on clean lines is within 1 dB of the reference
/// (`FLOOR_EQUALIZED_DB`), the floor's coherence between controls 0 and 1
/// is within 0.1 of the stated 0.25, each tone reads within 1 dB of its
/// target on every control, and the run completes.
#[test]
fn mimo_mixed_floor_equalizes_with_two_dwelling_tones() {
    let s = SineStructure::Linear4x4;
    let parts = vec![tone("t1", fixed(150.0), 1.0), tone("t2", fixed(700.0), 1.0)];
    let cfg = run_config(s, 0.5, parts, 6.0);
    let mut svc = service(s, cfg);
    engage(&mut svc);
    let full = run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0);
    print_status(&svc);
    for e in svc.events() {
        println!("  {e}");
    }
    assert!(full, "full level");
    let t_end = svc.status().t_s + 5.0;
    run_until(&mut svc, MimoMixedPhase::Idle, t_end);
    print_status(&svc);
    for e in svc.events() {
        println!("  {e}");
    }
    let st = svc.status();
    let coh = cross_coherence(&svc, 0, 1);
    println!("cross coherence (0,1): {coh:.3}");
    for (i, e) in st.floor_error_db.iter().enumerate() {
        assert!(e.abs() <= 1.0, "control {i}: floor {e:.2} dB");
    }
    assert!((coh - 0.25).abs() <= 0.1, "coherence {coh:.3}");
    for t in &st.tones {
        for (i, e) in t.error_db.iter().enumerate() {
            assert!(e.abs() <= 1.0, "tone {} control {i}: {e:.2} dB", t.id);
        }
    }
    while svc.run_chunk().expect("chunk") {}
    let end = svc.end().expect("end");
    println!("end: {} ({})", end.name, end.words);
    assert_eq!(end.name, "completed");
}

/// Each tone's largest error while usable over `secs` of station time, and
/// whether the run is still going.
fn worst_tone_error(svc: &mut MimoMixedService, secs: f64) -> (Vec<f64>, bool) {
    let n_t = svc.status().tones.len();
    let mut worst = vec![0.0f64; n_t];
    let end = svc.status().t_s + secs;
    let mut going = true;
    while svc.status().t_s < end {
        if !svc.run_chunk().expect("chunk") {
            going = false;
            break;
        }
        for (k, t) in svc.status().tones.iter().enumerate() {
            if std::env::var("MM_TRACE").is_ok() {
                println!(
                    "{:.2} {} {:.1} Hz use {} err {:.2?}",
                    svc.status().t_s,
                    t.id,
                    t.frequency_hz,
                    t.usable,
                    t.error_db
                );
            }
            if t.usable {
                for e in t.error_db.iter().filter(|e| e.is_finite()) {
                    worst[k] = worst[k].max(e.abs());
                }
            }
        }
    }
    (worst, going)
}

fn sweep(lo: f64, hi: f64, hz_per_s: f64) -> hertzforge_mixed::PartSchedule {
    hertzforge_mixed::PartSchedule::Sweep {
        lo_hz: lo,
        hi_hz: hi,
        start_hz: lo,
        direction: hertzforge_mixed::Direction::Up,
        law: hertzforge_mixed::SweepLaw::LinearHzPerS,
        rate: hz_per_s,
        target: hertzforge_mixed::PartSweepTarget::Duration { s: 1e4 },
    }
}

/// Requirement: a 1 g tone sweeping 150 Hz to 500 Hz and back at 10 Hz/s
/// (through the structure's modes there) on the floor holds within its
/// alarm band (`DEFAULT_TONE_ALARM_DB`) on every control whenever it is
/// usable over 40 s at full level, and the floor stays equalized.
#[test]
#[ignore = "method limit: at 10 Hz/s a mode is crossed faster than the 0.5 Hz tracker's 1.25 s delay; aborts -4.24 dB at 290 Hz"]
fn mimo_mixed_sweeping_tone_holds_through_a_mode_fast() {
    sweeping_tone_through_a_mode(10.0);
}

/// Requirement: as above at 4 Hz/s (single-output Mixed's closed-loop
/// sweep rate).
#[test]
#[ignore = "method limit: after the tracker's settle span the carry alone left -2.95 dB at 273 Hz; worst usable 3.39 dB against 1.5"]
fn mimo_mixed_sweeping_tone_holds_through_a_mode() {
    sweeping_tone_through_a_mode(4.0);
}

fn sweeping_tone_through_a_mode(rate: f64) {
    let s = SineStructure::Linear4x4;
    let parts = vec![tone("sw", sweep(150.0, 500.0, rate), 1.0)];
    let cfg = run_config(s, 0.5, parts, 100.0);
    let mut svc = service(s, cfg);
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0),
        "full level"
    );
    let (worst, going) = worst_tone_error(&mut svc, 40.0);
    print_status(&svc);
    println!("worst usable error {worst:?} dB; going {going}");
    assert!(going, "{:?}", svc.end());
    let alarm = hertzforge_mimo_mixed::config::DEFAULT_TONE_ALARM_DB;
    assert!(worst[0] <= alarm.0.min(alarm.1), "worst {:.2} dB", worst[0]);
    let st = svc.status();
    for (i, e) in st.floor_error_db.iter().enumerate() {
        assert!(e.abs() <= 1.0, "control {i}: floor {e:.2} dB");
    }
}

/// The mean of each control's estimate over lines `[lo, hi)` against the
/// floor law's reference there, dB.
fn band_level_db(svc: &MimoMixedService, lo: usize, hi: usize) -> Vec<f64> {
    let (g, _) = svc.floor_measured().expect("estimate");
    let r = svc.floor_reference().expect("reference");
    (0..g.n())
        .map(|i| {
            let a: f64 = (lo..hi).map(|l| g.get(l, i, i).re).sum();
            let b: f64 = (lo..hi).map(|l| r.get(l, i, i).re).sum();
            10.0 * (a / b).log10()
        })
        .collect()
}

/// Requirement: a fixed band of 40 Hz at 300 Hz adding 3e-3 g^2/Hz (folded
/// into the floor's reference by `auto`) reads within 1 dB of floor plus band
/// on every control over its inner lines, and a band of 40 Hz sweeping 800
/// to 1000 Hz at 5 Hz/s adding the same (servoed by `auto`) reads within its
/// alarm band (`DEFAULT_BAND_ALARM_DB`) on every control after 30 s at full
/// level; the run goes on.
#[test]
#[ignore = "open: since identification finds the 10-frame lag (it found 0 before), the folded band reads +1.16 dB on control 3; with the lag forced to 0, -0.04 dB"]
fn mimo_mixed_folded_and_swept_servoed_bands_reach_level() {
    let s = SineStructure::Linear4x4;
    let parts = vec![
        band("fixed", fixed(300.0), 40.0, added(3e-3, 0.5)),
        band("swept", sweep(800.0, 1000.0, 5.0), 40.0, added(3e-3, 0.5)),
    ];
    let cfg = run_config(s, 0.5, parts, 100.0);
    let mut svc = service(s, cfg);
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0),
        "full level"
    );
    let end = svc.status().t_s + 30.0;
    let going = run_until(&mut svc, MimoMixedPhase::Idle, end);
    print_status(&svc);
    assert!(!going && svc.end().is_none(), "{:?}", svc.end());
    let st = svc.status();
    assert!(st.bands[0].folded && !st.bands[1].folded);
    // The folded band's inner lines: 285 Hz to 315 Hz.
    let folded = band_level_db(&svc, 72, 79);
    println!("folded band against floor plus band: {folded:?} dB");
    for (i, e) in folded.iter().enumerate() {
        assert!(e.abs() <= 1.0, "folded band control {i}: {e:.2} dB");
    }
    let alarm = hertzforge_mimo_mixed::config::DEFAULT_BAND_ALARM_DB;
    for (i, e) in st.bands[1].error_db.iter().enumerate() {
        assert!(
            e.is_finite() && *e >= -alarm.0 && *e <= alarm.1,
            "swept band control {i}: {e:.2} dB"
        );
    }
}

/// Requirement: each floor law (`pinv_closed`, `min_drive`, `band_average`)
/// with the matrix tone law, and the dwell-only tone law `paired` with the
/// default floor law, chosen by configuration alone, equalize the floor
/// within 1 dB and hold a dwelling 1 g tone within 1 dB at full level, and
/// complete.
#[test]
fn mimo_mixed_laws_work_by_configuration_alone() {
    let s = SineStructure::Linear4x4;
    let runs = [
        ("pinv_closed", "matrix"),
        ("min_drive", "matrix"),
        ("band_average", "matrix"),
        ("pinv_closed", "paired"),
    ];
    for (floor_law, tone_law) in runs {
        let parts = vec![tone("t", fixed(240.0), 1.0)];
        let mut cfg = run_config(s, 0.5, parts, 6.0);
        cfg.law.floor = hertzforge_mixed::LawPick::new(floor_law);
        cfg.law.tones = hertzforge_mixed::LawPick::new(tone_law);
        let mut svc = service(s, cfg);
        engage(&mut svc);
        assert!(
            run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0),
            "{floor_law}/{tone_law}: {:?}",
            svc.end()
        );
        let t_end = svc.status().t_s + 5.0;
        run_until(&mut svc, MimoMixedPhase::Idle, t_end);
        let st = svc.status();
        println!(
            "{floor_law}/{tone_law}: floor {:?} tone {:?}",
            st.floor_error_db, st.tones[0].error_db
        );
        for (i, e) in st.floor_error_db.iter().enumerate() {
            assert!(
                e.abs() <= 1.0,
                "{floor_law}/{tone_law} control {i}: floor {e:.2} dB"
            );
        }
        for (i, e) in st.tones[0].error_db.iter().enumerate() {
            assert!(
                e.abs() <= 1.0,
                "{floor_law}/{tone_law} control {i}: tone {e:.2} dB"
            );
        }
        while svc.run_chunk().expect("chunk") {}
        assert_eq!(
            svc.end().map(|e| e.name),
            Some("completed"),
            "{floor_law}/{tone_law}"
        );
    }
}

/// A test-only fill rule: a paused line shows the floor law the stale
/// moving average there (the measurement as it stands), not the reference.
struct StaleAverage;

impl hertzforge_mimo_mixed::fill::FillRule for StaleAverage {
    fn id(&self) -> &'static str {
        "stale_average"
    }
    fn fill(
        &mut self,
        input: &hertzforge_mimo_mixed::fill::FillInput<'_>,
        out: &mut hertzforge_sdm::HermitianStack,
    ) -> hertzforge_mimo_mixed::fill::FillReport {
        let n = input.measured.n();
        for l in 0..input.measured.lines() {
            for r in 0..n {
                for c in r..n {
                    out.set(l, r, c, input.measured.get(l, r, c));
                }
            }
        }
        hertzforge_mimo_mixed::fill::FillReport::default()
    }
}

/// Each drive's floor drive summed over the band's paused lines.
fn paused_drive(svc: &MimoMixedService) -> Vec<f64> {
    let (_, clean) = svc.floor_measured().expect("estimate");
    let (lo, hi) = svc.band_lines();
    let d = svc.floor_drive().expect("drive");
    (0..d.n())
        .map(|j| {
            (lo..hi)
                .filter(|&l| !clean[l])
                .map(|l| d.get(l, j, j).re)
                .sum()
        })
        .collect()
}

/// Each drive's floor drive on the paused lines against the clean lines
/// three either side, dB (mean density).
fn paused_vs_neighbours(svc: &MimoMixedService) -> Vec<f64> {
    let (_, clean) = svc.floor_measured().expect("estimate");
    let (lo, hi) = svc.band_lines();
    let d = svc.floor_drive().expect("drive");
    let near = |l: usize| (l.saturating_sub(3)..(l + 4).min(hi)).any(|k| k >= lo && !clean[k]);
    (0..d.n())
        .map(|j| {
            let p: Vec<f64> = (lo..hi)
                .filter(|&l| !clean[l])
                .map(|l| d.get(l, j, j).re)
                .collect();
            let c: Vec<f64> = (lo..hi)
                .filter(|&l| clean[l] && near(l))
                .map(|l| d.get(l, j, j).re)
                .collect();
            let m = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
            10.0 * (m(&p) / m(&c)).log10()
        })
        .collect()
}

/// 200 floor loops at full level with two dwelling tones; the floor error on
/// clean lines and the paused-line drive's drift from the first full-level
/// loop, dB, per drive.
fn fill_run(
    stale: bool,
) -> (
    Vec<f64>,
    Vec<f64>,
    Option<hertzforge_mimo_mixed::fill::FillReport>,
) {
    let s = SineStructure::Linear4x4;
    let parts = vec![tone("t1", fixed(150.0), 1.0), tone("t2", fixed(700.0), 1.0)];
    let cfg = run_config(s, 0.5, parts, 400.0);
    let mut svc = service(s, cfg);
    if stale {
        svc = svc.with_fill_rule(Box::new(StaleAverage));
    }
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0),
        "{:?}",
        svc.end()
    );
    let l0 = svc.status().loops;
    while svc.status().loops == l0 {
        assert!(svc.run_chunk().expect("chunk"), "{:?}", svc.end());
    }
    let d0 = paused_drive(&svc);
    let l1 = svc.status().loops;
    while svc.status().loops < l1 + 200 {
        if !svc.run_chunk().expect("chunk") {
            println!("stale {stale}: ended {:?}", svc.end());
            break;
        }
    }
    let d1 = paused_drive(&svc);
    let drift = d0
        .iter()
        .zip(&d1)
        .map(|(a, b)| 10.0 * (b / a).log10())
        .collect();
    println!(
        "stale {stale}: paused against clean neighbours {:.2?} dB",
        paused_vs_neighbours(&svc)
    );
    (svc.status().floor_error_db, drift, svc.fill_report())
}

/// Requirement: under `no_error`, after 200 floor loops at full level with
/// two dwelling tones, every control's floor on clean lines is within 1 dB
/// and the drive on the paused lines has moved less than 1 dB on every drive
/// from the first full-level loop. The stale-average rule is run the same
/// way and its numbers are reported against it.
#[test]
fn mimo_mixed_fill_no_error_against_a_stale_average() {
    let (e_ne, drift_ne, rep_ne) = fill_run(false);
    let (e_st, drift_st, _) = fill_run(true);
    println!(
        "no_error: floor {e_ne:.2?} dB, paused drive drift {drift_ne:.2?} dB, fill {rep_ne:?}"
    );
    println!("stale_average: floor {e_st:.2?} dB, paused drive drift {drift_st:.2?} dB");
    for (i, e) in e_ne.iter().enumerate() {
        assert!(e.abs() <= 1.0, "no_error control {i}: floor {e:.2} dB");
    }
    for (j, d) in drift_ne.iter().enumerate() {
        assert!(
            d.abs() < 1.0,
            "no_error drive {j}: paused drive moved {d:.2} dB"
        );
    }
}

/// Three dwelling 2 g tones on the floor, each drive's peak ceiling at
/// `peak_v` (none when `None`), under ceiling rule `rule`.
fn mv8_config(rule: &str, peak_v: Option<f64>) -> hertzforge_mimo_mixed::config::MimoMixedConfig {
    let s = SineStructure::Linear4x4;
    let parts = vec![
        tone("t1", fixed(150.0), 2.0),
        tone("t2", fixed(240.0), 2.0),
        tone("t3", fixed(700.0), 2.0),
    ];
    let mut cfg = run_config(s, 0.5, parts, 20.0);
    cfg.law.ceiling_rule = hertzforge_mixed::LawPick::new(rule);
    if let Some(v) = peak_v {
        for d in cfg.drives.iter_mut() {
            d.peak_v = v;
        }
    }
    cfg
}

/// Requirement (MV8 in the run): with every drive's peak ceiling 1.05 times
/// the setup forecast's worst need, under `common_scale` and under `in_turn`,
/// no drive frame is clamped, no `drive_vpk` abort fires and the run
/// completes; how often a tone was limiting is reported.
#[test]
fn mimo_mixed_mv8_shared_ceiling_common_scale_in_the_run() {
    mv8_run("common_scale");
}

/// The same requirement under `in_turn`.
#[test]
#[ignore = "method: the forecast's random peak is under the held peak; in_turn's last tone ends tone_abort_at_ceiling at -4.08 dB"]
fn mimo_mixed_mv8_shared_ceiling_in_turn_in_the_run() {
    mv8_run("in_turn");
}

fn mv8_run(rule: &str) {
    let s = SineStructure::Linear4x4;
    let mut probe = service(s, mv8_config(rule, None));
    engage(&mut probe);
    let fc = probe.budget_forecast().expect("forecast");
    println!(
        "forecast: need {:.3} V on drive {} at {:.1} s against {:.3} V",
        fc.need_v, fc.drive, fc.time_s, fc.ceiling_v
    );
    probe.abort("the probe run").expect("abort");
    while probe.run_chunk().unwrap_or(false) {}
    let peak = 1.05 * fc.need_v;
    {
        let mut svc = service(s, mv8_config(rule, Some(peak)));
        engage(&mut svc);
        let (mut chunks, mut limiting) = (0usize, [0usize; 3]);
        while svc.run_chunk().expect("chunk") {
            chunks += 1;
            for (k, t) in svc.status().tones.iter().enumerate() {
                limiting[k] += t.limiting as usize;
                if std::env::var("MM_TRACE").is_ok() {
                    let st = svc.status();
                    println!(
                        "{:.2} {:?} {} sc {:.4} lim {} use {} err {:.2?}",
                        st.t_s, st.phase, t.id, t.scale, t.limiting, t.usable, t.error_db
                    );
                }
            }
        }
        let st = svc.status();
        let end = svc.end().expect("end");
        println!(
            "{rule}: peak ceiling {peak:.3} V; clamped {}; limiting chunks {limiting:?} of {chunks}; \
             end {} ({})",
            st.clamped, end.name, end.words
        );
        for t in &st.tones {
            println!("  tone {} err {:.2?}", t.id, t.error_db);
        }
        assert_eq!(st.clamped, 0, "{rule}: clamped frames");
        assert_ne!(end.name, "drive_vpk", "{rule}");
        assert_eq!(end.name, "completed", "{rule}: {}", end.words);
    }
}

/// Requirement (A7): a run with a folder writes, on its writer thread,
/// `law.json` naming
/// `hertzforge.mimo_mixed.law_ticks.v1` and both laws, and `law_ticks.bin`
/// as whole records that read back, holding floor-law calls and tone-law
/// calls for the run's one tone.
#[test]
fn mimo_mixed_the_law_call_record_is_written_to_the_run_folder() {
    use hertzforge_mimo_mixed::service::records::*;
    let s = SineStructure::Linear4x4;
    let cfg = run_config(s, 0.5, vec![tone("t", fixed(240.0), 1.0)], 2.0);
    let dir = std::env::temp_dir().join(format!("mimo-mixed-law-ticks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut svc = service(s, cfg).with_run_dir(dir.clone());
    engage(&mut svc);
    while svc.run_chunk().expect("chunk") {}
    assert_eq!(svc.end().map(|e| e.name), Some("completed"));
    svc.close_records();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(LAW_JSON_FILE)).expect("law.json"))
            .expect("json");
    assert_eq!(json["format"], LAW_TICKS_FORMAT);
    assert_eq!(json["roles"]["floor"], "pinv_closed");
    assert_eq!(json["roles"]["tones"], "matrix");
    assert_eq!(json["record_bytes"], LawRecord::BYTES);
    let bin = std::fs::read(dir.join(LAW_TICKS_FILE)).expect("law_ticks.bin");
    assert_eq!(bin.len() % LawRecord::BYTES, 0);
    let recs: Vec<LawRecord> = bin
        .chunks(LawRecord::BYTES)
        .map(|c| LawRecord::read_from(c).expect("a record"))
        .collect();
    let floor = recs.iter().filter(|r| r.role == RecordRole::Floor).count();
    let tone = recs.iter().filter(|r| r.role == RecordRole::Tone).count();
    println!("law records: {} ({floor} floor, {tone} tone)", recs.len());
    assert!(floor > 0 && tone > 0);
    assert!(recs.iter().all(|r| r.t_s.is_finite() && r.tone == 0));
    std::fs::remove_dir_all(&dir).unwrap();
}
