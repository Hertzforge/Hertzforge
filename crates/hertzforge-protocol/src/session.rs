//! THE RUN LEASE - who is allowed to drive, and what happens to everyone else (v3).
//!
//! # The rule
//!
//! A hardware host serves **exactly one Controller and any number of Observers.** The
//! Controller holds the run LEASE: it is the only peer whose mutating commands are
//! accepted ([`crate::Command::is_mutating`]). Observers see everything - status,
//! frames, provenance, capture lists - and can mutate nothing.
//!
//! This is not a permissions feature. It is the same argument as the one-construction-
//! point rule one layer down: two peers issuing `SetLevel` at a shaker, each unaware
//! of the other, is a state no operator can reason about and no log can explain
//! afterwards. One authority, named, at all times.
//!
//! # The second-Controller decision, and why it is a REFUSAL by default
//!
//! When a second peer says Hello asking to be Controller, there are two defensible
//! answers - hand it back an Observer session, or refuse the connection - and the
//! design asked for one to be picked and documented. **This is the pick:**
//!
//! > A Controller request while a Controller holds the lease is **REFUSED**, unless
//! > the client explicitly set [`Hello::accept_observer_downgrade`], in which case it
//! > is granted an Observer session and TOLD it was downgraded
//! > ([`LeaseAnswer::Observer`] carrying [`ObserverReason::Downgraded`]).
//!
//! The reasoning is asymmetric risk. A UI that believes it is in control and is
//! silently not will show enabled Start/Stop/Abort controls that do nothing - the
//! operator presses Abort, sees the button depress, and nothing happens. That is
//! strictly worse than a connection that refuses with a message naming the current
//! holder. So the downgrade exists (an observer-first UI wants it) but it must be
//! ASKED FOR, and it is always reported rather than inferred.
//!
//! Note what the lease is NOT: it is not a safety mechanism. Losing the lease, or
//! never getting it, does not make anything safe - the primary abort authority is
//! on-hardware and co-resident with the drive clock, backed by the physical E-stop
//! (crate docs). The lease decides who may ASK; the hardware
//! decides what is safe.
//!
//! # Client vanish, heartbeat, and the safe-stop
//!
//! A vanished Controller does NOT stop the run. That is deliberate and it is the
//! design rule: a network hiccup on the operator's laptop must not
//! abort a 40-minute test that the hardware host is running perfectly well. Instead
//! the daemon starts a controller-timeout clock, and when it expires WITHOUT a
//! reconnect it brings the run down through the CONTROLLED ramp-down path, not
//! through an abort - "nobody is watching" is a reason to stop safely, never a reason
//! to slam the drive to zero.
//!
//! The lease is reclaimable: a reconnecting peer that presents the same
//! [`LeaseId`] within the timeout resumes control rather than being told someone
//! else's session is in the way.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The RUN-AUTHORITY axis of a peer: may it drive, or only watch?
///
/// # Naming (a deliberate deviation, recorded)
///
/// The original design called this `PeerRole { Controller, Observer }`. That name was
/// already taken by [`crate::PeerRole`], which answers a DIFFERENT question - which
/// SIDE of the seam a peer is (`Ui` / `Core`). Merging them would have produced one
/// enum whose variants answer two unrelated questions, so that `PeerRole::Core` and
/// `PeerRole::Controller` would look like alternatives when they are orthogonal
/// facts. Both ride the [`Hello`]; this is the authority axis and it is named for it.
///
/// [`Hello`]: crate::Hello
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunRole {
    /// Holds the run lease: mutating commands are accepted.
    Controller,
    /// Read-only: status, frames, provenance, lists. Every mutating command is
    /// refused with [`crate::ProtocolError::NotController`].
    Observer,
}

impl Default for RunRole {
    /// **Observer** - the LEAST authority. A peer that forgot to state a role, or a
    /// decoder that defaulted the field, must not thereby acquire the right to drive
    /// a shaker.
    fn default() -> Self {
        RunRole::Observer
    }
}

impl fmt::Display for RunRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunRole::Controller => f.write_str("controller"),
            RunRole::Observer => f.write_str("observer"),
        }
    }
}

/// Identifies one granted lease. Server-assigned; a reconnecting peer presents it to
/// RECLAIM control rather than queue behind its own orphaned session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseId(pub u64);

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lease#{}", self.0)
    }
}

/// Why a peer is an Observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverReason {
    /// It asked to observe. Nothing happened to it.
    Requested,
    /// It asked to control, a Controller already held the lease, and it had set
    /// [`Hello::accept_observer_downgrade`]. See the module docs for why this is
    /// opt-in.
    ///
    /// [`Hello`]: crate::Hello
    Downgraded,
}

/// What the core answered a [`Hello`] with.
///
/// [`Hello`]: crate::Hello
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAnswer {
    /// The peer holds the run lease.
    Controller {
        /// The lease id, for reclaiming after a reconnect.
        lease: LeaseId,
        /// How long (seconds) the core will hold this lease open after the peer's
        /// heartbeat stops, before it ramps the run down. The UI shows this number;
        /// it is config on the daemon, never guessed by the client.
        timeout_s: f64,
    },
    /// The peer is read-only.
    Observer {
        /// Requested, or downgraded (see [`ObserverReason`]).
        reason: ObserverReason,
        /// Who holds the lease right now, for the UI to name. `None` when nobody
        /// does (an Observer that simply asked to observe).
        controller: Option<String>,
    },
}

impl LeaseAnswer {
    /// The role this answer grants.
    pub fn role(&self) -> RunRole {
        match self {
            LeaseAnswer::Controller { .. } => RunRole::Controller,
            LeaseAnswer::Observer { .. } => RunRole::Observer,
        }
    }

    /// Was this peer downgraded from the role it asked for? A UI MUST surface this -
    /// see the module docs on why a silent downgrade is the worst outcome.
    pub fn was_downgraded(&self) -> bool {
        matches!(
            self,
            LeaseAnswer::Observer {
                reason: ObserverReason::Downgraded,
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default is the LEAST authority. A `#[serde(default)]` that handed out
    /// Controller would mean a peer could acquire the right to drive by omitting a
    /// field.
    ///
    /// RED-MUTATION: make `RunRole::default()` return `Controller` and this reds.
    #[test]
    fn the_default_role_is_observer() {
        assert_eq!(RunRole::default(), RunRole::Observer);
    }

    #[test]
    fn a_downgrade_is_always_visible_in_the_answer() {
        let downgraded = LeaseAnswer::Observer {
            reason: ObserverReason::Downgraded,
            controller: Some("10.0.0.4:51022".into()),
        };
        assert!(downgraded.was_downgraded());
        assert_eq!(downgraded.role(), RunRole::Observer);

        let asked = LeaseAnswer::Observer {
            reason: ObserverReason::Requested,
            controller: None,
        };
        assert!(
            !asked.was_downgraded(),
            "asking to observe is not a downgrade"
        );

        let ctrl = LeaseAnswer::Controller {
            lease: LeaseId(1),
            timeout_s: 30.0,
        };
        assert_eq!(ctrl.role(), RunRole::Controller);
        assert!(!ctrl.was_downgraded());
    }
}
