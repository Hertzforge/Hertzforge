//! The zero-drive multiple-output law as a law file. It is the controller's
//! own [`ZeroLaw`]: this crate only exports it across the law interface.

use hertzforge_law_sdk::export_mimo_law;
use hertzforge_mimo_laws::law_file::LawFile;
use hertzforge_mimo_laws::ZeroLaw;

export_mimo_law!(LawFile<ZeroLaw>);
