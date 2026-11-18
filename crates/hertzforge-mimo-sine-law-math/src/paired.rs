//! `paired`: the simplest law, and the template a contributor copies.
//!
//! Each control is paired with one drive. Every update, that drive's amplitude
//! moves toward the ratio of the wanted to the measured amplitude at its
//! control, at the loop-speed rule's rate (in the log, `|D| <- |D| (|R| /
//! |C|)^g`); its phase is left where it is. Nothing of the structure's
//! coupling is used, so on a coupled structure the pairs fight: that is what
//! the law is there to show.
//!
//! **The first drive** is the drive this loop would settle to, found on the
//! identification before anything is played: each pair's drive starts at its
//! control's reference over the pair's own path, and the update is run on the
//! identification's response at the tone (`C = H D`, at a gain of one half)
//! until every control is within 0.01 dB or [`FIRST_DRIVE_ROUNDS`] rounds have
//! run. Only the pairs' own rule runs; nothing of the coupling is inverted.
//! If the rounds leave a control further off than the plain start did, the
//! plain start is kept. The pairs do not correct while the controller's
//! settle window holds them (`hold` `engage`), so without this a coupled
//! structure's cross paths are what the first judged updates see (on the
//! check's coupled 4 by 4 at 96 Hz the plain start reads 5.4 dB low).
//!
//! A new law needs four pieces, all visible here: its parameters, its state,
//! a first drive, and an update that turns one tick's measurements into the
//! next drive.

use crate::law::{aimed_reference, worst_error_db, Hold, Inputs, Outputs, SineLaw};
use crate::setup::{GroupDelay, Setup, TrackingRule};
use crate::speed::{gain, GainRule};
use crate::{TestTypes, C64};

/// The diagnostics `paired` declares, in order.
pub const PAIRED_VALUES: [&str; 2] = ["gain", "error_db"];

/// The most rounds of the pairs' own update run on the identification to find
/// the first drive.
pub const FIRST_DRIVE_ROUNDS: usize = 200;

/// The test types `paired` accepts: a dwell and a stepped sine, on a structure
/// whose pairs are weakly coupled. It has no model of the coupling, and on the
/// research testbed's coupled linear 4 by 4 it completed none (0 of 5 dwells,
/// 0 of 5 stepped sines); that is what the template is there to show.
pub const PAIRED_TEST_TYPES: TestTypes = TestTypes {
    sweep: false,
    dwell: true,
    stepped: true,
};

/// Parameters of `paired`.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedParams {
    /// The drive paired with each control (every drive used once at most).
    pub drive_of: Vec<usize>,
    /// The gain rule.
    pub gain_rule: GainRule,
}

impl PairedParams {
    /// Control `c` with drive `c`, for the first `min(controls, drives)` controls.
    pub fn diagonal(controls: usize, drives: usize) -> PairedParams {
        PairedParams {
            drive_of: (0..controls.min(drives)).collect(),
            gain_rule: GainRule::default(),
        }
    }
}

/// The `paired` law's state.
#[derive(Clone, Debug)]
pub struct PairedLaw {
    params: PairedParams,
    setup: Setup,
    delay: GroupDelay,
    tracking: TrackingRule,
}

impl PairedLaw {
    /// Build the law.
    pub fn new(setup: &Setup, params: PairedParams) -> Result<PairedLaw, String> {
        setup.check()?;
        let mut used = vec![false; setup.drives];
        if params.drive_of.len() > setup.controls {
            return Err("more pairs than controls".into());
        }
        for &d in &params.drive_of {
            if d >= setup.drives || used[d] {
                return Err("each pair needs its own drive".into());
            }
            used[d] = true;
        }
        Ok(PairedLaw {
            delay: GroupDelay::of_with(&setup.identification, params.gain_rule.delay_estimate()),
            tracking: setup.tracking,
            setup: setup.clone(),
            params,
        })
    }
}

impl SineLaw for PairedLaw {
    /// The first drive: each pair's own path inverted in amplitude.
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        self.setup.response_at(x.f_hz, &mut h);
        out.drive.iter_mut().for_each(|v| *v = C64::new(0.0, 0.0));
        let r = aimed_reference(x);
        for (c, &d) in self.params.drive_of.iter().enumerate() {
            let path = h[c * m + d].norm().max(1e-300);
            out.drive[d] = C64::new(r[c].norm() / path, 0.0);
        }
        // Run the pairs' own update on the identification (see the module's
        // documentation).
        let pairs = &self.params.drive_of;
        let worst = |d: &[C64]| -> f64 {
            pairs
                .iter()
                .enumerate()
                .map(|(c, _)| {
                    let y = (0..m).fold(C64::new(0.0, 0.0), |a, j| a + h[c * m + j] * d[j]);
                    (20.0 * (y.norm().max(1e-300) / r[c].norm().max(1e-300)).log10()).abs()
                })
                .fold(0.0, f64::max)
        };
        let plain = out.drive.clone();
        let mut d = plain.clone();
        for _ in 0..FIRST_DRIVE_ROUNDS {
            if worst(&d) < 0.01 {
                break;
            }
            let next: Vec<C64> = {
                let mut v = d.clone();
                for (c, &j) in pairs.iter().enumerate() {
                    let y = (0..m).fold(C64::new(0.0, 0.0), |a, k| a + h[c * m + k] * d[k]);
                    if y.norm() > 1e-300 {
                        v[j] = d[j] * (r[c].norm() / y.norm()).sqrt();
                    }
                }
                v
            };
            d = next;
        }
        if d.iter().all(|v| v.is_finite()) && worst(&d) < worst(&plain) {
            out.drive.copy_from_slice(&d);
        }
        out.values.clear();
        out.values.extend_from_slice(&[0.0, 0.0]);
        Ok(())
    }

    /// Each paired drive's amplitude moved by the ratio raised to the gain.
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let gn = gain(
            self.params.gain_rule,
            &self.tracking,
            self.setup.loop_delay,
            &self.delay,
            self.setup.tick_s,
            x.f_hz,
        );
        let g = if x.hold == Hold::Engage { 0.0 } else { gn.g };
        out.drive.iter_mut().for_each(|v| *v = C64::new(0.0, 0.0));
        let r = aimed_reference(x);
        for (c, &d) in self.params.drive_of.iter().enumerate() {
            let now = x.drive_in_force[d];
            let measured = x.controls[c].norm();
            let ratio = if measured > 1e-300 {
                r[c].norm() / measured
            } else {
                1.0
            };
            out.drive[d] = now * ratio.powf(g);
        }
        out.values.clear();
        out.values
            .extend_from_slice(&[g, worst_error_db(x.controls, x.reference)]);
        Ok(())
    }

    fn value_names(&self) -> &'static [&'static str] {
        &PAIRED_VALUES
    }
}
