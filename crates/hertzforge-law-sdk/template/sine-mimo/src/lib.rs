//! A MIMO Sine control law: each control paired with one drive, the drive's
//! amplitude following its control's ratio of wanted to measured amplitude,
//! as the worked example a law author starts from. The law itself is in
//! `law.rs`; this file only exports it across the law interface.

pub mod law;

hertzforge_law_sdk::export_sine_mimo_law!(law::PairRatio);
