//! The run's per-frame step: [`MixedProcessStep`], the engine's
//! [`ProcessStep`] for a Mixed Mode run (design section 3, per frame and per
//! control tick).
//!
//! Per frame: the next random sample from the ring (floor and bands,
//! synthesized ahead between chunks), every tone's oscillator, the parts'
//! ramps and the run's level ramp; the sum is judged against the drive ceiling
//! and abort and written to the drive. The response is read on each control
//! channel; every tone's tracker reads it against the phase played `lag`
//! frames before, and the residual (the response less every tracked tone) is
//! kept for the floor's estimate between chunks.
//!
//! Per control tick: the part clock moves; each tone that is on, settled, and
//! readable above the floor gets one call of its law, the core's caps, and a
//! slew to the new amplitude over the next tick.
//!
//! Everything the step owns is sized at construction; `process` allocates
//! nothing while the service drains its records every chunk.

use super::status::{MixedTripKind, RoleTally};
use super::LawRole;
use crate::parts::{MixedConfig, ToneAmplitude};
use crate::schedule::PartClock;
use crate::tone::{tone_readable, Ramp, ToneBank};
use hertzforge_engine::ProcessStep;
use hertzforge_sine::law::{
    guard::{call_law, cap_drive},
    Hold, LawClock, LawGuards, LawValues, SineControlLaw, TickInputs,
};
use hertzforge_sine::profile::SineProfile;
use std::collections::VecDeque;
use std::sync::Arc;

/// How long an abort takes the drive to zero, seconds: a ramp, never a step
/// (an instant step into an armature is a broadband impulse), short enough
/// that the abort's latency is the output queue's plus this.
pub const ABORT_RAMP_S: f64 = 0.02;
/// A tone's error must stay past its abort this long before the run aborts.
pub const TONE_ABORT_DEBOUNCE_S: f64 = 0.5;

/// What the step plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepMode {
    /// Nothing (before engage, after the end).
    Silent,
    /// The identification: the ring alone, recorded with the drive.
    Identify,
    /// The test.
    Run,
}

/// A tone's full-level amplitude, resolved once so a tick does not allocate.
#[derive(Debug, Clone)]
pub(crate) enum AmpModel {
    Constant(f64),
    Profile(SineProfile),
}

impl AmpModel {
    pub(crate) fn from(a: &ToneAmplitude) -> Self {
        match a {
            ToneAmplitude::Constant { g_pk } => AmpModel::Constant(*g_pk),
            ToneAmplitude::Profile { rows } => AmpModel::Profile(SineProfile {
                rows: rows.clone(),
                ..SineProfile::default()
            }),
        }
    }

    pub(crate) fn at(&self, f: f64) -> f64 {
        match self {
            AmpModel::Constant(a) => *a,
            AmpModel::Profile(p) => p.target_accel_g_pk(f).unwrap_or(0.0),
        }
    }
}

/// One tone's control state.
pub(crate) struct ToneCtl {
    pub(crate) part: usize,
    pub(crate) amp: AmpModel,
    pub(crate) law: Box<dyn SineControlLaw + Send>,
    pub(crate) values: LawValues,
    pub(crate) engaged: bool,
    pub(crate) ticks: u64,
    pub(crate) misses_in_row: u32,
    pub(crate) tally: RoleTally,
    pub(crate) cap_bound_s: f64,
    pub(crate) last_freq: f64,
    pub(crate) rate: f64,
    pub(crate) quiet_frames: u64,
    pub(crate) measured: Option<f64>,
    pub(crate) error_db: Option<f64>,
    pub(crate) usable: bool,
    /// Whether the tone's law has had a usable reading since it came in.
    pub(crate) read_once: bool,
    pub(crate) alarm: bool,
    pub(crate) abort_s: f64,
    pub(crate) alarm_db: (f64, f64),
    pub(crate) abort_db: (f64, f64),
    /// The operator's live switch.
    pub(crate) switched_on: bool,
}

/// What the step tripped on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StepTrip {
    pub(crate) kind: MixedTripKind,
    pub(crate) part: Option<usize>,
    pub(crate) frame: u64,
    /// An abort (the drive ramps out at once) or a controlled stop.
    pub(crate) abort: bool,
}

/// The identification on the line grid, as the tones read it.
#[derive(Debug, Clone, Default)]
pub(crate) struct PlantGrid {
    pub(crate) f0: f64,
    pub(crate) df: f64,
    /// Composite `|H|`, g per drive unit.
    pub(crate) gain: Vec<f64>,
    pub(crate) trusted: Vec<bool>,
    /// The floor's full-level target, g^2/Hz.
    pub(crate) floor: Vec<f64>,
}

impl PlantGrid {
    fn interp(&self, v: &[f64], f: f64) -> f64 {
        if v.is_empty() || self.df <= 0.0 {
            return 0.0;
        }
        let x = ((f - self.f0) / self.df).max(0.0);
        let k = (x.floor() as usize).min(v.len() - 1);
        let k1 = (k + 1).min(v.len() - 1);
        let t = (x - k as f64).clamp(0.0, 1.0);
        v[k] * (1.0 - t) + v[k1] * t
    }

    /// `|H|` at `f`, and whether the nearest line is trusted.
    pub(crate) fn gain_at(&self, f: f64) -> (f64, bool) {
        let k = (((f - self.f0) / self.df).round().max(0.0) as usize)
            .min(self.trusted.len().saturating_sub(1));
        (
            self.interp(&self.gain, f),
            self.trusted.get(k).copied().unwrap_or(false),
        )
    }

    pub(crate) fn floor_at(&self, f: f64) -> f64 {
        self.interp(&self.floor, f)
    }
}

/// The records the service drains every chunk.
#[derive(Debug, Default)]
pub(crate) struct Records {
    /// Raw control samples per control channel.
    pub(crate) control: Vec<Vec<f64>>,
    /// Residual samples per control channel.
    pub(crate) residual: Vec<Vec<f64>>,
    /// The random part's gain (level times its ramp) that produced each
    /// response sample (read `lag` frames back).
    pub(crate) gain: Vec<f64>,
    /// The random part as played, identification only.
    pub(crate) drive: Vec<f64>,
    /// Raw samples per limit channel.
    pub(crate) limits: Vec<Vec<f64>>,
    /// The random part's sum of squares and count, as played.
    pub(crate) rand_sumsq: f64,
    pub(crate) rand_n: u64,
    /// The summed drive's peak and sum of squares over the chunk.
    pub(crate) peak: f64,
    pub(crate) sumsq: f64,
    pub(crate) n: u64,
}

/// The engine's step for a Mixed Mode run.
pub(crate) struct MixedProcessStep {
    pub(crate) fs: f64,
    in_stride: usize,
    out_channels: usize,
    ctrl_slots: Vec<usize>,
    pub(crate) weights: Vec<f64>,
    pub(crate) mode: StepMode,
    /// The random part, synthesized ahead, at full level and full ramp.
    pub(crate) ring: VecDeque<f64>,
    pub(crate) underruns: u64,
    pub(crate) bank: ToneBank,
    pub(crate) tones: Vec<ToneCtl>,
    pub(crate) clock: PartClock,
    pub(crate) clock_running: bool,
    /// The run's level, as a gain on every part.
    pub(crate) level: Ramp,
    /// The random part's own ramp (in at engage, out at the stop).
    pub(crate) random: Ramp,
    abort_ramp: Ramp,
    gain_ring: Vec<f64>,
    gain_head: usize,
    pub(crate) frames: u64,
    pub(crate) tick_frames: usize,
    tick_at: usize,
    pub(crate) part_ramp_frames: f64,
    pub(crate) guards: LawGuards,
    pub(crate) tone_clock: Arc<dyn LawClock>,
    pub(crate) budget_s: f64,
    pub(crate) plant: PlantGrid,
    pub(crate) ceiling_vpk: f64,
    pub(crate) abort_vpk: f64,
    pub(crate) records: Records,
    /// Each tone law call, until the service drains them (bounded: a call
    /// past the capacity is not recorded rather than allocated for).
    pub(crate) law_log: Vec<super::law_records::MixedLawRecord>,
    /// The frame the loop engaged at.
    pub(crate) engaged_frame: u64,
    pub(crate) trip: Option<StepTrip>,
    in_scratch: Vec<f64>,
    /// The whole input block as f64, for the kinematic monitor.
    kin_scratch: Vec<f64>,
    /// Random's raw-sample kinematic monitor (velocity and displacement from
    /// acceleration), when the run asks for it and a channel has a limit.
    pub(crate) kin: Option<hertzforge_random::KinematicMonitor>,
    /// Per input slot: `(channel index, |x| at which it overloads, whether
    /// an overload aborts)`. A control or limit channel's overload aborts (the
    /// loop steers by it); any other channel's latches an indicator.
    pub(crate) overload: Vec<(u32, f64, bool)>,
    /// Channels whose overload indicator has latched (non-aborting channels).
    pub(crate) overloads: Vec<u32>,
    /// Limit channels' places in the input frame.
    pub(crate) lim_slots: Vec<usize>,
    /// Per input slot (as `overload`): the sum of squares and the frames read
    /// since the last rms, and the last rms over a half second.
    in_sq: Vec<f64>,
    in_frames: usize,
    pub(crate) input_rms: Vec<(u32, f64)>,
    res_scratch: Vec<f64>,
    amp_scratch: Vec<f64>,
    phase_scratch: Vec<f64>,
}

impl MixedProcessStep {
    /// A silent step. `ctrl_slots` are the control channels' places in the
    /// input frame, `in_stride` the frame's width. `capacity` sizes the
    /// records (frames per chunk, with room).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cfg: &MixedConfig,
        fs: f64,
        in_stride: usize,
        out_channels: usize,
        ctrl_slots: Vec<usize>,
        clock: PartClock,
        tone_clock: Arc<dyn LawClock>,
        capacity: usize,
    ) -> Self {
        let n_ctrl = ctrl_slots.len();
        let tick_s = cfg
            .control_tick_s
            .unwrap_or(hertzforge_sine::law::DEFAULT_TICK_S);
        let guards = cfg.law_guards.clone().unwrap_or_default().tones;
        let ceiling = cfg.limits.drive_vpk_ceiling;
        MixedProcessStep {
            fs,
            in_stride,
            out_channels: out_channels.max(1),
            weights: cfg.control.channels.iter().map(|c| c.weight).collect(),
            ctrl_slots,
            mode: StepMode::Silent,
            ring: VecDeque::with_capacity(capacity * 4),
            underruns: 0,
            bank: ToneBank::new(fs, &[], n_ctrl, cfg.tracking_hz, 0),
            tones: Vec::new(),
            clock,
            clock_running: false,
            level: Ramp::at(0.0),
            random: Ramp::at(0.0),
            abort_ramp: Ramp::at(1.0),
            gain_ring: vec![0.0; 1],
            gain_head: 0,
            frames: 0,
            tick_frames: ((tick_s * fs).round() as usize).max(1),
            tick_at: 0,
            part_ramp_frames: (cfg.part_ramp_s * fs).max(1.0),
            budget_s: guards.call_budget_s(tick_s),
            guards,
            tone_clock,
            plant: PlantGrid::default(),
            ceiling_vpk: ceiling,
            abort_vpk: cfg.limits.drive_vpk_abort.unwrap_or(ceiling),
            records: Records {
                control: vec![Vec::with_capacity(capacity); n_ctrl],
                residual: vec![Vec::with_capacity(capacity); n_ctrl],
                gain: Vec::with_capacity(capacity),
                drive: Vec::with_capacity(capacity),
                ..Records::default()
            },
            trip: None,
            law_log: Vec::with_capacity(64 * 128),
            engaged_frame: 0,
            in_scratch: vec![0.0; n_ctrl],
            kin_scratch: Vec::with_capacity(capacity.min(1 << 16) * in_stride.max(1)),
            kin: None,
            overload: Vec::new(),
            overloads: Vec::with_capacity(64),
            lim_slots: Vec::new(),
            in_sq: Vec::new(),
            in_frames: 0,
            input_rms: Vec::new(),
            res_scratch: vec![0.0; n_ctrl],
            amp_scratch: vec![0.0; n_ctrl],
            phase_scratch: vec![0.0; n_ctrl],
        }
    }

    /// Install the tones (at engage): the bank at the measured lag, one control
    /// state per tone, in parts-list order.
    pub(crate) fn set_tones(&mut self, bank: ToneBank, tones: Vec<ToneCtl>) {
        self.gain_ring = vec![0.0; bank.lag() + 1];
        self.gain_head = 0;
        self.bank = bank;
        self.tones = tones;
    }

    /// The pipeline lag the response is read at, frames.
    pub(crate) fn lag(&self) -> usize {
        self.gain_ring.len() - 1
    }

    /// Whether every output is at zero for good (the level ramp has reached
    /// zero and is going nowhere, or an abort has finished).
    pub(crate) fn drive_is_zero(&self) -> bool {
        (self.level.value() == 0.0 && self.level.target() == 0.0)
            || (self.abort_ramp.value() == 0.0)
            || self.mode == StepMode::Silent
    }

    /// Start the abort ramp (idempotent).
    pub(crate) fn begin_abort(&mut self) {
        if self.abort_ramp.target() != 0.0 {
            self.abort_ramp.go(0.0, ABORT_RAMP_S * self.fs);
        }
    }

    fn latch(&mut self, kind: MixedTripKind, part: Option<usize>, abort: bool) {
        if self.trip.is_none() {
            self.trip = Some(StepTrip {
                kind,
                part,
                frame: self.frames,
                abort,
            });
            if abort {
                self.begin_abort();
            }
        }
    }

    fn control_tick(&mut self) {
        let dt = self.tick_frames as f64 / self.fs;
        if self.clock_running {
            self.clock.tick(dt);
        }
        let lev = self.level.value();
        let level_quiet = self.level.settled();
        let settle_frames = (self.bank.tracker_settle_s() * self.fs) as u64 + self.lag() as u64;
        for j in 0..self.tones.len() {
            let now = self.clock.now()[self.tones[j].part];
            let f = now.freq_hz.unwrap_or(self.tones[j].last_freq);
            {
                let t = &mut self.tones[j];
                t.rate = if self.clock_running && !self.clock.held() {
                    (f - t.last_freq) / dt
                } else {
                    0.0
                };
                t.last_freq = f;
            }
            self.bank.voice_mut(j).set_frequency(f);
            let want_on = self.clock_running && now.on && self.tones[j].switched_on;
            let target_full = self.tones[j].amp.at(f);
            let (gain, trusted) = self.plant.gain_at(f);
            let prediction = (trusted && gain > 0.0 && target_full > 0.0)
                .then(|| 20.0 * (target_full / gain).log10());
            // Ramp in from the feed-forward, or out.
            let ramp_target = self.bank.voice(j).ramp_target();
            if want_on && ramp_target < 1.0 {
                if !self.tones[j].engaged {
                    let (amp, ph) = (
                        std::mem::take(&mut self.amp_scratch),
                        std::mem::take(&mut self.phase_scratch),
                    );
                    let drive = self.bank.voice(j).amplitude_cmd();
                    let t = &mut self.tones[j];
                    let inputs = tick_inputs(
                        t.ticks,
                        t.rate,
                        f,
                        target_full,
                        0.0,
                        (&amp, &ph),
                        drive,
                        prediction,
                        dt,
                    );
                    let (r, _) = call_law(t.law.as_mut(), true, &inputs, &mut t.values, None);
                    self.amp_scratch = amp;
                    self.phase_scratch = ph;
                    let a = match r {
                        Ok(out) => out.drive,
                        Err(_) => prediction.map_or(0.0, |db| 10f64.powf(db / 20.0)),
                    };
                    t.engaged = true;
                    t.tally.calls += 1;
                    let a = a.min(self.ceiling_vpk);
                    self.bank.voice_mut(j).set_amplitude(a);
                }
                let frames = self.part_ramp_frames;
                self.bank.voice_mut(j).ramp_to(1.0, frames);
            } else if !want_on && ramp_target > 0.0 {
                let frames = self.part_ramp_frames;
                self.bank.voice_mut(j).ramp_to(0.0, frames);
            }
            let voice = self.bank.voice(j);
            let full_in = voice.ramp_settled() && voice.scale() == 1.0;
            {
                let t = &mut self.tones[j];
                if full_in && level_quiet {
                    t.quiet_frames += self.tick_frames as u64;
                } else {
                    t.quiet_frames = 0;
                }
            }
            let (a, trk_settled) = self.bank.composite(j, &self.weights);
            let norm = lev * self.bank.voice(j).scale();
            let measured = (norm > 0.0).then(|| a / norm);
            let readable = tone_readable(target_full, self.plant.floor_at(f), self.bank.bn_hz());
            let usable = self.tones[j].engaged
                && full_in
                && trk_settled
                && readable
                && self.tones[j].quiet_frames >= settle_frames;
            {
                let t = &mut self.tones[j];
                t.usable = usable;
                t.measured = if trk_settled { measured } else { None };
                t.error_db = match (t.measured, usable) {
                    (Some(m), true) if m > 0.0 && target_full > 0.0 => {
                        Some(20.0 * (m / target_full).log10())
                    }
                    _ => None,
                };
            }
            // From engage on, the law is called every tick. Until the tone's
            // reading is usable (ramping, its tracker settling, under the
            // floor, the level moving) it is told the tone is on target, so
            // it follows its feed-forward along the sweep and never catches
            // up on a frozen drive at once (found: a swept tone held right
            // after it came in stepped 5 dB in half a second and aborted).
            if !self.tones[j].engaged || self.trip.is_some() {
                continue;
            }
            let in_force = self.bank.voice(j).amplitude_cmd();
            let mut call: Option<(f64, u8)>;
            let proposed = {
                let m = if usable {
                    measured.unwrap_or(0.0)
                } else {
                    target_full
                };
                for c in 0..self.weights.len() {
                    let trk = self.bank.tracker(j, c);
                    self.amp_scratch[c] = if usable {
                        trk.amplitude() / norm.max(1e-300)
                    } else {
                        target_full
                    };
                    self.phase_scratch[c] = trk.phase().to_degrees();
                }
                let (amp, ph) = (
                    std::mem::take(&mut self.amp_scratch),
                    std::mem::take(&mut self.phase_scratch),
                );
                let clock = self.tone_clock.clone();
                let budget = self.budget_s;
                let t = &mut self.tones[j];
                let inputs = tick_inputs(
                    t.ticks,
                    t.rate,
                    f,
                    target_full,
                    m,
                    (&amp, &ph),
                    in_force,
                    prediction,
                    dt,
                );
                let (r, took) = call_law(
                    t.law.as_mut(),
                    false,
                    &inputs,
                    &mut t.values,
                    Some((&*clock, budget)),
                );
                self.amp_scratch = amp;
                self.phase_scratch = ph;
                let t = &mut self.tones[j];
                t.tally.calls += 1;
                t.ticks += 1;
                t.tally.worst_call_us = t.tally.worst_call_us.max(took * 1e6);
                call = Some((took, 0u8));
                match r {
                    Ok(out) => {
                        t.misses_in_row = 0;
                        Some(out.drive)
                    }
                    Err(fault) => {
                        call = Some((took, miss_code(fault.reason())));
                        t.tally.misses += 1;
                        t.misses_in_row += 1;
                        if matches!(fault, hertzforge_sine::law::LawFault::BadOutput(_)) {
                            t.tally.refused += 1;
                        }
                        if t.misses_in_row >= self.guards.max_misses {
                            let reason = fault.reason().to_owned();
                            self.latch(
                                MixedTripKind::LawFault {
                                    role: LawRole::Tones,
                                    reason,
                                },
                                Some(self.tones[j].part),
                                false,
                            );
                        }
                        None
                    }
                }
            };
            // Until its first usable reading the tone plays its feed-forward
            // itself: the law, told it is on target, follows only its blend's
            // share of the prediction's change, so along a sweep the drive
            // drifts off the feed-forward and the first real reading starts
            // the servo on an error it ramps through at its rate cap while
            // the narrow tracker catches up (found: 0.7 dB at the handoff
            // became a 4 dB undershoot and the tone's abort).
            // Once it has been read, a tone the core cannot read for a while
            // (a level step, a hold) keeps the drive its law found.
            if usable {
                self.tones[j].read_once = true;
            }
            let proposed = match (usable || self.tones[j].read_once, proposed, prediction) {
                (false, Some(_), Some(db)) => Some(10f64.powf(db / 20.0)),
                (_, p, _) => p,
            };
            let mut bound_now = false;
            if let Some(p) = proposed {
                let (capped, bound) = cap_drive(in_force, p, &self.guards, dt);
                bound_now = bound;
                let capped = capped.min(self.ceiling_vpk);
                let t = &mut self.tones[j];
                if bound {
                    t.tally.capped += 1;
                    t.cap_bound_s += dt;
                } else {
                    t.cap_bound_s = 0.0;
                }
                let tick = self.tick_frames as f64;
                self.bank.voice_mut(j).slew_to(capped, tick);
                if self.tones[j].cap_bound_s > self.guards.divergence_s {
                    self.latch(MixedTripKind::Divergence, Some(self.tones[j].part), false);
                }
            }
            if let Some((took, miss)) = call {
                if self.law_log.len() < self.law_log.capacity() {
                    self.law_log.push(super::law_records::MixedLawRecord {
                        t_s: self.frames.saturating_sub(self.engaged_frame) as f64 / self.fs,
                        drive_in: in_force,
                        drive_out: self.bank.voice(j).amplitude_cmd(),
                        took_us: (took * 1e6) as f32,
                        role: super::law_records::RecordRole::Tone,
                        tone: j as u8,
                        miss,
                        flags: u8::from(bound_now),
                    });
                }
            }
            // Judge.
            if let Some(e) = self.tones[j].error_db {
                let t = &mut self.tones[j];
                t.alarm = e < -t.alarm_db.0 || e > t.alarm_db.1;
                if e < -t.abort_db.0 || e > t.abort_db.1 {
                    t.abort_s += dt;
                } else {
                    t.abort_s = 0.0;
                }
                if t.abort_s >= TONE_ABORT_DEBOUNCE_S {
                    let part = t.part;
                    self.latch(MixedTripKind::ToneAbort { error_db: e }, Some(part), true);
                }
            }
        }
    }
}

/// The miss code a law record carries: the index of the reason among
/// [`TONE_MISS_NAMES`], plus one.
pub(crate) fn miss_code(reason: &str) -> u8 {
    TONE_MISS_NAMES
        .iter()
        .position(|n| *n == reason)
        .map_or(u8::MAX, |i| i as u8 + 1)
}

/// The tone laws' miss reasons, in record code order (Sine's names).
pub const TONE_MISS_NAMES: [&str; 8] = [
    "error",
    "panicked",
    "over_budget",
    "over_memory",
    "non_finite",
    "negative",
    "undeclared_value",
    "too_many_values",
];

#[allow(clippy::too_many_arguments)]
fn tick_inputs<'a>(
    ticks: u64,
    rate: f64,
    f: f64,
    target: f64,
    measured: f64,
    channels: (&'a [f64], &'a [f64]),
    drive: f64,
    prediction_db: Option<f64>,
    dt: f64,
) -> TickInputs<'a> {
    TickInputs {
        tick_index: ticks,
        dt_s: dt,
        frequency_hz: f,
        sweep_rate_hz_per_s: rate,
        level_db: 0.0,
        target,
        measured,
        channel_amplitude: channels.0,
        channel_phase_deg: channels.1,
        drive,
        prediction_db,
        hold: Hold::Free,
        notch_ceiling: None,
        settling: false,
    }
}

impl MixedProcessStep {
    /// The raw-sample guards on one input block: the kinematic monitor and
    /// each input's overload. Either one aborts.
    /// Each input's rms over the last half second of samples, tones in.
    fn read_input_rms(&mut self, input: &[f32], n_frames: usize) {
        let slots = self.overload.len();
        if slots == 0 || self.in_stride == 0 {
            return;
        }
        self.in_sq.resize(slots, 0.0);
        let frames = n_frames.min(input.len() / self.in_stride);
        for fr in 0..frames {
            for (slot, sq) in self.in_sq.iter_mut().enumerate() {
                let v = f64::from(input[fr * self.in_stride + slot]);
                *sq += v * v;
            }
        }
        self.in_frames += frames;
        if self.in_frames as f64 >= 0.5 * self.fs {
            let n = self.in_frames as f64;
            self.input_rms.clear();
            for (slot, sq) in self.in_sq.iter_mut().enumerate() {
                self.input_rms
                    .push((self.overload[slot].0, (*sq / n).sqrt()));
                *sq = 0.0;
            }
            self.in_frames = 0;
        }
    }

    fn watch(&mut self, input: &[f32], n_frames: usize) {
        self.read_input_rms(input, n_frames);
        let n = (n_frames * self.in_stride).min(input.len());
        if let Some(kin) = self.kin.as_mut() {
            self.kin_scratch.clear();
            self.kin_scratch
                .extend(input[..n].iter().map(|v| *v as f64));
            if let Some(t) = kin.feed(&self.kin_scratch) {
                self.latch(
                    MixedTripKind::Kinematic {
                        channel_index: t.channel_index,
                        axis: format!("{:?}", t.axis).to_lowercase(),
                        value: t.value,
                        limit: t.limit,
                    },
                    None,
                    true,
                );
            }
        }
        if self.trip.is_none() && !self.overload.is_empty() {
            for fr in 0..n_frames {
                for slot in 0..self.overload.len() {
                    let (ch, at, aborts) = self.overload[slot];
                    let v = input
                        .get(fr * self.in_stride + slot)
                        .copied()
                        .unwrap_or(0.0);
                    if (v as f64).abs() < at {
                        continue;
                    }
                    if aborts {
                        self.latch(MixedTripKind::Overload { channel_index: ch }, None, true);
                        return;
                    }
                    if !self.overloads.contains(&ch) {
                        self.overloads.push(ch);
                    }
                }
            }
        }
    }
}

impl ProcessStep for MixedProcessStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        drive_out.clear();
        if self.mode != StepMode::Silent {
            self.watch(input, n_frames);
        }
        let w = self.gain_ring.len();
        for fr in 0..n_frames {
            let x = if self.mode == StepMode::Silent {
                0.0
            } else {
                let lev = self.level.next();
                let rg = self.random.next();
                let r = match self.ring.pop_front() {
                    Some(r) => r,
                    None => {
                        self.underruns += 1;
                        0.0
                    }
                };
                let rand_part = r * rg * lev;
                self.records.rand_sumsq += rand_part * rand_part;
                self.records.rand_n += 1;
                let tones = self.bank.drive_frame(lev);
                self.gain_ring[self.gain_head] = rg * lev;
                self.gain_head = (self.gain_head + 1) % w;
                let ab = self.abort_ramp.next();
                let x = (rand_part + tones) * ab;
                if self.mode == StepMode::Identify {
                    self.records.drive.push(rand_part * ab);
                }
                if x.abs() >= self.abort_vpk && self.trip.is_none() {
                    self.latch(
                        MixedTripKind::DriveVpk {
                            realized_vpk: x.abs(),
                            limit_vpk: self.abort_vpk,
                        },
                        None,
                        true,
                    );
                }
                x.clamp(-self.ceiling_vpk, self.ceiling_vpk)
            };
            drive_out.push(x as f32);
            for _ in 1..self.out_channels {
                drive_out.push(0.0);
            }
            self.records.peak = self.records.peak.max(x.abs());
            self.records.sumsq += x * x;
            self.records.n += 1;
            // The response.
            let base = fr * self.in_stride;
            for (c, &slot) in self.ctrl_slots.iter().enumerate() {
                self.in_scratch[c] = input.get(base + slot).copied().unwrap_or(0.0) as f64;
            }
            if self.mode != StepMode::Silent {
                for (l, &slot) in self.lim_slots.iter().enumerate() {
                    let v = input.get(base + slot).copied().unwrap_or(0.0) as f64;
                    self.records.limits[l].push(v);
                }
                self.bank
                    .track_frame(&self.in_scratch, &mut self.res_scratch);
                for c in 0..self.ctrl_slots.len() {
                    self.records.control[c].push(self.in_scratch[c]);
                    self.records.residual[c].push(self.res_scratch[c]);
                }
                // The gain that produced this response: the oldest held.
                self.records.gain.push(self.gain_ring[self.gain_head]);
                self.tick_at += 1;
                if self.tick_at >= self.tick_frames {
                    self.tick_at = 0;
                    if self.mode == StepMode::Run {
                        self.control_tick();
                    }
                }
            }
            self.frames += 1;
        }
    }
}
