#![cfg(feature = "alloc")]

mod common;

use common::*;

/// Reconstruct every frame still resident in the live window.
fn assert_live_window_reconstructs(b: &mut DocBuf, frames: &[DocState]) {
	let oldest = b.oldest_frame().expect("buffer should have live frames");
	let newest = b.newest_frame().unwrap();
	for f in oldest..=newest {
		assert_eq!(b.rollback_to(f).unwrap(), frames[f as usize], "frame {f}");
	}
}

#[test]
fn wraps_repeatedly_across_geometries() {
	let configs = [(313usize, 4u64), (512, 7), (999, 3), (1000, 60)];

	for (arena, interval) in configs {
		let mut b = buf(interval, arena);
		let frames: Vec<_> = (0..120).map(make_state).collect();
		for s in &frames {
			b.snapshot(s).unwrap();
		}
		assert_live_window_reconstructs(&mut b, &frames);
	}
}

#[test]
fn straddling_full_snapshot_anchor_reads_back() {
	let mut b = buf(4, 200);
	let frames: Vec<_> = (0..6).map(make_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}
	assert_eq!(b.rollback_to(5).unwrap(), frames[5]);
	assert_eq!(b.rollback_to(4).unwrap(), frames[4]);
}

#[test]
fn tiny_arena_still_holds_latest_window() {
	let mut b = buf(2, 168); // fits at most one 104-byte slot + slack
	let frames: Vec<_> = (0..50).map(make_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}
	assert_live_window_reconstructs(&mut b, &frames);
}
