//! The SRS shock template's law as a law file: the template's own `law.rs`,
//! exported across the law interface as the template's `lib.rs` exports it.

#[path = "../../../../hertzforge-law-sdk/template/shock-srs/src/law.rs"]
mod law;

hertzforge_law_sdk::export_srs_law!(law::PlainRatio);
