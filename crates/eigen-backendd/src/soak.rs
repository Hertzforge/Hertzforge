//! THE SOAK HARNESS ("Recorder on NI-SIM via the daemon, >= 30 min,
//! zero SchedulingFault, p99 tick <= 50% budget").
//!
//! # What it measures, and what the budget IS
//!
//! **The budget is the wall-clock time one pump segment REPRESENTS**, computed at
//! the rate the driver ACTUALLY programmed (the 9234 coerces a 65 536 Hz request
//! to 51 200 Hz; a request-derived budget is ~28% short on real hardware - the
//! first real soak failed on exactly that, 2026-07-30).
//!
//! **The gated statistic is the segment's LAG MARGIN, not raw elapsed.** On a
//! driver-paced station a segment TAKES its real-time span by construction (the
//! reads wait for the data), so raw elapsed can never satisfy a keeping-up gate;
//! the pump records `elapsed - frames/actual_rate` (clamped at zero), which is
//! the quantity the -200279 overrun failure mode actually punishes. The gate is
//! p99 lag <= 50% of the segment budget. On an UNPACED station (EIGEN-SIM) the
//! rate-locked pump sleeps to real time and the lag is the same honest number.
//! At 51 200 Hz / 16 384-frame segments the budget is **320.0 ms** and the gate
//! is **160.0 ms**; both are PRINTED at exit so the evidence row cites numbers a
//! reader can check rather than a verdict.
//!
//! **SchedulingFault count comes from the service**, via
//! `RecorderService::scheduling_faults()`. That accessor was added for exactly
//! this gate: `run_segment` swallows the fault by RETRYING it (a spurious
//! wall-clock overrun on a deterministic harness is not a real fault), and swallowed
//! and uncounted, the predicate was unmeasurable. A soak could have retried a hundred
//! segments and reported a clean run.
//!
//! # It runs THROUGH THE DAEMON, not beside it
//!
//! The harness starts the real server on a loopback socket, connects a real WebSocket
//! client, and drives the recorder with real protocol commands. A soak that called
//! `RecorderHost` directly would prove the recorder works and say nothing about the
//! daemon itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use eigen_protocol::{
    Command, CommandEnvelope, Doc, Envelope, Hello, PeerRole, RecorderCommand, RunRole, SchemaKind,
    StreamKind, StreamSub,
};

use crate::server::Server;

/// The soak's result, printed at exit.
#[derive(Debug, Clone, PartialEq)]
pub struct SoakReport {
    /// How long it ran.
    pub elapsed_s: f64,
    /// Pump segments completed.
    pub segments: u64,
    /// Frames pumped.
    pub frames: u64,
    /// SchedulingFaults the service counted (retried or not). **The gate is 0.**
    pub scheduling_faults: u64,
    /// p99 pump-segment LAG (wall time beyond the frames' real-time span at the
    /// driver's actual rate), seconds. Raw elapsed cannot gate a driver-paced
    /// station - a segment TAKES its real-time span there by construction.
    pub p99_s: Option<f64>,
    /// The per-segment budget: the wall time one segment REPRESENTS.
    pub budget_s: f64,
    /// Binary display frames the client actually received over the socket.
    pub frames_received: u64,
    /// The station the run was on, rendered.
    pub station_label: String,
}

impl SoakReport {
    /// The gate: p99 at or under HALF the budget.
    pub fn p99_gate_s(&self) -> f64 {
        self.budget_s * 0.5
    }

    /// Did it pass? Both halves, explicitly.
    pub fn passed(&self) -> bool {
        self.scheduling_faults == 0 && self.p99_s.is_some_and(|p| p <= self.p99_gate_s())
    }

    /// The exit text. Names every number so the ledger row cites evidence, not a
    /// verdict.
    pub fn render(&self) -> String {
        let p99 = match self.p99_s {
            Some(p) => format!("{:.1} ms", p * 1e3),
            // "No samples" is NOT a pass. A soak that pumped nothing has proven
            // nothing, and must never read as green.
            None => "NO SAMPLES (the pump never ran - this is not a pass)".into(),
        };
        format!(
            "SOAK REPORT\n\
             \x20 station          : {}\n\
             \x20 elapsed          : {:.1} s\n\
             \x20 segments         : {}\n\
             \x20 frames pumped    : {}\n\
             \x20 frames to client : {}\n\
             \x20 SchedulingFault  : {}  (gate: 0)\n\
             \x20 p99 segment LAG  : {}  (gate: <= {:.1} ms = 50% of the {:.1} ms actual-rate budget)\n\
             \x20 VERDICT          : {}",
            self.station_label,
            self.elapsed_s,
            self.segments,
            self.frames,
            self.frames_received,
            self.scheduling_faults,
            p99,
            self.p99_gate_s() * 1e3,
            self.budget_s * 1e3,
            if self.passed() { "PASS" } else { "FAIL" }
        )
    }
}

/// Run the soak for `duration`, through a real server on a loopback socket.
///
/// `server` must already be built; the caller binds it so the harness can be pointed
/// at whatever the config says (the box runs it against the configured station, which
/// is how "Recorder on NI-SIM via the daemon" is satisfied rather than asserted).
pub async fn run(
    server: Arc<Server>,
    duration: Duration,
) -> Result<SoakReport, Box<dyn std::error::Error + Send + Sync>> {
    let (listener, addr) = server.bind().await?;
    let token = server.cfg.token.clone();
    let serving = Arc::clone(&server);
    tokio::spawn(async move { serving.serve(listener).await });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/")).await?;

    // Handshake as the CONTROLLER - the soak drives.
    let hello = Hello::new(PeerRole::Ui, "eigen-backendd --soak")
        .asking(RunRole::Controller)
        .with_token(token);
    ws.send(Message::Text(serde_json::to_string(&hello)?))
        .await?;
    let _ack = next_text(&mut ws).await?;

    // Subscribe to the display path with a realistic operator selection, so the soak
    // exercises the SELECTOR path rather than the firehose.
    send(
        &mut ws,
        1,
        Command::Subscribe {
            subs: vec![StreamSub {
                kind: StreamKind::Spectrum,
                channels: eigen_protocol::ChannelSel::All,
                max_rate_hz: Some(10.0),
                bin_decimation: Some(2),
            }],
        },
    )
    .await?;
    let _ = next_text(&mut ws).await?;

    // Configure + start monitoring, over the wire.
    let cfg = eigen_recorder::service::RecorderConfig::default();
    // Provisional (request-derived); replaced with the ACTUAL driver rate after
    // the monitor is up - the 9234 coerces 65 536 -> 51 200 Hz and a budget
    // from the request is ~28% short on real hardware.
    let mut budget_s = 16_384.0 / cfg.acq.native_rate_hz;
    send(
        &mut ws,
        2,
        Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, serde_json::to_string(&cfg)?),
        }),
    )
    .await?;
    let configured = next_text(&mut ws).await?;
    if configured.contains("rejected") {
        return Err(format!("the soak could not configure the recorder: {configured}").into());
    }
    send(&mut ws, 3, Command::Recorder(RecorderCommand::StartMonitor)).await?;
    let started = next_text(&mut ws).await?;
    if started.contains("rejected") {
        return Err(format!("the soak could not start monitoring: {started}").into());
    }
    // The backend is up now: replace the request-derived budget with one from
    // the rate the driver actually programmed.
    if let Some(actual) = server.core.recorder.actual_rate_hz() {
        if actual > 0.0 {
            budget_s = 16_384.0 / actual;
        }
    }

    // Drain for the duration, counting the BINARY frames that actually arrived.
    let began = Instant::now();
    let mut frames_received = 0u64;
    while began.elapsed() < duration {
        match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
            Ok(Some(Ok(Message::Binary(_)))) => frames_received += 1,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(None) => break,
            Err(_) => {} // a quiet stretch is not a failure; keep going
        }
    }

    let m = &server.core.recorder.metrics;
    let report = SoakReport {
        elapsed_s: began.elapsed().as_secs_f64(),
        segments: m.segments.load(std::sync::atomic::Ordering::Relaxed),
        frames: m.frames.load(std::sync::atomic::Ordering::Relaxed),
        scheduling_faults: m
            .scheduling_faults
            .load(std::sync::atomic::Ordering::Relaxed),
        p99_s: m.p99_s(),
        budget_s,
        frames_received,
        station_label: server
            .core
            .recorder
            .provenance()
            .map(|p| p.station_label)
            .unwrap_or_else(|| "(no run)".into()),
    };

    // Leave the box clean: stop the run before the harness exits.
    let _ = send(&mut ws, 4, Command::Recorder(RecorderCommand::StopMonitor)).await;
    let _ = ws.close(None).await;
    Ok(report)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(
    ws: &mut Ws,
    id: u64,
    cmd: Command,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let env: CommandEnvelope = Envelope::new(id, cmd);
    ws.send(Message::Text(serde_json::to_string(&env)?)).await?;
    Ok(())
}

async fn next_text(ws: &mut Ws) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return Ok(t),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => return Err("the connection closed before answering".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(faults: u64, p99: Option<f64>) -> SoakReport {
        SoakReport {
            elapsed_s: 1800.0,
            segments: 7200,
            frames: 117_964_800,
            scheduling_faults: faults,
            p99_s: p99,
            budget_s: 0.25,
            frames_received: 18_000,
            station_label: "NI-SIM (driver-simulated)".into(),
        }
    }

    /// THE GATE, both halves. Either one fails the soak on its own.
    ///
    /// RED-MUTATION: drop the `scheduling_faults == 0` conjunct (the "they were
    /// retried, so they do not count" argument) and the first assertion reds - which
    /// is the state where a soak that retried its way through 40 faults is written up
    /// as clean.
    #[test]
    fn the_gate_is_zero_faults_and_p99_under_half_the_budget() {
        assert!(report(0, Some(0.100)).passed(), "0 faults, 100 ms < 125 ms");
        assert!(!report(1, Some(0.100)).passed(), "one fault fails it");
        assert!(!report(0, Some(0.200)).passed(), "200 ms > the 125 ms gate");
        // Exactly at the gate passes - the predicate is "<=", and a boundary that
        // silently fails would make the printed number wrong.
        assert!(report(0, Some(0.125)).passed());
        assert_eq!(report(0, None).p99_gate_s(), 0.125);
    }

    /// **NO SAMPLES IS NOT A PASS.** A soak whose pump never ran has proven nothing,
    /// and the report says so in those words rather than showing a blank.
    ///
    /// RED-MUTATION: make `passed()` treat `p99_s == None` as satisfying the latency
    /// half and this reds. The rule in one assertion: a result that cannot fail is not
    /// evidence.
    #[test]
    fn a_soak_that_never_pumped_is_a_failure_and_says_so() {
        let r = report(0, None);
        assert!(!r.passed(), "no samples must never read as a pass");
        let text = r.render();
        assert!(text.contains("NO SAMPLES"), "{text}");
        assert!(text.contains("not a pass"), "{text}");
        assert!(text.contains("VERDICT          : FAIL"), "{text}");
    }

    /// The report PRINTS both gate numbers and the station, so an evidence row cites
    /// something a reader can check instead of a verdict they must trust.
    #[test]
    fn the_report_prints_the_numbers_the_evidence_row_needs() {
        let text = report(0, Some(0.1)).render();
        for needle in [
            "NI-SIM (driver-simulated)",
            "SchedulingFault  : 0  (gate: 0)",
            "125.0 ms",
            "250.0 ms",
            "PASS",
        ] {
            assert!(text.contains(needle), "{needle} missing from:\n{text}");
        }
    }
}
