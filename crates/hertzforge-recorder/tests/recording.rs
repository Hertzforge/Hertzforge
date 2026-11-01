//! Gate tests: bit-exact round-trip (incl. pre-trigger + t0 convention),
//! crash recovery from a provisional sidecar, position-accurate gap
//! accounting, sidecar distrust on metadata mismatch,
//! overview min/max, run-name sanitizing + same-second collision handling, and
//! the disk-budget math.

use std::path::PathBuf;

use hertzforge_channel_table::{Channel, ChannelTable};
use hertzforge_recorder::{
    recorder_capture_config, recorder_disk_budget_seconds, AcquisitionConfig, RecorderSidecar,
    RecordingReader, RecordingWriter, TriggerEvent,
};

/// A unique-per-test temp root (mirrors the hertzforge-testkit temp-dir pattern;
/// the per-test tag keeps parallel tests in one process apart).
fn temp_root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hertzforge-recorder-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A minimal channel table with `n` channels (the writer takes its frame
/// layout from the table).
fn table(n: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n {
        t.channels.push(Channel::new(i as u32 + 1));
    }
    t
}

fn acq_at(native_rate_hz: f64) -> AcquisitionConfig {
    AcquisitionConfig {
        native_rate_hz,
        ..AcquisitionConfig::default()
    }
}

/// Interleaved 2-channel test pattern: frame i = (i, 1000 + i).
fn pattern_frames(range: std::ops::Range<u64>) -> Vec<f32> {
    let mut out = Vec::with_capacity(range.clone().count() * 2);
    for i in range {
        out.push(i as f32);
        out.push(1000.0 + i as f32);
    }
    out
}

// -------------------------------------------------------------- round trip

#[test]
fn bit_exact_round_trip_with_pretrigger_and_trigger_t0() {
    let root = temp_root("roundtrip");
    let acq = acq_at(1024.0);

    // Pre-trigger window: frames 0..40; live: frames 40..300 (the trigger
    // frame is the first live frame).
    let pre = pattern_frames(0..40);
    let event = TriggerEvent {
        trigger_frame_global: 40,
        pretrigger: pre.clone(),
        pretrigger_frames: 40,
    };
    let mut writer = RecordingWriter::start(&root, "burst01", &acq, table(2), Some(event)).unwrap();

    // The PROVISIONAL recorder.json exists before any data is finalized.
    let provisional_text = std::fs::read_to_string(writer.run_dir().join("recorder.json")).unwrap();
    let provisional: RecorderSidecar = serde_json::from_str(&provisional_text).unwrap();
    assert!(!provisional.clean);
    assert_eq!(provisional.pretrigger_frames, 40);
    assert_eq!(provisional.trigger_frame_global, Some(40));
    assert_eq!(provisional.t0_convention, "trigger");

    // Live continuation in uneven blocks.
    let live = pattern_frames(40..300);
    for block in live.chunks(2 * 37) {
        writer.push_frames(block, block.len() / 2);
    }
    let run_dir = writer.run_dir().to_path_buf();
    let summary = writer.finish().unwrap();
    assert_eq!(summary.frames_written, 300);
    assert!(summary.gap_free);
    assert!(summary.gaps.is_empty());
    assert_eq!(summary.bytes, 300 * 2 * 4);
    assert_eq!(summary.run_dir, run_dir);

    let mut reader = RecordingReader::open(&run_dir).unwrap();
    let info = reader.info().clone();
    assert_eq!(info.n_channels, 2);
    assert_eq!(info.rate_hz, 1024.0);
    assert_eq!(info.frames, 300);
    assert_eq!(info.duration_s, 300.0 / 1024.0);
    assert_eq!(info.t0_convention, "trigger");
    assert_eq!(info.pretrigger_frames, 40);
    assert_eq!(info.trigger_frame_global, Some(40));
    assert!(info.clean);
    assert!(!info.recovered);
    assert!(info.metadata_mismatch.is_none());
    assert_eq!(info.status, "clean");
    assert!(info.gaps.is_empty());

    // Bit-exact: the whole file equals pre ++ live.
    let expected = pattern_frames(0..300);
    let got = reader.read_region(0, 300).unwrap();
    assert_eq!(got, expected, "round-trip must be bit-exact");

    // Region + channel reads.
    assert_eq!(
        reader.read_region(295, 100).unwrap(),
        pattern_frames(295..300)
    );
    assert_eq!(reader.read_region(300, 10).unwrap(), Vec::<f32>::new());
    let ch1: Vec<f32> = reader.read_channel_region(1, 10, 5).unwrap();
    assert_eq!(ch1, vec![1010.0, 1011.0, 1012.0, 1013.0, 1014.0]);

    // t0 at the trigger sample: file frame 40 is t=0; earlier frames negative.
    assert_eq!(reader.time_of_frame(40), 0.0);
    assert_eq!(reader.time_of_frame(0), -40.0 / 1024.0);
    assert_eq!(reader.time_of_frame(40 + 1024), 1.0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn free_run_uses_start_t0() {
    let root = temp_root("start-t0");
    let acq = acq_at(2048.0);
    let mut writer = RecordingWriter::start(&root, "freerun", &acq, table(1), None).unwrap();
    let frames: Vec<f32> = (0..100).map(|i| i as f32).collect();
    writer.push_frames(&frames, 100);
    let summary = writer.finish().unwrap();
    assert!(summary.gap_free);

    let reader = RecordingReader::open(&summary.run_dir).unwrap();
    assert_eq!(reader.info().t0_convention, "start");
    assert_eq!(reader.info().pretrigger_frames, 0);
    assert_eq!(reader.info().trigger_frame_global, None);
    assert_eq!(reader.time_of_frame(0), 0.0);
    assert_eq!(reader.time_of_frame(2048), 1.0);

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------- crash recovery

#[test]
fn crash_recovery_from_provisional_sidecar_without_capture_json() {
    // Construct the aftermath of a mid-run death by hand: telemetry.f32 plus
    // the PROVISIONAL recorder.json, NO capture.json.
    let root = temp_root("crash");
    let run_dir = root.join("deadrun");
    std::fs::create_dir_all(&run_dir).unwrap();

    // 10 frames x 3 channels, values k = frame*3 + ch, little-endian f32.
    let n_frames = 10u64;
    let mut bytes = Vec::new();
    for k in 0..(n_frames * 3) {
        bytes.extend_from_slice(&(k as f32).to_le_bytes());
    }
    std::fs::write(run_dir.join("telemetry.f32"), &bytes).unwrap();

    let provisional = RecorderSidecar {
        clean: false,
        config: acq_at(2048.0),
        pretrigger_frames: 0,
        trigger_frame_global: None,
        t0_convention: "start".to_string(),
        gaps: Vec::new(),
        n_channels: 3,
        native_rate_hz: 2048.0,
    };
    std::fs::write(
        run_dir.join("recorder.json"),
        serde_json::to_string_pretty(&provisional).unwrap(),
    )
    .unwrap();

    let mut reader = RecordingReader::open(&run_dir).unwrap();
    let info = reader.info().clone();
    assert!(info.recovered, "missing capture.json must flag recovery");
    assert!(!info.clean);
    assert_eq!(info.status, "recovered (unclean)");
    assert_eq!(info.n_channels, 3, "channels recovered from recorder.json");
    assert_eq!(info.rate_hz, 2048.0, "rate recovered from recorder.json");
    assert_eq!(info.frames, 10, "frames recovered from the file size");
    assert!(info.metadata_mismatch.is_none());

    // The data reads back fine.
    let region = reader.read_region(2, 3).unwrap();
    let expected: Vec<f32> = (6..15).map(|k| k as f32).collect();
    assert_eq!(region, expected);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn open_fails_cleanly_with_no_sidecars_at_all() {
    let root = temp_root("nosidecar");
    let run_dir = root.join("bare");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("telemetry.f32"), [0u8; 16]).unwrap();
    assert!(RecordingReader::open(&run_dir).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

// ----------------------------------------------------------- gap accounting

#[test]
fn ring_overflow_records_position_accurate_gaps() {
    let root = temp_root("gaps");
    let acq = acq_at(65_536.0);
    // Minimal ring (1 chunk) + back-to-back chunk-sized pushes force drops
    // deterministically-in-practice: for zero drops the writer thread would
    // have to win the enqueue race on every single push.
    let mut writer =
        RecordingWriter::start_with_ring_capacity(&root, "flood", &acq, table(2), None, 1).unwrap();
    let chunk_frames = writer.chunk_frames();
    assert_eq!(chunk_frames, 8192, "~100 ms at 65,536 Hz is the 8192 chunk");

    let chunk = vec![0.5f32; chunk_frames * 2];
    let mut pushes = 0u64;
    while writer.gaps().is_empty() && pushes < 500 {
        writer.push_frames(&chunk, chunk_frames);
        pushes += 1;
    }
    // A few extra pushes so accepted data surrounds the gap.
    for _ in 0..3 {
        writer.push_frames(&chunk, chunk_frames);
        pushes += 1;
    }
    let summary = writer.finish().unwrap();

    assert!(
        !summary.gaps.is_empty(),
        "a capacity-1 ring flooded {pushes}x must drop"
    );
    assert!(!summary.gap_free);
    // Position accuracy: every gap sits on a chunk boundary of the FILE frame
    // space and covers exactly one chunk.
    for gap in &summary.gaps {
        assert_eq!(gap.dropped_frames, chunk_frames as u64);
        assert_eq!(gap.at_frame % chunk_frames as u64, 0);
        assert!(gap.at_frame <= summary.frames_written);
    }
    // Accounting closes: accepted + dropped == fed.
    let dropped: u64 = summary.gaps.iter().map(|g| g.dropped_frames).sum();
    assert_eq!(
        summary.frames_written + dropped,
        pushes * chunk_frames as u64
    );

    // The reader surfaces the same gaps and the honest frame count.
    let reader = RecordingReader::open(&summary.run_dir).unwrap();
    assert_eq!(reader.info().gaps, summary.gaps);
    assert_eq!(reader.info().frames, summary.frames_written);
    assert!(reader.info().metadata_mismatch.is_none());

    let _ = std::fs::remove_dir_all(&root);
}

// -------------------------------------------------------- metadata mismatch

#[test]
fn reader_trusts_the_file_over_a_lying_sidecar() {
    let root = temp_root("mismatch");
    let acq = acq_at(1024.0);
    let mut writer = RecordingWriter::start(&root, "liar", &acq, table(2), None).unwrap();
    let frames = pattern_frames(0..200);
    writer.push_frames(&frames, 200);
    let summary = writer.finish().unwrap();
    assert_eq!(summary.frames_written, 200);

    // Tamper: capture.json claims 1200 frames (sidecars can lie;
    // the Random-crate sidecar bug is the in-repo example).
    let cap_path = summary.run_dir.join("capture.json");
    let mut cap: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cap_path).unwrap()).unwrap();
    cap["frames_written"] = serde_json::json!(1200);
    std::fs::write(&cap_path, serde_json::to_string_pretty(&cap).unwrap()).unwrap();

    let mut reader = RecordingReader::open(&summary.run_dir).unwrap();
    let info = reader.info().clone();
    assert_eq!(info.frames, 200, "the file is the authority");
    let msg = info
        .metadata_mismatch
        .expect("the mismatch must be surfaced");
    assert!(
        msg.contains("1200"),
        "mismatch text carries the claim: {msg}"
    );
    assert!(
        msg.contains("200"),
        "mismatch text carries the truth: {msg}"
    );
    // Reads honor the real file extent.
    assert_eq!(reader.read_region(0, 10_000).unwrap().len(), 200 * 2);

    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------ overview

#[test]
fn overview_min_max_on_a_known_ramp() {
    let root = temp_root("overview");
    let acq = acq_at(512.0);
    let mut writer = RecordingWriter::start(&root, "ramp", &acq, table(1), None).unwrap();
    let ramp: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    writer.push_frames(&ramp, 1000);
    let summary = writer.finish().unwrap();

    let mut reader = RecordingReader::open(&summary.run_dir).unwrap();
    let cols = reader.overview(0, 10).unwrap();
    assert_eq!(cols.len(), 10);
    for (k, (lo, hi)) in cols.iter().enumerate() {
        assert_eq!(*lo, (k * 100) as f32, "column {k} min");
        assert_eq!(*hi, (k * 100 + 99) as f32, "column {k} max");
    }
    // Degenerate requests.
    assert!(reader.overview(0, 0).unwrap().is_empty());
    assert!(reader.overview(1, 10).is_err(), "channel out of range");

    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------- run naming and collisions

#[test]
fn run_names_are_sanitized_and_collisions_get_numeric_suffixes() {
    let root = temp_root("names");
    let acq = acq_at(1024.0);

    let w1 = RecordingWriter::start(&root, "run 001/burst", &acq, table(1), None).unwrap();
    assert_eq!(w1.run_dir(), root.join("run_001_burst"));
    let s1 = w1.finish().unwrap();

    // Same requested name in the same second: suffixed, both runs intact.
    let w2 = RecordingWriter::start(&root, "run 001/burst", &acq, table(1), None).unwrap();
    assert_eq!(w2.run_dir(), root.join("run_001_burst_2"));
    let s2 = w2.finish().unwrap();
    let w3 = RecordingWriter::start(&root, "run 001/burst", &acq, table(1), None).unwrap();
    assert_eq!(w3.run_dir(), root.join("run_001_burst_3"));
    let s3 = w3.finish().unwrap();

    assert_ne!(s1.run_dir, s2.run_dir);
    assert_ne!(s2.run_dir, s3.run_dir);
    for s in [&s1, &s2, &s3] {
        assert!(s.run_dir.join("recorder.json").exists());
        assert!(s.run_dir.join("capture.json").exists());
    }

    // An all-punctuation name falls back to "run".
    let w4 = RecordingWriter::start(&root, "///???", &acq, table(1), None).unwrap();
    assert_eq!(w4.run_dir(), root.join("run"));
    w4.finish().unwrap();

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- disk budget

#[test]
fn disk_budget_wraps_the_testkit_math_at_native_rate() {
    // 8 ch x 4 B x 65,536 Hz = 2,097,152 B/s (= 2.10 MB/s in SI megabytes).
    let acq = acq_at(65_536.0);
    let cfg = recorder_capture_config(&acq, 8);
    assert_eq!(
        cfg.decimation, 1,
        "the recorder never writes decimated data"
    );
    assert_eq!(cfg.bytes_per_second() as u64, 2_097_152);
    assert_eq!(
        recorder_disk_budget_seconds(&acq, 8, 2_097_152 * 10 + 5),
        10
    );
    // One GiB holds 512 whole seconds of 8-channel native capture.
    assert_eq!(recorder_disk_budget_seconds(&acq, 8, 1_073_741_824), 512);
}
