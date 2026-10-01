pub const MAX_ANCHORS: usize = 32;

/// An anchor entry mapping an entry to a offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AnchorEntry {
    entry: u64,
    offset: u32,
}

impl AnchorEntry {
    #[inline]
    pub fn entry(self) -> u64 {
        self.entry
    }

    #[inline]
    pub fn offset(self) -> u32 {
        self.offset
    }
}

/// # Examples
///
/// ```
/// use phoros::BPRB;
///
/// let mut buf = BPRB::<u64, [u8; 4096], 8, 11>::new_stack(10).unwrap();
/// buf.snapshot(&1u64).unwrap();
/// buf.snapshot(&2u64).unwrap();
/// buf.snapshot(&3u64).unwrap();
/// assert_eq!(buf.rollback_to(1).unwrap(), 2u64);
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AnchorIndex {
    entries: [Option<AnchorEntry>; MAX_ANCHORS],
    count: usize,
}

impl AnchorIndex {
    /// Create an empty index.
    ///
    /// # Examples
    ///
    /// ```
    /// use phoros::BPRB;
    ///
    /// # fn example() -> Result<(), phoros::BufferError> {
    /// let mut buf = BPRB::<u64, [u8; 4096], 8, 11>::new_stack(10)?;
    /// assert!(buf.is_empty());
    /// buf.snapshot(&42u64)?;
    /// assert_eq!(buf.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Self {
        Self {
            entries: [None; MAX_ANCHORS],
            count: 0,
        }
    }

    /// Insert an anchor.
    ///
    /// Returns `false` if the index is full.
    pub fn insert(&mut self, entry_num: u64, offset: u32) -> bool {
        if self.count >= MAX_ANCHORS {
            return false;
        }

        let entry = AnchorEntry {
            entry: entry_num,
            offset,
        };

        let pos = self.entries[..self.count]
            .iter()
            .position(|e| e.is_some_and(|e| e.entry > entry_num))
            .unwrap_or(self.count);

        for i in (pos..self.count).rev() {
            self.entries[i + 1] = self.entries[i];
        }

        self.entries[pos] = Some(entry);
        self.count += 1;
        true
    }

    /// Remove the entry written at `offset`. Returns `true` if found.
    pub fn remove_by_offset(&mut self, offset: u32) -> bool {
        let pos = self.entries[..self.count]
            .iter()
            .position(|e| e.is_some_and(|e| e.offset == offset));

        if let Some(pos) = pos {
            for i in pos..self.count - 1 {
                self.entries[i] = self.entries[i + 1];
            }
            self.entries[self.count - 1] = None;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// Binary search for the nearest anchor with `entry <= target_entry`.
    #[inline]
    pub fn find_nearest_le(&self, target_entry: u64) -> Option<AnchorEntry> {
        if self.count == 0 {
            return None;
        }

        let mut lo = 0usize;
        let mut hi = self.count;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry = self.entries[mid].unwrap();

            if entry.entry <= target_entry {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        if lo == 0 { None } else { self.entries[lo - 1] }
    }

    /// Drop all entries with `entry < min_entry`.
    pub fn evict_before(&mut self, min_entry: u64) {
        let mut write = 0;
        for read in 0..self.count {
            if let Some(entry) = self.entries[read]
                && entry.entry >= min_entry
            {
                self.entries[write] = Some(entry);
                write += 1;
            }
        }
        for i in write..self.count {
            self.entries[i] = None;
        }
        self.count = write;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Default for AnchorIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_find() {
        let mut index = AnchorIndex::new();
        index.insert(0, 0);
        index.insert(60, 1000);
        index.insert(120, 2000);

        assert_eq!(index.find_nearest_le(0).unwrap().entry, 0);
        assert_eq!(index.find_nearest_le(30).unwrap().entry, 0);
        assert_eq!(index.find_nearest_le(60).unwrap().entry, 60);
        assert_eq!(index.find_nearest_le(90).unwrap().entry, 60);
        assert_eq!(index.find_nearest_le(120).unwrap().entry, 120);
        assert_eq!(index.find_nearest_le(200).unwrap().entry, 120);
    }

    #[test]
    fn find_nearest_empty() {
        let index = AnchorIndex::new();
        assert!(index.find_nearest_le(0).is_none());
    }

    #[test]
    fn find_nearest_before_first() {
        let mut index = AnchorIndex::new();
        index.insert(60, 0);
        assert!(index.find_nearest_le(30).is_none());
    }

    #[test]
    fn remove_by_offset() {
        let mut index = AnchorIndex::new();
        index.insert(0, 0);
        index.insert(60, 1000);

        assert!(index.remove_by_offset(0));
        assert_eq!(index.len(), 1);
        assert!(index.find_nearest_le(0).is_none());
        assert_eq!(index.find_nearest_le(60).unwrap().entry, 60);
    }

    #[test]
    fn evict_before() {
        let mut index = AnchorIndex::new();
        index.insert(0, 0);
        index.insert(60, 1000);
        index.insert(120, 2000);
        index.insert(180, 3000);

        index.evict_before(60);

        assert_eq!(index.len(), 3);
        assert!(index.find_nearest_le(0).is_none());
        assert_eq!(index.find_nearest_le(60).unwrap().entry, 60);
        assert_eq!(index.find_nearest_le(180).unwrap().entry, 180);
    }
}
