//! `smart` - Hertzforge SMART module crate (stub, DELIBERATELY EMPTY).
//!
//! # Why this crate is still a stub
//!
//! The SMART command + run layer lives HAL-side, in the delegated backend's run layer
//! (`DelegatedSmartRun` / `SmartRunConfig`, private to the backend), exactly as the
//! Shock module did. Filling THIS crate (the `cdylib` module built
//! against the `hertzforge-module-abi` C ABI) is GATED on the delegated-module-ABI
//! decision, which is flagged-but-not-solved:
//!
//! * See the delegated backend's ABI note for the full design gap: a delegated
//!   module (SMART / Shock / Random on a delegated controller) never fills a per-tick
//!   `drive_ptr`, so it does not fit the host-closes-loop `TickCtx` vtable. It needs a
//!   sibling `DelegatedVTable` + a `LoopKind::Delegated` arm + a tagged level-command
//!   type (absolute-setpoint vs increment-nudge).
//! * The recommendation is concrete-slice-first: design the delegated ABI variant only
//!   once BOTH `DelegatedShockRun` and `DelegatedSmartRun` exist as evidence, because
//!   the ABI is a frozen C boundary (`abi/hertzforge_module_abi.h` is CI-diffed) where a premature
//!   shape is expensive. With both delegated runs now working HAL-side, that evidence
//!   exists; the freeze is the integrator's call.
//!
//! The UI talks to the HAL-side run through the host, not through this module ABI, so
//! leaving this crate a stub does NOT block the SMART slice.

#![allow(dead_code)]
