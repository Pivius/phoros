#[cfg(all(feature = "alloc", feature = "serde"))]
use super::BPRB;
#[cfg(all(feature = "alloc", feature = "serde"))]
use crate::error::BufferError;
#[cfg(all(feature = "alloc", feature = "serde"))]
use crate::storage::ArenaStorage;

/// Serializable snapshot of a [`BPRB`] for persistence.
///
/// Use [`BPRB::save`] to create and [`BPRB::load`] to reconstruct.
#[cfg(all(feature = "alloc", feature = "serde"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedState {
    arena: alloc::vec::Vec<u8>,
    anchor_index: crate::index::AnchorIndex,
    entry_counter: u64,
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
            entry_counter: self.entry_counter,
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
    /// assert_eq!(restored.rollback(0).unwrap(), 42u64);
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
        bprb.entry_counter = saved.entry_counter;
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
        bprb.entry_counter = saved.entry_counter;
        bprb.delta_threshold = saved.delta_threshold;
        bprb.used_bytes = saved.used_bytes;
        bprb.diverged = saved.diverged;
        Ok(bprb)
    }
}

#[cfg(all(test, feature = "serde"))]
mod save_load_tests {
    use super::*;

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
    fn save_load_roundtrip() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        let saved = buf.save();
        let mut restored = SBoxed::load(saved).unwrap();

        assert_eq!(restored.rollback(0).unwrap(), make(0));
        assert_eq!(restored.rollback(1).unwrap(), make(1));
        assert_eq!(restored.rollback(2).unwrap(), make(2));
        assert_eq!(restored.count(), 3);
    }

    #[test]
    fn save_load_empty_buffer() {
        let buf = SBoxed::new_boxed(4096, 10).unwrap();
        let saved = buf.save();
        let restored = SBoxed::load(saved).unwrap();

        assert!(restored.is_empty());
        assert_eq!(restored.count(), 0);
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
        // Rollback to entry 0, creating divergence
        buf.rollback(0).unwrap();

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
            assert_eq!(restored.rollback(i).unwrap(), make(i as usize));
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

        assert_eq!(restored.rollback(0).unwrap(), make(42));
    }
}
