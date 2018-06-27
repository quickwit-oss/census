use std::sync::{Arc, RwLock};
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Census<T> {
    items: Arc<RwLock<Vec<ManagedObject<T>>>>,
}

impl<T> Clone for Census<T> {
    fn clone(&self) -> Self {
        Census {
            items: self.items.clone()
        }
    }
}

impl<T> Census<T> {

    pub fn new() -> Census<T> {
        Census {
            items: Arc::default()
        }
    }

    pub fn list(&self) -> impl Iterator<Item=ManagedObject<T>> {
        self.items.read()
            .expect("Lock poisoned")
            .clone()
            .into_iter()
    }

    pub fn create(&self, t: T)  -> ManagedObject<T> {
        let self_clone: Census<T> = (*self).clone();
        let mut wlock = self.items.write().expect("Lock poisoned");
        let idx = wlock.len();
        let managed_object = ManagedObject {
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


impl<T> Drop for ManagedObject<T> {
    fn drop(&mut self) {
        let count_before = self.inner.count.fetch_sub(1, Ordering::SeqCst);
        if count_before == 1 {
            // this was the last reference.
            // Let's remove our object from the census.
            self.census.remove_at(self.inner.idx.load(Ordering::SeqCst));
        }
    }
}

impl<T> Clone for ManagedObject<T> {
    fn clone(&self) -> Self {
        self.inner.count.fetch_add(1, Ordering::SeqCst);
        ManagedObject {
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

pub struct ManagedObject<T> {
    census: Census<T>,
    inner: Arc<Inner<T>>,
}

impl<T> ManagedObject<T> {
    fn set_index(&self, pos: usize) {
        self.inner.idx.store(pos, Ordering::SeqCst);
    }
}

impl<T> Deref for ManagedObject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.val
    }
}

#[cfg(test)]
mod tests {

    use super::Census;

    #[test]
    fn test_census() {
        let census = Census::new();
        let a = census.create(1);
        let b = census.create(3);
        {
            let els = census.list().map(|m| *m).collect::<Vec<_>>();
            assert_eq!(els, vec![1, 3]);
        }
        drop(b);
        {
            let els = census.list().map(|m| *m).collect::<Vec<_>>();
            assert_eq!(els, vec![1]);
        }
        let a2 = a.clone();
        drop(a);
        {
            let els = census.list().map(|m| *m).collect::<Vec<_>>();
            assert_eq!(els, vec![1]);
        }
        drop(a2);
        {
            let els = census.list().map(|m| *m).collect::<Vec<_>>();
            assert_eq!(els, vec![   ]);
        }
    }
}
