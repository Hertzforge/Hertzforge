//! A multiple-output Random control law: the open-loop pseudo-inverse, written
//! out as the worked example a law author starts from. The law itself is in
//! `law.rs`; this file only exports it across the law interface.

pub mod law;

hertzforge_law_sdk::export_mimo_law!(law::OpenLoop);
