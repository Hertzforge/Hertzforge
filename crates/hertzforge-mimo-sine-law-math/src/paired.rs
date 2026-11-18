//! `paired`: the simplest law, and the template a contributor copies.
//!
//! Each control is paired with one drive. Every update, that drive's amplitude
//! moves toward the ratio of the wanted to the measured amplitude at its
//! control, at the loop-speed rule's rate (in the log, `|D| <- |D| (|R| /
//! |C|)^g`); its phase is left where it is. Nothing of the structure's
//! coupling is used, so on a coupled structure the pairs fight: that is what
//! the law is there to show.
//!
//! A new law needs four pieces, all visible here: its parameters, its state,
//! a first drive, and an update that turns one tick's measurements into the
//! next drive.

use crate::law::{worst_error_db, Hold, Inputs, Outputs, SineLaw};
use crate::setup::{GroupDelay, Setup, TrackingRule};
use crate::speed::{gain, GainRule};
use crate::{TestTypes, C64};

/// The diagnostics `paired` declares, in order.
pub const PAIRED_VALUES: [&str; 2] = ["gain", "error_db"];

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
            delay: GroupDelay::of(&setup.identification),
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
        for (c, &d) in self.params.drive_of.iter().enumerate() {
            let path = h[c * m + d].norm().max(1e-300);
            out.drive[d] = C64::new(x.reference[c].norm() / path, 0.0);
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
            &self.delay,
            self.setup.tick_s,
            x.f_hz,
        );
        let g = if x.hold == Hold::Engage { 0.0 } else { gn.g };
        out.drive.iter_mut().for_each(|v| *v = C64::new(0.0, 0.0));
        for (c, &d) in self.params.drive_of.iter().enumerate() {
            let now = x.drive_in_force[d];
            let measured = x.controls[c].norm();
            let ratio = if measured > 1e-300 {
                x.reference[c].norm() / measured
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
