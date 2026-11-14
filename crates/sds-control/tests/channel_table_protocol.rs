//! The channel table through the protocol, on a real loopback WebSocket: a row's
//! optional members (the analysis side's columns, the analysis tier, the limit mark
//! and any other column) survive `channels.set`, come back from `channels.get`, reach
//! a connection made afterwards (a shell page reloaded) and the `channels.changed`
//! event, and are what a run's `table.json` is written from.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::{options, serve, Client};

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": format!("Row {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" },
        "sensitivity_mv_per_eu": if drive { 1000 } else { 100 },
        "range_max": if drive { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

fn with(mut row: Value, members: Value) -> Value {
    let o = row.as_object_mut().unwrap();
    for (k, v) in members.as_object().unwrap() {
        o.insert(k.clone(), v.clone());
    }
    row
}

fn table() -> Value {
    json!([
        with(
            row(1, "control"),
            json!({
                "node": 101, "direction": "Z+", "channel_type": "acceleration",
                "analysis_role": "response", "daq_range_v": 5, "sensor_range_v": 4.5,
                "serial_number": "0042", "make": "Maker", "model": "M-1", "triax_dof": "Z",
                "expiration": "2027-03-01", "extra": {"cable": "C-12"}
            })
        ),
        with(
            row(2, "monitor"),
            json!({"node": 102, "direction": "X-", "limit": true, "analysis_tier": "rotating"})
        ),
        row(3, "drive"),
    ])
}

#[tokio::test]
async fn channel_table_analysis_columns_survive_set_get_and_a_new_connection() {
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    svc.start_background();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows = table();
    let set = c
        .ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    assert_eq!(set["report"]["errors"], json!([]));

    // The same connection reads them back, member for member.
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["channels"], rows);

    // So does a connection made afterwards: a page reload reads the table from the
    // service, not from its own memory.
    let mut reloaded = Client::connect(addr).await;
    let again = reloaded.ok("channels.get", json!({})).await;
    assert_eq!(again["channels"], rows);
    assert_eq!(again["revision"], got["revision"]);

    // And the event every connection gets after a write carries them too.
    c.pump(Duration::from_millis(200)).await;
    let changed = c.of_topic("channels.changed");
    let last = changed.last().expect("a channels.changed event");
    assert_eq!(last.data()["channels"], rows);
}
