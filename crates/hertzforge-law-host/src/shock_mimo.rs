//! MIMO Shock law files: the `shock-mimo` world (`wit/control-law/shock-mimo.wit`).
//!
//! The host recognizes a law file of this world, so loading one answers by name,
//! but it does not yet run one: [`crate::LawHost::load_any`] refuses it with
//! [`crate::LawError::NotALaw`] and [`SHOCK_MIMO_NOT_RUN`].

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "shock-mimo",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

pub(crate) use bindings::ShockMimoPre;

/// What [`crate::LawHost::load_any`] answers for a law file of the `shock-mimo`
/// world.
pub const SHOCK_MIMO_NOT_RUN: &str =
    "it is a MIMO Shock law (the shock-mimo world), and this version of the controller \
     cannot run MIMO Shock law files";
