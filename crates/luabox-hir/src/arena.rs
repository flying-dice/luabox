//! A minimal index-based arena (rust-analyzer style): a `Vec` of values plus
//! typed [`Idx`] handles. No `Rc`, no cycles — HIR nodes reference each other
//! only through these indices, so the whole graph is a plain owned tree that
//! is cheap to clone and trivially serializable.

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

/// A typed handle into an [`Arena<T>`].
///
/// `Idx` is a `u32` under the hood; the phantom `fn() -> T` keeps it invariant
/// and `Send`/`Sync` without dragging `T`'s bounds onto the derives (which is
/// why the trait impls below are hand-written rather than derived).
pub struct Idx<T> {
    raw: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Idx<T> {
    /// Wrap a raw index. Only [`Arena`] (and id-reservation in the lowerer)
    /// should mint these.
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    /// The underlying `u32` index.
    pub fn raw(self) -> u32 {
        self.raw
    }
}

impl<T> Clone for Idx<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Idx<T> {}

impl<T> PartialEq for Idx<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for Idx<T> {}

impl<T> PartialOrd for Idx<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Idx<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}

impl<T> Hash for Idx<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<T> std::fmt::Debug for Idx<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Idx({})", self.raw)
    }
}

/// A growable pool of `T` addressed by [`Idx<T>`].
#[derive(Debug, Clone)]
pub struct Arena<T> {
    data: Vec<T>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// Append `value`, returning its stable handle.
    ///
    /// # Panics
    /// If the arena would exceed `u32::MAX` elements.
    pub fn alloc(&mut self, value: T) -> Idx<T> {
        let raw = u32::try_from(self.data.len()).expect("arena index overflowed u32");
        self.data.push(value);
        Idx::from_raw(raw)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Iterate values paired with their handles, in allocation order.
    #[allow(
        clippy::missing_panics_doc,
        reason = "alloc already enforces len <= u32::MAX; the conversion cannot fail"
    )]
    pub fn iter(&self) -> impl Iterator<Item = (Idx<T>, &T)> {
        self.data.iter().enumerate().map(|(i, v)| {
            let raw = u32::try_from(i).expect("arena index overflowed u32");
            (Idx::from_raw(raw), v)
        })
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Index<Idx<T>> for Arena<T> {
    type Output = T;

    fn index(&self, index: Idx<T>) -> &T {
        &self.data[index.raw as usize]
    }
}

impl<T> IndexMut<Idx<T>> for Arena<T> {
    fn index_mut(&mut self, index: Idx<T>) -> &mut T {
        &mut self.data[index.raw as usize]
    }
}
