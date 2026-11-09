//! The Sine template's law as a law file: the template's own `law.rs`,
//! exported across the law interface as the template's `lib.rs` exports it.

#[path = "../../../../hertzforge-law-sdk/template/sine-pi/src/law.rs"]
mod law;

hertzforge_law_sdk::export_sine_law!(law::SinePi);
