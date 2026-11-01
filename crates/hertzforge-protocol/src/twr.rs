//! The TIME WAVEFORM REPLICATION (TWR) wire surface (v6): [`TwrProfile`] and the
//! [`TwrCommand`] group, including the chunked REFERENCE UPLOAD.
//!
//! # What this is
//!
//! A field-for-field WIRE MIRROR of `hertzforge_twr::service::config::TwrConfig` - minus
//! the reference samples and the capture block, plus nothing - and the verbs a TWR run
//! needs that the general command set cannot express. Typed rather than an opaque
//! [`crate::Doc`], for the reason [`crate::sine`]'s module docs give: this is the
//! payload that points a shaker, every field is a scalar or a small enum, and a
//! misparse must be impossible rather than merely unlikely.
//!
//! The same three-part FIDELITY FENCE applies: every operator-settable
//! field of `TwrConfig` appears here, the daemon's wire -> config conversion
//! destructures BOTH structs exhaustively so a field added to either is a compile error
//! rather than a silent default, and `tests/v6.rs` round-trips a fully-populated
//! profile. A field that exists on one side and not the other is the defect.
//!
//! # The REFERENCE does not ride the profile, and that is the whole shape of this module
//!
//! `TwrConfig::reference` is a `ReferenceWaveform` carrying `Vec<f64>` SAMPLES - a real
//! TWR record is 10^5 to 10^6 of them. Putting that on `SetProfile` would make the
//! command path carry megabytes, block every status poll behind one send, and give a
//! failed upload no expression other than a failed configure.
//!
//! So the record crosses ONCE, on its own, through
//! [`TwrCommand::BeginReference`] -> N x [`TwrCommand::ReferenceChunk`] ->
//! [`TwrCommand::CommitReference`], and [`TwrProfile::reference_sha`] names the STAGED
//! record the run is about. One authority for the bytes, one name for it, and a
//! configure that refuses by name when the sha it quotes was never staged.
//!
//! ## The upload's three-part guard, and the trade it buys
//!
//! `BeginReference` DECLARES `total_samples` and `sha256`; the chunks are checked for
//! STRICT `seq + 1` ordering as they arrive; `CommitReference` asserts BOTH that the
//! received count equals the declared `total_samples` AND that the digest of what was
//! staged equals the declared `sha256`. Three checks rather than one, because each
//! catches a failure the others cannot: a dropped chunk passes a sha check that is
//! never reached, a reordered pair passes a count check, and a corrupted-but-complete
//! transfer passes both ordering and count.
//!
//! The digest is quoted UP FRONT rather than at commit, which makes it the same
//! etag-shaped exchange [`crate::TableDoc`] uses: the client states what it believes it
//! is sending, the core states what it actually holds, and a disagreement is refused
//! with [`crate::ProtocolError::StaleUpload`] naming BOTH - never a silent accept of a
//! record that is not the one the operator chose.
//!
//! Samples cross as BASE64 of little-endian `f32`, not as a JSON number array: inbound
//! BINARY frames are refused by design at this seam (the binary direction is display
//! frames, core -> client), a JSON array of a million floats is roughly 20x the bytes,
//! and `f32` is what the drive path consumes anyway. The cost is stated rather than
//! hidden: the reference is stored at `f32` precision, which is ~24 bits on a record
//! whose own import is `f64` text.
//!
//! ## Uploads run against an IDLE core, and the daemon says so
//!
//! [`crate::Command::is_mutating`] is the CONTROLLER line, not a run-state line - it
//! answers who may ask, not when. So the three upload verbs additionally carry a
//! not-live requirement the daemon enforces with its own named refusal: re-staging the
//! record a run is currently replicating would swap the target under the law.
//!
//! # What is DELIBERATELY absent: the capture block
//!
//! `TwrConfig::capture` (`CaptureRules { auto_start, run_name }`) has no wire field, and
//! that is the same safety decision [`crate::shock`] documents: `auto_start` would start
//! a disk writer at ARM on the core host, where the remote operator cannot see the disk,
//! cannot see the writer die, and has no verb to stop it. The daemon FORCES
//! `auto_start = false` when it translates, and the wire has no field a hand-written
//! client could use to flip it back.
//!
//! # The post-sysID review flag ARRIVED with the host (additive within v6)
//!
//! ~~`TwrConfig` has no review-hold field at this tip, so neither does this.~~ That was
//! true when v6 landed and stopped being true the moment the host did: the host work added
//! `review_after_sysid` to `hertzforge_twr::TwrConfig` and this file gained
//! [`TwrProfile::review_after_sysid`] in the SAME change, which is the fidelity fence
//! working rather than being bypassed. The DEFAULT was the open question and it is
//! answered ON, for the reason the field's own docs give: an absent flag must mean hold.
//!
//! v6 is IN-TREE and UNDEPLOYED, so this is an edit-in-place of the v6 entry rather than
//! a v7 bump - the changelog's SERVED-NOW convention. Nothing outside this tree has ever
//! spoken v6.
//!
//! The general [`crate::Command::ProceedAfterReview`] is still the verb that releases the
//! park and is deliberately NOT duplicated into [`TwrCommand`].
//!
//! # `Command::Start` means ARM-AND-SYSTEM-ID here
//!
//! A TWR run does not start and then run continuously: it arms, identifies the plant,
//! and then the operator ITERATES ([`TwrCommand::IterateOnce`]) or hands the converged
//! drive to the schedule ([`TwrCommand::BeginPlayback`]). So the general
//! [`crate::Command::Start`] maps to arm-plus-sysID on the TWR dispatch arm, following
//! the precedent the Explorer set and [`crate::shock`] restated: `Start` means "whatever
//! this module does when the operator says go". Stating it here is what stops a UI
//! waiting for a `Running` that never arrives.
//!
//! Everything else a TWR run needs already exists as operator intent and stays there:
//! [`crate::Command::Stop`] ramps it down, [`crate::Command::Abort`] cuts it,
//! [`crate::Command::ProceedAfterReview`] releases a hold, and
//! [`crate::Command::SetProfile`] configures it.

use serde::{Deserialize, Serialize};

use crate::caps::ActionKind;
use crate::command::profile_schema;

/// The drive ceiling a TWR profile ships with, volts PEAK.
///
/// Mirrors `hertzforge_twr::limits::DEFAULT_MAX_DRIVE_VPK`, and it is the same 5.0 V every
/// other module ships - stated as a constant here rather than inlined so the wire
/// default and the core default cannot drift apart silently.
pub const DEFAULT_TWR_MAX_DRIVE_VPK: f64 = 5.0;

/// The serde default for [`TwrLimitsSpec::max_drive_vpk`]: `Some(5.0)`.
///
/// **`Some`, not `None`, and the distinction is load-bearing.** On the core side a
/// CLEARED ceiling refuses to run - the ceiling is the authorization, so its absence is
/// not "unlimited", it is "unauthorized" - and that meaning only survives the wire if
/// `None` is expressible. So the field is an `Option` (unlike
/// [`crate::shock::ClassicalShockProfile::max_drive_vpk`], which is a plain `f64`) and
/// an ABSENT field defaults to the safe number while an explicit `null` crosses as the
/// operator's cleared ceiling and is refused by name at the core.
fn default_twr_max_drive_vpk() -> Option<f64> {
    Some(DEFAULT_TWR_MAX_DRIVE_VPK)
}

/// The serde default for [`TwrProfile::review_after_sysid`]: `true`.
///
/// Named rather than `#[serde(default)]`, because a bare default on a `bool` is `false`
/// and this one must be `true`: a profile written before the field existed - or by a
/// hand-rolled client that never heard of it - has to arrive HOLDING, not running. The
/// core-side default (`hertzforge_twr::TwrConfig::review_after_sysid`) is the same value for
/// the same reason, which is what keeps the two ends from disagreeing about what an
/// absent field meant.
fn default_review_after_sysid() -> bool {
    true
}

// =====================================================================================
// The profile
// =====================================================================================

/// A TIME WAVEFORM REPLICATION test, as the operator authored it (v6).
///
/// The wire mirror of `hertzforge_twr::service::config::TwrConfig` - see the module docs for
/// the fidelity fence, for why the reference samples are NOT here, and for the two
/// fields (`capture`, a review flag) that are deliberately absent.
///
/// `schema_version` is the ONLY field with no counterpart on the config side: it is
/// this crate's exact-match profile gate ([`crate::PROFILE_SCHEMA`]), carried on the
/// outermost payload only. The nested types below do not repeat it - one document, one
/// version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrProfile {
    /// The profile-schema version this was written at. See [`crate::PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// The STAGED reference record this run replicates, named by its digest.
    ///
    /// The sha the client declared at [`TwrCommand::BeginReference`] and the core
    /// verified at [`TwrCommand::CommitReference`]. NOT the samples, and not a
    /// filename: a name can be reused, and a run whose reference silently changed
    /// between the review and the arm is the failure this field exists to make
    /// impossible. A profile quoting a sha the core has not staged is REFUSED by name -
    /// never run against whatever happens to be loaded.
    pub reference_sha: String,
    /// Reference preparation, applied in the documented order: resample -> detrend ->
    /// highpass -> scale -> taper. Applied by the CORE, to the RAW staged record, at
    /// EVERY arm - one authority, so a re-arm cannot compound a prep.
    pub prep: TwrPrep,
    /// The slice of the PREPARED reference to replicate. `None` = all of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<TwrSection>,
    /// Quiet pad BEFORE the section on the working grid (drive pre-ring room), samples.
    pub pad_pre: usize,
    /// Quiet pad AFTER the section (ring-down room), samples.
    pub pad_post: usize,
    /// The system-identification (ranging / pretest) plan.
    pub sysid: TwrSysIdSpec,
    /// Inversion + per-bin trust configuration.
    pub inverse: TwrInverseSpec,
    /// The correction law's knobs, including the correction band.
    pub law: TwrLawSpec,
    /// The level ladder.
    pub schedule: TwrLevelSchedule,
    /// What playback does once the full level has converged.
    pub run: TwrRunSchedule,
    /// Keep correcting DURING scheduled playback. `false` (the default posture) plays a
    /// frozen, converged drive, which is what makes a playback deterministic report
    /// evidence.
    pub adapt_during_playback: bool,
    /// Limits, aborts, the drive ceiling and the pre-arm feasibility ceilings.
    pub limits: TwrLimitsSpec,
    /// High-pass corner for the feasibility kinematics preview, Hz.
    pub kinematics_hp_hz: f64,
    /// PARK after a PASSING system ID, before the first correction iteration.
    /// Default ON - see [`default_review_after_sysid`] for why an absent field holds.
    ///
    /// The mirror of `hertzforge_twr::TwrConfig::review_after_sysid`. The park itself is the
    /// CALLER's: `TwrService::advance()` runs one unit per call and owns no loop, so it
    /// has nowhere to park, and the daemon host holds it between units. Released by
    /// [`crate::Command::ProceedAfterReview`], the general verb, which is why
    /// [`TwrCommand`] has no release verb of its own.
    ///
    /// **Not the coherence hold.** A FAILED sysID is `TwrState::CoherenceHold` and has
    /// no release verb at all; clearing this flag does not weaken that one.
    #[serde(default = "default_review_after_sysid")]
    pub review_after_sysid: bool,
    /// User metadata carried into the report.
    #[serde(default)]
    pub metadata: Vec<TwrMetaField>,
}

/// Reference preparation. Mirrors `hertzforge_twr::service::config::PrepConfig`.
///
/// The order is fixed and is NOT on the wire, deliberately: resample -> detrend ->
/// highpass -> scale -> taper is a property of the pipeline, not a choice, and a
/// transported order would be a second authority that can disagree with the code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrPrep {
    /// Resample the reference to the player rate when they differ.
    ///
    /// `false` makes a rate mismatch a configuration ERROR rather than a silent linear
    /// resample. **Downward resampling is refused whatever this says** - linear
    /// interpolation has no anti-alias filter, and converging on an aliased target is a
    /// test that passes against the wrong spectrum.
    pub auto_resample: bool,
    /// Remove the mean (`Some(false)`) or the linear trend (`Some(true)`). `None` =
    /// neither.
    ///
    /// Three-valued and kept exactly as the config states it. Collapsing it to a `bool`
    /// would fold "leave the trend alone" into "remove the mean", which are different
    /// records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detrend: Option<bool>,
    /// Drift high-pass corner, Hz, at or below the correction band's low edge. `None` =
    /// no high pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highpass_hz: Option<f64>,
    /// EU rescale factor (import-unit correction). `1.0` = none. Finite and non-zero.
    pub scale: f64,
    /// Cosine end-taper fraction per end, in `(0, 0.5]`. `None` = no taper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taper_frac: Option<f64>,
}

/// A from-to slice of the PREPARED reference to replicate. Mirrors
/// `hertzforge_twr::schedule::Section`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrSection {
    /// First sample index into the prepared reference.
    pub start: usize,
    /// Length in samples. `0` is invalid.
    pub len: usize,
}

/// The system-identification plan. Mirrors `hertzforge_twr::sysid::SysIdConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrSysIdSpec {
    /// The excitation shape.
    pub excitation: TwrExcitation,
    /// Excitation RMS in DRIVE units.
    ///
    /// ABSOLUTE, not a percentage of full level, and that is not an oversight: before
    /// sysID no FRF exists, so "10% of output" is not a computable quantity.
    pub drive_rms: f64,
    /// Excitation record length, seconds. More seconds = more Welch averages = lower
    /// FRF variance.
    pub duration_s: f64,
    /// Welch/FRF block size; a power of two `>= 64`. `df_sysid = fs / nfft`.
    pub nfft: usize,
    /// Identification band low edge, Hz. Also the correction band's outer bound.
    pub f_lo_hz: f64,
    /// Identification band high edge, Hz. Must be below `fs / 2`.
    pub f_hi_hz: f64,
    /// Minimum acceptable IN-BAND MEAN coherence.
    ///
    /// Below it the sysID HOLDS AND REPORTS; the level is never raised silently to
    /// reach it. Releasing that hold is a core-side Reset, exactly as it is for the
    /// sine `CoherenceHold` - there is deliberately no wire verb that clears a failed
    /// safety gate.
    pub min_mean_coherence: f64,
}

/// The ranging excitation shape. Mirrors `hertzforge_twr::sysid::Excitation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrExcitation {
    /// Band-limited flat-PSD random (COLA/WOLA synthesis). The default, and what
    /// commercial TWR pretests use.
    Random,
    /// Logarithmic chirp `f_lo -> f_hi` with cosine end ramps. Energy density falls
    /// ~1/f (flat per octave); deterministic, which some sites prefer.
    LogChirp,
}

/// Inversion + trust configuration. Mirrors `hertzforge_twr::inverse::InverseConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrInverseSpec {
    /// Tikhonov floor as a fraction of the in-band peak `|H|`, in `(0, 1)`. Plant gain
    /// below `beta_floor * Hmax` is treated as un-driveable.
    pub tikhonov_beta_floor: f64,
    /// Per-bin coherence floor for the trust mask, in `[0, 1]`. Bins identified below
    /// it get NO correction updates - held, never amplified.
    pub coherence_floor: f64,
}

/// The correction law's knobs. Mirrors `hertzforge_twr::law::LawConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrLawSpec {
    /// Relaxation / convergence gain, in `(0, 1]`. Above 1 is rejected: per-bin
    /// overshoot under FRF error is how an iteration diverges.
    pub beta: f64,
    /// Per-bin magnitude-growth clamp per iteration, dB.
    pub max_db_per_iter: f64,
    /// Reference dynamic-range floor, dB below the in-band target spectral peak. Bins
    /// below it are excluded from correction.
    pub ref_floor_db: f64,
    /// Correction band low edge, Hz. Must sit INSIDE the sysID band - `G` outside the
    /// identified band is extrapolation, and correcting there is correcting a guess.
    pub f_lo_hz: f64,
    /// Correction band high edge, Hz. Must sit inside the sysID band.
    pub f_hi_hz: f64,
    /// Conservative initial-drive backoff, dB `>= 0`. The first drive is `G * R_L`
    /// scaled DOWN by this much and the iteration recovers the level: undershoot is
    /// corrected upward safely, overshoot is an overtest.
    pub initial_drive_backoff_db: f64,
    /// Minimum fraction of the TOTAL target spectral energy the trust mask must cover.
    ///
    /// Unmasked bins are never corrected, so their target energy is a PERMANENT error
    /// floor: `rms_error_pct` can never fall below `100 * sqrt(1 - coverage)`. Carried
    /// on the wire because a gate tighter than the coverage allows is a run that cannot
    /// pass, and the operator has to be able to see the number that decides it.
    pub min_masked_energy_frac: f64,
}

/// The level ladder. Mirrors `hertzforge_twr::schedule::TwrLevelSchedule`.
///
/// Unlike [`crate::sine::SineLadderConfig`] the run MODE is carried here, because for
/// TWR it is not live-switchable state: `Auto` walks the table gating each level on
/// convergence and `Manual` hands stepping to the operator, and which one applies is
/// authored before anything arms rather than taken over mid-run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrLevelSchedule {
    /// Who advances the ladder.
    pub mode: TwrRunMode,
    /// Level table, dB relative to full. Non-empty and non-decreasing; the LAST entry
    /// is the full level.
    pub table_db: Vec<f64>,
    /// Convergence gate: a level advances at `rms_error_pct <=` this.
    pub gate_rms_error_pct: f64,
    /// Maximum correction passes per level before `on_gate_fail` applies.
    pub max_iterations_per_level: u32,
    /// What happens when a level exhausts its iterations without gating.
    pub on_gate_fail: TwrGateFailPolicy,
}

/// Who advances the TWR ladder. Mirrors `hertzforge_twr::schedule::TwrRunMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrRunMode {
    /// Walk the table, gating each level on convergence.
    Auto,
    /// The operator iterates and steps levels by hand.
    Manual,
}

/// What to do when a level exhausts its iterations without gating. Mirrors
/// `hertzforge_twr::schedule::GateFailPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrGateFailPolicy {
    /// Abort the run - the safety-conservative choice, and the core's default.
    Abort,
    /// Hold at the level, stop iterating, wait for the operator.
    Hold,
}

/// The playback schedule once the full level has converged. Mirrors
/// `hertzforge_twr::schedule::RunSchedule`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrRunSchedule {
    /// Play the record once.
    Once,
    /// Loop the record N times (`N >= 1`).
    LoopN(u32),
    /// Play repeatedly until this many seconds of SECTION time have elapsed.
    DurationS(f64),
}

/// Limits, aborts, the drive ceiling and the pre-arm feasibility ceilings. Mirrors
/// `hertzforge_twr::limits::TwrLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrLimitsSpec {
    /// Alarm at `|y - r_L| > alarm_frac * peak|r_L|`. Latched; the run continues.
    pub envelope_alarm_frac: f64,
    /// Abort at `|y - r_L| > abort_frac * peak|r_L|` for `envelope_debounce_samples`
    /// CONSECUTIVE samples.
    pub envelope_abort_frac: f64,
    /// Consecutive violating samples before the envelope abort fires. `>= 1`, so a
    /// single noise spike cannot kill a run.
    pub envelope_debounce_samples: usize,
    /// Achieved-record RMS ceiling in EU; exceeded aborts. `None` = no ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_rms_limit: Option<f64>,
    /// Per-sample drive hard clip, drive units. `None` = no clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_clip: Option<f64>,
    /// Drive-record RMS ceiling, drive units; exceeded aborts BEFORE the play.
    ///
    /// **This is NOT the drive ceiling.** A TWR reference is a transient with a crest
    /// factor of 5-10, so an RMS ceiling of 5 authorizes 25-50 V of peak. The
    /// authorization is `max_drive_vpk`; this is an additional energy guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_rms_limit: Option<f64>,
    /// THE DRIVE CEILING: maximum PEAK volts of the REALIZED drive record.
    ///
    /// Checked after the inverse filter has produced the record and before it reaches
    /// the player, and it REFUSES rather than clipping - clipping would distort the
    /// drive silently and the law would then converge around its own distortion.
    ///
    /// **An `Option`, unlike the shock profiles' plain `f64`, and deliberately.**
    /// `None` is the operator having CLEARED the ceiling, which the core refuses to run
    /// on ("the ceiling is the authorization, so no ceiling is no authorization"). A
    /// plain `f64` could not carry that state, so a cleared ceiling would arrive as
    /// 5.0 V and a refusal the operator was owed would become a run. ABSENT defaults to
    /// `Some(5.0)` - see [`default_twr_max_drive_vpk`].
    ///
    /// Shock's profile-time projection is deliberately NOT imported: a TWR drive is the
    /// output of an inverse filter, and a plant notch makes it arbitrarily larger than
    /// the reference, so a ceiling computed from the reference peak would bless a run
    /// whose first corrected drive is twenty times that.
    #[serde(default = "default_twr_max_drive_vpk")]
    pub max_drive_vpk: Option<f64>,
    /// Divergence debounce: consecutive rising iterations before abort. `>= 1`.
    pub divergence_n: usize,
    /// Relative rise per iteration that counts as "rising".
    pub divergence_tol_frac: f64,
    /// Pre-arm kinematic ceilings.
    pub feasibility: TwrFeasibilityCeilings,
}

/// Pre-arm kinematic ceilings, evaluated against the FULL-LEVEL prepared reference.
/// Mirrors `hertzforge_twr::limits::FeasibilityCeilings`. A `None` axis is NOT EVALUATED,
/// which is not the same as passing and is reported as such.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrFeasibilityCeilings {
    /// Peak `|acceleration|` ceiling, waveform EU (nominally g).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_accel: Option<f64>,
    /// Peak `|velocity|` ceiling, in/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_velocity_ips: Option<f64>,
    /// Peak-to-peak displacement ceiling, inches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disp_pkpk_in: Option<f64>,
}

/// One user metadata field carried into the report. Mirrors
/// `hertzforge_twr::service::config::MetaField` - three fields like
/// [`crate::shock::ShockMetaField`], because the report prints the label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwrMetaField {
    /// Stable machine key.
    pub key: String,
    /// Human label.
    pub label: String,
    /// Freeform value.
    pub value: String,
}

// =====================================================================================
// The command group
// =====================================================================================

/// The TWR verbs the general command set cannot express (v6).
///
/// Two families in one group: the ITERATION lifecycle (a loop that closes over seconds,
/// between plays, not per tick) and the chunked REFERENCE UPLOAD (see the module docs).
///
/// **[`crate::Command::ProceedAfterReview`] is NOT duplicated here.** It is the general
/// verb that releases a post-sysID review hold and it already exists; a second verb
/// meaning the same thing is how a UI and a core end up disagreeing about what the
/// operator asked for. The same rule keeps `Start` / `Stop` / `Abort` / `SetProfile`
/// out of this enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrCommand {
    /// Run ONE correction pass: play the drive, measure, correct. MUTATING.
    ///
    /// The unit of a TWR run. The loop closes over SECONDS (play -> measure -> correct
    /// -> play), which is why it is a verb at all: a per-tick loop would be core-side
    /// machinery no operator names, and this one is a decision an operator makes each
    /// time at low levels.
    IterateOnce,
    /// Hand the CONVERGED drive to the playback schedule and run it. MUTATING.
    ///
    /// Refused before the full level has converged, by name: playback is the
    /// deterministic report evidence, and playing an unconverged drive would produce a
    /// record labelled as something it is not.
    BeginPlayback,
    /// Step the ladder one rung. MUTATING.
    ///
    /// `up: true` climbs, `false` descends. A relative step rather than two verbs,
    /// because it is one intent with a direction - the same shape as
    /// [`crate::Command::AdjustLevel`]'s signed delta.
    StepLevel {
        /// Climb (`true`) or descend (`false`).
        up: bool,
    },
    /// Jump to a level-table index. MUTATING.
    JumpLevel {
        /// The zero-based index into the schedule's `table_db`.
        index: usize,
    },
    /// Freeze (or unfreeze) the correction law, LIVE. MUTATING.
    ///
    /// Frozen, the drive stops being corrected and the run replays what it has. It is
    /// the same authority class as the sine blend weight: it decides how the NEXT drive
    /// is computed, which is why it is gated like a level trim rather than like a start.
    FreezeAdaptation {
        /// `true` freezes the law, `false` resumes correcting.
        frozen: bool,
    },
    /// Clear the run state back to unarmed, dropping the service and RELEASING the
    /// core. MUTATING.
    ///
    /// The release is the load-bearing half, and it is what "clear the run state back
    /// to unarmed" has to mean for a module whose arm consumed a station backend: a
    /// stopped module that still holds the core takes the next bare
    /// [`crate::Command::Start`], which is a run addressed to the wrong module. The
    /// shock groups learned this on their first wire run and their `Reset` does the
    /// same thing.
    Reset,
    /// Begin staging a reference record. MUTATING.
    ///
    /// DECLARES what is coming, so the core can refuse a record it cannot hold before
    /// any of it is sent, and so [`TwrCommand::CommitReference`] has something to check
    /// against. Supersedes any incomplete staging by the same controller.
    BeginReference {
        /// Operator-facing label (filename, description). Display only - the run is
        /// keyed by `sha256`, never by this.
        name: String,
        /// The record's sample rate, Hz. A DOWNWARD resample to the device rate is
        /// refused here, at upload time, so the operator learns immediately rather than
        /// at arm.
        fs_hz: f64,
        /// Engineering-unit label ("g" by convention). Display only.
        eu: String,
        /// How many samples will be sent. Asserted at commit against what arrived.
        total_samples: u64,
        /// Lowercase hex SHA-256 of the little-endian `f32` sample bytes, declared UP
        /// FRONT (the etag-shaped exchange - see the module docs).
        sha256: String,
    },
    /// One chunk of the staging record. MUTATING.
    ///
    /// STRICT `seq + 1` ordering: a gap or a reorder is refused by name rather than
    /// stitched, because a record that is complete-but-scrambled passes a count check
    /// and produces a test against a waveform nobody authored.
    ReferenceChunk {
        /// Zero-based chunk index. Must be exactly one more than the last accepted.
        seq: u32,
        /// Base64 of little-endian `f32` samples. Bounded by the server's stated
        /// message-size ceiling; keeping a chunk at or under 1 MB is also what lets
        /// status polls interleave between chunks on a FIFO command path.
        samples_b64_f32le: String,
    },
    /// Finish staging: assert the count AND the digest, then publish the record.
    /// MUTATING.
    ///
    /// **A struct variant with no fields, deliberately.** Everything it checks was
    /// declared at [`TwrCommand::BeginReference`], so it carries nothing today - but a
    /// unit variant would serialize as the bare string `"commit_reference"`, and giving
    /// it a field later would change the wire shape from a string to an object. `{}`
    /// costs two bytes now and makes the growth additive.
    ///
    /// Answers [`crate::Response::Twr`] with the staged record's manifest - including
    /// the sha the core actually computed - so a client sees WHAT was staged rather
    /// than only that something was.
    CommitReference {},
    /// Fetch the staged reference's manifest and the APPLIED prep. Read-only.
    ///
    /// A QUERY rather than configure-time-only output, for the reason
    /// [`crate::ShockCommand::SrsFreqs`] is one: the two peers who most need it never
    /// saw the upload - an observer that joined mid-run, and a controller that
    /// reconnected after a vanish. It also carries the prep the core ACTUALLY applied
    /// (prepared length, adopted rate, resample note), which is the only place the
    /// operator can check that the record being replicated is the record they authored.
    ReferenceInfo,
    /// Fetch the per-iteration convergence history. Read-only.
    ///
    /// Rows, not records: the RMS error, the level and the outcome per pass. Answers a
    /// [`crate::Doc`] for the same byte-budget reason [`crate::ShockCommand::PulseList`]
    /// does - a history is a table an operator scrolls, and shipping every iteration's
    /// samples to render it would put megabytes on the command path.
    History,
}

impl TwrCommand {
    /// Does this verb change core state? The Observer gate consults
    /// [`crate::Command::is_mutating`], which consults this.
    ///
    /// **The three UPLOAD verbs are MUTATING**, and that is the answer to a question
    /// worth stating: they do not move a shaker, but they write the document the next
    /// run will replicate. An Observer that could stage a reference could change what
    /// the Controller's next arm plays - which is the two-authorities confusion the
    /// lease exists to prevent, arriving through a door nobody was watching.
    ///
    /// `is_mutating` is the CONTROLLER line and not a run-state line: it answers who
    /// may ask, never when. The "not while a run is live" rule the upload also needs is
    /// the daemon's, enforced with its own named refusal.
    pub fn is_mutating(&self) -> bool {
        match self {
            TwrCommand::IterateOnce
            | TwrCommand::BeginPlayback
            | TwrCommand::StepLevel { .. }
            | TwrCommand::JumpLevel { .. }
            | TwrCommand::FreezeAdaptation { .. }
            | TwrCommand::Reset
            | TwrCommand::BeginReference { .. }
            | TwrCommand::ReferenceChunk { .. }
            | TwrCommand::CommitReference {} => true,
            TwrCommand::ReferenceInfo | TwrCommand::History => false,
        }
    }

    /// The [`ActionKind`] this verb is gated by, or `None` for the read-only queries
    /// (which have no matrix row and must not claim one).
    ///
    /// MAPPED onto the EXISTING rows, never matrixed - the same trade as
    /// [`crate::ExplorerCommand::action_kind`] and [`crate::ShockCommand::action_kind`]:
    /// every `ActionMatrix` literal in the tree would otherwise grow a field for a verb
    /// with identical support semantics.
    pub fn action_kind(&self) -> Option<ActionKind> {
        match self {
            // Both make the machine move: an iteration PLAYS the drive, and playback is
            // the run proper. On an iterative producer, playing IS the run.
            TwrCommand::IterateOnce | TwrCommand::BeginPlayback => Some(ActionKind::Start),
            TwrCommand::StepLevel { .. } | TwrCommand::JumpLevel { .. } => {
                Some(ActionKind::SetLevel)
            }
            // Freezing the law decides how much of the next drive is a correction -
            // level authority, live, the same mapping `SineCommand::SetBlendW` takes.
            TwrCommand::FreezeAdaptation { .. } => Some(ActionKind::AdjustLevel),
            // Reset is the run ENDING and the state clearing, which is Stop's gate.
            TwrCommand::Reset => Some(ActionKind::Stop),
            // Staging the reference is PROFILE authority: it is the document the
            // profile names, and a backend that may be given a profile is exactly one
            // that may be given the record that profile points at.
            TwrCommand::BeginReference { .. }
            | TwrCommand::ReferenceChunk { .. }
            | TwrCommand::CommitReference {} => Some(ActionKind::SetProfile),
            TwrCommand::ReferenceInfo | TwrCommand::History => None,
        }
    }
}
