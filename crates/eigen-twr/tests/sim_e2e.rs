//! THE TWR acceptance test:
//! the full service pipeline - sysid -> gated ladder of inverse-FRF correction
//! iterations -> scheduled playback - against the REAL eigen-hal-sim plant
//! (multi-mode resonant, transport delay, noise), converging a synthetic
//! field-like transient.
//!
//! Run with `cargo test -p eigen-twr --test sim_e2e -- --nocapture` to see the
//! table.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable};
use eigen_hal_sim::SimConfig;
use eigen_twr::service::config::TwrConfig;
use eigen_twr::service::TwrState;
use eigen_twr::{BackendReferencePlayer, ReferenceWaveform, RunSchedule, TwrService};

const FS: f64 = 8192.0;

/// A synthetic "field-like" transient: three decaying tones plus a scattered
/// multi-sine floor, 0.75 s, peak-normalized to 1 g. Deterministic.
fn field_like_reference() -> ReferenceWaveform {
    let n = 6144usize;
    let mut s = vec![0.0f64; n];
    let tones: [(f64, f64, f64, f64); 3] = [
        // (freq Hz, amplitude, decay tau s, phase)
        (160.0, 1.00, 0.18, 0.3),
        (400.0, 0.65, 0.10, 1.7),
        (750.0, 0.35, 0.06, 4.1),
    ];
    for (f, a, tau, ph) in tones {
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / FS;
            *v += a * (-t / tau).exp() * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    // A scattered multi-sine floor (deterministic "texture", 60..1100 Hz).
    for k in 0..15 {
        let f = 60.0 + 70.0 * k as f64;
        let ph = (k as f64) * 2.399963; // golden-angle phases
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / FS;
            *v += 0.04 * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    let peak = s.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    for v in s.iter_mut() {
        *v /= peak;
    }
    ReferenceWaveform::from_samples("synthetic field transient", FS, s).unwrap()
}

fn sim_player() -> BackendReferencePlayer<eigen_hal_sim::SimBackend> {
    // A 3-mode plant with transport delay and a noise floor - deliberately NOT
    // flat: the inverse-FRF law has real work to do.
    let backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(vec![(180.0, 8.0), (420.0, 10.0), (950.0, 6.0)])
        .feedthrough(0.25)
        .noise_level(2e-4)
        .transport_delay_samples(24)
        .shared_clock(true)
        .seed(0x71E5_1DE2_E001_0001)
        .build();
    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    table.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    table.channels.push(drv);
    BackendReferencePlayer::new(backend, table, eigen_module_abi::StationClass::EigenSim)
        .unwrap()
        .with_quiet_frames(1024)
}

fn config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(field_like_reference());
    c.prep.highpass_hz = Some(20.0);
    c.prep.taper_frac = Some(0.05);
    c.pad_pre = 512;
    c.pad_post = 1024;
    // SysId SNR sizing: the coherence trust gate excludes bins the estimate
    // cannot phase-trust, and every excluded bin's target energy is a PERMANENT
    // error floor (the coverage guard aborts when it exceeds
    // 1 - min_masked_energy_frac). A 10% gate needs ~99% coverage, so the
    // ranging drive is sized to lift the plant's inter-mode valleys well above
    // the sim noise floor and the run is longer for more Welch averages.
    c.sysid.drive_rms = 0.15;
    c.sysid.duration_s = 8.0;
    c.sysid.nfft = 2048;
    c.sysid.f_lo_hz = 25.0;
    c.sysid.f_hi_hz = 1500.0;
    c.inverse.tikhonov_beta_floor = 0.02;
    c.inverse.coherence_floor = 0.6;
    // 6 dB conservative initial backoff: the first play starts at half the
    // predicted drive so the acceptance table SHOWS the iterative correction
    // walking the error down (and it is what a cautious operator does against
    // an uncertain plant).
    c.law.initial_drive_backoff_db = 6.0;
    c.law.beta = 0.5;
    c.law.max_db_per_iter = 6.0;
    c.law.ref_floor_db = 50.0;
    c.law.f_lo_hz = 25.0;
    c.law.f_hi_hz = 1400.0;
    c.law.min_masked_energy_frac = 0.98;
    c.schedule.gate_rms_error_pct = 10.0;
    c.schedule.max_iterations_per_level = 10;
    c.run = RunSchedule::LoopN(2);
    c
}

#[test]
fn twr_converges_on_the_sim_plant_through_the_full_ladder() {
    let mut svc = TwrService::new(sim_player(), config()).unwrap();
    let end = svc.run_to_completion(400).unwrap();

    // ---- the acceptance evidence: the convergence table ----
    println!();
    println!("TWR Sim-validation convergence table:");
    println!("level_db  iter  rms_error_pct  peak_error_pct  spectral_err_db  drive_rms  clamped  clipped");
    for r in &svc.history().records {
        println!(
            "{:>8.1}  {:>4}  {:>13.2}  {:>14.2}  {:>15.2}  {:>9.5}  {:>7}  {:>7}",
            r.level_db,
            r.iteration,
            r.metrics.rms_error_pct,
            r.metrics.peak_error_pct,
            r.metrics.spectral_error_db,
            r.metrics.drive_rms,
            r.clamped_bins,
            r.clipped_samples
        );
    }
    let st = svc.status();
    println!(
        "sysid in-band mean coherence: {:.4}; mask energy coverage: {:.4}",
        st.sysid_mean_coherence.unwrap_or(0.0),
        st.mask_energy_coverage.unwrap_or(0.0)
    );
    println!(
        "final state: {:?}; plays: {}; events:",
        end, st.plays_completed
    );
    for e in svc.report().events {
        println!("  {e}");
    }

    // ---- assertions ----
    assert_eq!(end, TwrState::Complete, "run did not complete");
    assert!(st.abort_reason.is_none(), "aborted: {:?}", st.abort_reason);
    assert_eq!(st.plays_completed, 2, "run schedule plays");
    assert!(
        st.sysid_mean_coherence.unwrap() > 0.95,
        "sysid coherence too low"
    );

    let gate = svc.config().schedule.gate_rms_error_pct;
    // Every ladder level gated.
    for level in [-12.0, -9.0, -6.0, -3.0, 0.0] {
        let last = svc
            .history()
            .records
            .iter()
            .rfind(|r| (r.level_db - level).abs() < 1e-9 && r.iteration > 0)
            .unwrap_or_else(|| panic!("no iterations recorded at {level} dB"));
        assert!(
            last.metrics.rms_error_pct <= gate,
            "{level} dB did not gate: {:.2}%",
            last.metrics.rms_error_pct
        );
    }

    // The error at the FIRST level improves iteration over iteration (the
    // inverse-FRF relaxation actually converging, not luck): strictly
    // decreasing until the gate.
    let first_level: Vec<f64> = svc
        .history()
        .records
        .iter()
        .filter(|r| (r.level_db + 12.0).abs() < 1e-9 && r.iteration > 0)
        .map(|r| r.metrics.rms_error_pct)
        .collect();
    assert!(!first_level.is_empty());
    for w in first_level.windows(2) {
        assert!(
            w[1] < w[0] || w[1] <= gate,
            "-12 dB error not decreasing: {first_level:?}"
        );
    }

    // Full-level playback stayed inside the envelope (no alarm is not required
    // - noise can graze the alarm band - but the final error must be under the
    // gate and the drive bounded).
    let final_rec = svc.history().records.last().unwrap();
    assert!(final_rec.metrics.rms_error_pct <= gate);
    assert_eq!(final_rec.clipped_samples, 0, "drive clipped at full level");
}

#[test]
fn twr_negative_out_of_band_reference_trips_the_coverage_guard() {
    // A reference whose content lives ABOVE the correction band can never be
    // replicated: its energy sits in unmasked bins, a PERMANENT error floor.
    // The coverage guard must catch it at sysid time with a clear message
    // instead of letting the ladder "iterate" toward an impossible gate.
    let n = 4096usize;
    let s: Vec<f64> = (0..n)
        .map(|i| {
            // Hann-windowed so spectral leakage into the band is negligible.
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            w * (std::f64::consts::TAU * 3000.0 * i as f64 / FS).sin()
        })
        .collect();
    let reference = ReferenceWaveform::from_samples("out-of-band tone", FS, s).unwrap();
    let mut c = config();
    c.reference = reference;
    c.prep.highpass_hz = Some(10.0);
    let mut svc = TwrService::new(sim_player(), c).unwrap();
    // Sysid itself is fine; the coverage guard aborts the run.
    let _ = svc.advance();
    assert_eq!(svc.state(), TwrState::Aborted);
    let reason = svc.status().abort_reason.unwrap();
    assert!(
        reason.contains("target spectral energy"),
        "unexpected reason: {reason}"
    );
}
