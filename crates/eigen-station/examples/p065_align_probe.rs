//! `p065-align-probe` - the alignment discriminator: ONE variable at a time against
//! the ~2049-frame silence that precedes an OVER-BUFFER classical shock record.
//!
//! # The question
//!
//! `ni_module_sweep`'s classical alignment row measures xcorr lag -2370 against a
//! model of -321 on a 6195-frame record (residual -2049 frames = 40.0 ms of
//! silence at the head, tail lost off the end), bit-stable across runs and across
//! two applied fixes. An earlier baseline sitting measured -323 on a 2817-frame
//! record. Those two numbers were taken on DIFFERENT builds with DIFFERENT record
//! lengths, so "in-buffer is fine, over-buffer is late" was never a controlled
//! comparison - it was two observations with two variables moved.
//!
//! This probe fires the SAME pulse spec at BOTH compensation geometries and at
//! a swept pre-start commit depth, in ONE process on ONE build, and prints the
//! write ledger beside the alignment for every case. It moves one variable per
//! row, which is what the last two sittings did not do.
//!
//! # What each case answers
//!
//! * **1.5x shoulders (record 2817 < the 3072-frame AO buffer)** vs **4.5x
//!   shoulders (record 6195 > buffer)** at the same commit policy: is RECORD
//!   LENGTH the variable at this build, or was the baseline number a build artifact?
//! * **4.5x at commit depths {512, 1024, 2048, uncapped}**: if the silence
//!   scales with (something - committed) the delay is HOST-side accounting; if
//!   it stays ~2049 regardless the delay is DEVICE-side; if it tracks the
//!   2048-frame lookahead it is the `start()` top-up path.
//!
//! Run with `EIGEN_NI_DIAG=1` - the ledger lines (`[shock-diag]
//! commit_whole_record`, `[ni-diag] start()`) are the host-side half of the
//! evidence and this probe is not worth a sitting without them.
//!
//! # What this probe CANNOT see
//!
//! Everything here is measured through the AI jumper, so it cannot separate
//! "the drive was late" from "the acquisition was early".
//! `tools/probes/shock_dmm.py` on the OUTPUT NODE is the instrument for that,
//! and the drive-side truth is a separate row of the investigation.
//!
//! # Bench contract
//!
//! The one every chassis example carries: AO0 jumpered to AI0, NOTHING
//! downstream, IEPE OFF, drive well inside +/-1 V.
//!
//! ```text
//! EIGEN_NI_DIAG=1 cargo run --release -p eigen-station --example p065_align_probe
//! ```
//!
//! Exit: 0 = ran and printed (this probe asserts NOTHING - it is an
//! instrument); 2 = could not reach real hardware.

use eigen_shock_common::ShockLevelSchedule;
use eigen_shock_host::service::firer::quiet_lead_frames;
use eigen_shock_host::{
    BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig, ToleranceStandard,
};
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
    NI_BUILD_RATE_HZ,
};

/// The AI and AO modules this example binds. `EIGEN_NI_AI_DEV` / `EIGEN_NI_AO_DEV`
/// override the defaults, so a chassis that enumerates under another name
/// (`cDAQ2Mod1`, ...) runs without editing this file and without renaming the
/// chassis in NI MAX.
fn ai_dev() -> String {
    ni_dev_from_env("EIGEN_NI_AI_DEV", "cDAQ1Mod1")
}

fn ao_dev() -> String {
    ni_dev_from_env("EIGEN_NI_AO_DEV", "cDAQ1Mod2")
}

/// `var` when it is set and not blank, else `default`.
fn ni_dev_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The sweep's teaching pulse, unchanged: 11 ms half-sine.
const DURATION_MS: f64 = 11.0;
/// Commanded MAIN-pulse peak at 0 dB. The rows fire at -12 dB.
const AMPLITUDE: f64 = 0.5;
/// Detection floor for the correlation peak - the same value and the same
/// meaning as the sweep's: it decides whether a peak exists, not whether it is
/// in the right place.
const PEAK_FLOOR_R: f64 = 0.30;

fn station() -> eigen_station::StationBackend {
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            inputs: Vec::new(),
            outputs: Vec::new(),
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            restrict_devices: vec![ai_dev(), ao_dev()],
        },
        ni_block_frames: eigen_hal::default_block_frames(NI_BUILD_RATE_HZ),
    };
    let s = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station: REFUSED - {e:?}");
            std::process::exit(2);
        }
    };
    if s.provenance.driver_simulated == Some(true) {
        eprintln!("station resolved DRIVER-SIMULATED - this probe is only evidence on real iron");
        std::process::exit(2);
    }
    s
}

fn volts_table() -> eigen_channel_table::ChannelTable {
    use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::VoltageV;
    ctrl.sensitivity_mv_per_eu = 1000.0;
    ctrl.range_max = 1.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("eigen-p065-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}

fn cfg(shoulder_frac: f64) -> ClassicalTestConfig {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, AMPLITUDE, DURATION_MS);
    spec.compensation.pre_frac = shoulder_frac;
    spec.compensation.post_frac = shoulder_frac;
    let mut c = ClassicalTestConfig::new(spec);
    c.standard = ToleranceStandard::Mil810_516;
    c.schedule = ShockLevelSchedule::manual_default();
    c.schedule.start_db = -12.0;
    c
}

/// The sweep's xcorr, verbatim in convention: `b[j]` pairs with `a[j + lag]`,
/// so a record delayed by N frames reports lag `-N`.
fn xcorr_peak(a: &[f64], b: &[f64]) -> (isize, f64) {
    if a.is_empty() || b.is_empty() {
        return (0, 0.0);
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let ea: f64 = a.iter().map(|x| (x - ma) * (x - ma)).sum::<f64>().sqrt();
    let eb: f64 = b.iter().map(|x| (x - mb) * (x - mb)).sum::<f64>().sqrt();
    if ea <= 0.0 || eb <= 0.0 {
        return (0, 0.0);
    }
    let (n, m) = (a.len() as isize, b.len() as isize);
    let (mut best_lag, mut best) = (0isize, 0.0f64);
    for lag in -(m - 1)..n {
        let mut acc = 0.0;
        let jlo = (-lag).max(0);
        let jhi = (n - lag).min(m);
        for j in jlo..jhi {
            acc += (a[(j + lag) as usize] - ma) * (b[j as usize] - mb);
        }
        let r = acc / (ea * eb);
        if r.abs() > best.abs() {
            best = r;
            best_lag = lag;
        }
    }
    (best_lag, best)
}

/// The first frame whose magnitude clears `thresh`. The direct read of "how
/// much silence is at the head", independent of the correlation.
fn first_above(v: &[f64], thresh: f64) -> Option<usize> {
    v.iter().position(|x| x.abs() > thresh)
}

fn rms(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

fn one_case(label: &str, shoulder_frac: f64, cap: Option<usize>) {
    println!();
    println!("--- CASE: {label} ---");
    let st = station();
    let firer = match BackendPulseFirer::new(st.backend, volts_table(), st.station) {
        Ok(f) => f.with_pre_start_cap(cap),
        Err(e) => {
            println!("  REFUSED firer construct: {e:?}");
            return;
        }
    };
    let mut svc = match ClassicalShockService::new(
        cfg(shoulder_frac),
        volts_table(),
        firer,
        Some(tmp(label)),
    ) {
        Ok(s) => s,
        Err(e) => {
            println!("  REFUSED service construct: {e:?}");
            return;
        }
    };
    if let Err(e) = svc.assess_feasibility() {
        println!("  REFUSED feasibility: {e:?}");
        return;
    }
    if let Err(e) = svc.arm() {
        println!("  REFUSED arm: {e:?}");
        return;
    }
    if let Err(e) = svc.fire_next_pulse() {
        println!("  REFUSED fire: {e:?}");
        return;
    }
    let fs = svc.fs_hz();
    let Some(rec) = svc.pulse_store().last() else {
        println!("  the fire returned Ok and the store is EMPTY");
        return;
    };
    let commanded = svc.config().spec.synthesize(fs).waveform_g;
    let q = quiet_lead_frames(DURATION_MS / 1000.0, fs);
    let adc = eigen_hal_ni::clock::group_delay_samples(fs)
        .map(|d| d.ceil() as usize)
        .unwrap_or(0);
    let model_lag = -((q + adc) as isize);
    let (lag, r) = xcorr_peak(&commanded, &rec.time_history);

    // The head, measured directly rather than inferred from the correlation.
    // Threshold: 10x the RMS of the first 256 frames, which on this jumper is
    // acquisition noise in every case.
    let head_noise = rms(&rec.time_history[..256.min(rec.time_history.len())]);
    let onset = first_above(&rec.time_history, (head_noise * 10.0).max(1e-4));

    println!(
        "  record {} frames ({} composite + 2 x {q} quiet) at {fs:.4} Hz; pre-start cap {cap:?}",
        rec.time_history.len(),
        commanded.len()
    );
    println!("  MODEL lag {model_lag} = -({q} quiet lead + {adc} ADC group delay)",);
    if r.abs() < PEAK_FLOOR_R {
        println!("  MEASURED NO-PEAK: best |r| {:.4} at lag {lag}", r.abs());
    } else {
        println!(
            "  MEASURED lag {lag} (r {r:+.6}); RESIDUAL {:+} frames = {:+.2} ms",
            lag - model_lag,
            (lag - model_lag) as f64 * 1000.0 / fs
        );
    }
    println!(
        "  HEAD    noise rms {head_noise:.6} EU over the first 256 frames; first frame above \
         10x that: {}",
        onset
            .map(|i| format!("{i} ({:.2} ms in)", i as f64 * 1000.0 / fs))
            .unwrap_or_else(|| "NONE (the whole record is at the noise floor)".into())
    );
}

fn main() {
    println!("=== p065-align-probe: one variable at a time ===");
    if std::env::var_os("EIGEN_NI_DIAG").is_none() {
        println!("(EIGEN_NI_DIAG is NOT set - the commit/prefill ledger will be absent and this");
        println!(" run is half an instrument. Re-run with EIGEN_NI_DIAG=1.)");
    }

    // A. RECORD LENGTH, commit policy held at the shipped default.
    one_case("A1 in-buffer (1.5x shoulders), default commit", 1.5, None);
    one_case("A2 over-buffer (4.5x shoulders), default commit", 4.5, None);

    // B. COMMIT DEPTH, record held at the over-buffer geometry.
    one_case("B1 over-buffer, commit cap 512", 4.5, Some(512));
    one_case("B2 over-buffer, commit cap 1024", 4.5, Some(1024));
    one_case("B3 over-buffer, commit cap 2048", 4.5, Some(2048));
    one_case(
        "B4 over-buffer, commit cap 4096 (>= buffer: uncapped)",
        4.5,
        Some(4096),
    );

    println!();
    println!("=== end p065-align-probe (this probe asserts nothing) ===");
}
