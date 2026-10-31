//! The WebSocket server: one [`Service`] shared by every connection.
//!
//! A run belongs to the service, not to a connection: any client may start or stop
//! it and every connected client receives its `random.display` events. Requests on
//! one connection are answered independently (a slow `sim.stop` does not hold up
//! a `sim.state`), so responses can arrive out of request order; the `id` pairs
//! them.

use crate::codec::{self, Arrays, Frame};
use crate::profiles;
use crate::protocol::{self, MethodError, Request};
use crate::sim::{self, RunRequest, SimRun};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

/// Display event topic.
pub const DISPLAY_TOPIC: &str = "random.display";
/// Display event period (5 per second).
pub const DISPLAY_PERIOD: Duration = Duration::from_millis(200);
/// How long `sim.stop` waits for the run to wind down.
const STOP_WAIT: Duration = Duration::from_secs(15);
/// Events buffered per connection before a slow client starts losing them.
const EVENT_BUFFER: usize = 64;

#[derive(Default)]
struct Controller {
    run: Option<Arc<SimRun>>,
    started: u64,
}

/// The control service.
pub struct Service {
    controller: Mutex<Controller>,
    events: broadcast::Sender<Arc<Vec<u8>>>,
    seq: AtomicU64,
}

impl Service {
    pub fn new() -> Arc<Service> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Arc::new(Service {
            controller: Mutex::new(Controller::default()),
            events,
            seq: AtomicU64::new(0),
        })
    }

    /// Subscribe to encoded event frames.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<u8>>> {
        self.events.subscribe()
    }

    /// Answer one request.
    pub async fn call(
        self: &Arc<Self>,
        method: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Value, MethodError> {
        match method {
            "sim.profiles" => Ok(json!({
                "profiles": profiles::all().iter().map(|p| p.describe()).collect::<Vec<_>>()
            })),
            "sim.start" => self.sim_start(params).await,
            "sim.stop" => self.sim_stop().await,
            "sim.state" => Ok(self.sim_state().await),
            other => Err(MethodError::new(
                protocol::ErrorCode::UnknownMethod,
                format!(
                    "the control service has no method \"{other}\"; it serves sim.profiles, \
                     sim.start, sim.stop and sim.state"
                ),
            )),
        }
    }

    async fn sim_start(
        self: &Arc<Self>,
        params: &serde_json::Map<String, Value>,
    ) -> Result<Value, MethodError> {
        let profile_id = protocol::param_str(params, "profile")?.ok_or_else(|| {
            MethodError::bad_request(format!(
                "sim.start needs \"profile\"; sim.profiles lists them ({})",
                profiles::ids().join(", ")
            ))
        })?;
        let profile = profiles::find(profile_id).ok_or_else(|| {
            MethodError::not_found(format!(
                "no profile named \"{profile_id}\"; sim.profiles lists the ones that exist ({})",
                profiles::ids().join(", ")
            ))
        })?;
        let request = RunRequest {
            profile,
            level_db: protocol::param_f64(params, "level_db")?.unwrap_or(0.0),
            speed: protocol::param_f64(params, "speed")?.unwrap_or(1.0),
            duration_s: protocol::param_f64(params, "duration_s")?
                .unwrap_or(sim::DEFAULT_DURATION_S),
        };
        request.validate().map_err(MethodError::bad_request)?;

        let mut ctl = self.controller.lock().await;
        if let Some(run) = &ctl.run {
            if !run.is_finished() {
                return Err(MethodError::refused(format!(
                    "run {} is still running; send sim.stop first",
                    run.run_id()
                )));
            }
        }
        ctl.started += 1;
        let run_id = format!("sim-{}-{}", unix_ms(), ctl.started);
        let echo = json!({
            "profile": request.profile.id,
            "level_db": request.level_db,
            "speed": request.speed,
            "duration_s": request.duration_s,
        });
        let run = tokio::task::spawn_blocking(move || SimRun::start(run_id, request))
            .await
            .map_err(|e| MethodError::internal(format!("the run start task failed: {e}")))?
            .map_err(MethodError::refused)?;
        let run = Arc::new(run);
        ctl.run = Some(run.clone());
        drop(ctl);
        self.spawn_display(run.clone());

        let mut result = json!({"run_id": run.run_id(), "source": "SIM"});
        if let (Value::Object(r), Value::Object(e)) = (&mut result, echo) {
            r.extend(e);
        }
        Ok(result)
    }

    async fn sim_stop(self: &Arc<Self>) -> Result<Value, MethodError> {
        let run = self.controller.lock().await.run.clone();
        let Some(run) = run else {
            return Ok(json!({"state": "idle", "run_id": Value::Null}));
        };
        run.request_stop();
        let waiter = run.clone();
        let finished = tokio::task::spawn_blocking(move || waiter.wait(STOP_WAIT))
            .await
            .unwrap_or(false);
        if !finished {
            return Err(MethodError::internal(format!(
                "run {} did not stop within {} s; the stop request stands",
                run.run_id(),
                STOP_WAIT.as_secs()
            )));
        }
        Ok(json!({"state": run.protocol_state(), "run_id": run.run_id()}))
    }

    async fn sim_state(self: &Arc<Self>) -> Value {
        let run = self.controller.lock().await.run.clone();
        match run {
            None => json!({"state": "idle", "run_id": Value::Null}),
            Some(run) => {
                let snap = run.snapshot();
                let finish = snap.finish.as_ref();
                json!({
                    "state": run.protocol_state(),
                    "run_id": run.run_id(),
                    "source": "SIM",
                    "profile": run.request().profile.id,
                    "phase": sim::phase_name(snap.phase),
                    "elapsed_s": crate::views::round_to(run.elapsed_s(), 3),
                    "level_db": crate::views::round_to(snap.level_db, 2),
                    "outcome": finish.map(|f| f.outcome.as_str()),
                    "reason": finish.map(|f| f.reason.clone()),
                })
            }
        }
    }

    /// Publish `random.display` for `run` every [`DISPLAY_PERIOD`] until it has
    /// finished, then once more with the final state and the reason it ended.
    fn spawn_display(self: &Arc<Self>, run: Arc<SimRun>) {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(DISPLAY_PERIOD);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let finished = run.is_finished();
                let mut arrays = Arrays::new();
                let data = run.display_data(&mut arrays);
                let seq = svc.seq.fetch_add(1, Ordering::Relaxed);
                match codec::encode(&protocol::event(DISPLAY_TOPIC, seq, data), &arrays) {
                    Ok(frame) => {
                        // No subscribers is not an error: the run goes on unwatched.
                        let _ = svc.events.send(Arc::new(frame));
                    }
                    Err(e) => eprintln!("sds-control: display frame not encoded: {e}"),
                }
                if finished {
                    break;
                }
            }
        });
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Accept connections until the listener fails.
pub async fn serve(listener: TcpListener, service: Arc<Service>) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let service = service.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(stream, service).await {
                eprintln!("sds-control: connection ended: {e}");
            }
        });
    }
}

async fn connection(
    stream: TcpStream,
    service: Arc<Service>,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();
    sink.send(Message::Text(protocol::hello().to_string()))
        .await?;

    // The writer owns the sink: responses arrive on `out`, events on `events`.
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let mut events = service.subscribe();
    let writer = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                m = out_rx.recv() => match m {
                    Some(m) => m,
                    None => break,
                },
                e = events.recv() => match e {
                    Ok(frame) => Message::Binary(frame.as_ref().clone()),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    while let Some(msg) = source.next().await {
        let message = match msg? {
            Message::Text(text) => codec::decode_text(&text),
            Message::Binary(bytes) => codec::decode(&bytes).map(|d| d.message),
            Message::Close(_) => break,
            _ => continue,
        };
        let service = service.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            let reply = match message {
                Err(e) => protocol::error_response(
                    protocol::UNREADABLE_ID,
                    &MethodError::bad_request(format!("the frame could not be decoded: {e}")),
                ),
                Ok(message) => match protocol::parse_request(&message) {
                    Err((id, e)) => protocol::error_response(id, &e),
                    Ok(Request { id, method, params }) => {
                        match service.call(&method, &params).await {
                            Ok(result) => protocol::response(id, result),
                            Err(e) => protocol::error_response(id, &e),
                        }
                    }
                },
            };
            let frame = match codec::encode_frame(&reply, &Arrays::new()) {
                Ok(Frame::Text(t)) => Message::Text(t),
                Ok(Frame::Binary(b)) => Message::Binary(b),
                Err(e) => Message::Text(
                    protocol::error_response(
                        protocol::UNREADABLE_ID,
                        &MethodError::internal(format!("the reply could not be encoded: {e}")),
                    )
                    .to_string(),
                ),
            };
            let _ = out.send(frame).await;
        });
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}
