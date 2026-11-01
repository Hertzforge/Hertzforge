//! THE WIRE TESTS: every gate, over a REAL loopback WebSocket.
//!
//! These are integration tests on purpose. The lease machine, the token check and the
//! schema gates each have unit tests that prove the rule; what those cannot prove is
//! that the SERVER puts them in the right order and actually closes the connection.
//! A refusal computed correctly and then not sent is indistinguishable from no refusal
//! at all, and only a socket can tell the difference.
//!
//! Every server binds `127.0.0.1:0` and reports the port it got. A test that hard-codes
//! a port fails whenever a daemon is already up on the box, which on the bench is most
//! of the time.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use eigen_backendd::config::DaemonConfig;
use eigen_backendd::server::Server;
use eigen_protocol::{
    Command, CommandEnvelope, Doc, Envelope, ExplorerCommand, Hello, HelloAck, LeaseAnswer,
    ObserverReason, PeerRole, ProtocolError, RecorderCommand, Response, RunRole, SchemaKind,
    StreamKind, StreamSub, PROTOCOL_VERSION,
};

const TOKEN: &str = "p5-shared-token";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A running daemon on an ephemeral loopback port, with its own data root.
struct Harness {
    addr: std::net::SocketAddr,
    server: Arc<Server>,
    root: std::path::PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn start(extra: &str) -> Harness {
    let root = std::env::temp_dir().join(format!(
        "eigen-backendd-wire-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let text = format!(
        "bind = \"127.0.0.1:0\"\ntoken = \"{TOKEN}\"\ndata_root = {:?}\n{extra}",
        root.to_string_lossy()
    );
    let cfg = DaemonConfig::parse(&text, std::path::Path::new("test.toml")).expect("valid config");
    let server = eigen_backendd::build(cfg);
    let (listener, addr) = server.bind().await.expect("bind");
    let serving = Arc::clone(&server);
    tokio::spawn(async move { serving.serve(listener).await });
    Harness { addr, server, root }
}

async fn connect(h: &Harness) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/", h.addr))
        .await
        .expect("connect");
    ws
}

async fn next_text(ws: &mut Ws) -> Option<String> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return Some(t),
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => return None,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) => return None,
            Err(_) => return None,
        }
    }
}

/// Hold a connection open for `d`, SERVICING it - reading whatever arrives so the
/// WebSocket layer answers the server's pings.
///
/// A real client has a read loop; a test that merely holds the socket variable alive
/// without polling it never sends a pong, and the daemon correctly declares it gone
/// after three missed beats. Sleeping instead of this would test the heartbeat rather
/// than the thing under test.
async fn idle(ws: &mut Ws, d: Duration) {
    let until = std::time::Instant::now() + d;
    while std::time::Instant::now() < until {
        let _ = tokio::time::timeout(Duration::from_millis(50), ws.next()).await;
    }
}

async fn say_hello(ws: &mut Ws, hello: &Hello) -> Option<String> {
    ws.send(Message::Text(serde_json::to_string(hello).unwrap()))
        .await
        .ok()?;
    next_text(ws).await
}

/// Send a command and wait for ITS response, skipping any EVENT that arrives in
/// between.
///
/// The skip is load-bearing rather than defensive: the daemon pushes
/// `Event::RunProvenance` at connect (so a peer attaching mid-run learns what the data
/// is) and events flow on the same text stream as responses. A helper that read "the
/// next text frame" would fail on whichever test happened to have a run in flight,
/// which is a flaky test dressed up as a protocol bug.
async fn cmd(ws: &mut Ws, id: u64, c: Command) -> Response {
    let env: CommandEnvelope = Envelope::new(id, c);
    ws.send(Message::Text(serde_json::to_string(&env).unwrap()))
        .await
        .unwrap();
    loop {
        let text = next_text(ws).await.expect("a response");
        if let Ok(env) = serde_json::from_str::<Envelope<Response>>(&text) {
            if env.id == id {
                return env.body;
            }
        }
        // An event, or a response to an earlier command: keep reading.
    }
}

fn refusal(text: &str) -> ProtocolError {
    match serde_json::from_str::<Envelope<Response>>(text)
        .expect("a response envelope")
        .body
    {
        Response::Rejected { error } => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn controller() -> Hello {
    Hello::new(PeerRole::Ui, "wire-test")
        .asking(RunRole::Controller)
        .with_token(TOKEN)
}

fn observer() -> Hello {
    Hello::new(PeerRole::Ui, "wire-test")
        .asking(RunRole::Observer)
        .with_token(TOKEN)
}

// =====================================================================================
// GATE 1: the version, exact match
// =====================================================================================

/// A FOREIGN VERSION is refused at the handshake, naming both numbers, and the
/// connection is CLOSED - not left open to accept commands anyway.
///
/// RED-MUTATION: delete the `hello.check()` arm in `serve_one` and this reds twice -
/// no refusal arrives, and the connection stays open.
#[tokio::test]
async fn a_foreign_protocol_version_is_refused_and_the_connection_closes() {
    let h = start("").await;
    let mut ws = connect(&h).await;

    let mut old = controller();
    old.v = PROTOCOL_VERSION - 1;
    let text = say_hello(&mut ws, &old).await.expect("a refusal arrives");
    match refusal(&text) {
        ProtocolError::VersionMismatch { ours, theirs } => {
            assert_eq!(ours, PROTOCOL_VERSION);
            assert_eq!(theirs, PROTOCOL_VERSION - 1);
        }
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
    // CLOSED: nothing more arrives.
    assert!(
        next_text(&mut ws).await.is_none(),
        "the connection must close after a version refusal"
    );

    // ANTI-VACUITY: the CURRENT version is admitted on the same server.
    let mut ok = connect(&h).await;
    let ack = say_hello(&mut ok, &controller()).await.expect("an ack");
    assert!(serde_json::from_str::<HelloAck>(&ack).is_ok(), "{ack}");
}

// =====================================================================================
// GATE 2: the shared token
// =====================================================================================

/// A WRONG or ABSENT token is refused, the refusal never echoes the offered token, and
/// the connection closes.
///
/// RED-MUTATION: delete the `hello.check_token` arm and this reds - which is the state
/// where any process that can reach the port can drive a shaker.
#[tokio::test]
async fn a_bad_or_absent_token_is_refused_without_echoing_it() {
    let h = start("").await;

    for (label, hello) in [
        (
            "wrong",
            Hello::new(PeerRole::Ui, "t")
                .asking(RunRole::Controller)
                .with_token("not-the-token"),
        ),
        (
            "absent",
            Hello::new(PeerRole::Ui, "t").asking(RunRole::Controller),
        ),
    ] {
        let mut ws = connect(&h).await;
        let text = say_hello(&mut ws, &hello)
            .await
            .unwrap_or_else(|| panic!("{label}: a refusal must arrive"));
        assert!(
            matches!(refusal(&text), ProtocolError::Unauthorized { .. }),
            "{label}: {text}"
        );
        assert!(
            !text.contains("not-the-token"),
            "{label}: the refusal echoed the offered token: {text}"
        );
        assert!(
            next_text(&mut ws).await.is_none(),
            "{label}: the connection must close"
        );
    }

    // ANTI-VACUITY: the right token is admitted.
    let mut ok = connect(&h).await;
    assert!(say_hello(&mut ok, &controller()).await.is_some());
}

// =====================================================================================
// GATE 3: the run lease
// =====================================================================================

/// THE ONE-CONTROLLER RULE over the wire: a second Controller is REFUSED by name, and
/// is DOWNGRADED only when it asked to be.
///
/// RED-MUTATION: make the server ignore `Admission::RefusedControllerHeld` and admit
/// the second peer as a Controller anyway; this reds - which is the state where two
/// operators drive one shaker.
#[tokio::test]
async fn a_second_controller_is_refused_and_downgraded_only_on_request() {
    let h = start("").await;

    let mut first = connect(&h).await;
    let ack: HelloAck =
        serde_json::from_str(&say_hello(&mut first, &controller()).await.unwrap()).expect("an ack");
    assert!(matches!(ack.lease, LeaseAnswer::Controller { .. }));
    assert_eq!(ack.role(), RunRole::Controller);
    assert_eq!(ack.v, PROTOCOL_VERSION);
    // The daemon reports its OWN build stamp so a bug report names both halves.
    assert!(!ack.build.is_empty());

    // A second Controller that did NOT opt in: refused, naming the holder.
    let mut second = connect(&h).await;
    let text = say_hello(&mut second, &controller())
        .await
        .expect("a refusal");
    match refusal(&text) {
        ProtocolError::NotController { holder } => {
            assert!(
                holder.starts_with("127.0.0.1:"),
                "names the holder: {holder}"
            )
        }
        other => panic!("expected NotController, got {other:?}"),
    }
    assert!(next_text(&mut second).await.is_none(), "and it closes");

    // A third that DID opt in: admitted as an Observer, TOLD it was downgraded.
    let mut third = connect(&h).await;
    let ack: HelloAck = serde_json::from_str(
        &say_hello(&mut third, &controller().accepting_observer_downgrade())
            .await
            .unwrap(),
    )
    .expect("an ack");
    assert_eq!(ack.role(), RunRole::Observer);
    assert!(ack.lease.was_downgraded(), "the downgrade must be visible");
    match ack.lease {
        LeaseAnswer::Observer { reason, controller } => {
            assert_eq!(reason, ObserverReason::Downgraded);
            assert!(controller.is_some(), "and it names who has it");
        }
        other => panic!("{other:?}"),
    }
}

/// AN OBSERVER IS READ-ONLY, over the wire, on every mutating verb - and its refusals
/// are TYPED (`NotController`), not generic.
///
/// RED-MUTATION: delete the `env.body.is_mutating()` block in `serve_one` and this
/// reds - which is the state where any watcher on the lab LAN can stop or reconfigure
/// someone else's run.
#[tokio::test]
async fn an_observer_is_refused_every_mutating_command_and_served_every_read_only_one() {
    let h = start("").await;

    // A Controller exists, so the refusals can name a holder.
    let mut ctrl = connect(&h).await;
    say_hello(&mut ctrl, &controller()).await.unwrap();

    let mut obs = connect(&h).await;
    let ack: HelloAck =
        serde_json::from_str(&say_hello(&mut obs, &observer()).await.unwrap()).unwrap();
    assert_eq!(ack.role(), RunRole::Observer);
    assert!(
        !ack.lease.was_downgraded(),
        "asking to observe is not a downgrade"
    );

    let mutating = [
        Command::Recorder(RecorderCommand::StartMonitor),
        Command::Recorder(RecorderCommand::Arm),
        Command::Recorder(RecorderCommand::RecordNow),
        Command::Recorder(RecorderCommand::StopMonitor),
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, "{}"),
        }),
        Command::Abort,
        Command::Stop,
        Command::PutChannelTable {
            table: eigen_protocol::TableDoc::new("", "{}"),
        },
        Command::SelectBackend {
            backend: eigen_protocol::BackendId::new("ni"),
        },
    ];
    for (i, c) in mutating.into_iter().enumerate() {
        match cmd(&mut obs, 100 + i as u64, c.clone()).await {
            Response::Rejected {
                error: ProtocolError::NotController { holder },
            } => assert!(!holder.is_empty(), "{c:?}: the holder must be named"),
            other => panic!("{c:?} must be refused to an observer, got {other:?}"),
        }
    }

    // ... and every read-only verb is SERVED, which is what makes it an observer
    // rather than a disconnected socket.
    assert!(matches!(
        cmd(&mut obs, 200, Command::GetStatus).await,
        Response::Status(_)
    ));
    assert!(matches!(
        cmd(&mut obs, 201, Command::ListBackends).await,
        Response::Backends(_)
    ));
    assert!(matches!(
        cmd(&mut obs, 202, Command::GetChannelTable).await,
        Response::ChannelTable(_)
    ));
    assert!(matches!(
        cmd(&mut obs, 203, Command::ListCaptures).await,
        Response::Captures(_)
    ));
    assert!(matches!(
        cmd(&mut obs, 204, Command::Recorder(RecorderCommand::Status)).await,
        Response::Recorder(_)
    ));
    // Subscribing is read-only too - an observer that could not subscribe could not
    // observe.
    assert!(matches!(
        cmd(
            &mut obs,
            205,
            Command::Subscribe {
                subs: vec![StreamSub::all(StreamKind::Spectrum)]
            }
        )
        .await,
        Response::Accepted { .. }
    ));
}

// =====================================================================================
// The LIST verb, the table, and the local-only honesty verb
// =====================================================================================

/// The LIST verb answers over the wire, including on an empty data root - a fresh
/// install must not look like a failure.
#[tokio::test]
async fn the_capture_list_verb_answers_over_the_wire() {
    let h = start("").await;

    // A capture on disk, with the fields that make it evidence.
    let dir = h.root.join("rec_0001");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("telemetry.f32"), vec![0u8; 4 * 100 * 4]).unwrap();
    std::fs::write(
        dir.join("capture.json"),
        serde_json::json!({
            "config": {"n_channels": 4, "sample_rate_hz": 51200.0, "decimation": 1, "ring_capacity": 8},
            "channel_table": {"schema_version": 2, "channels": [], "reference": null},
            "start_unix_secs": 1_753_400_000u64,
            "run_config_json": "{\"stationLabel\":\"EIGEN-SIM\",\"runId\":\"3\"}",
            "chunks_written": 1, "chunks_dropped": 0, "frames_written": 100, "complete": true
        })
        .to_string(),
    )
    .unwrap();

    let mut ws = connect(&h).await;
    say_hello(&mut ws, &observer()).await.unwrap();
    match cmd(&mut ws, 1, Command::ListCaptures).await {
        Response::Captures(list) => {
            assert_eq!(list.len(), 1, "{list:?}");
            assert_eq!(list[0].name, "rec_0001");
            assert_eq!(list[0].frames, 100);
            assert!(list[0].complete);
            assert_eq!(list[0].station_label, "EIGEN-SIM");
            assert_eq!(list[0].run_id, "3");
        }
        other => panic!("{other:?}"),
    }
}

/// The channel table round-trips over the wire and the ETAG gate holds across two
/// separate connections - which is the case it exists for.
#[tokio::test]
async fn the_channel_table_etag_gate_holds_across_two_connections() {
    let h = start("").await;

    let mut a = connect(&h).await;
    say_hello(&mut a, &controller()).await.unwrap();
    let first = match cmd(&mut a, 1, Command::GetChannelTable).await {
        Response::ChannelTable(t) => t,
        other => panic!("{other:?}"),
    };

    // A's edit lands.
    let mut edited = eigen_channel_table::ChannelTable::from_json(&first.json).unwrap();
    edited.channels[0].description = "control".into();
    let stored = match cmd(
        &mut a,
        2,
        Command::PutChannelTable {
            table: eigen_protocol::TableDoc::new(first.etag.clone(), edited.to_json().unwrap()),
        },
    )
    .await
    {
        Response::ChannelTable(t) => t,
        other => panic!("{other:?}"),
    };
    assert_ne!(stored.etag, first.etag);

    // A SECOND controller (after A leaves) holding the ORIGINAL etag is refused.
    drop(a);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut b = connect(&h).await;
    say_hello(&mut b, &controller()).await.unwrap();
    let mut theirs = eigen_channel_table::ChannelTable::from_json(&first.json).unwrap();
    theirs.channels[1].description = "monitor".into();
    match cmd(
        &mut b,
        3,
        Command::PutChannelTable {
            table: eigen_protocol::TableDoc::new(first.etag.clone(), theirs.to_json().unwrap()),
        },
    )
    .await
    {
        Response::Rejected {
            error:
                ProtocolError::StaleUpload {
                    current_etag,
                    provided_etag,
                },
        } => {
            assert_eq!(current_etag, stored.etag);
            assert_eq!(provided_etag, first.etag);
        }
        other => panic!("expected StaleUpload, got {other:?}"),
    }
    // A's edit survived.
    match cmd(&mut b, 4, Command::GetChannelTable).await {
        Response::ChannelTable(t) => {
            let table = eigen_channel_table::ChannelTable::from_json(&t.json).unwrap();
            assert_eq!(table.channels[0].description, "control");
        }
        other => panic!("{other:?}"),
    }
}

/// THE LOCAL-ONLY HONESTY VERB over the wire: an uncovered module is a NAMED refusal,
/// never an acceptance. This is what a remote UI renders as "local-only in remote
/// mode" instead of silently falling back.
///
/// `Command::Start` LEFT this test when the Random host landed: the Random module is
/// covered now, so an unconfigured Start answers an ACTIONABLE `Refused` ("send
/// SetProfile first") - a different honesty verb for a different fact.
///
/// The FREQUENCY intent left it at v4: it is routed through `Core::live_module` now
/// and answers a `Refused` that describes the core's STATE ("no frequency-bearing
/// module is live on this core"), because the same command becomes servable the moment
/// a frequency-bearing module is live.
///
/// **The SWEEP verbs left it** when the Explorer host landed: they route the same
/// way and answer the same shape of refusal. ~~`LocalOnly` naming the chassis gate.~~
/// The remaining LocalOnly row is the delegated open-loop sine profile, which really is
/// local-only - it drives a delegated controller from the desktop app.
#[tokio::test]
async fn an_uncovered_module_is_a_named_local_only_refusal_over_the_wire() {
    let h = start("").await;
    let mut ws = connect(&h).await;
    say_hello(&mut ws, &controller()).await.unwrap();

    match cmd(&mut ws, 1, Command::Start).await {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => assert!(message.contains("SetProfile"), "{message}"),
        other => panic!("expected an actionable Refused, got {other:?}"),
    }
    match cmd(&mut ws, 2, Command::GoToFrequency { hz: 500.0 }).await {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => assert!(
            message.contains("no frequency-bearing module is live on this core"),
            "{message}"
        ),
        other => panic!("expected the routed not-live Refused, got {other:?}"),
    }
    match cmd(&mut ws, 3, Command::Explorer(ExplorerCommand::StopSweep)).await {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => assert!(
            message.contains("no sweep-bearing module is live on this core"),
            "{message}"
        ),
        other => panic!("expected the routed not-live Refused, got {other:?}"),
    }
    // Still LocalOnly, and still naming what: the delegated open-loop path has no daemon.
    match cmd(
        &mut ws,
        4,
        Command::SetProfile {
            profile: eigen_protocol::TestProfile::OpenLoopSine(eigen_protocol::SineProfile {
                schema_version: eigen_protocol::PROFILE_SCHEMA,
                start_hz: 100.0,
                band_hz: None,
                max_drive_v: None,
            }),
        },
    )
    .await
    {
        Response::Rejected {
            error: ProtocolError::LocalOnly { what },
        } => assert!(!what.is_empty(), "refused without naming what"),
        other => panic!("expected LocalOnly, got {other:?}"),
    }
    // v5 BUNDLE: a dB LEVEL intent is routed too, and its refusal names a DIFFERENT
    // module family from the frequency one above - Random, not the Explorer. Pinned
    // over the real socket because the tail is the sentence an operator reads, and the
    // constant it replaced said "Explorer" for every intent including this one, which
    // pointed at the one module that has no dB ladder.
    match cmd(
        &mut ws,
        5,
        Command::SetLevel {
            level: eigen_protocol::LevelTarget::Db(-6.0),
        },
    )
    .await
    {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => {
            assert!(
                message.contains("no level-bearing module is live on this core"),
                "{message}"
            );
            // Stay-true tail (cross-review 2026-08-08): the closed-loop FAMILY +
            // the capability listing, never one module by name.
            assert!(
                message.contains("closed-loop modules") && message.contains("DescribeCapabilities"),
                "{message}"
            );
            assert!(!message.contains("Explorer profile"), "{message}");
        }
        other => panic!("expected the routed not-live Refused, got {other:?}"),
    }
    // v5 BUNDLE: the new wire surfaces decode and are refused by a NAMED missing host,
    // over the real socket. This is the row that proves the batched bump is safe to
    // deploy ahead of the hosts - the shape crosses, and the answer is honest.
    //
    // **The SINE row moved** when its host landed: a sine verb with nothing live is
    // now the ROUTED not-live refusal (the same shape the frequency and sweep rows
    // above answer), not a missing-host one. Asserting the OLD text here would have
    // pinned the pre-host posture over the real socket and gone on passing against a
    // daemon that had stopped implementing it - the failure mode the Explorer row hit
    // and this comment exists to stop repeating.
    match cmd(&mut ws, 6, Command::Sine(eigen_protocol::SineCommand::Hold)).await {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => {
            assert!(message.contains("is live on this core"), "{message}");
            assert!(
                !message.contains("MISSING HOST"),
                "the sine host is not missing any more: {message}"
            );
        }
        other => panic!("expected the routed not-live Refused, got {other:?}"),
    }
    // ... and SHOCK is served now too (their hosts landed in the same merge window),
    // so the second posture this row used to pin - "still a missing host" - is
    // retired the same way sine's was: an idle-core FirePulse gets the SERVED
    // not-configured refusal, and the MISSING HOST sentence must be GONE from it.
    // The missing-host answer itself lives on for the next module family
    // (Insight is inbound); nothing on this core produces it today.
    match cmd(
        &mut ws,
        8,
        Command::Shock(eigen_protocol::ShockCommand::FirePulse),
    )
    .await
    {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => {
            assert!(
                message.contains("not configured") && message.contains("ShockClassical"),
                "an idle-core FirePulse must be the served not-configured refusal: {message}"
            );
            assert!(
                !message.contains("MISSING HOST"),
                "the shock host is not missing any more: {message}"
            );
        }
        other => panic!("expected the served not-configured Refused, got {other:?}"),
    }
    // ... and the remote-capture refusal, which is a DIFFERENT answer that does not
    // expire when the shock host lands. The wire probe asserts this string too.
    match cmd(
        &mut ws,
        7,
        Command::Shock(eigen_protocol::ShockCommand::StartCapture),
    )
    .await
    {
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } => {
            assert!(
                message.starts_with("remote capture is not served over this seam"),
                "{message}"
            );
            assert!(!message.contains("MISSING HOST"), "{message}");
        }
        other => panic!("expected the remote-capture Refused, got {other:?}"),
    }
}

/// THE WHOLE RANDOM RUN OVER THE WIRE: a remote controller sends the profile,
/// starts, receives spectrum frames from the RANDOM pump on the shared bus, reads a
/// status that says Running with a level, and stops. On the Sim station, so it runs
/// hardware-free in CI - the NI half of the claim is the bench probe's job.
#[tokio::test]
async fn a_remote_controller_runs_random_end_to_end() {
    let h = start("").await;
    let mut ws = connect(&h).await;
    say_hello(&mut ws, &controller()).await.unwrap();

    // Subscribe the spectrum stream FIRST, so the run's frames flow to this peer.
    match cmd(
        &mut ws,
        1,
        Command::Subscribe {
            subs: vec![eigen_protocol::StreamSub::all(
                eigen_protocol::StreamKind::Spectrum,
            )],
        },
    )
    .await
    {
        Response::Accepted { .. } => {}
        other => panic!("subscribe: {other:?}"),
    }

    let profile = eigen_protocol::TestProfile::Random(eigen_protocol::RandomProfile {
        frf_pairs: None,
        feasibility: None,
        schema_version: eigen_protocol::PROFILE_SCHEMA,
        limits: None,
        control_weights: None,
        composite_mode: None,
        breakpoints: vec![
            eigen_protocol::PsdBreakpoint {
                freq_hz: 20.0,
                level: eigen_protocol::PsdLevel::Psd(0.01),
                alarm_db: None,
                abort_db: None,
            },
            eigen_protocol::PsdBreakpoint {
                freq_hz: 2000.0,
                level: eigen_protocol::PsdLevel::Psd(0.01),
                alarm_db: None,
                abort_db: None,
            },
        ],
        schedule: vec![
            eigen_protocol::ScheduleEntry {
                db: -12.0,
                dwell_s: Some(0.5),
            },
            eigen_protocol::ScheduleEntry {
                db: 0.0,
                dwell_s: Some(0.5),
            },
        ],
        ladder: eigen_protocol::LadderMode::Auto,
        duration_s: Some(2.0),
        acquisition: None,
        // This row drives a run end to end, so it opts out of the post-sysID review
        // hold; the hold has its own coverage in the service acceptance suite.
        control: Some(eigen_protocol::ControlProps {
            review_after_sysid: Some(false),
            ..Default::default()
        }),
    });
    match cmd(&mut ws, 2, Command::SetProfile { profile }).await {
        Response::Accepted { .. } => {}
        other => panic!("set_profile: {other:?}"),
    }
    match cmd(&mut ws, 3, Command::Start).await {
        Response::Accepted { .. } => {}
        other => panic!("start: {other:?}"),
    }
    // The pretest is ASYNC since 2026-08-03: `Start` answers ACCEPTED as soon
    // as the run is ARMED, and sysID -> gate -> engage happen on the run
    // thread (their duration scales with the acquisition settings and must not
    // sit inside the front end's 60 s command window). Give the priming a
    // bounded moment before asserting on a DRIVING status.
    for _ in 0..200 {
        if let Response::Status(st) = cmd(&mut ws, 30, Command::GetStatus).await {
            if !matches!(st.run, eigen_protocol::RunState::Idle) && st.level_db.is_some() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Status reflects a real run. The unpaced Sim counts dwell in SAMPLE time, so
    // the whole 3 s schedule can complete in milliseconds of wall clock - a poll
    // that lands after the finish sees Stopped WITH a measured composite, which is
    // a run that happened, not a run that failed to start. (First version of this
    // row demanded Running and learned the pacing fact from the red.)
    match cmd(&mut ws, 4, Command::GetStatus).await {
        Response::Status(st) => {
            assert!(st.level_db.is_some(), "{st:?}");
            match st.run {
                eigen_protocol::RunState::Running | eigen_protocol::RunState::Armed => {}
                eigen_protocol::RunState::Stopped => {
                    assert!(
                        st.measured_grms.is_some(),
                        "Stopped without ever measuring is a run that never ran: {st:?}"
                    );
                }
                other => panic!("unexpected run state {other:?}: {st:?}"),
            }
        }
        other => panic!("status: {other:?}"),
    }

    // A BINARY spectrum frame arrives from the Random pump - the same bus, the same
    // packer, the same decoder path as the recorder's stream.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut got_frame = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                assert!(b.len() > 16, "a packed frame has a header");
                got_frame = true;
                break;
            }
            Ok(Some(Ok(_))) => continue, // text (events, pings) - keep draining
            _ => break,
        }
    }
    assert!(got_frame, "no spectrum frame arrived from the Random pump");

    // Stop: accepted while the run is on; "nothing is running" when the unpaced Sim
    // already finished the schedule on its own. Both are the truth of the moment.
    match cmd(&mut ws, 5, Command::Stop).await {
        Response::Accepted { .. } => {}
        Response::Rejected {
            error: ProtocolError::Refused { message },
        } if message.contains("nothing is running") => {}
        other => panic!("stop: {other:?}"),
    }
}

/// A MALFORMED command carries the DECODER'S message back, which is the unknown-field
/// diagnostic reaching the operator - and the connection SURVIVES, because one bad
/// message is not a reason to drop a run's UI.
#[tokio::test]
async fn a_malformed_command_is_diagnosed_and_the_connection_survives() {
    let h = start("").await;
    let mut ws = connect(&h).await;
    say_hello(&mut ws, &controller()).await.unwrap();

    ws.send(Message::Text(
        format!(r#"{{"v":{PROTOCOL_VERSION},"id":1,"body":{{"set_profile":{{"profile":{{"open_loop_sine":{{"start_hz":100.0,"band_hz":null,"max_drive":1.0}}}}}}}}}}"#),
    ))
    .await
    .unwrap();
    let text = next_text(&mut ws).await.expect("a diagnosis");
    match refusal(&text) {
        ProtocolError::MalformedPayload { container, detail } => {
            assert_eq!(container, "command");
            assert!(
                detail.contains("max_drive"),
                "the unknown field must be named: {detail}"
            );
        }
        other => panic!("expected MalformedPayload, got {other:?}"),
    }
    // The connection is still usable.
    assert!(matches!(
        cmd(&mut ws, 9, Command::GetStatus).await,
        Response::Status(_)
    ));
}

// =====================================================================================
// The heartbeat / controller-timeout path
// =====================================================================================

/// THE CLIENT-VANISH RULE, end to end: the Controller disappears, the RUN CONTINUES,
/// and only when the controller-timeout expires does the daemon SAFE-STOP.
///
/// The timeout is 0.5 s here rather than the shipped 30 s, because a test that waits
/// half a minute is a test nobody runs - the DURATION is config, the BEHAVIOR is what
/// this asserts.
///
/// RED-MUTATION: make `on_disconnect` stop the run directly (rather than arm a clock)
/// and the mid-window assertion reds; delete the `expiry_loop`'s `safe_stop` call and
/// the post-window assertion reds. Both were run.
#[tokio::test]
async fn a_vanished_controller_leaves_the_run_up_then_the_timeout_safe_stops_it() {
    let h = start("heartbeat_s = 0.1\ncontroller_timeout_s = 0.5\n").await;

    let mut ctrl = connect(&h).await;
    say_hello(&mut ctrl, &controller()).await.unwrap();

    // Configure + start a real run on the EIGEN-SIM station.
    let cfg = eigen_recorder::service::RecorderConfig::default();
    assert!(
        matches!(
            cmd(
                &mut ctrl,
                1,
                Command::Recorder(RecorderCommand::Configure {
                    config: Doc::new(
                        SchemaKind::RecorderConfig,
                        serde_json::to_string(&cfg).unwrap()
                    ),
                })
            )
            .await,
            Response::Accepted { .. }
        ),
        "the recorder must configure on the Sim station"
    );
    assert!(matches!(
        cmd(
            &mut ctrl,
            2,
            Command::Recorder(RecorderCommand::StartMonitor)
        )
        .await,
        Response::Accepted { .. }
    ));
    assert!(h.server.core.recorder.is_live(), "the run is up");

    // An OBSERVER stays attached, and watches the whole thing.
    let mut obs = connect(&h).await;
    say_hello(&mut obs, &observer()).await.unwrap();

    // THE CONTROLLER VANISHES.
    drop(ctrl);

    // MID-WINDOW: the run is STILL UP. This is the assertion that makes the rule
    // real - a hiccup on the operator's laptop must not kill a running test. The
    // observer is SERVICED throughout (a real client has a read loop), so its own
    // liveness is never in question while we watch the controller's clock run.
    idle(&mut obs, Duration::from_millis(200)).await;
    assert!(
        h.server.core.recorder.is_live(),
        "the run must CONTINUE while the controller-timeout clock runs"
    );

    // PAST THE WINDOW: the safe-stop fires.
    idle(&mut obs, Duration::from_millis(900)).await;
    assert!(
        !h.server.core.recorder.is_live(),
        "the controller timeout must have brought the run down"
    );

    // The observer's connection is still alive and still answering - the safe-stop
    // ended the RUN, not the sessions watching it.
    assert!(matches!(
        cmd(&mut obs, 3, Command::GetStatus).await,
        Response::Status(_)
    ));
}

/// A CONTROLLER THAT STAYS is not stopped. Without this half, the test above would
/// pass just as well if the daemon stopped every run after half a second.
/// The peer that STARTS a run receives its RunProvenance event - not only
/// mid-run attachers via the at-connect push. Until 2026-07-30 the dispatch
/// path never emitted it and the probe's old substring assertion hid the gap.
/// The runId must be a JSON STRING on the wire (the 2^53 precision rule).
#[tokio::test]
async fn the_starting_controller_receives_run_provenance() {
    let h = start("").await;
    let mut ctrl = connect(&h).await;
    say_hello(&mut ctrl, &controller()).await.unwrap();

    let cfg = eigen_recorder::service::RecorderConfig::default();
    cmd(
        &mut ctrl,
        1,
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(
                SchemaKind::RecorderConfig,
                serde_json::to_string(&cfg).unwrap(),
            ),
        }),
    )
    .await;
    cmd(
        &mut ctrl,
        2,
        Command::Recorder(RecorderCommand::StartMonitor),
    )
    .await;

    // The event follows the StartMonitor response on the same text stream.
    let mut prov: Option<String> = None;
    for _ in 0..20 {
        let Some(t) = next_text(&mut ctrl).await else {
            break;
        };
        if t.contains("run_provenance") {
            prov = Some(t);
            break;
        }
    }
    let raw = prov.expect("the starting controller must receive run_provenance");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => {
                m.get(key).or_else(|| m.values().find_map(|x| find(x, key)))
            }
            serde_json::Value::Array(a) => a.iter().find_map(|x| find(x, key)),
            _ => None,
        }
    }
    let doc_json = find(&v, "json").and_then(|j| j.as_str()).expect("doc json");
    let p: serde_json::Value = serde_json::from_str(doc_json).unwrap();
    assert!(
        p.get("runId").map(|r| r.is_string()).unwrap_or(false),
        "runId must be a STRING on the wire, got {:?}",
        p.get("runId")
    );
    assert!(!p["stationLabel"].as_str().unwrap_or("").is_empty());

    cmd(
        &mut ctrl,
        9,
        Command::Recorder(RecorderCommand::StopMonitor),
    )
    .await;
}

#[tokio::test]
async fn a_live_controller_is_never_timed_out() {
    let h = start("heartbeat_s = 0.1\ncontroller_timeout_s = 0.3\n").await;
    let mut ctrl = connect(&h).await;
    say_hello(&mut ctrl, &controller()).await.unwrap();

    let cfg = eigen_recorder::service::RecorderConfig::default();
    cmd(
        &mut ctrl,
        1,
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(
                SchemaKind::RecorderConfig,
                serde_json::to_string(&cfg).unwrap(),
            ),
        }),
    )
    .await;
    cmd(
        &mut ctrl,
        2,
        Command::Recorder(RecorderCommand::StartMonitor),
    )
    .await;
    assert!(h.server.core.recorder.is_live());

    // Well past three timeouts, with the connection SERVICED (so it answers pings)
    // and demonstrably still answering commands.
    for i in 0..6 {
        idle(&mut ctrl, Duration::from_millis(150)).await;
        assert!(matches!(
            cmd(&mut ctrl, 10 + i, Command::ListCaptures).await,
            Response::Captures(_)
        ));
    }
    assert!(
        h.server.core.recorder.is_live(),
        "a LIVE controller must never be timed out"
    );
    cmd(
        &mut ctrl,
        99,
        Command::Recorder(RecorderCommand::StopMonitor),
    )
    .await;
}

// =====================================================================================
// The display stream + selectors
// =====================================================================================

/// The SELECTOR path end to end: a subscription that names two channels receives
/// BINARY frames for those channels only, decimated, with the observability block
/// stamped.
#[tokio::test]
async fn a_selected_subscription_receives_only_the_channels_it_asked_for() {
    let h = start("").await;
    let mut ws = connect(&h).await;
    say_hello(&mut ws, &controller()).await.unwrap();

    cmd(
        &mut ws,
        1,
        Command::Subscribe {
            subs: vec![StreamSub {
                kind: StreamKind::Spectrum,
                channels: eigen_protocol::ChannelSel::Ids(vec![1, 2]),
                max_rate_hz: None,
                bin_decimation: Some(4),
            }],
        },
    )
    .await;

    let cfg = eigen_recorder::service::RecorderConfig::default();
    cmd(
        &mut ws,
        2,
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(
                SchemaKind::RecorderConfig,
                serde_json::to_string(&cfg).unwrap(),
            ),
        }),
    )
    .await;
    cmd(&mut ws, 3, Command::Recorder(RecorderCommand::StartMonitor)).await;

    // Collect binary frames for a moment.
    let mut seen: Vec<u32> = Vec::new();
    let mut last_seq = 0u32;
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while seen.len() < 4 && std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                assert!(b.len() >= 44, "a spectrum frame carries the 44-byte header");
                let ch = u32::from_le_bytes(b[0..4].try_into().unwrap());
                let seq = u32::from_le_bytes(b[24..28].try_into().unwrap());
                // The OBS seq counts frames on THIS stream to THIS peer, so it
                // advances by exactly one per frame - which is what lets a decoder
                // detect a lost frame at all.
                assert_eq!(seq, last_seq + 1, "the obs seq must not skip");
                last_seq = seq;
                seen.push(ch);
            }
            Ok(Some(Ok(_))) => {}
            _ => {}
        }
    }
    cmd(&mut ws, 9, Command::Recorder(RecorderCommand::StopMonitor)).await;

    assert!(!seen.is_empty(), "the display stream produced no frames");
    for ch in &seen {
        assert!(
            *ch == 1 || *ch == 2,
            "channel {ch} was NOT subscribed; the selector is not being honored at \
             the producer"
        );
    }
}
