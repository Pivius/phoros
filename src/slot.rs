pub const SLOT_HEADER_SIZE: usize = 16;
pub const SLOT_TYPE_FULL_SNAPSHOT: u8 = 0x01;
pub const SLOT_TYPE_DELTA: u8 = 0x02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SlotType {
	FullSnapshot = SLOT_TYPE_FULL_SNAPSHOT,
	Delta = SLOT_TYPE_DELTA,
}

impl SlotType {
	pub fn from_u8(val: u8) -> Option<Self> {
		match val {
			SLOT_TYPE_FULL_SNAPSHOT => Some(SlotType::FullSnapshot),
			SLOT_TYPE_DELTA => Some(SlotType::Delta),
			_ => None,
		}
	}
}

/// 16-byte header at the start of each slot in the arena.
///
/// Serialized via [`to_bytes`](SlotHeader::to_bytes)
/// so the layout is independent of `repr(C)` padding and alignment.
#[derive(Debug, Clone, Copy)]
pub struct SlotHeader {
	pub frame: u64,
	pub kind: u8,
	pub payload_len: u16,
	pub checksum: u16,
	pub reserved: u8,
}

impl SlotHeader {
	#[inline]
	pub fn total_size(&self) -> usize {
		SLOT_HEADER_SIZE + self.payload_len as usize
	}

	/// Serialize into 16 little-endian bytes.
	pub fn to_bytes(&self) -> [u8; SLOT_HEADER_SIZE] {
		let mut b = [0u8; SLOT_HEADER_SIZE];
		b[0..8].copy_from_slice(&self.frame.to_le_bytes());
		b[8] = self.kind;
		b[9..11].copy_from_slice(&self.payload_len.to_le_bytes());
		b[11..13].copy_from_slice(&self.checksum.to_le_bytes());
		b[13] = self.reserved;
		b
	}

	/// Deserialize from at least 16 bytes. Returns `None` on short input or an
	/// invalid `kind` discriminant.
	pub fn from_bytes(data: &[u8]) -> Option<Self> {
		let bytes: &[u8; SLOT_HEADER_SIZE] = data.get(..SLOT_HEADER_SIZE)?.try_into().ok()?;

		let kind = bytes[8];
		SlotType::from_u8(kind)?;

		Some(Self {
			frame: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
			kind,
			payload_len: u16::from_le_bytes(bytes[9..11].try_into().unwrap()),
			checksum: u16::from_le_bytes(bytes[11..13].try_into().unwrap()),
			reserved: bytes[13],
		})
	}
}

/// `(current + SLOT_HEADER_SIZE + payload_len) % data_area_len`
#[inline]
pub fn next_slot_offset(current_offset: u32, payload_len: u16, data_area_len: u32) -> u32 {
	let slot_size = SLOT_HEADER_SIZE as u32 + payload_len as u32;
	current_offset.wrapping_add(slot_size) % data_area_len
}
