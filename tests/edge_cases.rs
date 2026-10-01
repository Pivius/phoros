#![cfg(feature = "alloc")]

mod common;

use common::*;
use phoros::{BufferError, RollbackError};

#[test]
fn empty_buffer_rejects_rollback() {
    let mut b = buf(60, 4096);
    assert!(b.is_empty());
    assert_eq!(b.len(), 0);
    assert_eq!(b.current_entry(), 0);
    assert_eq!(b.oldest_entry(), None);
    assert_eq!(b.newest_entry(), None);
    assert_eq!(b.rollback_to(0).err(), Some(RollbackError::EntryEvicted));
}

#[test]
fn rejects_arena_smaller_than_one_slot() {
    let need = 16 + DOC_STATE_SIZE;
    assert!(matches!(
        DocBuf::new_boxed(need - 1, 60),
        Err(BufferError::ArenaFull)
    ));
    assert!(DocBuf::new_boxed(need, 60).is_ok());
}

#[test]
fn out_of_range_entry_is_not_available() {
    let mut b = buf(60, 64 * 1024);
    b.snapshot(&make_state(0)).unwrap();
    b.snapshot(&make_state(1)).unwrap();
    // entry 5 was never written.
    assert_eq!(b.rollback_to(5).err(), Some(RollbackError::EntryEvicted));
}

#[test]
fn single_entry_window_bookkeeping() {
    let mut b = buf(60, 64 * 1024);
    let s = make_state(0);
    b.snapshot(&s).unwrap();

    assert!(!b.is_empty());
    assert_eq!(b.len(), 1);
    assert_eq!(b.current_entry(), 1);
    assert_eq!(b.oldest_entry(), Some(0));
    assert_eq!(b.newest_entry(), Some(0));
    assert_eq!(b.rollback_to(0).unwrap(), s);
}

#[test]
fn identical_states_still_roundtrip() {
    let mut b = buf(60, 64 * 1024);
    let s = make_state(7);
    for _ in 0..100 {
        b.snapshot(&s).unwrap();
    }
    for f in [0u64, 50, 99] {
        assert_eq!(b.rollback_to(f).unwrap(), s, "entry {f}");
    }
    assert_eq!(b.newest_entry(), Some(99));
    assert_eq!(b.oldest_entry(), Some(0));
}

#[test]
fn window_advances_with_entries() {
    let mut b = buf(60, 64 * 1024);
    for i in 0..10 {
        b.snapshot(&make_state(i)).unwrap();
    }
    assert_eq!(b.current_entry(), 10);
    assert_eq!(b.newest_entry(), Some(9));
    assert_eq!(b.oldest_entry(), Some(0));
}

#[test]
fn memory_usage_covers_arena() {
    let b = buf(60, 1234);
    assert!(b.memory_usage() > 1234);
}

#[test]
fn apply_delta_rejects_oversized_delta() {
    let mut b = buf(60, 64 * 1024);
    b.snapshot(&make_state(0)).unwrap();
    let big = vec![0u8; DOC_STATE_SIZE + 1];
    assert!(b.apply_delta(0, &big).is_err());
}
