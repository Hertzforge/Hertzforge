//! Brandon Zwink's band-average multiple-output Random law, as a law file.

use hertzforge_law_sdk::export_mimo_law;
use hertzforge_mimo_laws::law_file::LawFile;
use hertzforge_mimo_laws::BandAverageLaw;

export_mimo_law!(LawFile<BandAverageLaw>);
