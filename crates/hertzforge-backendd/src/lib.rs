//! `hertzforge-backendd` - the headless CORE of the split deployment.
//!
//! # What it is
//!
//! A daemon that runs ON the hardware host (the Linux box with the cDAQ) and serves a
//! remote front end over `hertzforge-protocol` v3 on a WebSocket. The operator's Mac renders
//! the UI; this process owns the hardware.
//!
//! # THE CONTROL LOOP NEVER CROSSES THE NETWORK
//!
//! This is the rule the whole design is arranged around, and it is not a performance
//! preference. Closing a servo loop across a link puts unbounded, jittery transport
//! delay INSIDE the loop, which destroys stability and blows the abort-latency budget
//! that gates `arm()`. So the engine, the watchdog and the drive clock are all in THIS
//! process, co-resident with the hardware. What crosses the wire is the decimated
//! display path down and operator intent up.
//!
//! **A remote Stop or Abort is SUPERVISORY.** It is honored, and it is never the
//! primary abort authority - that is the device's own limits (delegated loop) or the
//! host watchdog co-resident with the drive clock (host loop), backed by the physical
//! E-stop. No remote UI and no transport is in the armature-safety chain, and the UI
//! is required to say so.
//!
//! # The hardening, and where each piece lives
//!
//! | Rule | Here |
//! |---|---|
//! | Bind an EXPLICIT interface, never `0.0.0.0` | [`config`] - a wildcard is refused at start |
//! | Shared token at Hello | [`config`] + `hertzforge_protocol::Hello::check_token` |
//! | Exactly ONE Controller, N Observers | [`lease`] - and a second Controller is REFUSED unless it opted into the downgrade |
//! | Client vanish -> the run CONTINUES | [`lease`] - the disconnect arms a clock, it does not stop anything |
//! | Heartbeat + controller timeout -> safe-stop | [`server`] (ping/pong + the expiry loop) -> [`core::Core::safe_stop`], which routes to the LIVE module's controlled stop |
//! | No orphaned DAQmx tasks at start | [`preflight`] - an attempt-RESERVE, and an honest account of why a task census is not possible |
//! | Mutual exclusion vs the desktop app | [`lock`] - an advisory lockfile that NAMES its holder (it lives in `hertzforge-station` because the Tauri app READS it) |
//! | Channel table daemon-authoritative, versioned upload | [`table`] - etag concurrency, stale writes refused |
//! | Captures daemon-local + a LIST verb | [`captures`] |
//! | The 400-channel stream budget | the selectors, applied producer-side in [`server`] |
//!
//! # What it serves, and what it refuses
//!
//! **The Recorder, the Random module and the Explorer.** ~~The Recorder, and nothing
//! else.~~ **CORRECTED 2026-08-08 (byte beats doc):** that sentence described the plan's
//! arrival-day remote minimum and stopped being true when
//! [`random::RandomHost`] landed as the first DRIVE-PRODUCING module over the seam.
//! The Recorder was the right FIRST module precisely because it is monitor-only - it
//! drives nothing, so the remote surface was provable end to end without a remote peer
//! ever being able to command drive - but it has not been the only one for some time.
//! [`explorer::ExplorerHost`] joined them as the second drive-producing module and the
//! first OPEN-LOOP one: the operator commands volts and frequency, nothing corrects per
//! tick, and the arm posture is 0.000 V.
//!
//! **CORRECTED AGAIN 2026-08-08:** [`shock::ShockHost`] and [`srs::SrsHost`] make
//! five, and they are the first TRANSIENT producers over this seam - modules with no
//! continuous running state at all. They ARM, and then pulses are FIRED one verb at a
//! time. Almost everything in those two files that departs from the Explorer's rails is
//! that difference, and each departure is stated where it is made.
//!
//! Which of them owns the hardware at any moment is [`core::Core::live_module`]'s
//! answer, and it is the ONE authority: status, safe-stop, `Stop`/`Abort` dispatch,
//! the mid-run provenance push and the module mutual exclusion all consult it rather
//! than carrying their own pairwise chain.
//!
//! Everything else is `ProtocolError::LocalOnly` - a typed refusal a front end renders
//! as "local-only in remote mode". **There is never a fallback.** A front end that
//! quietly ran a module against its own local backend when the daemon had no answer
//! would put the run on different hardware than the operator selected, which is the
//! same class of defect as substituting the Sim for absent hardware.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod captures;
pub mod config;
pub mod core;
pub mod display;
pub mod explorer;
pub mod frames;
pub mod lease;
pub mod preflight;
/// The peak-level sweep profile raster MOVED to `hertzforge_explorer_host::profile_raster`
/// (2026-09-07) so the in-process delegated path can draw the same survey line;
/// re-exported here so `crate::profile_raster::ProfileRaster` keeps resolving.
pub use hertzforge_explorer_host::profile_raster;
pub mod provenance;
pub mod random;
pub mod recorder;
pub mod server;
pub mod shock;
pub mod sine;
pub mod soak;
pub mod srs;
pub mod table;
pub mod twr;

use std::sync::Arc;

/// The advisory lockfile, re-exported from `hertzforge-station`.
///
/// The daemon WRITES it and the Tauri desktop app READS it, so its definition lives in
/// the crate both already depend on rather than being duplicated - see
/// [`hertzforge_station::lock`] for the full rule and for why it is advisory.
pub use hertzforge_station::lock;

/// Build the whole daemon from a validated config: the table store, the recorder host,
/// the core, and the server state.
///
/// Split out from `main` so the integration tests build the SAME object the binary
/// does - a test that assembled its own core would be testing an arrangement nobody
/// ships.
pub fn build(cfg: config::DaemonConfig) -> Arc<server::Server> {
    let data_root = cfg.resolved_data_root();
    let table = Arc::new(table::TableStore::new(
        hertzforge_channel_table::ChannelTable::default_with_channels(4),
    ));
    // ONE display bus for the whole daemon: every module host publishes on it and a
    // connection subscribes once, so whichever module is live feeds the stream.
    let (frames, _) = tokio::sync::broadcast::channel(64);
    let recorder = Arc::new(recorder::RecorderHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        data_root.clone(),
        frames.clone(),
    ));
    let random = Arc::new(random::RandomHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        data_root.clone(),
        frames.clone(),
    ));
    // The Explorer host takes the CONTROLLER TIMEOUT because its safe-stop is a ramp
    // with a duration, and that duration has to be checked against the window the
    // daemon gives a vanished controller - see `ExplorerHost::set_profile`.
    let explorer = Arc::new(explorer::ExplorerHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        frames.clone(),
        cfg.controller_timeout_s,
    ));
    // The two TRANSIENT producers. `ShockHost` takes the controller timeout for the
    // same reason the Explorer does - its safe-stop has a duration that has to fit
    // inside the window a vanished controller gets - though the arithmetic is
    // different: there is no ramp, so the budget is the pulse in flight plus the task
    // stop, and a profile whose single pulse outlasts the whole timeout is refused with
    // both numbers. `SrsHost` needs no timeout: its safe-stop is the same
    // between-pulse cancel and its one long operation, the system ID, is bounded by
    // the profile's nfft rather than by an operator slew.
    let shock = Arc::new(shock::ShockHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        data_root.clone(),
        frames.clone(),
        cfg.controller_timeout_s,
    ));
    // Same reason as the Explorer's: Sine's safe-stop is a ramp with a computable
    // duration, and `SineHost::set_profile` refuses a profile whose ramp plus
    // generation drain would outlast the window a vanished controller gets.
    let sine = Arc::new(sine::SineHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        data_root.clone(),
        frames.clone(),
        cfg.controller_timeout_s,
    ));
    let srs = Arc::new(srs::SrsHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        data_root.clone(),
        frames.clone(),
    ));
    // TWR takes the controller timeout for a THIRD reason, distinct from the Explorer's
    // ramp and the Shock's pulse: its cancel latency is the device's COMMITTED REMAINDER
    // plus the HAL taper, which does not scale with the record. The host states that
    // arithmetic, checks the part that can be checked at configure, and journals the
    // OBSERVED commit depth against the window after every play - see
    // `TwrHost::set_profile`.
    let twr = Arc::new(twr::TwrHost::new(
        cfg.backend_kind(),
        cfg.ni_devices.clone(),
        frames.clone(),
        cfg.controller_timeout_s,
    ));
    let core = Arc::new(core::Core {
        recorder,
        random,
        explorer,
        shock,
        srs,
        sine,
        twr,
        table,
        frames,
        data_root,
        kind: cfg.backend_kind(),
        #[cfg(test)]
        probe: Default::default(),
    });
    server::Server::new(cfg, core)
}
