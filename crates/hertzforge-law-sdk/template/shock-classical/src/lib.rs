//! A classical shock control law: the frequency-domain correction written out
//! plainly, as the worked example a law author starts from. The law itself is
//! in `law.rs`; this file only exports it across the law interface.

pub mod law;

hertzforge_law_sdk::export_classical_law!(law::Correction);
