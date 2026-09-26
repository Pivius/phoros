use std::marker::PhantomData;
use std::vec;

use crate::{
	arena::{ArenaHeader, ARENA_HEADER_SIZE, available_space},
	slot::{SlotHeader, SlotType, SLOT_HEADER_SIZE, next_slot_offset},
	codec, index::AnchorIndex,
	error::{BufferError, RollbackError}
};

pub const DEFAULT_ANCHOR_INTERVAL: u64 = 60;
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.70;

/// Zero-allocation circular state history.
///
/// Allocates a single contiguous byte arena on creation and never allocates again.\
/// `T` must be `Copy + Sized + Send + 'static` with no drop logic.
pub struct BPRB<T> {
	arena: Vec<u8>,
	arena_ptr: *mut ArenaHeader,
	current_head_state: Option<T>,
	anchor_index: AnchorIndex,
	frame_counter: u64,
	state_size: usize,
	anchor_interval: u64,
	delta_threshold: f64,
	_marker: PhantomData<T>,
}

impl<T: Copy + Sized + Send + 'static> BPRB<T> {
	/// Create a new buffer with the given arena byte budget and anchor interval.
	///
	/// `arena_bytes` must fit at least one full snapshot slot (`16 + size_of::<T>()` bytes).
	///
	/// # Errors
	///
	/// - [`BufferError::InvalidState`] if `T` has drop logic.
	/// - [`BufferError::ArenaFull`] if `arena_bytes` is too small.
	pub fn new(arena_bytes: usize, anchor_interval: u64) -> Result<Self, BufferError> {
		Self::validate_type()?;

		let state_size = std::mem::size_of::<T>();
		let min_slot_size = SLOT_HEADER_SIZE + state_size;
		if arena_bytes < min_slot_size {
			return Err(BufferError::ArenaFull);
		}

		let total_size = ARENA_HEADER_SIZE + arena_bytes;
		let mut arena = vec![0u8; total_size];

		let header = ArenaHeader::new(arena_bytes as u32);
		// SAFETY: arena total_size >= ARENA_HEADER_SIZE + arena_bytes.
		unsafe {
			std::ptr::copy_nonoverlapping(
				&header as *const ArenaHeader as *const u8,
				arena.as_mut_ptr(),
				ARENA_HEADER_SIZE,
			);
		}

		let arena_ptr = arena.as_mut_ptr() as *mut ArenaHeader;

		Ok(Self {
			arena,
			arena_ptr,
			current_head_state: None,
			anchor_index: AnchorIndex::new(),
			frame_counter: 0,
			state_size,
			anchor_interval,
			delta_threshold: DEFAULT_DELTA_THRESHOLD,
			_marker: PhantomData,
		})
	}

	pub fn with_defaults(arena_bytes: usize) -> Result<Self, BufferError> {
		Self::new(arena_bytes, DEFAULT_ANCHOR_INTERVAL)
	}

	pub fn with_delta_threshold(mut self, threshold: f64) -> Self {
		self.delta_threshold = threshold;
		self
	}

	fn validate_type() -> Result<(), BufferError> {
		if std::mem::needs_drop::<T>() {
			return Err(BufferError::InvalidState);
		}
		Ok(())
	}

	/// Record `state` as a new frame. Stores a full snapshot on the first
	/// frame, every `anchor_interval` frames, or when the compressed delta
	/// exceeds the threshold. Otherwise stores a compressed XOR delta.
	pub fn snapshot(&mut self, state: &T) -> Result<(), BufferError> {
		let frame = self.frame_counter;
		let state_bytes = Self::state_as_bytes(state);

		let (kind, payload_buf);

		if self.should_store_snapshot(state_bytes, frame) {
			kind = SlotType::FullSnapshot;
			payload_buf = Vec::from(state_bytes);
		} else {
			let current = Self::state_as_bytes(
				self.current_head_state
					.as_ref()
					.expect("current_head_state must be set after first snapshot"),
			);
			let mut delta = vec![0u8; self.state_size];
			for i in 0..self.state_size {
				delta[i] = state_bytes[i] ^ current[i];
			}

			let max_encoded = 9 * ((self.state_size + 7) / 8);
			let mut encoded = vec![0u8; max_encoded];
			let encoded_len = codec::byte_masked_encode(&delta, &mut encoded);

			if encoded_len > ((self.state_size as f64 * self.delta_threshold) as usize) {
				kind = SlotType::FullSnapshot;
				payload_buf = Vec::from(state_bytes);
			} else {
				kind = SlotType::Delta;
				encoded.truncate(encoded_len);
				payload_buf = encoded;
			}
		};

		self.write_slot(frame, kind, &payload_buf)?;

		if kind == SlotType::FullSnapshot {
			let slot_offset = self.compute_slot_write_offset();
			self.anchor_index.insert(frame, slot_offset);
		}

		self.current_head_state = Some(*state);
		self.frame_counter += 1;

		Ok(())
	}

	/// Reconstruct the state at `target_frame` by finding the nearest anchor
	/// and walking the delta chain forward. Updates the internal head state.
	pub fn rollback_to(&mut self, target_frame: u64) -> Result<T, RollbackError> {
		let anchor = self
			.anchor_index
			.find_nearest_le(target_frame)
			.ok_or(RollbackError::FrameEvicted)?;

		let state_bytes = self.read_payload_at(anchor.offset)?;
		let mut working = Self::bytes_to_state(state_bytes);

		let mut offset = {
			let header = self.read_slot_header_at(anchor.offset)?;
			next_slot_offset(anchor.offset, header.payload_len, self.data_area_len())
		};

		for expected_frame in (anchor.frame + 1)..=target_frame {
			let header = self.read_slot_header_at(offset)?;

			if header.frame != expected_frame {
				return Err(RollbackError::CorruptedChain);
			}

			let payload = self.read_payload_at(offset)?;

			match SlotType::from_u8(header.kind) {
				Some(SlotType::FullSnapshot) => {
					working = Self::bytes_to_state(payload);
				}
				Some(SlotType::Delta) => {
					let mut delta = vec![0u8; self.state_size];
					codec::byte_masked_decode(payload, &mut delta)
						.map_err(|_| RollbackError::CorruptedChain)?;

					let working_bytes = Self::state_as_bytes_mut(&mut working);
					for i in 0..self.state_size {
						working_bytes[i] ^= delta[i];
					}
				}
				None => return Err(RollbackError::CorruptedChain),
			}

			offset = next_slot_offset(offset, header.payload_len, self.data_area_len());
		}

		self.current_head_state = Some(working);

		Ok(working)
	}

	pub fn current_frame(&self) -> u64 {
		self.frame_counter
	}

	pub fn oldest_frame(&self) -> Option<u64> {
		if self.frame_counter == 0 {
			return None;
		}
		let header = self.arena_header();
		if header.live_slot_count == 0 {
			return None;
		}
		self.read_slot_header_at(header.head_offset)
			.ok()
			.map(|h| h.frame)
	}

	pub fn newest_frame(&self) -> Option<u64> {
		if self.frame_counter == 0 {
			return None;
		}
		Some(self.frame_counter - 1)
	}

	pub fn len(&self) -> usize {
		self.arena_header().live_slot_count as usize
	}

	pub fn is_empty(&self) -> bool {
		self.frame_counter == 0
	}

	pub fn memory_usage(&self) -> usize {
		self.arena.len() + std::mem::size_of::<Self>()
	}

	// Private

	fn arena_header(&self) -> &ArenaHeader {
		// SAFETY: arena_ptr is valid, aligned, and initialized in `new()`.
		// The arena lives for the lifetime of `self`.
		unsafe { &*self.arena_ptr }
	}

	fn arena_header_mut(&mut self) -> &mut ArenaHeader {
		// SAFETY: same as arena_header, mutable.
		unsafe { &mut *self.arena_ptr }
	}

	fn data_area_len(&self) -> u32 {
		self.arena_header().data_area_len
	}

	fn compute_slot_write_offset(&self) -> u32 {
		self.arena_header().tail_offset
	}

	fn read_slot_header_at(&self, offset: u32) -> Result<SlotHeader, RollbackError> {
		let start = ARENA_HEADER_SIZE + offset as usize;
		let end = start + SLOT_HEADER_SIZE;

		if end > self.arena.len() {
			return Err(RollbackError::ArenaCorrupted);
		}

		SlotHeader::from_bytes(&self.arena[start..end])
			.ok_or(RollbackError::ArenaCorrupted)
	}

	fn read_payload_at(&self, slot_offset: u32) -> Result<&[u8], RollbackError> {
		let header = self.read_slot_header_at(slot_offset)?;
		let payload_start = (ARENA_HEADER_SIZE + slot_offset as usize) + SLOT_HEADER_SIZE;
		let payload_end = payload_start + header.payload_len as usize;

		if payload_end > self.arena.len() {
			return Err(RollbackError::ArenaCorrupted);
		}

		Ok(&self.arena[payload_start..payload_end])
	}

	fn should_store_snapshot(&self, _state_bytes: &[u8], frame: u64) -> bool {
		if self.current_head_state.is_none() {
			return true;
		}
		if frame % self.anchor_interval == 0 {
			return true;
		}
		false
	}

	/// Write a slot at `tail_offset`, evicting from `head_offset` if needed.\
	/// Handles arena wraparound when the slot straddles the boundary.
	fn write_slot(&mut self, frame: u64, kind: SlotType, payload: &[u8]) -> Result<(), BufferError> {
		let header = self.arena_header();
		let data_area_len = header.data_area_len;
		let tail = header.tail_offset;
		let head = header.head_offset;

		let slot_size = SLOT_HEADER_SIZE as u32 + payload.len() as u32;

		let mut avail = available_space(head, tail, data_area_len);
		while avail < slot_size && self.arena_header().live_slot_count > 0 {
			self.evict_oldest_slot();
			let h = self.arena_header();
			avail = available_space(h.head_offset, h.tail_offset, h.data_area_len);
		}

		if avail < slot_size {
			return Err(BufferError::ArenaFull);
		}

		let slot_header = SlotHeader {
			frame,
			kind: kind as u8,
			payload_len: payload.len() as u16,
			checksum: 0, // TODO: CRC16
			reserved: 0,
		};

		let write_offset = tail;
		let start = ARENA_HEADER_SIZE + write_offset as usize;
		let remaining_at_tail = data_area_len - tail;

		if slot_size <= remaining_at_tail {
			// SAFETY: bounds checked by avail >= slot_size.
			unsafe {
				slot_header.write_to(&mut self.arena[start..start + SLOT_HEADER_SIZE]);
			}
			let payload_start = start + SLOT_HEADER_SIZE;
			self.arena[payload_start..payload_start + payload.len()].copy_from_slice(payload);

			self.arena_header_mut().tail_offset = (write_offset + slot_size) % data_area_len;
		} else {
			// Straddles boundary - header at tail, payload wraps.
			unsafe {
				slot_header.write_to(&mut self.arena[start..]);
			}
			let part1_len = remaining_at_tail - SLOT_HEADER_SIZE as u32;
			let payload_start = start + SLOT_HEADER_SIZE;
			self.arena[payload_start..].copy_from_slice(&payload[..part1_len as usize]);

			let part2_len = payload.len() - part1_len as usize;
			let data_area_start = ARENA_HEADER_SIZE;
			self.arena[data_area_start..data_area_start + part2_len]
				.copy_from_slice(&payload[part1_len as usize..]);

			self.arena_header_mut().tail_offset = part2_len as u32;
		}

		self.arena_header_mut().live_slot_count += 1;

		Ok(())
	}

	fn evict_oldest_slot(&mut self) {
		let head = self.arena_header().head_offset;
		let header = match self.read_slot_header_at(head) {
			Ok(h) => h,
			Err(_) => return,
		};

		if header.kind == SlotType::FullSnapshot as u8 {
			self.anchor_index.remove_by_offset(head);
		}

		let new_head = next_slot_offset(head, header.payload_len, self.data_area_len());
		self.arena_header_mut().head_offset = new_head;
		self.arena_header_mut().live_slot_count -= 1;
	}

	fn state_as_bytes(state: &T) -> &[u8] {
		// SAFETY: T is Copy + Sized + needs_drop == false.
		unsafe { std::slice::from_raw_parts(state as *const T as *const u8, std::mem::size_of::<T>()) }
	}

	fn state_as_bytes_mut(state: &mut T) -> &mut [u8] {
		// SAFETY: T is Copy + Sized + needs_drop == false.
		unsafe {
			std::slice::from_raw_parts_mut(
				state as *mut T as *mut u8,
				std::mem::size_of::<T>(),
			)
		}
	}

	fn bytes_to_state(bytes: &[u8]) -> T {
		assert_eq!(bytes.len(), std::mem::size_of::<T>());
		// SAFETY: bytes.len() == size_of::<T>(), T is flat/Copy/no-drop.
		unsafe { std::ptr::read(bytes.as_ptr() as *const T) }
	}
}
