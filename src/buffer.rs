use std::marker::PhantomData;
use std::vec;

use crate::{
	arena::ArenaHeader,
	slot::{SlotHeader, SlotType, SLOT_HEADER_SIZE, next_slot_offset},
	codec, index::AnchorIndex,
	error::{BufferError, RollbackError}
};

pub const DEFAULT_ANCHOR_INTERVAL: u64 = 60;
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.70;

// Free functions for arena I/O and CRC

fn arena_read_into(arena: &[u8], off: u32, buf: &mut [u8]) {
	let data_len = arena.len();
	if data_len == 0 || buf.is_empty() {
		return;
	}
	let start = (off as usize) % data_len;
	let first = std::cmp::min(buf.len(), data_len - start);
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
	let first = std::cmp::min(bytes.len(), data_len - start);
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

/// Read a 16-byte slot header from the arena
fn read_header_at(arena: &[u8], off: u32) -> Option<SlotHeader> {
	let mut buf = [0u8; SLOT_HEADER_SIZE];
	arena_read_into(arena, off, &mut buf);
	SlotHeader::from_bytes(&buf)
}

/// Zero-allocation circular state history.
///
/// Allocates a single contiguous byte arena on creation and never allocates again.\
/// `T` must be `Copy + Sized + Send + 'static` with no drop logic.
pub struct BPRB<T> {
	arena: Box<[u8]>,
	header: ArenaHeader,
	used_bytes: u32,
	current_head_state: Option<T>,
	anchor_index: AnchorIndex,
	frame_counter: u64,
	diverged: bool,
	state_size: usize,
	anchor_interval: u64,
	delta_threshold: f64,
	// Scratch Buffers
	buf_delta: Box<[u8]>,
	buf_encoded: Box<[u8]>,
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

		// Scratch buffer sizing.
		let max_encoded = 10 * state_size.div_ceil(8) + 1;

		let arena = vec![0u8; arena_bytes].into_boxed_slice();
		let header = ArenaHeader::new(arena_bytes as u32);

		Ok(Self {
			arena,
			header,
			used_bytes: 0,
			current_head_state: None,
			anchor_index: AnchorIndex::new(),
			frame_counter: 0,
			diverged: false,
			state_size,
			anchor_interval,
			delta_threshold: DEFAULT_DELTA_THRESHOLD,
			buf_delta: vec![0u8; state_size].into_boxed_slice(),
			buf_encoded: vec![0u8; max_encoded].into_boxed_slice(),
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

	pub fn snapshot(&mut self, state: &T) -> Result<(), BufferError> {
		let frame = self.frame_counter;
		let state_bytes = Self::state_as_bytes(state);

		let mut kind;
		let mut payload_len;

		if self.should_store_snapshot(state_bytes, frame) {
			kind = SlotType::FullSnapshot;
			payload_len = self.state_size;
		} else {
			// XOR delta into buf_delta.
			let current = Self::state_as_bytes(
				self.current_head_state
					.as_ref()
					.expect("current_head_state must be set after first snapshot"),
			);
			for (d, (s, c)) in self.buf_delta.iter_mut().zip(state_bytes.iter().zip(current.iter())) {
				*d = *s ^ *c;
			}

			let encoded_len = codec::byte_masked_encode(&self.buf_delta, &mut self.buf_encoded);

			if encoded_len > ((self.state_size as f64 * self.delta_threshold) as usize) {
				kind = SlotType::FullSnapshot;
				payload_len = self.state_size;
			} else {
				kind = SlotType::Delta;
				payload_len = encoded_len;
			}
		};

		// Reserve
		self.ensure_space((SLOT_HEADER_SIZE + payload_len) as u32);

		if kind == SlotType::Delta && self.anchor_index.is_empty() {
			kind = SlotType::FullSnapshot;
			payload_len = self.state_size;
			self.ensure_space((SLOT_HEADER_SIZE + self.state_size) as u32);
		}

		let payload_ref: &[u8] = if kind == SlotType::Delta {
			&self.buf_encoded[..payload_len]
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

		// Write header + payload to arena
		let start_off = self.header.tail_offset;
		arena_write_to(&mut self.arena, start_off, &header_bytes);
		arena_write_to(&mut self.arena, start_off + SLOT_HEADER_SIZE as u32, payload_ref);

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
	/// and walking the delta chain forward. Updates the internal head state.
	pub fn rollback_to(&mut self, target_frame: u64) -> Result<T, RollbackError> {
		if target_frame > self.frame_counter.saturating_sub(1) {
			return Err(RollbackError::FrameEvicted);
		}

		let anchor = self
			.anchor_index
			.find_nearest_le(target_frame)
			.ok_or(RollbackError::FrameEvicted)?;

		let anchor_header = self.read_slot_at(anchor.offset)?;
		let mut working = Self::bytes_to_state(
			&self.buf_encoded[..anchor_header.payload_len as usize],
		);

		let mut offset = next_slot_offset(
			anchor.offset,
			anchor_header.payload_len,
			self.header.data_area_len,
		);

		for expected_frame in (anchor.frame + 1)..=target_frame {
			let header = self.read_slot_at(offset)?;

			if header.frame != expected_frame {
				return Err(RollbackError::CorruptedChain);
			}

			match SlotType::from_u8(header.kind) {
				Some(SlotType::FullSnapshot) => {
					working = Self::bytes_to_state(
						&self.buf_encoded[..header.payload_len as usize],
					);
				}
				Some(SlotType::Delta) => {
					self.buf_delta.fill(0);
					codec::byte_masked_decode(
						&self.buf_encoded[..header.payload_len as usize],
						&mut self.buf_delta,
					);

					let working_bytes = Self::state_as_bytes_mut(&mut working);
					for (w, d) in working_bytes.iter_mut().zip(self.buf_delta.iter()) {
						*w ^= *d;
					}
				}
				None => return Err(RollbackError::CorruptedChain),
			}

			offset = next_slot_offset(offset, header.payload_len, self.header.data_area_len);
		}

		self.current_head_state = Some(working);
		if target_frame != self.frame_counter.saturating_sub(1) {
			self.diverged = true;
		}

		Ok(working)
	}

	/// Reconstruct the state at `frame_idx`, XOR `delta` into it, update the
	/// internal head state, and return the result.
	pub fn apply_delta(&mut self, frame_idx: u64, delta: &[u8]) -> Result<T, RollbackError> {
		if delta.len() != self.state_size {
			return Err(RollbackError::CorruptedChain);
		}

		let mut state = self.rollback_to(frame_idx)?;

		let state_bytes = Self::state_as_bytes_mut(&mut state);
		for (s, d) in state_bytes.iter_mut().zip(delta.iter()) {
			*s ^= *d;
		}

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
		read_header_at(&self.arena, self.header.head_offset).map(|h| h.frame)
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
		self.arena.len() + std::mem::size_of::<Self>()
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

	/// Read a slot's header and payload at `offset` into `buf_encoded`,
	/// verifying the CRC16. Returns the parsed header.
	fn read_slot_at(&mut self, offset: u32) -> Result<SlotHeader, RollbackError> {
		let header = read_header_at(&self.arena, offset)
			.ok_or(RollbackError::ArenaCorrupted)?;

		let payload_len = header.payload_len as usize;
		arena_read_into(
			&self.arena,
			offset + SLOT_HEADER_SIZE as u32,
			&mut self.buf_encoded[..payload_len],
		);

		// CRC verify
		let checksum = compute_checksum(
			header.frame,
			header.kind,
			&self.buf_encoded[..payload_len],
		);
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

		while let Some(header) = read_header_at(&self.arena, off) {
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

		if count == 0 {
			return;
		}

		self.header.head_offset = off;
		self.header.live_slot_count -= count;
		self.used_bytes -= freed;

		let boundary = if self.header.live_slot_count == 0 {
			u64::MAX
		} else {
			read_header_at(&self.arena, off).map(|h| h.frame).unwrap_or(u64::MAX)
		};
		self.anchor_index.evict_before(boundary);
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
		// SAFETY: T is Copy + no-drop. read_unaligned handles byte-aligned source.
		unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const T) }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Clone, Copy, PartialEq, Eq, Debug)]
	struct S {
		x: u64,
		y: [u8; 40],
	}

	fn make(i: usize) -> S {
		let mut y = [0u8; 40];
		y[0] = i as u8;
		y[39] = (i * 3) as u8;
		S { x: i as u64, y }
	}

	#[test]
	fn rollback_detects_payload_corruption() {
		let mut buf = BPRB::<S>::new(64 * 1024, 1).unwrap();
		buf.snapshot(&make(0)).unwrap();

		buf.arena[SLOT_HEADER_SIZE + 1] ^= 0xFF;

		assert_eq!(buf.rollback_to(0).err(), Some(RollbackError::CorruptedChain));
	}

	#[test]
	fn rollback_reads_straddling_slot() {
		let mut buf = BPRB::<S>::new(100, 1).unwrap();
		let s0 = make(0);
		let s1 = make(1);
		buf.snapshot(&s0).unwrap();
		buf.snapshot(&s1).unwrap();

		assert_eq!(buf.rollback_to(1).unwrap(), s1);
	}
}
