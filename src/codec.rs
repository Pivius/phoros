/// Bit-level writer for encoding compressed deltas into a pre-allocated buffer.
pub struct BitWriter<'a> {
	buf: &'a mut [u8],
	bit_pos: usize,
	byte_pos: usize,
}

impl<'a> BitWriter<'a> {
	/// Buffer must be large enough for the encoded output.\
	/// Maximum encoded size: `9 * ceil(|T| / 8)` bytes.
	pub fn new(buf: &'a mut [u8]) -> Self {
		Self {
			buf,
			bit_pos: 0,
			byte_pos: 0,
		}
	}

	/// # Panics
	///
	/// Panics if the buffer is full.
	pub fn write_bit(&mut self, bit: bool) {
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
	}

	/// Aligns to byte boundary, then writes.
	///
	/// # Panics
	///
	/// Panics if the buffer is full.
	pub fn write_u8(&mut self, val: u8) {
		if self.bit_pos != 0 {
			self.bit_pos = 0;
			self.byte_pos += 1;
		}
		self.buf[self.byte_pos] = val;
		self.byte_pos += 1;
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

	/// # Panics
	///
	/// Panics if the reader has exhausted the buffer.
	pub fn read_bit(&mut self) -> bool {
		let val = (self.buf[self.byte_pos] >> self.bit_pos) & 1;
		self.bit_pos += 1;
		if self.bit_pos == 8 {
			self.bit_pos = 0;
			self.byte_pos += 1;
		}
		val != 0
	}

	/// Aligns to byte boundary, then reads.
	///
	/// # Panics
	///
	/// Panics if the reader has exhausted the buffer.
	pub fn read_u8(&mut self) -> u8 {
		if self.bit_pos != 0 {
			self.bit_pos = 0;
			self.byte_pos += 1;
		}
		let val = self.buf[self.byte_pos];
		self.byte_pos += 1;
		val
	}
}

/// Byte-masked sparse XOR encoding.
///
/// Partitions `delta` into 8-byte words. Zero words emit a `0` bit.
/// Non-zero words emit a `1` bit, an 8-bit byte mask, and only the
/// changed bytes. Returns the number of bytes written to `output`.
pub fn byte_masked_encode(delta: &[u8], output: &mut [u8]) -> usize {
	let k = delta.len().div_ceil(8);
	let mut writer = BitWriter::new(output);

	for i in 0..k {
		let word_start = i * 8;
		let word_end = std::cmp::min(word_start + 8, delta.len());

		let mut word = [0u8; 8];
		word[..word_end - word_start].copy_from_slice(&delta[word_start..word_end]);

		let word_val = u64::from_le_bytes(word);

		if word_val == 0 {
			writer.write_bit(false);
		} else {
			writer.write_bit(true);

			let mut mask = 0u8;
			for (j, &b) in word.iter().enumerate() {
				if b != 0 {
					mask |= 1 << j;
				}
			}
			writer.write_u8(mask);

			for (j, &b) in word.iter().enumerate() {
				if mask & (1 << j) != 0 {
					writer.write_u8(b);
				}
			}
		}
	}

	writer.finish()
}

/// CRC16-CCITT (polynomial 0x1021, init 0xFFFF).
pub fn crc16(data: &[u8]) -> u16 {
	let mut crc: u16 = 0xFFFF;
	for &byte in data {
		crc ^= (byte as u16) << 8;
		for _ in 0..8 {
			if crc & 0x8000 != 0 {
				crc = (crc << 1) ^ 0x1021;
			} else {
				crc <<= 1;
			}
		}
	}
	crc
}

pub fn crc16_update(mut crc: u16, data: &[u8]) -> u16 {
	for &byte in data {
		crc ^= (byte as u16) << 8;
		for _ in 0..8 {
			if crc & 0x8000 != 0 {
				crc = (crc << 1) ^ 0x1021;
			} else {
				crc <<= 1;
			}
		}
	}
	crc
}

/// Decode a byte-masked sparse XOR delta into `output`.
///
/// `output` must be pre-zeroed or contain the base state to XOR into.
pub fn byte_masked_decode(encoded: &[u8], output: &mut [u8]) {
	let k = output.len().div_ceil(8);
	let mut reader = BitReader::new(encoded);

	for i in 0..k {
		let word_start = i * 8;
		let word_end = std::cmp::min(word_start + 8, output.len());

		let flag = reader.read_bit();

		if !flag {
			for b in &mut output[word_start..word_end] {
				*b = 0;
			}
		} else {
			let mask = reader.read_u8();

			let mut word = [0u8; 8];
			for (j, slot) in word.iter_mut().enumerate() {
				if mask & (1 << j) != 0 {
					*slot = reader.read_u8();
				}
			}

			let copy_len = word_end - word_start;
			output[word_start..word_end].copy_from_slice(&word[..copy_len]);
		}
	}
}
