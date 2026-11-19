//! The TWR template's law as a law file: the template's own `law.rs`, exported
//! across the law interface as the template's `lib.rs` exports it.

#[path = "../../../../hertzforge-law-sdk/template/twr/src/law.rs"]
mod law;

hertzforge_law_sdk::export_twr_law!(law::Correction);
