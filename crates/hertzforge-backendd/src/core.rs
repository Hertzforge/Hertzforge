//! The [`CoreApi`] implementation: what the daemon answers, and what it REFUSES.
//!
//! # The two rules that shape every arm of the match below
//!
//! 1. **Anything not served here is a TYPED refusal, never a fallback.**
//!    [`ProtocolError::LocalOnly`] is the honesty verb: a front end that received it
//!    renders "local-only in remote mode" and cannot mistake the state for a working
//!    module. The alternative - a front end that quietly ran the Sine module against
//!    its own local backend when the daemon had no answer - would put the run on
//!    different hardware than the operator selected, which is the same class of defect
//!    as a Sim fallback.
//! 2. **The run lease is checked by the SERVER, before dispatch.** It is a session
//!    fact, not a core fact: the core does not know which socket a command arrived on,
//!    and threading a connection id through `CoreApi` would put transport state in a
//!    transport-free trait. [`CoreSession`] is the join - it holds the connection's
//!    identity and delegates the actual work to the shared [`Core`].
//!
//! # ONE authority over which module is live ([`Core::live_module`])
//!
//! Five sites need to know which module owns the hardware right now: the status poll,
//! the safe-stop path, the `Stop`/`Abort` dispatch, the mid-run provenance push, and
//! the mutual exclusion at `SetProfile`/`Start`/`Recorder(Configure)`. Each of them
//! used to carry its own pairwise `if random ... else if recorder ...` chain, and the
//! copies had already drifted apart. [`Core::live_module`] is now the single answer;
//! every dispatch matches exhaustively on [`Module`] with no `_` arm, so a module that
//! has not been wired to all five does not compile.
//!
//! # Remote Stop / Abort are SUPERVISORY, and the response says so
//!
//! Per the `hertzforge-protocol` crate docs, a Stop or Abort over this seam is operator
//! convenience on the supervisory path - never the primary abort authority, which is
//! on-hardware and backed by the physical E-stop. The daemon honors them (they route to
//! the recorder's controlled stop) and the UI is required to label them; nothing here
//! pretends the network is in the safety chain.

use std::sync::Arc;

use hertzforge_protocol::{
    ActionKind, ActionSupport, BackendId, BackendInfo, BackendTier, CapabilityDescriptor,
    CaptureInfo, ClockDomain, Command, Completion, CoreApi, Doc, Event, EventSink, LinkState,
    LoopOwner, ProtocolError, RecorderCommand, Response, RunState, SchemaKind, StatusReport,
    StreamKind,
};
use hertzforge_station::BackendKind;

use crate::explorer::ExplorerHost;
use crate::lease::ConnId;
use crate::provenance::RunProvenance;
use crate::random::RandomHost;
use crate::recorder::{DisplayBatch, RecorderHost};
use crate::shock::ShockHost;
use crate::sine::SineHost;
use crate::srs::SrsHost;
use crate::table::TableStore;
use crate::twr::TwrHost;

/// Every module this daemon can host. THE closed set.
///
/// # Why this type exists
///
/// Before it, five sites each hand-threaded their own pairwise `if random ... else if
/// recorder ...` chain: [`Core::status`], [`Core::safe_stop`], the `Stop`/`Abort`
/// dispatch, the mid-run provenance push in [`crate::server`], and the
/// `set_profile`/`start` mutual exclusion (which threaded a `recorder_live: bool`
/// parameter down into [`RandomHost`]). Five copies of one rule is five chances to
/// disagree, and they already did: the exclusion was ONE-DIRECTIONAL (Random refused
/// to configure while the Recorder monitored, but nothing stopped a Recorder configure
/// while a Random run was driving), and the `Stop` chain would have answered "nothing
/// is running" for any module that was not one of the two it knew.
///
/// # The compile-time property, stated exactly
///
/// Every DISPATCH site matches exhaustively on this enum with no `_` arm,
/// so a new variant that has not been wired to stop / status / provenance / exclusion
/// **does not compile**. What the compiler cannot force is membership of
/// [`Module::ALL`] - a new variant left out of the priority order would still build -
/// so that one hole is closed by `the_priority_order_lists_every_module` in this
/// file's tests instead. Both failure modes are caught; only one of them is caught by
/// `rustc`, and saying otherwise would be the "a result that cannot fail" trap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    /// The Random module - the first DRIVE-PRODUCING module over the seam.
    Random,
    /// The Explorer - the host-synthesized open-loop survey. The second
    /// drive-producing module, and the first FREQUENCY-BEARING one: it is what
    /// `GoToFrequency`, `AdjustFrequency`, `SetLevel(DriveVolts)` and the sweep verbs
    /// route to.
    Explorer,
    /// The CLASSICAL SHOCK module - the first TRANSIENT producer over the seam. It has
    /// no continuous running state: it ARMS, and then pulses are FIRED one verb at a
    /// time.
    Shock,
    /// The SRS synthesis module - the second transient producer, and the one that
    /// carries the post-sysID REVIEW HOLD. A held run is LIVE.
    Srs,
    /// The CLOSED-LOOP SINE module. The third drive-producing module,
    /// and the second frequency-bearing one: it serves `GoToFrequency` /
    /// `AdjustFrequency` like the Explorer AND the four level-schedule verbs like
    /// Random, which is why it is the first module to appear in both routers.
    Sine,
    /// The TIME WAVEFORM REPLICATION module.
    /// The fourth drive-producing module, and the first whose UNIT is a whole RECORD:
    /// one `Start` arms and identifies the plant, and each iteration after it plays the
    /// drive, measures, and corrects between plays.
    Twr,
    /// The Data Recorder - monitor-only, the arrival-day remote minimum.
    Recorder,
    /// A TEST-ONLY third module, present only under `cfg(test)`.
    ///
    /// It is not a placeholder for the Explorer host and it ships in nothing. It
    /// exists so the red test can put a module the pairwise chains never knew about
    /// into the live state and prove that `Stop` DISPATCHES to it rather than
    /// answering "nothing is running on this core" - which is exactly what the old
    /// hand-threaded chain would have done, and what an operator would have read as
    /// "the run already ended".
    #[cfg(test)]
    Probe,
}

impl Module {
    /// The modules in PRIORITY order - the tie-break when more than one holds a
    /// claim, stated once here instead of re-deciding it at five call sites.
    ///
    /// The DRIVE-PRODUCING modules first: if one of them and the Recorder are both up,
    /// the one that can move an armature is the one a status poll, a stop, and a
    /// provenance push must be about. Random outranks the Explorer only because it was
    /// here first and one order has to be written down - the two are mutually excluded
    /// at `Claim::Live` anyway, so the tie-break between them is reachable only for the
    /// weaker `Claim::Configured` question (two built-but-idle stations), where it
    /// decides which one a bare `Start` addresses.
    pub const ALL: &'static [Module] = &[
        Module::Random,
        Module::Explorer,
        Module::Shock,
        Module::Srs,
        Module::Sine,
        // AFTER Sine and BEFORE the Recorder. The position is a RULE, not a
        // preference: every module that can move an armature outranks the monitor-only
        // one (see `the_priority_order_lists_every_module`), and among the
        // drive-producers the order is only reachable for the weaker
        // `Claim::Configured` question - which decides which of two built-but-idle
        // stations a bare `Start` addresses, and must not move under an operator who
        // configured the other one first.
        Module::Twr,
        Module::Recorder,
        #[cfg(test)]
        Module::Probe,
    ];

    /// The name a refusal, a log line, or a status card uses. Exhaustive: a module
    /// with no name would produce the generic refusal the whole `LocalOnly` doctrine
    /// exists to avoid.
    pub fn label(self) -> &'static str {
        match self {
            Module::Random => "Random module",
            Module::Explorer => "Explorer",
            Module::Shock => "classical Shock module",
            Module::Srs => "SRS module",
            Module::Sine => "Sine module",
            Module::Twr => "TWR module",
            Module::Recorder => "Recorder",
            #[cfg(test)]
            Module::Probe => "test probe module",
        }
    }

    /// What to send to get this module OUT of the way, named in the refusal so the
    /// operator is told the next command rather than just being told no.
    pub fn stand_down_hint(self) -> &'static str {
        match self {
            Module::Random => "Stop",
            Module::Explorer => "Stop",
            Module::Shock => "Stop",
            Module::Srs => "Stop",
            Module::Sine => "Stop",
            Module::Twr => "Stop",
            Module::Recorder => "Recorder(StopMonitor)",
            #[cfg(test)]
            Module::Probe => "Stop",
        }
    }
}

/// How strong a hold on the core a module needs in order to BE the answer.
///
/// Two strengths, because the five sites genuinely ask two different questions and
/// collapsing them would break one or the other. A configured-but-idle Random owns a
/// built DAQmx station (`set_profile` builds it) and its status is what a poll must
/// report - but it is not driving, so it is not what a `Stop` acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Driving, or about to drive. The question `Stop` / `Abort` / safe-stop / the
    /// mutual exclusion ask.
    Live,
    /// Holds a built station, live or not. The question a status poll and the
    /// provenance push ask.
    Configured,
}

/// Everything the daemon serves, shared by every connection.
pub struct Core {
    /// The Recorder - the v1 daemon-side module coverage.
    pub recorder: Arc<RecorderHost>,
    /// The Random module - the first DRIVE-PRODUCING module over the seam.
    pub random: Arc<RandomHost>,
    /// The Explorer - the host-synthesized open-loop survey.
    pub explorer: Arc<ExplorerHost>,
    /// The classical Shock module - the first TRANSIENT producer over the seam.
    pub shock: Arc<ShockHost>,
    /// The SRS synthesis module.
    pub srs: Arc<SrsHost>,
    /// The CLOSED-LOOP SINE module.
    pub sine: Arc<SineHost>,
    /// The TIME WAVEFORM REPLICATION module.
    pub twr: Arc<TwrHost>,
    /// The daemon-authoritative channel table.
    pub table: Arc<TableStore>,
    /// The ONE display bus every module host publishes on; a connection subscribes
    /// here once and receives whichever module is live.
    pub frames: tokio::sync::broadcast::Sender<DisplayBatch>,
    /// Where captures live.
    pub data_root: std::path::PathBuf,
    /// The station this daemon was configured to serve.
    pub kind: BackendKind,
    /// The TEST-ONLY third module host. See [`Module::Probe`].
    #[cfg(test)]
    pub probe: ProbeHost,
}

/// The TEST-ONLY third module host: enough state to be live, to be stopped, and to
/// carry a provenance. See [`Module::Probe`].
#[cfg(test)]
#[derive(Default)]
pub struct ProbeHost {
    /// Is it driving?
    pub live: std::sync::atomic::AtomicBool,
    /// The reason of the last controlled stop it was given, if any.
    pub stopped_because: std::sync::Mutex<Option<String>>,
}

impl Core {
    /// The station rows this daemon can report. A headless daemon has ONE, named in
    /// its config file: it has no operator to ask, and "whatever is attached" is the
    /// substitution the station factory exists to refuse.
    pub fn backends(&self) -> Vec<BackendInfo> {
        let id = match self.kind {
            BackendKind::Ni => BackendId::new("ni"),
            _ => BackendId::new("sim"),
        };
        vec![BackendInfo {
            label: match self.kind {
                BackendKind::Ni => "NI-DAQmx (daemon)".into(),
                _ => "Hertzforge-Sim (daemon)".into(),
            },
            vendor: "hertzforge-backendd".into(),
            tier: BackendTier::Available,
            selected: true,
            link: LinkState::Detected,
            caps: Some(self.capabilities(id.clone())),
            id,
        }]
    }

    /// What this daemon can actually do.
    ///
    /// **It claims nothing it has not got.** `modes.random` stays UNCLAIMED even though
    /// this core hosts Random, because a mode claim here is an EVIDENCE claim and
    /// Random's evidence is per-station: a per-tick-corrected mode needs a PROVEN
    /// Shared clock, established when `SetProfile` builds the station and `arm()`
    /// consults the proven-set - not at descriptor time, where claiming it would be
    /// structurally inconsistent (`HostedClosedLoopWithoutSharedClock`). Discovery over
    /// this seam is therefore the VERB: `SetProfile(Random)` answers with a typed
    /// acceptance or a typed refusal carrying the station's actual reason.
    ///
    /// # `open_loop_sine` IS claimed, and here is the evidence it rests on
    ///
    /// `{HostSynthesized, None}`: this core synthesizes every drive sample and nothing
    /// adapts them inside the run. That resolution has NO shared-clock precondition -
    /// the operator is the loop - which is why it can be stated at descriptor time when
    /// Random's cannot. The claim is still an evidence claim, and the evidence is the
    /// Explorer chassis gate: `hertzforge-station/examples/ni_module_sweep.rs`'s Explorer
    /// section, which arms at 0 V, ramps to 0.2 Vpk (measured RMS x sqrt(2) within 5%),
    /// retunes 100 -> 400 Hz (measured within 2%), sweeps a decade up and down, ramps
    /// to a stop, and asserts the worst-tick compute under half the budget - with the
    /// DMM probe run against the same lifecycle for drive continuity. The ORDER was not
    /// inverted: the gate passed on the chassis before this line was written, which is
    /// the whole of the evidence rule (bench-test, arm on real hardware, THEN declare).
    ///
    /// # The action rows say what this daemon SERVES
    ///
    /// Every row below is a verb `CoreSession::handle` actually dispatches, and
    /// `ramp_down` is `Unsupported` because that arm is a typed refusal. They are
    /// `Instant` rather than `Converging` because a host-synthesized drive IS
    /// absolute-setpoint hardware: an intent is clamped and adopted before the response
    /// returns, and what happens afterwards is the SLEW walking the generated value to
    /// it - which is not a converging command, it is the answer already being true and
    /// the physics taking its time. (`Converging` is for the paced/nudged path, where
    /// the core does not know where it landed until telemetry says so.)
    ///
    /// The hardware vocabulary (loop owner, clock, rates, channel counts) IS filled,
    /// because it is a fact about the station and a UI shows it.
    ///
    /// # `modes.sine` STAYS UNCLAIMED, and here is the evidence ledger behind that
    ///
    /// The Sine host landed and `modes.sine` did not move, for exactly Random's reason
    /// and not out of caution: closed-loop sine is PER-TICK CORRECTED, so its
    /// resolution needs a PROVEN Shared clock, which is a per-STATION fact established
    /// when `SetProfile` builds the station and `arm()` consults the proven-set.
    /// Claiming it here, with `clock: Unknown`, would be structurally inconsistent
    /// (`HostedClosedLoopWithoutSharedClock`) and would advertise exactly what `arm()`
    /// goes on to refuse. Discovery over this seam is the VERB: `SetProfile(Sine)` +
    /// `Start` answers with a typed acceptance or a typed refusal carrying the
    /// station's actual reason.
    ///
    /// The Sine chassis evidence as it stands (a cDAQ-9178, AO0->AI0 jumper), recorded
    /// here because "the evidence is elsewhere" is how an unclaimed mode quietly
    /// becomes a claimed one:
    ///
    /// | Row | Result | Where |
    /// |---|---|---|
    /// | armed closed-loop, 51200.000 Hz adopted | GREEN | first engaged hardware sine, 2026-08-08 |
    /// | ladder to FullLevel | GREEN | same run |
    /// | −6 dB feedforward negative control closed by the servo (0.2000 V measured vs 0.2000 V target) | GREEN | same run |
    /// | dwell hold σ 0.000 dB | GREEN | same run |
    /// | abort took the e-stop path in 272 ms | GREEN | same run |
    /// | AwaitingReview parks on a PASSING gate, 3/3; proceed-from-CoherenceHold refused | GREEN | same run |
    /// | sysID band coherence 0.990133 / 0.9784 / 0.9785 against the 0.999 bar | **FAIL** | three arms, same run |
    /// | 80 Hz retune-settle run | **FAIL** (aborted) | same run |
    /// | COLA second-AO derivation | **FAIL**, since fixed | gate defect, not a module defect |
    ///
    /// Two of those are open and one is closed. **The sine chassis gate has not
    /// passed twice**, so this daemon claims no sine mode, and it will not until the ledger
    /// above is all-green twice - which is the evidence rule's order (bench-test, arm
    /// on real hardware, THEN declare), not a formality.
    ///
    /// # `modes.twr` IS CLAIMED (2026-08-09), and here is the evidence behind the claim
    ///
    /// THE ORDER WAS NOT INVERTED: the chassis gate came first and is recorded - the
    /// eight numeric TWR chassis rows PASSED TWICE at one tip on the real cDAQ-9178
    /// (bench asserted by the operator; run 2 witnessed, its log kept on the bench
    /// host). The rows behind the claim are numeric, not liveness: measured lag 41 vs
    /// model 39 frames (+2 residual in a +/-4 band); in-band sysID coherence 0.9999975
    /// vs the rig-derived 0.990 floor; the FULL LADDER converged (state Complete, 1.85%
    /// rms vs the 10% gate, 5 correction iterations - the first TWR replication
    /// iterations ever proven on hardware); 22.3x over-buffer streaming with ZERO
    /// driver underrun warnings; the rate refusal quoting 8192 vs 51200; the ceiling
    /// refusal quoting 0.3953 vs 0.2500 V; per-play cycle cost measured (arm->start
    /// median 138.5 ms, stop->unreserve median 69.6 ms - this host's watchdog budgets);
    /// Stop-mid-play -> `Stopped` with the task released. Every row was ALSO shown red
    /// by its `HERTZFORGE_T2_*` mutation in the same sitting, and the DMM measured the drive
    /// PRESENT during a play (+0.194 V) and ABSENT (1.4 mV) across the 8 s park.
    ///
    /// (The historical unclaimed-posture ledger below is kept as written - it is the
    /// record of what the claim was HELD ON until the evidence existed.)
    ///
    /// # `modes.twr` STAYED UNCLAIMED until then, and here is the evidence ledger behind that
    ///
    /// TWR's claim is UNLIKE every other module's on this core, and the difference is
    /// worth stating because it changes what "claimed" would even mean. `TestMode`
    /// mirrors `hertzforge_hal::NativeMode`, which has no `Twr` and deliberately will not get
    /// one: TWR is a HOST-ONLY mode on every path - its law runs BETWEEN plays, so
    /// adaptation stays host-side even under device delegation - and no device will ever
    /// declare it. The mirror invariant therefore has a stated exception here (see
    /// `hertzforge_protocol::caps`'s module docs), and the post-evidence claim path is a
    /// MANUAL ModeMatrix assignment citing the bench ledger, exactly as Shock's was.
    ///
    /// **What the claim will rest on, when it exists:** the eight numeric rows of the
    /// TWR chassis sitting, PASSED TWICE at one tip. Two independent authorities per
    /// row, and every row shown RED by an injected mutation in the same sitting: the
    /// rig-derived in-band sysID coherence floor; the alignment row (xcorr against the
    /// group-delay decomposition, failing on disagreement and naming which number came
    /// from where); a full-ladder convergence asserting
    /// `state == Complete && ladder.completed() && rms <= gate && abort_reason.is_none()
    /// && clipped_frac == 0`; the 21x-buffer streaming row with ZERO driver underrun
    /// warnings; the can-fail rate refusal quoting both rates; the ceiling refusal; the
    /// per-play cycle cost that feeds this host's watchdog thresholds; and Stop-mid-play
    /// ending `Stopped` rather than `Aborted`.
    ///
    /// **What it will NOT rest on:** the 2026-07-30 sweep rows. Those called
    /// `run_to_completion(1)`, which from `Configured` executes the sysID pass and
    /// nothing else, and the sweep's TWR section asserted nothing numeric (it printed
    /// `0 convergence record(s)`). **ZERO TWR replication iterations have ever run on
    /// hardware.** Nothing may be declared from a liveness row.
    ///
    /// The discipline is MECHANICAL rather than a note here: while `crate::twr::
    /// TWR_CHASSIS_LEDGER_DATE` is `None`, `TwrHost::start` refuses BY NAME on an
    /// NI-REAL station. The wire probe asserts that refusal when it is not on
    /// SIM, so the guard is a gate rather than a comment.
    pub fn capabilities(&self, id: BackendId) -> CapabilityDescriptor {
        let mut caps = CapabilityDescriptor::claim_nothing(id);
        caps.loop_owner = LoopOwner::HostClosesLoop;
        // UNKNOWN, deliberately: the daemon has not proven a shared clock on this
        // station, and the audit posture is never to claim one that was not proven.
        // Note that this does NOT gate the open-loop claim below - the clock rule binds
        // the ADAPTATION axis, and open-loop sine has no adaptation.
        caps.clock = ClockDomain::Unknown;
        // ALL THREE display streams, corrected in the v5 bundle.
        //
        // ~~`vec![StreamKind::Spectrum]`~~ was true when the Recorder was the only
        // producer. It stopped being true when the time taps landed: `display.rs` packs
        // `scope_batch` and `wave_batch` for every host that exposes a raw tap, the
        // Explorer's time streams are chassis-verified on the wire, and the subscribe
        // path has served all three kinds for as long as they have existed. A
        // descriptor that under-claims is not the safe direction it looks like - a UI
        // that reads this list to decide which subscriptions to offer would hide time
        // streams the core is already producing, and the operator would conclude the
        // daemon cannot do time.
        //
        // This is a claim about the TRANSPORT, not about any one module: a module with
        // no raw tap simply publishes nothing on those kinds, which the per-feed health
        // surface already reports honestly.
        caps.streams = vec![
            StreamKind::Scope,
            StreamKind::Waveform,
            StreamKind::Spectrum,
        ];
        caps.modes.open_loop_sine =
            hertzforge_protocol::ModeSupport::Supported(hertzforge_protocol::Resolution::new(
                hertzforge_protocol::DriveOrigin::HostSynthesized,
                hertzforge_protocol::AdaptationOwner::None,
            ));
        // ---- SHOCK AND SRS ARE CLAIMED, and here is the evidence they rest on ------
        //
        // `{HostSynthesized, HostBetweenRuns}` for both: this core synthesizes every
        // drive sample (the compensated classical composite, the SRS wavelet drive) and
        // its correction law runs BETWEEN shots - the classical drive-limit carry and
        // the SRS per-band ratio update both act on the NEXT pulse, never inside the one
        // in flight. That resolution has NO shared-clock precondition (the clock rule
        // binds the ADAPTATION axis and nothing here corrects per tick), which is why it
        // can be stated at descriptor time where Random's cannot.
        //
        // THE ORDER WAS NOT INVERTED. The chassis gate came first and is recorded:
        // **PASS TWICE at one tip**, two consecutive full sweeps on the real cDAQ-9178,
        // classical 14/14 + SRS 4/4 green both runs (both logs kept on the bench host).
        // The rows behind this claim are numeric, not liveness: the group-delay
        // alignment row at measured -323 vs model -321, r +0.999985; the DMM-timed
        // pulse width at 7.300 ms against a stated 7.324 +/- 0.100 ms - the first
        // drive-side truth this module has ever had; the wall-clock rate assay within
        // 0.3%; the drive-volts ceiling refusing by name; abort -> re-arm -> fire
        // proving no zombie AO; a 5-pulse train at wall-clock spacing within 0.48%; and
        // on the SRS side the sysID band-coherence floor, the 3-iteration convergence
        // trend and the review hold entered, held with the DMM at 0.000 V, and released.
        //
        // What this claim does NOT rest on: the 2026-07-30 sweep rows, which assert
        // "fired and captured" only and predate both the 2026-08-08 large-nPts drive
        // fix and the numeric bar. Nothing may be declared from those.
        let shock_resolution =
            hertzforge_protocol::ModeSupport::Supported(hertzforge_protocol::Resolution::new(
                hertzforge_protocol::DriveOrigin::HostSynthesized,
                hertzforge_protocol::AdaptationOwner::HostBetweenRuns,
            ));
        caps.modes.shock = shock_resolution.clone();
        caps.modes.srs = shock_resolution;
        // TWR: HostSynthesized drive + HostBetweenRuns adaptation (the law runs BETWEEN
        // plays - the honesty twist that survives even device delegation). Claimed per the
        // PASS-x2 ledger in this function's doc comment; the compiled ledger date
        // (`crate::twr::TWR_CHASSIS_LEDGER_DATE`) flipped in the same commit.
        caps.modes.twr =
            hertzforge_protocol::ModeSupport::Supported(hertzforge_protocol::Resolution::new(
                hertzforge_protocol::DriveOrigin::HostSynthesized,
                hertzforge_protocol::AdaptationOwner::HostBetweenRuns,
            ));
        caps.actions = hertzforge_protocol::ActionMatrix {
            set_profile: ActionSupport::Instant,
            start: ActionSupport::Instant,
            stop: ActionSupport::Instant,
            abort: ActionSupport::Instant,
            // REFUSED by the `RampDown` arm below. A dedicated ramp-down verb has no
            // module behind it here: Random's shutdown ramp rides `Stop`, and the
            // Explorer's rides `Stop` too.
            ramp_down: ActionSupport::Unsupported,
            set_level: ActionSupport::Instant,
            adjust_level: ActionSupport::Instant,
            go_to_frequency: ActionSupport::Instant,
            adjust_frequency: ActionSupport::Instant,
        };
        caps
    }

    /// Every capture on this host.
    pub fn captures(&self) -> Vec<CaptureInfo> {
        crate::captures::list(&self.data_root)
    }

    /// **THE ONE AUTHORITY over which module owns this core**, at the requested
    /// [`Claim`] strength. Every site that used to ask "is Random up? no? is the
    /// Recorder up?" asks this instead.
    ///
    /// Returns the highest-priority module holding the claim ([`Module::ALL`] states
    /// the order), or `None` when the core is idle in that sense. `None` is a real
    /// answer, not a gap: it is what makes "nothing is running on this core" a truth
    /// about the whole core rather than about the two modules a chain happened to
    /// know.
    pub fn live_module(&self, claim: Claim) -> Option<Module> {
        Module::ALL.iter().copied().find(|&m| self.holds(m, claim))
    }

    /// Does `m` hold the core at this strength? The per-module half of
    /// [`Core::live_module`], exhaustive so a new module must answer both questions.
    fn holds(&self, m: Module, claim: Claim) -> bool {
        match (m, claim) {
            (Module::Random, Claim::Live) => self.random.is_live(),
            (Module::Random, Claim::Configured) => self.random.is_configured(),
            (Module::Explorer, Claim::Live) => self.explorer.is_live(),
            (Module::Explorer, Claim::Configured) => self.explorer.is_configured(),
            // BOTH answers are `true` through a fire, a train, a system ID and the
            // review hold. The hold is the one that needed deciding and it is the
            // interesting half: nothing is driving there and the hardware is parked,
            // but the module still OWNS the core - a second module binding the chassis
            // while an operator studies an FRF would take the run away from under them.
            // `is_live` on both hosts counts Armed for the same reason the Explorer's
            // does: the arm commits the DAQmx tasks.
            (Module::Shock, Claim::Live) => self.shock.is_live(),
            (Module::Shock, Claim::Configured) => self.shock.is_configured(),
            (Module::Srs, Claim::Live) => self.srs.is_live(),
            (Module::Srs, Claim::Configured) => self.srs.is_configured(),
            (Module::Sine, Claim::Live) => self.sine.is_live(),
            (Module::Sine, Claim::Configured) => self.sine.is_configured(),
            // BOTH answers are `true` through every TWR park - the review hold, the
            // coherence hold, the feasibility block and the gate-fail hold. Same
            // reasoning as SRS's: a park is a PAUSED run holding a claimed core, not an
            // idle one, and a second module binding the chassis while an operator
            // studies an FRF would take the run away from under them.
            (Module::Twr, Claim::Live) => self.twr.is_live(),
            (Module::Twr, Claim::Configured) => self.twr.is_configured(),
            (Module::Recorder, Claim::Live) => self.recorder.is_live(),
            (Module::Recorder, Claim::Configured) => self.recorder.is_configured(),
            #[cfg(test)]
            (Module::Probe, Claim::Live) | (Module::Probe, Claim::Configured) => {
                self.probe.live.load(std::sync::atomic::Ordering::SeqCst)
            }
        }
    }

    /// Bring `m` down through its CONTROLLED stop - the taper, never a hard cut.
    ///
    /// Never fails, for the reason each host's own `safe_stop` never fails: a stop
    /// that refuses because the module was already idle turns a timeout into an error
    /// an operator has to interpret.
    pub fn stop_module(&self, m: Module, reason: &str) {
        match m {
            Module::Random => self.random.safe_stop(reason),
            Module::Explorer => self.explorer.safe_stop(reason),
            Module::Shock => self.shock.safe_stop(reason),
            // The OPERATOR's stop, which DOES end a review hold - see `Core::safe_stop`
            // for why the vanish path does not.
            Module::Srs => self.srs.operator_stop(reason),
            Module::Sine => self.sine.safe_stop(reason),
            // The OPERATOR's stop, which DOES end a park - see `Core::safe_stop` for why
            // the vanish path does not (the same split the SRS module needs).
            Module::Twr => self.twr.operator_stop(reason),
            Module::Recorder => self.recorder.safe_stop(reason),
            #[cfg(test)]
            Module::Probe => {
                self.probe
                    .live
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                *self.probe.stopped_because.lock().expect("probe poisoned") =
                    Some(reason.to_string());
            }
        }
    }

    /// Bring `m` down IMMEDIATELY - `Command::Abort`'s path.
    ///
    /// Split from [`Core::stop_module`] when the Explorer landed, because from then on
    /// the two verbs stop meaning the same thing. Both were routed to the controlled
    /// stop while every module the daemon hosted had only one way down; the Explorer
    /// has two, and its difference is the whole point: `Stop` ramps the drive to zero
    /// at the operator's slew, `Abort` issues `emergency_stop` and lets the backend's
    /// own `RampDownThenAbort` taper be the backstop. A daemon that answered an abort
    /// with a seven-second ramp would be deciding, on the operator's behalf, that
    /// whatever made them hit Abort can wait.
    ///
    /// Modules with only a controlled stop still get it here, and that is not a fudge:
    /// their fastest way down IS the taper.
    pub fn abort_module(&self, m: Module, reason: &str) {
        match m {
            Module::Random => self.random.safe_stop(reason),
            Module::Explorer => {
                if let Err(e) = self.explorer.abort(reason) {
                    tracing::warn!(target: "hertzforge::backendd", "explorer abort: {e}");
                }
            }
            Module::Shock => self.shock.abort(reason),
            Module::Srs => self.srs.abort(reason),
            // Sine's abort is `emergency_stop` with the backend's 25 ms taper as the
            // backstop, and it is NOT the ramp: an abort during a 27 s sysID burst has
            // to be seen inside one block, which the service's cancellation flag is what
            // provides.
            Module::Sine => {
                if let Err(e) = self.sine.abort(reason) {
                    tracing::warn!(target: "hertzforge::backendd", "sine abort: {e}");
                }
            }
            // `emergency_stop`, with the backend's 25 ms taper as the backstop rather
            // than the ramped stop's 250 ms one. Bounded below by whatever the device has
            // already committed: mid-record preemption is refused by this chassis
            // (-200289), so a TWR abort is a promise about the committed remainder.
            Module::Twr => self.twr.abort(reason),
            Module::Recorder => self.recorder.safe_stop(reason),
            #[cfg(test)]
            Module::Probe => self.stop_module(m, reason),
        }
    }

    /// `m`'s provenance - what its data IS - if it has one.
    pub fn module_provenance(&self, m: Module) -> Option<RunProvenance> {
        match m {
            Module::Random => self.random.provenance(),
            Module::Explorer => self.explorer.provenance(),
            Module::Shock => self.shock.provenance(),
            Module::Srs => self.srs.provenance(),
            Module::Sine => self.sine.provenance(),
            Module::Twr => self.twr.provenance(),
            Module::Recorder => self.recorder.provenance(),
            #[cfg(test)]
            Module::Probe => None,
        }
    }

    /// Fold `m`'s truth into a protocol status report.
    fn fill_module_status(&self, m: Module, s: &mut StatusReport) {
        match m {
            Module::Random => self.random.fill_status(s),
            Module::Explorer => self.explorer.fill_status(s),
            Module::Shock => self.shock.fill_status(s),
            Module::Srs => self.srs.fill_status(s),
            Module::Sine => self.sine.fill_status(s),
            Module::Twr => self.twr.fill_status(s),
            Module::Recorder => {
                // Monitor-only: no level, no frequency, no drive. The only honest
                // thing it can say is that it is up, and it says that only while it
                // actually is - a configured recorder that is not monitoring is not
                // a running one.
                if self.recorder.is_live() {
                    s.run = RunState::Running;
                    s.elapsed_s = Some(0.0);
                }
            }
            #[cfg(test)]
            Module::Probe => {
                s.run = RunState::Running;
            }
        }
    }

    /// The run status a remote poll answers with: whichever module HOLDS the core
    /// answers it, through the one authority.
    pub fn status(&self) -> StatusReport {
        let mut s = StatusReport::idle();
        s.backend = Some(match self.kind {
            BackendKind::Ni => BackendId::new("ni"),
            _ => BackendId::new("sim"),
        });
        if let Some(m) = self.live_module(Claim::Configured) {
            self.fill_module_status(m, &mut s);
        }
        s
    }

    /// THE SAFE-STOP PATH for the controller timeout: whichever module is live is
    /// brought down through its CONTROLLED stop.
    ///
    /// # Why this is not simply [`Core::stop_module`] any more
    ///
    /// The two callers ask different questions and one module answers them
    /// differently. An OPERATOR Stop means "end this run"; a controller TIMEOUT means
    /// "nobody is watching, make the hardware safe". For every module those coincide,
    /// because everything else that holds the core is either driving or monitoring.
    /// The SRS review hold is neither: the plant is already parked, nothing can move,
    /// and the only thing a teardown would accomplish is discarding a measured system
    /// ID because a network went quiet. So the vanish path leaves the hold STANDING
    /// and the operator's Stop still ends it.
    pub fn safe_stop(&self, reason: &str) {
        match self.live_module(Claim::Live) {
            Some(Module::Srs) => self.srs.safe_stop(reason),
            // TWR has the same split for the same reason, and TWO parks in it: the
            // post-sysID review hold and the coherence hold both leave the hardware
            // parked with a measured system ID behind them.
            Some(Module::Twr) => self.twr.safe_stop(reason),
            Some(m) => self.stop_module(m, reason),
            None => {}
        }
    }

    /// The NAMED refusal for an intent no live module can serve, routed through the
    /// one authority so it describes the core's actual state.
    ///
    /// Two sentences, deliberately: the first says what is (or is not) live, the
    /// second says where the capability is going to come from. A refusal that only
    /// said "unsupported" would leave an operator pressing the same control.
    pub fn no_module_for(&self, intent: &str) -> ProtocolError {
        let head = match self.live_module(Claim::Live) {
            Some(m) => format!(
                "the {} is live on this core and takes no {intent} intents",
                m.label()
            ),
            None => format!("no {intent}-bearing module is live on this core"),
        };
        ProtocolError::Refused {
            message: format!("{head}. {}", how_to_get_one(intent)),
        }
    }

    /// **THE MUTUAL-EXCLUSION GATE**: may `want` take the core right now?
    ///
    /// One module at a time owns the hardware. `Ok(())` when the core is idle or when
    /// `want` is already the occupant (its own host then answers the finer question -
    /// "a Random run is live; Stop it before reconfiguring"); a NAMED refusal
    /// otherwise, carrying the occupant and the verb that stands it down.
    ///
    /// This replaced a `recorder_live: bool` threaded down into [`RandomHost`], which
    /// only ever expressed one direction of the rule. Asking the authority expresses
    /// both: a Recorder configure while a Random run is DRIVING is now refused too,
    /// where before it was silently allowed to build a second station on the same
    /// hardware.
    pub fn claim_core(&self, want: Module) -> Result<(), ProtocolError> {
        match self.live_module(Claim::Live) {
            None => Ok(()),
            Some(live) if live == want => Ok(()),
            Some(live) => Err(ProtocolError::Refused {
                message: format!(
                    "the {} is live on this core; send {} before configuring or \
                     starting the {}",
                    live.label(),
                    live.stand_down_hint(),
                    want.label()
                ),
            }),
        }
    }

    /// **THE IDLE-CLAIM HOTFIX**: configuring a module RELEASES every other module's
    /// CONFIGURED-BUT-IDLE claim, so a bare `Start` addresses the module the operator
    /// just configured.
    ///
    /// # The operator failure this closes
    ///
    /// The operator's first acceptance run of the Sine module, 2026-08-09: an Explorer
    /// probe had left a STOPPED Explorer service parked on the shared daemon. The
    /// operator configured a sine profile and pressed Run, and the daemon answered
    /// **"Explorer cannot start from Stopped"** - a refusal naming a module the
    /// operator had never opened. Nothing was broken in the sine path; `Command::Start`
    /// walks [`Module::ALL`] at [`Claim::Configured`] and the Explorer outranks Sine,
    /// so the parked config took the verb. The same pairing reproduces from the Shock
    /// panel, and the string the operator reads is identical, because the module doing
    /// the refusing is the parked one.
    ///
    /// # What it does and does NOT touch
    ///
    /// It releases the IDLE claims only. A **LIVE** module still blocks
    /// [`Core::claim_core`] exactly as before, with the same named refusal and the
    /// same stand-down hint: a run that is driving, armed, firing or holding is never
    /// displaced by someone else's `SetProfile`, and the ordering here guarantees it -
    /// the caller claims the core FIRST and only reaches this on the `Ok`.
    ///
    /// This is the HOTFIX half of the fix, not its close. The other candidate - `Start`
    /// carrying the module family - is the wire-level fix and rides the TWR v6 bump; it
    /// is what makes the routing unambiguous when two clients genuinely both hold
    /// configs. This one makes the operator's own sequence (configure, then run) mean
    /// what it says, with no wire change.
    ///
    /// Returns the modules actually released, so the caller journals NAMES. A silent
    /// release is how a second operator's parked setup would vanish without a trace.
    pub fn release_idle_claims(&self, keep: Module) -> Vec<Module> {
        let mut released = Vec::new();
        for &m in Module::ALL {
            if m == keep {
                continue;
            }
            // IDLE means: holds a built station, and is not live. Both questions go
            // through the one authority - asking a host directly is how the five
            // pairwise chains this module enum replaced got out of step.
            if !self.holds(m, Claim::Configured) || self.holds(m, Claim::Live) {
                continue;
            }
            if self.release_module(m) {
                released.push(m);
            }
        }
        released
    }

    /// Drop `m`'s built-but-idle service. The per-module half of
    /// [`Core::release_idle_claims`], exhaustive so a new module must answer it.
    ///
    /// **The Recorder deliberately answers `false`** and that is a decision, not an
    /// omission. It cannot cause the defect being fixed: a bare `Start` against a
    /// configured Recorder is already refused BY NAME ("the Recorder holds this core;
    /// it runs on Recorder(StartMonitor)"), so it never steals the verb the way a
    /// parked Explorer does. And its configure is a distinct operator action with its
    /// own panel - discarding it silently would be a new surprise of exactly the class
    /// this fix removes. A monitoring Recorder is LIVE and was never reachable here.
    fn release_module(&self, m: Module) -> bool {
        match m {
            Module::Random => self.random.release_idle(),
            Module::Explorer => self.explorer.release_idle(),
            // Both already own the verb for this: `reset()` is documented as "I am
            // done with this module", and its docs name THIS defect - a stopped run
            // left configured taking the next `Start` and answering "cannot arm from
            // Stopped". Reusing it keeps one teardown per module.
            Module::Shock => self.shock.reset().is_ok(),
            Module::Srs => self.srs.reset().is_ok(),
            Module::Sine => self.sine.release_idle(),
            // The STAGED REFERENCE deliberately survives this: the record is not the
            // run, and an operator who uploaded a 60 s field trace and then opened
            // another panel must not have to send it again. `Twr(Reset)` discards it.
            Module::Twr => self.twr.release_idle(),
            Module::Recorder => false,
            // Unreachable by construction: the probe's `holds` answers the same bool
            // for both claims, so it is never configured-but-idle. Written out anyway
            // because the exhaustive match is the fence.
            #[cfg(test)]
            Module::Probe => false,
        }
    }
}

/// One connection's view of the [`Core`].
pub struct CoreSession {
    core: Arc<Core>,
    /// The connection this session speaks for.
    pub conn: ConnId,
    /// Who it is, for provenance and for refusal messages.
    pub who: String,
}

impl CoreSession {
    /// Bind a connection to the core.
    pub fn new(core: Arc<Core>, conn: ConnId, who: String) -> Self {
        CoreSession { core, conn, who }
    }

    /// The shared core.
    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// **The `SetProfile` seam**: claim the core for `want`, then release every other
    /// module's parked config so the bare `Start` that follows addresses `want`.
    ///
    /// The ORDER is the safety property. `claim_core` runs first and its refusal is
    /// unchanged, so a LIVE module still blocks the configure and nothing of its state
    /// is touched; only on the `Ok` - the core is idle, or `want` already owns it - are
    /// the idle claims swept. See [`Core::release_idle_claims`] for the operator
    /// failure this closes.
    fn claim_and_displace(&self, want: Module) -> Result<(), ProtocolError> {
        self.core.claim_core(want)?;
        let released = self.core.release_idle_claims(want);
        if !released.is_empty() {
            let names: Vec<&str> = released.iter().map(|m| m.label()).collect();
            tracing::info!(
                target: "hertzforge::backendd",
                configured = want.label(),
                released = names.join(", "),
                who = %self.who,
                "released parked configured-but-idle claims so Start addresses \
                 the module just configured"
            );
        }
        Ok(())
    }
}

/// The modules that exist in the local app and are NOT served over this seam.
///
/// Enumerated rather than answered with a generic "not supported", because the front
/// end renders the NAME: "the Sine module is local-only in remote mode" is actionable,
/// "unsupported action" is not.
fn local_only(what: &str) -> Response {
    Response::Rejected {
        error: ProtocolError::LocalOnly { what: what.into() },
    }
}

/// The MODULE FAMILIES that can serve each routed intent, named in the refusal so the
/// answer says how to GET the capability rather than only that it is absent right now.
///
/// ~~`EXPLORER_HOW_TO_GET_ONE`, one hard-coded sentence naming the Explorer.~~
/// **REPLACED in the v5 bundle.** That constant was correct while the Explorer was the
/// only routed module, and it aged badly the moment there were four routed intents with
/// different servers: a `SetLevel{Db}` with nothing live answered "send SetProfile with
/// an Explorer profile", and the Explorer is precisely the module that CANNOT serve a dB
/// level. The refusal was pointing the operator at a dead end.
///
/// It is written to GROW SAFELY. Each arm names the families that serve that intent
/// TODAY, and an intent nobody serves falls to the generic tail rather than borrowing
/// another intent's answer - the failure mode of a shared sentence is that it stays
/// plausible while becoming wrong, which is exactly what happened to the constant this
/// replaces. Sine and Shock are deliberately NOT listed yet: their wire surfaces exist
/// at v5 but no host does, and naming a module the operator cannot start would be the
/// same dead end in a new place (a capability claim is an evidence claim).
fn how_to_get_one(intent: &str) -> &'static str {
    match intent {
        "frequency" | "sweep" => {
            "Send SetProfile with an Explorer profile and then Start: the Explorer is \
             the open-loop survey that serves frequency and sweep intents."
        }
        "drive-volts" => {
            "Send SetProfile with an Explorer profile and then Start: drive VOLTS is \
             the open-loop level basis, and the Explorer is the module that serves it."
        }
        "level" | "level-schedule" | "review-hold" => {
            // No module named on purpose (cross-review, 2026-08-08): an
            // exclusivity claim here goes stale BY DESIGN the moment another
            // closed-loop host lands. The capability listing is the authority.
            "dB levels, the level ladder and the post-sysID review hold belong to \
             the closed-loop modules. Send SetProfile with one of those profiles \
             and then Start; DescribeCapabilities lists which this core serves."
        }
        _ => {
            "Send SetProfile with a profile this core serves, and then Start; \
             DescribeCapabilities lists what it can do."
        }
    }
}

/// The four routed level intents, as data.
///
/// ~~A `FnOnce(&RandomHost)`~~ - the closure form was exactly right while Random was
/// the only module with a level, and it stopped being right the moment two more
/// modules had one: a closure that names one host cannot be dispatched to another, so
/// every new module would have had to be given its own copy of the router. As DATA the
/// intent survives the dispatch and each module answers it in one place.
#[derive(Debug, Clone, Copy)]
enum LevelIntent {
    SetLevel(hertzforge_protocol::LevelTarget),
    Adjust(f64),
    Ladder(hertzforge_protocol::LadderMode),
    Proceed,
}

/// The shock modules' answer to `SetLadderMode`, shared because it is one fact about
/// both of them.
///
/// AUTO-vs-MANUAL is not a mode a shock run holds; it is which VERB the operator sends.
/// Saying "unsupported" would leave an operator pressing a control that will never do
/// anything; saying this tells them the two verbs that do what the control was for.
const LADDER_IS_THE_VERB: &str =
    "a transient producer has no ladder MODE to take over - which verb you send IS the \
     mode. FirePulse fires one pulse at the level you are on (manual); RunAuto walks the \
     profile's schedule (auto); JumpToLevel names a rung directly. The schedule itself is \
     authored in the profile, where it is reviewable before anything arms";

/// The delegated open-loop sine profile: local-only, and not the same thing as the
/// host-synthesized Explorer survey.
const SMART_PROFILE_IS_LOCAL: &str = "the open-loop sine (SMART) profile";

/// TWR's answer to both frequency intents, shared because it is ONE fact about the
/// module and two copies of a refusal is two chances for them to drift.
const TWR_HAS_NO_FREQUENCY: &str =
    "a TWR run replicates a recorded TIME HISTORY sample for sample, so it has no tone and \
     no frequency to move - its band is the CORRECTION BAND authored in the profile \
     (law.f_lo_hz / law.f_hi_hz), which bounds where the inverse is trusted rather than \
     naming a frequency the drive sits at. Frequency intents belong to the sweep modules";

/// **The remote-capture refusal, verbatim.** Remote capture over this seam is a
/// COMMITTED requirement that is not built, and the three capture verbs exist on both
/// shock groups solely so this sentence can be the answer.
///
/// Why present-and-refusing rather than absent: an ABSENT verb fails to decode and
/// crosses back as [`ProtocolError::MalformedPayload`], which reads to an operator as
/// "your client is broken". A verb that exists and refuses reads as "this is not built
/// yet", which is the truth. The wire probe asserts this string, so it is a contract
/// and not a log line - edit it and the probe reds.
const REMOTE_CAPTURE_NOT_SERVED: &str =
    "remote capture is not served over this seam: captures are written on the CORE host \
     and no fetch path crosses this link yet, so a capture started from this console \
     would create a file the operator cannot see, cannot size, and cannot retrieve - \
     start and stop captures on the box.";

// ~~`not_served_yet`~~ - **DELETED at the SineHost x ShockHost merge (2026-08-08)**,
// with the same rule its NO_* constants already followed: every v5 module family now
// has a landed host, so nothing produces the missing-host sentence, and a
// plausible-looking helper nothing calls is exactly the drift this file's refusal
// taxonomy exists to prevent. The SHAPE (accepts-at-schema, refuses-by-missing-host,
// becomes-an-acceptance-with-no-wire-change) is documented in PROTOCOL_CHANGELOG v5
// and proven in git history (tests retired the same day); the next module family
// (Insight) re-adds it WITH its callers, or writes a better one.
//
// **RE-ADDED at the v6 TWR bump, WITH its callers** - which is the condition the
// paragraph above set. The module family turned out to be TWR rather than Insight; the
// rule is the same either way, and it stays the same on the way out: when `TwrHost`
// lands and turns both arms below into acceptances, this helper and its constant go
// with them rather than sitting here looking plausible.
//
// **DELETED AGAIN (2026-08-09), on exactly that condition.** `TwrHost` landed,
// both arms are acceptances, and nothing produces the missing-host sentence any more -
// so the helper and `NO_TWR_MODULE` go with them. The wire probe's twelve missing-host
// rows were retired in the SAME commit, which is what the two-file contract required:
// serve the strings verbatim, or change both files together. Neither half is left
// asserting a sentence the other no longer produces.
//
// The SHAPE is still the right one for the next module family and is documented in
// PROTOCOL_CHANGELOG v5 and v6 plus this file's git history. Whoever needs it next
// re-adds it WITH its callers, as the v6 bump did.

/// The refusal a module gives for an intent it holds the core against but cannot serve.
///
/// Names the OCCUPANT, the INTENT, the REASON, and the verb that stands the occupant
/// down - the four things an operator needs in order to do something other than press
/// the control again. A refusal that only said "unsupported" would leave them
/// wondering whether the module was broken or the command was.
fn module_takes_no(m: Module, what: &str, why: &str) -> ProtocolError {
    ProtocolError::Refused {
        message: format!(
            "the {} holds this core and takes no {what}: {why}. Send {} first if you \
             meant to run a different module.",
            m.label(),
            m.stand_down_hint()
        ),
    }
}

// ~~`NO_SINE_MODULE`~~ - DELETED with `not_served_yet` (same merge, same rule).
// ~~`NO_SHOCK_MODULE` / `NO_SRS_MODULE`~~ - **DELETED when the shock hosts landed**.
// They were the missing-host sentence for the two shock surfaces, and a shock verb
// against an unconfigured core now gets its HOST's own refusal instead ("the classical
// Shock module is not configured on this core; send SetProfile with a ShockClassical
// profile first"), which names the next command rather than the absent module. Keeping
// them would have left two plausible-looking strings nothing produces - the state a
// future reader mistakes for the current behaviour.
// ~~`NO_SINE_MODULE`~~ - **RETIRED when the Sine host landed.**
//
// It was the phrase every sine-side `not_served_yet` refusal ended its first
// sentence on, and there are no sine-side missing-host refusals left: the profile
// arm configures a real service and the command group routes through
// `Core::live_module` like every other verb. Removing the constant rather than
// leaving it unused is the point - a name that still exists is a name the next
// arm reaches for, and reaching for it would re-introduce a refusal for a
// capability this daemon now has.

fn accepted() -> Response {
    Response::Accepted {
        completion: Completion::Immediate,
    }
}

impl CoreApi for CoreSession {
    fn handle(&mut self, cmd: Command, events: &mut dyn EventSink) -> Response {
        // Every document-carrying command has its schema checked BEFORE anything acts
        // on it. One call, at the top, so a new verb cannot forget.
        if let Err(error) = cmd.check_schemas() {
            return Response::Rejected { error };
        }

        match cmd {
            // ---- discovery / status -------------------------------------------
            Command::DescribeCapabilities => {
                let id = self.core.backends()[0].id.clone();
                Response::Capabilities(Box::new(self.core.capabilities(id)))
            }
            Command::GetStatus => Response::Status(Box::new(self.core.status())),
            Command::ListBackends => Response::Backends(self.core.backends()),
            Command::SelectBackend { backend } => {
                // A headless daemon's station is CONFIG, not a runtime selection.
                // Accepting a change here would let a remote peer rebind the hardware
                // out from under a run on a box nobody is standing next to.
                let mine = &self.core.backends()[0].id;
                if &backend == mine {
                    accepted()
                } else {
                    Response::Rejected {
                        error: ProtocolError::Refused {
                            message: format!(
                                "this core serves the {mine:?} station, named in its \
                                 config file, and does not change stations on request. \
                                 A headless host has no operator standing next to it \
                                 to see what got rebound - edit the daemon config and \
                                 restart."
                            ),
                        },
                    }
                }
            }

            // ---- the channel table (daemon-authoritative) ----------------------
            Command::GetChannelTable => Response::ChannelTable(self.core.table.read()),
            Command::PutChannelTable { table } => match self.core.table.write(&table) {
                Ok(stored) => Response::ChannelTable(stored),
                Err(error) => Response::Rejected { error },
            },

            // ---- captures (LIST only; fetch is deferred) -----------------------
            Command::ListCaptures => Response::Captures(self.core.captures()),

            // ---- the Recorder --------------------------------------------------
            Command::Recorder(r) => self.recorder(r, events),

            // ---- supervisory stop / abort --------------------------------------
            //
            // Honored, and SUPERVISORY: the primary abort authority is on-hardware
            // and co-resident with the drive clock, backed by the physical E-stop. The
            // UI labels these; the daemon routes them to the LIVE module's controlled
            // stop - which on the NI backend is the measured shutdown taper, never a
            // hard cut.
            //
            // ROUTED THROUGH THE ONE AUTHORITY (`Core::live_module`), not through a
            // pairwise chain: "nothing is running" must be a statement about the
            // WHOLE core. The old chain would have said it while a module it did not
            // know about was driving, and an operator reads that as "the run already
            // ended".
            //
            // SPLIT when the Explorer landed: `Stop` is the CONTROLLED stop (the
            // Explorer's drive ramps to zero at the operator slew before the tasks
            // stop), `Abort` is immediate (`emergency_stop`, with the backend's own
            // taper as the backstop). They were one arm while every hosted module had a
            // single way down.
            Command::Stop => match self.core.live_module(Claim::Live) {
                Some(m) => {
                    self.core
                        .stop_module(m, &format!("remote supervisory stop from {}", self.who));
                    accepted()
                }
                None => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: "nothing is running on this core".into(),
                    },
                },
            },
            Command::Abort => match self.core.live_module(Claim::Live) {
                Some(m) => {
                    self.core
                        .abort_module(m, &format!("remote supervisory abort from {}", self.who));
                    accepted()
                }
                None => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: "nothing is running on this core".into(),
                    },
                },
            },

            // ---- the Random module (the first drive-producing module) ---
            Command::SetProfile { profile } => match profile {
                hertzforge_protocol::TestProfile::Random(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Random) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .random
                        .set_profile(p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
                hertzforge_protocol::TestProfile::OpenLoopSine(_) => {
                    local_only(SMART_PROFILE_IS_LOCAL)
                }
                // SERVED. ~~`local_only(EXPLORER_LOCAL_ONLY)`~~ - the host
                // landed and its chassis gate is a recorded PASS, so the wire surface v4
                // opened is now backed by a module.
                hertzforge_protocol::TestProfile::Explorer(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Explorer) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .explorer
                        .set_profile(p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
                // ---- v5: ACCEPTED at the schema level, refused until a host lands --
                //
                // These three arms are what makes the batched v5 bump safe to land
                // ahead of the hosts. The wire had to move once, for both parallel
                // sessions at once (see PROTOCOL_CHANGELOG v5); the daemon coverage
                // arrives per module. So the profile decodes, the schema gate passes,
                // and the answer is a NAMED refusal that says which host is missing -
                // never an ack, and never a silent drop.
                // SERVED. ~~`not_served_yet(...)`~~ - the host landed, so the wire
                // surface v5 opened is now backed by a module. The two arms below stay
                // refusing until their own sessions land theirs, which is exactly the
                // per-module arrival the batched bump was designed for.
                hertzforge_protocol::TestProfile::Sine(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Sine) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .sine
                        .set_profile(*p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
                // SERVED. ~~`not_served_yet(..)`~~ on both - the hosts landed and the
                // chassis evidence they rest on is the shock PASS-TWICE sweep.
                hertzforge_protocol::TestProfile::ShockClassical(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Shock) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .shock
                        .set_profile(*p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
                hertzforge_protocol::TestProfile::Srs(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Srs) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .srs
                        .set_profile(*p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
                // ---- v6: ACCEPTED at the schema level, refused until a host lands ---
                //
                // The same posture the three v5 arms above shipped in, for the same
                // reason: the wire moves once and the daemon coverage arrives per
                // module. The profile decodes, the schema gate passes, and the answer
                // NAMES the missing host - never an ack, never a silent drop.
                //
                // It STAGES NOTHING. There is no reference store on this core yet and
                // no `Module::Twr` to claim it with, so accepting-and-remembering would
                // be a core that says yes and forgets, which is worse than a refusal.
                //
                // SERVED. ~~`not_served_yet("the TWR profile", ..)`~~ - the
                // host landed, so the wire surface v6 opened is now backed by a module.
                //
                // It goes through `claim_and_displace` like the five before it:
                // a LIVE module still blocks the configure with its own named refusal,
                // and only on the `Ok` are the other modules' PARKED configs released so
                // the bare `Start` that follows addresses TWR.
                hertzforge_protocol::TestProfile::Twr(p) => {
                    if let Err(error) = self.claim_and_displace(Module::Twr) {
                        return Response::Rejected { error };
                    }
                    match self
                        .core
                        .twr
                        .set_profile(*p, self.core.table.snapshot(), &self.who)
                    {
                        Ok(()) => accepted(),
                        Err(error) => Response::Rejected { error },
                    }
                }
            },
            // `Start` DISPATCHES - it used to be Random's by hard-coding.
            //
            // The rule is the same one authority every other site asks: whichever module
            // HOLDS the core (at `Configured` strength - a built station, live or not) is
            // the one a bare `Start` is about. An idle core still routes to Random, so
            // its actionable "send SetProfile first" refusal is unchanged; that is the
            // one arm where the fallback is a deliberate choice rather than a lookup.
            Command::Start => match self.core.live_module(Claim::Configured) {
                Some(Module::Explorer) => {
                    if let Err(error) = self.core.claim_core(Module::Explorer) {
                        return Response::Rejected { error };
                    }
                    match self.core.explorer.start() {
                        Ok(()) => {
                            if let Some(p) = self.core.module_provenance(Module::Explorer) {
                                events.emit(Event::RunProvenance {
                                    provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                                });
                            }
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
                // ---- START IS **ARM** FOR THE TWO TRANSIENT PRODUCERS -----------
                //
                // A shock run has no continuous "running" state to start: it arms, and
                // then pulses are FIRED one verb at a time (`hertzforge_protocol::shock`'s
                // module docs state this on the wire side, and these two arms are what
                // make the statement true). The Explorer set the precedent that `Start`
                // means "whatever this module does when the operator says go"; without
                // these arms a UI would sit waiting for a `Running` that never arrives.
                //
                // ~~`not_served_yet(...)`~~ - the hosts landed and their chassis gate is
                // a recorded PASS-TWICE.
                Some(Module::Shock) => match self.core.shock.arm(None) {
                    Ok(()) => {
                        if let Some(p) = self.core.module_provenance(Module::Shock) {
                            events.emit(Event::RunProvenance {
                                provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                            });
                        }
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                Some(Module::Srs) => match self.core.srs.arm(None) {
                    Ok(()) => {
                        if let Some(p) = self.core.module_provenance(Module::Srs) {
                            events.emit(Event::RunProvenance {
                                provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                            });
                        }
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                // START IS **ARM-AND-SYSTEM-ID** for TWR (the wire surface says so in
                // `hertzforge_protocol::twr`'s module docs, and this arm is what makes the
                // statement true): a TWR run does not start and then run continuously -
                // it arms, identifies the plant, and then the operator ITERATES or hands
                // the converged drive to the schedule.
                Some(Module::Twr) => {
                    if let Err(error) = self.core.claim_core(Module::Twr) {
                        return Response::Rejected { error };
                    }
                    match self.core.twr.start() {
                        Ok(()) => {
                            if let Some(p) = self.core.module_provenance(Module::Twr) {
                                events.emit(Event::RunProvenance {
                                    provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                                });
                            }
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
                Some(Module::Sine) => {
                    if let Err(error) = self.core.claim_core(Module::Sine) {
                        return Response::Rejected { error };
                    }
                    match self.core.sine.start() {
                        Ok(()) => {
                            if let Some(p) = self.core.module_provenance(Module::Sine) {
                                events.emit(Event::RunProvenance {
                                    provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                                });
                            }
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
                // The Recorder holds a built station but has its own lifecycle verbs;
                // saying so beats routing to Random and answering about a module the
                // operator never mentioned.
                Some(Module::Recorder) => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: "the Recorder holds this core; it runs on \
                                  Recorder(StartMonitor), not Start"
                            .into(),
                    },
                },
                #[cfg(test)]
                Some(Module::Probe) => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: "the test probe module holds this core".into(),
                    },
                },
                Some(Module::Random) | None => {
                    if let Err(error) = self.core.claim_core(Module::Random) {
                        return Response::Rejected { error };
                    }
                    match self.core.random.start() {
                        Ok(()) => {
                            // The STARTING controller learns what its data IS, exactly
                            // as the recorder's StartMonitor does - and for the same
                            // reason: the at-connect push covers mid-run attachers only,
                            // so without this the one peer guaranteed to care never gets
                            // the provenance and renders the NO PROVENANCE honesty chip
                            // over a perfectly attributed run (read live, 2026-08-01).
                            if let Some(p) = self.core.module_provenance(Module::Random) {
                                events.emit(Event::RunProvenance {
                                    provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                                });
                            }
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
            },
            Command::RampDown => Response::Rejected {
                error: ProtocolError::UnsupportedAction {
                    action: ActionKind::RampDown,
                },
            },
            // DRIVE-VOLTS is the OPEN-LOOP level basis, so it is a question for
            // whichever open-loop module is live - routed through the one authority
            // rather than falling into Random's `InvalidArgument`, which named the
            // basis but never said WHO could serve it. The Explorer is that module now,
            // and the routing arm has somewhere to route to.
            Command::SetLevel {
                level: level @ hertzforge_protocol::LevelTarget::DriveVolts(_),
            } => match self.core.live_module(Claim::Live) {
                Some(Module::Explorer) => match self.core.explorer.set_level(level) {
                    Ok(vpk) => {
                        events.emit(Event::StateChanged {
                            status: Box::new(self.core.status()),
                        });
                        tracing::info!(target: "hertzforge::backendd", vpk, who = %self.who, "remote drive level");
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                // A MODULE THAT HOLDS THE CORE AND HAS A LEVEL - just not this basis -
                // answers for itself. Falling through to `no_module_for` here would tell
                // an operator whose shock run is armed to "send an Explorer profile",
                // which is the dead-end class the capability-aware tail was written to
                // end: the answer would be about a module they never mentioned and
                // would stand the one they are running down.
                // TWR answers for itself for the same reason: a run whose drive is the
                // output of an inverse filter has no volts SETPOINT at all, and falling
                // through would tell an operator mid-replication to go send an Explorer
                // profile.
                Some(m @ Module::Twr) => Response::Rejected {
                    error: module_takes_no(
                        m,
                        "drive-VOLTS level",
                        "a TWR drive is the output of an inverse filter, so its volts are \
                         whatever reproducing the reference demands - there is no setpoint to \
                         command. The volts number this run enforces is max_drive_vpk, checked \
                         on the REALIZED record before every play, and it refuses rather than \
                         clipping",
                    ),
                },
                _ => match self.core.live_module(Claim::Configured) {
                    Some(m @ (Module::Shock | Module::Srs)) => Response::Rejected {
                        error: match shock_db_target(m, level) {
                            Err(e) => e,
                            // Unreachable: this arm only matches DriveVolts. Written as a
                            // typed answer rather than an `unwrap_err` so a future basis
                            // cannot turn a refusal into a panic.
                            Ok(_) => self.core.no_module_for("drive-volts"),
                        },
                    },
                    _ => Response::Rejected {
                        error: self.core.no_module_for("drive-volts"),
                    },
                },
            },
            // ---- THE FOUR LEVEL-SCHEDULE VERBS, now ROUTED (v5 shared bundle) ----
            //
            // These four were the last hard-coded `self.core.random.*` calls in the
            // dispatcher - the residue of the era when Random was the only module that
            // had a level at all. They are the same defect `Core::live_module` was
            // built to end at every other site (see this file's header): with a
            // NON-Random module holding the core, a `SetLevel` reached Random anyway
            // and answered about a module the operator never mentioned - "Random is not
            // configured on this core", while an Explorer run was driving.
            //
            // WHICH CLAIM STRENGTH, and why it is `Configured` rather than `Live`.
            // A level schedule and a review hold belong to whichever module HOLDS the
            // core, driving or not - the same question `Command::Start` asks. Routing
            // on `Live` would have been the tighter-looking choice and is the wrong
            // one: `RandomHost::is_live` matches only the driving states, and the
            // review hold is not one of them, so `ProceedAfterReview` on `Live` would
            // make the hold unreleasable over the seam - a run parked forever with the
            // one verb that frees it refusing by design. `Configured` also preserves
            // today's Random behaviour byte-for-byte in the configured-but-idle case,
            // where the host's own `not_configured` / service errors are the right
            // answer and always were.
            //
            // `None` is `no_module_for`, NOT a fallback to Random: "nothing here can
            // serve a level intent" has to be a statement about the whole core.
            Command::SetLevel { level } => self.route_level_intent(
                "level",
                "absolute dB level",
                "remote level jump",
                events,
                LevelIntent::SetLevel(level),
            ),
            Command::AdjustLevel { delta_db } => self.route_level_intent(
                "level",
                "relative dB level step",
                "remote level step",
                events,
                LevelIntent::Adjust(delta_db),
            ),
            Command::SetLadderMode { ladder } => self.route_level_intent(
                "level-schedule",
                "level-schedule takeover",
                "remote ladder-mode takeover",
                events,
                LevelIntent::Ladder(ladder),
            ),
            Command::ProceedAfterReview => self.route_level_intent(
                "review-hold",
                "post-sysID review release",
                "operator proceeded after sysID review",
                events,
                LevelIntent::Proceed,
            ),
            // FREQUENCY INTENTS are ROUTED, not blanket-refused (protocol v4).
            // The rule is "dispatch to the live module if it is frequency-bearing,
            // otherwise refuse by NAME" - so the refusal describes the core's actual
            // state instead of asserting that frequency control does not exist here.
            // ~~"No module this core hosts today is frequency-bearing"~~ - the Explorer
            // is, and the dispatching arm arrived with it.
            //
            // The answer carries the CLAMPED value into the journal, not the request: a
            // request that got clamped at a band edge and one that was honored must not
            // look the same afterwards either.
            //
            // TWO frequency-bearing modules since the Sine host landed. Both CLAMP into
            // their own band and both answer with the value they ADOPTED, which is what
            // the journal line carries: a request that got clamped at a band edge and
            // one that was honored must not look the same afterwards. Sine's clamp
            // lives in the scheduler, so the echo, the NCO and the status readout are
            // one number.
            Command::GoToFrequency { hz } => match self.core.live_module(Claim::Live) {
                Some(Module::Explorer) => match self.core.explorer.go_to_frequency(hz) {
                    Ok(adopted) => {
                        events.emit(Event::StateChanged {
                            status: Box::new(self.core.status()),
                        });
                        tracing::info!(target: "hertzforge::backendd", hz, adopted, who = %self.who, "remote frequency intent");
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                Some(Module::Sine) => match self.core.sine.go_to_frequency(hz) {
                    Ok(adopted) => {
                        events.emit(Event::StateChanged {
                            status: Box::new(self.core.status()),
                        });
                        tracing::info!(target: "hertzforge::backendd", hz, adopted, who = %self.who, "remote frequency intent");
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                // WRITTEN rather than inherited from the tail: TWR is the first
                // frequency-BEARING-looking module that is not one, and "send an Explorer
                // profile" is the wrong next step for someone mid-replication.
                Some(m @ Module::Twr) => Response::Rejected {
                    error: module_takes_no(m, "frequency intents", TWR_HAS_NO_FREQUENCY),
                },
                _ => Response::Rejected {
                    error: self.core.no_module_for("frequency"),
                },
            },
            Command::AdjustFrequency { delta_hz } => match self.core.live_module(Claim::Live) {
                Some(Module::Explorer) => match self.core.explorer.adjust_frequency(delta_hz) {
                    Ok(adopted) => {
                        events.emit(Event::StateChanged {
                            status: Box::new(self.core.status()),
                        });
                        tracing::info!(target: "hertzforge::backendd", delta_hz, adopted, who = %self.who, "remote frequency step");
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                Some(Module::Sine) => match self.core.sine.adjust_frequency(delta_hz) {
                    Ok(adopted) => {
                        events.emit(Event::StateChanged {
                            status: Box::new(self.core.status()),
                        });
                        tracing::info!(target: "hertzforge::backendd", delta_hz, adopted, who = %self.who, "remote frequency step");
                        accepted()
                    }
                    Err(error) => Response::Rejected { error },
                },
                Some(m @ Module::Twr) => Response::Rejected {
                    error: module_takes_no(m, "frequency intents", TWR_HAS_NO_FREQUENCY),
                },
                _ => Response::Rejected {
                    error: self.core.no_module_for("frequency"),
                },
            },

            // ---- the Explorer group (v4 wire surface, SERVED) ------------------
            //
            // The sweep verbs are the ones the general verb set cannot express, and
            // they route the same way everything else does: to the live module if it can
            // serve them, otherwise a refusal that describes the core's state. It is no
            // longer `LocalOnly` - that answer meant "this capability lives in the local
            // app", which stopped being the whole truth when the host landed.
            Command::Explorer(e) => match self.core.live_module(Claim::Live) {
                Some(Module::Explorer) => {
                    let out = match e {
                        hertzforge_protocol::ExplorerCommand::StartSweep {
                            direction,
                            rate_oct_per_min,
                        } => self.core.explorer.start_sweep(
                            match direction {
                                hertzforge_protocol::SweepDirection::Up => {
                                    hertzforge_explorer_host::step::SweepDirection::Up
                                }
                                hertzforge_protocol::SweepDirection::Down => {
                                    hertzforge_explorer_host::step::SweepDirection::Down
                                }
                            },
                            rate_oct_per_min,
                        ),
                        hertzforge_protocol::ExplorerCommand::StopSweep => {
                            self.core.explorer.stop_sweep()
                        }
                    };
                    match out {
                        Ok(()) => {
                            events.emit(Event::StateChanged {
                                status: Box::new(self.core.status()),
                            });
                            tracing::info!(target: "hertzforge::backendd", cmd = ?e, who = %self.who, "remote sweep intent");
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
                _ => Response::Rejected {
                    error: self.core.no_module_for("sweep"),
                },
            },

            // ---- the v5 module groups (wire surface open, hosts pending) --------
            //
            // Same posture as the three profile arms above, with ONE exception that is
            // not about hosts at all: the three CAPTURE verbs answer the remote-capture
            // refusal whether or not a host is live, because remote capture is refused
            // BY DESIGN for both shock modules rather than pending on their arrival.
            // Their host landing will not change that answer, so saying "no shock
            // module is live" would be a refusal the operator could reasonably expect
            // to expire, and it would not.
            // Both inner matches are EXHAUSTIVE with no `_` arm, even though every
            // non-capture arm answers the same refusal today: a wildcard here is the
            // shape where a fourth capture-like verb, added by whichever session lands
            // the host, quietly inherits "no shock module is live" and ships as a
            // refusal nobody chose.
            // ---- the Sine group (v5 wire surface, SERVED) ----------------------
            //
            // Routed at `Claim::Configured`, not `Live`, and for the same reason the
            // four level verbs are: `OverrideFeasibility` releases a park the run sits
            // at BEFORE it has armed, so routing on `Live` would make the one verb that
            // frees that park refuse by design. The transport verbs reach a service
            // that answers its own "not engaged (no step)" when nothing is driving,
            // which is the right answer and always was.
            Command::Sine(c) => match self.core.live_module(Claim::Configured) {
                Some(Module::Sine) => {
                    let sine = &self.core.sine;
                    let out = match &c {
                        hertzforge_protocol::SineCommand::Hold => sine.hold(),
                        hertzforge_protocol::SineCommand::Resume => sine.resume(),
                        hertzforge_protocol::SineCommand::Reverse => sine.reverse(),
                        hertzforge_protocol::SineCommand::SetBlendW { w } => sine.set_blend_w(*w),
                        hertzforge_protocol::SineCommand::OverrideFeasibility { ack } => {
                            sine.override_feasibility(ack)
                        }
                    };
                    match out {
                        Ok(()) => {
                            events.emit(Event::StateChanged {
                                status: Box::new(self.core.status()),
                            });
                            tracing::info!(target: "hertzforge::backendd", cmd = ?c, who = %self.who, "remote sine intent");
                            accepted()
                        }
                        Err(error) => Response::Rejected { error },
                    }
                }
                // EXHAUSTIVE, no `_` arm: a module added to this daemon has
                // to state what it does with a sine verb, and "nothing, it is not a
                // sine run" is an answer that must be WRITTEN rather than inherited
                // from whichever arm a wildcard fell into.
                Some(
                    m @ (Module::Random
                    | Module::Explorer
                    | Module::Recorder
                    | Module::Shock
                    | Module::Srs
                    | Module::Twr),
                ) => Response::Rejected {
                    error: module_takes_no(
                        m,
                        "sine transport intents",
                        "the sweep hold / resume / reverse verbs, the feedforward blend \
                         weight and the feasibility override all belong to a closed-loop \
                         SINE run, and this module is not one",
                    ),
                },
                #[cfg(test)]
                Some(m @ Module::Probe) => Response::Rejected {
                    error: module_takes_no(m, "sine transport intents", "it drives nothing"),
                },
                None => Response::Rejected {
                    error: self.core.no_module_for("sine-transport"),
                },
            },
            // ---- the two shock command groups, SERVED ---------------------------
            //
            // The three CAPTURE verbs keep their remote-capture answer, and that is not
            // an oversight now that the hosts exist: remote capture is refused BY
            // DESIGN for shock v1 rather than pending on a host's arrival, so the
            // answer does not change when one lands. A missing-host refusal is one an
            // operator may reasonably expect to expire; this one will not.
            //
            // Both matches stay EXHAUSTIVE with no `_` arm: a wildcard is
            // the shape where a fourth capture-like verb quietly inherits whatever the
            // catch-all happened to answer.
            Command::Shock(c) => match c {
                hertzforge_protocol::ShockCommand::StartCapture
                | hertzforge_protocol::ShockCommand::StopCapture
                | hertzforge_protocol::ShockCommand::SaveSnapshot => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: REMOTE_CAPTURE_NOT_SERVED.into(),
                    },
                },
                // READ-ONLY verbs answer Docs and never touch the service: they read the
                // host's bounded pulse ring and its published grid, so a `PulseList`
                // during a train is answered rather than parked behind the fire.
                hertzforge_protocol::ShockCommand::PulseList => {
                    doc_or_refusal(self.core.shock.pulse_list_doc())
                }
                hertzforge_protocol::ShockCommand::PulseDetail { seq } => {
                    doc_or_refusal(self.core.shock.pulse_detail_doc(seq))
                }
                hertzforge_protocol::ShockCommand::SrsFreqs => {
                    doc_or_refusal(self.core.shock.srs_freqs_doc())
                }
                hertzforge_protocol::ShockCommand::FirePulse => {
                    self.shock_act(events, |h| h.fire_pulse())
                }
                hertzforge_protocol::ShockCommand::RunAuto { max_pulses } => {
                    self.shock_act(events, |h| h.run_auto(max_pulses))
                }
                hertzforge_protocol::ShockCommand::FireTrain { spec } => {
                    self.shock_act(events, |h| h.fire_train(spec))
                }
                hertzforge_protocol::ShockCommand::FireGunfireBurst => {
                    self.shock_act(events, |h| h.fire_gunfire())
                }
                hertzforge_protocol::ShockCommand::JumpToLevel { index } => {
                    self.shock_act(events, |h| h.jump_to_level(index).map(|_| ()))
                }
                hertzforge_protocol::ShockCommand::ArmWithOverride { ack } => {
                    if ack.trim().is_empty() {
                        return Response::Rejected {
                            error: ProtocolError::InvalidArgument {
                                message: EMPTY_ACK.into(),
                            },
                        };
                    }
                    self.shock_act(events, move |h| h.arm(Some(ack)))
                }
                hertzforge_protocol::ShockCommand::Reset => self.shock_act(events, |h| h.reset()),
            },
            Command::Srs(c) => match c {
                hertzforge_protocol::SrsCommand::StartCapture
                | hertzforge_protocol::SrsCommand::StopCapture
                | hertzforge_protocol::SrsCommand::SaveSnapshot => Response::Rejected {
                    error: ProtocolError::Refused {
                        message: REMOTE_CAPTURE_NOT_SERVED.into(),
                    },
                },
                hertzforge_protocol::SrsCommand::PulseList => {
                    doc_or_refusal(self.core.srs.pulse_list_doc())
                }
                hertzforge_protocol::SrsCommand::PulseDetail { seq } => {
                    doc_or_refusal(self.core.srs.pulse_detail_doc(seq))
                }
                hertzforge_protocol::SrsCommand::SrsFreqs => {
                    doc_or_refusal(self.core.srs.srs_freqs_doc())
                }
                // The predictive-notch readout, out of the same kind of lock-free
                // mirror the grid uses. Additive within v5 (2026-08-09): before it, the
                // remote Notch Status card rendered permanently empty because the plan
                // is core state with no `StatusReport` field and no verb to ask for it.
                hertzforge_protocol::SrsCommand::NotchStatus => {
                    doc_or_refusal(self.core.srs.notch_doc())
                }
                hertzforge_protocol::SrsCommand::FirePulse => {
                    self.srs_act(events, |h| h.fire_pulse())
                }
                hertzforge_protocol::SrsCommand::RunAuto { max_pulses } => {
                    self.srs_act(events, |h| h.run_auto(max_pulses))
                }
                hertzforge_protocol::SrsCommand::RunSysId => {
                    self.srs_act(events, |h| h.run_sysid())
                }
                hertzforge_protocol::SrsCommand::JumpToLevel { index } => {
                    self.srs_act(events, |h| h.jump_to_level(index).map(|_| ()))
                }
                hertzforge_protocol::SrsCommand::ArmWithOverride { ack } => {
                    if ack.trim().is_empty() {
                        return Response::Rejected {
                            error: ProtocolError::InvalidArgument {
                                message: EMPTY_ACK.into(),
                            },
                        };
                    }
                    self.srs_act(events, move |h| h.arm(Some(ack)))
                }
                hertzforge_protocol::SrsCommand::Reset => self.srs_act(events, |h| h.reset()),
            },

            // ---- the TWR group, SERVED -----------------------------------------
            //
            // ~~One refusal for all eleven verbs~~ - the host landed, and the arm that
            // listed them is the arm that made the TWR host state each one. It stays
            // EXHAUSTIVE with no `_` arm: a twelfth verb must be routed deliberately,
            // not inherit whichever branch a wildcard fell into.
            //
            // **The group SPLITS in two, and the split is the interesting part.**
            //
            // The five STAGING and QUERY verbs are about the host's own document store,
            // not about who owns the hardware, so they do NOT go through
            // `Core::live_module`. That is not laxity - it is the normal sequence: an
            // operator stages a record on an idle core and only then configures a
            // profile that names it. Routing them on a claim would make the first verb
            // of every TWR session refuse. Their own gate is
            // `TwrHost::require_not_live`, which refuses an upload while a TWR RUN is
            // live: `is_mutating` is the CONTROLLER line and answers who
            // may ask, never when.
            //
            // The six LIFECYCLE verbs DO claim the core, so one sent while another
            // module is driving names that module instead of failing somewhere inside a
            // host it should never have reached.
            //
            // Note what is deliberately NOT here: no remote-capture verbs. Remote
            // capture is refused BY DESIGN on the shock groups and that answer does not
            // expire when a host lands; TWR has no capture verb on the wire at all -
            // its capture block is absent from the profile for the same safety reason,
            // and the daemon FORCES `auto_start = false` when it translates.
            Command::Twr(c) => match c {
                // ---- staging + queries: answered whoever holds the core -------------
                hertzforge_protocol::TwrCommand::BeginReference {
                    name,
                    fs_hz,
                    eu,
                    total_samples,
                    sha256,
                } => match self
                    .core
                    .twr
                    .begin_reference(name, fs_hz, eu, total_samples, sha256)
                {
                    Ok(()) => accepted(),
                    Err(error) => Response::Rejected { error },
                },
                hertzforge_protocol::TwrCommand::ReferenceChunk {
                    seq,
                    samples_b64_f32le,
                } => match self.core.twr.reference_chunk(seq, &samples_b64_f32le) {
                    Ok(()) => accepted(),
                    Err(error) => Response::Rejected { error },
                },
                // The COMMIT answers a DOCUMENT rather than a bare acceptance: a client
                // that just staged a megabyte needs to know WHAT was staged - the sha the
                // CORE computed over the bytes it holds - and "accepted" would leave it
                // believing its own declaration, which is the thing the commit exists to
                // check.
                hertzforge_protocol::TwrCommand::CommitReference {} => {
                    twr_doc(self.core.twr.commit_reference())
                }
                hertzforge_protocol::TwrCommand::ReferenceInfo => {
                    twr_doc(self.core.twr.reference_info_doc())
                }
                hertzforge_protocol::TwrCommand::History => twr_doc(self.core.twr.history_doc()),
                // ---- the lifecycle verbs: routed through the ONE authority ----------
                hertzforge_protocol::TwrCommand::IterateOnce => {
                    self.twr_act(events, |h| h.iterate_once())
                }
                hertzforge_protocol::TwrCommand::BeginPlayback => {
                    self.twr_act(events, |h| h.begin_playback())
                }
                hertzforge_protocol::TwrCommand::StepLevel { up } => {
                    self.twr_act(events, move |h| h.step_level(up).map(|_| ()))
                }
                hertzforge_protocol::TwrCommand::JumpLevel { index } => {
                    self.twr_act(events, move |h| h.jump_level(index).map(|_| ()))
                }
                hertzforge_protocol::TwrCommand::FreezeAdaptation { frozen } => {
                    self.twr_act(events, move |h| h.freeze_adaptation(frozen))
                }
                hertzforge_protocol::TwrCommand::Reset => self.twr_act(events, |h| h.reset()),
            },

            // Subscriptions are handled by the TRANSPORT, which owns the per-peer
            // stream state; reaching the core means the server routed one wrong.
            Command::Subscribe { .. } | Command::Unsubscribe { .. } => Response::Rejected {
                error: ProtocolError::Internal {
                    message: "subscriptions are handled at the transport, not the core".into(),
                },
            },
        }
    }
}

impl CoreSession {
    /// **THE FOUR LEVEL-SCHEDULE VERBS' ONE ROUTER** (v5 shared bundle).
    ///
    /// `SetLevel{Db}` / `AdjustLevel` / `SetLadderMode` / `ProceedAfterReview` were the
    /// last hard-coded `self.core.random.*` calls in the dispatcher. They now ask
    /// [`Core::live_module`] like every other site, at `Claim::Configured` - see the
    /// call sites for why that strength and not `Live`.
    ///
    /// The per-module match is EXHAUSTIVE with no `_` arm. That is the
    /// point of routing them at all: a module added to this daemon has to state what it
    /// does with a level intent, and "nothing, because it commands volts" is an answer
    /// that must be WRITTEN rather than inherited from whichever arm a wildcard fell
    /// into.
    ///
    /// `intent` names the capability for [`Core::no_module_for`] when the core is idle;
    /// `what` names the intent inside a module's refusal; `log` is the journal line on
    /// success.
    ///
    /// # TWO closures, not one
    ///
    /// It took one - `on_random` - while Random was the only module with a dB ladder,
    /// and Sine is the second. They are separate closures rather than one over a shared
    /// trait because the two hosts answer these verbs DIFFERENTLY and the difference is
    /// the interesting part: an off-ladder `SetLevel{Db}` is a REFUSAL from Random
    /// (which prints its ladder) and a NEAREST-RUNG RESOLUTION from Sine (which echoes
    /// the rung it applied). A trait would have to erase that, and erasing it is how
    /// one module's semantics quietly become the other's.
    fn route_level_intent(
        &self,
        intent: &str,
        what: &str,
        log: &str,
        events: &mut dyn EventSink,
        act: LevelIntent,
    ) -> Response {
        let out = match self.core.live_module(Claim::Configured) {
            Some(Module::Random) => match act {
                LevelIntent::SetLevel(level) => self.core.random.set_level(level).map(|_| ()),
                LevelIntent::Adjust(delta_db) => {
                    self.core.random.adjust_level(delta_db).map(|_| ())
                }
                LevelIntent::Ladder(ladder) => self.core.random.set_ladder_mode(ladder),
                LevelIntent::Proceed => self.core.random.proceed_after_review(),
            },
            // ---- THE TWO TRANSIENT PRODUCERS ------------------------------------
            //
            // Written out rather than inherited, which is the whole point of routing
            // these four at all: a module that joins this daemon has to STATE what it
            // does with a level intent, and "nothing, and here is why" is an answer
            // that has to be written.
            Some(m @ Module::Shock) => match act {
                LevelIntent::SetLevel(level) => self.shock_set_level(m, level),
                LevelIntent::Adjust(delta_db) => self.core.shock.adjust_level(delta_db).map(|_| ()),
                LevelIntent::Ladder(_) => Err(module_takes_no(m, what, LADDER_IS_THE_VERB)),
                LevelIntent::Proceed => Err(module_takes_no(
                    m,
                    what,
                    "a classical shock run has no post-sysID review hold because it runs no \
                     system ID - it plays an authored pulse, it does not identify a plant. \
                     Its pre-arm gate is FEASIBILITY, and the verb that releases that one is \
                     Shock(ArmWithOverride) with an acknowledgement",
                )),
            },
            Some(m @ Module::Srs) => match act {
                LevelIntent::SetLevel(level) => self.srs_set_level(m, level),
                LevelIntent::Adjust(delta_db) => self.core.srs.adjust_level(delta_db).map(|_| ()),
                LevelIntent::Ladder(_) => Err(module_takes_no(m, what, LADDER_IS_THE_VERB)),
                LevelIntent::Proceed => self.core.srs.proceed_after_review(),
            },
            // The SECOND closed-loop dB module (SineHost). Every intent the
            // Random arm serves has a Sine analogue; the clamp/nearest-rung
            // resolution lives in the service so the echo, the scheduler and the
            // status readout stay one number.
            Some(Module::Sine) => match act {
                LevelIntent::SetLevel(level) => self.core.sine.set_level(level).map(|_| ()),
                LevelIntent::Adjust(delta_db) => self.core.sine.adjust_level(delta_db),
                LevelIntent::Ladder(ladder) => self.core.sine.set_ladder_mode(ladder),
                LevelIntent::Proceed => self.core.sine.proceed_after_review(),
            },
            // **THE FOURTH CLOSED-LOOP MODULE, and it serves exactly ONE of the four.**
            // Each refusal is WRITTEN rather than inherited, which is the whole point of
            // routing these verbs: a module that joins this daemon has to state what it
            // does with a level intent.
            //
            // The CONSERVATIVE option was taken on `SetLevel{Db}`. Sine
            // resolves an off-ladder dB to the NEAREST RUNG and Random refuses by listing
            // the ladder; TWR does neither - it refuses by name and names its own two
            // verbs. A TWR rung is not a setpoint a servo walks to: stepping it SCALES
            // THE CARRIED DRIVE by the step, with the volts ceiling re-checked against
            // the realized record before anything moves, so a nearest-rung resolution
            // would silently apply a different scale factor from the one the operator
            // asked for. `StepLevel` and `JumpLevel` are the served verbs and both answer
            // with the level actually REACHED.
            Some(m @ Module::Twr) => match act {
                LevelIntent::SetLevel(_) | LevelIntent::Adjust(_) => Err(module_takes_no(
                    m,
                    what,
                    "a TWR level step SCALES THE CARRIED DRIVE by the step and re-checks the \
                     volts ceiling against the realized record before the ladder moves, so \
                     there is no setpoint to jump to and no nearest-rung resolution that would \
                     mean what you asked. Send Twr(StepLevel) or Twr(JumpLevel) - both answer \
                     with the level actually REACHED",
                )),
                LevelIntent::Ladder(_) => Err(module_takes_no(
                    m,
                    what,
                    "a TWR run's AUTO-vs-MANUAL mode is authored in the profile and is NOT \
                     live-switchable, deliberately: it decides whether the ladder advances \
                     itself on the convergence gate, and taking it over mid-run would change \
                     what the next play means while a corrected drive is carried forward. \
                     Author the mode in the profile, where it is reviewable before anything \
                     arms",
                )),
                // SERVED: the post-sysID review park is HOST-side and this is the general
                // verb that releases it. From the COHERENCE hold it refuses with the
                // service's own sentence - see `TwrHost::proceed_after_review`.
                LevelIntent::Proceed => self.core.twr.proceed_after_review(),
            },
            Some(m @ Module::Explorer) => Err(module_takes_no(
                m,
                what,
                "an open-loop survey commands drive VOLTS and measures the response, so \
                 it has no dB ladder to move and no post-sysID hold to release - it arms \
                 at zero drive and the operator raises it. Use SetLevel with a \
                 DriveVolts target",
            )),
            Some(m @ Module::Recorder) => Err(module_takes_no(
                m,
                what,
                "the Recorder is monitor-only and never drives, so there is no level, no \
                 schedule and no review hold to act on",
            )),
            #[cfg(test)]
            Some(m @ Module::Probe) => Err(module_takes_no(
                m,
                what,
                "the test probe module drives nothing",
            )),
            None => Err(self.core.no_module_for(intent)),
        };
        match out {
            Ok(()) => {
                events.emit(Event::StateChanged {
                    status: Box::new(self.core.status()),
                });
                tracing::info!(target: "hertzforge::backendd", who = %self.who, "{log}");
                accepted()
            }
            Err(error) => Response::Rejected { error },
        }
    }

    /// One mutating classical-shock verb: claim the core, act, and push the new status.
    ///
    /// The claim is asked FIRST and through the one authority, so a `FirePulse` sent
    /// while the Explorer is driving names the Explorer instead of failing somewhere
    /// inside a host that never should have been reached.
    fn shock_act(
        &self,
        events: &mut dyn EventSink,
        f: impl FnOnce(&crate::shock::ShockHost) -> Result<(), ProtocolError>,
    ) -> Response {
        if let Err(error) = self.core.claim_core(Module::Shock) {
            return Response::Rejected { error };
        }
        match f(&self.core.shock) {
            Ok(()) => {
                events.emit(Event::StateChanged {
                    status: Box::new(self.core.status()),
                });
                accepted()
            }
            Err(error) => Response::Rejected { error },
        }
    }

    /// [`CoreSession::shock_act`] for the SRS module.
    fn srs_act(
        &self,
        events: &mut dyn EventSink,
        f: impl FnOnce(&crate::srs::SrsHost) -> Result<(), ProtocolError>,
    ) -> Response {
        if let Err(error) = self.core.claim_core(Module::Srs) {
            return Response::Rejected { error };
        }
        match f(&self.core.srs) {
            Ok(()) => {
                events.emit(Event::StateChanged {
                    status: Box::new(self.core.status()),
                });
                accepted()
            }
            Err(error) => Response::Rejected { error },
        }
    }

    /// [`CoreSession::shock_act`] for the TWR module.
    ///
    /// Same shape and the same reason: the claim is asked FIRST and through the one
    /// authority, so a `Twr(IterateOnce)` sent while the Explorer is driving names the
    /// Explorer instead of failing somewhere inside a host that never should have been
    /// reached.
    fn twr_act(
        &self,
        events: &mut dyn EventSink,
        f: impl FnOnce(&crate::twr::TwrHost) -> Result<(), ProtocolError>,
    ) -> Response {
        if let Err(error) = self.core.claim_core(Module::Twr) {
            return Response::Rejected { error };
        }
        match f(&self.core.twr) {
            Ok(()) => {
                events.emit(Event::StateChanged {
                    status: Box::new(self.core.status()),
                });
                accepted()
            }
            Err(error) => Response::Rejected { error },
        }
    }

    /// `SetLevel { Db }` on a shock ladder: JUMP TO THE NEAREST RUNG, and be honest
    /// about it.
    ///
    /// A shock ladder is a TABLE of rungs, not a continuum: `pulses_per_level` is
    /// counted per rung and an arbitrary dB between two of them is not a state the run
    /// can be in. The two alternatives were both worse. Refusing every off-rung value
    /// makes a perfectly ordinary "take me to about -6" fail for want of four decimal
    /// places. Silently adopting the request would put a number on the panel the run is
    /// not at.
    ///
    /// So: nearest rung, and **the echo is the ADOPTED level, never the request** - the
    /// `StateChanged` event this router emits on success carries the status, whose
    /// `level_db` is what the ladder actually moved to, and the journal line carries
    /// both numbers. `DriveVolts` is refused by name: a shock ladder is dB relative to
    /// the authored pulse, and reinterpreting one basis as the other is how a -6
    /// becomes six volts.
    fn shock_set_level(
        &self,
        m: Module,
        level: hertzforge_protocol::LevelTarget,
    ) -> Result<(), ProtocolError> {
        let db = shock_db_target(m, level)?;
        let table = self.core.shock.level_table()?;
        let (idx, rung) = nearest_rung(&table, db, m)?;
        let adopted = self.core.shock.jump_to_level(idx)?;
        tracing::info!(
            target: "hertzforge::backendd",
            requested_db = db,
            adopted_db = adopted,
            rung,
            idx,
            "remote shock level jumped to the NEAREST ladder rung"
        );
        Ok(())
    }

    /// [`CoreSession::shock_set_level`] for the SRS module.
    fn srs_set_level(
        &self,
        m: Module,
        level: hertzforge_protocol::LevelTarget,
    ) -> Result<(), ProtocolError> {
        let db = shock_db_target(m, level)?;
        let table = self.core.srs.level_table()?;
        let (idx, rung) = nearest_rung(&table, db, m)?;
        let adopted = self.core.srs.jump_to_level(idx)?;
        tracing::info!(
            target: "hertzforge::backendd",
            requested_db = db,
            adopted_db = adopted,
            rung,
            idx,
            "remote SRS level jumped to the NEAREST ladder rung"
        );
        Ok(())
    }

    fn recorder(&mut self, cmd: RecorderCommand, events: &mut dyn EventSink) -> Response {
        let rec = &self.core.recorder;
        let out = match cmd {
            RecorderCommand::Configure { config } => {
                // THE OTHER DIRECTION of the mutual exclusion, which the old
                // `recorder_live: bool` could not express: building a recorder
                // station while a Random run is DRIVING would reserve the same
                // hardware twice.
                match self.core.claim_core(Module::Recorder) {
                    Err(error) => Err(error),
                    Ok(()) => {
                        match serde_json::from_str::<hertzforge_recorder::service::RecorderConfig>(
                            &config.json,
                        ) {
                            Ok(cfg) => rec
                                .configure(cfg, self.core.table.snapshot(), &self.who)
                                .map(|()| accepted()),
                            Err(e) => Err(ProtocolError::MalformedPayload {
                                container: "recorder configuration".into(),
                                detail: e.to_string(),
                            }),
                        }
                    }
                }
            }
            RecorderCommand::StartMonitor => self
                .core
                .claim_core(Module::Recorder)
                .and_then(|()| rec.start_monitor())
                .map(|()| {
                    // The STARTING controller learns what its data IS the same way
                    // a mid-run attacher does at connect (server.rs pushes the same
                    // event there). Until 2026-07-30 the event existed ONLY on the
                    // at-connect path, so the one peer guaranteed to care - the one
                    // that started the run - never received it; the old probe's
                    // substring assertion masked the gap.
                    if let Some(p) = rec.provenance() {
                        events.emit(Event::RunProvenance {
                            provenance: Doc::new(SchemaKind::Provenance, p.to_json()),
                        });
                    }
                    accepted()
                }),
            RecorderCommand::Arm => rec.arm().map(|()| accepted()),
            RecorderCommand::RecordNow => rec.record_now().map(|()| accepted()),
            RecorderCommand::StopRecording => rec.stop_recording().map(|()| accepted()),
            RecorderCommand::StopMonitor => rec.stop_monitor().map(|()| accepted()),
            RecorderCommand::Status => Ok(Response::Recorder(Doc::new(
                SchemaKind::RecorderConfig,
                rec.status_json(),
            ))),
            RecorderCommand::Meters => Ok(Response::Recorder(Doc::new(
                SchemaKind::RecorderConfig,
                rec.meters_json(),
            ))),
            RecorderCommand::Frf => Ok(Response::Recorder(Doc::new(
                SchemaKind::RecorderConfig,
                rec.frf_json(),
            ))),
            RecorderCommand::Recordings => Ok(Response::Recorder(Doc::new(
                SchemaKind::RecorderConfig,
                rec.recordings_json(),
            ))),
        };
        match out {
            Ok(r) => r,
            Err(error) => Response::Rejected { error },
        }
    }
}

/// A TWR document verb's answer, or its refusal.
///
/// Separate from [`doc_or_refusal`] because the RESPONSE VARIANT is different, and the
/// variant is what a client decodes: `Response::Shock(Doc)` and `Response::Twr(Doc)` are
/// two shapes and a client reads the answer to the query it asked, never a kind field on
/// the document (see [`hertzforge_protocol::SchemaKind`]'s docs).
fn twr_doc(d: Result<Doc, ProtocolError>) -> Response {
    match d {
        Ok(doc) => Response::Twr(doc),
        Err(error) => Response::Rejected { error },
    }
}

/// A read-only shock verb's answer, or its refusal.
fn doc_or_refusal(d: Result<Doc, ProtocolError>) -> Response {
    match d {
        Ok(doc) => Response::Shock(doc),
        Err(error) => Response::Rejected { error },
    }
}

/// The refusal for an EMPTY feasibility-override acknowledgement.
///
/// A required field nobody has to fill in is not a gate. Checked at the dispatcher as
/// well as inside the service so the refusal is the wire's own sentence rather than a
/// service string an operator has to interpret.
const EMPTY_ACK: &str =
    "ArmWithOverride needs a non-empty acknowledgement. The ack is the operator's signed \
     statement that they read the feasibility reason and are proceeding anyway; an empty \
     one is a gate that gates nothing, so it is refused rather than accepted and logged \
     as a blank.";

/// The dB a shock level intent names, or a named refusal for the wrong basis.
fn shock_db_target(
    m: Module,
    level: hertzforge_protocol::LevelTarget,
) -> Result<f64, ProtocolError> {
    match level {
        hertzforge_protocol::LevelTarget::Db(db) => Ok(db),
        hertzforge_protocol::LevelTarget::DriveVolts(_) => Err(module_takes_no(
            m,
            "drive-VOLTS level",
            "a shock level is dB relative to the AUTHORED pulse (or the authored target \
             spectrum), which is a different quantity from an open-loop volts setpoint. \
             The volts ceiling this run enforces is max_drive_vpk in the profile. Send \
             SetLevel with a Db target",
        )),
    }
}

/// The nearest rung of a shock ladder to `db`, as `(index, level_db)`.
///
/// Ties go to the LOWER rung. An arbitrary tie-break has to be written down somewhere,
/// and "when in doubt, less drive" is the only defensible direction to pick beside a
/// shaker.
fn nearest_rung(table: &[f64], db: f64, m: Module) -> Result<(usize, f64), ProtocolError> {
    if table.is_empty() {
        return Err(module_takes_no(
            m,
            "dB level",
            "this run's level schedule has no rungs, so there is nothing to jump to",
        ));
    }
    if !db.is_finite() {
        return Err(ProtocolError::InvalidArgument {
            message: format!("{db} dB is not a finite level"),
        });
    }
    let mut best = (0usize, table[0]);
    for (i, &rung) in table.iter().enumerate() {
        let d_new = (rung - db).abs();
        let d_best = (best.1 - db).abs();
        if d_new < d_best || ((d_new - d_best).abs() < 1e-12 && rung < best.1) {
            best = (i, rung);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::ChannelTable;
    use hertzforge_protocol::VecSink;

    fn core() -> Arc<Core> {
        let root =
            std::env::temp_dir().join(format!("hertzforge-backendd-core-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let (frames, _) = tokio::sync::broadcast::channel(64);
        Arc::new(Core {
            recorder: Arc::new(RecorderHost::new(
                BackendKind::Sim,
                Vec::new(),
                root.clone(),
                frames.clone(),
            )),
            random: Arc::new(crate::random::RandomHost::new(
                BackendKind::Sim,
                Vec::new(),
                root.clone(),
                frames.clone(),
            )),
            explorer: Arc::new(crate::explorer::ExplorerHost::new(
                BackendKind::Sim,
                Vec::new(),
                frames.clone(),
                // The daemon's own default (`config::default_controller_timeout_s`), so
                // the safe-stop budget these tests configure against is the shipped one.
                30.0,
            )),
            shock: Arc::new(crate::shock::ShockHost::new(
                BackendKind::Sim,
                Vec::new(),
                root.clone(),
                frames.clone(),
                30.0,
            )),
            sine: Arc::new(crate::sine::SineHost::new(
                BackendKind::Sim,
                Vec::new(),
                root.clone(),
                frames.clone(),
                30.0,
            )),
            srs: Arc::new(crate::srs::SrsHost::new(
                BackendKind::Sim,
                Vec::new(),
                root.clone(),
                frames.clone(),
            )),
            twr: Arc::new(crate::twr::TwrHost::new(
                BackendKind::Sim,
                Vec::new(),
                frames.clone(),
                30.0,
            )),
            table: Arc::new(TableStore::new(explorer_ready_table())),
            frames,
            data_root: root,
            kind: BackendKind::Sim,
            probe: ProbeHost::default(),
        })
    }

    /// Three inputs and ONE Drive row.
    ///
    /// The daemon's shipped seed table (`default_with_channels(4)`) has no Drive row at
    /// all, which Random tolerates (it derives an output regardless) and the Explorer
    /// REFUSES by name: an open-loop survey commands volts onto a specific output, and
    /// "whichever AO the factory picks" is not a thing an operator standing next to a
    /// shaker should have to infer. So the fixture states the row, exactly as an
    /// operator (or the wire probe) has to `PutChannelTable` one before configuring.
    fn explorer_ready_table() -> ChannelTable {
        let mut t = ChannelTable::default_with_channels(4);
        t.channels[3].role = hertzforge_channel_table::ChannelRole::Drive;
        t.channels[3].units = hertzforge_channel_table::Units::VoltageV;
        t
    }

    fn session() -> CoreSession {
        CoreSession::new(core(), 1, "10.0.0.2:5000".into())
    }

    fn run(s: &mut CoreSession, c: Command) -> Response {
        let mut sink = VecSink::default();
        s.handle(c, &mut sink)
    }

    fn sine() -> hertzforge_protocol::SineProfile {
        hertzforge_protocol::SineProfile {
            schema_version: hertzforge_protocol::PROFILE_SCHEMA,
            start_hz: 100.0,
            band_hz: None,
            max_drive_v: None,
        }
    }

    /// A minimal but VALID v5 sine profile. The daemon refuses it for want of a host,
    /// not for want of content - which is the distinction the test using it asserts,
    /// so the payload has to be one a real host would accept.
    fn sine_wire() -> hertzforge_protocol::SineWireProfile {
        use hertzforge_protocol::*;
        SineWireProfile {
            schema_version: PROFILE_SCHEMA,
            profile: SineControlProfile {
                label: "core-test".into(),
                rows: vec![
                    SineProfileRow {
                        freq_hz: 20.0,
                        quantity: SineQuantity::AccelGPk,
                        value: 1.0,
                        alarm_db: None,
                        abort_db: None,
                    },
                    SineProfileRow {
                        freq_hz: 2000.0,
                        quantity: SineQuantity::AccelGPk,
                        value: 1.0,
                        alarm_db: None,
                        abort_db: None,
                    },
                ],
                global_alarm_db: (1.5, 1.5),
                global_abort_db: (3.0, 3.0),
            },
            mode: SineModeSpec::Sweep(SineSweepSpec {
                band_hz: (20.0, 2000.0),
                rate: SineSweepRate::LogOctPerMin(1.0),
                start_hz: 20.0,
                initial_direction: SweepDirection::Up,
                target: SineSweepTarget::Sweeps(1),
            }),
            control: SineControlSet {
                channels: vec![SineControlChannel {
                    index: 1,
                    weight: 1.0,
                }],
                mode: SineCompositeMode::WeightedAvg,
            },
            limit_profiles: Vec::new(),
            blend_w: 0.5,
            compression: SineCompression::Slow,
            tracking_bw: SineBandwidthMode::FixedHz(10.0),
            feedforward: SineFeedforwardConfig {
                coherence_floor: 0.5,
                h_floor_frac: 0.05,
            },
            notch: SineNotchConfig {
                drive_floor: 0.01,
                smoothing_ticks: 3.0,
                release_ramp_db_per_s: 20.0,
            },
            ladder: SineLadderConfig {
                sysid_level_db: -20.0,
                start_db: -12.0,
                table_db: vec![-12.0, 0.0],
                per_rung: SineRungFor::Sweeps(1),
                full_for: SineRungFor::Sweeps(1),
            },
            sysid_level_db: -20.0,
            coherence_floor: 0.5,
            drive_vpk_ceiling: 1.0,
            drive_vpk_abort: 2.0,
            fault_debounce_ticks: 5,
            kinematic_monitor: false,
            capture: SineCaptureConfig {
                rule: SineCaptureRule::Off,
                level_settle_s: 1.0,
                periodic_save_s: None,
            },
            cola: SineColaConfig {
                enabled: false,
                volts_pk: 1.0,
                waveform: SineColaWaveform::Sine,
            },
            feasibility: None,
            control_tick_frames: 64,
            fs_hz: None,
            review_hold: true,
            metadata: Vec::new(),
        }
    }

    /// A minimal but VALID v5 classical-shock profile. See [`sine_wire`].
    fn classical() -> hertzforge_protocol::ClassicalShockProfile {
        use hertzforge_protocol::*;
        ClassicalShockProfile {
            schema_version: PROFILE_SCHEMA,
            spec: ShockPulseSpec {
                shape: ShockPulseShape::HalfSine,
                amplitude_g: 0.4, // g==V on the wire; inside the 5 V default ceiling honestly (the old 20.0 rode the fictional sensitivity conversion the ceiling no longer performs)
                duration_ms: 11.0,
                compensation: ShockCompensation {
                    shape: ShockCompensationShape::HalfSine,
                    pre_frac: 1.5,
                    post_frac: 1.5,
                    symmetric: true,
                },
            },
            fs_hz: 5120.0,
            standard: ShockToleranceStandard::Mil810_516,
            schedule: shock_schedule(),
            train: ShockTrainSpec {
                count: 1,
                spacing: ShockSpacing::Delay(0.0),
            },
            gunfire: None,
            rate_model: ShockRateModel {
                cooling_floor_s: 1.0,
                settling_floor_s: 0.5,
                compute_floor_s: 0.25,
                device_recompute_floor_s: 20.0,
                read_cadence_ok: true,
            },
            drive_limit: ShockDriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            feasibility: None,
            residual_velocity_tol_ips: 1.0,
            residual_disp_tol_in: 0.1,
            srs_f_lo_hz: 20.0,
            srs_f_hi_hz: 2000.0,
            srs_bands_per_octave: 3.0,
            srs_band_tol_db: 3.0,
            compute_pvss: false,
            abort_on_tolerance_failure: true,
            max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
            metadata: Vec::new(),
        }
    }

    /// A minimal but VALID v5 SRS profile. See [`sine_wire`].
    fn srs() -> hertzforge_protocol::SrsProfile {
        use hertzforge_protocol::*;
        SrsProfile {
            schema_version: PROFILE_SCHEMA,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            fraction: SrsBandFraction::Third,
            // 40 g against the seed table's 100 mV/EU Drive row is 4.0 Vpk implied at
            // the top rung, inside the 5.0 Vpk default ceiling. It was 100 g, which the
            // host's ceiling check correctly REFUSED at 10 Vpk - the fixture was asking
            // for a run this daemon will not do.
            target: SrsTarget::Flat { accel_g: 0.2 }, // g==V on the wire; honest vs the 5 V ceiling
            q: 10.0,
            engine: SrsEngineKind::Wavsyn,
            synthesis: SrsSynthesisOpts {
                max_iters: 50,
                tol_db: 1.5,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
            schedule: shock_schedule(),
            limits: None,
            feasibility: None,
            feasibility_tol: SrsFeasibilityTol {
                residual_velocity_ips: 1.0,
                residual_disp_in: 0.1,
            },
            drive_limit: ShockDriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            drive_abort_factor: 1.5,
            kinematic_abort: SrsKinematicAbort {
                accel_g: None,
                velocity_ips: None,
                disp_pp_in: None,
            },
            envelope: SrsEnvelopeSpec {
                alarm_db: (3.0, 3.0),
                abort_db: (6.0, 6.0),
                per_band: None,
            },
            sysid: None,
            compute_pvss: false,
            ring_out_s: 0.5,
            review_after_sysid: true,
            max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
            metadata: Vec::new(),
        }
    }

    fn shock_schedule() -> hertzforge_protocol::ShockLevelSchedule {
        hertzforge_protocol::ShockLevelSchedule {
            mode: hertzforge_protocol::ShockRunMode::Auto,
            start_db: -12.0,
            table_db: vec![-12.0, 0.0],
            pulses_per_level: vec![1],
        }
    }

    /// A minimal-but-valid v6 TWR profile.
    ///
    /// It names a reference sha nothing has staged, deliberately: the arm under test
    /// refuses for want of a HOST, and it must do so before any question about the
    /// staged record can be reached. A fixture that staged one first would make the two
    /// failure modes indistinguishable.
    fn twr_wire() -> hertzforge_protocol::TwrProfile {
        hertzforge_protocol::TwrProfile {
            schema_version: hertzforge_protocol::PROFILE_SCHEMA,
            reference_sha: "deadbeef".into(),
            prep: hertzforge_protocol::TwrPrep {
                auto_resample: true,
                detrend: Some(false),
                highpass_hz: Some(5.0),
                scale: 1.0,
                taper_frac: Some(0.02),
            },
            section: None,
            pad_pre: 512,
            pad_post: 1024,
            sysid: hertzforge_protocol::TwrSysIdSpec {
                excitation: hertzforge_protocol::TwrExcitation::Random,
                drive_rms: 0.05,
                duration_s: 4.0,
                nfft: 2048,
                f_lo_hz: 20.0,
                f_hi_hz: 2000.0,
                min_mean_coherence: 0.8,
            },
            inverse: hertzforge_protocol::TwrInverseSpec {
                tikhonov_beta_floor: 0.05,
                coherence_floor: 0.9,
            },
            law: hertzforge_protocol::TwrLawSpec {
                beta: 0.5,
                max_db_per_iter: 6.0,
                ref_floor_db: 50.0,
                f_lo_hz: 20.0,
                f_hi_hz: 2000.0,
                initial_drive_backoff_db: 0.0,
                min_masked_energy_frac: 0.9,
            },
            schedule: hertzforge_protocol::TwrLevelSchedule {
                mode: hertzforge_protocol::TwrRunMode::Manual,
                table_db: vec![-12.0, -6.0, 0.0],
                gate_rms_error_pct: 10.0,
                max_iterations_per_level: 8,
                on_gate_fail: hertzforge_protocol::TwrGateFailPolicy::Abort,
            },
            run: hertzforge_protocol::TwrRunSchedule::Once,
            adapt_during_playback: false,
            limits: hertzforge_protocol::TwrLimitsSpec {
                envelope_alarm_frac: 0.25,
                envelope_abort_frac: 0.5,
                envelope_debounce_samples: 8,
                response_rms_limit: None,
                drive_clip: None,
                drive_rms_limit: None,
                // The ceiling is the AUTHORIZATION: the fixture carries one because a
                // cleared one is a different test (and a different refusal).
                max_drive_vpk: Some(hertzforge_protocol::DEFAULT_TWR_MAX_DRIVE_VPK),
                divergence_n: 3,
                divergence_tol_frac: 0.10,
                feasibility: hertzforge_protocol::TwrFeasibilityCeilings::default(),
            },
            kinematics_hp_hz: 5.0,
            review_after_sysid: true,
            metadata: Vec::new(),
        }
    }

    fn explorer() -> hertzforge_protocol::ExplorerProfile {
        hertzforge_protocol::ExplorerProfile {
            schema_version: hertzforge_protocol::PROFILE_SCHEMA,
            fs_hz: 8192.0,
            band_hz: (20.0, 2000.0),
            start_hz: 100.0,
            // ARMED QUIET is the Explorer's posture; the test carries it so a
            // future change to the default is visible here.
            amplitude_vpk: 0.0,
            max_drive_vpk: 1.0,
            slew: hertzforge_protocol::ExplorerSlew {
                freq_oct_per_s: 1.0,
                drive_db_per_s: 6.0,
            },
            sweep: None,
            display_nfft: 4096,
        }
    }

    /// THE LOCAL-ONLY RULE, over every still-uncovered command. Each refusal NAMES
    /// what is not served, so the front end renders a state instead of a generic
    /// error - and none of them is an acceptance. Random's verbs (SetProfile / Start
    /// / SetLevel / AdjustLevel) left this list when the RandomHost landed.
    ///
    /// The frequency intents LEFT it at v4: they are routed through `Core::live_module`
    /// and answer a `Refused` that describes the core's state - see
    /// `frequency_intents_route_through_the_one_authority`. The Explorer PROFILE and
    /// the sweep verbs LEFT it when the Explorer host landed.
    ///
    /// **The delegated open-loop sine profile is what remains**, and it is genuinely
    /// local-only: that path drives a delegated controller from the desktop app,
    /// and no daemon hosts it.
    ///
    /// # What is NOT in this list, and why each one is somewhere else
    ///
    /// The list is one entry long and the interesting content is the absences, so they
    /// are enumerated rather than left to be inferred from an empty result:
    ///
    /// * **The CLOSED-LOOP SINE profile and the Sine command group** never belonged
    ///   here (they were never `LocalOnly` - the capability does not live in the
    ///   desktop app) and they are now SERVED. Their posture is asserted by
    ///   `the_sine_profile_is_served_and_its_group_routes` and
    ///   `the_four_level_verbs_reach_sine_and_never_random_when_sine_holds_the_core`.
    /// * **Shock and SRS** answer the MISSING-HOST refusal, which is a different thing
    ///   again: the wire shape is accepted and the host is pending. See
    ///   `the_v5_surfaces_are_accepted_on_the_wire_and_refused_by_a_named_missing_host`.
    /// * **The six capture verbs** answer the remote-capture text, a refusal that does
    ///   NOT expire when those hosts land. See
    ///   `the_capture_verbs_are_present_and_refuse_with_the_remote_capture_text`.
    ///
    /// Four distinct answers to "this core will not do that", each with its own test,
    /// because collapsing any two of them would tell an operator to wait for something
    /// that is not coming - or to stop waiting for something that is.
    #[test]
    fn every_uncovered_command_is_a_named_local_only_refusal_never_an_ack() {
        let mut s = session();
        let uncovered = [Command::SetProfile {
            profile: hertzforge_protocol::TestProfile::OpenLoopSine(sine()),
        }];
        for c in uncovered {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::LocalOnly { what },
                } => {
                    assert!(!what.is_empty(), "{c:?} refused without naming what");
                }
                other => panic!("{c:?} must be a named LocalOnly refusal, got {other:?}"),
            }
        }
    }

    // `the_v5_surfaces_..._missing_host` RETIRED at the SineHost x ShockHost merge
    // (2026-08-08): every v5 module family now has a landed host, so the missing-host
    // list is EMPTY - each parent had already replaced its own rows with a SERVED test
    // (the shock/srs group tests and `the_sine_profile_is_served_and_its_group_routes`),
    // which carry the other side of the same contract. The not_served_yet refusal
    // helper remains for the NEXT module family (Insight is inbound).
    //
    // The next family turned out to be TWR, and its rows are below.

    /// **THE TWR RED TEST: the v6 TWR surface is SERVED, and no verb answers MISSING
    /// HOST any more.**
    ///
    /// ~~`the_v6_twr_surface_is_accepted_at_the_schema_and_refused_by_missing_host`~~ -
    /// that test pinned the pre-host posture for the profile arm and all eleven verbs,
    /// and it was the in-tree half of a two-file contract with
    /// `tools/probes/twr_wire_probe.py`. The host landed, so BOTH halves changed in ONE
    /// commit: the probe's twelve missing-host rows became acceptance rows, and this is
    /// what replaces the assertion here.
    ///
    /// What it pins, in the order the failures matter:
    ///
    /// * **Nothing says `MISSING HOST`.** That phrase tells a front end to render "not
    ///   built yet", and a UI still rendering it would tell the operator to wait for a
    ///   module that is already here.
    /// * **The five STAGING and QUERY verbs answer on an IDLE core.** They are about the
    ///   host's document store, not about who owns the hardware, and routing them on a
    ///   claim would make the first verb of every TWR session refuse.
    /// * **The six LIFECYCLE verbs answer the HOST's own state**, naming `SetProfile` as
    ///   the next command rather than an absent module.
    ///
    /// RED-MUTATION: restore either `not_served_yet(..)` arm and the first property reds
    /// on that verb.
    #[test]
    fn the_v6_twr_surface_is_served_and_no_verb_answers_missing_host() {
        let mut s = session();
        let mut cmds = vec![Command::SetProfile {
            profile: hertzforge_protocol::TestProfile::Twr(Box::new(twr_wire())),
        }];
        cmds.extend(
            [
                hertzforge_protocol::TwrCommand::IterateOnce,
                hertzforge_protocol::TwrCommand::BeginPlayback,
                hertzforge_protocol::TwrCommand::StepLevel { up: true },
                hertzforge_protocol::TwrCommand::JumpLevel { index: 0 },
                hertzforge_protocol::TwrCommand::FreezeAdaptation { frozen: true },
                hertzforge_protocol::TwrCommand::Reset,
                hertzforge_protocol::TwrCommand::BeginReference {
                    name: "r.csv".into(),
                    fs_hz: 8192.0,
                    eu: "g".into(),
                    total_samples: 4,
                    sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
                        .into(),
                },
                hertzforge_protocol::TwrCommand::ReferenceChunk {
                    seq: 0,
                    samples_b64_f32le: "AACAPw==".into(),
                },
                hertzforge_protocol::TwrCommand::CommitReference {},
                hertzforge_protocol::TwrCommand::ReferenceInfo,
                hertzforge_protocol::TwrCommand::History,
            ]
            .map(Command::Twr),
        );
        assert_eq!(
            cmds.len(),
            12,
            "one profile arm plus every verb in the group"
        );

        for c in cmds {
            let answer = run(&mut s, c.clone());
            if let Response::Rejected {
                error: ProtocolError::Refused { message },
            } = &answer
            {
                assert!(
                    !message.contains("MISSING HOST"),
                    "{c:?} still answers MISSING HOST after the host landed, which tells the \
                     operator to wait for a module that is already here: {message}"
                );
            }
            assert!(
                !matches!(
                    answer,
                    Response::Rejected {
                        error: ProtocolError::LocalOnly { .. }
                    }
                ),
                "{c:?} must never be LocalOnly - the capability lives HERE"
            );
        }

        // ---- the STAGING + QUERY verbs answer on an IDLE core --------------------
        let mut s = session();
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::Twr(hertzforge_protocol::TwrCommand::BeginReference {
                        name: "r.csv".into(),
                        fs_hz: 8192.0,
                        eu: "g".into(),
                        total_samples: 1,
                        sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
                            .into(),
                    })
                ),
                Response::Accepted { .. }
            ),
            "an upload BEGINS on an idle core - that is the normal sequence"
        );
        // ...and a query with nothing staged names what to send, rather than an absent
        // module.
        match run(
            &mut s,
            Command::Twr(hertzforge_protocol::TwrCommand::ReferenceInfo),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(message.contains("BeginReference"), "{message}"),
            other => panic!("ReferenceInfo with nothing staged: {other:?}"),
        }

        // ---- a LIFECYCLE verb on an unconfigured core names SetProfile -----------
        match run(
            &mut s,
            Command::Twr(hertzforge_protocol::TwrCommand::IterateOnce),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(message.contains("SetProfile"), "{message}");
                assert!(!message.contains("MISSING HOST"), "{message}");
            }
            other => panic!("a TWR lifecycle verb on an idle core: {other:?}"),
        }
    }

    /// The TWR profile CLEARS the schema gate before anything acts on it - which is what
    /// makes an unstaged-reference refusal a statement about the RECORD rather than about
    /// the payload.
    ///
    /// Without this row the test above would pass just as happily against a daemon that
    /// rejected the profile for being unparseable, and the two failures need very
    /// different actions from an operator.
    #[test]
    fn the_twr_profile_clears_the_schema_gate_before_the_host_sees_it() {
        let good = Command::SetProfile {
            profile: hertzforge_protocol::TestProfile::Twr(Box::new(twr_wire())),
        };
        assert!(good.check_schemas().is_ok());

        // ANTI-VACUITY: a profile from the future is refused by the SCHEMA gate, with a
        // different error type, so the pass above is about the gate working rather than
        // about it having been removed.
        let mut future = twr_wire();
        future.schema_version = hertzforge_protocol::PROFILE_SCHEMA + 1;
        assert!(matches!(
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Twr(Box::new(future)),
            }
            .check_schemas(),
            Err(ProtocolError::UnsupportedSchema { .. })
        ));
    }

    /// **A PROFILE NAMING AN UNSTAGED REFERENCE IS REFUSED BY NAME, quoting the sha.**
    ///
    /// The load-bearing half of `reference_sha`: a profile is NEVER run against
    /// whatever happens to be loaded. A filename would not do - a name can be reused, and
    /// a run whose reference silently changed between the review and the arm is exactly
    /// what the digest makes impossible.
    ///
    /// RED-MUTATION: drop the sha comparison in `TwrHost::set_profile` and this reds on
    /// the acceptance, which is the state where a 60 s field trace gets replaced under an
    /// operator who thought they were re-running yesterday's test.
    #[test]
    fn a_twr_profile_naming_an_unstaged_reference_is_refused_and_quotes_the_sha() {
        let mut s = session();
        match run(
            &mut s,
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Twr(Box::new(twr_wire())),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains("no reference record is staged"),
                    "{message}"
                );
                assert!(
                    message.contains("deadbeef"),
                    "the refusal must quote the sha the profile named: {message}"
                );
                assert!(message.contains("BeginReference"), "{message}");
            }
            other => panic!("a profile with no staged record: {other:?}"),
        }
        assert_eq!(
            s.core().live_module(Claim::Configured),
            None,
            "a refused configure must leave the core UNHELD"
        );
    }

    /// **THE REMOTE-CAPTURE REFUSAL IS THE ANSWER TO ALL SIX CAPTURE VERBS**, on both
    /// shock groups, and it is a DIFFERENT answer from the missing-host one.
    ///
    /// The distinction is load-bearing and is why these do not share an arm: a
    /// missing-host refusal is one an operator may reasonably expect to expire when the
    /// module lands, and this one will not - remote capture is refused by design for
    /// shock v1 whether or not a host exists. Telling the operator otherwise would have
    /// them waiting for a capability nobody is building this cycle.
    ///
    /// The wire probe asserts this string, so it is a contract; this test is the
    /// in-tree half of that pin.
    #[test]
    fn the_capture_verbs_are_present_and_refuse_with_the_remote_capture_text() {
        let mut s = session();
        for c in [
            Command::Shock(hertzforge_protocol::ShockCommand::StartCapture),
            Command::Shock(hertzforge_protocol::ShockCommand::StopCapture),
            Command::Shock(hertzforge_protocol::ShockCommand::SaveSnapshot),
            Command::Srs(hertzforge_protocol::SrsCommand::StartCapture),
            Command::Srs(hertzforge_protocol::SrsCommand::StopCapture),
            Command::Srs(hertzforge_protocol::SrsCommand::SaveSnapshot),
        ] {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(
                        message.starts_with("remote capture is not served over this seam"),
                        "{c:?}: {message}"
                    );
                    assert_eq!(
                        message, REMOTE_CAPTURE_NOT_SERVED,
                        "{c:?} must carry the remote-capture text VERBATIM - the wire probe \
                         asserts this string"
                    );
                    // ... and NOT the missing-host sentence, which would imply the
                    // refusal expires when the shock host lands. It does not.
                    assert!(
                        !message.contains("MISSING HOST"),
                        "{c:?} must not read as a pending host: {message}"
                    );
                }
                other => panic!("{c:?} must be the remote-capture refusal, got {other:?}"),
            }
        }
    }

    /// **THE SHOCK RED TEST: the two shock surfaces are SERVED, and a shock verb with
    /// nothing configured names the STATE rather than the absent module.**
    ///
    /// ~~`the_v5_surfaces_..._refused_by_a_named_missing_host` covering these~~ - that
    /// test pinned the pre-host posture for all three v5 surfaces and two of them left
    /// it here. What replaces it pins both halves of the new posture:
    ///
    /// * a classical-shock profile and an SRS profile are ACCEPTED (a real station is
    ///   built on HERTZFORGE-SIM and a real service configured on it), and the core then
    ///   HOLDS at `Claim::Configured` with a provenance to prove which module it is;
    /// * a shock verb against an idle core answers a refusal that says what to SEND
    ///   (`SetProfile`), never "MISSING HOST", which would tell an operator to wait for
    ///   a module that is already here.
    ///
    /// RED-MUTATION: restore either `not_served_yet(..)` arm and this reds on the first
    /// acceptance.
    #[test]
    fn the_shock_surfaces_are_served_and_a_shock_verb_with_nothing_live_names_the_state() {
        let mut s = session();

        // ---- ANTI-VACUITY FIRST: nothing configured -> an ACTIONABLE refusal -------
        match run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::FirePulse),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(message.contains("SetProfile"), "{message}");
                assert!(
                    !message.contains("MISSING HOST"),
                    "the host is here; a missing-host refusal would tell the operator to \
                     wait for something that already landed: {message}"
                );
            }
            other => panic!("a shock verb on an idle core: {other:?}"),
        }

        // ---- the CLASSICAL profile is served --------------------------------------
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetProfile {
                        profile: hertzforge_protocol::TestProfile::ShockClassical(Box::new(
                            classical()
                        )),
                    }
                ),
                Response::Accepted { .. }
            ),
            "the classical Shock profile is served now"
        );
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Shock),
            "an accepted profile means a built station this core holds"
        );
        let p = s
            .core()
            .module_provenance(Module::Shock)
            .expect("a configured shock run has a provenance");
        assert_eq!(p.module, "shock.spectrum"); // the UI-registered feed name (runProvenance.ts)

        // The read-only verbs answer DOCS, and they answer them without the service:
        // `SrsFreqs` is the frequency axis a late-joining observer needs, and
        // `PulseList` is an empty table rather than a refusal on a run that has fired
        // nothing (an empty list is a measurement).
        match run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::SrsFreqs),
        ) {
            Response::Shock(d) => {
                assert_eq!(d.schema_version, hertzforge_protocol::SHOCK_SRS_GRID_SCHEMA);
                assert!(d.json.contains("\"axis\":\"ordinal\""), "{}", d.json);
                assert!(d.json.contains("freqs_hz"), "{}", d.json);
            }
            other => panic!("SrsFreqs: {other:?}"),
        }
        match run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::PulseList),
        ) {
            Response::Shock(d) => {
                assert_eq!(d.schema_version, hertzforge_protocol::SHOCK_PULSE_SCHEMA);
                assert!(d.json.contains("\"rows\":[]"), "{}", d.json);
                assert!(d.json.contains("\"dropped\":0"), "{}", d.json);
            }
            other => panic!("PulseList: {other:?}"),
        }
        // A detail for a pulse that never fired NAMES both bounds rather than
        // answering an empty document.
        match run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::PulseDetail { seq: 99 }),
        ) {
            Response::Rejected {
                error: ProtocolError::InvalidArgument { message },
            } => assert!(message.contains("pulse ring"), "{message}"),
            other => panic!("PulseDetail for an unfired pulse: {other:?}"),
        }

        // ---- and the SRS profile, on a core the shock module has stood down from ---
        run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::Reset),
        );
        let mut s = session();
        let r = run(
            &mut s,
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Srs(Box::new(srs())),
            },
        );
        assert!(
            matches!(r, Response::Accepted { .. }),
            "the SRS profile is served now: {r:?}"
        );
        assert_eq!(s.core().live_module(Claim::Configured), Some(Module::Srs),);
        assert_eq!(
            s.core()
                .module_provenance(Module::Srs)
                .expect("a configured SRS run has a provenance")
                .module,
            "shock.srs" // UI-registered feed name
        );
    }

    /// **EMPTY ACKS ARE REFUSED ON BOTH GROUPS.** A required field nobody has to fill
    /// in is not a gate, and the refusal is the WIRE's own sentence rather than a
    /// service string an operator has to interpret.
    ///
    /// Anti-vacuity: the same verb with a real ack gets past this check and fails (or
    /// passes) on the run's own state instead, so the refusal is about the ack.
    #[test]
    fn an_empty_override_ack_is_refused_on_both_shock_groups() {
        let mut s = session();
        for c in [
            Command::Shock(hertzforge_protocol::ShockCommand::ArmWithOverride { ack: "  ".into() }),
            Command::Srs(hertzforge_protocol::SrsCommand::ArmWithOverride { ack: String::new() }),
        ] {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::InvalidArgument { message },
                } => assert!(message.contains("non-empty acknowledgement"), "{message}"),
                other => panic!("{c:?}: {other:?}"),
            }
        }
        // ANTI-VACUITY: a real ack reaches the HOST, which refuses for a different
        // reason (nothing is configured) - so the check above is the ack's, not a
        // blanket refusal of the verb.
        match run(
            &mut s,
            Command::Shock(hertzforge_protocol::ShockCommand::ArmWithOverride {
                ack: "reviewed, proceeding".into(),
            }),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(message.contains("SetProfile"), "{message}"),
            other => panic!("a real ack on an unconfigured core: {other:?}"),
        }
    }

    /// **THE FOUR ROUTED VERBS REACH THE SHOCK MODULES AND EACH ANSWER IS WRITTEN.**
    ///
    /// `SetLadderMode` and `ProceedAfterReview` are the interesting two, because a
    /// wildcard arm would have made them inherit somebody else's sentence. Both name
    /// the OCCUPANT, say WHY, and point at the verb that actually does what the
    /// operator wanted - `FirePulse`/`RunAuto` for the ladder, `ArmWithOverride` for the
    /// classical gate.
    #[test]
    fn the_routed_verbs_reach_the_shock_modules_with_written_answers() {
        let mut s = session();
        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::ShockClassical(
                        Box::new(classical())
                    ),
                }
            ),
            Response::Accepted { .. }
        ));
        // SetLadderMode: there is no mode to take over; the VERB is the mode.
        match run(
            &mut s,
            Command::SetLadderMode {
                ladder: hertzforge_protocol::LadderMode::Manual,
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(message.contains(Module::Shock.label()), "{message}");
                assert!(message.contains("FirePulse"), "{message}");
                assert!(message.contains("RunAuto"), "{message}");
            }
            other => panic!("SetLadderMode on a shock run: {other:?}"),
        }
        // ProceedAfterReview: classical shock runs no system ID, so it has no hold -
        // and the refusal names the gate it DOES have.
        match run(&mut s, Command::ProceedAfterReview) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(message.contains(Module::Shock.label()), "{message}");
                assert!(message.contains("ArmWithOverride"), "{message}");
            }
            other => panic!("ProceedAfterReview on a classical run: {other:?}"),
        }
        // A drive-VOLTS level is the wrong basis and is refused by name rather than
        // reinterpreted.
        match run(
            &mut s,
            Command::SetLevel {
                level: hertzforge_protocol::LevelTarget::DriveVolts(0.5),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(message.contains("max_drive_vpk"), "{message}"),
            other => panic!("a volts level on a shock ladder: {other:?}"),
        }
    }

    /// THE NEAREST-RUNG RULE, including its stated tie-break.
    ///
    /// A shock ladder is a table of rungs and a `SetLevel{Db}` between two of them has
    /// to land on one. Ties go to the LOWER rung: an arbitrary tie-break has to be
    /// written down somewhere, and "when in doubt, less drive" is the only defensible
    /// direction beside a shaker.
    #[test]
    fn the_nearest_rung_rule_rounds_to_the_table_and_ties_go_down() {
        let table = [-12.0, -6.0, 0.0];
        assert_eq!(
            nearest_rung(&table, -11.9, Module::Shock).unwrap(),
            (0, -12.0)
        );
        assert_eq!(
            nearest_rung(&table, -5.0, Module::Shock).unwrap(),
            (1, -6.0)
        );
        assert_eq!(nearest_rung(&table, 99.0, Module::Shock).unwrap(), (2, 0.0));
        // Exactly between -12 and -6.
        assert_eq!(
            nearest_rung(&table, -9.0, Module::Shock).unwrap(),
            (0, -12.0),
            "a tie goes DOWN - less drive"
        );
        assert!(nearest_rung(&table, f64::NAN, Module::Shock).is_err());
        assert!(nearest_rung(&[], -6.0, Module::Shock).is_err());
    }

    /// The v4 Explorer surface is SERVED.
    ///
    /// ~~`the_explorer_refusal_says_the_host_lands_after_its_chassis_gate`~~ - that test
    /// pinned the pre-host posture and its subject no longer exists. What replaces it
    /// pins the two halves of the new one: the profile is ACCEPTED (a real station is
    /// built and a real service configured on it), and a sweep verb with nothing live
    /// answers a refusal that describes the core's STATE rather than claiming the verb
    /// is unsupported here.
    ///
    /// RED-MUTATION: restore `local_only(EXPLORER_LOCAL_ONLY)` on either arm and both
    /// halves red.
    #[test]
    fn the_explorer_profile_is_served_and_a_sweep_with_nothing_live_names_the_state() {
        let mut s = session();
        match run(
            &mut s,
            Command::Explorer(hertzforge_protocol::ExplorerCommand::StopSweep),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains("no sweep-bearing module is live on this core"),
                    "{message}"
                );
                // And it says how to GET one, rather than where else to go.
                assert!(message.contains("SetProfile"), "{message}");
            }
            other => panic!("a sweep with nothing live: {other:?}"),
        }
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetProfile {
                        profile: hertzforge_protocol::TestProfile::Explorer(explorer()),
                    }
                ),
                Response::Accepted { .. }
            ),
            "the Explorer profile is served now"
        );
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Explorer),
            "an accepted profile means a built station this core holds"
        );
        let p = s
            .core()
            .module_provenance(Module::Explorer)
            .expect("a configured Explorer has a provenance");
        assert_eq!(p.module, "explorer");
    }

    /// FREQUENCY AND DRIVE-VOLTS INTENTS ARE ROUTED, not blanket-refused (v4).
    ///
    /// With an idle core the refusal names the STATE ("no frequency-bearing module is
    /// live on this core"), not the verb. That distinction is the whole point of the
    /// routing rule: the same command becomes servable the moment a frequency-bearing
    /// module is live, so the refusal must describe what is missing rather than assert
    /// that the capability does not exist here.
    ///
    /// RED-MUTATION: answer these from a hard-coded `local_only(...)` again and the
    /// "is live on this core" assertion reds.
    ///
    /// **The tail is CAPABILITY-AWARE as of the v5 bundle**, and the last block below
    /// is what pins that. The tail used to be one hard-coded sentence naming the
    /// Explorer, which was right while the Explorer was the only routed module and
    /// became actively misleading when the four level verbs joined: a `SetLevel{Db}`
    /// with nothing live told the operator to send an Explorer profile, and the
    /// Explorer is exactly the module that cannot serve a dB level. A refusal that
    /// sends someone to a dead end is worse than one that only says no.
    #[test]
    fn frequency_intents_route_through_the_one_authority() {
        let mut s = session();
        for (c, want) in [
            (Command::GoToFrequency { hz: 100.0 }, "frequency"),
            (Command::AdjustFrequency { delta_hz: 1.0 }, "frequency"),
            (
                Command::SetLevel {
                    level: hertzforge_protocol::LevelTarget::DriveVolts(0.1),
                },
                "drive-volts",
            ),
        ] {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(
                        message.contains(&format!("no {want}-bearing module is live on this core")),
                        "{c:?}: {message}"
                    );
                    // The sentence that says how to get one. It used to point at a
                    // future phase ("the Explorer host lands after its chassis gate");
                    // now it names the verbs, because the host is here.
                    assert!(message.contains("SetProfile"), "{c:?}: {message}");
                    assert!(message.contains("Start"), "{c:?}: {message}");
                    // ... and it names the module family that can actually serve THIS
                    // intent.
                    assert!(
                        message.contains("Explorer profile"),
                        "{c:?} must point at the module that serves it: {message}"
                    );
                }
                other => panic!("{c:?}: {other:?}"),
            }
        }

        // THE ANTI-VACUITY HALF: a different intent gets a DIFFERENT family. Without
        // this the three assertions above pass against the single hard-coded sentence
        // the bundle replaced.
        match run(
            &mut s,
            Command::SetLevel {
                level: hertzforge_protocol::LevelTarget::Db(-6.0),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains("closed-loop modules")
                        && message.contains("DescribeCapabilities"),
                    "a dB level must point at the closed-loop family and the \
                     capability listing: {message}"
                );
                assert!(
                    !message.contains("Explorer profile"),
                    "the Explorer has no dB ladder and must not be offered for one: \
                     {message}"
                );
            }
            other => panic!("a dB level with nothing live: {other:?}"),
        }
    }

    /// The COVERED run verbs against an unconfigured core answer with an ACTIONABLE
    /// refusal (what to send first), never LocalOnly - LocalOnly would tell the front
    /// end the module does not exist here, which stopped being true when the Random
    /// host landed.
    #[test]
    fn random_verbs_before_configure_say_what_to_send_first() {
        let mut s = session();
        for c in [
            Command::Start,
            Command::SetLevel {
                level: hertzforge_protocol::LevelTarget::Db(-6.0),
            },
            Command::AdjustLevel { delta_db: 1.0 },
        ] {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(message.contains("SetProfile"), "{c:?}: {message}");
                }
                other => panic!("{c:?} must be an actionable refusal, got {other:?}"),
            }
        }
    }

    /// **THE RED TEST FOR THE v5 ROUTING BUNDLE**, one row per verb.
    ///
    /// `SetLevel{Db}` / `AdjustLevel` / `SetLadderMode` / `ProceedAfterReview` were the
    /// last four hard-coded `self.core.random.*` calls in the dispatcher. With a
    /// NON-Random module holding the core they reached Random anyway and answered about
    /// a module the operator never mentioned - "Random is not configured on this core"
    /// while something else was driving, which reads as a broken daemon rather than a
    /// wrong address.
    ///
    /// The discriminating observation is precise, and it is why Random is left
    /// UNCONFIGURED here: if a verb still reaches Random it answers Random's own
    /// `not_configured` refusal, whose text ("Random is not configured") cannot be
    /// produced by any other path. So the second assertion is what actually fails on a
    /// regression; the first only says the new answer looks right.
    ///
    /// Anti-vacuity: the same four verbs on the same core with the probe DOWN answer
    /// the idle refusal instead, so the pass depends on the probe's liveness and not on
    /// these verbs refusing everything.
    ///
    /// RED-MUTATION: restore any arm to `self.core.random.<verb>(..)` and that row reds
    /// on `must not have reached Random`.
    #[test]
    fn the_four_level_verbs_route_and_never_reach_random_when_another_module_holds() {
        let verbs = || {
            [
                Command::SetLevel {
                    level: hertzforge_protocol::LevelTarget::Db(-6.0),
                },
                Command::AdjustLevel { delta_db: 1.0 },
                Command::SetLadderMode {
                    ladder: hertzforge_protocol::LadderMode::Manual,
                },
                Command::ProceedAfterReview,
            ]
        };

        // ---- ANTI-VACUITY: nothing live -> the idle refusal, which NAMES the family
        // that could serve the intent. Random is that family, so this also pins the
        // capability-aware tail: the answer names the closed-loop FAMILY and
        // points at the capability listing - never one module by name (an
        // exclusivity claim goes stale by design the moment another closed-loop
        // host lands; cross-review wording, 2026-08-08).
        let mut s = session();
        for c in verbs() {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(
                        message.contains("is live on this core"),
                        "{c:?} on an idle core must describe the core's STATE: {message}"
                    );
                    assert!(
                        message.contains("closed-loop modules")
                            && message.contains("DescribeCapabilities"),
                        "{c:?} must name the family that CAN serve it and point at the \
                         capability listing: {message}"
                    );
                }
                other => panic!("{c:?} on an idle core: {other:?}"),
            }
        }

        // ---- A module the level verbs know nothing about now holds the core.
        s.core()
            .probe
            .live
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Probe),
            "the authority must see the third module"
        );

        for c in verbs() {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(
                        message.contains(Module::Probe.label()),
                        "{c:?} must NAME the occupant: {message}"
                    );
                    assert!(
                        !message.contains("Random is not configured"),
                        "{c:?} must not have reached Random - the occupant is the {}: \
                         {message}",
                        Module::Probe.label()
                    );
                    assert!(
                        message.contains(Module::Probe.stand_down_hint()),
                        "{c:?} must say what to send: {message}"
                    );
                }
                other => panic!("{c:?} while a third module holds the core: {other:?}"),
            }
        }
    }

    /// **THE PER-VERB RED TEST FOR THE SINE ROUTING**, one row per verb.
    ///
    /// With SINE holding the core, `SetLevel{Db}` / `AdjustLevel` / `SetLadderMode` /
    /// `ProceedAfterReview` must reach the SINE host. Before the routing arm existed
    /// they reached Random - the module the operator never mentioned - and answered
    /// "Random is not configured on this core" while a sine run held the hardware.
    ///
    /// The discriminating observation is precise, and it is why Random is left
    /// UNCONFIGURED here: if a verb still reaches Random it answers Random's own
    /// `not_configured` refusal, whose text cannot be produced by any other path in
    /// this daemon. So the FIRST assertion on every row is the one that actually fails
    /// on a regression.
    ///
    /// The four do NOT all answer the same way, and that asymmetry is asserted rather
    /// than smoothed over, because it is the reason `route_level_intent` takes two
    /// closures instead of one trait object:
    ///
    /// * `SetLadderMode` is ACCEPTED against a configured-but-unstarted run. A MANUAL
    ///   run is chosen at Start and the step is not built until the coherence gate
    ///   releases, so a mode that could only be posted into the step's mailbox would
    ///   arrive after the first rung rule had already been evaluated.
    /// * The other three are Sine's OWN service refusals, naming the state.
    ///
    /// RED-MUTATION: delete the `Some(Module::Sine) => on_sine(..)` arm from
    /// `route_level_intent` and all four rows red on `must not have reached Random`.
    #[test]
    fn the_four_level_verbs_reach_sine_and_never_random_when_sine_holds_the_core() {
        let mut s = session();
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetProfile {
                        profile: hertzforge_protocol::TestProfile::Sine(Box::new(sine_wire())),
                    }
                ),
                Response::Accepted { .. }
            ),
            "the Sine profile is SERVED"
        );
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Sine),
            "an accepted profile means a built station this core holds"
        );

        for c in [
            Command::SetLevel {
                level: hertzforge_protocol::LevelTarget::Db(-6.0),
            },
            Command::AdjustLevel { delta_db: 1.0 },
            Command::ProceedAfterReview,
        ] {
            match run(&mut s, c.clone()) {
                Response::Rejected {
                    error: ProtocolError::Refused { message },
                } => {
                    assert!(
                        !message.contains("Random is not configured"),
                        "{c:?} must not have reached Random - the occupant is the {}: \
                         {message}",
                        Module::Sine.label()
                    );
                    // ... and it IS the sine SERVICE's own answer, not the router's:
                    // both of these strings are produced only by `hertzforge-sine`.
                    assert!(
                        message.contains("not engaged (no step)")
                            || message.contains("proceed_after_review from"),
                        "{c:?} must carry the sine service's own refusal: {message}"
                    );
                }
                other => panic!("{c:?} while Sine holds the core: {other:?}"),
            }
        }
        // The one that is legal before the step exists.
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetLadderMode {
                        ladder: hertzforge_protocol::LadderMode::Manual,
                    }
                ),
                Response::Accepted { .. }
            ),
            "a MANUAL run is chosen at Start, before the step is built"
        );
        run(&mut s, Command::Stop);
    }

    /// **THE SINE PROFILE IS SERVED AND THE SINE GROUP ROUTES**.
    ///
    /// Replaces the two rows this module used to occupy in
    /// `the_v5_surfaces_are_accepted_on_the_wire_and_refused_by_a_named_missing_host`.
    /// Both halves of the new posture are pinned: a profile is ACCEPTED (a real station
    /// is built and a real service configured on it), and a sine verb with nothing live
    /// answers a refusal describing the core's STATE rather than claiming the verb is
    /// unsupported here.
    ///
    /// RED-MUTATION: restore `not_served_yet(..)` on either arm and both halves red.
    #[test]
    fn the_sine_profile_is_served_and_its_group_routes() {
        let mut s = session();
        // Nothing live: the group refusal describes the STATE. Deliberately NOT
        // pinned to the how-to-get-one tail's wording, which is shared with the other
        // routed intents and is edited by whichever session touches it last.
        match run(
            &mut s,
            Command::Sine(hertzforge_protocol::SineCommand::Hold),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains("is live on this core"),
                    "a sine verb on an idle core must describe the core's STATE: {message}"
                );
                assert!(
                    !message.contains("MISSING HOST"),
                    "the host is not missing any more: {message}"
                );
            }
            other => panic!("a sine verb with nothing live: {other:?}"),
        }

        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::Sine(Box::new(sine_wire())),
                }
            ),
            Response::Accepted { .. }
        ));
        let p = s
            .core()
            .module_provenance(Module::Sine)
            .expect("a configured Sine has a provenance");
        assert_eq!(p.module, "sine");

        // The group now REACHES the host. `OverrideFeasibility` with an empty ack is
        // the row worth asserting: it proves the verb arrived (the refusal is the
        // service's, about the ack) rather than being answered by the router.
        match run(
            &mut s,
            Command::Sine(hertzforge_protocol::SineCommand::OverrideFeasibility {
                ack: "  ".into(),
            }),
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(
                message.contains("acknowledgment") || message.contains("FeasibilityBlocked"),
                "the empty ack must be answered by the SERVICE: {message}"
            ),
            other => panic!("an empty override ack must be refused: {other:?}"),
        }
        run(&mut s, Command::Stop);
    }

    /// A CONFIGURED Random still answers all four verbs itself, byte-for-byte as before
    /// the routing landed.
    ///
    /// The bundle changed WHO is asked, not what Random says. Without this the previous
    /// test is satisfied by a router that refuses everyone, which would be a worse
    /// regression than the one it guards.
    #[test]
    fn a_configured_random_still_answers_the_four_level_verbs_itself() {
        let mut s = session();
        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::Random(random_profile()),
                }
            ),
            Response::Accepted { .. }
        ));
        // A level that IS a rung of the configured ladder is accepted by Random's own
        // logic; one that is not gets Random's own message, naming the ladder. Either
        // way the answer came from Random, which is the point.
        match run(
            &mut s,
            Command::SetLevel {
                level: hertzforge_protocol::LevelTarget::Db(-999.0),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::InvalidArgument { message },
            } => assert!(
                message.contains("rung of the configured ladder"),
                "the answer must be RANDOM's, not the router's: {message}"
            ),
            other => panic!("an off-ladder level must be Random's own refusal: {other:?}"),
        }
        run(&mut s, Command::Stop);
    }

    // =================================================================================
    // `Core::live_module` - the ONE authority
    // =================================================================================

    /// **THE RED TEST for the five-site refactor.**
    ///
    /// A THIRD module - one no pairwise `if random ... else if recorder ...` chain
    /// ever knew about - is live. `Stop` must DISPATCH TO IT. The old chain answered
    /// "nothing is running on this core", which an operator reads as "the run already
    /// ended" while an armature is still moving; that is the failure this refactor
    /// exists to make impossible.
    ///
    /// Anti-vacuity, because a green here would otherwise be free: the same core with
    /// the probe DOWN is asserted to still produce the "nothing is running" refusal,
    /// so the pass depends on the probe's liveness and not on `Stop` accepting
    /// everything. And the acceptance is not enough on its own - the probe must record
    /// that it was actually stopped, which is the difference between dispatching and
    /// merely answering.
    ///
    /// RED-MUTATION: restore `if random.is_live() {...} else if recorder.is_live()
    /// {...} else refuse` in the `Stop`/`Abort` arm and this reds on the first assert.
    #[test]
    fn a_third_live_module_is_stopped_not_answered_with_nothing_is_running() {
        let mut s = session();

        // ANTI-VACUITY: nothing live -> the honest refusal.
        match run(&mut s, Command::Stop) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(message.contains("nothing is running"), "{message}"),
            other => panic!("an idle core must refuse Stop: {other:?}"),
        }

        // Now a module the old chain had never heard of is driving.
        s.core()
            .probe
            .live
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            s.core().live_module(Claim::Live),
            Some(Module::Probe),
            "the authority must see the third module"
        );

        assert!(
            matches!(run(&mut s, Command::Stop), Response::Accepted { .. }),
            "Stop must dispatch to the live module, not refuse"
        );
        let reason = s
            .core()
            .probe
            .stopped_because
            .lock()
            .expect("probe poisoned")
            .clone()
            .expect("the third module must actually have been stopped, not just ack'd");
        assert!(
            reason.contains("supervisory stop"),
            "the stop must carry its reason: {reason}"
        );
        assert_eq!(
            s.core().live_module(Claim::Live),
            None,
            "the module is down after its controlled stop"
        );
    }

    // ---------------------------------------------------------------------------------
    // The Explorer against the one authority. These run a REAL service on the
    // HERTZFORGE-SIM station - configure, arm, drive, stop - because the whole point of the
    // five-site refactor is that a module wired at one site is wired at all of them, and
    // a mocked liveness flag would only prove `Module::Probe` again.
    // ---------------------------------------------------------------------------------

    /// Configure and start an Explorer run on the Sim station, and wait until it is
    /// really clocked rather than merely priming. Bounded, so a genuine failure fails
    /// the test instead of hanging.
    fn start_explorer(s: &mut CoreSession) {
        match run(
            s,
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Explorer(explorer()),
            },
        ) {
            Response::Accepted { .. } => {}
            other => panic!("configure: {other:?}"),
        }
        match run(s, Command::Start) {
            Response::Accepted { .. } => {}
            other => panic!("start: {other:?}"),
        }
        for _ in 0..600 {
            let st = s.core().status();
            if matches!(st.module_state.as_deref(), Some("armed") | Some("running")) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "the Explorer run never armed: {:?}",
            s.core().status().module_state
        );
    }

    /// A long-running Random profile, so the run is still live when the assertions run.
    fn random_profile() -> hertzforge_protocol::RandomProfile {
        hertzforge_protocol::RandomProfile {
            schema_version: hertzforge_protocol::PROFILE_SCHEMA,
            breakpoints: vec![
                hertzforge_protocol::PsdBreakpoint {
                    freq_hz: 20.0,
                    level: hertzforge_protocol::PsdLevel::Psd(0.01),
                    alarm_db: None,
                    abort_db: None,
                },
                hertzforge_protocol::PsdBreakpoint {
                    freq_hz: 2000.0,
                    level: hertzforge_protocol::PsdLevel::Psd(0.01),
                    alarm_db: None,
                    abort_db: None,
                },
            ],
            schedule: vec![hertzforge_protocol::ScheduleEntry {
                db: -12.0,
                dwell_s: Some(600.0),
            }],
            ladder: hertzforge_protocol::LadderMode::Manual,
            duration_s: Some(600.0),
            acquisition: None,
            control: Some(hertzforge_protocol::ControlProps {
                review_after_sysid: Some(false),
                ..Default::default()
            }),
            limits: None,
            control_weights: None,
            composite_mode: None,
            frf_pairs: None,
            feasibility: None,
        }
    }

    /// Wait for the core to go idle after a controlled stop. Bounded.
    fn wait_idle(s: &CoreSession) {
        for _ in 0..600 {
            if s.core().live_module(Claim::Live).is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "the core never went idle: {:?}",
            s.core().live_module(Claim::Live)
        );
    }

    /// **THE EXPLORER RED TEST.** A live Explorer takes the `Stop`, and it takes it
    /// through the one authority.
    ///
    /// This is the same claim `a_third_live_module_is_stopped_not_answered_with_nothing_
    /// is_running` makes with a synthetic module, made again with the REAL one that
    /// motivated the refactor. The failure it guards is precise: with a pairwise `if
    /// random ... else if recorder ...` chain, a `Stop` sent to a driving Explorer
    /// answers "nothing is running on this core", which an operator reads as "the run
    /// already ended" while an armature is moving.
    ///
    /// Anti-vacuity: the run is asserted LIVE first (so the acceptance is not free), and
    /// the core is asserted IDLE afterwards (so the acceptance is not merely an ack).
    ///
    /// RED-MUTATION: drop `Module::Explorer` from `Module::ALL` and this reds on the
    /// first assertion - `live_module` can never return a module the priority order
    /// does not list.
    #[test]
    fn a_live_explorer_is_stopped_not_answered_with_nothing_is_running() {
        let mut s = session();
        start_explorer(&mut s);
        assert_eq!(
            s.core().live_module(Claim::Live),
            Some(Module::Explorer),
            "the authority must see the Explorer run"
        );
        assert!(
            matches!(run(&mut s, Command::Stop), Response::Accepted { .. }),
            "Stop must dispatch to the live Explorer, not refuse"
        );
        wait_idle(&s);
        // The run ENDED and says what ended it, rather than going quiet.
        let st = s.core().status();
        assert!(
            st.module_state
                .as_deref()
                .is_some_and(|m| m.starts_with("stopped")),
            "the ended run must name its end: {:?}",
            st.module_state
        );
    }

    /// **THE MUTUAL EXCLUSION, Explorer vs Random, IN BOTH DIRECTIONS.**
    ///
    /// Two drive-producing modules on one station is the state this rule exists to
    /// prevent, and the direction that used to be missing entirely (a `recorder_live:
    /// bool` threaded into one host) is the reason `claim_core` asks the authority
    /// instead. Each refusal has to NAME the occupant and the verb that stands it down,
    /// or an operator is told no without being told what to do.
    #[test]
    fn explorer_and_random_refuse_each_other_by_name_in_both_directions() {
        // ---- Explorer live -> Random cannot configure ----------------------------
        let mut s = session();
        start_explorer(&mut s);
        let c = Command::SetProfile {
            profile: hertzforge_protocol::TestProfile::Random(random_profile()),
        };
        match run(&mut s, c.clone()) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains(Module::Explorer.label()),
                    "{c:?} must NAME the occupant: {message}"
                );
                assert!(
                    message.contains(Module::Explorer.stand_down_hint()),
                    "{c:?} must say what to send: {message}"
                );
            }
            other => panic!("{c:?} while the Explorer drives: {other:?}"),
        }
        // A bare `Start` is NOT an exclusion question here, and the answer proves the
        // `Start` dispatch: it routes to the module that holds the core, so it answers
        // about the EXPLORER ("already under way") rather than about Random ("send
        // SetProfile first"), which is what the old hard-coded arm would have said
        // while an Explorer run was driving.
        match run(&mut s, Command::Start) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(
                message.contains("already under way"),
                "Start must address the live Explorer: {message}"
            ),
            other => panic!("Start while the Explorer drives: {other:?}"),
        }
        run(&mut s, Command::Stop);
        wait_idle(&s);

        // ---- Random live -> the Explorer cannot configure ------------------------
        let mut s = session();
        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::Random(random_profile()),
                }
            ),
            Response::Accepted { .. }
        ));
        assert!(matches!(
            run(&mut s, Command::Start),
            Response::Accepted { .. }
        ));
        assert_eq!(s.core().live_module(Claim::Live), Some(Module::Random));
        match run(
            &mut s,
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Explorer(explorer()),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains(Module::Random.label()),
                    "the refusal must NAME the occupant: {message}"
                );
                assert!(
                    message.contains(Module::Random.stand_down_hint()),
                    "the refusal must say what to send: {message}"
                );
            }
            other => panic!("an Explorer configure while Random drives: {other:?}"),
        }
        run(&mut s, Command::Stop);
    }

    /// The SAFE-STOP path (controller timeout) goes through the same authority, so a
    /// module that `Stop` can reach is a module a vanished controller can also bring
    /// down. Before the refactor these were two independently written chains.
    #[test]
    fn the_safe_stop_path_reaches_the_same_module_stop_does() {
        let s = session();
        s.core()
            .probe
            .live
            .store(true, std::sync::atomic::Ordering::SeqCst);
        s.core().safe_stop("controller timeout");
        assert_eq!(s.core().live_module(Claim::Live), None);
        assert!(s
            .core()
            .probe
            .stopped_because
            .lock()
            .expect("probe poisoned")
            .as_deref()
            .is_some_and(|r| r.contains("controller timeout")));
    }

    // ---------------------------------------------------------------------------------
    // SetProfile releases the OTHER modules' parked configs.
    //
    // These run REAL services on the HERTZFORGE-SIM station, for the same reason the
    // Explorer block above does: the defect is a routing fact about built stations, and
    // a mocked liveness flag would only re-prove `Module::Probe`.
    // ---------------------------------------------------------------------------------

    /// The narrow-band sine profile these tests configure. [`sine_wire`] spans 20 Hz to
    /// 2 kHz, and the sysID burst is `~1600 / f_lo` seconds - 80 s at a 20 Hz low edge,
    /// which is a background thread outliving the test. The low edge is the only reason
    /// this fixture exists; everything else is [`sine_wire`] unchanged.
    fn sine_wire_narrow() -> hertzforge_protocol::SineWireProfile {
        let mut p = sine_wire();
        p.profile.rows[0].freq_hz = 200.0;
        p.mode = hertzforge_protocol::SineModeSpec::Sweep(hertzforge_protocol::SineSweepSpec {
            band_hz: (200.0, 2000.0),
            rate: hertzforge_protocol::SineSweepRate::LogOctPerMin(1.0),
            start_hz: 200.0,
            initial_direction: hertzforge_protocol::SweepDirection::Up,
            target: hertzforge_protocol::SineSweepTarget::Sweeps(1),
        });
        p
    }

    /// Run a command and hand back the events it emitted, so a test can read the RUN
    /// PROVENANCE rather than infer which module started from a state token.
    fn run_events(s: &mut CoreSession, c: Command) -> (Response, Vec<Event>) {
        let mut sink = VecSink::default();
        let r = s.handle(c, &mut sink);
        (r, sink.events)
    }

    /// The `module` field of the `RunProvenance` event this command emitted, if any.
    /// **The provenance, not the state**: a state token says what is happening, and
    /// the question here is WHO it is happening to.
    fn started_module(events: &[Event]) -> Option<String> {
        events.iter().find_map(|e| match e {
            Event::RunProvenance { provenance } => {
                serde_json::from_str::<serde_json::Value>(&provenance.json)
                    .ok()
                    .and_then(|v| v.get("module").and_then(|m| m.as_str().map(String::from)))
            }
            _ => None,
        })
    }

    /// Configure an Explorer run, start it, stop it: the PARKED STOPPED CONFIG that an
    /// earlier Explorer probe left on the shared daemon and that the operator's first
    /// sine acceptance walked into.
    fn park_a_stopped_explorer(s: &mut CoreSession) {
        start_explorer(s);
        run(s, Command::Stop);
        wait_idle(s);
        assert!(
            s.core().explorer.is_configured(),
            "the parked config is the whole premise: a stopped Explorer still HOLDS \
             its built station"
        );
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Explorer),
            "and it outranks every module below it, which is why a bare Start goes to it"
        );
    }

    /// **THE IDLE-CLAIM RED TEST - the operator's own sequence, in-tree.**
    ///
    /// Operator report, 2026-08-09, first acceptance run of the Sine module: an
    /// Explorer probe had left a STOPPED Explorer config parked on the shared daemon,
    /// the operator configured a sine profile, pressed Run, and read **"Explorer cannot
    /// start from Stopped"** - a refusal naming a module the operator had never opened.
    ///
    /// The anti-vacuity half runs FIRST and is not decoration: it reproduces the
    /// operator's refusal off the parked config, so the test cannot pass by the parked
    /// state failing to exist. Then the sine profile goes in and the SAME bare `Start`
    /// addresses Sine.
    ///
    /// **Asserted by PROVENANCE, not by state.** A run state says something is arming;
    /// only the provenance says which module it belongs to, and "which module" is the
    /// entire defect.
    ///
    /// RED-MUTATION: drop the `release_idle_claims` call out of `claim_and_displace`
    /// and this reds on the Start, with the operator's own sentence.
    #[test]
    fn a_parked_stopped_explorer_no_longer_steals_the_sine_start() {
        let mut s = session();
        park_a_stopped_explorer(&mut s);

        // ---- ANTI-VACUITY: the operator's refusal, reproduced ---------------------
        match run(&mut s, Command::Start) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(
                message.contains("Stopped"),
                "the parked Explorer is supposed to refuse a bare Start from Stopped - \
                 without that this test proves nothing: {message}"
            ),
            other => panic!("a bare Start on a parked stopped Explorer: {other:?}"),
        }

        // ---- the fix: configuring Sine releases the parked claim ------------------
        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetProfile {
                        profile: hertzforge_protocol::TestProfile::Sine(Box::new(
                            sine_wire_narrow()
                        )),
                    }
                ),
                Response::Accepted { .. }
            ),
            "the sine profile is served"
        );
        assert!(
            !s.core().explorer.is_configured(),
            "the parked Explorer config must be RELEASED, not merely outranked"
        );
        assert_eq!(
            s.core().live_module(Claim::Configured),
            Some(Module::Sine),
            "the module just configured is the one that holds the core"
        );

        let (r, events) = run_events(&mut s, Command::Start);
        assert!(
            matches!(r, Response::Accepted { .. }),
            "the operator's Run must be accepted, not answered about the Explorer: {r:?}"
        );
        assert_eq!(
            started_module(&events).as_deref(),
            Some("sine"),
            "the run that started is the SINE run"
        );

        run(&mut s, Command::Stop);
        wait_idle(&s);
    }

    /// The same parked Explorer, from the SHOCK panel. Requested off the shock side's
    /// own operator repro: the operator hit this from both panels and the refusal
    /// string is identical either way, because the module doing the refusing is the
    /// PARKED one and has nothing to do with which panel sent the profile.
    ///
    /// The Explorer outranks Shock in [`Module::ALL`], so this is a genuine misroute
    /// and not a restatement of the sine row.
    #[test]
    fn a_parked_stopped_explorer_no_longer_steals_the_shock_start() {
        let mut s = session();
        park_a_stopped_explorer(&mut s);

        assert!(
            matches!(
                run(
                    &mut s,
                    Command::SetProfile {
                        profile: hertzforge_protocol::TestProfile::ShockClassical(Box::new(
                            classical()
                        )),
                    }
                ),
                Response::Accepted { .. }
            ),
            "the classical shock profile is served"
        );
        assert!(
            !s.core().explorer.is_configured(),
            "the parked Explorer config must be RELEASED, not merely outranked"
        );
        assert_eq!(s.core().live_module(Claim::Configured), Some(Module::Shock));

        let (r, events) = run_events(&mut s, Command::Start);
        assert!(
            matches!(r, Response::Accepted { .. }),
            "the operator's Run must ARM the shock module: {r:?}"
        );
        assert_eq!(
            started_module(&events).as_deref(),
            Some("shock.spectrum"),
            "the run that armed is the SHOCK run"
        );

        run(&mut s, Command::Stop);
        wait_idle(&s);
    }

    /// **A LIVE module is NEVER displaced**, which is the half of the idle-claim fix
    /// that must not move. The hotfix releases IDLE claims; a run that is driving keeps
    /// the core, keeps its config, and keeps answering the same named refusal with the
    /// same stand-down hint.
    ///
    /// RED-MUTATION, observed red 2026-08-09, and the exact shape is worth recording:
    /// the live case is guarded TWICE - `release_idle_claims` skips any module holding
    /// at `Claim::Live`, and each host's own `release_idle` refuses when it is live -
    /// so removing either one alone leaves the behaviour correct and this test green.
    /// Sweeping the claims BEFORE `claim_core` **and** dropping the host guard reds it
    /// on `Accepted { completion: Immediate }`: a sine configure granted while the
    /// Explorer is driving.
    #[test]
    fn a_live_module_still_refuses_set_profile_and_keeps_its_config() {
        let mut s = session();
        start_explorer(&mut s);
        assert_eq!(s.core().live_module(Claim::Live), Some(Module::Explorer));

        match run(
            &mut s,
            Command::SetProfile {
                profile: hertzforge_protocol::TestProfile::Sine(Box::new(sine_wire_narrow())),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(
                    message.contains(Module::Explorer.label()),
                    "the refusal must NAME the occupant: {message}"
                );
                assert!(
                    message.contains(Module::Explorer.stand_down_hint()),
                    "the refusal must say what to send: {message}"
                );
            }
            other => panic!("a sine configure while the Explorer drives: {other:?}"),
        }
        assert!(
            s.core().explorer.is_configured() && s.core().explorer.is_live(),
            "the LIVE Explorer keeps its station and its run"
        );
        assert_eq!(
            s.core().live_module(Claim::Live),
            Some(Module::Explorer),
            "a refused configure changes nothing about who owns the core"
        );

        run(&mut s, Command::Stop);
        wait_idle(&s);
    }

    /// The SYMMETRIC case, for the families the sine row does not cover: a parked SINE
    /// config is released when a shock profile is configured, and the shock run then
    /// arms.
    ///
    /// Sine sits BELOW Shock in [`Module::ALL`], so a bare `Start` would have reached
    /// Shock either way - which is exactly why the discriminating assertion here is on
    /// `sine.is_configured()` and not on the routing. Before the fix the parked sine
    /// station survives the shock configure; after it, it does not.
    #[test]
    fn a_parked_sine_config_is_released_when_the_shock_family_configures() {
        let mut s = session();
        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::Sine(Box::new(sine_wire_narrow())),
                }
            ),
            Response::Accepted { .. }
        ));
        assert!(
            s.core().sine.is_configured() && !s.core().sine.is_live(),
            "the premise: a built-but-idle sine station"
        );

        assert!(matches!(
            run(
                &mut s,
                Command::SetProfile {
                    profile: hertzforge_protocol::TestProfile::ShockClassical(
                        Box::new(classical())
                    ),
                }
            ),
            Response::Accepted { .. }
        ));
        assert!(
            !s.core().sine.is_configured(),
            "the parked sine station must be released by the shock configure"
        );

        let (r, events) = run_events(&mut s, Command::Start);
        assert!(matches!(r, Response::Accepted { .. }), "{r:?}");
        assert_eq!(started_module(&events).as_deref(), Some("shock.spectrum"));

        run(&mut s, Command::Stop);
        wait_idle(&s);
    }

    /// The PRIORITY ORDER covers every variant.
    ///
    /// The compiler forces a new module to be wired at every dispatch site (the
    /// matches are exhaustive with no `_` arm); it does NOT force membership of
    /// `Module::ALL`, so this test is that half of the guarantee. Each variant is
    /// named explicitly here, which is what makes the list an enumeration rather than
    /// a count.
    #[test]
    fn the_priority_order_lists_every_module() {
        for m in [
            Module::Random,
            Module::Explorer,
            Module::Shock,
            Module::Srs,
            Module::Sine,
            Module::Twr,
            Module::Recorder,
            Module::Probe,
        ] {
            assert!(
                Module::ALL.contains(&m),
                "{m:?} is not in the priority order; `Core::live_module` can never \
                 return it"
            );
            assert!(!m.label().is_empty(), "{m:?} has no name for a refusal");
            assert!(!m.stand_down_hint().is_empty(), "{m:?} has no stop verb");
        }
        // RECOMPUTED FROM THE MERGED TREE at the TwrHost landing (2026-08-09):
        // Random, Explorer, Shock, Srs, Sine, Twr, Recorder + the cfg(test) Probe
        // = 8 in test builds. Counted off the enum above, NOT incremented from the
        // 7 this replaces - which is the rule every previous landing followed and
        // the reason the number has never been wrong after a merge.
        assert_eq!(
            Module::ALL.len(),
            8,
            "a module was added to (or removed from) the priority order without \
             being classified above"
        );
        assert_eq!(
            Module::ALL[0],
            Module::Random,
            "the DRIVE-PRODUCING modules outrank the monitor-only one"
        );
        assert!(
            Module::ALL.iter().position(|&m| m == Module::Explorer)
                < Module::ALL.iter().position(|&m| m == Module::Recorder),
            "the Explorer drives an armature and the Recorder does not; a status poll, \
             a stop and a provenance push must be about the one that can move something"
        );
        // THE SHOCK MODULES SIT ABOVE THE RECORDER for the same reason and for one more:
        // a transient producer between pulses is quiet but ARMED, with committed DAQmx
        // tasks and a pulse one verb away. A tie-break that put the monitor-only module
        // first would make a status poll during an inter-pulse gap report the Recorder.
        for m in [Module::Shock, Module::Srs] {
            assert!(
                Module::ALL.iter().position(|&x| x == m)
                    < Module::ALL.iter().position(|&x| x == Module::Recorder),
                "{m:?} can move an armature and the Recorder cannot"
            );
        }
        assert!(
            Module::ALL.iter().position(|&m| m == Module::Sine)
                < Module::ALL.iter().position(|&m| m == Module::Recorder),
            "Sine drives an armature and the Recorder does not"
        );
        assert!(
            Module::ALL.iter().position(|&m| m == Module::Explorer)
                < Module::ALL.iter().position(|&m| m == Module::Sine),
            "the slot Sine took is AFTER the Explorer: the three drive-producing modules \
             are mutually excluded at Claim::Live, so this order is only reachable for the \
             weaker Claim::Configured question - which decides which of two built-but-idle \
             stations a bare Start addresses, and that answer must not move under an \
             operator who configured the Explorer first"
        );
    }

    /// **THE RULE, not a list: every DRIVE-PRODUCING module precedes the Recorder**
    /// (landed with `Module::Twr`).
    ///
    /// The five hand-written `position(..) < position(Recorder)` assertions above are
    /// the same claim spelled out one module at a time, and a sixth module arrives with
    /// a sixth chance for nobody to add the line. This states the rule ONCE, over a set
    /// that a new variant must be classified into - `drive_producing` is exhaustive with
    /// no `_` arm, so a module added to the enum and not to it does not compile.
    ///
    /// Why the rule matters, restated because it is easy to read as bookkeeping: a
    /// status poll, a Stop and a provenance push must be about the module that can move
    /// an armature. A tie-break that put the monitor-only module first would make a poll
    /// during a TWR inter-play compute gap - which is quiet, but with a corrected drive
    /// one verb from the pin - report the Recorder.
    ///
    /// RED-MUTATION: move `Module::Twr` after `Module::Recorder` in `Module::ALL` and
    /// this reds naming Twr; the count test above stays green, which is the whole reason
    /// this is a rule test and not a count test.
    #[test]
    fn every_drive_producing_module_precedes_the_recorder() {
        /// Can this module put samples on a DAC? Exhaustive, no `_` arm: a new variant
        /// has to be classified rather than defaulting into the safe-looking answer.
        fn drive_producing(m: Module) -> bool {
            match m {
                Module::Random
                | Module::Explorer
                | Module::Shock
                | Module::Srs
                | Module::Sine
                | Module::Twr => true,
                Module::Recorder => false,
                // The test probe drives nothing; it exists to be a module the pairwise
                // chains never knew about.
                Module::Probe => false,
            }
        }
        let recorder = Module::ALL
            .iter()
            .position(|&m| m == Module::Recorder)
            .expect("the Recorder is in the priority order");
        let mut producers = 0usize;
        for (i, &m) in Module::ALL.iter().enumerate() {
            if drive_producing(m) {
                producers += 1;
                assert!(
                    i < recorder,
                    "{m:?} can move an armature and sits BELOW the Recorder at {i}: a status \
                     poll, a Stop and a provenance push would be about the module that cannot"
                );
            }
        }
        // ANTI-VACUITY: the loop above passes trivially if nothing is classified as a
        // producer, which is exactly what a mis-edited `drive_producing` would do.
        assert_eq!(
            producers, 6,
            "six drive-producing modules were classified; recount `drive_producing` \
             against the enum rather than adjusting this number"
        );
    }

    /// THE MUTUAL EXCLUSION, in BOTH directions, each naming the occupant and the
    /// verb that stands it down.
    ///
    /// The reverse direction is new: the old `recorder_live: bool` could only express
    /// "Random refuses while the Recorder monitors", so a `Recorder(Configure)` during
    /// a live Random run built a second station on the same hardware without a word.
    #[test]
    fn the_exclusion_names_the_occupant_in_both_directions() {
        let s = session();
        s.core()
            .probe
            .live
            .store(true, std::sync::atomic::Ordering::SeqCst);

        for want in [Module::Random, Module::Recorder] {
            match s.core().claim_core(want) {
                Err(ProtocolError::Refused { message }) => {
                    assert!(
                        message.contains(Module::Probe.label()),
                        "the refusal must NAME the occupant: {message}"
                    );
                    assert!(
                        message.contains(Module::Probe.stand_down_hint()),
                        "the refusal must say what to send: {message}"
                    );
                }
                other => panic!("{want:?} must be refused while another module is live: {other:?}"),
            }
        }

        // The occupant itself is NOT refused here - its own host answers the finer
        // "already live" question, which is the only place that knows what live means
        // for that module.
        assert!(s.core().claim_core(Module::Probe).is_ok());
    }

    /// An idle core claims cleanly. Anti-vacuity for the test above: without this a
    /// `claim_core` that refused everything would pass it.
    #[test]
    fn an_idle_core_grants_the_claim() {
        let s = session();
        assert_eq!(s.core().live_module(Claim::Live), None);
        assert!(s.core().claim_core(Module::Random).is_ok());
        assert!(s.core().claim_core(Module::Recorder).is_ok());
    }

    /// The daemon answers discovery honestly: ONE station; `open_loop_sine`, `shock`
    /// and `srs` CLAIMED (each on its own recorded chassis gate); `random` and `mimo`
    /// still UNCLAIMED.
    ///
    /// The asymmetry is the point and it is not an oversight. Random is per-tick
    /// corrected, so its resolution needs a PROVEN Shared clock, which is per-station
    /// and established at `SetProfile`/`arm` - claiming it here with `clock: Unknown`
    /// would be structurally inconsistent (`HostedClosedLoopWithoutSharedClock`) and
    /// would advertise exactly what `arm()` goes on to refuse. Open-loop sine has no
    /// adaptation and therefore no clock precondition, so it CAN be stated at
    /// descriptor time - once the run itself has been proven on hardware, which the
    /// Explorer chassis gate did.
    ///
    /// RED-MUTATION: set `modes.random = hosted()` and `inconsistencies()` reds this -
    /// the descriptor cannot honestly claim a per-tick-corrected mode without the proven
    /// clock.
    #[test]
    fn the_daemon_claims_one_station_open_loop_sine_and_not_random() {
        let mut s = session();
        match run(&mut s, Command::ListBackends) {
            Response::Backends(rows) => {
                assert_eq!(rows.len(), 1, "a headless daemon has one station");
                assert!(rows[0].selected);
            }
            other => panic!("{other:?}"),
        }
        match run(&mut s, Command::DescribeCapabilities) {
            Response::Capabilities(caps) => {
                assert!(
                    caps.inconsistencies().is_empty(),
                    "the daemon's own caps must be structurally honest: {:?}",
                    caps.inconsistencies()
                );
                assert_eq!(
                    caps.clock,
                    ClockDomain::Unknown,
                    "never claim an unproven clock"
                );
                assert!(
                    matches!(
                        caps.modes.random,
                        hertzforge_protocol::ModeSupport::Unsupported(_)
                    ),
                    "the Random mode claim waits for per-station clock evidence (SetProfile)"
                );
                // The Sine HOST landed and the CLAIM did not move. Same rule,
                // same reason - closed-loop sine is per-tick corrected, so it needs the
                // proven Shared clock the descriptor cannot state. The evidence ledger
                // is in `capabilities()`'s doc comment and it is not all-green twice.
                assert!(
                    matches!(
                        caps.modes.sine,
                        hertzforge_protocol::ModeSupport::Unsupported(_)
                    ),
                    "a landed HOST is not a mode claim: the Sine claim waits \
                     for per-station clock evidence and the sine chassis gate passing twice"
                );
                assert!(
                    caps.modes.open_loop_sine.is_hosted(),
                    "the Explorer host synthesizes every drive sample: {:?}",
                    caps.modes.open_loop_sine
                );
                // SHOCK AND SRS ARE CLAIMED, on the shock PASS-TWICE chassis evidence,
                // and they are claimed in the resolution their arm gate actually
                // requests: host-synthesized drive, corrected BETWEEN shots. Claiming
                // `HostPerTick` here would demand a proven Shared clock and
                // `inconsistencies()` (asserted empty above) would say so.
                for (mode, support) in [
                    (hertzforge_protocol::TestMode::Shock, &caps.modes.shock),
                    (hertzforge_protocol::TestMode::Srs, &caps.modes.srs),
                ] {
                    assert_eq!(
                        support.resolution().map(|r| r.adaptation),
                        Some(hertzforge_protocol::AdaptationOwner::HostBetweenRuns),
                        "{mode:?} corrects between shots, never per tick"
                    );
                    assert!(support.is_hosted(), "{mode:?}: {support:?}");
                }
                // MIMO stays UNCLAIMED - nothing was built for it, and a matrix that
                // grew a claim because its neighbours did would be the inventory this
                // one is deliberately not.
                assert!(
                    matches!(
                        caps.modes.mimo,
                        hertzforge_protocol::ModeSupport::Unsupported(_)
                    ),
                    "MIMO has no host and no evidence"
                );
                assert_eq!(
                    caps.modes.open_loop_sine.resolution().map(|r| r.adaptation),
                    Some(hertzforge_protocol::AdaptationOwner::None),
                    "nothing adapts an open-loop survey inside the run - the operator is \
                     the loop"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// THE ACTION MATRIX SAYS WHAT THIS DAEMON DISPATCHES - no more, no less.
    ///
    /// Anti-vacuity in both directions: `ramp_down` must be `Unsupported` (its arm is a
    /// typed refusal, so claiming it would advertise a verb that always fails), and the
    /// frequency / drive rows must be `Instant` (the Explorer clamps and adopts before
    /// the response returns). A matrix that claimed everything would pass the first half
    /// of this and a claim-nothing matrix would pass the second.
    #[test]
    fn the_action_matrix_matches_what_the_daemon_serves() {
        let s = session();
        let caps = s.core().capabilities(BackendId::new("sim"));
        for (kind, want) in [
            (ActionKind::SetProfile, ActionSupport::Instant),
            (ActionKind::Start, ActionSupport::Instant),
            (ActionKind::Stop, ActionSupport::Instant),
            (ActionKind::Abort, ActionSupport::Instant),
            (ActionKind::SetLevel, ActionSupport::Instant),
            (ActionKind::AdjustLevel, ActionSupport::Instant),
            (ActionKind::GoToFrequency, ActionSupport::Instant),
            (ActionKind::AdjustFrequency, ActionSupport::Instant),
            (ActionKind::RampDown, ActionSupport::Unsupported),
        ] {
            assert_eq!(caps.actions.support(kind), want, "{kind:?}");
        }
        // And the claim is CHECKED against the dispatcher, not just asserted: RampDown
        // really is refused.
        let mut s = s;
        assert!(matches!(
            run(&mut s, Command::RampDown),
            Response::Rejected {
                error: ProtocolError::UnsupportedAction {
                    action: ActionKind::RampDown
                }
            }
        ));
    }

    /// A station change is REFUSED with the reason - a headless host has no operator
    /// standing next to it to see what got rebound.
    #[test]
    fn a_remote_station_change_is_refused_with_the_reason() {
        let mut s = session();
        // Its OWN station is accepted (idempotent), so the refusal below is about
        // CHANGING and not about the verb.
        assert!(matches!(
            run(
                &mut s,
                Command::SelectBackend {
                    backend: BackendId::new("sim")
                }
            ),
            Response::Accepted { .. }
        ));
        match run(
            &mut s,
            Command::SelectBackend {
                backend: BackendId::new("ni"),
            },
        ) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => {
                assert!(message.contains("config file"), "{message}");
                assert!(message.contains("restart"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// The channel table round-trips through the core, and the etag moves.
    #[test]
    fn the_table_verbs_go_through_the_authoritative_store() {
        let mut s = session();
        let first = match run(&mut s, Command::GetChannelTable) {
            Response::ChannelTable(t) => t,
            other => panic!("{other:?}"),
        };
        let mut edited = s.core().table.snapshot();
        edited.channels[0].description = "control".into();
        let put = Command::PutChannelTable {
            table: hertzforge_protocol::TableDoc::new(
                first.etag.clone(),
                edited.to_json().unwrap(),
            ),
        };
        let stored = match run(&mut s, put) {
            Response::ChannelTable(t) => t,
            other => panic!("{other:?}"),
        };
        assert_ne!(stored.etag, first.etag);

        // The same write again is now STALE, and the refusal crosses as a typed error.
        let again = Command::PutChannelTable {
            table: hertzforge_protocol::TableDoc::new(first.etag, edited.to_json().unwrap()),
        };
        assert!(matches!(
            run(&mut s, again),
            Response::Rejected {
                error: ProtocolError::StaleUpload { .. }
            }
        ));
    }

    /// A recorder verb against an UNCONFIGURED core is a refusal that says what to
    /// send first, not a panic and not a silent no-op.
    #[test]
    fn recorder_verbs_before_configure_say_what_to_send_first() {
        let mut s = session();
        match run(&mut s, Command::Recorder(RecorderCommand::StartMonitor)) {
            Response::Rejected {
                error: ProtocolError::Refused { message },
            } => assert!(message.contains("Configure"), "{message}"),
            other => panic!("{other:?}"),
        }
        // Read-only verbs still answer, with `configured: false` - a status poll must
        // not fail just because nothing is set up yet.
        match run(&mut s, Command::Recorder(RecorderCommand::Status)) {
            Response::Recorder(d) => {
                assert!(d.json.contains("\"configured\":false"), "{}", d.json);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A malformed recorder configuration carries the DECODER'S message, which is what
    /// names the offending field.
    #[test]
    fn a_malformed_recorder_config_carries_the_decoder_message() {
        let mut s = session();
        let cmd = Command::Recorder(RecorderCommand::Configure {
            config: Doc::new(SchemaKind::RecorderConfig, "{ not json"),
        });
        match run(&mut s, cmd) {
            Response::Rejected {
                error: ProtocolError::MalformedPayload { container, detail },
            } => {
                assert_eq!(container, "recorder configuration");
                assert!(!detail.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    /// A document at a foreign SCHEMA never reaches the handler at all.
    #[test]
    fn a_foreign_schema_is_refused_before_the_handler_runs() {
        let mut s = session();
        let cmd = Command::Recorder(RecorderCommand::Configure {
            config: Doc {
                schema_version: 99,
                json: "{}".into(),
            },
        });
        assert!(matches!(
            run(&mut s, cmd),
            Response::Rejected {
                error: ProtocolError::UnsupportedSchema { .. }
            }
        ));
    }
}
