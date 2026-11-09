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
