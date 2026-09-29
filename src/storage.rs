/// Trait for arena backing storage.
///
/// Implementations decide where the byte arena lives — stack, heap, or borrowed
/// memory — while the ring buffer logic stays generic over this trait.
///
/// # Examples
///
/// ```
/// use phoros::ArenaStorage;
///
/// let mut buf = [0u8; 1024];
/// assert_eq!(buf.len(), 1024);
/// assert_eq!(buf.as_slice().len(), 1024);
/// buf.as_mut_slice()[0] = 42;
/// ```
pub trait ArenaStorage {
	/// Returns the number of bytes in the storage.
	fn len(&self) -> usize;

	/// Borrow the storage as an immutable byte slice.
	fn as_slice(&self) -> &[u8];

	/// Borrow the storage as a mutable byte slice.
	fn as_mut_slice(&mut self) -> &mut [u8];
}

impl<const N: usize> ArenaStorage for [u8; N] {
	#[inline]
	fn len(&self) -> usize {
		N
	}
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self
	}
	#[inline]
	fn as_mut_slice(&mut self) -> &mut [u8] {
		self
	}
}

impl ArenaStorage for [u8] {
	#[inline]
	fn len(&self) -> usize {
		(*self).len()
	}
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self
	}
	#[inline]
	fn as_mut_slice(&mut self) -> &mut [u8] {
		self
	}
}

#[cfg(feature = "alloc")]
impl ArenaStorage for alloc::boxed::Box<[u8]> {
	#[inline]
	fn len(&self) -> usize {
		(**self).len()
	}
	#[inline]
	fn as_slice(&self) -> &[u8] {
		self
	}
	#[inline]
	fn as_mut_slice(&mut self) -> &mut [u8] {
		self
	}
}
