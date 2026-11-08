//! The simulated station the core's run tests share: `sim_e2e`'s three-mode
//! plant and field-like reference, and a wrapper that changes the plant after
//! the identification (a gain step, a sign flip, a moved delay), the way a
//! fixture, a cable or a loose mount would.

#![allow(dead_code)]

use std::collections::VecDeque;

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{BackendReferencePlayer, ReferenceWaveform, RunSchedule};

pub const FS: f64 = 8192.0;

/// `sim_e2e`'s field-like transient: three decaying tones and a scattered
/// multi-sine floor, 0.75 s, peak 1 g.
pub fn field_like_reference() -> ReferenceWaveform {
    let n = 6144usize;
    let mut s = vec![0.0f64; n];
    let tones: [(f64, f64, f64, f64); 3] = [
        (160.0, 1.00, 0.18, 0.3),
        (400.0, 0.65, 0.10, 1.7),
        (750.0, 0.35, 0.06, 4.1),
    ];
    for (f, a, tau, ph) in tones {
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / FS;
            *v += a * (-t / tau).exp() * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    for k in 0..15 {
        let f = 60.0 + 70.0 * k as f64;
        let ph = (k as f64) * 2.399963;
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / FS;
            *v += 0.04 * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    let peak = s.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    for v in s.iter_mut() {
        *v /= peak;
    }
    ReferenceWaveform::from_samples("synthetic field transient", FS, s).unwrap()
}

/// `sim_e2e`'s plant: three modes, feedthrough, a noise floor and a 24-sample
/// transport delay, on a fixed seed.
pub fn sim_backend() -> SimBackend {
    SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(vec![(180.0, 8.0), (420.0, 10.0), (950.0, 6.0)])
        .feedthrough(0.25)
        .noise_level(2e-4)
        .transport_delay_samples(24)
        .shared_clock(true)
        .seed(0x71E5_1DE2_E001_0001)
        .build()
}

pub fn table() -> ChannelTable {
    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    table.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    table.channels.push(drv);
    table
}

pub fn player<B: Backend>(backend: B) -> BackendReferencePlayer<B> {
    BackendReferencePlayer::new(
        backend,
        table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
    .with_quiet_frames(1024)
}

/// `sim_e2e`'s configuration, the correction law at its defaults.
pub fn config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(field_like_reference());
    c.prep.highpass_hz = Some(20.0);
    c.prep.taper_frac = Some(0.05);
    c.pad_pre = 512;
    c.pad_post = 1024;
    c.sysid.drive_rms = 0.15;
    c.sysid.duration_s = 8.0;
    c.sysid.nfft = 2048;
    c.sysid.f_lo_hz = 25.0;
    c.sysid.f_hi_hz = 1500.0;
    c.inverse.tikhonov_beta_floor = 0.02;
    c.inverse.coherence_floor = 0.6;
    c.law.initial_drive_backoff_db = 6.0;
    c.law.ref_floor_db = 50.0;
    c.law.f_lo_hz = 25.0;
    c.law.f_hi_hz = 1400.0;
    c.law.min_masked_energy_frac = 0.98;
    c.schedule.gate_rms_error_pct = 10.0;
    c.schedule.max_iterations_per_level = 10;
    c.run = RunSchedule::LoopN(2);
    c
}

/// A station whose plant changes once the identification has played: from
/// the `after`-th play on (the identification is play 1), every input is
/// scaled by `gain` and the control channel arrives `extra_delay` frames
/// later than before.
pub struct Twisted<B: Backend> {
    pub inner: B,
    starts: usize,
    after: usize,
    gain: f32,
    extra_delay: usize,
    /// Samples held back by the extra delay, per play.
    held: VecDeque<f32>,
}

impl<B: Backend> Twisted<B> {
    pub fn new(inner: B, after: usize, gain: f32, extra_delay: usize) -> Self {
        Twisted {
            inner,
            starts: 0,
            after,
            gain,
            extra_delay,
            held: VecDeque::new(),
        }
    }

    fn twisted(&self) -> bool {
        self.starts > self.after
    }
}

impl<B: Backend> Backend for Twisted<B> {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.starts += 1;
        self.held.clear();
        if self.twisted() {
            self.held
                .extend(std::iter::repeat_n(0.0f32, self.extra_delay));
        }
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            if self.twisted() {
                for v in out.iter_mut().take(n) {
                    self.held.push_back(*v * self.gain);
                    *v = self.held.pop_front().unwrap_or(0.0);
                }
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
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
        self.inner.emergency_stop()
    }
}
