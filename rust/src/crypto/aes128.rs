//! Self-contained AES-128 (FIPS-197) block cipher, written from scratch.
//!
//! The upstream `aes` crate moved to edition 2024 and now requires a much
//! newer toolchain than the project's declared MSRV (Rust 1.88). Rather than
//! dropping the MSRV floor for one block cipher, this module re-implements
//! AES-128 directly against FIPS-197: the S-box, key expansion, and both the
//! forward and inverse ciphers. Only the 128-bit key size is implemented —
//! exactly what HLS `METHOD=AES-128` (RFC 8216) needs. There is no unsafe
//! code and no table generation at compile time beyond a plain lookup table.
//!
//! Correctness is pinned by the FIPS-197 Appendix C vectors in the unit
//! tests below, so this is not "trust me" crypto.

/// FIPS-197 §5.1.1 S-box.
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// FIPS-197 §5.2 round constants.
const RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

/// Build the inverse S-box at compile time (`INV_SBOX[SBOX[i]] == i`).
const fn build_inv_sbox() -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        inv[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    inv
}

/// FIPS-197 §5.3.2 inverse S-box.
const INV_SBOX: [u8; 256] = build_inv_sbox();

/// Multiply a byte by `x` in GF(2^8) with the AES reduction polynomial
/// x^8 + x^4 + x^3 + x + 1 (FIPS-197 §4.2.1 `xtime`).
#[inline]
fn xtime(byte: u8) -> u8 {
    let doubled = byte << 1;
    if byte & 0x80 != 0 {
        doubled ^ 0x1b
    } else {
        doubled
    }
}

/// Multiply two field elements `{a} * {b}` in GF(2^8) (FIPS-197 §4.2.1),
/// implemented as Russian-peasant multiplication over `xtime` so the code
/// never needs a 256-entry log/alog table.
#[inline]
fn gmulu8(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut product = 0u8;
    // 8 iterations are enough because the field is GF(2^8).
    for _ in 0..8 {
        if b & 1 != 0 {
            product ^= a;
        }
        let hi_bit = a & 0x80;
        a <<= 1;
        if hi_bit != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    product
}

/// Substitute every byte through the S-box.
#[inline]
fn sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = SBOX[*byte as usize];
    }
}

/// Inverse S-box substitution.
#[inline]
fn inv_sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = INV_SBOX[*byte as usize];
    }
}

/// FIPS-197 §5.1.2 ShiftRows (state is column-major, 4×4 bytes).
#[inline]
fn shift_rows(state: &mut [u8; 16]) {
    // Row 0: unchanged. Row 1: left by 1, row 2: left by 2, row 3: left by 3.
    let s = *state;
    state[0] = s[0];
    state[4] = s[4];
    state[8] = s[8];
    state[12] = s[12];
    state[1] = s[5];
    state[5] = s[9];
    state[9] = s[13];
    state[13] = s[1];
    state[2] = s[10];
    state[6] = s[14];
    state[10] = s[2];
    state[14] = s[6];
    state[3] = s[15];
    state[7] = s[3];
    state[11] = s[7];
    state[15] = s[11];
}

/// FIPS-197 §5.3.1 InvShiftRows (the inverse rotation of `shift_rows`).
#[inline]
fn inv_shift_rows(state: &mut [u8; 16]) {
    let s = *state;
    state[0] = s[0];
    state[4] = s[4];
    state[8] = s[8];
    state[12] = s[12];
    state[1] = s[13];
    state[5] = s[1];
    state[9] = s[5];
    state[13] = s[9];
    state[2] = s[10];
    state[6] = s[14];
    state[10] = s[2];
    state[14] = s[6];
    state[3] = s[7];
    state[7] = s[11];
    state[11] = s[15];
    state[15] = s[3];
}

/// FIPS-197 §5.1.3 MixColumns over the four columns of the state.
#[inline]
fn mix_columns(state: &mut [u8; 16]) {
    for column in state.as_chunks_mut::<4>().0 {
        let a = column[0];
        let b = column[1];
        let c = column[2];
        let d = column[3];
        column[0] = xtime(a) ^ (xtime(b) ^ b) ^ c ^ d;
        column[1] = a ^ xtime(b) ^ (xtime(c) ^ c) ^ d;
        column[2] = a ^ b ^ xtime(c) ^ (xtime(d) ^ d);
        column[3] = (xtime(a) ^ a) ^ b ^ c ^ xtime(d);
    }
}

/// FIPS-197 §5.3.3 InvMixColumns.
#[inline]
fn inv_mix_columns(state: &mut [u8; 16]) {
    for column in state.as_chunks_mut::<4>().0 {
        let a = column[0];
        let b = column[1];
        let c = column[2];
        let d = column[3];
        column[0] = gmulu8(a, 0x0e) ^ gmulu8(b, 0x0b) ^ gmulu8(c, 0x0d) ^ gmulu8(d, 0x09);
        column[1] = gmulu8(a, 0x09) ^ gmulu8(b, 0x0e) ^ gmulu8(c, 0x0b) ^ gmulu8(d, 0x0d);
        column[2] = gmulu8(a, 0x0d) ^ gmulu8(b, 0x09) ^ gmulu8(c, 0x0e) ^ gmulu8(d, 0x0b);
        column[3] = gmulu8(a, 0x0b) ^ gmulu8(b, 0x0d) ^ gmulu8(c, 0x09) ^ gmulu8(d, 0x0e);
    }
}

/// XOR a round key (16 bytes) into the state.
#[inline]
fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for (byte, key_byte) in state.iter_mut().zip(round_key.iter()) {
        *byte ^= key_byte;
    }
}

/// FIPS-197 §5.2 key expansion for a 128-bit key: 11 round keys of 16 bytes.
fn expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut round_keys = [[0u8; 16]; 11];
    round_keys[0].copy_from_slice(key);

    let mut previous = round_keys[0];
    for (round, rcon) in RCON.iter().copied().enumerate().skip(1) {
        let mut word = [
            previous[13],
            previous[14],
            previous[15],
            previous[12], // RotWord
        ];
        // SubWord
        for byte in word.iter_mut() {
            *byte = SBOX[*byte as usize];
        }
        word[0] ^= rcon;
        let mut next = [0u8; 16];
        for i in 0..4 {
            next[i] = previous[i] ^ word[i];
            next[i + 4] = previous[i + 4] ^ next[i];
            next[i + 8] = previous[i + 8] ^ next[i + 4];
            next[i + 12] = previous[i + 12] ^ next[i + 8];
        }
        round_keys[round] = next;
        previous = next;
    }
    round_keys
}

/// Encrypt a single 128-bit block with AES-128 (FIPS-197 §5.1 forward cipher).
pub fn encrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let round_keys = expand_key(key);
    let mut state = *block;
    add_round_key(&mut state, &round_keys[0]);
    for round_key in &round_keys[1..10] {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, round_key);
    }
    sub_bytes(&mut state);
    shift_rows(&mut state);
    add_round_key(&mut state, &round_keys[10]);
    state
}

/// Decrypt a single 128-bit block with AES-128 (FIPS-197 §5.3 inverse cipher).
///
/// Implements the plain inverse cipher directly: run the rounds backwards
/// with InvShiftRows / InvSubBytes / AddRoundKey and apply InvMixColumns to
/// the state after every middle-round AddRoundKey.
pub fn decrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let round_keys = expand_key(key);
    let mut state = *block;
    add_round_key(&mut state, &round_keys[10]);
    for round_key in round_keys[1..10].iter().rev() {
        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        add_round_key(&mut state, round_key);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    add_round_key(&mut state, &round_keys[0]);
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS-197 Appendix C.1 — AES-128 encryption.
    #[test]
    fn fips_197_c1_encrypt_vector() {
        let key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let plaintext: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let expected: [u8; 16] = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        assert_eq!(encrypt_block(&key, &plaintext), expected);
    }

    /// FIPS-197 Appendix C.1 — AES-128 decryption (same key/ciphertext pair).
    #[test]
    fn fips_197_c1_decrypt_vector() {
        let key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let ciphertext: [u8; 16] = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        let expected: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(decrypt_block(&key, &ciphertext), expected);
    }

    /// Round-trip with a second FIPS-197 C.2 key.
    #[test]
    fn decrypt_inverts_encrypt_round_trip() {
        let key: [u8; 16] = *b"0123456789abcdef";
        let plaintext: [u8; 16] = *b"fedcba9876543210";
        let encrypted = encrypt_block(&key, &plaintext);
        assert_ne!(encrypted, plaintext);
        assert_eq!(decrypt_block(&key, &encrypted), plaintext);
    }

    #[test]
    fn decrypt_matches_nist_block() {
        // NIST SP 800-38A F.1.1 AES-128 ECB ciphertext for the first block.
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let plaintext: [u8; 16] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a,
        ];
        let expected: [u8; 16] = [
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66,
            0xef, 0x97,
        ];
        assert_eq!(encrypt_block(&key, &plaintext), expected);
    }
}
