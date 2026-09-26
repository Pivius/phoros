use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackError {
	FrameEvicted,
	CorruptedChain,
	ArenaCorrupted,
	Codec(CodecError),
}

impl fmt::Display for RollbackError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			RollbackError::FrameEvicted => f.write_str("frame evicted: anchor no longer in arena"),
			RollbackError::CorruptedChain => f.write_str("corrupted chain: slot sequence broken"),
			RollbackError::ArenaCorrupted => f.write_str("arena corrupted: header inconsistency"),
			RollbackError::Codec(e) => write!(f, "codec error: {e}"),
		}
	}
}

#[cfg(feature = "std")]
impl std::error::Error for RollbackError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
	ArenaFull,
	InvalidState,
	Codec(CodecError),
}

impl fmt::Display for BufferError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			BufferError::ArenaFull => f.write_str("arena full: no space available after eviction"),
			BufferError::InvalidState => {
				f.write_str("invalid state: T must be Copy + Sized with no drop logic")
			}
			BufferError::Codec(e) => write!(f, "codec error: {e}"),
		}
	}
}

#[cfg(feature = "std")]
impl std::error::Error for BufferError {}

impl From<CodecError> for RollbackError {
	fn from(e: CodecError) -> Self {
		RollbackError::Codec(e)
	}
}

impl From<CodecError> for BufferError {
	fn from(e: CodecError) -> Self {
		BufferError::Codec(e)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
	BitstreamOverflow,
	BitstreamUnderflow,
}

impl fmt::Display for CodecError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			CodecError::BitstreamOverflow => f.write_str("bitstream overflow: writer buffer full"),
			CodecError::BitstreamUnderflow => f.write_str("bitstream underflow: reader buffer exhausted"),
		}
	}
}

#[cfg(feature = "std")]
impl std::error::Error for CodecError {}
