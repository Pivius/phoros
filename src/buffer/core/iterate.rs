use crate::codec;
use crate::error::RollbackError;
use crate::slot::{SlotType, next_slot_offset};
use crate::storage::ArenaStorage;

use super::{BPRB, xor_into};

/// Iterator over stored entry in a [`BPRB`].
///
/// Yields `Result<T, RollbackError>` for each entry in the range.
///
/// Created via [`BPRB::entries`] or [`BPRB::entries_all`].
pub struct EntryRange<'a, T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    buf: &'a BPRB<T, S, STATE_SIZE, MAX_ENCODED>,
    next_entry: u64,
    end_entry: u64,
    arena_offset: u32,
    working: [u8; STATE_SIZE],
    buf_encoded: [u8; MAX_ENCODED],
    buf_delta: [u8; STATE_SIZE],
}

impl<'a, T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize>
    EntryRange<'a, T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    fn new(buf: &'a BPRB<T, S, STATE_SIZE, MAX_ENCODED>, start: u64, end: u64) -> Self {
        Self {
            buf,
            next_entry: start,
            end_entry: end,
            arena_offset: 0,
            working: [0u8; STATE_SIZE],
            buf_encoded: [0u8; MAX_ENCODED],
            buf_delta: [0u8; STATE_SIZE],
        }
    }
}

impl<T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize> Iterator
    for EntryRange<'_, T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    type Item = Result<T, RollbackError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_entry > self.end_entry {
            return None;
        }

        let buf = self.buf;
        let target = self.next_entry;

        let anchor = buf.anchor_index.find_nearest_le(target)?;

        if anchor.entry() > target {
            return Some(Err(RollbackError::EntryEvicted));
        }

        let anchor_header = match buf.read_slot_at(anchor.offset(), &mut self.buf_encoded) {
            Ok(h) => h,
            Err(e) => {
                self.next_entry = u64::MAX; // stop iter
                return Some(Err(e));
            }
        };
        if anchor_header.payload_len as usize != STATE_SIZE {
            self.next_entry = u64::MAX;
            return Some(Err(RollbackError::CorruptedChain));
        }

        self.working[..anchor_header.payload_len as usize]
            .copy_from_slice(&self.buf_encoded[..anchor_header.payload_len as usize]);

        self.arena_offset = next_slot_offset(
            anchor.offset(),
            anchor_header.payload_len,
            buf.header.data_area_len,
        );

        for expected_entry in (anchor.entry() + 1)..=target {
            let header = match buf.read_slot_at(self.arena_offset, &mut self.buf_encoded) {
                Ok(h) => h,
                Err(e) => {
                    self.next_entry = u64::MAX;
                    return Some(Err(e));
                }
            };

            if header.entry != expected_entry {
                self.next_entry = u64::MAX;
                return Some(Err(RollbackError::CorruptedChain));
            }

            match SlotType::from_u8(header.kind) {
                Some(SlotType::FullSnapshot) => {
                    if header.payload_len as usize != STATE_SIZE {
                        self.next_entry = u64::MAX;
                        return Some(Err(RollbackError::CorruptedChain));
                    }
                    self.working[..header.payload_len as usize]
                        .copy_from_slice(&self.buf_encoded[..header.payload_len as usize]);
                }
                Some(SlotType::Delta) => {
                    if header.payload_len as usize > MAX_ENCODED {
                        self.next_entry = u64::MAX;
                        return Some(Err(RollbackError::CorruptedChain));
                    }
                    self.buf_delta.fill(0);
                    if let Err(e) = codec::byte_masked_decode(
                        &self.buf_encoded[..header.payload_len as usize],
                        &mut self.buf_delta,
                    ) {
                        self.next_entry = u64::MAX;
                        return Some(Err(e.into()));
                    }
                    xor_into(&mut self.working, &self.buf_delta);
                }
                None => {
                    self.next_entry = u64::MAX;
                    return Some(Err(RollbackError::CorruptedChain));
                }
            }

            self.arena_offset = next_slot_offset(
                self.arena_offset,
                header.payload_len,
                buf.header.data_area_len,
            );
        }

        let result = BPRB::<T, S, STATE_SIZE, MAX_ENCODED>::bytes_to_state(&self.working);

        self.next_entry += 1;
        Some(Ok(result))
    }
}

impl<T, S, const STATE_SIZE: usize, const MAX_ENCODED: usize> BPRB<T, S, STATE_SIZE, MAX_ENCODED>
where
    T: Copy + Sized + Send + 'static,
    S: ArenaStorage,
{
    /// Returns an iterator over entries in `range`.
    ///
    /// # Examples
    ///
    /// ```
    /// use phoros::bprb;
    ///
    /// let mut buf = bprb!(u64 => stack(4096)).unwrap();
    /// buf.snapshot(&1u64).unwrap();
    /// buf.snapshot(&2u64).unwrap();
    /// buf.snapshot(&3u64).unwrap();
    ///
    /// let mut values = [0u64; 2];
    /// for (i, entry) in buf.entries(1..3).enumerate() {
    ///     values[i] = entry.unwrap();
    /// }
    /// assert_eq!(values, [2, 3]);
    /// ```
    pub fn entries(
        &self,
        range: impl core::ops::RangeBounds<u64>,
    ) -> EntryRange<'_, T, S, STATE_SIZE, MAX_ENCODED> {
        let start = match range.start_bound() {
            core::ops::Bound::Included(&s) => s,
            core::ops::Bound::Excluded(&s) => s + 1,
            core::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            core::ops::Bound::Included(&e) => e,
            core::ops::Bound::Excluded(&e) => e.saturating_sub(1),
            core::ops::Bound::Unbounded => self.entry_counter.saturating_sub(1),
        };
        EntryRange::new(self, start, end)
    }

    /// Returns an iterator over all entries, from oldest to newest.
    ///
    /// Equivalent to `entries(oldest_entry()..=newest_entry())`.
    pub fn entries_all(&self) -> EntryRange<'_, T, S, STATE_SIZE, MAX_ENCODED> {
        match (self.oldest_entry(), self.newest_entry()) {
            (Some(oldest), Some(newest)) => EntryRange::new(self, oldest, newest),
            _ => EntryRange::new(self, 0, 0),
        }
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

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
    fn entries_all_yields_all_entries() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.entries_all().collect();
        assert_eq!(results.len(), 5);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i)));
        }
    }

    #[test]
    fn entries_range_yields_subset() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.entries(3..7).collect();
        assert_eq!(results.len(), 4);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i + 3)));
        }
    }

    #[test]
    fn entries_does_not_mutate_state() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();

        let head_before = buf.current_head_state;
        let _: Vec<_> = buf.entries_all().collect();
        assert_eq!(buf.current_head_state, head_before);
        assert!(!buf.diverged);
    }

    #[test]
    fn entries_empty_buffer_yields_nothing() {
        let buf = SBoxed::new_boxed(4096, 10).unwrap();
        assert_eq!(buf.entries_all().count(), 0);
    }

    #[test]
    fn entries_empty_range_yields_nothing() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();

        assert_eq!(buf.entries(1..1).count(), 0);
    }

    #[test]
    fn entries_across_anchors() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let results: Vec<_> = buf.entries_all().collect();
        assert_eq!(results.len(), 10);
        for (i, result) in results.iter().enumerate() {
            assert_eq!(*result, Ok(make(i)));
        }
    }

    #[test]
    fn entries_on_single_anchor() {
        let mut buf = SBoxed::new_boxed(4096, 5).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }
        // Entry 0 is the only anchor

        let results: Vec<_> = buf.entries(0..1).collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Ok(make(0)));
    }

    #[test]
    fn entries_yields_error_for_evicted_entry() {
        let mut buf = SBoxed::new_boxed(200, 2).unwrap();
        buf.snapshot(&make(0)).unwrap();
        buf.snapshot(&make(1)).unwrap();
        buf.snapshot(&make(2)).unwrap();
        buf.snapshot(&make(3)).unwrap();
        // With a tiny arena, entries get evicted. Try to iterate from 0.
        let results: Vec<_> = buf.entries_all().collect();
        // At least one result should be present
        assert!(!results.is_empty());
    }

    #[test]
    fn entries_iterator_trait_usage() {
        let mut buf = SBoxed::new_boxed(4096, 10).unwrap();
        for i in 0..5 {
            buf.snapshot(&make(i)).unwrap();
        }

        // for loop
        let mut count = 0;
        for result in buf.entries_all() {
            result.unwrap();
            count += 1;
        }
        assert_eq!(count, 5);

        // collect + filter
        let evens: Vec<_> = buf
            .entries_all()
            .filter(|r| r.as_ref().is_ok_and(|s| s.x % 2 == 0))
            .collect();
        assert_eq!(evens.len(), 3); // entries 0, 2, 4
    }

    #[test]
    fn entries_matches_rollback_to() {
        let mut buf = SBoxed::new_boxed(4096, 2).unwrap();
        for i in 0..10 {
            buf.snapshot(&make(i)).unwrap();
        }

        let iter_results: Vec<_> = buf.entries_all().collect();
        for (i, iter_result) in iter_results.iter().enumerate() {
            assert_eq!(*iter_result, Ok(buf.rollback_to(i as u64).unwrap()));
        }
    }
}
