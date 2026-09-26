# phoros

phoros is a zero-allocation bit-packed ring buffer with XOR delta encoding along with state rollback.

Low-latency systems can use this to get historical state, undo/redo state trees, and lightweight state telemetry 
without triggering runtime heap allocation.

## Data Structures

### `ArenaHeader` (32 bytes)

Ring state and offsets are maintained by storing them at the top of the arena.

- `magic`: `0x42505242` (BPRB)
- `head/tail_offset`: Track the physical bounds of valid ring data.
- `data_area_len`: Physical capacity of the data arena.

### `SlotHeader` (16 bytes)

This struct prefixes every version entry in the buffer.

- `frame`: A monotonic sequence index.
- `kind`: `FullSnapshot` or `Delta`.
- `payload_len` & `checksum`: Size and integrity validation.

## Quickstart

### Prerequisites & Invariants

Types stored in `BPRB<T>` are limited to Plain Old Data. They can't implement `Drop`, and must not contain pointers, 
`Vec` or any dynamic heap references inside `T`.

```rust
use phoros::BPRB;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
struct State {
	cursor_position: u32,
	selection_length: u32,
	document_flags: u16,
	viewport_offset: u16
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	// 1 MB continuous memory, placing keyframes every 50 actions.
	let arena_size = 1024 * 1024;
	let anchor_interval = 50;

	let mut history = BPRB::<State>::new(arena_size, anchor_interval)?;

	let mut state = State {
		cursor_position: 0,
		selection_length: 0,
		document_flags: 0b0001,
		viewport_offset: 0
	};

	// Move cursor action
	for step in 0..100 {
		state.cursor_position += 1;
		history.snapshot(&state)?;
	}

	// Perform an uno action which reverts state to step 42
	let undo_target = 42;
	let restored_state = history.rollback_to(undo_target)?;

	println!("Restored State at Action {}: {:?}", undo_target, restored_state)?;
	Ok(())
}
```

## State Reconstruction

1. `rollback_to(step)` uses `AnchorIndex` to perform a binary search for the nearest prior `FullSnapshot` sequence when called.
2. Memory gets the nearest base snapshot read directly into it.
3. The process then iterates forward through intermediate sparse deltas, decoding bit-masks and XORing byte changes directly into memory until the target! step is reconstructed exactly.