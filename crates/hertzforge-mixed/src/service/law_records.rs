//! The run's law records: one fixed-size record per law call (the floor law's
//! per loop, each tone's per tick), written to `law_ticks.bin` beside a
//! capture with `law.json` describing the layout, as Sine's are, with the role
//! and the tone's index added.

/// The file the records are written to, in the capture's folder.
pub const LAW_TICKS_FILE: &str = "law_ticks.bin";
/// The file that describes them.
pub const LAW_JSON_FILE: &str = "law.json";
/// The records' format name, in `law.json`.
pub const LAW_TICKS_FORMAT: &str = "hertzforge.mixed.law_ticks.v1";

/// Which law a record is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordRole {
    /// The floor law (one call per loop).
    Floor = 0,
    /// A tone's law instance (one call per control tick).
    Tone = 1,
}

/// One law call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixedLawRecord {
    /// Station time, seconds since engage.
    pub t_s: f64,
    /// The drive in force before the call: the floor's in-band drive rms
    /// (volts) or the tone's amplitude (volts peak).
    pub drive_in: f64,
    /// The drive the core took from the call, same unit.
    pub drive_out: f64,
    /// The call's time, microseconds (0 untimed).
    pub took_us: f32,
    /// Which law.
    pub role: RecordRole,
    /// The tone's index among the tones (0 for the floor).
    pub tone: u8,
    /// 0 for a usable call, else the miss's code (the index into the role's
    /// miss names, plus one).
    pub miss: u8,
    /// Bit 0: a core cap bound. Bit 1: the call was the engage (or the floor
    /// law's seed).
    pub flags: u8,
}

/// The fields of a record in file order, with their types, for `law.json`.
pub const LAW_TICK_FIELDS: [(&str, &str); 8] = [
    ("t_s", "f64"),
    ("drive_in", "f64"),
    ("drive_out", "f64"),
    ("took_us", "f32"),
    ("role", "u8"),
    ("tone", "u8"),
    ("miss", "u8"),
    ("flags", "u8"),
];

impl MixedLawRecord {
    /// Bytes per record.
    pub const BYTES: usize = 32;

    /// Append the record, little-endian, in [`LAW_TICK_FIELDS`] order.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.t_s.to_le_bytes());
        out.extend_from_slice(&self.drive_in.to_le_bytes());
        out.extend_from_slice(&self.drive_out.to_le_bytes());
        out.extend_from_slice(&self.took_us.to_le_bytes());
        out.extend_from_slice(&[self.role as u8, self.tone, self.miss, self.flags]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_its_stated_size_in_field_order() {
        let r = MixedLawRecord {
            t_s: 1.5,
            drive_in: 0.25,
            drive_out: 0.5,
            took_us: 12.0,
            role: RecordRole::Tone,
            tone: 3,
            miss: 0,
            flags: 1,
        };
        let mut b = Vec::new();
        r.write_to(&mut b);
        assert_eq!(b.len(), MixedLawRecord::BYTES);
        let sizes: usize = LAW_TICK_FIELDS
            .iter()
            .map(|(_, t)| match *t {
                "f64" => 8,
                "f32" => 4,
                _ => 1,
            })
            .sum();
        assert_eq!(sizes, MixedLawRecord::BYTES);
        assert_eq!(f64::from_le_bytes(b[0..8].try_into().unwrap()), 1.5);
        assert_eq!(&b[28..32], &[1, 3, 0, 1]);
    }
}
