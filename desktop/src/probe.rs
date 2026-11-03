//! Whether a run is live, asked of the control service over its own protocol, on a
//! connection of the app's own that only reads (it never claims control).

use serde_json::{json, Value};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use tungstenite::Message;

const CONNECT_WAIT: Duration = Duration::from_secs(2);
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// A sentence naming the live run, or `None` when nothing runs. An error means
/// the service could not be asked (it has gone, or does not answer).
pub fn live_run(port: u16) -> Result<Option<String>, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&addr, CONNECT_WAIT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(ANSWER_WAIT))
        .map_err(|e| e.to_string())?;
    let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/"), stream)
        .map_err(|e| e.to_string())?;
    for (id, method) in [(1, "random.state"), (2, "sim.state")] {
        let req = json!({"type": "request", "id": id, "method": method, "params": {}});
        ws.send(Message::text(req.to_string()))
            .map_err(|e| e.to_string())?;
    }
    let (mut random, mut sim) = (None, None);
    let deadline = Instant::now() + ANSWER_WAIT;
    while random.is_none() || sim.is_none() {
        if Instant::now() > deadline {
            return Err("the control service did not answer".into());
        }
        let Message::Text(text) = ws.read().map_err(|e| e.to_string())? else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        if v["type"] != "response" {
            continue;
        }
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        match v["id"].as_i64() {
            Some(1) => random = Some(result),
            Some(2) => sim = Some(result),
            _ => {}
        }
    }
    let _ = ws.close(None);
    Ok(describe(
        random.as_ref().unwrap_or(&Value::Null),
        sim.as_ref().unwrap_or(&Value::Null),
    ))
}

/// The live run in words, from the answers to `random.state` and `sim.state`.
pub fn describe(random: &Value, sim: &Value) -> Option<String> {
    let state = random["state"].as_str().unwrap_or("");
    let source = random["source"].as_str().unwrap_or("");
    let on = if source == "HARDWARE" {
        "on the hardware"
    } else {
        "on the simulator"
    };
    let doing = match state {
        "armed" => Some("is armed"),
        "running" => Some("is running"),
        "review" => Some("is holding for review"),
        "stopping" => Some("is stopping"),
        _ => None,
    };
    if let Some(doing) = doing {
        let title = random["title"].as_str().filter(|t| !t.is_empty());
        return Some(match title {
            Some(t) => format!("The Random test \"{t}\" {doing} {on}."),
            None => format!("A Random test {doing} {on}."),
        });
    }
    if sim["state"] == "running" {
        return Some("A simulation is running.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_run_is_named_and_an_ended_one_is_not() {
        let idle = json!({"state": "idle", "run_id": null});
        for state in ["armed", "running", "review", "stopping"] {
            let r = json!({"state": state, "source": "SIM", "title": "Bench"});
            let said = describe(&r, &idle).unwrap();
            assert!(said.contains("\"Bench\""), "{said}");
            assert!(said.contains("on the simulator"), "{said}");
        }
        let hw = json!({"state": "running", "source": "HARDWARE", "title": ""});
        assert_eq!(
            describe(&hw, &idle).unwrap(),
            "A Random test is running on the hardware."
        );
        for state in ["unconfigured", "configured", "blocked", "finished"] {
            assert_eq!(describe(&json!({"state": state}), &idle), None, "{state}");
        }
        assert!(describe(&Value::Null, &json!({"state": "running"})).is_some());
        assert_eq!(describe(&Value::Null, &Value::Null), None);
    }
}
