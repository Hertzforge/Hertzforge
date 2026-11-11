//! Stations (schema/PROTOCOL.md, "Stations"): the simulator and NI-DAQmx hardware.
//!
//! Every station is built through `hertzforge-station`, the controller's one
//! construction point, and never falls back to the simulator: a missing, busy or
//! refused NI selection is refused by name. Selecting opens nothing; the station is
//! built when a run arms ([`build`]).
//!
//! The NI scan sits behind [`NiScanner`] so the selection and binding rules are
//! tested on a computer with no NI driver ([`FixedScanner`]). The real scanner is
//! the station crate's own scan.

use crate::protocol::{ErrorCode, MethodError};
use hertzforge_hal::DiscoveredDevice;
use hertzforge_module_abi::StationClass;
use hertzforge_station::{
    BackendKind, NiSelection, SimPlant, SimPlantSpec, StationBackend, StationBuildError,
    StationSpec,
};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

/// Inputs and outputs the simulator offers.
pub const SIM_INPUTS: u32 = 8;
pub const SIM_OUTPUTS: u32 = 1;

/// The simulator's plant: two resonances, `(f_n, Q)`, on one shared sample clock. The
/// plant the controller's Random acceptance gates were validated against.
pub const SIM_RESONANCES: [(f64, f64); 2] = [(200.0, 2.5), (550.0, 2.5)];

/// The NI-DAQmx scan.
pub trait NiScanner: Send + Sync {
    fn scan(&self) -> Result<Vec<DiscoveredDevice>, StationBuildError>;
}

/// The driver's own scan, through the station crate.
pub struct DriverScanner;

impl NiScanner for DriverScanner {
    fn scan(&self) -> Result<Vec<DiscoveredDevice>, StationBuildError> {
        hertzforge_station::scan_ni()
    }
}

/// A fixed scan, for tests and demonstrations on a computer without NI hardware.
pub struct FixedScanner(pub Result<Vec<DiscoveredDevice>, StationBuildError>);

impl NiScanner for FixedScanner {
    fn scan(&self) -> Result<Vec<DiscoveredDevice>, StationBuildError> {
        self.0.clone()
    }
}

/// The kind of station selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Sim,
    Ni { devices: Vec<String> },
}

/// One scan's outcome.
#[derive(Debug, Clone)]
pub struct Scan {
    pub at_unix: u64,
    pub result: Result<Vec<DiscoveredDevice>, ScanFailure>,
}

/// Why the NI station is unavailable.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanFailure {
    pub reason: &'static str,
    pub message: String,
}

impl ScanFailure {
    pub fn to_error(&self) -> MethodError {
        MethodError::named(ErrorCode::Refused, self.reason, self.message.clone())
    }
}

/// The station state the service holds: the scanner, the latest scan and the
/// selection.
pub struct Stations {
    scanner: Arc<dyn NiScanner>,
    last: Option<Scan>,
    selected: Selection,
    /// The class of the NI selection, from the scan it was checked against.
    ni_class: StationClass,
}

impl Stations {
    pub fn new(scanner: Arc<dyn NiScanner>) -> Stations {
        Stations {
            scanner,
            last: None,
            selected: Selection::Sim,
            ni_class: StationClass::NiSimulated,
        }
    }

    pub fn scanner(&self) -> Arc<dyn NiScanner> {
        self.scanner.clone()
    }

    pub fn selected(&self) -> &Selection {
        &self.selected
    }

    pub fn last_scan(&self) -> Option<&Scan> {
        self.last.as_ref()
    }

    /// The devices of the latest successful scan (empty when none).
    pub fn devices(&self) -> &[DiscoveredDevice] {
        match &self.last {
            Some(Scan { result: Ok(d), .. }) => d,
            _ => &[],
        }
    }

    /// Record a scan taken off the async runtime.
    pub fn record_scan(&mut self, scan: Scan) {
        self.last = Some(scan);
    }

    /// The selected station's class.
    pub fn class(&self) -> StationClass {
        match self.selected {
            Selection::Sim => StationClass::HertzforgeSim,
            Selection::Ni { .. } => self.ni_class,
        }
    }

    /// Select the simulator.
    pub fn select_sim(&mut self) {
        self.selected = Selection::Sim;
    }

    /// Select NI devices, checked against `scan` (fresh) and the hardware lock.
    pub fn select_ni(
        &mut self,
        devices: Vec<String>,
        scan: Scan,
        busy: Option<String>,
    ) -> Result<(), MethodError> {
        self.last = Some(scan.clone());
        let found = scan.result.map_err(|f| f.to_error())?;
        let class = check_ni_selection(&devices, &found)?;
        if let Some(holder) = busy {
            return Err(MethodError::named(
                ErrorCode::Refused,
                "hardware_busy",
                format!("Another program holds the NI hardware: {holder}"),
            )
            .with_detail(json!({ "holder": holder })));
        }
        self.selected = Selection::Ni { devices };
        self.ni_class = class;
        Ok(())
    }

    /// The `station.list` result.
    pub fn list(&self) -> Value {
        json!({
            "stations": [sim_station(), self.ni_station()],
            "selected": self.selected_json(),
        })
    }

    fn ni_station(&self) -> Value {
        let mut m = Map::new();
        m.insert("kind".into(), Value::from("ni"));
        m.insert("title".into(), Value::from("NI-DAQmx"));
        match &self.last {
            None => {
                m.insert("available".into(), Value::from(false));
                m.insert(
                    "unavailable".into(),
                    json!({"reason": "scan_failed",
                           "message": "The NI devices have not been scanned yet; rescan."}),
                );
                m.insert("scanned_unix".into(), Value::Null);
                m.insert("devices".into(), json!([]));
            }
            Some(scan) => match &scan.result {
                Ok(devices) => {
                    m.insert("available".into(), Value::from(true));
                    m.insert("unavailable".into(), Value::Null);
                    m.insert("scanned_unix".into(), Value::from(scan.at_unix));
                    m.insert("devices".into(), Value::Array(devices_json(devices)));
                }
                Err(f) => {
                    m.insert("available".into(), Value::from(false));
                    m.insert(
                        "unavailable".into(),
                        json!({"reason": f.reason, "message": f.message}),
                    );
                    m.insert("scanned_unix".into(), Value::from(scan.at_unix));
                    m.insert("devices".into(), json!([]));
                }
            },
        }
        Value::Object(m)
    }

    /// The `selected` object.
    pub fn selected_json(&self) -> Value {
        match &self.selected {
            Selection::Sim => json!({
                "kind": "sim", "devices": [], "class": "sim", "source": "SIM",
                "inputs": SIM_INPUTS, "outputs": SIM_OUTPUTS, "timing_family": null,
            }),
            Selection::Ni { devices } => {
                let scan = self.devices();
                let chosen: Vec<&DiscoveredDevice> =
                    scan.iter().filter(|d| devices.contains(&d.id)).collect();
                let families = hertzforge_station::timing_families(scan);
                let family = devices
                    .first()
                    .and_then(|id| families.iter().find(|(d, _)| d == id))
                    .map(|(_, f)| Value::from(f.clone()))
                    .unwrap_or(Value::Null);
                let class = self.ni_class;
                json!({
                    "kind": "ni",
                    "devices": devices,
                    "class": class_name(class),
                    "source": source_of(class),
                    "inputs": chosen.iter().map(|d| d.inputs.len()).sum::<usize>(),
                    "outputs": chosen.iter().map(|d| d.outputs.len()).sum::<usize>(),
                    "timing_family": family,
                })
            }
        }
    }
}

/// The simulator's station entry.
pub fn sim_station() -> Value {
    json!({"kind": "sim", "title": "Simulator", "available": true, "class": "sim",
           "inputs": SIM_INPUTS, "outputs": SIM_OUTPUTS, "clock": "shared"})
}

/// The protocol name of a station class.
pub fn class_name(class: StationClass) -> &'static str {
    match class {
        StationClass::NiReal => "ni_real",
        StationClass::NiSimulated => "ni_simulated",
        _ => "sim",
    }
}

/// `source` follows the class: only physical NI devices give HARDWARE.
pub fn source_of(class: StationClass) -> &'static str {
    match class {
        StationClass::NiReal => "HARDWARE",
        _ => "SIM",
    }
}

/// Take a scan now (blocking: call it off the async runtime).
pub fn take_scan(scanner: &dyn NiScanner) -> Scan {
    let at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let result = match scanner.scan() {
        Ok(d) if d.is_empty() => Err(ScanFailure {
            reason: "no_ni_devices",
            message: "The NI-DAQmx scan found no devices. Check that the chassis is powered \
                      and connected, then rescan."
                .into(),
        }),
        Ok(d) => Ok(d),
        Err(StationBuildError::NiDriverUnavailable(_)) => Err(ScanFailure {
            reason: "ni_driver_unavailable",
            message: "The NI-DAQmx runtime is not installed on this computer, so no NI \
                      station can be used. Install NI-DAQmx, or run on the Simulator."
                .into(),
        }),
        Err(e) => Err(ScanFailure {
            reason: "scan_failed",
            message: format!("The NI-DAQmx driver refused the scan: {e}"),
        }),
    };
    Scan { at_unix, result }
}

/// The trailing segment of a discovered channel id: the backend names a channel
/// `<device>/<channel>`, and the protocol and the channel table carry the two halves
/// apart.
pub fn channel_id(device: &str, id: &str) -> String {
    id.strip_prefix(device)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(id)
        .to_owned()
}

fn devices_json(devices: &[DiscoveredDevice]) -> Vec<Value> {
    let families = hertzforge_station::timing_families(devices);
    devices
        .iter()
        .map(|d| {
            let family = families
                .iter()
                .find(|(id, _)| id == &d.id)
                .map(|(_, f)| Value::from(f.clone()))
                .unwrap_or(Value::Null);
            let chans = |list: &[hertzforge_hal::DiscoveredChannel]| -> Value {
                Value::Array(
                    list.iter()
                        .map(|c| json!({"id": channel_id(&d.id, &c.id), "label": c.label}))
                        .collect(),
                )
            };
            json!({
                "id": d.id, "product": d.product, "serial": d.serial,
                "simulated": d.simulated, "parent": d.parent, "slot": d.slot,
                "timing_family": family,
                "inputs": chans(&d.inputs), "outputs": chans(&d.outputs),
            })
        })
        .collect()
}

/// Check an NI device selection against a scan: every device present, all
/// driver-simulated or all physical, one timing family. Returns the class.
pub fn check_ni_selection(
    devices: &[String],
    scan: &[DiscoveredDevice],
) -> Result<StationClass, MethodError> {
    let found: Vec<String> = scan.iter().map(|d| d.id.clone()).collect();
    let missing: Vec<String> = devices
        .iter()
        .filter(|d| !found.contains(d))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "devices_absent",
            format!(
                "Selected device not present: {}. The scan reports {}.",
                missing.join(", "),
                if found.is_empty() {
                    "no devices".to_owned()
                } else {
                    found.join(", ")
                }
            ),
        )
        .with_detail(json!({"missing": missing, "found": found})));
    }
    let chosen: Vec<&DiscoveredDevice> = scan.iter().filter(|d| devices.contains(&d.id)).collect();
    let simulated: Vec<String> = chosen
        .iter()
        .filter(|d| d.simulated)
        .map(|d| d.id.clone())
        .collect();
    let real: Vec<String> = chosen
        .iter()
        .filter(|d| !d.simulated)
        .map(|d| d.id.clone())
        .collect();
    if !simulated.is_empty() && !real.is_empty() {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "mixed_simulated_and_real",
            format!(
                "The selection mixes driver-simulated devices ({}) with physical ones ({}); a \
                 run on both would be half a measurement. Select one kind.",
                simulated.join(", "),
                real.join(", ")
            ),
        )
        .with_detail(json!({"simulated": simulated, "real": real})));
    }
    let families_of = hertzforge_station::timing_families(scan);
    let mut families: Vec<String> = Vec::new();
    for id in devices {
        if let Some((_, f)) = families_of.iter().find(|(d, _)| d == id) {
            if !families.contains(f) {
                families.push(f.clone());
            }
        }
    }
    if families.len() > 1 {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "mixed_timing_families",
            format!(
                "The selected devices cannot share one sample clock (timing families {}). \
                 Select devices in one chassis, or one standalone device.",
                families.join(", ")
            ),
        )
        .with_detail(json!({ "families": families })));
    }
    Ok(if real.is_empty() {
        StationClass::NiSimulated
    } else {
        StationClass::NiReal
    })
}

/// Who holds the NI hardware, if another live process does.
pub fn hardware_holder(data_root: &Path) -> Option<String> {
    let holder = hertzforge_station::lock::read_live(data_root)?;
    (holder.pid != std::process::id()).then(|| holder.advice())
}

/// What a run arms on.
#[derive(Debug, Clone)]
pub enum StationPlan {
    /// The simulator. With `plant` and `outputs` both `None` it is the station every
    /// single-output run arms on: the [`SIM_RESONANCES`] bank, [`SIM_INPUTS`] inputs and
    /// [`SIM_OUTPUTS`] output. A sized `plant` (modal or FRF) sets the station's size to
    /// its own and refuses a table of any other size at `configure`; `outputs` raises
    /// the output count offered by the resonator bank (which plays the first drive row).
    Sim {
        fs_hz: f64,
        /// The plant, when not the resonator bank.
        plant: Option<SimPlant>,
        /// Outputs offered, when not [`SIM_OUTPUTS`].
        outputs: Option<u32>,
    },
    Ni {
        devices: Vec<String>,
        inputs: Vec<String>,
        outputs: Vec<String>,
        fs_hz: f64,
    },
}

impl StationPlan {
    /// The simulator every single-output run arms on: the resonator bank, one output.
    pub fn sim(fs_hz: f64) -> StationPlan {
        StationPlan::Sim {
            fs_hz,
            plant: None,
            outputs: None,
        }
    }
}

/// Build the station a run arms on. The simulator through the factory; NI from a
/// fresh scan through the factory's own classification. Never a simulator for NI.
pub fn build(plan: &StationPlan, scanner: &dyn NiScanner) -> Result<StationBackend, MethodError> {
    match plan {
        StationPlan::Sim {
            fs_hz,
            plant,
            outputs,
        } => {
            let spec = StationSpec {
                sim: SimPlantSpec {
                    sample_rate_hz: *fs_hz,
                    resonances: SIM_RESONANCES.to_vec(),
                    noise_level: 2e-4,
                    feedthrough: 0.35,
                    // A software plant advances input and output on one sample index;
                    // the arm gate requires the claim to be stated.
                    shared_clock: true,
                    seed: 0x5A5A_1234,
                    max_input_channels: SIM_INPUTS,
                    max_output_channels: outputs.unwrap_or(SIM_OUTPUTS),
                    plant: plant.clone().unwrap_or_default(),
                    ..SimPlantSpec::default()
                },
                ..StationSpec::default()
            };
            hertzforge_station::try_build_station_backend(BackendKind::Sim, &spec)
                .map_err(build_refusal)
        }
        StationPlan::Ni {
            devices,
            inputs,
            outputs,
            fs_hz,
        } => {
            let scan = scanner.scan().map_err(build_refusal)?;
            let sel = NiSelection {
                inputs: inputs.clone(),
                outputs: outputs.clone(),
                derive_inputs: inputs.len().max(1),
                derive_outputs: outputs.len(),
                // IEPE is applied per row from the channel table at configure; the
                // task is built without it.
                iepe: false,
                restrict_devices: devices.clone(),
            };
            hertzforge_station::ni_station_from_scan(
                scan,
                &sel,
                hertzforge_hal::default_block_frames(*fs_hz),
            )
            .map_err(build_refusal)
        }
    }
}

/// A station refusal by name.
pub fn build_refusal(e: StationBuildError) -> MethodError {
    let text = e.to_string();
    match e {
        StationBuildError::NiDriverUnavailable(_) => {
            MethodError::named(ErrorCode::Refused, "ni_driver_unavailable", text)
        }
        StationBuildError::NoNiDevices => {
            MethodError::named(ErrorCode::Refused, "no_ni_devices", text)
        }
        StationBuildError::NotEnoughNiChannels {
            want_inputs,
            want_outputs,
            have_inputs,
            have_outputs,
        } => MethodError::named(ErrorCode::Refused, "not_enough_channels", text).with_detail(
            json!({"want_inputs": want_inputs, "want_outputs": want_outputs,
                   "have_inputs": have_inputs, "have_outputs": have_outputs}),
        ),
        StationBuildError::MixedSimulatedAndReal { simulated, real } => {
            MethodError::named(ErrorCode::Refused, "mixed_simulated_and_real", text)
                .with_detail(json!({"simulated": simulated, "real": real}))
        }
        StationBuildError::SelectedDevicesAbsent { missing, found } => {
            MethodError::named(ErrorCode::Refused, "devices_absent", text)
                .with_detail(json!({"missing": missing, "found": found}))
        }
        StationBuildError::MixedTimingFamilies(_) => {
            MethodError::named(ErrorCode::Refused, "mixed_timing_families", text)
        }
        StationBuildError::Ni(_) => MethodError::named(ErrorCode::Refused, "arm_refused", text),
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use hertzforge_hal::DiscoveredChannel;

    /// The bench rig's shape: a chassis, a four-input module and a four-output module.
    pub fn rig(simulated: bool) -> Vec<DiscoveredDevice> {
        let dev = |id: &str, product: &str, n_in: usize, n_out: usize, slot: Option<u32>| {
            let mut d = DiscoveredDevice::new(id, product);
            d.serial = if simulated {
                String::new()
            } else {
                format!("SN-{id}")
            };
            d.simulated = simulated;
            if slot.is_some() {
                d.parent = Some("cDAQ2".into());
                d.slot = slot;
            }
            d.inputs = (0..n_in)
                .map(|k| DiscoveredChannel::new(format!("{id}/ai{k}"), format!("ai{k}")))
                .collect();
            d.outputs = (0..n_out)
                .map(|k| DiscoveredChannel::new(format!("{id}/ao{k}"), format!("ao{k}")))
                .collect();
            d
        };
        vec![
            dev("cDAQ2", "cDAQ-9178", 0, 0, None),
            dev("cDAQ2Mod1", "NI 9234", 4, 0, Some(1)),
            dev("cDAQ2Mod2", "NI 9263", 0, 4, Some(2)),
        ]
    }

    /// A standalone card: its own timing family.
    pub fn card(simulated: bool) -> DiscoveredDevice {
        let mut d = DiscoveredDevice::new("Dev1", "PCIe-6363");
        d.simulated = simulated;
        d.inputs = vec![DiscoveredChannel::new("Dev1/ai0", "ai0")];
        d.outputs = vec![DiscoveredChannel::new("Dev1/ao0", "ao0")];
        d
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn scan_of(devices: Vec<DiscoveredDevice>) -> Scan {
        Scan {
            at_unix: 1,
            result: Ok(devices),
        }
    }

    #[test]
    fn the_service_starts_on_the_simulator() {
        let s = Stations::new(Arc::new(FixedScanner(Ok(rig(false)))));
        assert_eq!(s.selected(), &Selection::Sim);
        let sel = s.selected_json();
        assert_eq!(sel["class"], "sim");
        assert_eq!(sel["source"], "SIM");
    }

    #[test]
    fn physical_devices_are_hardware_and_driver_simulated_ones_are_not() {
        let mut s = Stations::new(Arc::new(FixedScanner(Ok(rig(false)))));
        let devices = vec!["cDAQ2Mod1".to_owned(), "cDAQ2Mod2".to_owned()];
        s.select_ni(devices.clone(), scan_of(rig(false)), None)
            .unwrap();
        let sel = s.selected_json();
        assert_eq!(sel["class"], "ni_real");
        assert_eq!(sel["source"], "HARDWARE");
        assert_eq!(sel["inputs"], 4);
        assert_eq!(sel["outputs"], 4);
        assert_eq!(sel["timing_family"], "cDAQ2");

        s.select_ni(devices, scan_of(rig(true)), None).unwrap();
        let sel = s.selected_json();
        assert_eq!(
            sel["class"], "ni_simulated",
            "driver-simulated NI is not hardware"
        );
        assert_eq!(sel["source"], "SIM");
    }

    #[test]
    fn a_selection_is_refused_by_name_and_never_falls_back() {
        let mut s = Stations::new(Arc::new(FixedScanner(Ok(rig(false)))));
        let e = s
            .select_ni(vec!["cDAQ9Mod1".into()], scan_of(rig(false)), None)
            .unwrap_err();
        assert_eq!(e.reason, Some("devices_absent"));
        assert_eq!(e.detail.as_ref().unwrap()["missing"][0], "cDAQ9Mod1");
        assert_eq!(
            s.selected(),
            &Selection::Sim,
            "the selection did not change"
        );

        let mut mixed = rig(false);
        mixed[2].simulated = true;
        let e = s
            .select_ni(
                vec!["cDAQ2Mod1".into(), "cDAQ2Mod2".into()],
                scan_of(mixed),
                None,
            )
            .unwrap_err();
        assert_eq!(e.reason, Some("mixed_simulated_and_real"));

        let mut two = rig(false);
        two.push(card(false));
        let e = s
            .select_ni(vec!["cDAQ2Mod1".into(), "Dev1".into()], scan_of(two), None)
            .unwrap_err();
        assert_eq!(e.reason, Some("mixed_timing_families"));

        let no_driver = Scan {
            at_unix: 1,
            result: Err(ScanFailure {
                reason: "ni_driver_unavailable",
                message: "not installed".into(),
            }),
        };
        let e = s
            .select_ni(vec!["cDAQ2Mod1".into()], no_driver, None)
            .unwrap_err();
        assert_eq!(e.reason, Some("ni_driver_unavailable"));

        let e = s
            .select_ni(
                vec!["cDAQ2Mod1".into()],
                scan_of(rig(false)),
                Some("another process".into()),
            )
            .unwrap_err();
        assert_eq!(e.reason, Some("hardware_busy"));
        assert_eq!(s.selected(), &Selection::Sim);
    }

    #[test]
    fn the_scan_maps_to_the_named_unavailable_reasons() {
        let s = take_scan(&FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "tried nicaiu".into(),
        ))));
        assert_eq!(s.result.unwrap_err().reason, "ni_driver_unavailable");
        let s = take_scan(&FixedScanner(Ok(Vec::new())));
        assert_eq!(s.result.unwrap_err().reason, "no_ni_devices");
        let s = take_scan(&FixedScanner(Err(StationBuildError::Ni("-50103".into()))));
        assert_eq!(s.result.unwrap_err().reason, "scan_failed");
    }

    #[test]
    fn the_list_names_chassis_modules_and_channel_halves() {
        let mut s = Stations::new(Arc::new(FixedScanner(Ok(rig(false)))));
        s.record_scan(scan_of(rig(false)));
        let list = s.list();
        let ni = &list["stations"][1];
        assert_eq!(ni["available"], true);
        let devices = ni["devices"].as_array().unwrap();
        assert_eq!(devices[0]["id"], "cDAQ2");
        assert_eq!(devices[0]["parent"], Value::Null);
        assert_eq!(devices[1]["parent"], "cDAQ2");
        assert_eq!(devices[1]["slot"], 1);
        assert_eq!(devices[1]["timing_family"], "cDAQ2");
        assert_eq!(
            devices[1]["inputs"][0],
            json!({"id": "ai0", "label": "ai0"}),
            "the channel id is the trailing half, as the table binds it"
        );
        assert_eq!(devices[2]["outputs"][3]["id"], "ao3");
    }

    #[test]
    fn an_ni_run_is_built_from_the_scan_and_is_never_a_simulator() {
        let plan = StationPlan::Ni {
            devices: vec!["cDAQ2Mod1".into(), "cDAQ2Mod2".into()],
            inputs: vec!["cDAQ2Mod1/ai0".into()],
            outputs: vec!["cDAQ2Mod2/ao0".into()],
            fs_hz: 12_800.0,
        };
        let st = build(&plan, &FixedScanner(Ok(rig(false)))).unwrap();
        assert_eq!(st.station, StationClass::NiReal);
        assert_eq!(st.provenance.backend_kind, "hertzforge-hal-ni");
        let st = build(&plan, &FixedScanner(Ok(rig(true)))).unwrap();
        assert_eq!(st.station, StationClass::NiSimulated);

        let e = build(
            &plan,
            &FixedScanner(Err(StationBuildError::NiDriverUnavailable("x".into()))),
        )
        .unwrap_err();
        assert_eq!(e.reason, Some("ni_driver_unavailable"));
        let mut gone = rig(false);
        gone.remove(2);
        let e = build(&plan, &FixedScanner(Ok(gone))).unwrap_err();
        assert_eq!(e.reason, Some("devices_absent"));
    }

    #[test]
    fn the_sim_run_is_built_on_the_shared_clock_plant() {
        let st = build(&StationPlan::sim(8192.0), &DriverScanner).unwrap();
        assert_eq!(st.station, StationClass::HertzforgeSim);
        let caps = st.backend.caps();
        assert_eq!(caps.clock, hertzforge_hal::ClockDomain::Shared);
        assert_eq!(caps.max_input_channels, SIM_INPUTS);
    }

    /// `n_in` Control rows then `n_drive` Drive rows.
    fn mimo_shock_table(n_in: u32, n_drive: u32) -> hertzforge_channel_table::ChannelTable {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
        let mut t = ChannelTable::new();
        for i in 0..n_in {
            let mut c = Channel::new(i);
            c.role = ChannelRole::Control;
            t.channels.push(c);
        }
        for d in 0..n_drive {
            let mut c = Channel::new(n_in + d);
            c.role = ChannelRole::Drive;
            c.units = Units::VoltageV;
            t.channels.push(c);
        }
        t
    }

    fn mimo_shock_arm(
        st: &mut StationBackend,
        t: &hertzforge_channel_table::ChannelTable,
    ) -> Result<(), hertzforge_hal::HalError> {
        st.backend.open()?;
        let acq = hertzforge_hal::AcquisitionConfig::new(st.backend.caps().sample_rate_hz, 256);
        st.backend.configure(t, &acq)?;
        st.backend.arm()?;
        st.backend.stop()
    }

    #[test]
    fn mimo_shock_the_default_sim_plan_is_the_single_output_station() {
        let plan = StationPlan::sim(8192.0);
        assert!(matches!(
            plan,
            StationPlan::Sim {
                plant: None,
                outputs: None,
                ..
            }
        ));
        let mut st = build(&plan, &DriverScanner).unwrap();
        let caps = st.backend.caps();
        assert_eq!(caps.max_input_channels, SIM_INPUTS);
        assert_eq!(caps.max_output_channels, SIM_OUTPUTS);
        assert_eq!(caps.sample_rate_hz, 8192.0);
        // One drive arms; a second drive row is refused by the station itself.
        mimo_shock_arm(&mut st, &mimo_shock_table(1, 1)).unwrap();
        let mut st = build(&plan, &DriverScanner).unwrap();
        let e = mimo_shock_arm(&mut st, &mimo_shock_table(1, 2)).unwrap_err();
        assert!(e.to_string().contains("exceeds max 1"), "{e}");
    }

    #[test]
    fn mimo_shock_a_modal_four_by_four_plant_arms_with_four_drive_rows() {
        let plant = hertzforge_station::SimStandardPlant::from_name("modal-4x4").unwrap();
        let plan = StationPlan::Sim {
            fs_hz: 8192.0,
            plant: Some(SimPlant::Modal(plant)),
            outputs: Some(4),
        };
        let mut st = build(&plan, &DriverScanner).unwrap();
        assert_eq!(st.station, StationClass::HertzforgeSim);
        assert_eq!(st.backend.caps().max_output_channels, 4);
        mimo_shock_arm(&mut st, &mimo_shock_table(4, 4)).unwrap();
        // The sized plant refuses a table of another size.
        let mut st = build(&plan, &DriverScanner).unwrap();
        assert!(mimo_shock_arm(&mut st, &mimo_shock_table(4, 3)).is_err());
    }

    #[test]
    fn mimo_shock_an_output_count_alone_widens_the_resonator_station() {
        let plan = StationPlan::Sim {
            fs_hz: 8192.0,
            plant: None,
            outputs: Some(2),
        };
        let mut st = build(&plan, &DriverScanner).unwrap();
        assert_eq!(st.backend.caps().max_output_channels, 2);
        mimo_shock_arm(&mut st, &mimo_shock_table(1, 2)).unwrap();
    }
}
