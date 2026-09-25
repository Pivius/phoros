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
#[repr(C)]
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

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < SLOT_HEADER_SIZE {
            return None;
        }
        // SAFETY: data.len() >= SLOT_HEADER_SIZE. repr(C), no padding.
        let header = unsafe { *(data.as_ptr() as *const SlotHeader) };
        if SlotType::from_u8(header.kind).is_none() {
            return None;
        }
        Some(header)
    }

    /// # Safety
    ///
    /// `dest` must have at least `SLOT_HEADER_SIZE` bytes available.
    pub unsafe fn write_to(&self, dest: &mut [u8]) {
        let src = self as *const Self as *const u8;
        std::ptr::copy_nonoverlapping(src, dest.as_mut_ptr(), SLOT_HEADER_SIZE);
    }
}

/// `(current + SLOT_HEADER_SIZE + payload_len) % data_area_len`
#[inline]
pub fn next_slot_offset(current_offset: u32, payload_len: u16, data_area_len: u32) -> u32 {
    let slot_size = SLOT_HEADER_SIZE as u32 + payload_len as u32;
    current_offset.wrapping_add(slot_size) % data_area_len
}
