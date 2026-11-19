//! Device enumeration: DAQmx system/device attributes -> `hertzforge_hal::discovery`.
//!
//! Agnosticism is the whole point. Whatever is attached is what the list shows;
//! nothing here knows the dev rig's model numbers, and the only device names in
//! this file are in tests.
//!
//! ## Degrade vs. fail
//!
//! Attributes fall into three classes, and the difference is deliberate:
//!
//! | Attribute | Missing/error | Why |
//! |---|---|---|
//! | system device list | **hard error** | there is nothing to enumerate |
//! | `simulated` | **hard error** | see below |
//! | product, serial | degrade to `""` | cosmetic identity |
//! | chassis, slot | degrade to `None` | a non-cDAQ card legitimately has neither |
//! | AI/AO channel lists | degrade to empty | a chassis has no channels of its own |
//!
//! `simulated` is a hard error because [`hertzforge_hal::DiscoveredDevice::simulated`]
//! is a `bool` with no "unknown" state, and BOTH lies are worse than a failed
//! scan: reporting `false` for a simulated device hides the SIMULATED badge and
//! recreates the exact "looked wired-in while painting simulated data" failure
//! this project has already paid for; reporting `true` for real hardware would
//! label a live shaker rig as fake. A scan that says "I could not read the
//! simulated flag for cDAQ1Mod1" is honest; a device row with a guessed badge is
//! not.

use crate::error::NiError;
use crate::ffi::{self, Driver};
use hertzforge_hal::discovery::{DiscoveredChannel, DiscoveredDevice};

/// The DAQmx system/device attribute reads enumeration needs.
///
/// A trait, not a set of free functions, so the parsing and degradation logic
/// above is provable on a host with no driver: tests substitute a canned
/// implementation and drive every branch, including the error branches.
pub trait SysQuery {
    fn sys_dev_names(&self) -> Result<String, NiError>;
    fn dev_product_type(&self, dev: &str) -> Result<String, NiError>;
    fn dev_serial_num(&self, dev: &str) -> Result<u32, NiError>;
    fn dev_is_simulated(&self, dev: &str) -> Result<bool, NiError>;
    fn dev_chassis_name(&self, dev: &str) -> Result<String, NiError>;
    fn dev_slot_num(&self, dev: &str) -> Result<u32, NiError>;
    fn dev_ai_physical_chans(&self, dev: &str) -> Result<String, NiError>;
    fn dev_ao_physical_chans(&self, dev: &str) -> Result<String, NiError>;
}

impl SysQuery for Driver {
    fn sys_dev_names(&self) -> Result<String, NiError> {
        Driver::sys_dev_names(self)
    }
    fn dev_product_type(&self, dev: &str) -> Result<String, NiError> {
        Driver::dev_product_type(self, dev)
    }
    fn dev_serial_num(&self, dev: &str) -> Result<u32, NiError> {
        Driver::dev_serial_num(self, dev)
    }
    fn dev_is_simulated(&self, dev: &str) -> Result<bool, NiError> {
        Driver::dev_is_simulated(self, dev)
    }
    fn dev_chassis_name(&self, dev: &str) -> Result<String, NiError> {
        Driver::dev_chassis_name(self, dev)
    }
    fn dev_slot_num(&self, dev: &str) -> Result<u32, NiError> {
        Driver::dev_slot_num(self, dev)
    }
    fn dev_ai_physical_chans(&self, dev: &str) -> Result<String, NiError> {
        Driver::dev_ai_physical_chans(self, dev)
    }
    fn dev_ao_physical_chans(&self, dev: &str) -> Result<String, NiError> {
        Driver::dev_ao_physical_chans(self, dev)
    }
}

/// Format a DAQmx `uInt32` serial the way NI MAX presents it: 8 upper-case hex
/// digits. Zero is rendered `00000000` rather than blanked - a driver-simulated
/// device really does report serial 0, and blanking it would turn a fact into an
/// absence.
pub fn format_serial(raw: u32) -> String {
    format!("{raw:08X}")
}

/// Scan the system and return every device the driver reports.
pub fn enumerate() -> Result<Vec<DiscoveredDevice>, NiError> {
    let driver = Driver::load()?;
    enumerate_with(&driver)
}

/// Scan using an explicit attribute source.
///
/// Devices come back in the driver's own order. The list is FLAT; the chassis
/// relationship is carried by `parent`/`slot`, which the UI turns into a tree.
pub fn enumerate_with<Q: SysQuery + ?Sized>(q: &Q) -> Result<Vec<DiscoveredDevice>, NiError> {
    let names = ffi::split_names(&q.sys_dev_names()?);
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // Cosmetic identity: degrade to empty rather than fail a whole scan.
        let product = q.dev_product_type(&name).unwrap_or_default();
        let serial = q
            .dev_serial_num(&name)
            .map(format_serial)
            .unwrap_or_default();

        // Safety-relevant: never guessed. See the module note.
        let simulated = q.dev_is_simulated(&name)?;

        // cDAQ-only attributes: a standalone card has neither, and asking for
        // them errors. Absence is a fact about the device, not a scan failure.
        let parent = q
            .dev_chassis_name(&name)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // A slot number without a chassis is meaningless, so it is reported only
        // alongside one.
        let slot = if parent.is_some() {
            q.dev_slot_num(&name).ok()
        } else {
            None
        };

        let inputs = channels_from_list(&q.dev_ai_physical_chans(&name).unwrap_or_default());
        let outputs = channels_from_list(&q.dev_ao_physical_chans(&name).unwrap_or_default());

        out.push(DiscoveredDevice {
            id: name,
            product,
            serial,
            simulated,
            parent,
            slot,
            inputs,
            outputs,
        });
    }
    Ok(out)
}

/// Turn a DAQmx comma-separated physical-channel list into discovery channels.
///
/// The physical name goes in `id` VERBATIM (it is this backend's opaque handle);
/// the trailing segment becomes the human `label` so no consumer ever has to
/// split the id.
fn channels_from_list(list: &str) -> Vec<DiscoveredChannel> {
    ffi::split_names(list)
        .into_iter()
        .map(|physical| {
            let label = ffi::channel_label(&physical).to_string();
            DiscoveredChannel {
                id: physical,
                label,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A canned attribute source. Every getter can be made to ERROR for a given
    /// device by simply omitting its entry, which is how the degradation paths
    /// get exercised. `dev_names: None` makes the system list itself fail.
    #[derive(Default)]
    struct Fake {
        dev_names: Option<String>,
        product: HashMap<&'static str, &'static str>,
        serial: HashMap<&'static str, u32>,
        simulated: HashMap<&'static str, bool>,
        chassis: HashMap<&'static str, &'static str>,
        slot: HashMap<&'static str, u32>,
        ai: HashMap<&'static str, &'static str>,
        ao: HashMap<&'static str, &'static str>,
    }

    fn absent(what: &str, dev: &str) -> NiError {
        NiError::Daqmx {
            code: -200220,
            text: format!("attribute {what} is not supported by device {dev}"),
            context: what.into(),
        }
    }

    impl SysQuery for Fake {
        fn sys_dev_names(&self) -> Result<String, NiError> {
            self.dev_names
                .clone()
                .ok_or_else(|| absent("SysDevNames", "system"))
        }
        fn dev_product_type(&self, dev: &str) -> Result<String, NiError> {
            self.product
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("ProductType", dev))
        }
        fn dev_serial_num(&self, dev: &str) -> Result<u32, NiError> {
            self.serial
                .get(dev)
                .copied()
                .ok_or_else(|| absent("SerialNum", dev))
        }
        fn dev_is_simulated(&self, dev: &str) -> Result<bool, NiError> {
            self.simulated
                .get(dev)
                .copied()
                .ok_or_else(|| absent("IsSimulated", dev))
        }
        fn dev_chassis_name(&self, dev: &str) -> Result<String, NiError> {
            self.chassis
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("CompactDAQChassisDevName", dev))
        }
        fn dev_slot_num(&self, dev: &str) -> Result<u32, NiError> {
            self.slot
                .get(dev)
                .copied()
                .ok_or_else(|| absent("CompactDAQSlotNum", dev))
        }
        fn dev_ai_physical_chans(&self, dev: &str) -> Result<String, NiError> {
            self.ai
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("AIPhysicalChans", dev))
        }
        fn dev_ao_physical_chans(&self, dev: &str) -> Result<String, NiError> {
            self.ao
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("AOPhysicalChans", dev))
        }
    }

    /// A chassis + two modules, the shape the simulated rig reports.
    fn cdaq_rig() -> Fake {
        Fake {
            dev_names: Some("ChassisA, ChassisAMod1, ChassisAMod2".into()),
            product: HashMap::from([
                ("ChassisA", "Chassis-Model"),
                ("ChassisAMod1", "AI-Module"),
                ("ChassisAMod2", "AO-Module"),
            ]),
            serial: HashMap::from([
                ("ChassisA", 0u32),
                ("ChassisAMod1", 0x1A2B_3C4Du32),
                ("ChassisAMod2", 0x0000_00FFu32),
            ]),
            simulated: HashMap::from([
                ("ChassisA", true),
                ("ChassisAMod1", true),
                ("ChassisAMod2", true),
            ]),
            chassis: HashMap::from([("ChassisAMod1", "ChassisA"), ("ChassisAMod2", "ChassisA")]),
            slot: HashMap::from([("ChassisAMod1", 1u32), ("ChassisAMod2", 2u32)]),
            ai: HashMap::from([
                ("ChassisA", ""),
                (
                    "ChassisAMod1",
                    "ChassisAMod1/ai0, ChassisAMod1/ai1, ChassisAMod1/ai2, ChassisAMod1/ai3",
                ),
                ("ChassisAMod2", ""),
            ]),
            ao: HashMap::from([
                ("ChassisA", ""),
                ("ChassisAMod1", ""),
                ("ChassisAMod2", "ChassisAMod2/ao0, ChassisAMod2/ao1"),
            ]),
        }
    }

    #[test]
    fn parses_a_chassis_tree_with_parents_slots_and_channels() {
        let devs = enumerate_with(&cdaq_rig()).unwrap();
        assert_eq!(devs.len(), 3);

        let chassis = &devs[0];
        assert_eq!(chassis.id, "ChassisA");
        assert_eq!(chassis.product, "Chassis-Model");
        assert_eq!(chassis.parent, None, "a chassis has no parent");
        assert_eq!(chassis.slot, None);
        assert_eq!(chassis.channel_count(), 0);

        let ai_mod = &devs[1];
        assert_eq!(ai_mod.parent.as_deref(), Some("ChassisA"));
        assert_eq!(ai_mod.slot, Some(1));
        assert_eq!(ai_mod.inputs.len(), 4);
        assert!(ai_mod.outputs.is_empty());
        assert_eq!(ai_mod.inputs[0].id, "ChassisAMod1/ai0");
        assert_eq!(ai_mod.inputs[0].label, "ai0");
        assert_eq!(ai_mod.inputs[3].label, "ai3");

        let ao_mod = &devs[2];
        assert_eq!(ao_mod.slot, Some(2));
        assert!(ao_mod.inputs.is_empty());
        assert_eq!(ao_mod.outputs.len(), 2);
        assert_eq!(ao_mod.outputs[1].id, "ChassisAMod2/ao1");
        assert_eq!(ao_mod.outputs[1].label, "ao1");
    }

    #[test]
    fn serial_is_formatted_the_way_ni_max_shows_it() {
        let devs = enumerate_with(&cdaq_rig()).unwrap();
        assert_eq!(devs[1].serial, "1A2B3C4D");
        // Small values are zero-padded to 8 digits, and a zero serial is shown
        // as 00000000 rather than blanked.
        assert_eq!(devs[2].serial, "000000FF");
        assert_eq!(devs[0].serial, "00000000");
        assert_eq!(format_serial(0), "00000000");
        assert_eq!(format_serial(u32::MAX), "FFFFFFFF");
    }

    #[test]
    fn simulated_flag_is_carried_through_verbatim() {
        let devs = enumerate_with(&cdaq_rig()).unwrap();
        assert!(devs.iter().all(|d| d.simulated));

        // And a REAL device set is reported as real - proving the flag is read,
        // not hard-coded.
        let mut real = cdaq_rig();
        real.simulated = HashMap::from([
            ("ChassisA", false),
            ("ChassisAMod1", false),
            ("ChassisAMod2", false),
        ]);
        let devs = enumerate_with(&real).unwrap();
        assert!(devs.iter().all(|d| !d.simulated));
    }

    #[test]
    fn a_non_cdaq_card_degrades_chassis_and_slot_to_none_without_failing() {
        // No chassis entry, no slot entry: both getters error, exactly as DAQmx
        // does for a PCIe card.
        let fake = Fake {
            dev_names: Some("Dev1".into()),
            product: HashMap::from([("Dev1", "Multifunction-Card")]),
            serial: HashMap::from([("Dev1", 0x0102_0304u32)]),
            simulated: HashMap::from([("Dev1", false)]),
            chassis: HashMap::new(),
            slot: HashMap::new(),
            ai: HashMap::from([("Dev1", "Dev1/ai0, Dev1/ai1")]),
            ao: HashMap::from([("Dev1", "Dev1/ao0")]),
        };
        let devs = enumerate_with(&fake).unwrap();
        assert_eq!(devs.len(), 1, "the scan must NOT fail");
        assert_eq!(devs[0].parent, None);
        assert_eq!(devs[0].slot, None);
        assert_eq!(devs[0].inputs.len(), 2);
        assert_eq!(devs[0].outputs.len(), 1);
    }

    #[test]
    fn an_empty_chassis_name_is_treated_as_no_chassis() {
        // DAQmx answers some non-cDAQ devices with an empty string rather than
        // an error; the two must land in the same place.
        let mut fake = cdaq_rig();
        fake.chassis.insert("ChassisAMod1", "   ");
        let devs = enumerate_with(&fake).unwrap();
        assert_eq!(devs[1].parent, None);
        assert_eq!(
            devs[1].slot, None,
            "a slot without a chassis is meaningless and must be dropped too"
        );
    }

    #[test]
    fn missing_product_and_serial_degrade_to_empty_strings() {
        let mut fake = cdaq_rig();
        fake.product.remove("ChassisAMod1");
        fake.serial.remove("ChassisAMod1");
        let devs = enumerate_with(&fake).unwrap();
        assert_eq!(devs[1].product, "");
        assert_eq!(devs[1].serial, "");
        // ...and the rest of the row still parsed.
        assert_eq!(devs[1].inputs.len(), 4);
    }

    #[test]
    fn missing_channel_lists_degrade_to_no_channels() {
        let mut fake = cdaq_rig();
        fake.ai.remove("ChassisAMod1");
        fake.ao.remove("ChassisAMod2");
        let devs = enumerate_with(&fake).unwrap();
        assert!(devs[1].inputs.is_empty());
        assert!(devs[2].outputs.is_empty());
    }

    #[test]
    fn an_unreadable_simulated_flag_fails_the_scan_rather_than_guessing() {
        let mut fake = cdaq_rig();
        fake.simulated.remove("ChassisAMod1");
        let err = enumerate_with(&fake)
            .expect_err("a device whose simulated flag cannot be read must fail the scan");
        assert!(
            err.to_string().contains("IsSimulated"),
            "the failure must name the attribute: {err}"
        );
    }

    #[test]
    fn a_failed_system_device_list_is_a_hard_error() {
        let fake = Fake {
            dev_names: None,
            ..Default::default()
        };
        assert!(enumerate_with(&fake).is_err());
    }

    #[test]
    fn no_devices_is_an_empty_list_not_an_error() {
        let fake = Fake {
            dev_names: Some(String::new()),
            ..Default::default()
        };
        assert!(enumerate_with(&fake).unwrap().is_empty());
    }

    #[test]
    fn channel_lists_tolerate_daqmx_spacing_and_stray_commas() {
        let mut fake = cdaq_rig();
        fake.ai.insert("ChassisAMod1", " Dev/ai0 ,, Dev/ai7 ,");
        let devs = enumerate_with(&fake).unwrap();
        let ids: Vec<&str> = devs[1].inputs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["Dev/ai0", "Dev/ai7"]);
        assert_eq!(devs[1].inputs[1].label, "ai7");
    }

    #[test]
    fn a_channel_id_without_a_slash_uses_the_whole_string_as_its_label() {
        let mut fake = cdaq_rig();
        fake.ai.insert("ChassisAMod1", "weirdname");
        let devs = enumerate_with(&fake).unwrap();
        assert_eq!(devs[1].inputs[0].id, "weirdname");
        assert_eq!(devs[1].inputs[0].label, "weirdname");
    }
}
