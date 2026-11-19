//! The rate the station runs a TWR run at is the rate the run ASKED, or the run
//! is refused at arm.
//!
//! Bench T0 (2026-11-08, NI loopback): a run asked 12 800 Hz of an NI station
//! whose caps state the chassis's default, 51 200 Hz, and came up at 51 200 Hz.
//! The player configured the station at the caps rate, the service prepared the
//! record at that rate (resampling the 12 800 Hz reference up), and the arm
//! gate compared the record's rate with the rate the station adopted - the
//! same number twice - so nothing refused. The simulator never showed it,
//! because its station is built at the asked rate.
//!
//! These build the simulated station at 51 200 Hz, as the NI station is built,
//! and ask 12 800 Hz of it (the simulator can run at either, as the 9234 runs at
//! 51 200 / 4).
//!
//! RED-MUTATION: put `self.backend.caps().sample_rate_hz` back as the request in
//! `BackendReferencePlayer::open_and_configure` and the first test reds (the
//! station runs at 51 200 Hz under a record prepared at 12 800 Hz, and the rate
//! policy refuses the arm).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_twr::player::ReferencePlayer;
use hertzforge_twr::schedule::Section;
use hertzforge_twr::{BackendReferencePlayer, ReferenceWaveform, TwrConfig, TwrService, TwrState};

/// The rate the NI station's caps state before any configure.
const DEFAULT_HZ: f64 = 51_200.0;
/// The rate the run asks (bench T0's).
const ASKED_HZ: f64 = 12_800.0;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    t.channels.push(drv);
    t
}

/// A simulated station BUILT at `hz`: its caps state `hz` until a configure says
/// otherwise.
fn station_built_at(hz: f64) -> SimBackend {
    SimConfig::new()
        .sample_rate_hz(hz)
        .resonances(vec![(180.0, 8.0), (420.0, 10.0)])
        .feedthrough(0.3)
        .noise_level(2e-4)
        .shared_clock(true)
        .seed(0x5A7E_0012_8000_0001)
        .build()
}

/// A 0.5 s reference at the asked rate: two decaying tones.
fn reference() -> ReferenceWaveform {
    let n = (ASKED_HZ / 2.0) as usize;
    let s = (0..n)
        .map(|i| {
            let t = i as f64 / ASKED_HZ;
            0.5 * (-t / 0.15).exp() * (std::f64::consts::TAU * 250.0 * t).sin()
                + 0.3 * (-t / 0.08).exp() * (std::f64::consts::TAU * 620.0 * t + 0.7).sin()
        })
        .collect();
    ReferenceWaveform::from_samples("two tones", ASKED_HZ, s).unwrap()
}

fn config(section: Option<Section>) -> TwrConfig {
    let mut c = TwrConfig::for_reference(reference());
    c.section = section;
    c.sysid.duration_s = 1.0;
    c.sysid.nfft = 512;
    c.sysid.drive_rms = 0.1;
    c
}

/// A station that runs only at its native rate, whatever it is asked, and counts
/// what it is given to play.
struct NativeRateOnly<B: Backend> {
    inner: B,
    native_hz: f64,
    starts: Arc<AtomicUsize>,
    written: Arc<AtomicUsize>,
}

impl<B: Backend> Backend for NativeRateOnly<B> {
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
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(
            table,
            &AcquisitionConfig::new(self.native_hz, acq.block_frames),
        )
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.written.fetch_add(drive.len(), Ordering::SeqCst);
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}

fn player<B: Backend>(backend: B) -> BackendReferencePlayer<B> {
    BackendReferencePlayer::new(
        backend,
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
}

/// **The station runs at the rate asked, and the record, its section and the grid
/// are at that rate.**
#[test]
fn a_station_built_at_its_default_rate_runs_at_the_requested_rate() {
    let section = Section {
        start: 640,
        len: 4096,
    };
    let p = player(station_built_at(DEFAULT_HZ)).with_requested_rate_hz(ASKED_HZ);
    assert_eq!(p.fs_hz(), ASKED_HZ, "the request is the player's rate");
    let mut svc = TwrService::new(p, config(Some(section))).unwrap();

    // The record, prepared before anything is opened, at the asked rate: the
    // reference is not resampled and the section is the configured samples.
    let g = svc.grid().clone();
    assert_eq!((g.fs_hz, g.section_len, g.pad_pre), (ASKED_HZ, 4096, 512));
    assert_eq!(g.n, 8192, "next power of two over 4096 + 512 + 1024");
    assert_eq!(svc.prepared_reference().fs_hz, ASKED_HZ);
    assert_eq!(
        svc.prepared_reference().samples.len(),
        reference().samples.len()
    );
    assert_eq!(svc.status().requested_fs_hz, ASKED_HZ);
    assert_eq!(svc.status().actual_fs_hz, None, "nothing is up yet");

    // Up: the station adopted the asked rate, and nothing was played to get there.
    svc.arm_station()
        .expect("the station can run at the asked rate");
    assert_eq!(svc.status().actual_fs_hz, Some(ASKED_HZ));
    assert_eq!(
        svc.station_frames(),
        0,
        "bringing the station up plays nothing"
    );
    assert_eq!(svc.state(), TwrState::Configured);
    svc.arm_station().expect("idempotent once up");

    // The identification clocks one second at the asked rate (and its rest pad);
    // at the default rate it would be 51 200 frames and more.
    svc.advance().expect("the identification runs");
    let clocked = svc.station_frames();
    assert!(
        (12_800..2 * 12_800).contains(&clocked),
        "the identification clocked {clocked} frames"
    );
    assert_eq!(svc.status().actual_fs_hz, Some(ASKED_HZ));
    assert_eq!(svc.grid(), &g, "the grid never moves");
}

/// Without a request the player asks for the caps rate, as every caller that
/// states none always has (the daemon host builds its station at the record's rate,
/// the module sweep at the chassis's).
#[test]
fn without_a_request_the_player_asks_for_the_caps_rate() {
    let p = player(station_built_at(ASKED_HZ));
    assert_eq!(p.fs_hz(), ASKED_HZ);
    let mut svc = TwrService::new(p, config(None)).unwrap();
    svc.arm_station().unwrap();
    assert_eq!(svc.status().actual_fs_hz, Some(ASKED_HZ));
    assert_eq!(svc.grid().fs_hz, ASKED_HZ);
}

/// **A station that cannot run at the asked rate is refused at arm, by the rate
/// policy, before anything plays** - and the run is never re-gridded at the rate
/// the station can run at, with section counts that meant another rate.
#[test]
fn a_station_that_cannot_run_at_the_requested_rate_is_refused_before_anything_plays() {
    let starts = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicUsize::new(0));
    let station = NativeRateOnly {
        inner: station_built_at(DEFAULT_HZ),
        native_hz: DEFAULT_HZ,
        starts: starts.clone(),
        written: written.clone(),
    };
    let section = Section {
        start: 640,
        len: 4096,
    };
    let mut svc = TwrService::new(
        player(station).with_requested_rate_hz(ASKED_HZ),
        config(Some(section)),
    )
    .unwrap();
    let grid = svc.grid().clone();

    let text = svc
        .arm_station()
        .expect_err("the station adopts 51 200 Hz for a record at 12 800 Hz")
        .to_string();
    assert!(text.contains("arm refused"), "{text}");
    assert!(
        text.contains("12800") && text.contains("51200"),
        "both rates: {text}"
    );
    assert!(
        text.contains("never resamples"),
        "the rate policy's gross band: {text}"
    );
    // The identification brings the path up itself when nothing has; it is
    // refused the same way.
    let again = svc.advance().expect_err("still refused").to_string();
    assert!(again.contains("arm refused"), "{again}");

    assert_eq!(svc.grid(), &grid, "never re-gridded at the station's rate");
    assert_eq!(svc.grid().fs_hz, ASKED_HZ);
    assert_eq!(svc.state(), TwrState::Configured);
    assert_eq!(svc.station_frames(), 0);
    assert_eq!(
        (
            starts.load(Ordering::SeqCst),
            written.load(Ordering::SeqCst)
        ),
        (0, 0),
        "nothing played"
    );

    // The player alone, with no record stated, refuses too: the request is a
    // claim of its own.
    let mut alone = player(NativeRateOnly {
        inner: station_built_at(DEFAULT_HZ),
        native_hz: DEFAULT_HZ,
        starts: starts.clone(),
        written: written.clone(),
    })
    .with_requested_rate_hz(ASKED_HZ);
    let text = alone
        .arm()
        .expect_err("asked 12 800 Hz, adopted 51 200 Hz")
        .to_string();
    assert!(text.contains("arm refused"), "{text}");
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}
