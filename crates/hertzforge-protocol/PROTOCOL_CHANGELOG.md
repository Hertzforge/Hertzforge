# `hertzforge-protocol` wire changelog

The UI <-> core seam is EXACT-MATCH versioned: a peer whose `PROTOCOL_VERSION` differs
is refused at the handshake and again at every dispatch, and there is no best-effort
parse of a foreign version. A misparsed operator command aimed at a physical shaker is
a materially worse outcome than a refused connection.

That rule has a direct consequence for scheduling: **there is no graceful degradation,
so every wire change must land before the deployment that would otherwise have to
negotiate it.** Wire changes are therefore BATCHED into as few bumps as possible rather
than shipped one at a time.

| Version | Phase | Carries |
|---|---|---|
| 1 | pre-NI | The initial seam. |
| 2 | NI buildout (wire bump #1) | Two-axis `ModeSupport`; the binary frame observability block; `MCS1` as the universal spectrum layout; `CoreStamp` on `Hello`. |
| 3 | NI buildout (wire bump #2) | `Subscribe` selectors; `schema_version` on profiles + the new opaque documents; the run lease + shared token on the handshake, and the `HelloAck` that answers it; the capture LIST verb; channel-table get/put with etag concurrency; the Recorder command group; six new typed refusals. |
| 4 | Explorer-on-NI (wire bump #3) | `TestProfile::Explorer(ExplorerProfile)`; the `Explorer(ExplorerCommand)` group (`StartSweep` / `StopSweep`); `StatusReport.drive_vpk` + `.sweep_active` and FILLED semantics for `frequency_hz`; `Command::check_schemas` made exhaustive. |
| 5 | Sine + Shock (wire bump #4) | THREE module surfaces in one increment: `TestProfile::Sine` / `::ShockClassical` / `::Srs`; the `Sine` / `Shock` / `Srs` command groups; nine additive `StatusReport` readouts; `Response::Shock(Doc)`; two shock `SchemaKind`s. `PROFILE_SCHEMA` stays 1. |
| **6** | **TWR (wire bump #5)** | `TestProfile::Twr(Box<TwrProfile>)`; the eleven-verb `Twr` command group including the chunked REFERENCE UPLOAD; `Response::Twr(Doc)`; `SchemaKind::TwrReference`; and `TestMode::Twr` - the FIRST new mode variant this crate has taken - with `ModeMatrix.twr`. `PROFILE_SCHEMA` stays 1. |

---

## v6 - TWR-on-NI, wire bump #5

**One module's surface, and the first bump whose payload does not fit in a profile.**

v5 batched three surfaces because two sessions were building against this seam at the
same time. v6 does not batch, because TWR was the only surface waiting when the bump was
taken. The rule was never "collect modules" - it is "move the wire as few times as
possible" - and holding a finished surface back to wait for a hypothetical second one
would trade a certain delay for a speculative saving. Every bump before this one made
the same argument from the other direction.

**Deploy order is DAEMON FIRST, and BOTH ENDS MOVE.** The daemon restarts before the Mac
app is delivered. Between the two there is a window in which the app is refused at
connect - the designed behaviour, not an outage. `REMOTE_PROTOCOL_VERSION` in
`ui/src/lib/ipc/remote.ts` moved 5 -> 6 with the Rust constant, and `remote.test.ts` pins
the number so a drift is visible.

**SERVED NOW: nothing.** Every `Twr` verb and the `TestProfile::Twr` arm answer the typed
`Refused` naming the absent host, with the words `MISSING HOST` in it - never an
acknowledgement, never a silent drop, and never `LocalOnly` (which would claim the
capability lives in the desktop app; this one will live in the daemon). `TwrHost` lands
next, in this same session, and turns both arms into acceptances with **no further wire
change** - which is the property the accept-at-schema-refuse-by-host shape exists to
have. When it lands, this paragraph gets edited in place, the way v5's did: the record of
what was true when the wire moved is worth more than a paragraph that quietly became
right.

`ModeMatrix.twr` is UNCLAIMED for the same reason and one more: capability claims follow
evidence, not the wire, and TWR's evidence is a bench ledger rather than a
device declaration - see item 4.

### 1. `TestProfile::Twr(TwrProfile)` - and the reference is NOT in it

A field-for-field WIRE MIRROR of `hertzforge_twr::service::config::TwrConfig`: 13 fields over
12 nested types, `deny_unknown_fields` throughout, `schema_version` on the outermost
payload only, boxed like the three v5 variants. Typed rather than an opaque `Doc` for the
reason `sine.rs` gives: this is the payload that points a shaker.

**What is deliberately absent, and what replaces it.** `TwrConfig::reference` is a
`ReferenceWaveform` carrying `Vec<f64>` samples, and a real TWR record is 10^5 to 10^6 of
them. On `SetProfile` that would put megabytes on the command path, block every status
poll behind one send, and leave a failed upload with no expression other than a failed
configure. So the record crosses ONCE through the upload group (item 2) and the profile
carries `reference_sha` - the digest of the STAGED record. A profile quoting a sha the
core has not staged is refused by name, never run against whatever happens to be loaded.
A filename would not do: a name can be reused, and a run whose reference silently changed
between the review and the arm is the failure this field makes impossible.

**The capture block is ABSENT, the same safety decision the shock profiles record.**
`CaptureRules::auto_start` would start a disk writer at ARM on the core host, where the
remote operator cannot see the disk, cannot see the writer die, and has no verb to stop
it. The daemon forces `auto_start = false` at translation and the wire has no
field a hand-written client could use to flip it back.

**~~No review-hold flag~~ - `review_after_sysid` LANDED WITH THE HOST, edited in
place.** When the wire moved, `TwrConfig` had no review-hold field, so the wire had none:
a wire field with no config counterpart is exactly the drift the fidelity fence exists to
prevent, and the DEFAULT for that hold was an open question to the operator rather than
one the bump could answer. The host work answered it and added the field to BOTH sides in ONE
change - `hertzforge_twr::TwrConfig::review_after_sysid` and `TwrProfile::review_after_sysid`,
serde default `true` on each - which is the fence working rather than being bypassed.

This is an EDIT IN PLACE of the v6 entry and not a v7 bump, under the same SERVED-NOW
convention v5's entry used: v6 is in-tree and UNDEPLOYED, nothing outside this tree has
ever spoken it, and the profile is `deny_unknown_fields` with a defaulted field - so an
absent flag decodes and takes the safe value. The default is `true` on both ends because
a bare `#[serde(default)]` on a `bool` is `false`, and defaulting a missing review hold to
"go" would silently un-gate every stored setup on the first run after an upgrade.

**The park is the CALLER's, not the service's**, and the field's docs say so rather than
leaving a reader to grep the service for a consumer and conclude the flag is dead:
`TwrService::advance()` runs one unit per call and owns no loop, so it has nowhere to
park. The daemon's `TwrHost` holds it between units (pump exits at the park,
`ProceedAfterReview` respawns it - the `sine.rs` / `srs.rs` shape). The general
`Command::ProceedAfterReview` is the release verb and is deliberately NOT duplicated into
`TwrCommand`. It does not touch `TwrState::CoherenceHold`, which is the gate FAILING and
has no release verb at all.

**`max_drive_vpk` is an `Option<f64>`, unlike the shock profiles' plain `f64`, and the
difference is load-bearing.** On the core side a CLEARED ceiling refuses to run: the
ceiling is the authorization, so its absence is not "unlimited", it is "unauthorized". A
plain `f64` cannot carry that state, so a cleared ceiling would arrive as 5.0 V and a
refusal the operator was owed would become a run. An ABSENT field defaults to `Some(5.0)`
- the safe value for a stored profile written before the field existed - and an explicit
`null` crosses as cleared. `tests/v6.rs` pins both halves.

Shock's profile-time peak projection is explicitly NOT imported. A TWR drive is the
output of an inverse filter, and a plant notch makes it arbitrarily larger than the
reference, so a ceiling computed from the reference peak would bless a run whose first
corrected drive is twenty times that.

### 2. The `Twr(TwrCommand)` group, and the chunked REFERENCE UPLOAD

```rust
enum TwrCommand {
    // the iteration lifecycle
    IterateOnce, BeginPlayback,
    StepLevel { up: bool }, JumpLevel { index: usize },
    FreezeAdaptation { frozen: bool }, Reset,
    // the upload
    BeginReference { name, fs_hz, eu, total_samples, sha256 },
    ReferenceChunk { seq: u32, samples_b64_f32le: String },
    CommitReference {},
    // read-only
    ReferenceInfo, History,
}
```

Everything else stays general: `Start` / `Stop` / `Abort` run it, `SetProfile` configures
it, and `ProceedAfterReview` releases a hold. **`Command::Start` means ARM-AND-SYSTEM-ID
on the TWR arm** - a TWR run does not start and then run continuously; it arms, identifies
the plant, and then the operator iterates or hands the converged drive to the schedule.
The Explorer set the precedent and the shock groups restated it; stating it here is what
stops a UI waiting for a `Running` that never arrives.

**THE TRADE, stated: three checks on the upload rather than one.** `BeginReference`
declares `total_samples` and `sha256`; chunks are checked for strict `seq + 1` ordering as
they arrive; `CommitReference` asserts BOTH the received count and the digest. Each catches
a failure the others cannot - a dropped chunk passes a sha check that is never reached, a
reordered pair passes a count check, and a corrupted-but-complete transfer passes both
ordering and count. Three named refusals cost three code paths; one check would have cost
a test that converged on a waveform nobody authored.

The digest is quoted UP FRONT rather than at commit, which makes it the same etag-shaped
exchange `TableDoc` uses: the client states what it believes it is sending, the core
states what it actually holds, and a disagreement is `StaleUpload` naming both. That is
also why `CommitReference` carries nothing.

**THE TRADE, stated: base64 f32-LE rather than a JSON number array.** Inbound BINARY
frames are refused by design at this seam (the binary direction is display frames, core
to client), a JSON array of a million floats is roughly 20x the bytes, and `f32` is what
the drive path consumes anyway. The cost is real and is not hidden: the staged record is
`f32`, roughly 24 bits, on a record whose own text import is `f64`. Chunk size at or under
1 MB is what lets status polls interleave between chunks on a FIFO command path.

**`CommitReference {}` is a STRUCT variant with no fields, deliberately.** A unit variant
serializes as the bare string `"commit_reference"`; giving it a field later would change
the wire shape from a string to an object mid-version, which is a decode failure on every
peer that got past the gate. Two bytes now buys additive growth, and `tests/v6.rs` pins
both the object form and the bare-string form's refusal.

**THE TRADE, stated: the per-verb mutating split.** The three UPLOAD verbs are MUTATING
even though they move no shaker - they write the document the next run replicates, so an
Observer that could stage a reference could change what the Controller's next arm plays.
That is the two-authorities confusion the lease exists to prevent, arriving through a door
nobody was watching. `ReferenceInfo` and `History` are NOT mutating, so an observer can
read the manifest and the convergence table, which is most of what an observer is for.

`is_mutating` is the CONTROLLER line and not a run-state line: it answers who may ask,
never when. The "not while a run is live" rule the upload also needs is the daemon's,
enforced with its own named refusal - re-staging the record a run is currently
replicating would swap the target under the law.

`action_kind` MAPS onto the EXISTING nine rows and `ActionMatrix` gains none:
Iterate/BeginPlayback -> `Start`, Step/Jump -> `SetLevel`, `FreezeAdaptation` ->
`AdjustLevel` (the `SetBlendW` precedent - it decides how the next drive is computed),
`Reset` -> `Stop` (the shock `Reset` precedent - a run ending and state clearing),
the three uploads -> `SetProfile` (staging the record a profile names is profile
authority), and the two queries -> `None`, so they claim no row.

`Command::check_schemas` enumerates all eleven by name, no wildcard. The
three uploads are why the rule earns its keep here: they are the most document-shaped
payloads on this wire and they are still not versioned documents.

### 3. `Response::Twr(Doc)` and `SchemaKind::TwrReference`

`Response::Twr` answers the two read-only queries AND an accepted `CommitReference`.
**The commit answers a DOCUMENT rather than a bare acceptance**, which is
`Response::ChannelTable`'s precedent applied to an upload: a client that just staged a
megabyte needs to know WHAT was staged - the sha the core actually computed over the
bytes it holds, the count it counted, the rate it will play them at. Answering "accepted"
would leave the client believing its own declaration, which is the thing the commit
exists to check.

ONE `SchemaKind` for three documents (commit ack, `ReferenceInfo` manifest, `History`
table) at schema 1. They share a version because they are three views of one thing - the
record and its iteration - and a client that can read the manifest can read the history it
produced. Three constants that always move together is three chances for two of them to
disagree.

Same rules as the shock kinds: stamping and checking only, never a wire tag, and the
one-directional additivity invariant holds (an old peer never sends a TWR verb, so it can
never receive a response shape it cannot decode).

### 4. `TestMode::Twr` - the first new mode variant, and the MIRROR EXCEPTION

`TestMode` mirrors `hertzforge_hal::NativeMode` one-for-one. `Twr` is the first and only stated
exception, and `caps.rs`'s module docs carry it explicitly rather than leaving a reader to
notice the asymmetry:

- TWR's law runs BETWEEN plays, on the host, over seconds. Even on a smart controller that
  replays a downloaded record, the ITERATION is host-side - `{DeviceReplay,
  HostBetweenRuns}`, the cell the two-axis `ModeSupport` exists to name and which already
  cites TWR as the case that proves it.
- So there is no "the device natively runs TWR" claim for a backend to make, and
  `NativeMode` deliberately will NOT grow a `Twr`: a slot whose only possible value is a
  lie is worse than no slot.

**THE TRADE, stated:** the exception costs the single-source rule one of its two halves.
`modes.twr` can never be filled from the `Caps::native_modes` side of the constructor, so
a CLAIM there is a manual `ModeMatrix` assignment in the core, made after the module's
on-chassis gate is a recorded PASS and citing that ledger entry - the Shock precedent. The
evidence-first rule is unchanged: evidence still precedes the claim, only the evidence is
a bench ledger rather than a device's own declaration. What is lost is the mechanical
cross-check; what is gained is not carrying a fictional native mode in the HAL forever.

`closes_loop_on_host()` answers `false` for `Twr`, which is a decision rather than an
omission. Listing it would be this method's own documented v1 conflation in a new place:
the shared-clock precondition binds per-tick correction, and demanding a proven-`Shared`
clock for a mode that plays a record and then thinks about it would refuse a run that is
safe without one.

`TestMode::ALL` is `[TestMode; 7]`, and `ModeMatrix` grew the row plus its
`all_unsupported()` / `support()` / `support_mut()` arms in the same edit - the exhaustive
struct has no list to pad, so a missed arm is a compile error.

### 5. The server's inbound message ceiling is now STATED (not a wire change)

`crates/hertzforge-backendd/src/server.rs` called `accept_async`, which rides tungstenite's
DEFAULT limits. That was survivable while every inbound message was a command envelope of
a few hundred bytes and stopped being survivable the moment the seam grew a verb whose
whole job is to carry bulk. The accept path now passes an explicit `WebSocketConfig` with
`max_message_size = 4 MiB`: it bounds the 1 MB chunk the upload contract asks for, with
room for base64's 4/3 expansion and the JSON envelope, and ~3x headroom before the
refusal.

A ceiling nobody chose is a ceiling nobody can act on - the operator gets a dropped socket
with no message, which reads as a network fault rather than as "that chunk was too big".
Recorded here rather than as a wire change because it is a transport parameter: no
payload shape moved. It bounds INBOUND assembly only; the outbound display path is
bounded by the producers and by each client's own `max_size`, and the two numbers cap
opposite directions for opposite reasons.

### What v6 does NOT carry

- **No `Module::Twr` host in the daemon.** That is the next step, in this session. The v6
  landing deliberately stops at the wire plus the two minimal refusal arms.
- **No remote capture.** That refusal is unchanged, and TWR has no capture verb on the wire at
  all - its capture block is absent from the profile (item 1). The six shock capture verbs
  keep their own non-expiring refusal, which is a DIFFERENT answer from this bump's
  missing-host one and stays apart from it.
- **No MIMO profile.** Still no module serves it.
- **No `NativeMode::Twr`.** Item 4: deliberately, permanently.
- **No TWR `StatusReport` readouts.** The echoes the design asks for ride the Doc surfaces -
  `ReferenceInfo` carries the APPLIED prep (prepared length, adopted rate, the resample
  note) and `History` carries the per-iteration table - rather than growing status fields
  before a host exists to fill them. If the host work finds it needs live numeric status that the
  existing fields (`elapsed_s`, `remaining_s`, `level_db`, `module_state`,
  `band_coherence`, the rate pair) cannot express, that is a v7 conversation and should be
  raised as one rather than smuggled in.
- **No sibling probe re-pinning.** `explorer_wire_probe.py`, `sine_wire_probe.py`,
  `shock_wire_probe.py` and `explorer_profile_rate_probe.py` still declare
  `PROTOCOL_VERSION = 5`, on purpose: a probe's constant tracks the DAEMON IT TARGETS, and
  the deployed box is still v5. They move when the box moves, not when the tree does.

---

## v5 - Sine-on-NI + Shock-on-NI, wire bump #4

**Three modules' surfaces, jointly authored, landed in ONE version increment.** Two
sessions were building against this seam at the same time. Three separate bumps would
have been three coordinated daemon-then-app redeploys, each with its own
connect-refusal window, and the middle two would have been refusing peers over surfaces
nobody was using yet. Batching is the cheap option under an exact-match gate; that is
the same argument v2, v3 and v4 each made, and it is why this file exists.

**Deploy order is DAEMON FIRST, and BOTH ENDS MOVE.** The daemon restarts before the
Mac app is delivered. Between the two there is a window in which the app is refused at
connect - that is the designed behaviour, not an outage. `REMOTE_PROTOCOL_VERSION` in
`ui/src/lib/ipc/remote.ts` moved 4 -> 5 with the Rust constant, and `remote.test.ts`
pins the number so a drift is visible.

**The wire ACCEPTS shapes the daemon does not yet serve, and says so.** All three
profile variants and all three command groups decode cleanly, clear the schema gate,
and are answered with a typed refusal that NAMES the absent host - never an
acknowledgement, never a silent drop, and never `LocalOnly` (which claims the
capability lives in the desktop app; these will live here). The hosts land per module,
in their own sessions, and each one turns its refusal into an acceptance with no
further wire change.

**SERVED NOW: `ShockClassical`, `Srs`, and both command groups** (2026-08-09,
Shock-on-NI). `hertzforge-backendd` grew `ShockHost` and `SrsHost`, and the paragraph above
went from a promise to a record: the two shock profile arms and both command groups became
acceptances with **no wire change at all** - which is the property the batched bump was
built to have, demonstrated rather than asserted. `Command::Start` means ARM on both
(stated in `hertzforge_protocol::shock`'s module docs before any host existed, and now true);
the four routed level verbs answer per-module; `Response::Shock(Doc)` carries real
documents; and `ModeMatrix.shock` / `.srs` are CLAIMED, citing the chassis gate's
PASS-TWICE sweep rather than the host's existence.

**The three capture verbs did NOT change and will not.** They still answer the
remote-capture refusal verbatim on both groups. That refusal was never about a missing
host - remote capture is refused by design for shock v1 - so a host landing is exactly the
event that must not change it, and the daemon test plus the wire probe both pin the
string.

**Still pending a host: the Sine surface.** `TestProfile::Sine` and the `Sine` command
group continue to answer the named MISSING-HOST refusal, which is the state this section
describes and the reason it is worth keeping.

### 1. `PROFILE_SCHEMA` STAYS 1, deliberately

It is ONE global exact-match number shared by every `TestProfile` variant. Bumping it
to mark the three new variants would have refused the deployed application's stored
Random and Explorer profiles - documents that did not change - mid-workflow, for a
reason that has nothing to do with them. The new variants default `schema_version` to
this value and are gated by `PROTOCOL_VERSION`, which is the gate that actually
distinguishes a peer that knows them from one that does not.

`tests/v5.rs` carries the compatibility row: a v4-era `schema_version: 1` Random
document still decodes and still clears `check_schemas` on a v5 build. A profile from
the future is still refused, so the row is a decision and not a hole.

### 2. `TestProfile::Sine(SineWireProfile)` - the closed-loop sine surface

A field-for-field WIRE MIRROR of `hertzforge_sine::service::config::SineTestConfig`: 23
fields over 29 nested types, `deny_unknown_fields` throughout, `schema_version` on the
outermost payload only.

Typed rather than an opaque `Doc` (the recorder-configuration posture) because this is
the payload that points a shaker, every field is a scalar or a small enum, and the
`FeasibilityInputs` precedent already says so. The fidelity fence has three parts,
stated in `sine.rs`'s module docs: every operator-settable config field appears on the
wire; the daemon's translation destructures BOTH structs exhaustively so a field added
on either side is a compile error rather than a silent default; and `tests/v5.rs`
round-trips a fully-populated profile. That is what keeps sine out of the
authored-on-screen-and-silently-defaulted hole.

**Two DELIBERATE duplicate definitions, named rather than left to be discovered.**
`SineSweepRate` is the third definition of `hertzforge_dsp::sweep::SweepRate` in the tree
(the real enum, `hertzforge-sine`'s serde remote-derive, and now this) and
`SineBandwidthMode` the third of `BandwidthMode`. This crate's dependency floor is
serde and nothing else, so it can neither import nor remote-derive from `hertzforge-dsp`.
The fence is the round-trip corpus plus the exhaustive converter: a variant added to
the dsp enum and not here stops the converter compiling. `SweepDirection` is NOT
duplicated - the Explorer's mirror is reused.

**Not on the wire, and why.** The ladder carries no `manual` flag: AUTO-vs-MANUAL is
live-switchable through `Command::SetLadderMode` and lives as RUN state on the core's
`LadderPlan`, so a configure-time copy would be a second authority that disagrees with
the live one the moment an operator takes over. `review_hold` sits at the top level of
the profile because that is where `SineTestConfig` carries it.

`Command::Sine(SineCommand)` = `Hold` / `Resume` / `Reverse` / `SetBlendW{w}` /
`OverrideFeasibility{ack}` - the five intents the general verb set cannot express.
Everything else stays general: `Start`/`Stop`/`Abort` run it, `GoToFrequency` /
`AdjustFrequency` move the tone, `SetLevel{Db}` jumps a rung, `AdjustLevel` trims it,
`SetLadderMode` hands the schedule over, `ProceedAfterReview` releases the hold.

**`ProceedAfterReview` releases `AwaitingReview` and NOTHING ELSE.** The sine service
also has a `CoherenceHold`, which is the gate FAILING. Mapping one verb onto both would
have shipped a one-click bypass of a safety refusal, so there is deliberately no wire
verb that clears a `CoherenceHold` - only a core-side Reset does.

### 3. `TestProfile::ShockClassical` + `TestProfile::Srs` - two profiles, not one

Classical shock states a PULSE and is judged in the time domain; SRS states a TARGET
SPECTRUM and is synthesized to match it. One variant carrying both would have half its
fields meaningless for whichever test was running, and "meaningless" is one refactor
away from "silently defaulted". Both mirror their service configs, both
`deny_unknown_fields`, both `schema_version`-defaulted.

**Both carry `max_drive_vpk`, defaulted to 5.0** (`DEFAULT_SHOCK_MAX_DRIVE_VPK`) - the
operator's ceiling, enforced whatever the station is. An omitted field defaults to a
real number rather than to zero or to unbounded.

**The classical profile carries the `RateModel` as an OPERATOR INPUT**, not a hidden
core default. Its floors are the cooling, settling and compute times for a particular
shaker in a particular room, and they decide how fast a train may legally fire. A train
refused by a number nobody can see is a refusal nobody can act on.

**The capture block is ABSENT from both, and that is a safety decision.**
`CaptureRules::auto_start` would start a disk writer at ARM on the core host; over this
seam the operator cannot see the disk, cannot see the writer die, and has no verb to
stop it. The daemon FORCES `auto_start = false` at translation, and the wire has no
field a hand-written client could use to flip it back.

`Command::Shock(ShockCommand)` = `FirePulse`, `RunAuto{max_pulses}`,
`JumpToLevel{index}`, `ArmWithOverride{ack}`, `Reset`, `PulseList`, `PulseDetail{seq}`,
`SrsFreqs`, `FireTrain{spec}`, `FireGunfireBurst`, plus the three capture verbs.
`Command::Srs(SrsCommand)` is the same core minus the two train verbs plus `RunSysId`.

**`Command::Start` means ARM on the Shock and Srs dispatch arms.** A shock run has no
continuous running state to start: it arms, and then pulses are FIRED one verb at a
time. The Explorer set the precedent that `Start` means "whatever this module does when
the operator says go"; stating it here is what stops a UI waiting for a `Running` that
never arrives.

**`PulseList` / `PulseDetail` / `SrsFreqs` are READ-ONLY and Observer-safe.** They are
the only shock verbs that are. `SrsFreqs` is a QUERY rather than configure-time-only
output because the two peers who most need the frequency axis never saw the configure:
an observer that joined mid-run, and a controller that reconnected after a vanish.

### 4. The three capture verbs are PRESENT AND REFUSING

`StartCapture` / `StopCapture` / `SaveSnapshot` exist on both shock groups and the daemon
answers a named refusal carrying the remote-capture refusal text verbatim. An ABSENT verb
would fail to decode and cross back as `MalformedPayload`, which reads to an operator as
"your client is broken" rather than "this is not built yet". The wire probes assert the
string, so it is a contract rather than a log line.

They are classified MUTATING in `is_mutating` even though they are refused today: the
classification answers what the verb IS, not whether it currently works, and a verb
filed read-only because its implementation happens to be a refusal becomes
Observer-reachable the moment someone implements it. They claim no `ActionKind` row.

**This refusal is DIFFERENT from the missing-host one, and the daemon keeps them
apart.** A missing-host refusal expires when the module lands; this one does not.

### 5. `StatusReport` gains nine additive readouts

Four sine: `sweeps_done`, `thd_pct`, `hold`, `notch_active`. Five shock:
`pulses_fired`, `pulses_at_level`, `train_remaining`, `last_pulse_seq`, `last_pulse`.
All `#[serde(default, skip_serializing_if)]`, all present in `StatusReport::idle()`
(a complete literal with no `..`, so the compiler forces the decision).

`hold` is a STRING and not a `RunState`: a held sine run is still driving at the held
frequency and level, so mapping it onto `Stopping` or `Armed` would tell a UI the
output had stopped when it had not. `notch_active` is three-valued for the same reason
`sweep_active` is - a run under target BECAUSE a notch is holding it there is not a run
failing to reach target, and an operator who cannot tell those apart chases the wrong
problem.

`last_pulse` is a status-borne `Doc`, not a new `Event` variant. The trade, stated: a
per-pulse event would be new wire machinery whose only advantage is push semantics the
status path already has, and it would be LOST across a reconnect - whereas a status
field survives, so a controller that vanished and came back still sees what the last
pulse did.

`module_state` tokens are NAMESPACED by prefix - `feasibility_blocked: <reason>` and
`awaiting_review: <stat>`, one `Option<String>`, one token at a time. No new `RunState`
variant: a review hold maps to `Armed` plus the token, which is Random's precedent.

### 6. `Response::Shock(Doc)` and two new `SchemaKind`s

`Response::Shock` answers the read-only shock queries, following `Response::Recorder`'s
precedent. `SchemaKind` gains `ShockPulse` and `ShockSrsGrid`, both at schema 1.

**The shock kinds are STAMPING and CHECKING labels, never wire tags.** `SchemaKind`
does not appear on a `Doc` - a `Doc` is a schema number and a JSON string and nothing
more. The kind exists so the sender stamps the right constant, the receiver checks the
right one, and a refusal can say which document is at fault. A client answers the query
it asked; it does not read a kind off a document and dispatch on it.

**The one-directional additivity invariant, recorded:** a new `Response`, `Event` or
`SchemaKind` variant may only be emitted in reply to a command that only a NEW peer can
send. An old peer never sends a shock verb - the version gate refused it at connect -
so it can never receive a response shape it cannot decode.

### 7. Boxing, and why the wire did not change

`Response::Status`, `Event::StateChanged.status` and the three new `TestProfile`
variants are now `Box`ed. `StatusReport` grew past the other variants of `Event` and
`Response` and made those enums as large as a full status report - including a display
`Frame`, the event that arrives tens of times a second. `Box` is transparent to serde:
the JSON is identical and no peer sees a difference.

### ADDITIVE growth within v5 (the version did NOT move)

**`SrsCommand::NotchStatus` + `SchemaKind::ShockNotch` at `SHOCK_NOTCH_SCHEMA = 1`**
(2026-08-09). A read-only query answering the predictive-notch readout - the
arm-time notch plan, the limit report, and the predicted limit-channel SRS curves, the
same three the local core's `shock_srs_notch_status` returns. Without it the remote Notch
Status card rendered permanently empty: the plan is core state with no `StatusReport`
field and there was no verb to ask for it.

**Why no field on something already crossing.** Both candidates were checked.
`PulseDetail{seq}` is PER-PULSE and the plan is RUN state computed once at arm.
`PulseList` is run-scoped and would have avoided a new verb, but this readout is polled
beside the status at 5 Hz and `PulseList` carries up to 512 rows - dragging the pulse
table across the link every 200 ms is a worse answer than the empty plan it replaces.

**Why the version did not move, stated in both directions.** The ANSWER is a new
`SchemaKind` variant emitted only in reply to a command that only a NEW peer can send,
which is exactly the one-directional additivity invariant recorded above - satisfied by
construction rather than by care. The COMMAND is the direction that is genuinely new: a
v5 peer built after this can send a verb a v5 daemon built before it cannot decode.
Deploy order is DAEMON FIRST, so that ordering is the abnormal one; when it does happen
the old daemon answers a typed decode refusal and the client LATCHES back to the empty
plan it rendered before the verb existed. That is a degradation an operator can read, on
a read-only verb that points nothing at a shaker, and it costs no coordinated redeploy of
both ends for a display readout.

### What v5 does NOT carry

- **No MIMO profile.** No module serves it.
- **No `Module::Sine` / `Module::Shock` / `Module::Srs` in the daemon.** Those land
  with their hosts, in the sessions building them. The v5 landing deliberately stops at
  the wire plus the shared routing bundle.
- **No remote capture.** Refused by design; see item 4.
- **No new trace ids beyond the sine sysID pair.** `1150` (|H1|, derived) and `1151`
  (coherence, overlay) are reserved for the sine gate publish; the contested 940-999
  range (four claimants) is untouched.

---

## v4 - Explorer-on-NI, wire bump #3

Four changes, batched into ONE version increment, all of them consequences of the same
fact: a host-synthesized open-loop survey is arriving on the daemon side, and the seam
has to be able to carry it before the deployment that would otherwise have to negotiate
it.

**Deploy order is DAEMON FIRST.** The daemon restarts before the Mac app is delivered,
which is the existing routine; an app delivered first would be refused at connect until
the daemon caught up. That is the intended failure, not a regression.

### 1. `TestProfile::Explorer(ExplorerProfile)` - a NEW variant, not a grown `SineProfile`

```rust
struct ExplorerProfile {
    schema_version: u32,          // defaulted, exact-match gated
    fs_hz: f64,                   // REQUESTED; the station may coerce it
    band_hz: (f64, f64),          // every frequency intent clamps here
    start_hz: f64,
    amplitude_vpk: f64,           // 0.0 = ARMED QUIET, the standard posture
    max_drive_vpk: f64,           // the operator's ceiling
    slew: ExplorerSlew,           // { freq_oct_per_s, drive_db_per_s }
    sweep: Option<SweepSpec>,     // { direction, rate_oct_per_min }
    display_nfft: usize,          // DISPLAY only
}
```

`#[serde(deny_unknown_fields)]`, same posture and same reasoning as `SineProfile` and
`RandomProfile` (see v3 item 2).

**Why a new variant rather than four more fields on `SineProfile`.** `SineProfile` is
`deny_unknown_fields`, which makes growing it the opposite of additive: a peer built
before the new field decodes the payload as a hard serde field error, part-way through
a workflow, on the document that points a shaker. A new variant moves that failure to
the connect handshake, where the exact-match rule already refuses cleanly and where a
version mismatch belongs. The bump costs one coordinated restart; the alternative costs
a mid-workflow decode failure on an operator's screen.

**`SineProfile` is left UNTOUCHED and is now DEAD WIRE SURFACE.** No core serves it -
the delegated open-loop path is local-only, and the host-synthesized survey is the
Explorer variant. It is kept rather than removed because the delegated variant is a
circle-back once NI is functional (decided 2026-08-08); removing it would spend a
version bump now and another one to put it back.

**`amplitude_vpk` is required and `0.0` is legal.** The Explorer arms at zero drive and
the operator raises it - that posture is why the module needs no review hold (nothing
moves until a human commands a level). A `skip_serializing_if` reflex that treated zero
as absent would turn the safe posture into a decode failure or, worse, into a default;
`tests/v4.rs` pins the zero on the wire.

**`slew` is per SECOND, not per tick.** A tick-denominated rate is a device mechanic
wearing an intent's clothes: it silently changes meaning when the block size or the
adopted sample rate changes. The core converts to per-frame increments from the rate it
ACTUALLY got.

### 2. The `Explorer(ExplorerCommand)` group

```rust
enum ExplorerCommand {
    StartSweep { direction: SweepDirection, rate_oct_per_min: f64 },
    StopSweep,
}
enum SweepDirection { Up, Down }
```

Mirrors `RecorderCommand`'s shape and answers the same three questions the exhaustive
matches force: both variants are MUTATING (either moves, or stops moving, a shaker -
an Observer that could start a sweep is an Observer that can walk someone else's
shaker across a resonance), both map to `ActionKind::GoToFrequency` rather than adding
an `ActionMatrix` row (same trade as `SetLadderMode` -> `AdjustLevel`: a backend that
serves absolute frequency intents is exactly one that can be walked between two of
them), and neither carries a versioned document.

**Only the sweep is here.** Everything else the Explorer needs already exists as
operator intent and stays there: `GoToFrequency` / `AdjustFrequency` move the tone,
`SetLevel { DriveVolts }` moves the drive, `Start` / `Stop` run it. A parallel
`ExplorerCommand::SetDrive` would be a second way to say one thing, which is how a UI
and a core end up disagreeing about what the operator asked for. A sweep is different:
"traverse the band at N octaves per minute until an edge or a stop" has no expression
in a setpoint verb, and expressing it as a stream of `GoToFrequency` calls would put
the ramp on the network - the one thing this seam does not do.

`StopSweep` HOLDS the frequency where the sweep reached; it does not return to the
start and it is not a run stop. An operator stops a sweep at the moment they see
something, and moving the tone off that spot destroys what they stopped to look at.

### 3. `StatusReport`: `drive_vpk`, `sweep_active`, and FILLED `frequency_hz`

Both new fields are `#[serde(default, skip_serializing_if = "Option::is_none")]` - the
crate's established additive pattern, absent on an older daemon and ignored by an older
front end.

- `drive_vpk: Option<f64>` - commanded drive, volts PEAK: the open-loop counterpart of
  `level_db`. Carried BESIDE it rather than folded into it, because a UI that shared one
  field between two bases would eventually render a volts number under a dB label. It
  is what the core is COMMANDING, not what the amplifier delivered.
- `sweep_active: Option<bool>` - THREE-VALUED on purpose: `Some(true)` sweeping,
  `Some(false)` a sweep-capable module is live and parked, `None` no such module is
  live. A plain `bool` would fold "parked" and "not applicable" into one `false`, and
  the front end needs them apart: it suppresses spectrum averaging while sweeping (a
  moving tone must not smear) and must not suppress it for a module that never sweeps.
- `frequency_hz` is UNCHANGED on the wire and gains stated FILLED semantics: it is
  filled by whichever module is live and frequency-bearing - the same authority that
  answers a `GoToFrequency` - and it is the frequency the core is SYNTHESIZING, never
  the last value a front end requested. A readout that echoed the request would show an
  operator their own intent while the tone sat clamped at a band edge.

### 4. `Command::check_schemas` is EXHAUSTIVE

It ended in a `_ => Ok(())` wildcard, which is the shape where a new document-carrying
verb compiles clean and silently skips its schema gate. Every
variant is now listed; forgetting is a compile error rather than a runtime surprise.
No wire change - recorded here because it is the gate the wire depends on.

### What v4 does NOT carry

The daemon does not serve the Explorer yet. The wire surface exists so the seam changes
ONCE, but `SetProfile(Explorer)` and both sweep verbs answer `LocalOnly` naming the
reason ("the Explorer host lands after its chassis gate"), and the frequency /
drive-volts intents answer a routed `Refused` naming the core's state ("no
frequency-bearing module is live on this core"). Capability claims follow evidence, not
the wire: `Core::capabilities()` still claims nothing, and the Explorer
rows land only after the on-chassis gate is a recorded PASS.

---

## v3 - NI buildout, wire bump #2

Seven changes, batched into ONE version increment, all of them consequences of the
same fact: the front end and the core are now different machines.

### 1. `Subscribe` gains SELECTORS

```rust
// v2
Subscribe { streams: Vec<StreamKind> }
// v3
Subscribe { subs: Vec<StreamSub> }

struct StreamSub {
    kind: StreamKind,
    channels: ChannelSel,          // All | Ids(Vec<u32>) | Group(u32)
    max_rate_hz: Option<f64>,
    bin_decimation: Option<u32>,
}
```

The v2 behavior is exactly `StreamSub::all(kind)`, so the migration is mechanical.

In one process, sending every channel of every stream costs a memcpy. Across a network
it costs the link, and at the 400-channel posture the design targets a full waveform
stream is not a display feed - a laptop screen has perhaps a dozen useful traces on it.
The selectors are honored by the PRODUCER (bytes never produced) rather than filtered
at the consumer (bytes not shown); a selector honored at the receiver saves nothing.

`tests/v3.rs` states the budget the gate asserts: **1.0 MB/s per subscribed stream**,
an order of magnitude under a 100 Mbit/s lab link, so the display path cannot crowd out
the command path. The test's anti-vacuity half shows the same 400-channel source
subscribed the v2 way is two orders of magnitude over it.

Normalization rules, all tested: a `max_rate_hz` of `0`, negative, or NaN means NO
ceiling rather than silence (a display that goes dark from an arithmetic bug is
indistinguishable from a dead link); `bin_decimation` of `None`/`0`/`1` all mean every
bin; bin 0 is always kept; decimated bin counts round UP. `ChannelSel::Group(0)` does
NOT match an ungrouped channel - "no group" is not "group 0".

### 2. `schema_version` on profiles, and the unknown-field answer

`SineProfile` and `RandomProfile` gain `schema_version` (defaulted, so payloads written
before the field are not spuriously refused within one wire version) and
`#[serde(deny_unknown_fields)]`.

**The deny is the "unknown-field diagnostic" the design called for, and the shape was
chosen deliberately.** The obvious alternative - a `#[serde(flatten)]` capture map that
accepts the payload and reports the leftover keys - was rejected twice over. First, it
is the silent best-effort parse this seam bans one layer up: "we ignored part of your
profile and ran anyway", on the payload that points a shaker. Second, `flatten`
requires a self-describing codec and would have quietly voided this crate's stated
ability to ride non-self-describing ones. Refusing costs nothing and says more: serde's
own message names the offending field, and `ProtocolError::MalformedPayload` carries
that sentence verbatim to the operator.

`TestProfile::check_schema` and `Command::check_schemas` are the gates; the latter is
one match over every document-carrying command, so a new one cannot forget.

### 3. Opaque VERSIONED documents: `Doc` and `TableDoc`

The channel table lives in `hertzforge-channel-table` and a recorder configuration in
`hertzforge-recorder`; neither may be imported here (dependency floor). Mirroring them the
way `Caps` is mirrored would be a bad trade - a live document with its own migration
story and an 18-column CSV contract is not a stable 40-field hardware posture. So they
cross as a JSON string plus the schema version they were written at, which is the
`FrameBytes` posture applied to text.

`SchemaKind::check` is exact-match, same posture as the protocol version, and the two
answer different questions: the protocol version says "we speak the same wire", a
schema version says "we agree on what is inside this document". Two peers can be on
the same wire and still disagree about a channel table.

### 4. The RUN LEASE and the shared token, on the handshake

`Hello` gains `run_role: RunRole` (defaulting to `Observer`, the least authority),
`accept_observer_downgrade: bool` (defaulting to `false`), and `token: Option<String>`.
The core answers with a new `HelloAck` carrying a `LeaseAnswer`.

**One Controller, N Observers.** `Command::is_mutating` is the single exhaustive
statement of which commands the lease gates - one match a reviewer can read, rather
than a condition repeated at every handler, and a new verb has to answer it to compile.

**The second-Controller decision (the design said pick one and document it): REFUSE by
default, downgrade only on request.** A UI that believes it is in control and silently
is not renders enabled Start/Stop/Abort controls that do nothing - the operator presses
Abort, the button depresses, nothing happens. A refused connection naming the current
holder is strictly better. The downgrade exists for observer-first clients, must be
asked for, and is always REPORTED (`ObserverReason::Downgraded`), never inferred.

`Abort` is classified as MUTATING. It is supervisory-only over this seam and never the
primary abort authority, but that is an argument about what it is for, not about whether
it changes state. The answer for a peer without the lease is the physical E-stop, which is
in the room with the hardware.

The token is a shared secret over a plaintext socket on a trusted lab network - not a
security system, the door that stops an accidental second workstation from reaching a
shaker. A BLANK configured token refuses everyone rather than admitting everyone; the
failure mode of reading a mis-edited config as "no authentication required" is a shaker
on an open port.

### 5. The capture LIST verb

`Command::ListCaptures` -> `Response::Captures(Vec<CaptureInfo>)`. Metadata only:
captures stay daemon-local and fetch-to-front-end is deliberately deferred (pulling a
multi-gigabyte binary across the link that carries the display path, during a run,
deserves its own design). `CaptureInfo` carries `station_label`, `run_id` and
`complete` alongside the size, because a list that omitted them would let a remote
operator treat a truncated driver-simulated run as a measurement.

### 6. Channel-table get/put with ETAG CONCURRENCY

`GetChannelTable` / `PutChannelTable { table: TableDoc }` ->
`Response::ChannelTable(TableDoc)`. The CORE is authoritative: the table is what the
running acquisition is bound to, and it lives where the hardware lives.

Every read hands back an etag; every write quotes the etag it was derived from; a write
quoting anything else is `ProtocolError::StaleUpload` naming both. **Last-write-wins is
not an option on a document that carries abort limits** - two operators (or one with a
stale tab) would otherwise silently erase each other's edits. A successful write
answers with the table AS STORED plus its new etag, so a core-side migration is visible
immediately rather than on the next read.

### 7. The Recorder command group, and six new typed refusals

`Command::Recorder(RecorderCommand)` covers the arrival-day remote minimum
(configure / monitor / arm / record / stop, plus read-only status, meters, FRF,
recordings). It is the ONLY module group in v3.

New `ProtocolError` variants, all typed because a UI cannot branch on prose:
`Unauthorized`, `NotController`, `StaleUpload`, `UnsupportedSchema`, `MalformedPayload`,
`LocalOnly`.

`LocalOnly` is the honesty verb and the reason the module coverage being partial is
safe: every module the daemon does not serve is an explicit refusal that a remote UI
renders as "local-only in remote mode". The alternative - falling back to a local
backend when the remote one has no answer - would put the run on different hardware
than the operator selected, which is the same class of defect as a Sim fallback.

### New events

`RunProvenance` (the remote half of "what IS this data" - an event, not a status
field, so a UI attaching mid-run receives it on connect), `LeaseChanged` (the only way
an Observer learns the lease came free without polling), and `SafeStopping` (WHY a run
is coming down, broadcast to every peer including the Observers who would otherwise
see it end for no visible reason; always the controlled ramp-down path, because
"nobody is watching" is a reason to stop safely, never a reason to slam the drive to
zero).

---

## v2 - NI buildout, wire bump #1

Four changes, deliberately batched into ONE version increment.

### 1. `ModeSupport` mirrors the two-axis loop resolution

`ModeSupport` was three valueless variants (`Hosted` / `Delegated` / `Unsupported`),
mirroring the original single-axis loop resolution. A 2026-07-16 design change split
loop resolution into two axes, and the single axis could not name the
cells that actually exist:

- `{DeviceReplay(mode), HostBetweenRuns}` - the device plays a downloaded record and
  the HOST iterates between plays (TWR on a smart controller). Calling this "Delegated"
  told the UI the device was closing a loop it is not closing.
- `{DeviceReplay(mode), None}` - frozen playback with nothing correcting. Also
  "Delegated" under v1, and materially different: the UI must not draw adaptation
  affordances for it.
- `{None, None}` - the Recorder's capture-only shape, which needed no special case once
  the axes were separate.

v2 shape:

```rust
enum ModeSupport {
    Supported(Resolution),          // { drive: DriveOrigin, adaptation: AdaptationOwner }
    Unsupported(UnsupportedReason), // TYPED, not a bare "no"
}
```

`DriveOrigin` = `None` | `HostSynthesized` | `DeviceReplay { mode }`.
`AdaptationOwner` = `None` | `HostPerTick` | `HostBetweenRuns` | `Device`.
`UnsupportedReason` mirrors `hertzforge_module_abi::UnsupportedReason` (no native mode /
clock not shared / response not observable / no abort-latency bound / delegation target
is a host loop / no host drive path / nothing to adapt / rate mismatch). It is typed
because the operator has to know WHICH precondition failed in order to act on it - plug
in a clock route, or regenerate a profile - and a UI cannot branch on prose.

`StationClass` (`HertzforgeSim` / `NiSimulated` / `NiReal`) also crosses the seam now,
because the UI badges the three differently and the abort-latency rule binds only the
hardware two.

**Mirrored, never imported.** `hertzforge-protocol` depends on `serde` and nothing else;
`hertzforge-module-abi` is a hardware-side crate. The types are duplicated as this crate's
own serializable shapes and kept aligned by review, exactly like the existing `Caps`
mirror. Diff against `crates/hertzforge-module-abi/src/lib.rs` when either side changes.

**Consequences for consumers.** `ModeMatrix` is no longer `Copy` (the refusal reason
carries a `Vec`), and `ModeMatrix::support` returns `&ModeSupport`. The v1 questions
survive as `ModeSupport::is_hosted()` / `is_delegated()`, and the v1 shapes as
`ModeSupport::hosted()` / `delegated(mode)`, so the migration is mechanical for anyone
who only asked those.

`CapabilityDescriptor::inconsistencies` gained a check the single-axis form could not
express (`AdaptationWithoutDrive`), and one it had wrong: the shared-clock requirement
now binds `AdaptationOwner::HostPerTick` rather than a hard-coded list of "closed-loop"
modes. Under v1 it demanded a shared clock for modes the host merely synthesizes
open-loop, and could not demand one for a per-tick-corrected mode outside that list.

### 2. The binary frame observability block

Every binary display frame (`ScopeFrame`, `WaveformFrame`, `SpectrumFrame`, and the
batched `MCS1` multi-channel spectrum) gained a 20-byte block appended to the END of its
fixed header, in the same field order for all four:

```
+0   u32   seq            monotonic per-STREAM frame counter
+4   u64   sample_index   source frame counter at capture; 0 == not available
+12  u64   run_id         the run this frame belongs to; 0 == unattributed
```

Header sizes: Scope / Waveform / Spectrum `24 -> 44`; `MCS1` `28 -> 48`. Full layouts,
per-feed `sample_index` sources, and the `0`-means-absent conventions are in
`ui/IPC_CONTRACT.md` ("Frame observability block").

The frames cross this crate as opaque `FrameBytes` and are NOT re-encoded here - the
protocol is a pipe. They are listed in this changelog because they are versioned by the
same number: `ui/IPC_CONTRACT.md` states the wire version, and the handshake's
exact-match rule is what stops a v1 webview from misparsing a v2 header.

### 3. `MCS1` is the universal spectrum layout

The batched multi-channel layout is documented as the format every per-module spectrum
feed can use, and both codecs (Rust packer, TS decoder) support it fully. The per-feed
MIGRATIONS onto it are later work; as of v2 only the delegated-Random measured PSDs
emit `MCS1`. Documented here rather than at migration time so the wire changes once.

### 4. `Hello` carries a `CoreStamp`

```rust
struct CoreStamp { sha: String, dirty: bool, host: String, target: String }
```

Mirrored from `hertzforge_buildinfo::BuildInfo` (same dependency-floor rule as the `Caps`
mirror). It answers "which core is actually running", which matters most in the
situation where you cannot ask: a bench session where a fix was built on one machine,
copied to another, and the symptom did not change.

**The stamp is COMPARED AND REPORTED, NEVER ENFORCED.** The exact-match gate stays on
the `u16` version alone. A stamp mismatch is a logged WARNING plus a surfaced field, not
a refusal - two dev machines legitimately differ, and the split deployment is a Mac
front end talking to a Linux core, which differs in host and target BY DESIGN. Making
the stamp a gate would break the intended topology on day one.

`Hello::compare_stamp` is three-valued (`Match` / `Differs` / `Unknown`) so a build
with no git tree reports honestly instead of being folded into "mismatch".
`Hello::stamp_report` produces the log line, naming both stamps.

The field is `#[serde(default)]`, so an encoder that omits it still decodes - within
v2. It is not a compatibility shim across the v1/v2 boundary; the version gate is.
