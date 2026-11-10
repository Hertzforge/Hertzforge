//! The Signals goldens in `schema/golden/`, against this crate's own code: a backend
//! that reads and drives the goldens' samples, tapped as every station is, gives the
//! golden data frame byte for byte, and the golden bytes decode to their expected JSON
//! and encode back; the golden subscription parses to the golden result; and an
//! unsubscribe takes and gives nothing.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use sds_control::codec::{self, Arrays};
use sds_control::signals::{subscribe, Hub, Tapped};
use sds_control::{protocol, table};
use serde_json::Value;
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// Structural equality with numbers compared by value and keys in order.
fn same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            assert_eq!(x.as_f64(), y.as_f64(), "number differs at {path}")
        }
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            assert_eq!(kx, ky, "keys differ at {path}");
            for (k, v) in x {
                same(v, &y[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "lengths differ at {path}");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                same(u, v, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "values differ at {path}"),
    }
}

/// Row `index`'s sample at station frame `frame`, as `make_golden.py` writes it.
fn sample(index: u32, frame: u64) -> f32 {
    match index {
        1 => ((frame % 16) as f32 - 8.0) / 8.0,
        3 => ((frame % 8) as f32 - 4.0) / 16.0,
        _ => ((frame % 32) as f32 - 16.0) / 64.0,
    }
}

/// Reads rows 1 and 3 and accepts every drive frame, at the rate asked.
struct Script {
    frame: u64,
}

impl Backend for Script {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 1024.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = out.len() / 2;
        for f in 0..frames {
            let at = self.frame + f as u64;
            out[2 * f] = sample(1, at);
            out[2 * f + 1] = sample(3, at);
        }
        self.frame += frames as u64;
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// Rows 1 (Table, control), 2 (Drive) and 3 (Fixture, monitor).
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut fixture = table::new_row(3);
    fixture.role = ChannelRole::Monitor;
    fixture.description = "Fixture".into();
    fixture.units = Units::AccelG;
    t.channels[0].description = "Table".into();
    t.channels.push(fixture);
    t
}

/// The goldens' run: 2048 frames read and driven at 1024 Hz, through the tap.
fn tapped() -> Tapped<Script> {
    let mut b = Tapped::new(Script { frame: 0 }, Hub::new());
    b.configure(&table(), &AcquisitionConfig::new(1024.0, 256))
        .unwrap();
    let mut out = vec![0f32; 2 * 256];
    for block in 0..8u64 {
        let drive: Vec<f32> = (0..256).map(|f| sample(2, block * 256 + f)).collect();
        assert_eq!(b.write_drive(&drive).unwrap(), 256);
        assert_eq!(b.read_input(&mut out).unwrap(), InputRead::Frames(256));
    }
    b
}

fn golden_subscription() -> subscribe::Subscription {
    let request = read_json("request_signals_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "signals.subscribe");
    subscribe::parse(&parsed.params).unwrap()
}

#[test]
fn signals_the_subscription_parses_to_the_golden_result() {
    let b = tapped();
    let got = subscribe::result_json(&golden_subscription(), b.run().map(|r| &**r));
    same(
        &got,
        &read_json("response_signals_subscribe.json")["result"],
        "result",
    );
}

#[test]
fn signals_the_data_frame_is_the_golden_bytes() {
    let b = tapped();
    let run = b.run().unwrap();
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(run, &golden_subscription(), &mut arrays);
    let got = codec::encode(&protocol::event("signals.data", 220, data), &arrays).unwrap();
    let want = std::fs::read(golden("event_signals_data.bin")).unwrap();
    if got != want {
        let (a, b) = (
            codec::decode(&got).unwrap().expanded().to_string(),
            codec::decode(&want).unwrap().expanded().to_string(),
        );
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        let from = at.saturating_sub(150);
        panic!(
            "decoded frames differ at {at}\n got: {}\nwant: {}",
            &a[from..(at + 150).min(a.len())],
            &b[from..(at + 150).min(b.len())]
        );
    }
}

#[test]
fn signals_the_golden_bytes_decode_to_their_expected_json_and_back() {
    let bytes = std::fs::read(golden("event_signals_data.bin")).unwrap();
    let decoded = codec::decode(&bytes).unwrap();
    same(
        &decoded.expanded(),
        &read_json("event_signals_data.expected.json"),
        "event",
    );
    let order = codec::header_array_order(&bytes).unwrap();
    let order: Vec<&str> = order.iter().map(String::as_str).collect();
    assert_eq!(
        codec::reencode(&decoded, &order).unwrap(),
        bytes,
        "decode then encode gives the golden bytes"
    );
}

#[test]
fn signals_an_unsubscribe_takes_and_gives_nothing() {
    let request = read_json("request_signals_unsubscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "signals.unsubscribe");
    assert!(parsed.params.is_empty());
    let response = read_json("response_signals_unsubscribe.json");
    assert_eq!(response["result"], serde_json::json!({}));
}
