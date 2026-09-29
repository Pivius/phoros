pub const SLOT_HEADER_SIZE: usize = 16;
pub const SLOT_TYPE_FULL_SNAPSHOT: u8 = 0x01;
pub const SLOT_TYPE_DELTA: u8 = 0x02;

/// Classification of a slot in the arena.
///
/// # Examples
///
/// ```
/// use phoros::SlotType;
///
/// assert_eq!(SlotType::from_u8(0x01), Some(SlotType::FullSnapshot));
/// assert_eq!(SlotType::from_u8(0x02), Some(SlotType::Delta));
/// assert_eq!(SlotType::from_u8(0xFF), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SlotType {
    /// A complete state snapshot.
    FullSnapshot = SLOT_TYPE_FULL_SNAPSHOT,
    /// A compressed XOR delta from the previous frame.
    Delta = SLOT_TYPE_DELTA,
}

impl SlotType {
    /// Convert a raw byte to a `SlotType`.
    ///
    /// Returns `None` for unknown discriminants.
    #[inline]
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
/// Uses `#[repr(C)]` with explicit layout.
///
/// ```text
/// Offset  Size  Field
/// ──────  ────  ─────────────
///   0       8   frame
///   8       2   payload_len
///  10       2   checksum
///  12       1   kind
///  13       3   reserved
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SlotHeader {
    pub frame: u64,
    pub payload_len: u16,
    pub checksum: u16,
    pub kind: u8,
    pub reserved: [u8; 3],
}

impl SlotHeader {
    #[inline]
    pub fn total_size(&self) -> usize {
        SLOT_HEADER_SIZE + self.payload_len as usize
    }

    /// Serialize into 16 bytes
    #[inline]
    pub fn to_bytes(&self) -> [u8; SLOT_HEADER_SIZE] {
        let mut b = [0u8; SLOT_HEADER_SIZE];
        b[0..8].copy_from_slice(&self.frame.to_le_bytes());
        b[8..10].copy_from_slice(&self.payload_len.to_le_bytes());
        b[10..12].copy_from_slice(&self.checksum.to_le_bytes());
        b[12] = self.kind;
        b[13..16].copy_from_slice(&self.reserved);
        b
    }

    /// Deserialize from at least 16 bytes. Returns `None` on short input or an
    /// invalid `kind` discriminant.
    #[inline]
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let bytes: &[u8; SLOT_HEADER_SIZE] = data.get(..SLOT_HEADER_SIZE)?.try_into().ok()?;

        let kind = bytes[12];
        SlotType::from_u8(kind)?;

        Some(Self {
            frame: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            payload_len: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            checksum: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
            kind,
            reserved: [bytes[13], bytes[14], bytes[15]],
        })
    }
}

/// `(current + SLOT_HEADER_SIZE + payload_len) % data_area_len`
#[inline]
pub fn next_slot_offset(current_offset: u32, payload_len: u16, data_area_len: u32) -> u32 {
    let slot_size = SLOT_HEADER_SIZE as u32 + payload_len as u32;
    current_offset.wrapping_add(slot_size) % data_area_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout() {
        assert_eq!(core::mem::size_of::<SlotHeader>(), 16);
        assert_eq!(core::mem::align_of::<SlotHeader>(), 8);

        let h = SlotHeader {
            frame: 0x0102030405060708,
            payload_len: 0x090A,
            checksum: 0x0B0C,
            kind: 0x0D,
            reserved: [0x0E, 0x0F, 0x00],
        };
        let b = h.to_bytes();

        assert_eq!(b[0..8], h.frame.to_le_bytes());
        assert_eq!(b[8..10], h.payload_len.to_le_bytes());
        assert_eq!(b[10..12], h.checksum.to_le_bytes());
        assert_eq!(b[12], 0x0D);
        assert_eq!(b[13..16], [0x0E, 0x0F, 0x00]);
    }

    #[test]
    fn roundtrip() {
        let h = SlotHeader {
            frame: 12345,
            payload_len: 42,
            checksum: 0xABCD,
            kind: SLOT_TYPE_FULL_SNAPSHOT,
            reserved: [1, 2, 3],
        };
        let b = h.to_bytes();
        let h2 = SlotHeader::from_bytes(&b).unwrap();
        assert_eq!(h.frame, h2.frame);
        assert_eq!(h.payload_len, h2.payload_len);
        assert_eq!(h.checksum, h2.checksum);
        assert_eq!(h.kind, h2.kind);
        assert_eq!(h.reserved, h2.reserved);
    }

    #[test]
    fn kind_roundtrip() {
        assert!(SlotHeader::from_bytes(&[0u8; 16]).is_none()); // kind=0 is invalid
        let mut b = [0u8; 16];
        b[12] = SLOT_TYPE_FULL_SNAPSHOT;
        assert!(SlotHeader::from_bytes(&b).is_some());
        b[12] = SLOT_TYPE_DELTA;
        assert!(SlotHeader::from_bytes(&b).is_some());
    }
}
