//! The guards that make the core a capture-only recorder, each added red before the
//! change that holds it (plan 2.3): no output opened and no drive sample written, the
//! station's programmed rate in everything derived, one engine and bounded batches,
//! nothing on the reading thread touching the disk, a run that cannot write ending and
//! saying so, a disk that falls behind ending the run, exact timed lengths, station
//! frames in every recording, and limits as marks.

mod support;

use hertzforge_recorder::service::{RecState, RecorderConfig, RecorderService};
use hertzforge_recorder::trigger::TriggerMode;
use hertzforge_recorder::{AcquisitionConfig, RecordingReader};
use support::{inputs, temp_root, Calls, ScriptedStation};

fn acq(fs: f64, block: usize) -> AcquisitionConfig {
    AcquisitionConfig {
        native_rate_hz: fs,
        block_size: block,
        analysis_decimation: 1,
        ..Default::default()
    }
}

fn manual(fs: f64) -> RecorderConfig {
    RecorderConfig {
        acq: acq(fs, 256),
        trigger_mode: TriggerMode::Manual,
        overload_fraction: 1e12,
        ..Default::default()
    }
}

/// A frame's value on column `c` at station frame `n`, exact in f32.
fn ramp(n: u64, c: usize) -> f32 {
    ((n % 4096) as f32) / 4096.0 + c as f32
}

// ---------------------------------------------------------------------------------
// G2: capture only. No output is opened and no drive sample is written.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_records_on_a_station_with_no_outputs() {
    // An input-only station: no output channel, and `write_drive` answers Unsupported,
    // as the NI backend's input-only task does.
    let mut station = ScriptedStation::new(2, 8192.0, Box::new(ramp));
    station.drive_unsupported = true;
    let root = temp_root("g2_no_outputs");
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor()
        .expect("a capture-only run needs no output");
    svc.record_now().unwrap();
    for _ in 0..8 {
        svc.pump_frames(1024)
            .expect("no drive is written, so nothing refuses one");
    }
    assert_eq!(svc.state(), RecState::Recording);
    svc.stop_monitor().unwrap();
    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1);
    let mut r = RecordingReader::open(&root.join(&list[0].name)).unwrap();
    assert_eq!(r.info().frames, 8192);
    let first = r.read_region(0, 1).unwrap();
    assert_eq!(first, vec![ramp(0, 0), ramp(0, 1)]);

    // The same on the simulator with no drive row in its table (the simulator keeps a
    // drive slot of its own; nothing writes to it).
    let sim = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(8192.0)
        .max_input_channels(2)
        .build();
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        inputs(2),
        Box::new(sim) as Box<dyn hertzforge_hal::Backend>,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("g2_sim_no_outputs"),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor()
        .expect("the simulator with no drive row");
    svc.record_now().unwrap();
    for _ in 0..4 {
        svc.pump_frames(1024).expect("the simulator records");
    }
    svc.stop_monitor().unwrap();
    assert_eq!(svc.recordings_list(None)[0].frames, 4096);
}

#[test]
fn recorder_writes_no_drive_sample() {
    // A station that would take a drive, so nothing faults and the count is the
    // only witness.
    let station = ScriptedStation::new(2, 8192.0, Box::new(ramp));
    let calls = station.calls();
    let mut table = inputs(2);
    // The operator's table keeps its drive row: the Recorder ignores it.
    let mut drive = hertzforge_channel_table::Channel::new(3);
    drive.role = hertzforge_channel_table::ChannelRole::Drive;
    drive.enabled = true;
    table.channels.push(drive);
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        table,
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g2_no_drive"),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    for _ in 0..8 {
        svc.pump_frames(1024).unwrap();
    }
    svc.stop_monitor().unwrap();
    assert_eq!(
        Calls::get(&calls.write_drives),
        0,
        "a Recorder run writes no drive sample, not even zeros"
    );
    assert_eq!(
        Calls::get(&calls.drive_rows),
        0,
        "the station is configured with no drive row: the table's drive row is not opened"
    );
}

// ---------------------------------------------------------------------------------
// G1: the rate. The station is asked the rate configured, and everything derived and
// written follows the rate it programs.
// ---------------------------------------------------------------------------------

fn json_at(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn recorder_records_at_the_rate_the_station_programmed() {
    // A delta-sigma device whose own rate is 51 200 Hz: it programs a rate on its
    // ladder, so 12 800 Hz exactly and 8192 Hz as 8533.33 Hz.
    let device = |asked: f64| {
        let mut st = ScriptedStation::new(2, 51_200.0, Box::new(ramp));
        st.program = support::delta_sigma_ladder;
        let calls = st.calls();
        let mut cfg = manual(asked);
        cfg.frf_pair = Some((0, 1));
        cfg.pretrigger_s = Some(0.5);
        cfg.timed_record_s = Some(0.25);
        let root = temp_root("g1_rate");
        let mut svc = RecorderService::with_backend(
            cfg,
            inputs(2),
            st,
            hertzforge_module_abi::StationClass::NiSimulated,
            root.clone(),
        )
        .unwrap()
        .with_block_frames(256);
        svc.configure().unwrap();
        svc.start_monitor().unwrap();
        (svc, calls, root)
    };

    let (mut svc, calls, _) = device(12_800.0);
    assert_eq!(
        calls.asked_rate(),
        12_800.0,
        "the station is asked the rate configured"
    );
    assert_eq!(svc.status(u64::MAX).fs_hz, 12_800.0);
    svc.stop_monitor().unwrap();

    let (mut svc, calls, root) = device(8192.0);
    let fs = 13_107_200.0 / 256.0 / 6.0;
    assert_eq!(calls.asked_rate(), 8192.0);
    let s = svc.status(u64::MAX);
    assert_eq!(
        (s.requested_fs_hz, s.fs_hz),
        (8192.0, fs),
        "both rates are stated"
    );
    assert_eq!(
        svc.timed_record_frames(),
        Some((0.25 * fs).round() as u64),
        "a timed length is counted at the programmed rate"
    );
    svc.arm().unwrap();
    assert_eq!(
        svc.status(u64::MAX).pre_frames_asked,
        (0.5 * fs).round() as u64,
        "the pre-window is counted at the programmed rate"
    );
    let mut n = 0;
    for _ in 0..16 {
        n += svc.pump_frames(1024).unwrap();
    }
    assert!((svc.session_seconds() - n as f64 / fs).abs() < 1e-9);
    let df = svc.frf().unwrap().df_hz;
    assert!((df - fs / 256.0).abs() < 1e-9, "the analysis grid: df {df}");

    svc.record_now().unwrap();
    for _ in 0..4 {
        svc.pump_frames(1024).unwrap();
    }
    svc.stop_monitor().unwrap();
    let name = svc.recordings_list(None)[0].name.clone();
    let dir = root.join(name);
    let rec = json_at(&dir.join("stream.json"));
    assert_eq!(rec["fs_hz"], serde_json::json!(fs), "stream.json: {rec}");
    assert_eq!(
        rec["requested_fs_hz"],
        serde_json::json!(8192.0),
        "stream.json: {rec}"
    );
    let cap = json_at(&dir.join("capture.json"));
    assert_eq!(cap["config"]["sample_rate_hz"], serde_json::json!(fs));
    assert_eq!(RecordingReader::open(&dir).unwrap().info().rate_hz, fs);
}

// ---------------------------------------------------------------------------------
// G9: one engine for the run, batches of two blocks, and any stop reason ends the run
// once, in its own words.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_a_batch_holds_the_service_for_two_blocks() {
    let station = ScriptedStation::new(2, 12_800.0, Box::new(ramp));
    let calls = station.calls();
    let mut svc = RecorderService::with_backend(
        manual(12_800.0),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g9_batch"),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..20 {
        let frames = svc.pump().unwrap();
        assert_eq!(
            frames, 512,
            "one pump is one batch of two 256-frame blocks: a verb waits at most that long"
        );
    }
    assert_eq!(Calls::get(&calls.opens), 1);
    assert_eq!(
        (Calls::get(&calls.arms), Calls::get(&calls.starts)),
        (1, 1),
        "the station is armed and started once for the run, never per batch"
    );
    assert_eq!(svc.status(u64::MAX).station_frames, 20 * 512);
}

#[test]
fn recorder_a_station_error_ends_the_run_once_with_its_words() {
    let count = |svc: &RecorderService<ScriptedStation>, words: &str| {
        svc.report_data()
            .event_log
            .events()
            .iter()
            .filter(|e| e.text.contains(words))
            .count()
    };

    // A station error mid-run: the run ends once, saying what the station said, and
    // the recording in progress is finished with every frame read.
    let mut station = ScriptedStation::new(2, 8192.0, Box::new(ramp));
    station.fail_after = Some((3000, "input buffer overflow (-200279)".into()));
    let calls = station.calls();
    let root = temp_root("g9_error");
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    let mut ended = false;
    for _ in 0..40 {
        if svc.pump_frames(512).is_err() {
            ended = true;
            break;
        }
    }
    assert!(ended, "the station's error ends the run");
    assert_eq!(svc.state(), RecState::Faulted);
    for _ in 0..3 {
        assert!(svc.pump().is_err(), "an ended run reads nothing more");
    }
    assert_eq!(count(&svc, "input buffer overflow (-200279)"), 1);
    assert_eq!(Calls::get(&calls.arms), 1, "nothing re-arms the station");
    svc.stop_monitor().unwrap();
    let list = svc.recordings_list(None);
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0].frames, 3000,
        "every frame read before the error is saved"
    );

    // A station whose input stream ends: a continuous acquisition that stops
    // delivering has failed, and the run ends saying so, once.
    let station = ScriptedStation::new(2, 8192.0, Box::new(ramp)).frames(3000);
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g9_ended"),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    for _ in 0..40 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(512);
    }
    assert_eq!(
        svc.state(),
        RecState::Faulted,
        "a station that stops delivering ends the run; it does not idle on as if monitoring"
    );
    assert_eq!(count(&svc, "the station ended its input stream"), 1);
    svc.stop_monitor().unwrap();
    assert_eq!(svc.recordings_list(None)[0].frames, 3000);
}

// ---------------------------------------------------------------------------------
// G3: the reading thread never touches the disk. Every recording call runs on the
// record thread; the pump hands frames over and never waits.
// ---------------------------------------------------------------------------------

/// A burst of 1.0 lasting 32 frames every `every` frames from `first`, on column 0.
fn bursts_every(first: u64, every: u64) -> support::Signal {
    Box::new(move |n, c| {
        if c == 0 && n >= first && (n - first) % every < 32 {
            1.0
        } else {
            0.0
        }
    })
}

fn level_on_column_0(holdoff_s: f64) -> TriggerMode {
    TriggerMode::Level(hertzforge_recorder::LevelTrigger {
        channel_index: 0,
        threshold_eu: 0.5,
        slope: hertzforge_recorder::Slope::Rising,
        hysteresis_eu: 0.1,
        holdoff_s,
    })
}

#[test]
fn recorder_the_reading_thread_never_touches_the_disk() {
    // A store that takes 2 s to start and to finish each recording: the station's
    // whole input buffer on NI. Five level-triggered recordings, 4.2 s apart, read in
    // real time.
    let fs = 2048.0;
    let station = ScriptedStation::new(2, fs, bursts_every(1024, (4.2 * fs) as u64)).paced();
    let (mut store, log) = support::StoreDouble::new(2);
    store.start_delay = std::time::Duration::from_secs(2);
    store.finish_delay = std::time::Duration::from_secs(2);
    let mut cfg = manual(fs);
    cfg.trigger_mode = level_on_column_0(0.0);
    cfg.pretrigger_s = Some(0.25);
    cfg.timed_record_s = Some(0.5);
    cfg.auto_rearm = 4;
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g3_thread"),
    )
    .unwrap()
    .with_block_frames(64)
    .with_store(Box::new(store));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    let pump_thread = std::thread::current().id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while svc.status(u64::MAX).recordings_saved < 5 {
        assert!(
            std::time::Instant::now() < deadline,
            "five recordings in a minute"
        );
        svc.pump().unwrap();
    }
    let gap = svc.status(u64::MAX).batch_gap_ms_max.unwrap();
    svc.stop_monitor().unwrap();

    let log = log.lock().unwrap();
    let on_pump = log
        .calls
        .iter()
        .filter(|c| match c {
            support::StoreCall::Start { thread, .. }
            | support::StoreCall::Push { thread, .. }
            | support::StoreCall::Finish { thread } => *thread == pump_thread,
        })
        .count();
    assert_eq!(
        on_pump, 0,
        "no store call on the thread that reads the station"
    );
    let finishes = log
        .calls
        .iter()
        .filter(|c| matches!(c, support::StoreCall::Finish { .. }))
        .count();
    assert_eq!(finishes, 5, "five complete recordings");
    assert!(
        gap < 50.0,
        "the longest gap between batches is {gap} ms: the reading thread waited on the store"
    );
}

// ---------------------------------------------------------------------------------
// G4: a run that cannot write ends and says so, and the recording is finalized with
// what reached the disk and the fault's words.
// ---------------------------------------------------------------------------------

fn logged(svc: &RecorderService<ScriptedStation>, words: &str) -> usize {
    svc.report_data()
        .event_log
        .events()
        .iter()
        .filter(|e| e.text.contains(words))
        .count()
}

/// Pump until the run ends or `limit` passes, letting the record thread answer.
fn pump_until_ended(svc: &mut RecorderService<ScriptedStation>, limit: usize) {
    for _ in 0..limit {
        if !svc.state().is_live() {
            return;
        }
        let _ = svc.pump();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn recorder_a_failed_write_ends_the_run_and_says_so() {
    let station = ScriptedStation::new(2, 8192.0, Box::new(ramp));
    let (mut store, log) = support::StoreDouble::new(2);
    store.fail_after_pushes = Some((
        5,
        "the disk refused the write: no space left on device".into(),
    ));
    let mut svc = RecorderService::with_backend(
        manual(8192.0),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g4_failed"),
    )
    .unwrap()
    .with_block_frames(256)
    .with_store(Box::new(store));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    pump_until_ended(&mut svc, 2000);
    assert_eq!(
        svc.state(),
        RecState::Faulted,
        "a run whose writes fail ends; it does not record on into nothing"
    );
    assert_eq!(
        logged(&svc, "the disk refused the write: no space left on device"),
        1,
        "the run ends once, in the fault's own words"
    );
    svc.stop_monitor().unwrap();
    let log = log.lock().unwrap();
    assert!(
        log.calls
            .iter()
            .any(|c| matches!(c, support::StoreCall::Finish { .. })),
        "the recording is finalized with what reached the disk"
    );
    assert_eq!(
        log.recordings[0].len(),
        5 * 512 * 2,
        "the five batches written before the fault"
    );
}

#[test]
fn recorder_the_free_space_floor_ends_the_run_and_says_so() {
    // A real writer on a real folder, with the floor a few megabytes under the free
    // space now: sixteen rows at 204 800 Hz in real time cross it in about a second.
    let fs = 204_800.0;
    let rows = 16;
    let root = temp_root("g4_floor");
    let free = hertzforge_capture::free_space_bytes_for(&root).expect("free space");
    let station = ScriptedStation::new(rows, fs, Box::new(ramp)).paced();
    let mut cfg = manual(fs);
    cfg.acq.block_size = 4096;
    cfg.keep_free_bytes = Some(free - 16 * 1024 * 1024);
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(rows as u32),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(4096)
    .with_store(Box::new(
        hertzforge_recorder::WriterStore::with_flush_interval(std::time::Duration::from_millis(50)),
    ));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while svc.state().is_live() && std::time::Instant::now() < deadline {
        let _ = svc.pump();
    }
    let ended = svc.state();
    let said = logged(&svc, "floor");
    svc.stop_monitor().unwrap();
    let dir = root.join("rec_0001");
    let sidecar = std::fs::read_to_string(dir.join("stream.json")).unwrap_or_default();
    let bytes = std::fs::metadata(dir.join("telemetry.f32"))
        .map(|m| m.len())
        .unwrap_or(0);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(ended, RecState::Faulted, "crossing the floor ends the run");
    assert_eq!(said, 1, "and says so, naming the floor");
    let rec: serde_json::Value = serde_json::from_str(&sidecar).unwrap();
    assert_eq!(rec["complete"], serde_json::json!(true), "{rec}");
    assert!(
        rec["fault"].as_str().is_some_and(|f| f.contains("floor")),
        "the recording names the fault: {rec}"
    );
    assert!(
        bytes > 0 && bytes.is_multiple_of(rows as u64 * 4),
        "{bytes} bytes"
    );
}

// ---------------------------------------------------------------------------------
// G5: a disk that falls behind ends the run, the gap recorded.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_falling_behind_the_disk_ends_the_run() {
    // A store that writes at half the stream's rate, read in real time: the backlog
    // grows half a second a second until the 2 s queue cannot take a batch.
    let fs = 2048.0;
    let station = ScriptedStation::new(2, fs, Box::new(ramp)).paced();
    let (mut store, log) = support::StoreDouble::new(2);
    store.slow_factor = Some((fs, 2.0));
    let mut svc = RecorderService::with_backend(
        manual(fs),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g5_behind"),
    )
    .unwrap()
    .with_block_frames(64)
    .with_store(Box::new(store));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut worst_backlog_s = 0.0f64;
    while svc.state().is_live() && std::time::Instant::now() < deadline {
        let _ = svc.pump();
        let s = svc.status(u64::MAX);
        worst_backlog_s =
            worst_backlog_s.max(s.frames_fed.saturating_sub(s.frames_on_disk) as f64 / fs);
    }
    let end = svc.status(u64::MAX).run_fault;
    svc.stop_monitor().unwrap();
    assert!(
        worst_backlog_s > 1.0,
        "the backlog rose on the status before the end: {worst_backlog_s} s"
    );
    let end = end.expect("a disk that far behind ends the run");
    assert_eq!(
        end.cause,
        hertzforge_recorder::service::EndCause::WriteBehind
    );
    assert!(end.message.contains("behind"), "{}", end.message);
    let report = svc.report_data();
    let gaps = &report.recordings[0].gaps;
    assert_eq!(
        gaps.len(),
        1,
        "the frames that could not be queued are a gap"
    );
    assert!(gaps[0].dropped_frames > 0);
    assert!(log
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|c| matches!(c, support::StoreCall::Finish { .. })));
}

// ---------------------------------------------------------------------------------
// G6: a timed recording holds exactly its frames after the trigger, plus its
// pre-window, whatever the batch.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_a_timed_recording_holds_exactly_its_frames() {
    let fs: f64 = 8192.0;
    // 0.3 s is 2457.6 frames: 2458, not a whole number of 512-frame batches.
    let record = (0.3 * fs).round() as u64;
    let pre = (0.1 * fs).round() as u64;
    let open = |mode: TriggerMode, signal: support::Signal, tag: &str| {
        let mut cfg = manual(fs);
        cfg.trigger_mode = mode;
        cfg.timed_record_s = Some(0.3);
        cfg.pretrigger_s = Some(0.1);
        let root = temp_root(tag);
        let mut svc = RecorderService::with_backend(
            cfg,
            inputs(2),
            ScriptedStation::new(2, fs, signal),
            hertzforge_module_abi::StationClass::NiSimulated,
            root.clone(),
        )
        .unwrap()
        .with_block_frames(256);
        svc.configure().unwrap();
        svc.start_monitor().unwrap();
        (svc, root)
    };
    let frames_of = |svc: &RecorderService<ScriptedStation>, root: &std::path::Path| {
        let list = svc.recordings_list(None);
        assert_eq!(list.len(), 1);
        let mut r = RecordingReader::open(&root.join(&list[0].name)).unwrap();
        let first = r.read_region(0, 1).unwrap()[0];
        (list[0].frames, first)
    };

    // Manual, not armed: no pre-window.
    let (mut svc, root) = open(TriggerMode::Manual, Box::new(ramp), "g6_manual");
    svc.pump_frames(1000).unwrap();
    svc.record_now().unwrap();
    for _ in 0..20 {
        svc.pump().unwrap();
    }
    assert_eq!(
        svc.state(),
        RecState::Monitoring,
        "the timed recording ended"
    );
    svc.stop_monitor().unwrap();
    assert_eq!(
        frames_of(&svc, &root).0,
        record,
        "exactly round(record_s x fs)"
    );

    // Manual, armed: the pre-window, then exactly the length.
    let (mut svc, root) = open(TriggerMode::Manual, Box::new(ramp), "g6_armed");
    svc.arm().unwrap();
    svc.pump_frames(2048).unwrap();
    svc.record_now().unwrap();
    for _ in 0..20 {
        svc.pump().unwrap();
    }
    svc.stop_monitor().unwrap();
    assert_eq!(frames_of(&svc, &root).0, pre + record);

    // Level: the trigger mid-batch, the pre-window, then exactly the length.
    let burst = bursts_every(5000, 1_000_000);
    let (mut svc, root) = open(
        level_on_column_0(0.0),
        Box::new(move |n, c| if c == 0 { burst(n, 0) } else { ramp(n, 1) }),
        "g6_level",
    );
    svc.arm().unwrap();
    for _ in 0..30 {
        svc.pump().unwrap();
    }
    svc.stop_monitor().unwrap();
    let (frames, first) = frames_of(&svc, &root);
    assert_eq!(frames, pre + record);
    let mut r = RecordingReader::open(&root.join(&svc.recordings_list(None)[0].name)).unwrap();
    let second = r.read_region(0, 1).unwrap()[1];
    assert_eq!(first, 0.0);
    assert_eq!(
        second,
        ramp(5000 - pre, 1),
        "the file begins a pre-window before the burst"
    );
}

// ---------------------------------------------------------------------------------
// G8: station time. Every recording states its station frames, the holdoff runs in
// station time from the trigger, and a re-armed trigger waits for a full pre-window
// acquired after the last recording ended.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_recordings_carry_station_frames() {
    let fs: f64 = 8192.0;
    let pre = (0.1 * fs).round() as u64;
    let burst = bursts_every(5000, 1_000_000);
    let mut cfg = manual(fs);
    cfg.trigger_mode = level_on_column_0(0.0);
    cfg.pretrigger_s = Some(0.1);
    cfg.timed_record_s = Some(0.3);
    let root = temp_root("g8_frames");
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(2),
        ScriptedStation::new(
            2,
            fs,
            Box::new(move |n, c| if c == 0 { burst(n, 0) } else { ramp(n, 1) }),
        ),
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    // Monitor a while before arming: the trigger's own count then differs from the
    // station's.
    svc.pump_frames(3072).unwrap();
    svc.arm().unwrap();
    for _ in 0..30 {
        svc.pump().unwrap();
    }
    svc.stop_monitor().unwrap();
    let rec = json_at(&root.join("rec_0001").join("stream.json"));
    assert_eq!(rec["trigger_station_frame"], 5000, "{rec}");
    assert_eq!(rec["first_station_frame"], 5000 - pre, "{rec}");
    assert_eq!(rec["pre_frames"], pre, "{rec}");
    assert_eq!(rec["pre_frames_asked"], pre, "{rec}");
    assert_eq!(rec["trigger"], "level", "{rec}");
    assert_eq!(rec["rows"], serde_json::json!([1, 2]), "{rec}");
    assert_eq!(rec["units"], serde_json::json!(["g", "g"]), "{rec}");
    let mut r = RecordingReader::open(&root.join("rec_0001")).unwrap();
    assert_eq!(r.read_region(0, 1).unwrap()[1], ramp(5000 - pre, 1));

    // A manual recording armed briefly: its trigger is the next batch's first frame,
    // and its pre-window states it had not filled.
    let root = temp_root("g8_manual");
    let mut cfg = manual(fs);
    cfg.pretrigger_s = Some(1.0);
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(2),
        ScriptedStation::new(2, fs, Box::new(ramp)),
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(256);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.pump_frames(2048).unwrap();
    svc.arm().unwrap();
    svc.pump_frames(1024).unwrap();
    svc.record_now().unwrap();
    svc.pump_frames(1024).unwrap();
    svc.stop_monitor().unwrap();
    let rec = json_at(&root.join("rec_0001").join("stream.json"));
    assert_eq!(rec["trigger"], "manual", "{rec}");
    assert_eq!(rec["trigger_station_frame"], 3072, "{rec}");
    assert_eq!(rec["pre_frames"], 1024, "{rec}");
    assert_eq!(rec["pre_frames_asked"], 8192, "{rec}");
    assert_eq!(rec["first_station_frame"], 2048, "{rec}");
}

#[test]
fn recorder_holdoff_counts_station_time_from_the_trigger() {
    // Bursts every 1.0 s from 0.5 s; each recording 1.0 s; a 2.9 s holdoff. The
    // holdoff ends 2.9 s of station time after a trigger, so the burst 3 s after it
    // fires; counted in armed time it would skip the recording's second too. Read in
    // real time, as a run is: unpaced, the 2 s queue to the record thread would fill
    // ahead of any disk.
    let fs: f64 = 2048.0;
    let mut cfg = manual(fs);
    cfg.trigger_mode = level_on_column_0(2.9);
    cfg.pretrigger_s = Some(0.25);
    cfg.timed_record_s = Some(1.0);
    cfg.auto_rearm = 2;
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(2),
        ScriptedStation::new(2, fs, bursts_every(1024, 2048)).paced(),
        hertzforge_module_abi::StationClass::NiSimulated,
        temp_root("g8_holdoff"),
    )
    .unwrap()
    .with_block_frames(64);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    let mut fires: Vec<u64> = Vec::new();
    for _ in 0..2000 {
        svc.pump().unwrap();
        let s = svc.status(u64::MAX);
        if let Some(f) = s.last_fire_station_frame {
            if fires.last() != Some(&f) {
                fires.push(f);
            }
        }
        if fires.len() == 3 {
            break;
        }
    }
    svc.stop_monitor().unwrap();
    assert_eq!(fires.len(), 3, "{fires:?}");
    assert_eq!(
        (fires[1] - fires[0], fires[2] - fires[1]),
        (3 * 2048, 3 * 2048),
        "each trigger 3 s of station time after the last: {fires:?}"
    );

    // A holdoff shorter than a recording: a re-armed trigger still waits for a full
    // pre-window acquired after the recording ended, so no frame is in two
    // recordings. Bursts every 0.3 s, recordings 0.1 s, pre-window 0.25 s.
    let mut cfg = manual(fs);
    cfg.trigger_mode = level_on_column_0(0.0);
    cfg.pretrigger_s = Some(0.25);
    cfg.timed_record_s = Some(0.1);
    cfg.auto_rearm = 2;
    let root = temp_root("g8_fresh");
    let mut svc = RecorderService::with_backend(
        cfg,
        inputs(2),
        ScriptedStation::new(2, fs, bursts_every(1024, 614)).paced(),
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(64);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    for _ in 0..2000 {
        svc.pump().unwrap();
        if svc.status(u64::MAX).recordings_saved == 3 {
            break;
        }
    }
    svc.stop_monitor().unwrap();
    let mut spans = Vec::new();
    for name in ["rec_0001", "rec_0002", "rec_0003"] {
        let rec = json_at(&root.join(name).join("stream.json"));
        let first = rec["first_station_frame"]
            .as_u64()
            .expect("first station frame");
        let frames = RecordingReader::open(&root.join(name))
            .unwrap()
            .info()
            .frames;
        spans.push((first, first + frames));
    }
    for w in spans.windows(2) {
        assert!(w[1].0 >= w[0].1, "a frame in two recordings: {spans:?}");
    }
}

// ---------------------------------------------------------------------------------
// G7: limits are marks. A row passing a limit is marked and recording goes on.
// ---------------------------------------------------------------------------------

#[test]
fn recorder_a_limit_crossing_is_marked_and_recording_goes_on() {
    use hertzforge_recorder::service::Mark;
    // Row 1: a 250 Hz tone of 0.5 g from 0.5 s to 1.0 s of every 2 s, quiet between;
    // row 2: 0.2 g of 250 Hz always. Both rows limited at 0.4 g; row 1's range 0.5 g
    // (its overload indicator at 0.45 g). On a hardware station.
    let fs: f64 = 2048.0;
    let period = 2 * 2048u64;
    let row1 = move |n: u64| -> f32 {
        let k = n % period;
        if (1024..2048).contains(&k) {
            (0.5 * (std::f64::consts::TAU * 250.0 * n as f64 / fs).sin()) as f32
        } else {
            0.0
        }
    };
    let row2 = move |n: u64| -> f32 {
        (0.2 * (std::f64::consts::TAU * 250.0 * n as f64 / fs).sin()) as f32
    };
    let station = ScriptedStation::new(
        2,
        fs,
        Box::new(move |n, c| if c == 0 { row1(n) } else { row2(n) }),
    )
    .paced();
    let mut table = inputs(2);
    table.channels[0].range_max = 0.5;
    for c in table.channels.iter_mut() {
        c.abort.accel_g = Some(0.4);
    }
    let mut cfg = manual(fs);
    cfg.timed_record_s = Some(6.0);
    cfg.overload_fraction = 0.9;
    let root = temp_root("g7_marks");
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        station,
        hertzforge_module_abi::StationClass::NiSimulated,
        root.clone(),
    )
    .unwrap()
    .with_block_frames(64);
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_from_start().unwrap();
    while svc.state() == RecState::Recording {
        svc.pump().expect("a limit crossing does not end the run");
    }
    assert_eq!(
        svc.state(),
        RecState::Monitoring,
        "the recording ran its length"
    );
    let marks = svc.marks().to_vec();
    svc.stop_monitor().unwrap();
    assert_eq!(svc.recordings_list(None)[0].frames, 6 * 2048);

    // One limit mark per episode on row 1, at its first sample past 0.4 g.
    let first_over = |from: u64| (from..).find(|&n| row1(n).abs() > 0.4).unwrap();
    let want: Vec<u64> = (0..3).map(|k| first_over(k * period)).collect();
    let limits: Vec<(u32, u64, f64)> = marks
        .iter()
        .filter_map(|m| match m {
            Mark::Limit {
                row,
                station_frame,
                value,
                recording,
                ..
            } => {
                assert_eq!(recording.as_deref(), Some("rec_0001"));
                Some((*row, *station_frame, *value))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        limits.iter().map(|l| l.1).collect::<Vec<_>>(),
        want,
        "a mark at each episode's first crossing, none on row 2: {limits:?}"
    );
    assert!(limits.iter().all(|l| l.0 == 1 && l.2 > 0.4));
    let overloads: Vec<&Mark> = marks
        .iter()
        .filter(|m| matches!(m, Mark::Overload { .. }))
        .collect();
    assert_eq!(
        overloads.len(),
        1,
        "the row's overload indicator latches once"
    );
    let rec = json_at(&root.join("rec_0001").join("stream.json"));
    assert_eq!(
        rec["marks"].as_array().unwrap().len(),
        4,
        "the marks inside it: {rec}"
    );
}
