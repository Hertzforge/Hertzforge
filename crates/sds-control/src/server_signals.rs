//! The Signals methods of the [`Service`] (schema/PROTOCOL.md, "Signals"): each
//! connection's subscription, and the pump that sends it `signals.data` while a run of
//! any test type acquires, and once more when that run ends.
//!
//! The pump reads the process's tap ([`crate::signals::global`]) and never anything a
//! run holds: it takes the hub's lock only to find the current run, and copies its
//! windows out of the ring without a lock. A run counts as acquiring while its tap is
//! live and the service holds a live run: a module that keeps its station open after
//! its run has ended is not taken for one still running.

use super::{Feed, Out, Service, DATA_SLACK};
use crate::codec::{self, Arrays};
use crate::protocol::{self, MethodError};
use crate::session::ConnId;
use crate::signals::subscribe::{self, Subscription};
use crate::signals::{self, TapRun};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the pump looks for frames due: `rate_hz` runs up to 10 a second.
pub(super) const TICK: Duration = Duration::from_millis(50);

/// One connection's Signals subscription and what it has been sent.
pub struct SignalsDisplay {
    sub: Arc<Subscription>,
    last: Option<Instant>,
    /// The run and its station frame at the last frame sent.
    sent: Option<(u64, u64)>,
    /// The run this connection has seen acquiring and is owed a last frame of.
    owed: Option<u64>,
}

impl Service {
    pub(super) fn signals_displays(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ConnId, SignalsDisplay>> {
        match self.signals_displays.lock() {
            Ok(d) => d,
            Err(p) => p.into_inner(),
        }
    }

    /// One `signals.data` frame of `tap` for connection `conn`.
    fn emit_signals(&self, conn: ConnId, tap: &TapRun, sub: &Subscription, available: bool) {
        let mut arrays = Arrays::new();
        let mut data = subscribe::data_json(tap, sub, &mut arrays);
        data["available"] = Value::from(available);
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match codec::encode(&protocol::event("signals.data", seq, data), &arrays) {
            Ok(frame) => {
                let _ = self
                    .events
                    .send(Arc::new(Out::To(conn, Feed::Signals, frame)));
            }
            Err(e) => eprintln!("sds-control: signals.data frame not encoded: {e}"),
        }
    }

    /// `signals.subscribe`: replace `conn`'s subscription, and send it a frame at once
    /// when a run is acquiring and has read something.
    pub(super) async fn signals_subscribe(
        &self,
        conn: ConnId,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let sub = Arc::new(subscribe::parse(params)?);
        let acquiring = self.inner.lock().await.any_live();
        let live = signals::global()
            .current()
            .filter(|t| acquiring && t.is_live());
        let result = subscribe::result_json(&sub, live.as_deref());
        let now = live.as_ref().map(|t| (t.id(), t.now()));
        let send = now.is_some_and(|(_, frames)| frames > 0);
        self.signals_displays().insert(
            conn,
            SignalsDisplay {
                sub: sub.clone(),
                last: send.then(Instant::now),
                sent: if send { now } else { None },
                owed: now.map(|(id, _)| id),
            },
        );
        if let (true, Some(t)) = (send, &live) {
            self.emit_signals(conn, t, &sub, true);
        }
        Ok(result)
    }

    /// `signals.unsubscribe`: no more frames for `conn`.
    pub(super) fn signals_unsubscribe(
        &self,
        conn: ConnId,
        params: &Map<String, Value>,
    ) -> Result<Value, MethodError> {
        if let Some(k) = params.keys().next() {
            return Err(MethodError::bad_params(
                k,
                format!("signals.unsubscribe takes no parameters, not \"{k}\""),
            ));
        }
        self.signals_displays().remove(&conn);
        Ok(json!({}))
    }

    /// One look of the pump: a frame to each subscribed connection whose period has
    /// passed while the run acquires and has read more since its last frame, and one
    /// last frame (`available` false) to each that saw the run, once it has ended. An
    /// ended run is let go when nobody is owed its last frame. `acquiring`: the service
    /// holds a live run.
    pub(super) fn signals_tick(&self, acquiring: bool) {
        let hub = signals::global();
        let Some(tap) = hub.current() else {
            return;
        };
        let now = Instant::now();
        let live = acquiring && tap.is_live();
        let frames = tap.now();
        let id = tap.id();
        let due: Vec<(ConnId, Arc<Subscription>)> = {
            let mut displays = self.signals_displays();
            displays
                .iter_mut()
                .filter_map(|(conn, d)| {
                    let send = if live {
                        d.owed = Some(id);
                        frames > 0
                            && d.sent != Some((id, frames))
                            && d.last
                                .is_none_or(|t| now + DATA_SLACK >= t + d.sub.period())
                    } else {
                        d.owed.take() == Some(id)
                    };
                    send.then(|| {
                        d.last = Some(now);
                        d.sent = Some((id, frames));
                        (*conn, d.sub.clone())
                    })
                })
                .collect()
        };
        for (conn, sub) in due {
            self.emit_signals(conn, &tap, &sub, live);
        }
        if !live && !self.signals_displays().values().any(|d| d.owed == Some(id)) {
            hub.release(id);
        }
    }
}
