//! SHA-256 and base64 DECODE, in-tree.
//!
//! # Why these are here and not a dependency
//!
//! The workspace has no `sha2` and no `base64`, and the TWR reference upload is the
//! first thing in it that needs either. Adding two crates to a build that must
//! cross-compile to the Windows kit without a C toolchain, pass `cargo deny check bans`
//! and stay inside the permissive-licence rule, in order to get one 60-line hash and one
//! 40-line decoder, is a poor trade. Both are fully specified, both are exercised
//! against published test vectors below, and the sha is additionally gated end-to-end by
//! the wire probe's happy path - which computes its digest with Python's `hashlib` and
//! asserts the number this file produced.
//!
//! Neither is a general-purpose implementation and neither pretends to be: the hash is
//! not constant-time (it is a content check on a waveform, not a MAC) and the decoder
//! accepts only canonical base64 with padding, which is what `serde_json` will have
//! carried.

use eigen_protocol::ProtocolError;

// =====================================================================================
// SHA-256 (FIPS 180-4)
// =====================================================================================

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// A streaming SHA-256, so a multi-megabyte upload is hashed CHUNK BY CHUNK as it
/// arrives rather than by re-walking the whole buffer at commit.
pub struct Sha256 {
    h: [u32; 8],
    /// Bytes not yet absorbed into a full 64-byte block.
    tail: Vec<u8>,
    len_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            tail: Vec::with_capacity(64),
            len_bytes: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.len_bytes = self.len_bytes.wrapping_add(data.len() as u64);
        let mut at = 0usize;
        if !self.tail.is_empty() {
            let want = 64 - self.tail.len();
            let take = want.min(data.len());
            self.tail.extend_from_slice(&data[..take]);
            at = take;
            if self.tail.len() == 64 {
                let block: [u8; 64] = self.tail[..].try_into().expect("64 bytes");
                self.compress(&block);
                self.tail.clear();
            }
        }
        while at + 64 <= data.len() {
            let block: [u8; 64] = data[at..at + 64].try_into().expect("64 bytes");
            self.compress(&block);
            at += 64;
        }
        self.tail.extend_from_slice(&data[at..]);
    }

    /// The digest as 64 lowercase hex characters.
    pub fn finish_hex(mut self) -> String {
        let bit_len = self.len_bytes.wrapping_mul(8);
        let mut pad = Vec::with_capacity(72);
        pad.push(0x80u8);
        while (self.tail.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bit_len.to_be_bytes());
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(&pad);
        for block in buf.chunks_exact(64) {
            let b: [u8; 64] = block.try_into().expect("64 bytes");
            self.compress(&b);
        }
        let mut out = String::with_capacity(64);
        for w in self.h {
            out.push_str(&format!("{w:08x}"));
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in self.h.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(v);
        }
    }
}

/// Is `s` exactly 64 LOWERCASE hex characters?
///
/// Lowercase specifically, and not a case-insensitive compare: the digest is the KEY a
/// profile names a staged record by, so `AB..` and `ab..` naming the same record would
/// make the lookup depend on how the client happened to format it.
pub fn is_lowercase_hex_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// =====================================================================================
// base64 decode (RFC 4648, canonical + padded)
// =====================================================================================

/// Decode canonical padded base64 into bytes.
///
/// STRICT on purpose: whitespace, the URL-safe alphabet and missing padding are all
/// refused rather than accommodated. A chunk that arrived mangled is a chunk whose bytes
/// are not the operator's, and the three-part upload guard exists precisely so that gets
/// said out loud instead of hashed into a record nobody authored.
pub fn decode_b64(s: &str) -> Result<Vec<u8>, ProtocolError> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(ProtocolError::MalformedPayload {
            container: "TWR reference chunk".into(),
            detail: format!(
                "base64 length {} is not a multiple of 4; canonical padded base64 is required \
                 (no whitespace, no URL-safe alphabet)",
                bytes.len()
            ),
        });
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (i, quad) in bytes.chunks_exact(4).enumerate() {
        let pad = quad.iter().filter(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && i + 1 != bytes.len() / 4) {
            return Err(ProtocolError::MalformedPayload {
                container: "TWR reference chunk".into(),
                detail: "base64 padding may appear only in the final group, at most twice".into(),
            });
        }
        let mut acc = 0u32;
        for &b in quad {
            let v = if b == b'=' {
                0
            } else {
                val(b).ok_or_else(|| ProtocolError::MalformedPayload {
                    container: "TWR reference chunk".into(),
                    detail: format!("{:?} is not a base64 character", b as char),
                })?
            };
            acc = (acc << 6) | v as u32;
        }
        let three = acc.to_be_bytes();
        out.push(three[1]);
        if pad < 2 {
            out.push(three[2]);
        }
        if pad < 1 {
            out.push(three[3]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        h.finish_hex()
    }

    /// **THE PUBLISHED VECTORS** (FIPS 180-4 / NIST CAVS). An in-tree hash that is only
    /// checked against itself is a checksum, not a digest: the whole point of SHA-256
    /// here is that the CLIENT computed it independently, so the implementation has to
    /// agree with the rest of the world and not merely with this file.
    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // A million 'a' - the vector that exercises the length encoding past 2^19 bits.
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(
            h.finish_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// The STREAMING property: the digest does not depend on how the bytes were split.
    ///
    /// This is the one that matters for the upload - the client chooses the chunk size
    /// and the core hashes as it goes, so a boundary-sensitive implementation would
    /// refuse valid uploads for reasons neither end could see.
    #[test]
    fn the_digest_is_independent_of_the_chunking() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let whole = hex(&data);
        for chunk in [1usize, 7, 63, 64, 65, 1000] {
            let mut h = Sha256::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finish_hex(), whole, "chunked by {chunk}");
        }
    }

    /// base64, both directions of the contract: canonical input decodes, and the three
    /// mangled shapes are REFUSED rather than silently coerced.
    #[test]
    fn base64_decodes_canonical_input_and_refuses_the_rest() {
        assert_eq!(decode_b64("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_b64("AAAAAA==").unwrap(), vec![0u8, 0, 0, 0]);
        // 1.0f32 little-endian is 00 00 80 3F - the shape every reference chunk carries.
        assert_eq!(
            decode_b64("AACAPw==").unwrap(),
            vec![0x00, 0x00, 0x80, 0x3f]
        );
        assert_eq!(decode_b64("TWFu").unwrap(), b"Man".to_vec());
        assert_eq!(decode_b64("TWE=").unwrap(), b"Ma".to_vec());

        for bad in ["TWFu=", "TW Fu", "TW-u", "TWFuTW=uTWFu"] {
            assert!(
                decode_b64(bad).is_err(),
                "{bad:?} must be refused, not coerced"
            );
        }
    }

    /// The digest FORMAT gate, which is what stops a record being staged under a key no
    /// profile could address.
    #[test]
    fn only_64_lowercase_hex_characters_are_a_sha() {
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(is_lowercase_hex_sha256(good));
        assert!(!is_lowercase_hex_sha256(&good.to_uppercase()));
        assert!(!is_lowercase_hex_sha256(&good[..63]));
        assert!(!is_lowercase_hex_sha256(&format!("{good}0")));
        assert!(!is_lowercase_hex_sha256("deadbeef"));
        assert!(!is_lowercase_hex_sha256(""));
    }
}
