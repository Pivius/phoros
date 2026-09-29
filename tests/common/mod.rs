#![allow(dead_code)]

extern crate alloc;

use phoros::BPRB;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DocState {
    pub cursor: u64,
    pub line_digests: [u8; 80],
}

pub const DOC_STATE_SIZE: usize = core::mem::size_of::<DocState>();
pub const DOC_MAX_ENCODED: usize = 10 * (DOC_STATE_SIZE + 7).div_ceil(8) + 1;

#[cfg(feature = "alloc")]
pub type DocBuf = BPRB<DocState, alloc::boxed::Box<[u8]>, DOC_STATE_SIZE, DOC_MAX_ENCODED>;

/// Deterministic state for frame `i`.
pub fn make_state(i: usize) -> DocState {
    DocState {
        cursor: i as u64,
        line_digests: {
            let mut d = [0u8; 80];
            d[0] = i as u8;
            d[79] = (i * 7) as u8;
            d
        },
    }
}

/// State where every byte differs from the previous one
pub fn make_churned_state(i: usize) -> DocState {
    DocState {
        cursor: u64::MAX / (i as u64 + 1),
        line_digests: {
            let mut d = [0u8; 80];
            for (j, b) in d.iter_mut().enumerate() {
                *b = ((i * 131 + j * 17) % 256) as u8;
            }
            d
        },
    }
}

#[cfg(feature = "alloc")]
pub fn buf(anchor_interval: u64, arena_bytes: usize) -> DocBuf {
    DocBuf::new_boxed(arena_bytes, anchor_interval)
        .unwrap_or_else(|e| panic!("failed to create buffer: {e:?}"))
}
