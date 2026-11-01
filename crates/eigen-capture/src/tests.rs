//! `eigen-capture` unit tests.
//!
//! The four durability tests each state the MUTATION that turns them red - the
//! falsifiability rule: a safety test that cannot fail is not evidence.

use super::*;
use eigen_channel_table::{Channel, ChannelRole, ChannelTable};

/// A minimal snapshot payload standing in for a mode's real payload, exercising the
/// generic [`Snapshot`] / [`save_snapshot`] seam without any mode dependency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TestPayload {
    est_freq_hz: f64,
    label: String,
}

/// A unique temp directory for one test (the tests run in parallel in one process).
fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "eigen-capture-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Durability settings that flush FAST, so the timed-cadence tests do not sleep for
/// the 5 s production interval.
fn fast(flush_ms: u64) -> DurabilityConfig {
    DurabilityConfig {
        flush_interval: Duration::from_millis(flush_ms),
        // The tests run on the developer's own volume; a 1 GiB auto floor would make
        // them pass or fail on how full that disk happens to be. The floor logic
        // itself is tested directly in `the_floor_is_the_larger_of_the_estimate_and_the_minimum`
        // and `a_capture_refuses_to_start_below_the_free_space_floor`.
        free_space_floor: FreeSpaceFloor::Bytes(0),
        allow_unknown_free_space: true,
    }
}

fn table_n(n: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n {
        let mut c = Channel::new((i + 1) as u32);
        c.role = ChannelRole::Control;
        c.enabled = true;
        t.channels.push(c);
    }
    t
}

fn read_sidecar(run_dir: &Path) -> CaptureSidecar {
    let text = std::fs::read_to_string(run_dir.join(SIDECAR_FILE)).expect("sidecar readable");
    serde_json::from_str(&text).expect("sidecar parses")
}

// =====================================================================================
// Pre-existing behavior (carried over from eigen-testkit, unchanged)
// =====================================================================================

#[test]
fn disk_budget_math() {
    // at_rate(8, 5120) = 8 channels * 4 bytes * 5120 Hz = 163,840 B/s.
    let cfg = CaptureConfig::at_rate(8, 5120.0);
    assert_eq!(cfg.bytes_per_second() as u64, 163_840);
    // 1 GB free -> ~6553 s.
    let secs = disk_budget_max_seconds(&cfg, 1_073_741_824);
    assert_eq!(secs, 1_073_741_824 / 163_840);
    // Raw is much heavier per second.
    let raw = CaptureConfig::raw(8);
    assert!(raw.bytes_per_second() > cfg.bytes_per_second() * 10.0);
}

#[test]
fn decimated_config_is_internally_consistent() {
    let cfg = CaptureConfig::decimated(8);
    // 65536 / 5120 = 12.8 -> rounds to 13.
    assert_eq!(cfg.decimation, 13);
    // sample_rate_hz is DERIVED as RAW/decimation, so it never contradicts the stride
    // (the old bug: 5120 stored alongside decimation 13, though 65536/13 = 5041.23).
    assert!((cfg.sample_rate_hz - RAW_CAPTURE_RATE_HZ / 13.0).abs() < 1e-9);
    assert!((cfg.source_rate_hz() - RAW_CAPTURE_RATE_HZ).abs() < 1e-6);
}

/// Env override + per-OS default for the data root, in ONE test function: the three
/// cases mutate the same process-wide env var, and separate `#[test]`s run on parallel
/// threads, so they must be sequenced here to avoid racing.
#[test]
fn data_root_env_override_and_default() {
    // 1. EIGEN_DATA_ROOT override is honored verbatim.
    let over = if cfg!(windows) {
        r"C:\eigen-capture-env-root"
    } else {
        "/eigen-capture-env-root"
    };
    std::env::set_var(DATA_ROOT_ENV, over);
    assert_eq!(default_data_root(), PathBuf::from(over));

    // 2. Without the env var: non-empty ABSOLUTE per-OS path ending in `Eigen_Data`
    //    (on macOS: `/Users/<user>/Eigen_Data`, the same location as
    //    the retired hard-coded constant).
    std::env::remove_var(DATA_ROOT_ENV);
    let root = default_data_root();
    assert!(!root.as_os_str().is_empty());
    assert!(
        root.is_absolute(),
        "default data root not absolute: {root:?}"
    );
    assert_eq!(
        root.file_name().and_then(|n| n.to_str()),
        Some(DATA_ROOT_DIR_NAME)
    );

    // 3. An EMPTY env var is treated as unset, not as a root of "".
    std::env::set_var(DATA_ROOT_ENV, "");
    assert_eq!(default_data_root(), root);
    std::env::remove_var(DATA_ROOT_ENV);
}

#[test]
fn at_rate_records_pushed_frames_honestly() {
    // The regression the recorder session asked for: the sidecar's declared rate
    // matches the frames actually written, with no phantom decimation.
    let rate = 8192.0;
    let dir = tmp("atrate");
    let cfg = CaptureConfig::at_rate(3, rate);
    assert_eq!(cfg.decimation, 1);
    assert!((cfg.source_rate_hz() - rate).abs() < 1e-9);
    let sink =
        CaptureSink::start_durable(&dir, "run001", cfg, table_n(3), "{}".into(), fast(20)).unwrap();
    // 20 chunks x 4 frames x 3 ch.
    for _ in 0..20 {
        sink.push(&[0.5f32; 12], 4);
    }
    std::thread::sleep(Duration::from_millis(50));
    let run_dir = sink.run_dir().to_path_buf();
    let sidecar = sink.finish().unwrap();
    // Sidecar rate matches what was declared, decimation is 1 (no lie).
    assert!((sidecar.config.sample_rate_hz - rate).abs() < 1e-9);
    assert_eq!(sidecar.config.decimation, 1);
    // File size == frames_written * n_ch * 4 (the DR2 reader's sanity check).
    let bin_len = std::fs::metadata(run_dir.join(TELEMETRY_FILE))
        .unwrap()
        .len();
    assert_eq!(bin_len, sidecar.frames_written * 3 * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_writes_binary_and_sidecar_with_drop_count() {
    let dir = tmp("cap");
    let cfg = CaptureConfig {
        n_channels: 2,
        sample_rate_hz: 5120.0,
        decimation: 1,
        ring_capacity: 4,
    };
    let sink =
        CaptureSink::start_durable(&dir, "run001", cfg, table_n(2), "{}".into(), fast(20)).unwrap();
    // Push several small chunks (2 channels, 3 frames each).
    for _ in 0..10 {
        let samples = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]; // 3 frames x 2 ch
        sink.push(&samples, 3);
    }
    std::thread::sleep(Duration::from_millis(50));
    let run_dir = sink.run_dir().to_path_buf();
    let sidecar = sink.finish().unwrap();
    assert!(run_dir.join(TELEMETRY_FILE).exists());
    assert!(run_dir.join(SIDECAR_FILE).exists());
    // Every pushed chunk is accounted for as written or dropped - none vanish.
    assert_eq!(sidecar.chunks_written + sidecar.chunks_dropped, 10);
    assert!(sidecar.chunks_written >= 1, "at least one chunk hit disk");
    let bin_len = std::fs::metadata(run_dir.join(TELEMETRY_FILE))
        .unwrap()
        .len();
    assert_eq!(bin_len, sidecar.frames_written * 2 * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn full_ring_drops_without_blocking_the_reader() {
    // A capacity-1 ring flooded with chunks must DROP (not block); some chunks are
    // dropped and the reader (this thread) never hangs.
    let dir = tmp("drop");
    let cfg = CaptureConfig {
        n_channels: 1,
        sample_rate_hz: 5120.0,
        decimation: 1,
        ring_capacity: 1,
    };
    let sink =
        CaptureSink::start_durable(&dir, "flood", cfg, table_n(1), "{}".into(), fast(20)).unwrap();
    let big = vec![0.0f32; 1024];
    for _ in 0..2000 {
        sink.push(&big, 1024);
    }
    let sidecar = sink.finish().unwrap();
    // Every push is either written or dropped - the accounting is EXACT, not bounded.
    assert_eq!(sidecar.chunks_written + sidecar.chunks_dropped, 2000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshot_round_trips_through_json() {
    let dir = tmp("snap");
    std::fs::create_dir_all(&dir).unwrap();
    let snap = Snapshot {
        unix_secs: 1_700_000_000,
        settings_json: "{\"band\":[20,2000]}".into(),
        marks: vec![Mark {
            ch: 1,
            freq_hz: 250.0,
            value: 3.2,
            label: "resonance".into(),
        }],
        payload: TestPayload {
            est_freq_hz: 250.0,
            label: "payload".into(),
        },
    };
    let path = save_snapshot(&dir, &snap).unwrap();
    assert!(path.exists());
    let text = std::fs::read_to_string(&path).unwrap();
    let back: Snapshot<TestPayload> = serde_json::from_str(&text).unwrap();
    assert_eq!(back.payload.est_freq_hz, 250.0);
    assert_eq!(back.marks.len(), 1);
    assert_eq!(back.marks[0].label, "resonance");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn f32_byte_cast_is_little_endian_and_sized() {
    let samples = [1.0f32, -2.0, 3.5];
    let bytes = f32_as_le_bytes(&samples);
    assert_eq!(bytes.len(), 12);
    let got = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
    assert_eq!(got, 1.0);
}

// =====================================================================================
// Item 12.1 - the REAL free-space preflight
// =====================================================================================

/// The query is a real syscall against a real filesystem, not the 256 GiB placeholder
/// constant it replaces. Falsifiability: the temp volume genuinely has SOME space and
/// is not absurdly large, so a stubbed `Ok(0)` or a hard-coded 256 GiB both fail here.
///
/// Red-mutation: return the old `256 * 1024^3` constant from `free_space_bytes` and the
/// exabyte bound plus `disk_budget_seconds_on`'s agreement with it go wrong; return
/// `Ok(0)` and the non-zero assertion reds. On Windows (the `GetDiskFreeSpaceExW`
/// arm), returning the old `Unsupported` answer reds the first `expect`.
#[test]
#[cfg(any(unix, windows))]
fn free_space_is_a_real_syscall_on_a_real_volume() {
    let here = std::env::temp_dir();
    let free = free_space_bytes(&here).expect("a free-space query on the temp volume");
    assert!(
        free > 0,
        "a writable temp volume reports non-zero free space"
    );
    // Sanity bound: no volume this code runs on has an exabyte free. A stub returning
    // u64::MAX (the "assume infinite space" failure) fails here.
    assert!(free < u64::MAX / 2, "implausible free-space figure {free}");

    // The budget helper agrees with the raw figure it is derived from.
    let cfg = CaptureConfig::at_rate(4, 1000.0); // 16,000 B/s
    let secs = disk_budget_seconds_on(&here, &cfg, 0).unwrap();
    assert_eq!(secs, disk_budget_max_seconds(&cfg, free));

    // A nonexistent path is an ERROR, not a fabricated number.
    let missing = here.join("eigen-capture-definitely-not-here-9f2a");
    assert!(matches!(
        free_space_bytes(&missing),
        Err(FreeSpaceError::Query { .. })
    ));

    // A FILE answers for its volume on every platform: `statvfs` takes any path, and
    // the Windows arm asks about the file's directory (`GetDiskFreeSpaceExW` takes
    // only directories). Without that, a caller handing in a file would get a Query
    // error on Windows and a number on unix.
    let file = here.join(format!(
        "eigen-capture-free-space-probe-{}",
        std::process::id()
    ));
    std::fs::write(&file, b"x").unwrap();
    let via_file = free_space_bytes(&file);
    let _ = std::fs::remove_file(&file);
    let via_file = via_file.expect("a file's volume answers like its directory's");
    assert!(via_file > 0 && via_file < u64::MAX / 2, "{via_file}");
}

/// The floor is `max(multiple x estimate, MIN_FREE_FLOOR_BYTES)`.
///
/// Red-mutation: drop the `.max(MIN_FREE_FLOOR_BYTES)` and the open-ended-run case
/// (estimate 0) reds; drop the `multiple` and the long-run case reds.
#[test]
fn the_floor_is_the_larger_of_the_estimate_and_the_minimum() {
    let cfg = CaptureConfig::at_rate(8, 5120.0); // 163,840 B/s
                                                 // Short run: the estimate is tiny, so the 1 GiB minimum governs.
    let short = FreeSpaceFloor::Auto {
        estimated_seconds: 10.0,
        multiple: DEFAULT_FLOOR_MULTIPLE,
    };
    assert_eq!(short.bytes(&cfg), MIN_FREE_FLOOR_BYTES);
    // Long run: 2 x (163,840 * 60,000) = ~19.7 GB governs.
    let long = FreeSpaceFloor::Auto {
        estimated_seconds: 60_000.0,
        multiple: DEFAULT_FLOOR_MULTIPLE,
    };
    let expected = (cfg.bytes_per_second() * 60_000.0 * 2.0) as u64;
    assert_eq!(long.bytes(&cfg), expected);
    assert!(expected > MIN_FREE_FLOOR_BYTES);
    // An open-ended run has no estimate to double, and gets the minimum - not zero.
    assert_eq!(FreeSpaceFloor::default().bytes(&cfg), MIN_FREE_FLOOR_BYTES);
    // An explicit floor is used verbatim.
    assert_eq!(FreeSpaceFloor::Bytes(4242).bytes(&cfg), 4242);
}

/// A capture whose floor cannot be met REFUSES TO START, and leaves nothing behind -
/// no run directory, no zero-length binary that a later scan would list as a capture.
///
/// Red-mutation: make `start_durable` log-and-continue instead of returning
/// `NotEnoughSpace` (or drop the `preflight_free_space` call entirely) and this reds on
/// both the error and the no-debris assertion.
#[test]
#[cfg(any(unix, windows))]
fn a_capture_refuses_to_start_below_the_free_space_floor() {
    let dir = tmp("nospace");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = CaptureConfig::at_rate(2, 1000.0);
    // A floor no real volume can satisfy.
    let durability = DurabilityConfig {
        flush_interval: Duration::from_millis(20),
        free_space_floor: FreeSpaceFloor::Bytes(u64::MAX - 1),
        allow_unknown_free_space: false,
    };
    let started = CaptureSink::start_durable(&dir, "run", cfg, table_n(2), "{}".into(), durability);
    let err = match started {
        Err(e) => e,
        Ok(_) => panic!("an unmeetable floor must refuse the start"),
    };
    match err {
        CaptureStartError::NotEnoughSpace {
            free_bytes,
            floor_bytes,
        } => {
            assert!(free_bytes < floor_bytes);
            // The message names both numbers so the operator can act on it.
            let msg = CaptureStartError::NotEnoughSpace {
                free_bytes,
                floor_bytes,
            }
            .to_string();
            assert!(msg.contains(&free_bytes.to_string()), "{msg}");
            assert!(msg.contains(&floor_bytes.to_string()), "{msg}");
        }
        other => panic!("wrong refusal: {other}"),
    }
    // Refused BEFORE any artifact was created.
    assert!(
        !dir.join("run").exists(),
        "a refused capture must leave no run directory behind"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An UNKNOWN free-space answer refuses by default and proceeds only when explicitly
/// allowed - the platform gap is named, never silently treated as "plenty of space".
///
/// Red-mutation: make `preflight_free_space` map `Err(..)` to `Ok(None)`
/// unconditionally and the refusal case reds.
#[test]
fn unknown_free_space_refuses_unless_explicitly_allowed() {
    let cfg = CaptureConfig::at_rate(2, 1000.0);
    // Simulate the no-implementation answer directly - the decision under test is the
    // policy, not the syscall.
    let unknown = FreeSpaceError::Unsupported {
        platform: "an unsupported target".into(),
    };
    assert!(unknown.to_string().contains("refuses"), "{unknown}");

    let strict = DurabilityConfig {
        allow_unknown_free_space: false,
        ..Default::default()
    };
    let lenient = DurabilityConfig {
        allow_unknown_free_space: true,
        ..Default::default()
    };
    // A path that cannot be queried on ANY platform stands in for the unknown answer.
    let missing = std::env::temp_dir().join("eigen-capture-missing-volume-5c1d/deeper");
    // On unix and Windows this is a `Query` error, on other platforms `Unsupported`;
    // either way the POLICY is what is asserted.
    assert!(free_space_bytes(&missing).is_err());
    assert!(preflight_free_space(&missing, &cfg, &strict).is_err());
    assert!(matches!(
        preflight_free_space(&missing, &cfg, &lenient),
        Ok(None)
    ));
}

// =====================================================================================
// Item 12.2 - timed flush + sync_data
// =====================================================================================

/// Data reaches the FILE on the flush cadence, without waiting for `finish()`.
///
/// Before the durability work the 8 KiB `BufWriter` was flushed only at `finish()`, so a crash lost
/// everything the buffer still held. This pushes less than one buffer's worth, waits
/// past two flush intervals, and asserts the bytes are on disk and the flush counter
/// advanced - with the sink still very much alive.
///
/// Red-mutation: remove the `flush_tick` call from the writer loop (leaving the flush
/// only on exit) and this reds - the file stays 0 bytes and `flushes()` stays 0.
#[test]
fn a_running_capture_flushes_and_syncs_on_the_cadence() {
    let dir = tmp("flush");
    let cfg = CaptureConfig::at_rate(2, 1000.0);
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(2), "{}".into(), fast(30)).unwrap();
    // 40 bytes total - two orders of magnitude under the BufWriter's 8 KiB capacity,
    // so nothing reaches the file incidentally.
    for _ in 0..5 {
        sink.push(&[1.0f32, 2.0], 1);
    }
    let bin = sink.run_dir().join(TELEMETRY_FILE);
    let mut on_disk = 0u64;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(20));
        on_disk = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
        if on_disk > 0 && sink.flushes() > 0 {
            break;
        }
    }
    assert_eq!(on_disk, 5 * 2 * 4, "5 frames x 2 ch x 4 B reached the file");
    assert!(
        sink.flushes() > 0,
        "at least one flush + sync_data completed"
    );
    assert!(sink.fault().is_none(), "healthy capture reports no fault");
    let _ = sink.finish();
    let _ = std::fs::remove_dir_all(&dir);
}

// =====================================================================================
// Item 12.3 - the PROVISIONAL sidecar (the kill -9 negative control)
// =====================================================================================

/// The sidecar exists and is INTERPRETABLE from the moment the capture starts, and
/// stays `complete: false` until a clean `finish()`.
///
/// This is the unit for the kill-9 negative control: dropping the sink without
/// `finish()` is the in-process stand-in for the process dying, and the assertion is
/// that what remains on disk is readable - channel count, rate, and layout all present.
///
/// Red-mutation: move the `base.write(&shared, false)` back out of `start_durable` (so
/// the sidecar is written only by `finish()`) and BOTH halves red - the at-start read
/// fails outright, and the post-drop read finds no file.
#[test]
fn a_killed_capture_leaves_a_readable_provisional_sidecar() {
    let dir = tmp("provisional");
    let cfg = CaptureConfig::at_rate(4, 2048.0);
    let sink = CaptureSink::start_durable(
        &dir,
        "run",
        cfg.clone(),
        table_n(4),
        "{\"mode\":\"test\"}".into(),
        fast(20),
    )
    .unwrap();
    let run_dir = sink.run_dir().to_path_buf();

    // 1. Readable BEFORE a single sample is pushed.
    let at_start = read_sidecar(&run_dir);
    assert!(!at_start.complete, "a live capture is not complete");
    assert_eq!(at_start.config.n_channels, 4);
    assert!((at_start.config.sample_rate_hz - 2048.0).abs() < 1e-9);
    assert_eq!(at_start.run_config_json, "{\"mode\":\"test\"}");
    assert_eq!(
        at_start.layout.interleave,
        Interleave::FrameInterleaved,
        "a reader must not have to infer the interleave"
    );

    // 2. Push, let a flush tick land, then DROP the sink without finish() - the
    //    in-process stand-in for `kill -9`.
    for _ in 0..8 {
        sink.push(&[0.25f32; 32], 8); // 8 frames x 4 ch
    }
    std::thread::sleep(Duration::from_millis(120));
    drop(sink);

    let after_kill = read_sidecar(&run_dir);
    assert!(
        !after_kill.complete,
        "a capture that was never finished must NOT claim to be complete"
    );
    assert!(
        after_kill.frames_written > 0,
        "the provisional sidecar carries live counters, not zeros"
    );
    assert_eq!(after_kill.config.n_channels, 4);
    // The binary is interpretable with only the sidecar in hand.
    let bin_len = std::fs::metadata(run_dir.join(TELEMETRY_FILE))
        .unwrap()
        .len();
    assert_eq!(bin_len, after_kill.frames_written * 4 * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `finish()` is what flips `complete` - and nothing else does.
#[test]
fn only_a_clean_finish_marks_the_sidecar_complete() {
    let dir = tmp("complete");
    let cfg = CaptureConfig::at_rate(2, 1000.0);
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(2), "{}".into(), fast(20)).unwrap();
    let run_dir = sink.run_dir().to_path_buf();
    sink.push(&[1.0f32, 2.0], 1);
    assert!(!read_sidecar(&run_dir).complete);
    let final_sidecar = sink.finish().unwrap();
    assert!(final_sidecar.complete);
    assert!(
        read_sidecar(&run_dir).complete,
        "the FILE says complete too"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sidecar carries the layout descriptor (scale-8): interleave, block size when
/// constant, and the channel order with its PROVENANCE.
///
/// Red-mutation: drop `layout` from `CaptureSidecar`, or make `from_table` guess an
/// order when the counts disagree, and this reds.
#[test]
fn the_sidecar_carries_the_layout_descriptor() {
    // (a) Matching table -> derived channel order, constant block size observed.
    let dir = tmp("layout");
    let cfg = CaptureConfig::at_rate(3, 1000.0);
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(3), "{}".into(), fast(20)).unwrap();
    for _ in 0..4 {
        sink.push(&[0.0f32; 12], 4); // always 4 frames
    }
    std::thread::sleep(Duration::from_millis(60));
    let s = sink.finish().unwrap();
    assert_eq!(s.layout.interleave, Interleave::FrameInterleaved);
    assert_eq!(s.layout.channel_order, vec![1, 2, 3]);
    assert_eq!(
        s.layout.channel_order_source,
        ChannelOrderSource::ChannelTableEnabledInputs
    );
    assert_eq!(
        s.layout.block_frames,
        Some(4),
        "a constant block size is stated"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // (b) Variable chunk sizes -> block_frames is None rather than a lie.
    let dir = tmp("layout-var");
    let cfg = CaptureConfig::at_rate(2, 1000.0);
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(2), "{}".into(), fast(20)).unwrap();
    sink.push(&[0.0f32; 4], 2);
    sink.push(&[0.0f32; 6], 3);
    std::thread::sleep(Duration::from_millis(60));
    let s = sink.finish().unwrap();
    assert_eq!(
        s.layout.block_frames, None,
        "variable chunks claim no block size"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // (c) Table that does NOT match n_channels -> order UNKNOWN, never guessed.
    let dir = tmp("layout-mismatch");
    let cfg = CaptureConfig::at_rate(5, 1000.0);
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(2), "{}".into(), fast(20)).unwrap();
    let s = sink.finish().unwrap();
    assert_eq!(s.layout.channel_order_source, ChannelOrderSource::Unknown);
    assert!(
        s.layout.channel_order.is_empty(),
        "a mismatched table yields NO order, not a plausible one"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A pre-item-12 sidecar (no `complete`, no `layout`) still deserializes, and reports
/// the honest "not complete / layout unknown" defaults rather than failing to parse.
#[test]
fn a_legacy_sidecar_still_parses_with_honest_defaults() {
    let legacy = r#"{
        "config": {"n_channels": 2, "sample_rate_hz": 5120.0, "decimation": 1, "ring_capacity": 256},
        "channel_table": {"channels": []},
        "start_unix_secs": 1700000000,
        "run_config_json": "{}",
        "chunks_written": 7,
        "chunks_dropped": 0,
        "frames_written": 21
    }"#;
    let s: CaptureSidecar = serde_json::from_str(legacy).expect("legacy sidecar parses");
    assert_eq!(s.chunks_written, 7);
    assert!(!s.complete, "an old sidecar makes no completeness claim");
    assert_eq!(s.layout.channel_order_source, ChannelOrderSource::Unknown);
    assert_eq!(s.flushes, 0);
    assert!(s.fault.is_none());
}

// =====================================================================================
// Item 12.4 - gap accounting on EVERY path
// =====================================================================================

/// A push after the writer is gone COUNTS as a drop and faults the capture. This is the
/// arm that used to return `false` silently.
///
/// Red-mutation: restore `Err(TrySendError::Disconnected(_)) => false` (no counter
/// bump) in `CaptureSink::push` and this reds - the drop count reads 0 while chunks
/// were demonstrably lost.
#[test]
fn a_disconnected_writer_still_counts_every_dropped_chunk() {
    let dir = tmp("disconnected");
    let cfg = CaptureConfig::at_rate(1, 1000.0);
    let mut sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(1), "{}".into(), fast(20)).unwrap();
    sink.push(&[1.0f32], 1);
    // Simulate the writer being gone: drop the sender + join, exactly as the sink's own
    // teardown does, then keep pushing.
    sink.tx = None;
    if let Some(j) = sink.join.take() {
        let _ = j.join();
    }
    for _ in 0..25 {
        assert!(
            !sink.push(&[2.0f32], 1),
            "a dead writer cannot accept a chunk"
        );
    }
    assert_eq!(sink.dropped(), 25, "every post-mortem push is counted");
    assert!(
        sink.fault().is_some(),
        "the capture reports WHY the chunks were dropped, live"
    );
    // The reason reaches disk through the sidecar the writer already wrote... and
    // through `status()` for a live UI readout.
    assert!(sink.status().fault.is_some());
    assert_eq!(sink.status().chunks_dropped, 25);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A faulted writer keeps DRAINING and COUNTING instead of exiting, so the ring never
/// wedges and the totals stay complete; the fault text reaches the sidecar.
///
/// The fault is induced through the same path a real one takes ([`CaptureShared::fault`]
/// is what an `io::Error` calls), so this exercises the production arm rather than a
/// test-only branch.
///
/// Red-mutation: make the writer loop `return` on `Err(e)` from `write_all` (the
/// pre-item-12 `?`) and this reds - the post-fault chunks vanish from the accounting
/// and the sidecar's `fault` is null.
#[test]
fn a_faulted_writer_keeps_counting_and_records_the_reason() {
    let dir = tmp("faulted");
    let cfg = CaptureConfig {
        n_channels: 1,
        sample_rate_hz: 1000.0,
        decimation: 1,
        ring_capacity: 64,
    };
    let sink =
        CaptureSink::start_durable(&dir, "run", cfg, table_n(1), "{}".into(), fast(20)).unwrap();
    sink.push(&[1.0f32], 1);
    std::thread::sleep(Duration::from_millis(60));
    let written_before = sink.written();
    assert!(written_before >= 1);

    sink.shared.fault("induced: disk went away");
    for _ in 0..30 {
        // The producer is NOT blocked by the fault - pushes are still accepted by the
        // ring and consumed by the still-running writer.
        sink.push(&[2.0f32], 1);
    }
    // Give the writer time to drain the post-fault chunks.
    for _ in 0..100 {
        if sink.dropped() >= 30 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        sink.dropped(),
        30,
        "post-fault chunks are counted, not lost"
    );
    assert_eq!(
        sink.written(),
        written_before,
        "nothing is written after the fault"
    );

    let run_dir = sink.run_dir().to_path_buf();
    let final_sidecar = sink.finish().unwrap();
    assert_eq!(final_sidecar.chunks_dropped, 30);
    let fault = final_sidecar.fault.expect("the sidecar names the fault");
    assert!(fault.contains("induced"), "{fault}");
    assert!(
        read_sidecar(&run_dir).fault.is_some(),
        "the fault is on DISK, not only in memory"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first fault wins: a cascade of consequential errors must not overwrite the one
/// that explains what happened.
#[test]
fn the_first_fault_is_the_one_reported() {
    let shared = CaptureShared {
        chunks_written: AtomicU64::new(0),
        chunks_dropped: AtomicU64::new(0),
        frames_written: AtomicU64::new(0),
        flushes: AtomicU64::new(0),
        faulted: AtomicBool::new(false),
        fault_text: Mutex::new(None),
        block_frames: Mutex::new(None),
        block_frames_seen: AtomicBool::new(false),
    };
    shared.fault("disk full");
    shared.fault("write failed (consequence)");
    assert_eq!(shared.fault_text().as_deref(), Some("disk full"));
}
