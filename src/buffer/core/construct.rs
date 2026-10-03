use crate::error::BufferError;

use super::{BPRB, DEFAULT_ANCHOR_INTERVAL};

#[cfg(feature = "alloc")]
impl<T, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
{
    /// Create a heap-backed buffer.
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
    /// Create a stack-backed buffer.
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

/// Requires the `alloc` feature. See [`BPRB`] for usage.
#[cfg(feature = "alloc")]
pub type BoxedBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize> =
    BPRB<T, alloc::boxed::Box<[u8]>, STATE_SIZE, MAX_ENCODED>;

/// Works in `no_std` without `alloc`. See [`BPRB`] for usage.
pub type StackBPRB<T, const STATE_SIZE: usize, const MAX_ENCODED: usize, const ARENA_SIZE: usize> =
    BPRB<T, [u8; ARENA_SIZE], STATE_SIZE, MAX_ENCODED>;

/// Construct a [`BPRB`] with `STATE_SIZE` and `MAX_ENCODED` derived.
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
        const __PHOROS_MAX_ENCODED: usize = $crate::buffer::max_encoded(__PHOROS_STATE_SIZE);
        $crate::BoxedBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED>::new_boxed(
            $arena_bytes,
            $crate::DEFAULT_ANCHOR_INTERVAL,
        )
    }};
    ($t:ty => boxed($arena_bytes:expr, $anchor_interval:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = $crate::buffer::max_encoded(__PHOROS_STATE_SIZE);
        $crate::BoxedBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED>::new_boxed(
            $arena_bytes,
            $anchor_interval,
        )
    }};
    ($t:ty => stack($arena_size:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = $crate::buffer::max_encoded(__PHOROS_STATE_SIZE);
        $crate::StackBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED, $arena_size>::new_stack(
            $crate::DEFAULT_ANCHOR_INTERVAL,
        )
    }};
    ($t:ty => stack($arena_size:expr, $anchor_interval:expr)) => {{
        const __PHOROS_STATE_SIZE: usize = core::mem::size_of::<$t>();
        const __PHOROS_MAX_ENCODED: usize = $crate::buffer::max_encoded(__PHOROS_STATE_SIZE);
        $crate::StackBPRB::<$t, __PHOROS_STATE_SIZE, __PHOROS_MAX_ENCODED, $arena_size>::new_stack(
            $anchor_interval,
        )
    }};
}
