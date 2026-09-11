//! Minimal big-endian bit reader used by the H.264 SPS parser.
//!
//! Only the operations needed to decode a Sequence Parameter Set are
//! implemented: fixed-width reads and the Exp-Golomb codes (ue/se).

use anyhow::{Result, bail};

pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub fn read_bit(&mut self) -> Result<u32> {
        if self.bit_pos >= self.data.len() * 8 {
            bail!("SPS bitstream exhausted");
        }
        let byte = self.data[self.bit_pos / 8];
        let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
        self.bit_pos += 1;
        Ok(u32::from(bit))
    }

    pub fn read_bits(&mut self, count: u32) -> Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()?;
        }
        Ok(value)
    }

    /// Unsigned Exp-Golomb code (`ue(v)`).
    pub fn read_ue(&mut self) -> Result<u32> {
        let mut leading_zeros = 0u32;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                bail!("invalid Exp-Golomb code (too many leading zeros)");
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let rest = self.read_bits(leading_zeros)?;
        Ok((1u32 << leading_zeros) - 1 + rest)
    }

    /// Signed Exp-Golomb code (`se(v)`).
    pub fn read_se(&mut self) -> Result<i32> {
        let code = self.read_ue()?;
        let value = code.div_ceil(2) as i32;
        Ok(if code % 2 == 1 { value } else { -value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exp_golomb_values() {
        // 0b1 -> ue=0; 0b010 -> ue=1; 0b011 -> ue=2; 0b00100 -> ue=3
        let data = [0b1010_0110, 0b0100_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_ue().unwrap(), 0);
        assert_eq!(r.read_ue().unwrap(), 1);
        assert_eq!(r.read_ue().unwrap(), 2);
        assert_eq!(r.read_ue().unwrap(), 3);
    }

    #[test]
    fn reads_fixed_width_bits() {
        let data = [0b1011_0010, 0b0101_1111];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(5).unwrap(), 0b10010);
        assert_eq!(r.read_bits(8).unwrap(), 0b0101_1111);
    }
}
