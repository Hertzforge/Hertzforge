//! The mixed check through `laws.check` with `mixed`: the built-in tone law passes
//! all eight cases in its role on stations built from each case's own facts, and
//! the role is refused by name when it is neither.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::json;
use std::time::Duration;
use support::{options, serve, Client};

fn service() -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_built_in_tone_law_passes_the_mixed_check_in_its_role() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    let e = c
        .refused("laws.check", json!({"id": "servo_blend", "mixed": "drive"}))
        .await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "mixed");
    let r = c
        .ok("laws.check", json!({"id": "servo_blend", "mixed": "tones"}))
        .await;
    let cases = r["report"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 9, "{r}");
    for k in cases {
        assert_eq!(k["role"], "tones");
        assert_eq!(k["passed"], true, "{k}");
    }
    assert_eq!(r["report"]["passed"], true);
    assert_eq!(r["report"]["recorded"], false);
    assert_eq!(r["law"]["hardware"], true, "a built-in drives hardware");
}
