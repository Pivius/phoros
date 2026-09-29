use crate::error::CodecError;

type CodecResult<T> = Result<T, CodecError>;

/// Bit-level writer for encoding compressed deltas into a pre-allocated buffer.
pub struct BitWriter<'a> {
    buf: &'a mut [u8],
    bit_pos: usize,
    byte_pos: usize,
}

impl<'a> BitWriter<'a> {
    /// Buffer must be large enough for the encoded output.\
    /// Maximum encoded size: `10 * ceil(|T| / 8) + 1` bytes.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            bit_pos: 0,
            byte_pos: 0,
        }
    }

    pub fn write_bit(&mut self, bit: bool) -> CodecResult<()> {
        if self.byte_pos >= self.buf.len() {
            return Err(CodecError::BitstreamOverflow);
        }
        if self.bit_pos == 0 {
            self.buf[self.byte_pos] = 0;
        }
        if bit {
            self.buf[self.byte_pos] |= 1 << self.bit_pos;
        }
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(())
    }

    /// Aligns to byte boundary, then writes.
    pub fn write_u8(&mut self, val: u8) -> CodecResult<()> {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        if self.byte_pos >= self.buf.len() {
            return Err(CodecError::BitstreamOverflow);
        }
        self.buf[self.byte_pos] = val;
        self.byte_pos += 1;
        Ok(())
    }

    pub fn bytes_written(&self) -> usize {
        if self.bit_pos > 0 {
            self.byte_pos + 1
        } else {
            self.byte_pos
        }
    }

    pub fn finish(self) -> usize {
        self.bytes_written()
    }
}

/// Bit-level reader for decoding compressed deltas.
pub struct BitReader<'a> {
    buf: &'a [u8],
    bit_pos: usize,
    byte_pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            bit_pos: 0,
            byte_pos: 0,
        }
    }

    pub fn read_bit(&mut self) -> CodecResult<bool> {
        if self.byte_pos >= self.buf.len() {
            return Err(CodecError::BitstreamUnderflow);
        }
        let val = (self.buf[self.byte_pos] >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(val != 0)
    }

    /// Aligns to byte boundary, then reads.
    pub fn read_u8(&mut self) -> CodecResult<u8> {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        if self.byte_pos >= self.buf.len() {
            return Err(CodecError::BitstreamUnderflow);
        }
        let val = self.buf[self.byte_pos];
        self.byte_pos += 1;
        Ok(val)
    }
}

/// Byte-masked sparse XOR encoding.
///
/// Partitions `delta` into 8-byte words. Zero words emit a `0` bit.
/// Non-zero words emit a `1` bit, an 8-bit byte mask, and only the
/// changed bytes. Returns the number of bytes written to `output`.
pub fn byte_masked_encode(delta: &[u8], output: &mut [u8]) -> CodecResult<usize> {
    let k = delta.len().div_ceil(8);
    let mut writer = BitWriter::new(output);

    for i in 0..k {
        let word_start = i * 8;
        let word_end = core::cmp::min(word_start + 8, delta.len());

        let mut word = [0u8; 8];
        word[..word_end - word_start].copy_from_slice(&delta[word_start..word_end]);

        if word == [0; 8] {
            writer.write_bit(false)?;
        } else {
            writer.write_bit(true)?;

            // Single pass: build mask and collect non-zero bytes.
            let mut mask = 0u8;
            let mut non_zero = [0u8; 8];
            let mut count = 0;
            for (j, &b) in word.iter().enumerate() {
                if b != 0 {
                    mask |= 1 << j;
                    non_zero[count] = b;
                    count += 1;
                }
            }
            writer.write_u8(mask)?;
            for &b in &non_zero[..count] {
                writer.write_u8(b)?;
            }
        }
    }

    Ok(writer.finish())
}

/// CRC16-CCITT (polynomial 0x1021, init 0xFFFF) with 256-entry lookup table.
const CRC16_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i = 0u16;
    while i < 256 {
        let mut crc = i << 8;
        let mut j = 0u8;
        while j < 8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

pub fn crc16(data: &[u8]) -> u16 {
    crc16_update(0xFFFF, data)
}

#[inline]
pub fn crc16_update(mut crc: u16, data: &[u8]) -> u16 {
    for &byte in data {
        crc = (crc << 8) ^ CRC16_TABLE[((crc >> 8) ^ byte as u16) as usize];
    }
    crc
}

/// Decode a byte-masked sparse XOR delta into `output`.
///
/// `output` must be pre-zeroed or contain the base state to XOR into.
pub fn byte_masked_decode(encoded: &[u8], output: &mut [u8]) -> CodecResult<()> {
    let k = output.len().div_ceil(8);
    let mut reader = BitReader::new(encoded);

    for i in 0..k {
        let word_start = i * 8;
        let word_end = core::cmp::min(word_start + 8, output.len());
        let word_len = word_end - word_start;

        let flag = reader.read_bit()?;

        if !flag {
            output[word_start..word_end].fill(0);
        } else {
            let mask = reader.read_u8()?;
            for j in 0..word_len {
                output[word_start + j] = if mask & (1 << j) != 0 {
                    reader.read_u8()?
                } else {
                    0
                };
            }
        }
    }

    Ok(())
}
