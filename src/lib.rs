//! # Census
//!
//! Census' `Inventory`  makes it possible to track a set of living items of a specific type.
//! Items are automatically removed from the `Inventory<T>` as the living item's are dropped.
//!
//! ```rust
//! use census::{Inventory, TrackedObject};
//!
//! let inventory = Inventory::new();
//!
//! //  Each object tracked needs to be registered explicitely in the Inventory.
//! //  A `TrackedObject<T>` wrapper is then returned.
//! let one = inventory.track("one".to_string());
//! let two = inventory.track("two".to_string());
//!
//! // A snapshot  of the list of living instances can be obtained...
//! // (no guarantee on their order)
//! let living_instances: Vec<TrackedObject<String>> = inventory.list();
//! assert_eq!(living_instances.len(), 2);
//! ```

use std::borrow::Borrow;
use std::{mem, fmt};
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::sync::mpsc;
use std::sync::atomic::Ordering::SeqCst;
use std::fmt::{Formatter, Error};
use std::time::Duration;

struct Stash<T: Send> {
    items: Vec<Arc<Inner<T>>>,
    release_queue_recv: mpsc::Receiver<Arc<Inner<T>>>,
}

impl<T: Send> Stash<T> {
    fn items_copy(&mut self, inventory: &Inventory<T>) -> Vec<TrackedObject<T>> {
        inventory.inner.purge(self);
        self.items
            .iter()
            .flat_map(|inner| {
                make_tracked_object(inner.clone(), inventory, false)
            })
            .collect()
    }
}

impl<T: Send> Stash<T> {
    fn new(release_queue_recv: mpsc::Receiver<Arc<Inner<T>>>) -> Self {
        Stash {
            items: Vec::new(),
            release_queue_recv
        }
    }
}

struct InnerInventory<T: Send> {
    items: Mutex<Stash<T>>,
    condvar: Condvar
}

impl<T: Send> InnerInventory<T> {

    pub(crate) fn purge(&self, stash: &mut Stash<T>) {
        let mut changed = false;
        while let Ok(el) = stash.release_queue_recv.try_recv() {
            self.remove_with_lock(&el,  stash);
            changed = true;
        }
        if changed {
            self.condvar.notify_all();
        }
    }

    fn remove_with_lock(
        &self,
        el: &Inner<T>,
        stash: &mut Stash<T>,
    ) {
        // just pop if this was the last element
        let pos = el.index();
        if pos + 1 == stash.items.len() {
            stash.items.pop();
        } else {
            stash.items.swap_remove(pos);
            stash.items[pos].set_index(pos);
        }
    }
}

enum ChangesIteratorState<'a, T: Send> {
    Started(MutexGuard<'a, Stash<T>>),
    NotStarted,
}

struct ChangesIterator<'a, T: Send> {
    inventory: &'a Inventory<T>,
    state: ChangesIteratorState<'a, T>,
}

impl<'a, T: Send> ChangesIteratorState<'a, T> {
    fn advance(
        self,
        inventory: &'a Inventory<T>
    ) -> (ChangesIteratorState<'a, T>, Vec<TrackedObject<T>>) {
        match self {
            ChangesIteratorState::NotStarted => {
                let mut guard = inventory.inner.items.lock().unwrap();
                let items_copy = guard.items_copy(inventory);
                (ChangesIteratorState::Started(guard), items_copy)
            }
            ChangesIteratorState::Started(mut guard) => {
                guard = {
                    let (guard, _) = inventory.inner.condvar.wait_timeout(guard, Duration::from_millis(100u64)).unwrap();
                    guard
                };
                let items_copy = guard.items_copy(inventory);
                (ChangesIteratorState::Started(guard), items_copy)
            }
        }
    }
}

impl<'a, T: Send> Iterator for ChangesIterator<'a, T> {
    type Item = Vec<TrackedObject<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        let state = mem::replace(&mut self.state, ChangesIteratorState::NotStarted);
        let (new_state, items) = state.advance(self.inventory);
        self.state = new_state;
        Some(items)
    }
}

/// The `Inventory` register and keeps track of all of the objects alive.
pub struct Inventory<T: Send> {
    inner: Arc<InnerInventory<T>>,
    release_queue_send: mpsc::Sender<Arc<Inner<T>>>,
}

impl<T: Send> Default for Inventory<T> {
    fn default() -> Self {
        let (release_queue_send, release_queue_recv) = mpsc::channel();
        let inner = InnerInventory {
            items: Mutex::new(Stash::new(release_queue_recv)),
            condvar: Condvar::default()
        };
        Inventory {
            inner: Arc::new(inner),
            release_queue_send
        }
    }
}

impl<T: Send> Clone for Inventory<T> {
    fn clone(&self) -> Self {
        Inventory {
            inner: self.inner.clone(),
            release_queue_send: self.release_queue_send.clone()
        }
    }
}

impl<T: Send> Inventory<T> {
    /// Creates a new inventory.
    pub fn new() -> Inventory<T> {
        Inventory::default()
    }


    fn try_purge(&self) {
        // this was the last reference.
        // Let's remove our object from the census.
        let try_wlock = self
            .inner
            .items
            .try_lock();
        if let Ok(mut wlock) = try_wlock {
            self.inner.purge(&mut wlock);
        }
        // If the lock is not available, not problem... Someone is probably
        // already just holding the lock.
        //
        // This is fine. The object will simply get cleaned up when this locked is dropped.
    }

    /// Takes a snapshot of the list of tracked object.
    ///
    /// Note that the list is a simple `Vec` of tracked object.
    /// As a result, it is a consistent snapshot of the
    /// list of living instance at the time of the call,
    ///
    /// Obviously, instances may have been created after the call.
    /// They will obviously not appear in the snapshot.
    ///
    /// ```rust
    /// use census::{Inventory, TrackedObject};
    ///
    /// let inventory = Inventory::new();
    ///
    /// let one = inventory.track("one".to_string());
    /// let living_instances: Vec<TrackedObject<String>> = inventory.list();
    /// let two = inventory.track("two".to_string());
    ///
    /// // our snapshot is a bit old.
    /// assert_eq!(living_instances.len(), 1);
    ///
    /// // a fresher snapshot would contain our new element.
    /// assert_eq!(inventory.list().len(), 2);
    /// ```
    ///
    /// Also, the instance in the snapshot itself
    /// are considered "living".
    ///
    /// As a result, as long as a snapshot is not dropped,
    /// all of its instances will be part of the inventory.
    ///
    /// ```rust
    /// # use census::{Inventory, TrackedObject};
    ///
    /// let inventory = Inventory::new();
    ///
    /// let one = inventory.track("one".to_string());
    /// let living_instances: Vec<TrackedObject<String>> = inventory.list();
    ///
    /// // let's drop one here
    /// drop(one);
    ///
    /// // The instance is technically still in the inventory
    /// // as our previous snapshot is extending its life...
    /// assert_eq!(inventory.list().len(), 1);
    ///
    /// // If we drop our previous snapshot however...
    /// drop(living_instances);
    ///
    /// // `one` is really untracked.
    /// assert!(inventory.list().is_empty());
    /// ```
    ///
    pub fn list(&self) -> Vec<TrackedObject<T>> {
        let mut wlock = self.inner.items.lock().expect("Lock poisoned");
        wlock.items_copy(self)
    }

    /// Returns an `Iterator<Item=Vec<TrackedObject<T>>>` that observes
    /// the changes in the inventory.
    ///
    /// # Disclaimer
    ///
    /// This method is very slow, and is meant to use only on small inventories.
    /// Between every call to next, the creation of items in the inventory is blocked,
    /// and is unlocked when `.next()` is called.
    ///
    /// `.next()` then returns when the list of tracked object has changed.
    /// There is no guarantee that all intermediary state are emitted by the iterator.
    pub fn changes_iter<'a>(&'a self) -> impl 'a + Iterator<Item = Vec<TrackedObject<T>>> {
        ChangesIterator {
            inventory: &self,
            state: ChangesIteratorState::NotStarted,
        }
    }

    /// Starts tracking a given `T` object.
    pub fn track(&self, t: T) -> TrackedObject<T> {
        let mut wlock = self
            .inner
            .items
            .lock()
            .expect("Inventory lock poisoned on write");
        self.inner.purge(&mut wlock);
        let idx = wlock.items.len();
        let inner = Arc::new(Inner {
            val: t,
            count: AtomicUsize::new(0),
            idx: AtomicUsize::new(idx),
        });
        let tracked_object = make_tracked_object(inner.clone(), self, true).unwrap();
        wlock.items.push(inner);
        self.inner.condvar.notify_all();
        tracked_object
    }
}

impl<T: Send> Drop for TrackedObject<T> {
    fn drop(&mut self) {
        let count_before = self.inner.count.fetch_sub(1, Ordering::SeqCst);
        if count_before <= 1 {
            // The last remaining object is the one in the invenotory.
            // Let's schedule our object for deletion.
            let _ = self.census.release_queue_send.send(self.inner.clone());
            self.census.try_purge();
        }
    }
}

impl<T: Send> Clone for TrackedObject<T> {
    fn clone(&self) -> Self {
        make_tracked_object(self.inner.clone(), &self.census, false).unwrap()
    }
}

struct Inner<T: Send> {
    val: T,
    count: AtomicUsize, //< always >= and eventually = to the number of instance of TrackedObject with this item
    idx: AtomicUsize,
}

fn make_tracked_object<T: Send>(inner: Arc<Inner<T>>, inventory: &Inventory<T>, force: bool) -> Option<TrackedObject<T>> {
    let count = inner.count.fetch_add(1, Ordering::SeqCst);
    if count == 0 && !force {
        return None
    }
    Some(TrackedObject {
        census: inventory.clone(),
        inner
    })
}


/// Your tracked object.
///
/// A tracked object contains reference counting logic and an
/// `Arc` to your object. It is cloneable but calling clone will
/// not clone your internal object.
///
/// Your object cannot be mutated. You can borrow it using
/// the `Deref` interface.
pub struct TrackedObject<T: Send> {
    census: Inventory<T>,
    inner: Arc<Inner<T>>
}

impl<T: Send + fmt::Debug> fmt::Debug for TrackedObject<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "Tracked({:?}, count={})", self.inner.val, self.inner.count.load(SeqCst))
    }
}

impl<T: Send> Inner<T> {
    fn index(&self) -> usize {
        self.idx.load(Ordering::SeqCst)
    }

    fn set_index(&self, pos: usize) {
        self.idx.store(pos, Ordering::SeqCst);
    }
}


impl<T: Send> TrackedObject<T> {
    /// Creates a new object from an existing one.
    ///
    /// The new object will be registered
    /// in your original object's inventory.
    ///
    /// ```rust
    /// use census::{Inventory, TrackedObject};
    ///
    /// let inventory = Inventory::new();
    ///
    /// let seven = inventory.track(7);
    /// let fourteen = seven.map(|i| i * 2);
    /// assert_eq!(*fourteen, 14);
    ///
    /// let living_instances = inventory.list();
    /// assert_eq!(living_instances.len(), 2);
    /// ```
    pub fn map<F>(&self, f: F) -> TrackedObject<T>
    where
        F: FnOnce(&T) -> T,
    {
        let t = f(&*self);
        self.census.track(t)
    }
}

impl<T: Send> Deref for TrackedObject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.val
    }
}

impl<T: Send> AsRef<T> for TrackedObject<T> {
    fn as_ref(&self) -> &T {
        &self.inner.val
    }
}

impl<T: Send> Borrow<T> for TrackedObject<T> {
    fn borrow(&self) -> &T {
        &self.inner.val
    }
}

#[cfg(test)]
mod tests {

    use super::Inventory;
    use std::thread;

    #[test]
    fn test_census_map() {
        let census = Inventory::new();
        let a = census.track(1);
        let _b = a.map(|v| v * 7);
        assert_eq!(
            census.list().into_iter().map(|m| *m).collect::<Vec<_>>(),
            vec![1, 7]
        );
    }

    #[test]
    fn test_census() {
        let census = Inventory::new();
        let _a = census.track(1);
        let _b = census.track(3);
        assert_eq!(
            census.list().into_iter().map(|m| *m).collect::<Vec<_>>(),
            vec![1, 3]
        );
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
            census.list().into_iter().map(|m| *m).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn test_census_list_extends_life() {
        let census = Inventory::new();
        let a = census.track(1);
        let living = census.list();
        assert_eq!(living.len(), 1);
        drop(a);
        let living_2 = census.list();
        assert_eq!(living_2.len(), 1);
        drop(living_2);
        drop(living);
        assert!(census.list().is_empty());
    }

    #[test]
    fn test_census_race_condition() {
        let census = Inventory::new();
        let census_clone = census.clone();
        thread::spawn(move || {
            for _ in 0..1_000 {
                let _a = census_clone.track(1);
            }
        });
        for _ in 0..10_000 {
            census.list();
        }
    }

    fn test_census_changes_iter_util(el: usize) {
        let census = Inventory::new();
        for i in 0..el {
            let tracked = census.track(i);
            thread::spawn(move || {
                let _tracked = tracked;
            });
        }
        for objs in census.changes_iter() {
            if objs.len() == 0 {
                break;
            }
        }
    }

    #[test]
    fn test_census_changes_iter0() {
        let census = Inventory::new();
        let tracked = census.track(1);
        thread::spawn(move || {
            let _tracked = tracked;
        });
        for objs in census.changes_iter() {
            if objs.len() == 0 {
                break;
            }
        }
    }

    #[test]
    fn test_census_changes_iter_many() {
        for _ in 0..10 {
            for i in 1..20 {
                test_census_changes_iter_util(i);
            }
        }
    }
}
