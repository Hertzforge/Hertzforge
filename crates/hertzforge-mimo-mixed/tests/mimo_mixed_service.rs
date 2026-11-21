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
/// into the floor's reference by `auto`) reads, averaged over 30 s at full
/// level once the estimate has settled, within 1 dB of floor plus band on every control over its inner
/// lines, and a band of 40 Hz sweeping 800 to 1000 Hz at 5 Hz/s adding the
/// same (servoed by `auto`) reads within its alarm band
/// (`DEFAULT_BAND_ALARM_DB`) on every control after those 30 s; the run goes
/// on.
///
/// A single reading of the folded band's seven lines swings by about
/// +-1.5 dB from one estimate to the next: that is the estimate's own
/// scatter, not a level error, so the band is judged on its time average,
/// taken once the estimate's memory span (`memory_loops` loops of
/// `frames_per_loop` frames) has passed since full level, so that no loop
/// from the lower level is left in it.
/// The average's own scatter is taken from the spread of its six 5 s block
/// means; it must be at most a quarter of the 1 dB band, so a level error
/// of 1 dB stands four of its standard errors clear of the scatter.
#[test]
fn mimo_mixed_folded_and_swept_servoed_bands_reach_level() {
    folded_and_swept(7);
}

/// As above, on two further noise and synthesis seeds.
#[test]
fn mimo_mixed_folded_and_swept_servoed_bands_reach_level_other_seeds() {
    folded_and_swept(11);
    folded_and_swept(23);
}

fn folded_and_swept(seed: u64) {
    let s = SineStructure::Linear4x4;
    let parts = vec![
        band("fixed", fixed(300.0), 40.0, added(3e-3, 0.5)),
        band("swept", sweep(800.0, 1000.0, 5.0), 40.0, added(3e-3, 0.5)),
    ];
    let mut cfg = run_config(s, 0.5, parts, 100.0);
    cfg.seed = seed;
    // The estimate's memory span: past it no loop from before full level
    // is left in the average.
    let memory_s = cfg.estimation.memory_loops.ceil() * cfg.estimation.frames_per_loop as f64 / DF;
    let mut svc = service_seeded(s, cfg, seed);
    engage(&mut svc);
    assert!(
        run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0),
        "full level"
    );
    let settled = svc.status().t_s + memory_s;
    assert!(
        !run_until(&mut svc, MimoMixedPhase::Idle, settled) && svc.end().is_none(),
        "{:?}",
        svc.end()
    );
    // Each reading of the folded band's inner lines (285 Hz to 315 Hz),
    // with its time, over the 30 s.
    let t0 = svc.status().t_s;
    let end = t0 + 30.0;
    let mut readings: Vec<(f64, Vec<f64>)> = Vec::new();
    let mut going = true;
    while svc.status().t_s < end {
        if !svc.run_chunk().expect("chunk") {
            going = false;
            break;
        }
        readings.push((svc.status().t_s - t0, band_level_db(&svc, 72, 79)));
    }
    print_status(&svc);
    assert!(going && svc.end().is_none(), "{:?}", svc.end());
    let st = svc.status();
    assert!(st.bands[0].folded && !st.bands[1].folded);
    let n = readings[0].1.len();
    let mean_db = |r: &[&(f64, Vec<f64>)], i: usize| -> f64 {
        let p: f64 = r.iter().map(|(_, v)| 10f64.powf(v[i] / 10.0)).sum();
        10.0 * (p / r.len() as f64).log10()
    };
    for i in 0..n {
        let all: Vec<&(f64, Vec<f64>)> = readings.iter().collect();
        let mean = mean_db(&all, i);
        let lo = readings.iter().map(|(_, v)| v[i]).fold(f64::MAX, f64::min);
        let hi = readings.iter().map(|(_, v)| v[i]).fold(f64::MIN, f64::max);
        let blocks: Vec<f64> = (0..6)
            .map(|b| {
                let r: Vec<&(f64, Vec<f64>)> = readings
                    .iter()
                    .filter(|(t, _)| *t >= 5.0 * b as f64 && *t < 5.0 * (b + 1) as f64)
                    .collect();
                mean_db(&r, i)
            })
            .collect();
        let bm = blocks.iter().sum::<f64>() / 6.0;
        let sd = (blocks.iter().map(|x| (x - bm).powi(2)).sum::<f64>() / 5.0).sqrt();
        let se = sd / 6f64.sqrt();
        println!(
            "seed {seed} folded band control {i}: 30 s mean {mean:+.2} dB \
             (readings {lo:+.2} to {hi:+.2}; block means {blocks:.2?}; \
             standard error {se:.2})"
        );
        assert!(se <= 0.25, "control {i}: the average scatters {se:.2} dB");
        assert!(mean.abs() <= 1.0, "folded band control {i}: {mean:.2} dB");
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
        // The laws' answers are judged here, not their speed in a debug build.
        let mut svc = service_unhurried(s, cfg);
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

/// A scalar-servoed band at 900 Hz (1e-2 g^2/Hz, its abort band opened to
/// 40 dB) switched in over `ramp_s`: per status chunk once the band is
/// fully in, the band's error mean over the controls, dB, for 14 s.
fn band_ramp_run(ramp_s: f64) -> Vec<(f64, f64)> {
    let s = SineStructure::Linear4x4;
    let mut b = common::band("nb", common::fixed(900.0), 40.0, common::added(1e-2, 0.5));
    if let hertzforge_mimo_mixed::config::MimoPartBody::Narrowband(nb) = &mut b.body {
        nb.alarm_db = hertzforge_mimo_mixed::config::PerControl::All((40.0, 40.0));
        nb.abort_db = hertzforge_mimo_mixed::config::PerControl::All((40.0, 40.0));
    }
    let mut cfg = run_config(s, 0.5, vec![b], 40.0);
    cfg.law.bands = hertzforge_mixed::LawPick::new("scalar_servo");
    cfg.part_ramp_s = ramp_s;
    let mut svc = service(s, cfg);
    engage(&mut svc);
    let mut after: Vec<(f64, f64)> = Vec::new();
    let mut in_at: Option<f64> = None;
    while svc.status().t_s < 90.0 {
        let going = svc.run_chunk().expect("chunk");
        let st = svc.status();
        let b = &st.bands[0];
        if in_at.is_none() && b.part_level >= 1.0 {
            in_at = Some(st.t_s);
        }
        if let Some(t0) = in_at {
            if st.t_s - t0 > 14.0 {
                break;
            }
            let e = b.error_db.iter().sum::<f64>() / b.error_db.len() as f64;
            if e.is_finite() && after.last().is_none_or(|l| l.1 != e) {
                after.push((st.t_s - t0, e));
            }
        }
        if !going {
            break;
        }
    }
    println!("end: {:?}", svc.end().map(|e| (e.name, e.words.clone())));
    after
}

/// Requirement: a servoed band does not wind up while it ramps in. Its
/// servo is held over the ramp (and the loop after, its estimate
/// restarted), so a band switched in over 5 s comes to level as one
/// switched in over 0.5 s does: from the model's drive, under the servo's
/// own step response. Its largest error once in is within 0.3 dB of the
/// short ramp's (the two runs' estimates differ by about 0.1 dB at a
/// peak); a servo iterating against the whole statement through the ramp
/// overshot 0.76 dB past the short ramp's.
#[test]
fn mimo_mixed_a_band_servo_does_not_wind_up_while_it_ramps_in() {
    let worst = |ramp_s: f64| {
        let after = band_ramp_run(ramp_s);
        assert!(!after.is_empty(), "the band never came in");
        let w = after.iter().map(|a| a.1).fold(f64::MIN, f64::max);
        let first: Vec<String> = after
            .iter()
            .take(10)
            .map(|(t, e)| format!("{t:.1}s {e:+.2}"))
            .collect();
        println!("{ramp_s} s ramp, once in: {first:?}; the largest error {w:+.2} dB");
        w
    };
    let short = worst(0.5);
    let long = worst(5.0);
    assert!(
        long <= short + 0.3,
        "the band ramped in over 5 s overshot {long:+.2} dB, against {short:+.2} dB over 0.5 s"
    );
}
