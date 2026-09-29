#![cfg(feature = "alloc")]

mod common;

use common::*;

#[test]
fn single_snapshot_roundtrip() {
	let mut b = buf(60, 64 * 1024);
	let s = make_state(0);
	b.snapshot(&s).unwrap();
	assert_eq!(b.rollback_to(0).unwrap(), s);
}

#[test]
fn many_snapshots_roundtrip_all() {
	let mut b = buf(60, 128 * 1024);
	let frames: Vec<_> = (0..50).map(make_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}
	for (i, expected) in frames.iter().enumerate() {
		assert_eq!(b.rollback_to(i as u64).unwrap(), *expected, "frame {i}");
	}
}

#[test]
fn rollback_across_anchor_boundary() {
	let mut b = buf(10, 128 * 1024);
	let frames: Vec<_> = (0..35).map(make_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}
	for target in [9, 10, 11, 19, 20, 21, 34] {
		assert_eq!(b.rollback_to(target).unwrap(), frames[target as usize], "target {target}");
	}
}

#[test]
fn interleaved_rollback_then_snapshot_keeps_chain_valid() {
	let mut b = buf(60, 128 * 1024);
	let a = make_state(1);
	let base = make_state(2);
	let next = make_state(3);
	b.snapshot(&make_state(0)).unwrap();
	b.snapshot(&a).unwrap();
	b.snapshot(&base).unwrap();

	assert_eq!(b.rollback_to(1).unwrap(), a); // rewind

	b.snapshot(&next).unwrap(); // recorded against current head (frame 1's state)
	assert_eq!(b.rollback_to(1).unwrap(), a);
	assert_eq!(b.rollback_to(3).unwrap(), next);
}

#[test]
fn full_snapshot_midchain_is_replaced_not_xored() {
	let mut b = buf(1000, 128 * 1024); // huge interval: anchors only via threshold
	let frames: Vec<_> = (0..20).map(make_churned_state).collect();
	for s in &frames {
		b.snapshot(s).unwrap();
	}
	for (i, expected) in frames.iter().enumerate() {
		assert_eq!(b.rollback_to(i as u64).unwrap(), *expected, "frame {i}");
	}
}

#[test]
fn apply_delta_xors_into_reconstructed_state() {
	let mut b = buf(60, 128 * 1024);
	let s0 = make_state(10);
	b.snapshot(&s0).unwrap();

	let mut delta = vec![0u8; DOC_STATE_SIZE];
	delta[0] ^= 0xAA; // flip bits in the cursor word
	let applied = b.apply_delta(0, &delta).unwrap();

	let mut expected_bytes = bytemuck_like(&s0);
	expected_bytes[0] ^= 0xAA;
	assert_eq!(applied, from_bytes_like(&expected_bytes));
}

#[test]
fn apply_delta_rejects_wrong_length() {
	let mut b = buf(60, 128 * 1024);
	b.snapshot(&make_state(0)).unwrap();
	let wrong = vec![0u8; 3];
	assert!(b.apply_delta(0, &wrong).is_err());
}

fn bytemuck_like(s: &DocState) -> Vec<u8> {
	let ptr = s as *const DocState as *const u8;
	unsafe { core::slice::from_raw_parts(ptr, DOC_STATE_SIZE).to_vec() }
}

fn from_bytes_like(bytes: &[u8]) -> DocState {
	assert_eq!(bytes.len(), DOC_STATE_SIZE);
	unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const DocState) }
}
