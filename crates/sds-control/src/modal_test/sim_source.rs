//! The simulated source a Modal Test reads on the simulator: the station's backend with
//! the structure's ([`hertzforge_modal_test::structure`]) forces and responses written
//! over its reference and response rows. Every run that reads it is labeled SIM.
//!
//! * **Impact mode.** Hits land at fixed station frames: the first
//!   [`FIRST_HIT_S`] after the start, then every [`HIT_PERIOD_S`], whenever the test is
//!   armed. Each is a hammer pulse at the selected point's reference DOF and the
//!   structure's response at every response DOF. A test can ask the kind of the next
//!   hits (clean, double or overloaded) through a small queue.
//! * **Shaker mode.** The service's simulated source solves the force reference for
//!   every shaker: shaker `k`'s force row reads its own drive as the station played it,
//!   times [`structure::SHAKER_LBF_PER_V`], and each response row reads the sum over the
//!   shakers of the structure's impulse response from that shaker's DOF (its force
//!   row's DOF) convolved with its force. Drive and responses meet at the same station
//!   frame.
//!
//! The loop thread takes no lock here: the point, the armed flag and the queued kinds
//! are atomics, and everything the hits need is made before the station opens.

use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_modal_test::plan::Plan;
use hertzforge_modal_test::setup::Mode;
use hertzforge_modal_test::structure::{self, HitKind};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

/// The first simulated hit after the start, and the hits' period, seconds.
pub const FIRST_HIT_S: f64 = 0.5;
pub const HIT_PERIOD_S: f64 = 1.5;
/// The noise on every simulated force and response, as a fraction of its row's range.
pub const NOISE_FRACTION: f64 = 1e-5;
/// The queue of asked kinds.
const KINDS: usize = 64;

/// What the run's thread and the simulated source share.
pub struct SimShared {
    pub point: AtomicUsize,
    pub armed: AtomicBool,
    kinds: [AtomicU8; KINDS],
    head: AtomicU64,
    tail: AtomicU64,
    /// Hits played.
    pub hits: AtomicU64,
}

impl SimShared {
    pub fn new() -> SimShared {
        SimShared {
            point: AtomicUsize::new(0),
            armed: AtomicBool::new(false),
            kinds: std::array::from_fn(|_| AtomicU8::new(0)),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            hits: AtomicU64::new(0),
        }
    }

    /// Queue the kind of the next hit. `false` when the queue is full.
    pub fn queue(&self, k: HitKind) -> bool {
        let t = self.tail.load(Ordering::Acquire);
        if t - self.head.load(Ordering::Acquire) >= KINDS as u64 {
            return false;
        }
        self.kinds[(t % KINDS as u64) as usize].store(code(k), Ordering::Release);
        self.tail.store(t + 1, Ordering::Release);
        true
    }

    fn next_kind(&self) -> HitKind {
        let h = self.head.load(Ordering::Acquire);
        if h == self.tail.load(Ordering::Acquire) {
            return HitKind::Clean;
        }
        let c = self.kinds[(h % KINDS as u64) as usize].load(Ordering::Acquire);
        self.head.store(h + 1, Ordering::Release);
        match c {
            1 => HitKind::Double,
            2 => HitKind::Overload,
            _ => HitKind::Clean,
        }
    }

    /// The kinds still queued.
    pub fn queued(&self) -> u64 {
        self.tail.load(Ordering::Acquire) - self.head.load(Ordering::Acquire)
    }
}

impl Default for SimShared {
    fn default() -> Self {
        SimShared::new()
    }
}

fn code(k: HitKind) -> u8 {
    match k {
        HitKind::Clean => 0,
        HitKind::Double => 1,
        HitKind::Overload => 2,
    }
}

/// One simulated hit: the force and every response.
type Wave = (Vec<f32>, Vec<Vec<f32>>);

/// A hit's waveforms: per point, per kind, the force and every response.
struct Hits {
    /// `[point][kind]`.
    w: Vec<[Wave; 3]>,
}

/// The shaker side: per point, per response, per shaker, the impulse response.
struct Shakers {
    firs: Vec<Vec<Vec<Vec<f64>>>>,
    /// The drive frame's slot of each shaker.
    slots: Vec<usize>,
    outputs: usize,
    /// The force each shaker has made, newest last, `taps` long.
    hist: Vec<VecDeque<f64>>,
    /// Drive frames written and not yet read back, the first of them `base`.
    written: VecDeque<Vec<f64>>,
    base: u64,
    /// The station frames a written drive frame is read back after: one engine block,
    /// so it has always been written by then.
    latency: u64,
}

/// The backend with the simulated structure written over its measured rows.
pub struct SimSource {
    inner: Box<dyn Backend>,
    shared: Arc<SimShared>,
    n_inputs: usize,
    fs_hz: f64,
    ref_slots: Vec<usize>,
    resp_slots: Vec<usize>,
    ranges: Vec<f64>,
    hits: Option<Hits>,
    shakers: Option<Shakers>,
    frame: u64,
    playing: Option<(usize, usize, usize)>,
    overloads: Vec<bool>,
    full_scale: Vec<f64>,
}

impl SimSource {
    /// Wrap `inner` for `plan` at `fs`. `table` is the engine's table.
    pub fn new(
        inner: Box<dyn Backend>,
        plan: &Plan,
        mode: Mode,
        fs_hz: f64,
        table: &ChannelTable,
        shared: Arc<SimShared>,
    ) -> SimSource {
        let taps = ((structure::FIR_S * fs_hz) as usize).next_power_of_two();
        let ref_slots: Vec<usize> = plan.refs.iter().map(|r| r.slot).collect();
        let resp_slots: Vec<usize> = plan.resps.iter().map(|r| r.slot).collect();
        let ranges: Vec<f64> = plan
            .refs
            .iter()
            .chain(&plan.resps)
            .map(|r| r.range)
            .collect();
        let full_scale: Vec<f64> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != hertzforge_channel_table::ChannelRole::Drive)
            .map(|c| c.range_max)
            .collect();
        let (hits, shakers) = match mode {
            Mode::Impact => {
                let len = taps + (0.1 * fs_hz) as usize;
                let range = plan.refs[0].range;
                let w = plan
                    .measurements
                    .iter()
                    .map(|m| {
                        let firs: Vec<Vec<f64>> = m
                            .resps
                            .iter()
                            .map(|y| structure::impulse_response(y, &m.refs[0], fs_hz, taps))
                            .collect();
                        let make = |k: HitKind| {
                            let f = structure::hammer(
                                k,
                                fs_hz,
                                len,
                                0,
                                structure::PULSE_PEAK_LBF,
                                range,
                            );
                            let ys = firs
                                .iter()
                                .map(|h| {
                                    structure::convolve(&f, h, len)
                                        .into_iter()
                                        .map(|v| v as f32)
                                        .collect()
                                })
                                .collect();
                            (f.into_iter().map(|v| v as f32).collect(), ys)
                        };
                        [
                            make(HitKind::Clean),
                            make(HitKind::Double),
                            make(HitKind::Overload),
                        ]
                    })
                    .collect();
                (Some(Hits { w }), None)
            }
            Mode::Shaker => {
                let firs = plan
                    .measurements
                    .iter()
                    .map(|m| {
                        m.resps
                            .iter()
                            .map(|y| {
                                m.refs
                                    .iter()
                                    .map(|b| structure::impulse_response(y, b, fs_hz, taps))
                                    .collect()
                            })
                            .collect()
                    })
                    .collect();
                let n = plan.drives.len();
                (
                    None,
                    Some(Shakers {
                        firs,
                        slots: plan.drives.iter().map(|d| d.slot).collect(),
                        outputs: plan.outputs,
                        hist: vec![VecDeque::from(vec![0.0; taps]); n],
                        written: VecDeque::new(),
                        base: 0,
                        latency: 0,
                    }),
                )
            }
        };
        SimSource {
            inner,
            shared,
            n_inputs: full_scale.len().max(1),
            fs_hz,
            ref_slots,
            resp_slots,
            ranges,
            hits,
            shakers,
            frame: 0,
            playing: None,
            overloads: vec![false; full_scale.len()],
            full_scale,
        }
    }

    /// A small deterministic noise for frame `f`, channel `c`, in `(-1, 1)`.
    fn noise(f: u64, c: usize) -> f64 {
        let mut z = f
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((c as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9));
        z = (z ^ (z >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // The sum of two uniforms: a triangle in (-1, 1).
        let a = (z >> 40) as f64 / (1u64 << 24) as f64;
        let b = ((z >> 16) & 0xFF_FFFF) as f64 / (1u64 << 24) as f64;
        a + b - 1.0
    }

    fn impact_frame(&mut self, row: &mut [f32]) {
        let Some(h) = self.hits.as_ref() else {
            return;
        };
        let first = (FIRST_HIT_S * self.fs_hz).round() as u64;
        let period = (HIT_PERIOD_S * self.fs_hz).round() as u64;
        if self.playing.is_none()
            && self.frame >= first
            && (self.frame - first).is_multiple_of(period)
            && self.shared.armed.load(Ordering::Acquire)
        {
            let p = self.shared.point.load(Ordering::Acquire).min(h.w.len() - 1);
            let k = code(self.shared.next_kind()) as usize;
            self.playing = Some((p, k, 0));
            self.shared.hits.fetch_add(1, Ordering::AcqRel);
        }
        if let Some((p, k, at)) = self.playing {
            let (f, ys) = &h.w[p][k];
            if let Some(s) = self.ref_slots.first().and_then(|s| row.get_mut(*s)) {
                *s = f[at];
            }
            for (j, slot) in self.resp_slots.iter().enumerate() {
                if let Some(s) = row.get_mut(*slot) {
                    *s = ys[j][at];
                }
            }
            self.playing = (at + 1 < f.len()).then_some((p, k, at + 1));
        } else {
            for s in self.ref_slots.iter().chain(&self.resp_slots) {
                if let Some(v) = row.get_mut(*s) {
                    *v = 0.0;
                }
            }
        }
    }

    fn shaker_frame(&mut self, row: &mut [f32]) {
        let p = self.shared.point.load(Ordering::Acquire);
        let Some(sh) = self.shakers.as_mut() else {
            return;
        };
        // The drive frame written `latency` frames ago, or zero before it.
        let want = self.frame.checked_sub(sh.latency);
        let mut d = Vec::new();
        if let Some(want) = want {
            while sh.base < want && sh.written.pop_front().is_some() {
                sh.base += 1;
            }
            if sh.base == want {
                if let Some(x) = sh.written.pop_front() {
                    d = x;
                    sh.base += 1;
                }
            }
        }
        for (k, slot) in sh.slots.iter().enumerate() {
            let v = d.get(*slot).copied().unwrap_or(0.0) * structure::SHAKER_LBF_PER_V;
            sh.hist[k].pop_front();
            sh.hist[k].push_back(v);
            if let Some(s) = self.ref_slots.get(k).and_then(|s| row.get_mut(*s)) {
                *s = v as f32;
            }
        }
        let firs = &sh.firs[p.min(sh.firs.len() - 1)];
        for (j, slot) in self.resp_slots.iter().enumerate() {
            let mut y = 0.0;
            for (k, h) in firs[j].iter().enumerate() {
                let x = &sh.hist[k];
                let n = x.len();
                for (t, hv) in h.iter().enumerate() {
                    y += hv * x[n - 1 - t];
                }
            }
            if let Some(s) = row.get_mut(*slot) {
                *s = y as f32;
            }
        }
    }
}

impl Backend for SimSource {
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
        let a = self.inner.configure(table, acq)?;
        self.fs_hz = a.actual_rate_hz;
        if let Some(sh) = self.shakers.as_mut() {
            sh.latency = a.block_frames as u64;
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
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let read = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = read {
            let w = self.n_inputs;
            for f in 0..n {
                let Some(row) = out.get_mut(f * w..(f + 1) * w) else {
                    break;
                };
                if self.hits.is_some() {
                    self.impact_frame(row);
                } else {
                    self.shaker_frame(row);
                }
                for (c, slot) in self.ref_slots.iter().chain(&self.resp_slots).enumerate() {
                    if let Some(v) = row.get_mut(*slot) {
                        *v += (NOISE_FRACTION * self.ranges[c] * Self::noise(self.frame, c)) as f32;
                    }
                }
                self.frame += 1;
            }
            let block = &out[..(n * w).min(out.len())];
            hertzforge_hal::mark_overloads(block, &self.full_scale, &mut self.overloads);
        }
        Ok(read)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        if let Some(sh) = self.shakers.as_mut() {
            let w = sh.outputs.max(1);
            for f in 0..n {
                sh.written.push_back(
                    drive[f * w..((f + 1) * w).min(drive.len())]
                        .iter()
                        .map(|v| f64::from(*v))
                        .collect(),
                );
            }
        }
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.overloads.clone()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}
