//! The Explorer contract (schema/PROTOCOL.md, "Explorer").
//!
//! Every method in the contract's table is the service's; and the configuration defaults
//! the contract states are the Explorer core's own, with the stop budget's arithmetic, so
//! the wire's defaults and the core's cannot drift apart unseen.

use hertzforge_explorer_host::slew::SlewLimits;
use hertzforge_explorer_host::{AmplitudeRamp, ExplorerHostConfig};
use sds_control::protocol::ErrorCode;
use sds_control::server::Service;
use serde_json::Map;

/// The methods of the contract's table, in its order.
const EXPLORER_METHODS: [&str; 15] = [
    "explorer.configure",
    "explorer.config",
    "explorer.start",
    "explorer.steer",
    "explorer.sweep",
    "explorer.hold",
    "explorer.stop",
    "explorer.abort",
    "explorer.survey",
    "explorer.survey.clear",
    "explorer.reset",
    "explorer.clear",
    "explorer.state",
    "explorer.log",
    "explorer.subscribe",
];

#[tokio::test]
async fn explorer_methods_are_served() {
    let svc = Service::new();
    let conn = svc.connect("explorer contract".into());
    // Every method of the contract's table is the service's: none is an unknown method,
    // whatever it answers without a configuration or control.
    for method in EXPLORER_METHODS {
        if let Err(e) = svc.call(&conn, method, &Map::new()).await {
            assert_ne!(e.code, ErrorCode::UnknownMethod, "{method}: {e:?}");
        }
    }
    // The read-only ones answer.
    for method in [
        "explorer.state",
        "explorer.config",
        "explorer.log",
        "explorer.survey",
    ] {
        assert!(
            svc.call(&conn, method, &Map::new()).await.is_ok(),
            "{method}"
        );
    }
    // A method the contract does not have still is one.
    let e = svc
        .call(&conn, "explorer.nope", &Map::new())
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::UnknownMethod);
}

#[test]
fn explorer_wire_defaults_are_the_cores_defaults() {
    // PROTOCOL.md, "Explorer Configuration": the defaults of every field but the
    // ceiling, which the wire requires.
    let core = ExplorerHostConfig::default();
    assert_eq!(core.fs_hz, 8192.0, "acquisition.fs_hz");
    assert_eq!(core.band_hz, (10.0, 2000.0), "band_hz");
    assert_eq!(core.start_hz, 100.0, "start_hz");
    assert_eq!(
        core.slew,
        SlewLimits {
            freq_oct_per_s: 1.0,
            drive_db_per_s: 6.0
        },
        "slew"
    );
    assert_eq!(core.display_nfft, 4096, "display.nfft");
    // A rise and the stop span 40 dB, then the step to or from 0 V.
    assert_eq!(core.ramp, AmplitudeRamp { ramp_span_db: 40.0 });

    // The stop budget: the service waits 30 s for a stop (`derived.stop_wait_s`), so the
    // slowest drive slew the contract accepts is 40 dB over 30 s, and the default slew's
    // stop takes 6.667 s (`derived.stop_s`).
    let stop_wait_s = sds_control::session::TIMEOUT.as_secs_f64();
    assert_eq!(stop_wait_s, 30.0);
    let slowest_db_per_s = core.ramp.ramp_span_db / stop_wait_s;
    assert!(
        (slowest_db_per_s - 1.333).abs() < 5e-4,
        "{slowest_db_per_s}"
    );
    let stop_s = core.ramp.ramp_span_db / core.slew.drive_db_per_s;
    assert!((stop_s - 6.667).abs() < 5e-4, "{stop_s}");
}
