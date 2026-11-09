//! An SRS shock control law: the per-band ratio update written out plainly,
//! as the worked example a law author starts from. The law itself is in
//! `law.rs`; this file only exports it across the law interface.

pub mod law;

hertzforge_law_sdk::export_srs_law!(law::PlainRatio);
