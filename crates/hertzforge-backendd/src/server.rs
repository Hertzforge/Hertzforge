//! The WebSocket transport: handshake, admission, dispatch, streams, heartbeat.
//!
//! # Frame discipline
//!
//! * **TEXT frames carry JSON**: `Hello` / `HelloAck` first, then `CommandEnvelope`
//!   up and `ResponseEnvelope` / `EventEnvelope` down.
//! * **BINARY frames carry packed display frames**, byte-for-byte the layouts pinned
//!   by `ui/IPC_CONTRACT.md`. They are NOT wrapped in an envelope and NOT base64'd
//!   into JSON: the whole point of the `FrameBytes` posture is that the same bytes
//!   the packer emits reach the TS decoder untouched, and a text-encoded binary frame
//!   would inflate the 400-channel budget by a third for nothing.
//!
//! A client therefore routes on the WebSocket frame TYPE, exactly as the Tauri client
//! routes on the channel a message arrived through.
//!
//! # The order of the gates, which is not arbitrary
//!
//! 1. Version (exact match) - before anything else is believed.
//! 2. Token - before a peer is admitted at all.
//! 3. Lease - before a mutating command is executed.
//!
//! Each one refuses with a typed error that crosses the wire, and 1 and 2 close the
//! connection afterwards. Refusing at the handshake rather than at first use is what
//! lets a UI render its controls from a fact.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use hertzforge_protocol::{
    dispatch, Command, CommandEnvelope, CoreStamp, Doc, Envelope, Event, EventEnvelope, Hello,
    HelloAck, LeaseAnswer, PeerRole, ProtocolError, Response, ResponseEnvelope, RunRole,
    SchemaKind, StreamKind, StreamSub, VecSink, PROTOCOL_VERSION,
};

use crate::config::DaemonConfig;
use crate::core::{Core, CoreSession};
use crate::frames::{decimate, pack_spectrum_frame, ObsStamp};
use crate::lease::{Admission, ConnId, LeaseRegistry};

/// The largest INBOUND WebSocket message this daemon will assemble, bytes.
///
/// # Why this is stated rather than inherited
///
/// Until v6 the accept path called `accept_async`, which rides tungstenite's DEFAULT
/// limits. That was survivable while every inbound message was a command envelope of a
/// few hundred bytes, and it stopped being survivable when v6 added
/// [`hertzforge_protocol::TwrCommand::ReferenceChunk`]: the seam now has a verb whose whole
/// job is to carry bulk. A ceiling nobody chose is a ceiling nobody can act on - the
/// operator gets a dropped connection with no message, which reads as a network fault
/// rather than as "that chunk was too big".
///
/// # Why 4 MiB
///
/// It BOUNDS the 1 MB chunk the TWR upload contract asks clients to keep to, with room
/// for base64's 4/3 expansion (1 MB of `f32` samples is ~1.34 MB of base64) and for the
/// JSON envelope around it, and still leaves roughly 3x headroom before the refusal.
/// Larger would make a single mis-sized chunk cost the daemon 16 MB of assembly buffer
/// per connection; smaller would refuse chunks the contract invites.
///
/// # What it is NOT
///
/// It bounds INBOUND assembly only. The OUTBOUND display path - binary frames, core to
/// client - is bounded by the producers and by the client's own `max_size`, which is
/// where the probes set `1 << 24`. The two numbers are unrelated and must not be
/// "harmonized": they cap opposite directions for opposite reasons.
const MAX_INBOUND_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The accept-side WebSocket configuration, with the inbound ceiling STATED.
///
/// Frame size is left at the library default deliberately: the ceiling that matters is
/// the assembled MESSAGE, and pinning both would mean two numbers to keep in step for
/// one property. A client that fragments a legal message across many frames is doing
/// nothing wrong.
fn inbound_ws_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_INBOUND_WS_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// Shared server state.
pub struct Server {
    /// The validated configuration this daemon was started with.
    pub cfg: DaemonConfig,
    /// Everything the daemon serves.
    pub core: Arc<Core>,
    /// The run lease: one Controller, N Observers.
    pub leases: Arc<Mutex<LeaseRegistry>>,
    next_conn: AtomicU64,
}

impl Server {
    /// Build the server state.
    pub fn new(cfg: DaemonConfig, core: Arc<Core>) -> Arc<Self> {
        let timeout = Duration::from_secs_f64(cfg.controller_timeout_s);
        Arc::new(Server {
            cfg,
            core,
            leases: Arc::new(Mutex::new(LeaseRegistry::new(timeout))),
            next_conn: AtomicU64::new(1),
        })
    }

    /// Bind and serve until `shutdown` resolves.
    ///
    /// Returns the bound address, which is what a test uses when it asks for port 0 -
    /// a test that hard-codes a port is a test that fails when someone else's daemon
    /// is up.
    pub async fn bind(self: &Arc<Self>) -> std::io::Result<(TcpListener, std::net::SocketAddr)> {
        let listener = TcpListener::bind(self.cfg.socket_addr()).await?;
        let addr = listener.local_addr()?;
        Ok((listener, addr))
    }

    /// The accept loop, plus the lease-expiry timer that owns the safe-stop.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        let expiry = Arc::clone(&self);
        tokio::spawn(async move { expiry.expiry_loop().await });

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let me = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(e) = me.serve_one(stream, peer.to_string()).await {
                            tracing::info!(target: "hertzforge::backendd", peer = %peer, "connection ended: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(target: "hertzforge::backendd", "accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }

    /// THE CONTROLLER-TIMEOUT CLOCK. Ticks at the heartbeat cadence; when the lease
    /// expires it invokes the SAFE-STOP.
    ///
    /// It lives here, on a timer, rather than in the disconnect path, precisely
    /// because the disconnect path must NOT end the run: the run continues, the clock
    /// runs, and only its expiry brings anything down.
    async fn expiry_loop(self: Arc<Self>) {
        // The expiry clock ticks at the heartbeat cadence, bounded: never slower than
        // 1 s (so a long heartbeat cannot make a safe-stop arrive a minute late) and
        // never faster than 50 ms (so a mis-set heartbeat cannot spin a core).
        let tick = Duration::from_secs_f64(self.cfg.heartbeat_s.clamp(0.05, 1.0));
        loop {
            tokio::time::sleep(tick).await;
            let expired = {
                let mut l = self.leases.lock().expect("leases poisoned");
                l.take_expired(Instant::now())
            };
            if let Some(who) = expired {
                let reason = format!(
                    "the controller ({who}) stopped answering and did not return \
                     within {:.0} s; ramping the run down",
                    self.cfg.controller_timeout_s
                );
                self.core.safe_stop(&reason);
            }
        }
    }

    async fn serve_one(
        self: Arc<Self>,
        stream: TcpStream,
        peer: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ws =
            tokio_tungstenite::accept_async_with_config(stream, Some(inbound_ws_config())).await?;
        let (mut tx, mut rx) = ws.split();
        let conn: ConnId = self.next_conn.fetch_add(1, Ordering::SeqCst);

        // ---- GATE 1 + 2: version and token, in the handshake -------------------
        let first = match rx.next().await {
            Some(Ok(Message::Text(t))) => t,
            _ => return Ok(()), // closed before saying anything
        };
        let hello: Hello = match serde_json::from_str(&first) {
            Ok(h) => h,
            Err(e) => {
                let err = ProtocolError::MalformedPayload {
                    container: "hello".into(),
                    detail: e.to_string(),
                };
                let _ = tx.send(Message::Text(refusal_json(&err))).await;
                let _ = tx.close().await;
                return Ok(());
            }
        };
        if let Err(err) = hello.check() {
            tracing::warn!(target: "hertzforge::backendd", peer = %peer, "{err}");
            let _ = tx.send(Message::Text(refusal_json(&err))).await;
            let _ = tx.close().await;
            return Ok(());
        }
        if let Err(err) = hello.check_token(&self.cfg.token) {
            tracing::warn!(target: "hertzforge::backendd", peer = %peer, "{err}");
            let _ = tx.send(Message::Text(refusal_json(&err))).await;
            let _ = tx.close().await;
            return Ok(());
        }

        // ---- GATE 3 (part 1): admission ---------------------------------------
        let admission = {
            let mut l = self.leases.lock().expect("leases poisoned");
            l.admit(
                conn,
                &peer,
                hello.run_role,
                hello.accept_observer_downgrade,
                None,
                Instant::now(),
            )
        };
        let lease = match admission {
            Admission::Admitted(a) => a,
            Admission::RefusedControllerHeld(holder) => {
                let err = ProtocolError::NotController { holder };
                tracing::warn!(target: "hertzforge::backendd", peer = %peer, "controller refused: {err}");
                let _ = tx.send(Message::Text(refusal_json(&err))).await;
                let _ = tx.close().await;
                return Ok(());
            }
        };
        let role = lease.role();
        tracing::info!(
            target: "hertzforge::backendd",
            peer = %peer, role = %role, downgraded = lease.was_downgraded(),
            "admitted"
        );

        // From this point the connection is IN the registry, so its exit - every
        // exit - must run the lease handling. See `LeaseDropGuard`.
        let guard = LeaseDropGuard {
            server: Arc::clone(&self),
            conn,
            peer: peer.clone(),
            clean: std::sync::atomic::AtomicBool::new(false),
        };

        let ack = HelloAck {
            v: PROTOCOL_VERSION,
            role: PeerRole::Core,
            build: format!("hertzforge-backendd {}", env!("CARGO_PKG_VERSION")),
            stamp: core_stamp(),
            lease: lease.clone(),
            heartbeat_s: self.cfg.heartbeat_s,
        };
        send_bounded(
            &mut tx,
            Message::Text(serde_json::to_string(&ack)?),
            self.cfg.heartbeat_s,
        )
        .await?;

        // The provenance of any run already in flight, so a peer attaching mid-run
        // learns what the data IS without waiting for the next run to start.
        //
        // ONE AUTHORITY (`Core::live_module`) picks whose: the module that HOLDS the
        // core answers, Random first because it is the drive-producing run. The old
        // `random.provenance().or_else(recorder.provenance())` chain asked the hosts
        // in a hard-coded order and could hand back a STALE Random provenance from a
        // finished run while the Recorder was the module actually up.
        let prov = self
            .core
            .live_module(crate::core::Claim::Configured)
            .and_then(|m| self.core.module_provenance(m));
        if let Some(p) = prov {
            let ev = EventEnvelope::new(
                0,
                Event::RunProvenance {
                    provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                },
            );
            send_bounded(
                &mut tx,
                Message::Text(serde_json::to_string(&ev)?),
                self.cfg.heartbeat_s,
            )
            .await?;
        }

        // ---- the connection loop ----------------------------------------------
        //
        // COMMAND DISPATCH RUNS BESIDE THE STREAM FORWARDER, never in front of
        // it. Dispatch used to run inline in the select loop; a mutating
        // command blocks on the service mutex - which the pump holds for nearly
        // a whole control tick on hardware, and SetProfile's station
        // build holds for 5-30 s - so for that whole window this arm starved
        // `frames.recv()`, the broadcast receiver LAGGED, and the peer's display
        // stream dropped its oldest frames. The operator saw it directly: every
        // mid-run level step / mode takeover produced a visible pause + gap in
        // the time pane, and the journal pinned it ("remote ladder-mode
        // takeover" immediately followed by "display stream lagged N batches",
        // every time, 2026-08-02). Commands now run on a blocking task, one at
        // a time in arrival order, while this loop keeps pumping frames,
        // heartbeats, and inbound traffic.
        let mut session_slot: Option<CoreSession> =
            Some(CoreSession::new(Arc::clone(&self.core), conn, peer.clone()));
        let mut inflight: Option<
            tokio::task::JoinHandle<(CoreSession, ResponseEnvelope, VecSink)>,
        > = None;
        let mut cmd_queue: std::collections::VecDeque<CommandEnvelope> =
            std::collections::VecDeque::new();
        let mut subs: Vec<StreamSub> = Vec::new();
        let mut frames = self.core.frames.subscribe();
        let mut seq: u64 = 1;
        let mut stream_seq: u32 = 0;
        let mut last_emit: Option<Instant> = None;
        let mut last_seen = Instant::now();
        let mut ping = tokio::time::interval(Duration::from_secs_f64(self.cfg.heartbeat_s));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let result = loop {
            tokio::select! {
                // ---- inbound -------------------------------------------------
                msg = rx.next() => {
                    let Some(msg) = msg else { break Ok(()) };
                    let msg = match msg {
                        Ok(m) => m,
                        // AN OVERSIZE MESSAGE IS ANSWERED, NOT DROPPED.
                        //
                        // Stating `max_message_size` bounds the memory; it does not
                        // tell the operator anything. Tungstenite's answer to a message
                        // past the ceiling is a `Capacity` error on the READ, and this
                        // loop's every-other-error path is `break` - so before this arm
                        // existed, a client that sent a chunk one byte too big got a
                        // socket that died with no message. That reads as a network
                        // fault, and the operator retries the same too-big chunk.
                        //
                        // **THE CONNECTION DOES NOT SURVIVE THIS, and the message says
                        // so.** Tungstenite itself treats a capacity error as
                        // non-fatal, but `tokio_tungstenite::WebSocketStream::poll_next`
                        // sets `ended = true` on ANY error and fuses the stream
                        // (tokio-tungstenite 0.24, lib.rs:302-309), so the next poll
                        // returns `None` and this loop exits however this arm ends.
                        // MEASURED, not assumed: the first cut of this arm said "the
                        // connection is still open" and `twr_wire_probe.py` caught the
                        // client being closed with 1006 one message later. The byte
                        // wins; the sentence was corrected rather than the behaviour
                        // being wished into place.
                        //
                        // What this arm buys is therefore the DIAGNOSIS, not the
                        // survival: the client learns the size, the ceiling, and what
                        // to do, instead of watching a socket die for no stated reason
                        // and retrying the same too-big chunk. The disconnect that
                        // follows is a normal controller-vanish - the run continues and
                        // the timeout clock starts - which is the right handling for a
                        // controller that has, in fact, lost its connection.
                        Err(tokio_tungstenite::tungstenite::Error::Capacity(c)) => {
                            let err = ProtocolError::MalformedPayload {
                                container: "client frame".into(),
                                detail: format!(
                                    "{c}. This core assembles inbound messages up to \
                                     {MAX_INBOUND_WS_MESSAGE_BYTES} bytes. The message \
                                     was DISCARDED and THIS CONNECTION IS NOW CLOSING - \
                                     reconnect and send it in smaller pieces. A TWR \
                                     ReferenceChunk should stay at or under 1 MB of \
                                     samples, which also lets status polls interleave \
                                     between chunks."
                                ),
                            };
                            tracing::warn!(target: "hertzforge::backendd", peer = %peer, "{err}");
                            send_bounded(&mut tx, Message::Text(refusal_json(&err)), self.cfg.heartbeat_s).await?;
                            continue;
                        }
                        Err(e) => break Err(e.into()),
                    };
                    // ANY inbound traffic is proof of life - a pong, a command, a
                    // ping of the client's own. The heartbeat is a liveness probe,
                    // not a protocol step, so anything that proves the socket is
                    // being read resets it.
                    last_seen = Instant::now();
                    match msg {
                        Message::Close(_) => { guard.mark_clean(); break Ok(()) },
                        // A pong is the heartbeat answer; tungstenite replies to
                        // pings for us, so an inbound Ping needs nothing here.
                        Message::Pong(_) | Message::Ping(_) | Message::Frame(_) => {}
                        Message::Binary(_) => {
                            // The client has no reason to send binary; refusing keeps
                            // the frame-type routing unambiguous in both directions.
                            let err = ProtocolError::MalformedPayload {
                                container: "client frame".into(),
                                detail: "binary frames are core -> client only".into(),
                            };
                            send_bounded(&mut tx, Message::Text(refusal_json(&err)), self.cfg.heartbeat_s).await?;
                        }
                        Message::Text(t) => {
                            let env: CommandEnvelope = match serde_json::from_str(&t) {
                                Ok(e) => e,
                                Err(e) => {
                                    // THE UNKNOWN-FIELD DIAGNOSTIC reaching the
                                    // operator: serde's own message names the field.
                                    let err = ProtocolError::MalformedPayload {
                                        container: "command".into(),
                                        detail: e.to_string(),
                                    };
                                    send_bounded(&mut tx, Message::Text(refusal_json(&err)), self.cfg.heartbeat_s).await?;
                                    continue;
                                }
                            };

                            // Subscriptions are TRANSPORT state - they never reach
                            // the core.
                            if let Command::Subscribe { subs: want } = &env.body {
                                subs = want.clone();
                                let resp: ResponseEnvelope = Envelope::new(
                                    env.id,
                                    Response::Accepted {
                                        completion: hertzforge_protocol::Completion::Immediate,
                                    },
                                );
                                send_bounded(&mut tx, Message::Text(serde_json::to_string(&resp)?), self.cfg.heartbeat_s).await?;
                                continue;
                            }
                            if let Command::Unsubscribe { streams } = &env.body {
                                subs.retain(|s| !streams.contains(&s.kind));
                                let resp: ResponseEnvelope = Envelope::new(
                                    env.id,
                                    Response::Accepted {
                                        completion: hertzforge_protocol::Completion::Immediate,
                                    },
                                );
                                send_bounded(&mut tx, Message::Text(serde_json::to_string(&resp)?), self.cfg.heartbeat_s).await?;
                                continue;
                            }

                            // ---- GATE 3 (part 2): the lease --------------------
                            if env.body.is_mutating() {
                                let held = {
                                    let l = self.leases.lock().expect("leases poisoned");
                                    (l.is_controller(conn), l.holder())
                                };
                                if !held.0 {
                                    let resp: ResponseEnvelope = Envelope::new(
                                        env.id,
                                        Response::Rejected {
                                            error: ProtocolError::NotController {
                                                holder: held.1,
                                            },
                                        },
                                    );
                                    send_bounded(&mut tx, Message::Text(serde_json::to_string(&resp)?), self.cfg.heartbeat_s).await?;
                                    continue;
                                }
                            }

                            // Queue for the dispatch worker; FIFO order
                            // is preserved because exactly one command runs at a
                            // time. The display arms keep running meanwhile.
                            cmd_queue.push_back(env);
                            if inflight.is_none() {
                                if let Some(next) = cmd_queue.pop_front() {
                                    let mut s = session_slot
                                        .take()
                                        .expect("session absent with no dispatch in flight");
                                    inflight = Some(tokio::task::spawn_blocking(move || {
                                        let mut sink = VecSink::default();
                                        let resp = dispatch(&mut s, next, &mut sink);
                                        (s, resp, sink)
                                    }));
                                }
                            }
                        }
                    }
                }

                // ---- a dispatched command finished -------------------
                res = async { inflight.as_mut().expect("guarded by if").await }, if inflight.is_some() => {
                    inflight = None;
                    let (s, resp, sink) = match res {
                        Ok(t) => t,
                        // A panicked dispatch is a daemon bug; the connection
                        // cannot continue without its session.
                        Err(e) => break Err(e.into()),
                    };
                    session_slot = Some(s);
                    send_bounded(&mut tx, Message::Text(serde_json::to_string(&resp)?), self.cfg.heartbeat_s).await?;
                    for ev in sink.events {
                        let e: EventEnvelope = Envelope::new(seq, ev);
                        seq += 1;
                        send_bounded(&mut tx, Message::Text(serde_json::to_string(&e)?), self.cfg.heartbeat_s).await?;
                    }
                    // FIFO: start the next queued command, if one arrived while
                    // this one ran.
                    if let Some(next) = cmd_queue.pop_front() {
                        let mut s = session_slot
                            .take()
                            .expect("session was just restored");
                        inflight = Some(tokio::task::spawn_blocking(move || {
                            let mut sink = VecSink::default();
                            let resp = dispatch(&mut s, next, &mut sink);
                            (s, resp, sink)
                        }));
                    }
                }

                // ---- outbound: the display stream ----------------------------
                batch = frames.recv() => {
                    let batch = match batch {
                        Ok(b) => b,
                        // Lagged: the peer could not keep up and frames were dropped.
                        // That is exactly what the observability `seq` exists to make
                        // visible - the counter keeps counting emitted frames, so the
                        // decoder sees the gap. The run is unaffected.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                target: "hertzforge::backendd", peer = %peer,
                                "display stream lagged {n} batches for this peer"
                            );
                            continue;
                        }
                        Err(_) => continue,
                    };
                    match batch {
                        crate::recorder::DisplayBatch::Spectrum(batch) => {
                            let Some(sub) = subs.iter().find(|s| s.kind == StreamKind::Spectrum) else {
                                continue;
                            };
                            // The RATE CEILING, applied at the producer: a frame this
                            // peer did not ask for often enough is never packed or sent.
                            if let (Some(min), Some(prev)) = (sub.min_period_s(), last_emit) {
                                if prev.elapsed().as_secs_f64() < min {
                                    continue;
                                }
                            }
                            last_emit = Some(Instant::now());

                            let groups = self.core.recorder.channel_groups();
                            for (ch, group, mags) in &batch.traces {
                                // The CHANNEL selector, also at the producer.
                                let group = group.or_else(|| {
                                    groups.iter().find(|(id, _)| id == ch).and_then(|(_, g)| *g)
                                });
                                if !sub.channels.accepts(*ch, group) {
                                    continue;
                                }
                                let (kept, df) = decimate(sub, mags, batch.df_hz);
                                stream_seq = stream_seq.wrapping_add(1);
                                let bytes = pack_spectrum_frame(
                                    *ch,
                                    batch.f0_hz as f32,
                                    df as f32,
                                    0,
                                    batch.window_seq,
                                    ObsStamp {
                                        seq: stream_seq,
                                        // The recorder's traces are report-derived and
                                        // carry no per-frame sample position: 0 is the
                                        // honest "no source index", not sample zero.
                                        sample_index: 0,
                                        run_id: batch.run_id,
                                    },
                                    &kept,
                                );
                                send_bounded(&mut tx, Message::Binary(bytes), self.cfg.heartbeat_s).await?;
                            }
                        }
                        // The raw time-domain views. Sent ONLY to peers that
                        // subscribed the kind - which is also what keeps an older
                        // front end from ever seeing the magic-prefixed layouts.
                        crate::recorder::DisplayBatch::Scope(b) => {
                            if !subs.iter().any(|s| s.kind == StreamKind::Scope) {
                                continue;
                            }
                            stream_seq = stream_seq.wrapping_add(1);
                            let channels: Vec<crate::frames::ScopeChannel> = b
                                .channels
                                .iter()
                                .map(|(id, rms, peak, mins, maxs)| crate::frames::ScopeChannel {
                                    id: *id,
                                    rms: *rms,
                                    peak: *peak,
                                    mins: mins.clone(),
                                    maxs: maxs.clone(),
                                })
                                .collect();
                            let n_cols = channels.first().map(|c| c.mins.len()).unwrap_or(0) as u32;
                            let bytes = crate::frames::pack_remote_scope_frame(
                                n_cols,
                                b.t_end_s,
                                b.dt_s,
                                ObsStamp { seq: stream_seq, sample_index: 0, run_id: b.run_id },
                                &channels,
                            );
                            send_bounded(&mut tx, Message::Binary(bytes), self.cfg.heartbeat_s).await?;
                        }
                        crate::recorder::DisplayBatch::Wave(b) => {
                            if !subs.iter().any(|s| s.kind == StreamKind::Waveform) {
                                continue;
                            }
                            stream_seq = stream_seq.wrapping_add(1);
                            let channels: Vec<crate::frames::WaveChannel> = b
                                .channels
                                .iter()
                                .map(|(id, samples)| crate::frames::WaveChannel {
                                    id: *id,
                                    samples: samples.clone(),
                                })
                                .collect();
                            let n = channels.first().map(|c| c.samples.len()).unwrap_or(0) as u32;
                            let bytes = crate::frames::pack_remote_waveform_frame(
                                n,
                                b.t_end_s,
                                b.dt_s,
                                ObsStamp { seq: stream_seq, sample_index: 0, run_id: b.run_id },
                                &channels,
                            );
                            send_bounded(&mut tx, Message::Binary(bytes), self.cfg.heartbeat_s).await?;
                        }
                    }
                }

                // ---- the heartbeat -------------------------------------------
                //
                // THE HALF-OPEN CASE, which is why a heartbeat exists at all. A
                // laptop that is closed, suspended, or has its Wi-Fi cut sends no
                // FIN: the TCP connection stays "open" for minutes and `rx.next()`
                // simply never yields. Without this probe the daemon would think the
                // Controller was present the whole time and the controller-timeout
                // clock would never start.
                //
                // THREE MISSED BEATS, not one: a single dropped pong on a lab
                // network is normal, and declaring a controller dead on it would
                // make the lease flap. After that the connection is dropped, which
                // arms the controller-timeout clock below - so the total time before
                // a safe-stop is (3 x heartbeat) + controller_timeout, and both
                // halves are visible in the log.
                _ = ping.tick() => {
                    const MISSED_BEATS_BEFORE_DEAD: u32 = 3;
                    let silent = last_seen.elapsed().as_secs_f64();
                    if silent > self.cfg.heartbeat_s * MISSED_BEATS_BEFORE_DEAD as f64 {
                        tracing::warn!(
                            target: "hertzforge::backendd", peer = %peer,
                            "no answer for {silent:.1} s ({MISSED_BEATS_BEFORE_DEAD} \
                             missed heartbeats); treating this connection as gone"
                        );
                        break Ok(());
                    }
                    send_bounded(&mut tx, Message::Ping(Vec::new()), self.cfg.heartbeat_s).await?;
                }
            }
        };

        // The lease handling is the guard's Drop - here AND on every abnormal
        // exit. Dropping it explicitly keeps the intent readable.
        drop(guard);
        result
    }
}

/// Send with a DEADLINE: an outbound write that cannot complete within
/// a few heartbeats means the peer is gone in a way the reads have not noticed
/// yet - and an unbounded `send(...).await` inside the session loop freezes the
/// WHOLE select (heartbeat arm included), which is one of the two ways a dead
/// controller's task can wedge while holding the lease. The other (panic /
/// abort) is covered by [`LeaseDropGuard`].
async fn send_bounded<S>(
    tx: &mut S,
    msg: Message,
    heartbeat_s: f64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let budget = Duration::from_secs_f64((heartbeat_s * 3.0).clamp(5.0, 60.0));
    match tokio::time::timeout(budget, tx.send(msg)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(format!(
            "outbound send stalled past {budget:?}; treating the connection as dead"
        )
        .into()),
    }
}

/// Releases a connection's lease state HOWEVER its task ends.
///
/// The post-loop disconnect handling used to be plain code after the connection
/// loop - which never runs if the task panics, or if an await inside the loop
/// wedges and the task is dropped. Observed live 2026-08-01: `ss` showed ZERO
/// sockets while the registry still held that peer as a LIVE controller, and
/// every reconnect (including the operator's) was refused naming a dead holder
/// until the daemon was restarted. This guard moves the handling into `Drop`,
/// which runs on normal return, on `?`, on panic, and on task abort alike.
struct LeaseDropGuard {
    server: Arc<Server>,
    conn: ConnId,
    peer: String,
    /// Set when the peer said GOODBYE (an explicit Close frame).
    clean: std::sync::atomic::AtomicBool,
}

impl LeaseDropGuard {
    fn mark_clean(&self) {
        self.clean.store(true, Ordering::SeqCst);
    }
}

impl Drop for LeaseDropGuard {
    fn drop(&mut self) {
        let clean = self.clean.load(Ordering::SeqCst);
        let armed = {
            let mut l = self.server.leases.lock().expect("leases poisoned");
            if clean {
                l.on_clean_close(self.conn, Instant::now())
            } else {
                l.on_disconnect(self.conn, Instant::now())
            }
        };
        if armed {
            if clean {
                tracing::info!(
                    target: "hertzforge::backendd", peer = %self.peer,
                    "the CONTROLLER left cleanly; lease RELEASED, and the run (if \
                     any) safe-stops in {:.0} s unless a controller adopts it",
                    self.server.cfg.controller_timeout_s
                );
            } else {
                tracing::warn!(
                    target: "hertzforge::backendd", peer = %self.peer,
                    "the CONTROLLER disconnected; the run CONTINUES and the {:.0} s \
                     controller-timeout clock is running",
                    self.server.cfg.controller_timeout_s
                );
            }
        }
    }
}

/// A bare refusal, for the pre-handshake phase where there is no envelope id to echo.
fn refusal_json(error: &ProtocolError) -> String {
    let env: ResponseEnvelope = Envelope::new(
        0,
        Response::Rejected {
            error: error.clone(),
        },
    );
    serde_json::to_string(&env).unwrap_or_else(|_| "{}".into())
}

/// This build's core stamp, mirrored into the protocol's own type.
pub fn core_stamp() -> CoreStamp {
    let b = hertzforge_buildinfo::build_info();
    CoreStamp {
        sha: b.sha,
        dirty: b.dirty,
        host: b.host,
        target: b.target,
    }
}

/// The role a connection currently holds, for a status surface.
pub fn role_of(server: &Server, conn: ConnId) -> RunRole {
    server.leases.lock().expect("leases poisoned").role_of(conn)
}

/// A refusal, rendered - used by the tests and by `--stop`.
pub fn refusal_of(json: &str) -> Option<ProtocolError> {
    let env: Envelope<Response> = serde_json::from_str(json).ok()?;
    match env.body {
        Response::Rejected { error } => Some(error),
        _ => None,
    }
}

/// The `LeaseAnswer` in an ack, for the tests.
pub fn lease_of(json: &str) -> Option<LeaseAnswer> {
    serde_json::from_str::<HelloAck>(json).ok().map(|a| a.lease)
}
