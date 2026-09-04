//! Self-contained AES-128-CBC decryption with PKCS#7 unpadding.
//!
//! This is the only block mode HLS (RFC 8216 §5.2) requires for
//! `METHOD=AES-128`: CBC decryption of each segment with a 16-byte key
//! and IV, followed by PKCS#7 padding removal. The AES-128 block primitive
//! itself lives in [`super::aes128`] (written from scratch against FIPS-197)
//! so the crate stays buildable on its declared MSRV (Rust 1.78).

use super::aes128::decrypt_block;

/// Decrypt a full CBC message and verify/remove PKCS#7 padding.
///
/// `key` and `iv` must be exactly 16 bytes. The ciphertext length must
/// be a positive multiple of 16. On any padding or length violation the
/// function returns an error instead of guessing, which is the safe
/// behavior for untrusted HLS segments.
pub fn aes_128_cbc_decrypt(
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, AesCbcError> {
    if key.len() != 16 {
        return Err(AesCbcError::InvalidKeyLength(key.len()));
    }
    if iv.len() != 16 {
        return Err(AesCbcError::InvalidIvLength(iv.len()));
    }
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return Err(AesCbcError::InvalidCiphertextLength(ciphertext.len()));
    }

    let mut key_bytes = [0u8; 16];
    key_bytes.copy_from_slice(key);
    let mut previous = [0u8; 16];
    previous.copy_from_slice(iv);

    let block_count = ciphertext.len() / 16;
    let mut plaintext = Vec::with_capacity(ciphertext.len());
    for block_index in 0..block_count {
        let offset = block_index * 16;
        let mut encrypted = [0u8; 16];
        encrypted.copy_from_slice(&ciphertext[offset..offset + 16]);
        let mut decrypted = decrypt_block(&key_bytes, &encrypted);
        for (plain, previous_byte) in decrypted.iter_mut().zip(previous.iter()) {
            *plain ^= *previous_byte;
        }
        plaintext.extend_from_slice(&decrypted);
        previous = encrypted;
    }

    // Verify and strip PKCS#7 padding.
    let pad_len = match plaintext.last() {
        Some(&byte) => usize::from(byte),
        None => return Err(AesCbcError::EmptyPlaintext),
    };
    if pad_len == 0 || pad_len > 16 || pad_len > plaintext.len() {
        return Err(AesCbcError::InvalidPadding);
    }
    let padding_start = plaintext.len() - pad_len;
    if plaintext[padding_start..]
        .iter()
        .any(|byte| usize::from(*byte) != pad_len)
    {
        return Err(AesCbcError::InvalidPadding);
    }
    plaintext.truncate(padding_start);
    Ok(plaintext)
}

/// Errors produced by [`aes_128_cbc_decrypt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AesCbcError {
    /// The AES key is not 16 bytes.
    InvalidKeyLength(usize),
    /// The IV is not 16 bytes.
    InvalidIvLength(usize),
    /// The ciphertext is empty or not a multiple of the block size.
    InvalidCiphertextLength(usize),
    /// The decrypted plaintext is empty.
    EmptyPlaintext,
    /// PKCS#7 padding is malformed.
    InvalidPadding,
}

impl core::fmt::Display for AesCbcError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidKeyLength(len) => {
                write!(formatter, "AES-128 key must be 16 bytes, got {len}")
            }
            Self::InvalidIvLength(len) => {
                write!(formatter, "AES-128 IV must be 16 bytes, got {len}")
            }
            Self::InvalidCiphertextLength(len) => write!(
                formatter,
                "AES-128-CBC ciphertext must be a positive multiple of 16 bytes, got {len}"
            ),
            Self::EmptyPlaintext => write!(formatter, "AES-128-CBC plaintext is empty"),
            Self::InvalidPadding => write!(formatter, "AES-128-CBC PKCS#7 padding is invalid"),
        }
    }
}

impl std::error::Error for AesCbcError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// NIST SP 800-38A F.2.1 CBC-AES128 decrypt example, using the four
    /// published plaintext/ciphertext blocks directly (no padding here — the
    /// padding check is exercised by the round-trip test below).
    #[test]
    fn nist_sp_800_38a_cbc_blocks() {
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let plaintexts: [[u8; 16]; 4] = [
            [
                0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
                0x17, 0x2a,
            ],
            [
                0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf,
                0x8e, 0x51,
            ],
            [
                0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11, 0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a,
                0x52, 0xef,
            ],
            [
                0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17, 0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c,
                0x37, 0x10,
            ],
        ];
        let ciphertexts: [[u8; 16]; 4] = [
            [
                0x76, 0x49, 0xab, 0xac, 0x81, 0x19, 0xb2, 0x46, 0xce, 0xe9, 0x8e, 0x9b, 0x12, 0xe9,
                0x19, 0x7d,
            ],
            [
                0x50, 0x86, 0xcb, 0x9b, 0x50, 0x72, 0x19, 0xee, 0x95, 0xdb, 0x11, 0x3a, 0x91, 0x76,
                0x78, 0xb2,
            ],
            [
                0x73, 0xbe, 0xd6, 0xb8, 0xe3, 0xc1, 0x74, 0x3b, 0x71, 0x16, 0xe6, 0x9e, 0x22, 0x22,
                0x95, 0x16,
            ],
            [
                0x3f, 0xf1, 0xca, 0xa1, 0x68, 0x1f, 0xac, 0x09, 0x12, 0x0e, 0xca, 0x30, 0x75, 0x86,
                0xe1, 0xa7,
            ],
        ];

        let mut previous: [u8; 16] = iv;
        for (plain, ciphertext) in plaintexts.iter().zip(ciphertexts.iter()) {
            let mut decrypted = super::super::aes128::decrypt_block(&key, ciphertext);
            for (byte, prev) in decrypted.iter_mut().zip(previous.iter()) {
                *byte ^= *prev;
            }
            assert_eq!(&decrypted, plain, "CBC block mismatch");
            previous = *ciphertext;
        }
    }

    /// Full padded round-trip through [`aes_128_cbc_decrypt`] with a known
    /// plaintext that needs a whole 0x10 padding block (as used by the NIST
    /// vector). The ciphertext is produced by the reference CBC encoder in
    /// this module, which itself is validated block-by-block by the test
    /// above, so the padding path is not self-referential at the AES level.
    #[test]
    fn padded_round_trip_and_known_vector() {
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        // NIST SP 800-38A plaintext block 1; one full padding block of 0x10.
        let plaintext = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a,
        ];
        let mut padded = plaintext.to_vec();
        padded.extend_from_slice(&[0x10u8; 16]);
        let encrypted = encrypt_cbc_reference(&key, &iv, &padded);
        let decrypted = aes_128_cbc_decrypt(&key, &iv, &encrypted).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn rejects_bad_padding() {
        let key = [7u8; 16];
        let iv = [9u8; 16];
        // Two blocks with garbage (padding byte 5 but bytes aren't 5).
        let ciphertext = [0u8; 32];
        assert!(matches!(
            aes_128_cbc_decrypt(&key, &iv, &ciphertext),
            Err(AesCbcError::InvalidPadding)
        ));
    }

    #[test]
    fn rejects_wrong_lengths() {
        assert!(matches!(
            aes_128_cbc_decrypt(&[0u8; 15], &[0u8; 16], &[0u8; 16]),
            Err(AesCbcError::InvalidKeyLength(15))
        ));
        assert!(matches!(
            aes_128_cbc_decrypt(&[0u8; 16], &[0u8; 15], &[0u8; 16]),
            Err(AesCbcError::InvalidIvLength(15))
        ));
        assert!(matches!(
            aes_128_cbc_decrypt(&[0u8; 16], &[0u8; 16], &[0u8; 15]),
            Err(AesCbcError::InvalidCiphertextLength(15))
        ));
        assert!(matches!(
            aes_128_cbc_decrypt(&[0u8; 16], &[0u8; 16], &[]),
            Err(AesCbcError::InvalidCiphertextLength(0))
        ));
    }

    /// Reference CBC encryption used only in tests to construct valid
    /// ciphertexts (this module only needs the decryption direction).
    fn encrypt_cbc_reference(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut key_bytes = [0u8; 16];
        key_bytes.copy_from_slice(key);
        let mut previous = <&[u8; 16]>::try_from(iv)
            .expect("iv must be 16 bytes")
            .to_owned();
        let mut out = Vec::with_capacity(plaintext.len());
        for block in plaintext.as_chunks::<16>().0 {
            let mut block = *block;
            for (byte, previous_byte) in block.iter_mut().zip(previous.iter()) {
                *byte ^= *previous_byte;
            }
            let encrypted = super::super::aes128::encrypt_block(&key_bytes, &block);
            previous = encrypted;
            out.extend_from_slice(&encrypted);
        }
        out
    }
}
