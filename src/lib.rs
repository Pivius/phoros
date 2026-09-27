#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod arena;
pub mod buffer;
pub mod codec;
pub mod error;
pub mod index;
pub mod slot;
pub mod storage;

pub use buffer::BPRB;
#[cfg(feature = "alloc")]
pub use buffer::BoxedBPRB;
pub use buffer::StackBPRB;
pub use buffer::{DEFAULT_ANCHOR_INTERVAL, DEFAULT_DELTA_THRESHOLD};
pub use error::{BufferError, CodecError, RollbackError};
pub use index::AnchorIndex;
pub use slot::SlotType;
pub use storage::ArenaStorage;

const _: () = assert!(
	core::mem::size_of::<arena::ArenaHeader>() == 32,
	"ArenaHeader must be exactly 32 bytes"
);

const _: () = assert!(
	core::mem::size_of::<slot::SlotHeader>() == 16,
	"SlotHeader must be exactly 16 bytes"
);

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn arena_header_size() {
		assert_eq!(core::mem::size_of::<arena::ArenaHeader>(), 32);
	}

	#[test]
	fn slot_header_size() {
		assert_eq!(core::mem::size_of::<slot::SlotHeader>(), 16);
	}

	#[test]
	fn anchor_entry_size() {
		assert_eq!(core::mem::size_of::<index::AnchorEntry>(), 16);
	}

	#[test]
	fn slot_kind_roundtrip() {
		assert_eq!(SlotType::from_u8(0x01), Some(SlotType::FullSnapshot));
		assert_eq!(SlotType::from_u8(0x02), Some(SlotType::Delta));
		assert_eq!(SlotType::from_u8(0x00), None);
		assert_eq!(SlotType::from_u8(0xFF), None);
	}

	#[test]
	fn arena_header_magic() {
		let header = arena::ArenaHeader::new(1024);
		assert_eq!(header.magic, arena::ARENA_MAGIC);
		assert_eq!(header.version, arena::ARENA_VERSION);
		assert!(header.is_valid());
	}

	#[test]
	fn arena_header_roundtrip() {
		let header = arena::ArenaHeader::new(4096);
		let bytes = header.as_bytes();
		let restored = arena::ArenaHeader::from_bytes(bytes).unwrap();
		assert_eq!(restored.data_area_len, 4096);
		assert_eq!(restored.head_offset, 0);
		assert_eq!(restored.tail_offset, 0);
	}

	#[test]
	fn physical_offset_wrapping() {
		assert_eq!(arena::physical_offset(100, 50, 200), 150);
		assert_eq!(arena::physical_offset(150, 100, 200), 50);
		assert_eq!(arena::physical_offset(0, 0, 200), 0);
	}

	#[test]
	fn available_space_calculation() {
		assert_eq!(arena::available_space(50, 100, 200), 150);
		assert_eq!(arena::available_space(100, 50, 200), 50);
	}
}
