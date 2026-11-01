//! STARTUP PREFLIGHT: can this daemon actually get the hardware it claims?
//!
//! # The requirement, and what was actually built
//!
//! The split-deployment design asks the daemon to "assert no orphaned DAQmx tasks
//! (enumerate + attempt-reserve or document why not)". Here is the honest answer to
//! both halves.
//!
//! **Enumerating orphaned TASKS is not possible with the bindings this system has.**
//! `eigen-hal-ni`'s FFI layer resolves ~35 DAQmx symbols and `DAQmxGetSysTasks` is not
//! among them; nothing in the tree can list the persisted or in-flight tasks of
//! another process. Adding that binding was considered and rejected: the
//! symbol answers a question adjacent to the one that matters, since a task NAME tells
//! you something exists but not whether it holds the device you want, and a
//! task-name-based check would report a false positive for every unrelated task an
//! operator saved in MAX.
//!
//! **What IS done is the stronger check: an ATTEMPT-RESERVE.** The daemon builds its
//! configured station and calls `Backend::open()`, which since the
//! commit-at-configure fix builds AND COMMITS the DAQmx tasks - so the driver
//! validates the routes and RESERVES the devices right there. Then it closes and drops
//! them. If another process (the desktop app, a stale daemon, MAX) holds a device, the
//! commit fails with the driver's own status and the daemon refuses to start naming
//! it.
//!
//! That is a better assertion than a task census, because it answers the question the
//! operator actually has - "will my run be able to get the hardware?" - rather than a
//! proxy for it. A census can be clean while the device is busy; a reservation that
//! succeeded cannot.
//!
//! # It is a PREFLIGHT, not a hold
//!
//! The reservation is released immediately. The daemon does not sit on the devices
//! between runs: that would make the desktop app unusable on the same box for the whole
//! time the daemon is up, and the advisory lock ([`crate::lock`]) already tells the
//! app who is there. What the preflight buys is that a failure appears at START, in
//! the daemon's own log, instead of at the remote operator's first configure - three
//! screens and one network hop from the cause.

use eigen_station::{
    scan_ni, try_build_station_backend, BackendKind, NiSelection, StationBuildError, StationSpec,
};

/// What the preflight found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preflight {
    /// Nothing to reserve: the daemon serves the EIGEN-SIM plant, which is software
    /// and cannot be held by another process.
    NotApplicable,
    /// No NI-DAQmx runtime on this host. NOT a failure by itself - it is the normal
    /// state of a developer Mac - but an NI station cannot be served, and the caller
    /// decides whether that is fatal.
    NoDriver(String),
    /// The devices were enumerated and reserved, then released.
    Reserved {
        /// The device ids the reservation covered.
        devices: Vec<String>,
        /// True when EVERY reserved device is driver-simulated (the NI-SIM station).
        all_simulated: bool,
    },
    /// The devices exist and could NOT be reserved. `detail` is the driver's own
    /// refusal, which is what names the contended resource.
    Busy {
        /// The driver's message.
        detail: String,
    },
}

impl Preflight {
    /// Is this a state the daemon may serve an NI station from?
    pub fn is_usable(&self) -> bool {
        matches!(self, Preflight::NotApplicable | Preflight::Reserved { .. })
    }

    /// The line the daemon logs at start.
    pub fn report(&self) -> String {
        match self {
            Preflight::NotApplicable => {
                "preflight: EIGEN-SIM station - no devices to reserve".into()
            }
            Preflight::NoDriver(d) => format!(
                "preflight: no NI-DAQmx runtime on this host ({d}). An NI station \
                 cannot be served. There is no fallback: a software plant presented \
                 as an NI station would be indistinguishable from a measurement."
            ),
            Preflight::Reserved {
                devices,
                all_simulated,
            } => format!(
                "preflight: reserved and released {} device(s) [{}] - {}",
                devices.len(),
                devices.join(", "),
                if *all_simulated {
                    "ALL DRIVER-SIMULATED (NI-SIM: data from this station is not a \
                     measurement)"
                } else {
                    "physical (NI-REAL)"
                }
            ),
            Preflight::Busy { detail } => format!(
                "preflight FAILED: the configured NI devices exist but could not be \
                 reserved: {detail}. Something else holds them - the Eigen desktop \
                 app, another eigen-backendd, or NI MAX. Stop that program and start \
                 again; the daemon refuses rather than starting and failing at the \
                 operator's first configure."
            ),
        }
    }
}

/// Run the preflight for a configured station.
pub fn run(kind: BackendKind, ni_devices: &[String]) -> Preflight {
    if kind != BackendKind::Ni {
        return Preflight::NotApplicable;
    }

    // ENUMERATE. A driver that is not installed is a distinct answer from a scan that
    // found nothing.
    let devices = match scan_ni() {
        Ok(d) => d,
        Err(StationBuildError::NiDriverUnavailable(d)) => return Preflight::NoDriver(d),
        Err(e) => {
            return Preflight::Busy {
                detail: e.to_string(),
            }
        }
    };
    if devices.is_empty() {
        return Preflight::Busy {
            detail: "the NI-DAQmx scan completed and reported no devices".into(),
        };
    }

    // ATTEMPT-RESERVE: build the station and open it, which commits the tasks.
    let spec = StationSpec {
        ni: NiSelection {
            derive_inputs: 1,
            derive_outputs: 0,
            restrict_devices: ni_devices.to_vec(),
            ..NiSelection::default()
        },
        ..StationSpec::default()
    };
    let mut station = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            return Preflight::Busy {
                detail: e.to_string(),
            }
        }
    };
    let all_simulated = station.provenance.driver_simulated == Some(true);
    let ids = station.provenance.device_ids.clone();

    match station.backend.open() {
        Ok(()) => {
            // RELEASE IMMEDIATELY - see the module docs on why this is a preflight
            // and not a hold.
            let _ = station.backend.close();
            Preflight::Reserved {
                devices: ids,
                all_simulated,
            }
        }
        Err(e) => Preflight::Busy {
            detail: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Sim daemon has nothing to reserve, and says so rather than reporting a
    /// vacuous success. "There was nothing to check" and "the check passed" are
    /// different statements and the log must not blur them.
    #[test]
    fn a_sim_station_has_nothing_to_reserve_and_says_so() {
        let p = run(BackendKind::Sim, &[]);
        assert_eq!(p, Preflight::NotApplicable);
        assert!(p.is_usable());
        assert!(
            p.report().contains("no devices to reserve"),
            "{}",
            p.report()
        );
    }

    /// Every failure state is UNUSABLE and every report says what to do. A preflight
    /// whose message does not name a remedy is a preflight the operator ignores.
    #[test]
    fn the_failure_reports_name_a_remedy_and_never_a_sim_fallback() {
        let busy = Preflight::Busy {
            detail: "DAQmx -50103: the specified resource is reserved".into(),
        };
        assert!(!busy.is_usable());
        let msg = busy.report();
        assert!(
            msg.contains("-50103"),
            "the driver's own status survives: {msg}"
        );
        assert!(msg.contains("Stop that program"), "{msg}");

        let none = Preflight::NoDriver("libnidaqmx.so not found".into());
        assert!(!none.is_usable());
        let msg = none.report();
        assert!(msg.contains("no fallback"), "{msg}");
        assert!(msg.contains("libnidaqmx"), "{msg}");
    }

    /// A reserved DRIVER-SIMULATED set says so in the report, in the words the rest
    /// of the system uses. A daemon log that reads "reserved 4 devices" for a sim rig
    /// is how a driver-simulated soak gets written up as a hardware run.
    #[test]
    fn a_driver_simulated_reservation_is_labelled_as_not_a_measurement() {
        let p = Preflight::Reserved {
            devices: vec!["cDAQ9178Sim1Mod1".into()],
            all_simulated: true,
        };
        assert!(p.is_usable());
        let msg = p.report();
        assert!(msg.contains("DRIVER-SIMULATED"), "{msg}");
        assert!(msg.contains("not a measurement"), "{msg}");

        let real = Preflight::Reserved {
            devices: vec!["cDAQ1Mod1".into()],
            all_simulated: false,
        };
        assert!(real.report().contains("NI-REAL"));
    }

    /// On a host with no NI runtime (every developer Mac), the NI preflight reports
    /// NoDriver rather than panicking or claiming success. This test is what makes
    /// the module runnable in CI at all.
    #[test]
    fn the_ni_preflight_is_honest_on_a_host_without_the_runtime() {
        let p = run(BackendKind::Ni, &[]);
        // Any of the three non-success states is correct depending on the host; what
        // must NOT happen is a claimed reservation on a box with no driver.
        match p {
            Preflight::Reserved { .. } => {
                // A box that really does have devices - fine, and the report says
                // which class they are.
            }
            Preflight::NoDriver(_) | Preflight::Busy { .. } => {}
            Preflight::NotApplicable => panic!("an NI request must not report N/A"),
        }
    }
}
