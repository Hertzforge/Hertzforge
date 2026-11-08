//! The run thread's next pulse is due the time between pulses after the device
//! started the last one, as the controller stamped it, and not after the moment
//! the thread asked for it.
//!
//! Between the ask and the device's start the controller waits out the rest of
//! the floor, checks the drive and brings the station up. Counted from the ask,
//! the countdown on screen reached zero that much before the controller would
//! fire, and a manual fire was accepted early (the controller then waited it out
//! itself). Here the simulated engines are wrapped so that their device starts
//! [`LATE`] after each ask and they say when; the countdown after a pulse and the
//! moment the next ask is let through must both count from that start.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(30);
/// The time between pulses, seconds.
const SPACING_S: f64 = 1.0;
/// From the ask to the device's start.
const LATE: Duration = Duration::from_millis(300);

type Times = Arc<Mutex<Vec<Instant>>>;

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": description,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows = json!([row(1, "control", "Control"), row(2, "drive", "Shaker")]);
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

/// The asks and the device's starts, as the wrapped engine saw them.
#[derive(Clone, Default)]
struct Seen {
    asked: Times,
    started: Times,
}

impl Seen {
    /// An ask, then the device's start [`LATE`] after it.
    fn ask_then_start(&self) -> Instant {
        self.asked.lock().unwrap().push(Instant::now());
        std::thread::sleep(LATE);
        let t = Instant::now();
        self.started.lock().unwrap().push(t);
        t
    }
}

mod srs_late {
    use super::Seen;
    use sds_control::protocol::MethodError;
    use sds_control::srs::engine::{
        ArmOutcome, ArmPlan, EngineContext, Fired, Identified, LevelMove, Position, Seeded,
        SrsEngine,
    };
    use sds_control::srs::sim::SimulatedEngine;
    use sds_control::srs::view::{IdentFigures, LawReading, Notch, Prediction};
    use std::time::Instant;

    /// The simulated SRS engine with a device that starts late and says when.
    pub struct Late {
        pub inner: SimulatedEngine,
        pub seen: Seen,
        pub last: Option<Instant>,
    }

    impl Late {
        pub fn new(ctx: &EngineContext, seen: Seen) -> Late {
            Late {
                inner: SimulatedEngine::new(ctx.clone()),
                seen,
                last: None,
            }
        }
    }

    impl SrsEngine for Late {
        fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
            self.inner.arm(plan)
        }
        fn identify(&mut self) -> Result<Identified, MethodError> {
            self.inner.identify()
        }
        fn seed(&mut self) -> Result<Seeded, MethodError> {
            self.inner.seed()
        }
        fn fire(&mut self) -> Result<Fired, MethodError> {
            self.last = Some(self.seen.ask_then_start());
            self.inner.fire()
        }
        fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
            self.inner.set_level(to)
        }
        fn predicted_vpk_at(&self, level_db: f64) -> Option<f64> {
            self.inner.predicted_vpk_at(level_db)
        }
        fn stop(&mut self) {
            self.inner.stop()
        }
        fn abort(&mut self, reason: &str) {
            self.inner.abort(reason)
        }
        fn position(&self) -> Position {
            self.inner.position()
        }
        fn identification(&self) -> Option<IdentFigures> {
            self.inner.identification()
        }
        fn prediction(&self) -> Option<Prediction> {
            self.inner.prediction()
        }
        fn notch(&self) -> Option<Notch> {
            self.inner.notch()
        }
        fn law_values(&self) -> Vec<LawReading> {
            self.inner.law_values()
        }
        fn misses(&self) -> u32 {
            self.inner.misses()
        }
        fn take_log(&mut self) -> Vec<(&'static str, String)> {
            self.inner.take_log()
        }
        fn last_pulse_start(&self) -> Option<Instant> {
            self.last
        }
    }
}

mod shock_late {
    use super::Seen;
    use sds_control::protocol::MethodError;
    use sds_control::shock::engine::{
        ArmOutcome, ArmPlan, EngineContext, Fired, Identified, LevelMove, Position, Seeded,
        ShockEngine,
    };
    use sds_control::shock::sim::SimulatedEngine;
    use sds_control::shock::view::{IdentFigures, LawReading};
    use std::time::Instant;

    /// The simulated classical shock engine with a device that starts late and
    /// says when. A train's last pulse starts as the train returns.
    pub struct Late {
        pub inner: SimulatedEngine,
        pub seen: Seen,
        pub last: Option<Instant>,
    }

    impl Late {
        pub fn new(ctx: &EngineContext, seen: Seen) -> Late {
            Late {
                inner: SimulatedEngine::new(ctx.clone()),
                seen,
                last: None,
            }
        }
    }

    impl ShockEngine for Late {
        fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
            self.inner.arm(plan)
        }
        fn identify(&mut self) -> Result<Identified, MethodError> {
            self.inner.identify()
        }
        fn seed(&mut self) -> Result<Seeded, MethodError> {
            self.inner.seed()
        }
        fn fire(&mut self) -> Result<Fired, MethodError> {
            self.last = Some(self.seen.ask_then_start());
            self.inner.fire()
        }
        fn train(&mut self, count: u32, spacing_s: f64) -> Result<Fired, MethodError> {
            self.seen.ask_then_start();
            let fired = self.inner.train(count, spacing_s);
            let t = Instant::now();
            *self.seen.started.lock().unwrap().last_mut().unwrap() = t;
            self.last = Some(t);
            fired
        }
        fn burst(&mut self) -> Result<Fired, MethodError> {
            self.last = Some(self.seen.ask_then_start());
            self.inner.burst()
        }
        fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
            self.inner.set_level(to)
        }
        fn predicted_vpk_at(&self, level_db: f64) -> Option<f64> {
            self.inner.predicted_vpk_at(level_db)
        }
        fn stop(&mut self) {
            self.inner.stop()
        }
        fn abort(&mut self, reason: &str) {
            self.inner.abort(reason)
        }
        fn position(&self) -> Position {
            self.inner.position()
        }
        fn identification(&self) -> Option<IdentFigures> {
            self.inner.identification()
        }
        fn law_values(&self) -> Vec<LawReading> {
            self.inner.law_values()
        }
        fn misses(&self) -> u32 {
            self.inner.misses()
        }
        fn take_log(&mut self) -> Vec<(&'static str, String)> {
            self.inner.take_log()
        }
        fn last_pulse_start(&self) -> Option<Instant> {
            self.last
        }
    }
}

/// `verb` refused `not_yet` until it is let through; the instant it was.
async fn first_accepted(c: &mut Client, verb: &str, params: Value) -> Instant {
    let give_up = Instant::now() + Duration::from_secs(5);
    loop {
        let reply = c.call(verb, params.clone()).await;
        let Some(e) = reply.get("error") else {
            return Instant::now();
        };
        assert_eq!(e["reason"], "not_yet", "{verb}: {e}");
        assert!(Instant::now() < give_up, "{verb} never let through");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The state event that shows `fired` pulses and the run waiting again.
async fn waiting_after(c: &mut Client, topic: &'static str, fired: u64) -> Value {
    let i = c
        .wait_for("waiting after a pulse", LONG, |e| {
            e.topic() == topic
                && e.data()["pulses"]["fired"] == fired
                && e.data()["phase"] == "waiting"
        })
        .await;
    c.events[i].data().clone()
}

/// After a pulse: the countdown shown counts from the device's start, and the
/// next ask is let through no sooner than the time between pulses after it.
fn assert_counts_from_the_start(seen: &Seen, waiting: &Value, accepted: Instant, what: &str) {
    let started = *seen.started.lock().unwrap().last().unwrap();
    let shown = waiting["next_pulse_s"].as_f64().expect("a countdown");
    let late = LATE.as_secs_f64();
    assert!(
        shown > SPACING_S - late / 2.0,
        "{what}: the countdown showed {shown} s just after the pulse; from the device's \
         start it is about {SPACING_S} s, from the ask about {} s",
        SPACING_S - late
    );
    let due = started + Duration::from_secs_f64(SPACING_S);
    assert!(
        accepted >= due,
        "{what}: the next ask was let through {:.3} s before the time between pulses had \
         passed since the device's start",
        (due - accepted).as_secs_f64()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn srs_counts_the_next_pulse_from_the_devices_start() {
    let seen = Seen::default();
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    let s = seen.clone();
    o.srs_engine = Some(Arc::new(move |ctx| {
        Box::new(srs_late::Late::new(ctx, s.clone()))
    }));
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let cfg = json!({
        "title": "SRS due",
        "target": {"kind": "flat", "accel_g": 50},
        "bands": {"f_lo_hz": 100, "f_hi_hz": 2000},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [-12, -6, 0], "pulses": [2, 2, 3]},
        "spacing": {"cooling_s": SPACING_S, "settling_s": 0, "compute_s": 0},
        "identification": {"nfft": 1024, "review": false},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
    });
    c.ok("srs.configure", json!({"config": cfg})).await;
    c.ok("srs.start", json!({})).await;
    c.wait_for("waiting", LONG, |e| {
        e.topic() == "srs.state" && e.data()["phase"] == "waiting"
    })
    .await;
    c.ok("srs.fire", json!({})).await;
    let waiting = waiting_after(&mut c, "srs.state", 1).await;
    let accepted = first_accepted(&mut c, "srs.fire", json!({})).await;
    assert_counts_from_the_start(&seen, &waiting, accepted, "srs.fire");
    c.ok("srs.stop", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn classical_counts_the_next_pulse_from_the_devices_start() {
    let seen = Seen::default();
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    let s = seen.clone();
    o.shock_engine = Some(Arc::new(move |ctx| {
        Box::new(shock_late::Late::new(ctx, s.clone()))
    }));
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let cfg = json!({
        "title": "Shock due",
        "pulse": {"shape": "half_sine", "amplitude_g": 20, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [-12, -9, -6, -3, 0], "pulses": [1, 1, 1, 1, 3]},
        "spacing": {"cooling_s": SPACING_S, "settling_s": 0, "compute_s": 0},
        "identification": {"duration_s": 0.5, "nfft": 2048, "review": false},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    });
    c.ok("shock.configure", json!({"config": cfg})).await;
    c.ok("shock.start", json!({})).await;
    c.wait_for("waiting", LONG, |e| {
        e.topic() == "shock.state" && e.data()["phase"] == "waiting"
    })
    .await;
    c.ok("shock.fire", json!({})).await;
    let waiting = waiting_after(&mut c, "shock.state", 1).await;
    let accepted = first_accepted(
        &mut c,
        "shock.train",
        json!({"count": 2, "spacing_s": SPACING_S}),
    )
    .await;
    assert_counts_from_the_start(&seen, &waiting, accepted, "shock.fire then shock.train");
    // After a train the next pulse counts from the train's last start.
    let waiting = waiting_after(&mut c, "shock.state", 3).await;
    let accepted = first_accepted(&mut c, "shock.fire", json!({})).await;
    assert_counts_from_the_start(&seen, &waiting, accepted, "shock.train then shock.fire");
    c.ok("shock.stop", json!({})).await;
}
