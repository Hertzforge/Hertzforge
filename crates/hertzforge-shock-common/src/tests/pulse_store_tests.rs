//! Per-pulse store tests: ingest computes kinematics + SRS via
//! hertzforge-dsp, and the snapshot payload wires hertzforge-testkit's Snapshot<P>.

use crate::pulse_store::{build_snapshot, capture_pulse, PulseStore, ShockSnapshotPayload};
use hertzforge_capture::{save_snapshot, CaptureConfig, CaptureSink};
use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::pulse;
use hertzforge_dsp::srs::{octave_freqs, OctaveFraction};

/// A half-sine pulse padded with a quiet tail so the SRS residual can develop and the
/// kinematics see the full net change (matching the hertzforge-dsp golden-test convention).
fn padded_half_sine(amp_g: f64, dur_ms: f64, fs: f64, tail: usize) -> Vec<f64> {
    let mut p = pulse::half_sine(amp_g, dur_ms, fs);
    p.extend(std::iter::repeat_n(0.0, tail));
    p
}

#[test]
fn ingest_computes_kinematics_and_srs_and_assigns_sequential_ids() {
    let fs = 5120.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    let p0 = padded_half_sine(20.0, 11.0, fs, 512);
    let seq0 = store.ingest(p0, fs, freqs.clone(), -6.0, false, 1_700_000_000);
    let p1 = padded_half_sine(30.0, 6.0, fs, 512);
    let seq1 = store.ingest(p1, fs, freqs.clone(), 0.0, true, 1_700_000_010);

    assert_eq!(seq0, 0);
    assert_eq!(seq1, 1);
    assert_eq!(store.len(), 2);

    let r0 = store.get(0).unwrap();
    // Peak accel of a 20 g half-sine is ~20 g.
    assert!(
        (r0.peak_accel_g() - 20.0).abs() < 0.5,
        "peak ~20 g, got {}",
        r0.peak_accel_g()
    );
    assert_eq!(r0.srs.len(), freqs.len());
    assert_eq!(r0.srs_freqs, freqs);
    assert!(r0.pvss.is_none(), "PVSS not requested for pulse 0");

    let r1 = store.get(1).unwrap();
    assert!(r1.pvss.is_some(), "PVSS requested for pulse 1");
    assert_eq!(r1.pvss.as_ref().unwrap().len(), freqs.len());
    assert_eq!(r1.level_db, 0.0);
}

#[test]
fn ingested_half_sine_residual_velocity_matches_analytic() {
    // A half-sine leaves residual velocity ~ G_TO_IN_S2 * A * 2T/pi (hertzforge-dsp golden).
    let fs = 5120.0;
    let amp = 20.0;
    let dur_ms = 11.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    store.ingest(
        padded_half_sine(amp, dur_ms, fs, 512),
        fs,
        freqs,
        -6.0,
        false,
        0,
    );
    let r = store.last().unwrap();
    let g_to_in_s2 = 386.0886;
    let t = dur_ms / 1000.0;
    let expected = g_to_in_s2 * amp * 2.0 * t / std::f64::consts::PI;
    let got = r.kinematics.residual_velocity_ips;
    // The discrete half-sine spans N-1 intervals (~10.74 ms effective for a 56-sample
    // pulse at 5120 Hz), a hair short of the nominal 11 ms, so the analytic 2AT/pi is
    // ~2.4% high; a 3% band covers the discretization gap.
    assert!(
        (got - expected).abs() / expected < 0.03,
        "residual velocity {got} vs analytic {expected}"
    );
}

#[test]
fn overlay_selection_returns_requested_pulses_in_order() {
    let fs = 5120.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    for i in 0..4 {
        store.ingest(
            padded_half_sine(10.0 + i as f64, 11.0, fs, 256),
            fs,
            freqs.clone(),
            0.0,
            false,
            i as u64,
        );
    }
    // Select 3, 0, 2 in that order; unknown seq 99 is skipped.
    let sel = store.select(&[3, 0, 2, 99]);
    let seqs: Vec<u64> = sel.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![3, 0, 2]);
}

#[test]
fn snapshot_payload_carries_pulses_and_context_and_round_trips() {
    let fs = 5120.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    store.ingest(
        padded_half_sine(20.0, 11.0, fs, 256),
        fs,
        freqs.clone(),
        -3.0,
        true,
        42,
    );
    let target = vec![15.0; freqs.len()];
    let payload = store.snapshot_payload(-3.0, 10.0, Some(target.clone()));
    assert_eq!(payload.pulses.len(), 1);
    assert_eq!(payload.current_level_db, -3.0);
    assert_eq!(payload.srs_q, 10.0);
    assert_eq!(payload.target_srs, Some(target));

    // JSON round-trip of the payload. serde_json's f64 parse can land an ULP off the
    // serialized value in this build, so we assert SEMANTIC round-trip (structure +
    // approximate f64 equality) rather than bit-exact struct equality.
    let js = serde_json::to_string(&payload).unwrap();
    let back: ShockSnapshotPayload = serde_json::from_str(&js).unwrap();
    assert_eq!(back.pulses.len(), payload.pulses.len());
    assert_eq!(back.current_level_db, payload.current_level_db);
    assert_eq!(back.srs_q, payload.srs_q);
    assert_eq!(back.target_srs, payload.target_srs);
    let (bp, pp) = (&back.pulses[0], &payload.pulses[0]);
    assert_eq!(bp.seq, pp.seq);
    assert_eq!(bp.srs.len(), pp.srs.len());
    for (a, b) in bp.srs.iter().zip(pp.srs.iter()) {
        assert!(
            (a - b).abs() <= 1e-9 * b.abs().max(1.0),
            "srs round-trip {a} vs {b}"
        );
    }
    assert!(
        (bp.kinematics.residual_velocity_ips - pp.kinematics.residual_velocity_ips).abs() < 1e-9
    );
}

#[test]
fn snapshot_saves_through_testkit_snapshot_seam() {
    let fs = 5120.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    store.ingest(
        padded_half_sine(20.0, 11.0, fs, 256),
        fs,
        freqs,
        -6.0,
        false,
        7,
    );
    let payload = store.snapshot_payload(-6.0, 10.0, None);
    let snap = build_snapshot(
        1_700_000_000,
        "{\"mode\":\"shock\"}".into(),
        vec![],
        payload,
    );

    let tmp = std::env::temp_dir().join(format!("hertzforge-shock-snap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let path = save_snapshot(&tmp, &snap).unwrap();
    assert!(path.exists());
    // Read the whole bundle back (generic Snapshot<ShockSnapshotPayload>).
    let text = std::fs::read_to_string(&path).unwrap();
    let back: hertzforge_capture::Snapshot<ShockSnapshotPayload> =
        serde_json::from_str(&text).unwrap();
    assert_eq!(back.payload.pulses.len(), 1);
    assert_eq!(back.payload.pulses[0].seq, 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn capture_pulse_streams_time_history_to_the_sink() {
    let fs = 5120.0;
    let freqs = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    let mut store = PulseStore::new();
    store.ingest(
        padded_half_sine(20.0, 11.0, fs, 64),
        fs,
        freqs,
        -6.0,
        false,
        0,
    );
    let record = store.last().unwrap();

    let tmp = std::env::temp_dir().join(format!("hertzforge-shock-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg = CaptureConfig {
        n_channels: 1,
        sample_rate_hz: fs,
        decimation: 1,
        ring_capacity: 8,
    };
    // Fixture table (deprecated outside tests).
    #[allow(deprecated)]
    let sink = CaptureSink::start(
        &tmp,
        "pulse000",
        cfg,
        ChannelTable::nonuniform_default_8ch(),
        "{}".into(),
    )
    .unwrap();
    let queued = capture_pulse(&sink, record, 1);
    assert!(queued, "the pulse chunk should queue on a fresh ring");
    std::thread::sleep(std::time::Duration::from_millis(50));
    let run_dir = sink.run_dir().to_path_buf();
    let sidecar = sink.finish().unwrap();
    assert!(run_dir.join("telemetry.f32").exists());
    // The written binary size equals the pulse length * 4 bytes (1 channel f32).
    let bin_len = std::fs::metadata(run_dir.join("telemetry.f32"))
        .unwrap()
        .len();
    assert_eq!(bin_len, sidecar.frames_written * 4);
    let _ = std::fs::remove_dir_all(&tmp);
}
