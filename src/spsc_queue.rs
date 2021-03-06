use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, Ordering};
use std::sync::Arc;

struct QueueInner<T> {
    head: AtomicUsize,
    tail: AtomicUsize,

    /* Size must be power of two */
    data: [UnsafeCell<MaybeUninit<T>>; 256],
}

impl<T> QueueInner<T> {
    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        let cap = self.data.len();
        let mask = cap - 1;

        return tail.wrapping_sub(head) & mask;
    }
}

impl<T> Drop for QueueInner<T> {
    fn drop(&mut self) {
        let head = *self.head.get_mut();
        let mut tail = *self.tail.get_mut();
        let cap = self.data.len();
        let mask = cap - 1;

        while tail != head {
            unsafe {
                drop(ptr::read(self.data[tail].get()).assume_init());
            }
            tail = tail.wrapping_sub(1) & mask;
        }
    }
}

pub struct QueueConsumer<T> {
    inner: Arc<QueueInner<T>>,
}

impl<T> QueueConsumer<T> {
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn other_side_alive(&self) -> bool {
        Arc::strong_count(&self.inner) == 2
    }

    pub fn pop(&mut self) -> Option<T> {
        /* Consumer "owns" head, so relaxed ordering can be used here */
        let head = self.inner.head.load(Ordering::Relaxed);
        let tail = self.inner.tail.load(Ordering::Acquire);

        if head == tail {
            return None;
        }

        let cap = self.inner.data.len();
        let mask = cap - 1;

        let newhead = head.wrapping_add(1) & mask;

        atomic::fence(Ordering::Acquire);
        let item = unsafe { ptr::read(self.inner.data[head].get()).assume_init() };
        atomic::fence(Ordering::Release);
        self.inner.head.store(newhead, Ordering::Release);

        return Some(item);
    }
}

pub struct QueueProducer<T> {
    inner: Arc<QueueInner<T>>,
}

impl<T> QueueProducer<T> {
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn other_side_alive(&self) -> bool {
        Arc::strong_count(&self.inner) == 2
    }

    pub fn push(&mut self, x: T) -> Option<T> {
        /* Producer "owns" tail, so relaxed ordering can be used here */
        let tail = self.inner.tail.load(Ordering::Relaxed);
        let head = self.inner.head.load(Ordering::Acquire);

        let cap = self.inner.data.len();
        let mask = cap - 1;
        let newtail = tail.wrapping_add(1) & mask;

        if newtail == head {
            return Some(x);
        }

        unsafe {
            ptr::write(self.inner.data[tail].get(), MaybeUninit::new(x));
        }

        /* To make sure ptr::write is visible on the other side and it isn't
         * reordered with the inner.tail store */
        atomic::fence(Ordering::AcqRel);
        self.inner.tail.store(newtail, Ordering::Release);

        return None;
    }
}
