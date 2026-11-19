//! The MIMO Sine template's law as a law file: the template's own `law.rs`,
//! exported across the law interface as the template's `lib.rs` exports it.

#[path = "../../../../hertzforge-law-sdk/template/sine-mimo/src/law.rs"]
mod law;

hertzforge_law_sdk::export_sine_mimo_law!(law::PairRatio);
