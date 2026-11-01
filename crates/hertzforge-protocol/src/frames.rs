//! Opaque carriage for the binary display frames.
//!
//! The scope / waveform / spectrum frames are hand-packed little-endian layouts,
//! pinned by `ui/IPC_CONTRACT.md` and produced by explicit byte writers
//! (`to_le_bytes` + `extend_from_slice`, deliberately NOT `#[repr(C)]` transmutes) so
//! "the layout is exactly the contract on every host". That property makes them
//! already transport-agnostic: the SAME bytes that cross the in-process Tauri channel
//! today can cross a socket tomorrow. This crate therefore carries them as
//! [`FrameBytes`] - tagged, opaque, bit-for-bit - and never re-encodes, re-packs, or
//! interprets them. The producer packers (`ui/src-tauri/src/frame.rs` today, the core
//! host after the split) and the TS decoder (`ui/src/lib/ipc/frame.ts`) stay the two
//! ends of that byte contract; this protocol is a pipe between them.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Which pinned frame layout a [`FrameBytes`] payload holds. Closed enum: one variant
/// per pinned layout in `ui/IPC_CONTRACT.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// `ScopeFrame`: decimated (min, max) envelope columns per channel.
    Scope,
    /// `WaveformFrame`: anti-alias-decimated raw samples per channel.
    Waveform,
    /// `SpectrumFrame`: one channel's magnitude bins on a line grid.
    Spectrum,
}

/// An opaque, already-packed display frame. The payload is EXACTLY the byte string
/// the producer's packer emitted - this type never inspects or transforms it.
///
/// `#[serde(transparent)]`: on the wire this is the byte sequence itself, with no
/// added structure, so the carried bytes stay bit-for-bit identical through any serde
/// codec (see the round-trip test in `tests/roundtrip.rs`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrameBytes(Vec<u8>);

impl FrameBytes {
    /// Wrap an already-packed frame. Takes ownership; no copy, no inspection.
    pub fn new(packed: Vec<u8>) -> Self {
        FrameBytes(packed)
    }

    /// The packed bytes, for handing to a decoder or a transport.
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Unwrap into the packed bytes (e.g. to hand to the existing TS-decoder path
    /// without a copy).
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    /// Payload length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when the payload is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for FrameBytes {
    fn from(packed: Vec<u8>) -> Self {
        FrameBytes(packed)
    }
}

/// Debug prints the length, not the payload - frames are tens of kilobytes and would
/// otherwise flood logs.
impl fmt::Debug for FrameBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FrameBytes({} bytes)", self.0.len())
    }
}
