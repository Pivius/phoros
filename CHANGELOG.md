# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `BPRB::get()` reconstruct an entry without mutating buffer state.
- `BPRB::iter()` iterator over live entries, yielding `Result<T, RollbackError>`.
- `EntryRange` iterator type.

### Changed

- Refactor `buffer.rs` into `buffer/` module.
- Rename "frames" to "entries" throughout the codebase.
- Rename methods to follow be similar to already existing conventions in Rust:
  - `current_entry()` -> `count()`
  - `oldest_entry()` -> `start()`
  - `newest_entry()` -> `end()`
  - `read_entry()` -> `get()`
  - `entries_all()` -> `iter()`
  - `rollback_to()` -> `rollback()`
  - `AnchorIndex::find_nearest_le()` -> `nearest_le()`
- Remove `entries(range)`, can be done with `iter().skip().take()` instead.

### Fixed

- `end()` now checks `live_slot_count` — returns `None` when nothing is live (was returning `Some(count-1)` after full eviction).
- `is_empty()` now checks `len() == 0` (consistent with Rust std semantics).
- `iter()` on an empty buffer yields nothing instead of entry 0.

## [0.1.2]

### Fixed

- `alloc` feature is independently usable without `std`. Gated `extern crate alloc` behind `#[cfg(feature = "alloc")]` so embedded users with a allocator can use `BoxedBPRB` without pulling in `std`.
- `BPRB::save()` and `BPRB::load()` for persistent arena state (requires `alloc` + `serde` features).
- `SavedState` struct for serializing/deserializing the full buffer state without requiring `T: Serialize`.
- `AnchorIndex` derives `Clone` and supports `Serialize`/`Deserialize` under the `serde` feature.

## [0.1.1]

### Changed

- Updated Rust edition to 2024.
- Fixed code and document formatting.

## [0.1.0] - 2026-09-29

### Added

- `ArenaStorage` trait supporting stack (`[u8; N]`), heap (`Box<[u8]>`), and borrowed (`&mut [u8]`) backends.
- Ergonomic macros and `BoxedBPRB` / `StackBPRB` type aliases for `STATE_SIZE` and `MAX_ENCODED` derivation.
- `no_std` compatibility for `StackBPRB` (`BoxedBPRB` available via default `alloc`/`std` feature).
- Real-time stream encoding with CRC16-CCITT integrity checks on slot headers and payloads.
- Automatic eviction on corrupt headers to prevent orphaned delta chains.
- `snapshot()` and `rollback_to()` implements stack-allocated scratch buffers for encoding/decoding.
- `STATE_SIZE` generic assertions at compile time and `repr(C)` header layouts and bounds-checked bit reading/writing.
- Feature-gated `Serialize`/`Deserialize` implementations for core slot and header types.