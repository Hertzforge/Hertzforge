//! The real-time step (design section 3, steps 1 and 2): one frame at a time,
//! the random part from the ring (`m` values a frame, at full level, already
//! clipped) times the level and the random part's ramp, every tone from the
//! bank, the stop or abort ramp over both; each drive's sum judged against
//! its own peak abort, then clamped at its peak ceiling as the last backstop.
//! The trackers read every control and limit channel and give the residual;
//! the residual, the raw samples, the drive played and the random gain are
//! handed to the loop side in preallocated buffers. Every control tick the
//! part clock moves and each tone's law is called (on this thread, as MIMO
//! Sine's core calls its law) under MIMO Sine's guards, then the ceiling rule
//! shares each drive's headroom among the tones.
//!
//! [`MimoMixedStep::process`] does not allocate and takes no lock.

use super::records::{LawRecord, RecordRole};
use crate::budget::{headroom, CeilingRule};
use crate::tone::{tone_readable, Ramp, ToneBank, ToneJudging};
use hertzforge_engine::ProcessStep;
use hertzforge_mimo_sine::guards::{cap_drives, first_drive_scale, DivergenceWatch};
use hertzforge_mimo_sine::law::{LawValues, MimoSineLaw};
use hertzforge_mimo_sine::setup::GuardSettings;
use hertzforge_mimo_sine::tick::{check_outputs, Hold, LawFault, TickInputs, TickOutputs};
use hertzforge_mixed::PartClock;
use hertzforge_random::service::engine::KinematicMonitor;
use hertzforge_sdm::C64;
use hertzforge_sine::law::LawClock;
use std::sync::Arc;

/// The abort ramp, seconds (single-output Mixed Mode's).
pub const ABORT_RAMP_S: f64 = 0.02;
/// The random part's held peak decays with this time constant, seconds
/// (single-output Mixed Mode's).
pub const RANDOM_PEAK_RELEASE_S: f64 = 1.0;
/// A tone's reading must stay outside its abort band this long, seconds
/// (single-output Mixed Mode's).
pub const TONE_ABORT_DEBOUNCE_S: f64 = 0.5;

/// Why the step tripped (latched; the service reads it between batches).
#[derive(Debug, Clone, PartialEq)]
pub enum StepTrip {
    /// A drive's demanded sum reached its peak abort.
    DriveVpk {
        /// The drive's index.
        drive: usize,
        /// The demanded value, volts.
        volts: f64,
    },
    /// A control or limit channel's input reached its overload level.
    Overload {
        /// The channel's index in the table.
        channel_index: u32,
        /// The sample.
        value: f64,
    },
    /// The kinematic monitor tripped.
    Kinematic {
        /// The channel's index in the table.
        channel_index: u32,
        /// The value.
        value: f64,
        /// Its limit.
        limit: f64,
    },
    /// A tone's reading stayed outside its abort band.
    ToneAbort {
        /// The tone's index among the tones.
        tone: usize,
        /// The control.
        control: usize,
        /// The judged error, dB.
        error_db: f64,
        /// Whether the tone was held by the shared ceiling.
        limiting: bool,
    },
    /// A tone's law faulted (at engage, or too many misses running).
    ToneLaw {
        /// The tone's index among the tones.
        tone: usize,
        /// The fault, in words.
        fault: String,
    },
    /// A tone's law kept asking past the change caps.
    ToneDivergence {
        /// The tone's index among the tones.
        tone: usize,
    },
}

/// A fixed-size ring of drive frames (`m` values each).
#[derive(Debug, Clone)]
pub(crate) struct Ring {
    buf: Vec<f64>,
    m: usize,
    cap: usize,
    head: usize,
    len: usize,
}

impl Ring {
    pub(crate) fn new(m: usize, cap_frames: usize) -> Self {
        Ring {
            buf: vec![0.0; m * cap_frames.max(1)],
            m,
            cap: cap_frames.max(1),
            head: 0,
            len: 0,
        }
    }

    pub(crate) fn frames(&self) -> usize {
        self.len
    }

    pub(crate) fn space(&self) -> usize {
        self.cap - self.len
    }

    /// Push whole frames; returns the frames taken.
    pub(crate) fn push(&mut self, frames: &[f64]) -> usize {
        let m = self.m;
        let k = (frames.len() / m).min(self.space());
        for f in 0..k {
            let at = (self.head + self.len) % self.cap;
            self.buf[at * m..(at + 1) * m].copy_from_slice(&frames[f * m..(f + 1) * m]);
            self.len += 1;
        }
        k
    }

    #[inline]
    fn pop(&mut self, out: &mut [f64]) -> bool {
        if self.len == 0 {
            return false;
        }
        let m = self.m;
        out[..m].copy_from_slice(&self.buf[self.head * m..(self.head + 1) * m]);
        self.head = (self.head + 1) % self.cap;
        self.len -= 1;
        true
    }

    /// Each drive's largest magnitude over the next `frames` frames.
    fn peak_ahead(&self, frames: usize, out: &mut [f64]) {
        out.fill(0.0);
        let m = self.m;
        for f in 0..frames.min(self.len) {
            let at = (self.head + f) % self.cap;
            for (j, o) in out.iter_mut().enumerate().take(m) {
                *o = o.max(self.buf[at * m + j].abs());
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

/// What the step hands the loop side, frame by frame, in preallocated
/// buffers (a frame that does not fit is dropped and counted).
#[derive(Debug, Clone)]
pub(crate) struct StepRecords {
    /// Controls then limits, per frame.
    pub(crate) residual: Vec<f64>,
    pub(crate) raw: Vec<f64>,
    /// The drive played, volts.
    pub(crate) drive: Vec<f64>,
    /// The random part's gain at drive time (level, random ramp and stop
    /// ramp together).
    pub(crate) gain: Vec<f64>,
    pub(crate) dropped: u64,
    /// The tone laws' call records since the loop side last took them.
    pub(crate) law: Vec<LawRecord>,
}

impl StepRecords {
    fn new(q: usize, m: usize, frames: usize) -> Self {
        StepRecords {
            residual: Vec::with_capacity(q * frames),
            raw: Vec::with_capacity(q * frames),
            drive: Vec::with_capacity(m * frames),
            gain: Vec::with_capacity(frames),
            dropped: 0,
            law: Vec::with_capacity(4096),
        }
    }

    /// Frames held.
    pub(crate) fn frames(&self) -> usize {
        self.gain.len()
    }

    pub(crate) fn clear(&mut self) {
        self.residual.clear();
        self.raw.clear();
        self.drive.clear();
        self.gain.clear();
    }
}

/// One tone's control state.
pub(crate) struct ToneCtl {
    /// Its index in the parts list.
    pub(crate) part: usize,
    /// Its reference on the grid, control-major (`c * lines + l`), at full
    /// level, peak.
    pub(crate) reference: Vec<C64>,
    /// Its feed-forward drive on the grid, line-major (`l * m + j`):
    /// `Z(f) R(f)`.
    pub(crate) ff: Vec<C64>,
    pub(crate) law: Box<dyn MimoSineLaw + Send>,
    pub(crate) values: LawValues,
    pub(crate) out: TickOutputs,
    pub(crate) engaged: bool,
    pub(crate) calls: u64,
    pub(crate) misses: u32,
    pub(crate) watch: DivergenceWatch,
    pub(crate) limiting: bool,
    /// Seconds left of the limiting mark's hold: a reading trails a cut by
    /// the tracker's delay and the judging average, so a tone stays marked
    /// that long after the ceiling last held it.
    pub(crate) limited_s: f64,
    /// Per control.
    pub(crate) judging: Vec<ToneJudging>,
    pub(crate) ms: Vec<f64>,
    pub(crate) err_db: Vec<f64>,
    pub(crate) alarm: Vec<bool>,
    pub(crate) abort_for_s: Vec<f64>,
    /// Per control: how long the averaged reading has averaged, seconds (0:
    /// it starts afresh at the next reading).
    pub(crate) avg_s: Vec<f64>,
    /// What the tone asks this tick, full level, before the ceiling rule.
    pub(crate) ask: Vec<C64>,
    /// The part's on/off target (0 or 1).
    pub(crate) target: f64,
    pub(crate) f_prev: f64,
    pub(crate) usable: bool,
    pub(crate) scale: f64,
}

/// The real-time step.
pub struct MimoMixedStep {
    pub(crate) fs: f64,
    pub(crate) n: usize,
    pub(crate) m: usize,
    pub(crate) p: usize,
    n_in: usize,
    /// Input slots of the controls then the limits.
    slots: Vec<usize>,
    /// The station's output frame width (every enabled drive row) and each
    /// drive's column in it.
    n_out: usize,
    out_slots: Vec<usize>,
    /// Overload watch on control and limit slots: (slot, level, channel).
    pub(crate) overload: Vec<(usize, f64, u32)>,
    pub(crate) kin: Option<KinematicMonitor>,
    kin_scratch: Vec<f64>,
    pub(crate) ring: Ring,
    pub(crate) underruns: u64,
    pub(crate) bank: ToneBank,
    pub(crate) tones: Vec<ToneCtl>,
    pub(crate) clock: PartClock,
    pub(crate) rule: Box<dyn CeilingRule>,
    pub(crate) guards: GuardSettings,
    law_clock: Arc<dyn LawClock>,
    pub(crate) law_budget_s: f64,
    /// The level as an amplitude fraction.
    pub(crate) level: Ramp,
    /// The random part's own ramp (the floor part's on/off and the
    /// identification's ramp).
    pub(crate) rand_ramp: Ramp,
    /// The stop or abort ramp, 1 to 0.
    pub(crate) stop: Ramp,
    /// Each drive's peak ceiling (the abort as demanded) and clamp, volts.
    pub(crate) peak_v: Vec<f64>,
    pub(crate) abort_v: Vec<f64>,
    pub(crate) trip: Option<StepTrip>,
    pub(crate) clamped: u64,
    pub(crate) records: StepRecords,
    /// Frames since the step was built.
    pub(crate) frame: u64,
    pub(crate) tick_frames: u64,
    pub(crate) tick_s: f64,
    /// Whether the parts may play (the floor has equalized).
    pub(crate) parts_live: bool,
    pub(crate) part_ramp_frames: f64,
    /// Whether the tone laws run (false during the identification).
    pub(crate) tones_run: bool,
    /// The floor's autospectra at full level, line-major (`l * n + i`).
    floor_diag: Vec<f64>,
    df: f64,
    lines: usize,
    held_peak: Vec<f64>,
    // Scratch.
    ch: Vec<f64>,
    res: Vec<f64>,
    rnd: Vec<f64>,
    d: Vec<f64>,
    y: Vec<C64>,
    refv: Vec<C64>,
    dif: Vec<C64>,
    prop: Vec<C64>,
    req: Vec<C64>,
    inf: Vec<f64>,
    room: Vec<f64>,
    scales: Vec<f64>,
    limiting: Vec<bool>,
    rpk: Vec<f64>,
    rpk_full: Vec<f64>,
    bounds: Vec<Option<f64>>,
    freq: Vec<Option<f64>>,
    on: Vec<bool>,
}

/// How the step is built.
pub(crate) struct StepShape {
    pub(crate) fs: f64,
    pub(crate) n: usize,
    pub(crate) m: usize,
    pub(crate) p: usize,
    pub(crate) n_in: usize,
    pub(crate) slots: Vec<usize>,
    pub(crate) n_out: usize,
    pub(crate) out_slots: Vec<usize>,
    pub(crate) peak_v: Vec<f64>,
    pub(crate) tick_s: f64,
    pub(crate) ring_frames: usize,
    pub(crate) record_frames: usize,
    pub(crate) max_block: usize,
    pub(crate) floor_diag: Vec<f64>,
    pub(crate) df: f64,
    pub(crate) lines: usize,
    pub(crate) part_ramp_s: f64,
}

impl MimoMixedStep {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        s: StepShape,
        bank: ToneBank,
        tones: Vec<ToneCtl>,
        clock: PartClock,
        rule: Box<dyn CeilingRule>,
        guards: GuardSettings,
        law_clock: Arc<dyn LawClock>,
    ) -> Self {
        let k = tones.len();
        let (n, m, p) = (s.n, s.m, s.p);
        let q = n + p;
        let tick_frames = ((s.tick_s * s.fs).round() as u64).max(1);
        let tick_s = tick_frames as f64 / s.fs;
        let parts = clock.now().len();
        MimoMixedStep {
            fs: s.fs,
            n,
            m,
            p,
            n_in: s.n_in,
            slots: s.slots,
            n_out: s.n_out.max(1),
            out_slots: s.out_slots,
            overload: Vec::new(),
            kin: None,
            kin_scratch: Vec::with_capacity(s.n_in * s.max_block),
            ring: Ring::new(m, s.ring_frames),
            underruns: 0,
            bank,
            tones,
            clock,
            rule,
            law_budget_s: guards.law_budget * tick_s,
            guards,
            law_clock,
            level: Ramp::at(0.0),
            rand_ramp: Ramp::at(0.0),
            stop: Ramp::at(1.0),
            abort_v: s.peak_v.clone(),
            peak_v: s.peak_v,
            trip: None,
            clamped: 0,
            records: StepRecords::new(q, m, s.record_frames),
            frame: 0,
            tick_frames,
            tick_s,
            parts_live: false,
            part_ramp_frames: (s.part_ramp_s * s.fs).max(1.0),
            tones_run: false,
            floor_diag: s.floor_diag,
            df: s.df,
            lines: s.lines,
            held_peak: vec![0.0; m],
            ch: vec![0.0; q],
            res: vec![0.0; q],
            rnd: vec![0.0; m],
            d: vec![0.0; m],
            y: vec![C64::new(0.0, 0.0); q],
            refv: vec![C64::new(0.0, 0.0); n],
            dif: vec![C64::new(0.0, 0.0); m],
            prop: vec![C64::new(0.0, 0.0); m],
            req: vec![C64::new(0.0, 0.0); k * m],
            inf: vec![0.0; k * m],
            room: vec![0.0; m],
            scales: vec![1.0; k],
            limiting: vec![false; k],
            rpk: vec![0.0; m],
            rpk_full: vec![0.0; m],
            bounds: vec![None; p],
            freq: vec![None; parts],
            on: vec![false; parts],
        }
    }

    /// Frames per control tick.
    pub fn tick_frames(&self) -> u64 {
        self.tick_frames
    }

    /// Whether every drive's output has reached zero (the stop ramp is done).
    pub(crate) fn stopped(&self) -> bool {
        self.stop.value() <= 0.0 && self.stop.settled()
    }

    /// Start the stop ramp over `seconds`.
    pub(crate) fn ramp_down(&mut self, seconds: f64) {
        if self.stop.target() > 0.0 {
            self.stop.go(0.0, (seconds * self.fs).max(1.0));
        }
    }

    fn latch(&mut self, t: StepTrip) {
        if self.trip.is_none() {
            self.trip = Some(t);
            self.ramp_down(ABORT_RAMP_S);
        }
    }

    #[inline]
    fn frame_step(&mut self, input: &[f32], drive_out: &mut [f32]) {
        let (n, m, p) = (self.n, self.m, self.p);
        let q = n + p;
        for (i, &s) in self.slots.iter().enumerate() {
            self.ch[i] = input.get(s).copied().unwrap_or(0.0) as f64;
        }
        if self.trip.is_none() {
            for &(slot, at, idx) in &self.overload {
                let v = input.get(slot).copied().unwrap_or(0.0) as f64;
                if v.abs() >= at {
                    self.trip = Some(StepTrip::Overload {
                        channel_index: idx,
                        value: v,
                    });
                    self.stop.go(0.0, (ABORT_RAMP_S * self.fs).max(1.0));
                    break;
                }
            }
        }
        let lev = self.level.next();
        let rg = self.rand_ramp.next();
        let stop = self.stop.next();
        if !self.ring.pop(&mut self.rnd) {
            self.underruns += 1;
            self.rnd.fill(0.0);
        }
        let g = lev * rg;
        for j in 0..m {
            self.d[j] = g * self.rnd[j];
        }
        self.bank.drive_frame(lev, &mut self.d);
        for j in 0..m {
            let x = self.d[j] * stop;
            if x.abs() >= self.abort_v[j] && self.trip.is_none() {
                self.trip = Some(StepTrip::DriveVpk { drive: j, volts: x });
                self.stop.go(0.0, (ABORT_RAMP_S * self.fs).max(1.0));
            }
            let pk = self.peak_v[j];
            let y = if x.abs() > pk {
                self.clamped += 1;
                x.clamp(-pk, pk)
            } else {
                x
            };
            self.d[j] = y;
            drive_out[self.out_slots[j]] = y as f32;
        }
        self.bank.track_frame(&self.ch, &mut self.res);
        let r = &mut self.records;
        if r.gain.len() < r.gain.capacity() {
            r.residual.extend_from_slice(&self.res[..q]);
            r.raw.extend_from_slice(&self.ch[..q]);
            r.drive.extend_from_slice(&self.d[..m]);
            r.gain.push(g * stop);
        } else {
            r.dropped += 1;
        }
        self.frame += 1;
        if self.frame.is_multiple_of(self.tick_frames) {
            self.tick();
        }
    }

    /// One control tick: the part clock, each tone's law, the ceiling rule.
    fn tick(&mut self) {
        let dt = self.tick_s;
        let (n, m) = (self.n, self.m);
        {
            let now = self.clock.tick(dt);
            for (i, pn) in now.iter().enumerate() {
                self.freq[i] = pn.freq_hz;
                self.on[i] = pn.on;
            }
        }
        let lam = self.level.value();
        let ahead = 2.0 * self.tick_frames as f64;
        let lam_ahead = lam.max(self.level.ahead(ahead));
        let quiet = self.level.settled() && self.stop.target() > 0.0;
        // The random part's held peak at full level, two ticks ahead.
        self.ring
            .peak_ahead(2 * self.tick_frames as usize, &mut self.rpk);
        let release = (-dt / RANDOM_PEAK_RELEASE_S).exp();
        let rg = self.rand_ramp.value().max(self.rand_ramp.ahead(ahead));
        for j in 0..m {
            self.held_peak[j] = self.rpk[j].max(self.held_peak[j] * release);
            self.rpk_full[j] = self.held_peak[j] * rg;
        }
        let k_tones = self.tones.len();
        for k in 0..k_tones {
            let part = self.tones[k].part;
            let f = self.freq[part].unwrap_or(self.tones[k].f_prev);
            let want = if self.on[part] && self.parts_live && self.stop.target() > 0.0 {
                1.0
            } else {
                0.0
            };
            if f > 0.0 {
                self.bank.set_frequency(k, f);
            }
            let line = ((f / self.df).round() as usize).min(self.lines.saturating_sub(1));
            let lines = self.lines;
            if want != self.tones[k].target {
                self.tones[k].target = want;
                self.bank.set_part_level(k, want, self.part_ramp_frames);
                if want > 0.0 && !self.tones[k].engaged {
                    let ff = &self.tones[k].ff[line * m..(line + 1) * m];
                    self.bank.set_drive(k, ff, 0);
                }
            }
            // A tracker reads a tone whose amplitude is still ramping (its
            // part ramp or the level's) as it was a settle time ago: its
            // settle count starts again until the ramps are done.
            if self.bank.part_level(k) < 1.0 - 1e-9 || !quiet {
                self.bank.restart_settle(k);
            }
            self.bank.drive_in_force(k, &mut self.dif);
            let readable = (0..n).all(|i| {
                let a = self.tones[k].reference[i * lines + line].norm();
                a <= 0.0 || tone_readable(a, self.floor_diag[line * n + i], self.bank.bn_hz())
            });
            let tc = &self.tones[k];
            let usable = self.tones_run
                && tc.target > 0.0
                && self.bank.part_level(k) >= 1.0 - 1e-9
                && self.bank.settled(k)
                && quiet
                && readable
                && lam > 0.0;
            let sweep_hz_s = if tc.f_prev > 0.0 {
                (f - tc.f_prev) / dt
            } else {
                0.0
            };
            let sweep_oct_min = if tc.f_prev > 0.0 && f > 0.0 {
                (f / tc.f_prev).log2() / dt * 60.0
            } else {
                0.0
            };
            let was_engaged = tc.engaged;
            self.tones[k].f_prev = f;
            self.tones[k].usable = usable;
            // An engaged tone with no usable reading (its tracker settling
            // after a ramp, the level moving) is still handed to its law
            // every tick, told it is on target, so the law carries its drive
            // along a sweep on its model and corrects nothing (single-output
            // Mixed's rule). Found: a swept tone held at its drive in force
            // through 20 Hz of a mode read 2 dB off the moment it was usable
            // again, and aborted at -3.2 dB a second later.
            let shown = !usable
                && was_engaged
                && want > 0.0
                && self.tones_run
                && lam > 0.0
                && self.trip.is_none();
            if !usable {
                let tc = &mut self.tones[k];
                tc.avg_s.fill(0.0);
                tc.abort_for_s.fill(0.0);
            }
            if !usable && !shown {
                let tc = &mut self.tones[k];
                if tc.engaged || want <= 0.0 {
                    tc.ask.copy_from_slice(&self.dif);
                } else {
                    let (ask, ff) = (&mut tc.ask, &tc.ff);
                    ask.copy_from_slice(&ff[line * m..(line + 1) * m]);
                }
                continue;
            }
            // The law: actual volts and readings at the level in force.
            self.bank.tracked_vector(k, &mut self.y);
            for i in 0..n {
                self.refv[i] = self.tones[k].reference[i * lines + line] * lam;
            }
            if shown {
                // Told on target, the law carries its own last ask, not the
                // drive the ceiling rule left in force: rebased on a scaled
                // drive with no error to see, each tick's cut would stay
                // (found: three 2 g tones read -3 dB on every control the
                // moment they were usable after the level's last step, with
                // no tone limiting at full level).
                self.y[..n].copy_from_slice(&self.refv);
                self.dif.copy_from_slice(&self.tones[k].ask);
            }
            for v in self.dif.iter_mut() {
                *v *= lam;
            }
            let engage = !self.tones[k].engaged;
            let t0 = self.law_clock.now_s();
            let answer = {
                let tc = &mut self.tones[k];
                tc.out.reset();
                let inputs = TickInputs {
                    index: tc.calls,
                    dt_s: dt,
                    frequency_hz: f,
                    sweep_hz_per_s: sweep_hz_s,
                    sweep_oct_per_min: sweep_oct_min,
                    level: lam,
                    reference: &self.refv,
                    controls: &self.y[..n],
                    limits: &self.y[n..],
                    limit_bounds: &self.bounds,
                    drive_in_force: &self.dif,
                    hold: Hold::Free,
                    notch_scale: 1.0,
                    settling: false,
                };
                let law = &mut tc.law;
                let out = &mut tc.out;
                let values = &mut tc.values;
                tc.calls += 1;
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if engage {
                        law.engage(&inputs, out, values)
                    } else {
                        law.tick(&inputs, out, values)
                    }
                })) {
                    Ok(r) => r,
                    Err(p) => Err(LawFault::Crashed(hertzforge_sine::law::guard::panic_text(
                        p.as_ref(),
                    ))),
                }
            };
            let took = self.law_clock.now_s() - t0;
            let answer = answer.and_then(|()| {
                if took.is_nan() || took > self.law_budget_s {
                    return Err(LawFault::OverBudget {
                        took_s: took,
                        budget_s: self.law_budget_s,
                    });
                }
                check_outputs(&self.tones[k].out, m, n).map_err(LawFault::BadOutput)
            });
            let in_norm: f64 = self.dif.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt();
            match answer {
                Err(fault) => {
                    let tc = &mut self.tones[k];
                    tc.misses += 1;
                    for (a, v) in tc.ask.iter_mut().zip(&self.dif) {
                        *a = *v / lam;
                    }
                    self.record(k, in_norm, in_norm, took, 1, engage, false);
                    if engage || self.tones[k].misses >= self.guards.law_misses {
                        self.latch(StepTrip::ToneLaw {
                            tone: k,
                            fault: fault.reason().to_string(),
                        });
                    }
                    continue;
                }
                Ok(()) => {
                    let tc = &mut self.tones[k];
                    tc.misses = 0;
                    for (pv, o) in self.prop.iter_mut().zip(&tc.out.drive) {
                        *pv = *o / lam;
                    }
                    for v in self.dif.iter_mut() {
                        *v /= lam;
                    }
                    let mut bound = false;
                    if engage {
                        tc.engaged = true;
                        let ff = &tc.ff[line * m..(line + 1) * m];
                        let ffn: f64 = ff.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt();
                        let limit = ffn
                            * 10f64
                                .powf(hertzforge_mimo_sine::limits::FIRST_DRIVE_MARGIN_DB / 20.0);
                        if let Some(s) = first_drive_scale(&self.prop, limit) {
                            for v in self.prop.iter_mut() {
                                *v *= s;
                            }
                            bound = true;
                        }
                    } else {
                        let cap = cap_drives(&self.dif, &mut self.prop, &self.guards, dt);
                        bound = cap.magnitude_bound || cap.rotation_bound;
                    }
                    let diverged = tc.watch.update(bound && !tc.limiting, dt);
                    tc.ask.copy_from_slice(&self.prop);
                    let out_norm: f64 = self.prop.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt();
                    self.record(k, in_norm, out_norm * lam, took, 0, engage, bound);
                    if diverged {
                        self.latch(StepTrip::ToneDivergence { tone: k });
                    }
                }
            }
            if usable {
                self.judge(k, line, lam, dt);
            }
        }
        if k_tones == 0 {
            return;
        }
        // The ceiling rule over every tone.
        for k in 0..k_tones {
            let rho = self.bank.part_level(k).max(self.tones[k].target);
            self.bank.drive_in_force(k, &mut self.dif);
            for j in 0..m {
                self.req[k * m + j] = self.tones[k].ask[j] * rho;
                self.inf[k * m + j] = self.dif[j].norm() * self.bank.part_level(k);
            }
        }
        headroom(lam_ahead, &self.peak_v, &self.rpk_full, &mut self.room);
        self.rule.apply(
            &self.req,
            &self.inf,
            k_tones,
            &self.room,
            &mut self.scales,
            &mut self.limiting,
        );
        let tf = self.tick_frames;
        for k in 0..k_tones {
            let s = self.scales[k];
            for (pv, a) in self.prop.iter_mut().zip(&self.tones[k].ask) {
                *pv = *a * s;
            }
            self.tones[k].scale = s;
            let hold = self.tones[k]
                .judging
                .iter()
                .map(|j| j.judge_s)
                .fold(0.0, f64::max)
                + hertzforge_dsp::tone::TRACKER_DELAY_FACTOR / self.bank.bn_hz();
            let tc = &mut self.tones[k];
            tc.limited_s = if self.limiting[k] {
                hold
            } else {
                (tc.limited_s - self.tick_s).max(0.0)
            };
            tc.limiting = self.limiting[k] || tc.limited_s > 0.0;
            self.bank.set_drive(k, &self.prop, tf);
        }
    }

    /// The tone's reading on every control against its tolerance (D17).
    fn judge(&mut self, k: usize, line: usize, lam: f64, dt: f64) {
        let n = self.n;
        let lines = self.lines;
        let bn = self.bank.bn_hz();
        for i in 0..n {
            let target = self.tones[k].reference[i * lines + line].norm() * lam;
            if target <= 0.0 {
                continue;
            }
            let y = self.bank.tracked(k, i).norm();
            let g = self.floor_diag[line * n + i] * lam * lam;
            let tc = &mut self.tones[k];
            let jd = tc.judging[i];
            // Single-output Mixed Mode's average: seeded with the first
            // reading, judged once it has averaged a time constant.
            let value = if jd.averages {
                let tau = jd.tau_s(bn).max(dt);
                if tc.avg_s[i] == 0.0 {
                    tc.ms[i] = y * y;
                } else {
                    tc.ms[i] += (y * y - tc.ms[i]) * (dt / tau).min(1.0);
                }
                tc.avg_s[i] += dt;
                if tc.avg_s[i] < tau {
                    tc.err_db[i] = f64::NAN;
                    tc.alarm[i] = false;
                    continue;
                }
                (tc.ms[i] - 2.0 * g * bn).max(1e-300).sqrt()
            } else {
                y
            };
            let e = 20.0 * (value / target).log10();
            tc.err_db[i] = e;
            tc.alarm[i] = e < -jd.alarm_db.0 || e > jd.alarm_db.1;
            if e < -jd.abort_db.0 || e > jd.abort_db.1 {
                tc.abort_for_s[i] += dt;
                if tc.abort_for_s[i] >= TONE_ABORT_DEBOUNCE_S {
                    let limiting = tc.limiting;
                    self.latch(StepTrip::ToneAbort {
                        tone: k,
                        control: i,
                        error_db: e,
                        limiting,
                    });
                    return;
                }
            } else {
                tc.abort_for_s[i] = 0.0;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        k: usize,
        drive_in: f64,
        drive_out: f64,
        took_s: f64,
        miss: u8,
        engage: bool,
        bound: bool,
    ) {
        let r = &mut self.records.law;
        if r.len() < r.capacity() {
            r.push(LawRecord {
                t_s: self.frame as f64 / self.fs,
                drive_in,
                drive_out,
                took_us: (took_s * 1e6) as f32,
                role: RecordRole::Tone,
                tone: k as u8,
                miss,
                flags: u8::from(bound) | (u8::from(engage) << 1),
            });
        }
    }
}

impl ProcessStep for MimoMixedStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        let w = self.n_out;
        drive_out.clear();
        drive_out.resize(n_frames * w, 0.0);
        let n_in = self.n_in.max(1);
        for f in 0..n_frames {
            let inp = input.get(f * n_in..(f + 1) * n_in).unwrap_or(&[]);
            let out = &mut drive_out[f * w..(f + 1) * w];
            self.frame_step(inp, out);
        }
        if let Some(kin) = self.kin.as_mut() {
            self.kin_scratch.clear();
            let take = (n_frames * n_in).min(input.len());
            if self.kin_scratch.capacity() >= take {
                self.kin_scratch
                    .extend(input[..take].iter().map(|&v| v as f64));
                if let Some(t) = kin.feed(&self.kin_scratch) {
                    if self.trip.is_none() {
                        self.trip = Some(StepTrip::Kinematic {
                            channel_index: t.channel_index,
                            value: t.value,
                            limit: t.limit,
                        });
                        self.stop.go(0.0, (ABORT_RAMP_S * self.fs).max(1.0));
                    }
                }
            }
        }
    }
}
