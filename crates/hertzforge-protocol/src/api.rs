//! The dispatcher seam: one [`CoreApi`], two transports.
//!
//! Everything that can drive the core - the in-process Tauri command layer today, a
//! socket server after the split - funnels through [`dispatch`], which enforces the
//! envelope version rule and hands the payload to a [`CoreApi`] implementation. No
//! transport type appears anywhere in these signatures; that absence is the
//! liftability property the tests pin.

use crate::command::Command;
use crate::envelope::{CommandEnvelope, Envelope, ResponseEnvelope};
use crate::event::Event;
use crate::response::Response;

/// Where core -> client events go. Each connection/transport supplies one: the Tauri
/// layer emits into the webview event channel; a socket server writes
/// [`crate::EventEnvelope`]s to the stream (stamping its per-connection sequence).
///
/// `emit` takes the bare [`Event`]; envelope stamping (the monotonic `id`) is
/// transport bookkeeping and stays in the transport.
pub trait EventSink {
    /// Deliver one event to the client.
    fn emit(&mut self, event: Event);
}

/// A trivial in-memory sink: collects events in order. Useful for tests and for
/// transports that batch.
#[derive(Debug, Default)]
pub struct VecSink {
    /// The events emitted so far, in emission order.
    pub events: Vec<Event>,
}

impl EventSink for VecSink {
    fn emit(&mut self, event: Event) {
        self.events.push(event);
    }
}

/// The core dispatch trait - what every transport calls through.
///
/// Implementations live core-side (they may hold hardware state, module services,
/// the works); this crate only defines the shape. Contract:
///
/// - `handle` is ACCEPT-AND-RETURN: it must not block for the duration of a
///   converging intent. It validates, starts whatever background pacing the intent
///   needs, and returns [`crate::Completion::Converging`]; progress then flows
///   through the connection's event stream from the core's own pacing loop. (That is
///   why this trait is synchronous and runtime-free: a fast accept needs no
///   executor, and prescribing one here would drag a runtime dependency into the
///   seam.)
/// - `events` is the calling connection's sink, for events that are an immediate,
///   synchronous consequence of THIS command (e.g. the `StateChanged` right after a
///   profile is accepted). Long-lived emission (frames, convergence progress) rides
///   the same logical stream but is delivered by the core's background machinery
///   through whatever long-lived handle the transport registered at connection
///   setup - this trait deliberately does not prescribe that plumbing.
/// - Refusals are DATA, not panics: return [`Response::Rejected`] so the refusal
///   crosses the wire to the operator.
pub trait CoreApi {
    /// Handle one operator command, returning its response.
    fn handle(&mut self, cmd: Command, events: &mut dyn EventSink) -> Response;
}

/// THE dispatcher: version-check the envelope, hand the command to the core, wrap
/// the response with the request's correlation `id`.
///
/// A version-mismatched envelope is answered with [`Response::Rejected`] carrying
/// [`crate::ProtocolError::VersionMismatch`] and NEVER reaches the [`CoreApi`] - a
/// foreign-version payload is by definition untrusted parse territory (see
/// [`crate::envelope`]). The reply envelope always carries OUR version, so even the
/// rejection is well-formed for the local dialect.
pub fn dispatch(
    api: &mut dyn CoreApi,
    request: CommandEnvelope,
    events: &mut dyn EventSink,
) -> ResponseEnvelope {
    let body = match request.check_version() {
        Err(error) => Response::Rejected { error },
        Ok(()) => api.handle(request.body, events),
    };
    Envelope::new(request.id, body)
}
