pub const ARENA_MAGIC: u32 = 0x4250_5242;
pub const ARENA_VERSION: u16 = 0x0001;
pub const ARENA_HEADER_SIZE: usize = 32;

/// The 32-byte header at the start of a pre-allocated byte arena.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArenaHeader {
    pub magic: u32,
    pub version: u16,
    pub flags: u16,
    pub data_area_len: u32,
    pub head_offset: u32,
    pub tail_offset: u32,
    pub live_slot_count: u32,
    pub total_entries: u64,
}

impl ArenaHeader {
    pub fn new(data_area_len: u32) -> Self {
        Self {
            magic: ARENA_MAGIC,
            version: ARENA_VERSION,
            flags: 0,
            data_area_len,
            head_offset: 0,
            tail_offset: 0,
            live_slot_count: 0,
            total_entries: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == ARENA_MAGIC && self.version == ARENA_VERSION
    }

    /// Reinterpret the header as a fixed-size byte array.
    ///
    /// # Safety
    ///
    /// The returned slice must not outlive the header or be used after
    /// the header is moved.
    pub fn as_bytes(&self) -> &[u8; ARENA_HEADER_SIZE] {
        // SAFETY: ArenaHeader is repr(C) with size == ARENA_HEADER_SIZE,
        // containing only plain integer types with no padding.
        unsafe { &*(self as *const Self as *const [u8; ARENA_HEADER_SIZE]) }
    }

    /// Mutable variant of [`as_bytes`](Self::as_bytes).
    ///
    /// # Safety
    ///
    /// Same constraints as `as_bytes`. Callers must ensure no other
    /// references exist to this memory.
    pub fn as_bytes_mut(&mut self) -> &mut [u8; ARENA_HEADER_SIZE] {
        // SAFETY: same as as_bytes, but mutable.
        unsafe { &mut *(self as *mut Self as *mut [u8; ARENA_HEADER_SIZE]) }
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < ARENA_HEADER_SIZE {
            return None;
        }
        let b: &[u8; ARENA_HEADER_SIZE] = data[..ARENA_HEADER_SIZE].try_into().ok()?;
        let header = Self {
            magic: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            version: u16::from_le_bytes(b[4..6].try_into().unwrap()),
            flags: u16::from_le_bytes(b[6..8].try_into().unwrap()),
            data_area_len: u32::from_le_bytes(b[8..12].try_into().unwrap()),
            head_offset: u32::from_le_bytes(b[12..16].try_into().unwrap()),
            tail_offset: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            live_slot_count: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            total_entries: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        };
        if !header.is_valid() {
            return None;
        }
        Some(header)
    }
}

/// `(head_offset + logical_offset) % data_area_len`
#[inline]
pub fn physical_offset(head_offset: u32, logical_offset: u32, data_area_len: u32) -> u32 {
    head_offset.wrapping_add(logical_offset) % data_area_len
}

/// Free bytes between `tail_offset` and `head_offset` (circular distance).
pub fn available_space(head_offset: u32, tail_offset: u32, data_area_len: u32) -> u32 {
    if tail_offset >= head_offset {
        (data_area_len - tail_offset) + head_offset
    } else {
        head_offset - tail_offset
    }
}
