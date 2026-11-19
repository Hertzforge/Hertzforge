//! Playback runs on the simulator through the run host: the play, its trips and its
//! stop, and a long file streamed with no underrun.

mod common;

use common::*;
use hertzforge_module_abi::StationClass;
use hertzforge_playback::{HostError, HostOptions, Pacing, Phase, PlaybackHost};
use std::io::Read;
use std::time::{Duration, Instant};

const SIM: StationClass = StationClass::HertzforgeSim;

fn wait_end(h: &PlaybackHost, wait: Duration) -> hertzforge_playback::Outcome {
    assert!(
        h.join(wait),
        "the run did not end in {wait:?}; status {:?}",
        h.status()
    );
    h.status().end.expect("an ended run has its outcome")
}

fn unpaced() -> HostOptions {
    HostOptions::default()
}

/// Four station seconds per wall second. A test whose verdict rests on the reader keeping
/// up with the loop, or on the loop outrunning a slow reader, runs paced: an unpaced loop
/// on a starved three-core runner outran a reader that was only waiting for a core.
fn paced() -> HostOptions {
    HostOptions {
        pacing: Pacing::WallClock { speed: 4.0 },
        ..HostOptions::default()
    }
}

#[test]
fn a_file_plays_to_its_end_with_its_rolls_on_each_drive() {
    let fs = 4096.0;
    let dir = temp("play");
    let a = burst(fs, 1.0, 100.0, 0.4);
    let b = burst(fs, 1.0, 230.0, 0.2);
    let folder = write(&dir, fs, &[a.clone(), b.clone()]);
    // file channel 0 to the second drive row, channel 1 to the first
    let t = table(2, 2, None);
    let drive = checked(&folder, &t, setup(vec![4, 3], -6.0, 1.0));
    let (backend, seen) = spy(wire(fs, 2, 2), 2, true);
    let h = PlaybackHost::start(drive, t, backend, SIM, unpaced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "completed", "{}", end.message);
    assert_eq!(end.played, 4096);
    assert!(!end.emergency_stop);
    let s = seen.lock().unwrap();
    assert!(s.estops.is_empty());
    let g = 10f64.powf(-6.0 / 20.0);
    let pre = 1024; // 0.25 s of pre-roll
                    // The station plays the drive behind the engine's lead-in: find where the file
                    // begins (its first sample is 0 V, its second is not).
    let first = s
        .drive
        .chunks(2)
        .position(|f| f[0] != 0.0 || f[1] != 0.0)
        .unwrap();
    let lead = first - 1;
    assert!(lead >= pre && lead < pre + 512, "lead {lead}");
    assert!(s.drive[..2 * lead].iter().all(|x| *x == 0.0));
    for k in [10usize, 1000, 2047, 4000] {
        let f = lead + k;
        assert!(
            (f64::from(s.drive[2 * f + 1]) - a[k] * g).abs() < 1e-6,
            "slot 1 at {k}"
        );
        assert!(
            (f64::from(s.drive[2 * f]) - b[k] * g).abs() < 1e-6,
            "slot 0 at {k}"
        );
    }
    // and the post-roll is zero to the end
    assert!(s.drive[2 * (lead + 4096)..].iter().all(|x| *x == 0.0));
    assert!(s.frames as usize >= pre + 4096 + 1024);
}

#[test]
fn a_response_abort_zeroes_the_drive_at_once() {
    let fs = 4096.0;
    let dir = temp("abort");
    let folder = write(&dir, fs, &[burst(fs, 2.0, 100.0, 0.5)]);
    // the wire returns the drive as the input: 0.5 V reads 0.5 g against a 0.1 g limit
    let t = table(1, 1, Some(0.1));
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    let (backend, seen) = spy(wire(fs, 1, 1), 1, true);
    let h = PlaybackHost::start(drive, t, backend, SIM, unpaced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "response_abort", "{}", end.message);
    let trip = end.trip.expect("the trip's facts");
    assert_eq!(trip.channel_index, 1);
    assert!(end.played < 8192, "the play did not run on");
    let s = seen.lock().unwrap();
    assert_eq!(
        s.estops.len(),
        1,
        "the engine's watchdog issued the emergency stop"
    );
    assert_eq!(
        s.after_estop_peak, 0.0,
        "nothing but zero after the emergency stop"
    );
    assert_eq!(h.status().phase, Phase::Zeroed);
}

/// A reader that gives `fast` bytes at once and then `per_read` bytes every `pause`.
struct Slow<R> {
    inner: R,
    fast: usize,
    given: usize,
    pause: Duration,
    fail_after: Option<usize>,
}

impl<R: Read> Read for Slow<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(n) = self.fail_after {
            if self.given >= n {
                return Err(std::io::Error::other("the disk went away"));
            }
        }
        let want = if self.given < self.fast {
            buf.len().min(self.fast - self.given)
        } else {
            std::thread::sleep(self.pause);
            buf.len().min(64)
        };
        let n = self.inner.read(&mut buf[..want])?;
        self.given += n;
        Ok(n)
    }
}

#[test]
fn an_underrun_aborts_to_zero() {
    let fs = 2048.0;
    let dir = temp("underrun");
    let folder = write(&dir, fs, &[burst(fs, 10.0, 50.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    // the prefill (2 s) and a little more at once, then a trickle far under the rate
    let reader = Slow {
        inner: drive.file.samples_reader().unwrap(),
        fast: (2.5 * fs) as usize * 8,
        given: 0,
        pause: Duration::from_millis(20),
        fail_after: None,
    };
    let (backend, seen) = spy(wire(fs, 1, 1), 1, false);
    let h =
        PlaybackHost::start_with_reader(drive, Box::new(reader), t, backend, SIM, paced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "drive_underrun", "{}", end.message);
    assert!(end.emergency_stop);
    assert!(end.played < 20_480);
    let s = seen.lock().unwrap();
    assert_eq!(s.estops.len(), 1);
    assert_eq!(s.after_estop_peak, 0.0);
}

#[test]
fn a_reader_stalled_inside_a_read_does_not_hold_up_the_end() {
    let fs = 2048.0;
    let dir = temp("stalled");
    let folder = write(&dir, fs, &[burst(fs, 10.0, 50.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    // the prefill and a little more, then one read that does not return for 20 s
    let reader = Slow {
        inner: drive.file.samples_reader().unwrap(),
        fast: (2.5 * fs) as usize * 8,
        given: 0,
        pause: Duration::from_secs(20),
        fail_after: None,
    };
    let (backend, seen) = spy(wire(fs, 1, 1), 1, false);
    let h =
        PlaybackHost::start_with_reader(drive, Box::new(reader), t, backend, SIM, paced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(10));
    assert_eq!(end.outcome, "drive_underrun", "{}", end.message);
    assert!(end.emergency_stop);
    let s = seen.lock().unwrap();
    assert_eq!(s.estops.len(), 1);
    assert_eq!(s.after_estop_peak, 0.0);
}

#[test]
fn a_reader_thread_failure_ends_in_the_emergency_stop() {
    let fs = 2048.0;
    let dir = temp("readerfail");
    let folder = write(&dir, fs, &[burst(fs, 10.0, 50.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    // past the read-ahead, so the failure comes while the file plays
    let fast = (6.0 * fs) as usize * 8;
    let reader = Slow {
        inner: drive.file.samples_reader().unwrap(),
        fast,
        given: 0,
        pause: Duration::ZERO,
        fail_after: Some(fast),
    };
    let (backend, seen) = spy(wire(fs, 1, 1), 1, false);
    let h =
        PlaybackHost::start_with_reader(drive, Box::new(reader), t, backend, SIM, paced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "reader_failed", "{}", end.message);
    assert!(
        end.message.contains("the disk went away"),
        "{}",
        end.message
    );
    assert!(end.emergency_stop);
    let s = seen.lock().unwrap();
    assert_eq!(s.estops.len(), 1);
    assert_eq!(s.after_estop_peak, 0.0);
}

#[test]
fn a_stop_ramps_the_streamed_drive_down() {
    let fs = 2048.0;
    let dir = temp("stop");
    // a long steady stretch: the burst over 60 s
    let folder = write(&dir, fs, &[burst(fs, 60.0, 40.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    let (backend, seen) = spy(wire(fs, 1, 1), 1, true);
    let opts = HostOptions {
        pacing: Pacing::WallClock { speed: 20.0 },
        ..HostOptions::default()
    };
    let h = PlaybackHost::start(drive, t, backend, SIM, opts).unwrap();
    let began = Instant::now();
    while h.status().played < (25.0 * fs) as u64 {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "the play did not advance"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    h.stop();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "stopped", "{}", end.message);
    assert!(!end.emergency_stop);
    let generated_at = h.status().stop_from.expect("where the ramp began") as usize;
    let s = seen.lock().unwrap();
    // The station plays each generated frame behind the engine's lead-in: the offset is
    // where the file's first non-zero sample lands, less the pre-roll.
    let offset = s.drive.iter().position(|x| *x != 0.0).unwrap() - 1 - (0.25 * fs) as usize;
    let from = generated_at + offset;
    assert!(s.estops.is_empty());
    assert_eq!(s.stops, 1, "the station was stopped once, after the ramp");
    let ramp = (3.0 * fs) as usize;
    let peak = |a: usize, b: usize| s.drive[a..b].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let window = (0.25 * fs) as usize;
    let before = peak(from - window, from);
    let first = peak(from, from + window);
    let last = peak(from + ramp - window, from + ramp);
    // 40 dB down over the ramp: the last quarter second near 1 % of where it began
    assert!(before > 0.3, "{before}");
    assert!(
        first <= before * 1.01 && first > 0.6 * before,
        "{first} {before}"
    );
    assert!(last < 0.02 * before, "{last} {before}");
    assert!(
        s.drive[from + ramp..].iter().all(|x| *x == 0.0),
        "zero after the ramp"
    );
    // never a jump up while the ramp runs
    let mut prev = before;
    for k in (from..from + ramp).step_by(window) {
        let p = peak(k, k + window);
        assert!(p <= prev * 1.05, "the ramp rose at {k}: {p} after {prev}");
        prev = p;
    }
}

#[test]
fn refuses_a_rate_the_station_does_not_run_the_file_at() {
    let fs = 25_600.0;
    let dir = temp("coerce");
    let folder = write(&dir, fs, &[burst(fs, 1.0, 100.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    // a station that programs 25 641 Hz when asked for 25 600
    let (_, seen) = spy(wire(fs, 1, 1), 1, true);
    let coercing: Box<dyn hertzforge_hal::Backend> = Box::new(Spy {
        inner: wire(fs, 1, 1),
        seen: seen.clone(),
        rate: Some(25_641.0),
        width: 1,
    });
    let e = match PlaybackHost::start(drive, t, coercing, SIM, unpaced()) {
        Err(e) => e,
        Ok(_) => panic!("a coerced rate armed"),
    };
    match e {
        HostError::Rate(r) => {
            assert_eq!(r.reason, "rate_mismatch");
            assert!(
                r.message.contains("25600 Hz") && r.message.contains("25641 Hz"),
                "{}",
                r.message
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(seen.lock().unwrap().frames, 0, "nothing was driven");
}

#[test]
fn a_long_file_streams_with_no_underrun() {
    // 60 s at 25.6 kHz on four drives, through a ring of a few seconds: the reader keeps
    // up and the whole file plays.
    let fs = 25_600.0;
    let dir = temp("long");
    let n = (60.0 * fs) as usize;
    let chans: Vec<Vec<f64>> = [37.0, 91.0, 155.0, 233.0]
        .iter()
        .map(|f| burst(fs, 60.0, *f, 0.3))
        .collect();
    let folder = write(&dir, fs, &chans);
    drop(chans);
    assert_eq!(
        std::fs::metadata(folder.join("drive.bin")).unwrap().len(),
        8 + 32 * n as u64
    );
    let t = table(4, 4, None);
    let drive = checked(&folder, &t, setup(vec![5, 6, 7, 8], -3.0, 1.0));
    let (backend, seen) = spy(wire(fs, 4, 4), 4, false);
    let h = PlaybackHost::start(drive, t, backend, SIM, unpaced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(300));
    assert_eq!(end.outcome, "completed", "{}", end.message);
    assert_eq!(end.played, n as u64);
    assert!(seen.lock().unwrap().estops.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_drive_ceiling_holds_on_every_sample() {
    // A drive that got past the checks with a ceiling under its peak (here set after
    // the check, as no setup can): the step zeroes it at the first sample over.
    let fs = 4096.0;
    let dir = temp("ceiling");
    let folder = write(&dir, fs, &[burst(fs, 1.0, 100.0, 0.5)]);
    let t = table(1, 1, None);
    let mut drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    drive.setup.ceiling_v = 0.2;
    let (backend, seen) = spy(wire(fs, 1, 1), 1, true);
    let h = PlaybackHost::start(drive, t, backend, SIM, unpaced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "drive_ceiling", "{}", end.message);
    assert!(end.emergency_stop);
    let s = seen.lock().unwrap();
    assert_eq!(s.estops.len(), 1);
    assert!(
        s.drive.iter().all(|x| x.abs() <= 0.2),
        "nothing over the ceiling left the step"
    );
    assert_eq!(s.after_estop_peak, 0.0);
}

/// A station whose read panics once the file is playing.
struct Panics {
    inner: Box<dyn hertzforge_hal::Backend>,
    reads: u64,
    after: u64,
    estops: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl hertzforge_hal::Backend for Panics {
    fn caps(&self) -> hertzforge_hal::Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        t: &hertzforge_channel_table::ChannelTable,
        a: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(t, a)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        self.reads += 1;
        if self.reads == self.after {
            panic!("a read that panics");
        }
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, d: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        self.inner.write_drive(d)
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.estops
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.emergency_stop()
    }
}

#[test]
fn a_panic_on_the_loop_thread_ends_in_the_emergency_stop() {
    let fs = 4096.0;
    let dir = temp("panic");
    let folder = write(&dir, fs, &[burst(fs, 2.0, 100.0, 0.5)]);
    let t = table(1, 1, None);
    let drive = checked(&folder, &t, setup(vec![2], 0.0, 1.0));
    let estops = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let backend = Box::new(Panics {
        inner: wire(fs, 1, 1),
        reads: 0,
        after: 40,
        estops: estops.clone(),
    });
    let h = PlaybackHost::start(drive, t, backend, SIM, unpaced()).unwrap();
    let end = wait_end(&h, Duration::from_secs(30));
    assert_eq!(end.outcome, "loop_panicked", "{}", end.message);
    assert!(end.emergency_stop);
    assert_eq!(estops.load(std::sync::atomic::Ordering::SeqCst), 1);
}
