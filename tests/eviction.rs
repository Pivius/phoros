#![cfg(feature = "alloc")]

mod common;

use common::*;
use phoros::RollbackError;

#[test]
fn eviction_drops_oldest_and_keeps_window_valid() {
    let mut b = buf(8, 2048);
    let n = 200;
    let entries: Vec<_> = (0..n).map(make_state).collect();
    for s in &entries {
        b.snapshot(s).unwrap();
    }

    assert_eq!(b.end(), Some(n as u64 - 1));
    let oldest = b.start().unwrap();
    assert!(oldest > 0, "expected eviction, oldest={oldest}");
    assert!(!b.is_empty() && b.len() < n);

    for f in oldest..=(n as u64 - 1) {
        assert_eq!(b.rollback(f).unwrap(), entries[f as usize], "entry {f}");
    }
}

#[test]
fn evicted_entries_return_entry_evicted() {
    let mut b = buf(8, 2048);
    let n = 200;
    for i in 0..n {
        b.snapshot(&make_state(i)).unwrap();
    }
    let oldest = b.start().unwrap();

    match b.rollback(oldest - 1) {
        Err(RollbackError::EntryEvicted) => {}
        other => panic!("expected EntryEvicted for {oldest}, got {other:?}"),
    }
    match b.rollback(0) {
        Err(RollbackError::EntryEvicted) => {}
        other => panic!("expected EntryEvicted for 0, got {other:?}"),
    }
}

#[test]
fn delta_chains_never_orphan_after_repeated_eviction() {
    let mut b = buf(5, 1500);
    let mut seen = Vec::new();
    for i in 0..80u64 {
        b.snapshot(&make_state(i as usize)).unwrap();
        seen.push(make_state(i as usize));
    }

    if let Some(oldest) = b.start() {
        for f in oldest..=79 {
            assert_eq!(b.rollback(f).unwrap(), seen[f as usize], "entry {f}");
        }
    }
}
