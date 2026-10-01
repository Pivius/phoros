use core::marker::PhantomData;

use crate::{
    arena::ArenaHeader,
    codec,
    error::{BufferError, RollbackError},
    index::AnchorIndex,
    slot::{SLOT_HEADER_SIZE, SlotHeader, SlotType, next_slot_offset},
    storage::ArenaStorage,
};

pub const DEFAULT_ANCHOR_INTERVAL: u64 = 60;
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.70;

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
fn compute_checksum(frame: u64, kind: u8, payload: &[u8]) -> u16 {
    let mut crc = 0xFFFF;
    crc = codec::crc16_update(crc, &frame.to_le_bytes());
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
    frame_counter: u64,
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
            frame_counter: 0,
            anchor_interval,
            delta_threshold: DEFAULT_DELTA_THRESHOLD,
            used_bytes: 0,
            diverged: false,
            _marker: PhantomData,
        })
    }

    /// Set the delta compression threshold.
    ///
    /// When a compressed delta exceeds `threshold * STATE_SIZE` bytes, a full
    /// snapshot is stored instead. Defaults to `0.70`.
    pub fn with_delta_threshold(mut self, threshold: f64) -> Self {
        self.delta_threshold = threshold;
        self
    }

    /// Record `state` as a new frame.
    ///
    /// Stores a full snapshot on the first frame, every `anchor_interval`
    /// frames, or when the compressed delta exceeds `delta_threshold`.
    /// Otherwise stores a compressed XOR delta.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::Codec`] if the internal bitstream encoder overflows.
    pub fn snapshot(&mut self, state: &T) -> Result<(), BufferError> {
        let frame = self.frame_counter;
        let state_bytes = Self::state_as_bytes(state);

        let mut buf_delta = [0u8; STATE_SIZE];
        let mut buf_encoded = [0u8; MAX_ENCODED];

        let mut kind;
        let mut payload_len;

        if self.should_store_snapshot(state_bytes, frame) {
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

        let checksum = compute_checksum(frame, kind as u8, payload_ref);
        let slot_header = SlotHeader {
            frame,
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
        self.header.total_frames += 1;
        self.used_bytes += slot_size;

        if kind == SlotType::FullSnapshot {
            self.anchor_index.insert(frame, start_off);
            self.diverged = false;
        }

        self.current_head_state = Some(*state);
        self.frame_counter += 1;

        Ok(())
    }

    /// Reconstruct the state at `target_frame`.
    ///
    /// Finds the nearest anchor near `target_frame`,
    /// then walks forward to the target frame.
    ///
    /// # Errors
    ///
    /// - [`RollbackError::FrameEvicted`] if `target_frame` is out of range
    /// - [`RollbackError::CorruptedChain`] if a slot header is invalid, a
    ///   payload length is inconsistent, or the delta chain is broken.
    /// - [`RollbackError::ArenaCorrupted`] if the arena header is unreadable.
    /// - [`RollbackError::Codec`] if the internal bitstream decoder overflows.
    pub fn read_frame(&self, target_frame: u64) -> Result<T, RollbackError> {
        if target_frame > self.frame_counter.saturating_sub(1) {
            return Err(RollbackError::FrameEvicted);
        }

        let anchor = self
            .anchor_index
            .find_nearest_le(target_frame)
            .ok_or(RollbackError::FrameEvicted)?;

        let mut buf_encoded = [0u8; MAX_ENCODED];

        // Read anchor payload into working byte array.
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

        for expected_frame in (anchor.frame() + 1)..=target_frame {
            let header = self.read_slot_at(offset, &mut buf_encoded)?;

            if header.frame != expected_frame {
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

    /// Reconstruct the state at `target_frame` by finding the nearest anchor
    /// and walking the chain forward. Updates the internal head state.
    ///
    /// # Errors
    ///
    /// See [`BPRB::read_frame`]
    pub fn rollback_to(&mut self, target_frame: u64) -> Result<T, RollbackError> {
        let result = self.read_frame(target_frame)?;

        self.current_head_state = Some(result);
        if target_frame != self.frame_counter.saturating_sub(1) {
            self.diverged = true;
        }

        Ok(result)
    }

    /// Reconstruct the state at `frame_idx`, XOR `delta` into it, update the
    /// internal head state, and return the result.
    ///
    /// # Errors
    ///
    /// - [`RollbackError::CorruptedChain`] if `delta.len() != STATE_SIZE` or
    ///   if the underlying `rollback_to` fails.
    /// - Other variants as documented on [`rollback_to`](Self::rollback_to).
    pub fn apply_delta(&mut self, frame_idx: u64, delta: &[u8]) -> Result<T, RollbackError> {
        if delta.len() != STATE_SIZE {
            return Err(RollbackError::CorruptedChain);
        }

        let mut state = self.rollback_to(frame_idx)?;

        let state_bytes = Self::state_as_bytes_mut(&mut state);
        xor_into(state_bytes, delta);

        self.current_head_state = Some(state);
        self.diverged = true;

        Ok(state)
    }

    /// Returns the current frame counter.
    #[inline]
    pub fn current_frame(&self) -> u64 {
        self.frame_counter
    }

    /// Returns the frame number of the oldest live slot, or `None` if empty.
    pub fn oldest_frame(&self) -> Option<u64> {
        if self.frame_counter == 0 || self.header.live_slot_count == 0 {
            return None;
        }
        read_header_at(self.storage.as_slice(), self.header.head_offset).map(|h| h.frame)
    }

    /// Returns the frame number of the newest live slot, or `None` if empty.
    pub fn newest_frame(&self) -> Option<u64> {
        if self.frame_counter == 0 {
            return None;
        }
        Some(self.frame_counter - 1)
    }

    /// Returns the number of live (unevicted) slots in the arena.
    #[inline]
    pub fn len(&self) -> usize {
        self.header.live_slot_count as usize
    }

    /// Returns `true` if no frames have been recorded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.frame_counter == 0
    }

    /// Returns an iterator over frames in `range`.
    ///
    /// # Examples
    ///
    /// ```
    /// use phoros::bprb;
    ///
    /// let mut buf = bprb!(u64 => stack(4096)).unwrap();
    /// buf.snapshot(&1u64).unwrap();
    /// buf.snapshot(&2u64).unwrap();
    /// buf.snapshot(&3u64).unwrap();
    ///
    /// let mut values = [0u64; 2];
    /// for (i, frame) in buf.frames(1..3).enumerate() {
    ///     values[i] = frame.unwrap();
    /// }
    /// assert_eq!(values, [2, 3]);
    /// ```
    pub fn frames(
        &self,
        range: impl core::ops::RangeBounds<u64>,
    ) -> FrameRange<'_, T, S, STATE_SIZE, MAX_ENCODED>
    where
        T: Copy + Sized + Send + 'static,
    {
        let start = match range.start_bound() {
            core::ops::Bound::Included(&s) => s,
            core::ops::Bound::Excluded(&s) => s + 1,
            core::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            core::ops::Bound::Included(&e) => e,
            core::ops::Bound::Excluded(&e) => e.saturating_sub(1),
            core::ops::Bound::Unbounded => self.frame_counter.saturating_sub(1),
        };
        FrameRange::new(self, start, end)
    }

    /// Returns an iterator over all live frames, from oldest to newest.
    ///
    /// Equivalent to `frames(oldest_frame()..=newest_frame())`.
    pub fn frames_all(&self) -> FrameRange<'_, T, S, STATE_SIZE, MAX_ENCODED>
    where
        T: Copy + Sized + Send + 'static,
    {
        match (self.oldest_frame(), self.newest_frame()) {
            (Some(oldest), Some(newest)) => FrameRange::new(self, oldest, newest),
            _ => FrameRange::new(self, 0, 0),
        }
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

        let checksum = compute_checksum(header.frame, header.kind, &payload_buf[..payload_len]);
        if checksum != header.checksum {
            return Err(RollbackError::CorruptedChain);
        }

        Ok(header)
    }

    #[inline]
    fn should_store_snapshot(&self, _state_bytes: &[u8], frame: u64) -> bool {
        if self.current_head_state.is_none() {
            return true;
        }
        if self.anchor_index.is_empty() {
            return true;
        }
        if self.diverged {
            return true;
        }
        if frame.is_multiple_of(self.anchor_interval) {
            return true;
        }
        false
    }

    /// Evict the oldest segment: the head slot plus any trailing deltas, stopping
    /// just before the next full-snapshot anchor.
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
            // Stop before the next anchor (but always consume the first slot).
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

        // If the loop ended because a header was corrupted, we
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
                .map(|h| h.frame)
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
        // SAFETY: T is Copy + no-drop. read_unaligned handles unaligned source.
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const T) }
    }
}

// Frame iteration

/// Iterator over stored frames in a [`BPRB`].
///
/// Yields `Result<T, RollbackError>` for each frame in the range.
///
/// Created via [`BPRB::frames`] or [`BPRB::frames_all`].
pub struct FrameRange<'a, T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    buf: &'a BPRB<T, S, STATE_SIZE, MAX_ENCODED>,
    next_frame: u64,
    end_frame: u64,
    arena_offset: u32,
    working: [u8; STATE_SIZE],
    buf_encoded: [u8; MAX_ENCODED],
    buf_delta: [u8; STATE_SIZE],
}

impl<'a, T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    FrameRange<'a, T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    fn new(buf: &'a BPRB<T, S, STATE_SIZE, MAX_ENCODED>, start: u64, end: u64) -> Self {
        Self {
            buf,
            next_frame: start,
            end_frame: end,
            arena_offset: 0,
            working: [0u8; STATE_SIZE],
            buf_encoded: [0u8; MAX_ENCODED],
            buf_delta: [0u8; STATE_SIZE],
        }
    }
}

impl<T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize> Iterator
    for FrameRange<'_, T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    type Item = Result<T, RollbackError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_frame > self.end_frame {
            return None;
        }

        let buf = self.buf;
        let target = self.next_frame;

        let anchor = buf.anchor_index.find_nearest_le(target)?;

        if anchor.frame() > target {
            return Some(Err(RollbackError::FrameEvicted));
        }

        // Load anchor payload into working buffer.
        let anchor_header = match buf.read_slot_at(anchor.offset(), &mut self.buf_encoded) {
            Ok(h) => h,
            Err(e) => {
                self.next_frame = u64::MAX; // stop iter
                return Some(Err(e));
            }
        };
        if anchor_header.payload_len as usize != STATE_SIZE {
            self.next_frame = u64::MAX;
            return Some(Err(RollbackError::CorruptedChain));
        }

        self.working[..anchor_header.payload_len as usize]
            .copy_from_slice(&self.buf_encoded[..anchor_header.payload_len as usize]);

        self.arena_offset = next_slot_offset(
            anchor.offset(),
            anchor_header.payload_len,
            buf.header.data_area_len,
        );

        for expected_frame in (anchor.frame() + 1)..=target {
            let header = match buf.read_slot_at(self.arena_offset, &mut self.buf_encoded) {
                Ok(h) => h,
                Err(e) => {
                    self.next_frame = u64::MAX;
                    return Some(Err(e));
                }
            };

            if header.frame != expected_frame {
                self.next_frame = u64::MAX;
                return Some(Err(RollbackError::CorruptedChain));
            }

            match SlotType::from_u8(header.kind) {
                Some(SlotType::FullSnapshot) => {
                    if header.payload_len as usize != STATE_SIZE {
                        self.next_frame = u64::MAX;
                        return Some(Err(RollbackError::CorruptedChain));
                    }
                    self.working[..header.payload_len as usize]
                        .copy_from_slice(&self.buf_encoded[..header.payload_len as usize]);
                }
                Some(SlotType::Delta) => {
                    if header.payload_len as usize > MAX_ENCODED {
                        self.next_frame = u64::MAX;
                        return Some(Err(RollbackError::CorruptedChain));
                    }
                    self.buf_delta.fill(0);
                    if let Err(e) = codec::byte_masked_decode(
                        &self.buf_encoded[..header.payload_len as usize],
                        &mut self.buf_delta,
                    ) {
                        self.next_frame = u64::MAX;
                        return Some(Err(e.into()));
                    }
                    xor_into(&mut self.working, &self.buf_delta);
                }
                None => {
                    self.next_frame = u64::MAX;
                    return Some(Err(RollbackError::CorruptedChain));
                }
            }

            self.arena_offset = next_slot_offset(
                self.arena_offset,
                header.payload_len,
                buf.header.data_area_len,
            );
        }

        let result = BPRB::<T, S, STATE_SIZE, MAX_ENCODED>::bytes_to_state(&self.working);

        self.next_frame += 1;
        Some(Ok(result))
    }
}

// Constructors

#[cfg(feature = "alloc")]
impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    /// Create a heap-backed buffer. The arena is allocated once and never resized.
    ///
    /// # Errors
    ///
    /// - [`BufferError::ArenaFull`] if `arena_bytes` is too small to hold one slot.
    /// - [`BufferError::InvalidState`] if `T` has drop logic.
    pub fn new_boxed(arena_bytes: usize, anchor_interval: u64) -> Result<Self, BufferError> {
        let arena = alloc::vec![0u8; arena_bytes].into_boxed_slice();
        Self::from_storage(arena, anchor_interval)
    }

    pub fn with_defaults_boxed(arena_bytes: usize) -> Result<Self, BufferError> {
        Self::new_boxed(arena_bytes, DEFAULT_ANCHOR_INTERVAL)
    }
}

impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize, const ARENA_SIZE: usize>
    BPRB<T, [u8; ARENA_SIZE], STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    /// Create a stack-backed buffer. The arena is a fixed array on the stack.
    ///
    /// # Errors
    ///
    /// - [`BufferError::ArenaFull`] if `ARENA_SIZE` is too small to hold one slot.
    /// - [`BufferError::InvalidState`] if `T` has drop logic.
    pub fn new_stack(anchor_interval: u64) -> Result<Self, BufferError> {
        Self::from_storage([0u8; ARENA_SIZE], anchor_interval)
    }

    pub fn with_defaults_stack() -> Result<Self, BufferError> {
        Self::new_stack(DEFAULT_ANCHOR_INTERVAL)
    }
}

// Type aliases

/// Heap-backed ring buffer. Arena lives in a `Box<[u8]>`.
///
/// Requires the `alloc` feature. See [`BPRB`] for usage.
#[cfg(feature = "alloc")]
pub type BoxedBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize> =
    BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>;

/// Stack-backed ring buffer. Arena lives in a `[u8; N]` array.
///
/// Works in `no_std` without `alloc`. See [`BPRB`] for usage.
pub type StackBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize, const ARENA_SIZE: usize> =
    BPRB<T, [u8; ARENA_SIZE], STATE_SIZE, MAX_ENCODED>;

/// Construct a [`BPRB`] with `STATE_SIZE` and `MAX_ENCODED` derived
/// automatically from the concrete type.
///
/// # Examples
///
/// ```
/// let buf = phoros::bprb!(u64 => stack({64 * 1024})).unwrap();
/// ```
///
/// With custom anchor interval:
///
/// ```
/// let buf = phoros::bprb!(u64 => stack({64 * 1024}, 10)).unwrap();
/// ```
#[macro_export]
macro_rules! bprb {
    ($t:ty => boxed($arena_bytes:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = 10 * ((__PHOROS_STATE_SIZE + 7) / 8) + 1;
        $crate::BoxedBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED>::new_boxed(
            $arena_bytes,
            $crate::DEFAULT_ANCHOR_INTERVAL,
        )
    }};
    ($t:ty => boxed($arena_bytes:expr, $anchor_interval:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = 10 * ((__PHOROS_STATE_SIZE + 7) / 8) + 1;
        $crate::BoxedBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED>::new_boxed(
            $arena_bytes,
            $anchor_interval,
        )
    }};
    ($t:ty => stack($arena_size:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = 10 * ((__PHOROS_STATE_SIZE + 7) / 8) + 1;
        $crate::StackBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED, $arena_size>::new_stack(
            $crate::DEFAULT_ANCHOR_INTERVAL,
        )
    }};
    ($t:ty => stack($arena_size:expr, $anchor_interval:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = 10 * ((__PHOROS_STATE_SIZE + 7) / 8) + 1;
        $crate::StackBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED, $arena_size>::new_stack(
            $anchor_interval,
        )
    }};
}

// Persistence

/// Serializable snapshot of a [`BPRB`] for persistence.
///
/// The head state is stored as raw bytes — `T` does not need to implement
/// `Serialize` or `Deserialize`.
///
/// Use [`BPRB::save`] to create and [`BPRB::load`] to reconstruct.
#[cfg(all(feature = "alloc", feature = "serde"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedState {
    arena: alloc::vec::Vec<u8>,
    anchor_index: AnchorIndex,
    frame_counter: u64,
    anchor_interval: u64,
    delta_threshold: f64,
    used_bytes: u32,
    diverged: bool,
    head_state: Option<alloc::vec::Vec<u8>>,
}

#[cfg(all(feature = "alloc", feature = "serde"))]
impl<T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize> BPRB<T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    /// Capture the current buffer state as a serializable [`SavedState`].
    pub fn save(&self) -> SavedState {
        let head_state = self.current_head_state.map(|s| {
            let ptr = &s as *const T as *const u8;
            let bytes = unsafe { core::slice::from_raw_parts(ptr, STATE_SIZE) };
            alloc::vec::Vec::from(bytes)
        });

        SavedState {
            arena: alloc::vec::Vec::from(self.storage.as_slice()),
            anchor_index: self.anchor_index.clone(),
            frame_counter: self.frame_counter,
            anchor_interval: self.anchor_interval,
            delta_threshold: self.delta_threshold,
            used_bytes: self.used_bytes,
            diverged: self.diverged,
            head_state,
        }
    }
}

#[cfg(all(feature = "alloc", feature = "serde"))]
impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    /// Reconstruct a heap-backed [`BPRB`] from a previously saved [`SavedState`].
    ///
    /// The type `T` and const generics must match the ones used when
    /// [`save`](Self::save) was called.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::ArenaFull`] if the saved arena is too small
    /// to hold a single slot.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use phoros::bprb;
    ///
    /// let mut buf = bprb!(u64 => boxed(4096)).unwrap();
    /// buf.snapshot(&42u64).unwrap();
    /// let saved = buf.save();
    /// let mut restored = phoros::BoxedBPRB::<u64, 8, 11>::load(saved).unwrap();
    /// assert_eq!(restored.rollback_to(0).unwrap(), 42u64);
    /// ```
    pub fn load(saved: SavedState) -> Result<Self, BufferError> {
        let head_state = saved.head_state.map(|bytes| {
            debug_assert_eq!(bytes.len(), STATE_SIZE);
            Self::bytes_to_state(&bytes)
        });

        let arena = alloc::vec::Vec::into_boxed_slice(saved.arena);
        let mut bprb = Self::from_storage(arena, saved.anchor_interval)?;
        bprb.anchor_index = saved.anchor_index;
        bprb.current_head_state = head_state;
        bprb.frame_counter = saved.frame_counter;
        bprb.delta_threshold = saved.delta_threshold;
        bprb.used_bytes = saved.used_bytes;
        bprb.diverged = saved.diverged;
        Ok(bprb)
    }
}

#[cfg(all(feature = "alloc", feature = "serde"))]
impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize, const ARENA_SIZE: usize>
    BPRB<T, [u8; ARENA_SIZE], STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    /// Reconstruct a stack-backed [`BPRB`] from a previously saved [`SavedState`].
    ///
    /// The saved arena length must match `ARENA_SIZE`.
    ///
    /// # Panics
    ///
    /// Panics if `saved.arena.len() != ARENA_SIZE`.
    pub fn load(saved: SavedState) -> Result<Self, BufferError> {
        let head_state = saved.head_state.map(|bytes| {
            debug_assert_eq!(bytes.len(), STATE_SIZE);
            Self::bytes_to_state(&bytes)
        });

        let mut arena = [0u8; ARENA_SIZE];
        arena.copy_from_slice(&saved.arena);
        let mut bprb = Self::from_storage(arena, saved.anchor_interval)?;
        bprb.anchor_index = saved.anchor_index;
        bprb.current_head_state = head_state;
        bprb.frame_counter = saved.frame_counter;
        bprb.delta_threshold = saved.delta_threshold;
        bprb.used_bytes = saved.used_bytes;
        bprb.diverged = saved.diverged;
        Ok(bprb)
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

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
    fn read_frame_matches_rollback_to() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        for i in 0..10u64 {
            assert_eq!(buf.read_frame(i).unwrap(), buf.rollback_to(i).unwrap());
        }
    }

    #[test]
    fn read_frame_does_not_set_diverged() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        assert!(!buf.diverged);
        buf.read_frame(0).unwrap();
        assert!(!buf.diverged);
    }

    #[test]
    fn read_frame_does_not_change_head_state() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        let head_before = buf.current_head_state;
        buf.read_frame(0).unwrap();
        assert_eq!(buf.current_head_state, head_before);
    }

    #[test]
    fn read_frame_out_of_range() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();

        assert_eq!(buf.read_frame(5).err(), Some(RollbackError::FrameEvicted));
    }

    #[test]
    fn read_frame_empty_buffer() {
        let buf = SBoxed::new_boxed(4096, 10).unwrap();
        assert_eq!(buf.read_frame(0).err(), Some(RollbackError::FrameEvicted));
    }

    #[test]
    fn read_frame_on_anchor() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();
        // Frame 2 is an anchor (2 % 2 == 0)

        assert_eq!(buf.read_frame(2).unwrap(), make(2));
    }

    #[test]
    fn read_frame_preserves_rollback_divergence() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();

        // Rollback sets diverged = true
        buf.rollback_to(0).unwrap();
        assert!(buf.diverged);

        // read_frame should not clear or change diverged
        buf.read_frame(0).unwrap();
        assert!(buf.diverged);
    }

    #[test]
    fn frames_all_yields_all_frames() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.frames_all().collect();
        assert_eq!(results.len(), 5);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i)));
        }
    }

    #[test]
    fn frames_range_yields_subset() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.frames(3..7).collect();
        assert_eq!(results.len(), 4);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i + 3)));
        }
    }

    #[test]
    fn frames_does_not_mutate_state() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        let head_before = buf.current_head_state;
        let _: Vec<_> = buf.frames_all().collect();
        assert_eq!(buf.current_head_state, head_before);
        assert!(!buf.diverged);
    }

    #[test]
    fn frames_empty_buffer_yields_nothing() {
        let buf = SBoxed::new_boxed(4096, 10).unwrap();
        assert_eq!(buf.frames_all().count(), 0);
    }

    #[test]
    fn frames_empty_range_yields_nothing() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();

        assert_eq!(buf.frames(1..1).count(), 0);
    }

    #[test]
    fn frames_across_anchors() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.frames_all().collect();
        assert_eq!(results.len(), 10);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i)));
        }
    }

    #[test]
    fn frames_on_single_anchor() {
        let mut buf = SBoxed::new_boxed(4096, 5).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }
        // Frame 0 is the only anchor

        let results: Vec<_> = buf.frames(0..1).collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Ok(make(0)));
    }

    #[test]
    fn frames_yields_error_for_evicted_frame() {
        let mut buf = SBoxed::new_boxed(200, 2).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();
        buf.snapshot(&make(3)).unwrap();
        // With a tiny arena, frames get evicted. Try to iterate from 0.
        let results: Vec<_> = buf.frames_all().collect();
        // At least one result should be present (evicted or valid)
        assert!(!results.is_empty());
    }

    #[test]
    fn frames_iterator_trait_usage() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }

        // for loop
        let mut count = 0;
        for result in buf.frames_all() {
            result.unwrap();
            count += 1;
        }
        assert_eq!(count, 5);

        // collect + filter
        let evens: Vec<_> = buf
            .frames_all()
            .filter(|r| r.as_ref().is_ok_and(|s| s.x % 2 == 0))
            .collect();
        assert_eq!(evens.len(), 3); // frames 0, 2, 4
    }

    #[test]
    fn frames_matches_rollback_to() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let iter_results: Vec<_> = buf.frames_all().collect();
        for (i, iter_result) in iter_results.iter().enumerate() {
            assert_eq!(*iter_result, Ok(buf.rollback_to(i as u64).unwrap()));
        }
    }

    #[cfg(feature = "serde")]
    mod save_load_tests {
        use super::*;

        #[test]
        fn save_load_roundtrip() {
            let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
            buf.snapshot(&make(0)).unwrap();
            buf.snapshot(&make(1)).unwrap();
            buf.snapshot(&make(2)).unwrap();

            let saved = buf.save();
            let mut restored = SBoxed::load(saved).unwrap();

            assert_eq!(restored.rollback_to(0).unwrap(), make(0));
            assert_eq!(restored.rollback_to(1).unwrap(), make(1));
            assert_eq!(restored.rollback_to(2).unwrap(), make(2));
            assert_eq!(restored.current_frame(), 3);
        }

        #[test]
        fn save_load_empty_buffer() {
            let buf = SBoxed::new_boxed(4096, 10).unwrap();
            let saved = buf.save();
            let restored = SBoxed::load(saved).unwrap();

            assert!(restored.is_empty());
            assert_eq!(restored.current_frame(), 0);
        }

        #[test]
        fn save_load_preserves_metadata() {
            let mut buf = SBoxed::new_boxed(4096, 5).unwrap();
            buf = buf.with_delta_threshold(0.5);
            buf.snapshot(&make(0)).unwrap();
            buf.snapshot(&make(1)).unwrap();

            let saved = buf.save();
            let restored = SBoxed::load(saved).unwrap();

            assert_eq!(restored.delta_threshold, 0.5);
            assert_eq!(restored.anchor_interval, 5);
        }

        #[test]
        fn save_load_preserves_diverged_state() {
            let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
            buf.snapshot(&make(0)).unwrap();
            buf.snapshot(&make(1)).unwrap();
            // Rollback to frame 0, creating divergence
            buf.rollback_to(0).unwrap();

            let saved = buf.save();
            let restored = SBoxed::load(saved).unwrap();

            assert!(restored.diverged);
        }

        #[test]
        fn save_load_cross_anchor() {
            let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
            for i in 0..10 {
                buf.snapshot(&make(i)).unwrap();
            }

            let saved = buf.save();
            let mut restored = SBoxed::load(saved).unwrap();

            for i in 0..10u64 {
                assert_eq!(restored.rollback_to(i).unwrap(), make(i as usize));
            }
        }

        #[test]
        fn save_load_serde_json_roundtrip() {
            let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
            buf.snapshot(&make(42)).unwrap();

            let saved = buf.save();
            let json = serde_json::to_string(&saved).unwrap();
            let deserialized: SavedState = serde_json::from_str(&json).unwrap();
            let mut restored = SBoxed::load(deserialized).unwrap();

            assert_eq!(restored.rollback_to(0).unwrap(), make(42));
        }
    }
}
