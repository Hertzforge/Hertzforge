//! UFF (Universal File Format) export - dataset 58 scaffold.
//!
//! UFF, also called the Universal File Format or "unv" format, is the de-facto
//! interchange format for structural-dynamics data (SDRL/IDEAS lineage; used by
//! LMS Test.Lab, Siemens, ME'scope, Photon, X-Modal, and most modal test tools).
//! Dataset **58** carries a single measured or computed **function** (a time history,
//! a spectrum, or an FRF) together with its channel/DOF identification header. A UFF
//! file is a sequence of datasets, each delimited by a line of `    -1` before and
//! after the dataset body, with the 4-character dataset number (`    58`) as the first
//! record of the body.
//!
//! # Why only a stub here
//!
//! This crate owns channel IDENTITY, not measured data. A complete dataset-58 writer
//! needs the actual sampled function per channel (record 6/7 abscissa spacing, the
//! ordinate data type, and the ordinate values themselves), which live in the capture
//! and DSP layers, not in the channel table. So this module scaffolds the entry point
//! and emits the channel-identification portion of the 58 header (records 1-5 and the
//! DOF fields of record 6) with the data records stubbed. The full binary/ASCII
//! ordinate layout is left as a documented TODO for the export crate to complete once
//! it holds the sampled functions.
//!
//! # TODO (full dataset-58 layout, ASCII form)
//!
//! Per-dataset record layout (fixed-column ASCII, one function per dataset):
//! - Record 1: ID line 1 (function description, 80 chars).
//! - Record 2: ID line 2. Record 3: ID line 3 (date/time). Records 4-5: ID lines 4-5.
//! - Record 6: DOF identification - function type (1=time, 2=auto-spectrum,
//!   4=cross-spectrum, 12=FRF, ...), function id, version, load case, plus the
//!   RESPONSE entity name/node/direction and the REFERENCE entity name/node/direction.
//!   The direction codes are 1..6 = +X/+Y/+Z, negatives for -X/-Y/-Z. This is where a
//!   channel's role (response vs. reference) and description map in.
//! - Record 7: data form - ordinate data type (2=real single, 4=real double,
//!   5=complex single, 6=complex double), number of data values, abscissa spacing
//!   (0=uneven, 1=even), abscissa minimum, abscissa increment, z-axis value.
//! - Records 8-11: abscissa/ordinate/z-axis data characteristics (specific data type,
//!   length/force/temperature exponents, axis labels and units - e.g. "Frequency"/"Hz"
//!   on the abscissa, "Acceleration"/"g" on the ordinate).
//! - Record 12+: the ordinate data itself. Even-abscissa real data: 6 values per line,
//!   `%13.5e`. Uneven or complex data interleaves abscissa/real/imag per the chosen
//!   ordinate data type and values-per-line count.
//!
//! Binary form (dataset 58b): identical records 1-11 in ASCII, then a binary header
//! line encoding byte ordering / floating-point format / record counts, then the raw
//! little- or big-endian ordinate bytes. Implement ASCII first; add 58b behind a flag.
//!
//! References to encode against when implementing: the Structural Dynamics Research Lab
//! UFF dataset 58 specification (widely mirrored, e.g. the `pyuff` project's dataset_58
//! docstring and the Siemens/SDRL "Universal File Formats for Modal Analysis Testing").

use crate::{ChannelRole, ChannelTable};

/// A single UFF dataset delimiter line. Datasets open and close with this exact token.
const DELIM: &str = "    -1";
/// Dataset number for a function record.
const DATASET_58: &str = "    58";

/// Placeholder to make the incompleteness impossible to miss in emitted files and in
/// any downstream diff. The full ordinate records are not yet written.
const TODO_MARKER: &str = "TODO: dataset-58 ordinate records not yet implemented";

impl ChannelTable {
    /// Export a UFF (`.unv`) skeleton: one dataset-58 block per ENABLED input channel,
    /// carrying only the channel-identification header (ID lines + a partial DOF record
    /// derived from the channel's index, description, role, and units). The ordinate
    /// data records are intentionally omitted and flagged with a TODO marker line.
    ///
    /// This is a STUB. It exists so the export entry point and file structure are in
    /// place and testable; it is not a spec-complete dataset-58 writer. Drive-role
    /// channels are skipped (they are outputs, not measured functions). See the module
    /// docs for the full record layout that the export crate must fill in.
    pub fn to_uff58_stub(&self) -> String {
        let mut out = String::new();
        for c in &self.channels {
            if !c.enabled || c.role == ChannelRole::Drive {
                continue;
            }
            let response_dir = 3; // +Z by convention until a real DOF map exists
            let is_reference = c.role == ChannelRole::Reference;
            let func_type = 1; // 1 = time response (placeholder default)

            out.push_str(DELIM);
            out.push('\n');
            out.push_str(DATASET_58);
            out.push('\n');
            // Record 1-5: ID lines (identification only; safe to fill from identity).
            let desc = if c.description.is_empty() {
                format!("Channel {}", c.index)
            } else {
                c.description.clone()
            };
            out.push_str(&format!("{desc}\n"));
            out.push_str("NONE\n"); // ID line 2
            out.push_str("NONE\n"); // ID line 3 (date/time TODO)
            out.push_str("NONE\n"); // ID line 4
            out.push_str("NONE\n"); // ID line 5
                                    // Record 6 (partial): function type + response/reference DOF identity.
            out.push_str(&format!(
                "{func_type:>10}{idx:>10}  RESP dir {response_dir}  {refflag}  units={units}\n",
                idx = c.index,
                refflag = if is_reference { "REF" } else { "RESP-ONLY" },
                units = super::units_str(&c.units),
            ));
            // Records 7-11 + ordinate data: not yet implemented.
            out.push_str(TODO_MARKER);
            out.push('\n');
            out.push_str(DELIM);
            out.push('\n');
        }
        out
    }
}
