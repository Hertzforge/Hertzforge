//! The chunked REFERENCE UPLOAD's state: an in-progress [`Staging`] and the published
//! [`StagedReference`].
//!
//! # Three checks, and each catches what the others cannot
//!
//! `BeginReference` DECLARES `total_samples` and `sha256`; the chunks are checked for
//! STRICT `seq + 1` ordering as they arrive; `CommitReference` asserts BOTH that the
//! received count equals the declared count AND that the digest of what was staged
//! equals the declared digest.
//!
//! * A DROPPED chunk passes a digest check that is never reached (the record is short,
//!   so commit refuses on the count first) - but a client that dropped one and re-sent
//!   the tail would pass the count, which is why ordering is its own gate.
//! * A REORDERED pair passes the count AND, on a symmetric record, could pass a
//!   casual eyeball. The digest catches it; the ordering gate catches it EARLIER, at the
//!   chunk that jumped, which is what makes the refusal name the seq.
//! * A CORRUPTED-BUT-COMPLETE transfer passes ordering and count. Only the digest sees it.
//!
//! Every refusal names BOTH numbers. "The upload failed" is not an actionable sentence
//! for someone holding a 60-second field trace.
//!
//! # The staged record is RAW
//!
//! The daemon stores exactly the samples that crossed and applies the PROFILE's prep at
//! configure. One authority: a prep can never compound, because the thing
//! it is applied to is never a prepped record.

use hertzforge_protocol::ProtocolError;

use super::digest::{decode_b64, Sha256};

/// A reference upload in progress.
pub struct Staging {
    name: String,
    fs_hz: f64,
    eu: String,
    total_samples: u64,
    declared_sha: String,
    /// The seq the NEXT chunk must carry.
    next_seq: u32,
    /// Samples decoded so far, in arrival order.
    samples: Vec<f64>,
    /// The digest, hashed as the bytes arrive rather than re-walked at commit.
    hasher: Sha256,
}

impl Staging {
    /// Open a staging from a `BeginReference` that has already been validated by the
    /// host (rate, count ceiling, digest format).
    pub fn new(
        name: String,
        fs_hz: f64,
        eu: String,
        total_samples: u64,
        declared_sha: String,
    ) -> Self {
        Staging {
            name,
            fs_hz,
            eu,
            total_samples,
            declared_sha,
            next_seq: 0,
            samples: Vec::with_capacity(total_samples.min(1 << 20) as usize),
            hasher: Sha256::new(),
        }
    }

    /// Samples received so far.
    pub fn received_samples(&self) -> u64 {
        self.samples.len() as u64
    }

    /// Accept one chunk. STRICT `seq + 1`: a gap or a reorder is refused BY NAME rather
    /// than stitched.
    pub fn accept_chunk(&mut self, seq: u32, samples_b64_f32le: &str) -> Result<(), ProtocolError> {
        if seq != self.next_seq {
            return Err(ProtocolError::Refused {
                message: format!(
                    "reference chunk {seq} arrived where chunk {} was expected. Chunks are \
                     STRICTLY seq+1 and a gap or a reorder is refused rather than stitched: a \
                     record that is complete-but-scrambled passes a COUNT check and produces a \
                     test against a waveform nobody authored. Restart the upload with \
                     BeginReference.",
                    self.next_seq
                ),
            });
        }
        let bytes = decode_b64(samples_b64_f32le)?;
        if !bytes.len().is_multiple_of(4) {
            return Err(ProtocolError::MalformedPayload {
                container: "TWR reference chunk".into(),
                detail: format!(
                    "{} bytes is not a whole number of little-endian f32 samples",
                    bytes.len()
                ),
            });
        }
        let n = (bytes.len() / 4) as u64;
        if self.samples.len() as u64 + n > self.total_samples {
            return Err(ProtocolError::Refused {
                message: format!(
                    "this chunk would take the upload to {} samples and BeginReference declared \
                     {}. The declaration is what the core sized itself against; send the record \
                     the manifest describes, or restart the upload.",
                    self.samples.len() as u64 + n,
                    self.total_samples
                ),
            });
        }
        // The digest is over the WIRE BYTES (little-endian f32), which is what the client
        // hashed. Hashing the decoded f64 samples instead would produce a number no
        // client could ever predict.
        self.hasher.update(&bytes);
        self.samples.extend(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64),
        );
        self.next_seq = self.next_seq.saturating_add(1);
        Ok(())
    }

    /// Assert the COUNT and the DIGEST, then publish the record.
    pub fn finish(self) -> Result<StagedReference, ProtocolError> {
        let received = self.samples.len() as u64;
        if received != self.total_samples {
            return Err(ProtocolError::Refused {
                message: format!(
                    "commit refused: {received} samples were received and BeginReference \
                     declared {}. A short record would replicate a truncated transient and look \
                     like a converged run.",
                    self.total_samples
                ),
            });
        }
        let computed = self.hasher.finish_hex();
        if computed != self.declared_sha {
            // THE ETAG SHAPE, in the `Refused` carrier and not in `ProtocolError::
            // StaleUpload` - and this is a deliberate departure from the design's
            // original wording.
            //
            // The design said "a `TableDoc`-shaped etag-guarded exchange (StaleUpload
            // vocabulary)", and the VOCABULARY is what transfers: the client states what
            // it believes it sent, the core states what it actually holds, and the
            // refusal names BOTH. The TYPE does not: `StaleUpload`'s own `Display`
            // reads "refused a STALE channel-table write ... this document carries the
            // abort limits", which is a sentence about a different document entirely.
            // A typed error whose rendered text is about the wrong thing is worse than a
            // `Refused` that says the right thing (byte beats doc).
            return Err(ProtocolError::Refused {
                message: format!(
                    "commit refused: BeginReference declared sha256 {} and the core computed \
                     {} over the {received} samples it actually holds. The bytes that arrived \
                     are not the record the operator chose, so nothing is staged. Re-send the \
                     upload.",
                    self.declared_sha, computed
                ),
            });
        }
        Ok(StagedReference {
            name: self.name,
            fs_hz: self.fs_hz,
            eu: self.eu,
            // The digest the core COMPUTED. Byte-identical to the declaration by the
            // check just above, and stored from the computed side deliberately: the
            // value a profile will name is the one this core verified, never the one a
            // client asserted.
            sha256: computed,
            samples: self.samples,
        })
    }
}

/// A reference record staged on this core, RAW.
#[derive(Debug, Clone)]
pub struct StagedReference {
    /// Operator-facing label. Display only - the run is keyed by [`StagedReference::sha256`].
    pub name: String,
    /// The record's own sample rate, Hz.
    pub fs_hz: f64,
    /// Engineering-unit label ("g" by convention). Display only.
    pub eu: String,
    /// The digest the core COMPUTED and verified against the client's declaration.
    pub sha256: String,
    /// The RAW samples. Prep is applied from the PROFILE at configure, never here.
    pub samples: Vec<f64>,
}

impl StagedReference {
    /// The manifest, as a JSON value (for embedding in `ReferenceInfo`).
    pub fn manifest_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "fs_hz": self.fs_hz,
            "eu": self.eu,
            // The sha the CORE computed over the bytes it holds - never the client's own
            // declaration echoed back, which is the thing the commit exists to check.
            "sha256": self.sha256,
            "samples": self.samples.len(),
            "duration_s": self.samples.len() as f64 / self.fs_hz,
            "peak_abs": self.samples.iter().fold(0.0f64, |m, &v| m.max(v.abs())),
        })
    }

    /// The `CommitReference` acknowledgement document.
    pub fn commit_json(&self) -> String {
        serde_json::json!({ "staged": self.manifest_json_value() }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 {
                A[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if c.len() > 2 {
                A[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    fn record(n: usize) -> (Vec<u8>, String) {
        let bytes: Vec<u8> = (0..n)
            .flat_map(|i| ((i as f32) * 0.25).to_le_bytes())
            .collect();
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha = h.finish_hex();
        (bytes, sha)
    }

    fn staging(n: usize, sha: &str) -> Staging {
        Staging::new("r.csv".into(), 8192.0, "g".into(), n as u64, sha.into())
    }

    /// THE HAPPY PATH: the commit answers the sha the CORE computed, and the samples
    /// survive the f32 wire round trip.
    #[test]
    fn a_clean_upload_commits_and_the_manifest_carries_the_core_computed_sha() {
        let (bytes, sha) = record(16);
        let mut st = staging(16, &sha);
        for (seq, chunk) in bytes.chunks(4 * 4).enumerate() {
            st.accept_chunk(seq as u32, &b64(chunk)).expect("chunk");
        }
        let staged = st.finish().expect("commit");
        assert_eq!(staged.sha256, sha);
        assert_eq!(staged.samples.len(), 16);
        assert!((staged.samples[4] - 1.0).abs() < 1e-9);
        assert!(staged.commit_json().contains(&sha));
    }

    /// A DROPPED chunk is refused AT THE GAP, naming both seqs - not at commit, where
    /// the only evidence left would be a count.
    ///
    /// RED-MUTATION: relax the check to `seq < self.next_seq` and the gap is accepted;
    /// the commit then refuses on the count instead, with a message that cannot say
    /// WHICH chunk went missing.
    #[test]
    fn a_dropped_chunk_is_refused_by_name_at_the_gap() {
        let (bytes, sha) = record(16);
        let chunks: Vec<&[u8]> = bytes.chunks(4 * 4).collect();
        let mut st = staging(16, &sha);
        st.accept_chunk(0, &b64(chunks[0])).unwrap();
        st.accept_chunk(1, &b64(chunks[1])).unwrap();
        match st.accept_chunk(3, &b64(chunks[3])) {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("chunk 3"), "{message}");
                assert!(message.contains("chunk 2"), "{message}");
                assert!(message.contains("scrambled"), "{message}");
            }
            other => panic!("a gap must be refused by name: {other:?}"),
        }
    }

    /// A REORDERED pair is refused at the chunk that jumped.
    #[test]
    fn a_reordered_chunk_is_refused_by_name() {
        let (bytes, sha) = record(16);
        let chunks: Vec<&[u8]> = bytes.chunks(4 * 4).collect();
        let mut st = staging(16, &sha);
        st.accept_chunk(0, &b64(chunks[0])).unwrap();
        assert!(st.accept_chunk(2, &b64(chunks[2])).is_err());
    }

    /// A SHORT record is refused at commit, naming BOTH counts.
    #[test]
    fn a_short_count_is_refused_with_both_numbers() {
        let (bytes, sha) = record(16);
        let mut st = staging(16, &sha);
        for (seq, chunk) in bytes.chunks(4 * 4).take(3).enumerate() {
            st.accept_chunk(seq as u32, &b64(chunk)).unwrap();
        }
        match st.finish() {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("12 samples"), "{message}");
                assert!(message.contains("16"), "{message}");
                assert!(message.contains("truncated transient"), "{message}");
            }
            other => panic!("a short record must be refused: {other:?}"),
        }
    }

    /// A CORRUPTED-BUT-COMPLETE transfer passes ordering AND count, and only the digest
    /// sees it. The refusal names BOTH digests - the etag shape applied to an upload.
    ///
    /// RED-MUTATION: compare `computed` against itself and the corrupt record commits
    /// silently, which is the state where a test runs against a waveform nobody
    /// authored.
    #[test]
    fn a_wrong_digest_is_refused_naming_both_digests() {
        let (bytes, sha) = record(16);
        let mut corrupt = bytes.clone();
        corrupt[7] ^= 0xff;
        let mut st = staging(16, &sha);
        for (seq, chunk) in corrupt.chunks(4 * 4).enumerate() {
            st.accept_chunk(seq as u32, &b64(chunk)).unwrap();
        }
        match st.finish() {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains(&sha), "the DECLARED digest: {message}");
                // ...and a DIFFERENT one, which is the half that says what the core
                // actually holds. Located by removing the declared one and looking for a
                // second full-length digest in what is left.
                let rest = message.replace(&sha, "");
                let computed = rest
                    .split_whitespace()
                    .find(|w| crate::twr::digest::is_lowercase_hex_sha256(w))
                    .unwrap_or_else(|| panic!("no computed digest in: {message}"));
                assert_ne!(computed, sha, "the COMPUTED digest must differ");
            }
            other => panic!("a corrupted record must be refused: {other:?}"),
        }
    }

    /// An OVERLONG chunk is refused before it is appended, so the ceiling the client
    /// declared is the one the core holds itself to.
    #[test]
    fn a_chunk_past_the_declared_total_is_refused() {
        let (bytes, sha) = record(16);
        let mut st = staging(8, &sha); // declare HALF the record
        st.accept_chunk(0, &b64(&bytes[..32])).unwrap();
        match st.accept_chunk(1, &b64(&bytes[32..])) {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("declared"), "{message}");
            }
            other => panic!("an over-long upload must be refused: {other:?}"),
        }
    }
}
