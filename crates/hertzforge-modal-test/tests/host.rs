//! The run host on the simulator: impact mode writes no drive sample; shaker mode ramps
//! up on a frame's start, stops by the shared ramp, and an abort zeroes at once.

use hertzforge_channel_table::{AbortLimits, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_modal_test::host::{
    capture_only_table, stop_ramp_frames, DriveBuild, DrivePhase, HostBuild, ModalHost, Pacing,
};
use hertzforge_modal_test::setup::SourceKind;
use hertzforge_modal_test::source::SourceBuild;
use hertzforge_module_abi::StationClass;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Seen {
    drive: Vec<f32>,
    calls: u64,
    estops: Vec<usize>,
    after_estop_peak: f32,
}

struct Spy {
    inner: Box<dyn Backend>,
    seen: Arc<Mutex<Seen>>,
    width: usize,
}

impl Backend for Spy {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        t: &ChannelTable,
        a: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(t, a)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(d)?;
        let mut s = self.seen.lock().unwrap();
        s.calls += 1;
        let taken = &d[..(n * self.width).min(d.len())];
        if !s.estops.is_empty() {
            let m = taken.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            s.after_estop_peak = s.after_estop_peak.max(m);
        }
        s.drive.extend_from_slice(taken);
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        let mut s = self.seen.lock().unwrap();
        let n = s.drive.len();
        s.estops.push(n);
        drop(s);
        self.inner.emergency_stop()
    }
}

const FS: f64 = 1024.0;
const N: usize = 256;

fn table(drives: u32) -> ChannelTable {
    let mut t = ChannelTable::default_with_channels(2);
    for c in t.channels.iter_mut() {
        c.abort = AbortLimits::default();
    }
    for k in 0..drives {
        let mut d = hertzforge_channel_table::Channel::new(20 + k);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        d.range_max = 10.0;
        t.channels.push(d);
    }
    t
}

fn sim(width: usize, drives: u32) -> (Box<dyn Backend>, Arc<Mutex<Seen>>) {
    let inner = Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(FS)
            .noise_level(0.0)
            .max_input_channels(8)
            .max_output_channels(drives.max(1))
            .build(),
    );
    let seen = Arc::new(Mutex::new(Seen::default()));
    (
        Box::new(Spy {
            inner,
            seen: seen.clone(),
            width,
        }),
        seen,
    )
}

fn wait_for(what: impl Fn() -> bool, wait: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < wait {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn shaker(drives: usize, ceiling_v: f64) -> HostBuild {
    HostBuild {
        fs_hz: FS,
        block: N,
        drive: Some((
            SourceBuild {
                kind: SourceKind::PeriodicChirp,
                fs_hz: FS,
                block: N,
                lo_line: 4,
                hi_line: 60,
                levels: vec![0.1; drives],
                burst_pct: 50.0,
                delay_frames: 0,
                seed: 1,
            },
            DriveBuild {
                outputs: drives,
                slots: (0..drives).collect(),
                block: N,
                ramp_frames: N as u64,
                ceiling_v,
                stop_ramp_frames: stop_ramp_frames(FS),
            },
        )),
        pacing: Pacing::WallClock { speed: 8.0 },
        record_until: None,
    }
}

#[test]
fn modal_test_impact_host_writes_no_drive_sample() {
    let t = table(1);
    let (b, seen) = sim(1, 1);
    let h = ModalHost::start(
        HostBuild {
            fs_hz: FS,
            block: N,
            drive: None,
            pacing: Pacing::WallClock { speed: 8.0 },
            record_until: None,
        },
        capture_only_table(&t),
        b,
        StationClass::HertzforgeSim,
    )
    .unwrap();
    assert_eq!(h.outputs, 0);
    assert!(wait_for(
        || h.status().frames_in > 2048,
        Duration::from_secs(20)
    ));
    let mut cap = h.capture.lock().unwrap().take().unwrap();
    let mut f = vec![0f32; cap.frame_len()];
    let mut n = 0;
    while cap.pop(&mut f) {
        n += 1;
    }
    assert!(n > 2000);
    h.finish();
    assert!(h.join(Duration::from_secs(20)));
    assert_eq!(h.status().end.unwrap().outcome, "ended");
    let s = seen.lock().unwrap();
    assert!(
        s.drive.iter().all(|v| *v == 0.0),
        "no drive sample in impact mode"
    );
    assert!(
        s.drive.is_empty(),
        "nothing written: {} samples",
        s.drive.len()
    );
    let _ = &mut cap;
}

#[test]
fn modal_test_shaker_host_ramps_on_a_frame_start_and_stops_by_the_shared_ramp() {
    let (b, seen) = sim(2, 2);
    let h = ModalHost::start(shaker(2, 1.0), table(2), b, StationClass::HertzforgeSim).unwrap();
    let lead = h.status().lead_in_frames;
    h.start_drive();
    assert!(wait_for(
        || h.status().phase == DrivePhase::Driving,
        Duration::from_secs(20)
    ));
    let sh = h.shared();
    let from = sh.ramp_from.load(std::sync::atomic::Ordering::Acquire);
    assert_eq!(
        (from - lead) % N as u64,
        0,
        "the ramp begins on a source frame's start"
    );
    std::thread::sleep(Duration::from_millis(300));
    h.stop_drive();
    assert!(wait_for(
        || h.status().phase == DrivePhase::Idle,
        Duration::from_secs(30)
    ));
    h.finish();
    assert!(h.join(Duration::from_secs(30)));
    assert_eq!(h.status().end.unwrap().outcome, "ended");
    let stop = sh.stop_from.load(std::sync::atomic::Ordering::Acquire) as usize;
    let quiet = sh.quiet_from.load(std::sync::atomic::Ordering::Acquire) as usize;
    assert_eq!(quiet - stop, stop_ramp_frames(FS) as usize);
    let s = seen.lock().unwrap();
    let d = &s.drive;
    let peak = |a: usize, b: usize| {
        (a..b)
            .map(|i| d[2 * i].abs().max(d[2 * i + 1].abs()))
            .fold(0f32, f32::max)
    };
    let full = peak(stop - N, stop);
    assert!(full > 0.1, "driving before the stop: {full}");
    // 40 dB down by the ramp's end, then zero.
    let tail = peak(quiet - N / 4, quiet);
    assert!(tail < full * 0.0125, "{tail} vs {full}");
    assert!(d[2 * quiet..].iter().all(|v| *v == 0.0));
    assert!(s.estops.is_empty());
}

#[test]
fn modal_test_shaker_host_abort_zeroes_at_once() {
    let (b, seen) = sim(1, 1);
    let h = ModalHost::start(shaker(1, 1.0), table(1), b, StationClass::HertzforgeSim).unwrap();
    h.start_drive();
    assert!(wait_for(
        || h.status().phase == DrivePhase::Driving,
        Duration::from_secs(20)
    ));
    h.abort();
    assert!(h.join(Duration::from_secs(20)));
    let o = h.status().end.unwrap();
    assert_eq!(o.outcome, "aborted");
    assert!(o.emergency_stop);
    let s = seen.lock().unwrap();
    assert_eq!(s.estops.len(), 1);
    assert_eq!(s.after_estop_peak, 0.0);
}

#[test]
fn modal_test_shaker_host_ceiling_trips_the_emergency_stop() {
    let (b, seen) = sim(1, 1);
    // A chirp of 0.1 V RMS peaks at 0.1414 V: over a 0.12 V ceiling once ramped.
    let h = ModalHost::start(shaker(1, 0.12), table(1), b, StationClass::HertzforgeSim).unwrap();
    h.start_drive();
    assert!(h.join(Duration::from_secs(30)));
    let o = h.status().end.unwrap();
    assert_eq!(o.outcome, "drive_ceiling");
    let s = seen.lock().unwrap();
    assert!(s.drive.iter().all(|v| v.abs() <= 0.12));
    assert_eq!(s.estops.len(), 1);
}
