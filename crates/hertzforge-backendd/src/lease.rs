//! The RUN LEASE registry: one Controller, N Observers, and the vanish clock.
//!
//! The policy lives in `hertzforge_protocol::session`; this is the state machine that
//! enforces it. Three rules, each with a test that reds when it is removed:
//!
//! 1. **A second Controller is REFUSED** unless it asked for the downgrade
//!    (`Hello::accept_observer_downgrade`), in which case it becomes an Observer and
//!    is TOLD so.
//! 2. **A vanished Controller does not end the run.** The lease stays assigned and a
//!    deadline is armed; the run continues. A reconnect with the same [`LeaseId`]
//!    resumes control and disarms the deadline.
//! 3. **When the deadline passes, the daemon SAFE-STOPS** - the controlled ramp-down
//!    path, never an abort.
//!
//! Time is a parameter, not a global: every method takes `now: Instant`. A lease
//! machine that reads the clock itself can only be tested by sleeping, and a test that
//! sleeps for 30 seconds is a test nobody runs.

use std::time::{Duration, Instant};

use hertzforge_protocol::{LeaseAnswer, LeaseId, ObserverReason, RunRole};

/// Identifies one live connection inside the registry.
pub type ConnId = u64;

/// The lease state machine.
#[derive(Debug)]
pub struct LeaseRegistry {
    next_lease: u64,
    /// The current lease, if one has ever been granted.
    current: Option<Lease>,
    /// How long a lease survives its holder's silence before the safe-stop.
    timeout: Duration,
    /// A run whose controller LEFT CLEANLY: the lease is already released (the
    /// next controller may connect immediately), but the run still needs an
    /// owner - if nobody takes it before this deadline, the safe-stop fires.
    /// `(who left, deadline)`.
    orphan: Option<(String, Instant)>,
}

/// One granted lease.
#[derive(Debug, Clone)]
struct Lease {
    id: LeaseId,
    /// Human-readable identity of the holder (the peer address), for messages.
    who: String,
    /// The connection holding it, or `None` while the holder is GONE and the
    /// deadline is running.
    conn: Option<ConnId>,
    /// When the lease expires if nobody reclaims it.
    deadline: Option<Instant>,
}

/// What [`LeaseRegistry::admit`] decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// Admitted; here is the answer to put in the `HelloAck`.
    Admitted(LeaseAnswer),
    /// Refused: a Controller already holds the lease and this peer did not accept a
    /// downgrade. `String` names the holder.
    RefusedControllerHeld(String),
}

impl LeaseRegistry {
    /// A registry with the configured controller timeout.
    pub fn new(timeout: Duration) -> Self {
        LeaseRegistry {
            next_lease: 1,
            current: None,
            timeout,
            orphan: None,
        }
    }

    /// Who holds the lease right now, for messages. Empty when nobody does.
    pub fn holder(&self) -> String {
        self.current
            .as_ref()
            .map(|l| l.who.clone())
            .unwrap_or_default()
    }

    /// Is `conn` the Controller?
    pub fn is_controller(&self, conn: ConnId) -> bool {
        self.current
            .as_ref()
            .and_then(|l| l.conn)
            .is_some_and(|c| c == conn)
    }

    /// The role `conn` holds.
    pub fn role_of(&self, conn: ConnId) -> RunRole {
        if self.is_controller(conn) {
            RunRole::Controller
        } else {
            RunRole::Observer
        }
    }

    /// Admit a connection with the role it asked for.
    ///
    /// `reclaim` is the [`LeaseId`] a reconnecting peer presents; a peer that
    /// presents the CURRENT lease id gets it back even while the deadline is running,
    /// which is what makes a network hiccup recoverable instead of terminal.
    pub fn admit(
        &mut self,
        conn: ConnId,
        who: &str,
        asked: RunRole,
        accept_downgrade: bool,
        reclaim: Option<LeaseId>,
        now: Instant,
    ) -> Admission {
        self.expire_if_due(now);

        if asked == RunRole::Observer {
            return Admission::Admitted(LeaseAnswer::Observer {
                reason: ObserverReason::Requested,
                controller: self
                    .current
                    .as_ref()
                    .and_then(|l| l.conn.map(|_| l.who.clone())),
            });
        }

        match &mut self.current {
            // A live Controller holds it.
            Some(l) if l.conn.is_some() => {
                let holder = l.who.clone();
                if accept_downgrade {
                    Admission::Admitted(LeaseAnswer::Observer {
                        reason: ObserverReason::Downgraded,
                        controller: Some(holder),
                    })
                } else {
                    Admission::RefusedControllerHeld(holder)
                }
            }
            // A lease exists but its holder is GONE (the deadline is running). The
            // ORIGINAL holder gets it back by presenting the id; anyone else is
            // treated as a fresh Controller request and takes over, because the
            // alternative - making a new operator wait out someone else's timeout on
            // a machine that already lost its network - helps nobody.
            Some(l) => {
                let reclaiming = reclaim == Some(l.id);
                l.conn = Some(conn);
                l.deadline = None;
                if !reclaiming {
                    l.who = who.to_string();
                }
                Admission::Admitted(LeaseAnswer::Controller {
                    lease: l.id,
                    timeout_s: self.timeout.as_secs_f64(),
                })
            }
            None => {
                let id = LeaseId(self.next_lease);
                self.next_lease += 1;
                self.current = Some(Lease {
                    id,
                    who: who.to_string(),
                    conn: Some(conn),
                    deadline: None,
                });
                // A new controller ADOPTS any orphaned run: it has an owner
                // again, so the orphan safe-stop clock disarms.
                self.orphan = None;
                Admission::Admitted(LeaseAnswer::Controller {
                    lease: id,
                    timeout_s: self.timeout.as_secs_f64(),
                })
            }
        }
    }

    /// A connection said GOODBYE (an explicit WebSocket Close).
    ///
    /// If it was the Controller, the lease is RELEASED IMMEDIATELY - the operator
    /// chose to leave, and making their own next connection (or anyone else's)
    /// wait out a vanish timeout for a peer that said goodbye serves nobody. The
    /// operator hit exactly this on 2026-08-01: refused as OBSERVER because "the
    /// run lease is held by" their own just-closed session.
    ///
    /// The RUN still needs an owner, so the same deadline is armed as an ORPHAN
    /// clock: if no controller adopts the run before it passes, the safe-stop
    /// fires exactly as it would for a vanish. Returns `true` when the clock was
    /// armed (the closer was the Controller).
    pub fn on_clean_close(&mut self, conn: ConnId, now: Instant) -> bool {
        match &self.current {
            Some(l) if l.conn == Some(conn) => {
                let who = l.who.clone();
                self.current = None;
                self.orphan = Some((who, now + self.timeout));
                true
            }
            _ => false,
        }
    }

    /// A connection went away WITHOUT saying goodbye (EOF, heartbeat death).
    ///
    /// If it was the Controller, THE RUN CONTINUES and the vanish deadline is armed.
    /// Returns `true` when a deadline was armed, so the caller can log the fact
    /// exactly once.
    pub fn on_disconnect(&mut self, conn: ConnId, now: Instant) -> bool {
        match &mut self.current {
            Some(l) if l.conn == Some(conn) => {
                l.conn = None;
                l.deadline = Some(now + self.timeout);
                true
            }
            _ => false,
        }
    }

    /// Has the vanish deadline passed? Call on a timer; `true` means SAFE-STOP now.
    ///
    /// Consumes the lease when it fires, so the caller cannot stop twice for one
    /// vanish.
    pub fn take_expired(&mut self, now: Instant) -> Option<String> {
        // The ORPHAN clock (a controller that left cleanly, run not yet adopted).
        if let Some((_, d)) = &self.orphan {
            if now >= *d {
                let (who, _) = self.orphan.take().expect("checked above");
                return Some(who);
            }
        }
        let expired = match &self.current {
            Some(l) if l.conn.is_none() => l.deadline.is_some_and(|d| now >= d),
            _ => false,
        };
        if !expired {
            return None;
        }
        let who = self
            .current
            .as_ref()
            .map(|l| l.who.clone())
            .unwrap_or_default();
        self.current = None;
        Some(who)
    }

    /// Seconds remaining on a running vanish deadline, for the status surface.
    pub fn vanish_remaining_s(&self, now: Instant) -> Option<f64> {
        let l = self.current.as_ref()?;
        if l.conn.is_some() {
            return None;
        }
        let d = l.deadline?;
        Some(d.saturating_duration_since(now).as_secs_f64())
    }

    fn expire_if_due(&mut self, now: Instant) {
        let _ = self.take_expired(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> LeaseRegistry {
        LeaseRegistry::new(Duration::from_secs(30))
    }

    /// THE ONE-CONTROLLER RULE, and the documented answer to a second one.
    ///
    /// RED-MUTATION: make the `Some(l) if l.conn.is_some()` arm fall through to
    /// granting a second Controller lease and this reds - which is the state where
    /// two operators drive one shaker, each unaware of the other.
    #[test]
    fn a_second_controller_is_refused_unless_it_asked_for_the_downgrade() {
        let t0 = Instant::now();
        let mut r = reg();

        let first = r.admit(1, "10.0.0.2:5000", RunRole::Controller, false, None, t0);
        assert!(matches!(
            first,
            Admission::Admitted(LeaseAnswer::Controller { .. })
        ));
        assert!(r.is_controller(1));

        // Second, NOT accepting a downgrade: REFUSED, naming the holder.
        match r.admit(2, "10.0.0.3:5000", RunRole::Controller, false, None, t0) {
            Admission::RefusedControllerHeld(who) => assert_eq!(who, "10.0.0.2:5000"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(r.is_controller(1), "the first still holds it");

        // Second, ACCEPTING the downgrade: admitted as an Observer, TOLD it was
        // downgraded, and told who has it.
        match r.admit(3, "10.0.0.3:5000", RunRole::Controller, true, None, t0) {
            Admission::Admitted(a @ LeaseAnswer::Observer { .. }) => {
                assert!(a.was_downgraded(), "a downgrade must be visible");
                assert_eq!(a.role(), RunRole::Observer);
            }
            other => panic!("expected an observer downgrade, got {other:?}"),
        }
        assert!(!r.is_controller(3));
        assert_eq!(r.role_of(3), RunRole::Observer);
    }

    /// An Observer that ASKED to observe is not a downgrade, and does not disturb the
    /// lease.
    #[test]
    fn observers_are_unlimited_and_never_take_the_lease() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "a", RunRole::Controller, false, None, t0);
        for conn in 2..=10 {
            match r.admit(conn, "watcher", RunRole::Observer, false, None, t0) {
                Admission::Admitted(a) => {
                    assert!(!a.was_downgraded());
                    assert_eq!(a.role(), RunRole::Observer);
                }
                other => panic!("an observer must always be admitted, got {other:?}"),
            }
            assert!(!r.is_controller(conn));
        }
        assert!(r.is_controller(1));
    }

    /// THE VANISH RULE: the run CONTINUES. Disconnecting the Controller arms a
    /// deadline and nothing fires before it.
    ///
    /// RED-MUTATION: make `on_disconnect` clear `self.current` outright (i.e. end the
    /// run when the client goes away) and the mid-window assertion reds - which is
    /// the state where a laptop's Wi-Fi hiccup kills a 40-minute test.
    #[test]
    fn a_vanished_controller_starts_a_clock_and_the_run_continues() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "10.0.0.2:5000", RunRole::Controller, false, None, t0);

        assert!(r.on_disconnect(1, t0), "the deadline is armed");
        // Mid-window: NOTHING has expired, and the lease is still assigned.
        let mid = t0 + Duration::from_secs(29);
        assert_eq!(r.take_expired(mid), None, "the run must continue");
        assert_eq!(r.vanish_remaining_s(mid), Some(1.0));
        // The holder is still named, so a new peer is told who is expected back.
        assert_eq!(r.holder(), "10.0.0.2:5000");

        // Past the deadline: NOW it fires, exactly once.
        let after = t0 + Duration::from_secs(31);
        assert_eq!(r.take_expired(after), Some("10.0.0.2:5000".to_string()));
        assert_eq!(r.take_expired(after), None, "it must not fire twice");
        assert_eq!(r.holder(), "");
    }

    /// A reconnect inside the window RECLAIMS control and disarms the deadline. This
    /// is what makes rule 2 useful rather than merely delayed.
    #[test]
    fn a_reconnect_inside_the_window_reclaims_the_lease() {
        let t0 = Instant::now();
        let mut r = reg();
        let lease = match r.admit(1, "10.0.0.2:5000", RunRole::Controller, false, None, t0) {
            Admission::Admitted(LeaseAnswer::Controller { lease, .. }) => lease,
            other => panic!("{other:?}"),
        };
        r.on_disconnect(1, t0);

        let back = t0 + Duration::from_secs(5);
        match r.admit(
            9,
            "10.0.0.2:5099",
            RunRole::Controller,
            false,
            Some(lease),
            back,
        ) {
            Admission::Admitted(LeaseAnswer::Controller { lease: got, .. }) => {
                assert_eq!(got, lease, "the SAME lease comes back");
            }
            other => panic!("expected the lease back, got {other:?}"),
        }
        assert!(r.is_controller(9));
        // The deadline is disarmed: nothing fires at what would have been its time.
        assert_eq!(r.take_expired(t0 + Duration::from_secs(31)), None);
        assert_eq!(r.vanish_remaining_s(back), None);
    }

    /// A DIFFERENT peer may take over a lease whose holder is gone. Making a new
    /// operator wait out the timeout of a machine that already lost its network helps
    /// nobody, and the run is still going.
    #[test]
    fn a_new_controller_may_take_over_an_abandoned_lease() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "gone", RunRole::Controller, false, None, t0);
        r.on_disconnect(1, t0);
        let t1 = t0 + Duration::from_secs(2);
        match r.admit(2, "10.0.0.7:5000", RunRole::Controller, false, None, t1) {
            Admission::Admitted(LeaseAnswer::Controller { .. }) => {}
            other => panic!("expected a takeover, got {other:?}"),
        }
        assert!(r.is_controller(2));
        assert_eq!(r.holder(), "10.0.0.7:5000");
        assert_eq!(r.take_expired(t0 + Duration::from_secs(31)), None);
    }

    /// A CLEAN CLOSE releases the lease immediately - the operator's own next
    /// connection is a controller again, not an observer waiting out a timeout.
    /// (Read live 2026-08-01: "the run lease is held by 192.168.4.36:62140" -
    /// the operator's own just-closed session.)
    #[test]
    fn a_clean_close_releases_the_lease_immediately() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "10.0.0.2:5000", RunRole::Controller, false, None, t0);
        assert!(r.on_clean_close(1, t0), "the orphan clock is armed");
        assert_eq!(
            r.holder(),
            "",
            "the lease is free the moment the close lands"
        );

        // The SAME operator reconnects one second later: CONTROLLER, no downgrade.
        let t1 = t0 + Duration::from_secs(1);
        match r.admit(2, "10.0.0.2:5001", RunRole::Controller, false, None, t1) {
            Admission::Admitted(LeaseAnswer::Controller { .. }) => {}
            other => panic!("expected an immediate controller grant, got {other:?}"),
        }
        assert!(r.is_controller(2));
        // Adoption disarmed the orphan clock: nothing fires later.
        assert_eq!(r.take_expired(t0 + Duration::from_secs(31)), None);
    }

    /// A cleanly-closed run with NO adopter still safe-stops on the same deadline
    /// a vanish would use - releasing the lease must not orphan a run forever.
    #[test]
    fn an_unadopted_clean_close_still_safe_stops() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "10.0.0.2:5000", RunRole::Controller, false, None, t0);
        r.on_clean_close(1, t0);
        assert_eq!(r.take_expired(t0 + Duration::from_secs(29)), None);
        assert_eq!(
            r.take_expired(t0 + Duration::from_secs(31)),
            Some("10.0.0.2:5000".to_string())
        );
        assert_eq!(r.take_expired(t0 + Duration::from_secs(32)), None, "once");
    }

    /// Disconnecting an OBSERVER changes nothing.
    #[test]
    fn an_observer_leaving_does_not_arm_anything() {
        let t0 = Instant::now();
        let mut r = reg();
        r.admit(1, "ctrl", RunRole::Controller, false, None, t0);
        r.admit(2, "obs", RunRole::Observer, false, None, t0);
        assert!(!r.on_disconnect(2, t0));
        assert_eq!(r.take_expired(t0 + Duration::from_secs(31)), None);
        assert!(r.is_controller(1));
    }
}
