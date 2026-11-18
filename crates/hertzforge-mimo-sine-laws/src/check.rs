//! The law check of a MIMO Sine law: not built in this version.
//!
//! The check that gates arming a law on hardware, as in every other world,
//! runs each law through whole runs on the engine on the standard simulated
//! structures, one case at a time, each with requirements stated before any
//! law is run. The engine that runs those cases is not in this version, so
//! no case is run and no law file is recorded as checked: [`run_check`]
//! answers [`CheckNotBuilt`]. The cases are named here so the catalog, the
//! records and the screens can refer to them now.

use std::fmt;

/// The cases the check will run, one simulated run each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimoSineCheckCase {
    /// A slow sweep on the linear four-by-four structure.
    SlowSweep,
    /// A fast sweep on the same structure.
    FastSweep,
    /// Two resonances close together.
    CloseModes,
    /// A frequency where the identification is nearly singular.
    NearSingular,
    /// Noise on the measurement.
    MeasurementNoise,
    /// A level ladder up to full level.
    LevelLadder,
    /// A limit channel near its bound.
    LimitChannel,
    /// A delay between the drive and its measurement.
    TransportDelay,
    /// More controls than drives.
    MoreControlsThanDrives,
    /// More drives than controls.
    MoreDrivesThanControls,
}

impl MimoSineCheckCase {
    /// Every case, in the order the check will run them.
    pub const ALL: [MimoSineCheckCase; 10] = [
        MimoSineCheckCase::SlowSweep,
        MimoSineCheckCase::FastSweep,
        MimoSineCheckCase::CloseModes,
        MimoSineCheckCase::NearSingular,
        MimoSineCheckCase::MeasurementNoise,
        MimoSineCheckCase::LevelLadder,
        MimoSineCheckCase::LimitChannel,
        MimoSineCheckCase::TransportDelay,
        MimoSineCheckCase::MoreControlsThanDrives,
        MimoSineCheckCase::MoreDrivesThanControls,
    ];

    /// The stable name.
    pub fn name(self) -> &'static str {
        match self {
            MimoSineCheckCase::SlowSweep => "slow_sweep",
            MimoSineCheckCase::FastSweep => "fast_sweep",
            MimoSineCheckCase::CloseModes => "close_modes",
            MimoSineCheckCase::NearSingular => "near_singular",
            MimoSineCheckCase::MeasurementNoise => "measurement_noise",
            MimoSineCheckCase::LevelLadder => "level_ladder",
            MimoSineCheckCase::LimitChannel => "limit_channel",
            MimoSineCheckCase::TransportDelay => "transport_delay",
            MimoSineCheckCase::MoreControlsThanDrives => "more_controls_than_drives",
            MimoSineCheckCase::MoreDrivesThanControls => "more_drives_than_controls",
        }
    }
}

/// The check cannot run in this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckNotBuilt;

impl fmt::Display for CheckNotBuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the MIMO Sine law check is not built in this version; no case was run")
    }
}

impl std::error::Error for CheckNotBuilt {}

/// Run the check of a law: not built in this version.
pub fn run_check() -> Result<(), CheckNotBuilt> {
    Err(CheckNotBuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_check_says_it_is_not_built() {
        assert_eq!(run_check(), Err(CheckNotBuilt));
        assert!(CheckNotBuilt.to_string().contains("not built"));
        let names: Vec<&str> = MimoSineCheckCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 10);
        assert!(names.contains(&"near_singular"));
    }
}
