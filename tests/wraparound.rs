#![cfg(feature = "alloc")]

mod common;

use common::*;

/// Reconstruct every entry still resident in the live window.
fn assert_live_window_reconstructs(b: &mut DocBuf, entries: &[DocState]) {
    let oldest = b.start().expect("buffer should have live entries");
    let newest = b.end().unwrap();
    for f in oldest..=newest {
        assert_eq!(b.rollback(f).unwrap(), entries[f as usize], "entry {f}");
    }
}

#[test]
fn wraps_repeatedly_across_geometries() {
    let configs = [(313usize, 4u64), (512, 7), (999, 3), (1000, 60)];

    for (arena, interval) in configs {
        let mut b = buf(interval, arena);
        let entries: Vec<_> = (0..120).map(make_state).collect();
        for s in &entries {
            b.snapshot(s).unwrap();
        }
        assert_live_window_reconstructs(&mut b, &entries);
    }
}

#[test]
fn straddling_full_snapshot_anchor_reads_back() {
    let mut b = buf(4, 200);
    let entries: Vec<_> = (0..6).map(make_state).collect();
    for s in &entries {
        b.snapshot(s).unwrap();
    }
    assert_eq!(b.rollback(5).unwrap(), entries[5]);
    assert_eq!(b.rollback(4).unwrap(), entries[4]);
}

#[test]
fn tiny_arena_still_holds_latest_window() {
    let mut b = buf(2, 168); // fits at most one 104-byte slot + slack
    let entries: Vec<_> = (0..50).map(make_state).collect();
    for s in &entries {
        b.snapshot(s).unwrap();
    }
    assert_live_window_reconstructs(&mut b, &entries);
}
