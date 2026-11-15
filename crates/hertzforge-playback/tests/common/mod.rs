//! What the Playback tests share: drive files in a temporary folder, the tables, a
//! station that records what it was asked, and the setup a run arms with.
#![allow(dead_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_playback::file::write_folder;
use hertzforge_playback::{check_file, CheckedDrive, DriveFile, DriveRow, PlaySetup, StationOffer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static N: AtomicU64 = AtomicU64::new(0);

/// A fresh folder under the system's temporary folder (never the repository).
pub fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "hf-playback-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A burst whose displacement is `sin²(πt/T) sin(2πft)`, differentiated twice: zero
/// acceleration, velocity and displacement at both ends. Scaled to `peak` volts.
pub fn burst(fs: f64, seconds: f64, f_hz: f64, peak: f64) -> Vec<f64> {
    let n = (fs * seconds).round() as usize;
    let t_len = n as f64 / fs;
    // a whole number of cycles, so the tone is zero at both ends
    let cycles = (f_hz * t_len).round().max(1.0);
    let w = 2.0 * std::f64::consts::PI * cycles / t_len;
    let p = std::f64::consts::PI / t_len;
    let mut a: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / fs;
            let h = (p * t).sin().powi(2);
            let h1 = p * (2.0 * p * t).sin();
            let h2 = 2.0 * p * p * (2.0 * p * t).cos();
            let (s, c) = (w * t).sin_cos();
            h2 * s + 2.0 * h1 * w * c - h * w * w * s
        })
        .collect();
    let m = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    a.iter_mut().for_each(|x| *x *= peak / m);
    a
}

/// Write a drive folder of `channels` (each a column), all at `fs`.
pub fn write(dir: &std::path::Path, fs: f64, channels: &[Vec<f64>]) -> PathBuf {
    let folder = dir.join("drive");
    let names: Vec<String> = (0..channels.len()).map(|k| format!("d{k}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let n = channels[0].len();
    let mut s = Vec::with_capacity(n * channels.len());
    for i in 0..n {
        for c in channels {
            s.push(c[i] as f32);
        }
    }
    write_folder(&folder, fs, &names, &s, serde_json::Map::new()).unwrap();
    folder
}

/// `inputs` control rows (1..) in g with `accel_limit`, then `drives` drive rows.
pub fn table(inputs: u32, drives: u32, accel_limit: Option<f64>) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..inputs {
        let mut c = Channel::new(k + 1);
        c.role = ChannelRole::Control;
        c.units = Units::AccelG;
        c.range_max = 50.0;
        c.abort.accel_g = accel_limit;
        t.channels.push(c);
    }
    for k in 0..drives {
        let mut d = Channel::new(inputs + k + 1);
        d.role = ChannelRole::Drive;
        d.units = Units::VoltageV;
        d.range_max = 10.0;
        t.channels.push(d);
    }
    t
}

pub fn rows(t: &ChannelTable) -> Vec<DriveRow> {
    t.channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Drive)
        .map(|c| DriveRow {
            index: c.index,
            range_max: c.range_max,
        })
        .collect()
}

pub fn setup(map: Vec<u32>, level_db: f64, ceiling_v: f64) -> PlaySetup {
    PlaySetup {
        level_db,
        ceiling_v,
        pre_roll_s: 0.25,
        post_roll_s: 0.25,
        map,
    }
}

/// Open, check and judge: the drive a run arms with.
pub fn checked(folder: &std::path::Path, t: &ChannelTable, s: PlaySetup) -> CheckedDrive {
    let file = DriveFile::open(folder).unwrap();
    let check = check_file(&file, Some(&file.sha256.clone())).unwrap();
    CheckedDrive::new(
        file,
        check,
        s,
        &rows(t),
        &StationOffer {
            rate_hz: None,
            nearest_hz: Vec::new(),
            outputs: 8,
        },
    )
    .unwrap()
}

/// A wire: no resonance, unity feedthrough, no noise.
pub fn wire(fs: f64, inputs: u32, drives: u32) -> Box<dyn Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(fs)
            .resonances(Vec::new())
            .feedthrough(1.0)
            .noise_level(0.0)
            .transport_delay_samples(0)
            .max_input_channels(inputs)
            .max_output_channels(drives)
            .build(),
    )
}

/// What a [`Spy`] saw.
#[derive(Default)]
pub struct Seen {
    /// Every drive frame accepted, interleaved (when `keep`).
    pub drive: Vec<f32>,
    pub keep: bool,
    pub frames: u64,
    /// The drive frames accepted before each emergency stop.
    pub estops: Vec<u64>,
    pub stops: u32,
    /// The largest drive magnitude accepted after the first emergency stop.
    pub after_estop_peak: f32,
}

/// A station that passes everything through and records what it was asked; with
/// `rate` it says it programmed that rate instead.
pub struct Spy {
    pub inner: Box<dyn Backend>,
    pub seen: Arc<Mutex<Seen>>,
    pub rate: Option<f64>,
    pub width: usize,
}

pub fn spy(
    inner: Box<dyn Backend>,
    width: usize,
    keep: bool,
) -> (Box<dyn Backend>, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen {
        keep,
        ..Seen::default()
    }));
    (
        Box::new(Spy {
            inner,
            seen: seen.clone(),
            rate: None,
            width,
        }),
        seen,
    )
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
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let mut a = self.inner.configure(table, acq)?;
        if let Some(r) = self.rate {
            a.actual_rate_hz = r;
        }
        Ok(a)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.seen.lock().unwrap().stops += 1;
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        let mut s = self.seen.lock().unwrap();
        let taken = &drive[..n * self.width];
        if s.keep {
            s.drive.extend_from_slice(taken);
        }
        if !s.estops.is_empty() {
            let m = taken.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            s.after_estop_peak = s.after_estop_peak.max(m);
        }
        s.frames += n as u64;
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
        let f = s.frames;
        s.estops.push(f);
        drop(s);
        self.inner.emergency_stop()
    }
}
