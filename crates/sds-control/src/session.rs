//! Controller and observers (schema/PROTOCOL.md, "Controller and Observers").
//!
//! One connection at a time holds control; every other connection observes. This
//! module is the seat's bookkeeping and nothing else: who holds it, the lease that
//! resumes it, and the orphan clock that ends an unattended run. It never touches a
//! run itself; [`Session::expire`] says when the service must stop one. Time is
//! passed in, so every rule here is tested without waiting.

use crate::protocol::{ErrorCode, MethodError};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// A connection's identity inside this process.
pub type ConnId = u64;

/// How often the service pings each connection.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// How long a seat or an unattended run is kept for a client to claim.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Heartbeats with nothing inbound before a connection is treated as gone.
pub const MISSED_HEARTBEATS: u32 = 3;

struct Seat {
    /// The holding connection, or `None` while its holder is absent (dropped without
    /// a goodbye; the seat is kept for the timeout so the same lease can resume it).
    conn: Option<ConnId>,
    name: String,
    lease: String,
}

/// The result of a successful `session.claim`.
#[derive(Debug, Clone, PartialEq)]
pub struct Claimed {
    pub lease: String,
    pub resumed: bool,
}

/// What [`Session::expire`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Expiry {
    /// The seat or the clock changed, so `session.changed` is due.
    pub changed: bool,
    /// A run is live and nobody holds control: end it with the controlled stop.
    pub safe_stop: bool,
}

/// The seat, its lease and the orphan clock.
pub struct Session {
    seat: Option<Seat>,
    leases: u64,
    /// When the orphan clock runs out, if it is running.
    deadline: Option<Instant>,
    heartbeat: Duration,
    timeout: Duration,
}

impl Session {
    pub fn new(heartbeat: Duration, timeout: Duration) -> Session {
        Session {
            seat: None,
            leases: 0,
            deadline: None,
            heartbeat,
            timeout,
        }
    }

    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Does `conn` hold control?
    pub fn is_controller(&self, conn: ConnId) -> bool {
        self.seat.as_ref().is_some_and(|s| s.conn == Some(conn))
    }

    /// The holder's name, present or absent.
    pub fn holder(&self) -> Option<&str> {
        self.seat.as_ref().map(|s| s.name.as_str())
    }

    /// Refuse `not_controller` unless `conn` holds control.
    pub fn require_controller(&self, conn: ConnId) -> Result<(), MethodError> {
        if self.is_controller(conn) {
            return Ok(());
        }
        let holder = self.holder().map(str::to_owned);
        let message = match &holder {
            Some(h) => format!("{h} controls this service; take control once it is released."),
            None => "This screen does not hold control; take control first.".to_owned(),
        };
        Err(
            MethodError::named(ErrorCode::Refused, "not_controller", message)
                .with_detail(json!({ "holder": holder })),
        )
    }

    /// The `controller` value: `{"name","present"}`, or null.
    pub fn controller_view(&self) -> Value {
        match &self.seat {
            Some(s) => json!({"name": s.name, "present": s.conn.is_some()}),
            None => Value::Null,
        }
    }

    /// Seconds left before an unattended live run is stopped, or `None`.
    pub fn orphan_s(&self, now: Instant, live: bool) -> Option<f64> {
        let deadline = self.deadline?;
        if !live {
            return None;
        }
        let left = deadline.saturating_duration_since(now).as_secs_f64();
        Some((left * 10.0).round() / 10.0)
    }

    /// The `session.state` result for `conn`.
    pub fn state(&self, conn: ConnId, now: Instant, live: bool) -> Value {
        json!({
            "role": if self.is_controller(conn) { "controller" } else { "observer" },
            "controller": self.controller_view(),
            "orphan_s": self.orphan_s(now, live),
            "heartbeat_s": self.heartbeat.as_secs_f64(),
            "timeout_s": self.timeout.as_secs_f64(),
        })
    }

    /// The `session.changed` event data.
    pub fn changed(&self, now: Instant, live: bool) -> Value {
        json!({"controller": self.controller_view(), "orphan_s": self.orphan_s(now, live)})
    }

    /// `session.claim`. A claim while another CONNECTED client holds control is
    /// refused `controller_held`; there is no forced takeover. A claim that presents
    /// an absent holder's lease within the timeout resumes it; any other claim takes
    /// the seat with a new lease.
    pub fn claim(
        &mut self,
        conn: ConnId,
        name: Option<&str>,
        lease: Option<&str>,
        default_name: &str,
        now: Instant,
    ) -> Result<Claimed, MethodError> {
        if let Some(seat) = &self.seat {
            if let Some(holder) = seat.conn {
                if holder == conn {
                    return Ok(Claimed {
                        lease: seat.lease.clone(),
                        resumed: false,
                    });
                }
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "controller_held",
                    format!(
                        "Another client controls this service ({}). Close that client, or \
                         wait for it to release control.",
                        seat.name
                    ),
                )
                .with_detail(json!({"holder": seat.name})));
            }
        }
        let in_time = self.deadline.is_none_or(|d| now < d);
        let resumed = match (&self.seat, lease) {
            (Some(seat), Some(l)) => seat.conn.is_none() && seat.lease == l && in_time,
            _ => false,
        };
        let lease = if resumed {
            self.seat
                .as_ref()
                .map(|s| s.lease.clone())
                .unwrap_or_default()
        } else {
            self.leases += 1;
            new_lease(self.leases)
        };
        let name = name.map(str::to_owned).unwrap_or_else(|| {
            if resumed {
                self.seat
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_default()
            } else {
                default_name.to_owned()
            }
        });
        self.seat = Some(Seat {
            conn: Some(conn),
            name,
            lease: lease.clone(),
        });
        self.deadline = None;
        Ok(Claimed { lease, resumed })
    }

    /// `session.release`, or a clean close (a goodbye): control is free at once, and a
    /// live run is kept for the timeout for a client to claim. True if anything changed.
    pub fn release(&mut self, conn: ConnId, now: Instant, live: bool) -> bool {
        if !self.is_controller(conn) {
            return false;
        }
        self.seat = None;
        self.deadline = live.then(|| now + self.timeout);
        true
    }

    /// The controlling connection went without a goodbye: the seat is kept, absent,
    /// for the timeout, and the run (if any) goes on. True if anything changed.
    pub fn dropped(&mut self, conn: ConnId, now: Instant) -> bool {
        match self.seat.as_mut() {
            Some(seat) if seat.conn == Some(conn) => {
                seat.conn = None;
                self.deadline = Some(now + self.timeout);
                true
            }
            _ => false,
        }
    }

    /// Run the orphan clock. At the deadline an absent seat is given up, and a live
    /// run with nobody present in control is due its safe stop.
    pub fn expire(&mut self, now: Instant, live: bool) -> Expiry {
        let Some(deadline) = self.deadline else {
            return Expiry::default();
        };
        if now < deadline {
            return Expiry::default();
        }
        self.deadline = None;
        if self.seat.as_ref().is_some_and(|s| s.conn.is_none()) {
            self.seat = None;
        }
        let present = self.seat.as_ref().is_some_and(|s| s.conn.is_some());
        Expiry {
            changed: true,
            safe_stop: live && !present,
        }
    }

    /// When the orphan clock runs out, if it is running.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// How the run log names a connection: the controller by its name, others as an
    /// observer at their peer address.
    pub fn client_name(&self, conn: ConnId, peer: &str) -> String {
        match &self.seat {
            Some(s) if s.conn == Some(conn) => format!("{} (controller)", s.name),
            _ => format!("an observer at {peer}"),
        }
    }
}

/// A lease token: unique in this process and not guessable from the count alone.
fn new_lease(n: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mix =
        (nanos as u64) ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ u64::from(std::process::id());
    format!("L{n}-{mix:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(HEARTBEAT, TIMEOUT)
    }

    #[test]
    fn every_connection_starts_as_an_observer_and_one_claim_wins() {
        let mut s = session();
        let t = Instant::now();
        assert!(!s.is_controller(1));
        assert_eq!(s.controller_view(), Value::Null);
        let c = s.claim(1, Some("Shell"), None, "peer", t).unwrap();
        assert!(!c.resumed);
        assert!(s.is_controller(1));
        let e = s.claim(2, None, None, "peer2", t).unwrap_err();
        assert_eq!(e.reason, Some("controller_held"));
        assert_eq!(e.detail.unwrap()["holder"], "Shell");
        let e = s.require_controller(2).unwrap_err();
        assert_eq!(e.reason, Some("not_controller"));
    }

    #[test]
    fn a_dropped_holder_resumes_with_its_lease_within_the_timeout() {
        let mut s = session();
        let t = Instant::now();
        let lease = s.claim(1, Some("Shell"), None, "peer", t).unwrap().lease;
        assert!(s.dropped(1, t));
        assert_eq!(
            s.controller_view(),
            json!({"name": "Shell", "present": false})
        );
        assert_eq!(s.orphan_s(t, true), Some(30.0));
        assert_eq!(
            s.orphan_s(t, false),
            None,
            "no countdown without a live run"
        );
        let c = s
            .claim(2, None, Some(&lease), "peer2", t + Duration::from_secs(10))
            .unwrap();
        assert!(c.resumed);
        assert_eq!(c.lease, lease);
        assert_eq!(s.holder(), Some("Shell"), "the resumed seat keeps its name");
        assert!(s.deadline().is_none());
    }

    #[test]
    fn a_claim_without_the_lease_takes_control_from_an_absent_holder() {
        let mut s = session();
        let t = Instant::now();
        let lease = s.claim(1, Some("Shell"), None, "peer", t).unwrap().lease;
        s.dropped(1, t);
        let c = s.claim(2, Some("Other"), None, "peer2", t).unwrap();
        assert!(!c.resumed);
        assert_ne!(c.lease, lease);
        assert_eq!(s.holder(), Some("Other"));
    }

    #[test]
    fn a_stale_lease_does_not_resume() {
        let mut s = session();
        let t = Instant::now();
        let lease = s.claim(1, None, None, "peer", t).unwrap().lease;
        s.dropped(1, t);
        let late = t + TIMEOUT + Duration::from_millis(1);
        let c = s.claim(2, None, Some(&lease), "peer2", late).unwrap();
        assert!(!c.resumed);
    }

    #[test]
    fn nobody_claiming_a_live_run_in_time_is_a_safe_stop() {
        let mut s = session();
        let t = Instant::now();
        s.claim(1, None, None, "peer", t).unwrap();
        s.dropped(1, t);
        assert_eq!(
            s.expire(t + Duration::from_secs(29), true),
            Expiry::default()
        );
        let e = s.expire(t + TIMEOUT, true);
        assert!(e.changed && e.safe_stop);
        assert_eq!(
            s.controller_view(),
            Value::Null,
            "the absent seat is given up"
        );
    }

    #[test]
    fn a_goodbye_frees_control_at_once_and_keeps_a_live_run_for_the_timeout() {
        let mut s = session();
        let t = Instant::now();
        s.claim(1, None, None, "peer", t).unwrap();
        assert!(s.release(1, t, true));
        assert_eq!(s.controller_view(), Value::Null);
        assert_eq!(s.orphan_s(t, true), Some(30.0));
        // Someone claims in time: no safe stop.
        s.claim(2, None, None, "peer2", t + Duration::from_secs(5))
            .unwrap();
        assert_eq!(s.expire(t + TIMEOUT, true), Expiry::default());
    }

    #[test]
    fn a_goodbye_with_no_live_run_starts_no_clock() {
        let mut s = session();
        let t = Instant::now();
        s.claim(1, None, None, "peer", t).unwrap();
        s.release(1, t, false);
        assert!(s.deadline().is_none());
        assert!(!s.expire(t + TIMEOUT, false).safe_stop);
    }

    #[test]
    fn an_observer_release_changes_nothing() {
        let mut s = session();
        let t = Instant::now();
        s.claim(1, None, None, "peer", t).unwrap();
        assert!(!s.release(2, t, true));
        assert!(s.is_controller(1));
    }
}
