//! Shared tri-field limit checking: the accel-direct-compare + HP-then-integrate
//! velocity/displacement streaming check, factored out of [`crate::watchdog`] so both
//! the [`crate::watchdog::AbortWatchdog`] (RAW-ring, `Backend`-driven abort) and the
//! delegated SMART run loop (in-loop check over decoded telemetry frames) use ONE
//! implementation.
//!
//! The two callers differ only in plumbing, not in the limit math:
//!
//! * `AbortWatchdog` owns a RAW-ring consumer and a `dyn Backend` it calls
//!   `emergency_stop()` on. It monitors many channels by frame offset.
//! * `DelegatedSmartRun` decodes telemetry frames and, per channel with limits set,
//!   checks the same tri-field. On a trip it sends the Stop block + `0x17` param=0
//!   itself (it is not a `Backend`, so it cannot use the watchdog's `emergency_stop`).
//!
//! Both feed acceleration (g) samples into a [`ChannelLimiter`] and act on the
//! returned [`AbortTrip`]. The integrator mirrors `hertzforge_dsp::integrate` (high-pass
//! the acceleration BEFORE integrating so a DC offset never becomes an unbounded
//! ramp), so the abort path and the analysis path agree on what "velocity" means.

use hertzforge_channel_table::AbortLimits;

/// Which axis of the tri-field limit tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortAxis {
    Accel,
    Velocity,
    Displacement,
}

/// A single tri-field limit trip: which channel/axis breached, the breaching value,
/// the limit, and the frame at which the sustained breach completed. The
/// [`crate::watchdog::AbortTrip`] mirrors these fields exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbortTrip {
    pub channel_index: u32,
    pub axis: AbortAxis,
    pub value: f64,
    pub limit: f64,
    /// Sample/frame index (within the run) at which the sustained breach completed.
    pub at_frame: u64,
}

const G_TO_IN_S2: f64 = 386.0886;

/// Streaming single/double integrator for one channel, mirroring
/// `hertzforge_dsp::integrate::accel_to_velocity` (first-order Butterworth high-pass on the
/// acceleration via bilinear transform, then trapezoidal integration) in a per-sample
/// streaming form. A second high-pass + integrate stage yields displacement,
/// high-passing the velocity before integrating again for the same drift reason.
#[derive(Debug, Clone)]
pub struct ChannelIntegrator {
    // high-pass 1 (on acceleration, in in/s^2)
    a: f64,
    b: f64,
    hp1_x_prev: f64,
    hp1_y_prev: f64,
    // trapezoidal velocity accumulator
    vel: f64,
    vel_acc_prev: f64,
    // high-pass 2 (on velocity)
    hp2_x_prev: f64,
    hp2_y_prev: f64,
    // trapezoidal displacement accumulator
    disp_in: f64,
    disp_vel_prev: f64,
    dt: f64,
    seeded: bool,
}

impl ChannelIntegrator {
    /// Build an integrator for a given sample rate and high-pass corner. Choose
    /// `hp_hz` a decade below the lowest band of interest.
    pub fn new(fs_hz: f64, hp_hz: f64) -> Self {
        let k = (std::f64::consts::PI * hp_hz / fs_hz).tan();
        let a = 1.0 / (1.0 + k);
        let b = (1.0 - k) / (1.0 + k);
        ChannelIntegrator {
            a,
            b,
            hp1_x_prev: 0.0,
            hp1_y_prev: 0.0,
            vel: 0.0,
            vel_acc_prev: 0.0,
            hp2_x_prev: 0.0,
            hp2_y_prev: 0.0,
            disp_in: 0.0,
            disp_vel_prev: 0.0,
            dt: 1.0 / fs_hz,
            seeded: false,
        }
    }

    /// Push one acceleration sample (in g). Returns `(velocity_ips, displacement_mil)`.
    /// Displacement is returned in mil (1 inch = 1000 mil) to match the tri-field.
    #[inline]
    pub fn push(&mut self, accel_g: f64) -> (f64, f64) {
        let acc = accel_g * G_TO_IN_S2; // in/s^2

        if !self.seeded {
            // Seed the HP state on the first sample, output 0 (mirrors the batch code).
            self.hp1_x_prev = acc;
            self.hp1_y_prev = 0.0;
            self.vel_acc_prev = 0.0;
            self.disp_vel_prev = 0.0;
            self.seeded = true;
            return (0.0, 0.0);
        }

        // High-pass the acceleration.
        let hp_acc = self.a * (acc - self.hp1_x_prev) + self.b * self.hp1_y_prev;
        self.hp1_x_prev = acc;
        self.hp1_y_prev = hp_acc;

        // Trapezoidal integrate to velocity (in/s).
        self.vel += 0.5 * (self.vel_acc_prev + hp_acc) * self.dt;
        self.vel_acc_prev = hp_acc;

        // High-pass the velocity, then integrate to displacement (inches).
        let hp_vel = self.a * (self.vel - self.hp2_x_prev) + self.b * self.hp2_y_prev;
        self.hp2_x_prev = self.vel;
        self.hp2_y_prev = hp_vel;
        self.disp_in += 0.5 * (self.disp_vel_prev + hp_vel) * self.dt;
        self.disp_vel_prev = hp_vel;

        (self.vel, self.disp_in * 1000.0) // mil
    }

    /// Reset integrator state across an epoch boundary or a marked time-gap:
    /// integrating across a discontinuity is exactly what the timebase rules forbid.
    pub fn reset(&mut self) {
        self.hp1_x_prev = 0.0;
        self.hp1_y_prev = 0.0;
        self.vel = 0.0;
        self.vel_acc_prev = 0.0;
        self.hp2_x_prev = 0.0;
        self.hp2_y_prev = 0.0;
        self.disp_in = 0.0;
        self.disp_vel_prev = 0.0;
        self.seeded = false;
    }
}

/// Check one axis against its optional limit with sustained-breach debounce. Bumps
/// the consecutive-over counter and returns a trip once it reaches `sustain`. A
/// below-limit sample resets the counter (the breach must be *consecutive*). `None`
/// limit means the axis never trips.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn check_axis(
    value_abs: f64,
    limit: Option<f64>,
    over_count: &mut u32,
    sustain: u32,
    axis: AbortAxis,
    channel_index: u32,
    at_frame: u64,
) -> Option<AbortTrip> {
    let limit = limit?; // no limit on this axis => never trips
    if value_abs > limit {
        *over_count += 1;
        if *over_count >= sustain {
            return Some(AbortTrip {
                channel_index,
                axis,
                value: value_abs,
                limit,
                at_frame,
            });
        }
    } else {
        *over_count = 0;
    }
    None
}

/// The full per-channel tri-field limiter: the [`ChannelIntegrator`] plus the
/// per-axis limits and sustained-breach counters. Feed it acceleration (g) samples
/// with [`ChannelLimiter::push`]; it integrates and returns the FIRST axis trip on
/// this sample (checking accel -> velocity -> displacement in order), or `None`.
///
/// This is the single object both the watchdog and the SMART run loop hold per
/// monitored channel, so the limit math lives in exactly one place.
#[derive(Debug, Clone)]
pub struct ChannelLimiter {
    channel_index: u32,
    limits: AbortLimits,
    integ: ChannelIntegrator,
    sustain: u32,
    over_accel: u32,
    over_vel: u32,
    over_disp: u32,
}

impl ChannelLimiter {
    /// Build a limiter for one channel. `sustain` is the debounce (1 =
    /// spike-sensitive; a genuine over-level persists for `sustain` consecutive
    /// samples before it trips). `sustain` is clamped to at least 1.
    pub fn new(
        channel_index: u32,
        limits: AbortLimits,
        fs_hz: f64,
        hp_hz: f64,
        sustain: u32,
    ) -> Self {
        ChannelLimiter {
            channel_index,
            limits,
            integ: ChannelIntegrator::new(fs_hz, hp_hz),
            sustain: sustain.max(1),
            over_accel: 0,
            over_vel: 0,
            over_disp: 0,
        }
    }

    /// Whether this channel has any tri-field limit set (else it is never monitored).
    pub fn has_limit(limits: &AbortLimits) -> bool {
        limits.accel_g.is_some()
            || limits.velocity_ips.is_some()
            || limits.displacement_mil.is_some()
    }

    /// Push one acceleration sample (g) at frame index `at_frame`. Returns the first
    /// axis trip found (accel direct-compare, then integrated velocity, then
    /// displacement), or `None`. The integrator advances every call regardless.
    #[inline]
    pub fn push(&mut self, accel_g: f64, at_frame: u64) -> Option<AbortTrip> {
        let (vel, disp) = self.integ.push(accel_g);
        if let Some(t) = check_axis(
            accel_g.abs(),
            self.limits.accel_g,
            &mut self.over_accel,
            self.sustain,
            AbortAxis::Accel,
            self.channel_index,
            at_frame,
        ) {
            return Some(t);
        }
        if let Some(t) = check_axis(
            vel.abs(),
            self.limits.velocity_ips,
            &mut self.over_vel,
            self.sustain,
            AbortAxis::Velocity,
            self.channel_index,
            at_frame,
        ) {
            return Some(t);
        }
        check_axis(
            disp.abs(),
            self.limits.displacement_mil,
            &mut self.over_disp,
            self.sustain,
            AbortAxis::Displacement,
            self.channel_index,
            at_frame,
        )
    }

    /// Reset the integrator and the breach counters (call across an epoch boundary or
    /// a marked gap). Does not clear any latch the caller keeps.
    pub fn reset(&mut self) {
        self.integ.reset();
        self.over_accel = 0;
        self.over_vel = 0;
        self.over_disp = 0;
    }

    /// The channel index this limiter monitors.
    pub fn channel_index(&self) -> u32 {
        self.channel_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(accel: Option<f64>, vel: Option<f64>, disp: Option<f64>) -> AbortLimits {
        AbortLimits {
            accel_g: accel,
            velocity_ips: vel,
            displacement_mil: disp,
        }
    }

    #[test]
    fn accel_trips_on_sustained_over_limit() {
        let mut lim = ChannelLimiter::new(1, limits(Some(10.0), None, None), 1000.0, 5.0, 3);
        // One stray sample must NOT trip (debounce).
        assert!(lim.push(50.0, 0).is_none());
        // Back under: counter resets.
        assert!(lim.push(1.0, 1).is_none());
        // Three consecutive over-limit samples trip on the third.
        assert!(lim.push(50.0, 2).is_none());
        assert!(lim.push(50.0, 3).is_none());
        let trip = lim
            .push(50.0, 4)
            .expect("should trip on the third consecutive");
        assert_eq!(trip.axis, AbortAxis::Accel);
        assert_eq!(trip.channel_index, 1);
        assert_eq!(trip.value, 50.0);
        assert_eq!(trip.limit, 10.0);
        assert_eq!(trip.at_frame, 4);
    }

    #[test]
    fn accel_spike_trips_immediately_at_sustain_1() {
        let mut lim = ChannelLimiter::new(2, limits(Some(10.0), None, None), 1000.0, 5.0, 1);
        for n in 0..5 {
            assert!(lim.push(3.0, n).is_none());
        }
        let trip = lim.push(42.0, 5).expect("spike trips at sustain=1");
        assert_eq!(trip.axis, AbortAxis::Accel);
        assert_eq!(trip.at_frame, 5);
    }

    #[test]
    fn velocity_trips_via_integration() {
        // A 2 g, 50 Hz sine integrates to a velocity of amplitude ~ (2*386.09)/(2*pi*50)
        // ~= 2.46 in/s; the peak observed after HP transient is ~4.7 in/s, so a 4.0 in/s
        // limit trips even though the per-sample accel (2 g) is under any plausible g
        // limit. This proves the integrated velocity PATH catches this class.
        let fs = 1000.0;
        let mut lim = ChannelLimiter::new(1, limits(None, Some(4.0), None), fs, 1.0, 1);
        let mut trip = None;
        for n in 0..500u64 {
            let t = n as f64 / fs;
            let accel = 2.0 * (2.0 * std::f64::consts::PI * 50.0 * t).sin();
            if let Some(tr) = lim.push(accel, n) {
                trip = Some(tr);
                break;
            }
        }
        let trip = trip.expect("velocity should trip");
        assert_eq!(trip.axis, AbortAxis::Velocity);
    }

    #[test]
    fn no_spurious_trip_during_hp_settle() {
        // The HP transient at the start of a run must not spuriously trip a velocity
        // limit while the integrator settles. Feed a modest steady sine that never
        // exceeds the (generous) velocity limit; assert no trip across the settle
        // window and beyond. A DC-offset accel is what would ramp without the HP; the
        // HP-then-integrate design keeps velocity bounded, so no settle-window trip.
        let fs = 1000.0;
        // 0.5 g @ 50 Hz -> ~0.6 in/s peak velocity, well under a 5.0 in/s limit.
        let mut lim = ChannelLimiter::new(1, limits(None, Some(5.0), None), fs, 1.0, 1);
        for n in 0..2000u64 {
            let t = n as f64 / fs;
            // Include a constant DC offset that, without the high-pass, would integrate
            // into an unbounded velocity ramp and trip. The HP removes it.
            let accel = 0.5 * (2.0 * std::f64::consts::PI * 50.0 * t).sin() + 0.3;
            assert!(
                lim.push(accel, n).is_none(),
                "no spurious trip during/after HP settle at frame {n}"
            );
        }
    }

    #[test]
    fn no_limit_never_trips() {
        let mut lim = ChannelLimiter::new(1, limits(None, None, None), 1000.0, 5.0, 1);
        for n in 0..50 {
            assert!(lim.push(1e6, n).is_none());
        }
        assert!(!ChannelLimiter::has_limit(&limits(None, None, None)));
        assert!(ChannelLimiter::has_limit(&limits(Some(1.0), None, None)));
    }

    #[test]
    fn reset_clears_counters_and_integrator() {
        let mut lim = ChannelLimiter::new(1, limits(Some(10.0), None, None), 1000.0, 5.0, 3);
        // Two consecutive over-limit samples (counter = 2, no trip yet).
        assert!(lim.push(50.0, 0).is_none());
        assert!(lim.push(50.0, 1).is_none());
        lim.reset();
        // After reset the counter is 0, so a single over-limit sample does not trip.
        assert!(lim.push(50.0, 2).is_none());
    }
}
