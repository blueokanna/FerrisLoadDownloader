//! Self-contained cryptographic primitives (SHA-256, AES-128-CBC) with
//! no unsafe code and no third-party dependencies beyond the audited
//! `aes` block cipher, used to verify downloaded binaries and decrypt
//! HLS AES-128 segments.

pub mod aes_cbc;
pub mod sha256;

pub use aes_cbc::{aes_128_cbc_decrypt, AesCbcError};
pub use sha256::{sha256, sha256_hex, Sha256};
