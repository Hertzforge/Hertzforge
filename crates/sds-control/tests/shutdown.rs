//! Shutting the control service down never ends the process with a run live.
//!
//! A killed process can leave an NI output holding its last sample (the workspace
//! `Cargo.toml` says why `panic = "abort"` is forbidden), so a stop request ends a
//! live run with its controlled stop first: `Service::shutdown` in the service, and
//! in the process the stop requests that lead to it (the end of standard input with
//! `--watch-stdin`, which is how the desktop app stops the service and how the
//! service learns that the app has died, and SIGTERM). The process tests run the
//! built `sds-control` binary with a scratch data root.
//!
//! Every run here is SIMULATED; nothing in this file is evidence about hardware.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(60);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// Claim control, select the simulator, configure a short manual run and start it;
/// back once the loop drives.
async fn driving(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok(
        "random.configure",
        json!({"config": sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0)}),
    )
    .await;
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shutdown_ends_a_driving_run_with_its_ramp_and_starts_no_other() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc.clone()).await;
    let schema = Schema::load();
    let mut c = Client::connect(addr).await;
    driving(&mut c).await;

    let shutdown = tokio::spawn(async move { svc.shutdown().await });
    let stopping = c.wait_state("stopping", LONG).await;
    assert_eq!(
        stopping["phase"], "ramping_down",
        "the shutdown ramps the level down: {stopping}"
    );
    // Nothing that could start or raise a drive is taken while the service shuts down.
    for (method, params) in [
        ("random.level", json!({"to_db": 0})),
        ("random.start", json!({})),
        ("sim.start", json!({"profile": "flat"})),
    ] {
        let r = c.call(method, params).await;
        schema.assert_valid(&r, None);
        assert_eq!(r["error"]["reason"], "shutting_down", "{method}: {r}");
    }
    let what = tokio::time::timeout(LONG, shutdown)
        .await
        .expect("the shutdown returns once the run has ended")
        .unwrap()
        .expect("the shutdown names the run it stopped");
    // It returns only when the run has ended: the state is final the moment it does.
    let state = c.ok("random.state", json!({})).await;
    assert_eq!(state["state"], "finished", "{state}");
    let end = &state["end"];
    assert_eq!(end["outcome"], "safe_stop", "{end}");
    assert_eq!(end["trip"], Value::Null, "a controlled stop, not an abort");
    assert!(
        end["message"]
            .as_str()
            .unwrap()
            .starts_with("The control service was shut down, so it stopped the run"),
        "{end}"
    );
    assert!(what.contains("(safe_stop)"), "{what}");
    let r = c.call("random.arm", json!({})).await;
    assert_eq!(r["error"]["reason"], "shutting_down", "{r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shutdown_with_no_run_live_returns_at_once() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    let t0 = Instant::now();
    assert_eq!(svc.shutdown().await, None);
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    // Reading goes on; starting does not.
    assert_eq!(
        c.ok("random.state", json!({})).await["state"],
        "unconfigured"
    );
    let r = c.call("random.start", json!({})).await;
    assert_eq!(r["error"]["reason"], "shutting_down", "{r}");
}

// ---- the process ------------------------------------------------------------------

/// The built service, on a free loopback port, with a scratch data root, its
/// standard input a pipe the test holds, and its standard error read line by line.
struct Process {
    child: Child,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    addr: SocketAddr,
}

impl Process {
    fn start(args: &[&str]) -> Process {
        let root = std::env::temp_dir().join(format!(
            "sds-control-shutdown-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut child = Command::new(env!("CARGO_BIN_EXE_sds-control"))
            .args(["--port", "0"])
            .args(args)
            .env("HERTZFORGE_DATA_ROOT", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the built service starts");
        let stderr = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut p = Process {
            child,
            lines,
            seen: Vec::new(),
            addr: "127.0.0.1:0".parse().unwrap(),
        };
        let line = p.wait_line("listening on ws://", Duration::from_secs(30));
        let at = line.split("ws://").nth(1).unwrap();
        p.addr = at[..at.find('/').unwrap()].parse().unwrap();
        p
    }

    /// The first line of standard error (seen or still to come) holding `text`.
    fn wait_line(&mut self, text: &str, wait: Duration) -> String {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(l) = self.seen.iter().find(|l| l.contains(text)) {
                return l.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) => self.seen.push(l),
                Err(_) => panic!(
                    "no \"{text}\" within {wait:?}; the service said {:?}",
                    self.seen
                ),
            }
        }
    }

    fn wait_exit(&mut self, wait: Duration) -> ExitStatus {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the service did not exit within {wait:?}; it said {:?}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // A test that failed before its stop leaves no process behind. Closing
        // standard input first lets a service that watches it stop its run.
        drop(self.child.stdin.take());
        if self.wait_exit_quietly(Duration::from_secs(2)).is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Process {
    fn wait_exit_quietly(&mut self, wait: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if let Ok(Some(s)) = self.child.try_wait() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

/// Stop a driving run's process with `stop`, and check that the run ended with its
/// controlled stop before the process ended, cleanly.
async fn stops_its_run_before_it_exits(args: &[&str], stop: impl FnOnce(&mut Process), why: &str) {
    let mut p = Process::start(args);
    let mut c = Client::connect(p.addr).await;
    driving(&mut c).await;
    stop(&mut p);

    let stopping = c.wait_state("stopping", LONG).await;
    assert_eq!(stopping["phase"], "ramping_down", "{stopping}");
    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{end}");
    assert_eq!(end["end"]["trip"], Value::Null);
    let status = p.wait_exit(LONG);
    assert!(
        status.success(),
        "{status:?}; the service said {:?}",
        p.seen
    );
    p.wait_line(&format!("{why}; shutting down"), Duration::from_secs(5));
    p.wait_line("ended (safe_stop)", Duration::from_secs(5));
    p.wait_line("sds-control: stopped", Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_end_of_standard_input_stops_the_run_then_the_process() {
    stops_its_run_before_it_exits(
        &["--watch-stdin"],
        |p| drop(p.child.stdin.take()),
        "standard input closed",
    )
    .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigterm_stops_the_run_then_the_process() {
    stops_its_run_before_it_exits(
        &[],
        |p| {
            let status = Command::new("kill")
                .args(["-TERM", &p.child.id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());
        },
        "terminated (SIGTERM)",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_flag_the_end_of_standard_input_changes_nothing() {
    // `scripts/dev.sh` starts the service in the background, where standard input is
    // already at its end: that must not stop it.
    let mut p = Process::start(&[]);
    drop(p.child.stdin.take());
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(p.child.try_wait().unwrap().is_none(), "{:?}", p.seen);
    let mut c = Client::connect(p.addr).await;
    assert_eq!(
        c.ok("random.state", json!({})).await["state"],
        "unconfigured"
    );
}
