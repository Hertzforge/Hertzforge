//! The read surfaces the control service maps onto the wire: the station's frame
//! count and both rates, the pass and batch timings, the trigger's held counters, the
//! pre-window's fill, the recording's frames fed and on disk, the writer's fault, the
//! overload latches, the FRF's phase, and the record store seam the recordings go
//! through. Each is held state or a counter, never a one-pass edge.

mod support;

use std::f64::consts::TAU;
use std::time::Duration;

use hertzforge_recorder::service::{RecState, RecorderConfig, RecorderService};
use hertzforge_recorder::trigger::{LevelTrigger, Slope, TriggerMode};
use hertzforge_recorder::AcquisitionConfig;
use support::{inputs, temp_root, ScriptedStation, StoreCall, StoreDouble};

const FS: f64 = 8_192.0;

fn acq() -> AcquisitionConfig {
    AcquisitionConfig {
        native_rate_hz: FS,
        block_size: 256,
        analysis_decimation: 1,
        ..Default::default()
    }
}

fn cfg(mode: TriggerMode) -> RecorderConfig {
    RecorderConfig {
        acq: acq(),
        trigger_mode: mode,
        overload_fraction: 1e12,
        ..Default::default()
    }
}

fn quiet() -> support::Signal {
    Box::new(|_, _| 0.0)
}

/// A burst of 1.0 lasting 32 frames at each of `at`, 0 elsewhere, on column 0.
fn bursts(at: &'static [u64]) -> support::Signal {
    Box::new(move |n, c| {
        if c == 0 && at.iter().any(|&a| n >= a && n < a + 32) {
            1.0
        } else {
            0.0
        }
    })
}

fn level(threshold: f64) -> TriggerMode {
    TriggerMode::Level(LevelTrigger {
        channel_index: 0,
        threshold_eu: threshold,
        slope: Slope::Rising,
        hysteresis_eu: 0.1,
        holdoff_s: 0.0,
    })
}

#[test]
fn recorder_status_counts_station_frames_and_states_both_rates() {
    let station = ScriptedStation::new(2, FS, quiet());
    let mut svc = RecorderService::with_backend(
        cfg(TriggerMode::Manual),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_frames"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    let mut pumped = 0;
    for _ in 0..3 {
        pumped += svc.pump_frames(1024).unwrap();
    }
    let s = svc.status(u64::MAX);
    assert_eq!(
        s.station_frames, pumped,
        "every frame read is a station frame"
    );
    assert!(s.batches >= 3, "batches {}", s.batches);
    assert_eq!(s.requested_fs_hz, FS);
    assert_eq!(s.fs_hz, FS);
}

#[test]
fn recorder_status_times_the_analysis_pass_and_the_gap_between_batches() {
    let station = ScriptedStation::new(1, FS, quiet());
    let mut svc = RecorderService::with_backend(
        cfg(TriggerMode::Manual),
        inputs(1),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_timing"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    let before = svc.status(u64::MAX);
    assert_eq!(before.pass_ms_last, None);
    assert_eq!(before.batch_gap_ms_max, None);

    svc.pump_frames(512).unwrap();
    let one = svc.status(u64::MAX);
    let (last, worst) = (one.pass_ms_last.unwrap(), one.pass_ms_worst.unwrap());
    assert!(last >= 0.0 && worst >= last, "last {last} worst {worst}");

    // A pause between two batches is the gap the status holds, at least as long.
    std::thread::sleep(Duration::from_millis(30));
    svc.pump_frames(512).unwrap();
    svc.pump_frames(512).unwrap();
    let gap = svc.status(u64::MAX).batch_gap_ms_max.unwrap();
    assert!(gap >= 30.0, "the longest gap is held: {gap} ms");
}

#[test]
fn recorder_status_holds_the_trigger_fires_and_the_last_fire_frame() {
    // Two bursts, each far past the pre-window's depth after the last.
    static AT: [u64; 2] = [3000, 9000];
    let station = ScriptedStation::new(1, FS, bursts(&AT)).frames(16_000);
    let mut c = cfg(level(0.5));
    c.pretrigger_s = Some(0.1);
    c.timed_record_s = Some(0.05);
    c.auto_rearm = 1;
    let mut svc = RecorderService::with_backend(
        c,
        inputs(1),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_fires"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    for _ in 0..40 {
        if !svc.state().is_live() {
            break;
        }
        let _ = svc.pump_frames(256);
    }
    let s = svc.status(u64::MAX);
    assert_eq!(s.trigger_fires, 2, "both fires are counted and held");
    assert_eq!(
        s.last_fire_station_frame,
        Some(9000),
        "the last fire's station frame is held after the pass that saw it"
    );
}

#[test]
fn recorder_status_holds_the_pre_window_fill() {
    let station = ScriptedStation::new(1, FS, quiet());
    let mut c = cfg(TriggerMode::Manual);
    c.pretrigger_s = Some(1.0);
    let mut svc = RecorderService::with_backend(
        c,
        inputs(1),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_pre"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.arm().unwrap();
    assert_eq!(svc.status(u64::MAX).pre_frames_held, 0);
    svc.pump_frames(2048).unwrap();
    let s = svc.status(u64::MAX);
    assert_eq!(s.pre_frames_asked, 8192, "one second at 8192 Hz");
    assert_eq!(s.pre_frames_held, 2048, "a quarter of it has filled");
}

#[test]
fn recorder_status_counts_recordings_and_the_frames_fed_and_on_disk() {
    let station = ScriptedStation::new(2, FS, Box::new(|n, c| (n as f32) + c as f32));
    let mut svc = RecorderService::with_backend(
        cfg(TriggerMode::Manual),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_disk"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    let mut fed = 0;
    for _ in 0..4 {
        fed += svc.pump_frames(2048).unwrap();
    }
    // The writer is on its own thread: wait for it to reach the disk.
    let mut s = svc.status(u64::MAX);
    for _ in 0..200 {
        if s.frames_on_disk > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        s = svc.status(u64::MAX);
    }
    assert_eq!(s.frames_fed, fed, "every frame handed on is counted");
    assert!(
        s.frames_on_disk > 0 && s.frames_on_disk <= s.frames_fed,
        "on disk {} of {} fed",
        s.frames_on_disk,
        s.frames_fed
    );
    assert_eq!(s.recordings_saved, 0);
    svc.stop_recording().unwrap();
    assert_eq!(svc.status(u64::MAX).recordings_saved, 1);
}

#[test]
fn recorder_status_names_the_writer_fault_in_its_words() {
    let station = ScriptedStation::new(1, FS, quiet());
    let (mut store, _log) = StoreDouble::new(1);
    store.fail_after_pushes = Some((1, "the disk refused the write".into()));
    let mut svc = RecorderService::with_backend(
        cfg(TriggerMode::Manual),
        inputs(1),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_fault"),
    )
    .unwrap()
    .with_store(Box::new(store));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.record_now().unwrap();
    assert_eq!(svc.status(u64::MAX).write_fault, None);
    svc.pump_frames(1024).unwrap();
    svc.pump_frames(1024).unwrap();
    assert_eq!(
        svc.status(u64::MAX).write_fault.as_deref(),
        Some("the disk refused the write")
    );
}

#[test]
fn recorder_overload_latches_are_held_state() {
    // One frame at 95% of the row's range, then quiet for good.
    let station = ScriptedStation::new(
        2,
        FS,
        Box::new(|n, c| if n == 100 && c == 1 { 95.0 } else { 0.0 }),
    );
    let mut c = cfg(TriggerMode::Manual);
    c.overload_fraction = 0.9;
    let mut svc = RecorderService::with_backend(
        c,
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_overload"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..6 {
        svc.pump_frames(1024).unwrap();
    }
    assert_eq!(
        svc.status(u64::MAX).overload_latches,
        vec![(0, false), (1, true)],
        "the latch holds long after the frame that set it"
    );
}

#[test]
fn recorder_frf_snapshot_carries_the_phase_of_h1() {
    // y is x delayed by 3 frames: H1 = e^{-j w 3}, phase -360 f 3 / fs degrees.
    let delay = 3u64;
    let x = |n: u64| -> f64 {
        let t = n as f64 / FS;
        [200.0, 500.0, 900.0]
            .iter()
            .enumerate()
            .map(|(k, f)| (TAU * f * t + 0.7 * k as f64).sin())
            .sum()
    };
    let station = ScriptedStation::new(
        2,
        FS,
        Box::new(move |n, c| {
            if c == 0 {
                x(n) as f32
            } else if n >= delay {
                x(n - delay) as f32
            } else {
                0.0
            }
        }),
    );
    let mut c = cfg(TriggerMode::Manual);
    c.acq.block_size = 1024;
    c.frf_pair = Some((0, 1));
    let mut svc = RecorderService::with_backend(
        c,
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_phase"),
    )
    .unwrap();
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    for _ in 0..12 {
        svc.pump_frames(2048).unwrap();
    }
    let snap = svc.frf().expect("a pair is configured");
    assert_eq!(snap.h1_phase_deg.len(), snap.h1_mag.len());
    for f in [200.0, 500.0, 900.0] {
        let bin = (f / snap.df_hz).round() as usize;
        let want = -360.0 * (bin as f64 * snap.df_hz) * delay as f64 / FS;
        let want = (want + 180.0).rem_euclid(360.0) - 180.0;
        let got = snap.h1_phase_deg[bin];
        assert!(
            (got - want).abs() < 1.0,
            "phase at {f} Hz: {got:.2} deg, want {want:.2}"
        );
    }
}

#[test]
fn recorder_every_recording_goes_through_the_store_seam() {
    let station = ScriptedStation::new(2, FS, Box::new(|n, c| n as f32 * 2.0 + c as f32));
    let (store, log) = StoreDouble::new(2);
    let mut svc = RecorderService::with_backend(
        cfg(TriggerMode::Manual),
        inputs(2),
        station,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        temp_root("rs_seam"),
    )
    .unwrap()
    .with_store(Box::new(store));
    svc.configure().unwrap();
    svc.start_monitor().unwrap();
    svc.pump_frames(1000).unwrap();
    let from = svc.status(u64::MAX).station_frames;
    svc.record_now().unwrap();
    let mut fed = 0;
    for _ in 0..3 {
        fed += svc.pump_frames(1000).unwrap();
    }
    svc.stop_recording().unwrap();
    assert_eq!(svc.state(), RecState::Monitoring);

    let log = log.lock().unwrap();
    let starts = log
        .calls
        .iter()
        .filter(|c| matches!(c, StoreCall::Start { .. }))
        .count();
    let finishes = log
        .calls
        .iter()
        .filter(|c| matches!(c, StoreCall::Finish { .. }))
        .count();
    let pushed: usize = log
        .calls
        .iter()
        .map(|c| match c {
            StoreCall::Push { frames, .. } => *frames,
            _ => 0,
        })
        .sum();
    assert_eq!((starts, finishes), (1, 1));
    assert_eq!(
        pushed as u64, fed,
        "every frame read while recording reached the store"
    );
    let rec = &log.recordings[0];
    assert_eq!(
        rec[0],
        from as f32 * 2.0,
        "the store's first frame is the next frame read"
    );
}
