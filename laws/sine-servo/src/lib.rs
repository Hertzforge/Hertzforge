//! The built-in Sine law, `servo_blend`, as a law file. It is the
//! controller's own `ServoBlendLaw`: this crate only carries it across the law
//! interface (the SDK's `sine::native::ServoBlendFile`), with the manifest
//! built from the same declaration (`hertzforge_sine::law::servo_blend`) as
//! the controller's catalog entry.

// The declaration the manifest is built from, named here so the dependency
// says what this file is.
pub use hertzforge_sine::law::servo_blend::ID;

hertzforge_law_sdk::export_sine_law!(hertzforge_law_sdk::sine::native::ServoBlendFile);
