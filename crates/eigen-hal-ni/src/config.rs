//! The NI task plan: which channel-table row drives which physical channel, at
//! what rate, in what block size.
//!
//! ## Ordering is EXPLICIT, never implied
//!
//! [`plan_from_table`] takes the physical-channel lists as arguments. It does not
//! scan, it does not guess, and it does not silently drop anything: enabled input
//! rows map to `inputs` in table order, drive rows map to `outputs` in table
//! order, and MORE rows than channels is an ERROR rather than a truncation. A
//! silent ordinal mapping is the watchdog-offset bug class - channel 3's limits
//! applied to channel 4's data - so the caller owns the ordering and the UI makes
//! it editable.
//!
//! ## ONE scaling owner: the host
//!
//! Every input row becomes a plain `DAQmxCreateAIVoltageChan` with a `+/-` range
//! in VOLTS, and **volts is the invariant across the whole system**. The
//! conversion to engineering units happens exactly once, host-side, with the
//! row's own sensitivity - the same arithmetic `eigen_dsp::eu::volts_to_eu`
//! performs for the live scope. [`AiAssign::to_eu`] is that one place, and a
//! unit test pins it bit-for-bit against `eigen_dsp`.
//!
//! There is deliberately NO driver-side scaling path. `DAQmxCreateAIAccelChan`
//! would make the driver apply the sensitivity too, and two owners multiply:
//! a `sensitivity^2` error that is silent, plausible-looking, and would differ
//! from the Sim path by a factor nobody would think to look for. The accel-chan
//! call also bakes ACCELEROMETER semantics into the channel, which is simply
//! wrong for an IEPE force or velocity sensor.
//!
//! ## What `interface` selects
//!
//! Not the channel type - the channel type is always voltage. `interface`
//! selects the FRONT END:
//!
//! - **IEPE** -> AC coupling (the sensor rides a DC bias that must be blocked;
//!   a DC-coupled IEPE channel rails at the bias) plus internal current
//!   excitation at [`NiConfig::excitation_a`] (default 2 mA).
//! - **Single-ended** -> DC coupling, excitation source `None`.
//!
//! Excitation is a CURRENT and is a backend policy value. It is NOT
//! `Channel::iepe_bias_v`, which is the sensor's bias VOLTAGE - a readback
//! quantity for telling a live sensor from an open or shorted one. Handing a
//! bias voltage to DAQmx as an excitation current would be a units error with
//! no diagnostic.

use crate::error::NiError;
use eigen_channel_table::{ChannelRole, ChannelTable, Interface};

/// Default AI buffer depth, in blocks. DAQmx's own default is generous but
/// unstated; sizing it explicitly is a requirement (>= 4x block).
// 8 blocks (was 4): at a 2048-frame block and 51.2 kS/s this is ~320 ms of
// slack. 4 blocks left ~160 ms, and the daemon pump's original fixed-cadence
// sleep drained it to -200279 within seconds (first cross-host wire probe,
// 2026-07-26). The pump is drain-paced now; the deeper ring is cheap insurance
// for every reader on a non-realtime host.
pub const DEFAULT_AI_BUFFER_BLOCKS: usize = 8;

/// Minimum AI ring depth in SECONDS of real time (see
/// [`NiConfig::ai_ring_frames`] for the incident that set it).
pub const MIN_AI_BUFFER_SECONDS: f64 = 2.0;
/// Default AO lookahead, in blocks: how much drive must be queued ahead of the
/// device's read pointer before a run is allowed to be considered fed.
pub const DEFAULT_LOOKAHEAD_BLOCKS: usize = 2;

/// Which FRONT END an input row asks for. Not the channel type - every input
/// row is a voltage channel (see the module note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiInterface {
    /// AC coupling + internal current excitation. For IEPE/ICP sensors of any
    /// kind - accelerometer, force, microphone - because the front-end setting
    /// says nothing about what the sensor measures.
    Iepe,
    /// DC coupling, no excitation.
    Voltage,
}

/// One input row bound to one physical channel.
#[derive(Debug, Clone, PartialEq)]
pub struct AiAssign {
    /// The backend's opaque physical-channel handle (a DAQmx physical-channel
    /// name). Parsed only inside this crate.
    pub physical: String,
    pub interface: AiInterface,
    /// Row sensitivity in mV per engineering unit. Never zero.
    pub sensitivity_mv_per_eu: f64,
    /// Full-scale range in the row's engineering unit. Strictly positive.
    pub max_eu: f64,
    /// The channel-table row this came from, for provenance and error messages.
    /// `None` when the assignment was hand-built (the selftest path).
    pub table_index: Option<u32>,
}

impl AiAssign {
    /// Convert one raw sample (VOLTS, as DAQmx delivers it) to this channel's
    /// engineering unit.
    ///
    /// `eu = volts * 1000 / sensitivity_mv_per_eu`, written in exactly that
    /// order so it is bit-identical to `eigen_dsp::eu::volts_to_eu` - the
    /// system's one EU conversion, used by the live scope and by every Sim
    /// path. The formula is duplicated here rather than called across a layer
    /// boundary (a Layer-3 backend must not depend on Layer-2 DSP), and a unit
    /// test pins the two together so a change to either side goes red.
    ///
    /// Applies to EVERY input row regardless of `interface`: the front end
    /// changes, the scaling owner does not.
    #[inline]
    pub fn to_eu(&self, volts: f64) -> f64 {
        volts * 1000.0 / self.sensitivity_mv_per_eu
    }

    /// The `+/-` channel range handed to DAQmx at creation time, in VOLTS.
    ///
    /// Full-scale engineering units converted through the same sensitivity, so
    /// a reading at the channel's electrical limit is exactly `max_eu`.
    pub fn range_volts(&self) -> f64 {
        self.max_eu * self.sensitivity_mv_per_eu / 1000.0
    }

    /// DAQmx input coupling for this row's front end.
    pub fn coupling(&self) -> i32 {
        match self.interface {
            // IEPE sensors ride a DC bias; DC coupling would rail the channel.
            AiInterface::Iepe => crate::ffi::val::COUPLING_AC,
            AiInterface::Voltage => crate::ffi::val::COUPLING_DC,
        }
    }

    /// DAQmx excitation source for this row's front end.
    pub fn excitation_source(&self) -> i32 {
        match self.interface {
            AiInterface::Iepe => crate::ffi::val::INTERNAL,
            AiInterface::Voltage => crate::ffi::val::NO_EXCITATION,
        }
    }
}

/// One drive row bound to one physical output channel.
#[derive(Debug, Clone, PartialEq)]
pub struct AoAssign {
    pub physical: String,
    /// The channel-table row this came from. `None` when hand-built.
    pub table_index: Option<u32>,
}

/// Everything the NI backend needs to build its tasks, EXCEPT the acquisition
/// rate and block size.
///
/// **Rate and block size are NOT here.** They live in
/// [`eigen_hal::AcquisitionConfig`], handed to [`eigen_hal::Backend::configure`],
/// which answers with the rate the driver actually programmed. Previously the rate
/// rode this struct from the constructor, so there was no seam at which a caller
/// could state a rate and learn what it got; a profile generated at one rate could
/// play at another with nothing in the type system objecting. This struct is now
/// the CHANNEL and FRONT-END plan (which physical channels, what coupling and
/// excitation, what buffer depth in BLOCKS), and it is deliberately blind to how
/// fast those channels run.
#[derive(Debug, Clone, PartialEq)]
pub struct NiConfig {
    pub ai_assignments: Vec<AiAssign>,
    pub ao_assignments: Vec<AoAssign>,
    /// AI buffer depth in blocks.
    pub ai_buffer_blocks: usize,
    /// AO lookahead in blocks; becomes `OutputBufferContract::min_lookahead_frames`.
    pub lookahead_blocks: usize,
    /// Arrival-gate 3's NEGATIVE CONTROL (clock_loopback --unrouted): when set,
    /// the AO task is built on its OWN clock even when a shared chassis route
    /// exists, so the loopback measurement can demonstrate drift on genuinely
    /// independent clocks. Never set on a normal run - an unrouted AO forfeits
    /// the shared-clock claim by construction.
    pub force_unrouted_ao: bool,
    /// `+/-` volts for every drive channel. NI AO modules differ (the common
    /// small ones are +/-10 V); this is the requested range, and DAQmx coerces
    /// with a warning if the device cannot deliver it.
    pub ao_range_v: f64,
    /// IEPE excitation CURRENT in amps, applied to every `AiInterface::Iepe`
    /// row. A backend policy value, never read from the channel table (see the
    /// module note on `iepe_bias_v`).
    pub excitation_a: f64,
}

/// Default drive range. `+/-10 V` is the most common NI AO full scale; a device
/// that cannot do it coerces with a WARNING, which is logged, not fatal.
pub const DEFAULT_AO_RANGE_V: f64 = 10.0;

impl NiConfig {
    /// A config with the default buffer/lookahead/range policy.
    pub fn new(ai_assignments: Vec<AiAssign>, ao_assignments: Vec<AoAssign>) -> Self {
        NiConfig {
            ai_assignments,
            ao_assignments,
            ai_buffer_blocks: DEFAULT_AI_BUFFER_BLOCKS,
            lookahead_blocks: DEFAULT_LOOKAHEAD_BLOCKS,
            force_unrouted_ao: false,
            ao_range_v: DEFAULT_AO_RANGE_V,
            excitation_a: crate::ffi::val::IEPE_EXCITATION_AMPS,
        }
    }

    pub fn n_input_channels(&self) -> usize {
        self.ai_assignments.len()
    }

    pub fn n_output_channels(&self) -> usize {
        self.ao_assignments.len()
    }

    /// AI buffer size in frames (samples per channel), for `block_frames`-frame
    /// blocks. The block size is a parameter now that it lives on
    /// [`eigen_hal::AcquisitionConfig`] rather than on this struct.
    pub fn ai_buffer_frames(&self, block_frames: usize) -> usize {
        block_frames * self.ai_buffer_blocks
    }

    /// The AI ring actually programmed into the driver: the block-count ring,
    /// FLOORED at [`MIN_AI_BUFFER_SECONDS`] of real time at the driver's ACTUAL
    /// rate. A ring sized only in blocks scales with the block, and small blocks
    /// produce absurdly thin rings: at 51.2 kHz the 8x1024-frame ring is 160 ms,
    /// and ordinary startup latency (station build does a live driver re-read
    /// for provenance, then spawns the pump thread) consumed more than that on
    /// the N100 - the task faulted -200279 ~150 ms after start (2026-07-30,
    /// first 8/8 probe attempt after the box outage). Ring depth is startup and
    /// stall HEADROOM, not a latency cost: reads drain from the head as data
    /// arrives regardless of depth. (400-ch note: 2 s x 400 ch x 51.2 kHz x 8 B
    /// is ~330 MB driver-side - fine on a PXIe host; revisit with the scale
    /// posture if it pinches.)
    pub fn ai_ring_frames(&self, block_frames: usize, actual_rate_hz: f64) -> usize {
        let floor = if actual_rate_hz.is_finite() && actual_rate_hz > 0.0 {
            (MIN_AI_BUFFER_SECONDS * actual_rate_hz).ceil() as usize
        } else {
            0
        };
        self.ai_buffer_frames(block_frames).max(floor)
    }

    /// AO buffer size in frames. Sized to the lookahead plus one block of slack
    /// so a full lookahead can be queued and topped up without wrapping.
    pub fn ao_buffer_frames(&self, block_frames: usize) -> usize {
        block_frames * (self.lookahead_blocks + 1)
    }

    /// Frames of drive that must be queued ahead of the device's read pointer.
    pub fn lookahead_frames(&self, block_frames: usize) -> usize {
        block_frames * self.lookahead_blocks
    }

    /// Structural checks that do not need a driver.
    ///
    /// Rate and block-size validity are NOT checked here: they are not this
    /// struct's fields any more. [`crate::backend::NiBackend`] validates the
    /// [`eigen_hal::AcquisitionConfig`] it is handed.
    pub fn validate(&self) -> Result<(), NiError> {
        if self.ai_assignments.is_empty() {
            return Err(NiError::config(
                "no input channels assigned: an NI task with no AI channels cannot acquire",
            ));
        }
        if self.ai_buffer_blocks == 0 {
            return Err(NiError::config("ai_buffer_blocks must be at least 1"));
        }
        if self.lookahead_blocks == 0 && !self.ao_assignments.is_empty() {
            return Err(NiError::config(
                "lookahead_blocks must be at least 1 when drive channels are configured",
            ));
        }
        check_unique(
            self.ai_assignments.iter().map(|a| a.physical.as_str()),
            "input",
        )?;
        check_unique(
            self.ao_assignments.iter().map(|a| a.physical.as_str()),
            "output",
        )?;
        for a in &self.ai_assignments {
            if a.physical.trim().is_empty() {
                return Err(NiError::config("an input assignment has an empty channel"));
            }
            if !(a.sensitivity_mv_per_eu.is_finite() && a.sensitivity_mv_per_eu != 0.0) {
                return Err(NiError::config(format!(
                    "channel {} has sensitivity {} mV/EU; the EU conversion divides by it",
                    a.physical, a.sensitivity_mv_per_eu
                )));
            }
            if !(a.max_eu.is_finite() && a.max_eu > 0.0) {
                return Err(NiError::config(format!(
                    "channel {} has non-positive full-scale range {}",
                    a.physical, a.max_eu
                )));
            }
        }
        for a in &self.ao_assignments {
            if a.physical.trim().is_empty() {
                return Err(NiError::config("an output assignment has an empty channel"));
            }
        }
        // Excitation is a CURRENT in amps. A value in the volt range (a bias
        // voltage pasted into the wrong field) would be orders of magnitude
        // beyond anything a DSA front end sources, so refuse it rather than
        // hand it to the driver.
        if !self.excitation_a.is_finite() || !(0.0..=0.1).contains(&self.excitation_a) {
            return Err(NiError::config(format!(
                "excitation_a is {} A; expected a current in [0, 0.1] A (2 mA is standard). \
                 This field is NOT the sensor's IEPE bias VOLTAGE.",
                self.excitation_a
            )));
        }
        if self.excitation_a == 0.0
            && self
                .ai_assignments
                .iter()
                .any(|a| a.interface == AiInterface::Iepe)
        {
            return Err(NiError::config(
                "excitation_a is 0 A but IEPE rows are configured; an unexcited IEPE sensor \
                 reads as a dead channel, which is indistinguishable from a quiet one",
            ));
        }
        Ok(())
    }

    /// Every distinct device named by the assignments, input devices first, in
    /// first-seen order.
    pub fn devices(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let all = self
            .ai_assignments
            .iter()
            .map(|a| a.physical.as_str())
            .chain(self.ao_assignments.iter().map(|a| a.physical.as_str()));
        for p in all {
            let d = crate::ffi::device_of(p).to_string();
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }

    /// Distinct devices named by the INPUT assignments only.
    pub fn input_devices(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in &self.ai_assignments {
            let d = crate::ffi::device_of(&a.physical).to_string();
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }

    /// Refuse a plan whose channels cannot share one DAQmx timing method.
    ///
    /// Separate from [`NiConfig::validate`] because it needs a fact `NiConfig`
    /// does not carry: which devices sit in which chassis, which only the SCAN
    /// knows. Callers that hold a scan (the station factory, the UI's bind step)
    /// run this too; callers that do not (the hand-built selftest plans) cannot
    /// and must not, because refusing without knowledge is a phantom fault.
    ///
    /// Three refusals, in the order an operator hits them: inputs spanning
    /// families, outputs spanning families, then drive on a different family
    /// than the measurement. See [`crate::timing`] for what a family is.
    pub fn validate_timing(
        &self,
        families: &crate::timing::TimingFamilyMap,
    ) -> Result<(), NiError> {
        let inputs: Vec<String> = self
            .ai_assignments
            .iter()
            .map(|a| a.physical.clone())
            .collect();
        let outputs: Vec<String> = self
            .ao_assignments
            .iter()
            .map(|a| a.physical.clone())
            .collect();
        families.refuse_mixed("input", &inputs)?;
        families.refuse_mixed("output", &outputs)?;
        families.refuse_split_io(&inputs, &outputs)
    }

    /// Distinct devices named by the OUTPUT assignments only.
    pub fn output_devices(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for a in &self.ao_assignments {
            let d = crate::ffi::device_of(&a.physical).to_string();
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }
}

fn check_unique<'a>(it: impl Iterator<Item = &'a str>, what: &str) -> Result<(), NiError> {
    let mut seen: Vec<&str> = Vec::new();
    for p in it {
        if seen.contains(&p) {
            return Err(NiError::config(format!(
                "physical channel {p} is assigned to two {what} rows; DAQmx would refuse the \
                 duplicate and an aliased assignment silently mixes two rows' scaling"
            )));
        }
        seen.push(p);
    }
    Ok(())
}

/// Build a task plan from a channel table plus EXPLICIT physical-channel lists.
///
/// Mapping rules, all of them deliberate:
///
/// - Only `enabled` rows participate. A disabled row consumes no physical channel.
/// - `Control` / `Monitor` / `Reference` rows are INPUTS, taken in table order and
///   bound to `inputs[0..]` in order.
/// - `Drive` rows are OUTPUTS, taken in table order and bound to `outputs[0..]`.
/// - More rows than channels is an ERROR. Never a truncation.
/// - Fewer rows than channels is allowed: the surplus physical channels are
///   simply not part of the task.
///
/// A row's `interface` selects its FRONT END (coupling + excitation) and nothing
/// else; its `units` are not consulted at all, because the channel is a voltage
/// channel either way and the unit only appears in the host-side conversion. An
/// IEPE force or velocity sensor is handled exactly like an IEPE accelerometer.
pub fn plan_from_table(
    table: &ChannelTable,
    inputs: &[String],
    outputs: &[String],
) -> Result<NiConfig, NiError> {
    let mut ai_assignments = Vec::new();
    let mut ao_assignments = Vec::new();

    for ch in table.channels.iter().filter(|c| c.enabled) {
        match ch.role {
            ChannelRole::Drive => {
                let physical = outputs.get(ao_assignments.len()).ok_or_else(|| {
                    NiError::config(format!(
                        "channel-table row {} is the {} enabled drive row but only {} output \
                         channel(s) were supplied; assign one physical channel per drive row \
                         (rows are never silently dropped)",
                        ch.index,
                        ordinal(ao_assignments.len() + 1),
                        outputs.len()
                    ))
                })?;
                ao_assignments.push(AoAssign {
                    physical: physical.clone(),
                    table_index: Some(ch.index),
                });
            }
            ChannelRole::Control | ChannelRole::Monitor | ChannelRole::Reference => {
                let physical = inputs.get(ai_assignments.len()).ok_or_else(|| {
                    NiError::config(format!(
                        "channel-table row {} is the {} enabled input row but only {} input \
                         channel(s) were supplied; assign one physical channel per input row \
                         (rows are never silently dropped)",
                        ch.index,
                        ordinal(ai_assignments.len() + 1),
                        inputs.len()
                    ))
                })?;
                // The row's UNIT is not consulted here, and that is the point:
                // the channel is a voltage channel whatever the sensor measures,
                // and the unit only ever appears in the host-side conversion via
                // the row's sensitivity. An IEPE force or velocity sensor works
                // exactly like an IEPE accelerometer.
                let interface = match ch.interface {
                    Interface::Iepe => AiInterface::Iepe,
                    Interface::SingleEnded => AiInterface::Voltage,
                };
                ai_assignments.push(AiAssign {
                    physical: physical.clone(),
                    interface,
                    sensitivity_mv_per_eu: ch.sensitivity_mv_per_eu,
                    max_eu: ch.range_max,
                    table_index: Some(ch.index),
                });
            }
        }
    }

    let cfg = NiConfig {
        ai_assignments,
        ao_assignments,
        ai_buffer_blocks: DEFAULT_AI_BUFFER_BLOCKS,
        lookahead_blocks: DEFAULT_LOOKAHEAD_BLOCKS,
        force_unrouted_ao: false,
        ao_range_v: DEFAULT_AO_RANGE_V,
        excitation_a: crate::ffi::val::IEPE_EXCITATION_AMPS,
    };
    cfg.validate()?;
    Ok(cfg)
}

/// [`plan_from_table`] plus the timing-family refusal, for callers that hold a
/// scan.
///
/// This is the planner the production path uses. The 3-argument
/// [`plan_from_table`] stays for callers with no device tree (the hand-built
/// selftest plans, the unit tests below); it produces the same plan and simply
/// cannot make a claim about chassis membership it has no source for.
pub fn plan_from_table_on(
    table: &ChannelTable,
    inputs: &[String],
    outputs: &[String],
    families: &crate::timing::TimingFamilyMap,
) -> Result<NiConfig, NiError> {
    let cfg = plan_from_table(table, inputs, outputs)?;
    cfg.validate_timing(families)?;
    Ok(cfg)
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_channel_table::{Channel, ChannelRole, Interface, Units};

    fn ins(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Mod1/ai{i}")).collect()
    }
    fn outs(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Mod2/ao{i}")).collect()
    }

    fn row(index: u32, role: ChannelRole) -> Channel {
        let mut c = Channel::new(index);
        c.role = role;
        c
    }

    fn iepe_row(index: u32, sens: f64, range: f64) -> Channel {
        let mut c = row(index, ChannelRole::Control);
        c.interface = Interface::Iepe;
        c.iepe_bias_v = Some(2.5);
        c.units = Units::AccelG;
        c.sensitivity_mv_per_eu = sens;
        c.range_max = range;
        c
    }

    fn se_row(index: u32, sens: f64, range: f64) -> Channel {
        let mut c = row(index, ChannelRole::Monitor);
        c.interface = Interface::SingleEnded;
        c.units = Units::AccelG;
        c.sensitivity_mv_per_eu = sens;
        c.range_max = range;
        c
    }

    fn table(rows: Vec<Channel>) -> ChannelTable {
        ChannelTable {
            channels: rows,
            ..ChannelTable::default()
        }
    }

    // --- mapping ------------------------------------------------------------

    #[test]
    fn an_iepe_row_asks_for_ac_coupling_and_internal_excitation() {
        let t = table(vec![iepe_row(1, 100.0, 10.0)]);
        let cfg = plan_from_table(&t, &ins(4), &[]).unwrap();
        assert_eq!(cfg.ai_assignments.len(), 1);
        let a = &cfg.ai_assignments[0];
        assert_eq!(a.interface, AiInterface::Iepe);
        assert_eq!(a.physical, "Mod1/ai0");
        assert_eq!(a.table_index, Some(1));
        assert_eq!(a.coupling(), crate::ffi::val::COUPLING_AC);
        assert_eq!(a.excitation_source(), crate::ffi::val::INTERNAL);
        // Excitation is a CURRENT and comes from config policy, never from the
        // row's `iepe_bias_v` (which is 2.5 V in this fixture).
        assert_eq!(cfg.excitation_a, 0.002);
    }

    #[test]
    fn a_single_ended_row_asks_for_dc_coupling_and_no_excitation() {
        let t = table(vec![se_row(1, 100.0, 10.0)]);
        let cfg = plan_from_table(&t, &ins(4), &[]).unwrap();
        let a = &cfg.ai_assignments[0];
        assert_eq!(a.interface, AiInterface::Voltage);
        assert_eq!(a.coupling(), crate::ffi::val::COUPLING_DC);
        assert_eq!(a.excitation_source(), crate::ffi::val::NO_EXCITATION);
    }

    #[test]
    fn the_front_end_choice_does_not_change_the_scaling() {
        // The correction this replaced an earlier design with: `interface`
        // selects coupling and excitation ONLY. Two rows with the same
        // sensitivity must convert identically no matter which front end they
        // asked for - if these ever diverge, a second scaling owner has crept
        // back in and readings differ by a factor of the sensitivity.
        let iepe = plan_from_table(&table(vec![iepe_row(1, 50.0, 4.0)]), &ins(1), &[]).unwrap();
        let se = plan_from_table(&table(vec![se_row(1, 50.0, 4.0)]), &ins(1), &[]).unwrap();
        let (a, b) = (&iepe.ai_assignments[0], &se.ai_assignments[0]);
        assert_ne!(a.coupling(), b.coupling(), "the FRONT ENDS must differ");
        for volts in [-1.0, -0.25, 0.0, 0.123_456_789, 1.0] {
            assert_eq!(
                a.to_eu(volts),
                b.to_eu(volts),
                "the SCALING must not differ (volts {volts})"
            );
        }
        assert_eq!(a.range_volts(), b.range_volts());
    }

    #[test]
    fn host_side_eu_conversion_is_bit_identical_to_the_systems_one_owner() {
        // `eigen_dsp::eu::volts_to_eu` is the system's EU conversion (the live
        // scope's path). `AiAssign::to_eu` duplicates the formula rather than
        // depending across a layer boundary, so this test is what keeps the two
        // from drifting - including in the last ulp, which is why it is
        // assert_eq on f64 and not an epsilon comparison.
        for sens in [10.0, 10.25, 50.0, 100.0, 1000.0, 3.7] {
            let a = AiAssign {
                physical: "Mod1/ai0".into(),
                interface: AiInterface::Iepe,
                sensitivity_mv_per_eu: sens,
                max_eu: 10.0,
                table_index: None,
            };
            for volts in [
                -10.0,
                -1.0,
                -0.000_123,
                0.0,
                0.1,
                0.5,
                0.123_456_789_012_345,
                1.0,
                9.999,
            ] {
                assert_eq!(
                    a.to_eu(volts),
                    eigen_dsp::eu::volts_to_eu(volts, sens),
                    "sens {sens} mV/EU, {volts} V"
                );
            }
        }
    }

    #[test]
    fn scaling_math_round_trips_through_engineering_units() {
        // 0.5 V measured on a 100 mV/g channel is 5 g.
        let a = AiAssign {
            physical: "Mod1/ai0".into(),
            interface: AiInterface::Voltage,
            sensitivity_mv_per_eu: 100.0,
            max_eu: 10.0,
            table_index: None,
        };
        assert!((a.to_eu(0.5) - 5.0).abs() < 1e-12);
        // A reading at the channel's electrical limit is exactly full scale.
        assert!((a.to_eu(a.range_volts()) - a.max_eu).abs() < 1e-12);
        // +/-10 g at 100 mV/g is a +/-1.0 V channel.
        assert!((a.range_volts() - 1.0).abs() < 1e-12, "{}", a.range_volts());

        // A 1000 mV/g channel is 10x less sensitive per g: the same 0.5 V is 0.5 g.
        let b = AiAssign {
            sensitivity_mv_per_eu: 1000.0,
            ..a.clone()
        };
        assert!((b.to_eu(0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn all_three_input_roles_map_to_inputs_in_table_order() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
            row(3, ChannelRole::Reference),
        ]);
        let cfg = plan_from_table(&t, &ins(3), &[]).unwrap();
        let phys: Vec<&str> = cfg
            .ai_assignments
            .iter()
            .map(|a| a.physical.as_str())
            .collect();
        assert_eq!(phys, vec!["Mod1/ai0", "Mod1/ai1", "Mod1/ai2"]);
        let idx: Vec<Option<u32>> = cfg.ai_assignments.iter().map(|a| a.table_index).collect();
        assert_eq!(idx, vec![Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn drive_rows_map_to_outputs_and_never_consume_an_input_channel() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
            row(3, ChannelRole::Monitor),
            row(4, ChannelRole::Drive),
        ]);
        let cfg = plan_from_table(&t, &ins(2), &outs(2)).unwrap();
        assert_eq!(cfg.n_input_channels(), 2);
        assert_eq!(cfg.n_output_channels(), 2);
        assert_eq!(cfg.ai_assignments[1].physical, "Mod1/ai1");
        assert_eq!(cfg.ao_assignments[0].physical, "Mod2/ao0");
        assert_eq!(cfg.ao_assignments[1].physical, "Mod2/ao1");
        assert_eq!(cfg.ao_assignments[1].table_index, Some(4));
    }

    #[test]
    fn disabled_rows_are_skipped_and_shift_nothing() {
        let mut rows = vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
            row(3, ChannelRole::Monitor),
            row(4, ChannelRole::Drive),
            row(5, ChannelRole::Drive),
        ];
        rows[1].enabled = false; // a disabled input
        rows[3].enabled = false; // a disabled drive
        let cfg = plan_from_table(&table(rows), &ins(2), &outs(1)).unwrap();
        // Rows 1 and 3 are the surviving inputs; they take ai0 and ai1.
        assert_eq!(cfg.n_input_channels(), 2);
        assert_eq!(
            cfg.ai_assignments
                .iter()
                .map(|a| a.table_index)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(3)]
        );
        // Row 5 is the only surviving drive; it takes ao0, NOT ao1.
        assert_eq!(cfg.n_output_channels(), 1);
        assert_eq!(cfg.ao_assignments[0].table_index, Some(5));
        assert_eq!(cfg.ao_assignments[0].physical, "Mod2/ao0");
    }

    #[test]
    fn fewer_rows_than_channels_leaves_the_surplus_unused() {
        let t = table(vec![row(1, ChannelRole::Control)]);
        let cfg = plan_from_table(&t, &ins(4), &outs(2)).unwrap();
        assert_eq!(cfg.n_input_channels(), 1);
        assert_eq!(cfg.n_output_channels(), 0);
    }

    // --- errors -------------------------------------------------------------

    #[test]
    fn more_input_rows_than_channels_is_an_error_not_a_truncation() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
            row(3, ChannelRole::Monitor),
        ]);
        let e =
            plan_from_table(&t, &ins(2), &[]).expect_err("3 input rows into 2 channels must fail");
        let s = e.to_string();
        assert!(s.contains("row 3"), "{s}");
        assert!(s.contains("3rd"), "{s}");
        assert!(s.contains("only 2 input channel"), "{s}");
    }

    #[test]
    fn more_drive_rows_than_output_channels_is_an_error() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
            row(3, ChannelRole::Drive),
        ]);
        let e = plan_from_table(&t, &ins(1), &outs(1))
            .expect_err("2 drive rows into 1 channel must fail");
        let s = e.to_string();
        assert!(s.contains("row 3"), "{s}");
        assert!(s.contains("only 1 output channel"), "{s}");
    }

    #[test]
    fn a_table_with_no_enabled_input_rows_is_an_error() {
        let t = table(vec![row(1, ChannelRole::Drive)]);
        let e = plan_from_table(&t, &ins(2), &outs(1))
            .expect_err("a drive-only table cannot make an AI task");
        assert!(e.to_string().contains("no input channels"), "{e}");
    }

    #[test]
    fn zero_sensitivity_is_an_error_because_the_conversion_divides_by_it() {
        let t = table(vec![se_row(1, 0.0, 10.0)]);
        let e = plan_from_table(&t, &ins(1), &[]).expect_err("zero sensitivity");
        assert!(e.to_string().contains("divides by it"), "{e}");
    }

    #[test]
    fn non_positive_range_is_an_error() {
        let t = table(vec![se_row(1, 100.0, 0.0)]);
        let e = plan_from_table(&t, &ins(1), &[]).expect_err("zero range");
        assert!(e.to_string().contains("non-positive"), "{e}");
    }

    #[test]
    fn a_duplicated_physical_channel_is_an_error() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
        ]);
        let dup = vec!["Mod1/ai0".to_string(), "Mod1/ai0".to_string()];
        let e = plan_from_table(&t, &dup, &[]).expect_err("aliased channel");
        assert!(e.to_string().contains("two input rows"), "{e}");

        // ...and the same on the output side.
        let t2 = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
            row(3, ChannelRole::Drive),
        ]);
        let dup_out = vec!["Mod2/ao0".to_string(), "Mod2/ao0".to_string()];
        let e2 = plan_from_table(&t2, &ins(1), &dup_out).expect_err("aliased output");
        assert!(e2.to_string().contains("two output rows"), "{e2}");
    }

    #[test]
    fn an_iepe_row_in_any_unit_is_accepted_and_scaled_by_its_own_sensitivity() {
        // An IEPE FORCE sensor: same front end, completely different unit. The
        // earlier accel-channel design would have baked accelerometer semantics
        // into this row; the voltage-channel design cannot, because the unit
        // never reaches the driver at all.
        let mut c = iepe_row(1, 50.0, 200.0);
        c.units = Units::ForceLbf;
        let cfg = plan_from_table(&table(vec![c]), &ins(1), &[])
            .expect("an IEPE force sensor is a perfectly ordinary row");
        let a = &cfg.ai_assignments[0];
        assert_eq!(a.interface, AiInterface::Iepe);
        assert_eq!(a.coupling(), crate::ffi::val::COUPLING_AC);
        // 50 mV/lbf: 0.5 V is 10 lbf. Same arithmetic as any other row.
        assert!((a.to_eu(0.5) - 10.0).abs() < 1e-12);
        // ...and a velocity sensor is equally unremarkable.
        let mut v = iepe_row(1, 500.0, 20.0);
        v.units = Units::VelocityIps;
        assert!(plan_from_table(&table(vec![v]), &ins(1), &[]).is_ok());
    }

    #[test]
    fn excitation_must_be_a_current_not_a_bias_voltage() {
        let ai = vec![AiAssign {
            physical: "Mod1/ai0".into(),
            interface: AiInterface::Iepe,
            sensitivity_mv_per_eu: 100.0,
            max_eu: 10.0,
            table_index: None,
        }];
        let mut cfg = NiConfig::new(ai.clone(), vec![]);
        assert!(cfg.validate().is_ok());

        // 2.5 would be a bias VOLTAGE pasted into a field that means amps.
        cfg.excitation_a = 2.5;
        let e = cfg
            .validate()
            .expect_err("2.5 A is not an excitation current");
        assert!(e.to_string().contains("bias VOLTAGE"), "{e}");

        // Zero excitation with IEPE rows leaves a dead sensor reading quiet.
        cfg.excitation_a = 0.0;
        let e = cfg.validate().expect_err("unexcited IEPE must be refused");
        assert!(e.to_string().contains("dead channel"), "{e}");

        // ...but zero excitation is fine when nothing asks for it.
        let mut se = NiConfig::new(ai, vec![]);
        se.ai_assignments[0].interface = AiInterface::Voltage;
        se.excitation_a = 0.0;
        assert!(se.validate().is_ok());
    }

    /// The rate/block VALIDITY checks moved with the fields: they are now the
    /// backend's business (`NiBackend::check_acq`), because rate and block size
    /// live on `eigen_hal::AcquisitionConfig`, not on `NiConfig`. What this
    /// test pins is that the plan builder no longer has an opinion about them
    /// at all - a plan is valid or not independently of how fast it runs.
    #[test]
    fn the_task_plan_is_valid_independently_of_any_rate() {
        let t = table(vec![row(1, ChannelRole::Control)]);
        assert!(plan_from_table(&t, &ins(1), &[]).is_ok());
    }

    #[test]
    fn an_empty_physical_channel_name_is_an_error() {
        let t = table(vec![row(1, ChannelRole::Control)]);
        let e = plan_from_table(&t, &[" ".to_string()], &[]).expect_err("empty name");
        assert!(e.to_string().contains("empty channel"), "{e}");
    }

    // --- derived sizes + device extraction -----------------------------------

    #[test]
    fn buffer_and_lookahead_sizes_derive_from_the_block_policy() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
        ]);
        let cfg = plan_from_table(&t, &ins(1), &outs(1)).unwrap();
        assert_eq!(cfg.ai_buffer_blocks, DEFAULT_AI_BUFFER_BLOCKS);
        assert_eq!(cfg.lookahead_blocks, DEFAULT_LOOKAHEAD_BLOCKS);
        // The frame-denominated sizes take the BLOCK as a parameter now.
        // (8 blocks since the drain-pacing fix, 2026-07-26 - see
        // DEFAULT_AI_BUFFER_BLOCKS.)
        assert_eq!(cfg.ai_buffer_frames(2048), 2048 * 8);
        assert_eq!(cfg.lookahead_frames(2048), 2048 * 2);
        // ...and they track it: half the block, half the buffer.
        assert_eq!(cfg.ai_buffer_frames(1024), 1024 * 8);
    }

    #[test]
    fn the_ai_ring_is_floored_in_seconds_of_real_time() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
        ]);
        let cfg = plan_from_table(&t, &ins(1), &outs(1)).unwrap();
        // The incident case: 1024-frame blocks at 51.2 kHz. Blocks alone give
        // 8192 frames = 160 ms; startup latency overflowed that (-200279,
        // 2026-07-30). The seconds floor must win: 2 s = 102 400 frames.
        assert_eq!(cfg.ai_ring_frames(1024, 51_200.0), 102_400);
        // A big block already clears the floor: 32768 x 8 = 262144 frames = 5.1 s
        // at 51.2 kHz - the block ring wins, the floor does not shrink it.
        assert_eq!(cfg.ai_ring_frames(32_768, 51_200.0), 32_768 * 8);
        // No rate (unopened/degenerate): fall back to the block ring alone.
        assert_eq!(cfg.ai_ring_frames(1024, 0.0), 1024 * 8);
        // The AO buffer holds the lookahead plus a block of slack.
        assert_eq!(cfg.ao_buffer_frames(2048), 2048 * 3);
        assert!(cfg.ao_buffer_frames(2048) > cfg.lookahead_frames(2048));
        // Plan requirement: the AI buffer is at least 4x the block.
        assert!(cfg.ai_buffer_frames(2048) >= 4 * 2048);
    }

    #[test]
    fn devices_are_extracted_from_the_physical_names_in_first_seen_order() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
            row(3, ChannelRole::Drive),
        ]);
        let inputs = vec!["ModA/ai0".to_string(), "ModB/ai0".to_string()];
        let outputs = vec!["ModC/ao0".to_string()];
        let cfg = plan_from_table(&t, &inputs, &outputs).unwrap();
        assert_eq!(cfg.input_devices(), vec!["ModA", "ModB"]);
        assert_eq!(cfg.output_devices(), vec!["ModC"]);
        assert_eq!(cfg.devices(), vec!["ModA", "ModB", "ModC"]);
    }

    #[test]
    fn a_zero_lookahead_is_only_an_error_when_there_is_drive_to_look_ahead_of() {
        let ai = vec![AiAssign {
            physical: "Mod1/ai0".into(),
            interface: AiInterface::Iepe,
            sensitivity_mv_per_eu: 100.0,
            max_eu: 10.0,
            table_index: None,
        }];
        let mut capture_only = NiConfig::new(ai.clone(), vec![]);
        capture_only.lookahead_blocks = 0;
        assert!(
            capture_only.validate().is_ok(),
            "an input-only task needs no lookahead"
        );

        let mut with_drive = NiConfig::new(
            ai,
            vec![AoAssign {
                physical: "Mod2/ao0".into(),
                table_index: None,
            }],
        );
        with_drive.lookahead_blocks = 0;
        assert!(with_drive.validate().is_err(), "drive needs a lookahead");
    }

    // --- timing families (milestone finding 1) --------------------------------

    fn fam_dev(
        id: &str,
        parent: Option<&str>,
        ai: usize,
        ao: usize,
    ) -> eigen_hal::discovery::DiscoveredDevice {
        use eigen_hal::discovery::{DiscoveredChannel, DiscoveredDevice};
        DiscoveredDevice {
            id: id.into(),
            product: String::new(),
            serial: String::new(),
            simulated: true,
            parent: parent.map(str::to_string),
            slot: parent.map(|_| 1),
            inputs: (0..ai)
                .map(|i| DiscoveredChannel {
                    id: format!("{id}/ai{i}"),
                    label: format!("ai{i}"),
                })
                .collect(),
            outputs: (0..ao)
                .map(|i| DiscoveredChannel {
                    id: format!("{id}/ao{i}"),
                    label: format!("ao{i}"),
                })
                .collect(),
        }
    }

    /// The milestone scan: a cDAQ chassis with an AI and an AO module, plus a
    /// standalone PCIe card that carries both.
    fn mixed_map() -> crate::timing::TimingFamilyMap {
        crate::timing::TimingFamilyMap::from_devices(&[
            fam_dev("cDAQ1", None, 0, 0),
            fam_dev("Mod1", Some("cDAQ1"), 4, 0),
            fam_dev("Mod2", Some("cDAQ1"), 0, 2),
            fam_dev("PCIe1", None, 8, 2),
        ])
    }

    /// The -201425 case, promoted to a typed CONFIG error at plan time.
    ///
    /// RED-MUTATION: drop the `cfg.validate_timing(families)?` line from
    /// `plan_from_table_on` and this test reds - the cross-family plan builds
    /// clean and the driver refusal moves back to arm.
    #[test]
    fn a_plan_spanning_two_timing_families_is_refused_at_plan_time() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
        ]);
        let mixed = vec!["Mod1/ai0".to_string(), "PCIe1/ai0".to_string()];
        let map = mixed_map();

        // The 3-arg planner has no scan and therefore no opinion: it builds.
        assert!(
            plan_from_table(&t, &mixed, &[]).is_ok(),
            "no scan knowledge means no refusal - that is deliberate"
        );

        // The scan-aware planner refuses, naming both families.
        let e = plan_from_table_on(&t, &mixed, &[], &map)
            .expect_err("cDAQ + PCIe inputs cannot share a timing method");
        let m = e.to_string();
        assert!(m.contains("cDAQ1"), "{m}");
        assert!(m.contains("PCIe1"), "{m}");
        assert!(m.contains("-201425"), "{m}");
        assert!(matches!(e, NiError::Config(_)), "{e:?}");
    }

    #[test]
    fn a_single_family_plan_still_builds_under_the_scan_aware_planner() {
        // Anti-vacuity: the dev rig's own configuration (AI + AO in one chassis)
        // must pass, or the refusal is refusing the working case.
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Monitor),
            row(3, ChannelRole::Drive),
        ]);
        let cfg = plan_from_table_on(
            &t,
            &["Mod1/ai0".into(), "Mod1/ai1".into()],
            &["Mod2/ao0".into()],
            &mixed_map(),
        )
        .expect("one chassis, AI and AO, is exactly the proven configuration");
        assert_eq!(cfg.n_input_channels(), 2);
        assert_eq!(cfg.n_output_channels(), 1);
    }

    #[test]
    fn drive_on_another_family_than_the_measurement_is_refused() {
        let t = table(vec![
            row(1, ChannelRole::Control),
            row(2, ChannelRole::Drive),
        ]);
        let e = plan_from_table_on(
            &t,
            &["Mod1/ai0".into()],
            &["PCIe1/ao0".into()],
            &mixed_map(),
        )
        .expect_err("the AO clock terminal does not cross families");
        assert!(e.to_string().contains("ai/SampleClock"), "{e}");
    }

    #[test]
    fn ordinals_read_like_english() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(13), "13th");
        assert_eq!(ordinal(21), "21st");
        assert_eq!(ordinal(101), "101st");
        assert_eq!(ordinal(111), "111th");
    }
}
