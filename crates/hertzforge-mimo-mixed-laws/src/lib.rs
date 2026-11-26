//! The laws of a MIMO Mixed Mode run.
//!
//! A run takes two laws: a multiple-output Random law for the floor (world
//! `mimo`) and a MIMO Sine law for the tones (world `sine_mimo`, one instance
//! per tone). Neither role has a world of its own. [`role_world`] names each
//! role's world; [`may_arm_in_role`] is the hardware rule of a role (its own
//! world's check and the MIMO Mixed check in that role); and
//! [`MimoMixedCheckCase`] lists the cases the composed run is held to on the
//! simulator's existing structures ([`run_mimo_mixed_check_case`]).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod record;

pub use check::{
    mimo_mixed_check_config, mimo_mixed_check_run_s, mimo_mixed_check_station,
    mimo_mixed_check_table, run_mimo_mixed_check_case, run_mimo_mixed_check_case_with,
    MimoMixedCheckCase, MimoMixedCheckClocks, MimoMixedCheckStation, MIMO_MIXED_CHECK_DELAY_FRAMES,
    MIMO_MIXED_CHECK_FS_HZ,
};
pub use hertzforge_mimo_mixed::service::LawRole;
pub use record::{
    may_arm_in_role, mimo_mixed_check_name, mimo_mixed_check_record, mimo_mixed_check_status,
    own_check_version, record_mimo_mixed_check, role_record_clears, role_world, RoleRefusal,
    MIMO_MIXED_CHECK_VERSION,
};
