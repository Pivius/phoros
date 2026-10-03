# phoros

A ring buffer for state snapshotting and rollback in Rust.

`phoros` tracks historical state changes using byte deltas anchored to periodic snapshots.  
Storage is fully generic over stack arrays, borrowed buffers, or heap slices, making it usable in both `#![no_std]` and standard environments.

## Usage

Add `phoros` to your `Cargo.toml`:

```toml
[dependencies]
phoros = "0.2"
```

For `#![no_std]` environments, disable default features:

```toml
[dependencies]
phoros = { version = "0.2", default-features = false }
```

## Quickstart

### Heap-backed

```rust
use phoros::{bprb, BufferError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransformState {
    coords: (i32, i32),
    scale: i32,
    rot: i32,
}

fn main() -> Result<(), BufferError> {
    // Allocate 1MB arena on the heap, anchor every 60 frames.
    let mut history = bprb!(TransformState => boxed(1024 * 1024, 60))?;
    let mut state = TransformState { coords: (0, 0), scale: 1, rot: 0 };

    for _ in 0..100 {
        state.coords.0 += 1;
        history.snapshot(&state)?;
    }

    let restored = history.rollback(42)?;
    assert_eq!(restored.coords.0, 43);

    Ok(())
}
```

### Stack-backed

```rust
use phoros::bprb;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EmbeddedState {
    sensor_val: u32,
}

fn run() {
    // 16KB arena on the stack.
    let mut history = bprb!(EmbeddedState => stack(16384)).unwrap();
    let state = EmbeddedState { sensor_val: 42 };

    history.snapshot(&state).unwrap();
}
```

## Type Constraints

States managed by `phoros` must implement `Copy + Sized + Send + 'static` and `needs_drop::<T>() == false`.
