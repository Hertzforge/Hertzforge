//! A built-in multiple-output Random law, the pseudo-inverse with closed-loop
//! correction (the default), as a law file. It is the controller's own
//! [`PseudoInverseLaw`]: this crate only exports it across the law interface,
//! with the manifest built from the same declaration as the controller's
//! catalog entry.

use hertzforge_law_sdk::export_mimo_law;
use hertzforge_mimo_laws::law_file::LawFile;
use hertzforge_mimo_laws::PseudoInverseLaw;

export_mimo_law!(LawFile<PseudoInverseLaw>);
