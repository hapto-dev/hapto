//! A small inline byte buffer.

use std::hash::{Hash, Hasher};

/// Up to `N` bytes stored inline, such as a message that isn't complete yet.
///
/// Two buffers compare and hash by the bytes that they hold.
#[derive(Clone, Copy)]
pub(crate) struct Buf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Buf<N> {
    pub(crate) const fn new() -> Self {
        Buf {
            bytes: [0; N],
            len: 0,
        }
    }

    /// Appends `byte`, or returns false if the buffer is full.
    pub(crate) fn push(&mut self, byte: u8) -> bool {
        let Some(slot) = self.bytes.get_mut(self.len) else {
            return false;
        };
        *slot = byte;
        // `len` is below N, because `get_mut` found a slot there.
        self.len = self.len.saturating_add(1);
        true
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn is_full(&self) -> bool {
        self.len >= N
    }

    pub(crate) const fn clear(&mut self) {
        self.len = 0;
    }
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Buf::new()
    }
}

impl<const N: usize> PartialEq for Buf<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const N: usize> Eq for Buf<N> {}

impl<const N: usize> Hash for Buf<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<const N: usize> std::fmt::Debug for Buf<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02X?}", self.as_slice())
    }
}
