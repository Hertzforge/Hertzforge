//! Timing families: which devices DAQmx will put on ONE shared timing method.
//!
//! ## The defect this module exists for
//!
//! The first live GUI session (2026-07-25) bulk-bound channel rows across the
//! cDAQ chassis AND a PCIe-6363 with "Assign in Scan Order", and the driver
//! refused the resulting AI task with **DAQmx -201425** ("Multidevice task
//! ... cannot be synchronized"). The refusal was honest and the ARM REFUSED panel
//! rendered it verbatim - but it arrived at ARM, three screens and one operator
//! decision after the mistake was made. The milestone finding's work item is that
//! the partition happens at BIND time, and that a cross-family input set is a
//! typed CONFIG error before the driver is ever asked.
//!
//! ## What a "timing family" is, and what it is NOT
//!
//! A family is the set of devices that can share one timing method:
//!
//! - every module in ONE cDAQ chassis is one family (the chassis owns the
//!   backplane clock and exposes `/<chassis>/ai/SampleClock` as a routable
//!   terminal);
//! - every standalone device (a PCIe/PXI card, anything the scan reports with no
//!   parent) is its OWN family.
//!
//! The grouping is read from the SCAN - `DiscoveredDevice::parent`, which
//! `enumerate` fills from DAQmx's own `CompactDAQChassisDevName` - and never
//! from a device name. `cDAQ1Mod1` looking like it belongs to `cDAQ1` is a naming
//! convention, not a fact, and the rule "when a doc and a byte disagree, the
//! byte wins" applies to inferred structure exactly as it applies to inferred
//! capability. A device the map has never seen has an **Unknown** family, and an
//! Unknown family never triggers a refusal: refusing on ignorance would turn a
//! missing scan into a phantom hardware fault.
//!
//! ## What this module deliberately does not do
//!
//! It does not offer an escape hatch. A genuinely-synchronizable multi-chassis
//! set exists in the world (shared timebase + start trigger over an external
//! wire, a PXI backplane), and a future operator with that rig will need a way to
//! say so. That is OUT OF SCOPE here on purpose: the mechanism would have to be
//! an EVIDENCE claim (which physical wire, proven by which capture), and this
//! project's standing rule is that a capability is declared after the bench
//! proves it, never before. It is tracked as a follow-up.

use crate::error::NiError;
use hertzforge_hal::discovery::DiscoveredDevice;
use std::collections::BTreeMap;
use std::fmt;

/// Whether a family is a chassis full of modules or one standalone device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyKind {
    /// A cDAQ/PXI chassis: every module in it shares the backplane clock.
    Chassis,
    /// A device the scan reports with no parent - its own timing island.
    Standalone,
}

/// One timing family: an id, what kind of thing it is, and its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingFamily {
    /// The chassis device name, or the standalone device's own name.
    pub id: String,
    pub kind: FamilyKind,
    /// Device ids in this family, in the driver's own scan order. A chassis
    /// device itself is a member of its own family (it carries no channels, but
    /// naming it keeps the family's membership honest).
    pub members: Vec<String>,
}

impl TimingFamily {
    /// How many devices in this family expose at least one channel. The tie-break
    /// for "largest family first" is about BINDABLE width, not row count: a
    /// chassis row itself binds nothing.
    pub fn bindable_len(&self) -> usize {
        self.members.len()
    }
}

impl fmt::Display for TimingFamily {
    /// Reads inside a sentence: "the cDAQ chassis `cDAQ9178Sim`".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            FamilyKind::Chassis => write!(f, "the chassis `{}`", self.id),
            FamilyKind::Standalone => write!(f, "the standalone device `{}`", self.id),
        }
    }
}

/// Device id -> timing family, built from one scan.
///
/// Construct it from the SAME device list the station was built from. An empty
/// map (no scan) answers `None` for everything and refuses nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimingFamilyMap {
    /// Every family, in first-seen scan order.
    families: Vec<TimingFamily>,
    /// device id -> index into `families`.
    by_device: BTreeMap<String, usize>,
}

impl TimingFamilyMap {
    /// Group a scan's devices into timing families.
    ///
    /// A module whose named parent is NOT in the scan becomes its own family
    /// rather than being folded into a phantom chassis - the same rule
    /// `hardware/guard.ts::deviceTree` uses for the UI tree, and for the same
    /// reason: a device that exists is a fact, and inventing its parent is not.
    pub fn from_devices(devices: &[DiscoveredDevice]) -> Self {
        let present: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        let mut families: Vec<TimingFamily> = Vec::new();
        let mut by_device: BTreeMap<String, usize> = BTreeMap::new();

        for d in devices {
            let (fid, kind) = match d.parent.as_deref() {
                Some(p) if present.contains(&p) => (p.to_string(), FamilyKind::Chassis),
                // No parent, or a parent the scan does not contain: an island.
                _ => (d.id.clone(), FamilyKind::Standalone),
            };
            let idx = match families.iter().position(|f| f.id == fid) {
                Some(i) => {
                    // A chassis row seen AFTER one of its modules upgrades the
                    // family's kind: the family is a chassis the moment anything
                    // claims it as a parent, whatever order the scan listed them.
                    if kind == FamilyKind::Chassis {
                        families[i].kind = FamilyKind::Chassis;
                    }
                    i
                }
                None => {
                    families.push(TimingFamily {
                        id: fid,
                        kind,
                        members: Vec::new(),
                    });
                    families.len() - 1
                }
            };
            families[idx].members.push(d.id.clone());
            by_device.insert(d.id.clone(), idx);
        }
        TimingFamilyMap {
            families,
            by_device,
        }
    }

    /// True when the map carries no scan knowledge at all.
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// Every family, in the driver's own scan order.
    pub fn families(&self) -> &[TimingFamily] {
        &self.families
    }

    /// The family a DEVICE belongs to, or `None` when the scan never saw it.
    pub fn family_of_device(&self, device: &str) -> Option<&TimingFamily> {
        self.by_device.get(device).map(|i| &self.families[*i])
    }

    /// The family a PHYSICAL CHANNEL (`Mod1/ai0`) belongs to.
    pub fn family_of_physical(&self, physical: &str) -> Option<&TimingFamily> {
        self.family_of_device(crate::ffi::device_of(physical))
    }

    /// The family id of a physical channel, or the device name when the scan does
    /// not know it. Used only for grouping/reporting, never for a refusal.
    pub fn family_key(&self, physical: &str) -> String {
        match self.family_of_physical(physical) {
            Some(f) => f.id.clone(),
            None => crate::ffi::device_of(physical).to_string(),
        }
    }

    /// Split physical channels into per-family buckets, families in scan order,
    /// channels in the order given. Channels the scan does not know keep their own
    /// device-named bucket rather than being dropped.
    pub fn partition<'a>(&self, physical: &'a [String]) -> Vec<(String, Vec<&'a str>)> {
        let mut out: Vec<(String, Vec<&'a str>)> = Vec::new();
        for p in physical {
            let key = self.family_key(p);
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(p.as_str()),
                None => out.push((key, vec![p.as_str()])),
            }
        }
        out
    }

    /// Refuse a channel set that spans more than one KNOWN timing family.
    ///
    /// `what` names the task ("input" / "output") so the message says which half
    /// of the plan is wrong. The message names BOTH families and one offending
    /// channel from each, and cites the DAQmx status the driver would have
    /// returned - so an operator who has seen -201425 once recognizes it here,
    /// where it is still fixable, instead of at arm.
    pub fn refuse_mixed(&self, what: &str, physical: &[String]) -> Result<(), NiError> {
        let mut first: Option<(&TimingFamily, &str)> = None;
        for p in physical {
            let Some(fam) = self.family_of_physical(p) else {
                // Unknown family: no knowledge, no refusal. See the module note.
                continue;
            };
            match first {
                None => first = Some((fam, p.as_str())),
                Some((seen, seen_chan)) if seen.id == fam.id => {
                    let _ = seen_chan;
                }
                Some((seen, seen_chan)) => {
                    return Err(NiError::config(format!(
                        "the {what} channels span two timing families: {seen} (e.g. `{seen_chan}`) \
                         and {fam} (e.g. `{p}`). One DAQmx task cannot share a timing method \
                         across them - the driver refuses the multidevice task with -201425 - so \
                         this set is refused HERE, before the driver is asked. Bind every {what} \
                         row inside one family."
                    )));
                }
            }
        }
        Ok(())
    }

    /// Refuse an input set whose family differs from the output set's.
    ///
    /// The AO task takes its sample clock from `/<chassis>/ai/SampleClock` (see
    /// `backend::common_chassis`), which is a terminal ON one family's backplane.
    /// Across families there is no such terminal, so the AO task falls back to its
    /// own clock and the drive stops being sample-aligned with the measurement -
    /// a closed loop running on two free-running clocks. Refused rather than
    /// silently downgraded, because the downgrade is invisible in every plot.
    pub fn refuse_split_io(&self, inputs: &[String], outputs: &[String]) -> Result<(), NiError> {
        let (Some(i), Some(o)) = (
            inputs.iter().find_map(|p| self.family_of_physical(p)),
            outputs.iter().find_map(|p| self.family_of_physical(p)),
        ) else {
            return Ok(());
        };
        if i.id == o.id {
            return Ok(());
        }
        Err(NiError::config(format!(
            "the input channels are on {i} and the drive channels are on {o}. The AO task takes \
             its sample clock from the AI task's `ai/SampleClock` terminal, which exists only \
             within one family; across families the drive would run on its own free-running \
             clock while every plot still looked alive. Bind the drive rows inside {i}."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_hal::discovery::{DiscoveredChannel, DiscoveredDevice};

    fn dev(id: &str, parent: Option<&str>, ai: usize, ao: usize) -> DiscoveredDevice {
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

    /// The exact shape of the box the milestone session ran against: one cDAQ
    /// chassis with three modules, plus a standalone PCIe card.
    fn milestone_scan() -> Vec<DiscoveredDevice> {
        vec![
            dev("cDAQ9178Sim", None, 0, 0),
            dev("Mod1", Some("cDAQ9178Sim"), 4, 0),
            dev("Mod2", Some("cDAQ9178Sim"), 0, 4),
            dev("Mod3", Some("cDAQ9178Sim"), 4, 0),
            dev("PCIe6363Sim", None, 16, 2),
        ]
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn one_chassis_is_one_family_and_a_standalone_card_is_its_own() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        assert_eq!(m.families().len(), 2, "{:?}", m.families());

        let chassis = m.family_of_device("Mod1").expect("Mod1 is in the scan");
        assert_eq!(chassis.id, "cDAQ9178Sim");
        assert_eq!(chassis.kind, FamilyKind::Chassis);
        // The chassis row is a member of its own family, as are all three modules.
        assert_eq!(
            chassis.members,
            vec!["cDAQ9178Sim", "Mod1", "Mod2", "Mod3"],
            "membership follows the scan, in scan order"
        );
        // Every module lands in the SAME family - that is the whole claim.
        assert_eq!(m.family_of_device("Mod2").unwrap().id, "cDAQ9178Sim");
        assert_eq!(m.family_of_device("Mod3").unwrap().id, "cDAQ9178Sim");
        assert_eq!(m.family_of_device("cDAQ9178Sim").unwrap().id, "cDAQ9178Sim");

        let pcie = m.family_of_device("PCIe6363Sim").unwrap();
        assert_eq!(pcie.id, "PCIe6363Sim");
        assert_eq!(pcie.kind, FamilyKind::Standalone);
        assert_eq!(pcie.members, vec!["PCIe6363Sim"]);
    }

    #[test]
    fn a_physical_channel_resolves_through_its_device() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        assert_eq!(m.family_of_physical("Mod1/ai0").unwrap().id, "cDAQ9178Sim");
        assert_eq!(
            m.family_of_physical("PCIe6363Sim/ai7").unwrap().id,
            "PCIe6363Sim"
        );
        // A device the scan never reported has NO family - not a guessed one.
        assert!(m.family_of_physical("GhostDev/ai0").is_none());
        // ...and `family_key` still gives it a distinct bucket for grouping.
        assert_eq!(m.family_key("GhostDev/ai0"), "GhostDev");
    }

    #[test]
    fn a_module_whose_chassis_is_missing_from_the_scan_is_its_own_family() {
        // DAQmx names a parent that is not in the device list (the chassis row was
        // filtered out upstream). Folding the module into a family whose device
        // nobody can see would invent structure; it becomes an island instead.
        let scan = vec![dev("OrphanMod", Some("GoneChassis"), 4, 0)];
        let m = TimingFamilyMap::from_devices(&scan);
        let f = m.family_of_device("OrphanMod").unwrap();
        assert_eq!(f.id, "OrphanMod");
        assert_eq!(f.kind, FamilyKind::Standalone);
    }

    #[test]
    fn a_chassis_listed_after_its_modules_still_makes_a_chassis_family() {
        // Scan order is the driver's, not ours: nothing guarantees the chassis row
        // arrives first. The family's KIND must not depend on that ordering.
        let scan = vec![
            dev("ModA", Some("LateChassis"), 4, 0),
            dev("LateChassis", None, 0, 0),
        ];
        let m = TimingFamilyMap::from_devices(&scan);
        let f = m.family_of_device("ModA").unwrap();
        assert_eq!(f.id, "LateChassis");
        assert_eq!(
            f.kind,
            FamilyKind::Chassis,
            "the family is a chassis the moment a module claims it as a parent"
        );
    }

    /// THE milestone case (finding 1). The set that produced DAQmx -201425 on the
    /// box is now a typed config error, with both families named, BEFORE any
    /// driver call.
    ///
    /// RED-MUTATION: make `refuse_mixed` return `Ok(())` unconditionally (or drop
    /// the `Some((seen, ..)) if seen.id != fam.id` arm) and this test reds - the
    /// cross-family set is accepted again and the -201425 goes back to being an
    /// arm-time surprise.
    #[test]
    fn a_cross_family_input_set_is_refused_and_the_message_names_both_families() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        let mixed = s(&["Mod1/ai0", "Mod1/ai1", "PCIe6363Sim/ai0"]);
        let e = m
            .refuse_mixed("input", &mixed)
            .expect_err("a cDAQ + PCIe input set cannot share a timing method");
        let msg = e.to_string();
        assert!(msg.contains("cDAQ9178Sim"), "names the chassis: {msg}");
        assert!(msg.contains("PCIe6363Sim"), "names the card: {msg}");
        assert!(msg.contains("Mod1/ai0"), "names a channel from each: {msg}");
        assert!(msg.contains("PCIe6363Sim/ai0"), "{msg}");
        assert!(msg.contains("-201425"), "cites the driver status: {msg}");
        assert!(msg.contains("input"), "says WHICH task: {msg}");
        // And it is a CONFIG error - the driver was never asked.
        assert!(matches!(e, NiError::Config(_)), "{e:?}");
    }

    #[test]
    fn a_single_family_set_is_accepted_however_many_modules_it_spans() {
        // The anti-vacuity half: the refusal must not fire on the set the dev rig
        // actually uses (three modules, one chassis), or it would refuse the
        // working configuration.
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        assert!(m
            .refuse_mixed(
                "input",
                &s(&["Mod1/ai0", "Mod1/ai3", "Mod3/ai0", "Mod3/ai2"])
            )
            .is_ok());
        assert!(m
            .refuse_mixed("output", &s(&["Mod2/ao0", "Mod2/ao1"]))
            .is_ok());
        // ...and an all-PCIe set is equally fine: one family is one family.
        assert!(m
            .refuse_mixed("input", &s(&["PCIe6363Sim/ai0", "PCIe6363Sim/ai9"]))
            .is_ok());
    }

    #[test]
    fn an_empty_map_refuses_nothing_because_it_knows_nothing() {
        // No scan = no knowledge. Refusing here would turn a missing scan into a
        // phantom hardware fault, which is the failure mode this project keeps
        // paying for in the other direction.
        let m = TimingFamilyMap::default();
        assert!(m.is_empty());
        assert!(m
            .refuse_mixed("input", &s(&["Mod1/ai0", "PCIe6363Sim/ai0"]))
            .is_ok());
        assert!(m.family_of_physical("Mod1/ai0").is_none());
    }

    #[test]
    fn unknown_devices_are_skipped_but_known_ones_still_refuse_around_them() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        // A ghost between two KNOWN cross-family channels must not mask them.
        let e = m.refuse_mixed(
            "input",
            &s(&["Mod1/ai0", "GhostDev/ai0", "PCIe6363Sim/ai0"]),
        );
        assert!(e.is_err(), "the ghost must not swallow the real conflict");
        // Two ghosts alone: nothing known, nothing refused.
        assert!(m
            .refuse_mixed("input", &s(&["GhostA/ai0", "GhostB/ai0"]))
            .is_ok());
    }

    /// RED-MUTATION: return `Ok(())` from `refuse_split_io` and this reds - a
    /// drive channel on another family is accepted and the AO silently falls back
    /// to its own free-running clock.
    #[test]
    fn drive_on_a_different_family_than_the_inputs_is_refused() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        let e = m
            .refuse_split_io(&s(&["Mod1/ai0"]), &s(&["PCIe6363Sim/ao0"]))
            .expect_err("AI on the chassis and AO on the card share no clock terminal");
        let msg = e.to_string();
        assert!(msg.contains("cDAQ9178Sim"), "{msg}");
        assert!(msg.contains("PCIe6363Sim"), "{msg}");
        assert!(msg.contains("ai/SampleClock"), "names the terminal: {msg}");

        // Same family both directions: fine.
        assert!(m
            .refuse_split_io(&s(&["Mod1/ai0"]), &s(&["Mod2/ao0"]))
            .is_ok());
        // Capture-only: no outputs, nothing to split.
        assert!(m.refuse_split_io(&s(&["Mod1/ai0"]), &[]).is_ok());
    }

    #[test]
    fn partition_buckets_channels_by_family_in_scan_order() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        let chans = s(&["Mod1/ai0", "PCIe6363Sim/ai0", "Mod3/ai0", "PCIe6363Sim/ai1"]);
        let parts = m.partition(&chans);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "cDAQ9178Sim");
        assert_eq!(parts[0].1, vec!["Mod1/ai0", "Mod3/ai0"]);
        assert_eq!(parts[1].0, "PCIe6363Sim");
        assert_eq!(parts[1].1, vec!["PCIe6363Sim/ai0", "PCIe6363Sim/ai1"]);
    }

    #[test]
    fn families_read_like_english_in_a_sentence() {
        let m = TimingFamilyMap::from_devices(&milestone_scan());
        assert_eq!(
            m.family_of_device("Mod1").unwrap().to_string(),
            "the chassis `cDAQ9178Sim`"
        );
        assert_eq!(
            m.family_of_device("PCIe6363Sim").unwrap().to_string(),
            "the standalone device `PCIe6363Sim`"
        );
    }
}
