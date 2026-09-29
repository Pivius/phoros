#![cfg(feature = "alloc")]

mod common;

use common::*;
use phoros::RollbackError;

#[test]
fn eviction_drops_oldest_and_keeps_window_valid() {
	let mut b = buf(8, 2048);
	let n = 200;
	let frames: Vec<_> = (0..n).map(make_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}

	assert_eq!(b.newest_frame(), Some(n as u64 - 1));
	let oldest = b.oldest_frame().unwrap();
	assert!(oldest > 0, "expected eviction, oldest={oldest}");
	assert!(!b.is_empty() && b.len() < n);

	for f in oldest..=(n as u64 - 1) {
		assert_eq!(b.rollback_to(f).unwrap(), frames[f as usize], "frame {f}");
	}
}

#[test]
fn evicted_frames_return_frame_evicted() {
	let mut b = buf(8, 2048);
	let n = 200;
	for i in 0..n {
		b.snapshot(&make_state(i)).unwrap();
	}
	let oldest = b.oldest_frame().unwrap();

	match b.rollback_to(oldest - 1) {
		Err(RollbackError::FrameEvicted) => {}
		other => panic!("expected FrameEvicted for {oldest}, got {other:?}"),
	}
	match b.rollback_to(0) {
		Err(RollbackError::FrameEvicted) => {}
		other => panic!("expected FrameEvicted for 0, got {other:?}"),
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

	if let Some(oldest) = b.oldest_frame() {
		for f in oldest..=79 {
			assert_eq!(b.rollback_to(f).unwrap(), seen[f as usize], "frame {f}");
		}
	}
}
