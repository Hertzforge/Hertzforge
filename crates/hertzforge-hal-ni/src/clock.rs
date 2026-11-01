//! The clock rule: when may this backend claim `ClockDomain::Shared`?
//!
//! `Shared` means the input and output run on the SAME physical clock. It is the
//! precondition for every closed-loop mode, so claiming it without evidence
//! deletes the only gate standing between a built-but-unproven control path and
//! a real armature. Two tiers, and only two:
//!
//! ## Tier 1 - driver-simulated device sets: `Shared`
//!
//! When the DRIVER reports every device in the task set as simulated
//! (`DAQmxGetDevIsSimulated`, read - never asserted by a human, never inferred
//! from a name), the answer is `Shared`. This is not a widened gate, it is an
//! honest statement about a different object: a simulated device has no physical
//! clock, so the physical-drift failure mode the gate exists to catch cannot
//! occur, and no armature can be attached to a device that does not exist.
//! Nothing physical is asserted, and the mandatory SIMULATED badge rides every
//! run made this way.
//!
//! What it buys: the pre-arrival NI path for the closed-loop modules. What it
//! must never become: a shippable override. There is no flag, no environment
//! variable, and no config field that turns tier 1 on for a real device - the
//! only input is the driver's own answer.
//!
//! ## Tier 2 - real devices: a per-CONFIGURATION proven set, and nothing else
//!
//! A real device set gets `Shared` only when its exact configuration - chassis
//! product, AI module product, AO module product, and the routing string - is in
//! [`PROVEN_SHARED_CLOCK`], each entry carrying the bench capture that proved it.
//! Anything else is `Unknown`.
//!
//! An entry is added only by a loopback capture (a long tone, minutes, routed AND
//! unrouted as the negative control, accumulated phase flat), and a different module
//! pair or a different chassis RE-PROVES. A one-time global flip outlives its
//! evidence; a keyed set does not.

use hertzforge_hal::ClockDomain;
use std::time::Duration;

// ===========================================================================
// The provisional `[H]` abort-latency bound
// ===========================================================================

/// NI-9234 delta-sigma input delay, SECONDS-per-sample-rate term.
///
/// **Source:** NI 9234 Operating Instructions and Specifications, "Input Delay"
/// / filter-delay specification, published as `38.4/fs + 3.2 us` (fs = the
/// module's data rate). The `38.4` is the decimation filter's group delay
/// expressed in SAMPLES; the `3.2 us` is the fixed analog/digital pipeline term.
///
/// `[H] PROVISIONAL` - a datasheet number, not a measurement on this rig.
///
/// **Gate 5 VALIDATED this bound on 2026-08-01 rather than replacing it.**
/// Measured on the DMM6500 digitizing the tee'd AO0 node (independent of the
/// abort, unlike Hertzforge's own AI, which `emergency_stop` kills along with the
/// drive), cDAQ-9178 + NI 9234 + NI 9263, 8533.33 Hz:
///
/// | phase | measured |
/// |---|---|
/// | limit crossing -> drive stops rising | 50.3 ms (the committed AO queue) |
/// | shutdown taper to zero | 23.1 ms (`ABORT_RAMP_MS` = 25) |
/// | **total, crossing -> drive at zero** | **73.4 ms** |
/// | final level after abort | 0.000912 V |
///
/// 73.4 ms measured against 160.75 ms derived: the bound is conservative in the
/// right direction, so it STAYS as the arm gate's refusal threshold. The
/// derivation was never wrong, it was pessimistic, which is what a safety bound
/// is for. What gate 5 did change is what happens INSIDE that window - see
/// `NiBackend::ramp_drive_to_zero`; before it, the drive did not go to zero at
/// all, it froze at the trip level.
const NI9234_GROUP_DELAY_SAMPLES: f64 = 38.4;

/// NI-9234 fixed pipeline term, seconds. `[H] PROVISIONAL`, same source.
const NI9234_FIXED_DELAY_S: f64 = 3.2e-6;

/// Extra blocks added to the configured lookahead when bounding abort latency.
///
/// Worst case, an abort is decided just after a block was queued: the block
/// already in the device's buffer must drain (1), plus the block being written
/// when the decision landed (1), on top of the configured lookahead. `[H]` in
/// the same sense as the constants above - a reasoned bound, not a measurement.
const ABORT_EXTRA_BLOCKS: u32 = 2;

/// A paper-derived, conservative abort-latency bound, with its derivation.
///
/// Every field is reported so the arm gate's refusal (or its permission) can be
/// read back to an operator with the arithmetic attached.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisionalAbortLatency {
    /// The bound itself. This is what rides `Caps::abort_latency_bound`.
    pub bound: Duration,
    /// Acquisition-path group delay in FRAMES at the actual rate, rounded UP.
    pub group_delay_frames: u32,
    /// `lookahead_blocks + ABORT_EXTRA_BLOCKS` - the N of `N x block_period`.
    pub blocks_ahead: u32,
    /// Operator/provenance string. Carries the `[H]` tag and the formula, so a
    /// reader can tell a datasheet derivation from a bench measurement without
    /// leaving the UI.
    pub evidence: String,
}

/// Group delay of the 9234-class delta-sigma input path, in SAMPLES, at `fs`.
///
/// `delay_seconds = 38.4/fs + 3.2 us`, so
/// `delay_samples = fs * delay_seconds = 38.4 + 3.2e-6 * fs`.
///
/// Note the shape: the delay is very nearly FIXED IN SAMPLES (the `38.4` term
/// dominates below ~1 MHz), which is why the bound must be recomputed whenever
/// the driver coerces the rate rather than cached as a duration.
///
/// `[H] PROVISIONAL` - see [`NI9234_GROUP_DELAY_SAMPLES`] for the citation.
pub fn group_delay_samples(actual_rate_hz: f64) -> Option<f64> {
    if !(actual_rate_hz.is_finite() && actual_rate_hz > 0.0) {
        return None;
    }
    Some(NI9234_GROUP_DELAY_SAMPLES + NI9234_FIXED_DELAY_S * actual_rate_hz)
}

/// The provisional abort-latency bound for one configuration.
///
/// `bound = group_delay_samples(fs)/fs + N * (block_frames/fs)`, with
/// `N = lookahead_blocks + 2`.
///
/// Returns `None` when an input is unavailable or nonsensical (non-positive
/// rate, zero block). **That `None` is load-bearing**: a backend whose formula
/// inputs are missing yields no bound, and the arm gate then REFUSES a drive
/// path on a hardware station. Nothing deadlocks (every real configuration
/// produces a bound), and the truly-unknown still refuses.
///
/// Applies to BOTH driver-simulated and real device sets: the simulated set is
/// still exercising the same block/lookahead machinery, and giving it no bound
/// would make the NI-SIM station unusable for the very arm-chain proof it exists
/// to provide.
pub fn provisional_abort_latency(
    actual_rate_hz: f64,
    block_frames: usize,
    lookahead_blocks: usize,
) -> Option<ProvisionalAbortLatency> {
    let delay_samples = group_delay_samples(actual_rate_hz)?;
    if block_frames == 0 {
        return None;
    }
    let blocks_ahead = u32::try_from(lookahead_blocks)
        .ok()?
        .saturating_add(ABORT_EXTRA_BLOCKS);
    let block_period_s = block_frames as f64 / actual_rate_hz;
    let seconds = delay_samples / actual_rate_hz + blocks_ahead as f64 * block_period_s;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let group_delay_frames = delay_samples.ceil() as u32;
    Some(ProvisionalAbortLatency {
        bound: Duration::from_secs_f64(seconds),
        group_delay_frames,
        blocks_ahead,
        evidence: format!(
            "[H] PROVISIONAL (paper-derived, NOT measured on this rig). \
             bound = group_delay_samples(fs)/fs + N x block_period, where \
             group_delay_samples(fs) = 38.4 + 3.2e-6*fs (NI 9234 Operating Instructions and \
             Specifications, input delay 38.4/fs + 3.2 us) and N = lookahead_blocks + {ABORT_EXTRA_BLOCKS}. \
             At fs = {actual_rate_hz} Hz, block = {block_frames} frames, N = {blocks_ahead}: \
             group delay {group_delay_frames} frames, bound {:.6} s. \
             REPLACED by the measured value at arrival-day gate 5; until then the physical \
             E-stop remains the safety boundary.",
            seconds
        ),
    })
}

/// One proven-`Shared` configuration.
///
/// # The bar for adding an entry
///
/// An entry here is a claim that a specific real hardware combination has been
/// MEASURED to share a clock. Adding one requires:
///
/// 1. A bench loopback capture on that exact combination - AO into AI, a long
///    tone held for minutes, accumulated phase flat.
/// 2. The NEGATIVE control from the same session: the same measurement with the
///    clock route removed, showing accumulated phase that does NOT stay flat. A
///    measurement that could not have failed is not evidence.
/// 3. `evidence` stating what the capture showed and its date.
///
/// Never add an entry from a datasheet, from a simulated device, or by analogy
/// with a different module pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvenClockConfig {
    /// Chassis product string, or `""` for a chassis-less (single-card) set.
    pub chassis_product: &'static str,
    /// Input module product string.
    pub ai_product: &'static str,
    /// Output module product string, or `""` when the set has no output.
    pub ao_product: &'static str,
    /// The routing string the AO task was configured with.
    pub routing: &'static str,
    /// Bench evidence: what the capture showed, and its date. MUST NOT be empty.
    pub evidence: &'static str,
}

/// Real hardware configurations proven to share a clock.
///
/// This table SHIPPED EMPTY until the first loopback capture (2026-07-30). See
/// [`ProvenClockConfig`] for what an entry costs; the arm refusal ran first on the
/// same bench and refused, which is the evidence that the refusal works.
///
/// The `routing` component is the NAME-NORMALIZED form the backend derives
/// (`/<chassis>/` in place of the operator-renameable device name).
pub const PROVEN_SHARED_CLOCK: &[ProvenClockConfig] = &[ProvenClockConfig {
    chassis_product: "cDAQ-9178",
    ai_product: "NI 9234",
    ao_product: "NI 9263",
    routing: "/<chassis>/ai/SampleClock",
    // Bench, Linux host + cDAQ-9178 (NI 9234 -> NI 9263, direct jumpers): routed
    // 5-min 1 kHz tone accumulated phase FLAT, drift +0.0051 ppm; NEGATIVE control
    // same bench, route removed: -322.6 ppm accumulating - the instrument
    // demonstrably sees unlocked clocks. Corroborated at 700 Hz (settle-excluded
    // refit -0.0000 ppm).
    evidence: "bench capture 2026-07-30: routed 1 kHz tone +0.0051 ppm FLAT over 300 s; \
               negative control (route removed) -322.6 ppm accumulating; 700 Hz \
               corroboration",
}];

/// The identity of one configuration, for lookup against the proven set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockKey {
    pub chassis_product: String,
    pub ai_product: String,
    pub ao_product: String,
    pub routing: String,
}

impl ClockKey {
    pub fn new(
        chassis_product: impl Into<String>,
        ai_product: impl Into<String>,
        ao_product: impl Into<String>,
        routing: impl Into<String>,
    ) -> Self {
        ClockKey {
            chassis_product: chassis_product.into(),
            ai_product: ai_product.into(),
            ao_product: ao_product.into(),
            routing: routing.into(),
        }
    }

    fn matches(&self, e: &ProvenClockConfig) -> bool {
        self.chassis_product == e.chassis_product
            && self.ai_product == e.ai_product
            && self.ao_product == e.ao_product
            && self.routing == e.routing
    }

    fn describe(&self) -> String {
        format!(
            "chassis={:?} ai={:?} ao={:?} routing={:?}",
            self.chassis_product, self.ai_product, self.ao_product, self.routing
        )
    }
}

/// The clock domain for a task set, plus the human-readable basis for the answer.
///
/// The basis string is not decoration: `Caps` has no evidence field (adding one
/// is separate work), so the reason a run is or is not allowed to
/// close a loop is surfaced through `NiBackend::clock_evidence()` and printed by
/// the selftest. An operator who sees a refusal can read WHY.
pub fn resolve_clock(all_simulated: bool, key: &ClockKey) -> (ClockDomain, String) {
    resolve_clock_in(PROVEN_SHARED_CLOCK, all_simulated, key)
}

/// [`resolve_clock`] against an explicit proven set. Exists so the lookup logic
/// is testable with a non-empty table while the SHIPPED table stays empty.
pub fn resolve_clock_in(
    proven: &[ProvenClockConfig],
    all_simulated: bool,
    key: &ClockKey,
) -> (ClockDomain, String) {
    if all_simulated {
        return (
            ClockDomain::Shared,
            format!(
                "driver-simulated device set ({}): no physical clock exists, so the \
                 physical-drift failure mode cannot occur and no armature can be attached. \
                 Driver-asserted via DAQmxGetDevIsSimulated. Every run made this way carries \
                 the SIMULATED badge.",
                key.describe()
            ),
        );
    }
    match proven.iter().find(|e| key.matches(e)) {
        Some(e) => (
            ClockDomain::Shared,
            format!("real hardware, proven-set entry: {}", e.evidence),
        ),
        None => (
            ClockDomain::Unknown,
            format!(
                "real hardware with no bench evidence for this configuration ({}). Shared \
                 requires a loopback capture for this exact chassis/module/routing combination \
                 in hertzforge_hal_ni::clock::PROVEN_SHARED_CLOCK; until then closed-loop modes \
                 refuse to arm.",
                key.describe()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ClockKey {
        ClockKey::new("Chassis-X", "AI-Mod", "AO-Mod", "/Chassis-X/ai/SampleClock")
    }

    #[test]
    fn a_driver_simulated_set_is_shared_and_says_why() {
        let (clock, why) = resolve_clock(true, &key());
        assert_eq!(clock, ClockDomain::Shared);
        assert!(why.contains("simulated"), "{why}");
        assert!(why.contains("no physical clock"), "{why}");
        assert!(why.contains("SIMULATED badge"), "{why}");
    }

    #[test]
    fn the_shipped_proven_set_holds_exactly_the_bench_entry() {
        // The loopback capture on the real bench (cDAQ-9178, 2026-07-30) added
        // the one entry (routed +0.0051 ppm FLAT; negative control -322.6 ppm).
        // The PROVEN pair resolves Shared and cites the capture...
        assert_eq!(PROVEN_SHARED_CLOCK.len(), 1);
        let proven = ClockKey::new(
            "cDAQ-9178",
            "NI 9234",
            "NI 9263",
            "/<chassis>/ai/SampleClock",
        );
        let (clock, why) = resolve_clock(false, &proven);
        assert_eq!(clock, ClockDomain::Shared);
        assert!(why.contains("bench capture 2026-07-30"), "{why}");
        // ...and EVERY other configuration still refuses: a different AO
        // module, an unrouted set, and a different chassis are all Unknown.
        for other in [
            ClockKey::new(
                "cDAQ-9178",
                "NI 9234",
                "NI 9260",
                "/<chassis>/ai/SampleClock",
            ),
            ClockKey::new("cDAQ-9178", "NI 9234", "NI 9263", ""),
            ClockKey::new(
                "cDAQ-9174",
                "NI 9234",
                "NI 9263",
                "/<chassis>/ai/SampleClock",
            ),
        ] {
            let (clock, why) = resolve_clock(false, &other);
            assert_eq!(clock, ClockDomain::Unknown, "{why}");
            assert!(why.contains("refuse to arm"), "{why}");
        }
    }

    #[test]
    fn a_matching_proven_entry_yields_shared_and_cites_its_capture() {
        let proven = [ProvenClockConfig {
            chassis_product: "Chassis-X",
            ai_product: "AI-Mod",
            ao_product: "AO-Mod",
            routing: "/Chassis-X/ai/SampleClock",
            evidence: "loopback_2099-01-01.h5 (phase flat over 300 s; unrouted control drifted)",
        }];
        let (clock, why) = resolve_clock_in(&proven, false, &key());
        assert_eq!(clock, ClockDomain::Shared);
        assert!(why.contains("loopback_2099-01-01.h5"), "{why}");
    }

    #[test]
    fn every_field_of_the_key_is_load_bearing() {
        let proven = [ProvenClockConfig {
            chassis_product: "Chassis-X",
            ai_product: "AI-Mod",
            ao_product: "AO-Mod",
            routing: "/Chassis-X/ai/SampleClock",
            evidence: "capture",
        }];
        // Baseline: the exact key matches.
        assert_eq!(
            resolve_clock_in(&proven, false, &key()).0,
            ClockDomain::Shared
        );
        // Changing ANY one field must drop back to Unknown - a different module
        // pair, chassis, or routing re-proves.
        for changed in [
            ClockKey::new("Chassis-Y", "AI-Mod", "AO-Mod", "/Chassis-X/ai/SampleClock"),
            ClockKey::new(
                "Chassis-X",
                "AI-Other",
                "AO-Mod",
                "/Chassis-X/ai/SampleClock",
            ),
            ClockKey::new(
                "Chassis-X",
                "AI-Mod",
                "AO-Other",
                "/Chassis-X/ai/SampleClock",
            ),
            ClockKey::new("Chassis-X", "AI-Mod", "AO-Mod", ""),
        ] {
            assert_eq!(
                resolve_clock_in(&proven, false, &changed).0,
                ClockDomain::Unknown,
                "key {changed:?} must not match the proven entry"
            );
        }
    }

    #[test]
    fn the_simulated_tier_ignores_the_proven_set_entirely() {
        // Tier 1 is not a lookup; it is a statement about a device with no clock.
        let (clock, _) = resolve_clock_in(&[], true, &key());
        assert_eq!(clock, ClockDomain::Shared);
    }

    // --- the provisional [H] abort-latency bound ------------------------------

    /// RED-MUTATION: drop the `N x block_period` term from
    /// `provisional_abort_latency` (return only the group-delay term) and this
    /// test reds - the bound would stop growing with the block period, which is
    /// exactly the queued-drive contribution the bound exists to cover.
    #[test]
    fn the_provisional_bound_is_monotonic_in_block_period_and_never_zero() {
        let fs = 51_200.0;
        let small =
            provisional_abort_latency(fs, 256, 2).expect("a real configuration has a bound");
        let large =
            provisional_abort_latency(fs, 4096, 2).expect("a real configuration has a bound");
        assert!(
            small.bound > Duration::ZERO,
            "a zero bound would read as 'aborts are instantaneous'"
        );
        assert!(
            large.bound > small.bound,
            "a longer block queues more drive ahead of an abort: {:?} !> {:?}",
            large.bound,
            small.bound
        );
        // ...and monotonic in the LOOKAHEAD too, for the same reason.
        let deeper = provisional_abort_latency(fs, 256, 8).expect("bound");
        assert!(deeper.bound > small.bound);
        // N = lookahead + 2 (the draining block plus the one being written).
        assert_eq!(small.blocks_ahead, 4);
        assert_eq!(deeper.blocks_ahead, 10);
    }

    /// RED-MUTATION: strip the `[H]` tag (or the formula) out of the evidence
    /// string and this test reds. The tag is what stops a datasheet derivation
    /// being read downstream as a bench measurement.
    #[test]
    fn the_bound_is_tagged_h_and_carries_its_formula() {
        let p = provisional_abort_latency(51_200.0, 2048, 2).expect("bound");
        assert!(p.evidence.contains("[H]"), "{}", p.evidence);
        assert!(p.evidence.contains("PROVISIONAL"), "{}", p.evidence);
        assert!(
            p.evidence.contains("38.4") && p.evidence.contains("3.2"),
            "the formula must be readable from the evidence: {}",
            p.evidence
        );
        assert!(
            p.evidence.contains("9234"),
            "the evidence must cite its source: {}",
            p.evidence
        );
        assert!(
            p.evidence.contains("gate 5"),
            "the evidence must say what replaces it: {}",
            p.evidence
        );
    }

    /// RED-MUTATION: make `group_delay_samples` return `Some(0.0)` for a
    /// non-positive rate (instead of `None`) and this test reds - the
    /// truly-unknown configuration would silently acquire a bound of zero,
    /// which the arm gate would then ACCEPT. `None` is what makes it refuse.
    #[test]
    fn missing_formula_inputs_yield_no_bound_rather_than_a_zero_one() {
        assert_eq!(group_delay_samples(0.0), None);
        assert_eq!(group_delay_samples(-1.0), None);
        assert_eq!(group_delay_samples(f64::NAN), None);
        assert!(provisional_abort_latency(0.0, 2048, 2).is_none());
        assert!(provisional_abort_latency(51_200.0, 0, 2).is_none());
    }

    #[test]
    fn group_delay_is_very_nearly_fixed_in_samples() {
        // 38.4 + 3.2e-6*fs: at audio-DSA rates the fs-dependent term is well
        // under one sample, which is why the bound is recomputed from the
        // ACTUAL rate rather than cached as a duration.
        let at_51k = group_delay_samples(51_200.0).unwrap();
        let at_102k = group_delay_samples(102_400.0).unwrap();
        assert!((at_51k - 38.4).abs() < 0.2, "{at_51k}");
        assert!(at_102k > at_51k, "the fixed 3.2 us term still scales");
        assert!((at_102k - at_51k) < 0.5, "but only barely");
        // And it converts to a real duration: 38.4 samples at 51200 Hz ~ 750 us.
        let p = provisional_abort_latency(51_200.0, 2048, 2).unwrap();
        assert_eq!(p.group_delay_frames, 39, "38.4 samples rounds UP to 39");
        // 39ish frames of delay + 4 blocks of 2048 at 51200 = ~160 ms.
        assert!(
            p.bound.as_millis() >= 160 && p.bound.as_millis() <= 165,
            "{:?}",
            p.bound
        );
    }

    #[test]
    fn every_shipped_proven_entry_carries_non_empty_evidence() {
        // This one stays meaningful forever: it goes red the moment someone adds
        // an entry without citing a capture.
        for e in PROVEN_SHARED_CLOCK {
            assert!(
                !e.evidence.trim().is_empty(),
                "proven-clock entry {e:?} has no bench evidence"
            );
            assert!(
                !e.ai_product.trim().is_empty(),
                "proven-clock entry {e:?} names no input module"
            );
        }
    }
}
