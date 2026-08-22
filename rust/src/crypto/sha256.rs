//! Streaming SHA-256 (FIPS 180-4) implemented from the public
//! specification with no unsafe code and no third-party dependencies.
//!
//! This module is used to verify downloaded binaries (yt-dlp) against
//! the official SHA-256 checksums, so it MUST match the FIPS reference
//! vectors exactly (validated by the `fips_vectors` test).

/// First 32 bits of the fractional parts of the cube roots of the first
/// 64 primes.
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

/// Initial hash values: first 32 bits of the fractional parts of the
/// square roots of the first 8 primes.
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// A streaming SHA-256 hasher.
///
/// Feed bytes with [`update`](Sha256::update) and finalize with
/// [`finalize`](Sha256::finalize). The internal state is a plain
/// struct, so the hasher can be copied before finalizing if a
/// mid-stream digest is required.
#[derive(Clone, Debug)]
pub struct Sha256 {
    /// Current 256-bit working state.
    state: [u32; 8],
    /// Remaining bytes of the current 64-byte block.
    buffer: [u8; 64],
    /// Number of valid bytes in `buffer`.
    buffered: usize,
    /// Total number of bytes processed (mod 2^64), for the length field.
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// Create a new hasher in the FIPS 180-4 initial state.
    pub const fn new() -> Self {
        Self {
            state: H0,
            buffer: [0u8; 64],
            buffered: 0,
            total_len: 0,
        }
    }

    /// Feed `data` into the hasher.
    pub fn update(&mut self, data: &[u8]) {
        // Total bytes processed feeds the length field in `finalize`;
        // accumulate it for every input byte (buffered or compressed).
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        let mut data = data;
        // If there are buffered bytes, top up the block first.
        if self.buffered > 0 && self.buffered + data.len() >= 64 {
            let take = 64 - self.buffered;
            self.buffer[self.buffered..64].copy_from_slice(&data[..take]);
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
            data = &data[take..];
        }

        // Compress whole blocks directly from the input slice.
        while data.len() >= 64 {
            let (block, rest) = data.split_at(64);
            let mut block_buf = [0u8; 64];
            block_buf.copy_from_slice(block);
            self.compress(&block_buf);
            data = rest;
        }

        // Stash the trailing partial block.
        if !data.is_empty() {
            self.buffer[self.buffered..self.buffered + data.len()].copy_from_slice(data);
            self.buffered += data.len();
        }
    }

    /// Finalize and return the 32-byte digest. The hasher is left in a
    /// valid-but-unspecified state; call [`reset`](Sha256::reset) before
    /// reusing it.
    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        // Append the 0x80 padding byte.
        self.update(&[0x80]);
        // Zero-fill the remainder of the block, then the final 8 bytes
        // hold the big-endian bit length.
        while self.buffered != 56 {
            self.update(&[0x00]);
        }
        // `buffered == 56`: exactly room for the length word.
        let len_bytes = bit_len.to_be_bytes();
        self.update(&len_bytes);
        debug_assert_eq!(self.buffered, 0);

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Reset the hasher to its initial state.
    pub fn reset(&mut self) {
        *self = Sha256::new();
    }

    /// Process one 64-byte block (FIPS 180-4 §6.2.2).
    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().enumerate().take(16) {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            self.state[0],
            self.state[1],
            self.state[2],
            self.state[3],
            self.state[4],
            self.state[5],
            self.state[6],
            self.state[7],
        );

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

/// One-shot SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

/// Hex digest (lowercase, 64 chars).
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fips_vectors() {
        // FIPS 180-4 Appendix B.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn streaming_matches_oneshot() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let mut hasher = Sha256::new();
        for chunk in data.chunks(7) {
            hasher.update(chunk);
        }
        assert_eq!(
            hex_digest(hasher.finalize()),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        assert_eq!(sha256(data), hasher_clone(data));
    }

    #[test]
    fn boundary_padding_cases() {
        // A 55-byte input leaves exactly one byte before the 56-byte
        // length position in the padded block.
        let input55 = [0x61u8; 55];
        let mut h = Sha256::new();
        h.update(&input55);
        let d1 = h.finalize();
        // A 56-byte input needs an extra padding block.
        let input56 = [0x61u8; 56];
        let d2 = sha256(&input56);
        // A 63-byte input: padding fills the same block.
        let input63 = [0x61u8; 63];
        let d3 = sha256(&input63);
        // A 64-byte input: one full block plus padding block.
        let input64 = [0x61u8; 64];
        let d4 = sha256(&input64);
        assert_ne!(d1, d2);
        assert_ne!(d2, d3);
        assert_ne!(d3, d4);
    }

    fn hex_digest(digest: [u8; 32]) -> String {
        let mut out = String::with_capacity(64);
        for byte in digest {
            out.push_str(&format!("{:02x}", byte));
        }
        out
    }

    fn hasher_clone(data: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize()
    }
}
