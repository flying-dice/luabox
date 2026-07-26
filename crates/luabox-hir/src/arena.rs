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
    #[expect(
        clippy::expect_used,
        reason = "len is the only growth path and is bounded by u32::MAX; a Vec of 2^32 elements would exhaust memory long before this conversion could fail"
    )]
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
    #[expect(
        clippy::expect_used,
        reason = "enumerate indices are < data.len(), which alloc bounded to <= u32::MAX, so the conversion cannot fail"
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

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;

    fn hash_of<H: Hash>(value: &H) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn alloc_returns_handles_in_allocation_order() {
        let mut arena = Arena::new();
        let a = arena.alloc("a");
        let b = arena.alloc("b");
        assert_eq!(a.raw(), 0);
        assert_eq!(b.raw(), 1);
        assert_eq!(arena[a], "a");
        assert_eq!(arena[b], "b");
    }

    #[test]
    fn a_fresh_arena_is_empty_and_default_matches_new() {
        let mut arena: Arena<u8> = Arena::default();
        assert!(arena.is_empty());
        assert_eq!(arena.len(), 0);
        assert_eq!(arena.iter().count(), 0);

        arena.alloc(7);
        assert!(!arena.is_empty());
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn index_mut_writes_through_the_handle() {
        let mut arena = Arena::new();
        let id = arena.alloc(1_u32);
        arena[id] += 41;
        assert_eq!(arena[id], 42);
    }

    #[test]
    fn iter_pairs_each_value_with_its_own_handle() {
        let mut arena = Arena::new();
        let ids: Vec<_> = (0..3).map(|i| arena.alloc(i * 10)).collect();
        let seen: Vec<_> = arena.iter().map(|(id, v)| (id, *v)).collect();
        assert_eq!(seen, vec![(ids[0], 0), (ids[1], 10), (ids[2], 20)]);
    }

    #[test]
    fn idx_orders_and_hashes_by_its_raw_index() {
        let mut arena = Arena::new();
        let a = arena.alloc('a');
        let b = arena.alloc('b');

        assert!(a < b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Less);
        assert_eq!(b.cmp(&a), std::cmp::Ordering::Greater);
        assert_eq!(a.cmp(&a), std::cmp::Ordering::Equal);
        assert_eq!(a.partial_cmp(&b), Some(std::cmp::Ordering::Less));

        let mut sorted = vec![b, a];
        sorted.sort_unstable();
        assert_eq!(sorted, vec![a, b]);

        // Equal handles hash equally, which is what the side tables keyed by
        // `Idx` (source map, resolutions) rely on.
        assert_eq!(hash_of(&a), hash_of(&Idx::<char>::from_raw(a.raw())));
        assert_ne!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn idx_is_copy_and_compares_structurally_not_by_identity() {
        let a = Idx::<u8>::from_raw(3);
        let copy = a;
        #[expect(
            clippy::clone_on_copy,
            reason = "exercises the hand-written Clone impl"
        )]
        let cloned = a.clone();
        assert_eq!(a, copy);
        assert_eq!(a, cloned);
        assert_ne!(a, Idx::<u8>::from_raw(4));
    }

    #[test]
    fn idx_debug_shows_the_raw_index_without_the_element_type() {
        assert_eq!(format!("{:?}", Idx::<String>::from_raw(12)), "Idx(12)");
    }
}
