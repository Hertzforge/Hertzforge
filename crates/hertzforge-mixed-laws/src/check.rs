//! The cases the composed Mixed Mode core is held to, each on a simulated
//! station, with the built-in laws in both roles (a law file in one role is
//! checked with the built-in in the other).

use hertzforge_mixed::MixedLawFactories;
use hertzforge_random_laws::CheckCase;
use std::fmt;

/// One case of the Mixed Mode check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MixedCheckCase {
    /// A floor alone: parity with a Random run on the same plant.
    FloorOnly,
    /// One dwelling tone on the floor.
    FixedToneOnFloor,
    /// A tone swept through the plant's mode on the floor.
    SweptToneThroughMode,
    /// Two tones at the closest spacing allowed.
    TwoTonesClose,
    /// A narrowband dwelling over the floor.
    NarrowbandFixed,
    /// A narrowband swept over the floor.
    NarrowbandSwept,
    /// Every part moved through a level ladder.
    LevelLadder,
    /// A flat wire behind a hardware station's lag.
    TransportDelay,
}

impl MixedCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MixedCheckCase; 8] = [
        MixedCheckCase::FloorOnly,
        MixedCheckCase::FixedToneOnFloor,
        MixedCheckCase::SweptToneThroughMode,
        MixedCheckCase::TwoTonesClose,
        MixedCheckCase::NarrowbandFixed,
        MixedCheckCase::NarrowbandSwept,
        MixedCheckCase::LevelLadder,
        MixedCheckCase::TransportDelay,
    ];

    /// The stable record and wire name.
    pub fn name(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => "floor_only",
            MixedCheckCase::FixedToneOnFloor => "fixed_tone_on_floor",
            MixedCheckCase::SweptToneThroughMode => "swept_tone_through_mode",
            MixedCheckCase::TwoTonesClose => "two_tones_close",
            MixedCheckCase::NarrowbandFixed => "narrowband_fixed",
            MixedCheckCase::NarrowbandSwept => "narrowband_swept",
            MixedCheckCase::LevelLadder => "level_ladder",
            MixedCheckCase::TransportDelay => "transport_delay",
        }
    }

    /// The case with `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The simulated article or condition, in words.
    pub fn plant(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => "one mode on a flat floor, no tones or bands",
            MixedCheckCase::FixedToneOnFloor => "one mode; a tone dwelling below it",
            MixedCheckCase::SweptToneThroughMode => "one mode; a tone swept through it",
            MixedCheckCase::TwoTonesClose => {
                "one mode; two dwelling tones two tracking widths apart"
            }
            MixedCheckCase::NarrowbandFixed => "one mode; a band dwelling away from it",
            MixedCheckCase::NarrowbandSwept => "one mode; a band swept through it",
            MixedCheckCase::LevelLadder => "one mode; a tone and a band through the level ladder",
            MixedCheckCase::TransportDelay => {
                "a flat wire behind a hardware station's lag; a tone and a band"
            }
        }
    }

    /// What the case asks of the laws, in words.
    pub fn requirement(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => {
                "the floor's control matches a Random run on the same plant within its alarm band"
            }
            MixedCheckCase::FixedToneOnFloor => {
                "the tone within its alarm band and the floor's clean lines within theirs at full level"
            }
            MixedCheckCase::SweptToneThroughMode => {
                "the tone within its alarm band through the mode; the floor within its alarm band"
            }
            MixedCheckCase::TwoTonesClose => {
                "each tone within its alarm band; neither tone's reading carries the other"
            }
            MixedCheckCase::NarrowbandFixed => {
                "the band's level within its alarm band widened to its scatter; the floor's clean lines within theirs"
            }
            MixedCheckCase::NarrowbandSwept => {
                "the band's moving-frame level within its widened alarm band through the mode"
            }
            MixedCheckCase::LevelLadder => {
                "at each level every part within its alarm band after its settle time"
            }
            MixedCheckCase::TransportDelay => {
                "the run completes and every part settles within its alarm band"
            }
        }
    }
}

/// Why a check case could not be run.
#[derive(Debug, Clone, PartialEq)]
pub enum MixedCheckError {
    /// The composed core cannot run a check yet.
    NotBuilt,
    /// The case's run failed; the core's words.
    Run(String),
}

impl fmt::Display for MixedCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixedCheckError::NotBuilt => f.write_str("the Mixed Mode check cannot run yet"),
            MixedCheckError::Run(s) => write!(f, "the check case failed to run: {s}"),
        }
    }
}

impl std::error::Error for MixedCheckError {}

/// Run `case` with `laws` on its simulated station at `fs_hz` and judge it.
pub fn run_mixed_check_case(
    case: MixedCheckCase,
    laws: &MixedLawFactories,
    fs_hz: f64,
) -> Result<CheckCase, MixedCheckError> {
    let _ = (case, laws, fs_hz);
    Err(MixedCheckError::NotBuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_cases_with_unique_names_that_read_back() {
        let mut names: Vec<_> = MixedCheckCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 8);
        for c in MixedCheckCase::ALL {
            assert_eq!(MixedCheckCase::from_name(c.name()), Some(c));
            assert!(!c.plant().is_empty() && !c.requirement().is_empty());
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8);
        assert_eq!(MixedCheckCase::from_name("nope"), None);
    }

    #[test]
    fn the_runner_refuses_until_built() {
        assert_eq!(
            run_mixed_check_case(
                MixedCheckCase::FloorOnly,
                &MixedLawFactories::builtin(),
                8192.0
            )
            .unwrap_err(),
            MixedCheckError::NotBuilt
        );
    }
}
