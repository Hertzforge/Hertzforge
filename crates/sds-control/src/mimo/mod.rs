//! MIMO Random through the protocol (schema/PROTOCOL.md, "MIMO Random"): the setup
//! and its validation, the driver trait the service runs a MIMO test through, the
//! status and report objects, and the display quantities.
//!
//! - [`setup`]: the setup on the wire, parsed and checked against the channel table
//!   and the law catalog.
//! - [`check`]: the validation report (ranks, positive semi-definiteness,
//!   feasibility per drive, law shape admission).
//! - [`driver`]: [`driver::MimoRunDriver`], what the service needs from a run.
//! - [`view`]: the status, prediction and report objects.

pub mod check;
pub mod driver;
pub mod setup;
pub mod view;
