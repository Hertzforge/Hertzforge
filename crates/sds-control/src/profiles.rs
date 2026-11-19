//! Demo random profiles, defined in code by breakpoints, each paired with the Sim
//! plant it runs against.
//!
//! The plant is the controller's own simulated shaker and fixture
//! (`hertzforge_hal_sim`): a small bank of single-degree-of-freedom resonators plus a
//! direct feedthrough path and a Gaussian noise floor. Nothing here describes real
//! hardware, and every run built from these profiles is reported as `source: "SIM"`.

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use serde_json::{json, Value};

/// The Sim plant parameters a profile runs against.
#[derive(Debug, Clone, PartialEq)]
pub struct PlantSpec {
    /// Resonances as `(natural frequency Hz, quality factor Q)`.
    pub modes: Vec<(f64, f64)>,
    /// Additive Gaussian noise floor (standard deviation, response units).
    pub noise_level: f64,
    /// Direct drive-to-response gain added to the resonator bank.
    pub feedthrough: f64,
    /// Plant noise seed (runs are reproducible).
    pub seed: u64,
}

impl PlantSpec {
    /// The controller's validated demo plant: two well-damped modes and healthy
    /// feedthrough (the plant its acceptance tests run against).
    pub fn demo() -> PlantSpec {
        PlantSpec {
            modes: vec![(200.0, 2.5), (550.0, 2.5)],
            noise_level: 2e-4,
            feedthrough: 0.35,
            seed: 0x5A5A_1234,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kind": "sdof_bank",
            "modes": self.modes.iter().map(|&(f, q)| json!({"frequency_hz": f, "q": q})).collect::<Vec<_>>(),
            "feedthrough": self.feedthrough,
            "noise_level": self.noise_level,
        })
    }
}

/// One demo profile.
#[derive(Debug, Clone)]
pub struct Profile {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub breakpoints: BreakpointTable,
    pub plant: PlantSpec,
    /// Alarm band `(minus dB, plus dB)` around the reference.
    pub alarm_db: (f64, f64),
    /// Abort band `(minus dB, plus dB)` around the reference.
    pub abort_db: (f64, f64),
    /// Drive ceiling, Vrms (the controller clamps the drive there).
    pub drive_ceiling_vrms: f64,
}

impl Profile {
    /// Low and high frequency of the breakpoint table.
    pub fn band_hz(&self) -> (f64, f64) {
        let rows = &self.breakpoints.rows;
        (
            rows.first().map_or(0.0, |r| r.freq_hz),
            rows.last().map_or(0.0, |r| r.freq_hz),
        )
    }

    /// Overall level of the profile at full level (0 dB), g rms.
    pub fn grms(&self) -> f64 {
        self.breakpoints.grms_of_table().unwrap_or(f64::NAN)
    }

    /// The `sim.profiles` entry: the contract's `{id, title, summary}` plus the band,
    /// the overall level, and the plant it runs against.
    pub fn describe(&self) -> Value {
        let (lo, hi) = self.band_hz();
        let grms = self.grms();
        json!({
            "id": self.id,
            "title": self.title,
            "summary": self.summary,
            "band_hz": [lo, hi],
            "grms": (grms * 1000.0).round() / 1000.0,
            "alarm_db": [self.alarm_db.0, self.alarm_db.1],
            "abort_db": [self.abort_db.0, self.abort_db.1],
            "plant": self.plant.to_json(),
        })
    }
}

fn table(label: &str, rows: &[BreakpointRow]) -> BreakpointTable {
    let mut t = BreakpointTable::new(label);
    for r in rows {
        t.push(*r);
    }
    t
}

/// All demo profiles, in display order.
pub fn all() -> Vec<Profile> {
    vec![
        Profile {
            id: "ramp_plateau",
            title: "Ramp and Plateau, 20 to 2000 Hz",
            summary: "+3 dB/oct from 20 to 80 Hz, flat 0.04 g²/Hz to 350 Hz, -3 dB/oct to 2000 Hz \
                      (about 6.1 g rms), on the two-mode demo plant.",
            breakpoints: table(
                "ramp_plateau",
                &[
                    BreakpointRow::psd(20.0, 0.01),
                    BreakpointRow::slope(80.0, 3.0),
                    BreakpointRow::psd(350.0, 0.04),
                    BreakpointRow::slope(2000.0, -3.0),
                ],
            ),
            plant: PlantSpec::demo(),
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            drive_ceiling_vrms: 20.0,
        },
        Profile {
            id: "narrowband_peak",
            title: "Broadband With a Narrowband Peak",
            summary: "Flat 0.005 g²/Hz from 20 to 2000 Hz with a 10 dB narrowband plateau at \
                      480 to 520 Hz, on the two-mode demo plant.",
            breakpoints: table(
                "narrowband_peak",
                &[
                    BreakpointRow::psd(20.0, 0.005),
                    BreakpointRow::psd(420.0, 0.005),
                    BreakpointRow::psd(480.0, 0.05),
                    BreakpointRow::psd(520.0, 0.05),
                    BreakpointRow::psd(580.0, 0.005),
                    BreakpointRow::psd(2000.0, 0.005),
                ],
            ),
            plant: PlantSpec::demo(),
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            drive_ceiling_vrms: 20.0,
        },
        Profile {
            id: "flat_fixture_mode",
            title: "Flat 20 to 2000 Hz With a Fixture Mode",
            summary: "Flat 0.01 g²/Hz from 20 to 2000 Hz on a plant with a third, less damped \
                      fixture mode at 800 Hz (Q 5) beside the two demo modes.",
            breakpoints: table(
                "flat_fixture_mode",
                &[
                    BreakpointRow::psd(20.0, 0.01),
                    BreakpointRow::psd(2000.0, 0.01),
                ],
            ),
            plant: PlantSpec {
                modes: vec![(200.0, 2.5), (550.0, 2.5), (800.0, 5.0)],
                ..PlantSpec::demo()
            },
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
            drive_ceiling_vrms: 20.0,
        },
    ]
}

/// The profile with `id`.
pub fn find(id: &str) -> Option<Profile> {
    all().into_iter().find(|p| p.id == id)
}

/// The known profile ids, for error messages.
pub fn ids() -> Vec<&'static str> {
    all().iter().map(|p| p.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_resolves_and_has_energy() {
        for p in all() {
            p.breakpoints.resolve().expect("breakpoints resolve");
            let g = p.grms();
            assert!(g.is_finite() && g > 0.5, "{}: {g} g rms", p.id);
            let (lo, hi) = p.band_hz();
            assert!(lo >= 20.0 && hi <= 2000.0 && lo < hi);
        }
    }

    #[test]
    fn the_ramp_profile_is_the_classic_six_g_shape() {
        let g = find("ramp_plateau").unwrap().grms();
        assert!((g - 6.06).abs() < 0.15, "{g}");
    }
}
