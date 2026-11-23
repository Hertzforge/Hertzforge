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
//! # The criterion
//!
//! A block is QUIET on a channel when both hold, as fractions of that
//! channel's own `+/-` range in volts:
//!
//! - `|mean| < SETTLE_MEAN_FRACTION`: the decaying bias step is a DC offset,
//!   and the block mean is where it shows. Ambient vibration is zero-mean, so
//!   it does not hold the channel back.
//! - `peak < SETTLE_PEAK_FRACTION`: nothing near the rail.
//!
//! A channel is SETTLED once it has been quiet for [`SETTLE_HOLD_S`] without a
//! break; a loud block resets its count. The whole set is settled when every
//! channel is settled at the same time.
//!
//! The offset matters beyond the accel limit: an abort that integrates to
//! velocity or displacement turns a residual offset into a growing trip value,
//! which is why the mean threshold is tight (0.5 % of range, 25 mV on a +/-5 V
//! channel).
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

/// The block mean, as a fraction of the channel's range, below which the
/// coupling's offset counts as decayed.
pub const SETTLE_MEAN_FRACTION: f64 = 0.005;
/// The block peak, as a fraction of the channel's range, below which no sample
/// is near the rail.
pub const SETTLE_PEAK_FRACTION: f64 = 0.5;
/// How long a channel must stay quiet, without a break, to count as settled.
pub const SETTLE_HOLD_S: f64 = 0.25;
/// How long the whole set may take before the arm is refused.
pub const SETTLE_LIMIT_S: f64 = 15.0;

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
    /// Frames of unbroken quiet, up to now.
    quiet_frames: u64,
    /// The frame the current quiet run began at.
    quiet_from: u64,
    /// The largest |volts| seen since the watch began: the startup pulse.
    pulse_v: f64,
    /// The last block's mean and peak, in volts, for the refusal.
    last_mean_v: f64,
    last_peak_v: f64,
}

/// Watches every IEPE input of one AI task until each has settled.
#[derive(Debug, Clone)]
pub struct IepeSettle {
    watches: Vec<Watch>,
    n_columns: usize,
    fs_hz: f64,
    hold_frames: u64,
    limit_frames: u64,
    frames_seen: u64,
}

impl IepeSettle {
    /// Watch `channels` in blocks of `n_columns` interleaved columns at `fs_hz`,
    /// with the module's hold and limit.
    pub fn new(channels: Vec<SettleChannel>, n_columns: usize, fs_hz: f64) -> Self {
        Self::with_times(channels, n_columns, fs_hz, SETTLE_HOLD_S, SETTLE_LIMIT_S)
    }

    /// As [`IepeSettle::new`] with a stated hold and limit, in seconds.
    pub fn with_times(
        channels: Vec<SettleChannel>,
        n_columns: usize,
        fs_hz: f64,
        hold_s: f64,
        limit_s: f64,
    ) -> Self {
        let fs = if fs_hz > 0.0 { fs_hz } else { 1.0 };
        IepeSettle {
            watches: channels
                .into_iter()
                .map(|spec| Watch {
                    spec,
                    quiet_frames: 0,
                    quiet_from: 0,
                    pulse_v: 0.0,
                    last_mean_v: 0.0,
                    last_peak_v: 0.0,
                })
                .collect(),
            n_columns: n_columns.max(1),
            fs_hz: fs,
            hold_frames: (hold_s * fs).ceil().max(1.0) as u64,
            limit_frames: (limit_s * fs).ceil().max(1.0) as u64,
            frames_seen: 0,
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
        if frames > 0 {
            for w in &mut self.watches {
                let col = w.spec.column;
                if col >= self.n_columns {
                    continue;
                }
                let mut sum = 0.0;
                let mut peak = 0.0f64;
                for f in 0..frames {
                    let v = block[f * self.n_columns + col];
                    sum += v;
                    peak = peak.max(v.abs());
                }
                let mean = sum / frames as f64;
                w.pulse_v = w.pulse_v.max(peak);
                w.last_mean_v = mean;
                w.last_peak_v = peak;
                let range = w.spec.range_v.abs();
                let quiet = range > 0.0
                    && mean.abs() < SETTLE_MEAN_FRACTION * range
                    && peak < SETTLE_PEAK_FRACTION * range;
                if quiet {
                    if w.quiet_frames == 0 {
                        w.quiet_from = self.frames_seen;
                    }
                    w.quiet_frames += frames as u64;
                } else {
                    w.quiet_frames = 0;
                }
            }
            self.frames_seen += frames as u64;
        }
        self.state()
    }

    /// Where the settle stands, without feeding it.
    pub fn state(&self) -> SettleState {
        if self
            .watches
            .iter()
            .all(|w| w.quiet_frames >= self.hold_frames)
        {
            SettleState::Settled
        } else if self.frames_seen >= self.limit_frames {
            SettleState::TimedOut
        } else {
            SettleState::Pending
        }
    }

    /// One line per channel for the run log: the startup pulse and when the
    /// channel went quiet.
    pub fn report(&self) -> Vec<String> {
        self.watches
            .iter()
            .map(|w| {
                let quiet = if w.quiet_frames >= self.hold_frames {
                    format!(
                        "settled {:.2} s after its input started",
                        w.quiet_from as f64 / self.fs_hz
                    )
                } else {
                    "did not settle".to_string()
                };
                format!(
                    "IEPE {}: {quiet}; the startup pulse peaked at {:.2} V of its +/-{} V range",
                    w.spec.name(),
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
            .filter(|w| w.quiet_frames < self.hold_frames)
            .map(|w| {
                format!(
                    "{} still reads a {:.3} V offset and a {:.3} V peak on its +/-{} V range",
                    w.spec.name(),
                    w.last_mean_v,
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

    /// The bench's case: a 10 V bias behind a 0.5 Hz coupling (tau 0.318 s)
    /// rails the first block, then decays. It settles once the offset is under
    /// 25 mV, about 0.318 * ln(10 / 0.025) = 1.9 s, plus the hold; and the
    /// report carries the pulse at the rail.
    #[test]
    fn a_ten_volt_bias_rails_then_settles_in_about_two_seconds() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        // tau = 1 / (2 pi 0.5 Hz) = 1 / pi
        let (st, t) = run(
            &mut s,
            &[&|t| coupled_step(10.0, std::f64::consts::FRAC_1_PI, t)],
        );
        assert_eq!(st, SettleState::Settled);
        assert!((1.9..2.4).contains(&t), "ready at {t} s");
        let r = s.report().join("\n");
        assert!(r.contains("channel 1 (cDAQ2Mod1/ai0)"), "{r}");
        assert!(r.contains("peaked at 5.12 V"), "{r}");
        assert!(r.contains("settled 1.9"), "{r}");
    }

    /// The block that aborted the Explorer run is never called quiet: all 128
    /// samples at the rail.
    #[test]
    fn a_block_at_the_rail_is_not_quiet() {
        let mut s = IepeSettle::with_times(vec![ch(1, 0)], 1, FS, 0.0, 1.0);
        assert_eq!(s.feed(&[5.1237; BLOCK], BLOCK), SettleState::Pending);
    }

    /// A channel that never comes off the rail (a circuit that is not there to
    /// discharge, or a sensor that is not IEPE) times out, and the refusal names
    /// it with what it was reading.
    #[test]
    fn a_channel_that_stays_railed_refuses_and_says_what_it_reads() {
        let mut s = IepeSettle::with_times(vec![ch(1, 0)], 1, FS, SETTLE_HOLD_S, 2.0);
        let (st, t) = run(&mut s, &[&|_| 5.1237]);
        assert_eq!(st, SettleState::TimedOut);
        assert!((2.0..2.1).contains(&t), "{t}");
        let msg = s.refusal();
        assert!(msg.contains("within 2 s"), "{msg}");
        assert!(
            msg.contains("channel 1 (cDAQ2Mod1/ai0) still reads a 5.124 V offset"),
            "{msg}"
        );
        assert!(s.report()[0].contains("did not settle"));
    }

    /// Ambient vibration at a third of the range, zero-mean, does not hold a
    /// settled channel back: the criterion is the offset, not the motion.
    #[test]
    fn zero_mean_vibration_does_not_hold_a_channel_back() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let (st, t) = run(
            &mut s,
            &[&|t| 1.6 * (2.0 * std::f64::consts::PI * 400.0 * t).sin()],
        );
        assert_eq!(st, SettleState::Settled);
        assert!(t < SETTLE_HOLD_S + 0.05, "{t}");
    }

    /// A knock after the channel went quiet starts the hold again.
    ///
    /// RED-MUTATION: keep the quiet count through a loud block (drop the reset)
    /// and this reds: the knock at 0.2 s would be forgiven and the set would
    /// call itself settled at 0.25 s.
    #[test]
    fn a_disturbance_restarts_the_hold() {
        let mut s = IepeSettle::new(vec![ch(1, 0)], 1, FS);
        let knock = |t: f64| if (0.2..0.215).contains(&t) { 4.0 } else { 0.0 };
        let (st, t) = run(&mut s, &[&knock]);
        assert_eq!(st, SettleState::Settled);
        assert!(
            t > 0.2 + SETTLE_HOLD_S,
            "ready at {t} s, the knock was at 0.2 s"
        );
    }

    /// Two IEPE inputs, each in its own column, with a single-ended column
    /// between them that is not watched: ready only when the slower one is.
    #[test]
    fn the_set_is_ready_when_its_slowest_channel_is() {
        let fast = |t: f64| coupled_step(10.0, 0.1, t);
        let other = |_t: f64| 3.0;
        let slow = |t: f64| coupled_step(10.0, 0.5, t);
        let mut s = IepeSettle::new(vec![ch(1, 0), ch(3, 2)], 3, FS);
        let (st, t) = run(&mut s, &[&fast, &other, &slow]);
        assert_eq!(st, SettleState::Settled);
        // slow: 0.5 * ln(400) = 3.0 s, plus the hold
        assert!((3.0..3.4).contains(&t), "{t}");
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
}
