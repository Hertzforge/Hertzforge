//! The service over a real loopback WebSocket: hello, the four methods, errors, and
//! the `random.display` stream of a paced SIM run.

use futures_util::{SinkExt, StreamExt};
use sds_control::codec;
use sds_control::server::{self, Service};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client {
    ws: Ws,
    next_id: i64,
    events: Vec<Value>,
}

impl Client {
    async fn connect() -> (Client, Value) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(server::serve(listener, Service::new()));
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .expect("connects");
        let hello = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("first frame must be a text hello, got {other:?}"),
        };
        (
            Client {
                ws,
                next_id: 1,
                events: Vec::new(),
            },
            hello,
        )
    }

    /// Read one frame: a text message, or a decoded binary event (recorded).
    async fn read(&mut self) -> Option<Value> {
        let msg = tokio::time::timeout(Duration::from_secs(30), self.ws.next())
            .await
            .expect("a frame within 30 s")?
            .expect("frame ok");
        match msg {
            Message::Text(t) => Some(serde_json::from_str(&t).unwrap()),
            Message::Binary(b) => {
                let decoded = codec::decode(&b).expect("binary frames decode");
                // Keep the decoded arrays' lengths with the event for structure checks.
                let mut ev = decoded.message.clone();
                let lens: Vec<Value> = decoded
                    .arrays
                    .values()
                    .map(|a| json!(a.data.len()))
                    .collect();
                ev["_array_lengths"] = Value::Array(lens);
                self.events.push(ev.clone());
                Some(ev)
            }
            _ => None,
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"type": "request", "id": id, "method": method, "params": params});
        self.ws.send(Message::Text(req.to_string())).await.unwrap();
        loop {
            if let Some(m) = self.read().await {
                if m["type"] == "response" && m["id"] == id {
                    return m;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_methods_errors_and_a_live_display_stream() {
    let (mut c, hello) = Client::connect().await;
    assert_eq!(
        hello,
        json!({"type": "hello", "protocol": "sds/1", "service": "control",
                             "version": sds_control::protocol::VERSION})
    );

    let r = c.call("sim.state", json!({})).await;
    assert_eq!(r["result"], json!({"state": "idle", "run_id": null}));

    let r = c.call("sim.profiles", json!({})).await;
    let profiles = r["result"]["profiles"].as_array().unwrap();
    assert!(profiles.len() >= 2);
    for p in profiles {
        for key in ["id", "title", "summary"] {
            assert!(p[key].is_string(), "profile lacks {key}: {p}");
        }
    }

    let r = c.call("no.such", json!({})).await;
    assert_eq!(r["error"]["code"], "unknown_method");
    let r = c.call("sim.start", json!({"profile": "nope"})).await;
    assert_eq!(r["error"]["code"], "not_found");
    assert!(r["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ramp_plateau"));
    let r = c
        .call(
            "sim.start",
            json!({"profile": "ramp_plateau", "level_db": 40}),
        )
        .await;
    assert_eq!(r["error"]["code"], "bad_request");

    let r = c
        .call(
            "sim.start",
            json!({"profile": "ramp_plateau", "level_db": 0, "speed": 1}),
        )
        .await;
    let run_id = r["result"]["run_id"].as_str().expect("run_id").to_owned();
    assert_eq!(r["result"]["source"], "SIM");
    let r = c
        .call("sim.start", json!({"profile": "ramp_plateau"}))
        .await;
    assert_eq!(r["error"]["code"], "refused", "one run at a time");

    // About five events a second while running.
    let t0 = Instant::now();
    let before = c.events.len();
    while t0.elapsed() < Duration::from_millis(2100) {
        c.read().await;
    }
    let streamed = &c.events[before..];
    assert!(
        (8..=13).contains(&streamed.len()),
        "{} events in 2.1 s",
        streamed.len()
    );
    for ev in streamed {
        assert_eq!(ev["type"], "event");
        assert_eq!(ev["topic"], "random.display");
        let d = &ev["data"];
        assert_eq!(d["run_id"], run_id.as_str());
        assert_eq!(d["source"], "SIM");
        assert_eq!(d["state"], "running");
        assert_eq!(d["views"][0]["view"], "spectrum");
        assert_eq!(d["views"][1]["view"], "reading");
        assert!(ev.get("arrays").is_none(), "decoder drops the arrays list");
    }
    let seqs: Vec<u64> = streamed
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
    let elapsed = streamed.last().unwrap()["data"]["elapsed_s"]
        .as_f64()
        .unwrap();
    assert!(
        (1.0..=2.3).contains(&elapsed),
        "paced: {elapsed} simulated s after ~2.1 s"
    );

    let r = c.call("sim.stop", json!({})).await;
    assert_eq!(r["result"]["state"], "stopped");
    // The stream ends with a stopped event that says why.
    let stopped = loop {
        if let Some(ev) = c.read().await {
            if ev["type"] == "event" && ev["data"]["state"] == "stopped" {
                break ev;
            }
        }
    };
    assert_eq!(stopped["data"]["outcome"], "operator_stop");
    assert!(stopped["data"]["reason"]
        .as_str()
        .unwrap()
        .contains("operator"));

    let r = c.call("sim.state", json!({})).await;
    assert_eq!(r["result"]["state"], "stopped");
    assert_eq!(r["result"]["run_id"], run_id.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_frames_get_bad_request_not_silence() {
    let (mut c, _) = Client::connect().await;
    c.ws.send(Message::Text("not json".into())).await.unwrap();
    let r = c.read().await.unwrap();
    assert_eq!(r["error"]["code"], "bad_request");
    assert_eq!(r["id"], sds_control::protocol::UNREADABLE_ID);
    c.ws.send(Message::Text(
        json!({"type": "request", "id": 9}).to_string(),
    ))
    .await
    .unwrap();
    let r = c.read().await.unwrap();
    assert_eq!(
        (r["id"].as_i64(), r["error"]["code"].as_str()),
        (Some(9), Some("bad_request"))
    );
}
