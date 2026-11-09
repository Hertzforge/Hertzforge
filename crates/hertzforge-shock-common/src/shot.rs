//! One shot of a pulse module, seen from the service that paces it.
//!
//! A host fire path takes each shot from a parked device: it arms the device,
//! commits the record's head, starts it, plays the record and parks again. The
//! arm and the commit take long enough on a real station to matter (about
//! 125 ms a shot on the NI loopback bench, 2026-11-07) and vary from shot to
//! shot. A service that waits out the time between pulses before the whole
//! fire puts all of that after the floor, so every interval runs long by it.
//!
//! The fire path therefore stops twice inside a fire and tells its caller:
//!
//! - [`ShotHooks::ready`], once the shot is brought up (armed, the record's head
//!   committed) and nothing has started. The service waits out the rest of the
//!   time between pulses here, so the start call is the only thing left on the
//!   deadline. Returning `false` withholds the shot: nothing plays, and the
//!   device stays armed with the record committed for the caller to park.
//! - [`ShotHooks::started`], once the start call has returned. A start call
//!   returns once the device's clock is running, so the record's first sample is
//!   out at or before this call, and the service stamps the pulse's start here.
//!   The next start call is issued no sooner than the time between pulses after
//!   that stamp, so the floor holds between the device's own starts.
//!
//! [`BringUp`] is the shot's timing, for the pulse's record and the service log.

use serde::{Deserialize, Serialize};

/// What a fire path reports once its shot is brought up, before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Prepared {
    /// The device's arm, seconds.
    pub arm_s: f64,
    /// The record's head committed to the device, its writes included, seconds.
    pub commit_s: f64,
}

/// The two moments a service needs from inside one fire (module docs).
pub trait ShotHooks {
    /// The shot is brought up and nothing has started. `true` starts it;
    /// `false` withholds it, leaving the device armed for the caller to park.
    fn ready(&mut self, prepared: Prepared) -> bool {
        let _ = prepared;
        true
    }

    /// The start call returned after `start_s` seconds: the clock is running.
    fn started(&mut self, start_s: f64) {
        let _ = start_s;
    }
}

/// Hooks that start every shot at once and note nothing: a fire nobody paces.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl ShotHooks for NoHooks {}

/// One pulse's bring-up, timed, milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BringUp {
    /// The drive's build and its checks, before the station is touched.
    pub checks_ms: f64,
    /// The station's arm.
    pub arm_ms: f64,
    /// The record's head committed to the station, its writes included.
    pub commit_ms: f64,
    /// The rest of the time between pulses, waited with the record committed.
    pub wait_ms: f64,
    /// The start call.
    pub start_ms: f64,
}

impl BringUp {
    /// The work done before the wait (the checks, the arm and the commit), ms:
    /// what a host that asks for the pulse ahead of its due time has to allow.
    pub fn ahead_ms(&self) -> f64 {
        self.checks_ms + self.arm_ms + self.commit_ms
    }

    /// One line for a service log.
    pub fn line(&self) -> String {
        format!(
            "checks {:.1} ms, arm {:.1} ms, commit {:.1} ms, wait {:.1} ms, start {:.1} ms",
            self.checks_ms, self.arm_ms, self.commit_ms, self.wait_ms, self.start_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_hooks_start_every_shot_and_the_bring_up_sums_the_work_before_the_wait() {
        let mut h = NoHooks;
        assert!(h.ready(Prepared::default()));
        h.started(0.001);
        let b = BringUp {
            checks_ms: 1.0,
            arm_ms: 20.0,
            commit_ms: 100.0,
            wait_ms: 4000.0,
            start_ms: 2.0,
        };
        assert_eq!(b.ahead_ms(), 121.0);
        assert_eq!(
            b.line(),
            "checks 1.0 ms, arm 20.0 ms, commit 100.0 ms, wait 4000.0 ms, start 2.0 ms"
        );
    }
}
