//! Binary display-frame packing on the daemon side.
//!
//! # This is a SECOND producer of a pinned layout - read this before touching it
//!
//! `ui/src-tauri/src/frame.rs` is the other one, and `ui/src/lib/ipc/frame.ts` is the
//! single decoder both feed. Two producers of one byte layout is a drift hazard of
//! exactly the class the rule "when a doc and a byte disagree, the byte wins" is
//! about, so it is fenced three ways:
//!
//! 1. The layout is written out below in the same table form as the other producer's,
//!    with the same offsets.
//! 2. [`tests::the_spectrum_header_is_the_pinned_layout_byte_for_byte`] pins every
//!    field at its offset against a hand-written expectation - not against this
//!    module's own constants, which would be circular.
//! 3. **A later phase should lift the packers into a shared crate** and delete this
//!    module. It was not done when this daemon was built because that phase's
//!    sanctioned extraction was `eigen-station` (station spec -> `StationBackend`), and
//!    widening a mechanical move into a second one mid-phase is how a "no behavior
//!    change" claim stops being checkable. The duplication is small (one packer),
//!    pinned, and recorded here so the later phase inherits a decision rather than a
//!    surprise.
//!
//! # Why the daemon packs per CONNECTION rather than once
//!
//! A subscription's selectors ([`eigen_protocol::StreamSub`]) differ per peer: the
//! operator's Mac wants 8 channels at 20 Hz, the observer on the bench wants 2. The
//! pump therefore publishes UNPACKED traces and each connection packs what it asked
//! for, which is what makes the selector producer-side (bytes never produced for a
//! channel nobody subscribed) instead of receiver-side (bytes produced, sent, and
//! thrown away). At the handful of connections this deployment has, packing twice is
//! cheaper than sending once too much.
//!
//! It also makes the observability `seq` correct: it counts frames on THIS stream to
//! THIS peer, so a gap means that peer lost a frame - which is the only reading that
//! lets a UI detect a lost frame at all.

use eigen_protocol::StreamSub;

/// Fixed header bytes of a single-channel `SpectrumFrame`: 24 + the 20-byte v2
/// observability block. Matches `SPECTRUM_HEADER_BYTES` in `ui/src/lib/ipc/frame.ts`.
pub const SPECTRUM_HEADER_BYTES: usize = 44;

/// The observability stamp appended to every frame header (protocol v2).
///
/// Mirrors `ui/src-tauri/src/obs.rs`'s `ObsStamp` field for field, including the
/// honest-absence conventions: `sample_index == 0` means "this producer exposes no
/// sample-domain counter", NOT sample zero; `run_id == 0` means unattributed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObsStamp {
    /// Monotonic per-stream frame counter for the peer this frame is going to.
    pub seq: u32,
    /// Source frame counter at capture; `0` = not available.
    pub sample_index: u64,
    /// The run this frame belongs to; `0` = unattributed.
    pub run_id: u64,
}

/// Pack one channel's spectrum, little-endian:
///
/// ```text
/// offset  type          field
/// 0       u32           ch
/// 4       u32           n_bins
/// 8       f32           f0_hz
/// 12      f32           df_hz
/// 16      u32           kind
/// 20      u32           window_seq
/// 24      u32           seq            // OBS block
/// 28      u64           sample_index
/// 36      u64           run_id
/// 44      f32[n_bins]   mags
/// ```
#[allow(clippy::too_many_arguments)]
pub fn pack_spectrum_frame(
    ch: u32,
    f0_hz: f32,
    df_hz: f32,
    kind: u32,
    window_seq: u32,
    obs: ObsStamp,
    mags: &[f32],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(SPECTRUM_HEADER_BYTES + mags.len() * 4);
    buf.extend_from_slice(&ch.to_le_bytes());
    buf.extend_from_slice(&(mags.len() as u32).to_le_bytes());
    buf.extend_from_slice(&f0_hz.to_le_bytes());
    buf.extend_from_slice(&df_hz.to_le_bytes());
    buf.extend_from_slice(&kind.to_le_bytes());
    buf.extend_from_slice(&window_seq.to_le_bytes());
    buf.extend_from_slice(&obs.seq.to_le_bytes());
    buf.extend_from_slice(&obs.sample_index.to_le_bytes());
    buf.extend_from_slice(&obs.run_id.to_le_bytes());
    for m in mags {
        buf.extend_from_slice(&m.to_le_bytes());
    }
    buf
}

/// Apply a subscription's `bin_decimation` to a magnitude array, and report the
/// `df_hz` the decimated array is on.
///
/// The grid MUST be restated: a decimated array whose `df` still claims the original
/// bin spacing would be drawn at the wrong frequencies, which is a plot that lies
/// rather than a plot that is coarse. Returning both together is what makes that hard
/// to get wrong.
pub fn decimate(sub: &StreamSub, mags: &[f32], df_hz: f64) -> (Vec<f32>, f64) {
    let d = sub.decimation() as usize;
    if d <= 1 {
        return (mags.to_vec(), df_hz);
    }
    let kept: Vec<f32> = mags.iter().copied().step_by(d).collect();
    debug_assert_eq!(kept.len(), sub.decimated_bins(mags.len()));
    (kept, df_hz * d as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_protocol::StreamKind;

    fn le_u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }
    fn le_u64(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }
    fn le_f32(b: &[u8], at: usize) -> f32 {
        f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    /// THE LAYOUT PIN. Every field at the offset `ui/IPC_CONTRACT.md` states, read
    /// back with hand-written little-endian decoders rather than with this module's
    /// own writer - a test that used the writer to check the writer would pass for
    /// any layout at all.
    ///
    /// RED-MUTATION: swap the `kind` and `window_seq` writes (they are adjacent u32s,
    /// so nothing else notices) and this reds at offsets 16 and 20 - which is the
    /// state where every remote spectrum renders with the settling bit of a window
    /// counter.
    #[test]
    fn the_spectrum_header_is_the_pinned_layout_byte_for_byte() {
        let mags = [1.0f32, 2.0, 3.0, 4.0];
        let bytes = pack_spectrum_frame(
            7,
            0.0,
            2.5,
            0x0100, // the settling bit, so a dropped high byte is visible
            0xDEAD,
            ObsStamp {
                seq: 0x11223344,
                sample_index: 0x0102030405060708,
                run_id: 0xF0E0D0C0B0A09080,
            },
            &mags,
        );

        assert_eq!(bytes.len(), SPECTRUM_HEADER_BYTES + 4 * 4);
        assert_eq!(le_u32(&bytes, 0), 7, "ch @0");
        assert_eq!(le_u32(&bytes, 4), 4, "n_bins @4");
        assert_eq!(le_f32(&bytes, 8), 0.0, "f0_hz @8");
        assert_eq!(le_f32(&bytes, 12), 2.5, "df_hz @12");
        assert_eq!(le_u32(&bytes, 16), 0x0100, "kind @16");
        assert_eq!(le_u32(&bytes, 20), 0xDEAD, "window_seq @20");
        assert_eq!(le_u32(&bytes, 24), 0x11223344, "obs.seq @24");
        assert_eq!(le_u64(&bytes, 28), 0x0102030405060708, "sample_index @28");
        assert_eq!(le_u64(&bytes, 36), 0xF0E0D0C0B0A09080, "run_id @36");
        assert_eq!(le_f32(&bytes, 44), 1.0, "first mag @44");
        assert_eq!(le_f32(&bytes, 56), 4.0, "last mag");

        // Explicitly little-endian on the wire, not merely round-trippable: the
        // low byte of `ch` comes first.
        assert_eq!(&bytes[0..4], &[7, 0, 0, 0]);
    }

    /// Decimation restates the GRID as well as the array. A decimated spectrum drawn
    /// on the original `df` is a plot that lies about frequency.
    ///
    /// RED-MUTATION: return `df_hz` unchanged from the `d > 1` path and this reds.
    #[test]
    fn decimation_restates_the_frequency_grid_with_the_array() {
        let mags: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let sub = StreamSub {
            bin_decimation: Some(4),
            ..StreamSub::all(StreamKind::Spectrum)
        };
        let (kept, df) = decimate(&sub, &mags, 1.25);
        assert_eq!(kept, vec![0.0, 4.0, 8.0, 12.0]);
        assert_eq!(df, 5.0, "df must scale with the stride");

        // Stride 1 is the identity, grid untouched.
        let (all, df) = decimate(&StreamSub::all(StreamKind::Spectrum), &mags, 1.25);
        assert_eq!(all, mags);
        assert_eq!(df, 1.25);
    }
}

// ---------------------------------------------------------------------------------
// REMOTE raw-stream frames (2026-08-01): ScopeFrame (min/max envelope
// columns) and WaveformFrame (decimated real samples), byte-identical to the
// pinned Tauri layouts (`ui/src-tauri/src/frame.rs`), PREFIXED with a magic u32.
//
// The prefix exists because Scope/Spectrum/Waveform all begin with a u32 field
// and a single WebSocket carries all of them - the client routes on the magic.
// Only peers that SUBSCRIBED to the Scope/Waveform streams ever receive these,
// so an older front end (which never subscribes them) never sees an unknown
// magic.
// ---------------------------------------------------------------------------------

/// The observability block size (seq u32 + sample_index u64 + run_id u64).
const OBS_BLOCK_BYTES: usize = 20;

fn write_obs(buf: &mut Vec<u8>, obs: ObsStamp) {
    buf.extend_from_slice(&obs.seq.to_le_bytes());
    buf.extend_from_slice(&obs.sample_index.to_le_bytes());
    buf.extend_from_slice(&obs.run_id.to_le_bytes());
}

/// Magic prefix on remote SCOPE frames ("SCOP", little-endian).
pub const REMOTE_SCOPE_MAGIC: u32 = 0x53434f50;
/// Magic prefix on remote WAVEFORM frames ("WAVE", little-endian).
pub const REMOTE_WAVE_MAGIC: u32 = 0x57415645;

/// One channel of a scope (envelope) frame.
#[derive(Debug, Clone)]
pub struct ScopeChannel {
    /// The channel table's 1-based id (u16 on the wire).
    pub id: u16,
    /// In-frame RMS, EU.
    pub rms: f32,
    /// In-frame peak (max |x|), EU.
    pub peak: f32,
    /// Per-column minima; `mins.len() == n_cols`.
    pub mins: Vec<f32>,
    /// Per-column maxima; `maxs.len() == n_cols`.
    pub maxs: Vec<f32>,
}

/// Pack a magic-prefixed ScopeFrame (layout: `ui/src-tauri/src/frame.rs`
/// `pack_scope_frame`, byte-for-byte after the 4-byte magic).
pub fn pack_remote_scope_frame(
    n_cols: u32,
    t_end_s: f64,
    dt_s: f32,
    obs: ObsStamp,
    channels: &[ScopeChannel],
) -> Vec<u8> {
    let cols = n_cols as usize;
    let mut buf = Vec::with_capacity(4 + 24 + OBS_BLOCK_BYTES + channels.len() * (12 + 8 * cols));
    buf.extend_from_slice(&REMOTE_SCOPE_MAGIC.to_le_bytes());
    buf.extend_from_slice(&(channels.len() as u32).to_le_bytes());
    buf.extend_from_slice(&n_cols.to_le_bytes());
    buf.extend_from_slice(&t_end_s.to_le_bytes());
    buf.extend_from_slice(&dt_s.to_le_bytes());
    buf.extend_from_slice(&0f32.to_le_bytes());
    write_obs(&mut buf, obs);
    for ch in channels {
        debug_assert_eq!(ch.mins.len(), cols);
        debug_assert_eq!(ch.maxs.len(), cols);
        buf.extend_from_slice(&ch.id.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&ch.rms.to_le_bytes());
        buf.extend_from_slice(&ch.peak.to_le_bytes());
        for v in &ch.mins {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in &ch.maxs {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    buf
}

/// One channel of a waveform frame.
#[derive(Debug, Clone)]
pub struct WaveChannel {
    /// The channel table's 1-based id (u16 on the wire).
    pub id: u16,
    /// Decimated samples, EU; `samples.len() == n_samples`.
    pub samples: Vec<f32>,
}

/// Pack a magic-prefixed WaveformFrame (layout: `ui/src-tauri/src/frame.rs`
/// `pack_waveform_frame`, byte-for-byte after the 4-byte magic).
pub fn pack_remote_waveform_frame(
    n_samples: u32,
    t_end_s: f64,
    dt_s: f32,
    obs: ObsStamp,
    channels: &[WaveChannel],
) -> Vec<u8> {
    let n = n_samples as usize;
    let mut buf = Vec::with_capacity(4 + 24 + OBS_BLOCK_BYTES + channels.len() * (4 + 4 * n));
    buf.extend_from_slice(&REMOTE_WAVE_MAGIC.to_le_bytes());
    buf.extend_from_slice(&(channels.len() as u32).to_le_bytes());
    buf.extend_from_slice(&n_samples.to_le_bytes());
    buf.extend_from_slice(&t_end_s.to_le_bytes());
    buf.extend_from_slice(&dt_s.to_le_bytes());
    buf.extend_from_slice(&0f32.to_le_bytes());
    write_obs(&mut buf, obs);
    for ch in channels {
        debug_assert_eq!(ch.samples.len(), n);
        buf.extend_from_slice(&ch.id.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        for v in &ch.samples {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    buf
}
