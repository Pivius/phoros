use core::fmt;

/// Errors that can occur during [`rollback`](crate::BPRB::rollback) and
/// [`apply_delta`](crate::BPRB::apply_delta).
///
/// # Examples
///
/// ```
/// use phoros::RollbackError;
///
/// let err = RollbackError::EntryEvicted;
/// assert_eq!(err.to_string(), "entry evicted: anchor no longer in arena");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RollbackError {
    /// The requested entry has been evicted from the arena.
    EntryEvicted,
    /// A slot in the delta chain is corrupted or has an unexpected index.
    CorruptedChain,
    /// The arena header is unreadable or invalid.
    ArenaCorrupted,
    /// An error occurred in the bitstream codec.
    Codec(CodecError),
}

impl fmt::Display for RollbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RollbackError::EntryEvicted => f.write_str("entry evicted: anchor no longer in arena"),
            RollbackError::CorruptedChain => f.write_str("corrupted chain: slot sequence broken"),
            RollbackError::ArenaCorrupted => f.write_str("arena corrupted: header inconsistency"),
            RollbackError::Codec(e) => write!(f, "codec error: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RollbackError {}

/// Errors that can occur during buffer construction and
/// [`snapshot`](crate::BPRB::snapshot).
///
/// # Examples
///
/// ```
/// use phoros::{bprb, BufferError};
///
/// let err = bprb!(u64 => stack(1)).unwrap_err();
/// assert!(matches!(err, BufferError::ArenaFull));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BufferError {
    /// The arena is too small to hold even one slot.
    ArenaFull,
    /// The state type `T` has drop logic, which is not supported.
    InvalidState,
    /// An error occurred in the bitstream codec.
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

/// Errors in the bitstream encoder/decoder.
///
/// # Examples
///
/// ```
/// use phoros::CodecError;
///
/// let err = CodecError::BitstreamOverflow;
/// assert_eq!(err.to_string(), "bitstream overflow: writer buffer full");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CodecError {
    /// The write buffer is full.
    BitstreamOverflow,
    /// The read buffer has been exhausted.
    BitstreamUnderflow,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::BitstreamOverflow => f.write_str("bitstream overflow: writer buffer full"),
            CodecError::BitstreamUnderflow => {
                f.write_str("bitstream underflow: reader buffer exhausted")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CodecError {}
