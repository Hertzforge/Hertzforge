//! Shared test support: a protocol client over a real loopback WebSocket, a service
//! started with test options, and the schema checker.
#![allow(dead_code)]

pub mod schema;

use futures_util::{SinkExt, StreamExt};
use sds_control::codec::{self, Decoded};
use sds_control::server::{self, Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One event as received: the message, and the decoded arrays of a binary frame.
pub struct Event {
    pub msg: Value,
    pub decoded: Option<Decoded>,
    /// When it was read.
    pub at: Instant,
}

impl Event {
    pub fn topic(&self) -> &str {
        self.msg["topic"].as_str().unwrap_or("")
    }
    pub fn data(&self) -> &Value {
        &self.msg["data"]
    }
}

/// Test options: a fixed NI scan, a scratch data directory, the simulator `speed`
/// times real time, and the given orphan timeout and heartbeat.
pub fn options(
    scan: FixedScanner,
    speed: f64,
    timeout: Duration,
    heartbeat: Duration,
) -> ServiceOptions {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    ServiceOptions {
        heartbeat,
        timeout,
        scanner: Arc::new(scan),
        data_root: dir.clone(),
        capture_root: Some(dir),
        sim_speed: speed,
        bind: "ws://127.0.0.1:0/".into(),
        mimo_driver: None,
    }
}

/// Serve `service` on an ephemeral loopback port.
pub async fn serve(service: Arc<Service>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, service));
    addr
}

pub struct Client {
    pub ws: Ws,
    next_id: i64,
    /// The id of the last request sent.
    pub last_id: i64,
    pub events: Vec<Event>,
}

impl Client {
    pub async fn connect(addr: SocketAddr) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .expect("connects");
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => {
                let hello: Value = serde_json::from_str(&t).unwrap();
                assert_eq!(hello["service"], "control");
            }
            other => panic!("first frame must be a text hello, got {other:?}"),
        }
        Client {
            ws,
            next_id: 1,
            last_id: 0,
            events: Vec::new(),
        }
    }

    /// Read one frame within `wait`; events are recorded. `None` on timeout.
    pub async fn read(&mut self, wait: Duration) -> Option<Value> {
        let msg = tokio::time::timeout(wait, self.ws.next()).await.ok()??;
        match msg.expect("frame ok") {
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == "event" {
                    self.events.push(Event {
                        msg: v.clone(),
                        decoded: None,
                        at: Instant::now(),
                    });
                }
                Some(v)
            }
            Message::Binary(b) => {
                // Binary responses are kept too, for their arrays.
                let decoded = codec::decode(&b).expect("binary frames decode");
                let v = decoded.message.clone();
                self.events.push(Event {
                    msg: v.clone(),
                    decoded: Some(decoded),
                    at: Instant::now(),
                });
                Some(v)
            }
            _ => Some(Value::Null),
        }
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.last_id = id;
        self.next_id += 1;
        let req = json!({"type": "request", "id": id, "method": method, "params": params});
        self.ws.send(Message::Text(req.to_string())).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self
                .read(left)
                .await
                .unwrap_or_else(|| panic!("no answer to {method} within 60 s"));
            if m["type"] == "response" && m["id"] == id {
                return m;
            }
        }
    }

    /// The result, or a panic naming the error.
    pub async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        if r.get("error").is_some() {
            panic!("{method} failed: {}", r["error"]);
        }
        r["result"].clone()
    }

    /// The error's reason.
    pub async fn refused(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert!(r.get("error").is_some(), "{method} was not refused: {r}");
        r["error"].clone()
    }

    /// Read until an event satisfies `pred` (recorded events are checked first).
    pub async fn wait_for(
        &mut self,
        what: &str,
        wait: Duration,
        pred: impl Fn(&Event) -> bool,
    ) -> usize {
        if let Some(i) = self.events.iter().position(&pred) {
            return i;
        }
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("no event for {what} within {wait:?}");
            }
            if self.read(left).await.is_some() {
                if let Some(i) = self.events.iter().position(&pred) {
                    return i;
                }
            }
        }
    }

    /// Wait for a `random.state` event whose state is `state`.
    pub async fn wait_state(&mut self, state: &str, wait: Duration) -> Value {
        let i = self
            .wait_for(&format!("random.state {state}"), wait, |e| {
                e.topic() == "random.state" && e.data()["state"] == state
            })
            .await;
        self.events[i].data().clone()
    }

    /// Wait for a `random.state` event in `phase`.
    pub async fn wait_phase(&mut self, phase: &str, wait: Duration) -> Value {
        let i = self
            .wait_for(&format!("random.state phase {phase}"), wait, |e| {
                e.topic() == "random.state" && e.data()["phase"] == phase
            })
            .await;
        self.events[i].data().clone()
    }

    /// Keep reading (answering pings) for `d`.
    pub async fn pump(&mut self, d: Duration) {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let _ = self.read(left).await;
        }
    }

    /// The decoded arrays of the response to request `id`, when it came as a binary
    /// frame.
    pub fn decoded_response(&self, id: &Value) -> Option<&Decoded> {
        self.events
            .iter()
            .rev()
            .find(|e| e.msg["type"] == "response" && &e.msg["id"] == id)
            .and_then(|e| e.decoded.as_ref())
    }

    pub fn of_topic(&self, topic: &str) -> Vec<&Event> {
        self.events.iter().filter(|e| e.topic() == topic).collect()
    }
}

/// A Random configuration for the simulator: the classic ramp-and-plateau profile,
/// 8192 Hz, 16 Hz lines, a half-second control loop of 8 averages, the protocol's
/// soft start and level ramp, and a short shutdown ramp.
///
/// Eight averages, not two: the controller restarts its accumulated control
/// estimate on every loop of a level ramp, so the first loop judged after a ramp is
/// a single loop's estimate. At two averages (4 degrees of freedom) about one line
/// in eleven reads below a -6 dB abort floor by chance, and the 10-line envelope
/// abort trips at the end of every ramp on a perfect plant.
pub fn sim_config(mode: &str, levels: Value, full_level_s: f64) -> Value {
    json!({
        "title": "Protocol Random",
        "profile": {
            "breakpoints": [
                {"freq_hz": 20, "psd_g2_hz": 0.01},
                {"freq_hz": 80, "slope_db_oct": 3},
                {"freq_hz": 350, "psd_g2_hz": 0.04},
                {"freq_hz": 2000, "slope_db_oct": -3}
            ],
            "alarm_db": [3, 3], "abort_db": [6, 6]
        },
        "acquisition": {"fs_hz": 8192, "n_pts": 512, "n_avg": 8},
        "schedule": {"mode": mode, "levels": levels, "full_level_s": full_level_s},
        "control": {
            "review_after_sysid": false, "drive_ceiling_vrms": 20,
            "shutdown_ramp_s": 0.75, "pretest": {"averages": 4}
        },
        "limits": [],
        "feasibility": null
    })
}
