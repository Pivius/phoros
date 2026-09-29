# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1]

### Changed

- Updated Rust edition to 2024.
- Fixed code and document formatting.

## [0.1.0] - 2026-09-29

### Added

- **Generic Arena Storage** — `ArenaStorage` trait supporting stack (`[u8; N]`), heap (`Box<[u8]>`), and borrowed (`&mut [u8]`) backends.
- **`bprb!()` Macro & Type Aliases** — Ergonomic macros and `BoxedBPRB` / `StackBPRB` type aliases for automatic `STATE_SIZE` and `MAX_ENCODED` derivation.
- **`no_std` Support** — `no_std` compatibility for `StackBPRB` (`BoxedBPRB` available via default `alloc`/`std` feature).
- **XOR Delta Compression & CRC16** — Real-time stream encoding with CRC16-CCITT integrity checks on slot headers and payloads.
- **Corrupted-Arena Recovery** — Automatic eviction on corrupt headers to prevent orphaned delta chains.
- **Zero-Allocation Stack Operations** — `snapshot()` and `rollback_to()` implements stack-allocated scratch buffers for zero-heap encoding/decoding.
- **Compile-time Safety Checks** — `STATE_SIZE` generic assertions at compile time alongside sound `repr(C)` header layouts and bounds-checked bit reading/writing.
- **Optional `serde` Integration** — Feature-gated `Serialize`/`Deserialize` implementations for core slot and header types.