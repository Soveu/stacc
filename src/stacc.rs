use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::{
    atomic::{AtomicIsize, Ordering},
    Arc,
};

/* We need parking_lot's implementation of RwLock, because it guarantees some fairness */
use parking_lot::{Mutex, RwLock};

pub(crate) struct AtomicPop<T> {
    slice: Box<[MaybeUninit<UnsafeCell<T>>]>,
    len: AtomicIsize,
}

unsafe impl<T> Send for AtomicPop<T> {}
unsafe impl<T> Sync for AtomicPop<T> {}

impl<T> AtomicPop<T> {
    pub(crate) fn new(n: usize) -> Self {
        let mut v = Vec::with_capacity(n);
        unsafe { v.set_len(n) };
        let slice = v.into_boxed_slice();
        let len = AtomicIsize::new(0);
        Self { slice, len }
    }

    pub(crate) fn pop(&self) -> Option<T> {
        let len = self.len.fetch_sub(1, Ordering::Acquire);
        if len == 0 {
            self.len.fetch_max(0, Ordering::Release);
        }
        if len <= 0 {
            return None;
        }

        let n = len as usize - 1;
        /* Now only we have access to element at n */
        let item = unsafe {
            let cellref = &*self.slice[n].as_ptr();
            ptr::read(cellref.get())
        };

        return Some(item);
    }
}

pub(crate) struct AtomicPush<T> {
    slice: Box<[MaybeUninit<UnsafeCell<T>>]>,
    len: AtomicIsize,
}

unsafe impl<T> Send for AtomicPush<T> {}
unsafe impl<T> Sync for AtomicPush<T> {}

impl<T> AtomicPush<T> {
    pub(crate) fn new(n: usize) -> Self {
        let mut v = Vec::with_capacity(n);
        unsafe { v.set_len(n) };
        let slice = v.into_boxed_slice();
        let len = AtomicIsize::new(0);
        Self { slice, len }
    }

    pub(crate) fn push(&self, x: T) -> Option<T> {
        /* Allocation can't be larger than isize::MAX anyway */
        let maxlen = self.slice.len() as isize;
        let oldlen = self.len.fetch_add(1, Ordering::Acquire);

        if oldlen == maxlen {
            self.len.fetch_min(maxlen, Ordering::Release);
        }

        if oldlen >= maxlen {
            return Some(x);
        }

        let n = oldlen as usize;
        /* Now we are the only one having access to self.slice[n] */
        unsafe {
            let cellref = &*self.slice[n].as_ptr();
            ptr::write(cellref.get(), x);
        }

        return None;
    }
}

struct StaccInner<T> {
    poppers: RwLock<AtomicPop<T>>,
    pushers: RwLock<AtomicPush<T>>,
    swap_lock: Mutex<()>,
}

impl<T> StaccInner<T> {
    fn new(n: usize) -> Self {
        Self {
            poppers: RwLock::new(AtomicPop::new(n)),
            pushers: RwLock::new(AtomicPush::new(n)),
            swap_lock: Mutex::new(()),
        }
    }

    fn swap_stacks(&self) {
        let swap_lock = self.swap_lock.try_lock();
        if swap_lock.is_none() {
            drop(self.swap_lock.lock());
            return;
        }

        let mut poppers = self.poppers.write();
        let mut pushers = self.pushers.write();

        std::mem::swap(&mut poppers.slice, &mut pushers.slice);
        std::mem::swap(&mut poppers.len, &mut pushers.len);
        drop(swap_lock);
    }

    fn push(&self, x: T) -> Option<T> {
        let lock = self.pushers.read();
        let x = match lock.push(x) {
            None => return None,
            Some(x) => x,
        };
        drop(lock);

        let poppers = self.poppers.read();
        let poppers_len = poppers.len.load(Ordering::Relaxed);
        let poppers_len = if poppers_len < 0 {
            0usize
        } else {
            poppers_len as usize
        };
        let poppers_maxlen = poppers.slice.len();
        drop(poppers);

        if poppers_len != poppers_maxlen {
            self.swap_stacks();
            return self.push(x);
        }

        return Some(x);
    }

    fn pop(&self) -> Option<T> {
        let lock = self.poppers.read();
        if let Some(x) = lock.pop() {
            return Some(x);
        }
        drop(lock);

        let pushers = self.pushers.read();
        let pushers_len = pushers.len.load(Ordering::Relaxed);
        let pushers_len = if pushers_len < 0 {
            0usize
        } else {
            pushers_len as usize
        };
        drop(pushers);

        if pushers_len != 0 {
            self.swap_stacks();
            return self.pop();
        }

        return None;
    }

    fn len(&self) -> usize {
        let len1 = self.pushers.read().len.load(Ordering::Relaxed);
        let len2 = self.poppers.read().len.load(Ordering::Relaxed);

        let len1 = if len1 < 0 { 0usize } else { len1 as usize };
        let len2 = if len2 < 0 { 0usize } else { len2 as usize };

        len1 + len2
    }
}

pub struct Stacc<T> {
    inner: Arc<StaccInner<T>>,
}

impl<T> Stacc<T> {
    pub fn new(n: usize) -> Self {
        let inner = Arc::new(StaccInner::new(n));
        Self { inner }
    }
    pub fn push(&self, x: T) -> Option<T> {
        self.inner.push(x)
    }
    pub fn pop(&self) -> Option<T> {
        self.inner.pop()
    }
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<T> Clone for Stacc<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
