//! The service, closed loop on the simulator: the sine structures at their
//! calibration rate, the reference on a 4 Hz grid (4096-sample frames).
#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::plants::{SineStructure, SINE_CALIBRATION_RATE_HZ};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_mimo_mixed::config::*;
use hertzforge_mimo_mixed::service::*;
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::LevelSchedule;
use hertzforge_random::RunMode;

const FS: f64 = SINE_CALIBRATION_RATE_HZ;
const DF: f64 = 4.0;
const LINES: usize = 2049;
/// 20 Hz to 1800 Hz.
const BAND: (usize, usize) = (5, 451);
const FLOOR_G2HZ: f64 = 1e-3;

fn table(s: SineStructure) -> ChannelTable {
    let (n_out, n_d) = s.dims();
    let mut t = ChannelTable::new();
    for k in 0..n_out + n_d {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < s.n_controls() {
            ChannelRole::Control
        } else if k < n_out {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

fn backend(s: SineStructure, seed: u64) -> Box<dyn hertzforge_hal::Backend> {
    let b: SimBackend = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(s.build().model)
        .noise_level(1e-4)
        .seed(seed)
        .shared_clock(true)
        .build();
    Box::new(b)
}

/// A run on structure `s`: the floor at `FLOOR_G2HZ` with coherence `gamma`
/// between every pair, `parts` after it; 8 frames a loop; the level from
/// -6 dB to full level, held `full_s`.
fn run_config(s: SineStructure, gamma: f64, parts: Vec<MimoPart>, full_s: f64) -> MimoMixedConfig {
    let (n_out, n_d) = s.dims();
    let n = s.n_controls();
    let mut all = vec![floor("floor")];
    all.extend(parts);
    let mut cfg = config(n, n_d, gamma, all);
    cfg.controls = (1..=n as u32).collect();
    cfg.drives = (0..n_d as u32)
        .map(|j| MimoDrive::new(n_out as u32 + 1 + j, 2.0))
        .collect();
    cfg.reference = reference(LINES, DF, BAND, FLOOR_G2HZ, &coherence(n, gamma));
    cfg.estimation.frames_per_loop = 8;
    cfg.exclusion_hz = 8.0;
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: -6.0,
        table_db: vec![-6.0, -3.0, 0.0],
        dwell_s: vec![2.0],
    };
    cfg.full_level_duration_s = Some(full_s);
    cfg.level_ramp_s = 0.5;
    cfg.shutdown_ramp_s = 0.5;
    cfg.part_ramp_s = 0.5;
    cfg.soft_start_db = 3.0;
    cfg
}

fn service(s: SineStructure, cfg: MimoMixedConfig) -> MimoMixedService {
    let laws = MimoMixedLaws::builtin(&cfg).expect("built-in laws");
    MimoMixedService::new(
        cfg,
        table(s),
        backend(s, 7),
        StationClass::HertzforgeSim,
        laws,
    )
    .expect("service")
}

/// Arm, identify, pass the gate, proceed.
fn engage(svc: &mut MimoMixedService) {
    svc.arm().expect("arm");
    let id = svc.identify().expect("identify");
    println!(
        "identified: level {:.2} dB, {} steps, lag {}, coherence {:?}",
        id.level_db, id.steps, id.lag_frames, id.band_coherence
    );
    assert!(svc.coherence_gate().expect("gate"), "the coherence gate");
    svc.proceed().expect("proceed");
}

/// Run until the phase is `phase` (or the run ends).
fn run_until(svc: &mut MimoMixedService, phase: MimoMixedPhase, limit_s: f64) -> bool {
    while svc.status().t_s < limit_s {
        if svc.phase() == phase {
            return true;
        }
        if !svc.run_chunk().expect("chunk") {
            return false;
        }
        if std::env::var("MM_TRACE").is_ok() {
            let st = svc.status();
            for t in &st.tones {
                println!(
                    "{:.2} {} pl {:.2} use {} eng {} sc {:.6} meas {:.3?} tgt {:.3?}",
                    st.t_s, t.id, t.part_level, t.usable, t.engaged, t.scale, t.measured, t.target
                );
            }
        }
    }
    svc.phase() == phase
}

fn print_status(svc: &MimoMixedService) {
    let s = svc.status();
    println!(
        "t {:.2} s phase {:?} level {:.2} dB floor {:?} eq {} loops {} underruns {} dropped {} \
         clamped {} unsteady {}",
        s.t_s,
        s.phase,
        s.level_db,
        s.floor_error_db,
        s.floor_equalized,
        s.loops,
        s.underruns,
        s.dropped,
        s.clamped,
        svc.unsteady_frames()
    );
    for t in &s.tones {
        println!(
            "  tone {} {:.1} Hz pl {:.2} eng {} use {} lim {} err {:?}",
            t.id, t.frequency_hz, t.part_level, t.engaged, t.usable, t.limiting, t.error_db
        );
    }
    for b in &s.bands {
        println!("  band {} folded {} err {:?}", b.id, b.folded, b.error_db);
    }
}

/// The floor's mean coherence `|G_ab|^2 / (G_aa G_bb)` over clean band lines.
fn cross_coherence(svc: &MimoMixedService, a: usize, b: usize) -> f64 {
    let (g, clean) = svc.floor_measured().expect("estimate");
    let (lo, hi) = svc.band_lines();
    let mut s = 0.0;
    let mut k = 0.0;
    for l in lo..hi {
        if !clean[l] {
            continue;
        }
        let d = g.get(l, a, a).re * g.get(l, b, b).re;
        if d > 0.0 {
            s += g.get(l, a, b).norm_sqr() / d;
            k += 1.0;
        }
    }
    s / k
}

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
