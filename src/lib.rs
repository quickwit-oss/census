//! ```rust
//! extern crate census;
//! use census::Inventory;
//!
//! fn main() {
//!     let census = Inventory::new();
//!     let value_1 = census.track(1);
//! }
//! ```
//!
//!
//!
//! let a = census.track(1);
//let b = census.track(3);
//{
//let els = census.list().map(|m| *m).collect::<Vec<_>>();
//assert_eq!(els, vec![1, 3]);
//}
//drop(b);
//{
//let els = census.list().map(|m| *m).collect::<Vec<_>>();
//assert_eq!(els, vec![1]);
//}
//let a2 = a.clone();
//drop(a);
//{
//let els = census.list().map(|m| *m).collect::<Vec<_>>();
//assert_eq!(els, vec![1]);
//}
//drop(a2);
//{
//let els = census.list().map(|m| *m).collect::<Vec<_>>();
//assert_eq!(els, vec![   ]);
//}
use std::sync::{Arc, RwLock};
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The `Inventory` register and keeps track of all of the objects alive.
pub struct Inventory<T> {
    items: Arc<RwLock<Vec<TrackedObject<T>>>>,
}

impl<T> Clone for Inventory<T> {
    fn clone(&self) -> Self {
        Inventory {
            items: self.items.clone()
        }
    }
}

impl<T> Inventory<T> {

    /// Creates a new inventory object
    pub fn new() -> Inventory<T> {
        Inventory {
            items: Arc::default()
        }
    }

    /// Takes a snapshot of the list of tracked object.
    pub fn list(&self) -> Vec<TrackedObject<T>> {
        self.items.read()
            .expect("Lock poisoned")
            .clone()
    }

    /// Tracks a given `T` object
    pub fn track(&self, t: T) -> TrackedObject<T> {
        let self_clone: Inventory<T> = (*self).clone();
        let mut wlock = self.items.write().expect("Lock poisoned");
        let idx = wlock.len();
        let managed_object = TrackedObject {
            census: self_clone,
            inner: Arc::new(Inner {
                val:t,
                count: AtomicUsize::new(0),
                idx: AtomicUsize::new(idx),
            })
        };
        wlock.push(managed_object.clone());
        managed_object
    }

    fn remove_at(&self, pos: usize) {
        let mut wlock = self.items.write().expect("Lock poisoned");
        // no need to do anything if this was the last element
        wlock.swap_remove(pos);
        if pos < wlock.len() {
            wlock[pos].set_index(pos);
        }
    }
}


impl<T> Drop for TrackedObject<T> {
    fn drop(&mut self) {
        let count_before = self.inner.count.fetch_sub(1, Ordering::SeqCst);
        if count_before == 1 {
            // this was the last reference.
            // Let's remove our object from the census.
            self.census.remove_at(self.inner.idx.load(Ordering::SeqCst));
        }
    }
}

impl<T> Clone for TrackedObject<T> {
    fn clone(&self) -> Self {
        self.inner.count.fetch_add(1, Ordering::SeqCst);
        TrackedObject {
            census: self.census.clone(),
            inner: self.inner.clone(),
        }
    }
}

struct Inner<T> {
    val: T,
    count: AtomicUsize,
    idx: AtomicUsize,
}

/// A tracked object.
pub struct TrackedObject<T> {
    census: Inventory<T>,
    inner: Arc<Inner<T>>,
}

impl<T> TrackedObject<T> {
    fn set_index(&self, pos: usize) {
        self.inner.idx.store(pos, Ordering::SeqCst);
    }


    pub fn map<F>(&self, f: F) -> TrackedObject<T>
        where F: FnOnce(&T)->T {
        let t = f(&*self);
        self.census.track(t)
    }
}

impl<T> Deref for TrackedObject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.val
    }
}

#[cfg(test)]
mod tests {

    use super::Inventory;

    #[test]
    fn test_census_map() {
        let census = Inventory::new();
        let a = census.track(1);
        let _b = a.map(|v| v*7);
        assert_eq!(
            census
                .list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1, 7]
        );
    }


    #[test]
    fn test_census() {
        let census = Inventory::new();
        let _a = census.track(1);
        let _b = census.track(3);
        assert_eq!(
            census
                .list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1, 3]);
    }

    #[test]
    fn test_census_2() {
        let census = Inventory::new();
        {
            let _a = census.track(1);
            let _b = census.track(3);
            // dropping both here
        }
        assert!(census.list().is_empty());
    }

    #[test]
    fn test_census_3() {
        let census = Inventory::new();
        let a = census.track(1);
        let _a2 = a.clone();
        drop(a);
        assert_eq!(
            census.list()
                .into_iter()
                .map(|m| *m)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }
}
