use std::{fmt, write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackError {
	FrameEvicted,
	CorruptedChain,
	ArenaCorrupted,
}

impl fmt::Display for RollbackError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			RollbackError::FrameEvicted => write!(f, "frame evicted: anchor no longer in arena"),
			RollbackError::CorruptedChain => write!(f, "corrupted chain: slot sequence broken"),
			RollbackError::ArenaCorrupted => write!(f, "arena corrupted: header inconsistency"),
		}
	}
}

impl std::error::Error for RollbackError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
	ArenaFull,
	InvalidState,
}

impl fmt::Display for BufferError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			BufferError::ArenaFull => write!(f, "arena full: no space available after eviction"),
			BufferError::InvalidState => {
				write!(f, "invalid state: T must be Copy + Sized with no drop logic")
			}
		}
	}
}

impl std::error::Error for BufferError {}
