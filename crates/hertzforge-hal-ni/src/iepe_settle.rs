//! IEPE settling before a run is ready.
//!
//! An IEPE input is a constant current into the sensor, and the sensor sits at a
//! DC bias (about 8 to 12 V) on top of which the signal rides. The input is AC
//! coupled to block that bias, so when the current comes on the coupling sees
//! a step of the full bias and takes time to recover. Until it does, the
//! channel reads its rail. The NI 9234 showed it on the first Explorer run with
//! an accelerometer (2026-11-22): every sample of the first block read
//! 512.37 g, 5.12 V at 10 mV/g, the top of the +/-5 V range, and the run
//! aborted on its 400 g limit 15 ms after start with the drive at 0 V.
//!
//! So an IEPE channel is not ready when its task is. [`IepeSettle`] watches the
//! input with nothing driving, block by block, and calls a channel settled once
//! its offset has decayed and stayed down. The NI backend runs it inside
//! `arm()`, which every module passes through on hardware, so no module sees
//! the transient and no module can start a drive before it is over. A channel
//! that never settles refuses the arm with what it was reading.
//!
//! The current comes on once and stays on for the whole test (Kevin,
//! 2026-11-23): identification, low-level and full-level shock pulses, every
//! play. The backend's `stop()` keeps an input task with IEPE inputs reserved,
//! so the settle runs at the test's first arm only, and a pulse module's arm
//! before every shot costs nothing. It runs again only after the input task is
//! cleared (a reconfigure, close, or the abort, which ends the test).
//!
//! # The criterion: the offset has stopped MOVING
//!
//! The input is watched in windows of [`SETTLE_WINDOW_S`]. A window is QUIET on
//! a channel when, as fractions of that channel's own `+/-` range in volts:
//!
//! - its mean differs from the previous window's mean by less than
//!   [`SETTLE_DRIFT_FRACTION`]: the decaying bias step is a slowly moving
//!   offset, and a window mean is where it shows. Ambient vibration averages
//!   out of a half-second mean, so it does not hold the channel back.
//! - no sample in it reached [`SETTLE_PEAK_FRACTION`]: nothing near the rail.
//!
//! The whole set is settled at the first window that is quiet on every channel.
//!
//! Why drift and not level. Every module's velocity and displacement aborts
//! high-pass the acceleration, then integrate twice, seeded on the first sample
//! of the run (`hertzforge_engine::limits`). A CONSTANT offset is therefore
//! harmless, and the 9234's own offset error is one. An offset still decaying
//! when the run starts is not: through a first-order high-pass at `hp` and two
//! integrations it leaves a lasting displacement of about
//! `tau^2 * delta_a` (`tau = 1 / (2 pi hp)`), 2450 mil per g of remaining
//! change at a 2 Hz corner. The bench showed it (2026-11-23, Explorer, 10 mV/g):
//! with the first criterion (block mean under 0.5 % of range, 25 mV) the
//! channel was called settled at 4.30 s, and the run then aborted on
//! displacement, 200.1 mil against a 200 mil limit, 0.46 s in, with the drive
//! at 0 V. A drift under 50 uV per half second on a +/-5 V channel leaves a
//! remaining change of about 0.1 mV, about 24 mil at 10 mV/g and 2 Hz.
//!
//! # What it does not claim
//!
//! These are starting values, not measured ones; the bench sets them. The
//! startup pulse each channel showed is reported so the bench can learn what an
//! open or a shorted circuit looks like on a given module. No open or short
//! verdict is given from it yet: whether the current comes on at the task's
//! commit or its start decides what a "no pulse" reading means, and that is
//! not measured. Nor is it measured that a reserved, stopped task keeps its
//! current on; the backend checks the first block after every later start for
//! an IEPE input at its rail and warns if it finds one.

/// How far a window's mean may move from the previous window's, as a fraction
/// of the channel's range, for the offset to count as no longer moving: 50 uV
/// on a +/-5 V channel.
pub const SETTLE_DRIFT_FRACTION: f64 = 1.0e-5;
/// The peak, as a fraction of the channel's range, below which no sample is
/// near the rail.
pub const SETTLE_PEAK_FRACTION: f64 = 0.5;
/// The window the offset is averaged over.
pub const SETTLE_WINDOW_S: f64 = 0.5;
/// How long the whole set may take before the arm is refused. Inside the
/// shell's 60 s wait on a request that arms (`ARM_REQUEST_TIMEOUT_MS`) with the
/// backend's 5 s of wall-clock margin.
pub const SETTLE_LIMIT_S: f64 = 45.0;

/// One IEPE input to watch.
#[derive(Debug, Clone, PartialEq)]
pub struct SettleChannel {
    /// The DAQmx physical channel, for messages.
    pub physical: String,
    /// The channel-table row, when there is one.
    pub table_index: Option<u32>,
    /// The channel's column in an interleaved block of the AI task.
    pub column: usize,
    /// The channel's `+/-` range in volts, as handed to DAQmx.
    pub range_v: f64,
}

impl SettleChannel {
    fn name(&self) -> String {
        match self.table_index {
            Some(i) => format!("channel {i} ({})", self.physical),
            None => self.physical.clone(),
        }
    }
}

/// Where a settle stands after a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleState {
    /// Still watching.
    Pending,
    /// Every channel has been quiet for the hold.
    Settled,
    /// The limit passed with at least one channel unsettled.
    TimedOut,
}

#[derive(Debug, Clone)]
struct Watch {
    spec: SettleChannel,
    /// The open window: frames, sum and peak so far.
    win_frames: u64,
    win_sum: f64,
    win_peak: f64,
    /// The last closed window's mean, and how far it moved from the one before.
    last_mean_v: Option<f64>,
    last_drift_v: Option<f64>,
    last_peak_v: f64,
    /// Was the last closed window quiet?
    quiet: bool,
    /// The largest |volts| seen since the watch began: the startup pulse.
    pulse_v: f64,
}

/// Watches every IEPE input of one AI task until each has settled.
#[derive(Debug, Clone)]
pub struct IepeSettle {
    watches: Vec<Watch>,
    n_columns: usize,
    fs_hz: f64,
    window_frames: u64,
    limit_frames: u64,
    frames_seen: u64,
    /// The frame the set was found settled at.
    settled_at: Option<u64>,
}

impl IepeSettle {
    /// Watch `channels` in blocks of `n_columns` interleaved columns at `fs_hz`,
    /// with the module's window and limit.
    pub fn new(channels: Vec<SettleChannel>, n_columns: usize, fs_hz: f64) -> Self {
        Self::with_times(channels, n_columns, fs_hz, SETTLE_WINDOW_S, SETTLE_LIMIT_S)
    }

    /// As [`IepeSettle::new`] with a stated window and limit, in seconds.
    pub fn with_times(
        channels: Vec<SettleChannel>,
        n_columns: usize,
        fs_hz: f64,
        window_s: f64,
        limit_s: f64,
    ) -> Self {
        let fs = if fs_hz > 0.0 { fs_hz } else { 1.0 };
        IepeSettle {
            watches: channels
                .into_iter()
                .map(|spec| Watch {
                    spec,
                    win_frames: 0,
                    win_sum: 0.0,
                    win_peak: 0.0,
                    last_mean_v: None,
                    last_drift_v: None,
                    last_peak_v: 0.0,
                    quiet: false,
                    pulse_v: 0.0,
                })
                .collect(),
            n_columns: n_columns.max(1),
            fs_hz: fs,
            window_frames: (window_s * fs).ceil().max(1.0) as u64,
            limit_frames: (limit_s * fs).ceil().max(1.0) as u64,
            frames_seen: 0,
            settled_at: None,
        }
    }

    /// Nothing to watch: the task has no IEPE input.
    pub fn is_empty(&self) -> bool {
        self.watches.is_empty()
    }

    /// Seconds of input watched so far.
    pub fn elapsed_s(&self) -> f64 {
        self.frames_seen as f64 / self.fs_hz
    }

    /// Take one block of `frames` interleaved frames, in VOLTS, and say where the
    /// settle stands.
    pub fn feed(&mut self, block: &[f64], frames: usize) -> SettleState {
        let frames = frames.min(block.len() / self.n_columns);
        if frames == 0 || self.settled_at.is_some() {
            return self.state();
        }
        let mut closed = false;
        for w in &mut self.watches {
            let col = w.spec.column;
            if col >= self.n_columns {
                continue;
            }
            for f in 0..frames {
                let v = block[f * self.n_columns + col];
                w.win_sum += v;
                w.win_peak = w.win_peak.max(v.abs());
            }
            w.win_frames += frames as u64;
            w.pulse_v = w.pulse_v.max(w.win_peak);
            if w.win_frames < self.window_frames {
                continue;
            }
            // Close the window: its mean, how far that moved, and its peak.
            closed = true;
            let mean = w.win_sum / w.win_frames as f64;
            let drift = w.last_mean_v.map(|prev| (mean - prev).abs());
            let range = w.spec.range_v.abs();
            w.quiet = range > 0.0
                && drift.is_some_and(|d| d < SETTLE_DRIFT_FRACTION * range)
                && w.win_peak < SETTLE_PEAK_FRACTION * range;
            w.last_mean_v = Some(mean);
            w.last_drift_v = drift;
            w.last_peak_v = w.win_peak;
            w.win_frames = 0;
            w.win_sum = 0.0;
            w.win_peak = 0.0;
        }
        self.frames_seen += frames as u64;
        if closed && self.watches.iter().all(|w| w.quiet) {
            self.settled_at = Some(self.frames_seen);
        }
        self.state()
    }

    /// Where the settle stands, without feeding it.
    pub fn state(&self) -> SettleState {
        if self.settled_at.is_some() || self.watches.is_empty() {
            SettleState::Settled
        } else if self.frames_seen >= self.limit_frames {
            SettleState::TimedOut
        } else {
            SettleState::Pending
        }
    }

    /// One line per channel for the run log: the startup pulse, when the set was
    /// ready, and the offset and drift the channel was left at.
    pub fn report(&self) -> Vec<String> {
        self.watches
            .iter()
            .map(|w| {
                let when = match self.settled_at {
                    Some(f) => format!("settled; ready {:.2} s after its input started", f as f64 / self.fs_hz),
                    None => "did not settle".to_string(),
                };
                format!(
                    "IEPE {}: {when}; offset {}, moving {} per {:.2} s; the startup pulse peaked at \
                     {:.2} V of its +/-{} V range",
                    w.spec.name(),
                    volts(w.last_mean_v),
                    volts(w.last_drift_v),
                    self.window_frames as f64 / self.fs_hz,
                    w.pulse_v,
                    w.spec.range_v
                )
            })
            .collect()
    }

    /// The refusal for a settle that timed out, naming every channel that had
    /// not settled and what it was last reading.
    pub fn refusal(&self) -> String {
        let unsettled: Vec<String> = self
            .watches
            .iter()
            .filter(|w| !w.quiet)
            .map(|w| {
                format!(
                    "{} still reads an offset of {} moving {} per {:.2} s, and a {:.3} V peak, on \
                     its +/-{} V range",
                    w.spec.name(),
                    volts(w.last_mean_v),
                    volts(w.last_drift_v),
                    self.window_frames as f64 / self.fs_hz,
                    w.last_peak_v,
                    w.spec.range_v
                )
            })
            .collect();
        format!(
            "IEPE input did not settle within {:.0} s with nothing driving: {}. Check the \
             cable, the connector and the sensor, then arm again.",
            self.limit_frames as f64 / self.fs_hz,
            unsettled.join("; ")
        )
    }
}

/// A voltage for a message, in the unit that reads best; "unknown" before a
/// window has closed.
fn volts(v: Option<f64>) -> String {
    match v {
        None => "unknown".to_string(),
        Some(v) if v.abs() >= 0.1 => format!("{v:.3} V"),
        Some(v) if v.abs() >= 1e-4 => format!("{:.2} mV", v * 1e3),
        Some(v) => format!("{:.1} uV", v * 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8_533.333_333_333_334;
    const BLOCK: usize = 128;

    fn ch(index: u32, column: usize) -> SettleChannel {
        SettleChannel {
            physical: format!("cDAQ2Mod1/ai{}", index - 1),
            table_index: Some(index),
            column,
            range_v: 5.0,
        }
    }

    /// The bias step through a first-order high-pass, clipped at the rail the
    /// way a delta-sigma converter reads it: `bias * exp(-t / tau)`, at most
    /// 5.1237 V (the 512.37 g the bench recorded at 10 mV/g).
    fn coupled_step(bias_v: f64, tau_s: f64, t_s: f64) -> f64 {
        (bias_v * (-t_s / tau_s).exp()).min(5.1237)
    }

    /// Feed `columns` signals, each a function of time, until the settle leaves
    /// Pending; return the end state and the seconds watched.
    fn run(s: &mut IepeSettle, columns: &[&dyn Fn(f64) -> f64]) -> (SettleState, f64) {
        let n = columns.len();
        let mut frame = 0u64;
        loop {
            let mut block = vec![0.0; BLOCK * n];
            for f in 0..BLOCK {
                let t = (frame + f as u64) as f64 / FS;
                for (c, sig) in columns.iter().enumerate() {
                    block[f * n + c] = sig(t);
                }
            }
            frame += BLOCK as u64;
            let st = s.feed(&block, BLOCK);
            if st != SettleState::Pending {
                return (st, s.elapsed_s());
            }
        }
    }

    /// A 10 V bias behind a 0.5 Hz coupling (tau = 1 / pi s) rails the first
    /// block, then decays. Half-second windows move by about 0.8 of the offset
    /// left, so the drift is under 50 uV once the offset in the EARLIER window is under
    /// about 62 uV: ln(10 / 6.2e-5) / pi = 3.8 s at its middle, so ready at the
    /// close of the window after it, about 5 s. The
    /// report carries the pulse at the rail and the offset it was left at.
    #[test]
    fn a_ten_volt_bias_rails_then_settles_once_its_offset_stops_moving() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let (st, t) = run(
            &mut s,
            &[&|t| coupled_step(10.0, std::f64::consts::FRAC_1_PI, t)],
        );
        assert_eq!(st, SettleState::Settled);
        assert!((4.5..5.6).contains(&t), "ready at {t} s");
        let r = s.report().join("\n");
        assert!(r.contains("channel 1 (cDAQ2Mod1/ai0)"), "{r}");
        assert!(r.contains("peaked at 5.12 V"), "{r}");
        assert!(r.contains("settled; ready 5."), "{r}");
        assert!(r.contains("uV per 0.50 s"), "{r}");
    }

    /// The bench failure of 2026-11-23, as a test. A 10 mV/g accelerometer on the
    /// 9234 whose offset decays with a 0.72 s time constant (4.30 s to fall
    /// under 25 mV, as the bench saw). After the settle, the run's abort
    /// integrators (a first-order high-pass at 2 Hz, then two integrations,
    /// seeded on the first sample: `integrate_disp_mil`, which mirrors
    /// `hertzforge_engine::limits`) see what is left of the tail. Its
    /// displacement must stay well under the 200 mil limit Explorer aborted on.
    ///
    /// RED-MUTATION: put the old level criterion back (any window mean under
    /// 0.5 % of range counts, `SETTLE_DRIFT_FRACTION = 5e-3` on the drift
    /// here) and the run's displacement goes past 200 mil, as it did on the
    /// bench.
    #[test]
    fn what_the_settle_leaves_moves_the_displacement_abort_by_less_than_a_quarter_of_its_limit() {
        let tail = |t: f64| coupled_step(10.0, 0.72, t);
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let (st, t0) = run(&mut s, &[&tail]);
        assert_eq!(st, SettleState::Settled);
        let g_per_v = 1000.0 / 10.0; // 10 mV/g
        let peak_mil = integrate_disp_mil(|t| tail(t0 + t) * g_per_v, FS, 2.0, 10.0);
        assert!(
            peak_mil < 50.0,
            "{peak_mil} mil after a settle ready at {t0} s"
        );
        assert!((7.5..10.0).contains(&t0), "ready at {t0} s");
    }

    /// The block that aborted the first Explorer run is never called quiet: all
    /// 128 samples at the rail.
    #[test]
    fn a_block_at_the_rail_is_not_quiet() {
        let mut s = IepeSettle::with_times(vec![ch(1, 0)], 1, FS, 0.0, 1.0);
        assert_eq!(s.feed(&[5.1237; BLOCK], BLOCK), SettleState::Pending);
        assert_eq!(s.feed(&[5.1237; BLOCK], BLOCK), SettleState::Pending);
    }

    /// A channel that never comes off the rail (a circuit that is not there to
    /// discharge, or a sensor that is not IEPE) times out, and the refusal names
    /// it with what it was reading. A flat rail does not drift, so it is the
    /// peak that refuses it.
    ///
    /// RED-MUTATION: drop the peak test from a quiet window and this reds: a
    /// channel pinned at its rail is called settled at the second window.
    #[test]
    fn a_channel_that_stays_railed_refuses_and_says_what_it_reads() {
        let mut s = IepeSettle::with_times(vec![ch(1, 0)], 1, FS, SETTLE_WINDOW_S, 2.0);
        let (st, t) = run(&mut s, &[&|_| 5.1237]);
        assert_eq!(st, SettleState::TimedOut);
        assert!((2.0..2.1).contains(&t), "{t}");
        let msg = s.refusal();
        assert!(msg.contains("within 2 s"), "{msg}");
        assert!(
            msg.contains("channel 1 (cDAQ2Mod1/ai0) still reads an offset of 5.124 V"),
            "{msg}"
        );
        assert!(msg.contains("a 5.124 V peak"), "{msg}");
        assert!(s.report()[0].contains("did not settle"));
    }

    /// A constant offset is not motion: the abort integrators are seeded on the
    /// first sample, so the 9234's own offset error costs nothing, and the
    /// channel is ready at the second window.
    ///
    /// RED-MUTATION: judge the offset's level instead of its drift (a window
    /// mean under 50 uV) and this channel never settles.
    #[test]
    fn a_constant_offset_is_settled_at_the_second_window() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let (st, t) = run(&mut s, &[&|_| 0.003]);
        assert_eq!(st, SettleState::Settled);
        assert!(t < 2.0 * SETTLE_WINDOW_S + 0.1, "{t}");
        assert!(s.report()[0].contains("offset 3.00 mV"), "{:?}", s.report());
    }

    /// Ambient vibration does not hold a settled channel back: a large tone and a
    /// small one at a frequency that does not divide the window both average out
    /// of a half-second mean.
    #[test]
    fn zero_mean_vibration_does_not_hold_a_channel_back() {
        use std::f64::consts::TAU;
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let (st, t) = run(
            &mut s,
            &[&|t| 1.6 * (TAU * 400.0 * t).sin() + 0.002 * (TAU * 37.0 * t).sin()],
        );
        assert_eq!(st, SettleState::Settled);
        assert!(t < 2.0 * SETTLE_WINDOW_S + 0.1, "{t}");
    }

    /// A knock in a window spoils that window (its mean moved) and the next (its
    /// mean moved back), and the set is ready only after a quiet window beyond
    /// both.
    #[test]
    fn a_knock_spoils_its_window_and_the_next() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let knock = |t: f64| if (0.7..0.715).contains(&t) { 4.0 } else { 0.0 };
        let (st, t) = run(&mut s, &[&knock]);
        assert_eq!(st, SettleState::Settled);
        assert!(t > 1.9, "ready at {t} s, the knock was at 0.7 s");
    }

    /// Two IEPE inputs, each in its own column, with a single-ended column
    /// between them that is not watched: ready only when the slower one is.
    #[test]
    fn the_set_is_ready_when_its_slowest_channel_is() {
        let fast = |t: f64| coupled_step(10.0, 0.1, t);
        let other = |t: f64| 3.0 * (t * 0.7).sin();
        let slow = |t: f64| coupled_step(10.0, 0.5, t);
        let mut s = IepeSettle::new(vec![ch(1, 0), ch(3, 2)], 3, FS);
        let (st, t) = run(&mut s, &[&fast, &other, &slow]);
        assert_eq!(st, SettleState::Settled);
        // slow: drift 0.64 of the offset per window, under 50 uV once the offset
        // in the earlier window is under 78 uV: 0.5 * ln(10 / 7.8e-5) = 5.9 s at
        // its middle, ready at the close of the next
        assert!((6.4..7.6).contains(&t), "{t}");
        let r = s.report();
        assert!(
            r[0].contains("channel 1") && r[1].contains("channel 3"),
            "{r:?}"
        );
    }

    #[test]
    fn a_task_without_iepe_inputs_has_nothing_to_watch() {
        let s = IepeSettle::new(Vec::new(), 2, FS);
        assert!(s.is_empty());
        assert_eq!(s.state(), SettleState::Settled);
    }

    /// The abort integrators' displacement path, as `hertzforge_engine::limits`
    /// builds it: a first-order high-pass (bilinear) on acceleration seeded on the
    /// first sample, trapezoidal integration to velocity, the same high-pass on
    /// velocity, trapezoidal integration to displacement. Mirrored here because a
    /// backend does not depend on the engine. Returns the largest |displacement|
    /// in mil over `secs` of `accel_g(t)`.
    fn integrate_disp_mil(accel_g: impl Fn(f64) -> f64, fs: f64, hp_hz: f64, secs: f64) -> f64 {
        let k = (std::f64::consts::PI * hp_hz / fs).tan();
        let a = 1.0 / (1.0 + k);
        let b = (1.0 - k) / (1.0 + k);
        let dt = 1.0 / fs;
        let g = 386.088_582_677_165; // in/s^2 per g
        let (mut x1, mut y1, mut vel, mut va) = (accel_g(0.0) * g, 0.0, 0.0, 0.0);
        let (mut x2, mut y2, mut disp, mut dv) = (0.0, 0.0, 0.0f64, 0.0);
        let mut peak = 0.0f64;
        for n in 1..(secs * fs) as usize {
            let acc = accel_g(n as f64 * dt) * g;
            let hp_acc = a * (acc - x1) + b * y1;
            x1 = acc;
            y1 = hp_acc;
            vel += 0.5 * (va + hp_acc) * dt;
            va = hp_acc;
            let hp_vel = a * (vel - x2) + b * y2;
            x2 = vel;
            y2 = hp_vel;
            disp += 0.5 * (dv + hp_vel) * dt;
            dv = hp_vel;
            peak = peak.max(disp.abs() * 1000.0);
        }
        peak
    }
}
