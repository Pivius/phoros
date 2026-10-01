pub mod construct;
pub mod iterate;
pub mod persist;

use core::marker::PhantomData;

use crate::{
    arena::ArenaHeader,
    codec,
    error::{BufferError, RollbackError},
    index::AnchorIndex,
    slot::{SLOT_HEADER_SIZE, SlotHeader, SlotType, next_slot_offset},
    storage::ArenaStorage,
};

use super::{DEFAULT_ANCHOR_INTERVAL, DEFAULT_DELTA_THRESHOLD};

#[inline]
fn arena_read_into(arena: &[u8], off: u32, buf: &mut [u8]) {
    let data_len = arena.len();
    if data_len == 0 || buf.is_empty() {
        return;
    }
    let start = (off as usize) % data_len;
    let first = core::cmp::min(buf.len(), data_len - start);
    let rest = buf.len() - first;
    buf[..first].copy_from_slice(&arena[start..start + first]);
    if rest > 0 {
        buf[first..].copy_from_slice(&arena[..rest]);
    }
}

#[inline]
fn arena_write_to(arena: &mut [u8], off: u32, bytes: &[u8]) {
    let data_len = arena.len();
    if data_len == 0 || bytes.is_empty() {
        return;
    }
    let start = (off as usize) % data_len;
    let first = core::cmp::min(bytes.len(), data_len - start);
    arena[start..start + first].copy_from_slice(&bytes[..first]);
    if bytes.len() > first {
        arena[..bytes.len() - first].copy_from_slice(&bytes[first..]);
    }
}

#[inline]
fn compute_checksum(entry: u64, kind: u8, payload: &[u8]) -> u16 {
    let mut crc = 0xFFFF;
    crc = codec::crc16_update(crc, &entry.to_le_bytes());
    crc = codec::crc16_update(crc, &[kind]);
    crc = codec::crc16_update(crc, &(payload.len() as u16).to_le_bytes());
    crc = codec::crc16_update(crc, payload);
    crc
}

#[inline]
fn read_header_at(arena: &[u8], off: u32) -> Option<SlotHeader> {
    let mut buf = [0u8; SLOT_HEADER_SIZE];
    arena_read_into(arena, off, &mut buf);
    SlotHeader::from_bytes(&buf)
}

#[inline]
fn xor_into(dst: &mut [u8], src: &[u8]) {
    let n = core::cmp::min(dst.len(), src.len());
    let full = n / 8;
    for i in 0..full {
        let off = i * 8;
        let d = u64::from_le_bytes(dst[off..off + 8].try_into().unwrap());
        let s = u64::from_le_bytes(src[off..off + 8].try_into().unwrap());
        dst[off..off + 8].copy_from_slice(&(d ^ s).to_le_bytes());
    }
    for i in (full * 8)..n {
        dst[i] ^= src[i];
    }
}

/// Zero-allocation state history.
///
/// Records state snapshots and byte-masked deltas into a pre-allocated
/// arena. The arena storage is generic over [`ArenaStorage`].
///
/// `T` must be `Copy + Sized + Send + 'static` with no drop logic.
///
/// # Examples
///
/// ```
/// use phoros::bprb;
///
/// let mut buf = bprb!(u64 => stack(1024)).unwrap();
/// buf.snapshot(&1u64).unwrap();
/// buf.snapshot(&2u64).unwrap();
/// assert_eq!(buf.rollback_to(0).unwrap(), 1u64);
/// ```
#[derive(Debug)]
pub struct BPRB<T, S: ArenaStorage, const STATE_SIZE: usize, const MAX_ENCODED: usize> {
    storage: S,
    header: ArenaHeader,
    anchor_index: AnchorIndex,
    current_head_state: Option<T>,
    entry_counter: u64,
    anchor_interval: u64,
    delta_threshold: f64,
    used_bytes: u32,
    diverged: bool,
    _marker: PhantomData<T>,
}

impl<T, S: ArenaStorage, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    BPRB<T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    const _ENSURE_STATE_SIZE: () = assert!(
        STATE_SIZE == core::mem::size_of::<T>(),
        "STATE_SIZE must equal size_of::<T>()"
    );

    fn validate_type() -> Result<(), BufferError> {
        if core::mem::needs_drop::<T>() {
            return Err(BufferError::InvalidState);
        }
        Ok(())
    }

    fn from_storage(storage: S, anchor_interval: u64) -> Result<Self, BufferError> {
        let () = Self::_ENSURE_STATE_SIZE;
        Self::validate_type()?;

        let min_slot_size = SLOT_HEADER_SIZE + STATE_SIZE;
        if storage.len() < min_slot_size {
            return Err(BufferError::ArenaFull);
        }

        let header = ArenaHeader::new(storage.len() as u32);

        Ok(Self {
            storage,
            header,
            anchor_index: AnchorIndex::new(),
            current_head_state: None,
            entry_counter: 0,
            anchor_interval,
            delta_threshold: DEFAULT_DELTA_THRESHOLD,
            used_bytes: 0,
            diverged: false,
            _marker: PhantomData,
        })
    }

    /// Set the delta threshold.
    ///
    /// When a delta exceeds `threshold * STATE_SIZE` bytes, a
    /// snapshot is stored instead.
    pub fn with_delta_threshold(mut self, threshold: f64) -> Self {
        self.delta_threshold = threshold;
        self
    }

    /// Record `state` as a new entry.
    ///
    /// Stores a snapshot on the first entry, every `anchor_interval`
    /// entries, or when the delta exceeds `delta_threshold`.
    /// Stores the delta otherwise.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::Codec`] if the internal bitstream encoder overflows.
    pub fn snapshot(&mut self, state: &T) -> Result<(), BufferError> {
        let entry = self.entry_counter;
        let state_bytes = Self::state_as_bytes(state);

        let mut buf_delta = [0u8; STATE_SIZE];
        let mut buf_encoded = [0u8; MAX_ENCODED];

        let mut kind;
        let mut payload_len;

        if self.should_store_snapshot(state_bytes, entry) {
            kind = SlotType::FullSnapshot;
            payload_len = STATE_SIZE;
        } else {
            let current = Self::state_as_bytes(
                self.current_head_state
                    .as_ref()
                    .expect("current_head_state must be set after first snapshot"),
            );
            for (d, (s, c)) in buf_delta
                .iter_mut()
                .zip(state_bytes.iter().zip(current.iter()))
            {
                *d = *s ^ *c;
            }

            let encoded_len = codec::byte_masked_encode(&buf_delta, &mut buf_encoded)?;

            if encoded_len > ((STATE_SIZE as f64 * self.delta_threshold) as usize) {
                kind = SlotType::FullSnapshot;
                payload_len = STATE_SIZE;
            } else {
                kind = SlotType::Delta;
                payload_len = encoded_len;
            }
        };

        self.ensure_space((SLOT_HEADER_SIZE + payload_len) as u32);

        if kind == SlotType::Delta && self.anchor_index.is_empty() {
            kind = SlotType::FullSnapshot;
            payload_len = STATE_SIZE;
            self.ensure_space((SLOT_HEADER_SIZE + STATE_SIZE) as u32);
        }

        let payload_ref: &[u8] = if kind == SlotType::Delta {
            &buf_encoded[..payload_len]
        } else {
            state_bytes
        };

        let checksum = compute_checksum(entry, kind as u8, payload_ref);
        let slot_header = SlotHeader {
            entry,
            kind: kind as u8,
            payload_len: payload_len as u16,
            checksum,
            reserved: [0; 3],
        };
        let header_bytes = slot_header.to_bytes();

        let start_off = self.header.tail_offset;
        arena_write_to(self.storage.as_mut_slice(), start_off, &header_bytes);
        arena_write_to(
            self.storage.as_mut_slice(),
            start_off + SLOT_HEADER_SIZE as u32,
            payload_ref,
        );

        let slot_size = SLOT_HEADER_SIZE as u32 + payload_len as u32;
        self.header.tail_offset = start_off.wrapping_add(slot_size) % self.header.data_area_len;
        self.header.live_slot_count += 1;
        self.header.total_entries += 1;
        self.used_bytes += slot_size;

        if kind == SlotType::FullSnapshot {
            self.anchor_index.insert(entry, start_off);
            self.diverged = false;
        }

        self.current_head_state = Some(*state);
        self.entry_counter += 1;

        Ok(())
    }

    /// Read the state at `target_entry`.
    ///
    /// Finds the nearest anchor near `target_entry`, then walks forward.
    ///
    /// # Errors
    ///
    /// - [`RollbackError::EntryEvicted`] if `target_entry` is out of range
    /// - [`RollbackError::CorruptedChain`] if a slot header is invalid, a
    ///   payload length is inconsistent, or the delta chain is broken.
    /// - [`RollbackError::ArenaCorrupted`] if the arena header is unreadable.
    /// - [`RollbackError::Codec`] if the internal bitstream decoder overflows.
    pub fn read_entry(&self, target_entry: u64) -> Result<T, RollbackError> {
        if target_entry > self.entry_counter.saturating_sub(1) {
            return Err(RollbackError::EntryEvicted);
        }

        let anchor = self
            .anchor_index
            .find_nearest_le(target_entry)
            .ok_or(RollbackError::EntryEvicted)?;

        let mut buf_encoded = [0u8; MAX_ENCODED];

        let anchor_header = self.read_slot_at(anchor.offset(), &mut buf_encoded)?;
        if anchor_header.payload_len as usize != STATE_SIZE {
            return Err(RollbackError::CorruptedChain);
        }

        let mut working = [0u8; STATE_SIZE];
        working[..anchor_header.payload_len as usize]
            .copy_from_slice(&buf_encoded[..anchor_header.payload_len as usize]);

        let mut offset = next_slot_offset(
            anchor.offset(),
            anchor_header.payload_len,
            self.header.data_area_len,
        );

        let mut buf_delta = [0u8; STATE_SIZE];

        for expected_entry in (anchor.entry() + 1)..=target_entry {
            let header = self.read_slot_at(offset, &mut buf_encoded)?;

            if header.entry != expected_entry {
                return Err(RollbackError::CorruptedChain);
            }

            match SlotType::from_u8(header.kind) {
                Some(SlotType::FullSnapshot) => {
                    if header.payload_len as usize != STATE_SIZE {
                        return Err(RollbackError::CorruptedChain);
                    }
                    working[..header.payload_len as usize]
                        .copy_from_slice(&buf_encoded[..header.payload_len as usize]);
                }
                Some(SlotType::Delta) => {
                    if header.payload_len as usize > MAX_ENCODED {
                        return Err(RollbackError::CorruptedChain);
                    }
                    buf_delta.fill(0);
                    codec::byte_masked_decode(
                        &buf_encoded[..header.payload_len as usize],
                        &mut buf_delta,
                    )?;
                    xor_into(&mut working, &buf_delta);
                }
                None => return Err(RollbackError::CorruptedChain),
            }

            offset = next_slot_offset(offset, header.payload_len, self.header.data_area_len);
        }

        Ok(Self::bytes_to_state(&working))
    }

    /// Reconstruct the state at `target_entry` by [`read_entry`](Self::read_entry).
    /// setting the head to the entry position.
    pub fn rollback_to(&mut self, target_entry: u64) -> Result<T, RollbackError> {
        let result = self.read_entry(target_entry)?;

        self.current_head_state = Some(result);
        if target_entry != self.entry_counter.saturating_sub(1) {
            self.diverged = true;
        }

        Ok(result)
    }

    /// Reconstruct the state at `entry_idx`, walk into it, update the
    /// internal head state, and return the result.
    ///
    /// # Errors
    ///
    /// - [`RollbackError::CorruptedChain`] if `delta.len() != STATE_SIZE` or
    ///   if the underlying `rollback_to` fails.
    /// - Other variants as documented on [`rollback_to`](Self::rollback_to).
    pub fn apply_delta(&mut self, entry_idx: u64, delta: &[u8]) -> Result<T, RollbackError> {
        if delta.len() != STATE_SIZE {
            return Err(RollbackError::CorruptedChain);
        }

        let mut state = self.rollback_to(entry_idx)?;

        let state_bytes = Self::state_as_bytes_mut(&mut state);
        xor_into(state_bytes, delta);

        self.current_head_state = Some(state);
        self.diverged = true;

        Ok(state)
    }

    /// Returns the current number of entries.
    #[inline]
    pub fn current_entry(&self) -> u64 {
        self.entry_counter
    }

    /// Returns the index of the oldest slot, or `None` if empty.
    pub fn oldest_entry(&self) -> Option<u64> {
        if self.entry_counter == 0 || self.header.live_slot_count == 0 {
            return None;
        }
        read_header_at(self.storage.as_slice(), self.header.head_offset).map(|h| h.entry)
    }

    /// Returns the index of the newest slot, or `None` if empty.
    pub fn newest_entry(&self) -> Option<u64> {
        if self.entry_counter == 0 {
            return None;
        }
        Some(self.entry_counter - 1)
    }

    /// Returns the number of live (unevicted) slots in the arena.
    #[inline]
    pub fn len(&self) -> usize {
        self.header.live_slot_count as usize
    }

    /// Returns `true` if no entries have been recorded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entry_counter == 0
    }

    /// Returns the total memory usage in bytes, arena + struct overhead.
    pub fn memory_usage(&self) -> usize {
        self.storage.len() + core::mem::size_of::<Self>()
    }

    /// Borrow the arena storage immutably.
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Borrow the arena storage mutably.
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    // Private

    #[inline]
    fn free_space(&self) -> u32 {
        self.header.data_area_len.saturating_sub(self.used_bytes)
    }

    fn ensure_space(&mut self, need: u32) {
        while self.free_space() < need && self.header.live_slot_count > 0 {
            self.evict_oldest_segment();
        }
    }

    fn read_slot_at(
        &self,
        offset: u32,
        payload_buf: &mut [u8],
    ) -> Result<SlotHeader, RollbackError> {
        let header =
            read_header_at(self.storage.as_slice(), offset).ok_or(RollbackError::ArenaCorrupted)?;

        let payload_len = header.payload_len as usize;
        if payload_len > payload_buf.len() {
            return Err(RollbackError::ArenaCorrupted);
        }
        arena_read_into(
            self.storage.as_slice(),
            offset + SLOT_HEADER_SIZE as u32,
            &mut payload_buf[..payload_len],
        );

        let checksum = compute_checksum(header.entry, header.kind, &payload_buf[..payload_len]);
        if checksum != header.checksum {
            return Err(RollbackError::CorruptedChain);
        }

        Ok(header)
    }

    #[inline]
    fn should_store_snapshot(&self, _state_bytes: &[u8], entry: u64) -> bool {
        if self.current_head_state.is_none() {
            return true;
        }
        if self.anchor_index.is_empty() {
            return true;
        }
        if self.diverged {
            return true;
        }
        if entry.is_multiple_of(self.anchor_interval) {
            return true;
        }
        false
    }

    /// Evict the oldest segment, the head slot plus any trailing entries, stopping
    /// just before the next anchor.
    fn evict_oldest_segment(&mut self) {
        if self.header.live_slot_count == 0 {
            return;
        }
        let data_len = self.header.data_area_len;
        let tail = self.header.tail_offset;
        let mut off = self.header.head_offset;

        let mut count = 0u32;
        let mut freed = 0u32;

        while let Some(header) = read_header_at(self.storage.as_slice(), off) {
            if count > 0 && header.kind == SlotType::FullSnapshot as u8 {
                break;
            }
            freed += SLOT_HEADER_SIZE as u32 + header.payload_len as u32;
            count += 1;

            off = next_slot_offset(off, header.payload_len, data_len);
            if off == tail {
                break;
            }
        }

        // If the loop ended because a header was corrupted, don't
        // cannot trust slot boundaries.
        if read_header_at(self.storage.as_slice(), off).is_none() && count > 0 {
            self.header.head_offset = tail;
            self.header.live_slot_count = 0;
            self.used_bytes = 0;
            self.anchor_index = AnchorIndex::new();
            return;
        }

        if count == 0 {
            return;
        }

        self.header.head_offset = off;
        self.header.live_slot_count -= count;
        self.used_bytes -= freed;

        let boundary = if self.header.live_slot_count == 0 {
            u64::MAX
        } else {
            read_header_at(self.storage.as_slice(), off)
                .map(|h| h.entry)
                .unwrap_or(u64::MAX)
        };
        self.anchor_index.evict_before(boundary);
    }

    #[inline]
    fn state_as_bytes(state: &T) -> &[u8] {
        // SAFETY: T is Copy + Sized + needs_drop == false.
        unsafe { core::slice::from_raw_parts(state as *const T as *const u8, STATE_SIZE) }
    }

    #[inline]
    fn state_as_bytes_mut(state: &mut T) -> &mut [u8] {
        // SAFETY: T is Copy + Sized + needs_drop == false.
        unsafe { core::slice::from_raw_parts_mut(state as *mut T as *mut u8, STATE_SIZE) }
    }

    #[inline]
    fn bytes_to_state(bytes: &[u8]) -> T {
        core::debug_assert_eq!(bytes.len(), STATE_SIZE);
        // SAFETY: T is Copy + no-drop
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const T) }
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct S {
        x: u64,
        y: [u8; 40],
    }

    const S_STATE_SIZE: usize = core::mem::size_of::<S>();
    const S_MAX_ENCODED: usize = 10 * (S_STATE_SIZE + 7).div_ceil(8) + 1;

    type SBoxed = BPRB<S, alloc::boxed::Box<[u8]>, S_STATE_SIZE, S_MAX_ENCODED>;

    fn make(i: usize) -> S {
        let mut y = [0u8; 40];
        y[0] = i as u8;
        y[39] = (i * 3) as u8;
        S { x: i as u64, y }
    }

    #[test]
    fn rollback_detects_payload_corruption() {
        let mut buf = SBoxed::new_boxed(64 * 1024, 1).unwrap();
        buf.snapshot(&make(0)).unwrap();

        buf.storage_mut()[SLOT_HEADER_SIZE + 1] ^= 0xFF;

        assert_eq!(
            buf.rollback_to(0).err(),
            Some(RollbackError::CorruptedChain)
        );
    }

    #[test]
    fn rollback_reads_straddling_slot() {
        let mut buf = SBoxed::new_boxed(100, 1).unwrap();
        let s0 = make(0);
        let s1 = make(1);
        buf.snapshot(&s0).unwrap();
        buf.snapshot(&s1).unwrap();

        assert_eq!(buf.rollback_to(1).unwrap(), s1);
    }

    #[test]
    fn read_entry_matches_rollback_to() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        for i in 0..10u64 {
            assert_eq!(buf.read_entry(i).unwrap(), buf.rollback_to(i).unwrap());
        }
    }

    #[test]
    fn read_entry_does_not_set_diverged() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        assert!(!buf.diverged);
        buf.read_entry(0).unwrap();
        assert!(!buf.diverged);
    }

    #[test]
    fn read_entry_does_not_change_head_state() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        let head_before = buf.current_head_state;
        buf.read_entry(0).unwrap();
        assert_eq!(buf.current_head_state, head_before);
    }

    #[test]
    fn read_entry_out_of_range() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();

        assert_eq!(buf.read_entry(5).err(), Some(RollbackError::EntryEvicted));
    }

    #[test]
    fn read_entry_empty_buffer() {
        let buf = SBoxed::new_boxed(4096, 10).unwrap();
        assert_eq!(buf.read_entry(0).err(), Some(RollbackError::EntryEvicted));
    }

    #[test]
    fn read_entry_on_anchor() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();
        // Entry 2 is an anchor (2 % 2 == 0)

        assert_eq!(buf.read_entry(2).unwrap(), make(2));
    }

    #[test]
    fn read_entry_preserves_rollback_divergence() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();

        // Rollback sets diverged = true
        buf.rollback_to(0).unwrap();
        assert!(buf.diverged);

        // read_entry should not clear or change diverged
        buf.read_entry(0).unwrap();
        assert!(buf.diverged);
    }
}
