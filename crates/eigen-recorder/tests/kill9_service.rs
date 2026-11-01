//! The capture kill-9 recovery CONTROL, at the service level.
//!
//! ## What already exists, and why this is not it
//!
//! The provisional-sidecar mechanics shipped earlier, with their unit:
//! `eigen-capture/src/tests.rs::a_killed_capture_leaves_a_readable_provisional_sidecar`
//! pushes chunks into a bare `CaptureSink` and then `drop`s it without
//! `finish()`. Its own comment calls that "the in-process stand-in for
//! `kill -9`", which is exactly right and exactly its limit: `Drop for
//! CaptureSink` runs, joins the writer thread, and lets it finish its work.
//! `eigen-recorder/tests/recording.rs::crash_recovery_from_provisional_sidecar_without_capture_json`
//! covers the reader side, from an aftermath built by hand.
//!
//! Neither one runs a `RecorderService`, and neither one is a kill. This is the
//! service-level version, with a REAL `SIGKILL`:
//!
//! * a child process builds an actual `RecorderService` on the Sim station,
//!   monitors, records, and parks;
//! * the parent kills it with `Child::kill()` - `SIGKILL` on unix, no
//!   destructors, no thread join, no flush, no `finish()`;
//! * the parent then reads the wreckage back and asserts it is HONEST.
//!
//! "Honest" is the whole point and it is a conjunction: `capture.json` says
//! `complete: false`, `recorder.json` says `clean: false`, the binary is a whole
//! number of frames, `RecordingReader` opens it and reports the unclean status,
//! and the frame count it reports comes from the FILE - never from a sidecar
//! counter that a dead process never got to update. A capture that came back
//! claiming to be complete would be far worse than one that failed to open.
//!
//! ## How the child is spawned without a new binary target or a new dependency
//!
//! The test re-invokes its OWN binary (`std::env::current_exe()`) with the child
//! test's name and an env var naming the data root. The child branch runs first
//! thing in the test body, so no other test in this file executes in that
//! process. No `[[bin]]`, no `tempfile`, no `libc`, no `assert_cmd`.
//!
//! ## Why the data is on disk when the child dies
//!
//! Not the timed flush - that cadence is 5 s and this test does not wait for it.
//! `CaptureSink`'s writer wraps the file in an 8 KiB `BufWriter`, so once more
//! than 8 KiB of samples have been written the buffer spills into the file, and
//! a `write(2)` that has returned survives `SIGKILL` (the kernel owns the data
//! from that point, not the process). The child therefore records far more than
//! 8 KiB and waits until the sink reports frames actually WRITTEN - not merely
//! pushed - before it parks. What is lost is the final partial buffer, which is
//! the honest kill-9 outcome the sidecar's `complete: false` exists to announce.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Interface, Polarity, Units};
use eigen_recorder::service::{RecorderConfig, RecorderService};
use eigen_recorder::trigger::TriggerMode;
use eigen_recorder::RecordingReader;

/// Names the data root AND selects the child role.
const ROLE_ENV: &str = "EIGEN_P4_KILL9_ROOT";
/// The child writes this once its recording has frames on disk.
const READY_MARKER: &str = "child_ready";

fn input_table(n: u32) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 1..=n {
        let mut c = Channel::new(i);
        c.role = ChannelRole::Monitor;
        c.enabled = true;
        c.units = Units::AccelG;
        c.interface = Interface::SingleEnded;
        c.polarity = Polarity::Positive;
        c.range_max = 100.0;
        c.sensitivity_mv_per_eu = 100.0;
        t.channels.push(c);
    }
    t
}

const N_CH: usize = 2;
const RATE_HZ: f64 = 8_192.0;

/// The CHILD role: run a real recorder session, then park forever.
///
/// It never returns. The parent's `SIGKILL` is the only exit, which is the
/// point: nothing here gets an orderly shutdown, and `finish_recording` is never
/// reached.
fn child_record_and_park(root: PathBuf) -> ! {
    let table = input_table(N_CH as u32);
    let cfg = RecorderConfig {
        acq: eigen_recorder::AcquisitionConfig {
            native_rate_hz: RATE_HZ,
            block_size: 256,
            analysis_decimation: 1,
            ..Default::default()
        },
        trigger_mode: TriggerMode::Manual,
        ..Default::default()
    };

    let backend = eigen_hal_sim::SimConfig::new()
        .max_input_channels(N_CH as u32)
        .build();
    let mut svc = RecorderService::with_backend(
        cfg,
        table,
        Box::new(backend),
        eigen_module_abi::StationClass::EigenSim,
        root.clone(),
    )
    .expect("child: service");
    svc.configure().expect("child: configure");
    svc.start_monitor().expect("child: monitor");
    svc.record_now().expect("child: record");

    // Push frames until the SINK reports data actually WRITTEN (not merely
    // pushed): the writer is on its own thread behind a ring, so "I handed it
    // over" is not "it reached the file". Only then is the marker honest.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        svc.pump_frames(4096).expect("child: pump");
        let run_dir = active_run_dir(&root);
        if let Some(dir) = run_dir {
            let bytes = std::fs::metadata(dir.join("telemetry.f32"))
                .map(|m| m.len())
                .unwrap_or(0);
            // Comfortably past the writer's 8 KiB BufWriter, so the spill has
            // certainly happened and the bytes belong to the kernel.
            if bytes >= 64 * 1024 {
                std::fs::write(root.join(READY_MARKER), dir.to_string_lossy().as_bytes())
                    .expect("child: marker");
                break;
            }
        }
        assert!(Instant::now() < deadline, "child: never reached the disk");
    }

    // Park. No stop, no finish, no drop - the parent kills this process.
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The one run directory under `root` (the service names it itself).
fn active_run_dir(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_dir() && p.join("telemetry.f32").exists()).then_some(p)
    })
}

/// THE control. A recorder session dies mid-recording; what is on disk is
/// readable and says what it is.
///
/// RED-MUTATION: make `CaptureSink::start_durable` write its provisional sidecar
/// with `complete: true` (or skip writing it until `finish`) and this reds - the
/// wreckage either claims to be a finished capture or cannot be opened at all.
/// Second: have `RecordingReader` take its frame count from the sidecar instead
/// of the file size and the frame assertions red, because a killed process never
/// updated that counter.
#[test]
#[cfg_attr(
    not(unix),
    ignore = "Child::kill is SIGKILL on unix; this control is unix-only"
)]
fn a_kill_9_mid_recording_leaves_readable_honest_artifacts() {
    // ---- child role -------------------------------------------------------
    if let Ok(dir) = std::env::var(ROLE_ENV) {
        child_record_and_park(PathBuf::from(dir));
    }

    // ---- parent role ------------------------------------------------------
    let root = std::env::temp_dir().join(format!("eigen-p4-kill9-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let exe = std::env::current_exe().expect("the test binary path");
    let mut child = Command::new(exe)
        .arg("a_kill_9_mid_recording_leaves_readable_honest_artifacts")
        .arg("--exact")
        .arg("--test-threads=1")
        .env(ROLE_ENV, &root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the child recorder");

    // Wait for the child to say its frames are on disk.
    let marker = root.join(READY_MARKER);
    let deadline = Instant::now() + Duration::from_secs(90);
    while !marker.exists() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&root);
            panic!("the child never reported frames on disk");
        }
        // If the child died on its own, this control proves nothing - fail loudly
        // rather than reporting on a process that was never killed.
        if let Ok(Some(status)) = child.try_wait() {
            let _ = std::fs::remove_dir_all(&root);
            panic!("the child exited on its own ({status}); nothing was killed");
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // KILL -9. No unwinding, no Drop, no writer join, no flush, no finish().
    child.kill().expect("SIGKILL the child");
    let status = child.wait().expect("reap the child");
    assert!(
        !status.success(),
        "a killed process must not report success: {status:?}"
    );

    // ---- the wreckage -----------------------------------------------------
    let run_dir = active_run_dir(&root).expect("a run directory survived the kill");

    // 1. The CAPTURE sidecar exists, parses, and says it is not complete.
    let capture_json = std::fs::read_to_string(run_dir.join("capture.json"))
        .expect("capture.json must be readable after a kill (it is written before any data)");
    let capture: serde_json::Value =
        serde_json::from_str(&capture_json).expect("capture.json must be valid JSON");
    assert_eq!(
        capture["complete"],
        serde_json::Value::Bool(false),
        "a killed capture must NOT claim completeness: {capture_json}"
    );
    // The layout descriptor is what makes the bytes interpretable at all.
    assert_eq!(capture["config"]["n_channels"], N_CH);
    assert_eq!(capture["config"]["sample_rate_hz"], RATE_HZ);
    assert!(
        capture["layout"]["interleave"].is_string(),
        "the layout descriptor must survive: {capture_json}"
    );

    // 2. The RECORDER sidecar exists, parses, and says the run was not clean.
    let rec_json = std::fs::read_to_string(run_dir.join("recorder.json"))
        .expect("recorder.json must be readable (written before any data)");
    let rec: serde_json::Value = serde_json::from_str(&rec_json).expect("recorder.json JSON");
    assert_eq!(
        rec["clean"],
        serde_json::Value::Bool(false),
        "only finish() sets clean: {rec_json}"
    );

    // 3. The binary is a whole number of frames - a torn frame would make every
    //    channel after the tear point read as another channel's data.
    let bytes = std::fs::metadata(run_dir.join("telemetry.f32"))
        .expect("telemetry.f32")
        .len();
    assert!(
        bytes >= 64 * 1024,
        "the child reported {bytes} bytes on disk"
    );
    let frame_bytes = 4 * N_CH as u64;
    assert_eq!(
        bytes % frame_bytes,
        0,
        "{bytes} bytes is not a whole number of {N_CH}-channel frames"
    );

    // 4. It OPENS, and it is honest about what it is.
    let mut reader = RecordingReader::open(&run_dir).expect("the wreckage must be readable");
    let info = reader.info().clone();
    assert!(!info.clean, "a killed run is not clean");
    assert!(info.status.contains("unclean"), "status: {}", info.status);
    assert_eq!(info.n_channels, N_CH);
    assert_eq!(info.rate_hz, RATE_HZ);

    // 5. The frame count comes from the FILE, not from a counter the dead
    //    process never got to update (here under a real kill).
    assert_eq!(
        info.frames,
        bytes / frame_bytes,
        "the reader must count frames from the file"
    );
    assert!(info.frames > 0);

    // 6. And the data is real: readable, finite, the right width. A recovered
    //    capture that opens but yields garbage is not a recovered capture.
    let want = 128usize.min(info.frames as usize);
    let region = reader.read_region(0, want).expect("read the region");
    assert_eq!(region.len(), want * N_CH, "interleave width");
    assert!(
        region.iter().all(|v| v.is_finite()),
        "recovered samples must be finite"
    );

    let _ = std::fs::remove_dir_all(&root);
}
