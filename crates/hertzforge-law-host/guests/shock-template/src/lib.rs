//! The classical shock template's law as a law file: the template's own `law.rs`,
//! exported across the law interface as the template's `lib.rs` exports it.

#[path = "../../../../hertzforge-law-sdk/template/shock-classical/src/law.rs"]
mod law;

hertzforge_law_sdk::export_classical_law!(law::Correction);
