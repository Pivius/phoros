use core::marker::PhantomData;

use crate::{
	arena::ArenaHeader,
	slot::{SlotHeader, SlotType, SLOT_HEADER_SIZE, next_slot_offset},
	codec, index::AnchorIndex,
	error::{BufferError, RollbackError},
	storage::ArenaStorage,
};

pub const DEFAULT_ANCHOR_INTERVAL: u64 = 60;
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.70;

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

fn compute_checksum(frame: u64, kind: u8, payload: &[u8]) -> u16 {
	let mut crc = 0xFFFF;
	crc = codec::crc16_update(crc, &frame.to_le_bytes());
	crc = codec::crc16_update(crc, &[kind]);
	crc = codec::crc16_update(crc, &(payload.len() as u16).to_le_bytes());
	crc = codec::crc16_update(crc, payload);
	crc
}

fn read_header_at(arena: &[u8], off: u32) -> Option<SlotHeader> {
	let mut buf = [0u8; SLOT_HEADER_SIZE];
	arena_read_into(arena, off, &mut buf);
	SlotHeader::from_bytes(&buf)
}

/// XOR `src` into `dst` in 8-byte chunks
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

/// Zero-allocation circular state history.
///
/// `T` must be `Copy + Sized + Send + 'static` with no drop logic.
pub struct BPRB<T, S: ArenaStorage, const STATE_SIZE: usize, const MAX_ENCODED: usize> {
	storage: S,
	header: ArenaHeader,
	used_bytes: u32,
	current_head_state: Option<T>,
	anchor_index: AnchorIndex,
	frame_counter: u64,
	diverged: bool,
	anchor_interval: u64,
	delta_threshold: f64,
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
		let _ = Self::_ENSURE_STATE_SIZE;
		Self::validate_type()?;

		let min_slot_size = SLOT_HEADER_SIZE + STATE_SIZE;
		if storage.len() < min_slot_size {
			return Err(BufferError::ArenaFull);
		}

		let header = ArenaHeader::new(storage.len() as u32);

		Ok(Self {
			storage,
			header,
			used_bytes: 0,
			current_head_state: None,
			anchor_index: AnchorIndex::new(),
			frame_counter: 0,
			diverged: false,
			anchor_interval,
			delta_threshold: DEFAULT_DELTA_THRESHOLD,
			_marker: PhantomData,
		})
	}

	pub fn with_delta_threshold(mut self, threshold: f64) -> Self {
		self.delta_threshold = threshold;
		self
	}

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
			xor_into(&mut buf_delta, state_bytes);
			xor_into(&mut buf_delta, current);

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
			reserved: 0,
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

	/// Reconstruct the state at `target_frame` by finding the nearest anchor
	/// and walking the chain forward. Updates the internal head state.
	pub fn rollback_to(&mut self, target_frame: u64) -> Result<T, RollbackError> {
		if target_frame > self.frame_counter.saturating_sub(1) {
			return Err(RollbackError::FrameEvicted);
		}

		let anchor = self
			.anchor_index
			.find_nearest_le(target_frame)
			.ok_or(RollbackError::FrameEvicted)?;

		let mut buf_encoded = [0u8; MAX_ENCODED];

		// Read anchor payload (full snapshot) into working byte array.
		let anchor_header = self.read_slot_at(anchor.offset, &mut buf_encoded)?;
		let mut working = [0u8; STATE_SIZE];
		working[..anchor_header.payload_len as usize]
			.copy_from_slice(&buf_encoded[..anchor_header.payload_len as usize]);

		let mut offset = next_slot_offset(
			anchor.offset,
			anchor_header.payload_len,
			self.header.data_area_len,
		);

		let mut buf_delta = [0u8; STATE_SIZE];

		for expected_frame in (anchor.frame + 1)..=target_frame {
			let header = self.read_slot_at(offset, &mut buf_encoded)?;

			if header.frame != expected_frame {
				return Err(RollbackError::CorruptedChain);
			}

			match SlotType::from_u8(header.kind) {
				Some(SlotType::FullSnapshot) => {
					working[..header.payload_len as usize]
						.copy_from_slice(&buf_encoded[..header.payload_len as usize]);
				}
				Some(SlotType::Delta) => {
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

		// SAFETY: working was reconstructed from valid full-snapshot bytes
		// and correctly applied deltas, so it represents a valid T.
		let result = Self::bytes_to_state(&working);

		self.current_head_state = Some(result);
		if target_frame != self.frame_counter.saturating_sub(1) {
			self.diverged = true;
		}

		Ok(result)
	}

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

	pub fn current_frame(&self) -> u64 {
		self.frame_counter
	}

	pub fn oldest_frame(&self) -> Option<u64> {
		if self.frame_counter == 0 || self.header.live_slot_count == 0 {
			return None;
		}
		read_header_at(self.storage.as_slice(), self.header.head_offset).map(|h| h.frame)
	}

	pub fn newest_frame(&self) -> Option<u64> {
		if self.frame_counter == 0 {
			return None;
		}
		Some(self.frame_counter - 1)
	}

	pub fn len(&self) -> usize {
		self.header.live_slot_count as usize
	}

	pub fn is_empty(&self) -> bool {
		self.frame_counter == 0
	}

	pub fn memory_usage(&self) -> usize {
		self.storage.len() + core::mem::size_of::<Self>()
	}

	pub fn storage(&self) -> &S {
		&self.storage
	}

	pub fn storage_mut(&mut self) -> &mut S {
		&mut self.storage
	}

	// Private

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

	fn state_as_bytes(state: &T) -> &[u8] {
		// SAFETY: T is Copy + Sized + needs_drop == false.
		unsafe { core::slice::from_raw_parts(state as *const T as *const u8, STATE_SIZE) }
	}

	fn state_as_bytes_mut(state: &mut T) -> &mut [u8] {
		// SAFETY: T is Copy + Sized + needs_drop == false.
		unsafe { core::slice::from_raw_parts_mut(state as *mut T as *mut u8, STATE_SIZE) }
	}

	fn bytes_to_state(bytes: &[u8]) -> T {
		core::debug_assert_eq!(bytes.len(), STATE_SIZE);
		// SAFETY: T is Copy + no-drop. read_unaligned handles unaligned source.
		unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const T) }
	}
}

// ── Convenience constructors ─────────────────────────────────────────

#[cfg(feature = "alloc")]
impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize>
	BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>
where
	T: Copy + Sized + Send + 'static,
{
	/// Create a heap-backed buffer. The arena is allocated once and never resized.
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
	pub fn new_stack(anchor_interval: u64) -> Result<Self, BufferError> {
		Self::from_storage([0u8; ARENA_SIZE], anchor_interval)
	}

	pub fn with_defaults_stack() -> Result<Self, BufferError> {
		Self::new_stack(DEFAULT_ANCHOR_INTERVAL)
	}
}

// Type aliases

/// Heap-backed ring buffer
pub type BoxedBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize> =
	BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>;

/// Stack-backed ring buffer
pub type StackBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize, const ARENA_SIZE: usize> =
	BPRB<T, [u8; ARENA_SIZE], STATE_SIZE, MAX_ENCODED>;

/// Construct a [`BoxedBPRB`] with `STATE_SIZE` and `MAX_ENCODED` derived
/// automatically from the concrete type.
///
/// # Examples
///
/// ```
/// let buf = phoros::bprb!(u64 => boxed(64 * 1024)).unwrap();
/// ```
///
/// With custom anchor interval:
///
/// ```
/// let buf = phoros::bprb!(u64 => boxed(64 * 1024, 10)).unwrap();
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

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Clone, Copy, PartialEq, Eq, Debug)]
	struct S {
		x: u64,
		y: [u8; 40],
	}

	const S_STATE_SIZE: usize = core::mem::size_of::<S>();
	const S_MAX_ENCODED: usize = 10 * ((S_STATE_SIZE + 7) / 8) + 1;

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

		assert_eq!(buf.rollback_to(0).err(), Some(RollbackError::CorruptedChain));
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
}
