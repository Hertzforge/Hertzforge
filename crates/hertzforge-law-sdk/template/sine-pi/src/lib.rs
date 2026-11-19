//! A single-output Sine control law: a proportional-integral loop on the
//! error in dB, as the worked example a law author starts from. The law
//! itself is in `law.rs`; this file only exports it across the law
//! interface.

pub mod law;

hertzforge_law_sdk::export_sine_law!(law::SinePi);
