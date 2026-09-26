/// Trait for arena backing storage.
///
/// Implementations decide where the byte arena lives — stack, heap, or borrowed
/// memory — while the ring buffer logic stays generic over this trait.
pub trait ArenaStorage {
	fn len(&self) -> usize;
	fn as_slice(&self) -> &[u8];
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
