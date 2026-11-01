//! Predictive guards + reactive abort, DEVICE-NEUTRAL half.
//!
//! ## Predictive (before a level or frequency intent is acted on)
//!
//! [`PredictiveGuard`] refuses a proposed frequency outside `[low, high]` or a proposed
//! drive above either ceiling. This is the "never past the band edges; drive never above
//! the ceiling" rule, enforced at the service level so a jump/sweep/manual step cannot
//! overshoot.
//!
//! The two `StepModel`-typed helpers (`allowed_up_freq_nudges` /
//! `allowed_down_freq_nudges`) are not here: `StepModel` is a delegated-controller nudge
//! concept and lives with the delegated backend, as an extension trait on this type.
//!
//! ## Reactive (over acquired frames, against the channel-table abort limits)
//!
//! [`AbortMonitor`] runs the SHARED tri-field checker ([`hertzforge_engine::limits`]) over
//! interleaved frames against each channel's abort limits. On a trip the caller must
//! IMMEDIATELY stop (no ramp) and latch the run Aborted with the trip record. The
//! `max_velocity_ips` / `max_displacement_mil` PLANT limits are the velocity/displacement
//! axes of the SAME tri-field check (the integrator derives them from acceleration), so
//! they are enforced reactively here.
//!
//! ## Settle-awareness
//!
//! The velocity/displacement integrators have a startup high-pass transient. The
//! [`AbortMonitor`] holds a SETTLING flag for `settle_frames` (N high-pass time
//! constants) after arming, during which the derived-axis (velocity/displacement) trips
//! are SUPPRESSED so the HP transient cannot cause a spurious abort. The accel axis is
//! never suppressed (it is a direct compare with no integrator transient). After the
//! settle window a genuine derived-axis breach trips normally.

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_engine::limits::{AbortAxis, AbortTrip, ChannelLimiter};

/// The DOMAIN a drive number lives in.
///
/// The drive has two incompatible representations and no free conversion between them:
///
/// * [`DriveDomain::Estimate`] - dimensionless device EU. An open-loop model counts
///   nudges through a step model; the number is really a nudge-count proxy. ALWAYS
///   available, on any station, in any posture.
/// * [`DriveDomain::Volts`] - volts peak at the drive output. Only knowable when something
///   MEASURES it (a bench loopback), when a drive calibration has fitted the EU->volts
///   scale (`vpp_per_eu`), or when the host SYNTHESIZES the drive and therefore commands
///   volts natively (the host-closes-loop posture). In the field on an open-loop device,
///   with neither, volts are simply UNKNOWN.
///
/// This type exists because conflating the two is what froze the drive on 2026-07-16: a
/// ceiling labelled "5.0 V" was compared against ~4.7 EU and refused every further nudge.
/// A bare `f64` cannot say which domain it is in, so it travels with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveDomain {
    /// Volts peak at the drive output.
    Volts,
    /// Dimensionless open-loop model units (a nudge-count proxy). NOT volts.
    Estimate,
}

impl DriveDomain {
    /// The unit suffix to render after a number in this domain. NEVER returns "V" for
    /// [`DriveDomain::Estimate`] - that label is the original bug.
    pub fn unit_label(self) -> &'static str {
        match self {
            DriveDomain::Volts => "V pk",
            DriveDomain::Estimate => "est",
        }
    }
}

/// A drive quantity that carries its [`DriveDomain`], so a number can never silently cross
/// a unit boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveValue {
    pub value: f64,
    pub domain: DriveDomain,
}

impl DriveValue {
    /// A volts-peak drive value.
    pub fn volts(value: f64) -> Self {
        Self {
            value,
            domain: DriveDomain::Volts,
        }
    }

    /// An open-loop model (device EU) drive value.
    pub fn estimate(value: f64) -> Self {
        Self {
            value,
            domain: DriveDomain::Estimate,
        }
    }

    /// This value in VOLTS, using `vpp_per_eu` to convert an [`DriveDomain::Estimate`].
    /// `None` when the value is EU and no conversion is available - the honest answer, and
    /// the one that stops `est * vpp_per_eu / 2` being applied to a number that is already
    /// volts (the 2026-07-16 double-scale readout bug).
    pub fn to_volts(self, vpp_per_eu: Option<f64>) -> Option<f64> {
        match self.domain {
            DriveDomain::Volts => Some(self.value),
            DriveDomain::Estimate => vpp_per_eu.map(|k| (self.value * k) / 2.0),
        }
    }
}

/// The predictive guard: refuses intents that would exceed the frequency band edges or
/// either drive ceiling BEFORE they reach the device.
///
/// ## Two drive bounds, both always enforced
///
/// There is no single "the drive ceiling", because there is no single drive domain:
///
/// * [`max_drive_est`](Self::max_drive_est) bounds the OPEN-LOOP MODEL in device EU. It is
///   always enforceable (the model always exists) and is therefore the always-on runaway
///   backstop. It is what the pre-2026-07-16 `max_drive_v` actually bounded, despite its
///   name.
/// * [`max_drive_volts`](Self::max_drive_volts) bounds the real VOLTS, and is enforced only
///   on a poll where volts are knowable (measured, converted through a drive cal, or
///   commanded directly by a host that synthesizes the drive).
///
/// Both must pass. Keeping the EU bound always-on is what makes the volts bound safe to
/// leave unenforced when volts are unknown: a broken or absent drive measurement can never
/// open the drive up, because the model bound still clamps it.
#[derive(Debug, Clone, Copy)]
pub struct PredictiveGuard {
    pub low_hz: f64,
    pub high_hz: f64,
    /// The OPEN-LOOP MODEL ceiling, in device EU. Always enforced. NOT volts.
    pub max_drive_est: f64,
    /// The VOLTS ceiling. `None` = not set. Enforced only when volts are knowable.
    pub max_drive_volts: Option<f64>,
}

/// Why a predictive guard refused an intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRefusal {
    /// The frequency estimate would fall below the band low edge.
    BelowBand,
    /// The frequency estimate would rise above the band high edge.
    AboveBand,
    /// The open-loop DRIVE MODEL would exceed `max_drive_est` (device EU, NOT volts).
    OverDriveEstimate,
    /// The MEASURED / calibrated / commanded drive VOLTS would exceed `max_drive_volts`.
    OverDriveVolts,
}

impl PredictiveGuard {
    /// Build a guard for a `[low, high]` Hz band with an EU model ceiling and no volts
    /// ceiling. See [`Self::with_volts_ceiling`].
    pub fn new(low_hz: f64, high_hz: f64, max_drive_est: f64) -> Self {
        Self {
            low_hz,
            high_hz,
            max_drive_est,
            max_drive_volts: None,
        }
    }

    /// Attach the real VOLTS ceiling (enforced whenever volts are knowable).
    pub fn with_volts_ceiling(mut self, max_drive_volts: Option<f64>) -> Self {
        self.max_drive_volts = max_drive_volts.filter(|v| v.is_finite() && *v > 0.0);
        self
    }

    /// Check a proposed FREQUENCY estimate against the band. `Ok(())` to proceed, or the
    /// refusal reason.
    pub fn check_freq(&self, proposed_hz: f64) -> Result<(), GuardRefusal> {
        if proposed_hz < self.low_hz {
            Err(GuardRefusal::BelowBand)
        } else if proposed_hz > self.high_hz {
            Err(GuardRefusal::AboveBand)
        } else {
            Ok(())
        }
    }

    /// Check a proposed drive against BOTH ceilings.
    ///
    /// * `proposed_est` - the open-loop model's next value, device EU. Always checked
    ///   against [`Self::max_drive_est`].
    /// * `proposed_volts` - the next value in VOLTS when knowable (measured, converted
    ///   through a drive cal, or commanded), else `None`. Checked against
    ///   [`Self::max_drive_volts`] only when both are present.
    ///
    /// `None` volts does NOT open the drive up: the EU model bound above is unconditional,
    /// so an absent or broken drive measurement still clamps. This is why the volts bound
    /// can be skipped safely rather than having to refuse (refusing outright would deadlock
    /// every field run, where volts are never knowable).
    pub fn check_drive(
        &self,
        proposed_est: f64,
        proposed_volts: Option<f64>,
    ) -> Result<(), GuardRefusal> {
        if proposed_est > self.max_drive_est {
            return Err(GuardRefusal::OverDriveEstimate);
        }
        if let (Some(max_v), Some(v)) = (self.max_drive_volts, proposed_volts) {
            if v > max_v {
                return Err(GuardRefusal::OverDriveVolts);
            }
        }
        Ok(())
    }

    /// True when a VOLTS ceiling is set but this posture can never evaluate it - i.e. the
    /// operator asked for a limit the station cannot enforce. The caller surfaces this
    /// BEFORE the run rather than arming into a drive whose volts ceiling is silently
    /// inert. `volts_knowable` is "a bench loopback measurement is expected, a drive cal
    /// is installed, or the host commands volts directly".
    pub fn volts_ceiling_unenforceable(&self, volts_knowable: bool) -> bool {
        self.max_drive_volts.is_some() && !volts_knowable
    }
}

/// Why an [`AbortMonitor`] refused to be built.
///
/// Construction refuses rather than degrades. See [`AbortMonitor::new`] for why the
/// alternative (silently clamping the offending slot) was a safety defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortMonitorError {
    /// A limited, enabled, non-drive channel's interleave slot (`index - 1`) lies outside
    /// the acquired frame. Nothing in the frame carries that channel, so its limits cannot
    /// be checked at all.
    ChannelOutOfRange {
        /// The offending channel's 1-based table index.
        channel_index: u32,
        /// The slot that index derives to (`index - 1`).
        slot: usize,
        /// The interleave width of an acquired frame.
        n_interleave: usize,
    },
    /// The interleave width is zero, so there are no frames to check.
    ZeroInterleave,
}

impl std::fmt::Display for AbortMonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AbortMonitorError::ChannelOutOfRange {
                channel_index,
                slot,
                n_interleave,
            } => write!(
                f,
                "channel {channel_index} has abort limits but its interleave slot {slot} is \
                 outside the {n_interleave}-channel frame: its limits could not be checked"
            ),
            AbortMonitorError::ZeroInterleave => {
                write!(f, "abort monitor built with an interleave width of zero")
            }
        }
    }
}

impl std::error::Error for AbortMonitorError {}

/// The reactive abort monitor: one [`ChannelLimiter`] per enabled non-drive channel that
/// has any tri-field limit set, plus a settle window that suppresses derived-axis trips
/// during the integrator's startup transient.
pub struct AbortMonitor {
    limiters: Vec<ChannelLimiter>,
    /// The interleaved frame slot each limiter reads: channel `index` lives at slot
    /// `index - 1`, independent of which other channels are enabled. (Position-among-
    /// enabled slots, the pre-2026-07-15 scheme, monitored the WRONG channels whenever
    /// any channel was disabled.)
    slots: Vec<usize>,
    /// The interleave stride of an acquired frame, supplied by the caller.
    stride: usize,
    /// Frames remaining in the settle window (derived-axis trips suppressed while > 0).
    settle_remaining: u64,
    settle_frames: u64,
    frames_seen: u64,
    trip: Option<AbortTrip>,
}

impl AbortMonitor {
    /// Build the monitor from a channel table.
    ///
    /// `n_interleave` is the number of channels per acquired frame - the stride of the
    /// chunks that will be fed to [`feed`](Self::feed), and the bound every limited
    /// channel's slot must fall inside. It is a PARAMETER, not a constant: a delegated
    /// link decodes 8 physical channels every frame, an NI station decodes as many as its
    /// configured task has, and a monitor built for one width cannot read the other.
    /// `fs_hz` is the frame rate for the integrators, `hp_hz` the high-pass corner,
    /// `sustain` the debounce, and `settle_hp_constants` how many high-pass
    /// time-constants to hold the settle flag (derived-axis trips suppressed) after
    /// arming; a typical value is 5.
    ///
    /// ## An out-of-range channel is a REFUSAL, not a clamp
    ///
    /// A limited channel whose slot (`index - 1`) is `>= n_interleave` returns
    /// [`AbortMonitorError::ChannelOutOfRange`]. The earlier code instead wrote
    /// `.min(NCH - 1)`, which silently redirected that channel's limiter to the LAST
    /// slot in the frame. That is a safety defect, not a convenience: the operator sets a
    /// limit on channel 9, the monitor watches channel 8 and reports trips as channel 9,
    /// and the channel the operator asked to protect is not protected at all - with a
    /// green monitor claiming otherwise. There is no safe fallback here (no other slot
    /// carries that channel's signal), so construction fails and the caller surfaces it
    /// at configure time, before anything is armed.
    pub fn new(
        table: &ChannelTable,
        n_interleave: usize,
        fs_hz: f64,
        hp_hz: f64,
        sustain: u32,
        settle_hp_constants: f64,
    ) -> Result<Self, AbortMonitorError> {
        if n_interleave == 0 {
            return Err(AbortMonitorError::ZeroInterleave);
        }
        let mut limiters = Vec::new();
        let mut slots = Vec::new();
        for ch in &table.channels {
            if !ch.enabled {
                continue;
            }
            if ch.role == ChannelRole::Drive {
                continue; // outputs are not limit-monitored inputs
            }
            if !ChannelLimiter::has_limit(&ch.abort) {
                continue;
            }
            // Channel `index` is decoded at interleave slot `index - 1`, independent of
            // which other channels are enabled. Out of range = refuse; see the doc above.
            let slot = (ch.index as usize).saturating_sub(1);
            if slot >= n_interleave {
                return Err(AbortMonitorError::ChannelOutOfRange {
                    channel_index: ch.index,
                    slot,
                    n_interleave,
                });
            }
            limiters.push(ChannelLimiter::new(
                ch.index, ch.abort, fs_hz, hp_hz, sustain,
            ));
            slots.push(slot);
        }
        // Settle window = settle_hp_constants high-pass time constants. One HP time
        // constant is ~ 1/(2*pi*hp_hz) seconds -> * fs_hz frames.
        let tc_frames = if hp_hz > 0.0 {
            (fs_hz / (std::f64::consts::TAU * hp_hz)).max(0.0)
        } else {
            0.0
        };
        let settle_frames = (settle_hp_constants * tc_frames).round().max(0.0) as u64;
        Ok(Self {
            stride: n_interleave,
            limiters,
            slots,
            settle_remaining: settle_frames,
            settle_frames,
            frames_seen: 0,
            trip: None,
        })
    }

    /// True while the integrator settle window is still open (derived-axis trips
    /// suppressed). Exposed for the UI "settling" readout.
    pub fn settling(&self) -> bool {
        self.settle_remaining > 0
    }

    /// The number of frames the settle window spans (0 if no high-pass).
    pub fn settle_frames(&self) -> u64 {
        self.settle_frames
    }

    /// The interleave width this monitor expects per frame (the `n_interleave` it was
    /// built with).
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// The latched trip, if any has fired.
    pub fn trip(&self) -> Option<AbortTrip> {
        self.trip
    }

    /// Feed one interleaved chunk (`[c0f0, c1f0, ..., c0f1, ...]`,
    /// [`stride`](Self::stride) values per frame) through the tri-field check. Latches
    /// the FIRST trip and returns it. During the settle window, velocity/displacement
    /// trips are suppressed (the HP transient must not cause a spurious abort); the accel
    /// axis is always live.
    pub fn feed(&mut self, samples: &[f32]) -> Option<AbortTrip> {
        if self.trip.is_some() || self.stride == 0 {
            return self.trip;
        }
        let frames = samples.len() / self.stride;
        for f in 0..frames {
            self.frames_seen += 1;
            let settling = self.settle_remaining > 0;
            if settling {
                self.settle_remaining -= 1;
            }
            let frame = &samples[f * self.stride..f * self.stride + self.stride];
            for (lim, &slot) in self.limiters.iter_mut().zip(self.slots.iter()) {
                let accel = frame[slot] as f64;
                if let Some(t) = lim.push(accel, self.frames_seen) {
                    // During settle, suppress derived-axis (vel/disp) trips only.
                    if settling && matches!(t.axis, AbortAxis::Velocity | AbortAxis::Displacement) {
                        continue;
                    }
                    self.trip = Some(t);
                    return self.trip;
                }
            }
        }
        None
    }
}

/// Convenience: does a channel table have any tri-field limit on any enabled non-drive
/// channel? (Whether the abort monitor will monitor anything at all.)
pub fn table_has_any_limit(table: &ChannelTable) -> bool {
    table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != ChannelRole::Drive)
        .any(|c| ChannelLimiter::has_limit(&c.abort))
}

/// Re-export the shared axis type so the service/report can name it without a direct
/// `hertzforge-engine` dependency at the UI boundary.
pub use hertzforge_engine::limits::AbortAxis as TripAxis;

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
    use std::f64::consts::TAU;

    /// The delegated controller's interleave width, used as the stride in these tests so the moved
    /// behaviour is exercised at the width it was written against.
    const NCH: usize = 8;

    #[test]
    fn predictive_guard_band_and_drive() {
        let g = PredictiveGuard::new(20.0, 2000.0, 5.0);
        assert_eq!(g.check_freq(10.0), Err(GuardRefusal::BelowBand));
        assert_eq!(g.check_freq(3000.0), Err(GuardRefusal::AboveBand));
        assert!(g.check_freq(500.0).is_ok());
        // The EU MODEL ceiling: always enforced, volts irrelevant.
        assert_eq!(
            g.check_drive(6.0, None),
            Err(GuardRefusal::OverDriveEstimate)
        );
        assert!(g.check_drive(4.9, None).is_ok());
    }

    fn control_table_with_accel_limit(g_limit: f64) -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.abort = AbortLimits {
            accel_g: Some(g_limit),
            velocity_ips: None,
            displacement_mil: None,
        };
        t.channels.push(c);
        t
    }

    /// One `NCH`-wide frame with `value` on channel `index` (slot `index - 1`).
    fn frame_with(index: u32, value: f32) -> Vec<f32> {
        let mut f = vec![0.0f32; NCH];
        f[(index as usize) - 1] = value;
        f
    }

    #[test]
    fn accel_trip_fires_over_limit() {
        let table = control_table_with_accel_limit(10.0);
        let mut mon = AbortMonitor::new(&table, NCH, 65536.0, 5.0, 1, 5.0).unwrap();
        // One over-limit NCH-wide frame trips accel immediately (sustain 1).
        let trip = mon
            .feed(&frame_with(1, 50.0))
            .expect("over-limit accel trips");
        assert_eq!(trip.axis, AbortAxis::Accel);
        assert_eq!(trip.channel_index, 1);
        // Latched: a subsequent under-limit feed still returns the trip.
        assert!(mon.trip().is_some());
    }

    /// The 2026-07-15 stride regression: a limited channel keeps its PHYSICAL slot
    /// (`index - 1`) even when lower-index channels are disabled. The earlier
    /// position-among-enabled slots would have read CH1's slot for CH3 here.
    #[test]
    fn limited_channel_reads_its_physical_slot_when_others_are_disabled() {
        let mut t = ChannelTable::new();
        let mut ch1 = Channel::new(1);
        ch1.enabled = false; // disabled -> earlier code would shift every slot down
        let ch2 = Channel::new(2); // enabled, no limits
        let mut ch3 = Channel::new(3);
        ch3.role = ChannelRole::Control;
        ch3.abort = AbortLimits {
            accel_g: Some(10.0),
            ..Default::default()
        };
        t.channels.push(ch1);
        t.channels.push(ch2);
        t.channels.push(ch3);
        let mut mon = AbortMonitor::new(&t, NCH, 65536.0, 5.0, 1, 5.0).unwrap();
        // Over-limit value on CH1's PHYSICAL slot (0): must NOT trip (CH1 is disabled
        // and unlimited; the limiter watches CH3).
        assert!(mon.feed(&frame_with(1, 50.0)).is_none());
        // Over-limit value on CH3's physical slot (2): trips, attributed to CH3.
        let trip = mon.feed(&frame_with(3, 50.0)).expect("CH3 trips on slot 2");
        assert_eq!(trip.channel_index, 3);
    }

    /// The `.min(NCH - 1)` clamp is GONE. A limited channel whose slot falls outside
    /// the acquired frame is refused at construction; it used to be silently redirected to
    /// the last slot, which monitored the wrong channel under the right channel's name.
    #[test]
    fn out_of_range_limited_channel_is_refused_not_clamped() {
        let mut t = ChannelTable::new();
        // CH1 enabled and unlimited so the table is otherwise ordinary.
        t.channels.push(Channel::new(1));
        // CH9 is outside an 8-wide frame and CARRIES A LIMIT: nothing in the frame is its
        // signal, so there is no honest slot to read.
        let mut ch9 = Channel::new(9);
        ch9.role = ChannelRole::Control;
        ch9.abort = AbortLimits {
            accel_g: Some(10.0),
            ..Default::default()
        };
        t.channels.push(ch9);
        // `AbortMonitor` is deliberately not `Debug` (it was not before the move either),
        // so unwrap the error side by hand rather than reaching for `expect_err`.
        let err = AbortMonitor::new(&t, NCH, 65536.0, 5.0, 1, 5.0)
            .err()
            .expect("an out-of-range limited channel must refuse construction");
        assert_eq!(
            err,
            AbortMonitorError::ChannelOutOfRange {
                channel_index: 9,
                slot: 8,
                n_interleave: NCH,
            }
        );
        // The refusal is about the FRAME WIDTH, not the channel number: the same table is
        // accepted by a monitor wide enough to actually carry channel 9.
        let mon = AbortMonitor::new(&t, 16, 65536.0, 5.0, 1, 5.0)
            .expect("a 16-wide frame does carry channel 9");
        assert_eq!(mon.stride(), 16);
    }

    /// The stride is the caller's, not a constant: a narrow frame de-interleaves at the
    /// narrow width, and a channel that fits is monitored at its own slot.
    #[test]
    fn stride_follows_the_supplied_interleave_width() {
        let mut t = ChannelTable::new();
        let mut ch2 = Channel::new(2);
        ch2.role = ChannelRole::Control;
        ch2.abort = AbortLimits {
            accel_g: Some(10.0),
            ..Default::default()
        };
        t.channels.push(Channel::new(1));
        t.channels.push(ch2);
        let mut mon = AbortMonitor::new(&t, 2, 65536.0, 5.0, 1, 5.0).unwrap();
        assert_eq!(mon.stride(), 2);
        // Two 2-wide frames: CH1's slot hot in the first, CH2's slot hot in the second.
        assert!(mon.feed(&[50.0, 0.0]).is_none(), "CH1 is unlimited");
        let trip = mon.feed(&[0.0, 50.0]).expect("CH2 trips on slot 1");
        assert_eq!(trip.channel_index, 2);
    }

    #[test]
    fn zero_interleave_is_refused() {
        let t = control_table_with_accel_limit(10.0);
        assert_eq!(
            AbortMonitor::new(&t, 0, 65536.0, 5.0, 1, 5.0).err(),
            Some(AbortMonitorError::ZeroInterleave)
        );
    }

    #[test]
    fn no_spurious_velocity_trip_during_settle() {
        // A velocity limit with a DC-offset-plus-tone signal. The HP transient during
        // settle must NOT trip; after settle a steady modest signal stays under limit.
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.abort = AbortLimits {
            accel_g: None,
            velocity_ips: Some(5.0),
            displacement_mil: None,
        };
        t.channels.push(c);
        let fs = 65536.0;
        let mut mon = AbortMonitor::new(&t, NCH, fs, 5.0, 1, 5.0).unwrap();
        assert!(mon.settling(), "settle window open at start");
        // Feed a modest 0.5 g @ 250 Hz sine plus a DC offset for the whole settle window
        // and well beyond (CH1 slot of NCH-wide frames); no spurious trip.
        let n = (mon.settle_frames() + 20_000) as usize;
        let mut samples = vec![0.0f32; n * NCH];
        for (k, frame) in samples.chunks_exact_mut(NCH).enumerate() {
            let t = k as f64 / fs;
            frame[0] = (0.5 * (TAU * 250.0 * t).sin() + 0.3) as f32;
        }
        assert!(
            mon.feed(&samples).is_none(),
            "no spurious velocity trip during/after settle"
        );
    }

    #[test]
    fn velocity_trips_after_settle() {
        // A big low-frequency signal integrates to a large velocity that trips AFTER the
        // settle window closes. Prove the settle-suppression is temporary, not permanent.
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.role = ChannelRole::Control;
        c.abort = AbortLimits {
            accel_g: None,
            velocity_ips: Some(4.0),
            displacement_mil: None,
        };
        t.channels.push(c);
        let fs = 65536.0;
        // Small settle window so the trip lands after it in a reasonable sample count.
        let mut mon = AbortMonitor::new(&t, NCH, fs, 50.0, 1, 1.0).unwrap();
        let settle = mon.settle_frames();
        // A 5 g @ 30 Hz sine -> velocity amplitude ~ (5*386.09)/(2*pi*30) ~= 10 in/s,
        // well over the 4 in/s limit, tripping once the integrator has settled.
        let n = (settle + 40_000) as usize;
        let mut samples = vec![0.0f32; n * NCH];
        for (k, frame) in samples.chunks_exact_mut(NCH).enumerate() {
            let tt = k as f64 / fs;
            frame[0] = (5.0 * (TAU * 30.0 * tt).sin()) as f32;
        }
        let trip = mon.feed(&samples).expect("velocity trips after settle");
        assert_eq!(trip.axis, AbortAxis::Velocity);
        assert!(
            trip.at_frame > settle,
            "trip {} should be after the settle window {}",
            trip.at_frame,
            settle
        );
    }

    #[test]
    fn monitor_skips_drive_and_unlimited_channels() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.abort = AbortLimits {
            accel_g: Some(10.0),
            ..Default::default()
        };
        let mut mon_only = Channel::new(2); // monitor, no limits -> not monitored
        mon_only.role = ChannelRole::Monitor;
        let mut drive = Channel::new(3); // drive output -> never monitored
        drive.role = ChannelRole::Drive;
        drive.abort = AbortLimits {
            accel_g: Some(1.0),
            ..Default::default()
        };
        t.channels.push(ctrl);
        t.channels.push(mon_only);
        t.channels.push(drive);
        let mon = AbortMonitor::new(&t, NCH, 65536.0, 5.0, 1, 5.0).unwrap();
        // The chunk stride is the supplied interleave width regardless of the table.
        assert_eq!(mon.stride(), NCH);
        assert!(table_has_any_limit(&t));
    }

    /// A DRIVE-role row beyond the frame width must NOT refuse: outputs are never
    /// limit-monitored, so their index never derives a slot. (The delegated and NI tables both
    /// carry a drive row at index 9 today.)
    #[test]
    fn out_of_range_drive_row_does_not_refuse() {
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.abort = AbortLimits {
            accel_g: Some(10.0),
            ..Default::default()
        };
        let mut drv = Channel::new(9);
        drv.role = ChannelRole::Drive;
        drv.abort = AbortLimits {
            accel_g: Some(1.0),
            ..Default::default()
        };
        t.channels.push(ctrl);
        t.channels.push(drv);
        let mon = AbortMonitor::new(&t, NCH, 65536.0, 5.0, 1, 5.0)
            .expect("a drive row outside the frame is not monitored, so it cannot refuse");
        assert_eq!(mon.stride(), NCH);
    }
}
