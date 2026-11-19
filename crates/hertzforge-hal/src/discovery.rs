//! Backend-neutral hardware discovery types.
//!
//! A backend that can SCAN its bus (a DAQ driver enumerating attached devices, a
//! future USB/Ethernet discovery) reports what it found as [`DiscoveredDevice`]s.
//! These types are the seam between the scanning backend and everything above it:
//! the UI's station tree, the channel table's available-channel list, and run
//! provenance.
//!
//! ## Protocol law: no vendor vocabulary crosses this seam
//!
//! Nothing in this module names a vendor, a driver, or a device-specific concept.
//! There is no `physical_channel`, no `ai0`, no chassis/slot *semantics* beyond
//! "this device has a parent and sits in a numbered position". A backend that
//! happens to key its channels by a vendor string puts that string in
//! [`DiscoveredChannel::id`], which is **opaque to every consumer**: only the
//! backend that produced an id may parse it. Consumers match ids for equality,
//! store them, and hand them back - never split, prefix-match, or interpret them.
//! (The one exception is [`DiscoveredChannel::label`], which exists precisely so a
//! consumer has something short and human-readable to render WITHOUT parsing the
//! id.)
//!
//! This is the same rule the UI<->core seam carries in `hertzforge-protocol`: the layer
//! above speaks intent and identity, never device mechanics.
//!
//! ## Why the fields are what they are
//!
//! `serial` and `simulated` are load-bearing for the anti-sim-leak architecture:
//! a driver-simulated device is a THIRD data category, distinct from real hardware
//! and from Hertzforge's own `SimBackend` plant, and it must be labeled as such
//! everywhere it appears. A scan row that shows a serial but hides
//! `simulated: true` is the exact failure this project has already paid for once.
//!
//! `serial` is a `String`, not an integer: serial-number formatting is a
//! per-vendor presentation decision (some render hex, some decimal, some
//! alphanumeric), and the value's only jobs are display and equality.

use serde::{Deserialize, Serialize};

/// One channel a backend found on a device.
///
/// `id` is the backend's opaque handle for the channel - the value that gets
/// handed back to that backend to select it. `label` is a short human string for
/// rendering (a row caption, a tree leaf). Consumers MUST NOT parse `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiscoveredChannel {
    /// Backend-opaque channel identity. Stable for as long as the device is
    /// attached under the same name. Never parsed outside its own backend.
    pub id: String,
    /// Short human-readable caption for this channel. Display only.
    pub label: String,
}

impl DiscoveredChannel {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        DiscoveredChannel {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// One device a backend found while scanning.
///
/// Devices form a shallow tree: a module in a chassis carries `parent =
/// Some(chassis_id)` and `slot = Some(n)`; a standalone card carries both as
/// `None`. A backend that cannot determine a parent/slot reports `None` rather
/// than guessing - absence is a fact, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    /// Backend-opaque device identity (also the key `parent` refers to).
    pub id: String,
    /// Product/model string as the backend reports it (e.g. a module model name).
    pub product: String,
    /// Serial number as a display string. Empty when the backend cannot read one.
    pub serial: String,
    /// True when the DRIVER reports this device as simulated - i.e. no physical
    /// hardware exists behind it. Asserted by the driver, never by a human, and
    /// never inferred from a name. Consumers MUST surface this distinctly.
    pub simulated: bool,
    /// Enclosing device (chassis) id, when this device sits inside one.
    pub parent: Option<String>,
    /// Position within `parent`, when the backend can report one.
    pub slot: Option<u32>,
    /// Measurement (input) channels on this device.
    pub inputs: Vec<DiscoveredChannel>,
    /// Drive (output) channels on this device.
    pub outputs: Vec<DiscoveredChannel>,
}

impl DiscoveredDevice {
    /// A device with no channels, no parent, and no serial - the shape a backend
    /// starts from before it fills in whatever attributes it can actually read.
    pub fn new(id: impl Into<String>, product: impl Into<String>) -> Self {
        DiscoveredDevice {
            id: id.into(),
            product: product.into(),
            serial: String::new(),
            simulated: false,
            parent: None,
            slot: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// Total channel count (inputs + outputs).
    pub fn channel_count(&self) -> usize {
        self.inputs.len() + self.outputs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_device_defaults_are_the_honest_unknowns() {
        // A freshly-constructed device claims nothing it has not read: no serial,
        // not simulated, no parent/slot, no channels.
        let d = DiscoveredDevice::new("DevX", "Some-Product");
        assert_eq!(d.id, "DevX");
        assert_eq!(d.product, "Some-Product");
        assert_eq!(d.serial, "");
        assert!(!d.simulated);
        assert_eq!(d.parent, None);
        assert_eq!(d.slot, None);
        assert_eq!(d.channel_count(), 0);
    }

    #[test]
    fn channel_count_sums_both_directions() {
        let mut d = DiscoveredDevice::new("DevX", "P");
        d.inputs.push(DiscoveredChannel::new("DevX/in0", "in0"));
        d.inputs.push(DiscoveredChannel::new("DevX/in1", "in1"));
        d.outputs.push(DiscoveredChannel::new("DevX/out0", "out0"));
        assert_eq!(d.channel_count(), 3);
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.outputs.len(), 1);
    }

    #[test]
    fn round_trips_through_json_including_the_simulated_flag() {
        // The `simulated` flag is the anti-sim-leak label: it must survive every
        // hop between the backend that read it and the badge that renders it.
        let mut chassis = DiscoveredDevice::new("Chassis1", "Chassis-Product");
        chassis.simulated = true;
        chassis.serial = "0F1E2D3C".into();
        let mut module = DiscoveredDevice::new("Chassis1Mod1", "Module-Product");
        module.simulated = true;
        module.parent = Some("Chassis1".into());
        module.slot = Some(1);
        module
            .inputs
            .push(DiscoveredChannel::new("Chassis1Mod1/in0", "in0"));
        module
            .outputs
            .push(DiscoveredChannel::new("Chassis1Mod1/out0", "out0"));

        let tree = vec![chassis, module];
        let json = serde_json::to_string(&tree).unwrap();
        let back: Vec<DiscoveredDevice> = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
        assert!(back.iter().all(|d| d.simulated));
        assert_eq!(back[1].parent.as_deref(), Some("Chassis1"));
        assert_eq!(back[1].slot, Some(1));
    }

    #[test]
    fn label_is_separate_from_id_so_consumers_never_parse_the_id() {
        // The whole point of `label`: a caller can render something short without
        // splitting the opaque id.
        let c = DiscoveredChannel::new("SomeBackendOpaque::Handle#7", "in7");
        assert_ne!(c.id, c.label);
        assert_eq!(c.label, "in7");
    }
}
